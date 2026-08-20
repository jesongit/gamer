//! HTTP REST + WebSocket API
//!
//! REST: 设备 CRUD / 连接控制 / 截图 / 模板 / 脚本 / 任务 / 日志 / 认证
//! WS:   WebRTC 信令（/ws/device/:id）

mod ws;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::device::DeviceManager;
use crate::engine::Runner;
use crate::matcher;
use crate::scheduler::{next_run, Scheduler};
use crate::store::{Db, Device, LogEntry, Script, ScreenMode, Task};

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub devices: Arc<DeviceManager>,
    pub scheduler: Arc<Scheduler>,
    pub runner: Arc<Runner>,
    pub cfg: Config,
    /// 脚本运行停止标志
    pub run_stops: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>>,
    /// 每设备的活跃 viewer（WebRTC 会话）注册表（main.rs 创建，与 Scheduler 共享）：
    /// 同一设备只允许一个活跃 viewer——新连接踢掉旧连接（旧 pusher 停止 + 旧 peer 关闭），
    /// 避免多连接多推流导致浏览器端 srcObject 串流/资源浪费。
    /// control_dc 字段供引擎反向推送脚本可视化事件（tap/swipe/匹配命中）。
    pub viewers: crate::webrtc::ViewerMap,
}

pub fn build_router(
    db: Db,
    devices: Arc<DeviceManager>,
    scheduler: Arc<Scheduler>,
    cfg: Config,
    viewers: crate::webrtc::ViewerMap,
) -> Router {
    let runner = Arc::new(Runner::new(db.clone(), devices.clone(), viewers.clone()));
    let state = AppState {
        db,
        devices,
        scheduler,
        runner,
        cfg,
        run_stops: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        viewers,
    };

    // 视频静默看门狗：自动重连断流设备（见 spawn_watchdog）
    spawn_watchdog(state.clone());
    Router::new()
        .route("/api/login", post(api_login))
        .route("/api/devices", get(api_list_devices).post(api_create_device))
        .route("/api/devices/scan", post(api_scan_devices))
        .route("/api/devices/:id", delete(api_delete_device).put(api_update_device))
        .route("/api/devices/:id/apps", get(api_device_apps))
        .route("/api/apps", get(api_apps_by_addr))
        .route("/api/devices/:id/connect", post(api_connect_device))
        .route("/api/devices/:id/disconnect", post(api_disconnect_device))
        .route("/api/devices/:id/screenshot", post(api_screenshot))
        .route("/api/devices/:id/control", post(api_control))
        .route("/api/templates", get(api_list_templates).post(api_upload_template))
        .route("/api/templates/:name", delete(api_delete_template).put(api_rename_template))
        .route("/api/templates/:name/image", get(api_get_template_image))
        .route("/api/templates/:name/test", post(api_test_template))
        .route("/api/scripts", get(api_list_scripts).post(api_save_script))
        .route("/api/scripts/:id", delete(api_delete_script))
        .route("/api/scripts/:id/run", post(api_run_script))
        .route("/api/scripts/:id/stop", post(api_stop_script))
        .route("/api/scripts/:id/status", get(api_script_status))
        .route("/api/tasks", get(api_list_tasks).post(api_save_task))
        .route("/api/tasks/:id", delete(api_delete_task))
        .route("/api/tasks/:id/run", post(api_run_task_now))
        .route("/api/logs", get(api_list_logs).delete(api_clear_logs))
        .route("/api/op-templates", get(api_op_templates))
        .route("/ws/device/:id", get(ws::ws_device))
        .fallback_service(ServeDir::new("./web-dist").fallback(ServeDir::new("./web-dist/index.html")))
        .layer(DefaultBodyLimit::max(20 * 1024 * 1024))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// 视频静默看门狗：设备在线但视频流超过阈值无新帧时的处置。
/// 兜底一切"静默断流"场景：无线 adb 隧道假死、scrcpy-server 卡死、
/// 设备编码器停摆等——否则浏览器画面会永久定格在最后一帧。
///
/// 注意虚拟屏**无应用时编码器完全不出帧**（黑屏 0 帧，连 i-frame-interval 的
/// IDR 都没有）——静默≠故障：
/// - 无 viewer 且无脚本：静默大多是黑屏空转，重连只会白白 churn（重建会话/
///   虚拟屏，还可能触发 adb 异常），直接断开进低功耗，下次脚本/投屏自动重连
/// - 有 viewer 或脚本运行中：先 reset_video 请求关键帧探测编码器是否存活
///   （黑屏虚拟屏重连后照样黑）；探测后仍静默才断开重连（真断流兜底，慢 15s）
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
                let (has_viewer, served_ago_ms) = {
                    let map = st.viewers.lock().unwrap();
                    match map.get(&id) {
                        Some(h) => {
                            let t = h.last_serve.load(std::sync::atomic::Ordering::Relaxed);
                            (true, if t == 0 { i64::MAX } else { now_unix_ms() - t })
                        }
                        None => (false, i64::MAX),
                    }
                };
                let running = st.devices.has_running_scripts(&id);
                if !has_viewer && !running {
                    warn!(device = %id, idle_ms = idle, "video silent, no viewer/script: disconnect (low-power)");
                    nudged.remove(&id);
                    st.devices.disconnect_device(&id).await;
                    continue;
                }
                // viewer 正在被投喂（设备 0 帧是静态屏常态，pusher 静止补帧还活着）
                // → 会话对 viewer 是健康的：不 nudge（reset 反而打断补帧，MTK 静态
                // 屏 reset 后长时间不出 IDR → 浏览器断供被前端杀连接），也不走
                // 35s 兜底重连（静态屏挂机会话会被无限循环重连踢 viewer）。
                // 真断流时 pusher 退出、last_serve 过期，仍走 nudge → 15s → 重连兜底
                if has_viewer && served_ago_ms.max(0) < 10_000 {
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
                st.devices.disconnect_device(&id).await;
                if let Err(e) = st.devices.connect_device(&id).await {
                    warn!(device = %id, err = %e, "auto-reconnect failed");
                }
            }
        }
    });
}

