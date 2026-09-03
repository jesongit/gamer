use super::*;

// ---------- 阶段 5：任务参数快照 / 签名过期 / 重新确认（HTTP 形状） ----------

const TASK_SCRIPT_V1: &str = "params:
  - 'bool:enable:是否启用:true'
  - 'text:message:提示文本:\"hello\"'
  - 'time:timeout:最长等待'
steps:
  - log: 'ok'
";

const TASK_SCRIPT_V2: &str = "params:
  - 'bool:enable:是否启用:true'
  - 'text:message:提示文本:\"NEW-DEFAULT-VALUE\"'
  - 'time:timeout:最长等待'
steps:
  - log: 'ok'
";

const TASK_SCRIPT_NO_PARAMS: &str = "steps:
  - log: 'ok'
";

/// next_run 序列化必须带时区偏移（`2026-09-01 10:00:00+08:00` 形态）：前端
/// task-tz.js 从该偏移推导「服务端时区 UTC+08:00」标签（/api/system/info 按契约
/// 禁止暴露 timezone）；无偏移旧形态会让标签一直休眠兜底。
#[tokio::test]
async fn task_next_run_serialized_with_timezone_offset() {
    let t = build_app(
        "task-next-run",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    save_task_script(&t, &sid, "daily.yaml", TASK_SCRIPT_NO_PARAMS).await;
    let resp = post_json(
        &t,
        &sid,
        "/api/tasks",
        serde_json::json!({"name": "Daily", "cron": "0 * * * * *",
                "script_id": "com.test.app/daily.yaml", "device_id": "d1"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);

    let resp = get_json(&t, &sid, "/api/tasks").await;
    let j = json_body(resp).await;
    let next = j[0]["next_run"]
        .as_str()
        .expect("next_run 必须是字符串")
        .to_string();
    let parsed = chrono::DateTime::parse_from_str(&next, "%Y-%m-%d %H:%M:%S%:z")
        .unwrap_or_else(|e| panic!("next_run 必须带 ±HH:MM 时区偏移且可回读: {next} ({e})"));
    assert_eq!(
        parsed.offset().local_minus_utc(),
        chrono::Local::now().offset().local_minus_utc(),
        "偏移必须是服务端本地时区偏移: {next}"
    );
    // 详情端点同口径
    let task_id = j[0]["id"].as_str().unwrap().to_string();
    let resp = get_json(&t, &sid, &format!("/api/tasks/{task_id}")).await;
    let j = json_body(resp).await;
    let next = j["next_run"].as_str().unwrap();
    assert!(
        chrono::DateTime::parse_from_str(next, "%Y-%m-%d %H:%M:%S%:z").is_ok(),
        "详情 next_run 必须带时区偏移: {next}"
    );
}

#[tokio::test]
async fn task_args_snapshot_save_conflict_reconfirm_and_stale_flag() {
    let t = build_app(
        "task-snapshot",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    save_task_script(&t, &sid, "daily.yaml", TASK_SCRIPT_V1).await;

    // 缺必填参数（无 args）→ 400 invalid_args + param.args.missing_required
    let resp = post_json(
        &t,
        &sid,
        "/api/tasks",
        serde_json::json!({"name": "Daily", "cron": "0 * * * * *",
                "script_id": "com.test.app/daily.yaml", "device_id": "d1"}),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "{:?}",
        json_body(resp).await
    );
    let j = json_body(resp).await;
    assert_eq!(j["error"], "invalid_args");
    assert!(j["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["code"] == "param.args.missing_required"));

    // 未知参数 → 400 param.args.unknown
    let resp = post_json(
        &t,
        &sid,
        "/api/tasks",
        serde_json::json!({"name": "Daily", "cron": "0 * * * * *",
                "script_id": "com.test.app/daily.yaml", "device_id": "d1",
                "args": {"enable": true, "message": "m", "timeout": "30s", "ghost": 1}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let j = json_body(resp).await;
    assert!(j["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["code"] == "param.args.unknown"));

    // 正常创建：稀疏 args → 服务端解析为完整快照 + psig1 签名落库
    let resp = post_json(
        &t,
        &sid,
        "/api/tasks",
        serde_json::json!({"name": "Daily", "cron": "0 * * * * *",
                "script_id": "com.test.app/daily.yaml", "device_id": "d1",
                "args": {"enable": false, "message": "custom-text", "timeout": "45s"}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    let j = json_body(resp).await;
    let task_id = j["id"].as_str().unwrap().to_string();
    let sig_v1 = j["param_signature"].as_str().unwrap().to_string();
    assert!(sig_v1.starts_with("psig1|"));
    assert_eq!(j["args"]["enable"], false);
    assert_eq!(j["args"]["message"], "custom-text");
    assert_eq!(
        j["args"]["timeout"], "45s",
        "快照必须是全量（必填项也有值）"
    );

    // 列表：param_stale=false / has_args=true；详情返回 args 解析视图
    let resp = get_json(&t, &sid, "/api/tasks").await;
    let j = json_body(resp).await;
    assert_eq!(j[0]["param_stale"], false);
    assert_eq!(j[0]["has_args"], true);
    assert_eq!(j[0]["param_signature"], sig_v1);
    let resp = get_json(&t, &sid, &format!("/api/tasks/{task_id}")).await;
    let j = json_body(resp).await;
    assert_eq!(j["args"]["message"], "custom-text", "详情含原快照视图");

    // 脚本默认值变化 → 签名过期：列表 param_stale=true，PUT 无 reconfirm → 409
    update_task_script(&t, &sid, TASK_SCRIPT_V2).await;
    let resp = get_json(&t, &sid, "/api/tasks").await;
    let j = json_body(resp).await;
    assert_eq!(j[0]["param_stale"], true);

    let resp = post_json(
        &t,
        &sid,
        "/api/tasks",
        serde_json::json!({"id": task_id, "name": "Daily", "cron": "0 * * * * *",
                "script_id": "com.test.app/daily.yaml", "device_id": "d1"}),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "{:?}",
        json_body(resp).await
    );
    let j = json_body(resp).await;
    assert_eq!(j["code"], "param_signature_conflict");
    assert_eq!(j["reason"], "signature_mismatch");
    assert_eq!(j["actual"], sig_v1);
    assert!(j["expected"].as_str().unwrap().starts_with("psig1|"));
    assert_eq!(j["resource"], "com.test.app/daily.yaml");

    // 立即运行：签名过期 → 409 param_signature_conflict（明确失败不空跑）
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/tasks/{task_id}/run"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "{:?}",
        json_body(resp).await
    );
    assert_eq!(json_body(resp).await["code"], "param_signature_conflict");

    // reconfirm:true（不带 args）：存活参数保留原值 + 签名重算 → 200
    let resp = post_json(
        &t,
        &sid,
        "/api/tasks",
        serde_json::json!({"id": task_id, "name": "Daily", "cron": "0 * * * * *",
                "script_id": "com.test.app/daily.yaml", "device_id": "d1",
                "reconfirm": true}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    let j = json_body(resp).await;
    let sig_v2 = j["param_signature"].as_str().unwrap().to_string();
    assert_ne!(sig_v2, sig_v1, "重新确认必须重算签名");
    assert_eq!(j["args"]["message"], "custom-text", "存活参数保留原值");
    assert_eq!(j["args"]["timeout"], "45s");

    // 过期标记消除
    let resp = get_json(&t, &sid, "/api/tasks").await;
    let j = json_body(resp).await;
    assert_eq!(j[0]["param_stale"], false);
    assert_eq!(j[0]["param_signature"], sig_v2);

    // 无参数脚本也必须落完整、非空快照：{} + 有效 psig1 签名。
    save_task_script(&t, &sid, "no-params.yaml", TASK_SCRIPT_NO_PARAMS).await;
    let resp = post_json(
        &t,
        &sid,
        "/api/tasks",
        serde_json::json!({"name": "NoArgs", "cron": "0 * * * * *",
                "script_id": "com.test.app/no-params.yaml", "device_id": "d1"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    let j = json_body(resp).await;
    let no_args_task_id = j["id"].as_str().unwrap().to_string();
    assert_eq!(j["args"], serde_json::json!({}));
    assert_eq!(j["param_signature"], "psig1|");
    let resp = get_json(&t, &sid, &format!("/api/tasks/{no_args_task_id}")).await;
    let j = json_body(resp).await;
    assert_eq!(j["args"], serde_json::json!({}));
    assert_eq!(j["has_args"], true);
    assert_eq!(j["param_signature"], "psig1|");

    // 立即运行恢复 202（门禁通过；设备不存在→202 提交后 prepare 失败属正常语义）
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/tasks/{task_id}/run"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::ACCEPTED,
        "{:?}",
        json_body(resp).await
    );

    // 不存在的脚本保存任务 → 404
    let resp = post_json(
        &t,
        &sid,
        "/api/tasks",
        serde_json::json!({"name": "X", "cron": "0 * * * * *",
                "script_id": "com.test.app/nope.yaml", "device_id": "d1",
                "args": {}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn user_tasks_and_presets_have_independent_lifecycles() {
    let t = build_app(
        "generic-user-task",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let schedule = serde_json::json!({
        "kind": "cron",
        "value": {"expression": "*/5 * * * *"}
    });
    let resp = post_json(
        &t,
        &sid,
        "/api/task-presets",
        serde_json::json!({
            "id": "preset-daily",
            "app_package": "official.xxx",
            "name": "Daily preset",
            "runner_id": "missing.runner",
            "entrypoint": "daily",
            "payload": {"mode": "safe"},
            "schedule": schedule
        }),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "{:?}",
        json_body(resp).await
    );

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
    let resp = send(
        &t.app,
        req(
            "GET",
            "/api/task-presets?app_package=other.xxx",
            None,
            &json_headers(sid.to_string()),
            None,
        ),
    )
    .await;
    assert!(json_body(resp).await.as_array().unwrap().is_empty());

    let resp = send(
        &t.app,
        req(
            "PUT",
            "/api/task-presets/preset-daily",
            None,
            &json_headers(sid.to_string()),
            Some(
                serde_json::json!({
                    "app_package": "official.xxx",
                    "name": "Updated preset",
                    "runner_id": "missing.runner",
                    "entrypoint": "daily",
                    "payload": {"mode": "safe", "revision": 2},
                    "schedule": schedule
                })
                .to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);

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
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "{:?}",
        json_body(resp).await
    );
    let task = json_body(resp).await;
    let task_id = task["id"].as_str().unwrap().to_string();
    assert_eq!(task["app"]["content_package"], "official.xxx");
    assert_eq!(task["preset_id"], "preset-daily");
    assert_eq!(task["state"], "active");

    let resp = get_json(&t, &sid, "/api/user-tasks").await;
    let tasks = json_body(resp).await;
    assert_eq!(tasks.as_array().unwrap().len(), 1);
    assert_eq!(tasks[0]["runner_id"], "missing.runner");

    // The missing runner is a task dependency failure. It is persisted as
    // Suspended and returned as a dependency error, not surfaced at startup.
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/user-tasks/{task_id}/run"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FAILED_DEPENDENCY);
    assert_eq!(json_body(resp).await["code"], "dependency_unavailable");
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/user-tasks/{task_id}/run"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FAILED_DEPENDENCY);
    let resp = get_json(&t, &sid, &format!("/api/user-tasks/{task_id}")).await;
    let task = json_body(resp).await;
    assert_eq!(task["state"], "suspended");
    assert_eq!(task["suspend_reason"], "runner unavailable: missing.runner");

    // Suspend/resume are explicit and preserve the opaque schedule.
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/user-tasks/{task_id}/suspend"),
        serde_json::json!({"reason": "maintenance"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["state"], "suspended");
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/user-tasks/{task_id}/resume"),
        serde_json::json!({}),
    )
    .await;
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
    let resp = get_json(&t, &sid, &format!("/api/user-tasks/{task_id}")).await;
    assert_eq!(json_body(resp).await["preset_id"], "preset-daily");
}
