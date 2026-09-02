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

/// Frame acquisition and metadata boundary.
///
/// `latest` and `capture` return handles only. A concrete adapter owns decode,
/// retention, and screenshot encoding policy.
#[async_trait]
pub trait FrameService: Send + Sync {
    async fn latest(&self, device: &DeviceHandle) -> CapabilityResult<Option<FrameHandle>>;

    async fn capture(&self, device: &DeviceHandle) -> CapabilityResult<FrameHandle>;

    async fn size(&self, frame: FrameHandle) -> CapabilityResult<FrameSize>;
}
