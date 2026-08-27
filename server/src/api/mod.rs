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
mod ws;

pub mod auth;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware as axmw;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tower_http::services::ServeDir;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::device::DeviceManager;
use crate::matcher;
use crate::scheduler::{next_run, Scheduler};
use crate::scripts::ScriptStore;
use crate::store::{Db, Device, LogEntry, ScreenMode, Task};

/// 普通 JSON API 请求体上限（256KiB）
const BODY_LIMIT_JSON: usize = 256 * 1024;
/// 模板上传 / 脚本保存的 JSON 上限：data_b64 base64 有 4/3 膨胀，模板真实字节
/// 上限另有 matcher 侧 10MiB 硬闸；脚本 YAML 实际由导入侧 1MiB 对齐口径
const BODY_LIMIT_UPLOAD: usize = 16 * 1024 * 1024;
/// ZIP 导入请求体上限（解压侧硬限见 scripts.rs import 常量）
const BODY_LIMIT_ZIP_IMPORT: usize = 20 * 1024 * 1024;
/// 公开豁免组请求体上限（登录等极小 JSON）
const BODY_LIMIT_PUBLIC: usize = 64 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub devices: Arc<DeviceManager>,
    pub scheduler: Arc<Scheduler>,
    /// 统一运行管理（阶段 3 RUN-001）：手动/调度共用 run_id 注册表与设备级互斥
    pub runs: Arc<crate::run_manager::RunManager>,
    pub cfg: Config,
    /// 脚本文件存储（data/scripts/<package>/）
    pub scripts: Arc<ScriptStore>,
    /// 每设备的活跃 viewer（WebRTC 会话）注册表（main.rs 创建，与 Scheduler 共享）：
    /// 同一设备只允许一个活跃 viewer——新连接踢掉旧连接（旧 pusher 停止 + 旧 peer 关闭），
    /// 避免多连接多推流导致浏览器端 srcObject 串流/资源浪费。
    /// control_dc 字段供引擎反向推送脚本可视化事件（tap/swipe/匹配命中）。
    pub viewers: crate::webrtc::ViewerMap,
    /// 优雅停机信号（POST /api/shutdown 拆完会话后触发，main 的 axum 优雅退出）
    pub shutdown: tokio::sync::watch::Sender<bool>,
    /// 鉴权状态：会话表 / 登录限流 / 凭据 / 回环管理令牌（阶段 2）
    pub auth: Arc<auth::AuthState>,
}

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
    let state = AppState {
        db,
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
    spawn_watchdog(state.clone());
    auth::spawn_sweeper(state.auth.clone());

    // ---- 公开豁免组：登录三端点自身实现契约语义；health/metrics 探针匿名；
    //      静态资源兜底（前端 SPA）。这些路径不经过 auth_guard。
    let public: Router<()> = Router::new()
        .route("/api/login", post(api_login))
        .route("/api/session", get(api_session))
        .route("/api/logout", post(api_logout))
        .route("/health/live", get(|| async { (StatusCode::OK, "ok") }))
        .route("/health/ready", get(api_health_ready))
        .route("/metrics", get(api_metrics))
        .fallback_service(
            ServeDir::new("./web-dist").fallback(ServeDir::new("./web-dist/index.html")),
        )
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(BODY_LIMIT_PUBLIC));

    // ---- 受保护组（普通 JSON API，≤256KiB）：设备 / 截图 / 控制 / 模板查询删除 /
    //      脚本运行停止状态导出 / 任务 / 日志 / op-templates / shutdown。
    //      高风险接口标注（专项测试见文件尾 tests）：shutdown、设备控制
    //      （api_control）、脚本运行·停止、模板删除（api_delete_template）。
    let protected_json: Router<()> = Router::new()
        .route(
            "/api/devices",
            get(api_list_devices).post(api_create_device),
        )
        .route("/api/devices/scan", post(api_scan_devices))
        .route(
            "/api/devices/:id",
            delete(api_delete_device).put(api_update_device),
        )
        .route("/api/devices/:id/apps", get(api_device_apps))
        .route("/api/apps", get(api_apps_by_addr))
        .route("/api/devices/:id/connect", post(api_connect_device))
        .route("/api/devices/:id/disconnect", post(api_disconnect_device))
        .route("/api/devices/:id/screenshot", post(api_screenshot))
        .route("/api/devices/:id/control", post(api_control))
        .route(
            "/api/templates/:name",
            delete(api_delete_template).put(api_rename_template),
        )
        .route("/api/templates/:name/image", get(api_get_template_image))
        .route("/api/templates/:name/test", post(api_test_template))
        .route("/api/scripts/:id", delete(api_delete_script))
        .route("/api/scripts/:id/run", post(api_run_script))
        .route("/api/scripts/:id/stop", post(api_stop_script))
        .route("/api/scripts/:id/status", get(api_script_status))
        .route("/api/devices/:id/run", get(api_device_run))
        .route("/api/runs/:run_id", get(api_get_run))
        .route("/api/runs/:run_id/cancel", post(api_cancel_run))
        .route("/api/scripts/export", get(api_export_partition))
        .route("/api/tasks", get(api_list_tasks).post(api_save_task))
        .route("/api/tasks/:id", delete(api_delete_task))
        .route("/api/tasks/:id/run", post(api_run_task_now))
        .route("/api/logs", get(api_list_logs).delete(api_clear_logs))
        .route("/api/op-templates", get(api_op_templates))
        .route("/api/shutdown", post(api_shutdown))
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
            get(api_list_templates).post(api_upload_template),
        )
        .route("/api/scripts", get(api_list_scripts).post(api_save_script))
        .with_state(state.clone())
        .route_layer(axmw::from_fn_with_state(
            state.auth.clone(),
            auth::auth_guard,
        ))
        .layer(DefaultBodyLimit::max(BODY_LIMIT_UPLOAD));

    // ---- 受保护组（ZIP 导入 ≤20MiB，高风险接口）：解压侧硬限另见 scripts.rs import
    let protected_import: Router<()> = Router::new()
        .route("/api/scripts/import", post(api_import_script))
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

/// 结构化 readiness 探针：检查服务本地运行所需的持久化目录、SQLite、
/// scrcpy-server 资源和外部工具。探针本身匿名可访问，响应只返回布尔状态，
/// 不把本机路径、命令行或底层错误泄露给客户端。
async fn api_health_ready(State(st): State<AppState>) -> Response {
    let data_dir = st.cfg.data_dir.clone();
    let scrcpy_server = st.cfg.scrcpy_server.clone();
    let db = st.db.clone();
    let cfg = st.cfg.clone();
    let (data_dir_ok, scrcpy_ok, db_ok, tools) = tokio::join!(
        tokio::task::spawn_blocking(move || data_dir.is_dir()),
        tokio::task::spawn_blocking(move || scrcpy_server.is_file()),
        tokio::task::spawn_blocking(move || db.health_check().is_ok()),
        tokio::task::spawn_blocking(move || cfg.probe_external_tools()),
    );
    let data_dir_ok = data_dir_ok.unwrap_or(false);
    let scrcpy_ok = scrcpy_ok.unwrap_or(false);
    let db_ok = db_ok.unwrap_or(false);
    let tool_probes = tools.ok().unwrap_or_default();
    let adb_ok = tool_probes
        .iter()
        .find(|p| p.name == "adb")
        .map(|p| p.status.is_ok())
        .unwrap_or(false);
    let ffmpeg_ok = tool_probes
        .iter()
        .find(|p| p.name == "ffmpeg")
        .map(|p| p.status.is_ok())
        .unwrap_or(false);
    let ready = data_dir_ok && scrcpy_ok && db_ok && adb_ok && ffmpeg_ok;
    let body = serde_json::json!({
        "ready": ready,
        "checks": {
            "data_dir": { "ok": data_dir_ok },
            "sqlite": { "ok": db_ok },
            "scrcpy_server": { "ok": scrcpy_ok },
            "adb": { "ok": adb_ok },
            "ffmpeg": { "ok": ffmpeg_ok },
        }
    });
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body)).into_response()
}

