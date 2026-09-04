//! Keymap extension adapter: the normalized input envelope, E2E trace, the
//! capability action executors, and the keymap WASM runtime plumbing.
//!
//! Mapping itself lives entirely in the keymap WASM guest — the native
//! mapping engine has been removed.  This module owns only transport-neutral
//! wire types shared with the guest ABI, permission-gated action execution,
//! and profile loading; with no keymap extension running,
//! `ExtensionService::dispatch_keymap_input` passes input straight through to
//! the legacy scrcpy path.

use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::app_packages::{
    parse_android_package_name, parse_app_package_id, AppPackageStore, InstalledVersion,
    ResourcePath,
};
use crate::capabilities::{
    CapabilityRegistry, DeviceHandle, KeyAction, KeyCode, KeyInput, SwipeGesture, TextInput,
    TouchHandle, TouchPoint,
};
use crate::keymaps::{parse_keymap_content, Keymap};

use super::error::{ExtensionError, ExtensionResult};
use super::host_api::HostApi;
use super::model::{ExtensionId, ExtensionVersion};

pub const KEYMAP_EXTENSION_ID: &str = "gamer.keymap";
pub const KEYMAP_PANEL_ID: &str = "keymaps";
pub const KEYMAP_WASM_ABI_VERSION: &str = "gamer-keymap@1";
pub const INPUT_PROTOCOL_VERSION: &str = "gamer-input@1";

/// Canonical manifest for the first shipped keymap extension.  The package
/// still has to be installed through the normal `.gplugin` service; keeping
/// the manifest here makes the extension's requested surface reviewable and
/// gives package builders one source of truth for the panel contribution.
pub const KEYMAP_EXTENSION_MANIFEST_TOML: &str = r#"
manifest_version = 1
id = "gamer.keymap"
version = "1.0.0"
name = "Keymap"
description = "Application-specific keyboard, mouse, and gamepad mappings"
entry = "plugin.wasm"
permissions = ["input.tap", "input.swipe", "input.key", "touch"]

[host_api]
input = "^1.0"
touch = "^1.0"

[[ui.contributions]]
# runtime = "core"：面板由宿主 Vue 组件渲染，component 键由前端
# core-component-registry 解释（console.keymaps = 映射面板）。
panel_id = "keymaps"
title = "映射"
icon = "⌨"
order = 30
location = "console.right"
runtime = "core"
requires_device = true
preferred_width = 360
component = "console.keymaps"
"#;

// ===================== Phase 6 Keymap E2E trace（默认关闭） =====================
//
// 计划 8.3：`input_event` 信封可选携带 `trace_id` 与 `client_send_ts`（epoch
// 微秒），服务端沿链路记录各阶段时间戳并产出 [`KeymapTraceRecord`]。默认零
// 开销零行为变化：仅当基准/测试经 [`install_keymap_trace_sink`] 安装收集器，
// 或设置 `GAMER_KEYMAP_E2E_TRACE=1`（每条记录以 tracing::info 输出）时激活。
//
// 阶段口径（均为相对 server_receive 的单调微秒）：
// - `wasm_begin`：WASM 链路 = guest handle 调用前（含 service 实例查找与
//   命令队列排队）；native（直通）链路 = pass-through 判定后开始。
// - `wasm_end`：guest handle 返回 / native 直通决策完成。
// - `device_action`：guest 结果授权+映射完成、执行第一个动作前。
// - `scrcpy_write`：最后一个动作的 scrcpy 控制写完成。
//
// `server_receive_epoch_us` / `client_send_epoch_us` 是 epoch 微秒绝对时间，
// 仅在同进程（时钟同源，基准的进程内 DataChannel 客户端）时可直接相减得到
// Browser→Server；真实浏览器与服务器时钟不可靠同步，必须按计划 8.3 把
// Browser RTT 与服务端内部阶段分开统计。

/// trace 记录的链路形态：WASM 插件 or 原生直通。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeymapTracePath {
    Native,
    Wasm,
}

/// 单个输入事件的全链路时间戳记录（Phase 6 基准的数据单元）。
#[derive(Clone, Debug, serde::Serialize)]
pub struct KeymapTraceRecord {
    pub trace_id: String,
    pub path: KeymapTracePath,
    pub client_send_epoch_us: Option<u64>,
    pub server_receive_epoch_us: u64,
    pub wasm_begin_us: u64,
    pub wasm_end_us: u64,
    pub device_action_us: u64,
    pub scrcpy_write_us: u64,
    /// scrcpy 控制写完成时刻的进程内单调时钟（只供同进程基准做写顺序
    /// 断言，不参与 JSON 输出；相对阶段值不再承担顺序语义）
    #[serde(skip_serializing)]
    pub scrcpy_write_instant: Instant,
}

/// 链路起点（WebRTC 信令解析处）构造、随调用传递的 trace 上下文。
#[derive(Clone, Debug)]
pub struct KeymapTraceContext {
    pub trace_id: String,
    pub client_send_epoch_us: Option<u64>,
    server_receive_epoch_us: u64,
    server_receive_instant: Instant,
}

impl KeymapTraceContext {
    pub fn new(trace_id: impl Into<String>, client_send_epoch_us: Option<u64>) -> Self {
        Self {
            trace_id: trace_id.into(),
            client_send_epoch_us,
            server_receive_epoch_us: now_epoch_us(),
            server_receive_instant: Instant::now(),
        }
    }

    fn since_receive(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.server_receive_instant)
            .as_micros() as u64
    }

    /// 由链路各埋点处在阶段完成时刻调用，产出记录（阶段值为相对
    /// server_receive 的单调微秒）。
    pub fn record(
        &self,
        path: KeymapTracePath,
        wasm_begin: Instant,
        wasm_end: Instant,
        device_action: Instant,
        scrcpy_write: Instant,
    ) -> KeymapTraceRecord {
        KeymapTraceRecord {
            trace_id: self.trace_id.clone(),
            path,
            client_send_epoch_us: self.client_send_epoch_us,
            server_receive_epoch_us: self.server_receive_epoch_us,
            wasm_begin_us: self.since_receive(wasm_begin),
            wasm_end_us: self.since_receive(wasm_end),
            device_action_us: self.since_receive(device_action),
            scrcpy_write_us: self.since_receive(scrcpy_write),
            scrcpy_write_instant: scrcpy_write,
        }
    }
}

/// 当前 epoch 微秒（与前端 `client_send_ts` 同单位；同进程内可直接比较）。
pub fn now_epoch_us() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or_default()
}

type KeymapTraceSink = tokio::sync::mpsc::UnboundedSender<KeymapTraceRecord>;

static TRACE_SINK: std::sync::RwLock<Option<KeymapTraceSink>> = std::sync::RwLock::new(None);
static ENV_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn env_trace_enabled() -> bool {
    *ENV_TRACE.get_or_init(|| std::env::var("GAMER_KEYMAP_E2E_TRACE").as_deref() == Ok("1"))
}

/// trace 是否激活（基准安装了收集器，或显式 env 门禁开启）。
pub fn keymap_trace_active() -> bool {
    TRACE_SINK.read().expect("keymap trace sink").is_some() || env_trace_enabled()
}

/// 基准/测试安装进程内收集器（未消费上限的 channel，绝不阻塞控制链路）。
pub fn install_keymap_trace_sink(sink: KeymapTraceSink) {
    *TRACE_SINK.write().expect("keymap trace sink") = Some(sink);
}

/// 基准/测试结束移除收集器，恢复默认零开销路径。
pub fn clear_keymap_trace_sink() {
    *TRACE_SINK.write().expect("keymap trace sink") = None;
}

pub(crate) fn emit_keymap_trace(record: KeymapTraceRecord) {
    if env_trace_enabled() {
        tracing::info!(
            target: "keymap_e2e_trace",
            trace = %serde_json::to_string(&record).unwrap_or_default(),
            "keymap e2e trace"
        );
    }
    if let Some(sink) = TRACE_SINK.read().expect("keymap trace sink").as_ref() {
        let _ = sink.send(record);
    }
}

/// Decode the transport-neutral input envelope before it reaches a runner.
/// WebRTC remains responsible for framing; this function owns only the
/// versioned JSON payload used by the keymap extension.
pub fn decode_input_event(bytes: &[u8]) -> ExtensionResult<InputEvent> {
    serde_json::from_slice(bytes)
        .map_err(|error| ExtensionError::Runtime(format!("input event 无效: {error}")))
}

