use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use image::ImageEncoder;

use crate::device::DeviceManager;

use super::super::{
    CapabilityError, CapabilityResult, DeviceHandle, FrameHandle, FrameService, FrameSize,
    SearchRegion,
};

// Match the existing decoded-image boundary. These also cover synthetic frames,
// which can enter the frame table without passing through the PNG decoder.
const MAX_FRAME_DIMENSION: u32 = 16_384;
const MAX_FRAME_PIXELS: u64 = 32_000_000;
const MAX_FRAME_PNG_BYTES: usize = 32 * 1024 * 1024;

fn checked_image_region(
    size: (u32, u32),
    region: Option<SearchRegion>,
) -> CapabilityResult<SearchRegion> {
    let (width, height) = size;
    if width == 0
        || height == 0
        || width > MAX_FRAME_DIMENSION
        || height > MAX_FRAME_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_FRAME_PIXELS
    {
        return Err(CapabilityError::InvalidRequest(
            "frame image exceeds the dimension or pixel budget".into(),
        ));
    }
    let region = region.unwrap_or(SearchRegion::new(0, 0, width, height));
    if region.width == 0
        || region.height == 0
        || !region
            .x
            .checked_add(region.width)
            .is_some_and(|end| end <= width)
        || !region
            .y
            .checked_add(region.height)
            .is_some_and(|end| end <= height)
    {
        return Err(CapabilityError::InvalidRequest(
            "frame image region must be nonempty and entirely inside the frame".into(),
        ));
    }
    Ok(region)
}

#[derive(Default)]
struct BoundedPngBytes {
    bytes: Vec<u8>,
    exceeded: bool,
}

impl Write for BoundedPngBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_FRAME_PNG_BYTES.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "encoded frame exceeds the 32 MiB PNG budget",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Shared by live and recorded frame adapters. The caller submits this CPU work
/// to `matcher::compute`; it only encodes the supplied decoded image.
pub(crate) fn encode_frame_png(
    frame: &crate::matcher::DecodedFrame,
    region: Option<SearchRegion>,
) -> CapabilityResult<Vec<u8>> {
    let region = checked_image_region(frame.dimensions(), region)?;
    let original = frame.image();
    // Full-frame encoding borrows the shared RGB storage. Only a non-contiguous
    // ROI needs a temporary pixel buffer, dropped immediately after encoding.
    let cropped;
    let pixels = if region.x == 0 && region.width == original.width() {
        let row_bytes = region.width as usize * 3;
        let first = region.y as usize * row_bytes;
        &original.as_raw()[first..first + region.height as usize * row_bytes]
    } else {
        cropped =
            image::imageops::crop_imm(original, region.x, region.y, region.width, region.height)
                .to_image();
        cropped.as_raw().as_slice()
    };
    let mut output = BoundedPngBytes::default();
    let result = image::codecs::png::PngEncoder::new_with_quality(
        &mut output,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        pixels,
        region.width,
        region.height,
        image::ExtendedColorType::Rgb8,
    );
    if output.exceeded {
        return Err(CapabilityError::InvalidRequest(
            "encoded frame exceeds the 32 MiB PNG budget".into(),
        ));
    }
    result.map_err(|error| CapabilityError::Failed(format!("PNG encoding failed: {error}")))?;
    Ok(output.bytes)
}

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

    pub(crate) fn release(&self, handle: FrameHandle) {
        if let Ok(mut state) = self.state.lock() {
            state.frames.remove(&handle);
            state.stamps.remove(&handle);
            state.times.remove(&handle);
            state.matches.remove(&handle);
            state.order.retain(|stored| *stored != handle);
        }
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

    async fn image_png(
        &self,
        frame: FrameHandle,
        region: Option<SearchRegion>,
    ) -> CapabilityResult<Vec<u8>> {
        let frame = self.store.get(frame)?;
        crate::matcher::compute::run(move || encode_frame_png(&frame, region))
            .await
            .map_err(|error| CapabilityError::Failed(error.to_string()))?
    }

    fn release(&self, frame: FrameHandle) {
        self.store.release(frame);
    }
}

#[cfg(test)]
mod image_png_tests {
    use image::{Rgb, RgbImage};

    use super::*;

