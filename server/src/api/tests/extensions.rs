use super::*;

#[tokio::test]
async fn extension_rest_lifecycle_registers_and_cleans_ui_contributions() {
    let test_app = build_app(
        "extensions",
        test_credential("admin123"),
        Default::default(),
    );
    let login_response = login(&test_app.app).await;
    assert_eq!(login_response.status(), StatusCode::OK);
    let session = first_cookie_pair(&cookie_of(&login_response));

    let manifest = br#"manifest_version = 1
id = "com.example.extension"
version = "1.0.0"
name = "Hello extension"
entry = "plugin.wasm"

[[ui.contributions]]
panel_id = "hello"
title = "Hello"
runtime = "iframe"
entry = "ui/index.html"
"#;
    let archive = craft_zip(vec![
        ("manifest.toml", manifest.to_vec()),
        ("plugin.wasm", b"\0asm\x01\0\0\0".to_vec()),
        ("ui/index.html", b"<h1>hello</h1>".to_vec()),
    ]);

    let inspected = send(
        &test_app.app,
        req_bytes(
            "POST",
            "/api/extensions/inspect",
            None,
            &zip_headers(session.clone()),
            archive.clone(),
        ),
    )
    .await;
    assert_eq!(inspected.status(), StatusCode::OK);
    let inspected_json = json_body(inspected).await;
    assert_eq!(inspected_json["id"], "com.example.extension");
    assert_eq!(inspected_json["version"], "1.0.0");
    assert_eq!(inspected_json["signature"]["status"], "unsigned");
    assert_eq!(
        inspected_json["permission_diff"]["added"],
        serde_json::json!([])
    );

    let management = get_json(&test_app, &session, "/api/extensions/management").await;
    assert_eq!(management.status(), StatusCode::OK);
    let management_json = json_body(management).await;
    assert_eq!(management_json["schema_version"], 1);
    assert!(management_json["extensions"].as_array().unwrap().is_empty());

    let installed = send(
        &test_app.app,
        req_bytes(
            "POST",
            "/api/extensions",
            None,
            &zip_headers(session.clone()),
            archive,
        ),
    )
    .await;
    assert_eq!(installed.status(), StatusCode::CREATED);
    let installed_json = json_body(installed).await;
    // 安装即用：桩 wasm 的 start 失败 → 自动降级 Enabled（last_error 可见）
    assert_eq!(installed_json["state"], "enabled");

    // Enabled（含安装即用降级）即发布 UI 贡献——iframe 面板随安装即可见
    let contributions = get_json(&test_app, &session, "/api/extensions/ui").await;
    assert_eq!(contributions.status(), StatusCode::OK);
    assert_eq!(json_body(contributions).await[0]["panel_id"], "hello");

    let enabled = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.extension/enable",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(enabled.status(), StatusCode::OK);
    let contributions = get_json(&test_app, &session, "/api/extensions/ui").await;
    let contributions_json = json_body(contributions).await;
    assert_eq!(contributions_json[0]["panel_id"], "hello");

    let asset = get_json(
        &test_app,
        &session,
        "/api/extensions/com.example.extension/ui/index.html",
    )
    .await;
    assert_eq!(asset.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(asset.into_body(), 1024).await.unwrap(),
        "<h1>hello</h1>"
    );

    let disabled = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.extension/disable",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(disabled.status(), StatusCode::OK);
    let contributions = get_json(&test_app, &session, "/api/extensions/ui").await;
    assert!(json_body(contributions)
        .await
        .as_array()
        .unwrap()
        .is_empty());
    let asset = get_json(
        &test_app,
        &session,
        "/api/extensions/com.example.extension/ui/index.html",
    )
    .await;
    assert_eq!(asset.status(), StatusCode::NOT_FOUND);

    let enabled = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.extension/enable",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(enabled.status(), StatusCode::OK);
    let start = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.extension/start",
        serde_json::json!({}),
    )
    .await;
    let expected_start_status = if cfg!(feature = "wasm-runtime") {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    assert_eq!(start.status(), expected_start_status);

    let disabled = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.extension/disable",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(disabled.status(), StatusCode::OK);
    std::fs::create_dir_all(test_app.dir.join("extension-data/com.example.extension")).unwrap();
    std::fs::write(
        test_app
            .dir
            .join("extension-data/com.example.extension/preferences.json"),
        b"keep-until-confirmed",
    )
    .unwrap();
    let removed = send(
        &test_app.app,
        req(
            "DELETE",
            "/api/extensions/com.example.extension/1.0.0?delete_data=1",
            None,
            &json_headers(session),
            None,
        ),
    )
    .await;
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
    assert!(!test_app
        .dir
        .join("extensions/com.example.extension")
        .exists());
    assert!(!test_app
        .dir
        .join("extension-data/com.example.extension")
        .exists());
}