/// 暴露低基数 Prometheus 文本指标。读数据库和外部探测均移到 blocking 池，
/// 避免指标抓取把 rusqlite/命令执行带到 Tokio 核心线程；业务指标采集失败时
/// 仍返回合法响应，并用 `gamer_db_ready` 标记异常。
async fn api_metrics(State(st): State<AppState>) -> Response {
    let db = st.db.clone();
    let db_snapshot = tokio::task::spawn_blocking(move || db.metrics_snapshot()).await;
    let db_ready = db_snapshot.as_ref().is_ok_and(|r| r.is_ok());
    let db_metrics = db_snapshot.ok().and_then(Result::ok).unwrap_or_default();
    let configured_devices = st.devices.list_snapshot().len();
    let active_sessions = st.devices.online_sessions().len();
    let active_viewers = st.viewers.lock().map(|v| v.len()).unwrap_or_default();
    let active_runs = st.runs.active_count();

    let mut body = String::new();
    body.push_str("# HELP gamer_configured_devices Number of configured devices.\n");
    body.push_str("# TYPE gamer_configured_devices gauge\n");
    body.push_str(&format!(
        "gamer_configured_devices {}\n",
        configured_devices
    ));
    body.push_str("# HELP gamer_sessions_active Current online scrcpy sessions.\n");
    body.push_str("# TYPE gamer_sessions_active gauge\n");
    body.push_str(&format!("gamer_sessions_active {}\n", active_sessions));
    body.push_str("# HELP gamer_viewers_active Current registered WebRTC viewers.\n");
    body.push_str("# TYPE gamer_viewers_active gauge\n");
    body.push_str(&format!("gamer_viewers_active {}\n", active_viewers));
    body.push_str("# HELP gamer_runs_active Current non-terminal runs.\n");
    body.push_str("# TYPE gamer_runs_active gauge\n");
    body.push_str(&format!("gamer_runs_active {}\n", active_runs));
    body.push_str("# HELP gamer_db_ready Whether the database metrics query succeeded.\n");
    body.push_str("# TYPE gamer_db_ready gauge\n");
    body.push_str(&format!("gamer_db_ready {}\n", u8::from(db_ready)));
    body.push_str("# HELP gamer_db_devices_total Rows in the devices table.\n");
    body.push_str("# TYPE gamer_db_devices_total gauge\n");
    body.push_str(&format!("gamer_db_devices_total {}\n", db_metrics.devices));
    body.push_str("# HELP gamer_db_tasks_total Rows in the tasks table.\n");
    body.push_str("# TYPE gamer_db_tasks_total gauge\n");
    body.push_str(&format!("gamer_db_tasks_total {}\n", db_metrics.tasks));
    body.push_str("# HELP gamer_db_logs_total Rows in the logs table.\n");
    body.push_str("# TYPE gamer_db_logs_total gauge\n");
    body.push_str(&format!("gamer_db_logs_total {}\n", db_metrics.logs));
    body.push_str("# HELP gamer_scheduled_runs_total Rows in the scheduled_runs table.\n");
    body.push_str("# TYPE gamer_scheduled_runs_total gauge\n");
    body.push_str(&format!(
        "gamer_scheduled_runs_total {}\n",
        db_metrics.scheduled_runs
    ));
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )
        .body(Body::from(body))
        .expect("metrics response builder with static headers")
        .into_response()
}

/// 视频静默看门狗：设备在线但视频流超过阈值无新帧时的处置。
///
/// 判死以 `session.connected`（video socket 读取循环退出即 false）为准，
/// **视频静默 ≠ 链路死亡**：虚拟屏无应用/静态画面时编码器 0 帧是正常态。
/// - 会话确死（connected=false）：拆会话；有脚本或 viewer 在跑则立即重连
///   （脚本引擎逐步重新取 session，可接续；无消费者则等下次触发连接）
/// - 脚本运行中 + 会话活着 + 静默：不处置（静态屏正常态；判死看控制
///   socket——引擎 tap 失败会报错终止脚本，不需要看门狗拆会话帮倒忙）
/// - 无 viewer 无脚本 + 会话活着 + 静默：交给 idle_power_loop 空闲低功耗
/// - viewer 在看且未被补帧投喂 + 会话活着 + 静默：reset_video 探测，
///   15s 仍静默才拆开重连（pusher 卡死等边缘兜底，踢 viewer）
const VIDEO_IDLE_RECONNECT_MS: u64 = 20_000;
/// reset_video 探测后等待新帧的宽限期，超过则认定编码器/链路已死，升级为重连
const VIDEO_NUDGE_GRACE: Duration = Duration::from_secs(15);

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn spawn_watchdog(st: AppState) {
    tokio::spawn(async move {
        // 已发过 reset_video 探测的设备 → 探测时刻
        let mut nudged: std::collections::HashMap<String, std::time::Instant> =
            std::collections::HashMap::new();
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            for (id, session) in st.devices.online_sessions() {
                let idle = session.video_idle_ms();
                if idle < VIDEO_IDLE_RECONNECT_MS {
                    nudged.remove(&id);
                    continue;
                }
                let running = st.devices.has_running_scripts(&id);
                // 会话确死（video socket 已关）：唯一允许脚本运行中强拆重连的
                // 路径——控制 socket 同链路已死，不重连脚本会永远卡死
                if !session.connected.load(std::sync::atomic::Ordering::SeqCst) {
                    warn!(device = %id, idle_ms = idle, "session dead (video socket closed), tearing down");
                    nudged.remove(&id);
                    // 踢 viewer：旧 pusher 挂在旧帧通道上，重连建新通道后不会
                    // 自动迁移；踢掉让前端 onclose 立即重连到新会话
                    let kicked = {
                        let mut map = st.viewers.lock().unwrap();
                        map.remove(&id)
                    };
                    if let Some(h) = kicked {
                        h.running.store(false, std::sync::atomic::Ordering::SeqCst);
                        if let Some(p) = h.peer.upgrade() {
                            let _ = p.close().await;
                        }
                    }
                    st.devices.disconnect_device(&id, true).await;
                    if running {
                        if let Err(e) = st.devices.connect_device(&id).await {
                            warn!(device = %id, err = %e, "auto-reconnect failed");
                        }
                    }
                    continue;
                }
                // 脚本运行中 + 静默 = 静态屏正常态（虚拟屏编码器 0 帧），
                // 不处置；会话真死由上面的 connected 分支兜底
                if running {
                    nudged.remove(&id);
                    continue;
                }
                // 无消费者：空闲低功耗统一交给 idle_power_loop
                let has_viewer = st.viewers.lock().unwrap().contains_key(&id);
                if !has_viewer {
                    nudged.remove(&id);
                    continue;
                }
                // viewer 正在被投喂（设备 0 帧是静态屏常态，pusher 静止补帧还活着）
                // → 会话对 viewer 是健康的：不 nudge（reset 反而打断补帧，MTK 静态
                // 屏 reset 后长时间不出 IDR → 浏览器断供被前端杀连接），也不走
                // 35s 兜底重连（静态屏挂机会话会被无限循环重连踢 viewer）。
                // 真断流时 pusher 退出、last_serve 过期，仍走 nudge → 15s → 重连兜底
                let served_ago_ms = {
                    let map = st.viewers.lock().unwrap();
                    match map.get(&id) {
                        Some(h) => {
                            let t = h.last_serve.load(std::sync::atomic::Ordering::Relaxed);
                            if t == 0 {
                                i64::MAX
                            } else {
                                now_unix_ms() - t
                            }
                        }
                        None => i64::MAX,
                    }
                };
                if served_ago_ms.max(0) < 10_000 {
                    nudged.remove(&id);
                    continue;
                }
                match nudged.get(&id).copied() {
                    None => {
                        // 第一轮：reset_video 请求 config+IDR——编码器活着会立即出帧，
                        // idle 归零回到健康分支，避免黑屏空转被误判为断流
                        if let Some(s) = st.devices.session(&id) {
                            let _ = s.reset_video().await;
                        }
                        nudged.insert(id.clone(), std::time::Instant::now());
                        continue;
                    }
                    Some(t) if t.elapsed() < VIDEO_NUDGE_GRACE => continue,
                    Some(_) => {
                        nudged.remove(&id);
                    }
                }
                warn!(device = %id, idle_ms = idle, "video stream silent after keyframe nudge, auto-reconnecting scrcpy session");
                // 踢旧 viewer：pusher 停止 + peer 关闭 → ws.rs 退出清理
                let kicked = {
                    let mut map = st.viewers.lock().unwrap();
                    map.remove(&id)
                };
                if let Some(h) = kicked {
                    h.running.store(false, std::sync::atomic::Ordering::SeqCst);
                    if let Some(p) = h.peer.upgrade() {
                        let _ = p.close().await;
                    }
                }
                st.devices.disconnect_device(&id, false).await;
                if let Err(e) = st.devices.connect_device(&id).await {
                    warn!(device = %id, err = %e, "auto-reconnect failed");
                }
            }
        }
    });
}

