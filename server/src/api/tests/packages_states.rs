use super::*;

// plan §17（targets 兼容性检查，warning 语义）与 §18（Plugin Dependency
// 状态词表 available/missing_required/missing_optional/disabled/unknown）。
// 归档夹具与 packages_dormant.rs 同构（optional 声明 + 盘上数据）；因
// include! 模块彼此不可见，此处独立构造。

const DORMANT_KEYMAP_YAML: &str = "version: 1\nname: dormant\nbindings:\n  - key: KeyW\n    action:\n      type: hold\n      at: [0.25, 0.75]\n";

fn dormant_states_archive() -> Vec<u8> {
    craft_zip(vec![
        (
            "package.toml",
            r#"id = "official.hsr.daily"
name = "Dormant fixture"
version = "1.0.0"

[targets.android]
packages = ["com.miHoYo.hkrpg"]

[plugins."gamer.keymap"]
required = false

[plugins."vendor.ocr.example"]
required = false
"#
            .as_bytes()
            .to_vec(),
        ),
        (
            "plugins/gamer.keymap/mappings/dormant.yaml",
            DORMANT_KEYMAP_YAML.as_bytes().to_vec(),
        ),
        (
            "plugins/vendor.ocr.example/data/table.bin",
            b"\x00\x01unknown-plugin-bytes".to_vec(),
        ),
    ])
}

