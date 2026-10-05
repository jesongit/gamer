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
    stamps: HashMap<FrameHandle, super::super::FrameStamp>,
}

const MAX_STORED_FRAMES: usize = 32;

impl FrameStore {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(FrameState {
                frames: HashMap::new(),
                order: VecDeque::new(),
                stamps: HashMap::new(),
            }),
        }
    }

    pub(crate) fn insert(
        &self,
        frame: crate::matcher::DecodedFrame,
    ) -> CapabilityResult<FrameHandle> {
        self.insert_stamped(frame, None)
    }
    fn insert_stamped(
        &self,
        frame: crate::matcher::DecodedFrame,
        stamp: Option<super::super::FrameStamp>,
    ) -> CapabilityResult<FrameHandle> {
        let handle = FrameHandle::new();
        let mut state = self
            .state
            .lock()
            .map_err(|_| CapabilityError::Failed("frame state poisoned".into()))?;
        if state.frames.len() >= MAX_STORED_FRAMES {
            if let Some(oldest) = state.order.pop_front() {
                state.frames.remove(&oldest);
                state.stamps.remove(&oldest);
            }
        }
        state.order.push_back(handle);
        state.frames.insert(handle, Arc::new(frame));
        if let Some(stamp) = stamp {
            state.stamps.insert(handle, stamp);
        }
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
    async fn coordinate_space(
        &self,
        device: &DeviceHandle,
    ) -> CapabilityResult<(FrameSize, Option<super::super::FrameStamp>)> {
        if crate::targets::is_browser(device.id().as_str()) {
            let s = self
                .devices
                .browsers
                .session(device.id().as_str())
                .map_err(|e| CapabilityError::Failed(e.to_string()))?;
            return s
                .coordinate_space()
                .await
                .map(|(size, stamp)| (size, Some(stamp)))
                .map_err(|e| CapabilityError::Failed(e.to_string()));
        }
        let session = self
            .devices
            .session(device.id().as_str())
            .ok_or_else(|| CapabilityError::NotFound("device session".into()))?;
        let (size, stamp) = session.coordinate_space();
        session
            .validate_frame(&stamp)
            .map_err(|e| CapabilityError::Failed(e.to_string()))?;
        if size.width == 0 || size.height == 0 {
            return Err(CapabilityError::Failed("设备画面尺寸尚未就绪".into()));
        }
        Ok((size, Some(stamp)))
    }
    async fn stamp(
        &self,
        frame: FrameHandle,
    ) -> CapabilityResult<Option<super::super::FrameStamp>> {
        let state = self
            .store
            .state
            .lock()
            .map_err(|_| CapabilityError::Failed("frame state poisoned".into()))?;
        if !state.frames.contains_key(&frame) {
            return Err(CapabilityError::NotFound("frame handle".into()));
        }
        Ok(state.stamps.get(&frame).cloned())
    }

    async fn device_size(&self, device: &DeviceHandle) -> CapabilityResult<FrameSize> {
        if crate::targets::is_browser(device.id().as_str()) {
            let session = self
                .devices
                .browsers
                .session(device.id().as_str())
                .map_err(|e| CapabilityError::Failed(e.to_string()))?;
            let (w, h) = session.size();
            return Ok(FrameSize::new(w, h));
        }
        let session = self
            .devices
            .session(device.id().as_str())
            .ok_or_else(|| CapabilityError::NotFound("device session".into()))?;
        let (width, height) = session.video_size();
        if width == 0 || height == 0 {
            return Err(CapabilityError::Failed("设备画面尺寸尚未就绪".into()));
        }
        Ok(FrameSize::new(width, height))
    }

    async fn latest(&self, device: &DeviceHandle) -> CapabilityResult<Option<FrameHandle>> {
        self.capture(device).await.map(Some)
    }

    /// 截图直连 FrameCache 按需解码路径（`DeviceManager::screenshot_frame`）：
    /// 拿到的是已解码 RGB 帧（Arc 共享），注册进 FrameStore 返回 handle——
    /// 不再有「ffmpeg 出 PNG → Rust 解 PNG」的往返；PNG 编码只保留在 HTTP
    /// 截图边界（`DeviceManager::screenshot` / `decode_latest_png`）。帧缓存
    /// 不可用时由 DeviceManager 回退 adb 截图（该边界本身产出 PNG）。
    async fn capture(&self, device: &DeviceHandle) -> CapabilityResult<FrameHandle> {
        if crate::targets::is_browser(device.id().as_str()) {
            let session = self
                .devices
                .browsers
                .session(device.id().as_str())
                .map_err(|e| CapabilityError::Failed(e.to_string()))?;
            let (bytes, stamp) = session
                .capture()
                .await
                .map_err(|e| CapabilityError::Failed(e.to_string()))?;
            let frame = crate::matcher::compute::run(move || {
                crate::matcher::DecodedFrame::from_png(&bytes)
            })
            .await
            .and_then(|result| result)
            .map_err(|e| CapabilityError::Failed(e.to_string()))?;
            return self.store.insert_stamped(frame, Some(stamp));
        }
        let (size, expected) = self.coordinate_space(device).await?;
        let frame = self
            .devices
            .screenshot_frame(device.id().as_str())
            .await
            .map_err(|error| CapabilityError::Failed(error.to_string()))?;
        let (current_size, current_stamp) = self.coordinate_space(device).await?;
        if current_size != size
            || current_stamp != expected
            || frame.dimensions() != (size.width, size.height)
        {
            return Err(CapabilityError::Failed(
                "stale_frame: 截图期间设备连接或画面尺寸已改变，请重新截图".into(),
            ));
        }
        self.store.insert_stamped(frame, expected)
    }

    async fn size(&self, frame: FrameHandle) -> CapabilityResult<FrameSize> {
        let frame = self.store.get(frame)?;
        let (width, height) = frame.dimensions();
        Ok(FrameSize::new(width, height))
    }
}
