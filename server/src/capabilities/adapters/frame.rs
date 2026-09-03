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
}

#[async_trait]
impl FrameService for FrameAdapter {
    async fn latest(&self, device: &DeviceHandle) -> CapabilityResult<Option<FrameHandle>> {
        self.capture(device).await.map(Some)
    }

    /// 截图直连 FrameCache 按需解码路径（`DeviceManager::screenshot_frame`）：
    /// 拿到的是已解码 RGB 帧（Arc 共享），注册进 FrameStore 返回 handle——
    /// 不再有「ffmpeg 出 PNG → Rust 解 PNG」的往返；PNG 编码只保留在 HTTP
    /// 截图边界（`DeviceManager::screenshot` / `decode_latest_png`）。帧缓存
    /// 不可用时由 DeviceManager 回退 adb 截图（该边界本身产出 PNG）。
    async fn capture(&self, device: &DeviceHandle) -> CapabilityResult<FrameHandle> {
        let frame = self
            .devices
            .screenshot_frame(device.id().as_str())
            .await
            .map_err(|error| CapabilityError::Failed(error.to_string()))?;
        self.store.insert(frame)
    }

    async fn size(&self, frame: FrameHandle) -> CapabilityResult<FrameSize> {
        let frame = self.store.get(frame)?;
        let (width, height) = frame.dimensions();
        Ok(FrameSize::new(width, height))
    }
}