/// runtime = "core" 的 UI 贡献 REST 契约：安装 → enable → `ui_contributions`
/// 原样透传 component（服务端不做组件名白名单）→ disable 后不再发布
/// （非 Enabled|Running 不进面板注册表）。
#[tokio::test]
async fn core_runtime_contributions_publish_component_and_follow_enabled_state() {
    let test_app = build_app(
        "extensions-core-ui",
        test_credential("admin123"),
        Default::default(),
    );
    let login_response = login(&test_app.app).await;
    let session = first_cookie_pair(&cookie_of(&login_response));

    let manifest = r#"manifest_version = 1
id = "com.example.coreui"
version = "1.0.0"
name = "Core UI extension"
entry = "plugin.wasm"

[ui]
[[ui.contributions]]
panel_id = "automation"
title = "自动化"
runtime = "core"
requires_device = true
component = "console.scripts"
"#
    .as_bytes();
    let archive = craft_zip(vec![
        ("manifest.toml", manifest.to_vec()),
        ("plugin.wasm", b"\0asm\x01\0\0\0".to_vec()),
    ]);

    let installed = send(
        &test_app.app,
        req_bytes(
            "POST",
            "/api/extensions",
            None,
            &zip_headers(session.clone()),
            archive,
        ),
    )
    .await;
    assert_eq!(installed.status(), StatusCode::CREATED);

    let enabled = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.coreui/enable",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(enabled.status(), StatusCode::OK);

    let contributions = get_json(&test_app, &session, "/api/extensions/ui").await;
    let panels = json_body(contributions).await;
    let panels = panels.as_array().unwrap();
    assert_eq!(panels.len(), 1);
    assert_eq!(panels[0]["panel_id"], "automation");
    assert_eq!(panels[0]["runtime"], "core");
    assert_eq!(panels[0]["component"], "console.scripts");
    assert_eq!(panels[0]["requires_device"], true);
    assert!(panels[0]["entry"].is_null());

    // 列表视图的 snapshot.ui 同样透传 component。
    let list = get_json(&test_app, &session, "/api/extensions").await;
    let list_json = json_body(list).await;
    assert_eq!(list_json["extensions"][0]["ui"][0]["runtime"], "core");
    assert_eq!(
        list_json["extensions"][0]["ui"][0]["component"],
        "console.scripts"
    );

    let disabled = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.coreui/disable",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(disabled.status(), StatusCode::OK);
    let contributions = get_json(&test_app, &session, "/api/extensions/ui").await;
    assert!(json_body(contributions)
        .await
        .as_array()
        .unwrap()
        .is_empty());
}