// ---------- 认证（契约钉死，见 web/src/auth.js 同款口径） ----------
//
// POST /api/login {username,password} → 200 Set-Cookie gb_session(Path=/; HttpOnly; SameSite=Strict)
//                                        body {ok:true,username}
//   401 {"error":"invalid_credentials"}；429 {"error":"too_many_attempts","retry_after":秒}
// GET  /api/session → 200 {authenticated:true,username} / 401 {"error":"unauthorized"}
// POST /api/logout → 204 销毁会话 + 过期 Set-Cookie（幂等）
// Secure 标志仅 GAMER_PROFILE=prod 追加；dev 纯 HTTP LAN 保持无 Secure。
// 旧响应壳 {token:"demo-token"} 已废除。

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

async fn api_login(
    State(st): State<AppState>,
    Extension(ip): Extension<auth::IpKey>,
    headers: HeaderMap,
    Json(req): Json<LoginReq>,
) -> Response {
    if !auth::origin_allows_headers(&headers) {
        return err_response(StatusCode::FORBIDDEN, "forbidden_origin");
    }
    // 形状粗校验：缺字段/超长直接 400，不进限流与凭据比对
    if req.username.is_empty()
        || req.password.is_empty()
        || req.username.len() > 64
        || req.password.len() > 1024
    {
        return err_response(StatusCode::BAD_REQUEST, "bad_request");
    }
    match st.auth.attempt_login(&req.username, &req.password, &ip.0) {
        Ok((sid, username)) => (
            StatusCode::OK,
            [(header::SET_COOKIE, st.auth.session_cookie_for(&sid))],
            Json(serde_json::json!({"ok": true, "username": username})),
        )
            .into_response(),
        Err(auth::LoginError::Invalid) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid_credentials"})),
        )
            .into_response(),
        Err(auth::LoginError::RateLimited { retry_after_secs }) => (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_after_secs.to_string())],
            Json(
                serde_json::json!({"error": "too_many_attempts", "retry_after": retry_after_secs}),
            ),
        )
            .into_response(),
    }
}

/// 会话探测（豁免组但语义与受保护端点一致：有效会话续期并回身份）
async fn api_session(State(st): State<AppState>, headers: HeaderMap) -> Response {
    match auth::AuthState::extract_sid(&headers).and_then(|sid| st.auth.validate(&sid)) {
        Some(username) => {
            Json(serde_json::json!({"authenticated": true, "username": username})).into_response()
        }
        None => auth::unauthorized_response(),
    }
}

/// 登出：销毁当前会话 + 下发过期 Cookie；无会话时同样 204（幂等）
async fn api_logout(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !auth::origin_allows_headers(&headers) {
        return err_response(StatusCode::FORBIDDEN, "forbidden_origin");
    }
    if let Some(sid) = auth::AuthState::extract_sid(&headers) {
        st.auth.destroy(&sid);
    }
    (
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, st.auth.expired_cookie())],
    )
        .into_response()
}

// ---------- 设备 ----------

#[derive(Serialize)]
struct DeviceView {
    id: String,
    name: String,
    kind: String,
    addr: String,
    screen_mode: String,
    vd_res: Option<String>,
    vd_dpi: Option<u32>,
    pkg: Option<String>,
    fps: Option<u32>,
    status: String,
    error: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
}

async fn api_list_devices(State(st): State<AppState>) -> Response {
    Json(device_views(&st)).into_response()
}

