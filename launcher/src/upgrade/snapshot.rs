//! LCH-011：升级前离线快照（data/ + config/config.toml）与同卷恢复。
//!
//! - 快照 = `backups/<update-id>/`：按 data/ 树 + config.toml 逐文件复制，
//!   `manifest.json` 记录相对路径 + size + sha256；写完后**整体验证一遍**
//!   （验证不全不进入 migrating，由调用方门禁）。
//! - SQLite 完整性：快照内存在 *.db 时用候选 exe 的 `inspect --data-dir <p> --json`
//!   校验副本（exit 0 + JSON 才算过）；候选 exe 缺失且有 db → 验证失败。
//!   恢复路径只做逐文件 hash 复验（字节级完整性在创建时已用候选 exe 门禁过）。
//! - 恢复 = 先验证快照 → 现 data/ 与 config.toml rename 到 quarantine/（不静默
//!   删除）→ 逐文件复制回 → 再验证。任何失败：保留 quarantine 证据并报错
//!   （调用方进 manual_recovery_required）。
//!
//! 前置条件（调用方保证）：server 进程已完整退出（Child::wait / drain），
//! 本模块不重复判活。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::digest::{sha256_file_hex, to_hex};
use crate::layout::InstallLayout;
use crate::state::atomic::{now_unix_millis, write_json_atomic};
use sha2::{Digest, Sha256};

pub const SNAPSHOT_MANIFEST: &str = "manifest.json";
const SCHEMA_VERSION: u32 = 1;

/// 快照清单（backups/<update-id>/manifest.json）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub schema_version: u32,
    pub update_id: String,
    pub created_at_unix_ms: u64,
    pub files: Vec<SnapshotFile>,
    pub file_count: u64,
    pub total_bytes: u64,
    /// 对“本字段为空”的规范 JSON（含末尾换行）计算的 SHA-256。
    #[serde(default)]
    pub manifest_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotFile {
    /// 相对安装根的路径，`/` 分隔（如 `data/gamer.db`）。
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// 快照结果（journal.snapshot 的 SnapshotInfo + schema 观察）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotReport {
    pub id: String,
    pub path: String,
    pub file_count: u64,
    pub total_bytes: u64,
    /// 候选 exe 对快照副本 inspect 出的 schema（不可用时 None）。
    pub schema_after: Option<u32>,
}

trait SchemaInspector {
    fn inspect(&self, exe: &Path, data_dir: &Path) -> Option<u32>;
}

struct NativeSchemaInspector;

impl SchemaInspector for NativeSchemaInspector {
    fn inspect(&self, exe: &Path, data_dir: &Path) -> Option<u32> {
        inspect_schema(exe, data_dir)
    }
}

pub fn backup_dir(layout: &InstallLayout, update_id: &str) -> PathBuf {
    layout.backups_dir().join(update_id)
}

fn sanitize_id(update_id: &str) -> Result<(), String> {
    let ok = !update_id.is_empty()
        && update_id.len() <= 128
        && update_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(format!("update_id 非法: {update_id:?}"))
    }
}

#[cfg(windows)]
const REPARSE_POINT: u32 = 0x0000_0400;

fn is_reparse_point(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        let _ = meta;
        false
    }
}

/// 读取目录项自身，绝不跟随 symlink/reparse point。
fn safe_metadata(path: &Path) -> Result<fs::Metadata, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("读取 {:?} 失败: {e}", path))?;
    if meta.file_type().is_symlink() || is_reparse_point(&meta) {
        return Err(format!("拒绝 symlink/reparse point: {}", path.display()));
    }
    Ok(meta)
}

/// 目录**遍历根**自身的元数据：允许根是 symlink/junction（QA-005 跨盘部署形态：
/// C: 安装根 + junction 形式的 data/ 指向另一块盘），但必须解析到目录；
/// 树**内部**的条目仍走 safe_metadata 逐项拒绝（防 zip-slip/链接攻击语义不变）。
fn dir_root_metadata(path: &Path) -> Result<fs::Metadata, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("读取 {:?} 失败: {e}", path))?;
    if meta.file_type().is_symlink() || is_reparse_point(&meta) {
        return fs::metadata(path).map_err(|e| format!("读取 {:?} 目标失败: {e}", path));
    }
    Ok(meta)
}

/// 递归收集目录下全部常规文件；遇到链接、特殊文件或读取竞态直接失败。
/// 只有 `base` 本身允许是 reparse point（见 dir_root_metadata）。
fn walk_files(base: &Path) -> Result<Vec<PathBuf>, String> {
    if !base.exists() {
        return Ok(Vec::new());
    }
    let base_meta = dir_root_metadata(base)?;
    if !base_meta.is_dir() {
        return Err(format!("路径不是目录: {}", base.display()));
    }
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| format!("遍历 {} 失败: {e}", dir.display()))?
        {
            let entry = entry.map_err(|e| format!("读取 {} 目录项失败: {e}", dir.display()))?;
            let path = entry.path();
            let meta = safe_metadata(&path)?;
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                out.push(path);
            } else {
                return Err(format!("拒绝特殊文件: {}", path.display()));
            }
        }
    }
    out.sort();
    Ok(out)
}