/// declarative `plugin.call` REST 往返：真实 Component guest（tests/call-guest），
/// 安装 → 启动 → call（回声校验 action/values 全链路）→ 未声明 action 拒绝 →
/// stop 后 call 冲突 → 卸载。
#[cfg(feature = "wasm-runtime")]
#[tokio::test]
async fn declarative_plugin_call_roundtrip_through_rest() {
    let test_app = build_app(
        "extensions-call",
        test_credential("admin123"),
        Default::default(),
    );
    let login_response = login(&test_app.app).await;
    let session = first_cookie_pair(&cookie_of(&login_response));

    let manifest = r#"manifest_version = 1
id = "com.example.panel"
version = "1.0.0"
name = "Panel extension"
entry = "plugin.wasm"

[ui]
[[ui.contributions]]
panel_id = "settings"
title = "设置"
runtime = "declarative"

[[ui.contributions.fields]]
type = "button"
label = "刷新"
action = "refresh"

[[ui.contributions.fields]]
type = "text"
name = "token"
label = "令牌"
default = "abc"
"#
    .as_bytes();
    let archive = craft_zip(vec![
        ("manifest.toml", manifest.to_vec()),
        ("plugin.wasm", call_guest_component()),
    ]);

    let installed = send(
        &test_app.app,
        req_bytes(
            "POST",
            "/api/extensions",
            None,
            &zip_headers(session.clone()),
            archive,
        ),
    )
    .await;
    assert_eq!(installed.status(), StatusCode::CREATED);

    // 安装即用（2026-09-05）：安装响应即 Running，无需再 enable/start。
    let installed_json = json_body(installed).await;
    assert_eq!(installed_json["state"], "running");

    let call = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.panel/call",
        serde_json::json!({ "action": "refresh", "values": { "token": "abc" } }),
    )
    .await;
    assert_eq!(call.status(), StatusCode::OK);
    let call_json = json_body(call).await;
    assert_eq!(call_json["echo"]["action"], "refresh");
    assert_eq!(call_json["echo"]["values"]["token"], "abc");

    // 未在 manifest declarative 按钮集合内声明的 action 一律拒绝（防任意入口）。
    let rejected = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.panel/call",
        serde_json::json!({ "action": "drop_tables", "values": {} }),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);

    let stopped = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.panel/stop",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(stopped.status(), StatusCode::OK);
    let after_stop = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.panel/call",
        serde_json::json!({ "action": "refresh", "values": {} }),
    )
    .await;
    assert_eq!(after_stop.status(), StatusCode::CONFLICT);

    let removed = send(
        &test_app.app,
        req(
            "DELETE",
            "/api/extensions/com.example.panel/1.0.0",
            None,
            &json_headers(session),
            None,
        ),
    )
    .await;
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
}

