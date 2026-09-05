use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::core::fs::atomic_write;

use super::archive::{extract_archive, validate_and_read_manifest};
use super::error::{AppPackageError, AppPackageResult};
use super::manifest::{parse_manifest, PackageManifest};
use super::model::{
    parse_android_package_name, parse_app_package_id, AndroidPackageName, AppPackageId,
    InstalledVersion, ResourcePath,
};
use super::presets::PresetDeclaration;
use super::resolver::ResourceResolver;

/// App Package 卸载后的唯一生命周期接缝。实现者只负责把仍持久化的
/// User Task 置为 Suspended，不得删除任务或修改基础设备能力。
#[async_trait]
pub(crate) trait AppPackageTaskHook: Send + Sync {
    async fn suspend_for_package(&self, package: &AppPackageId) -> anyhow::Result<usize>;
}

/// 激活版本时的任务预设发布接缝（Phase 9「包提供任务预设」）。实现者把
/// 包内 `presets/` 声明灌入任务预设存储；幂等性由确定性发布 id 保证。
#[async_trait]
pub(crate) trait AppPackagePresetHook: Send + Sync {
    async fn publish_presets(
        &self,
        package: &AppPackageId,
        presets: &[PresetDeclaration],
    ) -> anyhow::Result<usize>;
}

#[derive(Default)]
pub(crate) struct NoopAppPackageTaskHook;

#[async_trait]
impl AppPackageTaskHook for NoopAppPackageTaskHook {
    async fn suspend_for_package(&self, _package: &AppPackageId) -> anyhow::Result<usize> {
        Ok(0)
    }
}

#[derive(Default)]
struct NoopAppPackagePresetHook;

#[async_trait]
impl AppPackagePresetHook for NoopAppPackagePresetHook {
    async fn publish_presets(
        &self,
        _package: &AppPackageId,
        _presets: &[PresetDeclaration],
    ) -> anyhow::Result<usize> {
        Ok(0)
    }
}

/// Production hook adapter. Keeping Scheduler behind this small trait means
/// AppPackageStore does not know Timer Core internals and can be tested with a
/// fake hook.
pub(crate) struct TimerTaskSuspendedHook {
    timer: Arc<crate::timer_core::TimerCore>,
}

impl TimerTaskSuspendedHook {
    pub(crate) fn new(timer: Arc<crate::timer_core::TimerCore>) -> Self {
        Self { timer }
    }
}

#[async_trait]
impl AppPackageTaskHook for TimerTaskSuspendedHook {
    async fn suspend_for_package(&self, package: &AppPackageId) -> anyhow::Result<usize> {
        self.timer
            .on_app_package_uninstalled(package.as_str())
            .await
    }
}

/// Composition-root adapter used by the running service. The package store
/// talks to this hook only; it does not gain a dependency on Timer Core.
pub(crate) struct SchedulerTaskSuspendedHook {
    scheduler: Arc<crate::scheduler::Scheduler>,
}

impl SchedulerTaskSuspendedHook {
    pub(crate) fn new(scheduler: Arc<crate::scheduler::Scheduler>) -> Self {
        Self { scheduler }
    }
}

#[async_trait]
impl AppPackageTaskHook for SchedulerTaskSuspendedHook {
    async fn suspend_for_package(&self, package: &AppPackageId) -> anyhow::Result<usize> {
        self.scheduler
            .on_app_package_uninstalled(package.as_str())
            .await
    }
}

/// 包内 `presets/*.yaml` 的 schedule 以 `{kind, value}` 声明（包格式契约）；
/// 在包存储 → Timer Core 边界翻译为 `{provider_id, config}`（ADR-12）。
fn package_schedule(value: &serde_json::Value) -> anyhow::Result<crate::timer_core::TaskSchedule> {
    let kind = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("schedule.kind 必须是非空字符串"))?;
    crate::timer_core::TaskSchedule::new(
        kind,
        value
            .get("value")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )
}

/// Production preset-publish adapter over Timer Core. The package store only
/// sees the narrow [`AppPackagePresetHook`] seam, never Timer Core internals.
pub(crate) struct TimerPresetPublishHook {
    timer: Arc<crate::timer_core::TimerCore>,
}