fn rel_forward(base: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(base).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

fn check_manifest_path(path: &str) -> Result<(), String> {
    if let Some(reason) = crate::manifest::pathsafe::check_single_path(path) {
        return Err(format!("路径不安全 {path:?}: {reason}"));
    }
    if path != "config/config.toml" && !path.starts_with("data/") {
        return Err(format!("快照路径不在允许范围: {path:?}"));
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|b| b.is_ascii_hexdigit())
        && value.bytes().all(|b| !b.is_ascii_uppercase())
}

fn manifest_digest(manifest: &SnapshotManifest) -> Result<String, String> {
    let mut unsigned = manifest.clone();
    unsigned.manifest_sha256.clear();
    let mut bytes =
        serde_json::to_vec_pretty(&unsigned).map_err(|e| format!("清单序列化失败: {e}"))?;
    bytes.push(b'\n');
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(to_hex(&hasher.finalize()))
}

fn validate_manifest_shape(manifest: &SnapshotManifest, update_id: &str) -> Result<(), String> {
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "不支持的快照清单 schema: {}",
            manifest.schema_version
        ));
    }
    if manifest.update_id != update_id {
        return Err("快照清单 update_id 与事务不符".to_string());
    }
    if !valid_sha256(&manifest.manifest_sha256) {
        return Err("快照清单自身 sha256 缺失或格式非法".to_string());
    }
    if manifest.manifest_sha256 != manifest_digest(manifest)? {
        return Err("快照清单自身 sha256 不符".to_string());
    }
    let mut seen = BTreeSet::new();
    for file in &manifest.files {
        check_manifest_path(&file.path)?;
        if !seen.insert(file.path.to_lowercase()) {
            return Err(format!("快照清单存在重复或大小写碰撞路径: {}", file.path));
        }
        if !valid_sha256(&file.sha256) {
            return Err(format!("快照文件 sha256 格式非法: {}", file.path));
        }
    }
    let count = u64::try_from(manifest.files.len()).map_err(|_| "快照文件数溢出".to_string())?;
    if manifest.file_count != count {
        return Err("快照清单 file_count 与文件列表不符".to_string());
    }
    let total = manifest
        .files
        .iter()
        .try_fold(0u64, |acc, f| acc.checked_add(f.size))
        .ok_or_else(|| "快照清单 total_bytes 溢出".to_string())?;
    if manifest.total_bytes != total {
        return Err("快照清单 total_bytes 与文件列表不符".to_string());
    }
    Ok(())
}

fn read_manifest(root: &Path, update_id: &str) -> Result<SnapshotManifest, String> {
    let manifest_path = root.join(SNAPSHOT_MANIFEST);
    let raw =
        fs::read(&manifest_path).map_err(|e| format!("快照清单缺失（{manifest_path:?}）: {e}"))?;
    let manifest: SnapshotManifest =
        serde_json::from_slice(&raw).map_err(|e| format!("快照清单解析失败: {e}"))?;
    validate_manifest_shape(&manifest, update_id)?;
    Ok(manifest)
}

/// SQLite 派生旁车文件（`<db>-wal` / `<db>-shm`，大小写不敏感后缀）且清单未收录：
/// 快照完整性检查用 server `inspect` 打开副本时，对 WAL 库的读写兜底打开会在
/// 快照 data/ 下留下这两个临时产物（实测缺陷，2026-08-31）。它们是派生文件、
/// 不参与恢复（恢复只复制清单内文件），验证时按良性旁车忽略。
fn is_unlisted_sqlite_sidecar(path: &str, listed: &BTreeSet<String>) -> bool {
    let lower = path.to_lowercase();
    for suffix in ["-wal", "-shm"] {
        if let Some(base) = lower.strip_suffix(suffix) {
            if listed.contains(&lower) {
                // 清单里明确收录的旁车文件仍按普通条目校验
                continue;
            }
            if listed.contains(base) {
                return true;
            }
        }
    }
    false
}

fn listed_snapshot_files(root: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    // 恢复 staging 目录没有 manifest.json（只复制清单内文件，不存在旁车）；
    // 快照目录则有——用清单判定「未收录的 SQLite 旁车」并按良性过滤。
    let listed: BTreeSet<String> = {
        let manifest_path = root.join(SNAPSHOT_MANIFEST);
        match fs::read(&manifest_path) {
            Ok(raw) => {
                let manifest: SnapshotManifest =
                    serde_json::from_slice(&raw).map_err(|e| format!("快照清单解析失败: {e}"))?;
                manifest
                    .files
                    .iter()
                    .map(|file| file.path.to_lowercase())
                    .collect()
            }
            Err(_) => BTreeSet::new(),
        }
    };
    let mut actual = BTreeMap::new();
    for path in walk_files(root)? {
        let rel = rel_forward(root, &path);
        if rel == SNAPSHOT_MANIFEST {
            continue;
        }
        check_manifest_path(&rel)?;
        if is_unlisted_sqlite_sidecar(&rel, &listed) {
            continue;
        }
        if actual.insert(rel.to_lowercase(), path).is_some() {
            return Err("快照目录存在大小写碰撞文件".to_string());
        }
    }
    Ok(actual)
}

