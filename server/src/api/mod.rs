//! HTTP REST + WebSocket API
//!
//! REST: 设备 CRUD / 连接控制 / 截图 / 模板 / 脚本 / 任务 / 日志 / 认证
//! WS:   WebRTC 信令（/ws/device/:id）
//!
//! 鉴权（阶段 2 SEC，见 auth.rs）：
//! - 公开豁免组（public）：POST /api/login、GET /api/session、POST /api/logout
//!   （三者自身实现契约语义）、GET /health/live、GET /health/ready、GET /metrics、静态资源 fallback；
//! - 受保护组（protected）：其余全部 /api/** 与 /ws/device/:id——统一经 auth_guard：
//!   未认证 401 {"error":"unauthorized"}；状态变更/WS 升级 Origin≠Host 403；
//!   回环 + X-Admin-Token 快捷通道放行本机管理脚本；
//! - 分路由 body 限额：普通 JSON ≤256KiB；模板上传/脚本保存 JSON ≤16MiB
//!   （data_b64/base64 膨胀需要余量，真实图片字节上限在 matcher 收口）；
//!   ZIP 导入 ≤20MiB。CORS 层已整体移除（vite 代理同源不受影响）。

pub mod auth;
mod common;
mod devices;
mod error;
mod logs;
mod runs;
mod scripts;
mod system;
mod tasks;
mod templates;
#[cfg(test)]
mod tests;
mod ws;

pub(crate) use error::ApiError;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::middleware as axmw;
use axum::routing::{delete, get, post};
use axum::Router;
use tower_http::services::ServeDir;

use crate::config::Config;
use crate::device::DeviceManager;
use crate::scheduler::Scheduler;
use crate::scripts::ScriptStore;
use crate::store::Db;

