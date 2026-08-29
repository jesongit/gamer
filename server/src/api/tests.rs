//! Direct API integration and validation tests.

use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::device::DeviceManager;
use crate::matcher;
use crate::scheduler::Scheduler;
use crate::scripts::ScriptStore;
use crate::store::{Db, Device, ScreenMode};
use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use base64::Engine;

use super::common::{BODY_LIMIT_JSON, BODY_LIMIT_ZIP_IMPORT};
use super::devices::{
    parse_ctl, session_affecting_change, validate_device_req, ControlReq, CreateDeviceReq,
};
use super::logs::clamp_log_limit;
use super::runs::{validate_run_script_req, RunScriptReq};
use super::tasks::{validate_task_req, SaveTaskReq};
use super::templates::validate_template_name;
use super::{auth, build_router, ApiError};

// ---------- 集成测试（阶段 2 SEC 验收矩阵自动化子集） ----------
//
// 走真实 build_router 全栈（DeviceManager 只构造不 start——无 adb 扫描副作用；
// Store 用临时目录 sqlite），请求经 tower oneshot 直驱，ConnectInfo 以扩展注入
// 模拟来源地址。WS 场景以"升级前被 guard 拒绝"断言（真握手过繁，见 auth.rs 决策内核注释）。

#[cfg(test)]
mod sec_tests {
    use super::*;
    use axum::extract::ConnectInfo;
    use axum::http::{Request as HttpRequest, Response as HttpResponse};
    use std::io::Write as _;
    use std::net::SocketAddr;
    use tower::ServiceExt;
    use tracing::instrument::WithSubscriber as _;

    #[derive(Clone, Default)]
    struct CapturedLogs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    struct CapturedWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CapturedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
        type Writer = CapturedWriter;

