use super::*;

#[tokio::test]
async fn mcp_remote_access_is_hot_applied_persistent_and_separate_from_admin_auth() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        data_dir: dir.path().into(),
        source_path: Some(dir.path().join("config.toml")),
        ..Default::default()
    };
    std::fs::write(cfg.source_path.as_ref().unwrap(), toml::to_string(&cfg).unwrap()).unwrap();
    let t = build_app_with_config(cfg.clone(), test_credential("admin123"), cfg.auth.clone());
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let created = json_body(post_json(&t, &sid, "/api/mcp/tokens", serde_json::json!({"label":"remote fixture"})).await).await;
    let id = created["metadata"]["token_id"].as_str().unwrap();
    let mcp_headers = vec![
        ("Host".into(), "192.0.2.10:18443".into()),
        ("Content-Type".into(), JSON_CT.into()),
        ("Accept".into(), "application/json, text/event-stream".into()),
        ("Authorization".into(), format!("Bearer {}", created["token"].as_str().unwrap())),
    ];
    let message = |method: &str, headers: &[(String, String)]| req("POST", "/api/mcp", Some("192.0.2.20:33333"), headers,
        Some(serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":{}}).to_string()));
    assert_eq!(send(&t.app, message("ping", &mcp_headers)).await.status(), StatusCode::FORBIDDEN);

    let initial = cfg.settings_view().unwrap();
    let mut values = initial.saved;
    values.mcp_allow_remote = true;
    let saved = cfg.save_settings(values, &initial.revision).unwrap();
    assert!(!saved.restart_required);
    let restarted: Config = toml::from_str(&std::fs::read_to_string(cfg.source_path.as_ref().unwrap()).unwrap()).unwrap();
    assert!(restarted.mcp_allow_remote);
    let catalog = json_body(get_json(&t, &sid, "/api/mcp/catalog").await).await;
    assert_eq!(catalog["remote_allowed"], true);
    assert_eq!(catalog["endpoint_path"], "/api/mcp");
    assert_eq!(send(&t.app, message("ping", &mcp_headers)).await.status(), StatusCode::OK);
    let listed = json_body(send(&t.app, message("tools/list", &mcp_headers)).await).await;
    assert!(listed["result"]["tools"].as_array().is_some());

    // A remote MCP credential never logs into the management API.
    assert_eq!(send(&t.app, req("GET", "/api/tasks", Some("192.0.2.20:33333"), &mcp_headers, None)).await.status(), StatusCode::UNAUTHORIZED);
    let mut cookie_only = mcp_headers.clone();
    cookie_only.retain(|(name, _)| name != "Authorization");
    cookie_only.push(("Cookie".into(), sid.clone()));
    cookie_only.push(("X-Admin-Token".into(), "not-a-mcp-token".into()));
    let mut pending = message("ping", &cookie_only);
    *pending.body_mut() = Body::from_stream(futures_util::stream::pending::<Result<axum::body::Bytes, std::io::Error>>());
    let rejected = tokio::time::timeout(Duration::from_secs(2), send(&t.app, pending)).await.unwrap();
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    let mut cross_origin = mcp_headers.clone();
    cross_origin.push(("Origin".into(), "https://attacker.invalid".into()));
    assert_eq!(send(&t.app, message("ping", &cross_origin)).await.status(), StatusCode::FORBIDDEN);

    let mut values = saved.saved;
    values.mcp_allow_remote = false;
    cfg.save_settings(values, &saved.revision).unwrap();
    assert_eq!(send(&t.app, message("ping", &mcp_headers)).await.status(), StatusCode::FORBIDDEN);
    let mut local_headers = mcp_headers.clone();
    local_headers[0].1 = "127.0.0.1:8443".into();
    let local = || req("POST", "/api/mcp", Some("127.0.0.1:33333"), &local_headers,
        Some(serde_json::json!({"jsonrpc":"2.0","id":2,"method":"ping"}).to_string()));
    assert_eq!(send(&t.app, local()).await.status(), StatusCode::OK);
    assert_eq!(send(&t.app, req("DELETE", &format!("/api/mcp/tokens/{id}"), None, &json_headers(sid), None)).await.status(), StatusCode::OK);
    let current = cfg.settings_view().unwrap();
    let mut values = current.saved;
    values.mcp_allow_remote = true;
    cfg.save_settings(values, &current.revision).unwrap();
    assert_eq!(send(&t.app, message("ping", &mcp_headers)).await.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn local_only_listener_also_blocks_remote_mcp_when_opted_in() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config { data_dir: dir.path().into(), local_only: true, mcp_allow_remote: true, ..Default::default() };
    let t = build_app_with_config(cfg.clone(), test_credential("admin123"), cfg.auth.clone());
    let headers = vec![("Host".into(), "192.0.2.10:18443".into())];
    assert_eq!(send(&t.app, req("POST", "/api/mcp", Some("192.0.2.20:1"), &headers, None)).await.status(), StatusCode::FORBIDDEN);
}

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
