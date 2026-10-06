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

type FrameMatchEvidence = (serde_json::Value, Option<Arc<Vec<u8>>>);

struct FrameState {
    frames: HashMap<FrameHandle, Arc<crate::matcher::DecodedFrame>>,
    order: VecDeque<FrameHandle>,
    stamps: HashMap<FrameHandle, super::super::FrameStamp>,
    times: HashMap<FrameHandle, (String, &'static str)>,
    matches: HashMap<FrameHandle, FrameMatchEvidence>,
}

const MAX_STORED_FRAMES: usize = 32;

impl FrameStore {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(FrameState {
                frames: HashMap::new(),
                order: VecDeque::new(),
                stamps: HashMap::new(),
                times: HashMap::new(),
                matches: HashMap::new(),
            }),
        }
    }

    pub(crate) fn insert(
        &self,
        frame: crate::matcher::DecodedFrame,
    ) -> CapabilityResult<FrameHandle> {
        self.insert_stamped(frame, None, chrono::Utc::now().to_rfc3339(), "synthetic")
    }
    fn insert_stamped(
        &self,
        frame: crate::matcher::DecodedFrame,
        stamp: Option<super::super::FrameStamp>,
        captured_at: String,
        time_basis: &'static str,
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
                state.times.remove(&oldest);
                state.matches.remove(&oldest);
            }
        }
        state.times.insert(handle, (captured_at, time_basis));
        state.order.push_back(handle);
        state.frames.insert(handle, Arc::new(frame));
        if let Some(stamp) = stamp {
            state.stamps.insert(handle, stamp);
        }
        Ok(handle)
    }

    pub(crate) fn record_match(
        &self,
        frame: FrameHandle,
        metadata: serde_json::Value,
        template: Option<Vec<u8>>,
    ) {
        if let Ok(mut state) = self.state.lock() {
            if state.frames.contains_key(&frame) {
                state
                    .matches
                    .insert(frame, (metadata, template.map(Arc::new)));
            }
        }
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
    trace: Option<Arc<crate::runtime_trace::TraceStore>>,
}

impl FrameAdapter {
    pub(crate) fn with_trace(mut self, trace: Arc<crate::runtime_trace::TraceStore>) -> Self {
        self.trace = Some(trace);
        self
    }

    pub(crate) fn new(devices: Arc<DeviceManager>, store: Arc<FrameStore>) -> Self {
        Self {
            devices,
            store,
            trace: None,
        }
    }
}

#[async_trait]
impl FrameService for FrameAdapter {
    async fn trace_snapshot(
        &self,
        run_id: String,
        snapshot: serde_json::Value,
    ) -> CapabilityResult<()> {
        if let Some(trace) = &self.trace {
            trace
                .snapshot(&run_id, snapshot)
                .map_err(|e| CapabilityError::Failed(e.to_string()))?;
        }
        Ok(())
    }
    async fn trace_frame(
        &self,
        frame: FrameHandle,
        mut metadata: serde_json::Value,
        forced: bool,
    ) -> CapabilityResult<Option<String>> {
        let Some(trace) = &self.trace else {
            return Ok(None);
        };
        let state = self
            .store
            .state
            .lock()
            .map_err(|_| CapabilityError::Failed("frame state poisoned".into()))?;
        let evidence = crate::runtime_trace::CapturedEvidence {
            frame: state
                .frames
                .get(&frame)
                .cloned()
                .ok_or_else(|| CapabilityError::NotFound("frame handle".into()))?,
            captured_at: state
                .times
                .get(&frame)
                .map(|(time, _)| time.clone())
                .unwrap_or_default(),
            source: state.stamps.get(&frame).cloned(),
        };
        metadata["capture_time_basis"] = state
            .times
            .get(&frame)
            .map(|(_, basis)| *basis)
            .unwrap_or("unknown")
            .into();
        let mut template = None;
        if metadata["kind"].as_str() == Some("consumed") {
            if let Some((details, bytes)) = state.matches.get(&frame) {
                if let (Some(target), Some(source)) =
                    (metadata.as_object_mut(), details.as_object())
                {
                    target.extend(source.clone());
                }
                template = bytes.clone();
            }
        }
        drop(state);
        Ok(trace.submit(evidence, metadata, forced, template))
    }
    async fn trace_capture(
        &self,
        device: &DeviceHandle,
        metadata: serde_json::Value,
        forced: bool,
    ) -> CapabilityResult<Option<String>> {
        let Some(trace) = &self.trace else {
            return Ok(None);
        };
        trace.configure(&metadata);
        if !forced && metadata["trace_enabled"] == false {
            return Ok(None);
        }
        if metadata["kind"].as_str() == Some("error_fresh") && !trace.root_error(&metadata) {
            return Ok(None);
        }
        // CDP screenshots are on-demand and may be expensive. No continuous
        // browser screenshot loop is implied by enabling trace.
        match self.capture(device).await {
            Ok(frame) => self.trace_frame(frame, metadata, forced).await,
            Err(error) => {
                if let Some(run_id) = metadata["run_id"].as_str() {
                    trace.gap(run_id, &format!("capture_failed: {error}"));
                }
                Ok(None)
            }
        }
    }

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
            let captured_at =
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let frame = crate::matcher::compute::run(move || {
                crate::matcher::DecodedFrame::from_png(&bytes)
            })
            .await
            .and_then(|result| result)
            .map_err(|e| CapabilityError::Failed(e.to_string()))?;
            return self
                .store
                .insert_stamped(frame, Some(stamp), captured_at, "capture_completed");
        }
        let (size, expected) = self.coordinate_space(device).await?;
        let (frame, captured_at, time_basis) = self
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
        self.store
            .insert_stamped(frame, expected, captured_at, time_basis)
    }

    async fn size(&self, frame: FrameHandle) -> CapabilityResult<FrameSize> {
        let frame = self.store.get(frame)?;
        let (width, height) = frame.dimensions();
        Ok(FrameSize::new(width, height))
    }
}
