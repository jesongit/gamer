use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::scripts::ScriptStore;

use super::super::{
    CapabilityError, CapabilityResult, ResourceHandle, ResourceId, ResourceLease, ResourceService,
};

struct ResolvedResource {
    id: ResourceId,
    path: PathBuf,
}

/// Logical template resource adapter. `PathBuf` is retained only in this module
/// and is never placed in a capability request or response.
pub(crate) struct ResourceAdapter {
    scripts: Arc<ScriptStore>,
    resources: Mutex<HashMap<ResourceHandle, ResolvedResource>>,
    by_id: Mutex<HashMap<ResourceId, ResourceHandle>>,
}

impl ResourceAdapter {
    pub(crate) fn new(scripts: Arc<ScriptStore>) -> Self {
        Self {
            scripts,
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
        let logical_name = id.name().strip_prefix("tmpl/").unwrap_or(id.name());
        let path = self
            .scripts
            .resolve_template_path(id.namespace(), logical_name)
            .map_err(|error| CapabilityError::NotFound(error.to_string()))?;
        if !path.is_file() {
            return Err(CapabilityError::NotFound(format!(
                "resource {}/{}",
                id.namespace(),
                id.name()
            )));
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
}