#[tokio::test]
async fn compatibility_endpoint_warns_without_blocking() {
    let t = build_app(
        "pkgcompat",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    let created = post_json(
        &t,
        &sid,
        "/api/packages",
        serde_json::json!({
            "id": "official.hsr.daily",
            "targets": {
                "android": {"packages": ["com.miHoYo.hkrpg", "com.HoYoverse.hkrpgoversea"]},
            },
        }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED, "{}", json_body(created).await);

    // 命中声明目标 → 兼容
    let hit = get_json(
        &t,
        &sid,
        "/api/packages/official.hsr.daily/compatibility?android_package=com.miHoYo.hkrpg",
    )
    .await;
    assert_eq!(hit.status(), StatusCode::OK);
    let body = json_body(hit).await;
    assert_eq!(body["compatible"], true);
    assert_eq!(body["android_package"], "com.miHoYo.hkrpg");
    assert_eq!(body["android_targets"].as_array().unwrap().len(), 2);

    // 未声明目标 → compatible=false（仅 warning，不禁止使用：第三方版本/渠道服）
    let miss = get_json(
        &t,
        &sid,
        "/api/packages/official.hsr.daily/compatibility?android_package=com.other.channel",
    )
    .await;
    assert_eq!(miss.status(), StatusCode::OK);
    assert_eq!(json_body(miss).await["compatible"], false);

    // 0 个声明 target = 通用包，恒兼容
    let generic = post_json(
        &t,
        &sid,
        "/api/packages",
        serde_json::json!({ "id": "user.toolkit" }),
    )
    .await;
    assert_eq!(generic.status(), StatusCode::CREATED);
    let any = get_json(
        &t,
        &sid,
        "/api/packages/user.toolkit/compatibility?android_package=com.anything.else",
    )
    .await;
    assert_eq!(json_body(any).await["compatible"], true);

    // 缺 android_package 参数 → 400；包不存在 → 404
    let no_param = get_json(&t, &sid, "/api/packages/user.toolkit/compatibility").await;
    assert_eq!(no_param.status(), StatusCode::BAD_REQUEST);
    let gone = get_json(
        &t,
        &sid,
        "/api/packages/nope.pkg/compatibility?android_package=com.x",
    )
    .await;
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn package_detail_reports_plugin_dependency_states() {
    let t = build_app(
        "pkgstates",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // required 声明 + 扩展未安装 → missing_required
    let created = post_json(
        &t,
        &sid,
        "/api/packages",
        serde_json::json!({ "id": "user.req", "plugins": { "gamer.yaml": true } }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    // 导入 dormant 包：gamer.keymap / vendor.ocr.example 均 optional 声明
    let imported = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import",
            None,
            &zip_headers(sid.clone()),
            dormant_states_archive(),
        ),
    )
    .await;
    assert_eq!(imported.status(), StatusCode::CREATED, "{}", json_body(imported).await);

    // 盘上未声明插件目录 → unknown（经资源 API 写入 ghost.plugin 数据）
    let ghost = put_package_text(
        &t,
        &sid,
        "official.hsr.daily",
        "ghost.plugin",
        "blob.txt",
        "undeclared plugin data",
    )
    .await;
    assert_eq!(ghost.status(), StatusCode::OK, "{}", json_body(ghost).await);

    async fn states_of(t: &TestApp, sid: &str, pkg: &str) -> Vec<serde_json::Value> {
        let detail = get_json(t, sid, &format!("/api/packages/{pkg}")).await;
        assert_eq!(detail.status(), StatusCode::OK, "{}", json_body(detail).await);
        json_body(detail).await["plugin_states"]
            .as_array()
            .unwrap()
            .clone()
    }
    fn find<'a>(states: &'a [serde_json::Value], plugin: &str) -> &'a serde_json::Value {
        states
            .iter()
            .find(|s| s["plugin"] == plugin)
            .unwrap_or_else(|| panic!("plugin_states 缺少 {plugin}: {states:?}"))
    }

    let states = states_of(&t, &sid, "user.req").await;
    assert_eq!(find(&states, "gamer.yaml")["state"], "missing_required");
    assert_eq!(find(&states, "gamer.yaml")["required"], true);

    let states = states_of(&t, &sid, "official.hsr.daily").await;
    assert_eq!(find(&states, "gamer.keymap")["state"], "missing_optional");
    assert_eq!(find(&states, "gamer.keymap")["required"], false);
    assert_eq!(find(&states, "vendor.ocr.example")["state"], "missing_optional");
    assert_eq!(find(&states, "ghost.plugin")["state"], "unknown");
    assert_eq!(find(&states, "ghost.plugin")["required"], serde_json::Value::Null);
}

/// 安装并停用 keymap 扩展后 → disabled；运行中 → available（§18 Disabled 态）。
#[cfg(feature = "wasm-runtime")]
#[tokio::test]
async fn plugin_states_track_installed_extension_lifecycle() {
    let t = build_app(
        "pkgstateslife",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    let imported = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import",
            None,
            &zip_headers(sid.clone()),
            dormant_states_archive(),
        ),
    )
    .await;
    assert_eq!(imported.status(), StatusCode::CREATED);

    // 安装即自动 start → Running → available
    let component = crate::extensions::build_guest_fixture_component();
    let gplugin = crate::extensions::package_guest_fixture_gplugin(
        &component,
        &["input.tap", "input.swipe", "input.key", "touch"],
    );
    let mut headers = zip_headers(sid.clone());
    headers.push(("x-gamer-permission-confirm".to_string(), "1".to_string()));
    let installed = send(
        &t.app,
        req_bytes("POST", "/api/extensions", None, &headers, gplugin),
    )
    .await;
    assert_eq!(installed.status(), StatusCode::CREATED, "{}", json_body(installed).await);

    let detail = get_json(&t, &sid, "/api/packages/official.hsr.daily").await;
    let states = json_body(detail).await["plugin_states"].as_array().unwrap().clone();
    let entry = states
        .iter()
        .find(|s| s["plugin"] == "gamer.keymap")
        .unwrap();
    assert_eq!(entry["state"], "available");

    // disable（运行中自动 stop → Disabled）→ 插件状态 disabled
    let disabled = post_json(&t, &sid, "/api/extensions/gamer.keymap/disable", serde_json::json!({})).await;
    assert_eq!(disabled.status(), StatusCode::OK);
    let detail = get_json(&t, &sid, "/api/packages/official.hsr.daily").await;
    let states = json_body(detail).await["plugin_states"].as_array().unwrap().clone();
    let entry = states
        .iter()
        .find(|s| s["plugin"] == "gamer.keymap")
        .unwrap();
    assert_eq!(entry["state"], "disabled");
}
