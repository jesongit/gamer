//! LCH-007：依赖修复编排。
//!
//! inventory 深检 → 对缺失/损坏组件按 seed → cache → remote 获取产物 →
//! staging 安全解压 + 逐文件复验 → 旧目录 quarantine → 原子 rename 换装 →
//! 复验探针。任一步失败：staging 整体放弃删除；已 quarantine 的旧目录 rename
//! 回原位，**保持上一份完好 runtime 不被破坏**（UPDATE_CONTRACT §5.1）。
//! 单实例锁在入口获取（并发 repair 只有一个执行者）。

use std::fs;
use std::path::{Path, PathBuf};

use crate::archive::{self, ExtractOptions};
use crate::digest::sha256_file_hex;
use crate::fetch::{self, FetchOptions, Obtained};
use crate::inventory::{self, CheckOptions, ComponentFinding, ComponentSpec};
use crate::layout::InstallLayout;
use crate::manifest::model::{Platform, RequiredFile};
use crate::state::atomic::{now_unix_millis, rename_with_retry, LoadOutcome};
use crate::state::lock::{InstanceLock, LockError};
use crate::state::{CurrentState, StateStore};

#[derive(Debug, Clone, Default)]
pub struct RepairOptions {
    pub fetch: FetchOptions,
    /// 修复后复验附版本探针（adb/ffmpeg）。
    pub probe: bool,
}

/// 应用组件安装规格（来自已验签 manifest `platforms.<plat>.app` +
/// `resources.scrcpy_server`；安装位 = `versions/<release.version>/`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInstallSpec {
    /// 应用版本（= release.version，versions/<semver>/ 目录名，须过路径安全检查）
    pub version: String,
    /// 入口程序相对版本目录的路径（如 gamer-server.exe）
    pub entrypoint: String,
    pub artifact_name: String,
    pub artifact_sha256: String,
    pub artifact_size: u64,
    pub artifact_url: String,
    /// scrcpy-server jar 相对版本目录的路径（如 assets/scrcpy-server.jar）
    pub scrcpy_path: String,
    pub scrcpy_sha256: String,
}

impl AppInstallSpec {
    /// 从已验签 manifest 模型构建（version 用作目录名、entrypoint/path 用作拼接，
    /// 全部先过 manifest 同源路径安全检查，防目录逃逸）。
    pub fn from_model(platform: &Platform, release_version: &str) -> Result<Self, String> {
        if let Some(reason) = crate::manifest::pathsafe::check_single_path(release_version) {
            return Err(format!(
                "release.version {release_version:?} 不能用作目录名（{reason}）"
            ));
        }
        for (label, path) in [
            ("app.entrypoint", &platform.app.entrypoint),
            (
                "resources.scrcpy_server.path",
                &platform.resources.scrcpy_server.path,
            ),
        ] {
            if let Some(reason) = crate::manifest::pathsafe::check_single_path(path) {
                return Err(format!("{label} {path:?} 非法（{reason}）"));
            }
        }
        let artifact = &platform.app.artifact;
        if artifact.name.contains('/') || artifact.name.contains('\\') {
            return Err(format!(
                "app.artifact.name {:?} 必须为单一文件名",
                artifact.name
            ));
        }
        Ok(Self {
            version: release_version.to_string(),
            entrypoint: platform.app.entrypoint.clone(),
            artifact_name: artifact.name.clone(),
            artifact_sha256: artifact.sha256.to_ascii_lowercase(),
            artifact_size: u64::try_from(artifact.size).unwrap_or(0),
            artifact_url: artifact.url.clone(),
            scrcpy_path: platform.resources.scrcpy_server.path.clone(),
            scrcpy_sha256: platform.resources.scrcpy_server.sha256.to_ascii_lowercase(),
        })
    }

    /// 安装位：versions/<version>/（契约 §1；安装成功后只读，不原地覆盖）。
    pub fn install_dir(&self, layout: &InstallLayout) -> PathBuf {
        layout.versions_dir().join(&self.version)
    }
}

