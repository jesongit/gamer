use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::core::fs::atomic_write;

use super::archive::{extract_archive, validate_and_read_manifest};
use super::error::{AppPackageError, AppPackageResult};
use super::manifest::{parse_manifest, PackageManifest};
use super::model::{
    parse_android_package_name, parse_app_package_id, AndroidPackageName, AppPackageId,
    InstalledVersion, ResourcePath,
};
use super::resolver::ResourceResolver;

#[derive(Clone, Debug)]
pub(crate) struct InstalledPackage {
    manifest: PackageManifest,
    root: PathBuf,
}

impl InstalledPackage {
    pub(crate) fn manifest(&self) -> &PackageManifest {
        &self.manifest
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

/// Filesystem boundary for immutable App Packages and user-owned overrides.
#[derive(Clone, Debug)]
pub(crate) struct AppPackageStore {
    data_root: PathBuf,
}

impl AppPackageStore {
    pub(crate) fn new(data_root: impl AsRef<Path>) -> Self {
        Self {
            data_root: data_root.as_ref().to_path_buf(),
        }
    }

    pub(crate) fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub(crate) fn resolver(&self) -> ResourceResolver {
        ResourceResolver::new(self)
    }

    pub(crate) fn list_installed(&self) -> AppPackageResult<Vec<InstalledPackage>> {
        let root = self.app_packages_root();
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut installed = Vec::new();
        for package_entry in entries {
            let package_entry = package_entry?;
            if !package_entry.file_type()?.is_dir() || package_entry.file_name() == ".staging" {
                continue;
            }
            let package_id = parse_app_package_id(&package_entry.file_name().to_string_lossy())?;
            for version_entry in fs::read_dir(package_entry.path())? {
                let version_entry = version_entry?;
                if !version_entry.file_type()?.is_dir() {
                    continue;
                }
                let version =
                    InstalledVersion::parse(&version_entry.file_name().to_string_lossy())?;
                let manifest =
                    parse_manifest(&fs::read(version_entry.path().join("manifest.toml"))?)?;
                if manifest.id() != &package_id || manifest.version() != &version {
                    return Err(AppPackageError::InvalidManifest(format!(
                        "目录与 manifest 不一致: {}@{}",
                        package_id, version
                    )));
                }
                installed.push(InstalledPackage {
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

    /// Install one validated archive into a new version directory. Existing
    /// versions are never overwritten, even when the incoming bytes differ.
    pub(crate) fn install_archive(&self, archive: &[u8]) -> AppPackageResult<InstalledPackage> {
        let manifest_bytes = validate_and_read_manifest(archive)?;
        let manifest = parse_manifest(&manifest_bytes)?;
        let final_root = self
            .app_packages_root()
            .join(manifest.id().as_str())
            .join(manifest.version().as_str());
        if path_exists(&final_root)? {
            return Err(AppPackageError::AlreadyInstalled {
                package: manifest.id().to_string(),
                version: manifest.version().to_string(),
            });
        }

        let staging_parent = self.app_packages_root().join(".staging");
        fs::create_dir_all(&staging_parent)?;
        let staging = create_staging_directory(&staging_parent, manifest.id(), manifest.version())?;
        let result = (|| -> AppPackageResult<InstalledPackage> {
            extract_archive(archive, &staging)?;
            if path_exists(&final_root)? {
                return Err(AppPackageError::AlreadyInstalled {
                    package: manifest.id().to_string(),
                    version: manifest.version().to_string(),
                });
            }
            if let Some(parent) = final_root.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&staging, &final_root).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    AppPackageError::AlreadyInstalled {
                        package: manifest.id().to_string(),
                        version: manifest.version().to_string(),
                    }
                } else {
                    AppPackageError::Io(error)
                }
            })?;
            sync_directory(final_root.parent().expect("version directory has parent"))?;
            Ok(InstalledPackage {
                manifest: manifest.clone(),
                root: final_root.clone(),
            })
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }

    /// Remove one immutable version only. User overrides and unrelated data
    /// are deliberately outside this path and remain untouched.
    pub(crate) fn uninstall(
        &self,
        package: &AppPackageId,
        version: &InstalledVersion,
    ) -> AppPackageResult<bool> {
        let package = parse_app_package_id(package.as_str())?;
        let package_root = self.app_packages_root().join(package.as_str());
        let version_root = package_root.join(version.as_str());
        if !path_exists(&version_root)? {
            return Ok(false);
        }
        fs::remove_dir_all(&version_root)?;
        let _ = fs::remove_dir(&package_root);
        sync_directory(&self.app_packages_root())?;
        Ok(true)
    }

    /// Uninstall a version and notify Timer Core only when the package has no
    /// remaining installed version. User tasks stay persisted and become
    /// Suspended; deleting a package must never delete a user schedule.
    pub(crate) async fn uninstall_and_update_tasks(
        &self,
        package: &AppPackageId,
        version: &InstalledVersion,
        timer: &crate::timer_core::TimerCore,
    ) -> anyhow::Result<(bool, usize)> {
        let removed = self.uninstall(package, version)?;
        if !removed {
            return Ok((false, 0));
        }
        let package_still_installed = self
            .list_installed()?
            .iter()
            .any(|installed| installed.manifest().id() == package);
        let suspended = if package_still_installed {
            0
        } else {
            timer.on_app_package_uninstalled(package.as_str()).await?
        };
        Ok((true, suspended))
    }

    pub(crate) fn write_user_override(
        &self,
        android_package: &AndroidPackageName,
        logical_path: &ResourcePath,
        bytes: &[u8],
    ) -> AppPackageResult<()> {
        let android_package = parse_android_package_name(android_package.as_str())?;
        let path = append_resource_path(
            &self
                .data_root
                .join("user-overrides")
                .join(android_package.as_str()),
            logical_path,
        );
        atomic_write(&path, bytes)
            .map_err(|error| AppPackageError::Io(std::io::Error::other(error.to_string())))
    }

    pub(crate) fn remove_user_override(
        &self,
        android_package: &AndroidPackageName,
        logical_path: &ResourcePath,
    ) -> AppPackageResult<bool> {
        let android_package = parse_android_package_name(android_package.as_str())?;
        let path = append_resource_path(
            &self
                .data_root
                .join("user-overrides")
                .join(android_package.as_str()),
            logical_path,
        );
        match fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn app_packages_root(&self) -> PathBuf {
        self.data_root.join("app-packages")
    }
}

fn path_exists(path: &Path) -> AppPackageResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn create_staging_directory(
    parent: &Path,
    package: &AppPackageId,
    version: &InstalledVersion,
) -> AppPackageResult<PathBuf> {
    for _ in 0..8 {
        let candidate = parent.join(format!(
            "{}-{}-{}",
            package,
            version,
            Uuid::new_v4().simple()
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(AppPackageError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "无法创建唯一 staging 目录",
    )))
}

fn append_resource_path(root: &Path, resource_path: &ResourcePath) -> PathBuf {
    resource_path
        .components()
        .fold(root.to_path_buf(), |path, component| path.join(component))
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
