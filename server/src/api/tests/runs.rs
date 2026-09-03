use super::*;

#[tokio::test]
async fn removed_script_stop_and_status_routes_return_not_found() {
    let t = build_app(
        "removed-run-routes",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    for (method, uri) in [
        ("POST", "/api/scripts/missing/stop"),
        ("GET", "/api/scripts/missing/status"),
    ] {
        let resp = send(
            &t.app,
            req(
                method,
                uri,
                None,
                &[(header::COOKIE.to_string(), sid.clone())],
                None,
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{method} {uri}");
    }
}

// ---------- 统一 RunTarget：函数测试运行端点（POST /api/functions/:id/run）----------

/// 挂起到取消的假执行器（真实 RunManager 语义下测 router 行为）。
struct HangExecutor {
    release: Arc<tokio::sync::Notify>,
}

impl HangExecutor {
    fn new() -> (Self, Arc<tokio::sync::Notify>) {
        let release = Arc::new(tokio::sync::Notify::new());
        (
            Self {
                release: release.clone(),
            },
            release,
        )
    }
}

impl crate::run_manager::RunExecutor for HangExecutor {
    fn prepare<'a>(
        &'a self,
        _: &'a crate::core::RunContext,
        _: &'a crate::core::RunRequest,
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn execute<'a>(
        &'a self,
        _: &'a crate::core::RunContext,
        _: &'a crate::core::RunRequest,
        _realtime_logs: bool,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
        Box::pin(async move {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            self.release.notified().await;
            Ok(vec![("info".into(), "stopped".into())])
        })
    }
    fn acquire(
        &self,
        _: &crate::core::RunContext,
    ) -> anyhow::Result<Box<dyn crate::core::ActivityLease>> {
        Ok(Box::new(crate::core::NoopLease))
    }
}

#[tokio::test]
async fn function_run_endpoint_conflict_args_and_cancel() {
    let (executor, release) = HangExecutor::new();
    let executor = Arc::new(executor);
    let t = build_app_with_executor(
        "fnrun",
        test_credential("admin123"),
        Default::default(),
        executor,
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // 建函数库（带参数声明）
    let body = serde_json::json!({
        "pkg": "com.test.app",
        "name": "common",
        "content": "login:
  params:
    - 'text:who:称呼:\"玩家\"'
    - 'bool:fast:快速:false'
  steps:
    - log: $who
",
    });
    let resp = post_json(&t, &sid, "/api/functions", body).await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);

    // 未知函数文件 → 404
    let resp = post_json(
        &t,
        &sid,
        "/api/functions/com.test.app%2Fnope.yaml/run",
        serde_json::json!({"device_id": "d1"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // args 类型不符 → 400 + 结构化诊断
    let resp = post_json(
        &t,
        &sid,
        "/api/functions/com.test.app%2Fcommon.yaml/run",
        serde_json::json!({"device_id": "d1", "args": {"fast": "不是布尔"}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let j = json_body(resp).await;
    assert_eq!(j["error"], "invalid_args");
    assert!(j["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["code"].as_str().unwrap().starts_with("param.args.")));

    // 正常提交：固定 202 + {run_id,state,resolved_args}（默认值合并视图）
    let resp = post_json(
        &t,
        &sid,
        "/api/functions/com.test.app%2Fcommon.yaml/run",
        serde_json::json!({"device_id": "d1", "function": "login", "args": {"who": "路由"}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let j = json_body(resp).await;
    assert!(j.get("run_id").and_then(|v| v.as_str()).is_some());
    assert_eq!(j["state"], "starting");
    assert!(j.get("resolved_args").is_some());
    assert_eq!(j["resolved_args"]["who"], "路由");
    assert_eq!(j["resolved_args"]["fast"], false);
    let run_id = j["run_id"].as_str().unwrap().to_string();

    // 设备活动查询：固定使用嵌套 {active:true,run:<RunRecord>}。
    let resp = get_json(&t, &sid, "/api/devices/d1/run").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_body(resp).await;
    assert_eq!(j["active"], true);
    assert_eq!(j["run"]["run_id"], run_id);
    assert!(j["run"]["state"].is_string());
    assert!(j.get("run_id").is_none(), "RunRecord must remain nested");

    // GET /api/runs/:run_id 只返回该 run_id 对应的完整单次运行记录。
    let mut running = false;
    for _ in 0..200 {
        let resp = get_json(&t, &sid, &format!("/api/runs/{run_id}")).await;
        let status = json_body(resp).await;
        if status["state"] == "running" {
            running = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        running,
        "run must reach running before cancellation assertions"
    );

    // 设备互斥：同设备第二个函数运行 → 409，busy 摘要携带展示标签
    let resp = post_json(
        &t,
        &sid,
        "/api/functions/com.test.app%2Fcommon.yaml/run",
        serde_json::json!({"device_id": "d1"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let j = json_body(resp).await;
    assert_eq!(j["error"], "device_busy");
    assert_eq!(j["script_id"], "com.test.app/common.yaml#login");

    // 取消 → 202；终态 cancelled 可查询；活动期重复取消保持 202 幂等。
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/runs/{run_id}/cancel"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/runs/{run_id}/cancel"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    release.notify_one();
    let mut cancelled = false;
    for _ in 0..200 {
        let resp = get_json(&t, &sid, &format!("/api/runs/{run_id}")).await;
        let j = json_body(resp).await;
        if j["state"] == "cancelled" {
            cancelled = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(cancelled, "run must reach cancelled");

    // 终态取消按当前契约拒绝，并明确返回终态；设备 active 查询回到唯一空形状。
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/runs/{run_id}/cancel"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let j = json_body(resp).await;
    assert_eq!(j["error"], "already_finished");
    assert_eq!(j["state"], "cancelled");
    let resp = get_json(&t, &sid, "/api/devices/d1/run").await;
    assert_eq!(json_body(resp).await, serde_json::json!({"active": false}));

    // 未知 run_id 只能得到单次运行资源的 404。
    let resp = post_json(
        &t,
        &sid,
        "/api/runs/not-a-run/cancel",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(resp).await["error"], "run_not_found");

    // 设备恢复：取消后可再次提交（脚本入口同样带 args）
    let body = serde_json::json!({
        "pkg": "com.test.app",
        "name": "runme.yaml",
        "content": "params:
  - 'text:msg:消息:\"默认\"'
steps:
  - log: $msg
",
    });
    let resp = post_json(&t, &sid, "/api/scripts", body).await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    let resp = post_json(
        &t,
        &sid,
        "/api/scripts/com.test.app%2Frunme.yaml/run",
        serde_json::json!({"device_id": "d1", "args": {"msg": "脚本实参"}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let j = json_body(resp).await;
    assert_eq!(j["resolved_args"]["msg"], "脚本实参");
    let run_id = j["run_id"].as_str().unwrap().to_string();
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/runs/{run_id}/cancel"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    release.notify_one();
    for _ in 0..200 {
        let resp = get_json(&t, &sid, &format!("/api/runs/{run_id}")).await;
        let j = json_body(resp).await;
        if j["state"] == "cancelled" {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("second run must reach cancelled");
}
