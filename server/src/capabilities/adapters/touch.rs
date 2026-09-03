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
}

/// Maps opaque capability touch handles to scrcpy pointer IDs. The latter never
/// crosses the capability boundary.
pub(crate) struct TouchAdapter {
    device: Arc<DeviceAdapter>,
    active: Mutex<HashMap<TouchHandle, ActiveTouch>>,
    next_pointer_id: AtomicU64,
}

impl TouchAdapter {
    pub(crate) fn new(device: Arc<DeviceAdapter>) -> Self {
        Self {
            device,
            active: Mutex::new(HashMap::new()),
            next_pointer_id: AtomicU64::new(1),
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
            })
            .ok_or_else(|| CapabilityError::NotFound("touch handle".into()))
    }

    async fn inject(
        &self,
        state: &ActiveTouch,
        action: u8,
        point: TouchPoint,
    ) -> CapabilityResult<()> {
        self.device
            .session(&state.device)?
            .inject_touch(
                action,
                state.pointer_id,
                point.x() as f32,
                point.y() as f32,
                point.pressure(),
            )
            .await
            .map_err(|error| CapabilityError::Failed(error.to_string()))
    }
}

#[async_trait]
impl TouchService for TouchAdapter {
    async fn begin(
        &self,
        device: &DeviceHandle,
        point: TouchPoint,
    ) -> CapabilityResult<TouchHandle> {
        let state = ActiveTouch {
            device: device.clone(),
            pointer_id: self.next_pointer_id.fetch_add(1, Ordering::Relaxed),
            point,
        };
        self.inject(&state, ACTION_DOWN, point).await?;
        let handle = TouchHandle::new();
        self.active
            .lock()
            .map_err(|_| CapabilityError::Failed("touch state poisoned".into()))?
            .insert(handle, state);
        Ok(handle)
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
        self.inject(
            &state,
            ACTION_UP,
            TouchPoint::new(state.point.x(), state.point.y(), 0.0),
        )
        .await?;
        self.active
            .lock()
            .map_err(|_| CapabilityError::Failed("touch state poisoned".into()))?
            .remove(touch);
        Ok(())
    }
}
