//! App Package → 本地编辑区提取（`POST /api/app-packages/:id/:version/edit`）。
//!
//! 语义：把指定 immutable 已安装包整体提取为 `data/<android 包名>/` 工作区，
//! **不做 merge**——受管理条目（六个资源目录 + `package.toml`）整体替换，
//! 工作区其余兄弟文件/目录不动。提取后工作区层（EditableLocal）立即以最高
//! 优先级参与 composite 解析（user-overrides 与包内容保持原样，同名资源被
//! 编辑区遮蔽）。
//!
//! 流程（业务逻辑收在本模块，HTTP handler 只做参数解析与响应装配）：
//!
//! ```text
//! staging 提取（data/.edit-staging/<uuid>/，六目录 1:1 拷贝 + package.toml）
//! → Preflight（复用 PackageBuilder::validate_dir，与导出同源校验器）
//! → 原子替换（既有受管理条目先移入 data/.edit-backup/<uuid>/，再把 staging
//!   条目移入工作区；任一步失败从备份移回并删 staging，工作区保持原状）
//! → 模板目录缓存失效 + 清理 staging/备份
//! ```
//!
//! 明确不做的事：任务预设不发布（预设只来自包激活链路）；active 注册表、
//! 安装目录与 user-overrides 一概不动。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use uuid::Uuid;

use crate::matcher;
use crate::resources::ResourceStore;

use super::builder::{CollectedFile, PackageBuilder, RESOURCE_ROOTS};
use super::error::{AppPackageError, AppPackageResult};
use super::manifest::PackageManifest;
use super::model::{AndroidPackageName, ResourceKind};
use super::store::{sync_directory, InstalledPackage};
use super::workspace;

/// 受管理条目：六个资源目录 + 工作区元数据文件。提取替换只触碰这些名字，
/// 工作区根下的其他兄弟文件/目录保持原状。
const MANAGED_ENTRIES: [&str; 7] = [
    "scripts",
    "functions",
    "templates",
    "keymaps",
    "presets",
    "resources",
    workspace::WORKSPACE_METADATA_FILE,
];

/// 提取结果：包元数据（写入 package.toml 的那份 manifest）+ 提取文件计数。
pub(crate) struct EditOutcome {
    pub(crate) metadata: PackageManifest,
    /// 提取的文件数（与 workspace stats 口径一致：scripts/functions/keymaps
    /// 只数 .yaml/.yml，templates/presets/resources 数全部文件；隐藏文件跳过）。
    pub(crate) replaced: workspace::WorkspaceStats,
}

