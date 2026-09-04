// 工作区元数据 + App Package 导出 REST 集成测试（include 于 api/tests.rs 的
// sec_tests 模块内，复用其 build_app / login / valid_template_png 等装配助手）。
use super::*;

const EXPORT_URI: &str = "/api/app-packages/export";

fn export_body(android: &str) -> serde_json::Value {
    serde_json::json!({ "android_package": android })
}

/// PUT JSON（工作区元数据端点是 PUT 语义；harness 原生只有 get/post 助手）。
async fn put_json(
    t: &TestApp,
    sid: &str,
    uri: &str,
    body: serde_json::Value,
) -> HttpResponse<Body> {
    send(
        &t.app,
        req(
            "PUT",
            uri,
            None,
            &json_headers(sid.to_string()),
            Some(body.to_string()),
        ),
    )
    .await
}

fn sha256_of(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

fn export_preset_yaml(name: &str, expression: &str) -> Vec<u8> {
    format!(
        "name: {name}\nrunner_id: gamer.yaml\nentrypoint: run\npayload: {{}}\nschedule:\n  kind: cron\n  value:\n    expression: \"{expression}\"\n"
    )
    .into_bytes()
}

/// 在 TestApp 数据目录下布置一个完整合法的工作区（六目录各一文件）。
fn seed_workspace(dir: &std::path::Path, android: &str) {
    let ws = dir.join(android);
    std::fs::create_dir_all(ws.join("scripts")).unwrap();
    std::fs::write(ws.join("scripts/daily.yaml"), b"steps: []\n").unwrap();
    std::fs::create_dir_all(ws.join("functions")).unwrap();
    std::fs::write(ws.join("functions/common.yaml"), b"login:\n  steps: []\n").unwrap();
    std::fs::create_dir_all(ws.join("templates")).unwrap();
    std::fs::write(ws.join("templates/icon.png"), valid_template_png()).unwrap();
    std::fs::create_dir_all(ws.join("keymaps")).unwrap();
    std::fs::write(
        ws.join("keymaps/wasd.yaml"),
        b"version: 1\nname: wasd\nbindings: []\n",
    )
    .unwrap();
    std::fs::create_dir_all(ws.join("presets")).unwrap();
    std::fs::write(
        ws.join("presets/daily.yaml"),
        export_preset_yaml("daily", "0 8 * * *"),
    )
    .unwrap();
    std::fs::create_dir_all(ws.join("resources")).unwrap();
    std::fs::write(ws.join("resources/config.json"), b"{}").unwrap();
}

#[tokio::test]
async fn workspace_get_reports_missing_metadata_and_zero_stats() {
    let test_app = build_app(
        "wsgetempty",
        test_credential("admin123"),
        Default::default(),
    );
    let session = first_cookie_pair(&cookie_of(&login(&test_app.app).await));

    // 未初始化工作区：metadata 为 null、stats 全 0
    let resp = get_json(&test_app, &session, "/api/workspace/com.example.game").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert!(body["metadata"].is_null());
    for kind in ["scripts", "functions", "templates", "keymaps", "presets", "resources"] {
        assert_eq!(body["stats"][kind], 0, "{kind} 应计 0");
    }

    // 非法 android 包名 → 400
    let resp = get_json(&test_app, &session, "/api/workspace/not%20a%20pkg").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn workspace_put_creates_updates_and_rejects_invalid_metadata() {
    let test_app = build_app(
        "wsput",
        test_credential("admin123"),
        Default::default(),
    );
    let session = first_cookie_pair(&cookie_of(&login(&test_app.app).await));

    // 创建：id 缺省取路径上的 android 包名
    let created = put_json(
        &test_app,
        &session,
        "/api/workspace/com.example.game",
        serde_json::json!({ "version": "1.0.0", "android_packages": ["com.example.game"] }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK, "{:?}", json_body(created).await);
    let body = json_body(created).await;
    assert_eq!(body["metadata"]["format_version"], 2);
    assert_eq!(body["metadata"]["id"], "com.example.game");
    assert_eq!(body["metadata"]["version"], "1.0.0");
    assert!(body["metadata"]["name"].is_null());
    assert_eq!(body["metadata"]["android_packages"][0], "com.example.game");
    // package.toml 已原子落盘
    assert!(test_app.dir.join("com.example.game/package.toml").is_file());

    // 更新：显式 id + name
    let updated = put_json(
        &test_app,
        &session,
        "/api/workspace/com.example.game",
        serde_json::json!({
            "id": "official.demo",
            "version": "1.1.0",
            "name": "演示包",
            "android_packages": ["com.example.game", "com.example.game2"]
        }),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let body = json_body(updated).await;
    assert_eq!(body["metadata"]["id"], "official.demo");
    assert_eq!(body["metadata"]["name"], "演示包");
    assert_eq!(body["metadata"]["android_packages"].as_array().unwrap().len(), 2);

    // GET 返回同一份元数据
    let got = get_json(&test_app, &session, "/api/workspace/com.example.game").await;
    let body = json_body(got).await;
    assert_eq!(body["metadata"]["id"], "official.demo");
    assert_eq!(body["metadata"]["version"], "1.1.0");

    // 校验失败 → 400：version 必须含数字
    let bad_version = put_json(
        &test_app,
        &session,
        "/api/workspace/com.example.game",
        serde_json::json!({ "version": "v-abc", "android_packages": ["com.example.game"] }),
    )
    .await;
    assert_eq!(bad_version.status(), StatusCode::BAD_REQUEST);

    // 校验失败 → 400：android_packages 不能为空 / 含非法包名 / 重复
    for packages in [
        serde_json::json!([]),
        serde_json::json!(["com..bad"]),
        serde_json::json!(["com.example.game", "com.example.game"]),
    ] {
        let resp = put_json(
            &test_app,
            &session,
            "/api/workspace/com.example.game",
            serde_json::json!({ "version": "1.0.0", "android_packages": packages }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{:?}", json_body(resp).await);
    }

    // 校验失败 → 400：非法 id
    let bad_id = put_json(
        &test_app,
        &session,
        "/api/workspace/com.example.game",
        serde_json::json!({ "id": "bad id!", "version": "1.0.0", "android_packages": ["com.example.game"] }),
    )
    .await;
    assert_eq!(bad_id.status(), StatusCode::BAD_REQUEST);

    // 路径参数非法 → 400
    let bad_path = put_json(
        &test_app,
        &session,
        "/api/workspace/bad%20pkg",
        serde_json::json!({ "version": "1.0.0", "android_packages": ["com.example.game"] }),
    )
    .await;
    assert_eq!(bad_path.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn export_happy_path_returns_reinstallable_archive() {
    let test_app = build_app(
        "exportok",
        test_credential("admin123"),
        Default::default(),
    );
    let session = first_cookie_pair(&cookie_of(&login(&test_app.app).await));

    // 未初始化元数据 → 404
    let missing = post_json(&test_app, &session, EXPORT_URI, export_body("com.example.game")).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert!(
        json_body(missing).await["error"].as_str().unwrap().contains("package.toml"),
        "404 消息应提示先初始化元数据"
    );

    // 布置工作区 + 初始化元数据
    seed_workspace(&test_app.dir, "com.example.game");
    let init = put_json(
        &test_app,
        &session,
        "/api/workspace/com.example.game",
        serde_json::json!({
            "id": "official.demo",
            "version": "1.0.0",
            "android_packages": ["com.example.game"]
        }),
    )
    .await;
    assert_eq!(init.status(), StatusCode::OK);

    // 导出 → 200 二进制 + 响应头断言
    let resp = send(
        &test_app.app,
        req(
            "POST",
            EXPORT_URI,
            None,
            &json_headers(session.clone()),
            Some(export_body("com.example.game").to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()["content-type"],
        "application/octet-stream",
        "Content-Type 必须是 application/octet-stream"
    );
    let disposition = resp
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(
        disposition, "attachment; filename=\"official.demo-1.0.0.gamerpkg\"",
        "Content-Disposition 必须是 attachment + id-version.gamerpkg"
    );
    let sha_header = resp
        .headers()
        .get("x-content-sha256")
        .expect("X-Content-Sha256 响应头缺失")
        .to_str()
        .unwrap()
        .to_string();
    let body = axum::body::to_bytes(resp.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    assert_eq!(sha_header, sha256_of(&body), "X-Content-Sha256 必须等于归档字节摘要");
    assert_eq!(&body[..2], b"PK", "响应体必须是 zip 归档");

    // round-trip：导出产物可直接经安装 API 安装并自动激活
    let installed = send(
        &test_app.app,
        req_bytes(
            "POST",
            "/api/app-packages/install",
            None,
            &[
                (header::COOKIE.to_string(), session.clone()),
                (header::CONTENT_TYPE.to_string(), "application/zip".into()),
                ("X-Expected-Sha256".to_string(), sha_header),
            ],
            body,
        ),
    )
    .await;
    assert_eq!(installed.status(), StatusCode::CREATED, "{:?}", json_body(installed).await);
    let installed_json = json_body(installed).await;
    assert_eq!(installed_json["id"], "official.demo");
    assert_eq!(installed_json["active_version"], "1.0.0");
}

#[tokio::test]
async fn export_preflight_failure_reports_all_problems_with_code() {
    let test_app = build_app(
        "exportbad",
        test_credential("admin123"),
        Default::default(),
    );
    let session = first_cookie_pair(&cookie_of(&login(&test_app.app).await));

    let ws = test_app.dir.join("com.example.game");
    std::fs::create_dir_all(ws.join("scripts")).unwrap();
    std::fs::write(ws.join("scripts/daily.yaml"), b"steps: 42\n").unwrap();
    std::fs::create_dir_all(ws.join("templates")).unwrap();
    std::fs::write(ws.join("templates/icon.png"), b"not a png").unwrap();
    let init = put_json(
        &test_app,
        &session,
        "/api/workspace/com.example.game",
        serde_json::json!({ "version": "1.0.0", "android_packages": ["com.example.game"] }),
    )
    .await;
    assert_eq!(init.status(), StatusCode::OK);

    let resp = post_json(&test_app, &session, EXPORT_URI, export_body("com.example.game")).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = json_body(resp).await;
    assert_eq!(body["code"], "preflight_failed", "preflight 失败必须带机器码");
    let message = body["error"].as_str().unwrap();
    // 多问题一次报全，且消息带文件定位
    assert!(message.contains("scripts/daily.yaml"), "{message}");
    assert!(message.contains("templates/icon.png"), "{message}");
}

#[tokio::test]
async fn export_rejects_invalid_android_package_and_unknown_fields() {
    let test_app = build_app(
        "exportrej",
        test_credential("admin123"),
        Default::default(),
    );
    let session = first_cookie_pair(&cookie_of(&login(&test_app.app).await));

    // 非法 android 包名 → 400
    let bad = post_json(
        &test_app,
        &session,
        EXPORT_URI,
        serde_json::json!({ "android_package": "bad pkg!" }),
    )
    .await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

    // deny_unknown_fields：未知字段 → 4xx（axum Json 提取拒绝）
    let unknown = post_json(
        &test_app,
        &session,
        EXPORT_URI,
        serde_json::json!({ "android_package": "com.example.game", "extra": 1 }),
    )
    .await;
    assert!(unknown.status().is_client_error());
}

#[tokio::test]
async fn workspace_and_export_require_login() {
    let test_app = build_app(
        "wsauth",
        test_credential("admin123"),
        Default::default(),
    );
    let resp = send(
        &test_app.app,
        req(
            "GET",
            "/api/workspace/com.example.game",
            None,
            &[],
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp = send(
        &test_app.app,
        req(
            "POST",
            EXPORT_URI,
            None,
            &[],
            Some(export_body("com.example.game").to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
