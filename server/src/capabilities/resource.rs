use async_trait::async_trait;
use uuid::Uuid;

use super::CapabilityResult;

pub(crate) use crate::core::ResourceId;

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
/// exposed to callers. [`ResourceId`] = Package Resource 三元组
/// `(package, plugin, path)`；实现方必须把插件限制在自己的
/// `packages/<package>/plugins/<plugin>/` 前缀内。
#[async_trait]
pub trait ResourceService: Send + Sync {
    async fn resolve(&self, id: &ResourceId) -> CapabilityResult<ResourceHandle>;

    async fn open(&self, resource: ResourceHandle) -> CapabilityResult<ResourceLease>;

    /// 解析后的实际文件名（模板含 `#区域` 后缀，供搜索区域推断/回显）。
    async fn resolved_file_name(&self, handle: ResourceHandle) -> CapabilityResult<String>;
}
