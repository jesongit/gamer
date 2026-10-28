use super::*;

#[tokio::test]
async fn mcp_settings_create_view_delete_require_admin_and_permanent_token_is_revoked() {
    let t = build_app("mcp-simple", test_credential("admin123"), Default::default());
    for (method, uri) in [("GET", "/api/mcp/tokens"), ("GET", "/api/mcp/tokens/example"), ("DELETE", "/api/mcp/tokens/example")] {
        assert_eq!(send(&t.app, req(method, uri, None, &[], None)).await.status(), StatusCode::UNAUTHORIZED);
    }
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let created = post_json(&t, &sid, "/api/mcp/tokens", serde_json::json!({"label":"fixture","expires_in_seconds":0})).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(created.headers()[header::CACHE_CONTROL], "no-store");
    let created = json_body(created).await;
    let secret = created["token"].as_str().unwrap();
    let id = created["metadata"]["token_id"].as_str().unwrap();
    assert_eq!(created["metadata"]["expires_at"], 0);
    assert_eq!(created["metadata"]["allowed"]["devices"], serde_json::json!(["*"]));
    let listed = json_body(get_json(&t, &sid, "/api/mcp/tokens").await).await;
    assert_eq!(listed["tokens"][0]["can_view"], true);
    assert!(!listed.to_string().contains(secret));
    let url = format!("/api/mcp/tokens/{id}");
    let revealed = get_json(&t, &sid, &url).await;
    assert_eq!(revealed.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(json_body(revealed).await["token"], secret);

    let mcp_headers = vec![
        ("Host".into(), "127.0.0.1:8443".into()),
        ("Content-Type".into(), "application/json".into()),
        ("Accept".into(), "application/json, text/event-stream".into()),
        ("Authorization".into(), format!("Bearer {secret}")),
    ];
    let ping = || req("POST", "/api/mcp", Some("127.0.0.1:33333"), &mcp_headers,
        Some(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"ping"}).to_string()));
    assert_eq!(send(&t.app, ping()).await.status(), StatusCode::OK);
    let deleted = send(&t.app, req("DELETE", &url, None, &json_headers(sid.clone()), None)).await;
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(json_body(deleted).await["deleted"], true);
    assert!(json_body(get_json(&t, &sid, "/api/mcp/tokens").await).await["tokens"].as_array().unwrap().is_empty());
    assert_eq!(get_json(&t, &sid, &url).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(send(&t.app, ping()).await.status(), StatusCode::UNAUTHORIZED);
}
