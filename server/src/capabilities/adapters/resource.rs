use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::resources::PackageStore;

use super::super::{
    CapabilityError, CapabilityResult, ResourceHandle, ResourceId, ResourceLease, ResourceService,
};

struct ResolvedResource {
    id: ResourceId,
    path: PathBuf,
}

/// Package Resource 适配器：把 [`ResourceId`]`(package, plugin, path)` 三元组
/// 解析到 [`PackageStore`] 的磁盘路径。`PathBuf` 只留在本模块内，绝不进入
/// capability 请求/响应。解析强制限定在 `packages/<package>/plugins/<plugin>/`
/// 前缀内（插件数据隔离由 PackageStore 的路径校验保证）；模板 `#` 后缀短名
/// 消歧为文件名约定（精确名优先，否则同扩展名唯一候选）。
pub(crate) struct ResourceAdapter {
    store: Arc<PackageStore>,
    resources: Mutex<HashMap<ResourceHandle, ResolvedResource>>,
    by_id: Mutex<HashMap<ResourceId, ResourceHandle>>,
}

impl ResourceAdapter {
    pub(crate) fn new(store: Arc<PackageStore>) -> Self {
        Self {
            store,
            resources: Mutex::new(HashMap::new()),
            by_id: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn read(&self, handle: ResourceHandle) -> CapabilityResult<Vec<u8>> {
        let path = self
            .resources
            .lock()
            .map_err(|_| CapabilityError::Failed("resource state poisoned".into()))?
            .get(&handle)
            .map(|resource| resource.path.clone())
            .ok_or_else(|| CapabilityError::NotFound("resource handle".into()))?;
        std::fs::read(&path).map_err(|error| CapabilityError::Failed(error.to_string()))
    }

    /// 解析后的实际文件名（模板含 `#区域` 后缀，供搜索区域推断）。
    pub(crate) fn file_name(&self, handle: ResourceHandle) -> CapabilityResult<String> {
        let path = self
            .resources
            .lock()
            .map_err(|_| CapabilityError::Failed("resource state poisoned".into()))?
            .get(&handle)
            .map(|resource| resource.path.clone())
            .ok_or_else(|| CapabilityError::NotFound("resource handle".into()))?;
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| CapabilityError::NotFound("resource file name".into()))
    }

    pub(crate) fn id(&self, handle: ResourceHandle) -> CapabilityResult<ResourceId> {
        self.resources
            .lock()
            .map_err(|_| CapabilityError::Failed("resource state poisoned".into()))?
            .get(&handle)
            .map(|resource| resource.id.clone())
            .ok_or_else(|| CapabilityError::NotFound("resource handle".into()))
    }
}

#[async_trait]
impl ResourceService for ResourceAdapter {
    async fn resolve(&self, id: &ResourceId) -> CapabilityResult<ResourceHandle> {
        let path = self
            .store
            .resolve_short_path(id.package(), id.plugin(), id.path())
            .map_err(|error| CapabilityError::NotFound(error.to_string()))?;
        if !path.is_file() {
            return Err(CapabilityError::NotFound(id.composite_key()));
        }
        let handle = ResourceHandle::new();
        let mut resources = self
            .resources
            .lock()
            .map_err(|_| CapabilityError::Failed("resource state poisoned".into()))?;
        let mut by_id = self
            .by_id
            .lock()
            .map_err(|_| CapabilityError::Failed("resource index poisoned".into()))?;
        if let Some(handle) = by_id.get(id).copied() {
            if let Some(resource) = resources.get_mut(&handle) {
                resource.path = path;
                return Ok(handle);
            }
            by_id.remove(id);
        }
        resources.insert(
            handle,
            ResolvedResource {
                id: id.clone(),
                path,
            },
        );
        by_id.insert(id.clone(), handle);
        Ok(handle)
    }

    async fn open(&self, resource: ResourceHandle) -> CapabilityResult<ResourceLease> {
        let (handle, byte_len) = {
            let resources = self
                .resources
                .lock()
                .map_err(|_| CapabilityError::Failed("resource state poisoned".into()))?;
            let resolved = resources
                .get(&resource)
                .ok_or_else(|| CapabilityError::NotFound("resource handle".into()))?;
            let byte_len = std::fs::metadata(&resolved.path)
                .map_err(|error| CapabilityError::Failed(error.to_string()))?
                .len();
            (resource, byte_len)
        };
        Ok(ResourceLease::new(handle, Some(byte_len)))
    }

    async fn resolved_file_name(&self, handle: ResourceHandle) -> CapabilityResult<String> {
        ResourceAdapter::file_name(self, handle)
    }
}