        fn make_writer(&'a self) -> Self::Writer {
            CapturedWriter(self.0.clone())
        }
    }

    impl CapturedLogs {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    struct TestApp {
        app: Router,
        /// 保活 shutdown 接收端，模拟 main 的优雅退出监听
        _shutdown_rx: tokio::sync::watch::Receiver<bool>,
        #[allow(dead_code)]
        dir: std::path::PathBuf,
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "gamer-apitest-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn build_app(
        tag: &str,
        credential: auth::Credential,
        mut auth_cfg: crate::config::AuthConfig,
    ) -> TestApp {
        // 测试专用会话 TTL 缺省（生产默认 12h/2h 太长，无法实测过期）
        if auth_cfg.session_abs_secs == 12 * 3600 {
            auth_cfg.session_abs_secs = 3600;
        }
        if auth_cfg.session_idle_secs == 2 * 3600 {
            auth_cfg.session_idle_secs = 1800;
        }
        let dir = tmp_dir(tag);
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let db: Db = Arc::new(crate::store::Store::open(&cfg).unwrap());
        let scripts = Arc::new(ScriptStore::open(&cfg).unwrap());
        let viewers: crate::webrtc::ViewerMap =
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let devices = Arc::new(DeviceManager::new(db.clone(), cfg.clone(), viewers.clone()));
        // 生产执行器装配（设备离线时 prepare 即失败，正好覆盖"连接失败锁释放"路径）
        let executor = Arc::new(crate::run_manager::EngineExecutor::new(
            Arc::new(crate::engine::Runner::new(
                devices.clone(),
                viewers.clone(),
                scripts.clone(),
            )),
            devices.clone(),
            db.clone(),
        ));
        assemble_app(
            db, devices, cfg, scripts, viewers, credential, auth_cfg, executor,
        )
    }

    /// 注入自定义执行器的装配（仲裁层 HTTP 集成测试用假执行器；其余与 build_app 相同）
    #[cfg(test)]
    #[allow(dead_code)]
    fn build_app_with_executor(
        tag: &str,
        credential: auth::Credential,
        mut auth_cfg: crate::config::AuthConfig,
        executor: Arc<dyn crate::run_manager::RunExecutor>,
    ) -> TestApp {
        if auth_cfg.session_abs_secs == 12 * 3600 {
            auth_cfg.session_abs_secs = 3600;
        }
        if auth_cfg.session_idle_secs == 2 * 3600 {
            auth_cfg.session_idle_secs = 1800;
        }
        let dir = tmp_dir(tag);
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let db: Db = Arc::new(crate::store::Store::open(&cfg).unwrap());
        let scripts = Arc::new(ScriptStore::open(&cfg).unwrap());
        let viewers: crate::webrtc::ViewerMap =
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let devices = Arc::new(DeviceManager::new(db.clone(), cfg.clone(), viewers.clone()));
        assemble_app(
            db, devices, cfg, scripts, viewers, credential, auth_cfg, executor,
        )
    }

    /// 公共装配体：RunManager + Scheduler + Router（TestApp 持有临时目录负责清理边界注释）
    #[allow(clippy::too_many_arguments)]
    fn assemble_app(
        db: Db,
        devices: Arc<DeviceManager>,
        cfg: Config,
        scripts: Arc<ScriptStore>,
        viewers: crate::webrtc::ViewerMap,
        credential: auth::Credential,
        auth_cfg: crate::config::AuthConfig,
        executor: Arc<dyn crate::run_manager::RunExecutor>,
    ) -> TestApp {
        let runs = Arc::new(crate::run_manager::RunManager::new(executor));
        let scheduler = Arc::new(Scheduler::new(db.clone(), scripts.clone(), runs.clone()));
        let auth = Arc::new(auth::AuthState::new(
            credential,
            auth_cfg,
            false,
            Some("test-token".into()),
        ));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let dir = cfg.data_dir.clone();
        let app = build_router(
            db,
            devices,
            runs,
            scheduler,
            cfg,
            viewers,
            scripts,
            shutdown_tx,
            auth.clone(),
        );
        TestApp {
            app,
            _shutdown_rx: shutdown_rx,
            dir,
        }
    }

    fn req(
        method: &str,
        uri: &str,
        remote: Option<&str>,
        headers: &[(String, String)],
        body: Option<String>,
    ) -> HttpRequest<Body> {
        let mut b = HttpRequest::builder().method(method).uri(uri);
        if let Some(r) = remote {
            b = b.extension(ConnectInfo::<SocketAddr>(r.parse().unwrap()));
        }
        for (k, v) in headers {
            b = b.header(k.as_str(), v);
        }
        match body {
            Some(s) => b.body(Body::from(s)).unwrap(),
            None => b.body(Body::empty()).unwrap(),
        }
    }

    fn req_bytes(
        method: &str,
        uri: &str,
        remote: Option<&str>,
        headers: &[(String, String)],
        body: Vec<u8>,
    ) -> HttpRequest<Body> {
        let mut b = HttpRequest::builder().method(method).uri(uri);
        if let Some(r) = remote {
            b = b.extension(ConnectInfo::<SocketAddr>(r.parse().unwrap()));
        }
        for (k, v) in headers {
            b = b.header(k.as_str(), v);
        }
        b.body(Body::from(body)).unwrap()
    }

    async fn send(app: &Router, r: HttpRequest<Body>) -> HttpResponse<Body> {
        app.clone().oneshot(r).await.unwrap()
    }

    fn cookie_of(resp: &HttpResponse<Body>) -> String {
        resp.headers()
            .get(header::SET_COOKIE)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    }

    fn first_cookie_pair(set_cookie: &str) -> String {
        set_cookie
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    }

    async fn json_body(resp: HttpResponse<Body>) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    const JSON_CT: &str = "application/json";
    const ADMIN_JSON: &str = r#"{"username":"admin","password":"admin123"}"#;

    fn craft_zip(entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, data) in entries {
                zw.start_file(name, opts).unwrap();
                zw.write_all(&data).unwrap();
            }
            zw.finish().unwrap();
        }
        buf
    }

    fn pixel_bomb_png(width: u32, height: u32) -> Vec<u8> {
        fn crc32(data: &[u8]) -> u32 {
            let mut crc: u32 = 0xFFFF_FFFF;
            for &b in data {
                crc ^= b as u32;
                for _ in 0..8 {
                    let mask = (crc & 1).wrapping_neg();
                    crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
                }
            }
            !crc
        }
        let mut out = vec![137, 80, 78, 71, 13, 10, 26, 10];
        let ihdr = [
            13u32.to_be_bytes().as_slice(),
            b"IHDR".as_slice(),
            &width.to_be_bytes()[..],
            &height.to_be_bytes()[..],
            &[8u8, 0, 0, 0, 0],
        ]
        .concat();
        out.extend_from_slice(&ihdr);
        out.extend_from_slice(&crc32(&ihdr[4..]).to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(b"IEND");
        out.extend_from_slice(&crc32(b"IEND").to_be_bytes());
        out
    }

    async fn send_json_login(app: &Router, remote: Option<&str>, body: &str) -> HttpResponse<Body> {
        send(
            app,
            req(
                "POST",
                "/api/login",
                remote,
                &[(header::CONTENT_TYPE.to_string(), JSON_CT.into())],
                Some(body.to_string()),
            ),
        )
        .await
    }

    async fn login(app: &Router) -> HttpResponse<Body> {
        send_json_login(app, None, ADMIN_JSON).await
    }

    #[tokio::test]
    async fn unauthenticated_devices_list_is_401() {
        let t = build_app(
            "401devs",
            auth::Credential::Plain("admin123".into()),
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
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
        let resp = send(&t.app, req("GET", "/api/tasks", None, &[], None)).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let j = json_body(resp).await;
        assert_eq!(j["error"], "unauthorized");
    }

    #[tokio::test]
    async fn unauthenticated_shutdown_is_401_and_service_stays_alive() {
        let t = build_app(
            "401sd",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
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
            auth::Credential::Plain("admin123".into()),
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
        let t = build_app(
            "vacuum",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
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
    async fn readiness_is_public_structured_and_does_not_leak_paths() {
        let t = build_app(
            "ready",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
        let resp = send(&t.app, req("GET", "/health/ready", None, &[], None)).await;
        assert!(matches!(
            resp.status(),
            StatusCode::OK | StatusCode::SERVICE_UNAVAILABLE
        ));
        let status = resp.status();
        let body = json_body(resp).await;
        assert!(body["ready"].is_boolean());
        for name in ["data_dir", "sqlite", "scrcpy_server", "adb", "ffmpeg"] {
            assert!(body["checks"][name]["ok"].is_boolean(), "{name}");
        }
        assert_eq!(body["ready"], status == StatusCode::OK);
        assert!(!body
            .to_string()
            .contains(&t.dir.to_string_lossy().to_string()));
    }

    #[tokio::test]
    async fn metrics_is_public_prometheus_text_with_low_cardinality() {
        let t = build_app(
            "metrics",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
        let resp = send(&t.app, req("GET", "/metrics", None, &[], None)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/plain; version=0.0.4; charset=utf-8")
        );
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("gamer_sessions_active "));
        assert!(body.contains("gamer_runs_active "));
        assert!(body.contains("gamer_db_ready 1"));
        assert!(!body.contains(&t.dir.to_string_lossy().to_string()));
    }

    #[tokio::test]
    async fn login_sets_cookie_with_contract_attributes() {
        let t = build_app(
            "cookie",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
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
        let t = build_app(
            "badpw",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
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
        let t = build_app("rl429", auth::Credential::Plain("admin123".into()), cfg);
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
        let resp = send_json_login(&t.app, Some("203.0.113.7:5555"), ADMIN_JSON).await;
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
        let t = build_app("rlpair", auth::Credential::Plain("admin123".into()), cfg);

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
            r#"{"username":"decoy","password":"admin123"}"#,
        )
        .await;
        assert_eq!(decoy.status(), StatusCode::TOO_MANY_REQUESTS);
        let admin = send_json_login(&t.app, Some("203.0.113.30:4000"), ADMIN_JSON).await;
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
        let locked = send_json_login(&t.app, Some("203.0.113.31:4000"), ADMIN_JSON).await;
        assert_eq!(locked.status(), StatusCode::TOO_MANY_REQUESTS);
        let other_ip = send_json_login(&t.app, Some("203.0.113.32:4000"), ADMIN_JSON).await;
        assert_eq!(other_ip.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn session_probe_and_logout_semantics() {
        let t = build_app(
            "sess",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );

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
        let t = build_app(
            "expired-route",
            auth::Credential::Plain("admin123".into()),
            cfg,
        );
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
            auth::Credential::Plain("admin123".into()),
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
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
        let evil = [
            (header::ORIGIN.to_string(), "https://evil.example".into()),
            (header::HOST.to_string(), "localhost:8443".into()),
            (header::CONTENT_TYPE.to_string(), JSON_CT.into()),
        ];
        let resp = send(
            &t.app,
            req("POST", "/api/login", None, &evil, Some(ADMIN_JSON.into())),
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
        let t = build_app(
            "origin",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
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
        let t = build_app(
            "wsauth",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
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
            auth::Credential::Plain("admin123".into()),
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
            auth::Credential::Plain("admin123".into()),
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
        let t = build_app(
            "lanrej",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
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
        let t = build_app(
            "badtok",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
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

    // 环境变量密码链路优先级：GAMER_ADMIN_PASSWORD 设置时覆盖 config 明文
    #[test]
    fn resolve_credential_prefers_env_then_hash_then_plain() {
        use crate::config::Config;

        // 仅明文
        let cfg = Config::default(); // password=admin123
        let c = auth::resolve_credential(&cfg);
        assert!(matches!(c, auth::Credential::Plain(_)));
        let st = auth::AuthState::new(c, Default::default(), false, None);
        assert!(st.verify_credentials("admin123"));

        // hash 覆盖明文
        let salt = [0x11u8; 8];
        let digest: [u8; 32] = {
            use sha2::{Digest, Sha256};
            let mut m = Sha256::new();
            m.update(salt);
            m.update(b"hashed-pw");
            m.finalize().into()
        };
        let boxed = format!(
            "sha256${}${}",
            salt.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            digest
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let mut cfg2 = Config::default();
        cfg2.auth.password_hash = boxed;
        let st2 = auth::AuthState::new(
            auth::resolve_credential(&cfg2),
            Default::default(),
            false,
            None,
        );
        assert_eq!(st2.credential_source(), "config:password_hash");
        assert!(st2.verify_credentials("hashed-pw"));
        assert!(!st2.verify_credentials("admin123"));
    }

    // ---------- Wave 2：输入与资源限额（SEC-004） ----------

    fn ctl_req(json: &str) -> ControlReq {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn control_parse_rejects_missing_and_invalid_fields() {
        // tap 缺坐标 / 越界 / 负值（NaN/Infinity 在 JSON 层即反序列化失败）
        assert!(parse_ctl(&ctl_req(r#"{"type":"tap"}"#)).is_err());
        assert!(
            parse_ctl(&ctl_req(r#"{"type":"tap","x":500}"#)).is_err(),
            "缺 y 拒绝"
        );
        assert!(parse_ctl(&ctl_req(r#"{"type":"tap","x":1e30,"y":0}"#)).is_err());
        assert!(parse_ctl(&ctl_req(r#"{"type":"tap","x":-1,"y":0}"#)).is_err());

        // swipe 缺坐标 / duration 非法
        assert!(parse_ctl(&ctl_req(r#"{"type":"swipe","x1":1,"y1":1,"x2":2}"#)).is_err());
        assert!(parse_ctl(&ctl_req(
            r#"{"type":"swipe","x1":1,"y1":1,"x2":2,"y2":2,"duration":999999999}"#
        ))
        .is_err());
        assert!(
            parse_ctl(&ctl_req(
                r#"{"type":"swipe","x1":1,"y1":1,"x2":2,"y2":2,"duration":300}"#
            ))
            .is_ok(),
            "合法 swipe 带时长放行"
        );

        // text 空 / 超 300 字节协议上限；多字节字符按字节计
        assert!(parse_ctl(&ctl_req(r#"{"type":"text"}"#)).is_err());
        assert!(parse_ctl(&ctl_req(r#"{"type":"text","text":""}"#)).is_err());
        let long = format!("{{\"type\":\"text\",\"text\":\"{}\"}}", "字".repeat(101)); // 303 字节
        assert!(parse_ctl(&ctl_req(&long)).is_err());
        let ok_len = format!("{{\"type\":\"text\",\"text\":\"{}\"}}", "a".repeat(299));
        assert!(parse_ctl(&ctl_req(&ok_len)).is_ok());

        // press keycode 0 与越界拒绝，合法值放行
        assert!(parse_ctl(&ctl_req(r#"{"type":"press"}"#)).is_err());
        assert!(parse_ctl(&ctl_req(r#"{"type":"press","keycode":0}"#)).is_err());
        assert!(parse_ctl(&ctl_req(r#"{"type":"press","keycode":1001}"#)).is_err());
        assert!(parse_ctl(&ctl_req(r#"{"type":"press","keycode":187}"#)).is_ok());

        // start_app 包名校验
        assert!(parse_ctl(&ctl_req(r#"{"type":"start_app"}"#)).is_err());
        assert!(parse_ctl(&ctl_req(r#"{"type":"start_app","app":"bad pkg!"}"#)).is_err());
        assert!(parse_ctl(&ctl_req(
            r#"{"type":"start_app","app":"+com.miHoYo.hkrpg"}"#
        ))
        .is_ok());
        assert!(
            parse_ctl(&ctl_req(r#"{"type":"start_app","app":"+bad/pkg"}"#)).is_err(),
            "+ 后非法包名拒绝"
        );
        assert!(
            parse_ctl(&ctl_req(r#"{"type":"start_app","app":"?崩坏星穹铁道"}"#)).is_ok(),
            "? 按名搜索透传"
        );
        assert!(parse_ctl(&ctl_req(r#"{"type":"start_app","app":"?"}"#)).is_err());
        assert!(parse_ctl(&ctl_req(r#"{"type":"start_app","app":"com.miHoYo.hkrpg"}"#)).is_ok());
        for injected in [
            "com.safe.app;id",
            "com.safe.app&&id",
            "com.safe.app$(id)",
            "com.safe.app`id`",
            "com.safe.app\nid",
            "com.safe.app --user 0",
            "+com.safe.app;id",
            "+--user",
        ] {
            let body = serde_json::json!({"type": "start_app", "app": injected}).to_string();
            assert!(
                parse_ctl(&ctl_req(&body)).is_err(),
                "可能进入 adb shell 包名拼接边界的注入载荷必须拒绝: {injected:?}"
            );
        }
        assert!(
            parse_ctl(&ctl_req(r#"{"type":"start_app","app":"?游戏; id"}"#)).is_ok(),
            "? 搜索名只经 scrcpy 二进制控制协议透传，不进入 adb shell 包名路径"
        );

        // clipboard 上限与空值
        assert!(parse_ctl(&ctl_req(r#"{"type":"clipboard","text":""}"#)).is_err());

        // 无参动作不受影响
        assert!(parse_ctl(&ctl_req(r#"{"type":"home"}"#)).is_ok());
        assert!(parse_ctl(&ctl_req(r#"{"type":"rotate"}"#)).is_ok());

        // 未知命令仍 400 文案
        match parse_ctl(&ctl_req(r#"{"type":"touch","action":"down"}"#)) {
            Err(e) => {
                assert_eq!(e.status(), StatusCode::BAD_REQUEST);
                assert_eq!(e.message(), "unknown command");
            }
            Ok(_) => panic!("touch 不属于 REST 控制命令"),
        }
    }

    #[tokio::test]
    async fn malformed_control_payload_is_400_even_offline() {
        let t = build_app(
            "ctl400",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
        let ck = cookie_of(&login(&t.app).await);
        let sid = first_cookie_pair(&ck);
        // 设备不存在：先经过输入校验（400），轮不到会话检查（409）
        let resp = send(
            &t.app,
            req(
                "POST",
                "/api/devices/nope/control",
                None,
                &[
                    (header::COOKIE.to_string(), sid),
                    (header::CONTENT_TYPE.to_string(), JSON_CT.into()),
                ],
                Some(r#"{"type":"tap"}"#.into()),
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn template_upload_rejects_byte_and_pixel_bombs_with_4xx() {
        let t = build_app(
            "tmpllimits",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
        let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
        let headers = |cookie: String| {
            vec![
                (header::COOKIE.to_string(), cookie),
                (header::CONTENT_TYPE.to_string(), JSON_CT.into()),
            ]
        };

        let bomb_b64 =
            base64::engine::general_purpose::STANDARD.encode(pixel_bomb_png(30_000, 30_000));
        let body = serde_json::json!({
            "name": "bomb.png",
            "pkg": "com.test.app",
            "data_b64": bomb_b64,
        })
        .to_string();
        let resp = send(
            &t.app,
            req(
                "POST",
                "/api/templates",
                None,
                &headers(sid.clone()),
                Some(body),
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // 构造超过原始模板字节上限的 base64，但保持请求体低于 16MiB 路由上限，
        // 断言 API 在解码前直接以 400 拒绝，不分配/解码图片。
        let too_large_b64 = "A".repeat((matcher::TEMPLATE_MAX_INPUT_BYTES / 3 + 2) * 4);
        let body = serde_json::json!({
            "name": "large.png",
            "pkg": "com.test.app",
            "data_b64": too_large_b64,
        })
        .to_string();
        let resp = send(
            &t.app,
            req("POST", "/api/templates", None, &headers(sid), Some(body)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_error_maps_status_and_json_body() {
        let resp = ApiError::conflict("device_busy").into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, serde_json::json!({"error": "device_busy"}));
    }

    #[tokio::test]
    async fn zip_import_rejects_slip_duplicate_and_pixel_bomb_with_4xx() {
        let t = build_app(
            "ziplimits",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
        let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
        let headers = |cookie: String| {
            vec![
                (header::COOKIE.to_string(), cookie),
                (header::CONTENT_TYPE.to_string(), "application/zip".into()),
            ]
        };
        let cases = [
            craft_zip(vec![("yaml/../escape.yaml", b"steps: []\n".to_vec())]),
            craft_zip(vec![("../escape.yaml", b"steps: []\n".to_vec())]),
            craft_zip(vec![("/absolute.yaml", b"steps: []\n".to_vec())]),
            craft_zip(vec![("yaml\\..\\escape.yaml", b"steps: []\n".to_vec())]),
            craft_zip(vec![
                ("yaml/one.yaml", b"steps: []\n".to_vec()),
                ("yaml/ONE.yaml", b"steps: []\n".to_vec()),
            ]),
            craft_zip(vec![("tmpl/bomb.png", pixel_bomb_png(30_000, 30_000))]),
        ];
        for zip_bytes in cases {
            let resp = send(
                &t.app,
                req_bytes(
                    "POST",
                    "/api/scripts/import?pkg=com.test.app&confirm=1",
                    None,
                    &headers(sid.clone()),
                    zip_bytes,
                ),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn request_body_limits_reject_oversize_json_and_zip_with_413() {
        let t = build_app(
            "bodylimits",
            auth::Credential::Plain("admin123".into()),
            Default::default(),
        );
        let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
        let json_headers = [
            (header::COOKIE.to_string(), sid.clone()),
            (header::CONTENT_TYPE.to_string(), JSON_CT.into()),
        ];
        let resp = send(
            &t.app,
            req_bytes(
                "POST",
                "/api/devices",
                None,
                &json_headers,
                vec![b'x'; BODY_LIMIT_JSON + 1],
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let zip_headers = [
            (header::COOKIE.to_string(), sid),
            (header::CONTENT_TYPE.to_string(), "application/zip".into()),
        ];
        let resp = send(
            &t.app,
            req_bytes(
                "POST",
                "/api/scripts/import?pkg=com.test.app",
                None,
                &zip_headers,
                vec![0u8; BODY_LIMIT_ZIP_IMPORT + 1],
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn log_limit_clamped_to_1_1000() {
        assert_eq!(clamp_log_limit(None), 200);
        assert_eq!(clamp_log_limit(Some(50)), 50);
        assert_eq!(clamp_log_limit(Some(0)), 1);
        assert_eq!(clamp_log_limit(Some(-100)), 1);
        assert_eq!(clamp_log_limit(Some(1001)), 1000);
        assert_eq!(clamp_log_limit(Some(1_000_000)), 1000);
    }

    #[test]
    fn route_validation_rejects_ambiguous_device_configuration() {
        let valid = CreateDeviceReq {
            name: "demo".into(),
            kind: "redroid".into(),
            addr: Some("127.0.0.1:5555".into()),
            screen_mode: Some("virtual".into()),
            vd_res: Some("1920x1080".into()),
            vd_dpi: Some(420),
            pkg: Some("com.example.game".into()),
            fps: Some(60),
        };
        assert!(validate_device_req(&valid).is_ok());

        let mut invalid = valid;
        invalid.screen_mode = Some("unexpected".into());
        assert!(validate_device_req(&invalid).is_err());
        invalid.screen_mode = Some("virtual".into());
        invalid.vd_res = Some("1920".into());
        assert!(validate_device_req(&invalid).is_err());
        invalid.vd_res = Some("1x1080".into());
        assert!(validate_device_req(&invalid).is_err());
        invalid.vd_res = Some("1920x1080".into());
        invalid.fps = Some(121);
        assert!(validate_device_req(&invalid).is_err());
        invalid.fps = Some(60);
        invalid.name = "line\nfeed".into();
        assert!(validate_device_req(&invalid).is_err());
    }

    #[test]
    fn session_affecting_change_only_detects_casting_fields() {
        let base = Device {
            id: "d1".into(),
            name: "挂机一号".into(),
            kind: "redroid".into(),
            addr: "127.0.0.1:5555".into(),
            screen_mode: ScreenMode::Virtual,
            vd_res: Some("1920x1080".into()),
            vd_dpi: Some(420),
            pkg: Some("com.example.game".into()),
            fps: Some(30),
            created_at: "2026-01-01 00:00:00".into(),
        };
        let mutate = |f: &dyn Fn(&mut Device)| {
            let mut d = base.clone();
            f(&mut d);
            d
        };

        // 非投屏字段（名称/包名）任意变化 → 不重建会话
        assert!(!session_affecting_change(
            &base,
            &mutate(&|d| d.name = "改名".into()),
            30
        ));
        assert!(!session_affecting_change(
            &base,
            &mutate(&|d| d.pkg = None),
            30
        ));
        assert!(!session_affecting_change(
            &base,
            &mutate(&|d| d.pkg = Some("com.other.app".into())),
            30
        ));

        // 写法差异但生效值相同（空串/None/默认值归一）→ 不重建会话
        assert!(!session_affecting_change(
            &base,
            &mutate(&|d| d.vd_res = Some(" 1920X1080 ".into())),
            30
        ));
        assert!(!session_affecting_change(
            &base,
            &mutate(&|d| d.vd_res = None),
            30
        ));
        // DPI None 与 0 同为"自动"
        let no_dpi = mutate(&|d| d.vd_dpi = None);
        assert!(!session_affecting_change(
            &no_dpi,
            &mutate(&|d| d.vd_dpi = Some(0)),
            30
        ));
        assert!(!session_affecting_change(
            &base,
            &mutate(&|d| d.fps = None),
            30
        ));

        // 投屏字段实质变化 → 重建会话
        assert!(session_affecting_change(
            &base,
            &mutate(&|d| d.screen_mode = ScreenMode::Mirror),
            30
        ));
        assert!(session_affecting_change(
            &base,
            &mutate(&|d| d.vd_res = Some("1280x720".into())),
            30
        ));
        assert!(session_affecting_change(
            &base,
            &mutate(&|d| d.vd_dpi = Some(320)),
            30
        ));
        assert!(session_affecting_change(
            &base,
            &mutate(&|d| d.fps = Some(60)),
            30
        ));
        assert!(session_affecting_change(
            &base,
            &mutate(&|d| d.addr = "192.168.1.9:5555".into()),
            30
        ));
        assert!(session_affecting_change(
            &base,
            &mutate(&|d| d.kind = "emu".into()),
            30
        ));

        // fps None 跟随全局配置：全局值不同则生效值不同 → 重建
        assert!(session_affecting_change(
            &base,
            &mutate(&|d| d.fps = None),
            60
        ));
    }

    #[test]
    fn route_validation_rejects_path_like_template_names() {
        for name in [
            "",
            ".hidden.png",
            "..",
            "../escape.png",
            "a\\b.png",
            "a:b.png",
        ] {
            assert!(
                validate_template_name(name).is_err(),
                "{name:?} must be rejected"
            );
        }
        assert_eq!(
            validate_template_name("login#0.1_0.2_0.3_0.4.png").unwrap(),
            "login#0.1_0.2_0.3_0.4.png"
        );
        assert!(validate_template_name("截图 1.png").is_ok());
    }

    #[test]
    fn route_validation_bounds_run_and_task_requests() {
        let task = SaveTaskReq {
            id: None,
            name: "daily".into(),
            cron: "*/5 * * * *".into(),
            script_id: "com.example.game/daily.yaml".into(),
            device_id: "device-1".into(),
            enabled: Some(true),
        };
        assert!(validate_task_req(&task).is_ok());
        let mut bad_task = task;
        bad_task.device_id.clear();
        assert!(validate_task_req(&bad_task).is_err());

        let run = RunScriptReq {
            device_id: "device-1".into(),
            start_index: Some(100_000),
            func: Some("main".into()),
        };
        assert!(validate_run_script_req(&run).is_ok());
        let bad_run = RunScriptReq {
            start_index: Some(100_001),
            ..run
        };
        assert!(validate_run_script_req(&bad_run).is_err());
    }
}