    fn adapter() -> (tempfile::TempDir, FrameAdapter) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let db = Arc::new(crate::store::Store::open(&cfg).unwrap());
        let devices = Arc::new(DeviceManager::new(db, cfg));
        let adapter = FrameAdapter::new(devices, Arc::new(FrameStore::new()));
        (dir, adapter)
    }

    fn pixels() -> RgbImage {
        RgbImage::from_fn(4, 3, |x, y| Rgb([x as u8 * 23, y as u8 * 57, 201]))
    }

    #[tokio::test]
    async fn exports_exact_handle_pixels_and_preserves_its_identity() {
        let (_dir, adapter) = adapter();
        let original = pixels();
        let stamp = crate::capabilities::FrameStamp {
            target: "bound-target".into(),
            epoch: "capture-epoch".into(),
            revision: 7,
        };
        let handle = adapter
            .store
            .insert_stamped(
                crate::matcher::DecodedFrame::from_rgb(original.clone()),
                Some(stamp.clone()),
                "2026-10-07T00:00:00Z".into(),
                "synthetic",
            )
            .unwrap();
        // Another frame is now newest. Export must still use the requested one.
        adapter
            .store
            .insert(crate::matcher::DecodedFrame::from_rgb(
                RgbImage::from_pixel(1, 1, Rgb([255, 0, 0])),
            ))
            .unwrap();

        let png = adapter.image_png(handle, None).await.unwrap();
        let exported = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(exported, original);
        assert_eq!(adapter.stamp(handle).await.unwrap(), Some(stamp));
        assert_eq!(adapter.size(handle).await.unwrap(), FrameSize::new(4, 3));
    }

    #[test]
    fn crops_exact_pixels_including_bottom_and_right_edges() {
        let original = pixels();
        let frame = crate::matcher::DecodedFrame::from_rgb(original.clone());
        for region in [
            SearchRegion::new(1, 1, 3, 2),
            SearchRegion::new(0, 1, 4, 2),
            SearchRegion::new(3, 2, 1, 1),
        ] {
            let png = encode_frame_png(&frame, Some(region)).unwrap();
            let exported = image::load_from_memory(&png).unwrap().to_rgb8();
            let expected = image::imageops::crop_imm(
                &original,
                region.x,
                region.y,
                region.width,
                region.height,
            )
            .to_image();
            assert_eq!(exported, expected);
        }
        assert_eq!(frame.dimensions(), (4, 3));
        assert_eq!(frame.pixel(3, 2), Some(original.get_pixel(3, 2).0));
    }

    #[test]
    fn rejects_empty_overflowing_and_outside_regions_without_clamping() {
        let frame = crate::matcher::DecodedFrame::from_rgb(pixels());
        for region in [
            SearchRegion::new(0, 0, 0, 1),
            SearchRegion::new(0, 0, 1, 0),
            SearchRegion::new(4, 0, 1, 1),
            SearchRegion::new(0, 3, 1, 1),
            SearchRegion::new(2, 1, 3, 2),
            SearchRegion::new(u32::MAX, 0, 2, 1),
            SearchRegion::new(0, u32::MAX, 1, 2),
        ] {
            assert!(matches!(
                encode_frame_png(&frame, Some(region)),
                Err(CapabilityError::InvalidRequest(_))
            ));
        }
    }

    #[tokio::test]
    async fn rejects_evicted_frame_handles_instead_of_capturing_a_replacement() {
        let (_dir, adapter) = adapter();
        let first = adapter
            .store
            .insert(crate::matcher::DecodedFrame::from_rgb(pixels()))
            .unwrap();
        for _ in 0..MAX_STORED_FRAMES {
            adapter
                .store
                .insert(crate::matcher::DecodedFrame::from_rgb(pixels()))
                .unwrap();
        }
        assert!(matches!(
            adapter.image_png(first, None).await,
            Err(CapabilityError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn release_invalidates_only_its_handle_and_clears_retained_metadata() {
        let (_dir, adapter) = adapter();
        let stamp = crate::capabilities::FrameStamp {
            target: "retained-target".into(),
            epoch: "retained-epoch".into(),
            revision: 8,
        };
        let first = adapter
            .store
            .insert_stamped(
                crate::matcher::DecodedFrame::from_rgb(pixels()),
                Some(stamp.clone()),
                "2026-10-07T00:00:00Z".into(),
                "synthetic",
            )
            .unwrap();
        adapter.store.record_match(
            first,
            serde_json::json!({"found":false}),
            Some(vec![1, 2, 3]),
        );
        let retained = adapter
            .store
            .insert_stamped(
                crate::matcher::DecodedFrame::from_rgb(pixels()),
                Some(stamp.clone()),
                "2026-10-07T00:00:00Z".into(),
                "synthetic",
            )
            .unwrap();
        // An already-submitted trace may retain its Arc after the handle is gone.
        let evidence_pixels = adapter.store.get(first).unwrap();
        adapter.release(first);
        adapter.release(first);
        assert!(matches!(
            adapter.image_png(first, None).await,
            Err(CapabilityError::NotFound(_))
        ));
        assert_eq!(evidence_pixels.dimensions(), (4, 3));
        assert_eq!(adapter.stamp(retained).await.unwrap(), Some(stamp));
        let png = adapter.image_png(retained, None).await.unwrap();
        assert_eq!(image::load_from_memory(&png).unwrap().to_rgb8(), pixels());
        let state = adapter.store.state.lock().unwrap();
        assert!(!state.frames.contains_key(&first));
        assert!(!state.stamps.contains_key(&first));
        assert!(!state.times.contains_key(&first));
        assert!(!state.matches.contains_key(&first));
        assert!(!state.order.contains(&first));
        assert_eq!(state.frames.len(), 1);
    }

    #[test]
    fn enforces_decoded_pixel_and_encoded_byte_budgets() {
        for size in [(0, 5), (5, 0), (16_385, 1), (1, 16_385), (8_000, 4_001)] {
            assert!(matches!(
                checked_image_region(size, Some(SearchRegion::new(0, 0, 1, 1))),
                Err(CapabilityError::InvalidRequest(_))
            ));
        }
        assert!(checked_image_region((8_000, 4_000), None).is_ok());
        let mut output = BoundedPngBytes {
            bytes: vec![0; MAX_FRAME_PNG_BYTES],
            exceeded: false,
        };
        assert!(output.write_all(&[1]).is_err());
        assert!(output.exceeded);
        assert_eq!(output.bytes.len(), MAX_FRAME_PNG_BYTES);
    }
}
