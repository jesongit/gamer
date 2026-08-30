use super::*;

const TEST_PASSWORD: &str = "test-password";
const AUTH_ADMIN_JSON: &str = r#"{"username":"admin","password":"test-password"}"#;

fn test_credential(password: &str) -> auth::Credential {
    auth::parse_password_hash(&auth::hash_password(password).unwrap()).unwrap()
}

async fn login(app: &Router) -> HttpResponse<Body> {
    send_json_login(app, None, AUTH_ADMIN_JSON).await
}

#[tokio::test]
async fn unauthenticated_devices_list_is_401() {
    let t = build_app(
        "401devs",
        test_credential(TEST_PASSWORD),
        Default::default(),
    );
    let resp = send(&t.app, req("GET", "/api/devices", None, &[], None)).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let j = json_body(resp).await;
    assert_eq!(j["error"], "unauthorized");
}

#[tokio::test]
async fn unauthenticated_tasks_list_is_401() {
    let t = build_app(
        "401tasks",
        test_credential(TEST_PASSWORD),
        Default::default(),
    );
    let resp = send(&t.app, req("GET", "/api/tasks", None, &[], None)).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let j = json_body(resp).await;
    assert_eq!(j["error"], "unauthorized");
}

#[tokio::test]
async fn unauthenticated_shutdown_is_401_and_service_stays_alive() {
    let t = build_app("401sd", test_credential(TEST_PASSWORD), Default::default());
    let resp = send(&t.app, req("POST", "/api/shutdown", None, &[], None)).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // 进程仍存活：后续请求正常应答
    let alive = send(&t.app, req("GET", "/health/live", None, &[], None)).await;
    assert_eq!(alive.status(), StatusCode::OK);
}

#[tokio::test]
async fn unauthenticated_high_risk_endpoints_are_all_401() {
    let t = build_app(
        "401highrisk",
        test_credential(TEST_PASSWORD),
        Default::default(),
    );
    let cases = [
        ("POST", "/api/shutdown"),
        ("POST", "/api/devices/missing/control"),
        ("POST", "/api/scripts/missing/run"),
        ("POST", "/api/scripts/missing/stop"),
        ("DELETE", "/api/templates/missing?pkg=com.test.app"),
        ("POST", "/api/scripts/import?pkg=com.test.app"),
    ];
    for (method, uri) in cases {
        let resp = send(&t.app, req(method, uri, None, &[], None)).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
        let body = json_body(resp).await;
        assert_eq!(body["error"], "unauthorized", "{method} {uri}");
    }
}

