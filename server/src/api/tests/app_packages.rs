// App Package REST 集成测试（include 于 api/tests.rs 的 sec_tests 模块内，
// 复用其 build_app / login / req_bytes / craft_zip 等装配助手）。
use super::*;

fn gamer_pkg_manifest(id: &str, version: &str, android: &str) -> Vec<u8> {
    format!(
        "format_version = 2\nid = \"{id}\"\nversion = \"{version}\"\n[android]\npackages = [\"{android}\"]\n"
    )
    .into_bytes()
}

fn preset_yaml(name: &str, expression: &str) -> Vec<u8> {
    format!(
        "name: {name}\nrunner_id: gamer.yaml\nentrypoint: run\npayload: {{}}\nschedule:\n  kind: cron\n  value:\n    expression: \"{expression}\"\n"
    )
    .into_bytes()
}

fn archive_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[tokio::test]
async fn app_package_install_list_activate_uninstall_lifecycle() {
    let test_app = build_app(
        "apppackages",
        test_credential("admin123"),
        Default::default(),
    );
    let login_response = login(&test_app.app).await;
    assert_eq!(login_response.status(), StatusCode::OK);
    let session = first_cookie_pair(&cookie_of(&login_response));

    let archive = craft_zip(vec![
        (
            "manifest.toml",
            gamer_pkg_manifest("official.a", "1.0.0", "com.example.game"),
        ),
        ("templates/icon.png", b"package-template".to_vec()),
        ("presets/daily.yaml", preset_yaml("daily", "0 8 * * *")),
    ]);

    // 错误的 X-Expected-Sha256 → 400，且不产生任何安装
    let mismatched = send(
        &test_app.app,
        req_bytes(
            "POST",
            "/api/app-packages/install",
            None,
            &[
                (header::COOKIE.to_string(), session.clone()),
                (header::CONTENT_TYPE.to_string(), "application/zip".into()),
                ("X-Expected-Sha256".to_string(), "0".repeat(64)),
            ],
            archive.clone(),
        ),
    )
    .await;
    assert_eq!(mismatched.status(), StatusCode::BAD_REQUEST);
    assert!(json_body(mismatched).await["error"]
        .as_str()
        .unwrap()
        .contains("SHA-256"));

    // 正确摘要 → 201 + 自动激活 + 包内预设发布
    let expected = archive_sha256(&archive);
    let installed = send(
        &test_app.app,
        req_bytes(
            "POST",
            "/api/app-packages/install",
            None,
            &[
                (header::COOKIE.to_string(), session.clone()),
                (header::CONTENT_TYPE.to_string(), "application/zip".into()),
                ("X-Expected-Sha256".to_string(), expected),
            ],
            archive,
        ),
    )
    .await;
    assert_eq!(installed.status(), StatusCode::CREATED);
    let installed_json = json_body(installed).await;
    assert_eq!(installed_json["id"], "official.a");
    assert_eq!(installed_json["active_version"], "1.0.0");
    assert!(installed_json["versions"][0]["sha256"].is_string());

    // 列表：active 版本、android targets、每版本 sha256
    let list = get_json(&test_app, &session, "/api/app-packages").await;
    assert_eq!(list.status(), StatusCode::OK);
    let list_json = json_body(list).await;
    assert_eq!(list_json["packages"].as_array().unwrap().len(), 1);
    assert_eq!(list_json["packages"][0]["android_packages"][0], "com.example.game");
    assert_eq!(list_json["packages"][0]["versions"][0]["version"], "1.0.0");

    // 包内预设已灌入任务预设（按来源包查询）
    let presets = get_json(&test_app, &session, "/api/task-presets?app_package=official.a").await;
    assert_eq!(presets.status(), StatusCode::OK);
    let presets_json = json_body(presets).await;
    assert_eq!(presets_json.as_array().unwrap().len(), 1);
    assert_eq!(presets_json[0]["name"], "daily");
    assert_eq!(presets_json[0]["app_package"], "official.a");
    assert_eq!(presets_json[0]["schedule"]["provider_id"], "cron");

    // 新版本安装 → 自动切换 active（旧版本保留可回滚）
    let upgraded = craft_zip(vec![(
        "manifest.toml",
        gamer_pkg_manifest("official.a", "1.1.0", "com.example.game"),
    )]);
    let upgraded = send(
        &test_app.app,
        req_bytes(
            "POST",
            "/api/app-packages/install",
            None,
            &zip_headers(session.clone()),
            upgraded,
        ),
    )
    .await;
    assert_eq!(upgraded.status(), StatusCode::CREATED);
    let list = get_json(&test_app, &session, "/api/app-packages").await;
    let list_json = json_body(list).await;
    assert_eq!(list_json["packages"][0]["active_version"], "1.1.0");
    assert_eq!(list_json["packages"][0]["versions"].as_array().unwrap().len(), 2);

    // 与已激活包 android targets 冲突的第二内容包 → 409（未安装）
    let conflicting = craft_zip(vec![(
        "manifest.toml",
        gamer_pkg_manifest("official.b", "2.0.0", "com.example.game"),
    )]);
    let conflict = send(
        &test_app.app,
        req_bytes(
            "POST",
            "/api/app-packages/install",
            None,
            &zip_headers(session.clone()),
            conflicting,
        ),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert!(json_body(conflict).await["error"]
        .as_str()
        .unwrap()
        .contains("com.example.game"));

    // 不冲突的包可并存
    let other = craft_zip(vec![(
        "manifest.toml",
        gamer_pkg_manifest("official.b", "2.0.0", "com.other.game"),
    )]);
    let other = send(
        &test_app.app,
        req_bytes(
            "POST",
            "/api/app-packages/install",
            None,
            &zip_headers(session.clone()),
            other,
        ),
    )
    .await;
    assert_eq!(other.status(), StatusCode::CREATED);

    // 显式激活旧版本
    let activate = post_json(
        &test_app,
        &session,
        "/api/app-packages/official.a/activate",
        serde_json::json!({"version": "1.0.0"}),
    )
    .await;
    assert_eq!(activate.status(), StatusCode::OK);
    assert_eq!(json_body(activate).await["active_version"], "1.0.0");

    // 激活未安装版本 → 404
    let activate = post_json(
        &test_app,
        &session,
        "/api/app-packages/official.a/activate",
        serde_json::json!({"version": "9.9.9"}),
    )
    .await;
    assert_eq!(activate.status(), StatusCode::NOT_FOUND);

    // 卸载单个版本：非 active 的 1.1.0 直接移除
    let removed = send(
        &test_app.app,
        req(
            "DELETE",
            "/api/app-packages/official.a/1.1.0",
            None,
            &json_headers(session.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);

    // 卸载 active 的 1.0.0：预设记录保留，包从列表消失
    let removed = send(
        &test_app.app,
        req(
            "DELETE",
            "/api/app-packages/official.a/1.0.0",
            None,
            &json_headers(session.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
    let presets = get_json(&test_app, &session, "/api/task-presets?app_package=official.a").await;
    assert_eq!(json_body(presets).await.as_array().unwrap().len(), 1);

    // 重复卸载 → 404
    let removed = send(
        &test_app.app,
        req(
            "DELETE",
            "/api/app-packages/official.a/1.0.0",
            None,
            &json_headers(session.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(removed.status(), StatusCode::NOT_FOUND);

    let list = get_json(&test_app, &session, "/api/app-packages").await;
    let list_json = json_body(list).await;
    assert_eq!(list_json["packages"].as_array().unwrap().len(), 1);
    assert_eq!(list_json["packages"][0]["id"], "official.b");
}
