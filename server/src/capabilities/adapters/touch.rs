use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::device::scrcpy::{ACTION_DOWN, ACTION_MOVE, ACTION_UP};

use super::super::{
    CapabilityError, CapabilityResult, DeviceHandle, TouchHandle, TouchPoint, TouchService,
};
use super::DeviceAdapter;

struct ActiveTouch {
    device: DeviceHandle,
    pointer_id: u64,
    point: TouchPoint,
    identity: crate::core::control::ControlIdentity,
    android_session: Option<Arc<crate::device::scrcpy::ScrcpySession>>,
}

// Capability adapters may be recreated while a browser session survives.
// A late release from an old adapter must never match a new contact.
static NEXT_POINTER_ID: AtomicU64 = AtomicU64::new(1);

/// Maps opaque capability touch handles to scrcpy pointer IDs. The latter never
/// crosses the capability boundary.
pub(crate) struct TouchAdapter {
    device: Arc<DeviceAdapter>,
    active: Mutex<HashMap<TouchHandle, ActiveTouch>>,
}

impl TouchAdapter {
    pub(crate) fn new(device: Arc<DeviceAdapter>) -> Self {
        Self {
            device,
            active: Mutex::new(HashMap::new()),
        }
    }

    fn active(&self, touch: &TouchHandle) -> CapabilityResult<ActiveTouch> {
        self.active
            .lock()
            .map_err(|_| CapabilityError::Failed("touch state poisoned".into()))?
            .get(touch)
            .map(|state| ActiveTouch {
                device: state.device.clone(),
                pointer_id: state.pointer_id,
                point: state.point,
                identity: state.identity.clone(),
                android_session: state.android_session.clone(),
            })
            .ok_or_else(|| CapabilityError::NotFound("touch handle".into()))
    }

    async fn inject(
        &self,
        state: &ActiveTouch,
        action: u8,
        point: TouchPoint,
    ) -> CapabilityResult<()> {
        let permit = self
            .device
            .devices
            .controls
            .admit_identity(state.device.id().as_str(), &state.identity)
            .await
            .map_err(|e| CapabilityError::Failed(e.to_string()))?;
        permit
            .scope(self.inject_admitted(state, action, point))
            .await
    }

    async fn inject_admitted(
        &self,
        state: &ActiveTouch,
        action: u8,
        point: TouchPoint,
    ) -> CapabilityResult<()> {
        if crate::targets::is_browser(state.device.id().as_str()) {
            let session = self
                .device
                .devices
                .browsers
                .session(state.device.id().as_str())
                .map_err(|e| CapabilityError::Failed(e.to_string()))?;
            if action != ACTION_UP {
                if let Some(expected) = state.device.expected_frame() {
                    session
                        .validate(expected)
                        .map_err(|e| CapabilityError::Failed(e.to_string()))?;
                }
            }
            let action = match action {
                ACTION_DOWN => "down",
                ACTION_UP => "up",
                _ => "move",
            };
            return super::with_capability_input_source(session.mapped_contact(
                state.pointer_id,
                action,
                point.x(),
                point.y(),
                state.device.manual_frame(),
            ))
            .await
            .map_err(|e| CapabilityError::Failed(e.to_string()));
        }
        // 录制输入观察（合同 §2.1）：能力层触控经此进入设备发送路径；
        // source 标注调用方 scope 优先（keymap/runner），缺省 "plugin"
        // （扩展能力输入的历史缺省语义），精确归属由调用方
        // `with_caller_input_source` 声明。
        super::with_capability_input_source(async {
            let session = state
                .android_session
                .as_ref()
                .ok_or_else(|| CapabilityError::Failed("touch session unavailable".into()))?;
            if action != ACTION_UP {
                let current = self.device.session(&state.device)?;
                if !Arc::ptr_eq(&current, session) {
                    return Err(CapabilityError::Failed(
                        "stale_frame: 原触点的设备连接已结束".into(),
                    ));
                }
            }
            match session
                .inject_touch(
                    action,
                    state.pointer_id,
                    point.x() as f32,
                    point.y() as f32,
                    point.pressure(),
                )
                .await
            {
                Ok(()) => Ok(()),
                Err(error) => Err(CapabilityError::Failed(error.to_string())),
            }
        })
        .await
    }
}

#[async_trait]
impl TouchService for TouchAdapter {
    async fn begin(
        &self,
        device: &DeviceHandle,
        point: TouchPoint,
    ) -> CapabilityResult<TouchHandle> {
        let permit = self.device.admit(device).await?;
        permit
            .scope(async {
                let android_session = if crate::targets::is_browser(device.id().as_str()) {
                    None
                } else {
                    Some(self.device.session(device)?)
                };
                let state = ActiveTouch {
                    device: device.clone(),
                    pointer_id: NEXT_POINTER_ID.fetch_add(1, Ordering::Relaxed),
                    point,
                    identity: self.device.devices.controls.identity(device.id().as_str()),
                    android_session,
                };
                self.inject(&state, ACTION_DOWN, point).await?;
                let handle = TouchHandle::new();
                self.active
                    .lock()
                    .map_err(|_| CapabilityError::Failed("touch state poisoned".into()))?
                    .insert(handle, state);
                Ok(handle)
            })
            .await
    }

    async fn move_touch(&self, touch: &TouchHandle, point: TouchPoint) -> CapabilityResult<()> {
        let state = self.active(touch)?;
        self.inject(&state, ACTION_MOVE, point).await?;
        self.active
            .lock()
            .map_err(|_| CapabilityError::Failed("touch state poisoned".into()))?
            .get_mut(touch)
            .ok_or_else(|| CapabilityError::NotFound("touch handle".into()))?
            .point = point;
        Ok(())
    }

    async fn end(&self, touch: &TouchHandle) -> CapabilityResult<()> {
        let state = self.active(touch)?;
        let result = self
            .inject(
                &state,
                ACTION_UP,
                TouchPoint::new(state.point.x(), state.point.y(), 0.0),
            )
            .await;
        self.active
            .lock()
            .map_err(|_| CapabilityError::Failed("touch state poisoned".into()))?
            .remove(touch);
        result
    }
}
