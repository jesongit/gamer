//! The first extension-specific adapter: a device-independent keymap runner.
//!
//! The runner is deliberately split from both the YAML store and the WASM
//! adapter.  A keymap extension receives normalized input events, owns only
//! logical binding state, and emits capability actions.  In particular, a
//! `TouchHandle` is the only contact identity that can be retained here;
//! scrcpy pointer IDs stay inside the transport adapter.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

use crate::app_packages::{
    parse_android_package_name, parse_app_package_id, AppPackageStore, InstalledVersion,
    ResourcePath,
};
use crate::capabilities::{
    CapabilityRegistry, DeviceHandle, KeyAction, KeyCode, KeyInput, SwipeGesture, TextInput,
    TouchHandle, TouchPoint,
};
use crate::device::scrcpy::{ScrcpySession, ACTION_DOWN, ACTION_MOVE, ACTION_UP};
use crate::keymaps::{parse_keymap_content, Keymap, KeymapAction, KeymapBinding};

use super::error::{ExtensionError, ExtensionResult};
use super::host_api::{HostApi, HostApiCatalog};
use super::manifest::ExtensionManifest;
use super::permissions::Permission;

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
panel_id = "keymaps"
title = "映射"
icon = "⌨"
order = 30
location = "console.right"
runtime = "iframe"
requires_device = true
preferred_width = 360
entry = "ui/index.html"
"#;

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

/// Native transport adapter for the keymap capability boundary.
///
/// The runner only sees `TouchHandle`s.  This adapter owns the short-lived
/// mapping from those handles to scrcpy's wire-level pointer slots, including
/// allocation and cleanup when a socket write fails.  No pointer id is
/// serialized in `InputEvent`, `DeviceAction`, or the WASM ABI.
pub struct ScrcpyDeviceActionExecutor {
    session: Arc<ScrcpySession>,
    pointers: Mutex<HashMap<TouchHandle, u64>>,
}

impl ScrcpyDeviceActionExecutor {
    pub fn new(session: Arc<ScrcpySession>) -> Self {
        Self {
            session,
            pointers: Mutex::new(HashMap::new()),
        }
    }

    fn validate_device(&self, device: &DeviceHandle) -> ExtensionResult<()> {
        if device.id().as_str() == self.session.device.id {
            Ok(())
        } else {
            Err(ExtensionError::Runtime(format!(
                "keymap device handle 不匹配 scrcpy 会话: {}",
                device.id().as_str()
            )))
        }
    }

    async fn begin_touch(&self, point: TouchPoint) -> ExtensionResult<TouchHandle> {
        let touch = TouchHandle::new();
        let pointer_id = {
            let mut pointers = self.pointers.lock().await;
            let Some(pointer_id) =
                (1..=31).find(|candidate| !pointers.values().any(|active| active == candidate))
            else {
                return Err(ExtensionError::Runtime(
                    "scrcpy touch pointer slots exhausted".to_string(),
                ));
            };
            pointers.insert(touch, pointer_id);
            pointer_id
        };

        if let Err(error) = self
            .session
            .inject_touch(
                ACTION_DOWN,
                pointer_id,
                point.x() as f32,
                point.y() as f32,
                point.pressure(),
            )
            .await
        {
            self.pointers.lock().await.remove(&touch);
            return Err(ExtensionError::Runtime(format!(
                "scrcpy touch.begin failed: {error}"
            )));
        }
        Ok(touch)
    }

    async fn move_touch(&self, touch: &TouchHandle, point: TouchPoint) -> ExtensionResult<()> {
        let Some(pointer_id) = self.pointers.lock().await.get(touch).copied() else {
            return Err(ExtensionError::Runtime(
                "scrcpy touch.move received an unknown TouchHandle".to_string(),
            ));
        };
        self.session
            .inject_touch(
                ACTION_MOVE,
                pointer_id,
                point.x() as f32,
                point.y() as f32,
                point.pressure(),
            )
            .await
            .map_err(|error| ExtensionError::Runtime(format!("scrcpy touch.move failed: {error}")))
    }

