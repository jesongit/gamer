use std::fs;
use std::path::{Path, PathBuf};

use super::error::AppPackageResult;
use super::manifest::parse_manifest;
use super::model::{
    parse_android_package_name, parse_app_package_id, resource_id, AndroidPackageName,
    AppPackageId, InstalledVersion, ResourceId, ResourcePath,
};
use super::store::AppPackageStore;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResourceSource {
    /// 本地编辑区分区文件（`data/<android>/<logical_path>`），最高优先。
    EditableLocal {
        android_package: AndroidPackageName,
    },
    UserOverride {
        android_package: AndroidPackageName,
    },
    Installed {
        app_package: AppPackageId,
        version: InstalledVersion,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedResource {
    id: ResourceId,
    source: ResourceSource,
    path: PathBuf,
}

impl ResolvedResource {
    pub(crate) fn id(&self) -> &ResourceId {
        &self.id
    }

    pub(crate) fn source(&self) -> &ResourceSource {
        &self.source
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn read_bytes(&self) -> AppPackageResult<Vec<u8>> {
        Ok(fs::read(&self.path)?)
    }
}

/// Resolves editable-local partition content first, then user-owned overrides,
/// before immutable installed package content (same priority as
/// [`super::composite::CompositeResolver`]).
#[derive(Clone, Debug)]
pub(crate) struct ResourceResolver {
    data_root: PathBuf,
}

impl ResourceResolver {
    pub(crate) fn new(store: &AppPackageStore) -> Self {
        Self {
            data_root: store.data_root().to_path_buf(),
        }
    }

    pub(crate) fn resolve(
        &self,
        android_package: &AndroidPackageName,
        id: &ResourceId,
    ) -> AppPackageResult<Option<ResolvedResource>> {
        let android_package = parse_android_package_name(android_package.as_str())?;
        let app_package = parse_app_package_id(id.app_package().as_str())?;
        let version = match id.revision() {
            Some(revision) => InstalledVersion::parse(revision)?,
            None => return Ok(None),
        };
        let logical_path = ResourcePath::parse(id.logical_path())?;

        // 层 1：本地编辑区（分区目录，目录即类型 —— logical_path 首段即资源根）
        let local_path = append_resource_path(
            &self.data_root.join(android_package.as_str()),
            &logical_path,
        );
        if is_regular_file(&local_path)? {
            return Ok(Some(ResolvedResource {
                id: id.clone(),
                source: ResourceSource::EditableLocal {
                    android_package: android_package.clone(),
                },
                path: local_path,
            }));
        }

        // 层 2：user-overrides
        let override_path = append_resource_path(
            &self
                .data_root
                .join("user-overrides")
                .join(android_package.as_str()),
            &logical_path,
        );
        if is_regular_file(&override_path)? {
            return Ok(Some(ResolvedResource {
                id: id.clone(),
                source: ResourceSource::UserOverride {
                    android_package: android_package.clone(),
                },
                path: override_path,
            }));
        }

        // 层 3：指定版本的已安装包内容
        let installed_root = self
            .data_root
            .join("app-packages")
            .join(app_package.as_str())
            .join(version.as_str());
        let manifest_path = installed_root.join("manifest.toml");
        if !is_regular_file(&manifest_path)? {
            return Ok(None);
        }
        let manifest = parse_manifest(&fs::read(manifest_path)?)?;
        if manifest.id() != &app_package || manifest.version() != &version {
            return Ok(None);
        }
        if !manifest.supports_android_package(&android_package) {
            return Ok(None);
        }
        let resource_path = append_resource_path(&installed_root, &logical_path);
        if !is_regular_file(&resource_path)? {
            return Ok(None);
        }
        Ok(Some(ResolvedResource {
            id: id.clone(),
            source: ResourceSource::Installed {
                app_package: id.app_package().clone(),
                version,
            },
            path: resource_path,
        }))
    }

    pub(crate) fn resolve_path(
        &self,
        android_package: &AndroidPackageName,
        app_package: AppPackageId,
        version: InstalledVersion,
        logical_path: ResourcePath,
    ) -> AppPackageResult<Option<ResolvedResource>> {
        let id = resource_id(app_package, &version, &logical_path)?;
        self.resolve(android_package, &id)
    }
}

fn append_resource_path(root: &Path, resource_path: &ResourcePath) -> PathBuf {
    resource_path
        .components()
        .fold(root.to_path_buf(), |path, component| path.join(component))
}

fn is_regular_file(path: &Path) -> AppPackageResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
