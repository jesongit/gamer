use super::*;

// ---------- P12.3：entrypoint 参数 schema API + v3 参数链（POST /api/runs）----------
//
// - GET /api/runners/:runner_id/entrypoint?entrypoint=<资源id>：v2/v3 双格式
//   参数 schema（前端不为取参数而解析 YAML）。
// - POST /api/runs：version:3 脚本手动运行从 runner 边界修通（v2 loader 会把
//   合法 v3 顶层键集拒成解析失败）；缺必填/未知键/类型不符前置 400。

fn dispatch_body_for(entrypoint: &str, device_id: &str, payload: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "runner_id": "gamer.yaml",
        "entrypoint": entrypoint,
        "device_id": device_id,
        "payload": payload,
    })
}

async fn save_resource(
    t: &TestApp,
    sid: &str,
    kind: &str,
    name: &str,
    content: &str,
) {
    let body = serde_json::json!({ "name": name, "content": content });
    let resp = post_json(
        t,
        sid,
        &format!("/api/apps/com.test.app/resources/{kind}"),
        body,
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "保存 {kind}/{name} 失败: {:?}",
        json_body(resp).await
    );
}

/// 直写分区目录（函数库保存期校验仍按 v2 严格解析，v3 扩展类型经 API 保存
/// 被拒——descriptor/runner 边界已先行支持，保存侧放开归 call 统一任务）。
fn write_partition_file(t: &TestApp, kind_dir: &str, name: &str, content: &str) {
    let dir = t.dir.join("com.test.app").join(kind_dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), content).unwrap();
}

async fn describe_entrypoint(t: &TestApp, sid: &str, entrypoint: &str) -> (StatusCode, serde_json::Value) {
    let uri = format!(
        "/api/runners/gamer.yaml/entrypoint?entrypoint={}",
        urlencode(entrypoint)
    );
    let resp = get_json(t, sid, &uri).await;
    let status = resp.status();
    (status, json_body(resp).await)
}