/// 收集当前安装中的快照范围（data/ + config/config.toml），不把
/// `staging/`、`backups/` 或 `quarantine/` 等 launcher 自有目录算进来。
fn listed_live_files(layout: &InstallLayout) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut actual = BTreeMap::new();
    for path in walk_files(&layout.data_dir())? {
        let rel = rel_forward(&layout.root, &path);
        check_manifest_path(&rel)?;
        if actual.insert(rel.to_lowercase(), path).is_some() {
            return Err("现网数据目录存在大小写碰撞文件".to_string());
        }
    }
    if layout.config_file().exists() {
        let meta = safe_metadata(&layout.config_file())?;
        if !meta.is_file() {
            return Err("现网 config/config.toml 不是常规文件".to_string());
        }
        let rel = rel_forward(&layout.root, &layout.config_file());
        if actual
            .insert(rel.to_lowercase(), layout.config_file())
            .is_some()
        {
            return Err("现网快照范围存在重复文件".to_string());
        }
    }
    Ok(actual)
}

fn verify_file_map(
    files: &BTreeMap<String, PathBuf>,
    manifest: &SnapshotManifest,
    label: &str,
) -> Result<(), String> {
    let expected: BTreeSet<String> = manifest
        .files
        .iter()
        .map(|file| file.path.to_lowercase())
        .collect();
    if files.len() != expected.len() {
        return Err(format!(
            "{label} 文件集合与快照不符：实际 {} 项，清单 {} 项",
            files.len(),
            expected.len()
        ));
    }
    for file in &manifest.files {
        let path = files
            .get(&file.path.to_lowercase())
            .ok_or_else(|| format!("{label} 文件缺失: {}", file.path))?;
        let meta = safe_metadata(path)?;
        if !meta.is_file() {
            return Err(format!("{label} 文件不是常规文件: {}", file.path));
        }
        if meta.len() != file.size {
            return Err(format!(
                "{label} 文件 size 不符 {}: {}≠{}",
                file.path,
                meta.len(),
                file.size
            ));
        }
        let actual =
            sha256_file_hex(path).map_err(|e| format!("{label} hash {} 失败: {e}", file.path))?;
        if actual != file.sha256 {
            return Err(format!("{label} 文件 sha256 不符: {}", file.path));
        }
    }
    Ok(())
}

/// 创建快照并整体验证；`candidate_exe` 用于副本内 SQLite 的 `inspect` 完整性
/// 校验与 schema_after 观察（含 db 而无 exe → 失败，不进入 migrating）。
pub fn create(
    layout: &InstallLayout,
    update_id: &str,
    candidate_exe: Option<&Path>,
) -> Result<SnapshotReport, String> {
    create_with_inspector(layout, update_id, candidate_exe, &NativeSchemaInspector)
}

fn create_with_inspector(
    layout: &InstallLayout,
    update_id: &str,
    candidate_exe: Option<&Path>,
    inspector: &dyn SchemaInspector,
) -> Result<SnapshotReport, String> {
    sanitize_id(update_id)?;
    let root = backup_dir(layout, update_id);
    if root.exists() {
        // 幂等：同事务重复快照时清掉半截目录重做（此时尚未切换，无回滚价值）。
        fs::remove_dir_all(&root).map_err(|e| format!("清理旧快照目录失败: {e}"))?;
    }
    let data_dir = layout.data_dir();
    let config = layout.config_file();

    let mut sources: Vec<PathBuf> = if data_dir.is_dir() {
        walk_files(&data_dir).map_err(|e| format!("遍历 data/ 失败: {e}"))?
    } else {
        Vec::new()
    };
    if config.is_file() {
        sources.push(config.clone());
    }

    fs::create_dir_all(&root).map_err(|e| format!("创建快照目录失败: {e}"))?;
    let mut files = Vec::new();
    for src in &sources {
        let rel = rel_forward(&layout.root, src);
        check_manifest_path(&rel)?;
        let source_meta = safe_metadata(src)?;
        if !source_meta.is_file() {
            return Err(format!("快照源不是常规文件: {rel}"));
        }
        let dest = root.join(&rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建快照子目录失败: {e}"))?;
        }
        fs::copy(src, &dest).map_err(|e| format!("复制 {rel} 失败: {e}"))?;
        let size = source_meta.len();
        let sha = sha256_file_hex(src).map_err(|e| format!("hash {rel} 失败: {e}"))?;
        files.push(SnapshotFile {
            path: rel,
            size,
            sha256: sha,
        });
    }
    let total_bytes = files
        .iter()
        .try_fold(0u64, |total, file| total.checked_add(file.size))
        .ok_or_else(|| "快照总字节数溢出".to_string())?;
    let mut manifest = SnapshotManifest {
        schema_version: SCHEMA_VERSION,
        update_id: update_id.to_string(),
        created_at_unix_ms: now_unix_millis(),
        file_count: u64::try_from(files.len()).unwrap_or(0),
        total_bytes,
        files,
        manifest_sha256: String::new(),
    };
    manifest.manifest_sha256 = manifest_digest(&manifest)?;
    write_json_atomic(&root.join(SNAPSHOT_MANIFEST), &manifest)
        .map_err(|e| format!("写快照清单失败: {e}"))?;

    // 整体验证（不通过绝不返回成功——快照验证不全不进入 migrating）
    verify_with_inspector(layout, update_id, candidate_exe, true, inspector)?;

    // 清理 inspect 在副本旁留下的临时旁车文件（WAL 库读写兜底打开的副作用），
    // 让快照目录与清单精确一致；清理失败不影响快照有效性（验证已按忽略语义容忍）。
    let data_root = root.join("data");
    let listed: BTreeSet<String> = manifest
        .files
        .iter()
        .map(|file| file.path.to_lowercase())
        .collect();
    for path in walk_files(&data_root).unwrap_or_default() {
        let rel = rel_forward(&root, &path);
        if is_unlisted_sqlite_sidecar(&rel, &listed) {
            let _ = fs::remove_file(&path);
        }
    }

    Ok(SnapshotReport {
        id: update_id.to_string(),
        path: root.to_string_lossy().into_owned(),
        file_count: manifest.file_count,
        total_bytes: manifest.total_bytes,
        schema_after: candidate_exe.and_then(|exe| inspector.inspect(exe, &root.join("data"))),
    })
}