impl TimerPresetPublishHook {
    pub(crate) fn new(timer: Arc<crate::timer_core::TimerCore>) -> Self {
        Self { timer }
    }
}

#[async_trait]
impl AppPackagePresetHook for TimerPresetPublishHook {
    async fn publish_presets(
        &self,
        package: &AppPackageId,
        presets: &[PresetDeclaration],
    ) -> anyhow::Result<usize> {
        let converted = presets
            .iter()
            .map(|preset| {
                Ok(crate::timer_core::PackagePreset {
                    name: preset.name.clone(),
                    runner_id: preset.runner_id.clone(),
                    entrypoint: preset.entrypoint.clone(),
                    payload: preset.payload.clone(),
                    schedule: package_schedule(&preset.schedule)?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        self.timer
            .publish_package_presets(package, &converted)
            .await
    }
}

/// Per-version install metadata persisted next to `manifest.toml`
/// (`install.json`): the archive digest recorded at install time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InstallMeta {
    pub(crate) sha256: String,
    pub(crate) installed_at: String,
}

/// Active-version registry (`app-packages/active.json`): AppPackageId → the
/// version users of that package resolve against. One active version per App
/// Package; the primary-uniqueness rule (one active content package per
/// Android package) is enforced on install/activate.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ActiveRegistry(BTreeMap<String, String>);

impl ActiveRegistry {
    pub(crate) fn load(data_root: &Path) -> AppPackageResult<Self> {
        let path = data_root.join("app-packages").join("active.json");
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(error.into()),
        };
        if bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
            return Ok(Self::default());
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| AppPackageError::InvalidManifest(format!("active.json: {error}")))
    }

    pub(crate) fn save(&self, data_root: &Path) -> AppPackageResult<()> {
        let path = data_root.join("app-packages").join("active.json");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| AppPackageError::InvalidManifest(format!("active.json: {error}")))?;
        atomic_write(&path, &bytes)
            .map_err(|error| AppPackageError::Io(std::io::Error::other(error.to_string())))
    }

    pub(crate) fn get(&self, package: &AppPackageId) -> Option<&str> {
        self.0.get(package.as_str()).map(String::as_str)
    }

    pub(crate) fn remove(&mut self, package: &AppPackageId) -> Option<String> {
        self.0.remove(package.as_str())
    }

    pub(crate) fn insert(&mut self, package: AppPackageId, version: InstalledVersion) {
        self.0.insert(package.into_string(), version.into_string());
    }

    /// Iterate `(package_id, version)` pairs in deterministic id order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(id, version)| (id.as_str(), version.as_str()))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<(String, String)> for ActiveRegistry {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(items: I) -> Self {
        Self(items.into_iter().collect())
    }
}

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
#[derive(Clone)]
pub(crate) struct AppPackageStore {
    data_root: PathBuf,
    task_hook: Arc<dyn AppPackageTaskHook>,
    preset_hook: Arc<dyn AppPackagePresetHook>,
}

impl fmt::Debug for AppPackageStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppPackageStore")
            .field("data_root", &self.data_root)
            .finish_non_exhaustive()
    }
}

impl AppPackageStore {
    pub(crate) fn new(data_root: impl AsRef<Path>) -> Self {
        Self::with_task_hook(data_root, Arc::new(NoopAppPackageTaskHook))
    }

    pub(crate) fn with_task_hook(
        data_root: impl AsRef<Path>,
        task_hook: Arc<dyn AppPackageTaskHook>,
    ) -> Self {
        Self::with_hooks(data_root, task_hook, Arc::new(NoopAppPackagePresetHook))
    }

    pub(crate) fn with_hooks(
        data_root: impl AsRef<Path>,
        task_hook: Arc<dyn AppPackageTaskHook>,
        preset_hook: Arc<dyn AppPackagePresetHook>,
    ) -> Self {
        Self {
            data_root: data_root.as_ref().to_path_buf(),
            task_hook,
            preset_hook,
        }
    }

