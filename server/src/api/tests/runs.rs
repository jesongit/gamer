use super::*;

// ---------- 统一 RunTarget：函数测试运行端点（POST /api/functions/:id/run）----------

/// 挂起到取消的假执行器（真实 RunManager 语义下测 router 行为）。
struct HangExecutor;

impl crate::run_manager::RunExecutor for HangExecutor {
    fn prepare<'a>(
        &'a self,
        _: &'a crate::run_manager::StartRequest,
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn execute<'a>(
        &'a self,
        _: &'a crate::run_manager::StartRequest,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
        Box::pin(async move {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Ok(vec![("info".into(), "stopped".into())])
        })
    }
    fn occupy(&self, _: &str) {}
    fn release(&self, _: &str) {}
}

#[tokio::test]
async fn function_run_endpoint_conflict_args_and_cancel() {
    let executor = Arc::new(HangExecutor);
    let t = build_app_with_executor(
        "fnrun",
        auth::Credential::Plain("admin123".into()),
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

    // 正常提交：202 + run_id + resolved_args（默认值合并视图）
    let resp = post_json(
        &t,
        &sid,
        "/api/functions/com.test.app%2Fcommon.yaml/run",
        serde_json::json!({"device_id": "d1", "function": "login", "args": {"who": "路由"}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let j = json_body(resp).await;
    assert_eq!(j["state"], "starting");
    assert_eq!(j["resolved_args"]["who"], "路由");
    assert_eq!(j["resolved_args"]["fast"], false);
    let run_id = j["run_id"].as_str().unwrap().to_string();

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

    // 取消 → 202；终态 cancelled 可查询
    let resp = post_json(
        &t,
        &sid,
        &format!("/api/runs/{run_id}/cancel"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
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
    let _ = post_json(
        &t,
        &sid,
        &format!("/api/runs/{run_id}/cancel"),
        serde_json::json!({}),
    )
    .await;
}
