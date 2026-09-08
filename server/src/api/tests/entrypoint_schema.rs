use super::*;

// ---------- P12.3：entrypoint 参数 schema API + V1 参数链（POST /api/runs）----------
//
// - GET /api/runners/:runner_id/entrypoint?entrypoint=<资源id>：V1 params
//   schema（`schema` = 参数声明数组：name/type/required/default/desc；前端
//   不为取参数而解析 YAML）；旧 v3 源（version 字段）→ yaml.version.removed。
// - POST /api/runs：手动运行参数绑定（缺必填/未知键/类型不符前置 400）；
//   旧 v3 存量源提交即版本迁移错误（无 fallback）。

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
    // 建包（幂等：已存在即 409，忽略）
    let _ = post_json(
        t,
        sid,
        "/api/packages",
        serde_json::json!({ "id": "com.test.app" }),
    )
    .await;
    // Package API：PUT 创建/更新（force 跳过版本门禁，夹具语义 = 直写）
    let resp = send(
        &t.app,
        req(
            "PUT",
            &format!("/api/packages/com.test.app/plugins/gamer.yaml/resources/{kind}/{name}.yaml"),
            None,
            &json_headers(sid.to_string()),
            Some(
                serde_json::json!({ "content": content, "force": true }).to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "保存 {kind}/{name} 失败: {:?}",
        json_body(resp).await
    );
}

/// 直写分区目录（绕过保存期校验，构造「盘上已有」的存量资源形态）。
fn write_partition_file(t: &TestApp, kind_dir: &str, name: &str, content: &str) {
    // 直写包插件目录（新布局 packages/<pkg>/plugins/gamer.yaml/<kind>）
    let dir = t.dir
        .join("packages/com.test.app/plugins/gamer.yaml")
        .join(kind_dir);
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

/// GET /api/runners/:runner_id/functions：原生函数目录（插件函数 Schema 唯一
/// 前端数据源）；未知 runner → 404 runner_not_found。
#[tokio::test]
async fn runner_functions_endpoint_serves_native_catalog() {
    let t = build_app("fn-catalog", test_credential("admin123"), Default::default());
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    let resp = get_json(&t, &sid, "/api/runners/gamer.yaml/functions").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_body(resp).await;
    assert_eq!(j["runner_id"], "gamer.yaml");
    let functions = j["functions"].as_array().expect("functions 必须是数组");
    assert!(!functions.is_empty());
    let tap = functions
        .iter()
        .find(|f| f["name"] == "tap")
        .expect("tap 必须在目录中");
    assert_eq!(tap["source"], "plugin");
    assert_eq!(tap["params"][0]["name"], "position");
    assert_eq!(tap["params"][0]["type"], "point");
    let wait_find = functions.iter().find(|f| f["name"] == "wait_find").unwrap();
    assert_eq!(wait_find["params"][2]["default"], "30s");

    let resp = get_json(&t, &sid, "/api/runners/no.such/functions").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(resp).await["error"], "runner_not_found");
}

/// V1 schema（参数声明数组）+ 旧 v3 源版本迁移诊断 + not_found/invalid/未知 runner。
#[tokio::test]
async fn entrypoint_schema_endpoint_serves_v1_and_rejects_legacy_sources() {
    let t = build_app("ep-schema", test_credential("admin123"), Default::default());
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // 显式带 version 的旧 v3 源直写分区（保存边界已拒收，见 write_partition_file 注）：
    // describe 必须报 yaml.version.removed（V1 无 version 字段，无 fallback）
    write_partition_file(
        &t,
        "automations",
        "v3daily.yaml",
        "version: 3\nparams:\n  - 'text:msg:消息'\nsteps:\n  - log: $msg\n",
    );
    save_resource(
        &t,
        &sid,
        "automations",
        "v1daily",
        "params:\n  msg:\n    type: string\n    default: \"默认\"\n    desc: 消息\n  wait:\n    type: duration\n    default: 2s\n  count:\n    type: integer\n    default: 3\nrun:\n  - log: $msg\n",
    )
    .await;
    save_resource(
        &t,
        &sid,
        "automations",
        "v1req",
        "params:\n  secret:\n    type: string\n    required: true\nrun:\n  - log: $secret\n",
    )
    .await;
    // V1 函数库直写分区
    write_partition_file(
        &t,
        "functions",
        "lib.yaml",
        "functions:\n  greet:\n    params:\n      who:\n        type: string\n        default: \"玩家\"\n      times:\n        type: integer\n        default: 2\n    run:\n      - log: $who\n",
    );

    // 旧 v3 源 → 版本迁移 invalid（yaml.version.removed，无 fallback）
    let (status, v3) = describe_entrypoint(&t, &sid, "com.test.app/v3daily.yaml").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v3}");
    assert_eq!(v3["error"], "invalid_script");
    assert!(
        v3["diagnostics"].as_array().unwrap().iter().any(|d| d["code"] == "yaml.version.removed"),
        "旧 v3 源必须报版本迁移诊断: {v3}"
    );

    // V1 脚本：参数声明数组（name/type/required/default/desc）
    let (status, v1) = describe_entrypoint(&t, &sid, "com.test.app/v1daily.yaml").await;
    assert_eq!(status, StatusCode::OK, "{v1}");
    assert_eq!(v1["kind"], "script");
    assert_eq!(v1["format"], "yaml-params-v1");
    let schema = v1["schema"].as_array().unwrap();
    assert_eq!(schema.len(), 3);
    assert_eq!(schema[0]["name"], "msg");
    assert_eq!(schema[0]["type"], "string");
    assert_eq!(schema[0]["default"], "默认");
    assert_eq!(schema[0]["desc"], "消息");
    assert_eq!(schema[1]["type"], "duration");
    assert_eq!(schema[1]["default"], "2s");
    assert_eq!(schema[2]["name"], "count");
    assert_eq!(schema[2]["type"], "integer");
    assert!(v1.get("signature").is_none(), "V1 无签名字段");

    // 必填参数 required: true
    let (status, v1req) = describe_entrypoint(&t, &sid, "com.test.app/v1req.yaml").await;
    assert_eq!(status, StatusCode::OK, "{v1req}");
    assert_eq!(v1req["schema"][0]["required"], serde_json::json!(true));

    // V1 函数库 entrypoint（functions: 包装）
    let (status, greet) = describe_entrypoint(&t, &sid, "com.test.app/lib.yaml#greet").await;
    assert_eq!(status, StatusCode::OK, "{greet}");
    assert_eq!(greet["kind"], "function");
    assert_eq!(greet["schema"][1]["type"], "integer");

    // 资源缺失 → 结构化 not_found；解析失败 → 400 invalid_script；
    // 未知 runner → 404 runner_not_found
    let (status, missing) = describe_entrypoint(&t, &sid, "com.test.app/ghost.yaml").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    assert_eq!(missing["error"], "not_found");
    // 解析失败 → 400 invalid_script（直写坏源：保存期校验本就会拒绝它）
    write_partition_file(&t, "automations", "broken.yaml", "run:\n  - if: $x\n");
    let (status, broken) = describe_entrypoint(&t, &sid, "com.test.app/broken.yaml").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{broken}");
    assert_eq!(broken["error"], "invalid_script");
    assert!(!broken["diagnostics"].as_array().unwrap().is_empty());
    let resp = get_json(
        &t,
        &sid,
        &format!(
            "/api/runners/no.such%2Frunner/entrypoint?entrypoint={}",
            urlencode("com.test.app/v1daily.yaml")
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(resp).await["error"], "runner_not_found");
}

/// POST /api/runs V1 脚本手动运行：无参（默认值）/显式传参 202 + resolved_args；
/// 缺必填 / 未知键 / 类型不符前置 400 invalid_args。
#[tokio::test]
async fn v1_manual_runs_flow_through_param_binding() {
    let t = build_app("ep-v1run", test_credential("admin123"), Default::default());
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // 运行目标设备（POST /api/runs 的 Android 上下文严格取设备 pkg）。
    // 多设备 = 规避设备级单活动运行互斥。
    for id in ["d1", "d2", "d3", "d4", "d5", "d6", "d8"] {
        seed_device(&t, id, "com.example.game").await;
    }

    save_resource(
        &t,
        &sid,
        "automations",
        "v1opt",
        "params:\n  msg:\n    type: string\n    default: \"默认\"\n  fast:\n    type: boolean\n    default: false\n  wait:\n    type: duration\n    default: 2s\n  count:\n    type: integer\n    default: 3\nrun:\n  - log: $msg\n",
    )
    .await;
    save_resource(
        &t,
        &sid,
        "automations",
        "v1req",
        "params:\n  secret:\n    type: string\n    required: true\nrun:\n  - log: $secret\n",
    )
    .await;
    // V1 函数库直写分区（函数运行参数从目标函数声明解析）
    write_partition_file(
        &t,
        "functions",
        "lib.yaml",
        "functions:\n  greet:\n    params:\n      who:\n        type: string\n        default: \"玩家\"\n      times:\n        type: integer\n        default: 2\n    run:\n      - log: $who\n",
    );

    // 无参运行：202 + resolved_args 为默认值合并视图
    let resp = post_json(
        &t,
        &sid,
        "/api/runs",
        dispatch_body_for("com.test.app/v1opt.yaml", "d1", serde_json::json!({})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let j = json_body(resp).await;
    assert!(j.get("run_id").and_then(|v| v.as_str()).is_some());
    let mut dispatched_runs = vec![j["run_id"].as_str().unwrap().to_string()];
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
            "com.test.app/v1opt.yaml",
            "d2",
            serde_json::json!({"args": {"msg": "直跑", "fast": true, "wait": "3s"}}),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let j = json_body(resp).await;
    dispatched_runs.push(j["run_id"].as_str().unwrap().to_string());
    assert_eq!(j["resolved_args"]["msg"], "直跑");
    assert_eq!(j["resolved_args"]["fast"], true);
    assert_eq!(j["resolved_args"]["wait"], "3s");
    assert_eq!(j["resolved_args"]["count"], 3, "未覆盖参数取默认值");

    // 缺必填 → 400 invalid_args + param.args.missing_required（field 可回填表单）
    let resp = post_json(
        &t,
        &sid,
        "/api/runs",
        dispatch_body_for("com.test.app/v1req.yaml", "d3", serde_json::json!({})),
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
            "com.test.app/v1req.yaml",
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
            "com.test.app/v1req.yaml",
            "d5",
            serde_json::json!({"args": {"secret": 123}}),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let j = json_body(resp).await;
    assert!(j["diagnostics"].as_array().unwrap().iter().any(|d| d["code"]
        == "param.args.type_mismatch"));

    // V1 函数库 entrypoint（函数运行参数从函数声明解析）
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
    dispatched_runs.push(j["run_id"].as_str().unwrap().to_string());
    assert_eq!(j["resolved_args"]["who"], "函数");
    assert_eq!(j["resolved_args"]["times"], 2);

    // 旧 v3 存量脚本（直写分区）：运行提交即版本迁移 400（无 fallback）
    write_partition_file(
        &t,
        "automations",
        "v3run.yaml",
        "version: 3\nparams:\n  - 'text:msg:消息'\nsteps:\n  - log: $msg\n",
    );
    let resp = post_json(
        &t,
        &sid,
        "/api/runs",
        dispatch_body_for(
            "com.test.app/v3run.yaml",
            "d8",
            serde_json::json!({"args": {"msg": "v3 实参"}}),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let j = json_body(resp).await;
    assert_eq!(j["error"], "invalid_script");
    assert!(
        j["diagnostics"].as_array().unwrap().iter().any(|d| d["code"] == "yaml.version.removed"),
        "旧 v3 脚本手动运行必须报版本迁移诊断: {j}"
    );

    // 偶发防御：本测试派发的 run 无设备可连，后台 prepare 立即失败；
    // 逐个轮询到终态再收工（404 容忍重试：RunManager::finalize 摘注册表与
    // 入档案之间存在瞬时不可见间隙）。
    for run_id in dispatched_runs {
        for _ in 0..200 {
            let resp = get_json(&t, &sid, &format!("/api/runs/{run_id}")).await;
            let status = resp.status();
            if status == StatusCode::NOT_FOUND {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                continue;
            }
            let run = json_body(resp).await;
            let state = run["state"].as_str().unwrap_or_default();
            if state != "starting" && state != "running" {
                assert_eq!(state, "failed", "无设备 run 必须以失败收敛: {run}");
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}