/// Screen size used to turn persisted normalized coordinates into device
/// pixels.  Mouse coordinates are already pixels and are not passed through
/// this conversion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScreenSize {
    pub width: u32,
    pub height: u32,
}

impl ScreenSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    fn point(self, normalized: [f64; 2]) -> TouchPoint {
        TouchPoint::new(
            (normalized[0].clamp(0.0, 1.0) * self.width as f64).round() as u32,
            (normalized[1].clamp(0.0, 1.0) * self.height as f64).round() as u32,
            1.0,
        )
    }
}

/// Browser/gamepad input after the Core boundary has removed DOM-specific
/// objects.  All variants are small values suitable for a future WIT record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputEvent {
    KeyDown {
        code: String,
        #[serde(default)]
        repeat: bool,
        #[serde(default)]
        meta: u32,
    },
    KeyUp {
        code: String,
        #[serde(default)]
        meta: u32,
    },
    MouseDown {
        button: u8,
        x: u32,
        y: u32,
    },
    MouseUp {
        button: u8,
        x: u32,
        y: u32,
    },
    MouseMove {
        x: u32,
        y: u32,
        #[serde(default)]
        delta_x: i32,
        #[serde(default)]
        delta_y: i32,
    },
    Wheel {
        x: u32,
        y: u32,
        delta_x: i32,
        delta_y: i32,
    },
    GamepadButton {
        index: u8,
        pressed: bool,
        #[serde(default)]
        value: f32,
    },
    GamepadAxis {
        index: u8,
        value: f32,
    },
}

impl InputEvent {
    pub fn key_down(code: impl Into<String>) -> Self {
        Self::KeyDown {
            code: code.into(),
            repeat: false,
            meta: 0,
        }
    }

    pub fn key_up(code: impl Into<String>) -> Self {
        Self::KeyUp {
            code: code.into(),
            meta: 0,
        }
    }

    /// Return the closed selector vocabulary used by keymap YAML.  `None`
    /// means the event is intentionally passed to the normal input path.
    pub fn selector(&self) -> Option<String> {
        match self {
            Self::KeyDown { code, .. } | Self::KeyUp { code, .. } => Some(code.clone()),
            Self::MouseDown { button, .. } | Self::MouseUp { button, .. } => {
                mouse_selector(*button).map(str::to_string)
            }
            Self::MouseMove { .. } => Some("MouseMove".to_string()),
            Self::Wheel { .. } => None,
            Self::GamepadButton { index, .. } => Some(format!("GamepadButton{index}")),
            Self::GamepadAxis { index, .. } => Some(format!("GamepadAxis{index}")),
        }
    }

    fn is_press(&self) -> bool {
        matches!(
            self,
            Self::KeyDown { .. }
                | Self::MouseDown { .. }
                | Self::GamepadButton { pressed: true, .. }
        )
    }

    fn is_release(&self) -> bool {
        matches!(
            self,
            Self::KeyUp { .. } | Self::MouseUp { .. } | Self::GamepadButton { pressed: false, .. }
        )
    }

    fn repeat(&self) -> bool {
        matches!(self, Self::KeyDown { repeat: true, .. })
    }
}

fn mouse_selector(button: u8) -> Option<&'static str> {
    match button {
        0 => Some("MouseLeft"),
        1 => Some("MouseMiddle"),
        2 => Some("MouseRight"),
        3 => Some("MouseBack"),
        4 => Some("MouseForward"),
        _ => None,
    }
}

/// Actions returned by the mapping layer.  They are capability-level values,
/// not scrcpy wire packets.  A touch begin returns a handle through the
/// executor, and subsequent state stores that opaque handle only.
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceAction {
    Tap {
        point: TouchPoint,
    },
    Swipe {
        gesture: SwipeGesture,
    },
    Key {
        input: KeyInput,
    },
    Text {
        input: TextInput,
    },
    TouchBegin {
        point: TouchPoint,
    },
    TouchMove {
        touch: TouchHandle,
        point: TouchPoint,
    },
    TouchEnd {
        touch: TouchHandle,
    },
}

/// A small result that preserves the browser routing decision independently
/// from whether an action list happened to be empty (for example a repeated
/// tap is consumed but does not produce another tap).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputResult {
    pub consume: bool,
    pub actions: Vec<DeviceAction>,
}

impl InputResult {
    pub const fn pass() -> Self {
        Self {
            consume: false,
            actions: Vec::new(),
        }
    }

    pub fn consume(actions: Vec<DeviceAction>) -> Self {
        Self {
            consume: true,
            actions,
        }
    }
}

/// Host-side action executor.  Returning a handle is meaningful only for a
/// `TouchBegin`; all other actions return `None`.
#[async_trait]
pub trait DeviceActionExecutor: Send + Sync {
    async fn execute(
        &self,
        device: &DeviceHandle,
        action: &DeviceAction,
    ) -> ExtensionResult<Option<TouchHandle>>;
}

/// Adapter from stable capability traits to device actions.  It is the only
/// keymap-specific place that knows how to call the current Core services.
#[derive(Clone)]
pub struct CapabilityDeviceActionExecutor {
    capabilities: CapabilityRegistry,
}

impl CapabilityDeviceActionExecutor {
    pub fn new(capabilities: CapabilityRegistry) -> Self {
        Self { capabilities }
    }

    fn unavailable(name: &'static str) -> ExtensionError {
        ExtensionError::Runtime(format!("keymap capability unavailable: {name}"))
    }

    fn map_capability_error(error: impl std::fmt::Display) -> ExtensionError {
        ExtensionError::Runtime(format!("keymap capability failed: {error}"))
    }
}

