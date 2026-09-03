//! Immutable version storage plus the small mutable lifecycle state file.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::fs::atomic_write;

use super::archive::{extract_archive, inspect_archive};
use super::error::{ExtensionError, ExtensionResult};
use super::manifest::{parse_manifest, ExtensionManifest, MANIFEST_FILE_NAME};
use super::model::{ExtensionId, ExtensionPath, ExtensionRecord, ExtensionVersion};

#[derive(Clone, Debug)]
pub(crate) struct InstalledExtension {
    manifest: ExtensionManifest,
    root: PathBuf,
}

impl InstalledExtension {
    pub(crate) fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn wasm_path(&self) -> PathBuf {
        self.root.join(self.manifest.entry().as_str())
    }

    pub(crate) fn read_wasm(&self) -> ExtensionResult<Vec<u8>> {
        Ok(fs::read(self.wasm_path())?)
    }

    pub(crate) fn read_file(&self, path: &ExtensionPath) -> ExtensionResult<Vec<u8>> {
        let full_path = self.root.join(path.as_str());
        if !is_regular_file(&full_path)? {
            return Err(ExtensionError::VersionNotInstalled {
                id: self.manifest.id().to_string(),
                version: self.manifest.version().to_string(),
            });
        }
        Ok(fs::read(full_path)?)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PersistedState {
    #[serde(default)]
    plugins: BTreeMap<ExtensionId, ExtensionRecord>,
}

/// Filesystem boundary for extensions. Version directories are never replaced
/// and lifecycle state is stored separately so old versions remain immutable.
#[derive(Clone, Debug)]
pub(crate) struct ExtensionStore {
    data_root: PathBuf,
}

impl ExtensionStore {
    pub(crate) fn new(data_root: impl AsRef<Path>) -> Self {
        Self {
            data_root: data_root.as_ref().to_path_buf(),
        }
    }

    pub(crate) fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub(crate) fn list_installed(&self) -> ExtensionResult<Vec<InstalledExtension>> {
        let root = self.extensions_root();
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut installed = Vec::new();
        for id_entry in entries {
            let id_entry = id_entry?;
            if !id_entry.file_type()?.is_dir() || id_entry.file_name() == ".staging" {
                continue;
            }
            let id = ExtensionId::parse(&id_entry.file_name().to_string_lossy())?;
            for version_entry in fs::read_dir(id_entry.path())? {
                let version_entry = version_entry?;
                if !version_entry.file_type()?.is_dir() {
                    continue;
                }
                let version =
                    ExtensionVersion::parse(&version_entry.file_name().to_string_lossy())?;
                let manifest_path = version_entry.path().join(MANIFEST_FILE_NAME);
                if !is_regular_file(&manifest_path)? {
                    return Err(ExtensionError::InvalidManifest(format!(
                        "缺少 manifest.toml: {id}@{version}"
                    )));
                }
                let manifest = parse_manifest(&fs::read(manifest_path)?)?;
                if manifest.id() != &id || manifest.version() != &version {
                    return Err(ExtensionError::InvalidManifest(format!(
                        "目录与 manifest 不一致: {id}@{version}"
                    )));
                }
                if !is_regular_file(&version_entry.path().join(manifest.entry().as_str()))? {
                    return Err(ExtensionError::InvalidArchive(format!(
                        "WASM entry 不存在: {id}@{version}/{}",
                        manifest.entry()
                    )));
                }
                installed.push(InstalledExtension {
                    manifest,
                    root: version_entry.path(),
                });
            }
        }
        installed.sort_by(|left, right| {
            left.manifest
                .id()
                .cmp(right.manifest.id())
                .then_with(|| left.manifest.version().cmp(right.manifest.version()))
        });
        Ok(installed)
    }

    pub(crate) fn installed(
        &self,
        id: &ExtensionId,
        version: &ExtensionVersion,
    ) -> ExtensionResult<Option<InstalledExtension>> {
        Ok(self.list_installed()?.into_iter().find(|extension| {
            extension.manifest.id() == id && extension.manifest.version() == version
        }))
    }

    /// Install a validated archive into `<id>/<version>`. Existing version
    /// directories are never overwritten, even if incoming bytes differ.
    pub(crate) fn install_archive(&self, archive: &[u8]) -> ExtensionResult<InstalledExtension> {
        let manifest = inspect_archive(archive)?;
        let final_root = self
            .extensions_root()
            .join(manifest.id().as_str())
            .join(manifest.version().as_str());
        if path_exists(&final_root)? {
            return Err(ExtensionError::AlreadyInstalled {
                id: manifest.id().to_string(),
                version: manifest.version().to_string(),
            });
        }

        let staging_parent = self.extensions_root().join(".staging");
        fs::create_dir_all(&staging_parent)?;
        let staging = create_staging_directory(&staging_parent, &manifest)?;
        let result = (|| -> ExtensionResult<InstalledExtension> {
            let extracted = extract_archive(archive, &staging)?;
            if extracted != manifest {
                return Err(ExtensionError::InvalidManifest(
                    "归档 manifest 在校验与提取阶段不一致".to_string(),
                ));
            }
            if path_exists(&final_root)? {
                return Err(ExtensionError::AlreadyInstalled {
                    id: manifest.id().to_string(),
                    version: manifest.version().to_string(),
                });
            }
            if let Some(parent) = final_root.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&staging, &final_root).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    ExtensionError::AlreadyInstalled {
                        id: manifest.id().to_string(),
                        version: manifest.version().to_string(),
                    }
                } else {
                    ExtensionError::Io(error)
                }
            })?;
            sync_directory(final_root.parent().expect("version directory has parent"))?;
            Ok(InstalledExtension {
                manifest: manifest.clone(),
                root: final_root.clone(),
            })
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }

