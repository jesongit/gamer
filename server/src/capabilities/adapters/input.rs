use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::super::{
    CapabilityError, CapabilityResult, DeviceHandle, InputService, KeyAction, KeyInput,
    SwipeGesture, TextInput, TouchPoint, TouchService,
};
use super::{DeviceAdapter, TouchAdapter};

/// Standard input adapter. Pointer allocation and touch lifecycle are delegated
/// to TouchAdapter; key/text remain direct scrcpy control operations.
pub(crate) struct InputAdapter {
    device: Arc<DeviceAdapter>,
    touch: Arc<TouchAdapter>,
}

impl InputAdapter {
    pub(crate) fn new(device: Arc<DeviceAdapter>, touch: Arc<TouchAdapter>) -> Self {
        Self { device, touch }
    }

    async fn tap_touch(&self, device: &DeviceHandle, point: TouchPoint) -> CapabilityResult<()> {
        let touch = self.touch.begin(device, point).await?;
        tokio::time::sleep(Duration::from_millis(60)).await;
        self.touch.end(&touch).await
    }
}

#[async_trait]
impl InputService for InputAdapter {
    async fn tap(&self, device: &DeviceHandle, point: TouchPoint) -> CapabilityResult<()> {
        self.tap_touch(device, point).await
    }

    async fn swipe(&self, device: &DeviceHandle, gesture: SwipeGesture) -> CapabilityResult<()> {
        let touch = self.touch.begin(device, gesture.start()).await?;
        let result = async {
            for i in 1..=20u64 {
                let t = i as f32 / 20.0;
                let start = gesture.start();
                let end = gesture.end();
                let point = TouchPoint::new(
                    (start.x() as f32 + (end.x() as f32 - start.x() as f32) * t) as u32,
                    (start.y() as f32 + (end.y() as f32 - start.y() as f32) * t) as u32,
                    1.0,
                );
                self.touch.move_touch(&touch, point).await?;
                tokio::time::sleep(gesture.duration() / 20).await;
            }
            Ok::<_, CapabilityError>(())
        }
        .await;
        let end = self.touch.end(&touch).await;
        result.and(end)
    }

    async fn key(&self, device: &DeviceHandle, input: KeyInput) -> CapabilityResult<()> {
        let session = self.device.session(device)?;
        let result = match input.action() {
            KeyAction::Down => session.inject_keycode(0, input.code().value(), 0, 0).await,
            KeyAction::Up => session.inject_keycode(1, input.code().value(), 0, 0).await,
            KeyAction::Press => session.press_key(input.code().value()).await,
        };
        result.map_err(|error| CapabilityError::Failed(error.to_string()))
    }

    async fn text(&self, device: &DeviceHandle, input: TextInput) -> CapabilityResult<()> {
        self.device
            .session(device)?
            .inject_text(input.as_str())
            .await
            .map_err(|error| CapabilityError::Failed(error.to_string()))
    }
}
