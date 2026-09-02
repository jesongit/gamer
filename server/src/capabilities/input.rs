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
    async fn tap(&self, device: &DeviceHandle, point: TouchPoint) -> CapabilityResult<()>;

    async fn swipe(&self, device: &DeviceHandle, gesture: SwipeGesture) -> CapabilityResult<()>;

    async fn key(&self, device: &DeviceHandle, input: KeyInput) -> CapabilityResult<()>;

    async fn text(&self, device: &DeviceHandle, input: TextInput) -> CapabilityResult<()>;
}
