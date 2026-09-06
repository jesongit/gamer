use super::*;

// Dormant 插件数据（plan §5.2、§6、§14）：Package 可携带当前未安装/未启用
// 插件的数据目录——导入成功、数据原样保留（不被解释、不被清理）；后续安装
// 并 start 该插件后直接使用已有数据。Core 对未知插件目录零迁移零修剪。

const DORMANT_KEYMAP_YAML: &str = "version: 1\nname: dormant\nbindings:\n  - key: KeyW\n    action:\n      type: hold\n      at: [0.25, 0.75]\n";

/// 构造含 dormant 插件数据的 .gamerpkg：
/// - `plugins/gamer.keymap/mappings/`（keymap 扩展未安装时的既有方案数据）
/// - `plugins/vendor.ocr.example/`（Core 完全不认识的第三方插件数据）
fn dormant_package_archive() -> Vec<u8> {
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
async fn dormant_plugin_data_survives_import_duplicate_and_serves_later_start() {
    let t = build_app(
        "pkgdormant",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // 前置：keymap 扩展未安装（dormant 状态成立的前提）
    let extensions = get_json(&t, &sid, "/api/extensions").await;
    assert_eq!(extensions.status(), StatusCode::OK);
    let listed = json_body(extensions).await["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["id"] == "gamer.keymap");
    assert!(!listed, "测试前置失败：gamer.keymap 不应已安装");

    // 导入含 dormant 插件数据的包 → 201（plan §5.1/§5.2：缺插件允许导入）
    let imported = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import",
            None,
            &zip_headers(sid.clone()),
            dormant_package_archive(),
        ),
    )
    .await;
    assert_eq!(imported.status(), StatusCode::CREATED, "{}", json_body(imported).await);
    let manifest = json_body(imported).await;
    assert_eq!(manifest["id"], "official.hsr.daily");
    let plugins = manifest["plugins"].as_array().unwrap();
    assert!(
        plugins.contains(&serde_json::json!({"id": "gamer.keymap", "required": false})),
        "manifest 应保留 optional 依赖声明: {plugins:?}"
    );

    // 数据保留：盘上原样（不被解释、不被删）
    let dormant_yaml = t
        .dir
        .join("packages/official.hsr.daily/plugins/gamer.keymap/mappings/dormant.yaml");
    assert!(dormant_yaml.is_file(), "dormant keymap 数据必须原样保留");
    let unknown_bin = t
        .dir
        .join("packages/official.hsr.daily/plugins/vendor.ocr.example/data/table.bin");
    assert!(unknown_bin.is_file(), "未知插件目录必须原样保留");
    assert_eq!(
        std::fs::read(&unknown_bin).unwrap(),
        b"\x00\x01unknown-plugin-bytes",
        "未知插件字节内容不得被改写"
    );

    // 数据可经通用资源 API 读回（Core 内容无关透传，不解释不迁移）
    let read_back = get_json(
        &t,
        &sid,
        "/api/packages/official.hsr.daily/plugins/gamer.keymap/resources/mappings/dormant.yaml",
    )
    .await;
    assert_eq!(read_back.status(), StatusCode::OK);
    let detail = json_body(read_back).await;
    assert!(detail["content"].as_str().unwrap().contains("type: hold"));
    let unknown_list = get_json(
        &t,
        &sid,
        "/api/packages/official.hsr.daily/plugins/vendor.ocr.example/resources",
    )
    .await;
    assert_eq!(unknown_list.status(), StatusCode::OK);
    let entries = json_body(unknown_list).await["resources"].as_array().unwrap().clone();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["path"], "data/table.bin");

    // 列资源（read-only）之后 dormant 目录仍然存在（无隐式清理）
    assert!(dormant_yaml.is_file() && unknown_bin.is_file());

    // 复制包：dormant 数据跟随深拷贝
    let duplicated = post_json(
        &t,
        &sid,
        "/api/packages/official.hsr.daily/duplicate",
        serde_json::json!({"new_id": "user.hsr.custom"}),
    )
    .await;
    assert_eq!(duplicated.status(), StatusCode::CREATED, "{}", json_body(duplicated).await);
    assert!(t
        .dir
        .join("packages/user.hsr.custom/plugins/gamer.keymap/mappings/dormant.yaml")
        .is_file());
    assert!(t
        .dir
        .join("packages/user.hsr.custom/plugins/vendor.ocr.example/data/table.bin")
        .is_file());

    // 后续消费：PackageStore 隔离资源域可直接读出既有方案（插件安装前的
    // host 侧读取路径，与 keymap start 的 profile 加载同源）
    let store = crate::resources::PackageStore::open(&crate::config::Config {
        data_dir: t.dir.clone(),
        ..Default::default()
    })
    .unwrap();
    let profile = crate::extensions::load_user_profile(&store, "official.hsr.daily", "dormant")
        .expect("dormant 数据必须可被后续安装的插件直接使用");
    assert!(profile.contains("KeyW"));

    // 删除包 → 该包整体（含未知插件目录）消失（显式删除语义，非隐式清理）
    let removed = send(
        &t.app,
        req(
            "DELETE",
            "/api/packages/user.hsr.custom",
            None,
            &json_headers(sid),
            None,
        ),
    )
    .await;
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
    assert!(!t.dir.join("packages/user.hsr.custom").exists());
}