#[tokio::test]
async fn maintenance_vacuum_requires_auth_and_reports_file_sizes() {
    let t = build_app("vacuum", test_credential(TEST_PASSWORD), Default::default());
    // 未登录 → 401（受保护维护动作，与 /api/shutdown 同守卫）
    let resp = send(
        &t.app,
        req("POST", "/api/maintenance/vacuum", None, &[], None),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(resp).await["error"], "unauthorized");

    // 登录后 → 200，返回 vacuum 前后数据库文件字节数（均 > 0）
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/maintenance/vacuum",
            None,
            &[(header::COOKIE.to_string(), sid)],
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_body(resp).await;
    assert!(j["before_bytes"].is_u64(), "{j}");
    assert!(j["after_bytes"].is_u64(), "{j}");
    assert!(j["before_bytes"].as_u64().unwrap() > 0);
    assert!(j["after_bytes"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn login_sets_cookie_with_contract_attributes() {
    let t = build_app("cookie", test_credential(TEST_PASSWORD), Default::default());
    let resp = login(&t.app).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ck = cookie_of(&resp);
    assert!(ck.starts_with("gb_session="), "{ck}");
    assert!(
        !first_cookie_pair(&ck)[11..].trim().is_empty(),
        "session id 非空: {ck}"
    );
    assert!(ck.contains("Path=/"), "{ck}");
    assert!(ck.contains("HttpOnly"), "{ck}");
    assert!(ck.contains("SameSite=Strict"), "{ck}");
    assert!(
        !ck.contains("Secure"),
        "dev profile 不加 Secure 保证纯 HTTP LAN 可用: {ck}"
    );
    let j = json_body(resp).await;
    assert_eq!(j["ok"], true);
    assert_eq!(j["username"], "admin");
}

#[tokio::test]
async fn wrong_password_gives_invalid_credentials() {
    let t = build_app("badpw", test_credential(TEST_PASSWORD), Default::default());
    let resp = send_json_login(&t.app, None, r#"{"username":"admin","password":"nope"}"#).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let j = json_body(resp).await;
    assert_eq!(j["error"], "invalid_credentials");
}

#[tokio::test]
async fn consecutive_failures_trigger_429_too_many_attempts() {
    let cfg = crate::config::AuthConfig {
        login_max_fails: 3,
        login_window_secs: 300,
        ..Default::default()
    };
    let t = build_app("rl429", test_credential(TEST_PASSWORD), cfg);
    for i in 0..3 {
        let resp = send_json_login(
            &t.app,
            Some("203.0.113.7:5555"),
            r#"{"username":"admin","password":"wrong"}"#,
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "第{i}次失败应为401"
        );
    }
    // 正确口令在锁定期同样拒绝
    let resp = send_json_login(&t.app, Some("203.0.113.7:5555"), AUTH_ADMIN_JSON).await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(resp.headers().contains_key(header::RETRY_AFTER));
    let j = json_body(resp).await;
    assert_eq!(j["error"], "too_many_attempts");
    assert!(j["retry_after"].as_u64().unwrap_or(0) >= 1);
}

#[tokio::test]
async fn login_rate_limit_is_scoped_to_ip_and_username_pair() {
    let cfg = crate::config::AuthConfig {
        login_max_fails: 2,
        login_window_secs: 300,
        ..Default::default()
    };
    let t = build_app("rlpair", test_credential(TEST_PASSWORD), cfg);

    // 同 IP 的诱饵用户名锁定后，admin 仍能登录。
    for _ in 0..2 {
        let resp = send_json_login(
            &t.app,
            Some("203.0.113.30:4000"),
            r#"{"username":"decoy","password":"wrong"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
    let decoy = send_json_login(
        &t.app,
        Some("203.0.113.30:4000"),
        r#"{"username":"decoy","password":"test-password"}"#,
    )
    .await;
    assert_eq!(decoy.status(), StatusCode::TOO_MANY_REQUESTS);
    let admin = send_json_login(&t.app, Some("203.0.113.30:4000"), AUTH_ADMIN_JSON).await;
    assert_eq!(admin.status(), StatusCode::OK);

    // admin 在一个 IP 锁定后，另一 IP 仍可登录。
    for _ in 0..2 {
        let resp = send_json_login(
            &t.app,
            Some("203.0.113.31:4000"),
            r#"{"username":"admin","password":"wrong"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
    let locked = send_json_login(&t.app, Some("203.0.113.31:4000"), AUTH_ADMIN_JSON).await;
    assert_eq!(locked.status(), StatusCode::TOO_MANY_REQUESTS);
    let other_ip = send_json_login(&t.app, Some("203.0.113.32:4000"), AUTH_ADMIN_JSON).await;
    assert_eq!(other_ip.status(), StatusCode::OK);
}

#[tokio::test]
async fn session_probe_and_logout_semantics() {
    let t = build_app("sess", test_credential(TEST_PASSWORD), Default::default());

    // 未认证探测 → 401 unauthorized
    let resp = send(&t.app, req("GET", "/api/session", None, &[], None)).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 登录拿 cookie → 探测通过且回身份
    let ck = cookie_of(&login(&t.app).await);
    let sid = first_cookie_pair(&ck);
    let resp = send(
        &t.app,
        req(
            "GET",
            "/api/session",
            None,
            &[(header::COOKIE.to_string(), sid.clone())],
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_body(resp).await;
    assert_eq!(j["authenticated"], true);
    assert_eq!(j["username"], "admin");

    // 登出 → 204 + 过期 Cookie；旧 cookie 立即失效
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/logout",
            None,
            &[(header::COOKIE.to_string(), sid.clone())],
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let clear_ck = cookie_of(&resp);
    assert!(
        clear_ck.contains("Max-Age=0") && clear_ck.starts_with("gb_session="),
        "{clear_ck}"
    );
    let resp = send(
        &t.app,
        req(
            "GET",
            "/api/devices",
            None,
            &[(header::COOKIE.to_string(), sid)],
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 登出幂等：无/坏 cookie 再登出仍 204
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/logout",
            None,
            &[(header::COOKIE.to_string(), "gb_session=deadbeef".into())],
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = send(&t.app, req("POST", "/api/logout", None, &[], None)).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn expired_cookie_is_rejected_by_protected_route() {
    // abs 用 5s 窗口而非 1s：与 session_lifecycle 同因——并行负载下
    // login→首请求→sleep 的调度抖动可能超 1s，窗口太紧会误判未过期/过期翻转
    let cfg = crate::config::AuthConfig {
        session_abs_secs: 5,
        session_idle_secs: 60,
        ..Default::default()
    };
    let t = build_app("expired-route", test_credential(TEST_PASSWORD), cfg);
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    let before = send(
        &t.app,
        req(
            "GET",
            "/api/devices",
            None,
            &[(header::COOKIE.to_string(), sid.clone())],
            None,
        ),
    )
    .await;
    assert_eq!(before.status(), StatusCode::OK);

    tokio::time::sleep(Duration::from_millis(5_200)).await;
    let after = send(
        &t.app,
        req(
            "GET",
            "/api/devices",
            None,
            &[(header::COOKIE.to_string(), sid)],
            None,
        ),
    )
    .await;
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(after).await["error"], "unauthorized");
}

#[tokio::test]
async fn authentication_logs_rejection_metadata_without_secrets() {
    let t = build_app(
        "safe-auth-log",
        test_credential(TEST_PASSWORD),
        Default::default(),
    );
    let password = "log-secret-password-7a8b";
    let cookie = "gb_session=log-secret-cookie-9c0d";
    let bearer = "Bearer log-secret-authorization-1e2f";
    let capture = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(capture.clone())
        .finish();

    let (login_resp, protected_resp) = async {
        let login_resp = send_json_login(
            &t.app,
            Some("203.0.113.40:4000"),
            &format!(r#"{{"username":"admin","password":"{password}"}}"#),
        )
        .await;
        let protected_resp = send(
            &t.app,
            req(
                "GET",
                "/api/devices",
                None,
                &[
                    (header::COOKIE.to_string(), cookie.into()),
                    (header::AUTHORIZATION.to_string(), bearer.into()),
                ],
                None,
            ),
        )
        .await;
        (login_resp, protected_resp)
    }
    .with_subscriber(subscriber)
    .await;

    assert_eq!(login_resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(protected_resp.status(), StatusCode::UNAUTHORIZED);
    let logs = capture.text();
    assert!(logs.contains("authentication rejected"), "{logs}");
    assert!(logs.contains("outcome=\"unauthorized\""), "{logs}");
    for secret in [password, cookie, bearer] {
        assert!(!logs.contains(secret), "敏感值进入日志: {secret}: {logs}");
    }
}

#[tokio::test]
async fn cross_origin_login_and_logout_are_rejected_without_state_change() {
    let t = build_app(
        "csrf-public",
        test_credential(TEST_PASSWORD),
        Default::default(),
    );
    let evil = [
        (header::ORIGIN.to_string(), "https://evil.example".into()),
        (header::HOST.to_string(), "localhost:8443".into()),
        (header::CONTENT_TYPE.to_string(), JSON_CT.into()),
    ];
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/login",
            None,
            &evil,
            Some(AUTH_ADMIN_JSON.into()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let cookie = cookie_of(&login(&t.app).await);
    let sid = first_cookie_pair(&cookie);
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/logout",
            None,
            &[
                (header::COOKIE.to_string(), sid.clone()),
                (header::ORIGIN.to_string(), "https://evil.example".into()),
                (header::HOST.to_string(), "localhost:8443".into()),
            ],
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // 被拒绝的跨源 logout 不能销毁会话。
    let resp = send(
        &t.app,
        req(
            "GET",
            "/api/session",
            None,
            &[(header::COOKIE.to_string(), sid)],
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn cross_origin_state_change_is_403_but_matching_origin_passes() {
    let t = build_app("origin", test_credential(TEST_PASSWORD), Default::default());
    let ck = cookie_of(&login(&t.app).await);
    let sid = first_cookie_pair(&ck);

    // 外站 Origin 打状态变更接口（已带合法会话）→ 403 forbidden_origin
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/devices/scan",
            None,
            &[
                (header::COOKIE.to_string(), sid.clone()),
                (header::ORIGIN.to_string(), "http://evil.example".into()),
                (header::HOST.to_string(), "localhost:8443".into()),
            ],
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let j = json_body(resp).await;
    assert_eq!(j["error"], "forbidden_origin");

    // 同源 Origin + 会话 → 通过 guard 进入处理器（设备不存在 → 非 4xx 卫兵错）
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/devices/nope/control",
            None,
            &[
                (header::COOKIE.to_string(), sid),
                (header::ORIGIN.to_string(), "http://localhost:8443".into()),
                (header::HOST.to_string(), "localhost:8443".into()),
                (header::CONTENT_TYPE.to_string(), "application/json".into()),
            ],
            Some(r#"{"type":"home"}"#.into()),
        ),
    )
    .await;
    assert_ne!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn ws_upgrade_without_cookie_rejected_before_handshake() {
    let t = build_app("wsauth", test_credential(TEST_PASSWORD), Default::default());
    // 无 cookie 的 WS 升级：guard 在握手前 401（无需真实建连）
    let resp = send(
        &t.app,
        req(
            "GET",
            "/ws/device/d1",
            None,
            &[
                (header::UPGRADE.to_string(), "websocket".into()),
                (header::CONNECTION.to_string(), "Upgrade".into()),
            ],
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // 完整同源握手头 + 合法 cookie 应通过 guard 进入 WS extractor/处理器。
    let ck = cookie_of(&login(&t.app).await);
    let sid = first_cookie_pair(&ck);
    let resp = send(
        &t.app,
        req(
            "GET",
            "/ws/device/d1",
            None,
            &[
                (header::COOKIE.to_string(), sid.clone()),
                (header::UPGRADE.to_string(), "websocket".into()),
                (header::CONNECTION.to_string(), "Upgrade".into()),
                ("sec-websocket-version".into(), "13".into()),
                (
                    "sec-websocket-key".into(),
                    "dGhlIHNhbXBsZSBub25jZQ==".into(),
                ),
                (header::ORIGIN.to_string(), "http://localhost:8443".into()),
                (header::HOST.to_string(), "localhost:8443".into()),
            ],
            None,
        ),
    )
    .await;
    assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(resp.status(), StatusCode::FORBIDDEN);

    // 合法会话不能让跨站页面借 WS Upgrade 绕过 Origin 校验。
    let resp = send(
        &t.app,
        req(
            "GET",
            "/ws/device/d1",
            None,
            &[
                (header::COOKIE.to_string(), sid.clone()),
                (header::UPGRADE.to_string(), "websocket".into()),
                (header::CONNECTION.to_string(), "Upgrade".into()),
                (header::ORIGIN.to_string(), "https://evil.example".into()),
                (header::HOST.to_string(), "localhost:8443".into()),
            ],
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Origin 存在但 Host 缺失也必须拒绝，避免把畸形握手当作同源。
    let resp = send(
        &t.app,
        req(
            "GET",
            "/ws/device/d1",
            None,
            &[
                (header::COOKIE.to_string(), sid),
                (header::UPGRADE.to_string(), "websocket".into()),
                (header::CONNECTION.to_string(), "Upgrade".into()),
                (header::ORIGIN.to_string(), "https://localhost:8443".into()),
            ],
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn cross_origin_high_risk_endpoints_are_all_403_after_authentication() {
    let t = build_app(
        "403highrisk",
        test_credential(TEST_PASSWORD),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let cases = [
        ("POST", "/api/shutdown", None),
        (
            "POST",
            "/api/devices/missing/control",
            Some(r#"{"type":"home"}"#),
        ),
        (
            "POST",
            "/api/scripts/missing/run",
            Some(r#"{"device_id":"d1"}"#),
        ),
        ("POST", "/api/scripts/missing/stop", None),
        ("DELETE", "/api/templates/missing?pkg=com.test.app", None),
        (
            "POST",
            "/api/scripts/import?pkg=com.test.app",
            Some("not-a-zip"),
        ),
    ];
    for (method, uri, body) in cases {
        let mut headers = vec![
            (header::COOKIE.to_string(), sid.clone()),
            (header::ORIGIN.to_string(), "https://evil.example".into()),
            (header::HOST.to_string(), "localhost:8443".into()),
        ];
        if body.is_some() {
            headers.push((header::CONTENT_TYPE.to_string(), JSON_CT.into()));
        }
        let resp = send(
            &t.app,
            req(method, uri, None, &headers, body.map(str::to_string)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{method} {uri}");
        assert_eq!(json_body(resp).await["error"], "forbidden_origin");
    }
}

#[tokio::test]
async fn loopback_admin_token_channel_open_close() {
    let t = build_app(
        "admintok",
        test_credential(TEST_PASSWORD),
        Default::default(),
    );

    // 回环 + 正确 token → 放行执行 shutdown（测试栈里 viewers/devices 为空，安全）
    let ok = send(
        &t.app,
        req(
            "POST",
            "/api/shutdown",
            Some("127.0.0.1:33333"),
            &[(
                super::auth::ADMIN_TOKEN_HEADER.to_string(),
                "test-token".into(),
            )],
            None,
        ),
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);
    let j = json_body(ok).await;
    assert_eq!(j["ok"], true);
    // 回环通道放行后 shutdown 已触发 watch 信号——router 本身仍活着
    let alive = send(&t.app, req("GET", "/health/live", None, &[], None)).await;
    assert_eq!(alive.status(), StatusCode::OK);
}

#[tokio::test]
async fn non_loopback_same_token_is_401() {
    let t = build_app("lanrej", test_credential(TEST_PASSWORD), Default::default());
    for addr in ["192.168.1.50:40000", "10.1.2.3:8443"] {
        let resp = send(
            &t.app,
            req(
                "POST",
                "/api/shutdown",
                Some(addr),
                &[(
                    super::auth::ADMIN_TOKEN_HEADER.to_string(),
                    "test-token".into(),
                )],
                None,
            ),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "{addr} 同 token 必须拒绝"
        );
    }
    // 无 token 头也拒
    let resp = send(
        &t.app,
        req("POST", "/api/shutdown", Some("127.0.0.1:1"), &[], None),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn wrong_token_even_loopback_is_401() {
    let t = build_app("badtok", test_credential(TEST_PASSWORD), Default::default());
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/shutdown",
            Some("127.0.0.1:22222"),
            &[(super::auth::ADMIN_TOKEN_HEADER.to_string(), "nope".into())],
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn config_hash_is_the_only_persisted_credential_format() {
    use crate::config::Config;

    let mut cfg = Config::default();
    cfg.auth.password_hash = auth::hash_password("hashed-pw").unwrap();
    let st2 = auth::AuthState::new(
        auth::resolve_credential_for_profile(&cfg, crate::config::Profile::Prod),
        Default::default(),
        false,
        None,
    );
    assert_eq!(st2.credential_source(), "config:password_hash");
    assert!(st2.verify_credentials("hashed-pw"));
    assert!(!st2.verify_credentials("test-password"));

    cfg.auth.password_hash = "sha256$00112233445566778899aabb$deadbeef".into();
    assert!(matches!(
        auth::resolve_credential_for_profile(&cfg, crate::config::Profile::Prod),
        auth::Credential::Unavailable
    ));
}

#[tokio::test]
async fn unconfigured_credentials_fail_closed() {
    let t = build_app(
        "no-credential",
        auth::Credential::Unavailable,
        Default::default(),
    );
    let resp = login(&t.app).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(resp).await["error"], "invalid_credentials");
}

// ---------- Wave 2：输入与资源限额（SEC-004） ----------
