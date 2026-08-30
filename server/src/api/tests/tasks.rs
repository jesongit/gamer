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

#[tokio::test]
async fn task_args_snapshot_save_conflict_reconfirm_and_stale_flag() {
    let t = build_app(
        "task-snapshot",
        auth::Credential::Plain("admin123".into()),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    save_task_script(&t, &sid, TASK_SCRIPT_V1).await;

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
    save_task_script(&t, &sid, TASK_SCRIPT_V2).await;
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
