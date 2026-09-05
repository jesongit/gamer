//! HTTP REST + WebSocket API
//!
//! REST: 设备 CRUD / 连接控制 / 截图 / 模板 / 脚本 / 任务 / 日志 / 认证
//! WS:   WebRTC 信令（/ws/device/:id）
//!
//! 鉴权（阶段 2 SEC，见 auth.rs）：
//! - 公开豁免组（public）：POST /api/login、GET /api/session、POST /api/logout、
//!   GET/POST /api/auth/setup（仅 POST 首次设置限制回环）
//!   （三者自身实现契约语义）、GET /health/live、GET /health/ready、GET /metrics、静态资源 fallback；
//! - 受保护组（protected）：其余全部 /api/** 与 /ws/device/:id——统一经 auth_guard：
//!   未认证 401 {"error":"unauthorized"}；状态变更/WS 升级 Origin≠Host 403；
//!   回环 + X-Admin-Token 快捷通道放行本机管理脚本；
//! - 分路由 body 限额：普通 JSON ≤256KiB；模板上传/脚本保存 JSON ≤16MiB
//!   （data_b64/base64 膨胀需要余量，真实图片字节上限在 matcher 收口）；
//!   扩展包归档 ≤20MiB；App Package 安装 ≤100MiB（对齐解压总量预算）。
//!   CORS 层已整体移除（vite 代理同源不受影响）。

mod app_packages;
pub mod auth;
mod common;
mod devices;
mod error;
mod extensions;
mod extensions_management;
pub(crate) mod gate;
mod logs;
mod resources;
mod runs;
pub(crate) mod system;
mod tasks;
#[cfg(test)]
mod tests;
pub(crate) mod update;
mod vision;
mod ws;

pub(crate) use error::ApiError;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::middleware as axmw;
use axum::routing::{any, delete, get, post, put};
use axum::Router;
use tower_http::services::ServeDir;

use crate::config::Config;
use crate::device::DeviceManager;
use crate::resources::ResourceStore;
use crate::scheduler::Scheduler;
use crate::store::Db;

use common::{
    BODY_LIMIT_JSON, BODY_LIMIT_PACKAGE_INSTALL, BODY_LIMIT_PUBLIC, BODY_LIMIT_UPLOAD,
    BODY_LIMIT_ZIP_IMPORT,
};

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
    /// 通用资源存储（六目录 + composite 三层 + 扩展注册的内容钩子；
    /// P11.3：ScriptStore/KeymapStore 消解后的 Core 侧唯一资源层）
    pub resources: Arc<ResourceStore>,
    /// 每设备的活跃 viewer 注册表。
    pub viewers: crate::webrtc::ViewerMap,
    /// 统一停机协调器（OPS-001）：/api/shutdown 经它触发 drain，
    /// 与 Ctrl+C / SIGTERM 共用同一路径
    pub shutdown: Arc<crate::shutdown::ShutdownCoordinator>,
    /// 鉴权状态
    pub auth: Arc<auth::AuthState>,
    /// 更新子系统（SYS-004：状态聚合 + 动作受理 + 策略存储）
    pub update: Arc<crate::update::service::UpdateService>,
    /// 已安装扩展与其 Host/UI 生命周期。
    pub extensions: Arc<crate::extensions::ExtensionService>,
    /// Immutable App Package storage. Its unload path is wired to the Timer
    /// task-suspension hook at the composition root.
    pub app_packages: Arc<crate::app_packages::AppPackageStore>,
}

