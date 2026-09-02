use async_trait::async_trait;
use uuid::Uuid;

use super::CapabilityResult;

/// Logical resource identity. `name` is an application-level name, not a host
/// filesystem path.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ResourceId {
    namespace: String,
    name: String,
}

impl ResourceId {
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Opaque resource capability token.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResourceHandle(Uuid);

impl ResourceHandle {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Small metadata returned after an adapter authorizes a resource for opening.
/// Resource bytes and host paths stay behind the adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceLease {
    handle: ResourceHandle,
    byte_len: Option<u64>,
}

impl ResourceLease {
    pub(crate) fn new(handle: ResourceHandle, byte_len: Option<u64>) -> Self {
        Self { handle, byte_len }
    }

    pub fn handle(self) -> ResourceHandle {
        self.handle
    }

    pub fn byte_len(self) -> Option<u64> {
        self.byte_len
    }
}

/// Logical resource resolution/open boundary. No `PathBuf` or storage handle is
/// exposed to callers.
#[async_trait]
pub trait ResourceService: Send + Sync {
    async fn resolve(&self, id: &ResourceId) -> CapabilityResult<ResourceHandle>;

    async fn open(&self, resource: ResourceHandle) -> CapabilityResult<ResourceLease>;
}
