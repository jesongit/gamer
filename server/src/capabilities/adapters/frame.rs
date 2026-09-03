use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::device::DeviceManager;

use super::super::{
    CapabilityError, CapabilityResult, DeviceHandle, FrameHandle, FrameService, FrameSize,
};

/// Short-lived decoded frame table. Handles make cross-layer ownership explicit
/// while the decoded pixels remain shared and backend-private.
pub(crate) struct FrameStore {
    state: Mutex<FrameState>,
}

struct FrameState {
    frames: HashMap<FrameHandle, Arc<crate::matcher::DecodedFrame>>,
    order: VecDeque<FrameHandle>,
}

const MAX_STORED_FRAMES: usize = 32;

impl FrameStore {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(FrameState {
                frames: HashMap::new(),
                order: VecDeque::new(),
            }),
        }
    }

    pub(crate) fn insert(
        &self,
        frame: crate::matcher::DecodedFrame,
    ) -> CapabilityResult<FrameHandle> {
        let handle = FrameHandle::new();
        let mut state = self
            .state
            .lock()
            .map_err(|_| CapabilityError::Failed("frame state poisoned".into()))?;
        if state.frames.len() >= MAX_STORED_FRAMES {
            if let Some(oldest) = state.order.pop_front() {
                state.frames.remove(&oldest);
            }
        }
        state.order.push_back(handle);
        state.frames.insert(handle, Arc::new(frame));
        Ok(handle)
    }

    pub(crate) fn get(
        &self,
        handle: FrameHandle,
    ) -> CapabilityResult<Arc<crate::matcher::DecodedFrame>> {
        self.state
            .lock()
            .map_err(|_| CapabilityError::Failed("frame state poisoned".into()))?
            .frames
            .get(&handle)
            .cloned()
            .ok_or_else(|| CapabilityError::NotFound("frame handle".into()))
    }
}

pub(crate) struct FrameAdapter {
    devices: Arc<DeviceManager>,
    pub(crate) store: Arc<FrameStore>,
}

impl FrameAdapter {
    pub(crate) fn new(devices: Arc<DeviceManager>, store: Arc<FrameStore>) -> Self {
        Self { devices, store }
    }

    pub(crate) async fn import_png(&self, png: Vec<u8>) -> CapabilityResult<FrameHandle> {
        let frame =
            crate::matcher::compute::run(move || crate::matcher::DecodedFrame::from_png(&png))
                .await
                .map_err(|error| CapabilityError::Failed(error.to_string()))?
                .map_err(|error| CapabilityError::Failed(error.to_string()))?;
        self.store.insert(frame)
    }

    async fn capture_png(&self, device: &DeviceHandle) -> CapabilityResult<Vec<u8>> {
        self.devices
            .screenshot(device.id().as_str())
            .await
            .map_err(|error| CapabilityError::Failed(error.to_string()))
    }
}

#[async_trait]
impl FrameService for FrameAdapter {
    async fn latest(&self, device: &DeviceHandle) -> CapabilityResult<Option<FrameHandle>> {
        self.capture(device).await.map(Some)
    }

    async fn capture(&self, device: &DeviceHandle) -> CapabilityResult<FrameHandle> {
        // TODO(phase4): expose a decoded FrameCache snapshot so this adapter can
        // consume the H264/GOP path directly without changing the existing PNG
        // screenshot hot path. The handle boundary is already stable today.
        let png = self.capture_png(device).await?;
        self.import_png(png).await
    }

    async fn size(&self, frame: FrameHandle) -> CapabilityResult<FrameSize> {
        let frame = self.store.get(frame)?;
        let (width, height) = frame.dimensions();
        Ok(FrameSize::new(width, height))
    }
}