#[cfg(feature = "wasm-runtime")]
fn call_guest_component() -> Vec<u8> {
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::OnceLock;

    static COMPONENT: OnceLock<Vec<u8>> = OnceLock::new();
    COMPONENT
        .get_or_init(|| {
            let server_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let guest_dir = server_dir.join("tests").join("call-guest");
            let target_dir = server_dir.join("target").join("call-guest");
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let run = |args: &[&str]| {
                let mut command = Command::new(&cargo);
                command
                    .current_dir(&guest_dir)
                    .args(args)
                    .arg("--target-dir")
                    .arg(&target_dir);
                let output = command.output().unwrap_or_else(|error| {
                    panic!("无法启动 call guest cargo 子进程: {error}")
                });
                assert!(
                    output.status.success(),
                    "call guest 构建失败: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            };
            run(&["build", "--locked", "--quiet", "--release", "--lib", "--target", "wasm32-unknown-unknown"]);
            let module = target_dir
                .join("wasm32-unknown-unknown")
                .join("release")
                .join("gamer_call_fixture.wasm");
            let component_path = target_dir.join("gamer_call_fixture.component.wasm");
            // componentize（host bin）把 core module 封装为 WIT Component。
            let output = Command::new(&cargo)
                .current_dir(&guest_dir)
                .args([
                    "run",
                    "--locked",
                    "--quiet",
                    "--release",
                    "--bin",
                    "componentize",
                    "--target-dir",
                ])
                .arg(&target_dir)
                .arg("--")
                .arg(&module)
                .arg(&component_path)
                .output()
                .expect("无法启动 call guest componentize");
            assert!(
                output.status.success(),
                "call guest componentize 失败: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            std::fs::read(&component_path).expect("call guest Component 不存在")
        })
        .clone()
}

/// Phase 10 验收（官方市场端到端，使用提交进仓库的真实产物）：
/// 市场列表（web/public/registry.json）可见两个官方插件 → 产物 .gplugin 的
/// sha256 与 registry 一致 → 带 Registry proof 的官方安装（服务端验签：manifest
/// 签名走内嵌 dev 信任锚、proof 绑定 id/version/url/sha256）→ 权限确认 →
/// 启动 → UI 贡献出现 → 停止 → 卸载。
#[cfg(feature = "wasm-runtime")]
#[tokio::test]
async fn official_plugin_market_end_to_end_with_committed_artifacts() {
    use sha2::Digest as _;
    let repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("server 目录必有父级")
        .to_path_buf();
    let registry_bytes = std::fs::read(repo_root.join("web/public/registry.json"))
        .expect("web/public/registry.json 不存在；先运行 tools/build-plugins.ps1");
    let registry: serde_json::Value = serde_json::from_slice(&registry_bytes).unwrap();
    let plugins = registry["plugins"].as_array().unwrap().clone();
    let ids: Vec<&str> = plugins
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["gamer.keymap", "gamer.yaml"]);

    let test_app = build_app(
        "extensions-market",
        test_credential("admin123"),
        Default::default(),
    );
    let login_response = login(&test_app.app).await;
    let session = first_cookie_pair(&cookie_of(&login_response));

    for entry in &plugins {
        let id = entry["id"].as_str().unwrap();
        let download_url = entry["download_url"].as_str().unwrap();
        let sha256 = entry["sha256"].as_str().unwrap();
        let proof = entry["signature"]["value"].as_str().unwrap();

        // 市场产物可下载且哈希一致（web-dist 托管后即为同源 URL）。
        let artifact_path = repo_root
            .join("web/public")
            .join(download_url.trim_start_matches('/'));
        let artifact = std::fs::read(&artifact_path)
            .unwrap_or_else(|error| panic!("官方插件产物缺失 {:?}: {error}", artifact_path));
        let digest = format!("{:x}", sha2::Sha256::digest(&artifact));
        assert_eq!(digest, sha256, "{id} 产物 sha256 与 registry 不一致");

        // 官方安装：proof + 权限确认（服务端验证 manifest 签名与 proof 绑定）。
        let headers = vec![
            (header::COOKIE.to_string(), session.clone()),
            (header::CONTENT_TYPE.to_string(), "application/zip".into()),
            ("x-gamer-extension-source".to_string(), "official".into()),
            ("x-gamer-registry-proof".to_string(), proof.to_string()),
            ("x-gamer-permission-confirm".to_string(), "1".into()),
        ];
        let installed = send(
            &test_app.app,
            req_bytes("POST", "/api/extensions", None, &headers, artifact),
        )
        .await;
        assert_eq!(installed.status(), StatusCode::CREATED, "{id} 官方安装被拒绝");
        // 安装即用（2026-09-05）：官方安装自动 enable → start。keymap 长驻实例
        // 真实启动 → Running；gamer.yaml 为无实例模型（start 仅注册 timer
        // runner），测试装配未接 registrar 走通用实例路径失败 → 降级 Enabled
        // （生产 main.rs 接线 registrar 后即 Running）。
        let state = json_body(installed).await["state"].clone();
        if id == "gamer.yaml" {
            assert_eq!(state, "enabled", "{id} 安装后应降级为 Enabled");
        } else {
            assert_eq!(state, "running", "{id} 安装后应为 Running");
        }
    }

    // UI 贡献出现：keymaps + automation + functions。
    let ui = get_json(&test_app, &session, "/api/extensions/ui").await;
    let ui_json = json_body(ui).await;
    let panels: Vec<String> = ui_json
        .as_array()
        .unwrap()
        .iter()
        .map(|panel| panel["panel_id"].as_str().unwrap().to_string())
        .collect();
    assert!(panels.contains(&"keymaps".to_string()));
    assert!(panels.contains(&"automation".to_string()));
    assert!(panels.contains(&"functions".to_string()));

    for entry in &plugins {
        let id = entry["id"].as_str().unwrap();
        let version = entry["version"].as_str().unwrap();
        // 卸载守卫拒绝 Running：keymap 已在运行需先 stop；yaml 已降级 Enabled
        // 可直接删。
        if id == "gamer.keymap" {
            let stopped = post_json(
                &test_app,
                &session,
                &format!("/api/extensions/{id}/stop"),
                serde_json::json!({}),
            )
            .await;
            assert_eq!(stopped.status(), StatusCode::OK, "{id} 停止失败");
        }
        let removed = send(
            &test_app.app,
            req(
                "DELETE",
                &format!("/api/extensions/{id}/{version}"),
                None,
                &json_headers(session.clone()),
                None,
            ),
        )
        .await;
        assert_eq!(removed.status(), StatusCode::NO_CONTENT);
    }
}

/// 版本回滚 API（POST /api/extensions/:id/activate）契约：
/// 切换 active_version 指针返回 `{"id","active_version","state"}`；
/// 未安装版本 404；Running 409；切换后 start 链路（active manifest/UI 资产）
/// 使用新版本。
#[tokio::test]
async fn extension_activate_switches_version_404_missing_and_409_running() {
    let test_app = build_app(
        "extension-activate",
        test_credential("admin123"),
        Default::default(),
    );
    let login_response = login(&test_app.app).await;
    let session = first_cookie_pair(&cookie_of(&login_response));

    let manifest = |version: &str| -> String {
        format!(
            "manifest_version = 1\nid = \"com.example.rollback\"\nversion = \"{version}\"\nname = \"Rollback extension\"\nentry = \"plugin.wasm\"\n\n[[ui.contributions]]\npanel_id = \"rollback\"\ntitle = \"Rollback\"\nruntime = \"iframe\"\nentry = \"ui/index.html\"\n"
        )
    };
    let archive = |version: &str, content: &str| -> Vec<u8> {
        craft_zip(vec![
            ("manifest.toml", manifest(version).into_bytes()),
            ("plugin.wasm", b"\0asm\x01\0\0\0".to_vec()),
            ("ui/index.html", content.as_bytes().to_vec()),
        ])
    };

    let base = zip_headers(session.clone());
    let first = send(
        &test_app.app,
        req_bytes("POST", "/api/extensions", None, &base, archive("1.0.0", "v1 bytes")),
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);

    // 第二个版本并排安装，不自动激活
    let second = send(
        &test_app.app,
        req_bytes("POST", "/api/extensions", None, &base, archive("1.1.0", "v2 bytes")),
    )
    .await;
    assert_eq!(second.status(), StatusCode::CREATED);
    // 注：该插件 wasm 为桩字节，安装即用的自动 start 会失败并降级为 Enabled
    // （activate 拒绝 Running，此状态下本就无碍）。
    let installed = send(
        &test_app.app,
        req(
            "GET",
            "/api/extensions",
            None,
            &json_headers(session.clone()),
            None,
        ),
    )
    .await;
    let installed_json = json_body(installed).await;
    let rollback = installed_json["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "com.example.rollback")
        .unwrap()
        .clone();
    assert_eq!(rollback["active_version"], "1.0.0");
    assert_eq!(rollback["installed_versions"], serde_json::json!(["1.0.0", "1.1.0"]));

    // 切换 active_version：200 + 约定契约体
    let activated = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.rollback/activate",
        serde_json::json!({"version": "1.1.0"}),
    )
    .await;
    assert_eq!(activated.status(), StatusCode::OK);
    let activated_json = json_body(activated).await;
    assert_eq!(activated_json["id"], "com.example.rollback");
    assert_eq!(activated_json["active_version"], "1.1.0");
    // 激活保持生命周期状态（stop 后 = Enabled）
    assert_eq!(activated_json["state"], "enabled");

    // Enabled 的插件切换后保持 Enabled（下次 start 用新版本）
    let enabled = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.rollback/enable",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(enabled.status(), StatusCode::OK);

    // start/UI 链路使用新 active 版本：ui 资产来自 1.1.0 的文件
    let asset = get_json(
        &test_app,
        &session,
        "/api/extensions/com.example.rollback/ui/index.html",
    )
    .await;
    assert_eq!(asset.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(asset.into_body(), 1024).await.unwrap(),
        "v2 bytes"
    );

    let back = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.rollback/activate",
        serde_json::json!({"version": "1.0.0"}),
    )
    .await;
    assert_eq!(back.status(), StatusCode::OK);
    let back_json = json_body(back).await;
    assert_eq!(back_json["state"], "enabled");
    assert_eq!(back_json["active_version"], "1.0.0");

    // 回滚后 ui 资产同样回到 1.0.0 的文件（start 链路同理取 active manifest）
    let asset = get_json(
        &test_app,
        &session,
        "/api/extensions/com.example.rollback/ui/index.html",
    )
    .await;
    assert_eq!(asset.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(asset.into_body(), 1024).await.unwrap(),
        "v1 bytes"
    );

    // 未安装版本 → 404
    let missing = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.rollback/activate",
        serde_json::json!({"version": "9.9.9"}),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    // 非法版本串 → 400
    let malformed = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.rollback/activate",
        serde_json::json!({"version": "not-a-version"}),
    )
    .await;
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

    // Running → 409（直接落 state.json 模拟运行中：激活只读生命周期状态）
    let state_path = test_app.dir.join("extensions").join("state.json");
    std::fs::write(
        &state_path,
        serde_json::json!({
            "plugins": {
                "com.example.rollback": {
                    "id": "com.example.rollback",
                    "active_version": "1.0.0",
                    "state": "running",
                    "last_error": null
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    let conflict = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.rollback/activate",
        serde_json::json!({"version": "1.1.0"}),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
}

/// Phase 4/8 验收：无任何已安装扩展（含 gamer.yaml）时，基础设备控制 REST 与
/// capability 注册表完全可用——零业务资源发行基线不受扩展生命周期影响。
#[tokio::test]
async fn baseline_control_and_capabilities_work_without_installed_extensions() {
    let test_app = build_app(
        "extensions-none",
        test_credential("admin123"),
        Default::default(),
    );
    let login_response = login(&test_app.app).await;
    assert_eq!(login_response.status(), StatusCode::OK);
    let session = first_cookie_pair(&cookie_of(&login_response));

    // 扩展视图为空但接口正常：列表 / UI 贡献 / 管理聚合均可查询
    let list = get_json(&test_app, &session, "/api/extensions").await;
    assert_eq!(list.status(), StatusCode::OK);
    let list_json = json_body(list).await;
    assert_eq!(list_json["extensions"], serde_json::json!([]));
    assert_eq!(list_json["ui_contributions"], serde_json::json!([]));

    let ui = get_json(&test_app, &session, "/api/extensions/ui").await;
    assert_eq!(ui.status(), StatusCode::OK);
    assert!(json_body(ui).await.as_array().unwrap().is_empty());

    let management = get_json(&test_app, &session, "/api/extensions/management").await;
    assert_eq!(management.status(), StatusCode::OK);
    assert!(json_body(management).await["extensions"]
        .as_array()
        .unwrap()
        .is_empty());

    // 基础设备控制 REST 不受影响：路由/鉴权/状态机完好，未连接设备给出
    // 明确的设备级错误（409 设备未连接），而不是扩展/注册表类故障
    let control = post_json(
        &test_app,
        &session,
        "/api/devices/no-such-device/control",
        serde_json::json!({"type": "tap", "x": 0.5, "y": 0.5, "width": 1000, "height": 500}),
    )
    .await;
    assert_eq!(control.status(), StatusCode::CONFLICT);

    // capability 注册表在零扩展下完整注册（vision/device/frame 是纯 Core 能力）
    let cfg = crate::config::Config {
        data_dir: test_app.dir.clone(),
        ..Default::default()
    };
    let db: crate::store::Db = std::sync::Arc::new(crate::store::Store::open(&cfg).unwrap());
    let scripts = std::sync::Arc::new(crate::resources::ResourceStore::open(&cfg).unwrap());
    let devices = std::sync::Arc::new(crate::device::DeviceManager::new(
        db.clone(),
        cfg.clone(),
    ));
    let runs = std::sync::Arc::new(crate::run_manager::RunManager::new(
        std::sync::Arc::new(crate::extensions::gamer_yaml::runner_adapter::EngineExecutor::new(
            devices.clone(),
            db.clone(),
        )),
    ));
    let registry = crate::capabilities::adapters::build_registry(
        devices, scripts, db, runs,
    );
    assert!(registry.device().is_some(), "device capability 必须可用");
    assert!(registry.input().is_some(), "input capability 必须可用");
    assert!(registry.frame().is_some(), "frame capability 必须可用");
    assert!(registry.vision().is_some(), "vision capability 必须可用");
    assert!(registry.resource().is_some(), "resource capability 必须可用");
    assert!(registry.log().is_some(), "log capability 必须可用");
}