/// 测试专用兼容入口：自建 capabilities registry / ExtensionService / AppState
/// 后装配完整路由（生产由 main.rs 的组合根 `RuntimeServices::start` 装配并注入）。
#[cfg(test)]
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
    resources: Arc<ResourceStore>,
    shutdown: Arc<crate::shutdown::ShutdownCoordinator>,
    auth: Arc<auth::AuthState>,
    update: Arc<crate::update::service::UpdateService>,
) -> Router {
    let capabilities = crate::capabilities::adapters::build_registry(
        devices.clone(),
        resources.clone(),
        db.clone(),
        runs.clone(),
    );
    let extensions = Arc::new(crate::extensions::ExtensionService::for_data_root(
        cfg.data_dir.clone(),
        capabilities,
    ));
    build_router_with_extensions(
        db, devices, runs, scheduler, cfg, viewers, resources, shutdown, auth, update, extensions,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "router assembly keeps existing call shape while injecting extensions"
)]
pub(crate) fn build_router_with_extensions(
    db: Db,
    devices: Arc<DeviceManager>,
    runs: Arc<crate::run_manager::RunManager>,
    scheduler: Arc<Scheduler>,
    cfg: Config,
    viewers: crate::webrtc::ViewerMap,
    resources: Arc<ResourceStore>,
    shutdown: Arc<crate::shutdown::ShutdownCoordinator>,
    auth: Arc<auth::AuthState>,
    update: Arc<crate::update::service::UpdateService>,
    extensions: Arc<crate::extensions::ExtensionService>,
) -> Router {
    let metrics = db.metrics();
    // 预设发布用 Timer Core 门面：publish_package_presets 只写 task_presets
    // 行，不触碰调度循环（Scheduler 内核另持有一个已 start 的实例，字段私有；
    // 该未 start 门面与其共享同一 Db，预设发布与调度互不可见对方的通知通道）。
    let preset_timer = crate::timer_core::TimerCore::new(db.clone());
    let state = AppState {
        db,
        metrics,
        devices,
        scheduler: scheduler.clone(),
        runs,
        cfg: cfg.clone(),
        resources,
        viewers,
        shutdown,
        auth,
        update,
        extensions,
        app_packages: Arc::new(crate::app_packages::AppPackageStore::with_hooks(
            cfg.data_dir.clone(),
            Arc::new(crate::app_packages::SchedulerTaskSuspendedHook::new(
                scheduler.clone(),
            )),
            Arc::new(crate::app_packages::TimerPresetPublishHook::new(
                preset_timer,
            )),
        )),
    };

    // ---- 公开豁免组：登录三端点自身实现契约语义；health/metrics 探针匿名；
    //      静态资源兜底（前端 SPA）。这些路径不经过 auth_guard。
    let public: Router<()> = Router::new()
        .route("/api/login", post(auth::api_login))
        .route("/api/session", get(auth::api_session))
        .route("/api/logout", post(auth::api_logout))
        .route(
            "/api/auth/setup",
            get(auth::api_setup_status).post(auth::api_setup_password),
        )
        .route("/health/live", get(|| async { (StatusCode::OK, "ok") }))
        .route("/health/ready", get(system::api_health_ready))
        .route("/health/shutdown", get(system::api_shutdown_state))
        .route("/metrics", get(system::api_metrics))
        .fallback_service({
            // PATH-002：web-dist 相对 GAMER_APP_DIR（应用版本目录）解析；
            // 未注入回退现状 cwd 相对（开发流不变）。ServeDir 只读取该目录，
            // 版本目录只读时前端仍可服务
            let web_dist = cfg.web_dist_dir();
            ServeDir::new(&web_dist).fallback(ServeDir::new(web_dist.join("index.html")))
        })
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(BODY_LIMIT_PUBLIC));

    // ---- 受保护组（普通 JSON API，≤256KiB）：设备 / 截图 / 控制 / 通用资源
    //      CRUD / 统一运行分发与状态查询 / 任务 / 日志 / shutdown / 维护 vacuum。
    //      高风险接口标注（专项测试见文件尾 tests）：shutdown、设备控制
    //      （devices::api_control）、运行分发、资源删除。
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
        // Generic Resource API（P11.6 / §11.2）：scripts/functions/templates/
        // keymaps/presets/resources 六类别统一；内容校验经扩展注册的
        // ResourceKindHandler 回调（gamer.yaml / gamer.keymap）。
        .route(
            "/api/apps/:app/resources",
            get(resources::api_list_all_resources),
        )
        .route(
            "/api/apps/:app/resources/:kind",
            get(resources::api_list_kind_resources),
        )
        .route(
            "/api/apps/:app/resources/:kind/*id",
            get(resources::api_get_resource).delete(resources::api_delete_resource),
        )
        // Vision 能力位（模板匹配测试 = vision 语义，Core 合法）
        .route(
            "/api/capabilities/vision/test",
            post(vision::api_vision_test_template),
        )
        // 统一执行入口（P11.6 / §11.3）：原 /api/scripts/:id/run 与
        // /api/functions/:id/run 删除，经 Runner 注册表分发。
        .route("/api/runs", post(runs::api_dispatch_run))
        .route("/api/devices/:id/run", get(runs::api_device_run))
        .route("/api/runs/:run_id", get(runs::api_get_run))
        .route("/api/runs/:run_id/cancel", post(runs::api_cancel_run))
        // Unified Task API（P11.1 / ADR-12：Task = 任意 ScheduleProvider + 任意
        // Runner）。原 legacy `/api/tasks`（script_id+cron）与 `/api/user-tasks`
        // 已收口为这一组端点；presets 只保留 `/api/task-presets`。
        .route(
            "/api/tasks",
            get(tasks::api_list_tasks).post(tasks::api_save_task),
        )
        .route(
            "/api/tasks/:id",
            get(tasks::api_get_task)
                .put(tasks::api_update_task)
                .delete(tasks::api_delete_task),
        )
        .route("/api/tasks/:id/run", post(tasks::api_run_task_now))
        .route("/api/tasks/:id/suspend", post(tasks::api_suspend_task))
        .route("/api/tasks/:id/resume", post(tasks::api_resume_task))
        .route("/api/tasks/:id/enable", post(tasks::api_enable_task))
        .route("/api/tasks/:id/disable", post(tasks::api_disable_task))
        .route("/api/tasks/:id/cancel", post(tasks::api_cancel_task))
        // UI 支撑只读端点：TaskBoard 的执行器 / 触发方式下拉数据源。
        .route("/api/runners", get(tasks::api_list_runners))
        .route(
            "/api/runners/:runner_id/entrypoint",
            get(tasks::api_runner_entrypoint_schema),
        )
        .route(
            "/api/schedule-providers",
            get(tasks::api_list_schedule_providers),
        )
        .route(
            "/api/task-presets",
            get(tasks::api_list_task_presets).post(tasks::api_save_task_preset),
        )
        .route(
            "/api/task-presets/:id",
            get(tasks::api_get_task_preset)
                .put(tasks::api_update_task_preset)
                .delete(tasks::api_delete_task_preset),
        )
        .route(
            "/api/task-presets/:id/instantiate",
            post(tasks::api_instantiate_task_preset),
        )
        .route(
            "/api/logs",
            get(logs::api_list_logs).delete(logs::api_clear_logs),
        )
        .route("/api/system/info", get(system::api_system_info))
        .route(
            "/api/app-packages",
            get(app_packages::api_list_app_packages),
        )
        .route(
            "/api/app-packages/export",
            post(app_packages::api_export_app_package),
        )
        .route(
            "/api/app-packages/:id/activate",
            post(app_packages::api_activate_app_package),
        )
        .route(
            "/api/app-packages/:id/:version",
            delete(app_packages::api_uninstall_app_package),
        )
        .route(
            "/api/app-packages/:id/:version/edit",
            post(app_packages::api_edit_app_package),
        )
        .route(
            "/api/workspace/:android_package",
            get(app_packages::api_get_workspace).put(app_packages::api_put_workspace),
        )
        .route("/api/system/update", get(update::api_get_update))
        .route("/api/system/update/check", post(update::api_update_check))
        .route(
            "/api/system/update/download",
            post(update::api_update_download),
        )
        .route(
            "/api/system/update/install",
            post(update::api_update_install),
        )
        .route(
            "/api/system/update/rollback",
            post(update::api_update_rollback),
        )
        .route(
            "/api/system/update/policy",
            axum::routing::put(update::api_update_policy),
        )
        .route("/api/shutdown", post(system::api_shutdown))
        .route(
            "/api/maintenance/vacuum",
            post(system::api_maintenance_vacuum),
        )
        // 未知 API 路径不应落入 SPA 静态 fallback（其 POST 默认会返回 405）；
        // 统一由受保护组明确返回 404，旧运行路径因此不再保留任何兼容处理器。
        .route("/api/*path", any(|| async { StatusCode::NOT_FOUND }))
        // WS 信令与 REST 同守卫：升级握手完成前由 auth_guard 判定
        .route("/ws/device/:id", get(ws::ws_device))
        .with_state(state.clone())
        .route_layer(axmw::from_fn_with_state(
            state.auth.clone(),
            auth::auth_guard,
        ))
        .layer(DefaultBodyLimit::max(BODY_LIMIT_JSON));

    // ---- 受保护组（≤16MiB）：资源创建（POST）与更新/替换/重命名（PUT）。
    //      文本 kind 收 JSON、字节 kind 收原始字节（模板按 Content-Type 区分
    //      字节替换与重命名）。统一注册在本组以获得上传体限额；文本内容另有
    //      1MiB 校验兜底。GET/DELETE 在 protected_json 组（小响应、无 body）。
    let protected_upload: Router<()> = Router::new()
        .route(
            "/api/apps/:app/resources/:kind",
            post(resources::api_create_resource),
        )
        .route(
            "/api/apps/:app/resources/:kind/*id",
            put(resources::api_update_resource),
        )
        .with_state(state.clone())
        .route_layer(axmw::from_fn_with_state(
            state.auth.clone(),
            auth::auth_guard,
        ))
        .layer(DefaultBodyLimit::max(BODY_LIMIT_UPLOAD));

    // ---- 受保护组（App Package 安装，body 上限对齐包归档解压总量预算）：
    //      归档侧另有 entries/解压总量/单文件/manifest 硬限（archive_validation）。
    let protected_import: Router<()> = Router::new()
        .route(
            "/api/app-packages/install",
            post(app_packages::api_install_app_package),
        )
        .with_state(state.clone())
        .route_layer(axmw::from_fn_with_state(
            state.auth.clone(),
            auth::auth_guard,
        ))
        .layer(DefaultBodyLimit::max(BODY_LIMIT_PACKAGE_INSTALL));

    // ---- 受保护的扩展包组：归档安装/更新与 UI iframe 静态资源。
    // 生命周期接口留在普通 JSON 组以复用同一认证与错误语义。
    let protected_extensions: Router<()> = Router::new()
        .route(
            "/api/extensions/management",
            get(extensions_management::api_extension_management),
        )
        .route(
            "/api/extensions/inspect",
            post(extensions_management::api_inspect_extension),
        )
        .route(
            "/api/extensions",
            get(extensions::api_list_extensions).post(extensions::api_install_extension),
        )
        .route(
            "/api/extensions/ui",
            get(extensions::api_list_ui_contributions),
        )
        .route(
            "/api/extensions/contributions",
            get(extensions::api_list_ui_contributions),
        )
        .route(
            "/api/extensions/:id/update",
            post(extensions::api_update_extension),
        )
        .route(
            "/api/extensions/:id/enable",
            post(extensions::api_enable_extension),
        )
        .route(
            "/api/extensions/:id/disable",
            post(extensions::api_disable_extension),
        )
        .route(
            "/api/extensions/:id/activate",
            post(extensions::api_activate_extension),
        )
        .route(
            "/api/extensions/:id/start",
            post(extensions::api_start_extension),
        )
        .route(
            "/api/extensions/:id/stop",
            post(extensions::api_stop_extension),
        )
        .route(
            "/api/extensions/:id/call",
            post(extensions::api_call_extension),
        )
        .route(
            "/api/extensions/:id/:version",
            delete(extensions::api_uninstall_extension),
        )
        .route(
            "/api/extensions/:id/ui/*path",
            get(extensions::api_get_extension_ui_asset),
        )
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
        .merge(protected_extensions)
        .layer(axmw::from_fn(auth::inject_ip_key))
}
