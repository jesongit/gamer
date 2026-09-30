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
    /// Export this exact decoded frame; zero preserves original dimensions.
    async fn png(
        &self,
        _frame: FrameHandle,
        _max_side: u32,
    ) -> CapabilityResult<(Vec<u8>, FrameSize)> {
        Err(super::CapabilityError::Unavailable(
            "frame export unavailable",
        ))
    }
    /// Size and identity read together, for converting literal relative coordinates.
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
