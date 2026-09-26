use async_trait::async_trait;
use std::time::Duration;

use super::{CapabilityResult, DeviceHandle, TouchPoint};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyAction {
    Down,
    Up,
    Press,
}

/// Backend-neutral key identifier. Its numeric value is not a scrcpy pointer ID.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct KeyCode(u32);

impl KeyCode {
    pub fn new(value: u32) -> Self {
        Self(value)
    }

    pub fn value(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyInput {
    code: KeyCode,
    action: KeyAction,
}

impl KeyInput {
    pub fn new(code: KeyCode, action: KeyAction) -> Self {
        Self { code, action }
    }

    pub fn code(self) -> KeyCode {
        self.code
    }

    pub fn action(self) -> KeyAction {
        self.action
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextInput(String);

impl TextInput {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwipeGesture {
    start: TouchPoint,
    end: TouchPoint,
    duration: Duration,
}

impl SwipeGesture {
    pub const fn new(start: TouchPoint, end: TouchPoint, duration: Duration) -> Self {
        Self {
            start,
            end,
            duration,
        }
    }

    pub fn start(self) -> TouchPoint {
        self.start
    }

    pub fn end(self) -> TouchPoint {
        self.end
    }

    pub fn duration(self) -> Duration {
        self.duration
    }
}

/// Keyboard and text input boundary.
#[async_trait]
pub trait InputService: Send + Sync {
    async fn tap_from_frame(
        &self,
        device: &DeviceHandle,
        point: TouchPoint,
        stamp: Option<&super::FrameStamp>,
    ) -> CapabilityResult<()> {
        if stamp.is_some() {
            return Err(super::CapabilityError::InvalidRequest(
                "此输入来源不支持该画面坐标".into(),
            ));
        }
        self.tap(device, point).await
    }
    async fn key_named(
        &self,
        device: &DeviceHandle,
        name: &str,
        action: KeyAction,
    ) -> CapabilityResult<()> {
        let code = android_key_code(name)?;
        self.key(device, KeyInput::new(KeyCode::new(code), action))
            .await
    }

    async fn tap(&self, device: &DeviceHandle, point: TouchPoint) -> CapabilityResult<()>;

    async fn swipe(&self, device: &DeviceHandle, gesture: SwipeGesture) -> CapabilityResult<()>;
    async fn swipe_from_frame(
        &self,
        device: &DeviceHandle,
        gesture: SwipeGesture,
        stamp: Option<&super::FrameStamp>,
    ) -> CapabilityResult<()> {
        if stamp.is_some() {
            return Err(super::CapabilityError::InvalidRequest(
                "此输入来源不支持该画面坐标".into(),
            ));
        }
        self.swipe(device, gesture).await
    }

    async fn key(&self, device: &DeviceHandle, input: KeyInput) -> CapabilityResult<()>;

    async fn text(&self, device: &DeviceHandle, input: TextInput) -> CapabilityResult<()>;
}

// Existing Android key vocabulary. Browser implementations override key_named.
pub(super) fn android_key_code(text: &str) -> CapabilityResult<u32> {
    if let Ok(code) = text.parse::<u32>() {
        return Ok(code);
    }
    Ok(match text.to_ascii_uppercase().as_str() {
        "HOME" => 3,
        "BACK" => 4,
        "MENU" => 82,
        "APP_SWITCH" | "RECENTS" => 187,
        "VOL_UP" | "VOLUME_UP" => 24,
        "VOL_DOWN" | "VOLUME_DOWN" => 25,
        "ESC" | "ESCAPE" => 111,
        "ENTER" | "RETURN" => 66,
        "SPACE" => 62,
        "TAB" => 61,
        "BACKSPACE" | "DEL" => 67,
        other => {
            return Err(super::CapabilityError::InvalidRequest(format!(
                "不支持的 Android key: {other}"
            )))
        }
    })
}
