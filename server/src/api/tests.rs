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

use super::common::{BODY_LIMIT_JSON, BODY_LIMIT_PACKAGE_INSTALL};
use super::devices::{
    parse_ctl, session_affecting_change, validate_device_req, ControlReq, CreateDeviceReq,
};
use super::logs::clamp_log_limit;
use super::runs::{validate_run_req, RunReqArgs};
use super::tasks::{build_task, RunnerSpecDto, SaveTaskReq};
use super::templates::{compose_region_suffix, validate_short_name, validate_template_name};
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
        let devices = Arc::new(DeviceManager::new(db.clone(), cfg.clone()));
        // 生产执行器装配（设备离线时 prepare 即失败，正好覆盖"连接失败锁释放"路径）
        let executor = Arc::new(crate::engine::EngineExecutor::new(
            Arc::new(crate::engine::Runner::new(
                devices.clone(),
                Arc::new(crate::webrtc::ViewerEventSink::new(viewers.clone())),
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
        scripts: Arc<ScriptStore>,
        viewers: crate::webrtc::ViewerMap,
        credential: auth::Credential,
        auth_cfg: crate::config::AuthConfig,
        executor: Arc<dyn crate::run_manager::RunExecutor>,
    ) -> TestApp {
        let runs = Arc::new(crate::run_manager::RunManager::new(executor));
        let scheduler = Arc::new(Scheduler::new(db.clone()));
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
        let dir = cfg.data_dir.clone();
        let app = build_router(
            db,
            devices,
            runs,
            scheduler,
            cfg,
            viewers,
            scripts,
            shutdown,
            auth.clone(),
            update,
        );
        TestApp { app, dir }
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

    fn valid_template_png() -> Vec<u8> {
        let mut img = image::GrayImage::new(8, 8);
        for (x, y, p) in img.enumerate_pixels_mut() {
            p.0[0] = if (x + y) % 2 == 0 { 32 } else { 224 };
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageLuma8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
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
    mod resources_tests {
        include!("tests/resources.rs");
    }
    mod keymaps_tests {
        include!("tests/keymaps.rs");
    }
    mod runs_tests {
        include!("tests/runs.rs");
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
    mod app_packages_tests {
        include!("tests/app_packages.rs");
    }
    mod app_packages_export_tests {
        include!("tests/app_packages_export.rs");
    }
    mod app_packages_edit_tests {
        include!("tests/app_packages_edit.rs");
    }
    mod app_packages_lifecycle_tests {
        include!("tests/app_packages_lifecycle.rs");
    }
}