/// 验证快照完整性：清单存在 → 逐文件 size+sha256 → 计数/总量吻合 →
/// `check_db=true` 且存在 *.db 时用候选 exe inspect 校验副本。
pub fn verify(
    layout: &InstallLayout,
    update_id: &str,
    candidate_exe: Option<&Path>,
    check_db: bool,
) -> Result<SnapshotManifest, String> {
    verify_with_inspector(
        layout,
        update_id,
        candidate_exe,
        check_db,
        &NativeSchemaInspector,
    )
}

fn verify_with_inspector(
    layout: &InstallLayout,
    update_id: &str,
    candidate_exe: Option<&Path>,
    check_db: bool,
    inspector: &dyn SchemaInspector,
) -> Result<SnapshotManifest, String> {
    sanitize_id(update_id)?;
    let root = backup_dir(layout, update_id);
    let manifest = read_manifest(&root, update_id)?;
    let actual = listed_snapshot_files(&root)?;
    verify_file_map(&actual, &manifest, "快照")?;
    if check_db {
        let data_copy = root.join("data");
        let has_db = walk_files(&data_copy)
            .map(|files| {
                files.iter().any(|p| {
                    p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("db"))
                })
            })
            .unwrap_or(false);
        if has_db {
            let exe = candidate_exe.ok_or_else(|| {
                "快照含 SQLite 数据但未提供候选 exe，不能校验副本完整性（不进入 migrating）"
                    .to_string()
            })?;
            inspector.inspect(exe, &data_copy).ok_or_else(|| {
                "候选 exe inspect 校验快照失败（退出码非 0 或输出非 JSON）".to_string()
            })?;
        }
    }
    Ok(manifest)
}

