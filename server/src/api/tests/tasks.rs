use super::*;

// ---------- P11.1：统一 /api/tasks（ADR-12 模型）HTTP 契约 ----------

const YAML_RUNNER: &str = "gamer.yaml";

fn task_body(name: &str, runner_id: &str, entrypoint: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "app": {"device_id": "d1", "android_package": "com.example.game", "content_package": "com.example.game"},
        "runner": {"runner_id": runner_id, "entrypoint": entrypoint, "payload": {"args": {}}},
        "schedule": {"provider_id": "cron", "config": {"expression": "0 8 * * *"}},
        "enabled": true
    })
}

/// CRUD 全链路 + enable/disable 生命周期：嵌套 runner/schedule 形状回读一致，
/// enable/disable 走显式状态迁移（Active ↔ Suspended+"disabled"）。
#[tokio::test]
async fn unified_task_crud_and_enable_disable_lifecycle() {
    let t = build_app(
        "task-crud",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // POST 创建 → 201 + 嵌套形状
    let resp = post_json(&t, &sid, "/api/tasks", task_body("Daily", YAML_RUNNER, "com.example.game/daily.yaml")).await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);
    let created = json_body(resp).await;
    let task_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["runner"]["runner_id"], YAML_RUNNER);
    assert_eq!(created["runner"]["entrypoint"], "com.example.game/daily.yaml");
    assert_eq!(created["schedule"]["provider_id"], "cron");
    assert_eq!(created["schedule"]["config"]["expression"], "0 8 * * *");
    assert_eq!(created["state"], "active");
    assert_eq!(created["app"]["android_package"], "com.example.game");
    // 旧平铺字段（script_id/cron）不得出现在响应里
    assert!(created.get("script_id").is_none());
    assert!(created.get("cron").is_none());

    // GET 列表 / 详情
    let resp = get_json(&t, &sid, "/api/tasks").await;
    let list = json_body(resp).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let resp = get_json(&t, &sid, &format!("/api/tasks/{task_id}")).await;
    assert_eq!(json_body(resp).await["id"], task_id);

    // PUT 更新（路径 id 与 body id 一致性校验）
    let mut updated = task_body("Daily-renamed", YAML_RUNNER, "com.example.game/daily.yaml");
    updated["id"] = serde_json::json!(task_id);
    updated["schedule"]["config"]["expression"] = serde_json::json!("*/10 * * * *");
    let resp = send(
        &t.app,
        req(
            "PUT",
            &format!("/api/tasks/{task_id}"),
            None,
            &json_headers(sid.to_string()),
            Some(updated.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    assert_eq!(json_body(resp).await["name"], "Daily-renamed");

    // disable → Suspended + reason=disabled；enable → Active
    let resp = post_json(&t, &sid, &format!("/api/tasks/{task_id}/disable"), serde_json::json!({})).await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    let disabled = json_body(resp).await;
    assert_eq!(disabled["state"], "suspended");
    assert_eq!(disabled["enabled"], false);
    assert_eq!(disabled["suspend_reason"], "disabled");
    let resp = post_json(&t, &sid, &format!("/api/tasks/{task_id}/enable"), serde_json::json!({})).await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    let enabled = json_body(resp).await;
    assert_eq!(enabled["state"], "active");
    assert_eq!(enabled["enabled"], true);
    assert!(!enabled["next_wakeup"].is_null(), "enable 必须重算唤醒游标");

    // DELETE → 404
    let resp = send(
        &t.app,
        req(
            "DELETE",
            &format!("/api/tasks/{task_id}"),
            None,
            &json_headers(sid.to_string()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = get_json(&t, &sid, &format!("/api/tasks/{task_id}")).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// ADR-12 验收：可以保存未知 runner_id / 未注册 schedule provider（任务先存，
/// 依赖后装）。触发时才失败（dependency_missing），保存边界不得拒绝。
#[tokio::test]
async fn task_can_be_saved_with_unknown_runner_and_provider() {
    let t = build_app(
        "task-unknown-runner",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let body = task_body("Future", "future.runner", "daily");
    let resp = post_json(&t, &sid, "/api/tasks", body).await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);
    let task_id = json_body(resp).await["id"].as_str().unwrap().to_string();
    let resp = get_json(&t, &sid, &format!("/api/tasks/{task_id}")).await;
    assert_eq!(json_body(resp).await["runner"]["runner_id"], "future.runner");

    // 未注册 provider 同样放行保存
    let mut body = task_body("FutureSchedule", YAML_RUNNER, "daily");
    body["schedule"] = serde_json::json!({"provider_id": "thirdparty.calendar", "config": {}});
    let resp = post_json(&t, &sid, "/api/tasks", body).await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);

    // 已注册 provider 必须接受 config（cron 表达式非法 → 400）
    let mut body = task_body("BadCron", YAML_RUNNER, "daily");
    body["schedule"]["config"]["expression"] = serde_json::json!("not a cron");
    let resp = post_json(&t, &sid, "/api/tasks", body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{:?}", json_body(resp).await);
}

/// ADR-12 验收：runner 缺失时任务进入 dependency_missing 状态且**不删除**；
/// 立即运行返回 424 dependency_unavailable。
#[tokio::test]
async fn missing_runner_marks_task_dependency_missing_and_keeps_it() {
    let t = build_app(
        "task-dependency-missing",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let resp = post_json(
        &t,
        &sid,
        "/api/tasks",
        task_body("Ghost", "missing.runner", "daily"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let task_id = json_body(resp).await["id"].as_str().unwrap().to_string();

    // 立即运行：424 + dependency_unavailable
    let resp = post_json(&t, &sid, &format!("/api/tasks/{task_id}/run"), serde_json::json!({})).await;
    assert_eq!(resp.status(), StatusCode::FAILED_DEPENDENCY, "{:?}", json_body(resp).await);
    let err = json_body(resp).await;
    assert_eq!(err["code"], "dependency_unavailable");
    assert_eq!(err["runner_id"], "missing.runner");

    // 任务保留，状态 = dependency_missing，reason 记 missing_dependency=<runner_id>
    let resp = get_json(&t, &sid, &format!("/api/tasks/{task_id}")).await;
    let task = json_body(resp).await;
    assert_eq!(task["state"], "dependency_missing");
    assert_eq!(task["suspend_reason"], "missing_dependency=missing.runner");
    assert!(task["next_wakeup"].is_null(), "依赖缺失任务必须休眠");

    // 显式 enable/resume 仍可恢复（恢复语义 Wave2 收口，本期不删除任务即可）
    let resp = post_json(&t, &sid, &format!("/api/tasks/{task_id}/enable"), serde_json::json!({})).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["state"], "active");
}

/// presets 与任务生命周期独立（预设删除不级联任务），使用新 Task Schema
/// （嵌套 runner + {provider_id, config} schedule）。
#[tokio::test]
async fn task_presets_use_new_schema_and_instantiate_independently() {
    let t = build_app(
        "task-presets-v2",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let schedule = serde_json::json!({
        "provider_id": "cron",
        "config": {"expression": "*/5 * * * *"}
    });
    let resp = post_json(
        &t,
        &sid,
        "/api/task-presets",
        serde_json::json!({
            "id": "preset-daily",
            "app_package": "official.xxx",
            "name": "Daily preset",
            "runner": {"runner_id": "missing.runner", "entrypoint": "daily", "payload": {"mode": "safe"}},
            "schedule": schedule
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);
    let preset = json_body(resp).await;
    assert_eq!(preset["runner"]["runner_id"], "missing.runner");
    assert_eq!(preset["schedule"]["provider_id"], "cron");

    let resp = send(
        &t.app,
        req(
            "GET",
            "/api/task-presets?app_package=official.xxx",
            None,
            &json_headers(sid.to_string()),
            None,
        ),
    )
    .await;
    assert_eq!(json_body(resp).await.as_array().unwrap().len(), 1);

    let resp = post_json(
        &t,
        &sid,
        "/api/task-presets/preset-daily/instantiate",
        serde_json::json!({
            "app": {
                "device_id": "d1",
                "android_package": "com.example.game",
                "content_package": "official.xxx"
            }
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);
    let task = json_body(resp).await;
    let task_id = task["id"].as_str().unwrap().to_string();
    assert_eq!(task["app"]["content_package"], "official.xxx");
    assert_eq!(task["preset_id"], "preset-daily");
    assert_eq!(task["state"], "active");
    assert_eq!(task["schedule"], schedule);

    // Missing runner：立即运行 424，任务保留为 dependency_missing
    let resp = post_json(&t, &sid, &format!("/api/tasks/{task_id}/run"), serde_json::json!({})).await;
    assert_eq!(resp.status(), StatusCode::FAILED_DEPENDENCY);
    let resp = get_json(&t, &sid, &format!("/api/tasks/{task_id}")).await;
    let task = json_body(resp).await;
    assert_eq!(task["state"], "dependency_missing");
    assert_eq!(task["suspend_reason"], "missing_dependency=missing.runner");

    // Suspend/resume 显式且保留 schedule
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/tasks/{task_id}/suspend"),
        serde_json::json!({"reason": "maintenance"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["state"], "suspended");
    let resp = post_json(&t, &sid, &format!("/api/tasks/{task_id}/resume"), serde_json::json!({})).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resumed = json_body(resp).await;
    assert_eq!(resumed["state"], "active");
    assert_eq!(resumed["schedule"], schedule);

    // Removing a preset never removes the generated user-owned schedule.
    let resp = send(
        &t.app,
        req(
            "DELETE",
            "/api/task-presets/preset-daily",
            None,
            &json_headers(sid.to_string()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = get_json(&t, &sid, &format!("/api/tasks/{task_id}")).await;
    assert_eq!(json_body(resp).await["preset_id"], "preset-daily");
}

/// UI 支撑只读端点：GET /api/runners、GET /api/schedule-providers。
/// P11.2 裸 Core 语义（ADR-13）：测试组合根不接扩展 registrar，没有任何
/// runner 注册——/api/runners 为空数组；gamer.yaml 任务仍可保存，但立即
/// 运行会因 runner 缺失进入 dependency_missing。cron provider 仍内置注册。
#[tokio::test]
async fn runner_and_schedule_provider_lists_are_exposed() {
    let t = build_app(
        "task-registry-lists",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    let resp = get_json(&t, &sid, "/api/runners").await;
    let runners = json_body(resp).await;
    // 测试装配与生产组合根等价：gamer.yaml 扩展 Running 期间其 runner 在册；
    // 裸 Core（无扩展 start）为空的语义由 scheduler 单测锁定
    assert_eq!(runners.as_array().unwrap().len(), 1, "装配含 gamer.yaml runner");
    assert_eq!(runners[0]["runner_id"], "gamer.yaml");
    assert_eq!(runners[0]["owner_extension_id"], "gamer.yaml");

    let resp = get_json(&t, &sid, "/api/schedule-providers").await;
    let providers = json_body(resp).await;
    assert_eq!(providers.as_array().unwrap().len(), 1);
    assert_eq!(providers[0]["provider_id"], "cron");

    // gamer.yaml 任务可保存（runner 未注册不阻止保存），立即运行 → 显式
    // 依赖缺失（任务保留），响应里带 runner_id 诊断。
    let resp = post_json(
        &t,
        &sid,
        "/api/tasks",
        task_body("BareCore", YAML_RUNNER, "com.example.game/daily.yaml"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let task_id = json_body(resp).await["id"].as_str().unwrap().to_string();
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/tasks/{task_id}/run"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FAILED_DEPENDENCY);
    assert_eq!(json_body(resp).await["runner_id"], YAML_RUNNER);
    let task = json_body(get_json(&t, &sid, &format!("/api/tasks/{task_id}")).await).await;
    assert_eq!(task["state"], "dependency_missing");
    // runner 已注册但入口脚本不存在：同口径进入依赖缺失，reason 为脚本不存在
    assert_eq!(task["suspend_reason"], "脚本不存在");
}

// ---------- P12.11（计划 §16.7）：Task params 全链 ----------
//
// Program.params → TaskBoard 保存（runner.payload.args）→ /api/tasks/:id/run
// → gamer.yaml runner 门禁重绑 → 执行器收到绑定后的全量类型化覆盖。
// 手动路径（POST /api/runs）的参数桥已有 entrypoint_schema 验收；本测试补
// 「保存任务 → 运行 → payload 生效」的 api 级证据与非法 payload 的 400 门禁。

/// 捕获 RunRequest payload 的执行器：prepare 恒成功，execute 记录 payload 后
/// 立即成功结束（本测试只验证任务参数绑定，不执行真实脚本）。
struct PayloadCaptureExecutor {
    payloads: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
}

impl crate::run_manager::RunExecutor for PayloadCaptureExecutor {
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
        request: &'a crate::core::RunRequest,
        _realtime_logs: bool,
        _stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
        let payloads = self.payloads.clone();
        let payload = request.payload.as_value().clone();
        Box::pin(async move {
            payloads.lock().unwrap().push(payload);
            Ok(vec![("info".into(), "captured".into())])
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
async fn task_run_binds_saved_payload_args_through_yaml_runner() {
    let payloads = Arc::new(std::sync::Mutex::new(Vec::new()));
    let t = build_app_with_executor(
        "task-run-params",
        test_credential("admin123"),
        Default::default(),
        Arc::new(PayloadCaptureExecutor {
            payloads: payloads.clone(),
        }),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // 1. 保存带参数声明的 v3 脚本（TaskBoard 参数表单的数据源 = entrypoint schema）
    let resp = post_json(
        &t,
        &sid,
        "/api/apps/com.example.game/resources/scripts",
        serde_json::json!({
            "name": "daily",
            "content": "version: 3\nparams:\n  - 'text:msg:消息:\"默认\"'\n  - 'int:count:次数:3'\nsteps:\n  - log: $msg\n",
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);

    // 2. TaskBoard 保存任务：payload.args 携带用户填写的稀疏实参
    let resp = post_json(
        &t,
        &sid,
        "/api/tasks",
        serde_json::json!({
            "name": "Params Daily",
            "app": {"device_id": "d1", "android_package": "com.example.game", "content_package": "com.example.game"},
            "runner": {
                "runner_id": YAML_RUNNER,
                "entrypoint": "com.example.game/daily.yaml",
                "payload": {"args": {"msg": "任务实参"}}
            },
            "schedule": {"provider_id": "cron", "config": {"expression": "0 8 * * *"}},
            "enabled": true
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);
    let task_id = json_body(resp).await["id"].as_str().unwrap().to_string();

    // 3. 立即运行：202 + run_id（门禁在 runner 边界重绑参数后交 RunManager）
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/tasks/{task_id}/run"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED, "{:?}", json_body(resp).await);
    let run_id = json_body(resp).await["run_id"].as_str().unwrap().to_string();

    // 4. 执行器收到的 payload = 绑定后的全量覆盖（gamer.yaml 私有 wire：
    //    {target, args: [{name, value: {type, value}}]}）；任务实参覆盖默认值，
    //    未填参数取声明默认值（int 经 text wire 承载）。
    let mut captured = None;
    for _ in 0..200 {
        {
            let guard = payloads.lock().unwrap();
            if let Some(payload) = guard.iter().find(|p| p["args"].as_array().is_some()) {
                captured = Some(payload.clone());
            }
        }
        if captured.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let payload = captured.expect("执行器必须收到任务运行 payload");
    let args = payload["args"].as_array().unwrap();
    let arg_value = |name: &str| {
        args.iter()
            .find(|a| a["name"] == serde_json::json!(name))
            .map(|a| a["value"].clone())
            .unwrap_or_else(|| panic!("绑定覆盖缺少参数 {name}: {args:?}"))
    };
    assert_eq!(
        arg_value("msg"),
        serde_json::json!({"type": "text", "value": "任务实参"}),
        "任务实参覆盖默认值"
    );
    assert_eq!(
        arg_value("count"),
        serde_json::json!({"type": "text", "value": "3"}),
        "任务未填参数取声明默认值"
    );

    // 5. run 记录关联任务并收敛 Success；任务侧 last_run_at 由完成回调落库
    //    （轮询等待 = 后台完成链在测试运行时存活期内走完，不悬到关停窗口）。
    let mut record = None;
    for _ in 0..200 {
        let resp = get_json(&t, &sid, &format!("/api/runs/{run_id}")).await;
        let run = json_body(resp).await;
        if run["state"] == "success" {
            record = Some(run);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let run = record.expect("run 必须收敛 success");
    assert_eq!(run["task_id"], task_id);
    assert_eq!(run["runner_id"], YAML_RUNNER);
    assert_eq!(run["entrypoint"], "com.example.game/daily.yaml");
    let mut settled = false;
    for _ in 0..200 {
        let task = json_body(get_json(&t, &sid, &format!("/api/tasks/{task_id}")).await).await;
        if !task["last_run_at"].is_null() {
            assert_eq!(task["last_result"], "成功");
            assert_eq!(task["state"], "active", "任务运行完成后保持 Active");
            settled = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(settled, "完成回调必须把任务结果落库");

    // 6. 非法 payload 门禁（任务保存时 payload 不透明，运行时才校验）：
    //    必填参数缺失 → 400 + 结构化诊断消息。
    let resp = post_json(
        &t,
        &sid,
        "/api/apps/com.example.game/resources/scripts",
        serde_json::json!({
            "name": "required",
            "content": "version: 3\nparams:\n  - 'text:secret:密文'\nsteps:\n  - log: $secret\n",
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);
    let resp = post_json(
        &t,
        &sid,
        "/api/tasks",
        serde_json::json!({
            "name": "Missing Required",
            "app": {"device_id": "d1", "android_package": "com.example.game", "content_package": "com.example.game"},
            "runner": {
                "runner_id": YAML_RUNNER,
                "entrypoint": "com.example.game/required.yaml",
                "payload": {"args": {}}
            },
            "schedule": {"provider_id": "cron", "config": {"expression": "0 9 * * *"}},
            "enabled": true
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);
    let missing_id = json_body(resp).await["id"].as_str().unwrap().to_string();
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/tasks/{missing_id}/run"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{:?}", json_body(resp).await);
    let body = json_body(resp).await;
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("必填参数 secret 未提供"),
        "缺必填必须带结构化诊断: {body}"
    );

    // 7. 类型不符 → 400；带过期 param_signature 的旧数据 → 400 参数过期门禁。
    let update_task = |args: serde_json::Value| {
        serde_json::json!({
            "id": missing_id,
            "name": "Missing Required",
            "app": {"device_id": "d1", "android_package": "com.example.game", "content_package": "com.example.game"},
            "runner": {
                "runner_id": YAML_RUNNER,
                "entrypoint": "com.example.game/required.yaml",
                "payload": args
            },
            "schedule": {"provider_id": "cron", "config": {"expression": "0 9 * * *"}},
            "enabled": true
        })
    };
    let resp = send(
        &t.app,
        req(
            "PUT",
            &format!("/api/tasks/{missing_id}"),
            None,
            &json_headers(sid.to_string()),
            Some(update_task(serde_json::json!({"args": {"secret": 123}})).to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/tasks/{missing_id}/run"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{:?}", json_body(resp).await);
    let body = json_body(resp).await;
    assert!(
        body["error"].as_str().unwrap_or_default().contains("类型无效"),
        "类型不符必须诊断: {body}"
    );

    let resp = send(
        &t.app,
        req(
            "PUT",
            &format!("/api/tasks/{missing_id}"),
            None,
            &json_headers(sid.to_string()),
            Some(
                update_task(serde_json::json!(
                    {"args": {"secret": "ok"}, "param_signature": "psig1-stale"}
                ))
                .to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/tasks/{missing_id}/run"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{:?}", json_body(resp).await);
    let body = json_body(resp).await;
    assert!(
        body["error"].as_str().unwrap_or_default().contains("任务参数过期"),
        "过期签名必须走参数过期门禁: {body}"
    );
    // 门禁失败不落 run、任务不受牵连（保持 Active，等参数修好后可再跑）
    let task = json_body(get_json(&t, &sid, &format!("/api/tasks/{missing_id}")).await).await;
    assert_eq!(task["state"], "active");
}
