//! LCH-007：依赖修复编排。
//!
//! inventory 深检 → 对缺失/损坏组件按 seed → cache → remote 获取产物 →
//! staging 安全解压 + 逐文件复验 → 旧目录 quarantine → 原子 rename 换装 →
//! 复验探针。任一步失败：staging 整体放弃删除；已 quarantine 的旧目录 rename
//! 回原位，**保持上一份完好 runtime 不被破坏**（UPDATE_CONTRACT §5.1）。
//! 单实例锁在入口获取（并发 repair 只有一个执行者）。

use std::fs;
use std::path::PathBuf;

use crate::archive::{self, ExtractOptions};
use crate::fetch::{self, FetchOptions, Obtained};
use crate::inventory::{self, CheckOptions, ComponentFinding, ComponentSpec};
use crate::layout::InstallLayout;
use crate::manifest::model::RequiredFile;
use crate::state::atomic::{now_unix_millis, rename_with_retry};
use crate::state::lock::{InstanceLock, LockError};

#[derive(Debug, Clone, Default)]
pub struct RepairOptions {
    pub fetch: FetchOptions,
    /// 修复后复验附版本探针（adb/ffmpeg）。
    pub probe: bool,
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
}

impl RepairReport {
    pub fn failed_count(&self) -> usize {
        self.components
            .iter()
            .filter(|c| matches!(c.outcome, ComponentOutcome::Failed { .. }))
            .count()
    }

    pub fn repaired_count(&self) -> usize {
        self.components
            .iter()
            .filter(|c| matches!(c.outcome, ComponentOutcome::Repaired { .. }))
            .count()
    }
}

/// 修复总入口：获取单实例锁后执行。
pub fn repair_with_lock(
    layout: &InstallLayout,
    specs: &[ComponentSpec],
    opts: &RepairOptions,
) -> Result<RepairReport, RepairGate> {
    let lock = InstanceLock::acquire(&layout.state_dir()).map_err(|e| match e {
        LockError::Held { path } => RepairGate::Locked { path },
        LockError::Io(e) => RepairGate::Io(e),
    })?;
    tracing::debug!(lock = %lock.path().display(), "repair 持有单实例锁");
    Ok(repair_components(layout, specs, opts))
}

/// 不取锁的修复编排（tests 可直接调用；CLI 走 `repair_with_lock`）。
pub fn repair_components(
    layout: &InstallLayout,
    specs: &[ComponentSpec],
    opts: &RepairOptions,
) -> RepairReport {
    let mut out = Vec::new();
    for spec in specs {
        out.push(repair_one(layout, spec, opts));
    }
    RepairReport { components: out }
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

    // 3) 原子换装：旧目录先移入 quarantine，再 rename 新目录；第二步失败则移回
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
    if let Err(e) = rename_with_retry(&staging, &dir) {
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
