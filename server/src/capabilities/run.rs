use async_trait::async_trait;
use uuid::Uuid;

use super::{CapabilityResult, DeviceHandle, ResourceHandle};

/// Minimal run submission data. YAML/script-specific payloads stay in their
/// adapter until a later phase defines a shared model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunRequest {
    device: DeviceHandle,
    entry: ResourceHandle,
}

impl RunRequest {
    pub fn new(device: DeviceHandle, entry: ResourceHandle) -> Self {
        Self { device, entry }
    }

    pub fn device(&self) -> &DeviceHandle {
        &self.device
    }

    pub fn entry(&self) -> ResourceHandle {
        self.entry
    }
}

/// Opaque run identity returned by the run adapter.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RunHandle(Uuid);

impl RunHandle {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// Run lifecycle boundary.
#[async_trait]
pub trait RunService: Send + Sync {
    async fn submit(&self, request: RunRequest) -> CapabilityResult<RunHandle>;

    async fn cancel(&self, run: RunHandle) -> CapabilityResult<()>;

    async fn status(&self, run: RunHandle) -> CapabilityResult<RunStatus>;
}
