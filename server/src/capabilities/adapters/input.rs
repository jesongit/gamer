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
    async fn tap_from_frame(
        &self,
        device: &DeviceHandle,
        point: TouchPoint,
        stamp: Option<&super::super::FrameStamp>,
    ) -> CapabilityResult<()> {
        if crate::targets::is_browser(device.id().as_str()) {
            let session = self
                .device
                .devices
                .browsers
                .session(device.id().as_str())
                .map_err(|e| CapabilityError::Failed(e.to_string()))?;
            return session
                .input(
                    &serde_json::json!({"type":"tap","x":point.x(),"y":point.y()}),
                    stamp,
                )
                .await
                .map_err(|e| CapabilityError::Failed(e.to_string()));
        }
        if stamp.is_some() {
            return Err(CapabilityError::InvalidRequest(
                "画面目标与输入目标不一致".into(),
            ));
        }
        self.tap_touch(device, point).await
    }
    async fn key_named(
        &self,
        device: &DeviceHandle,
        name: &str,
        action: KeyAction,
    ) -> CapabilityResult<()> {
        if crate::targets::is_browser(device.id().as_str()) {
            let session = self
                .device
                .devices
                .browsers
                .session(device.id().as_str())
                .map_err(|e| CapabilityError::Failed(e.to_string()))?;
            let action = match action {
                KeyAction::Down => "down",
                KeyAction::Up => "up",
                KeyAction::Press => "press",
            };
            return session
                .input(
                    &serde_json::json!({"type":"key","key":name,"action":action}),
                    None,
                )
                .await
                .map_err(|e| CapabilityError::Failed(e.to_string()));
        }
        let code = super::super::input::android_key_code(name)?;
        self.key(
            device,
            KeyInput::new(super::super::KeyCode::new(code), action),
        )
        .await
    }

    async fn tap(&self, device: &DeviceHandle, point: TouchPoint) -> CapabilityResult<()> {
        self.tap_from_frame(device, point, None).await
    }

    async fn swipe(&self, device: &DeviceHandle, gesture: SwipeGesture) -> CapabilityResult<()> {
        self.swipe_from_frame(device, gesture, None).await
    }
    async fn swipe_from_frame(
        &self,
        device: &DeviceHandle,
        gesture: SwipeGesture,
        expected: Option<&super::super::FrameStamp>,
    ) -> CapabilityResult<()> {
        if crate::targets::is_browser(device.id().as_str()) {
            let session = self
                .device
                .devices
                .browsers
                .session(device.id().as_str())
                .map_err(|e| CapabilityError::Failed(e.to_string()))?;
            let stamp = expected.cloned().unwrap_or_else(|| session.stamp());
            let result=async {
                session.input(&serde_json::json!({"type":"pointer","action":"down","x":gesture.start().x(),"y":gesture.start().y()}),Some(&stamp)).await?;
                for i in 1..=20 {
                    let t=i as f64/20.0;
                    let x=gesture.start().x() as f64+(gesture.end().x() as f64-gesture.start().x() as f64)*t;
                    let y=gesture.start().y() as f64+(gesture.end().y() as f64-gesture.start().y() as f64)*t;
                    session.input(&serde_json::json!({"type":"pointer","action":"move","x":x,"y":y}),Some(&stamp)).await?;
                    tokio::time::sleep(gesture.duration()/20).await;
                }
                session.input(&serde_json::json!({"type":"pointer","action":"up","x":gesture.end().x(),"y":gesture.end().y()}),Some(&stamp)).await?;
                Ok::<_,anyhow::Error>(())
            }.await;
            if result.is_err() {
                session.release_inputs().await;
            }
            return result.map_err(|e| CapabilityError::Failed(e.to_string()));
        }

        if expected.is_some() {
            return Err(CapabilityError::InvalidRequest(
                "画面目标与输入目标不一致".into(),
            ));
        }
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
        // 录制输入观察（合同 §2.1）：能力层按键的 source 标注——调用方
        // scope 优先（keymap/runner），缺省 "plugin"；观察本体在 scrcpy
        // 注入原语内。
        let result = super::with_capability_input_source(async {
            match input.action() {
                KeyAction::Down => session.inject_keycode(0, input.code().value(), 0, 0).await,
                KeyAction::Up => session.inject_keycode(1, input.code().value(), 0, 0).await,
                KeyAction::Press => session.press_key(input.code().value()).await,
            }
        })
        .await;
        result.map_err(|error| CapabilityError::Failed(error.to_string()))
    }

    async fn text(&self, device: &DeviceHandle, input: TextInput) -> CapabilityResult<()> {
        if crate::targets::is_browser(device.id().as_str()) {
            let session = self
                .device
                .devices
                .browsers
                .session(device.id().as_str())
                .map_err(|e| CapabilityError::Failed(e.to_string()))?;
            return session
                .input(
                    &serde_json::json!({"type":"text","text":input.as_str()}),
                    None,
                )
                .await
                .map_err(|e| CapabilityError::Failed(e.to_string()));
        }

        let session = self.device.session(device)?;
        let result = super::with_capability_input_source(async {
            session.inject_text(input.as_str()).await
        })
        .await;
        result.map_err(|error| CapabilityError::Failed(error.to_string()))
    }
}