/// 校验某目录（安装位或 staging）内的 app 组件：entrypoint 存在 +
/// scrcpy-server jar 存在且 sha256 与 manifest 一致。
pub fn verify_app_dir(dir: &Path, app: &AppInstallSpec) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("版本目录不存在: {}", dir.display()));
    }
    let exe = dir.join(&app.entrypoint);
    if !exe.is_file() {
        return Err(format!("入口程序缺失: {}", exe.display()));
    }
    let jar = dir.join(&app.scrcpy_path);
    if !jar.is_file() {
        return Err(format!("scrcpy-server 资源缺失: {}", jar.display()));
    }
    let actual = sha256_file_hex(&jar).map_err(|e| format!("scrcpy-server 读取失败: {e}"))?;
    if actual != app.scrcpy_sha256 {
        return Err(format!(
            "scrcpy-server sha256 不符（实际 {actual}，声明 {}）",
            app.scrcpy_sha256
        ));
    }
    Ok(())
}

/// 修复门禁错误（锁被其他实例持有时不得动安装目录）。
#[derive(Debug)]
pub enum RepairGate {
    Locked { path: PathBuf },
    Io(std::io::Error),
}

impl std::fmt::Display for RepairGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepairGate::Locked { path } => {
                write!(
                    f,
                    "安装根已被另一个 launcher 实例持有（{}）",
                    path.display()
                )
            }
            RepairGate::Io(e) => write!(f, "锁操作失败: {e}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentOutcome {
    /// 深检即通过，无需修复。
    Healthy,
    /// 已恢复（source：seed/cache/remote）。
    Repaired {
        source: String,
    },
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct ComponentRepair {
    pub id: String,
    pub version: String,
    pub outcome: ComponentOutcome,
}

#[derive(Debug, Clone)]
pub struct RepairReport {
    pub components: Vec<ComponentRepair>,
    /// app 组件（manifest 声明了 app 包时必有）。
    pub app: Option<AppRepair>,
}

impl RepairReport {
    pub fn failed_count(&self) -> usize {
        self.components
            .iter()
            .filter(|c| matches!(c.outcome, ComponentOutcome::Failed { .. }))
            .count()
            + usize::from(
                self.app
                    .as_ref()
                    .is_some_and(|a| matches!(a.outcome, AppOutcome::Failed { .. })),
            )
    }

    pub fn repaired_count(&self) -> usize {
        self.components
            .iter()
            .filter(|c| matches!(c.outcome, ComponentOutcome::Repaired { .. }))
            .count()
            + usize::from(
                self.app
                    .as_ref()
                    .is_some_and(|a| matches!(a.outcome, AppOutcome::Installed { .. })),
            )
    }
}

#[derive(Debug, Clone)]
pub struct AppRepair {
    pub version: String,
    pub outcome: AppOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppOutcome {
    /// versions/<v>/ 已存在且校验通过（版本目录不可变，不重装、不原地覆盖）。
    Healthy,
    /// 本次安装成功（source：seed/cache/remote），版本指针已写入/校正。
    Installed {
        source: String,
    },
    Failed {
        reason: String,
    },
}

/// 修复总入口：获取单实例锁后执行。
pub fn repair_with_lock(
    layout: &InstallLayout,
    specs: &[ComponentSpec],
    app: Option<&AppInstallSpec>,
    opts: &RepairOptions,
) -> Result<RepairReport, RepairGate> {
    let lock = InstanceLock::acquire(&layout.state_dir()).map_err(|e| match e {
        LockError::Held { path } => RepairGate::Locked { path },
        LockError::Io(e) => RepairGate::Io(e),
    })?;
    tracing::debug!(lock = %lock.path().display(), "repair 持有单实例锁");
    Ok(repair_components(layout, specs, app, opts))
}

/// 不取锁的修复编排（tests 可直接调用；CLI 走 `repair_with_lock`）。
pub fn repair_components(
    layout: &InstallLayout,
    specs: &[ComponentSpec],
    app: Option<&AppInstallSpec>,
    opts: &RepairOptions,
) -> RepairReport {
    let mut out = Vec::new();
    for spec in specs {
        out.push(repair_one(layout, spec, opts));
    }
    let app_repair = app.map(|a| repair_app(layout, a, opts));
    RepairReport {
        components: out,
        app: app_repair,
    }
}

/// 单组件修复（不取锁）：升级下载阶段对候选 manifest 的组件逐个换装复用。
pub fn repair_component(
    layout: &InstallLayout,
    spec: &ComponentSpec,
    opts: &RepairOptions,
) -> ComponentRepair {
    repair_one(layout, spec, opts)
}

fn repair_one(
    layout: &InstallLayout,
    spec: &ComponentSpec,
    opts: &RepairOptions,
) -> ComponentRepair {
    let dir = spec.install_dir(layout);
    let finding = inventory::check_component(
        &dir,
        spec,
        CheckOptions {
            deep: true,
            probe: opts.probe,
        },
    );
    if finding.status == inventory::ComponentStatus::Ok {
        return ComponentRepair {
            id: spec.id.clone(),
            version: spec.version.clone(),
            outcome: ComponentOutcome::Healthy,
        };
    }
    tracing::warn!(
        id = %spec.id,
        version = %spec.version,
        broken = ?broken_summary(&finding),
        "组件缺失/损坏，开始修复"
    );

    // 1) 获取产物（seed → cache → remote；全部过 sha256+size 校验）
    let artifact = match fetch::obtain_artifact(
        layout,
        &spec.artifact_name,
        &spec.artifact_sha256,
        spec.artifact_size,
        Some(&spec.artifact_url),
        &opts.fetch,
    ) {
        Ok(a) => a,
        Err(e) => {
            return ComponentRepair {
                id: spec.id.clone(),
                version: spec.version.clone(),
                outcome: ComponentOutcome::Failed {
                    reason: format!("获取组件产物失败: {e}"),
                },
            };
        }
    };

    // 2) 安全解压到 staging 并复验（extract 内部已逐文件校验，这里双保险）
    let staging = layout.staging_dir().join(format!(
        "repair-{}-{}-{}",
        spec.id,
        spec.version,
        now_unix_millis()
    ));
    let staged = (|| -> Result<(), String> {
        archive::extract_component_zip(
            artifact.path(),
            &staging,
            &required_of(spec),
            &extract_limits(spec),
        )
        .map_err(|e| e.to_string())?;
        let recheck = inventory::check_component(
            &staging,
            spec,
            CheckOptions {
                deep: true,
                probe: false,
            },
        );
        if recheck.status != inventory::ComponentStatus::Ok {
            return Err(format!("staging 复验未通过: {}", broken_summary(&recheck)));
        }
        Ok(())
    })();
    if let Err(reason) = staged {
        cleanup_dir(&staging);
        return ComponentRepair {
            id: spec.id.clone(),
            version: spec.version.clone(),
            outcome: ComponentOutcome::Failed {
                reason: format!("staging 安装失败（放弃并清理）: {reason}"),
            },
        };
    }

    // 3) 原子换装：先确保目标父目录存在（首装时 runtime/<id>/ 尚不存在，
    //    fs::rename 不建父目录——曾致全新安装根 repair 必败 os error 3）；
    //    旧目录先移入 quarantine，再 rename 新目录；第二步失败则移回
    let mut quarantined: Option<PathBuf> = None;
    if dir.exists() {
        let q = layout.quarantine_dir().join(format!(
            "{}-{}-{}",
            spec.id,
            spec.version,
            now_unix_millis()
        ));
        if let Err(e) =
            fs::create_dir_all(layout.quarantine_dir()).and_then(|_| rename_with_retry(&dir, &q))
        {
            cleanup_dir(&staging);
            return ComponentRepair {
                id: spec.id.clone(),
                version: spec.version.clone(),
                outcome: ComponentOutcome::Failed {
                    reason: format!("旧组件目录移入 quarantine 失败（旧目录保持原位）: {e}"),
                },
            };
        }
        quarantined = Some(q);
    }
    let parent = dir.parent().map(Path::to_path_buf);
    if let Err(e) = match parent {
        Some(p) => fs::create_dir_all(&p).and_then(|_| rename_with_retry(&staging, &dir)),
        None => rename_with_retry(&staging, &dir),
    } {
        // 换装失败：旧目录移回，保持上一份 runtime 原样；staging 放弃删除
        let moved_back = quarantined
            .as_ref()
            .map(|q| rename_with_retry(q, &dir).is_ok())
            .unwrap_or(true);
        if !moved_back {
            tracing::error!(
                quarantine = ?quarantined,
                target = %dir.display(),
                "旧目录移回失败，需要人工恢复（quarantine 内保留原目录）"
            );
        }
        cleanup_dir(&staging);
        return ComponentRepair {
            id: spec.id.clone(),
            version: spec.version.clone(),
            outcome: ComponentOutcome::Failed {
                reason: format!(
                    "新组件目录 rename 到位失败: {e}（{}）",
                    if moved_back {
                        "旧目录已恢复原位"
                    } else {
                        "旧目录在 quarantine，需人工恢复"
                    }
                ),
            },
        };
    }

    // 4) 复验（hash + 可选探针）；quarantine 按契约保留，不静默删除
    let recheck = inventory::check_component(
        &dir,
        spec,
        CheckOptions {
            deep: true,
            probe: opts.probe,
        },
    );
    if recheck.status == inventory::ComponentStatus::Ok {
        if let Some(q) = &quarantined {
            tracing::info!(quarantine = %q.display(), "损坏旧目录已保留在 quarantine");
        }
        ComponentRepair {
            id: spec.id.clone(),
            version: spec.version.clone(),
            outcome: ComponentOutcome::Repaired {
                source: artifact.source_label().to_string(),
            },
        }
    } else {
        ComponentRepair {
            id: spec.id.clone(),
            version: spec.version.clone(),
            outcome: ComponentOutcome::Failed {
                reason: format!("修复后复验失败: {}", broken_summary(&recheck)),
            },
        }
    }
}

/// app 组件修复/首装编排：versions/<v>/ 缺失或损坏 → 获取 app 包 → 安全解压
/// staging → 校验 entrypoint + scrcpy jar → 原子换装（损坏旧目录 quarantine，
/// 失败移回）→ 复验 → 写 `state/current.json` 版本指针。契约 §2：版本目录
/// 安装成功后不可变——已存在且完好时返回 Healthy，绝不原地覆盖。
fn repair_app(layout: &InstallLayout, app: &AppInstallSpec, opts: &RepairOptions) -> AppRepair {
    let failed = |reason: String| AppRepair {
        version: app.version.clone(),
        outcome: AppOutcome::Failed { reason },
    };
    let dir = app.install_dir(layout);
    if dir.is_dir() {
        if verify_app_dir(&dir, app).is_ok() {
            tracing::info!(version = %app.version, "app 版本目录已安装且校验通过");
            if let Err(e) = ensure_current_pointer(layout, &app.version) {
                return failed(e);
            }
            return AppRepair {
                version: app.version.clone(),
                outcome: AppOutcome::Healthy,
            };
        }
        tracing::warn!(version = %app.version, "app 版本目录损坏，开始重装");
    }

    // 1) 获取产物（seed → cache → remote；sha256+size 校验）
    let artifact = match fetch::obtain_artifact(
        layout,
        &app.artifact_name,
        &app.artifact_sha256,
        app.artifact_size,
        Some(&app.artifact_url),
        &opts.fetch,
    ) {
        Ok(a) => a,
        Err(e) => return failed(format!("获取应用产物失败: {e}")),
    };

    // 2) 安全解压到 staging 并校验（app 包内容随构建变化，无逐文件白名单；
    //    完整性由产物整体 sha256 + 解压防线 + entrypoint/jar 校验锚定）
    let staging =
        layout
            .staging_dir()
            .join(format!("repair-app-{}-{}", app.version, now_unix_millis()));
    let staged = (|| -> Result<(), String> {
        archive::extract_app_zip(artifact.path(), &staging, &ExtractOptions::default())
            .map_err(|e| e.to_string())?;
        verify_app_dir(&staging, app)
    })();
    if let Err(reason) = staged {
        cleanup_dir(&staging);
        return failed(format!("app staging 安装失败（放弃并清理）: {reason}"));
    }

    // 3) 原子换装：损坏旧目录先 quarantine；versions/ 父目录首装时可能不存在
    let mut quarantined: Option<PathBuf> = None;
    if dir.exists() {
        let q = layout
            .quarantine_dir()
            .join(format!("app-{}-{}", app.version, now_unix_millis()));
        if let Err(e) =
            fs::create_dir_all(layout.quarantine_dir()).and_then(|_| rename_with_retry(&dir, &q))
        {
            cleanup_dir(&staging);
            return failed(format!(
                "旧版本目录移入 quarantine 失败（旧目录保持原位）: {e}"
            ));
        }
        quarantined = Some(q);
    }
    if let Err(e) =
        fs::create_dir_all(layout.versions_dir()).and_then(|_| rename_with_retry(&staging, &dir))
    {
        let moved_back = quarantined
            .as_ref()
            .map(|q| rename_with_retry(q, &dir).is_ok())
            .unwrap_or(true);
        if !moved_back {
            tracing::error!(
                quarantine = ?quarantined,
                target = %dir.display(),
                "旧版本目录移回失败，需要人工恢复（quarantine 内保留原目录）"
            );
        }
        cleanup_dir(&staging);
        return failed(format!(
            "新版本目录 rename 到位失败: {e}（{}）",
            if moved_back {
                "旧目录已恢复原位"
            } else {
                "旧目录在 quarantine，需人工恢复"
            }
        ));
    }

    // 4) 复验 + 写版本指针（原子写）
    if let Err(reason) = verify_app_dir(&dir, app) {
        return failed(format!("app 安装后复验失败: {reason}"));
    }
    if let Err(e) = ensure_current_pointer(layout, &app.version) {
        return failed(e);
    }
    if let Some(q) = &quarantined {
        tracing::info!(quarantine = %q.display(), "损坏旧版本目录已保留在 quarantine");
    }
    AppRepair {
        version: app.version.clone(),
        outcome: AppOutcome::Installed {
            source: artifact.source_label().to_string(),
        },
    }
}

/// 安装成功后确保 `state/current.json` 指向刚装好的版本（原子写；已指向则不动）。
fn ensure_current_pointer(layout: &InstallLayout, version: &str) -> Result<(), String> {
    let store = StateStore::new(&layout.root);
    match store.load_current() {
        Ok(LoadOutcome::Present(c)) if c.current == version => Ok(()),
        Ok(LoadOutcome::Present(c)) => store
            .write_current(&CurrentState::new(version, Some(c.current)))
            .map_err(|e| format!("写入 state/current.json 失败: {e}")),
        Ok(LoadOutcome::Missing) => store
            .write_current(&CurrentState::new(version, None))
            .map_err(|e| format!("写入 state/current.json 失败: {e}")),
        Ok(LoadOutcome::Corrupted { backup_path }) => {
            tracing::warn!(
                backup = %backup_path.display(),
                "state/current.json 损坏已备份，按首装重建版本指针"
            );
            store
                .write_current(&CurrentState::new(version, None))
                .map_err(|e| format!("写入 state/current.json 失败: {e}"))
        }
        Err(e) => Err(format!("读取 state/current.json 失败: {e}")),
    }
}

fn required_of(spec: &ComponentSpec) -> Vec<RequiredFile> {
    spec.files
        .iter()
        .map(|f| RequiredFile {
            path: f.path.clone(),
            size: i64::try_from(f.size).unwrap_or(i64::MAX),
            sha256: f.sha256.clone(),
        })
        .collect()
}

fn extract_limits(spec: &ComponentSpec) -> ExtractOptions {
    // 白名单外条目被拒绝、目录条目不计字节，故解压总量恰为 required_files 声明总和
    let total = spec.total_declared_bytes().max(1);
    ExtractOptions {
        max_total_uncompressed: total,
        max_file_uncompressed: total,
    }
}

fn broken_summary(finding: &ComponentFinding) -> String {
    let files: Vec<String> = finding
        .files
        .iter()
        .filter(|f| f.check != inventory::FileCheck::Ok)
        .map(|f| match &f.check {
            inventory::FileCheck::Missing => format!("{}: 缺失", f.path),
            inventory::FileCheck::SizeMismatch { actual, expected } => {
                format!("{f_path}: size {actual}≠{expected}", f_path = f.path)
            }
            inventory::FileCheck::HashMismatch { .. } => format!("{}: sha256 不符", f.path),
            inventory::FileCheck::Io(e) => format!("{}: {e}", f.path),
            inventory::FileCheck::Ok => unreachable!(),
        })
        .collect();
    let mut parts = files;
    match &finding.probe {
        Some(inventory::ProbeCheck::Match { reported }) => {
            parts.push(format!("探针: 匹配（{reported}）"));
        }
        Some(inventory::ProbeCheck::Mismatch { reported }) => {
            parts.push(format!("探针: 版本不符（{reported}）"));
        }
        Some(inventory::ProbeCheck::Failed { reason }) => {
            parts.push(format!("探针: {reason}"));
        }
        _ => {}
    }
    if parts.is_empty() {
        "（无明细）".to_string()
    } else {
        parts.join("; ")
    }
}

fn cleanup_dir(path: &PathBuf) {
    if path.exists() {
        let _ = fs::remove_dir_all(path);
    }
}

/// 取产物来源标签暴露给测试（seed/cache/remote）。
pub fn obtained_source(obtained: &Obtained) -> &'static str {
    obtained.source_label()
}
