//! Device CRUD, connection control, screenshots, and device app discovery.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use super::common::{err_response, require_pkg, validate_text_field};
use super::{ApiError, AppState};
use crate::device::DeviceManager;
use crate::store::{Device, ScreenMode};

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

/// 设备请求的纯输入校验：不改变设备管理层的默认值，只拒绝会被静默
/// 转换为错误配置或可能污染日志/路径的输入。
pub(super) fn validate_device_req(req: &CreateDeviceReq) -> Result<(), ApiError> {
    validate_text_field(&req.name, "设备名称", 255)?;
    validate_text_field(&req.kind, "设备类型", 64)?;
    if let Some(addr) = req.addr.as_deref().filter(|v| !v.is_empty()) {
        validate_text_field(addr, "设备地址", 255)?;
    }
    if let Some(mode) = req.screen_mode.as_deref() {
        if !matches!(mode, "mirror" | "virtual") {
            return Err(ApiError::bad_request(
                "screen_mode 只允许 mirror 或 virtual",
            ));
        }
    }
    if let Some(res) = req.vd_res.as_deref().filter(|v| !v.trim().is_empty()) {
        validate_text_field(res, "虚拟屏分辨率", 32)?;
        let Some((width, height)) = res.trim().split_once('x') else {
            return Err(ApiError::bad_request("vd_res 必须是 WIDTHxHEIGHT"));
        };
        let valid_dimension = |v: &str| {
            v.parse::<u32>()
                .ok()
                .is_some_and(|n| (16..=16_384).contains(&n))
        };
        if !valid_dimension(width) || !valid_dimension(height) {
            return Err(ApiError::bad_request(
                "vd_res 的宽高必须在 16..16384 范围内",
            ));
        }
    }
    if req.vd_dpi.is_some_and(|dpi| dpi > 1_000) {
        return Err(ApiError::bad_request("vd_dpi 必须在 0..1000 范围内"));
    }
    if req.fps.is_some_and(|fps| fps > 120) {
        return Err(ApiError::bad_request("fps 必须在 0..120 范围内"));
    }
    if let Some(pkg) = req.pkg.as_deref().filter(|v| !v.trim().is_empty()) {
        require_pkg(Some(pkg))?;
    }
    Ok(())
}

/// 判断新旧配置里「投屏会话相关」字段是否变化。
///
/// scrcpy 会话组装实际消费的参数：kind / addr / screen_mode / vd_res / vd_dpi / fps
/// （见 `device::scrcpy`）；name / pkg 不参与建会话，只改它们无需断线重连。
/// 比较按生效值归一（与 scrcpy 侧 `unwrap_or` 默认一致），None 与等值默认之间的
/// 写法差异不触发重连；fps 未设置时以全局配置为生效值。
pub(super) fn session_affecting_change(prev: &Device, next: &Device, global_fps: u32) -> bool {
    let norm_res = |s: Option<&String>| -> String {
        s.map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .unwrap_or("1920x1080")
            .to_ascii_lowercase()
    };
    prev.kind != next.kind
        || prev.addr.trim() != next.addr.trim()
        || prev.screen_mode != next.screen_mode
        || norm_res(prev.vd_res.as_ref()) != norm_res(next.vd_res.as_ref())
        || prev.vd_dpi.unwrap_or(0) != next.vd_dpi.unwrap_or(0)
        || prev.fps.unwrap_or(global_fps) != next.fps.unwrap_or(global_fps)
}

pub(super) async fn api_list_devices(State(st): State<AppState>) -> Response {
    match device_views(&st).await {
        Ok(devices) => Json(devices).into_response(),
        Err(err) => err.into_response(),
    }
}

/// 渲染设备列表视图（带运行时状态/分辨率）。数据库查询走异步 worker RPC，
/// 数据库失败必须向调用方返回 500，而不是伪装成空列表。
async fn device_views(st: &AppState) -> Result<Vec<DeviceView>, ApiError> {
    let devices_snapshot = st
        .db
        .list_devices_async()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    render_device_views(&devices_snapshot, &st.devices)
}

