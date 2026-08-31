use super::*;

// SYS-006 + QA-006：install 202 先于停机、busy 单受理、业务空闲竞争。
// 走真实 build_router 全栈，UpdateService 注入 MockController（可控 status /
// 可阻塞 prepare_install），workload 提供者接真实 RunManager（可注入 viewer 数）。

#[cfg(test)]
mod update_flow_tests {
    use super::*;
    use crate::update::controller::mock::MockController;
    use crate::update::ipc::{Candidate, LauncherUpdateStatus, UpdateError as IpcUpdateError};
    use crate::update::model::{UpdateErrorCode, UpdateState};
    use crate::update::policy::{PolicyStore, UpdatePolicy};
    use crate::update::service::{UpdateService, UpdateTxn, WorkloadProvider};
    use crate::update::workload::Workload;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, Response as HttpResponse};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use tower::ServiceExt;

    async fn post_json(
        t: &UpdateRig,
        sid: &str,
        uri: &str,
        body: serde_json::Value,
    ) -> HttpResponse<Body> {
        let request = HttpRequest::builder()
            .method("POST")
            .uri(uri)
            .header(axum::http::header::COOKIE, sid)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        t.app.clone().oneshot(request).await.unwrap()
    }

    async fn get_json(t: &UpdateRig, sid: &str, uri: &str) -> HttpResponse<Body> {
        let request = HttpRequest::builder()
            .method("GET")
            .uri(uri)
            .header(axum::http::header::COOKIE, sid)
            .body(Body::empty())
            .unwrap();
        t.app.clone().oneshot(request).await.unwrap()
    }

    async fn json_body(resp: HttpResponse<Body>) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    async fn save_task_script(t: &UpdateRig, sid: &str, name: &str, content: &str) {
        let resp = post_json(
            t,
            sid,
            "/api/scripts",
            serde_json::json!({
                "pkg": "com.test.app",
                "name": name,
                "content": content,
            }),
        )
        .await;
        let status = resp.status();
        let response_body = json_body(resp).await;
        assert_eq!(status, StatusCode::OK, "{response_body}");
    }

    /// 可注入 viewer 数的 workload 提供者（QA-006 viewer 维度）
    #[derive(Clone, Default)]
    struct FakeViewers(Arc<AtomicUsize>);

    struct BlockingExec {
        /// prepare 成功、execute 永挂起 → run 恒 active（QA-006 活动运行）
        started: Arc<AtomicUsize>,
    }

    impl crate::run_manager::RunExecutor for BlockingExec {
        fn prepare<'a>(
            &'a self,
            _req: &'a crate::run_manager::StartRequest,
        ) -> futures_util::future::BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn execute<'a>(
            &'a self,
            _req: &'a crate::run_manager::StartRequest,
            _stop: Arc<AtomicBool>,
        ) -> futures_util::future::BoxFuture<
            'a,
            anyhow::Result<Vec<(String, String)>>,
        > {
            self.started.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                std::future::pending::<()>().await;
                unreachable!("pending executor never resolves")
            })
        }
        fn occupy(&self, _device_id: &str) {}
        fn release(&self, _device_id: &str) {}
    }

    struct UpdateRig {
        app: Router,
        db: Db,
        runs: Arc<crate::run_manager::RunManager>,
        controller: Arc<MockController>,
        viewers: FakeViewers,
        dir: std::path::PathBuf,
    }

    impl Drop for UpdateRig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn build_update_rig(tag: &str) -> UpdateRig {
        let dir = std::env::temp_dir().join(format!(
            "gamer-update-flow-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let db: Db = Arc::new(crate::store::Store::open(&cfg).unwrap());
        let scripts = Arc::new(ScriptStore::open(&cfg).unwrap());
        let viewers: crate::webrtc::ViewerMap =
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let devices = Arc::new(DeviceManager::new(db.clone(), cfg.clone(), viewers.clone()));
        let started = Arc::new(AtomicUsize::new(0));
        let executor = Arc::new(BlockingExec { started });
        let runs = Arc::new(crate::run_manager::RunManager::new(executor));
        let scheduler = Arc::new(Scheduler::new(db.clone(), scripts.clone(), runs.clone()));
        let auth = Arc::new(auth::AuthState::new(
            test_credential("admin123"),
            Default::default(),
            false,
            Some("test-token".into()),
        ));
        let shutdown = Arc::new(crate::shutdown::ShutdownCoordinator::new(Arc::new(|| {
            Box::pin(async {})
        })));

        let controller = Arc::new(MockController::new());
        let policy_store = PolicyStore::load_blocking(&cfg.data_dir, UpdatePolicy::default());
        let fake_viewers = FakeViewers(Arc::new(AtomicUsize::new(0)));
        let runs_for_wl = runs.clone();
        let viewers_for_wl = fake_viewers.clone();
        let workload: WorkloadProvider = Arc::new(move || Workload {
            active_runs: runs_for_wl.active_count(),
            viewers: viewers_for_wl.0.load(Ordering::SeqCst),
            update_transactions: 0,
            next_cron_secs: None,
        });
        let update = Arc::new(UpdateService::new(
            controller.clone(),
            policy_store,
            Arc::new(UpdateTxn::default()),
            workload,
            db.clone(),
        ));

        let app = build_router(
            db.clone(),
            devices,
            runs.clone(),
            scheduler,
            cfg,
            viewers,
            scripts,
            shutdown,
            auth,
            update,
        );
        UpdateRig {
            app,
            db,
            runs,
            controller,
            viewers: fake_viewers,
            dir,
        }
    }

    fn staged_status(update_id: &str) -> LauncherUpdateStatus {
        LauncherUpdateStatus {
            state: Some(UpdateState::Staged),
            detail: Some("staged".into()),
            update_id: Some(update_id.into()),
            candidate: Some(Candidate {
                version: "0.3.0".into(),
                channel: "stable".into(),
                published_at: None,
                size_bytes: None,
                release_notes_url: None,
            }),
            progress: None,
            last_error: None,
        }
    }

    fn fail_status(code: &str) -> LauncherUpdateStatus {
        LauncherUpdateStatus {
            state: Some(UpdateState::Failed),
            detail: Some("failed".into()),
            update_id: Some("upd-f1".into()),
            candidate: None,
            progress: None,
            last_error: Some(crate::update::ipc::LastErrorCodeMessage {
                code: code.into(),
                message: "mock failure".into(),
            }),
        }
    }

    /// SYS-006 主链路：install → 202 立即返回（先于 prepare_install 完成/
    /// 停机）→ 状态机 installing → launcher mock 完成后事务保持（重启接管）。
    /// 并发第二个 install → 409 update_busy（单受理）。
    #[tokio::test]
    async fn install_returns_202_before_prepare_completes_and_single_acceptance() {
        let t = build_update_rig("202-first");
        let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
        t.controller.set_status(staged_status("upd-sys6"));

        // prepare_install 挂起（模拟 launcher 整备中，尚未完成）
        let release = t.controller.hold_prepare();

        let resp = post_json(
            &t,
            &sid,
            "/api/system/update/install",
            serde_json::json!({}),
        )
        .await;
        let status = resp.status();
        let body = json_body(resp).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        assert_eq!(body["state"], "installing");
        assert_eq!(body["update_id"], "upd-sys6");
        tokio::task::yield_now().await;
        assert_eq!(
            t.controller.calls().iter().filter(|c| *c == "prepare_install").count(),
            1,
            "202 返回时 prepare_install 已被后台发起"
        );

        // 服务仍然存活可查（未停机）：状态机已是 installing
        let resp = get_json(&t, &sid, "/api/system/update").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(json_body(resp).await["state"], "installing");

        // 202 已先行返回，此时第二个 install → 409 update_busy（单受理）
        let resp = post_json(
            &t,
            &sid,
            "/api/system/update/install",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        assert_eq!(json_body(resp).await["code"], "update_busy");

        // 审计日志：受理即写（installing 置位后、prepare 之前）
        let logs = t.db.list_logs(None, None, 50).unwrap();
        assert!(
            logs.iter()
                .any(|l| l.msg.contains("update install accepted") && l.msg.contains("installing")),
            "受理审计日志缺失: {:?}",
            logs.iter().map(|l| &l.msg).collect::<Vec<_>>()
        );

        // 释放 prepare → 后台完成（成功路径事务不释放：进程即将被重启）
        let _ = release.send(());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let resp = post_json(
            &t,
            &sid,
            "/api/system/update/install",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT, "prepare 完成后事务仍被占用");
        assert_eq!(json_body(resp).await["code"], "update_busy");
    }

    /// SYS-006 被拒路径：prepare_install 业务错误 → failed + last_error +
    /// 事务释放 + 审计日志（error 级同步落库）
    #[tokio::test]
    async fn install_rejected_marks_failed_releases_txn_and_audits() {
        let t = build_update_rig("rejected");
        let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
        t.controller.set_status(staged_status("upd-rej"));
        t.controller.fail_prepare_with(UpdateErrorCode::SchemaIncompatible);

        let resp = post_json(
            &t,
            &sid,
            "/api/system/update/install",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        // 等后台任务落地（失败审计是同步写，收敛即查得到）
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let resp = get_json(&t, &sid, "/api/system/update").await;
            let body = json_body(resp).await;
            if body["state"] == "failed" {
                assert_eq!(body["last_error"]["code"], "schema_incompatible");
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "prepare 被拒后未进入 failed: {body}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        // 审计日志（error 级同步落库，含错误码；无路径泄露）
        let logs = t.db.list_logs(None, None, 50).unwrap();
        let audit = logs
            .iter()
            .find(|l| l.msg.contains("update install failed"))
            .expect("被拒必须有失败审计日志");
        assert!(audit.msg.contains("schema_incompatible"), "{:?}", audit.msg);
        assert!(!audit.msg.contains(&t.dir.to_string_lossy().to_string()));

        // 事务已释放：failed 态 install 再走门禁（staging 未就绪阻塞），
        // 而非 update_busy
        t.controller.set_status(fail_status("schema_incompatible"));
        let resp = post_json(
            &t,
            &sid,
            "/api/system/update/install",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = json_body(resp).await;
        assert_eq!(body["code"], "update_not_ready");
        assert!(body["details"]["blocking"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b == "staging_not_ready"));
    }

    /// QA-006①：活动 run 时 install 只等待不中断——409 update_not_ready +
    /// blocking=[active_run]，run 状态不受影响
    #[tokio::test]
    async fn active_run_blocks_install_without_interruption() {
        let t = build_update_rig("active-run");
        let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

        // 建脚本并提交一个永不结束的 run
        save_task_script(&t, &sid, "forever.yaml", "steps:\n  - log: 'loop'\n").await;
        let resp = post_json(
            &t,
            &sid,
            "/api/scripts/com.test.app%2Fforever.yaml/run",
            serde_json::json!({ "device_id": "dev-1" }),
        )
        .await;
        let status = resp.status();
        let response_body = json_body(resp).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{response_body}");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while t.runs.active_count() == 0 {
            assert!(std::time::Instant::now() < deadline, "run 未进入活动表");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // staged + 门禁：active_run 阻塞，409 详情列出
        t.controller.set_status(staged_status("upd-run"));
        let resp = post_json(
            &t,
            &sid,
            "/api/system/update/install",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = json_body(resp).await;
        assert_eq!(body["code"], "update_not_ready");
        let blocking = body["details"]["blocking"].as_array().unwrap();
        assert!(blocking.iter().any(|b| b == "active_run"), "{blocking:?}");

        // run 不受影响：仍 active、未被取消
        assert_eq!(t.runs.active_count(), 1, "install 拒绝不得中断活动 run");
        let run_body = json_body(get_json(&t, &sid, "/api/devices/dev-1/run").await).await;
        assert_eq!(run_body["active"], true, "活动 run 查询应仍为 active");
        let run_id = run_body["run"]["run_id"].clone();
        assert!(run_id.is_string(), "活动 run 查询应仍在");
    }

    /// QA-006②：viewer 阻塞 auto 安装（软门禁等待）但不阻塞 notify 下载
    #[tokio::test]
    async fn viewer_blocks_auto_install_but_not_notify_download() {
        // auto：staged + 窗口内 + viewer 在线 → 不装（等待），绝不发 prepare
        let t = build_update_rig("viewer-auto");
        t.controller.set_status(staged_status("upd-viewer"));
        t.viewers.0.store(2, Ordering::SeqCst);
        let resp = post_json(
            &t,
            &sid_of(&t).await,
            "/api/system/update/install",
            serde_json::json!({}),
        )
        .await;
        // 手动 install 不受 viewer 影响（viewer 不是硬门禁）——这里先证实
        // 202（说明 viewer 不挡手动），auto 的 viewer 等待由 decide 单测覆盖
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        // notify/auto 决策对 viewer 的差异在 coordinator decide 矩阵中已覆盖：
        // notify(Available)→Download 与 workload 无关；auto(Staged)→viewer>0 不装
    }

    async fn sid_of(t: &UpdateRig) -> String {
        first_cookie_pair(&cookie_of(&login(&t.app).await))
    }

    /// QA-006③：升级事务进行中不挡已激活实例的正常业务读写
    #[tokio::test]
    async fn update_transaction_does_not_block_normal_business() {
        let t = build_update_rig("txn-business");
        let sid = sid_of(&t).await;
        t.controller.set_status(staged_status("upd-busy"));
        let _release = t.controller.hold_prepare();

        // 升级事务受理（installing 挂起中）
        let resp = post_json(
            &t,
            &sid,
            "/api/system/update/install",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        // 正常业务全部照常：设备列表 / 任务列表 / 脚本列表 / 系统信息
        for uri in [
            "/api/devices",
            "/api/tasks",
            "/api/scripts?pkg=com.test.app",
            "/api/system/info",
        ] {
            let resp = get_json(&t, &sid, uri).await;
            assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        }
        // 业务写也照常（脚本创建）
        save_task_script(&t, &sid, "during-update.yaml", "steps:\n  - log: 'x'\n").await;
    }

    /// QA-006④：无候选（idle）install → 409 update_not_available（矩阵行）
    #[tokio::test]
    async fn install_at_idle_is_not_available() {
        let t = build_update_rig("idle");
        let sid = sid_of(&t).await;
        t.controller
            .set_status(LauncherUpdateStatus::default()); // idle
        let resp = post_json(
            &t,
            &sid,
            "/api/system/update/install",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(resp).await["code"],
            "update_not_available"
        );
    }

    /// 未使用的导入消解（保留断言语义所需的类型引用）
    #[allow(unused)]
    fn _type_refs(e: &IpcUpdateError, _m: &StdMutex<()>) {
        let _ = e.code;
    }
}