/// 恢复快照：先验证（字节级 hash 复验；db 完整性已在创建时门禁）→ 现网 data/
/// 与 config.toml rename 到 quarantine（不删除）→ 复制回 → 恢复后再验证。
/// 失败返回 Err（调用方进 manual_recovery_required）。
pub fn restore(layout: &InstallLayout, update_id: &str) -> Result<(), String> {
    let manifest = verify(layout, update_id, None, false)?;
    let root = backup_dir(layout, update_id);
    let quarantine = layout.quarantine_dir();
    let stamp = now_unix_millis();

    // restore staging 位于安装根下，与 data/ 同卷；只有 staging 完整后才触碰现网。
    let restore_stage = layout
        .staging_dir()
        .join(format!("restore-{update_id}-{stamp}"));
    if restore_stage.exists() {
        return Err(format!(
            "恢复 staging 已存在，拒绝覆盖: {}",
            restore_stage.display()
        ));
    }
    fs::create_dir_all(restore_stage.join("data"))
        .map_err(|e| format!("创建恢复 staging 失败: {e}"))?;
    for file in &manifest.files {
        let src = root.join(&file.path);
        let dest = restore_stage.join(&file.path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建恢复 staging 子目录失败: {e}"))?;
        }
        fs::copy(&src, &dest).map_err(|e| format!("准备恢复 {} 失败: {e}", file.path))?;
    }
    let staged_files = listed_snapshot_files(&restore_stage)?;
    verify_file_map(&staged_files, &manifest, "恢复 staging")?;

    // 1) 现网数据/配置整体挪入 quarantine（失败数据不静默删除）
    let data_live = layout.data_dir();
    let config_live = layout.config_file();
    let quarantine_txn = quarantine.join(format!("rollback-{stamp}"));
    if data_live.exists() || config_live.exists() {
        fs::create_dir_all(&quarantine).map_err(|e| format!("创建 quarantine 失败: {e}"))?;
        fs::create_dir_all(&quarantine_txn)
            .map_err(|e| format!("创建 quarantine 事务目录失败: {e}"))?;
    }
    if data_live.exists() {
        // data 根本身允许是 junction（跨盘部署形态）；随后的 rename 只移动
        // junction 条目本身（同卷），物理数据留在目标盘、经 quarantine 可追溯。
        let meta = dir_root_metadata(&data_live)?;
        if !meta.is_dir() {
            return Err(format!("现网 data/ 不是目录: {}", data_live.display()));
        }
        let dest = quarantine_txn.join("data");
        crate::state::atomic::rename_with_retry(&data_live, &dest)
            .map_err(|e| format!("现网 data/ 移入 quarantine 失败（原位保留）: {e}"))?;
        tracing::warn!(from = %data_live.display(), to = %dest.display(), "现网数据已隔离保留（不静默删除）");
    }
    if config_live.exists() {
        let meta = safe_metadata(&config_live)?;
        if !meta.is_file() {
            return Err(format!(
                "现网 config.toml 不是常规文件: {}",
                config_live.display()
            ));
        }
        crate::state::atomic::rename_with_retry(&config_live, &quarantine_txn.join("config.toml"))
            .map_err(|e| format!("现网 config.toml 移入 quarantine 失败: {e}"))?;
    }

    // 2) 同卷换入；如果失败，quarantine 与 restore staging 均保留给人工恢复。
    crate::state::atomic::rename_with_retry(&restore_stage.join("data"), &data_live)
        .map_err(|e| format!("恢复 data/ 同卷换入失败: {e}"))?;
    if manifest
        .files
        .iter()
        .any(|file| file.path == "config/config.toml")
    {
        if let Some(parent) = config_live.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建 config/ 失败: {e}"))?;
        }
        crate::state::atomic::rename_with_retry(
            &restore_stage.join("config/config.toml"),
            &config_live,
        )
        .map_err(|e| format!("恢复 config.toml 同卷换入失败: {e}"))?;
    }

    // 3) 恢复后终验：hash 与文件集合都必须精确匹配，去掉候选新增文件。
    let live_files = listed_live_files(layout)?;
    verify_file_map(&live_files, &manifest, "恢复后现网")?;
    fs::remove_dir_all(&restore_stage)
        .map_err(|e| format!("清理恢复 staging 失败（数据已恢复，证据仍保留）: {e}"))?;
    if quarantine_txn.exists() {
        tracing::warn!(path = %quarantine_txn.display(), "旧现网数据已隔离保留");
    }
    Ok(())
}