#[async_trait]
impl DeviceActionExecutor for CapabilityDeviceActionExecutor {
    async fn execute(
        &self,
        device: &DeviceHandle,
        action: &DeviceAction,
    ) -> ExtensionResult<Option<TouchHandle>> {
        match action {
            DeviceAction::Tap { point } => {
                let service = self
                    .capabilities
                    .input()
                    .ok_or_else(|| Self::unavailable("input.tap"))?;
                service
                    .tap(device, *point)
                    .await
                    .map_err(Self::map_capability_error)?;
            }
            DeviceAction::Swipe { gesture } => {
                let service = self
                    .capabilities
                    .input()
                    .ok_or_else(|| Self::unavailable("input.swipe"))?;
                service
                    .swipe(device, *gesture)
                    .await
                    .map_err(Self::map_capability_error)?;
            }
            DeviceAction::Key { input } => {
                let service = self
                    .capabilities
                    .input()
                    .ok_or_else(|| Self::unavailable("input.key"))?;
                service
                    .key(device, *input)
                    .await
                    .map_err(Self::map_capability_error)?;
            }
            DeviceAction::Text { input } => {
                let service = self
                    .capabilities
                    .input()
                    .ok_or_else(|| Self::unavailable("input.text"))?;
                service
                    .text(device, input.clone())
                    .await
                    .map_err(Self::map_capability_error)?;
            }
            DeviceAction::TouchBegin { point } => {
                let service = self
                    .capabilities
                    .touch()
                    .ok_or_else(|| Self::unavailable("touch.begin"))?;
                let touch = service
                    .begin(device, *point)
                    .await
                    .map_err(Self::map_capability_error)?;
                return Ok(Some(touch));
            }
            DeviceAction::TouchMove { touch, point } => {
                let service = self
                    .capabilities
                    .touch()
                    .ok_or_else(|| Self::unavailable("touch.move"))?;
                service
                    .move_touch(touch, *point)
                    .await
                    .map_err(Self::map_capability_error)?;
            }
            DeviceAction::TouchEnd { touch } => {
                let service = self
                    .capabilities
                    .touch()
                    .ok_or_else(|| Self::unavailable("touch.end"))?;
                service
                    .end(touch)
                    .await
                    .map_err(Self::map_capability_error)?;
            }
        }
        Ok(None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizedPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeymapPanelContribution {
    pub plugin_id: &'static str,
    pub panel_id: &'static str,
    pub title: &'static str,
}

#[derive(Default)]
pub struct KeymapContributionRegistry {
    registered: bool,
}

impl KeymapContributionRegistry {
    pub fn register(&mut self) -> KeymapPanelContribution {
        self.registered = true;
        KeymapPanelContribution {
            plugin_id: KEYMAP_EXTENSION_ID,
            panel_id: KEYMAP_PANEL_ID,
            title: "映射",
        }
    }

    pub fn unregister(&mut self) {
        self.registered = false;
    }

    pub fn is_registered(&self) -> bool {
        self.registered
    }
}

/// Human-readable status used by diagnostics and runtime checks.
pub fn real_wasm_host_status() -> &'static str {
    "keymap WIT component entrypoint is executable; actions remain capability-gated"
}

/// 构建 keymap guest fixture 组件（`tests/keymap-guest`，wasm32 target 经
/// wit-component 编码）。WASM 组件测试与 Phase 6 真机 E2E 基准共用。
/// 仅测试构建可用（wit-component 是 dev-dependency）；内部显式清除
/// `CARGO_TARGET_DIR`，guest 产物固定落在 `tests/keymap-guest/target/`，
/// 避免外层基准目录设置导致读取落空。
#[cfg(all(test, feature = "wasm-runtime"))]
pub(crate) fn build_guest_fixture_component() -> Vec<u8> {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::OnceLock;

    static COMPONENT: OnceLock<Vec<u8>> = OnceLock::new();
    COMPONENT
        .get_or_init(|| {
            let server_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let guest_dir = server_dir.join("tests").join("keymap-guest");
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let status = Command::new(cargo)
                .current_dir(&guest_dir)
                .env_remove("CARGO_TARGET_DIR")
                .args([
                    "build",
                    "--quiet",
                    "--release",
                    "--target",
                    "wasm32-unknown-unknown",
                ])
                .status()
                .expect("无法构建 keymap guest fixture");
            assert!(status.success(), "keymap guest fixture 构建失败");
            let module = fs::read(
                guest_dir
                    .join("target")
                    .join("wasm32-unknown-unknown")
                    .join("release")
                    .join("gamer_keymap_fixture.wasm"),
            )
            .expect("keymap guest fixture wasm 不存在");
            wit_component::ComponentEncoder::default()
                .module(&module)
                .expect("keymap guest module 不是合法 WIT module")
                .validate(true)
                .encode()
                .expect("keymap guest module 无法 componentize")
        })
        .clone()
}

/// 把 guest 组件打包成可安装的 `gplugin` 归档（manifest + plugin.wasm + UI）。
/// 权限集合由调用方按 profile 实际动作声明。仅测试构建可用。
#[cfg(all(test, feature = "wasm-runtime"))]
pub(crate) fn package_guest_fixture_gplugin(component: &[u8], permissions: &[&str]) -> Vec<u8> {
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn manifest(permissions: &[&str]) -> String {
        let joined = permissions
            .iter()
            .map(|permission| format!("\"{permission}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            r#"manifest_version = 1
id = "gamer.keymap"
version = "1.0.0"
name = "Keymap"
description = "WIT keymap component fixture"
entry = "plugin.wasm"
permissions = [{joined}]

[host_api]
input = "^1.0"
touch = "^1.0"

[[ui.contributions]]
panel_id = "keymaps"
title = "映射"
icon = "⌨"
order = 30
location = "console.right"
runtime = "iframe"
requires_device = true
preferred_width = 360
entry = "ui/index.html"
"#
        )
    }

    let mut bytes = Vec::new();
    let mut archive = ZipWriter::new(Cursor::new(&mut bytes));
    let options = SimpleFileOptions::default();
    archive
        .start_file("manifest.toml", options)
        .expect("manifest entry");
    archive.write_all(manifest(permissions).as_bytes()).unwrap();
    archive
        .start_file("plugin.wasm", options)
        .expect("wasm entry");
    archive.write_all(component).expect("wasm bytes");
    archive
        .start_file("ui/index.html", options)
        .expect("ui entry");
    archive
        .write_all(include_bytes!("../../tests/keymap-guest/ui/index.html"))
        .expect("ui bytes");
    archive.finish().expect("finish gplugin");
    bytes
}

/// Start request for the keymap-specific Component world. It intentionally
/// lives beside the keymap adapter rather than extending the generic Phase 6
/// extension Host contract. `profile` carries the raw user-selected keymap
/// YAML for the current partition (None = guest built-in defaults, which is
/// full pass-through for keys the defaults do not map).
#[derive(Clone)]
pub(crate) struct KeymapWasmStartRequest {
    pub(crate) id: ExtensionId,
    pub(crate) version: ExtensionVersion,
    pub(crate) wasm: Vec<u8>,
    pub(crate) host: HostApi,
    pub(crate) app_context: Option<crate::core::AppContext>,
    pub(crate) profile: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct KeymapWasmInstanceHandle(uuid::Uuid);

impl KeymapWasmInstanceHandle {
    fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

#[async_trait]
pub(crate) trait KeymapWasmRuntime: Send + Sync {
    async fn start(
        &self,
        request: KeymapWasmStartRequest,
    ) -> ExtensionResult<KeymapWasmInstanceHandle>;

    async fn stop(&self, instance: KeymapWasmInstanceHandle) -> ExtensionResult<()>;

    async fn dispatch(
        &self,
        instance: KeymapWasmInstanceHandle,
        device: DeviceHandle,
        screen: ScreenSize,
        event: InputEvent,
        trace: Option<KeymapTraceContext>,
    ) -> ExtensionResult<InputResult>;

    fn is_available(&self) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NoKeymapWasmRuntime;

#[async_trait]
impl KeymapWasmRuntime for NoKeymapWasmRuntime {
    async fn start(
        &self,
        _request: KeymapWasmStartRequest,
    ) -> ExtensionResult<KeymapWasmInstanceHandle> {
        Err(ExtensionError::RuntimeUnavailable(
            "未启用 wasm-runtime feature",
        ))
    }

    async fn stop(&self, _instance: KeymapWasmInstanceHandle) -> ExtensionResult<()> {
        Err(ExtensionError::RuntimeUnavailable(
            "未启用 wasm-runtime feature",
        ))
    }

    async fn dispatch(
        &self,
        _instance: KeymapWasmInstanceHandle,
        _device: DeviceHandle,
        _screen: ScreenSize,
        _event: InputEvent,
        _trace: Option<KeymapTraceContext>,
    ) -> ExtensionResult<InputResult> {
        Err(ExtensionError::RuntimeUnavailable(
            "未启用 wasm-runtime feature",
        ))
    }

    fn is_available(&self) -> bool {
        false
    }
}

#[cfg(feature = "wasm-runtime")]
mod keymap_wasmtime {
    use std::collections::HashMap;
    use std::sync::{Arc, OnceLock};
    use std::time::Instant;

    use sha2::{Digest, Sha256};
    use tokio::sync::{mpsc, oneshot, Mutex};
    use wasmtime::component::{Component, Linker};
    use wasmtime::{Engine, Store};

    use crate::capabilities::{KeyAction, KeyCode, KeyInput, SwipeGesture, TextInput, TouchPoint};

    use super::super::error::{ExtensionError, ExtensionResult};
    use super::super::permissions::Permission;
    use super::super::wit::keymap::{exports::gamer::keymap::keymap as guest, KeymapHost};
    use super::{
        emit_keymap_trace, CapabilityDeviceActionExecutor, DeviceAction, DeviceActionExecutor,
        DeviceHandle, InputEvent, InputResult, KeymapTraceContext, KeymapTracePath,
        KeymapWasmInstanceHandle, KeymapWasmRuntime, KeymapWasmStartRequest, ScreenSize,
    };

    type GuestEvent = guest::InputEvent;
    type GuestResult = guest::InputResult;
    type GuestError = String;

    struct Invoke {
        device: DeviceHandle,
        screen: ScreenSize,
        event: InputEvent,
        trace: Option<KeymapTraceContext>,
        reply: oneshot::Sender<ExtensionResult<InputResult>>,
    }

    enum Command {
        Invoke(Invoke),
        Stop(oneshot::Sender<()>),
    }

    struct RunningKeymap {
        commands: mpsc::Sender<Command>,
        task: tokio::task::JoinHandle<()>,
    }

    /// The keymap guest has no WASI imports. Its only effect is the typed
    /// `handle` result, which this task authorizes and executes through the
    /// native capability adapter before the result is returned to Core.
    pub(crate) struct LazyKeymapWasmRuntime {
        engine: OnceLock<Engine>,
        components: Mutex<HashMap<[u8; 32], Arc<Component>>>,
        instances: Mutex<HashMap<KeymapWasmInstanceHandle, RunningKeymap>>,
    }

    impl LazyKeymapWasmRuntime {
        pub(crate) fn new() -> Self {
            Self {
                engine: OnceLock::new(),
                components: Mutex::new(HashMap::new()),
                instances: Mutex::new(HashMap::new()),
            }
        }

        pub(crate) fn is_initialized(&self) -> bool {
            self.engine.get().is_some()
        }

        fn engine(&self) -> &Engine {
            self.engine.get_or_init(|| {
                let config = wasmtime::Config::new();
                Engine::new(&config).expect("Wasmtime keymap engine config is valid")
            })
        }

        async fn component(&self, bytes: &[u8]) -> ExtensionResult<Arc<Component>> {
            let mut digest = [0u8; 32];
            digest.copy_from_slice(Sha256::digest(bytes).as_slice());
            let mut components = self.components.lock().await;
            if let Some(component) = components.get(&digest).cloned() {
                return Ok(component);
            }
            let component = Arc::new(Component::new(self.engine(), bytes).map_err(|error| {
                ExtensionError::Runtime(format!("keymap 组件编译失败: {error}"))
            })?);
            components.insert(digest, component.clone());
            Ok(component)
        }
    }

    #[async_trait::async_trait]
    impl KeymapWasmRuntime for LazyKeymapWasmRuntime {
        async fn start(
            &self,
            request: KeymapWasmStartRequest,
        ) -> ExtensionResult<KeymapWasmInstanceHandle> {
            let component = self.component(&request.wasm).await?;
            let engine = self.engine().clone();
            let (commands_tx, mut commands_rx) = mpsc::channel(32);
            let (ready_tx, ready_rx) = oneshot::channel();
            let executor = CapabilityDeviceActionExecutor::new(request.host.registry().clone());
            let host = request.host;
            let instance_id = KeymapWasmInstanceHandle::new();
            let app_context = request.app_context;
            let id = request.id;
            let version = request.version;
            let log_id = id.clone();
            let log_version = version.clone();
            let profile = request.profile;

            let task = tokio::spawn(async move {
                let linker: Linker<()> = Linker::new(&engine);
                let mut store = Store::new(&engine, ());
                let instance = match KeymapHost::instantiate(&mut store, &component, &linker) {
                    Ok(instance) => instance,
                    Err(error) => {
                        let _ = ready_tx.send(Err(ExtensionError::Runtime(format!(
                            "keymap 组件实例化失败: {error}"
                        ))));
                        return;
                    }
                };
                let keymap = instance.gamer_keymap_keymap();
                match keymap.call_start(&mut store, profile.as_deref()) {
                    Ok(Ok(())) => {
                        let _ = ready_tx.send(Ok(()));
                    }
                    Ok(Err(error)) => {
                        let _ = ready_tx.send(Err(ExtensionError::Runtime(format!(
                            "keymap start 失败: {error}"
                        ))));
                        return;
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(ExtensionError::Runtime(format!(
                            "keymap start trap: {error}"
                        ))));
                        return;
                    }
                }

                let mut touches: HashMap<u64, (DeviceHandle, crate::capabilities::TouchHandle)> =
                    HashMap::new();
                while let Some(command) = commands_rx.recv().await {
                    match command {
                        Command::Invoke(invoke) => {
                            if let Some(context) = &app_context {
                                if context.device_id.as_str() != invoke.device.id().as_str() {
                                    let _ = invoke.reply.send(Err(ExtensionError::Runtime(
                                        "keymap AppContext 与输入设备不匹配".to_string(),
                                    )));
                                    continue;
                                }
                            }
                            // Phase 6 E2E：wasm_begin 覆盖 guest 调用排队完成后的
                            // 实际执行起点（service 查找与命令队列排队计入
                            // Server Receive → WASM Begin 阶段）
                            let wasm_begin = Instant::now();
                            let guest_event = guest_event(&invoke.event);
                            let guest_result = keymap.call_handle(&mut store, &guest_event);
                            let wasm_end = Instant::now();
                            let result = invoke_guest_result(
                                &host,
                                &executor,
                                &mut touches,
                                &invoke,
                                guest_result,
                                wasm_begin,
                                wasm_end,
                            )
                            .await;
                            let _ = invoke.reply.send(result);
                        }
                        Command::Stop(reply) => {
                            for (_, (device, touch)) in touches.drain() {
                                let action = DeviceAction::TouchEnd { touch };
                                if let Err(error) = executor.execute(&device, &action).await {
                                    tracing::warn!(%error, "keymap touch cleanup failed");
                                }
                            }
                            let _ = reply.send(());
                            break;
                        }
                    }
                }
                tracing::debug!(extension = %id, version = %version, ?app_context, "keymap WASM instance stopped");
            });

            match ready_rx.await {
                Ok(Ok(())) => {
                    self.instances.lock().await.insert(
                        instance_id,
                        RunningKeymap {
                            commands: commands_tx,
                            task,
                        },
                    );
                    tracing::info!(extension = %log_id, version = %log_version, "keymap WASM component started");
                    Ok(instance_id)
                }
                Ok(Err(error)) => {
                    let _ = task.await;
                    Err(error)
                }
                Err(_) => {
                    let _ = task.await;
                    Err(ExtensionError::Runtime(
                        "keymap 启动任务意外退出".to_string(),
                    ))
                }
            }
        }

        async fn stop(&self, instance: KeymapWasmInstanceHandle) -> ExtensionResult<()> {
            let running = self
                .instances
                .lock()
                .await
                .remove(&instance)
                .ok_or(ExtensionError::RuntimeUnavailable("keymap WASM 实例不存在"))?;
            let RunningKeymap { commands, task } = running;
            let (reply, wait) = oneshot::channel();
            if commands.send(Command::Stop(reply)).await.is_err() {
                let _ = task.await;
                return Err(ExtensionError::RuntimeUnavailable("keymap WASM 实例已退出"));
            }
            let wait_result = wait.await;
            let task_result = task.await;
            wait_result.map_err(|_| ExtensionError::Runtime("keymap 停止确认失败".to_string()))?;
            task_result.map_err(|error| {
                ExtensionError::Runtime(format!("keymap 停止任务失败: {error}"))
            })?;
            Ok(())
        }

        async fn dispatch(
            &self,
            instance: KeymapWasmInstanceHandle,
            device: DeviceHandle,
            screen: ScreenSize,
            event: InputEvent,
            trace: Option<KeymapTraceContext>,
        ) -> ExtensionResult<InputResult> {
            let commands = self
                .instances
                .lock()
                .await
                .get(&instance)
                .map(|running| running.commands.clone())
                .ok_or(ExtensionError::RuntimeUnavailable("keymap WASM 实例不存在"))?;
            let (reply, wait) = oneshot::channel();
            commands
                .send(Command::Invoke(Invoke {
                    device,
                    screen,
                    event,
                    trace,
                    reply,
                }))
                .await
                .map_err(|_| ExtensionError::RuntimeUnavailable("keymap WASM 实例已退出"))?;
            wait.await
                .map_err(|_| ExtensionError::RuntimeUnavailable("keymap 调用任务已退出"))?
        }

        fn is_available(&self) -> bool {
            true
        }
    }

    fn guest_event(event: &InputEvent) -> GuestEvent {
        use guest::EventKind;
        let (kind, code, repeat, meta, button, x, y, delta_x, delta_y, index, pressed, value) =
            match event {
                InputEvent::KeyDown { code, repeat, meta } => (
                    EventKind::KeyDown,
                    Some(code.clone()),
                    *repeat,
                    *meta,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    false,
                    0.0,
                ),
                InputEvent::KeyUp { code, meta } => (
                    EventKind::KeyUp,
                    Some(code.clone()),
                    false,
                    *meta,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    false,
                    0.0,
                ),
                InputEvent::MouseDown { button, x, y } => (
                    EventKind::MouseDown,
                    None,
                    false,
                    0,
                    *button,
                    *x,
                    *y,
                    0,
                    0,
                    0,
                    true,
                    0.0,
                ),
                InputEvent::MouseUp { button, x, y } => (
                    EventKind::MouseUp,
                    None,
                    false,
                    0,
                    *button,
                    *x,
                    *y,
                    0,
                    0,
                    0,
                    false,
                    0.0,
                ),
                InputEvent::MouseMove {
                    x,
                    y,
                    delta_x,
                    delta_y,
                } => (
                    EventKind::MouseMove,
                    None,
                    false,
                    0,
                    0,
                    *x,
                    *y,
                    *delta_x,
                    *delta_y,
                    0,
                    false,
                    0.0,
                ),
                InputEvent::Wheel {
                    x,
                    y,
                    delta_x,
                    delta_y,
                } => (
                    EventKind::Wheel,
                    None,
                    false,
                    0,
                    0,
                    *x,
                    *y,
                    *delta_x,
                    *delta_y,
                    0,
                    false,
                    0.0,
                ),
                InputEvent::GamepadButton {
                    index,
                    pressed,
                    value,
                } => (
                    EventKind::GamepadButton,
                    None,
                    false,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    *index,
                    *pressed,
                    *value,
                ),
                InputEvent::GamepadAxis { index, value } => (
                    EventKind::GamepadAxis,
                    None,
                    false,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    *index,
                    false,
                    *value,
                ),
            };
        GuestEvent {
            kind,
            code,
            repeat,
            meta,
            button,
            x,
            y,
            delta_x,
            delta_y,
            index,
            pressed,
            value,
        }
    }

    async fn invoke_guest_result(
        host: &super::super::host_api::HostApi,
        executor: &CapabilityDeviceActionExecutor,
        touches: &mut HashMap<u64, (DeviceHandle, crate::capabilities::TouchHandle)>,
        invoke: &Invoke,
        guest_result: Result<Result<GuestResult, GuestError>, wasmtime::Error>,
        wasm_begin: Instant,
        wasm_end: Instant,
    ) -> ExtensionResult<InputResult> {
        let guest_result: Result<GuestResult, GuestError> = guest_result
            .map_err(|error| ExtensionError::Runtime(format!("keymap handle trap: {error}")))?;
        let guest_result = guest_result
            .map_err(|error| ExtensionError::Runtime(format!("keymap handle 失败: {error}")))?;
        // Authorize the complete result before executing its first native
        // action. A malicious component must not be able to smuggle an
        // allowed tap before a later forbidden action makes the invocation
        // fail; the WIT result is one permission-checked transaction. The
        // conversion itself stays sequential so a single result can legally
        // begin, move, and end a new touch contact.
        for action in &guest_result.actions {
            host.authorize(guest_action_permission(action))?;
        }

        let mut actions = Vec::with_capacity(guest_result.actions.len());
        // Phase 6 E2E：device_action 记授权+映射完成、动作执行开始
        let device_action = Instant::now();
        for action in guest_result.actions {
            let (_, native, slot) = native_action(action, invoke.screen, &invoke.device, touches)?;
            if let DeviceAction::TouchBegin { .. } = native {
                let slot =
                    slot.ok_or_else(|| ExtensionError::Runtime("touch slot 丢失".to_string()))?;
                if touches.contains_key(&slot) {
                    return Err(ExtensionError::Runtime(format!(
                        "touch slot 已占用: {slot}"
                    )));
                }
                let touch = executor
                    .execute(&invoke.device, &native)
                    .await?
                    .ok_or_else(|| {
                        ExtensionError::Runtime("touch.begin 未返回 TouchHandle".to_string())
                    })?;
                touches.insert(slot, (invoke.device.clone(), touch));
            } else {
                executor.execute(&invoke.device, &native).await?;
                if let DeviceAction::TouchEnd { touch } = &native {
                    touches.retain(|_, (_, active)| active != touch);
                }
            }
            actions.push(native);
        }
        if let Some(trace) = &invoke.trace {
            // scrcpy_write 记最后一个动作的控制写完成；无动作时与 device_action 同刻
            emit_keymap_trace(trace.record(
                KeymapTracePath::Wasm,
                wasm_begin,
                wasm_end,
                device_action,
                Instant::now(),
            ));
        }
        Ok(InputResult {
            consume: guest_result.consume,
            actions,
        })
    }

    fn guest_action_permission(action: &guest::DeviceAction) -> Permission {
        use guest::DeviceAction as GuestAction;
        match action {
            GuestAction::Tap(_) => Permission::InputTap,
            GuestAction::Swipe(_) => Permission::InputSwipe,
            GuestAction::Key(_) => Permission::InputKey,
            GuestAction::Text(_) => Permission::InputText,
            GuestAction::TouchBegin(_) | GuestAction::TouchMove(_) | GuestAction::TouchEnd(_) => {
                Permission::Touch
            }
        }
    }

    fn point(screen: ScreenSize, point: guest::Point) -> TouchPoint {
        TouchPoint::new(
            (point.x.clamp(0.0, 1.0) * screen.width as f32).round() as u32,
            (point.y.clamp(0.0, 1.0) * screen.height as f32).round() as u32,
            1.0,
        )
    }

    fn native_action(
        action: guest::DeviceAction,
        screen: ScreenSize,
        device: &DeviceHandle,
        touches: &HashMap<u64, (DeviceHandle, crate::capabilities::TouchHandle)>,
    ) -> ExtensionResult<(Permission, DeviceAction, Option<u64>)> {
        use guest::DeviceAction as GuestAction;
        Ok(match action {
            GuestAction::Tap(p) => (
                Permission::InputTap,
                DeviceAction::Tap {
                    point: point(screen, p),
                },
                None,
            ),
            GuestAction::Swipe(swipe) => (
                Permission::InputSwipe,
                DeviceAction::Swipe {
                    gesture: SwipeGesture::new(
                        point(screen, swipe.from_point),
                        point(screen, swipe.to),
                        std::time::Duration::from_millis(swipe.duration_ms.min(600_000)),
                    ),
                },
                None,
            ),
            GuestAction::Key(key) => {
                if !(1..=1000).contains(&key.code) {
                    return Err(ExtensionError::Runtime(format!(
                        "Android keycode 超出允许范围: {}",
                        key.code
                    )));
                }
                let action = match key.action.trim().to_ascii_lowercase().as_str() {
                    "down" => KeyAction::Down,
                    "up" => KeyAction::Up,
                    "press" => KeyAction::Press,
                    other => {
                        return Err(ExtensionError::Runtime(format!("未知 key action: {other}")))
                    }
                };
                (
                    Permission::InputKey,
                    DeviceAction::Key {
                        input: KeyInput::new(KeyCode::new(key.code), action),
                    },
                    None,
                )
            }
            GuestAction::Text(value) => {
                if value.len() > 16 * 1024 {
                    return Err(ExtensionError::Runtime(
                        "keymap text 超过 16KiB".to_string(),
                    ));
                }
                (
                    Permission::InputText,
                    DeviceAction::Text {
                        input: TextInput::new(value),
                    },
                    None,
                )
            }
            GuestAction::TouchBegin(begin) => (
                Permission::Touch,
                DeviceAction::TouchBegin {
                    point: point(screen, begin.point),
                },
                if (1..=1024).contains(&begin.slot) {
                    Some(begin.slot)
                } else {
                    return Err(ExtensionError::Runtime(format!(
                        "guest touch slot 超出范围: {}",
                        begin.slot
                    )));
                },
            ),
            GuestAction::TouchMove(move_) => {
                let (active_device, touch) =
                    touches.get(&move_.slot).cloned().ok_or_else(|| {
                        ExtensionError::Runtime(format!("未知 guest touch slot: {}", move_.slot))
                    })?;
                if active_device.id() != device.id() {
                    return Err(ExtensionError::Runtime(
                        "guest touch 不能跨设备继续操作".to_string(),
                    ));
                }
                (
                    Permission::Touch,
                    DeviceAction::TouchMove {
                        touch,
                        point: point(screen, move_.point),
                    },
                    None,
                )
            }
            GuestAction::TouchEnd(slot) => {
                let (active_device, touch) = touches.get(&slot).cloned().ok_or_else(|| {
                    ExtensionError::Runtime(format!("未知 guest touch slot: {slot}"))
                })?;
                if active_device.id() != device.id() {
                    return Err(ExtensionError::Runtime(
                        "guest touch 不能跨设备结束".to_string(),
                    ));
                }
                (Permission::Touch, DeviceAction::TouchEnd { touch }, None)
            }
        })
    }
}

#[cfg(feature = "wasm-runtime")]
pub(crate) use keymap_wasmtime::LazyKeymapWasmRuntime;

/// Resolve an application-specific `keymaps/<file>` resource from an App
/// Package.  App Package data is immutable and user overrides are selected by
/// the existing resolver; this adapter never falls back to another package.
#[derive(Clone, Debug)]
pub struct AppPackageKeymapSource {
    store: AppPackageStore,
}

impl AppPackageKeymapSource {
    pub fn new(store: AppPackageStore) -> Self {
        Self { store }
    }

    pub fn load(
        &self,
        android_package: &str,
        app_package: &str,
        version: &str,
        file: &str,
    ) -> ExtensionResult<Keymap> {
        let android_package = parse_android_package_name(android_package).map_err(to_runtime)?;
        let app_package = parse_app_package_id(app_package).map_err(to_runtime)?;
        let version = InstalledVersion::parse(version).map_err(to_runtime)?;
        if file.is_empty()
            || file.contains(['/', '\\'])
            || !(file.ends_with(".yaml") || file.ends_with(".yml"))
        {
            return Err(ExtensionError::Runtime(format!(
                "keymap 文件名必须是当前 App Package 的 YAML 短名: {file}"
            )));
        }
        let path = ResourcePath::parse(&format!("keymaps/{file}")).map_err(to_runtime)?;
        let Some(resource) = self
            .store
            .resolver()
            .resolve_path(&android_package, app_package, version, path)
            .map_err(to_runtime)?
        else {
            return Err(ExtensionError::Runtime(format!(
                "App Package keymap 不存在: {file}"
            )));
        };
        let bytes = resource.read_bytes().map_err(to_runtime)?;
        let content = std::str::from_utf8(&bytes)
            .map_err(|error| ExtensionError::Runtime(format!("keymap YAML 不是 UTF-8: {error}")))?;
        parse_keymap_content(content, &format!("keymaps/{file}"))
            .map_err(|diagnostics| ExtensionError::Runtime(format_diagnostics(&diagnostics)))
    }
}

fn to_runtime(error: impl std::fmt::Display) -> ExtensionError {
    ExtensionError::Runtime(error.to_string())
}

/// Load the raw YAML content of a user-selected keymap profile from the
/// existing per-partition storage (`data/<pkg>/keymaps/<name>.yaml`). The
/// content is handed to the keymap guest verbatim; the store already validated
/// the schema (`version/name/bindings`) when the file was written and
/// normalizes the scheme name, and a missing scheme is a start-time error
/// rather than a silent pass-through.
pub fn load_user_profile(
    store: &crate::keymaps::KeymapStore,
    partition: &str,
    name: &str,
) -> ExtensionResult<String> {
    let id = format!("{partition}/{name}");
    let file = store
        .get(&id)
        .map_err(to_runtime)?
        .ok_or_else(|| ExtensionError::Runtime(format!("keymap 方案不存在: {id}")))?;
    Ok(file.content)
}

fn format_diagnostics(diagnostics: &[crate::keymaps::KeymapDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("；")
}

pub(crate) fn android_keycode(code: &str) -> Option<u32> {
    if let Some(letter) = code.strip_prefix("Key") {
        let byte = letter.as_bytes().first().copied()?;
        if letter.len() == 1 && byte.is_ascii_uppercase() {
            return Some(29 + u32::from(byte - b'A'));
        }
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        let byte = digit.as_bytes().first().copied()?;
        if digit.len() == 1 && byte.is_ascii_digit() {
            return Some(7 + u32::from(byte - b'0'));
        }
    }
    Some(match code {
        "ArrowUp" => 19,
        "ArrowDown" => 20,
        "ArrowLeft" => 21,
        "ArrowRight" => 22,
        "Home" => 122,
        "End" => 123,
        "PageUp" => 92,
        "PageDown" => 93,
        "Insert" => 124,
        "Delete" => 112,
        "Space" => 62,
        "Enter" => 66,
        "NumpadEnter" => 160,
        "Tab" => 61,
        "Escape" => 111,
        "Backspace" => 67,
        "AltLeft" => 57,
        "AltRight" => 58,
        "ShiftLeft" => 59,
        "ShiftRight" => 60,
        "ControlLeft" => 113,
        "ControlRight" => 114,
        "MetaLeft" => 117,
        "MetaRight" => 118,
        "CapsLock" => 115,
        "NumLock" => 143,
        "ScrollLock" => 116,
        "PrintScreen" => 120,
        "Pause" => 121,
        "ContextMenu" => 82,
        "Backquote" => 68,
        "Minus" => 69,
        "Equal" => 70,
        "BracketLeft" => 71,
        "BracketRight" => 72,
        "Backslash" | "IntlBackslash" => 73,
        "Semicolon" => 74,
        "Quote" => 75,
        "Comma" => 55,
        "Period" => 56,
        "Slash" => 76,
        "F1" => 131,
        "F2" => 132,
        "F3" => 133,
        "F4" => 134,
        "F5" => 135,
        "F6" => 136,
        "F7" => 137,
        "F8" => 138,
        "F9" => 139,
        "F10" => 140,
        "F11" => 141,
        "F12" => 142,
        "Numpad0" => 144,
        "Numpad1" => 145,
        "Numpad2" => 146,
        "Numpad3" => 147,
        "Numpad4" => 148,
        "Numpad5" => 149,
        "Numpad6" => 150,
        "Numpad7" => 151,
        "Numpad8" => 152,
        "Numpad9" => 153,
        "NumpadDivide" => 154,
        "NumpadMultiply" => 155,
        "NumpadSubtract" => 156,
        "NumpadAdd" => 157,
        "NumpadDecimal" => 158,
        "NumpadComma" => 159,
        "NumpadEqual" => 161,
        "NumpadParenLeft" => 162,
        "NumpadParenRight" => 163,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::app_packages::AppPackageStore;
    use crate::keymaps::{KeymapAction, KeymapBinding};

    fn keymap(bindings: &[(&str, KeymapAction)]) -> Keymap {
        Keymap {
            version: 1,
            name: "test".to_string(),
            bindings: bindings
                .iter()
                .map(|(key, action)| KeymapBinding {
                    key: (*key).to_string(),
                    action: action.clone(),
                })
                .collect(),
        }
    }

    #[test]
    fn shipped_manifest_declares_keymap_panel_and_only_input_capabilities() {
        // The Phase 6 manifest parser does not yet own UI contribution
        // fields. Keep the package descriptor explicit here without
        // pretending that generic Host installation already accepts it.
        assert!(KEYMAP_EXTENSION_MANIFEST_TOML.contains("id = \"gamer.keymap\""));
        assert!(KEYMAP_EXTENSION_MANIFEST_TOML.contains("permissions = [\"input.tap\""));
        assert!(KEYMAP_EXTENSION_MANIFEST_TOML.contains("panel_id = \"keymaps\""));
        assert!(KEYMAP_EXTENSION_MANIFEST_TOML.contains("title = \"映射\""));
        assert!(KEYMAP_EXTENSION_MANIFEST_TOML.contains("runtime = \"core\""));
        assert!(KEYMAP_EXTENSION_MANIFEST_TOML.contains("component = \"console.keymaps\""));
        assert!(!KEYMAP_EXTENSION_MANIFEST_TOML.contains("entry = \"ui/"));
    }

    /// 官方市场打包源（tools/plugins/gamer.keymap/manifest.toml）与本常量锁同步：
    /// build-plugins.ps1 以文件为准打包，漂移会导致线上包与运行时语义不一致。
    #[test]
    fn packaging_manifest_stays_in_sync_with_shipped_constant() {
        let packaged = include_str!("../../../tools/plugins/gamer.keymap/manifest.toml");
        assert_eq!(
            KEYMAP_EXTENSION_MANIFEST_TOML.trim(),
            packaged.trim(),
            "tools/plugins/gamer.keymap/manifest.toml 与 KEYMAP_EXTENSION_MANIFEST_TOML 不一致"
        );
    }

    #[test]
    fn input_event_decoder_keeps_the_wire_contract_small() {
        let event =
            decode_input_event(br#"{"type":"gamepad_axis","index":2,"value":0.75}"#).unwrap();
        assert_eq!(
            event,
            InputEvent::GamepadAxis {
                index: 2,
                value: 0.75,
            }
        );
        assert!(decode_input_event(br#"{"type":"pointer","pointer_id":3}"#).is_err());
    }

    #[test]
    fn user_profile_loader_reads_partition_yaml_verbatim() {
        let temp = TempDir::new().unwrap();
        let store = crate::keymaps::KeymapStore::new(temp.path().to_path_buf());
        store
            .create(
                "com.example.game",
                "测试方案",
                &keymap(&[("KeyW", KeymapAction::Hold { at: [0.1, 0.9] })]),
            )
            .unwrap();
        let profile = load_user_profile(&store, "com.example.game", "测试方案").unwrap();
        assert!(profile.contains("KeyW"));
        assert!(load_user_profile(&store, "com.example.game", "缺失方案").is_err());
    }

    #[test]
    fn input_event_json_is_small_and_has_stable_names() {
        let event = InputEvent::GamepadButton {
            index: 2,
            pressed: true,
            value: 1.0,
        };
        assert_eq!(
            serde_json::to_value(event).unwrap()["type"],
            "gamepad_button"
        );
        assert_eq!(
            InputEvent::MouseDown {
                button: 0,
                x: 1,
                y: 2
            }
            .selector(),
            Some("MouseLeft".into())
        );
    }

    #[test]
    fn app_package_keymap_source_reads_only_the_selected_package() {
        let temp = TempDir::new().unwrap();
        let store = AppPackageStore::new(temp.path());
        let manifest = b"format_version = 2\nid = \"official.game\"\nversion = \"1.0.0\"\n[android]\npackages = [\"com.game\"]\n";
        let keymap = b"version: 1\nname: package\nbindings: []\n";
        let mut bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options = SimpleFileOptions::default();
            writer.start_file("manifest.toml", options).unwrap();
            std::io::Write::write_all(&mut writer, manifest).unwrap();
            writer.start_file("keymaps/default.yaml", options).unwrap();
            std::io::Write::write_all(&mut writer, keymap).unwrap();
            writer.finish().unwrap();
        }
        store.install_archive(&bytes, None).unwrap();
        let source = AppPackageKeymapSource::new(store);
        let loaded = source
            .load("com.game", "official.game", "1.0.0", "default.yaml")
            .unwrap();
        assert_eq!(loaded.name, "package");
        assert!(source
            .load("com.other", "official.game", "1.0.0", "default.yaml")
            .is_err());
    }
}

#[cfg(all(test, feature = "wasm-runtime"))]
mod wasm_component_tests {
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use async_trait::async_trait;
    use tempfile::TempDir;

    use super::*;
    use crate::capabilities::{
        CapabilityError, CapabilityRegistry, DeviceId, InputService, KeyInput, SwipeGesture,
        TextInput, TouchService,
    };
    use crate::core::AppContext;
    use crate::extensions::{
        ExtensionError, ExtensionId, ExtensionPath, ExtensionService, ExtensionState,
        ExtensionStore, NoWasmRuntime, PermissionError,
    };

    #[derive(Default)]
    struct Trace {
        events: Mutex<Vec<String>>,
    }

    impl Trace {
        fn push(&self, event: impl Into<String>) {
            self.events.lock().unwrap().push(event.into());
        }

        fn snapshot(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }

        fn clear(&self) {
            self.events.lock().unwrap().clear();
        }
    }

    #[async_trait]
    impl InputService for Trace {
        async fn tap(
            &self,
            _device: &DeviceHandle,
            point: TouchPoint,
        ) -> Result<(), CapabilityError> {
            self.push(format!("input.tap:{}:{}", point.x(), point.y()));
            Ok(())
        }

        async fn swipe(
            &self,
            _device: &DeviceHandle,
            gesture: SwipeGesture,
        ) -> Result<(), CapabilityError> {
            self.push(format!(
                "input.swipe:{}:{}:{}:{}:{}",
                gesture.start().x(),
                gesture.start().y(),
                gesture.end().x(),
                gesture.end().y(),
                gesture.duration().as_millis()
            ));
            Ok(())
        }

        async fn key(
            &self,
            _device: &DeviceHandle,
            input: KeyInput,
        ) -> Result<(), CapabilityError> {
            self.push(format!(
                "input.key:{}:{:?}",
                input.code().value(),
                input.action()
            ));
            Ok(())
        }

        async fn text(
            &self,
            _device: &DeviceHandle,
            input: TextInput,
        ) -> Result<(), CapabilityError> {
            self.push(format!("input.text:{}", input.as_str()));
            Ok(())
        }
    }

    #[async_trait]
    impl TouchService for Trace {
        async fn begin(
            &self,
            _device: &DeviceHandle,
            point: TouchPoint,
        ) -> Result<TouchHandle, CapabilityError> {
            self.push(format!("touch.begin:{}:{}", point.x(), point.y()));
            Ok(TouchHandle::new())
        }

        async fn move_touch(
            &self,
            _touch: &TouchHandle,
            point: TouchPoint,
        ) -> Result<(), CapabilityError> {
            self.push(format!("touch.move:{}:{}", point.x(), point.y()));
            Ok(())
        }

        async fn end(&self, _touch: &TouchHandle) -> Result<(), CapabilityError> {
            self.push("touch.end");
            Ok(())
        }
    }

    fn fixture_component() -> Vec<u8> {
        super::build_guest_fixture_component()
    }

    fn gplugin(component: &[u8], permissions: &[&str]) -> Vec<u8> {
        super::package_guest_fixture_gplugin(component, permissions)
    }

    fn registry(trace: Arc<Trace>) -> CapabilityRegistry {
        CapabilityRegistry::builder()
            .with_input_service(trace.clone() as Arc<dyn InputService>)
            .with_touch_service(trace as Arc<dyn TouchService>)
            .build()
    }

    fn service(
        temp: &TempDir,
        trace: Arc<Trace>,
        runtime: Arc<LazyKeymapWasmRuntime>,
    ) -> ExtensionService {
        ExtensionService::with_keymap_runtime(
            ExtensionStore::new(temp.path()),
            Arc::new(NoWasmRuntime),
            runtime,
            registry(trace),
        )
    }

    fn device() -> DeviceHandle {
        DeviceHandle::new(DeviceId::new("device-1"))
    }

    fn app_context() -> AppContext {
        AppContext::from_legacy_package("device-1", "com.example.game").unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_keymap_gplugin_invokes_wit_and_native_capabilities() {
        let component = fixture_component();
        assert!(component.starts_with(b"\0asm"));
        let package = gplugin(
            &component,
            &["input.tap", "input.swipe", "input.key", "touch"],
        );
        let mut archive = zip::ZipArchive::new(Cursor::new(&package)).unwrap();
        for path in ["manifest.toml", "plugin.wasm", "ui/index.html"] {
            assert!(archive.by_name(path).is_ok(), "真实 gplugin 缺少 {path}");
        }

        let temp = TempDir::new().unwrap();
        let trace = Arc::new(Trace::default());
        let runtime = Arc::new(LazyKeymapWasmRuntime::new());
        let service = service(&temp, trace.clone(), runtime);
        let installed = service.install(&package).await.unwrap();
        assert_eq!(installed.state(), ExtensionState::Installed);
        let id = ExtensionId::parse(KEYMAP_EXTENSION_ID).unwrap();
        let enabled = service.enable(&id).await.unwrap();
        assert_eq!(enabled.state(), ExtensionState::Enabled);
        let contributions = service.ui_contributions().unwrap();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].panel_id, KEYMAP_PANEL_ID);
        assert_eq!(
            service
                .read_ui_file(&id, &ExtensionPath::parse("ui/index.html").unwrap())
                .unwrap()
                .0,
            include_bytes!("../../tests/keymap-guest/ui/index.html")
        );

        let running = service
            .start_with_context(&id, Some(app_context()), None)
            .await
            .unwrap();
        assert_eq!(running.state(), ExtensionState::Running);
        let device = device();
        let screen = ScreenSize::new(1000, 500);

        for event in [
            InputEvent::key_down("KeyW"),
            InputEvent::key_down("KeyA"),
            InputEvent::MouseDown {
                button: 0,
                x: 10,
                y: 20,
            },
            InputEvent::MouseMove {
                x: 400,
                y: 300,
                delta_x: 390,
                delta_y: 280,
            },
            InputEvent::MouseUp {
                button: 0,
                x: 400,
                y: 300,
            },
            InputEvent::GamepadButton {
                index: 0,
                pressed: true,
                value: 1.0,
            },
            InputEvent::GamepadButton {
                index: 0,
                pressed: false,
                value: 0.0,
            },
            InputEvent::GamepadAxis {
                index: 0,
                value: 0.5,
            },
            InputEvent::GamepadAxis {
                index: 0,
                value: -0.5,
            },
            InputEvent::GamepadAxis {
                index: 0,
                value: 0.0,
            },
            InputEvent::key_up("KeyW"),
            InputEvent::key_up("KeyA"),
            InputEvent::key_down("Space"),
            InputEvent::key_down("KeyE"),
        ] {
            let result = service
                .dispatch_keymap_input(device.clone(), screen, event, None)
                .await
                .unwrap();
            assert!(result.consume, "fixture action should consume input");
        }
        let passed = service
            .dispatch_keymap_input(
                device.clone(),
                screen,
                InputEvent::Wheel {
                    x: 500,
                    y: 250,
                    delta_x: 0,
                    delta_y: -120,
                },
                None,
            )
            .await
            .unwrap();
        assert!(!passed.consume);
        assert!(passed.actions.is_empty());

        let events = trace.snapshot();
        assert!(events.iter().any(|event| event.starts_with("input.tap:")));
        assert!(events.iter().any(|event| event.starts_with("input.swipe:")));
        assert!(events
            .iter()
            .any(|event| event.starts_with("input.key:62:Down")));
        assert!(events
            .iter()
            .any(|event| event.starts_with("input.key:62:Up")));
        assert_eq!(
            events
                .iter()
                .filter(|event| event.starts_with("touch.begin:"))
                .count(),
            4
        );
        assert!(events.iter().any(|event| event.starts_with("touch.move:")));
        assert_eq!(
            events
                .iter()
                .filter(|event| event == &&"touch.end".to_string())
                .count(),
            4
        );

        // Stop must clean a live WASM-owned contact before the component task
        // is dropped; the UI contribution remains available while enabled.
        service
            .dispatch_keymap_input(device, screen, InputEvent::key_down("KeyW"), None)
            .await
            .unwrap();
        let stopped = service.stop(&id).await.unwrap();
        assert_eq!(stopped.state(), ExtensionState::Enabled);
        assert_eq!(service.ui_contributions().unwrap().len(), 1);
        assert!(
            trace
                .snapshot()
                .iter()
                .filter(|event| *event == "touch.end")
                .count()
                >= 5
        );

        assert!(service
            .uninstall(&id, installed.active_version())
            .await
            .unwrap());
        assert!(service.ui_contributions().unwrap().is_empty());
        assert!(!temp
            .path()
            .join("extensions")
            .join(KEYMAP_EXTENSION_ID)
            .exists());
    }

    /// Profile 数据通道端到端：start 携带的 keymap YAML 覆盖内置 WASD 规则，
    /// 未覆盖的按键回落内置默认，非法 profile 使 start 失败而非静默降级。
    #[tokio::test]
    async fn real_keymap_guest_consumes_user_profile_yaml() {
        let package = gplugin(
            &fixture_component(),
            &["input.tap", "input.swipe", "input.key", "touch"],
        );
        let temp = TempDir::new().unwrap();
        let trace = Arc::new(Trace::default());
        let service = service(&temp, trace.clone(), Arc::new(LazyKeymapWasmRuntime::new()));
        let installed = service.install(&package).await.unwrap();
        let id = ExtensionId::parse(KEYMAP_EXTENSION_ID).unwrap();
        service.enable(&id).await.unwrap();

        let profile = "version: 1\nname: fixture\nbindings:\n\
             - key: KeyW\n  action:\n    type: hold\n    at: [0.10, 0.90]\n\
             - key: KeyQ\n  action:\n    type: raw_key\n    code: Enter\n";
        let running = service
            .start_with_context(&id, Some(app_context()), Some(profile.to_string()))
            .await
            .unwrap();
        assert_eq!(running.state(), ExtensionState::Running);

        let screen = ScreenSize::new(1000, 500);
        // KeyW 被 profile 覆盖：hold at [0.10, 0.90] → (100, 450)，slot=guest 规则序号段。
        service
            .dispatch_keymap_input(device(), screen, InputEvent::key_down("KeyW"), None)
            .await
            .unwrap();
        assert!(trace
            .snapshot()
            .contains(&"touch.begin:100:450".to_string()));
        service
            .dispatch_keymap_input(device(), screen, InputEvent::key_up("KeyW"), None)
            .await
            .unwrap();
        // KeyQ raw_key Enter(66)：down/up 配对。
        service
            .dispatch_keymap_input(device(), screen, InputEvent::key_down("KeyQ"), None)
            .await
            .unwrap();
        service
            .dispatch_keymap_input(device(), screen, InputEvent::key_up("KeyQ"), None)
            .await
            .unwrap();
        // 未被 profile 覆盖的 KeyA 回落内置默认：touch.begin at (200, 300)。
        service
            .dispatch_keymap_input(device(), screen, InputEvent::key_down("KeyA"), None)
            .await
            .unwrap();
        let events = trace.snapshot();
        assert!(events
            .iter()
            .any(|event| event.starts_with("input.key:66:Down")));
        assert!(events
            .iter()
            .any(|event| event.starts_with("input.key:66:Up")));
        assert!(events.iter().any(|event| event == "touch.begin:200:300"));

        service.stop(&id).await.unwrap();
        trace.clear();

        // 非法 profile：guest start 返回错误 → 插件进入 Failed，不静默降级。
        let bad_profile =
            "version: 1\nbindings:\n  - key: KeyW\n    action:\n      type: nope\n".to_string();
        let error = service
            .start_with_context(&id, Some(app_context()), Some(bad_profile))
            .await
            .unwrap_err();
        assert!(
            matches!(error, ExtensionError::Runtime(message) if message.contains("keymap start 失败"))
        );
        let failed = service.list().unwrap().remove(0);
        assert_eq!(failed.state(), ExtensionState::Failed);
        // 重新启用恢复可启动状态（Failed → Enabled），再以合法 profile 启动成功。
        service.enable(&id).await.unwrap();
        service
            .start_with_context(&id, Some(app_context()), Some(profile.to_string()))
            .await
            .unwrap();
        service.stop(&id).await.unwrap();
        service
            .uninstall(&id, installed.active_version())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn real_wit_action_is_rejected_without_touch_permission() {
        let package = gplugin(&fixture_component(), &["input.key"]);
        let temp = TempDir::new().unwrap();
        let trace = Arc::new(Trace::default());
        let service = service(&temp, trace.clone(), Arc::new(LazyKeymapWasmRuntime::new()));
        let installed = service.install(&package).await.unwrap();
        let id = ExtensionId::parse(KEYMAP_EXTENSION_ID).unwrap();
        service.enable(&id).await.unwrap();
        service
            .start_with_context(&id, Some(app_context()), None)
            .await
            .unwrap();

        let error = service
            .dispatch_keymap_input(
                device(),
                ScreenSize::new(1000, 500),
                InputEvent::key_down("KeyW"),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ExtensionError::Permission(PermissionError::NotGranted(permission))
                if permission == "touch"
        ));
        assert!(trace
            .snapshot()
            .iter()
            .all(|event| !event.starts_with("touch.begin:")));
        service.stop(&id).await.unwrap();
        service
            .uninstall(&id, installed.active_version())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn real_wit_input_to_native_chain_p95_stays_bounded() {
        let package = gplugin(
            &fixture_component(),
            &["input.tap", "input.swipe", "input.key", "touch"],
        );
        let temp = TempDir::new().unwrap();
        let trace = Arc::new(Trace::default());
        let service = service(&temp, trace, Arc::new(LazyKeymapWasmRuntime::new()));
        let installed = service.install(&package).await.unwrap();
        let id = ExtensionId::parse(KEYMAP_EXTENSION_ID).unwrap();
        service.enable(&id).await.unwrap();
        service
            .start_with_context(&id, Some(app_context()), None)
            .await
            .unwrap();

        let mut samples = Vec::with_capacity(64);
        for _ in 0..64 {
            let started = Instant::now();
            let result = service
                .dispatch_keymap_input(
                    device(),
                    ScreenSize::new(1000, 500),
                    InputEvent::key_down("Space"),
                    None,
                )
                .await
                .unwrap();
            assert!(result.consume);
            samples.push(started.elapsed());
        }
        samples.sort_unstable();
        eprintln!(
            "real keymap WIT latency: p50={:?}, p95={:?}",
            samples[31], samples[60]
        );
        assert!(samples[31] < Duration::from_millis(50));
        assert!(samples[60] < Duration::from_millis(100));

        service.stop(&id).await.unwrap();
        service
            .uninstall(&id, installed.active_version())
            .await
            .unwrap();
    }
}