/// 把一个已安装包版本提取为本地编辑区。调用方负责先按 id/version 定位
/// [`InstalledPackage`]（未安装 → `NotInstalled`）。
pub(crate) fn extract_to_workspace(
    data_root: &Path,
    installed: &InstalledPackage,
    android: &AndroidPackageName,
    resources: Arc<ResourceStore>,
) -> AppPackageResult<EditOutcome> {
    let manifest = installed.manifest();
    if !manifest.supports_android_package(android) {
        return Err(AppPackageError::AndroidTargetNotSupported {
            android: android.as_str().to_string(),
            package: manifest.id().to_string(),
            version: manifest.version().to_string(),
            targets: manifest
                .android_packages()
                .iter()
                .map(|package| package.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        });
    }

    let workspace = workspace::workspace_dir(data_root, android);
    let staging_parent = data_root.join(".edit-staging");
    std::fs::create_dir_all(&staging_parent)?;
    let staging = staging_parent.join(Uuid::new_v4().simple().to_string());
    std::fs::create_dir(&staging)?;

    let result = (|| -> AppPackageResult<EditOutcome> {
        // 1) staging 提取：包内目录名与工作区已统一，六个资源目录 1:1 拷贝；
        //    package.toml 由 manifest 经固定字段序列化生成（字段全保留）
        for kind in RESOURCE_ROOTS {
            let source = installed.root().join(kind);
            if source.is_dir() {
                copy_dir_recursive(&source, &staging.join(kind))?;
            }
        }
        workspace::write_metadata(&staging, manifest)?;

        // 2) Preflight：与导出同源的校验器跑在 staging 目录上（收集全部问题）。
        //    PreflightFailed 原样上抛（400）；其余意外错误包一层提取语境（500）。
        let builder = PackageBuilder::new(data_root, android.clone(), resources);
        let files = builder
            .validate_dir(&staging)
            .map_err(|error| match error {
                AppPackageError::PreflightFailed { .. } => error,
                other => AppPackageError::PackageEditFailed(format!("Preflight 校验失败: {other}")),
            })?;
        let replaced = count_replaced(&files);

        // 3) 原子替换：既有受管理条目先备份，staging 条目再就位；失败整体回滚
        replace_managed_entries(data_root, &workspace, &staging)?;

        // 4) 分区 templates/ 的进程内模板缓存整体失效（keymap/脚本源码无缓存：
        //    KeymapStore 与运行快照均按需读盘，无需处理）
        matcher::invalidate_template_cache_dir(&workspace.join("templates"));
        // 替换已成功，目录 fsync 尽力而为（失败不回滚成功结果）
        let _ = sync_directory(&workspace);

        Ok(EditOutcome {
            metadata: manifest.clone(),
            replaced,
        })
    })();

    // staging 无论成败都清理：成功时条目已移走只剩空壳，失败时内容整体丢弃；
    // 父目录尽力删除（并发提取留有其他 staging 时自动失败，无害）
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir(&staging_parent);
    result
}

/// 受管理条目整体替换：工作区既有条目（存在者）先 rename 进
/// `data/.edit-backup/<uuid>/`，staging 条目再 rename 进工作区。任一步失败 →
/// 移除已就位的新条目、从备份逆序移回、清理备份目录，工作区保持原状。
fn replace_managed_entries(
    data_root: &Path,
    workspace: &Path,
    staging: &Path,
) -> AppPackageResult<()> {
    std::fs::create_dir_all(workspace)?;
    let backup_parent = data_root.join(".edit-backup");
    std::fs::create_dir_all(&backup_parent)?;
    let backup = backup_parent.join(Uuid::new_v4().simple().to_string());
    std::fs::create_dir(&backup)?;

    // (备份路径, 工作区原路径)，回滚时逆序移回
    let mut backed_up: Vec<(PathBuf, PathBuf)> = Vec::new();
    // 已从 staging 就位的工作区路径，回滚时先移除
    let mut placed: Vec<PathBuf> = Vec::new();
    let outcome = (|| -> AppPackageResult<()> {
        for entry in MANAGED_ENTRIES {
            let source = workspace.join(entry);
            if !path_exists(&source)? {
                continue;
            }
            let target = backup.join(entry);
            std::fs::rename(&source, &target)?;
            backed_up.push((target, source));
        }
        for entry in MANAGED_ENTRIES {
            let source = staging.join(entry);
            if !path_exists(&source)? {
                continue;
            }
            let target = workspace.join(entry);
            std::fs::rename(&source, &target)?;
            placed.push(target);
        }
        Ok(())
    })();

    if let Err(error) = outcome {
        // 回滚尽力而为（与旧分区快照导入链路同语义）；消息带原始失败原因
        for path in placed.iter().rev() {
            remove_entry(path);
        }
        for (target, source) in backed_up.iter().rev() {
            let _ = std::fs::rename(target, source);
        }
        let _ = std::fs::remove_dir_all(&backup);
        let _ = std::fs::remove_dir(&backup_parent);
        return Err(AppPackageError::PackageEditFailed(format!(
            "替换工作区条目失败，已回滚: {error}"
        )));
    }

    // 成功：旧条目备份不再需要；父目录尽力删除（并发提取留有其他备份时无害）
    let _ = std::fs::remove_dir_all(&backup);
    let _ = std::fs::remove_dir(&backup_parent);
    Ok(())
}

/// 递归复制目录（1:1，含隐藏文件；符号链接等非常规条目跳过，与 builder
/// 收集语义一致）。
fn copy_dir_recursive(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let entry_type = entry.file_type()?;
        let target = target.join(entry.file_name());
        if entry_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else if entry_type.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Preflight 收集结果 → replaced 计数（口径与 `workspace::compute_stats` 一致）。
fn count_replaced(files: &[CollectedFile]) -> workspace::WorkspaceStats {
    let mut stats = workspace::WorkspaceStats::default();
    for file in files {
        let name = file
            .path
            .as_str()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let is_yaml = name.ends_with(".yaml") || name.ends_with(".yml");
        match file.path.kind() {
            ResourceKind::Scripts if is_yaml => stats.scripts += 1,
            ResourceKind::Functions if is_yaml => stats.functions += 1,
            ResourceKind::Keymaps if is_yaml => stats.keymaps += 1,
            ResourceKind::Templates => stats.templates += 1,
            ResourceKind::Presets => stats.presets += 1,
            ResourceKind::Resources => stats.resources += 1,
            _ => {}
        }
    }
    stats
}

fn path_exists(path: &Path) -> AppPackageResult<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn remove_entry(path: &Path) {
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    } else {
        let _ = std::fs::remove_file(path);
    }
}
