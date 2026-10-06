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
    frozen: Option<Arc<Vec<u8>>>,
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
        if let Some(bytes) = self
            .resources
            .lock()
            .map_err(|_| CapabilityError::Failed("resource state poisoned".into()))?
            .get(&handle)
            .and_then(|r| r.frozen.clone())
        {
            return Ok((*bytes).clone());
        }
        let path = self
            .resources
            .lock()
            .map_err(|_| CapabilityError::Failed("resource state poisoned".into()))?
            .get(&handle)
            .map(|resource| resource.path.clone())
            .ok_or_else(|| CapabilityError::NotFound("resource handle".into()))?;
        let _snapshot = self.store.snapshot_barrier();
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
    fn release_frozen(&self, resource: ResourceHandle) {
        if let Ok(mut resources) = self.resources.lock() {
            if resources.get(&resource).is_some_and(|r| r.frozen.is_some()) {
                resources.remove(&resource);
            }
        }
    }
    async fn freeze(&self, resource: ResourceHandle) -> CapabilityResult<ResourceHandle> {
        let bytes = Arc::new(self.read(resource)?);
        let mut resources = self
            .resources
            .lock()
            .map_err(|_| CapabilityError::Failed("resource state poisoned".into()))?;
        let old = resources
            .get(&resource)
            .ok_or_else(|| CapabilityError::NotFound("resource".into()))?;
        let frozen = ResolvedResource {
            id: old.id.clone(),
            path: old.path.clone(),
            frozen: Some(bytes),
        };
        let handle = ResourceHandle::new();
        resources.insert(handle, frozen);
        Ok(handle)
    }
    async fn fingerprint(&self, resource: ResourceHandle) -> CapabilityResult<String> {
        use sha2::{Digest, Sha256};
        Ok(format!("{:x}", Sha256::digest(self.read(resource)?)))
    }

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
                frozen: None,
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
            let byte_len = if let Some(bytes) = &resolved.frozen {
                bytes.len() as u64
            } else {
                std::fs::metadata(&resolved.path)
                    .map_err(|error| CapabilityError::Failed(error.to_string()))?
                    .len()
            };
            (resource, byte_len)
        };
        Ok(ResourceLease::new(handle, Some(byte_len)))
    }

    async fn resolved_file_name(&self, handle: ResourceHandle) -> CapabilityResult<String> {
        ResourceAdapter::file_name(self, handle)
    }
    async fn resolved_path(&self, handle: ResourceHandle) -> CapabilityResult<String> {
        let (id, path) = {
            let resources = self
                .resources
                .lock()
                .map_err(|_| CapabilityError::Failed("resource state poisoned".into()))?;
            let resource = resources
                .get(&handle)
                .ok_or_else(|| CapabilityError::NotFound("resource".into()))?;
            (resource.id.clone(), resource.path.clone())
        };
        let root = self
            .store
            .plugin_dir(id.package(), id.plugin())
            .map_err(|e| CapabilityError::Failed(e.to_string()))?;
        path.strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .map_err(|_| CapabilityError::Failed("resource scope mismatch".into()))
    }
}

#[cfg(test)]
mod immutable_resource_tests {
    use super::*;
    #[tokio::test]
    async fn frozen_content_survives_edits_has_full_logical_identity_and_releases() {
        let root = tempfile::tempdir().unwrap();
        let config = crate::config::Config {
            data_dir: root.path().to_path_buf(),
            ..Default::default()
        };
        let store = Arc::new(PackageStore::open(&config).unwrap());
        store
            .create_package(crate::resources::PackageInput {
                id: "snapshot".into(),
                android_targets: vec!["*".into()],
                ..Default::default()
            })
            .unwrap();
        store
            .write_binary(
                "snapshot",
                "gamer-yaml",
                "templates/nested/shared.png",
                b"original",
                None,
                false,
            )
            .unwrap();
        let adapter = ResourceAdapter::new(store.clone());
        let id = ResourceId::new("snapshot", "gamer-yaml", "templates/nested/shared.png").unwrap();
        let live = adapter.resolve(&id).await.unwrap();
        let frozen = adapter.freeze(live).await.unwrap();
        assert_eq!(
            adapter.resolved_path(frozen).await.unwrap(),
            "templates/nested/shared.png"
        );
        let before = adapter.fingerprint(frozen).await.unwrap();
        store
            .write_binary(
                "snapshot",
                "gamer-yaml",
                "templates/nested/shared.png",
                b"changed",
                None,
                true,
            )
            .unwrap();
        assert_eq!(adapter.read(frozen).unwrap(), b"original");
        assert_eq!(adapter.read(live).unwrap(), b"changed");
        assert_eq!(adapter.fingerprint(frozen).await.unwrap(), before);
        assert_ne!(adapter.fingerprint(live).await.unwrap(), before);
        adapter.release_frozen(frozen);
        assert!(adapter.read(frozen).is_err());
        assert!(adapter.read(live).is_ok());
    }
}