/// 渲染设备列表视图（带运行时状态/分辨率）
fn device_views(st: &AppState) -> Vec<DeviceView> {
    let devices = match st.db.list_devices() {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for d in devices {
        let (_, status, error) = st
            .devices
            .snapshot(&d.id)
            .map(|(_, s, e)| ((), s, e))
            .unwrap_or(((), crate::device::DeviceStatus::Offline, None));
        let (width, height) = st
            .devices
            .frame_cache(&d.id)
            .map(|fc| fc.dims())
            .unwrap_or((0, 0));
        out.push(DeviceView {
            id: d.id.clone(),
            name: d.name,
            kind: d.kind,
            addr: d.addr,
            screen_mode: match d.screen_mode {
                ScreenMode::Mirror => "mirror".into(),
                ScreenMode::Virtual => "virtual".into(),
            },
            vd_res: d.vd_res.clone(),
            vd_dpi: d.vd_dpi,
            pkg: d.pkg.clone(),
            fps: d.fps,
            status: serde_json::to_value(status)
                .unwrap()
                .as_str()
                .unwrap_or("offline")
                .to_string(),
            error,
            width: if width > 0 { Some(width) } else { None },
            height: if height > 0 { Some(height) } else { None },
        });
    }
    out
}

/// 扫描 `adb devices -l`，自动注册新发现的设备（USB / 无线 adb / 模拟器），
/// 已注册的跳过；返回完整设备列表（前端"刷新"时调用）
async fn api_scan_devices(State(st): State<AppState>) -> Response {
    // 解析/去重/入库逻辑在 DeviceManager::scan_and_sync（与启动自举共用）
    let added = match st.devices.scan_and_sync().await {
        Ok(n) => n,
        Err(e) => {
            return err_response(StatusCode::BAD_GATEWAY, &format!("adb devices 失败: {}", e))
        }
    };
    Json(serde_json::json!({"ok": true, "added": added, "devices": device_views(&st)}))
        .into_response()
}

#[derive(Deserialize)]
struct CreateDeviceReq {
    name: String,
    kind: String,
    addr: Option<String>,
    screen_mode: Option<String>,
    vd_res: Option<String>,
    vd_dpi: Option<u32>,
    pkg: Option<String>,
    fps: Option<u32>,
}

async fn api_create_device(
    State(st): State<AppState>,
    Json(req): Json<CreateDeviceReq>,
) -> Response {
    let id = Uuid::new_v4().simple().to_string();
    let device = Device {
        id,
        name: req.name,
        kind: req.kind,
        addr: req.addr.unwrap_or_default(),
        screen_mode: if req.screen_mode.as_deref() == Some("virtual") {
            ScreenMode::Virtual
        } else {
            ScreenMode::Mirror
        },
        vd_res: req.vd_res,
        vd_dpi: req.vd_dpi,
        pkg: req.pkg,
        fps: req.fps,
        created_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    };
    match st.devices.upsert_device(&device).await {
        Ok(_) => Json(serde_json::json!({"ok": true, "id": device.id})).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// 更新设备配置（屏幕模式/虚拟屏参数/游戏包名等）
async fn api_update_device(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateDeviceReq>,
) -> Response {
    let Some(existing) = (match st.db.get_device(&id) {
        Ok(d) => d,
        Err(e) => return err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }) else {
        return err_response(StatusCode::NOT_FOUND, "设备不存在");
    };
    let device = Device {
        id: id.clone(),
        name: req.name,
        kind: req.kind,
        addr: req.addr.unwrap_or(existing.addr),
        screen_mode: if req.screen_mode.as_deref() == Some("virtual") {
            ScreenMode::Virtual
        } else {
            ScreenMode::Mirror
        },
        vd_res: req.vd_res,
        vd_dpi: req.vd_dpi,
        pkg: req.pkg,
        fps: req.fps,
        created_at: existing.created_at,
    };
    // 配置变更后断开重连以生效。脚本运行中：会话被运行守卫拦下（旧参数跑完
    // 当前脚本，新配置下次连接生效），viewer 也无需踢、画面不闪断。
    // 空闲时：踢掉活跃 viewer（pusher 停止 + peer 关闭）→ 断开 → 浏览器端
    // onclose 自动重连恢复画面，否则旧 pusher 悬挂在已关闭的帧队列上画面定格
    if st.devices.has_running_scripts(&id) {
        info!(device = %id, "config changed while script running, session kept (applied on next connect)");
    } else {
        let kicked = {
            let mut map = st.viewers.lock().unwrap();
            map.remove(&id)
        };
        if let Some(h) = kicked {
            h.running.store(false, std::sync::atomic::Ordering::SeqCst);
            if let Some(p) = h.peer.upgrade() {
                let _ = p.close().await;
            }
            info!(device = %id, "config changed, kicked viewer");
        }
        st.devices.disconnect_device(&id, false).await;
    }
    match st.devices.upsert_device(&device).await {
        Ok(_) => Json(serde_json::json!({"ok": true, "id": device.id})).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn api_delete_device(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match st.devices.delete_device(&id).await {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ---------- 设备应用列表（供前端下拉选择游戏包名） ----------

/// 列出设备已安装的第三方应用：[{ label, pkg }]
/// 设备端 shell 无法解析应用显示名（label 在 APK 资源里），
/// 用包名最后两段生成友好名，完整包名始终一并展示，可搜索选择。
async fn api_device_apps(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let device = match st.db.get_device(&id) {
        Ok(Some(d)) => d,
        Ok(None) => return err_response(StatusCode::NOT_FOUND, "设备不存在"),
        Err(e) => return err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let serial = if device.addr.is_empty() {
        "usb".to_string()
    } else {
        device.addr.clone()
    };
    list_device_apps(&st, &serial).await
}

/// 按地址查询（添加设备弹窗里还没建记录时用）
async fn api_apps_by_addr(
    State(st): State<AppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let addr = q.get("addr").cloned().unwrap_or_default();
    let serial = if addr.is_empty() {
        "usb".to_string()
    } else {
        addr
    };
    list_device_apps(&st, &serial).await
}

async fn list_device_apps(st: &AppState, serial: &str) -> Response {
    // 优先用项目自带 scrcpy-server 的 list_apps 模式：能拿到真实应用名（label）+ 包名。
    // 注意：list_apps 退出时会删掉设备上的 jar，因此每次调用前都必须重新 push。
    let server_path = st.cfg.scrcpy_server.to_string_lossy().to_string();
    let push_ok = st
        .devices
        .adb
        .push(serial, &server_path, "/data/local/tmp/scrcpy-server.jar")
        .await
        .is_ok();
    let mut apps: Vec<serde_json::Value> = Vec::new();
    if push_ok {
        let shell_cmd = format!(
            "CLASSPATH=/data/local/tmp/scrcpy-server.jar app_process / com.genymobile.scrcpy.Server {} list_apps=true",
            crate::device::scrcpy::SCRCPY_VERSION
        );
        match st
            .devices
            .adb
            .run(
                &["-s", serial, "shell", &shell_cmd],
                Duration::from_secs(90),
            )
            .await
        {
            Ok(out) => {
                // 输出形如 " * 应用商店   com.xiaomi.market"（系统应用）/ " - 崩坏：星穹铁道  com.miHoYo.hkrpg"（第三方）
                for line in out.lines() {
                    let line = line.trim();
                    let Some(rest) = line.strip_prefix("- ") else {
                        continue;
                    };
                    let rest = rest.trim();
                    if rest.is_empty() {
                        continue;
                    }
                    let pkg = rest.rsplit_once(' ').map(|(_, p)| p.trim()).unwrap_or(rest);
                    let label = rest[..rest.len().saturating_sub(pkg.len())].trim();
                    if label.is_empty() || pkg.is_empty() || pkg.contains(' ') {
                        continue;
                    }
                    apps.push(serde_json::json!({ "label": label, "pkg": pkg }));
                }
            }
            Err(e) => {
                warn!(device = %serial, "scrcpy list_apps failed, fallback to pm list: {}", e);
            }
        }
    }
    // 兜底：pm list packages -3（只有包名，无显示名）
    if apps.is_empty() {
        match st
            .devices
            .adb
            .run(
                &["-s", serial, "shell", "pm", "list", "packages", "-3"],
                Duration::from_secs(20),
            )
            .await
        {
            Ok(out) => {
                for l in out.lines() {
                    if let Some(pkg) = l.strip_prefix("package:") {
                        let pkg = pkg.trim().to_string();
                        if !pkg.is_empty() {
                            apps.push(
                                serde_json::json!({ "label": pretty_app_label(&pkg), "pkg": pkg }),
                            );
                        }
                    }
                }
            }
            Err(e) => {
                return err_response(StatusCode::BAD_GATEWAY, &format!("读取应用列表失败: {}", e))
            }
        }
    }
    apps.sort_by(|a, b| a["label"].as_str().cmp(&b["label"].as_str()));
    Json(apps).into_response()
}

/// 包名 → 友好显示名：取最后两段、下划线转空格、首字母大写
/// （com.tencent.mm → "Tencent Mm"；com.miHoYo.hkrpg → "MiHoYo Hkrpg"）
fn pretty_app_label(pkg: &str) -> String {
    let mut segs: Vec<&str> = pkg.split('.').filter(|s| !s.is_empty()).collect();
    if segs.len() > 2 {
        segs.drain(0..segs.len() - 2);
    }
    segs.iter()
        .map(|s| {
            let s = s.replace('_', " ");
            let mut chars = s.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn api_connect_device(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match st.devices.connect_device(&id).await {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => err_response(StatusCode::BAD_GATEWAY, &format!("连接失败: {}", e)),
    }
}

/// 强制断开（管理动作，绕过运行守卫）：拆 scrcpy 会话。注意前端"断开连接"
/// 按钮已不再调用此接口（只断本地 WebRTC，会话交给空闲低功耗管理）
async fn api_disconnect_device(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    st.devices.disconnect_device(&id, true).await;
    Json(serde_json::json!({"ok": true})).into_response()
}

async fn api_screenshot(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match st.devices.screenshot(&id).await {
        Ok(png) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/png")
            .body(Body::from(png))
            .unwrap(),
        Err(e) => err_response(StatusCode::BAD_GATEWAY, &format!("截图失败: {}", e)),
    }
}

#[derive(Deserialize)]
struct ControlReq {
    #[serde(rename = "type")]
    cmd: String,
    x: Option<f32>,
    y: Option<f32>,
    x1: Option<f32>,
    y1: Option<f32>,
    x2: Option<f32>,
    y2: Option<f32>,
    duration: Option<u64>,
    text: Option<String>,
    keycode: Option<u32>,
    app: Option<String>,
}

/// 坐标合法性：有限 + 常规显示分辨率范围（scrcpy 按视频分辨率像素注入，
/// 下游 clamp 到 0..w-1；这里只挡 NaN/Infinity/天文数字）
fn valid_coord(v: f32) -> bool {
    v.is_finite() && (0.0..=100_000.0).contains(&v)
}

/// Android keycode 合法范围（0=UNKNOWN 不放行，上限留足到自定义厂商键值）
fn valid_keycode(kc: u32) -> bool {
    (1..=1000).contains(&kc)
}

/// 校验后的控制动作（借用请求字段；不再有隐式缺省）
enum Ctl<'a> {
    Tap(f32, f32),
    Swipe(f32, f32, f32, f32, u64),
    Text(&'a str),
    Press(u32),
    Home,
    Back,
    Recents,
    StartApp(&'a str),
    Rotate,
    Clipboard(&'a str),
}

/// 控制命令解析与校验（纯函数，可单测）：
/// 坐标/时长/文本长度/包名不合格一律显式拒绝，替代旧 unwrap_or 静默缺省
fn parse_ctl(req: &ControlReq) -> Result<Ctl<'_>, String> {
    match req.cmd.as_str() {
        "tap" => match (req.x, req.y) {
            (Some(x), Some(y)) if valid_coord(x) && valid_coord(y) => Ok(Ctl::Tap(x, y)),
            _ => Err("缺少或非法的 tap 坐标 x/y".into()),
        },
        "swipe" => {
            let (Some(x1), Some(y1), Some(x2), Some(y2)) = (req.x1, req.y1, req.x2, req.y2) else {
                return Err("缺少或非法的 swipe 坐标 x1/y1/x2/y2".into());
            };
            if ![x1, y1, x2, y2].iter().all(|v| valid_coord(*v)) {
                return Err("swipe 坐标超出合法范围".into());
            }
            if req.duration.is_some_and(|d| !(1..=600_000).contains(&d)) {
                return Err("duration 必须在 1..600000 ms 内".into());
            }
            Ok(Ctl::Swipe(x1, y1, x2, y2, req.duration.unwrap_or(300)))
        }
        "text" => {
            let text = req
                .text
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .ok_or("text 不能为空")?;
            // scrcpy 控制协议单条文本上限 300 字节，超长下游静默截断——显式拒绝
            if text.len() > 300 {
                return Err("text 超过 300 字节（协议上限）".into());
            }
            Ok(Ctl::Text(text))
        }
        "press" => {
            let kc = req
                .keycode
                .filter(|k| valid_keycode(*k))
                .ok_or("keycode 缺失或不在 1..1000")?;
            Ok(Ctl::Press(kc))
        }
        "home" => Ok(Ctl::Home),
        "back" => Ok(Ctl::Back),
        "recents" => Ok(Ctl::Recents),
        "start_app" => {
            let app = req
                .app
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .ok_or("app 包名不能为空")?;
            if app.len() > 255 {
                return Err("app 包名超过 255 字节".into());
            }
            // "+" 强启 / "?" 按名搜索允许透传；裸包名必须是安全包名字符集
            if let Some(pkg) = app.strip_prefix('+') {
                if pkg.is_empty() || !crate::device::adb::is_safe_pkg(pkg) {
                    return Err("+ 前缀后须为合法包名".into());
                }
            } else if let Some(name) = app.strip_prefix('?') {
                if name.trim().len() < 2 {
                    return Err("? 搜索名过短".into());
                }
            } else if !crate::device::adb::is_safe_pkg(app) {
                return Err("包名非法（只允许字母数字 . _）".into());
            }
            Ok(Ctl::StartApp(app))
        }
        "rotate" => Ok(Ctl::Rotate),
        "clipboard" => {
            let text = req
                .text
                .as_deref()
                .filter(|t| !t.is_empty())
                .ok_or("clipboard 文本不能为空")?;
            // scrcpy 剪贴板协议上限 128KiB，超长下游静默截断——显式拒绝
            if text.len() > 131_072 {
                return Err("clipboard 文本超过 131072 字节（协议上限）".into());
            }
            Ok(Ctl::Clipboard(text))
        }
        _ => Err("unknown command".into()),
    }
}

async fn api_control(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ControlReq>,
) -> Response {
    // 先纯校验（与设备无关，离线设备也能拿到明确 400），再取会话执行
    let ctl = match parse_ctl(&req) {
        Ok(c) => c,
        Err(msg) => return err_response(StatusCode::BAD_REQUEST, &msg),
    };
    let Some(session) = st.devices.session(&id) else {
        return err_response(StatusCode::CONFLICT, "设备未连接");
    };
    let result = match ctl {
        Ctl::Tap(x, y) => session.tap(x, y).await,
        Ctl::Swipe(x1, y1, x2, y2, duration_ms) => session.swipe(x1, y1, x2, y2, duration_ms).await,
        Ctl::Text(text) => session.inject_text(text).await,
        Ctl::Press(kc) => session.press_key(kc).await,
        Ctl::Home => session.press_key(3).await,
        Ctl::Back => session.press_key(4).await,
        Ctl::Recents => session.press_key(187).await,
        Ctl::StartApp(app) => session.start_app(app).await,
        Ctl::Rotate => session.rotate_device().await,
        Ctl::Clipboard(text) => session.set_clipboard(text, false).await,
    };
    match result {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => err_response(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

// ---------- 模板（按应用分区 data/<pkg>/tmpl） ----------

#[derive(Deserialize)]
struct PkgQuery {
    pkg: Option<String>,
}

/// 校验必需的 pkg 参数（应用包名 = 分区名）：缺失/空串/非法包名均返回 None
/// （调用方各自回 400，不直接透出内部 Response，避免大 Err 载荷）
fn require_pkg(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(crate::scripts::sanitize_part)
}

/// 列出模板：?pkg= 指定分区时只列该分区，否则跨分区全列（条目带 pkg 字段）
async fn api_list_templates(State(st): State<AppState>, Query(q): Query<PkgQuery>) -> Response {
    let pkgs: Vec<String> = match q.pkg.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => match crate::scripts::sanitize_part(p) {
            Some(v) => vec![v],
            None => {
                return err_response(
                    StatusCode::BAD_REQUEST,
                    "应用包名非法（只允许字母数字 . _ -）",
                )
            }
        },
        None => st.scripts.partitions(),
    };
    let mut out = Vec::new();
    for pkg in pkgs {
        let dir = st.scripts.tmpl_dir(&pkg);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                // 模板目录专用：列出所有非隐藏文件（模板名可能带 .png/.jpg，也可能是 随机名字#x1_y1_x2_y2 这种带小数点无后缀名）
                let fname = e.file_name().to_string_lossy().to_string();
                if e.path().is_file() && !fname.starts_with('.') {
                    let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                    // mtime（unix 秒）：前端按修改时间倒序排模板列表
                    let mtime = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    out.push(serde_json::json!({"name": fname, "size": size, "mtime": mtime, "pkg": pkg}));
                }
            }
        }
    }
    Json(out).into_response()
}

#[derive(Deserialize)]
struct UploadTemplateReq {
    name: String,
    data_b64: String,
    pkg: String,
}

async fn api_upload_template(
    State(st): State<AppState>,
    Json(req): Json<UploadTemplateReq>,
) -> Response {
    let Some(pkg) = require_pkg(Some(&req.pkg)) else {
        return err_response(
            StatusCode::BAD_REQUEST,
            "应用包名非法（只允许字母数字 . _ -）",
        );
    };
    // base64 合法性与体积先于解码校验（4/3 膨胀后 16MiB ≈ 原始 12MiB 内的护栏）
    const MAX_B64_LEN: usize = (matcher::TEMPLATE_MAX_INPUT_BYTES / 3 + 1) * 4;
    if req.data_b64.len() > MAX_B64_LEN {
        return err_response(
            StatusCode::BAD_REQUEST,
            "图片超过上传上限（10 MiB），请裁剪后再试",
        );
    }
    let orig = match base64::engine::general_purpose::STANDARD.decode(&req.data_b64) {
        Ok(b) => b,
        Err(e) => return err_response(StatusCode::BAD_REQUEST, &format!("base64 解码失败: {}", e)),
    };
    // 统一重编码为灰度 PNG（匹配零损失 + 大幅减小体积；字节数/像素炸弹
    // 双层硬限在 reencode_template_gray_png 内收口）
    let bytes = match matcher::reencode_template_gray_png(&orig) {
        Ok(b) => b,
        Err(e) => return err_response(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let dir = st.scripts.tmpl_dir(&pkg);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let name = sanitize_filename(&req.name);
    let path = dir.join(&name);
    if let Err(e) = crate::scripts::atomic_write(&path, &bytes) {
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    Json(
        serde_json::json!({"ok": true, "name": name, "size": bytes.len(), "orig_size": orig.len()}),
    )
    .into_response()
}

async fn api_delete_template(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<PkgQuery>,
) -> Response {
    let Some(pkg) = require_pkg(q.pkg.as_deref()) else {
        return err_response(StatusCode::BAD_REQUEST, "缺少 pkg 参数（应用包名）");
    };
    let path = st.scripts.tmpl_dir(&pkg).join(sanitize_filename(&name));
    match std::fs::remove_file(&path) {
        Ok(_) => {
            st.scripts.cleanup_partition(&pkg); // 分区 yaml/tmpl 都空了则清理目录
            Json(serde_json::json!({"ok": true})).into_response()
        }
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct RenameTemplateReq {
    name: String,
}

/// 重命名模板：把旧文件字节写入新文件名，再删除旧文件
async fn api_rename_template(
    State(st): State<AppState>,
    Path(old_name): Path<String>,
    Query(q): Query<PkgQuery>,
    Json(req): Json<RenameTemplateReq>,
) -> Response {
    let Some(pkg) = require_pkg(q.pkg.as_deref()) else {
        return err_response(StatusCode::BAD_REQUEST, "缺少 pkg 参数（应用包名）");
    };
    let dir = st.scripts.tmpl_dir(&pkg);
    let old_path = dir.join(sanitize_filename(&old_name));
    let new_name = sanitize_filename(&req.name);
    if new_name == sanitize_filename(&old_name) {
        return err_response(StatusCode::BAD_REQUEST, "名称未变化");
    }
    let new_path = dir.join(&new_name);
    if new_path.exists() {
        return err_response(StatusCode::BAD_REQUEST, "已存在同名模板");
    }
    let bytes = match std::fs::read(&old_path) {
        Ok(b) => b,
        Err(_) => return err_response(StatusCode::NOT_FOUND, "模板不存在"),
    };
    if let Err(e) = crate::scripts::atomic_write(&new_path, &bytes) {
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    if std::fs::remove_file(&old_path).is_err() {
        let _ = std::fs::remove_file(&new_path);
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, "旧模板删除失败");
    }
    Json(serde_json::json!({"ok": true, "name": new_name})).into_response()
}

/// 返回模板图片原始字节（PNG/JPEG），供前端缩略图与预览使用。
/// Cache-Control: no-cache —— 模板被同名覆盖上传后浏览器必须重新拉取。
async fn api_get_template_image(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<PkgQuery>,
) -> Response {
    let Some(pkg) = require_pkg(q.pkg.as_deref()) else {
        return err_response(StatusCode::BAD_REQUEST, "缺少 pkg 参数（应用包名）");
    };
    let path = st.scripts.tmpl_dir(&pkg).join(sanitize_filename(&name));
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return err_response(StatusCode::NOT_FOUND, "模板不存在"),
    };
    let mime = match path
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        _ => "image/png",
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        bytes,
    )
        .into_response()
}

#[derive(Deserialize)]
struct TestTemplateReq {
    device_id: String,
    threshold: Option<f32>,
    region: Option<[u32; 4]>,
    pkg: String,
}

async fn api_test_template(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<TestTemplateReq>,
) -> Response {
    let Some(pkg) = require_pkg(Some(&req.pkg)) else {
        return err_response(
            StatusCode::BAD_REQUEST,
            "应用包名非法（只允许字母数字 . _ -）",
        );
    };
    let tpl_path = st.scripts.tmpl_dir(&pkg).join(sanitize_filename(&name));
    let tpl_bytes = match std::fs::read(&tpl_path) {
        Ok(b) => b,
        Err(_) => return err_response(StatusCode::NOT_FOUND, "模板不存在"),
    };
    let screen = match st.devices.screenshot(&req.device_id).await {
        Ok(s) => s,
        Err(e) => return err_response(StatusCode::BAD_GATEWAY, &format!("截图失败: {}", e)),
    };
    let mr = matcher::MatchRequest {
        screen_png: screen,
        template_png: tpl_bytes,
        threshold: req.threshold,
        region: req.region,
    };
    match matcher::match_template(&mr) {
        Ok(Some(m)) => Json(serde_json::json!({"hit": true, "x": m.x, "y": m.y, "width": m.width, "height": m.height, "score": m.score})).into_response(),
        Ok(None) => Json(serde_json::json!({"hit": false})).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ---------- 脚本 ----------

async fn api_list_scripts(State(st): State<AppState>) -> Response {
    match st.scripts.list() {
        Ok(s) => Json(s).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct SaveScriptReq {
    id: Option<String>,
    name: String,
    content: String,
    /// 目标应用分区（设备配置的应用包名）
    pkg: String,
}

async fn api_save_script(State(st): State<AppState>, Json(req): Json<SaveScriptReq>) -> Response {
    if req.name.trim().is_empty() {
        return err_response(StatusCode::BAD_REQUEST, "脚本名不能为空");
    }
    if crate::scripts::sanitize_part(&req.pkg).is_none() {
        return err_response(
            StatusCode::BAD_REQUEST,
            "应用包名非法（只允许字母数字 . _ -）",
        );
    }
    match st
        .scripts
        .save(req.id.as_deref(), &req.pkg, &req.name, &req.content)
    {
        Ok(s) => {
            Json(serde_json::json!({"ok": true, "id": s.id, "package": s.package, "name": s.name}))
                .into_response()
        }
        Err(e) => err_response(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn api_delete_script(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match st.scripts.delete(&id) {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// 导出整分区快照 zip（?pkg= 指定应用分区）：yaml/ 全部脚本 + tmpl/ 全部模板
async fn api_export_partition(State(st): State<AppState>, Query(q): Query<PkgQuery>) -> Response {
    let Some(pkg) = require_pkg(q.pkg.as_deref()) else {
        return err_response(StatusCode::BAD_REQUEST, "缺少 pkg 参数（应用包名）");
    };
    match st.scripts.export_partition(&pkg) {
        Ok((filename, bytes)) => zip_response(&filename, bytes),
        Err(e) => err_response(StatusCode::NOT_FOUND, &e.to_string()),
    }
}

/// zip 下载响应：文件名可能是 unicode，用 RFC 5987 filename*
/// （percent-encoded UTF-8），直接塞非 ASCII 进 header 会被 hyper 拒绝
fn zip_response(filename: &str, bytes: Vec<u8>) -> Response {
    let enc: String = filename.bytes().map(|b| format!("%{:02X}", b)).collect();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename*=UTF-8''{}", enc),
        )
        .body(Body::from(bytes))
        .unwrap()
}

#[derive(Deserialize)]
struct ImportQuery {
    #[serde(default)]
    confirm: Option<String>,
    /// 目标应用分区（应用包名，必填）
    #[serde(default)]
    pkg: Option<String>,
}

/// 导入分区快照 zip（body 为原始 zip 字节，?pkg= 指定目标分区）。
/// confirm 缺省/false：只解析并返回同名冲突列表（前端二次确认）；
/// confirm=1/true：落盘，同名替换。
async fn api_import_script(
    State(st): State<AppState>,
    Query(q): Query<ImportQuery>,
    body: axum::body::Bytes,
) -> Response {
    let confirm = matches!(q.confirm.as_deref(), Some("1") | Some("true"));
    let Some(pkg) = require_pkg(q.pkg.as_deref()) else {
        return err_response(StatusCode::BAD_REQUEST, "缺少 pkg 参数（应用包名）");
    };
    match st.scripts.import(&body, &pkg, confirm) {
        Ok(rep) => Json(rep).into_response(),
        Err(e) => err_response(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct RunScriptReq {
    device_id: String,
    /// 从第几个 step 开始运行（0=从头；前端选中某个 "- " 逻辑行时传入）
    #[serde(default)]
    start_index: Option<usize>,
    /// 直接运行指定函数体（Console 选中函数名行 / 函数体内的行时传入）；
    /// start_index 此时是函数体内的步骤序号——0（函数名行）先检查函数 cond，
    /// >0（体内行）跳过 cond 从该步执行
    #[serde(default)]
    func: Option<String>,
}

/// 手动运行的完成钩子：终态摘要行落库（realtime 模式引擎日志已实时入库，
/// 这里只补一条终局提示，与旧实现的"脚本执行完成/失败"行语义对齐）
fn manual_finish_hook(db: Db) -> crate::run_manager::FinishHook {
    use crate::run_manager::RunOutcome;
    Arc::new(move |rec, outcome| match outcome {
        RunOutcome::Success(_) => {
            let _ = db.add_log(&rec.device_id, &rec.script_id, "success", "脚本执行完成");
        }
        RunOutcome::Failed(msg, _) => {
            let _ = db.add_log(
                &rec.device_id,
                &rec.script_id,
                "error",
                &format!("脚本执行失败: {}", msg),
            );
        }
        RunOutcome::Cancelled(_) => {
            let _ = db.add_log(&rec.device_id, &rec.script_id, "info", "脚本已停止");
        }
    })
}

async fn api_run_script(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RunScriptReq>,
) -> Response {
    // 脚本存在性先校验（404 优先于设备冲突）
    let Some(script) = (match st.scripts.get(&id) {
        Ok(s) => s,
        Err(e) => return err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }) else {
        return err_response(StatusCode::NOT_FOUND, "脚本不存在");
    };
    if req.device_id.trim().is_empty() {
        return err_response(StatusCode::BAD_REQUEST, "缺少 device_id");
    }
    // RUN-002 契约：启动即返回 202 {run_id, state:"starting"}，不等脚本结束；
    // 设备级互斥冲突 → 409 {error:"device_busy", run_id, script_id, source, started_at}
    let rreq = crate::run_manager::StartRequest {
        run_id: String::new(),
        device_id: req.device_id.clone(),
        script_id: id.clone(),
        content: script.content.clone(),
        source: crate::run_manager::RunSource::Manual,
        task_id: None,
        scheduled_at: None,
        start_index: req.start_index.unwrap_or(0),
        run_func: req.func.filter(|s| !s.trim().is_empty()),
        realtime_logs: true,
    };
    match st
        .runs
        .submit(rreq, Some(manual_finish_hook(st.db.clone())))
    {
        Ok(rec) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "run_id": rec.run_id,
                "state": serde_json::to_value(rec.state).unwrap_or_default(),
            })),
        )
            .into_response(),
        Err(crate::run_manager::StartError::Conflict(busy)) => {
            (StatusCode::CONFLICT, Json(busy.busy_payload())).into_response()
        }
        Err(crate::run_manager::StartError::ShuttingDown) => {
            err_response(StatusCode::SERVICE_UNAVAILABLE, "shutting_down")
        }
    }
}

/// 旧停止端点（兼容窗口）：按 script_id 定位活动 run 并取消。
/// 同一脚本可能在不同设备各有一个实例——逐个取消。响应保持旧形状 {ok:true}。
async fn api_stop_script(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    for run in st.runs.active_for_script(&id) {
        st.runs.cancel(&run.run_id);
    }
    Json(serde_json::json!({"ok": true})).into_response()
}

/// 旧脚本运行查询（兼容窗口）：内部经 RunManager 反查该脚本的任意活动实例
async fn api_script_status(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let running = !st.runs.active_for_script(&id).is_empty();
    Json(serde_json::json!({"running": running})).into_response()
}

/// 设备当前运行查询（前端刷新恢复运行态）：
/// 新契约 active:true + 完整 RunRecord / active:false；
/// （旧 {running,script_id,script_name} 形状已随阶段 3 废弃）
async fn api_device_run(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match st.runs.active_for_device(&id) {
        Some(rec) => {
            let mut v = serde_json::to_value(&rec).unwrap_or_else(|_| serde_json::json!({}));
            v["active"] = serde_json::json!(true);
            Json(v).into_response()
        }
        None => Json(serde_json::json!({"active": false})).into_response(),
    }
}

/// GET /api/runs/:run_id → 完整 RunRecord（活动在册 + 终态档案均可查；未知 404）
async fn api_get_run(State(st): State<AppState>, Path(run_id): Path<String>) -> Response {
    match st.runs.get_run(&run_id) {
        Some(rec) => Json(serde_json::to_value(&rec).unwrap_or_else(|_| serde_json::json!({})))
            .into_response(),
        None => err_response(StatusCode::NOT_FOUND, "run_not_found"),
    }
}

/// POST /api/runs/:run_id/cancel → 202 {"cancelling":true}；
/// 终态由客户端随后 GET /api/runs/:id 确认（cancelled/success/failed）
async fn api_cancel_run(State(st): State<AppState>, Path(run_id): Path<String>) -> Response {
    use crate::run_manager::CancelOutcome;
    match st.runs.cancel(&run_id) {
        CancelOutcome::Accepted => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"cancelling": true})),
        )
            .into_response(),
        CancelOutcome::NotFound => err_response(StatusCode::NOT_FOUND, "run_not_found"),
        CancelOutcome::AlreadyFinished(state) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "already_finished",
                "state": serde_json::to_value(state).unwrap_or_default(),
            })),
        )
            .into_response(),
    }
}

// ---------- 定时任务 ----------

async fn api_list_tasks(State(st): State<AppState>) -> Response {
    match st.db.list_tasks() {
        Ok(tasks) => {
            let out: Vec<serde_json::Value> = tasks
                .into_iter()
                .map(|t| {
                    let next = if t.enabled { next_run(&t.cron).map(|x| x.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_else(|| "-".into()) } else { "-".into() };
                    serde_json::json!({
                        "id": t.id, "name": t.name, "cron": t.cron, "script_id": t.script_id,
                        "device_id": t.device_id, "enabled": t.enabled, "last_result": t.last_result,
                        "last_run_at": t.last_run_at, "next_run": next
                    })
                })
                .collect();
            Json(out).into_response()
        }
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct SaveTaskReq {
    id: Option<String>,
    name: String,
    cron: String,
    script_id: String,
    device_id: String,
    enabled: Option<bool>,
}

async fn api_save_task(State(st): State<AppState>, Json(req): Json<SaveTaskReq>) -> Response {
    // 校验 cron（5/6/7 字段）
    if !crate::scheduler::validate_cron(&req.cron) {
        return err_response(StatusCode::BAD_REQUEST, "cron 表达式无效");
    }
    let id = req
        .id
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let existing = st
        .db
        .list_tasks()
        .ok()
        .and_then(|ts| ts.into_iter().find(|t| t.id == id));
    let task = Task {
        id,
        name: req.name,
        cron: req.cron,
        script_id: req.script_id,
        device_id: req.device_id,
        enabled: req
            .enabled
            .unwrap_or(existing.as_ref().map(|t| t.enabled).unwrap_or(true)),
        last_result: existing.as_ref().and_then(|t| t.last_result.clone()),
        last_run_at: existing.as_ref().and_then(|t| t.last_run_at.clone()),
        created_at: existing
            .as_ref()
            .map(|t| t.created_at.clone())
            .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()),
    };
    match st.db.upsert_task(&task) {
        Ok(_) => Json(serde_json::json!({"ok": true, "id": task.id})).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn api_delete_task(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match st.db.delete_task(&id) {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// 立即运行定时任务（RUN-002 契约）：202 {run_id} 提交即返回，不占用 HTTP
/// 连接等任务完成；设备冲突 409 device_busy；停机 drain 中 503。
async fn api_run_task_now(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    use crate::scheduler::RunNowError;
    let Some(task) = (match st.db.list_tasks() {
        Ok(t) => t,
        Err(e) => return err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    })
    .into_iter()
    .find(|t| t.id == id) else {
        return err_response(StatusCode::NOT_FOUND, "任务不存在");
    };
    match st.scheduler.run_now(&task).await {
        Ok(run_id) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "run_id": run_id })),
        )
            .into_response(),
        Err(RunNowError::Start(crate::run_manager::StartError::Conflict(busy))) => {
            (StatusCode::CONFLICT, Json(busy.busy_payload())).into_response()
        }
        Err(RunNowError::Start(crate::run_manager::StartError::ShuttingDown)) => {
            err_response(StatusCode::SERVICE_UNAVAILABLE, "shutting_down")
        }
        Err(RunNowError::ScriptMissing | RunNowError::Io(_)) => {
            err_response(StatusCode::BAD_REQUEST, "脚本不存在或读取失败")
        }
    }
}

// ---------- 日志 ----------

#[derive(Deserialize)]
struct LogQuery {
    device_id: Option<String>,
    level: Option<String>,
    limit: Option<i64>,
}

/// 日志查询条数钳制：1..=1000（阶段 2 SEC-004），缺省 200。
/// 非法值钳进合法区间而非报错——前端只需要"少拿点"，不存在语义歧义
fn clamp_log_limit(limit: Option<i64>) -> i64 {
    match limit {
        Some(n) if n < 1 => 1,
        Some(n) => n.min(1000),
        None => 200,
    }
}

async fn api_list_logs(State(st): State<AppState>, Query(q): Query<LogQuery>) -> Response {
    match st.db.list_logs(
        q.device_id.as_deref(),
        q.level.as_deref(),
        clamp_log_limit(q.limit),
    ) {
        Ok(logs) => Json(logs).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn api_clear_logs(State(st): State<AppState>) -> Response {
    match st.db.clear_logs() {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// 操作记录 YAML 模板（前端 alt 模式追加到编辑区用，来源 config.toml [op_templates]）
async fn api_op_templates(State(st): State<AppState>) -> Response {
    Json(st.cfg.op_templates.clone()).into_response()
}

/// 优雅停机（gamer.ps1 stop/rebuild 先调此端点，超时才兜底硬杀）：
/// ① RunManager drain——先拒绝新 run（503），等待活动任务结束，超时强停
/// （RUN-001：服务关闭先停止接收新任务再取消/等待活动任务）；
/// ② 踢所有 viewer（只关 peer 不发 taken_over——那是"被顶替"信号会让页面放弃自动
/// 重连；普通断开页面会在服务重启后自动重连）；③ 拆所有 scrcpy 会话/清 reverse
/// 隧道（防孤儿 adb 楔死后续连接，见 DeviceManager::shutdown_all）；④ 触发进程退出
async fn api_shutdown(State(st): State<AppState>) -> Response {
    info!("graceful shutdown requested (POST /api/shutdown)");
    // RunManager drain（宽限 10s；活动脚本短则提前返回）
    st.runs.begin_shutdown(Duration::from_secs(10)).await;
    // 踢 viewer：关 WebRTC peer（ws 循环随 peer_closed 退出），否则常驻 WS 连接
    // 会让 axum 的 graceful drain 一直等不到收尾
    let viewers = st.viewers.lock().unwrap().clone();
    for (id, vh) in &viewers {
        info!(device = %id, "shutdown: closing viewer peer");
        vh.running.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(p) = vh.peer.upgrade() {
            let _ = p.close().await;
        }
    }
    st.devices.shutdown_all().await;
    let _ = st.shutdown.send(true);
    Json(serde_json::json!({"ok": true})).into_response()
}

// ---------- 工具 ----------

fn err_response(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({"error": msg}))).into_response()
}

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' || c == ' ' || c == '#' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unnamed.png".into()
    } else {
        cleaned
    }
}

#[allow(dead_code)]
fn _unused(_: LogEntry, _: Duration) {}

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
        assert_eq!(body["ready"].is_boolean(), true);
        for name in ["data_dir", "sqlite", "scrcpy_server", "adb", "ffmpeg"] {
            assert_eq!(body["checks"][name]["ok"].is_boolean(), true, "{name}");
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
        // 带合法 cookie 则放行进入 upgrade 处理器（此处无 Upgrade 头，
        // axum 会以非 101 拒绝，但绝不再是我们 guard 的 401）
        let ck = cookie_of(&login(&t.app).await);
        let sid = first_cookie_pair(&ck);
        let resp = send(
            &t.app,
            req(
                "GET",
                "/ws/device/d1",
                None,
                &[(header::COOKIE.to_string(), sid.clone())],
                None,
            ),
        )
        .await;
        assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);

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

        // clipboard 上限与空值
        assert!(parse_ctl(&ctl_req(r#"{"type":"clipboard","text":""}"#)).is_err());

        // 无参动作不受影响
        assert!(parse_ctl(&ctl_req(r#"{"type":"home"}"#)).is_ok());
        assert!(parse_ctl(&ctl_req(r#"{"type":"rotate"}"#)).is_ok());

        // 未知命令仍 400 文案
        match parse_ctl(&ctl_req(r#"{"type":"touch","action":"down"}"#)) {
            Err(e) => assert_eq!(e, "unknown command"),
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
}
