use async_trait::async_trait;
use uuid::Uuid;

use super::{CapabilityResult, DeviceHandle};

/// Opaque decoded-frame reference. The RGB/YUV storage remains owned by the
/// frame adapter and is never copied through this contract.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FrameHandle(Uuid);

impl FrameHandle {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

impl FrameSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FrameStamp {
    pub target: String,
    pub epoch: String,
    pub revision: u64,
}

/// Frame acquisition and metadata boundary.
///
/// `latest` and `capture` return handles only. A concrete adapter owns decode,
/// retention, and screenshot encoding policy.
#[async_trait]
pub trait FrameService: Send + Sync {
    /// Persist the exact consumed frame, including failed matches. A disabled
    /// automatic trace still retains the last consumed frame for root errors.
    async fn trace_frame(
        &self,
        _frame: FrameHandle,
        _metadata: serde_json::Value,
        _forced: bool,
    ) -> CapabilityResult<Option<String>> {
        Ok(None)
    }
    /// Capture an explicitly identified boundary/error image. Never relabel a
    /// previously consumed image as a fresh capture.
    async fn trace_capture(
        &self,
        _device: &DeviceHandle,
        _metadata: serde_json::Value,
        _forced: bool,
    ) -> CapabilityResult<Option<String>> {
        Ok(None)
    }
    async fn trace_snapshot(
        &self,
        _run_id: String,
        _snapshot: serde_json::Value,
    ) -> CapabilityResult<()> {
        Ok(())
    }

    /// Size and identity read together, for converting literal relative coordinates.
    /// Android and browser adapters identify the live session epoch and its
    /// coordinate revision. Equal dimensions do not imply the same session.
    async fn coordinate_space(
        &self,
        device: &DeviceHandle,
    ) -> CapabilityResult<(FrameSize, Option<FrameStamp>)> {
        Ok((self.device_size(device).await?, None))
    }
    async fn stamp(&self, _frame: FrameHandle) -> CapabilityResult<Option<FrameStamp>> {
        Ok(None)
    }

    /// Current input/video coordinate space. Metadata only: do not decode a
    /// screenshot just to convert a relative tap or swipe to pixels.
    async fn device_size(&self, device: &DeviceHandle) -> CapabilityResult<FrameSize>;

    async fn latest(&self, device: &DeviceHandle) -> CapabilityResult<Option<FrameHandle>>;

    async fn capture(&self, device: &DeviceHandle) -> CapabilityResult<FrameHandle>;

    async fn size(&self, frame: FrameHandle) -> CapabilityResult<FrameSize>;
}