/// 安装 gamer.keymap 后直接消费导入时带来的 dormant 数据（plan §6「后续安装
/// 插件后自动恢复对应能力」）：start 携带 `{package_id, android_package}` 双
/// 字段上下文，profile 从既有 `mappings/` 数据加载成功。
#[cfg(feature = "wasm-runtime")]
#[tokio::test]
async fn installed_keymap_plugin_starts_directly_on_dormant_data() {
    let t = build_app(
        "pkgdormantstart",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // 导入含 dormant keymap 数据的包（复用归档夹具）
    let imported = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import",
            None,
            &zip_headers(sid.clone()),
            dormant_package_archive(),
        ),
    )
    .await;
    assert_eq!(imported.status(), StatusCode::CREATED);

    // 安装 keymap 插件（fixture guest；安装即用自动 start → Running）
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
    assert_eq!(json_body(installed).await["state"], "running");

    // 安装即用已 Running：先停回 Enabled，再以 dormant 数据上下文启动
    let stopped = post_json(&t, &sid, "/api/extensions/gamer.keymap/stop", serde_json::json!({})).await;
    assert_eq!(stopped.status(), StatusCode::OK);

    // 已有 dormant 数据直接可用：package_id 数据上下文 + 方案名 → Running。
    // 若导入/后续路径清理过 mappings/，此 start 会因「keymap 方案不存在」失败。
    let started = post_json(
        &t,
        &sid,
        "/api/extensions/gamer.keymap/start",
        serde_json::json!({
            "app_context": {
                "device_id": "device-1",
                "android_package": "com.miHoYo.hkrpg",
                "content_package": "official.hsr.daily",
            },
            "profile": "dormant",
        }),
    )
    .await;
    assert_eq!(started.status(), StatusCode::OK, "{}", json_body(started).await);
    assert_eq!(json_body(started).await["state"], "running");
    let stopped = post_json(&t, &sid, "/api/extensions/gamer.keymap/stop", serde_json::json!({})).await;
    assert_eq!(stopped.status(), StatusCode::OK);

    // 数据上下文缺失 → 400（android_package 不再承担数据分区语义）
    let missing_context = post_json(
        &t,
        &sid,
        "/api/extensions/gamer.keymap/start",
        serde_json::json!({
            "app_context": {
                "device_id": "device-1",
                "android_package": "official.hsr.daily",
            },
            "profile": "dormant",
        }),
    )
    .await;
    assert_eq!(missing_context.status(), StatusCode::BAD_REQUEST);
    assert!(json_body(missing_context).await["error"]
        .as_str()
        .unwrap()
        .contains("content_package"));

    // package_id 指向不存在方案的包 → 启动失败（缺数据是显式错误，非静默降级）
    let _ = post_json(&t, &sid, "/api/packages", serde_json::json!({"id": "empty.pkg"})).await;
    let no_scheme = post_json(
        &t,
        &sid,
        "/api/extensions/gamer.keymap/start",
        serde_json::json!({
            "app_context": {
                "device_id": "device-1",
                "android_package": "com.miHoYo.hkrpg",
                "content_package": "empty.pkg",
            },
            "profile": "dormant",
        }),
    )
    .await;
    assert_eq!(no_scheme.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(json_body(no_scheme).await["error"]
        .as_str()
        .unwrap()
        .contains("keymap 方案不存在"));

    // 卸载插件不触碰 Package 数据（dormant 数据回归保留态）
    let uninstalled = send(
        &t.app,
        req(
            "DELETE",
            "/api/extensions/gamer.keymap/1.0.0",
            None,
            &json_headers(sid),
            None,
        ),
    )
    .await;
    assert_eq!(uninstalled.status(), StatusCode::NO_CONTENT);
    assert!(t
        .dir
        .join("packages/official.hsr.daily/plugins/gamer.keymap/mappings/dormant.yaml")
        .is_file());
}
