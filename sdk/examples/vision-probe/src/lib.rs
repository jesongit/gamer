//! 设备/视觉能力示例 guest：验证权限声明真实打开受控 Host API。
//!
//! 与 hello 示例同一条 `gamer:host/extension@1.0.0` 契约，差别只在 manifest：
//! 本插件声明 `device.read` / `vision.match` / `vision.color` / `input.tap`
//! 四项权限，因此 `device.resolve` → `vision.capture` → `vision.sample-color`
//! 能穿过宿主 capability 边界真实执行（hello 插件同样的调用只会拿到
//! `kind=denied`）。
//!
//! 安全设计：
//! - `probe` 全程只读（解析设备 + 抓一帧 + 采样一个像素），不注入任何输入；
//! - `tap` 默认 **dry-run**：只回显将要执行的点击，除非表单勾选
//!   `confirm_tap`（示例策略：示例代码绝不替你点真机）。

wit_bindgen::generate!({
    // 契约快照：gamer:host@1.0.0（与宿主 HOST_API_VERSION 对应）。
    path: "wit/gamer",
    world: "extension-host",
});

use exports::gamer::host::extension::Guest;
use gamer::host::types::{HostError, HostErrorKind};
use gamer::host::{device, input, log, vision};

struct VisionProbe;

fn write_log(level: &str, message: &str) {
    let _ = log::write(level, message, None, None);
}

fn kind_str(kind: &HostErrorKind) -> &'static str {
    match kind {
        HostErrorKind::Denied => "denied",
        HostErrorKind::Unavailable => "unavailable",
        HostErrorKind::InvalidRequest => "invalid-request",
        HostErrorKind::NotFound => "not-found",
        HostErrorKind::Cancelled => "cancelled",
        HostErrorKind::Failed => "failed",
    }
}

/// 把 capability 失败整理成结构化 JSON（返回给面板/调用方看 kind 定位问题：
/// denied=权限没声明、not-found=设备 id 不存在、unavailable=能力未注册等）。
fn stage_error(action: &str, stage: &str, error: &HostError) -> String {
    serde_json::json!({
        "ok": false,
        "action": action,
        "stage": stage,
        "kind": kind_str(&error.kind),
        "message": error.message,
    })
    .to_string()
}

fn require_device_id(action: &str, values: &serde_json::Value) -> Result<String, String> {
    let device_id = values
        .get("device_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match device_id {
        Some(device_id) => Ok(device_id.to_string()),
        None => Err(serde_json::json!({
            "ok": false,
            "action": action,
            "error": "missing_device_id",
            "hint": "在面板/请求值里填 device_id（设备管理列表中的稳定 id）",
        })
        .to_string()),
    }
}

fn point_at(values: &serde_json::Value) -> (u32, u32) {
    (
        values.get("x").and_then(|value| value.as_u64()).unwrap_or(0) as u32,
        values.get("y").and_then(|value| value.as_u64()).unwrap_or(0) as u32,
    )
}

/// action=probe：resolve 设备 → capture 一帧 → sample-color 一个像素。
/// 权限链：device.read（resolve）→ vision.match（capture）→ vision.color（采样）。
fn probe(values: &serde_json::Value) -> Result<String, String> {
    let action = "probe";
    let device_id = require_device_id(action, values)?;
    let device = match device::resolve(&device_id) {
        Ok(handle) => handle,
        Err(error) => return Ok(stage_error(action, "device.resolve", &error)),
    };
    let frame = match vision::capture(device) {
        Ok(handle) => handle,
        Err(error) => return Ok(stage_error(action, "vision.capture", &error)),
    };
    let point = point_at(values);
    match vision::sample_color(frame, vision::Point { x: point.0, y: point.1 }) {
        Ok((red, green, blue)) => {
            write_log(
                "info",
                format!("vision-probe: {device_id} @ ({}, {}) rgb=({red},{green},{blue})", point.0, point.1).as_str(),
            );
            Ok(serde_json::json!({
                "ok": true,
                "action": action,
                "device": device_id,
                "device_handle": device,
                "frame_handle": frame,
                "point": [point.0, point.1],
                "rgb": [red, green, blue],
            })
            .to_string())
        }
        Err(error) => Ok(stage_error(action, "vision.sample_color", &error)),
    }
}

/// action=tap：input.tap 的 dry 调用示例。
/// `confirm_tap != true` 时不触达设备，只回显将要执行的动作。
fn tap(values: &serde_json::Value) -> Result<String, String> {
    let action = "tap";
    let device_id = require_device_id(action, values)?;
    let point = point_at(values);
    let confirmed = values
        .get("confirm_tap")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if !confirmed {
        return Ok(serde_json::json!({
            "ok": true,
            "action": action,
            "dry_run": true,
            "would": { "op": "input.tap", "device": device_id, "point": [point.0, point.1] },
            "hint": "勾选 confirm_tap 后再次点击按钮才会真实注入",
        })
        .to_string());
    }
    let device = match device::resolve(&device_id) {
        Ok(handle) => handle,
        Err(error) => return Ok(stage_error(action, "device.resolve", &error)),
    };
    match input::tap(device, input::Point { x: point.0, y: point.1 }) {
        Ok(()) => {
            write_log("warn", format!("vision-probe: 真实注入 tap {device_id} @ ({},{})", point.0, point.1).as_str());
            Ok(serde_json::json!({
                "ok": true,
                "action": action,
                "dry_run": false,
                "device": device_id,
                "point": [point.0, point.1],
            })
            .to_string())
        }
        Err(error) => Ok(stage_error(action, "input.tap", &error)),
    }
}

impl Guest for VisionProbe {
    fn run() {
        write_log(
            "info",
            "vision-probe: 插件实例已启动（device.read/vision.match/vision.color/input.tap 已授权）",
        );
    }

    fn call(action: String, values_json: String) -> Result<String, String> {
        let values = serde_json::from_str::<serde_json::Value>(&values_json)
            .unwrap_or(serde_json::Value::Null);
        match action.as_str() {
            "probe" => probe(&values),
            "tap" => tap(&values),
            other => Err(serde_json::json!({
                "ok": false,
                "error": "unknown_action",
                "action": other,
            })
            .to_string()),
        }
    }
}

export!(VisionProbe);