    pub(crate) fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub(crate) fn resolver(&self) -> ResourceResolver {
        ResourceResolver::new(self)
    }

    /// 列出全部已安装版本。单个版本目录损坏（manifest 缺失/解析失败、目录名与
    /// manifest 不一致、非法版本目录名）只 `tracing::warn` 并跳过，不影响其余
    /// 版本列出；uninstall 按目录名删除、无需 manifest 解析，仍可清理这类目录。
    pub(crate) fn list_installed(&self) -> AppPackageResult<Vec<InstalledPackage>> {
        let root = self.app_packages_root();
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut installed = Vec::new();
        for package_entry in entries {
            let package_entry = match package_entry {
                Ok(entry) => entry,
                Err(error) => {
                    tracing::warn!(error = %error, "app-packages 目录项读取失败，已跳过");
                    continue;
                }
            };
            if !package_entry.file_type()?.is_dir() || package_entry.file_name() == ".staging" {
                continue;
            }
            let package_name = package_entry.file_name().to_string_lossy().to_string();
            let Ok(package_id) = parse_app_package_id(&package_name) else {
                tracing::warn!(directory = %package_name, "app-packages 下存在非法包目录，已跳过");
                continue;
            };
            let versions = match fs::read_dir(package_entry.path()) {
                Ok(versions) => versions,
                Err(error) => {
                    tracing::warn!(directory = %package_name, error = %error, "版本目录读取失败，已跳过");
                    continue;
                }
            };
            for version_entry in versions {
                let version_entry = match version_entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        tracing::warn!(directory = %package_name, error = %error, "版本目录项读取失败，已跳过");
                        continue;
                    }
                };
                if !version_entry.file_type()?.is_dir() {
                    continue;
                }
                let version_dir = version_entry.file_name().to_string_lossy().to_string();
                let skipped = |reason: String| {
                    tracing::warn!(
                        directory = %format!("{package_name}/{version_dir}"),
                        reason,
                        "损坏的已安装包版本目录已跳过"
                    );
                };
                let Ok(version) = InstalledVersion::parse(&version_dir) else {
                    skipped("非法版本目录名".to_string());
                    continue;
                };
                let manifest_bytes = match fs::read(version_entry.path().join("manifest.toml")) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        skipped(format!("manifest.toml 读取失败: {error}"));
                        continue;
                    }
                };
                let manifest = match parse_manifest(&manifest_bytes) {
                    Ok(manifest) => manifest,
                    Err(error) => {
                        skipped(format!("manifest 解析失败: {error}"));
                        continue;
                    }
                };
                if manifest.id() != &package_id || manifest.version() != &version {
                    skipped("目录与 manifest 不一致".to_string());
                    continue;
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

    /// Install one validated archive into a version directory. Re-installing
    /// the same `package_id + version` overwrites the existing directory
    /// (stage-then-swap; plan §13.5's simple rule — no historical-version
    /// migration); other installed versions are never touched. The archive
    /// SHA-256 is computed here and persisted next to the manifest
    /// (`install.json`); when `expected_sha256` is provided, a mismatch
    /// aborts the install before anything is staged.
    pub(crate) fn install_archive(
        &self,
        archive: &[u8],
        expected_sha256: Option<&str>,
    ) -> AppPackageResult<InstalledPackage> {
        let digest = archive_sha256(archive);
        verify_expected_sha256(expected_sha256, &digest)?;
        let manifest_bytes = validate_and_read_manifest(archive)?;
        let manifest = parse_manifest(&manifest_bytes)?;
        let final_root = self
            .app_packages_root()
            .join(manifest.id().as_str())
            .join(manifest.version().as_str());

        let staging_parent = self.app_packages_root().join(".staging");
        fs::create_dir_all(&staging_parent)?;
        let staging = create_staging_directory(&staging_parent, manifest.id(), manifest.version())?;
        let result = (|| -> AppPackageResult<InstalledPackage> {
            extract_archive(archive, &staging)?;
            let meta = InstallMeta {
                sha256: digest.clone(),
                installed_at: chrono::Utc::now()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            };
            let meta_bytes = serde_json::to_vec_pretty(&meta).map_err(|error| {
                AppPackageError::InvalidManifest(format!("install.json: {error}"))
            })?;
            atomic_write(&staging.join("install.json"), &meta_bytes)
                .map_err(|error| AppPackageError::Io(std::io::Error::other(error.to_string())))?;
            // Overwrite（同 id+version）：先整目录 rename 走既有版本（同卷
            // rename 原子），staging 内容再就位；就位失败把旧目录移回，成功
            // 后删除旧目录。首次安装则直接就位。
            let retired = if path_exists(&final_root)? {
                let retired = staging_parent.join(format!(
                    "{}-{}-{}.retired",
                    manifest.id(),
                    manifest.version(),
                    Uuid::new_v4().simple()
                ));
                fs::rename(&final_root, &retired)?;
                Some(retired)
            } else {
                None
            };
            if let Err(error) = (|| -> std::io::Result<()> {
                if let Some(parent) = final_root.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::rename(&staging, &final_root)
            })() {
                if let Some(retired) = &retired {
                    let _ = fs::rename(retired, &final_root);
                }
                return Err(AppPackageError::Io(error));
            }
            if let Some(retired) = &retired {
                let _ = fs::remove_dir_all(retired);
            }
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

    /// REST install path: verify the optional digest expectation, enforce the
    /// primary-uniqueness rule against currently active packages, install, then
    /// activate the new version (publishing its task presets). The primary
    /// check runs before staging so a conflicting archive never lands on disk.
    pub(crate) async fn install_and_activate(
        &self,
        archive: &[u8],
        expected_sha256: Option<&str>,
    ) -> AppPackageResult<InstalledPackage> {
        let digest = archive_sha256(archive);
        verify_expected_sha256(expected_sha256, &digest)?;
        let manifest = parse_manifest(&validate_and_read_manifest(archive)?)?;
        self.ensure_primary_available(&manifest)?;
        // Overwrite 语义下重装不会新建目录：激活失败只回滚「本次新装」的版本，
        // 不得连带删除重装前就存在的版本目录。
        let created_new = !path_exists(
            &self
                .app_packages_root()
                .join(manifest.id().as_str())
                .join(manifest.version().as_str()),
        )?;
        let installed = self.install_archive(archive, Some(&digest))?;
        if let Err(error) = self
            .activate(installed.manifest().id(), installed.manifest().version())
            .await
        {
            // Roll the just-staged version back so an activation failure never
            // leaves an installed-but-never-activatable package behind.
            if created_new {
                let _ =
                    self.remove_version(installed.manifest().id(), installed.manifest().version());
            }
            return Err(error);
        }
        Ok(installed)
    }

    /// Point the active version of one App Package at an installed version and
    /// (re)publish its bundled task presets. Activation enforces the
    /// primary-uniqueness rule: an Android package may have only one active
    /// content package.
    pub(crate) async fn activate(
        &self,
        package: &AppPackageId,
        version: &InstalledVersion,
    ) -> AppPackageResult<InstalledPackage> {
        let package = parse_app_package_id(package.as_str())?;
        let version = InstalledVersion::parse(version.as_str())?;
        let installed = self
            .list_installed()?
            .into_iter()
            .find(|installed| {
                installed.manifest().id() == &package && installed.manifest().version() == &version
            })
            .ok_or_else(|| AppPackageError::NotInstalled {
                package: package.to_string(),
                version: version.to_string(),
            })?;
        self.ensure_primary_available(installed.manifest())?;

        let presets = super::presets::read_package_presets(installed.root())?;
        if !presets.is_empty() {
            self.preset_hook
                .publish_presets(&package, &presets)
                .await
                .map_err(|error| AppPackageError::PresetHook(error.to_string()))?;
        }

        let mut registry = ActiveRegistry::load(&self.data_root)?;
        registry.insert(package, version);
        registry.save(&self.data_root)?;
        Ok(installed)
    }

    /// The Android targets of `incoming` must not intersect with the targets
    /// of any *other* currently active App Package.
    fn ensure_primary_available(&self, incoming: &PackageManifest) -> AppPackageResult<()> {
        let registry = ActiveRegistry::load(&self.data_root)?;
        for (active_id, active_version) in registry.iter() {
            if active_id == incoming.id().as_str() {
                continue;
            }
            let Ok(active_id) = parse_app_package_id(active_id) else {
                continue;
            };
            let Ok(active_version) = InstalledVersion::parse(active_version) else {
                continue;
            };
            let active_root = self
                .app_packages_root()
                .join(active_id.as_str())
                .join(active_version.as_str());
            let Ok(bytes) = fs::read(active_root.join("manifest.toml")) else {
                continue;
            };
            let Ok(active_manifest) = parse_manifest(&bytes) else {
                continue;
            };
            if active_manifest.id() != &active_id || active_manifest.version() != &active_version {
                continue;
            }
            for android in active_manifest.android_packages() {
                if incoming.supports_android_package(android) {
                    return Err(AppPackageError::PrimaryConflict {
                        android: android.to_string(),
                        active_package: active_id.to_string(),
                        active_version: active_version.to_string(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Active version of one App Package, if any.
    pub(crate) fn active_version(
        &self,
        package: &AppPackageId,
    ) -> AppPackageResult<Option<InstalledVersion>> {
        let registry = ActiveRegistry::load(&self.data_root)?;
        registry
            .get(package)
            .map(InstalledVersion::parse)
            .transpose()
    }

    /// Persisted install metadata (archive digest) of one version.
    pub(crate) fn install_meta(
        &self,
        package: &AppPackageId,
        version: &InstalledVersion,
    ) -> AppPackageResult<Option<InstallMeta>> {
        let path = self
            .app_packages_root()
            .join(package.as_str())
            .join(version.as_str())
            .join("install.json");
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| AppPackageError::InvalidManifest(format!("install.json: {error}")))
    }

    /// Remove one installed version (same-id+version reinstall in
    /// [`Self::install_archive`] is the only overwrite path), then notify the
    /// single task lifecycle hook if this was the last version of the App
    /// Package. The async API is
    /// intentional: no caller can silently forget the Suspended transition.
    /// User overrides and unrelated data are deliberately outside this path
    /// and remain untouched. A removed active version drops out of the active
    /// registry (task preset rows deliberately stay persisted; a later
    /// activate republishes them).
    pub(crate) async fn uninstall(
        &self,
        package: &AppPackageId,
        version: &InstalledVersion,
    ) -> AppPackageResult<bool> {
        let package = parse_app_package_id(package.as_str())?;
        let removed = self.remove_version(&package, version)?;
        if !removed {
            return Ok(false);
        }
        let mut registry = ActiveRegistry::load(&self.data_root)?;
        let was_active = registry.remove(&package).is_some();
        if was_active {
            registry.save(&self.data_root)?;
        }
        let package_still_installed = self
            .list_installed()?
            .iter()
            .any(|installed| installed.manifest().id() == &package);
        if !package_still_installed {
            self.task_hook
                .suspend_for_package(&package)
                .await
                .map_err(|error| AppPackageError::TaskHook(error.to_string()))?;
        }
        Ok(true)
    }

    fn remove_version(
        &self,
        package: &AppPackageId,
        version: &InstalledVersion,
    ) -> AppPackageResult<bool> {
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

pub(crate) fn sync_directory(path: &Path) -> std::io::Result<()> {
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

fn archive_sha256(archive: &[u8]) -> String {
    format!("{:x}", Sha256::digest(archive))
}

/// Optional install-time integrity gate: a present expectation must be a
/// 64-hex digest and must match the computed archive digest.
fn verify_expected_sha256(expected: Option<&str>, digest: &str) -> AppPackageResult<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let expected = expected.trim().to_ascii_lowercase();
    let valid = expected.len() == 64 && expected.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !valid || expected != digest {
        return Err(AppPackageError::Sha256Mismatch {
            expected,
            actual: digest.to_string(),
        });
    }
    Ok(())
}