/// `inspect --data-dir <p> --json` 解析 user_version；不可用返回 None（尽力而为）。
pub fn inspect_schema(exe: &Path, data_dir: &Path) -> Option<u32> {
    let output = Command::new(exe)
        .args(["inspect", "--data-dir"])
        .arg(data_dir)
        .arg("--json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    find_user_version(&value)
}

fn find_user_version(value: &serde_json::Value) -> Option<u32> {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if k == "user_version" {
                    if let Some(n) = v.as_u64() {
                        return u32::try_from(n).ok();
                    }
                }
                if let Some(found) = find_user_version(v) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(items) => items.iter().find_map(find_user_version),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::InstallLayout;

    fn temp_layout(tag: &str) -> InstallLayout {
        let root = std::env::temp_dir().join(format!(
            "gamer-snapshot-{tag}-{}-{}",
            std::process::id(),
            now_unix_millis()
        ));
        InstallLayout { root }
    }

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    #[test]
    fn create_verify_roundtrip() {
        let layout = temp_layout("rt");
        write(&layout.root, "data/a.txt", b"alpha");
        write(&layout.root, "data/sub/b.txt", b"beta");
        write(&layout.root, "config/config.toml", b"port = 8443\n");
        let report = create(&layout, "upd-1", None).expect("快照应成功");
        assert_eq!(report.file_count, 3);
        assert_eq!(report.total_bytes, 21u64);
        assert!(report.path.contains("backups"));
        assert!(verify(&layout, "upd-1", None, true).is_ok());
        // 篡改副本 → 验证失败
        write(
            &backup_dir(&layout, "upd-1").join("data"),
            "a.txt",
            b"TAMPER",
        );
        assert!(verify(&layout, "upd-1", None, true).is_err());
        let _ = fs::remove_dir_all(&layout.root);
    }

    #[test]
    fn restore_swaps_data_and_quarantines_live() {
        let layout = temp_layout("restore");
        write(&layout.root, "data/a.txt", b"alpha");
        write(&layout.root, "config/config.toml", b"port = 8443\n");
        create(&layout, "upd-2", None).expect("快照应成功");
        // 候选把数据改坏（模拟迁移写入）
        write(&layout.root, "data/a.txt", b"migrated-by-candidate");
        write(&layout.root, "data/candidate-new.txt", b"new");
        restore(&layout, "upd-2").expect("恢复应成功");
        assert_eq!(fs::read(layout.root.join("data/a.txt")).unwrap(), b"alpha");
        assert!(
            !layout.root.join("data/candidate-new.txt").exists(),
            "候选写入必须随换入消失"
        );
        assert_eq!(fs::read(layout.config_file()).unwrap(), b"port = 8443\n");
        // 失败数据保留在 quarantine
        let found = walk_files(&layout.quarantine_dir()).unwrap();
        assert!(
            found.iter().any(|p| p.ends_with("candidate-new.txt")),
            "被替换数据必须留在 quarantine: {found:?}"
        );
        let _ = fs::remove_dir_all(&layout.root);
    }

    #[test]
    fn verify_rejects_bad_ids_and_missing_manifest() {
        let layout = temp_layout("ids");
        assert!(verify(&layout, "../evil", None, true).is_err());
        assert!(verify(&layout, "missing-tx", None, true).is_err());
        assert!(create(&layout, "../evil", None).is_err());
        let _ = fs::remove_dir_all(&layout.root);
    }

    #[test]
    fn db_snapshot_requires_candidate_exe() {
        let layout = temp_layout("db");
        write(&layout.root, "data/gamer.db", b"not-a-real-db-but-present");
        assert!(
            create(&layout, "upd-3", None).is_err(),
            "含 db 而无候选 exe 时快照验证必须失败"
        );
        let _ = fs::remove_dir_all(&layout.root);
    }

    #[test]
    fn inspect_schema_returns_none_for_non_inspect_exe() {
        // 用 cmd 的失败实现冒充 inspect：exit 1 → None
        let fake = std::env::temp_dir().join(format!(
            "gamer-fake-inspect-{}-{}.cmd",
            std::process::id(),
            now_unix_millis()
        ));
        fs::write(&fake, "@exit /b 1\r\n").unwrap();
        assert!(inspect_schema(&fake, Path::new(".")).is_none());
        let _ = fs::remove_file(&fake);
    }

    struct MockInspector {
        schema: Option<u32>,
    }

    impl SchemaInspector for MockInspector {
        fn inspect(&self, _exe: &Path, _data_dir: &Path) -> Option<u32> {
            self.schema
        }
    }

    #[test]
    fn snapshot_manifest_binds_hashes_and_rejects_manifest_or_payload_tampering() {
        let layout = temp_layout("manifest-integrity");
        write(&layout.root, "data/state.bin", b"0123456789");
        write(&layout.root, "config/config.toml", b"port = 8443\n");

        let report = create(&layout, "upd-integrity", None).expect("快照应成功");
        let manifest_path = backup_dir(&layout, &report.id).join(SNAPSHOT_MANIFEST);
        let manifest: SnapshotManifest =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        assert_eq!(manifest.file_count, 2);
        assert_eq!(manifest.total_bytes, report.total_bytes);
        assert!(valid_sha256(&manifest.manifest_sha256));
        assert!(manifest.files.iter().all(|file| valid_sha256(&file.sha256)));
        assert!(verify(&layout, &report.id, None, true).is_ok());

        // 清单自身的摘要覆盖清单字段；只改一个字段也必须拒绝。
        let mut tampered: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        tampered["total_bytes"] = serde_json::json!(manifest.total_bytes + 1);
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&tampered).unwrap(),
        )
        .unwrap();
        let err = verify(&layout, &report.id, None, true).unwrap_err();
        assert!(
            err.contains("自身 sha256") || err.contains("total_bytes"),
            "{err}"
        );

        // 恢复原清单后，等长 payload 篡改仍由逐文件 hash 拦截。
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        write(
            &backup_dir(&layout, &report.id).join("data"),
            "state.bin",
            b"9876543210",
        );
        let err = verify(&layout, &report.id, None, true).unwrap_err();
        assert!(err.contains("sha256"), "{err}");

        let _ = fs::remove_dir_all(&layout.root);
    }

    #[test]
    fn sqlite_snapshot_integrity_uses_injected_inspector_and_records_schema() {
        let layout = temp_layout("sqlite-inspect");
        write(&layout.root, "data/gamer.db", b"sqlite fixture bytes");
        let valid = MockInspector { schema: Some(1) };
        let report =
            create_with_inspector(&layout, "upd-sqlite", Some(Path::new("candidate")), &valid)
                .expect("inspect 通过时含 db 快照应成功");
        assert_eq!(report.schema_after, Some(1));
        assert!(verify_with_inspector(
            &layout,
            "upd-sqlite",
            Some(Path::new("candidate")),
            true,
            &valid,
        )
        .is_ok());

        let invalid = MockInspector { schema: None };
        let err = verify_with_inspector(
            &layout,
            "upd-sqlite",
            Some(Path::new("candidate")),
            true,
            &invalid,
        )
        .unwrap_err();
        assert!(err.contains("SQLite") || err.contains("inspect"), "{err}");

        let _ = fs::remove_dir_all(&layout.root);
    }

    #[test]
    fn restore_failure_keeps_quarantine_and_restore_staging_evidence() {
        let layout = temp_layout("restore-failure");
        write(&layout.root, "data/state.bin", b"old");
        write(&layout.root, "config/config.toml", b"old-config");
        create(&layout, "upd-restore-failure", None).expect("快照应成功");

        write(&layout.root, "data/state.bin", b"new");
        // 让 data 已经隔离后，在 config 边界失败；恢复 staging 和 quarantine
        // 都必须保留，供 manual recovery 使用。
        fs::remove_file(layout.config_file()).unwrap();
        fs::create_dir(layout.config_file()).unwrap();
        let err = restore(&layout, "upd-restore-failure").unwrap_err();
        assert!(
            err.contains("config.toml") && err.contains("常规文件"),
            "{err}"
        );
        assert!(!layout.data_dir().exists(), "失败时不应静默删除隔离数据");
        assert!(walk_files(&layout.quarantine_dir())
            .unwrap()
            .iter()
            .any(|path| path.ends_with("state.bin")));
        let restore_staging = fs::read_dir(layout.staging_dir())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("restore-upd-restore-failure-"))
            });
        assert!(
            restore_staging.is_some(),
            "恢复 staging 必须保留: {restore_staging:?}"
        );

        let _ = fs::remove_dir_all(&layout.root);
    }

    #[test]
    fn sqlite_sidecar_left_by_inspect_is_ignored_on_verify_and_cleaned_after_create() {
        // 实测缺陷回归（2026-08-31）：server inspect 对 WAL 库的读写兜底打开会在
        // 快照 data/ 下留下 <db>-wal/<db>-shm；恢复验证不得把它们当多余文件，
        // 否则每次回滚都会落入 manual_recovery_required。
        let layout = temp_layout("sidecar");
        write(
            &layout.root,
            "data/gamer.db",
            b"SQLite format 3\0wal-fixture",
        );
        write(&layout.root, "config/config.toml", b"port = 8443\n");
        let inspector = MockInspector { schema: Some(1) };
        create_with_inspector(
            &layout,
            "upd-sidecar",
            Some(Path::new("candidate")),
            &inspector,
        )
        .expect("快照应成功");

        // 模拟 inspect 副作用：旁车文件出现在快照 data/ 下（不在清单内）
        write(
            &backup_dir(&layout, "upd-sidecar").join("data"),
            "gamer.db-wal",
            b"",
        );
        write(
            &backup_dir(&layout, "upd-sidecar").join("data"),
            "gamer.db-shm",
            &[0u8; 32768],
        );
        verify_with_inspector(
            &layout,
            "upd-sidecar",
            Some(Path::new("candidate")),
            true,
            &inspector,
        )
        .expect("未收录的 SQLite 旁车文件必须被容忍");

        // 新建快照结束时自带旁车清理：create 后写入的旁车被移除
        write(
            &backup_dir(&layout, "upd-sidecar").join("data"),
            "gamer.db-wal",
            b"",
        );
        create_with_inspector(
            &layout,
            "upd-sidecar",
            Some(Path::new("candidate")),
            &inspector,
        )
        .expect("同事务重复快照应成功");
        assert!(
            !backup_dir(&layout, "upd-sidecar")
                .join("data/gamer.db-wal")
                .exists(),
            "快照创建收尾必须清理未收录的旁车文件"
        );

        // 清单内未收录的旁车文件无论内容如何都按良性过滤（内容篡改由 gamer.db
        // 自身的 hash 门禁负责——见 manifest-integrity 测试）
        write(
            &backup_dir(&layout, "upd-sidecar").join("data"),
            "gamer.db-wal",
            b"x",
        );
        verify_with_inspector(
            &layout,
            "upd-sidecar",
            Some(Path::new("candidate")),
            true,
            &inspector,
        )
        .expect("旁车过滤与内容无关，验证应通过");

        // 副本内 gamer.db 被篡改仍必须拒绝（过滤只针对旁车，不放松主文件门禁）
        write(
            &backup_dir(&layout, "upd-sidecar").join("data"),
            "gamer.db",
            b"TAMPERED",
        );
        let report_err = verify_with_inspector(
            &layout,
            "upd-sidecar",
            Some(Path::new("candidate")),
            true,
            &inspector,
        )
        .unwrap_err();
        assert!(
            report_err.contains("sha256") || report_err.contains("size"),
            "{report_err}"
        );

        let _ = fs::remove_dir_all(&layout.root);
    }

    #[test]
    fn qa007_many_small_files_snapshot_with_bounded_db_fixture() {
        let layout = temp_layout("qa007-pressure");
        let db = layout.root.join("data/gamer.db");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        fs::write(&db, b"SQLite format 3\0fixture-db\n").unwrap();

        const SMALL_FILE_COUNT: usize = 2048;
        for index in 0..SMALL_FILE_COUNT {
            let path = layout
                .root
                .join("data/com.example.game/tmpl")
                .join(format!("fixture-{index:04}.bin"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("fixture-{index:04}\n")).unwrap();
        }

        let inspector = MockInspector { schema: Some(1) };
        let report = create_with_inspector(
            &layout,
            "qa007-pressure",
            Some(Path::new("candidate")),
            &inspector,
        )
        .expect("DB fixture 与大量小文件的快照应成功");
        assert_eq!(report.file_count, (SMALL_FILE_COUNT + 1) as u64);
        assert_eq!(report.schema_after, Some(1));
        assert!(report.total_bytes > 0);

        let snapshot_db = backup_dir(&layout, "qa007-pressure").join("data/gamer.db");
        assert_eq!(
            fs::metadata(&snapshot_db).unwrap().len(),
            fs::metadata(&db).unwrap().len()
        );
        let manifest = verify_with_inspector(
            &layout,
            "qa007-pressure",
            Some(Path::new("candidate")),
            true,
            &inspector,
        )
        .expect("压力快照复验应成功");
        assert_eq!(manifest.file_count, report.file_count);
        assert_eq!(manifest.total_bytes, report.total_bytes);

        let _ = fs::remove_dir_all(&layout.root);
    }

    #[cfg(windows)]
    fn create_junction(link: &Path, target: &Path) {
        let output = Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("mklink 进程应可启动");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.status.success() && link.exists(),
            "创建 junction 失败（{link:?} → {target:?}）: {text}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn junctioned_data_root_snapshots_but_nested_reparse_is_rejected() {
        // 实测缺陷回归（QA-005 跨盘，2026-09-01）：data 根本身是 junction
        // （C: 安装根 + D: 物理 data）时快照必须成功；树**内部**嵌套的
        // reparse point 仍然拒绝（防链接攻击语义不变）。
        let base = std::env::temp_dir().join(format!(
            "gamer-snapshot-junction-{}-{}",
            std::process::id(),
            now_unix_millis()
        ));
        let phys = base.join("phys-data");
        fs::create_dir_all(phys.join("nested")).unwrap();
        fs::write(phys.join("a.txt"), b"alpha").unwrap();
        fs::write(phys.join("nested/b.txt"), b"beta").unwrap();

        let layout = InstallLayout {
            root: base.join("inst"),
        };
        fs::create_dir_all(&layout.root).unwrap();
        create_junction(&layout.data_dir(), &phys);
        assert!(layout.data_dir().is_dir(), "junction 应解析到目录");

        let report = create(&layout, "upd-junction", None).expect("junction data 根快照应成功");
        assert_eq!(report.file_count, 2, "应穿透 junction 收集全部常规文件");
        assert!(verify(&layout, "upd-junction", None, false).is_ok());

        // 树内部嵌套 reparse point → 快照必须失败
        let elsewhere = base.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        create_junction(&layout.data_dir().join("sub"), &elsewhere);
        let err = create(&layout, "upd-nested", None).unwrap_err();
        assert!(
            err.contains("symlink") || err.contains("reparse"),
            "嵌套 reparse point 必须被拒绝: {err}"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(windows)]
    #[test]
    fn restore_with_junctioned_data_root_swaps_real_dir_and_quarantines_link() {
        // 回滚路径同样接受 junction data 根：恢复后现网 data/ 是含快照内容的
        // 真实目录，原 junction（指向被候选写坏的物理数据）保留在 quarantine。
        let base = std::env::temp_dir().join(format!(
            "gamer-snapshot-restore-j-{}-{}",
            std::process::id(),
            now_unix_millis()
        ));
        let phys = base.join("phys-data");
        fs::create_dir_all(&phys).unwrap();
        fs::write(phys.join("state.bin"), b"old").unwrap();

        let layout = InstallLayout {
            root: base.join("inst"),
        };
        fs::create_dir_all(&layout.root).unwrap();
        create_junction(&layout.data_dir(), &phys);
        write(&layout.root, "config/config.toml", b"cfg");

        create(&layout, "upd-restore-j", None).expect("junction data 根快照应成功");
        // 候选写坏数据（写穿 junction → 物理盘）
        fs::write(layout.data_dir().join("state.bin"), b"corrupted").unwrap();

        restore(&layout, "upd-restore-j").expect("junction data 根的回滚恢复应成功");
        assert_eq!(
            fs::read(layout.data_dir().join("state.bin")).unwrap(),
            b"old",
            "恢复后现网数据必须来自快照"
        );
        // 恢复后的 data/ 是真实目录（不再是 junction），旧 junction 留在 quarantine
        let meta = fs::symlink_metadata(layout.data_dir()).unwrap();
        assert!(
            !meta.file_type().is_symlink() && !is_reparse_point(&meta),
            "恢复换入的 data/ 应为真实目录"
        );
        let quarantine = layout.quarantine_dir();
        let txns: Vec<PathBuf> = fs::read_dir(&quarantine)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        let moved = txns
            .iter()
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("rollback-"))
            })
            .expect("旧数据必须留在 quarantine")
            .join("data");
        let moved_meta = fs::symlink_metadata(&moved).unwrap();
        assert!(
            moved_meta.file_type().is_symlink() || is_reparse_point(&moved_meta),
            "被隔离的旧 data/ 应保留 junction 形态: {moved:?}"
        );

        let _ = fs::remove_dir_all(&base);
    }
}