use common::{BODY_LIMIT_JSON, BODY_LIMIT_PUBLIC, BODY_LIMIT_UPLOAD, BODY_LIMIT_ZIP_IMPORT};

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    /// 进程内低基数运行指标；标签不接受请求中的任意字符串。
    pub metrics: Arc<crate::metrics::Metrics>,
    pub devices: Arc<DeviceManager>,
    pub scheduler: Arc<Scheduler>,
    /// 统一运行管理（阶段 3 RUN-001）：手动/调度共用 run_id 注册表与设备级互斥
    pub runs: Arc<crate::run_manager::RunManager>,
    pub cfg: Config,
    /// 脚本文件存储（data/<pkg>/yaml/ 与 tmpl/）
    pub scripts: Arc<ScriptStore>,
    /// 每设备的活跃 viewer 注册表。
    pub viewers: crate::webrtc::ViewerMap,
    /// 优雅停机信号
    pub shutdown: tokio::sync::watch::Sender<bool>,
    /// 鉴权状态
    pub auth: Arc<auth::AuthState>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "router assembly keeps existing call shape"
)]
pub fn build_router(
    db: Db,
    devices: Arc<DeviceManager>,
    runs: Arc<crate::run_manager::RunManager>,
    scheduler: Arc<Scheduler>,
    cfg: Config,
    viewers: crate::webrtc::ViewerMap,
    scripts: Arc<ScriptStore>,
    shutdown: tokio::sync::watch::Sender<bool>,
    auth: Arc<auth::AuthState>,
) -> Router {
    let metrics = db.metrics();
    let state = AppState {
        db,
        metrics,
        devices,
        scheduler,
        runs,
        cfg: cfg.clone(),
        scripts,
        viewers,
        shutdown,
        auth,
    };

    // 视频静默看门狗 + 会话过期清扫
    system::spawn_watchdog(state.clone());
    auth::spawn_sweeper(state.auth.clone());

    // ---- 公开豁免组：登录三端点自身实现契约语义；health/metrics 探针匿名；
    //      静态资源兜底（前端 SPA）。这些路径不经过 auth_guard。
    let public: Router<()> = Router::new()
        .route("/api/login", post(auth::api_login))
        .route("/api/session", get(auth::api_session))
        .route("/api/logout", post(auth::api_logout))
        .route("/health/live", get(|| async { (StatusCode::OK, "ok") }))
        .route("/health/ready", get(system::api_health_ready))
        .route("/metrics", get(system::api_metrics))
        .fallback_service(
            ServeDir::new("./web-dist").fallback(ServeDir::new("./web-dist/index.html")),
        )
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(BODY_LIMIT_PUBLIC));

    // ---- 受保护组（普通 JSON API，≤256KiB）：设备 / 截图 / 控制 / 模板查询删除 /
    //      脚本运行停止状态导出 / 任务 / 日志 / op-templates / shutdown / 维护 vacuum。
    //      高风险接口标注（专项测试见文件尾 tests）：shutdown、设备控制
    //      （devices::api_control）、脚本运行·停止、模板删除（templates::api_delete_template）。
    let protected_json: Router<()> = Router::new()
        .route(
            "/api/devices",
            get(devices::api_list_devices).post(devices::api_create_device),
        )
        .route("/api/devices/scan", post(devices::api_scan_devices))
        .route(
            "/api/devices/:id",
            delete(devices::api_delete_device).put(devices::api_update_device),
        )
        .route("/api/devices/:id/apps", get(devices::api_device_apps))
        .route("/api/apps", get(devices::api_apps_by_addr))
        .route(
            "/api/devices/:id/connect",
            post(devices::api_connect_device),
        )
        .route(
            "/api/devices/:id/disconnect",
            post(devices::api_disconnect_device),
        )
        .route("/api/devices/:id/screenshot", post(devices::api_screenshot))
        .route("/api/devices/:id/control", post(devices::api_control))
        .route(
            "/api/templates/:name",
            delete(templates::api_delete_template).put(templates::api_rename_template),
        )
        .route(
            "/api/templates/:name/image",
            get(templates::api_get_template_image),
        )
        .route(
            "/api/templates/:name/test",
            post(templates::api_test_template),
        )
        .route("/api/scripts/:id", delete(scripts::api_delete_script))
        .route("/api/scripts/:id/run", post(runs::api_run_script))
        .route("/api/scripts/:id/stop", post(runs::api_stop_script))
        .route("/api/scripts/:id/status", get(runs::api_script_status))
        .route("/api/devices/:id/run", get(runs::api_device_run))
        .route("/api/runs/:run_id", get(runs::api_get_run))
        .route("/api/runs/:run_id/cancel", post(runs::api_cancel_run))
        .route("/api/scripts/export", get(scripts::api_export_partition))
        .route(
            "/api/tasks",
            get(tasks::api_list_tasks).post(tasks::api_save_task),
        )
        .route("/api/tasks/:id", delete(tasks::api_delete_task))
        .route("/api/tasks/:id/run", post(tasks::api_run_task_now))
        .route(
            "/api/logs",
            get(logs::api_list_logs).delete(logs::api_clear_logs),
        )
        .route("/api/op-templates", get(system::api_op_templates))
        .route("/api/shutdown", post(system::api_shutdown))
        .route(
            "/api/maintenance/vacuum",
            post(system::api_maintenance_vacuum),
        )
        // WS 信令与 REST 同守卫：升级握手完成前由 auth_guard 判定
        .route("/ws/device/:id", get(ws::ws_device))
        .with_state(state.clone())
        .route_layer(axmw::from_fn_with_state(
            state.auth.clone(),
            auth::auth_guard,
        ))
        .layer(DefaultBodyLimit::max(BODY_LIMIT_JSON));

    // ---- 受保护组（大 JSON，≤16MiB）：模板上传（data_b64）/ 脚本保存+列表。
    //      GET 与 POST 同路径注册在一组以避免 merge 冲突，GET 本身无 body 不受限额影响。
    let protected_upload: Router<()> = Router::new()
        .route(
            "/api/templates",
            get(templates::api_list_templates).post(templates::api_upload_template),
        )
        .route(
            "/api/scripts",
            get(scripts::api_list_scripts).post(scripts::api_save_script),
        )
        .with_state(state.clone())
        .route_layer(axmw::from_fn_with_state(
            state.auth.clone(),
            auth::auth_guard,
        ))
        .layer(DefaultBodyLimit::max(BODY_LIMIT_UPLOAD));

    // ---- 受保护组（ZIP 导入 ≤20MiB，高风险接口）：解压侧硬限另见 scripts.rs import
    let protected_import: Router<()> = Router::new()
        .route("/api/scripts/import", post(scripts::api_import_script))
        .with_state(state.clone())
        .route_layer(axmw::from_fn_with_state(
            state.auth.clone(),
            auth::auth_guard,
        ))
        .layer(DefaultBodyLimit::max(BODY_LIMIT_ZIP_IMPORT));

    // 最外层注入来源 IP 键（登录限流用）；CORS 层已移除——vite dev proxy 同源转发不受影响
    Router::new()
        .merge(public)
        .merge(protected_json)
        .merge(protected_upload)
        .merge(protected_import)
        .layer(axmw::from_fn(auth::inject_ip_key))
}
