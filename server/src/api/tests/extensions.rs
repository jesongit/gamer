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
    assert_eq!(installed_json["state"], "installed");

    let contributions = get_json(&test_app, &session, "/api/extensions/ui").await;
    assert_eq!(contributions.status(), StatusCode::OK);
    assert!(json_body(contributions).await.as_array().unwrap().is_empty());

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
    assert_eq!(axum::body::to_bytes(asset.into_body(), 1024).await.unwrap(), "<h1>hello</h1>");

    let disabled = post_json(
        &test_app,
        &session,
        "/api/extensions/com.example.extension/disable",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(disabled.status(), StatusCode::OK);
    let contributions = get_json(&test_app, &session, "/api/extensions/ui").await;
    assert!(json_body(contributions).await.as_array().unwrap().is_empty());
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
    let removed = send(
        &test_app.app,
        req(
            "DELETE",
            "/api/extensions/com.example.extension/1.0.0",
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
}