    async fn end_touch(&self, touch: &TouchHandle) -> ExtensionResult<()> {
        let Some(pointer_id) = self.pointers.lock().await.remove(touch) else {
            return Err(ExtensionError::Runtime(
                "scrcpy touch.end received an unknown TouchHandle".to_string(),
            ));
        };
        if let Err(error) = self
            .session
            .inject_touch(ACTION_UP, pointer_id, 0.0, 0.0, 0.0)
            .await
        {
            self.pointers.lock().await.insert(*touch, pointer_id);
            return Err(ExtensionError::Runtime(format!(
                "scrcpy touch.end failed: {error}"
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl DeviceActionExecutor for ScrcpyDeviceActionExecutor {
    async fn execute(
        &self,
        device: &DeviceHandle,
        action: &DeviceAction,
    ) -> ExtensionResult<Option<TouchHandle>> {
        self.validate_device(device)?;
        match action {
            DeviceAction::Tap { point } => self
                .session
                .tap(point.x() as f32, point.y() as f32)
                .await
                .map_err(|error| ExtensionError::Runtime(format!("scrcpy tap failed: {error}")))?,
            DeviceAction::Swipe { gesture } => {
                let start = gesture.start();
                let end = gesture.end();
                self.session
                    .swipe(
                        start.x() as f32,
                        start.y() as f32,
                        end.x() as f32,
                        end.y() as f32,
                        gesture.duration().as_millis() as u64,
                    )
                    .await
                    .map_err(|error| {
                        ExtensionError::Runtime(format!("scrcpy swipe failed: {error}"))
                    })?;
            }
            DeviceAction::Key { input } => {
                let action = match input.action() {
                    KeyAction::Down => ACTION_DOWN,
                    KeyAction::Up => ACTION_UP,
                    KeyAction::Press => {
                        self.session
                            .inject_keycode(ACTION_DOWN, input.code().value(), 0, 0)
                            .await
                            .map_err(|error| {
                                ExtensionError::Runtime(format!("scrcpy key down failed: {error}"))
                            })?;
                        ACTION_UP
                    }
                };
                self.session
                    .inject_keycode(action, input.code().value(), 0, 0)
                    .await
                    .map_err(|error| {
                        ExtensionError::Runtime(format!("scrcpy key failed: {error}"))
                    })?;
            }
            DeviceAction::Text { input } => self
                .session
                .inject_text(input.as_str())
                .await
                .map_err(|error| ExtensionError::Runtime(format!("scrcpy text failed: {error}")))?,
            DeviceAction::TouchBegin { point } => {
                return self.begin_touch(*point).await.map(Some);
            }
            DeviceAction::TouchMove { touch, point } => {
                self.move_touch(touch, *point).await?;
            }
            DeviceAction::TouchEnd { touch } => {
                self.end_touch(touch).await?;
            }
        }
        Ok(None)
    }
}

#[derive(Clone, Debug)]
struct ActiveBinding {
    action: KeymapAction,
    touch: Option<TouchHandle>,
    raw_keycode: Option<u32>,
}

#[derive(Default)]
pub struct KeymapState {
    active: HashMap<String, ActiveBinding>,
    suppressed_keyups: HashSet<String>,
    mouse_touch: Option<TouchHandle>,
}

/// Deterministic keymap state machine.  All map actions are planned from the
/// immutable profile; only active raw keys and opaque touch handles are state.
pub struct KeymapRunner {
    profile: RwLock<Keymap>,
    device: DeviceHandle,
    screen: ScreenSize,
    executor: Arc<dyn DeviceActionExecutor>,
    state: Mutex<KeymapState>,
}

impl KeymapRunner {
    pub fn new(
        profile: Keymap,
        device: DeviceHandle,
        screen: ScreenSize,
        executor: Arc<dyn DeviceActionExecutor>,
    ) -> Self {
        Self {
            profile: RwLock::new(profile),
            device,
            screen,
            executor,
            state: Mutex::new(KeymapState::default()),
        }
    }

    pub async fn profile(&self) -> Keymap {
        self.profile.read().await.clone()
    }

    /// Replace the active scheme without leaving a previous hold or raw key
    /// pressed.  This is also used when an App Package is switched.
    pub async fn replace_profile(&self, profile: Keymap) -> ExtensionResult<()> {
        self.release_all().await?;
        *self.profile.write().await = profile;
        Ok(())
    }

    pub async fn dispatch(&self, event: InputEvent) -> ExtensionResult<InputResult> {
        match event {
            InputEvent::MouseDown { button, x, y } => {
                let selector = mouse_selector(button).map(str::to_string);
                let mapped = match selector.as_deref() {
                    Some(selector) => self.binding(selector).await.is_some(),
                    None => return Ok(InputResult::pass()),
                };
                if !mapped {
                    return self.handle_mouse_down(x, y).await;
                }
                return self
                    .handle_press(selector.expect("mouse selector checked"), false)
                    .await;
            }
            InputEvent::MouseUp { button, .. } => {
                let selector = mouse_selector(button).map(str::to_string);
                let mapped = match selector.as_deref() {
                    Some(selector) => self.binding(selector).await.is_some(),
                    None => return Ok(InputResult::pass()),
                };
                if !mapped {
                    return self.handle_mouse_up().await;
                }
                return self
                    .handle_release(selector.expect("mouse selector checked"))
                    .await;
            }
            InputEvent::MouseMove { x, y, .. } => return self.handle_mouse_move(x, y).await,
            event => {
                if let InputEvent::GamepadAxis { index, value } = event {
                    return self.handle_axis(index, value).await;
                }
                if let InputEvent::Wheel { .. } = event {
                    return Ok(InputResult::pass());
                }
                let Some(selector) = event.selector() else {
                    return Ok(InputResult::pass());
                };
                if event.is_press() {
                    return self.handle_press(selector, event.repeat()).await;
                }
                if event.is_release() {
                    return self.handle_release(selector).await;
                }
            }
        }
        Ok(InputResult::pass())
    }

    pub async fn release_all(&self) -> ExtensionResult<InputResult> {
        let active = {
            let mut state = self.state.lock().await;
            state.suppressed_keyups.clear();
            let active = std::mem::take(&mut state.active);
            let mouse = state.mouse_touch.take();
            (active, mouse)
        };
        let mut actions = Vec::new();
        for (selector, active) in active.0 {
            if let Some(touch) = active.touch {
                let action = DeviceAction::TouchEnd { touch };
                self.execute(&action).await?;
                actions.push(action);
            } else if let Some(keycode) = active.raw_keycode {
                let action = DeviceAction::Key {
                    input: KeyInput::new(KeyCode::new(keycode), KeyAction::Up),
                };
                self.execute(&action).await?;
                actions.push(action);
            }
            tracing::debug!(selector, "keymap active input released");
        }
        if let Some(touch) = active.1 {
            let action = DeviceAction::TouchEnd { touch };
            self.execute(&action).await?;
            actions.push(action);
        }
        Ok(if actions.is_empty() {
            InputResult::pass()
        } else {
            InputResult::consume(actions)
        })
    }

    pub async fn pressed_selectors(&self) -> Vec<String> {
        let state = self.state.lock().await;
        let mut selectors: Vec<_> = state.active.keys().cloned().collect();
        selectors.sort();
        selectors
    }

    async fn handle_press(&self, selector: String, repeat: bool) -> ExtensionResult<InputResult> {
        let binding = self.binding(&selector).await;
        let Some(binding) = binding else {
            return Ok(InputResult::pass());
        };

        if self.state.lock().await.active.contains_key(&selector) {
            // A tap/swipe repeat is consumed without repeating the action.  A
            // raw-key repeat is represented by another backend key-down; the
            // capability boundary intentionally does not expose transport
            // repeat bits.
            if repeat && matches!(binding.action, KeymapAction::RawKey { .. }) {
                if let Some(keycode) = raw_keycode(&binding.action) {
                    let action = DeviceAction::Key {
                        input: KeyInput::new(KeyCode::new(keycode), KeyAction::Down),
                    };
                    self.execute(&action).await?;
                    return Ok(InputResult::consume(vec![action]));
                }
            }
            return Ok(InputResult::consume(Vec::new()));
        }

        if repeat {
            return Ok(InputResult::consume(Vec::new()));
        }

        let (action, raw_keycode, touch) = match &binding.action {
            KeymapAction::Tap { at } => (
                DeviceAction::Tap {
                    point: self.screen.point(*at),
                },
                None,
                None,
            ),
            KeymapAction::Swipe {
                from,
                to,
                duration_ms,
            } => (
                DeviceAction::Swipe {
                    gesture: SwipeGesture::new(
                        self.screen.point(*from),
                        self.screen.point(*to),
                        Duration::from_millis(*duration_ms as u64),
                    ),
                },
                None,
                None,
            ),
            KeymapAction::RawKey { .. } => {
                let keycode = raw_keycode(&binding.action).ok_or_else(|| {
                    ExtensionError::Runtime("keymap raw_key 没有可用 Android keycode".to_string())
                })?;
                (
                    DeviceAction::Key {
                        input: KeyInput::new(KeyCode::new(keycode), KeyAction::Down),
                    },
                    Some(keycode),
                    None,
                )
            }
            KeymapAction::Hold { at } => (
                DeviceAction::TouchBegin {
                    point: self.screen.point(*at),
                },
                None,
                Some(()),
            ),
        };
        let handle = self.execute(&action).await?;
        let touch = if touch.is_some() {
            Some(handle.ok_or_else(|| {
                ExtensionError::Runtime("touch.begin 未返回 TouchHandle".to_string())
            })?)
        } else {
            None
        };
        if raw_keycode.is_some() || touch.is_some() {
            self.state.lock().await.active.insert(
                selector,
                ActiveBinding {
                    action: binding.action,
                    touch,
                    raw_keycode,
                },
            );
        }
        Ok(InputResult::consume(vec![action]))
    }

    async fn handle_release(&self, selector: String) -> ExtensionResult<InputResult> {
        {
            let mut state = self.state.lock().await;
            if state.suppressed_keyups.remove(&selector) {
                return Ok(InputResult::consume(Vec::new()));
            }
        }
        let Some(active) = self.state.lock().await.active.remove(&selector) else {
            return Ok(if self.binding(&selector).await.is_some() {
                InputResult::consume(Vec::new())
            } else {
                InputResult::pass()
            });
        };
        let Some(action) = active
            .touch
            .map(|touch| DeviceAction::TouchEnd { touch })
            .or_else(|| {
                active.raw_keycode.map(|keycode| DeviceAction::Key {
                    input: KeyInput::new(KeyCode::new(keycode), KeyAction::Up),
                })
            })
        else {
            return Ok(InputResult::consume(Vec::new()));
        };
        self.execute(&action).await?;
        Ok(InputResult::consume(vec![action]))
    }

    async fn handle_mouse_move(&self, x: u32, y: u32) -> ExtensionResult<InputResult> {
        let (mouse_touch, mapped_touches): (Option<_>, Vec<_>) = {
            let state = self.state.lock().await;
            let mapped = state
                .active
                .iter()
                .filter(|(selector, active)| {
                    selector.starts_with("Mouse") && active.touch.is_some()
                })
                .filter_map(|(_, active)| active.touch)
                .collect();
            (state.mouse_touch, mapped)
        };
        if mouse_touch.is_none() && mapped_touches.is_empty() {
            return Ok(InputResult::pass());
        }
        let point = TouchPoint::new(x, y, 1.0);
        let mut touches = mapped_touches;
        if let Some(touch) = mouse_touch {
            touches.insert(0, touch);
        }
        let mut actions = Vec::with_capacity(touches.len());
        for touch in touches {
            let action = DeviceAction::TouchMove { touch, point };
            self.execute(&action).await?;
            actions.push(action);
        }
        Ok(InputResult::consume(actions))
    }

    async fn handle_mouse_down(&self, x: u32, y: u32) -> ExtensionResult<InputResult> {
        if self.state.lock().await.mouse_touch.is_some() {
            return Ok(InputResult::consume(Vec::new()));
        }
        let action = DeviceAction::TouchBegin {
            point: TouchPoint::new(x, y, 1.0),
        };
        let touch = self.execute(&action).await?.ok_or_else(|| {
            ExtensionError::Runtime("mouse touch.begin 未返回 TouchHandle".to_string())
        })?;
        self.state.lock().await.mouse_touch = Some(touch);
        Ok(InputResult::consume(vec![action]))
    }

    async fn handle_mouse_up(&self) -> ExtensionResult<InputResult> {
        let Some(touch) = self.state.lock().await.mouse_touch.take() else {
            return Ok(InputResult::pass());
        };
        let action = DeviceAction::TouchEnd { touch };
        self.execute(&action).await?;
        Ok(InputResult::consume(vec![action]))
    }

    async fn handle_axis(&self, index: u8, value: f32) -> ExtensionResult<InputResult> {
        if !value.is_finite() || !(0..=7).contains(&index) {
            return Ok(InputResult::pass());
        }
        let selector = format!("GamepadAxis{index}");
        let active = self.state.lock().await.active.contains_key(&selector);
        let pressed = value.abs() >= 0.5;
        if pressed && !active {
            self.handle_press(selector, false).await
        } else if !pressed && active {
            self.handle_release(selector).await
        } else if self.binding(&selector).await.is_some() && pressed {
            Ok(InputResult::consume(Vec::new()))
        } else {
            Ok(InputResult::pass())
        }
    }

    async fn binding(&self, selector: &str) -> Option<KeymapBinding> {
        self.profile
            .read()
            .await
            .bindings
            .iter()
            .find(|binding| binding.key == selector)
            .cloned()
    }

    async fn execute(&self, action: &DeviceAction) -> ExtensionResult<Option<TouchHandle>> {
        self.executor.execute(&self.device, action).await
    }
}

/// Per-device input gateway.  It has no browser or WebRTC knowledge: the
/// existing transport adapter can call `dispatch` and forward `consume=false`
/// events to its legacy path.
pub struct InputGateway {
    runners: RwLock<HashMap<crate::capabilities::DeviceId, Arc<KeymapRunner>>>,
}

impl Default for InputGateway {
    fn default() -> Self {
        Self::new()
    }
}

impl InputGateway {
    pub fn new() -> Self {
        Self {
            runners: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register(&self, device: DeviceHandle, runner: Arc<KeymapRunner>) {
        self.runners
            .write()
            .await
            .insert(device.id().clone(), runner);
    }

    /// Install a native runner for an already-connected scrcpy session.  The
    /// session is captured by the transport adapter, while the runner keeps
    /// only the logical device handle and opaque touch state.
    pub async fn register_scrcpy(
        &self,
        device: DeviceHandle,
        profile: Keymap,
        screen: ScreenSize,
        session: Arc<ScrcpySession>,
    ) -> Arc<KeymapRunner> {
        let runner = Arc::new(KeymapRunner::new(
            profile,
            device.clone(),
            screen,
            Arc::new(ScrcpyDeviceActionExecutor::new(session)),
        ));
        self.register(device, runner.clone()).await;
        runner
    }

    pub async fn unregister(&self, device: &DeviceHandle) -> Option<Arc<KeymapRunner>> {
        self.runners.write().await.remove(device.id())
    }

    pub async fn dispatch(
        &self,
        device: &DeviceHandle,
        event: InputEvent,
    ) -> ExtensionResult<InputResult> {
        let runner = self.runners.read().await.get(device.id()).cloned();
        match runner {
            Some(runner) => runner.dispatch(event).await,
            None => Ok(InputResult::pass()),
        }
    }

    pub async fn dispatch_timed(
        &self,
        device: &DeviceHandle,
        event: InputEvent,
    ) -> ExtensionResult<(InputResult, Duration)> {
        let started = Instant::now();
        let result = self.dispatch(device, event).await?;
        Ok((result, started.elapsed()))
    }
}

/// Validate that a keymap extension manifest has the permissions required by
/// its profile before a runner is made visible to the gateway.
pub fn validate_manifest_for_keymap(
    manifest: &ExtensionManifest,
    profile: &Keymap,
    capabilities: CapabilityRegistry,
) -> ExtensionResult<()> {
    let host = HostApi::for_manifest(capabilities, HostApiCatalog::default(), manifest)?;
    if manifest.id().as_str() != KEYMAP_EXTENSION_ID {
        return Err(ExtensionError::InvalidManifest(format!(
            "keymap extension id 必须是 {KEYMAP_EXTENSION_ID}"
        )));
    }
    for permission in required_permissions(profile) {
        host.authorize(permission)?;
        if !host.domain_available(permission.domain()) {
            return Err(ExtensionError::Runtime(format!(
                "keymap 所需 capability 不可用: {}",
                permission.as_str()
            )));
        }
    }
    Ok(())
}

fn required_permissions(profile: &Keymap) -> Vec<Permission> {
    let mut permissions = HashSet::new();
    for binding in &profile.bindings {
        match &binding.action {
            KeymapAction::Tap { .. } => {
                permissions.insert(Permission::InputTap);
            }
            KeymapAction::Swipe { .. } => {
                permissions.insert(Permission::InputSwipe);
            }
            KeymapAction::RawKey { .. } => {
                permissions.insert(Permission::InputKey);
            }
            KeymapAction::Hold { .. } => {
                permissions.insert(Permission::Touch);
            }
        }
    }
    let mut result: Vec<_> = permissions.into_iter().collect();
    result.sort_by_key(|permission| permission.as_str());
    result
}

// Compatibility names for the first draft of the Phase 7 adapter.  Keeping
// these aliases local to the extension module lets callers migrate from the
// native runner terminology without adding another host-wide abstraction.
pub type AppPackageKeymapData = AppPackageKeymapSource;
pub type InputError = ExtensionError;
pub type KeymapProgram = Keymap;
pub type KeymapRuntime = KeymapRunner;
pub type KeymapRuntimeState = KeymapState;
pub type NativeKeymapRunner = KeymapRunner;

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

/// Human-readable status used by diagnostics and the feature-gated harness.
pub fn real_wasm_host_status() -> &'static str {
    "WIT Host imports are not executable until Phase 6 component bindings are wired"
}

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

fn format_diagnostics(diagnostics: &[crate::keymaps::KeymapDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("；")
}

fn raw_keycode(action: &KeymapAction) -> Option<u32> {
    let KeymapAction::RawKey { code, keycode } = action else {
        return None;
    };
    keycode.or_else(|| code.as_deref().and_then(android_keycode))
}

fn android_keycode(code: &str) -> Option<u32> {
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

/// The generic Phase 6 Host currently stops after module validation.  This
/// harness makes that boundary executable in CI without pretending that an
/// extension can already call WIT imports.  It validates the first keymap ABI
/// exports and returns a stable, actionable error from `invoke`.
#[cfg(all(feature = "wasm-runtime", feature = "keymap-wasm-harness"))]
pub struct KeymapWasmHarness {
    engine: wasmtime::Engine,
    module: wasmtime::Module,
}

#[cfg(all(feature = "wasm-runtime", feature = "keymap-wasm-harness"))]
impl KeymapWasmHarness {
    pub fn load(bytes: &[u8]) -> ExtensionResult<Self> {
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, bytes)
            .map_err(|error| ExtensionError::Runtime(format!("keymap WASM 无效: {error}")))?;
        for export in ["memory", "keymap_alloc", "keymap_handle_v1"] {
            if module.get_export(export).is_none() {
                return Err(ExtensionError::Runtime(format!(
                    "keymap WASM 缺少 ABI export: {export}"
                )));
            }
        }
        Ok(Self { engine, module })
    }

    pub fn abi_version(&self) -> &'static str {
        KEYMAP_WASM_ABI_VERSION
    }

    pub fn invoke(&self, _event: &InputEvent) -> ExtensionResult<InputResult> {
        let _ = (&self.engine, &self.module);
        Err(ExtensionError::RuntimeUnavailable(
            "Phase 6 Host 尚未提供 gamer:input/touch 的 WIT 实例调用；仅完成 ABI harness 校验",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::app_packages::AppPackageStore;
    use crate::capabilities::DeviceId;

    #[derive(Default)]
    struct RecordingExecutor {
        actions: StdMutex<Vec<DeviceAction>>,
    }

    #[async_trait]
    impl DeviceActionExecutor for RecordingExecutor {
        async fn execute(
            &self,
            _device: &DeviceHandle,
            action: &DeviceAction,
        ) -> ExtensionResult<Option<TouchHandle>> {
            self.actions.lock().unwrap().push(action.clone());
            Ok(match action {
                DeviceAction::TouchBegin { .. } => Some(TouchHandle::new()),
                _ => None,
            })
        }
    }

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

    fn runner(profile: Keymap) -> (Arc<KeymapRunner>, Arc<RecordingExecutor>, DeviceHandle) {
        let executor = Arc::new(RecordingExecutor::default());
        let device = DeviceHandle::new(DeviceId::new("device-1"));
        let runner = Arc::new(KeymapRunner::new(
            profile,
            device.clone(),
            ScreenSize::new(1000, 500),
            executor.clone(),
        ));
        (runner, executor, device)
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
        assert!(KEYMAP_EXTENSION_MANIFEST_TOML.contains("runtime = \"iframe\""));
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

    #[tokio::test]
    async fn gateway_returns_pass_or_consumed_actions_and_keeps_multi_key_handles() {
        let profile = keymap(&[
            ("KeyW", KeymapAction::Hold { at: [0.4, 0.6] }),
            ("KeyA", KeymapAction::Hold { at: [0.1, 0.2] }),
            ("Space", KeymapAction::Tap { at: [0.2, 0.3] }),
            (
                "GamepadButton0",
                KeymapAction::RawKey {
                    code: Some("KeyQ".to_string()),
                    keycode: None,
                },
            ),
        ]);
        let (runner, executor, device) = runner(profile);
        let gateway = InputGateway::new();
        gateway.register(device.clone(), runner.clone()).await;

        assert_eq!(
            gateway
                .dispatch(&device, InputEvent::key_down("KeyZ"))
                .await
                .unwrap(),
            InputResult::pass()
        );
        assert!(
            gateway
                .dispatch(&device, InputEvent::key_down("KeyW"))
                .await
                .unwrap()
                .consume
        );
        assert!(
            gateway
                .dispatch(&device, InputEvent::key_down("KeyA"))
                .await
                .unwrap()
                .consume
        );
        assert_eq!(runner.pressed_selectors().await, vec!["KeyA", "KeyW"]);
        let tap = gateway
            .dispatch(&device, InputEvent::key_down("Space"))
            .await
            .unwrap();
        assert!(tap.consume);
        assert!(matches!(tap.actions[0], DeviceAction::Tap { .. }));
        let gamepad = gateway
            .dispatch(
                &device,
                InputEvent::GamepadButton {
                    index: 0,
                    pressed: true,
                    value: 1.0,
                },
            )
            .await
            .unwrap();
        assert!(gamepad.consume);

        let release = runner.release_all().await.unwrap();
        assert_eq!(release.actions.len(), 3);
        assert!(executor
            .actions
            .lock()
            .unwrap()
            .iter()
            .any(|action| matches!(action, DeviceAction::TouchEnd { .. })));
        assert!(runner.pressed_selectors().await.is_empty());
    }

    #[tokio::test]
    async fn mouse_hold_uses_touch_handle_and_mouse_move_never_exposes_pointer_id() {
        let (runner, executor, device) = runner(keymap(&[(
            "MouseLeft",
            KeymapAction::Hold { at: [0.5, 0.5] },
        )]));
        let gateway = InputGateway::new();
        gateway.register(device.clone(), runner).await;
        gateway
            .dispatch(
                &device,
                InputEvent::MouseDown {
                    button: 0,
                    x: 10,
                    y: 20,
                },
            )
            .await
            .unwrap();
        let moved = gateway
            .dispatch(
                &device,
                InputEvent::MouseMove {
                    x: 400,
                    y: 300,
                    delta_x: 390,
                    delta_y: 280,
                },
            )
            .await
            .unwrap();
        assert!(matches!(moved.actions[0], DeviceAction::TouchMove { .. }));
        assert!(
            gateway
                .dispatch(
                    &device,
                    InputEvent::MouseUp {
                        button: 0,
                        x: 400,
                        y: 300,
                    },
                )
                .await
                .unwrap()
                .consume
        );
        assert!(executor
            .actions
            .lock()
            .unwrap()
            .iter()
            .all(|action| !format!("{action:?}").contains("pointer_id")));
    }

    #[tokio::test]
    async fn unknown_mouse_buttons_are_passed_to_the_legacy_input_path() {
        let (runner, _executor, device) = runner(keymap(&[]));
        let gateway = InputGateway::new();
        gateway.register(device.clone(), runner).await;
        assert_eq!(
            gateway
                .dispatch(
                    &device,
                    InputEvent::MouseDown {
                        button: 9,
                        x: 1,
                        y: 2,
                    },
                )
                .await
                .unwrap(),
            InputResult::pass()
        );
    }

    #[tokio::test]
    async fn profile_switch_releases_state_before_new_scheme() {
        let (runner, executor, _device) =
            runner(keymap(&[("KeyW", KeymapAction::Hold { at: [0.1, 0.1] })]));
        runner.dispatch(InputEvent::key_down("KeyW")).await.unwrap();
        runner
            .replace_profile(keymap(&[("KeyA", KeymapAction::Tap { at: [0.8, 0.8] })]))
            .await
            .unwrap();
        assert!(runner.pressed_selectors().await.is_empty());
        assert!(matches!(
            executor.actions.lock().unwrap().last(),
            Some(DeviceAction::TouchEnd { .. })
        ));
        let new_action = runner.dispatch(InputEvent::key_down("KeyA")).await.unwrap();
        assert!(new_action.consume);
        assert!(matches!(
            new_action.actions.first(),
            Some(DeviceAction::Tap { .. })
        ));
        assert!(
            !runner
                .dispatch(InputEvent::key_down("KeyW"))
                .await
                .unwrap()
                .consume
        );
    }

    #[tokio::test]
    async fn hot_path_dispatch_p95_is_bounded() {
        let (runner, _executor, device) =
            runner(keymap(&[("KeyA", KeymapAction::Tap { at: [0.2, 0.2] })]));
        let gateway = InputGateway::new();
        gateway.register(device.clone(), runner).await;
        let mut samples = Vec::with_capacity(128);
        for _ in 0..128 {
            let (_, elapsed) = gateway
                .dispatch_timed(&device, InputEvent::key_down("KeyZ"))
                .await
                .unwrap();
            samples.push(elapsed);
        }
        samples.sort_unstable();
        eprintln!(
            "keymap dispatch latency: p50={:?}, p95={:?}",
            samples[63], samples[121]
        );
        assert!(samples[63] < Duration::from_millis(20));
        assert!(samples[121] < Duration::from_millis(50));
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
        let manifest =
            b"id = \"official.game\"\nversion = \"1.0.0\"\n[android]\npackages = [\"com.game\"]\n";
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
        store.install_archive(&bytes).unwrap();
        let source = AppPackageKeymapSource::new(store);
        let loaded = source
            .load("com.game", "official.game", "1.0.0", "default.yaml")
            .unwrap();
        assert_eq!(loaded.name, "package");
        assert!(source
            .load("com.other", "official.game", "1.0.0", "default.yaml")
            .is_err());
    }

    #[cfg(all(feature = "wasm-runtime", feature = "keymap-wasm-harness"))]
    #[test]
    fn wasm_harness_loads_keymap_abi_but_fails_clearly_at_wit_boundary() {
        // Keep the fixture dependency-free: the harness is feature-gated and
        // must remain runnable even when no WAT toolchain is installed.
        let bytes: &[u8] = &[
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0c, 0x02, 0x60, 0x01, 0x7f,
            0x01, 0x7f, 0x60, 0x02, 0x7f, 0x7f, 0x01, 0x7e, 0x03, 0x03, 0x02, 0x00, 0x01, 0x05,
            0x03, 0x01, 0x00, 0x01, 0x07, 0x2c, 0x03, 0x06, 0x6d, 0x65, 0x6d, 0x6f, 0x72, 0x79,
            0x02, 0x00, 0x0c, 0x6b, 0x65, 0x79, 0x6d, 0x61, 0x70, 0x5f, 0x61, 0x6c, 0x6c, 0x6f,
            0x63, 0x00, 0x00, 0x10, 0x6b, 0x65, 0x79, 0x6d, 0x61, 0x70, 0x5f, 0x68, 0x61, 0x6e,
            0x64, 0x6c, 0x65, 0x5f, 0x76, 0x31, 0x00, 0x01, 0x0a, 0x0b, 0x02, 0x04, 0x00, 0x41,
            0x00, 0x0b, 0x04, 0x00, 0x42, 0x00, 0x0b,
        ];
        let harness = KeymapWasmHarness::load(bytes).unwrap();
        assert_eq!(harness.abi_version(), KEYMAP_WASM_ABI_VERSION);
        assert!(matches!(
            harness.invoke(&InputEvent::key_down("KeyA")),
            Err(ExtensionError::RuntimeUnavailable(_))
        ));
    }
}
