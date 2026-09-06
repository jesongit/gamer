//! Direct API integration and validation tests.

use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::device::DeviceManager;
use crate::resources::PackageStore;
use crate::scheduler::Scheduler;
use crate::store::{Db, Device, ScreenMode};
use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Router;

use super::devices::{
    parse_ctl, session_affecting_change, validate_device_req, ControlReq, CreateDeviceReq,
};
use super::logs::clamp_log_limit;
use super::tasks::{build_task, RunnerSpecDto, SaveTaskReq};
use super::{auth, build_router, ApiError};
use crate::timer_core::{ScheduleRegistry, TaskSchedule};

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
        /// 设备管理器句柄：测试用 `seed_device` 直注设备行（POST /api/devices
        /// 的 id 由服务端生成，测试需要固定 device_id 时走这里）。
        devices: Arc<DeviceManager>,
        #[allow(dead_code)]
        dir: std::path::PathBuf,
    }

    /// 直注一台设备（id 由调用方指定；pkg 缺省给一个合法 Android 包名——
    /// POST /api/runs 的 Android 上下文严格取设备配置，不再回退 Package id）。
    async fn seed_device(t: &TestApp, id: &str, pkg: &str) {
        let device = Device {
            id: id.to_string(),
            name: id.to_string(),
            kind: "wifi".into(),
            addr: "127.0.0.1:5555".into(),
            screen_mode: ScreenMode::Mirror,
            vd_res: None,
            vd_dpi: None,
            pkg: (!pkg.is_empty()).then(|| pkg.to_string()),
            fps: None,
            created_at: "2026-01-01 00:00:00".into(),
        };
        t.devices.upsert_device(&device).await.unwrap();
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
        let scripts = Arc::new(PackageStore::open(&cfg).unwrap());
        let viewers: crate::webrtc::ViewerMap =
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let devices = Arc::new(DeviceManager::new(db.clone(), cfg.clone()));
        // 生产执行器装配（v3-only；设备离线时 prepare 即失败，正好覆盖"连接失败锁释放"路径）
        let executor = Arc::new(crate::extensions::gamer_yaml::runner_adapter::EngineExecutor::new(
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
        let scripts = Arc::new(PackageStore::open(&cfg).unwrap());
        let viewers: crate::webrtc::ViewerMap =
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let devices = Arc::new(DeviceManager::new(db.clone(), cfg.clone()));
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
        scripts: Arc<PackageStore>,
        viewers: crate::webrtc::ViewerMap,
        credential: auth::Credential,
        auth_cfg: crate::config::AuthConfig,
        executor: Arc<dyn crate::run_manager::RunExecutor>,
    ) -> TestApp {
        let runs = Arc::new(crate::run_manager::RunManager::new(executor));
        let scheduler = Arc::new(Scheduler::new(db.clone()));
        // 与生产等价：gamer.yaml 扩展 Running 时注册其 timer runner（POST
        // /api/runs 手动分发与任务路径共用同一注册表）。测试装配直接同步注册。
        let yaml_runner = Arc::new(
            crate::extensions::gamer_yaml::timer_yaml::YamlTimerRunner::new(
                db.clone(),
                runs.clone(),
                scripts.clone(),
            ),
        );
        scheduler
            .register_runner_for_tests("gamer.yaml", "gamer.yaml", yaml_runner.clone())
            .unwrap();
        // P12.3：entrypoint 参数 schema 描述器（与生产 start 生命周期同构）
        scheduler.register_entrypoint_describer(
            "gamer.yaml",
            "gamer.yaml",
            yaml_runner.entrypoint_describer(),
        );
        let auth = Arc::new(auth::AuthState::new(
            credential,
            auth_cfg,
            false,
            Some("test-token".into()),
        ));
        // 测试用协调器：无会话可拆，drain 为空操作（行为断言在 shutdown.rs 单测）
        let shutdown = Arc::new(crate::shutdown::ShutdownCoordinator::new(Arc::new(|| {
            Box::pin(async {})
        })));
        // 更新服务：非托管实现（update 端点行为断言在 api/update.rs 契约测试；
        // 既有测试不受影响——controller 从不触网、从不读环境）
        let policy_store = crate::update::policy::PolicyStore::load_blocking(
            &cfg.data_dir,
            crate::update::policy::UpdatePolicy::default(),
        );
        let update = Arc::new(crate::update::service::UpdateService::new(
            Arc::new(crate::update::controller::UnsupportedController),
            policy_store,
            Arc::new(crate::update::service::UpdateTxn::default()),
            Arc::new(crate::update::workload::Workload::default),
            db.clone(),
        ));
        // 与生产组合根一致：注册扩展资源内容钩子（保存校验/注记）
        crate::extensions::gamer_yaml::register_resource_handlers(&scripts);
        crate::extensions::register_resource_handlers(&scripts);
        let dir = cfg.data_dir.clone();
        let app = build_router(
            db,
            devices.clone(),
            runs,
            scheduler,
            cfg,
            viewers,
            scripts,
            shutdown,
            auth.clone(),
            update,
        );
        TestApp {
            app,
            devices,
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

    fn test_credential(password: &str) -> auth::Credential {
        auth::parse_password_hash(&auth::hash_password(password).unwrap()).unwrap()
    }

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

    fn ctl_req(json: &str) -> ControlReq {
        serde_json::from_str(json).unwrap()
    }

    fn json_headers(cookie: String) -> Vec<(String, String)> {
        vec![
            (header::COOKIE.to_string(), cookie),
            (header::CONTENT_TYPE.to_string(), JSON_CT.into()),
        ]
    }

    fn zip_headers(cookie: String) -> Vec<(String, String)> {
        vec![
            (header::COOKIE.to_string(), cookie),
            (header::CONTENT_TYPE.to_string(), "application/zip".into()),
        ]
    }

    /// Package API 资源直写夹具：建包（幂等）+ PUT 文本资源（force = 夹具
    /// 语义直写，等价旧「绕过保存期校验」的盘上形态）。
    async fn put_package_text(
        t: &TestApp,
        sid: &str,
        pkg: &str,
        plugin: &str,
        path: &str,
        content: &str,
    ) -> HttpResponse<Body> {
        let _ = post_json(t, sid, "/api/packages", serde_json::json!({ "id": pkg })).await;
        send(
            &t.app,
            req(
                "PUT",
                &format!("/api/packages/{pkg}/plugins/{plugin}/resources/{path}"),
                None,
                &json_headers(sid.to_string()),
                Some(
                    serde_json::json!({ "content": content, "force": true }).to_string(),
                ),
            ),
        )
        .await
    }

    /// 模板文件名规则锁定（原 api/resources.rs 校验器随六目录模型退役；
    /// 规则本体收口到 PackageStore 资源路径校验）。
    fn validate_template_name(name: &str) -> Result<String, ()> {
        let segs = crate::resources::sanitize_rel_path(name).map_err(|_| ())?;
        if segs.len() != 1 || name.len() > 255 {
            return Err(());
        }
        Ok(name.to_string())
    }

    async fn post_json(
        t: &TestApp,
        sid: &str,
        uri: &str,
        body: serde_json::Value,
    ) -> HttpResponse<Body> {
        send(
            &t.app,
            req(
                "POST",
                uri,
                None,
                &json_headers(sid.to_string()),
                Some(body.to_string()),
            ),
        )
        .await
    }

    async fn get_json(t: &TestApp, sid: &str, uri: &str) -> HttpResponse<Body> {
        send(
            &t.app,
            req("GET", uri, None, &json_headers(sid.to_string()), None),
        )
        .await
    }

    mod auth_tests {
        include!("tests/auth.rs");
    }
    mod keymaps_tests {
        include!("tests/keymaps.rs");
    }
    mod runs_tests {
        include!("tests/runs.rs");
    }
    mod entrypoint_schema_tests {
        include!("tests/entrypoint_schema.rs");
    }
    mod system_tests {
        include!("tests/system.rs");
    }
    mod tasks_tests {
        include!("tests/tasks.rs");
    }
    mod update {
        include!("tests/update.rs");
    }
    mod extensions_tests {
        include!("tests/extensions.rs");
    }
    mod packages_tests {
        include!("tests/packages.rs");
    }
    mod packages_dormant_tests {
        include!("tests/packages_dormant.rs");
    }
    mod packages_states_tests {
        include!("tests/packages_states.rs");
    }
}