fn render_device_views(
    devices_snapshot: &[Device],
    devices: &Arc<DeviceManager>,
) -> Result<Vec<DeviceView>, ApiError> {
    let mut out = Vec::new();
    for d in devices_snapshot {
        let (_, status, error) = devices
            .snapshot(&d.id)
            .map(|(_, s, e)| ((), s, e))
            .unwrap_or(((), crate::device::DeviceStatus::Offline, None));
        let (width, height) = devices
            .frame_cache(&d.id)
            .map(|fc| fc.dims())
            .unwrap_or((0, 0));
        out.push(DeviceView {
            id: d.id.clone(),
            name: d.name.clone(),
            kind: d.kind.clone(),
            addr: d.addr.clone(),
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
    Ok(out)
}

/// 扫描 `adb devices -l`，自动注册新发现的设备（USB / 无线 adb / 模拟器），
/// 已注册的跳过；返回完整设备列表（前端"刷新"时调用）
pub(super) async fn api_scan_devices(State(st): State<AppState>) -> Response {
    // 解析/去重/入库逻辑在 DeviceManager::scan_and_sync（与启动自举共用）
    let added = match st.devices.scan_and_sync().await {
        Ok(n) => n,
        Err(e) => {
            return err_response(StatusCode::BAD_GATEWAY, &format!("adb devices 失败: {}", e))
        }
    };
    let devices = match device_views(&st).await {
        Ok(devices) => devices,
        Err(err) => return err.into_response(),
    };
    Json(serde_json::json!({"ok": true, "added": added, "devices": devices})).into_response()
}

#[derive(Deserialize)]
pub(super) struct CreateDeviceReq {
    pub(super) name: String,
    pub(super) kind: String,
    pub(super) addr: Option<String>,
    pub(super) screen_mode: Option<String>,
    pub(super) vd_res: Option<String>,
    pub(super) vd_dpi: Option<u32>,
    pub(super) pkg: Option<String>,
    pub(super) fps: Option<u32>,
}

pub(super) async fn api_create_device(
    State(st): State<AppState>,
    Json(req): Json<CreateDeviceReq>,
) -> Response {
    if let Err(err) = validate_device_req(&req) {
        return err.into_response();
    }
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
pub(super) async fn api_update_device(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateDeviceReq>,
) -> Response {
    if let Err(err) = validate_device_req(&req) {
        return err.into_response();
    }
    let existing = match st.db.get_device_async(&id).await {
        Ok(existing) => existing,
        Err(err) => return ApiError::internal(err.to_string()).into_response(),
    };
    let Some(existing) = existing else {
        return ApiError::not_found("设备不存在").into_response();
    };
    let device = Device {
        id: id.clone(),
        name: req.name,
        kind: req.kind,
        addr: req.addr.unwrap_or_else(|| existing.addr.clone()),
        screen_mode: if req.screen_mode.as_deref() == Some("virtual") {
            ScreenMode::Virtual
        } else {
            ScreenMode::Mirror
        },
        vd_res: req.vd_res,
        vd_dpi: req.vd_dpi,
        pkg: req.pkg,
        fps: req.fps,
        created_at: existing.created_at.clone(),
    };
    // 投屏相关参数（接入类型/地址/屏幕模式/虚拟屏参数/帧率）变更才需要重建会话：
    // 踢活跃 viewer + 拆会话，浏览器 onclose 自动重连恢复画面。仅改名称/应用包名
    // 等非投屏字段时保持现有连接不中断。脚本运行中仍受运行守卫保护不拆会话
    // （旧参数跑完当前脚本，新配置下次连接生效）。
    if session_affecting_change(&existing, &device, st.cfg.fps) {
        if st.devices.has_running_scripts(&id) {
            info!(device = %id, "casting config changed while script running, session kept (applied on next connect)");
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
                info!(device = %id, "casting config changed, kicked viewer");
            }
            st.devices.disconnect_device(&id, false).await;
        }
    } else {
        info!(device = %id, "config changed (non-casting fields only), session kept");
    }
    match st.devices.upsert_device(&device).await {
        Ok(_) => Json(serde_json::json!({"ok": true, "id": device.id})).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

pub(super) async fn api_delete_device(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match st.devices.delete_device(&id).await {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ---------- 设备应用列表（供前端下拉选择游戏包名） ----------

/// 列出设备已安装的第三方应用：[{ label, pkg }]
/// 设备端 shell 无法解析应用显示名（label 在 APK 资源里），
/// 用包名最后两段生成友好名，完整包名始终一并展示，可搜索选择。
pub(super) async fn api_device_apps(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let device = match st.db.get_device_async(&id).await {
        Ok(Some(device)) => device,
        Ok(None) => return ApiError::not_found("设备不存在").into_response(),
        Err(err) => return ApiError::internal(err.to_string()).into_response(),
    };
    let serial = if device.addr.is_empty() {
        "usb".to_string()
    } else {
        device.addr.clone()
    };
    list_device_apps(&st, &serial).await
}

/// 按地址查询（添加设备弹窗里还没建记录时用）
pub(super) async fn api_apps_by_addr(
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

pub(super) async fn api_connect_device(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match st.devices.connect_device(&id).await {
        Ok(_) => {
            st.metrics.scrcpy_connect(true);
            // 应用已启动（建会话探测存活 / 会话内启动过）：前端据此不弹「未启动应用」提示
            let app_started = st
                .devices
                .session(&id)
                .map(|s| s.app_started())
                .unwrap_or(false);
            Json(serde_json::json!({"ok": true, "app_started": app_started})).into_response()
        }
        Err(e) => {
            st.metrics.scrcpy_connect(false);
            err_response(StatusCode::BAD_GATEWAY, &format!("连接失败: {}", e))
        }
    }
}

/// 强制断开（管理动作，绕过运行守卫）：拆 scrcpy 会话。注意前端"断开连接"
/// 按钮已不再调用此接口（只断本地 WebRTC，会话交给空闲低功耗管理）
pub(super) async fn api_disconnect_device(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    st.devices.disconnect_device(&id, true).await;
    Json(serde_json::json!({"ok": true})).into_response()
}

pub(super) async fn api_screenshot(State(st): State<AppState>, Path(id): Path<String>) -> Response {
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
pub(super) struct ControlReq {
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
pub(super) enum Ctl<'a> {
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
pub(super) fn parse_ctl(req: &ControlReq) -> Result<Ctl<'_>, ApiError> {
    match req.cmd.as_str() {
        "tap" => match (req.x, req.y) {
            (Some(x), Some(y)) if valid_coord(x) && valid_coord(y) => Ok(Ctl::Tap(x, y)),
            _ => Err(ApiError::bad_request("缺少或非法的 tap 坐标 x/y")),
        },
        "swipe" => {
            let (Some(x1), Some(y1), Some(x2), Some(y2)) = (req.x1, req.y1, req.x2, req.y2) else {
                return Err(ApiError::bad_request("缺少或非法的 swipe 坐标 x1/y1/x2/y2"));
            };
            if ![x1, y1, x2, y2].iter().all(|v| valid_coord(*v)) {
                return Err(ApiError::bad_request("swipe 坐标超出合法范围"));
            }
            if req.duration.is_some_and(|d| !(1..=600_000).contains(&d)) {
                return Err(ApiError::bad_request("duration 必须在 1..600000 ms 内"));
            }
            Ok(Ctl::Swipe(x1, y1, x2, y2, req.duration.unwrap_or(300)))
        }
        "text" => {
            let text = req
                .text
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .ok_or_else(|| ApiError::bad_request("text 不能为空"))?;
            // scrcpy 控制协议单条文本上限 300 字节，超长下游静默截断——显式拒绝
            if text.len() > 300 {
                return Err(ApiError::bad_request("text 超过 300 字节（协议上限）"));
            }
            Ok(Ctl::Text(text))
        }
        "press" => {
            let kc = req
                .keycode
                .filter(|k| valid_keycode(*k))
                .ok_or_else(|| ApiError::bad_request("keycode 缺失或不在 1..1000"))?;
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
                .ok_or_else(|| ApiError::bad_request("app 包名不能为空"))?;
            if app.len() > 255 {
                return Err(ApiError::bad_request("app 包名超过 255 字节"));
            }
            // "+" 强启 / "?" 按名搜索允许透传；裸包名必须是安全包名字符集
            if let Some(pkg) = app.strip_prefix('+') {
                if pkg.is_empty() || !crate::device::adb::is_safe_pkg(pkg) {
                    return Err(ApiError::bad_request("+ 前缀后须为合法包名"));
                }
            } else if let Some(name) = app.strip_prefix('?') {
                if name.trim().len() < 2 {
                    return Err(ApiError::bad_request("? 搜索名过短"));
                }
            } else if !crate::device::adb::is_safe_pkg(app) {
                return Err(ApiError::bad_request("包名非法（只允许字母数字 . _）"));
            }
            Ok(Ctl::StartApp(app))
        }
        "rotate" => Ok(Ctl::Rotate),
        "clipboard" => {
            let text = req
                .text
                .as_deref()
                .filter(|t| !t.is_empty())
                .ok_or_else(|| ApiError::bad_request("clipboard 文本不能为空"))?;
            // scrcpy 剪贴板协议上限 128KiB，超长下游静默截断——显式拒绝
            if text.len() > 131_072 {
                return Err(ApiError::bad_request(
                    "clipboard 文本超过 131072 字节（协议上限）",
                ));
            }
            Ok(Ctl::Clipboard(text))
        }
        _ => Err(ApiError::bad_request("unknown command")),
    }
}

pub(super) async fn api_control(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ControlReq>,
) -> Response {
    // 先纯校验（与设备无关，离线设备也能拿到明确 400），再取会话执行
    let ctl = match parse_ctl(&req) {
        Ok(c) => c,
        Err(err) => return err.into_response(),
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
        Err(e) => ApiError::bad_gateway(e.to_string()).into_response(),
    }
}