    pub(crate) fn remove_version(
        &self,
        id: &ExtensionId,
        version: &ExtensionVersion,
    ) -> ExtensionResult<bool> {
        let version_root = self
            .extensions_root()
            .join(id.as_str())
            .join(version.as_str());
        if !path_exists(&version_root)? {
            return Ok(false);
        }
        fs::remove_dir_all(&version_root)?;
        let package_root = version_root.parent().expect("version directory has parent");
        let _ = fs::remove_dir(package_root);
        sync_directory(&self.extensions_root())?;
        Ok(true)
    }

    pub(crate) fn read_state(&self) -> ExtensionResult<BTreeMap<ExtensionId, ExtensionRecord>> {
        let path = self.state_path();
        if !path_exists(&path)? {
            return Ok(BTreeMap::new());
        }
        if !is_regular_file(&path)? {
            return Err(ExtensionError::InvalidState(
                "state.json 不是普通文件".to_string(),
            ));
        }
        let state: PersistedState = serde_json::from_slice(&fs::read(path)?)?;
        for (key, record) in &state.plugins {
            if key != &record.id {
                return Err(ExtensionError::InvalidState(format!(
                    "state.json 的 key 与插件 ID 不一致: {key}"
                )));
            }
        }
        Ok(state.plugins)
    }

    pub(crate) fn write_state(
        &self,
        plugins: &BTreeMap<ExtensionId, ExtensionRecord>,
    ) -> ExtensionResult<()> {
        let state = PersistedState {
            plugins: plugins.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&state)?;
        atomic_write(&self.state_path(), &bytes)
            .map_err(|error| ExtensionError::Io(std::io::Error::other(error.to_string())))
    }

    fn extensions_root(&self) -> PathBuf {
        self.data_root.join("extensions")
    }

    fn state_path(&self) -> PathBuf {
        self.extensions_root().join("state.json")
    }
}

fn create_staging_directory(
    parent: &Path,
    manifest: &ExtensionManifest,
) -> ExtensionResult<PathBuf> {
    for _ in 0..8 {
        let candidate = parent.join(format!(
            "{}-{}-{}",
            manifest.id(),
            manifest.version(),
            Uuid::new_v4().simple()
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(ExtensionError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "无法创建唯一 staging 目录",
    )))
}

fn path_exists(path: &Path) -> ExtensionResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn is_regular_file(path: &Path) -> ExtensionResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}
