use async_trait::async_trait;
use std::time::Duration;

use super::{CapabilityResult, DeviceHandle, TouchPoint};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyAction {
    Down,
    Up,
    Press,
}

/// Android-specific numeric key identifier. Portable callers use key_named.
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
    if let Some(code) = android_keycode(text) {
        return Ok(code);
    }
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

pub fn android_keycode(code: &str) -> Option<u32> {
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
