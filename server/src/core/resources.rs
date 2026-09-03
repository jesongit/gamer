//! Logical resource resolution boundary.

use futures_util::future::BoxFuture;

use super::{ResourceHandle, ResourceId};

/// Resolver-owned resource data.  No host path crosses this boundary.
#[derive(Clone, Debug)]
pub struct ResolvedResource {
    handle: ResourceHandle,
    bytes: Vec<u8>,
}

impl ResolvedResource {
    pub fn new(handle: impl Into<ResourceHandle>, bytes: Vec<u8>) -> Self {
        Self {
            handle: handle.into(),
            bytes,
        }
    }

    pub fn id(&self) -> &ResourceId {
        self.handle.id()
    }

    pub fn handle(&self) -> &ResourceHandle {
        &self.handle
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Maps a logical [`ResourceId`] to bytes.  Filesystem, package-store, and
/// future remote implementations stay behind this interface.
pub trait ResourceResolver: Send + Sync + 'static {
    fn resolve(&self, id: &ResourceId) -> BoxFuture<'_, anyhow::Result<ResolvedResource>>;
}