fn urlencode(raw: &str) -> String {
    let mut out = String::new();
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// v2/v3 双格式 schema + 结构化 not_found/invalid/未知 runner。
#[tokio::test]
async fn entrypoint_schema_endpoint_serves_v2_and_v3_formats() {
    let t = build_app("ep-schema", test_credential("admin123"), Default::default());
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    save_resource(
        &t,
        &sid,
        "scripts",
        "v2daily",
        "params:\n  - 'text:msg:消息:\"默认\"'\n  - 'bool:fast:快速'\nsteps:\n  - log: $msg\n",
    )
    .await;
    save_resource(
        &t,
        &sid,
        "scripts",
        "v3daily",
        "version: 3\nparams:\n  - 'text:msg:消息:\"默认\"'\n  - 'time:wait:等待:2s'\n  - name: count\n    type: int\n    default: 3\nsteps:\n  - log: $msg\n",
    )
    .await;
    save_resource(
        &t,
        &sid,
        "scripts",
        "v3req",
        "version: 3\nparams:\n  - 'text:secret:密文'\nsteps:\n  - log: $secret\n",
    )
    .await;
    // v3 函数库直写分区（保存期校验仍按 v2 严格解析，见 write_partition_file 注）
    write_partition_file(
        &t,
        "functions",
        "lib.yaml",
        "greet:\n  params:\n    - 'text:who:称呼:\"玩家\"'\n    - 'int:times:次数:2'\n  steps:\n    - log: $who\n",
    );

    // v2 存量脚本走同一端点（服务端 v2 解析）
    let (status, v2) = describe_entrypoint(&t, &sid, "com.test.app/v2daily.yaml").await;
    assert_eq!(status, StatusCode::OK, "{v2}");
    assert_eq!(v2["runner_id"], "gamer.yaml");
    assert_eq!(v2["entrypoint"], "com.test.app/v2daily.yaml");
    assert_eq!(v2["kind"], "script");
    assert_eq!(v2["schema"]["properties"]["msg"]["type"], "string");
    assert_eq!(v2["schema"]["properties"]["msg"]["default"], "默认");
    assert_eq!(v2["schema"]["properties"]["msg"]["description"], "消息");
    assert_eq!(v2["schema"]["properties"]["fast"]["type"], "boolean");
    assert_eq!(v2["schema"]["required"], serde_json::json!(["fast"]));
    assert!(v2["signature"].as_str().unwrap().starts_with("psig1|"));

    // v3 脚本：int → integer、string 形态默认值规整、time 保留书写串
    let (status, v3) = describe_entrypoint(&t, &sid, "com.test.app/v3daily.yaml").await;
    assert_eq!(status, StatusCode::OK, "{v3}");
    assert_eq!(v3["schema"]["properties"]["count"]["type"], "integer");
    assert_eq!(v3["schema"]["properties"]["count"]["default"], 3);
    assert_eq!(v3["schema"]["properties"]["msg"]["default"], "默认");
    assert_eq!(v3["schema"]["properties"]["wait"]["type"], "string");
    assert_eq!(v3["schema"]["properties"]["wait"]["param_type"], "time");
    assert_eq!(v3["schema"]["properties"]["wait"]["default"], "2s");
    assert_eq!(v3["schema"]["required"], serde_json::json!([]));
    assert!(v3["signature"].as_str().unwrap().starts_with("psig1|"));

    // v3 必填参数出现在 required
    let (status, v3req) = describe_entrypoint(&t, &sid, "com.test.app/v3req.yaml").await;
    assert_eq!(status, StatusCode::OK, "{v3req}");
    assert_eq!(v3req["schema"]["required"], serde_json::json!(["secret"]));

    // v3 函数库 entrypoint（bare-map，无 version 键）
    let (status, greet) = describe_entrypoint(&t, &sid, "com.test.app/lib.yaml#greet").await;
    assert_eq!(status, StatusCode::OK, "{greet}");
    assert_eq!(greet["kind"], "function");
    assert_eq!(greet["schema"]["properties"]["times"]["type"], "integer");

    // 资源缺失 → 结构化 not_found；解析失败 → 400 invalid_script；
    // 未知 runner → 404 runner_not_found
    let (status, missing) = describe_entrypoint(&t, &sid, "com.test.app/ghost.yaml").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    assert_eq!(missing["error"], "not_found");
    // 解析失败 → 400 invalid_script（直写坏源：保存期校验本就会拒绝它）
    write_partition_file(&t, "scripts", "broken.yaml", "version: 3\nparams: []\n");
    let (status, broken) = describe_entrypoint(&t, &sid, "com.test.app/broken.yaml").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{broken}");
    assert_eq!(broken["error"], "invalid_script");
    assert!(!broken["diagnostics"].as_array().unwrap().is_empty());
    let resp = get_json(
        &t,
        &sid,
        &format!(
            "/api/runners/no.such%2Frunner/entrypoint?entrypoint={}",
            urlencode("com.test.app/v3daily.yaml")
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(resp).await["error"], "runner_not_found");
}

/// POST /api/runs v3 脚本手动运行：无参（默认值）/显式传参 202 + resolved_args；
/// 缺必填 / 未知键 / 类型不符前置 400 invalid_args；v2 路径回归不变。
#[tokio::test]
async fn v3_manual_runs_flow_through_param_bridge() {
    let t = build_app("ep-v3run", test_credential("admin123"), Default::default());
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    save_resource(
        &t,
        &sid,
        "scripts",
        "v3opt",
        "version: 3\nparams:\n  - 'text:msg:消息:\"默认\"'\n  - 'bool:fast:快速:false'\n  - 'time:wait:等待:2s'\n  - name: count\n    type: int\n    default: 3\nsteps:\n  - log: $msg\n",
    )
    .await;
    save_resource(
        &t,
        &sid,
        "scripts",
        "v3req",
        "version: 3\nparams:\n  - 'text:secret:密文'\nsteps:\n  - log: $secret\n",
    )
    .await;
    // v3 函数库直写分区（int 声明使 v2 解析失败 → v3 宽松兜底）
    write_partition_file(
        &t,
        "functions",
        "lib.yaml",
        "greet:\n  params:\n    - 'text:who:称呼:\"玩家\"'\n    - 'int:times:次数:2'\n  steps:\n    - log: $who\n",
    );

    // 无参运行：202 + resolved_args 为默认值合并视图
    let resp = post_json(
        &t,
        &sid,
        "/api/runs",
        dispatch_body_for("com.test.app/v3opt.yaml", "d1", serde_json::json!({})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let j = json_body(resp).await;
    assert!(j.get("run_id").and_then(|v| v.as_str()).is_some());
    assert_eq!(j["resolved_args"]["msg"], "默认");
    assert_eq!(j["resolved_args"]["fast"], false);
    assert_eq!(j["resolved_args"]["wait"], "2s");
    assert_eq!(j["resolved_args"]["count"], 3);

    // 显式传参：覆盖进 resolved_args
    let resp = post_json(
        &t,
        &sid,
        "/api/runs",
        dispatch_body_for(
            "com.test.app/v3opt.yaml",
            "d2",
            serde_json::json!({"args": {"msg": "直跑", "fast": true, "wait": "3s"}}),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let j = json_body(resp).await;
    assert_eq!(j["resolved_args"]["msg"], "直跑");
    assert_eq!(j["resolved_args"]["fast"], true);
    assert_eq!(j["resolved_args"]["wait"], "3s");
    assert_eq!(j["resolved_args"]["count"], 3, "未覆盖参数取默认值");

    // 缺必填 → 400 invalid_args + param.args.missing_required（field 可回填表单）
    let resp = post_json(
        &t,
        &sid,
        "/api/runs",
        dispatch_body_for("com.test.app/v3req.yaml", "d3", serde_json::json!({})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let j = json_body(resp).await;
    assert_eq!(j["error"], "invalid_args");
    assert!(j["diagnostics"].as_array().unwrap().iter().any(|d| d["code"]
        == "param.args.missing_required"));

    // 未知键 → 400 param.args.unknown
    let resp = post_json(
        &t,
        &sid,
        "/api/runs",
        dispatch_body_for(
            "com.test.app/v3req.yaml",
            "d4",
            serde_json::json!({"args": {"secret": "v", "ghost": 1}}),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let j = json_body(resp).await;
    assert!(j["diagnostics"].as_array().unwrap().iter().any(|d| d["code"]
        == "param.args.unknown"));

    // 类型不符 → 400 param.args.type_mismatch
    let resp = post_json(
        &t,
        &sid,
        "/api/runs",
        dispatch_body_for(
            "com.test.app/v3req.yaml",
            "d5",
            serde_json::json!({"args": {"secret": 123}}),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let j = json_body(resp).await;
    assert!(j["diagnostics"].as_array().unwrap().iter().any(|d| d["code"]
        == "param.args.type_mismatch"));

    // v3 函数库 entrypoint（int 声明使 v2 解析失败 → v3 宽松兜底）
    let resp = post_json(
        &t,
        &sid,
        "/api/runs",
        dispatch_body_for(
            "com.test.app/lib.yaml#greet",
            "d6",
            serde_json::json!({"function": "greet", "args": {"who": "函数"}}),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let j = json_body(resp).await;
    assert_eq!(j["resolved_args"]["who"], "函数");
    assert_eq!(j["resolved_args"]["times"], 2);

    // v2 存量脚本路径回归：同一入口语义不变
    save_resource(
        &t,
        &sid,
        "scripts",
        "v2run",
        "params:\n  - 'text:msg:消息:\"默认\"'\nsteps:\n  - log: $msg\n",
    )
    .await;
    let resp = post_json(
        &t,
        &sid,
        "/api/runs",
        dispatch_body_for(
            "com.test.app/v2run.yaml",
            "d8",
            serde_json::json!({"args": {"msg": "v2 实参"}}),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let j = json_body(resp).await;
    assert_eq!(j["resolved_args"]["msg"], "v2 实参");
}
