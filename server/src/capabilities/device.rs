use async_trait::async_trait;

use super::CapabilityResult;

/// Logical application identifier. It is intentionally not a host path.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AppId(String);

impl AppId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Logical device identifier selected by an adapter.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque device reference passed between capabilities.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DeviceHandle {
    id: DeviceId,
}

impl DeviceHandle {
    pub fn new(id: DeviceId) -> Self {
        Self { id }
    }

    pub fn id(&self) -> &DeviceId {
        &self.id
    }
}

/// Device and application lifecycle boundary.
///
/// A concrete implementation owns connection state and any transport-specific
/// details. The capability layer only passes logical IDs and handles.
#[async_trait]
pub trait DeviceService: Send + Sync {
    async fn resolve(&self, id: &DeviceId) -> CapabilityResult<DeviceHandle>;

    async fn start_app(&self, device: &DeviceHandle, app: &AppId) -> CapabilityResult<()>;

    async fn stop_app(&self, device: &DeviceHandle, app: &AppId) -> CapabilityResult<()>;
}