// ---------- 认证 ----------

#[derive(Deserialize)]
struct LoginReq {
    user: String,
    password: String,
}

#[derive(Serialize)]
struct LoginResp {
    token: String,
}

async fn api_login(State(st): State<AppState>, Json(req): Json<LoginReq>) -> Response {
    if req.user == "admin" && req.password == st.cfg.password {
        Json(LoginResp { token: "demo-token".into() }).into_response()
    } else {
        (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "用户名或密码错误"}))).into_response()
    }
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
        let (_, status, error) = st.devices.snapshot(&d.id).map(|(_, s, e)| ((), s, e)).unwrap_or(((), crate::device::DeviceStatus::Offline, None));
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
            status: serde_json::to_value(status).unwrap().as_str().unwrap_or("offline").to_string(),
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
        Err(e) => return err_response(StatusCode::BAD_GATEWAY, &format!("adb devices 失败: {}", e)),
    };
    Json(serde_json::json!({"ok": true, "added": added, "devices": device_views(&st)})).into_response()
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

async fn api_create_device(State(st): State<AppState>, Json(req): Json<CreateDeviceReq>) -> Response {
    let id = Uuid::new_v4().simple().to_string();
    let device = Device {
        id,
        name: req.name,
        kind: req.kind,
        addr: req.addr.unwrap_or_default(),
        screen_mode: if req.screen_mode.as_deref() == Some("virtual") { ScreenMode::Virtual } else { ScreenMode::Mirror },
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
async fn api_update_device(State(st): State<AppState>, Path(id): Path<String>, Json(req): Json<CreateDeviceReq>) -> Response {
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
        screen_mode: if req.screen_mode.as_deref() == Some("virtual") { ScreenMode::Virtual } else { ScreenMode::Mirror },
        vd_res: req.vd_res,
        vd_dpi: req.vd_dpi,
        pkg: req.pkg,
        fps: req.fps,
        created_at: existing.created_at,
    };
    // 配置变更后自动断开重连；先踢掉该设备的活跃 viewer（pusher 停止 + peer 关闭），
    // 浏览器端 onclose → 自动重连（前端带页面锁，会重新建立会话并恢复画面），
    // 否则旧 viewer 的 pusher 悬挂在已关闭的帧队列上，浏览器画面定格/黑屏
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
    st.devices.disconnect_device(&id).await;
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
    let serial = if device.addr.is_empty() { "usb".to_string() } else { device.addr.clone() };
    list_device_apps(&st, &serial).await
}

/// 按地址查询（添加设备弹窗里还没建记录时用）
async fn api_apps_by_addr(State(st): State<AppState>, Query(q): Query<std::collections::HashMap<String, String>>) -> Response {
    let addr = q.get("addr").cloned().unwrap_or_default();
    let serial = if addr.is_empty() { "usb".to_string() } else { addr };
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
        match st.devices.adb.run(&["-s", serial, "shell", &shell_cmd], Duration::from_secs(90)).await {
            Ok(out) => {
                // 输出形如 " * 应用商店   com.xiaomi.market"（系统应用）/ " - 崩坏：星穹铁道  com.miHoYo.hkrpg"（第三方）
                for line in out.lines() {
                    let line = line.trim();
                    let Some(rest) = line.strip_prefix("- ") else { continue };
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
        match st.devices.adb.run(&["-s", serial, "shell", "pm", "list", "packages", "-3"], Duration::from_secs(20)).await {
            Ok(out) => {
                for l in out.lines() {
                    if let Some(pkg) = l.strip_prefix("package:") {
                        let pkg = pkg.trim().to_string();
                        if !pkg.is_empty() {
                            apps.push(serde_json::json!({ "label": pretty_app_label(&pkg), "pkg": pkg }));
                        }
                    }
                }
            }
            Err(e) => return err_response(StatusCode::BAD_GATEWAY, &format!("读取应用列表失败: {}", e)),
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

async fn api_disconnect_device(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    st.devices.disconnect_device(&id).await;
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

async fn api_control(State(st): State<AppState>, Path(id): Path<String>, Json(req): Json<ControlReq>) -> Response {
    let Some(session) = st.devices.session(&id) else {
        return err_response(StatusCode::CONFLICT, "设备未连接");
    };
    let result = match req.cmd.as_str() {
        "tap" => session.tap(req.x.unwrap_or(0.0), req.y.unwrap_or(0.0)).await,
        "swipe" => session
            .swipe(req.x1.unwrap_or(0.0), req.y1.unwrap_or(0.0), req.x2.unwrap_or(0.0), req.y2.unwrap_or(0.0), req.duration.unwrap_or(300))
            .await,
        "text" => session.inject_text(req.text.as_deref().unwrap_or("")).await,
        "press" => session.press_key(req.keycode.unwrap_or(0)).await,
        "home" => session.press_key(3).await,
        "back" => session.press_key(4).await,
        "recents" => session.press_key(187).await,
        "start_app" => session.start_app(req.app.as_deref().unwrap_or("")).await,
        "rotate" => session.rotate_device().await,
        "clipboard" => session.set_clipboard(req.text.as_deref().unwrap_or(""), false).await,
        _ => return err_response(StatusCode::BAD_REQUEST, "unknown command"),
    };
    match result {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => err_response(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

// ---------- 模板 ----------

fn templates_dir(st: &AppState) -> std::path::PathBuf {
    st.cfg.data_dir.join("templates")
}

async fn api_list_templates(State(st): State<AppState>) -> Response {
    let dir = templates_dir(&st);
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            // 模板目录专用：列出所有非隐藏文件（模板名可能带 .png/.jpg，也可能是 随机名字#x1_y1_x2_y2 这种带小数点无后缀名）
            let fname = e.file_name().to_string_lossy().to_string();
            if e.path().is_file() && !fname.starts_with('.') {
                let name = fname;
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                out.push(serde_json::json!({"name": name, "size": size}));
            }
        }
    }
    Json(out).into_response()
}

#[derive(Deserialize)]
struct UploadTemplateReq {
    name: String,
    data_b64: String,
}

async fn api_upload_template(State(st): State<AppState>, Json(req): Json<UploadTemplateReq>) -> Response {
    let bytes = match base64::engine::general_purpose::STANDARD.decode(&req.data_b64) {
        Ok(b) => b,
        Err(e) => return err_response(StatusCode::BAD_REQUEST, &format!("base64 解码失败: {}", e)),
    };
    // 校验是 PNG
    if image::load_from_memory(&bytes).is_err() {
        return err_response(StatusCode::BAD_REQUEST, "不是有效的图片");
    }
    let dir = templates_dir(&st);
    std::fs::create_dir_all(&dir).ok();
    let name = sanitize_filename(&req.name);
    let path = dir.join(&name);
    if let Err(e) = std::fs::write(&path, bytes) {
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    Json(serde_json::json!({"ok": true, "name": name})).into_response()
}

async fn api_delete_template(State(st): State<AppState>, Path(name): Path<String>) -> Response {
    let path = templates_dir(&st).join(sanitize_filename(&name));
    match std::fs::remove_file(&path) {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct RenameTemplateReq {
    name: String,
}

/// 重命名模板：把旧文件字节写入新文件名，再删除旧文件
async fn api_rename_template(State(st): State<AppState>, Path(old_name): Path<String>, Json(req): Json<RenameTemplateReq>) -> Response {
    let dir = templates_dir(&st);
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
    if let Err(e) = std::fs::write(&new_path, &bytes) {
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
async fn api_get_template_image(State(st): State<AppState>, Path(name): Path<String>) -> Response {
    let path = templates_dir(&st).join(sanitize_filename(&name));
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return err_response(StatusCode::NOT_FOUND, "模板不存在"),
    };
    let mime = match path.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
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
}

async fn api_test_template(State(st): State<AppState>, Path(name): Path<String>, Json(req): Json<TestTemplateReq>) -> Response {
    let tpl_path = templates_dir(&st).join(sanitize_filename(&name));
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
    match st.db.list_scripts() {
        Ok(s) => Json(s).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct SaveScriptReq {
    id: Option<String>,
    name: String,
    content: String,
}

async fn api_save_script(State(st): State<AppState>, Json(req): Json<SaveScriptReq>) -> Response {
    let id = req.id.unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let script = Script {
        id,
        name: req.name,
        content: req.content,
        updated_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    };
    match st.db.upsert_script(&script) {
        Ok(_) => Json(serde_json::json!({"ok": true, "id": script.id})).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn api_delete_script(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match st.db.delete_script(&id) {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct RunScriptReq {
    device_id: String,
    /// 从第几个 step 开始运行（0=从头；前端选中某个 "- " 逻辑行时传入）
    #[serde(default)]
    start_index: Option<usize>,
}

async fn api_run_script(State(st): State<AppState>, Path(id): Path<String>, Json(req): Json<RunScriptReq>) -> Response {
    let Some(script) = (match st.db.get_script(&id) {
        Ok(s) => s,
        Err(e) => return err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }) else {
        return err_response(StatusCode::NOT_FOUND, "脚本不存在");
    };
    // 同一脚本同时只允许一个运行实例（run_stops 条目存在 = 正在运行）
    {
        let stops = st.run_stops.lock().unwrap();
        if stops.contains_key(&id) {
            return err_response(StatusCode::CONFLICT, "脚本正在运行中");
        }
    }
    // 连接设备（若离线）
    if let Err(e) = st.devices.connect_device(&req.device_id).await {
        return err_response(StatusCode::BAD_GATEWAY, &format!("设备连接失败: {}", e));
    }
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    st.run_stops.lock().unwrap().insert(id.clone(), stop.clone());
    // 设备运行计数 +1（空闲断开守卫；spawn 结束时 run_end 归零）
    st.devices.run_begin(&req.device_id);
    let runner = st.runner.clone();
    let devices = st.devices.clone();
    let db = st.db.clone();
    let run_stops = st.run_stops.clone();
    let device_id = req.device_id.clone();
    let script_id = id.clone();
    let start_index = req.start_index.unwrap_or(0);
    let content = script.content.clone();
    // 实时日志：脚本每产生一条日志就立刻写入 DB，前端轮询即可实时显示
    let db_stream = st.db.clone();
    let log_cb: Option<Arc<dyn Fn(String, String) + Send + Sync>> = {
        let device_id = device_id.clone();
        let script_id = script_id.clone();
        Some(Arc::new(move |level, msg| {
            let _ = db_stream.add_log(&device_id, &script_id, &level, &msg);
        }))
    };
    tokio::spawn(async move {
        let logs = runner.run(&device_id, &script_id, &content, stop.clone(), log_cb, start_index).await;
        devices.run_end(&device_id);
        // 空闲低功耗：N 秒后无脚本运行且无 viewer → 断开 scrcpy 会话（adb 保留）
        devices.schedule_idle_disconnect(&device_id);
        match logs {
            Ok(_entries) => {
                let _ = db.add_log(&device_id, &script_id, "success", "脚本执行完成");
            }
            Err(e) => {
                let _ = db.add_log(&device_id, &script_id, "error", &format!("脚本执行失败: {}", e));
            }
        }
        // 运行结束：移除停止标志（条目存在与否同时作为"脚本是否在运行"的状态依据）
        let mut stops = run_stops.lock().unwrap();
        if let Some(cur) = stops.get(&script_id) {
            if Arc::ptr_eq(cur, &stop) {
                stops.remove(&script_id);
            }
        }
    });
    Json(serde_json::json!({"ok": true})).into_response()
}

async fn api_stop_script(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    if let Some(stop) = st.run_stops.lock().unwrap().get(&id) {
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    Json(serde_json::json!({"ok": true})).into_response()
}

/// 查询脚本是否正在运行（run_stops 条目存在 = 运行中，运行结束由 spawn 任务移除）
async fn api_script_status(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let running = st.run_stops.lock().unwrap().contains_key(&id);
    Json(serde_json::json!({"running": running})).into_response()
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
    let id = req.id.unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let existing = st.db.list_tasks().ok().and_then(|ts| ts.into_iter().find(|t| t.id == id));
    let task = Task {
        id,
        name: req.name,
        cron: req.cron,
        script_id: req.script_id,
        device_id: req.device_id,
        enabled: req.enabled.unwrap_or(existing.as_ref().map(|t| t.enabled).unwrap_or(true)),
        last_result: existing.as_ref().and_then(|t| t.last_result.clone()),
        last_run_at: existing.as_ref().and_then(|t| t.last_run_at.clone()),
        created_at: existing.as_ref().map(|t| t.created_at.clone()).unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()),
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

async fn api_run_task_now(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(task) = (match st.db.list_tasks() {
        Ok(t) => t,
        Err(e) => return err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    })
    .into_iter()
    .find(|t| t.id == id) else {
        return err_response(StatusCode::NOT_FOUND, "任务不存在");
    };
    st.scheduler.run_now(&task).await;
    Json(serde_json::json!({"ok": true})).into_response()
}

// ---------- 日志 ----------

#[derive(Deserialize)]
struct LogQuery {
    device_id: Option<String>,
    level: Option<String>,
    limit: Option<i64>,
}

async fn api_list_logs(State(st): State<AppState>, Query(q): Query<LogQuery>) -> Response {
    match st.db.list_logs(q.device_id.as_deref(), q.level.as_deref(), q.limit.unwrap_or(200)) {
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

// ---------- 工具 ----------

fn err_response(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({"error": msg}))).into_response()
}

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' || c == ' ' || c == '#' { c } else { '_' })
        .collect();
    if cleaned.is_empty() { "unnamed.png".into() } else { cleaned }
}

#[allow(dead_code)]
fn _unused(_: LogEntry, _: Duration) {}
