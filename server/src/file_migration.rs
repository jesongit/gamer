//! 可恢复文件迁移框架（DATA-004 / release/contracts/schema-policy.md §5）。
//!
//! 固定四阶段顺序：**plan → staging copy → hash/validate → marker**：
//! 1. `plan`：生成迁移计划（源→目标清单）并**先原子记录意图**（journal 落盘）
//!    后执行；
//! 2. `staging copy`：逐文件复制到目标（staging）位置，**源文件全程不动**；
//!    每个文件边界原子推进 journal（崩溃后可从任一边界恢复）；
//! 3. `hash/validate`：对全部目标逐文件 SHA-256 复验；任何不一致**禁止写出
//!    marker**（混合布局不得误标成功）；
//! 4. `marker`：校验全通过后原子写 marker，journal 推进 Committed。
//!
//! 契约要点：
//! - 独立 journal（本框架自管，**≠** `state/update-journal.json`），JSON 原子写
//!   （临时文件 + rename）；
//! - 重复运行幂等：Committed 后再次 `resume` = 复验 + no-op；
//! - `rollback`：仅 Committed 之前可用（清 staging + marker）；源文件从未被
//!   改动，故回滚无需恢复源。Committed 之后走快照恢复路径（契约 §6）；
//! - 旧源文件保留到升级提交 + 回滚保留期结束——本框架不提供源清理入口。
//!
//! **当前布局（`data/<pkg>/{yaml,func,tmpl}/`）无迁移需求**：本模块为纯库代码
//! 骨架 + 单测，未接线任何运行路径；未来文件布局迁移在此框架上落地。

// 纯框架批次：消费方接线属后续迁移需求落地时（batch 计划 DATA-006/QA-003）
#![allow(dead_code)]

use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// journal 结构版本（独立于 DB schema；结构变更需在此编号并兼容旧 journal）
pub const JOURNAL_SCHEMA_VERSION: i64 = 1;

/// SHA-256 分块读取缓冲（64KiB：模板图片/脚本文件量级下足够高效）
const COPY_BUFFER_SIZE: usize = 64 * 1024;

/// 迁移阶段（journal.phase）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// plan 已落盘，尚未开始复制
    Planned,
    /// 复制进行中（含部分完成的崩溃恢复点）
    Copying,
    /// 逐文件复验已通过，marker 未写（marker 写入与 journal 推进之间的崩溃点）
    Validated,
    /// marker 已写 + journal 已推进：迁移完成
    Committed,
    /// 已回滚（staging/marker 已清理）
    RolledBack,
}

/// plan 中的单条迁移条目：源文件 → 目标（staging）路径
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanEntry {
    pub source: PathBuf,
    pub target: PathBuf,
}

/// journal 中的单条条目状态：复制完成后记录内容 SHA-256
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryState {
    pub source: PathBuf,
    pub target: PathBuf,
    /// 目标内容 SHA-256（小写 hex）；None = 尚未复制
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// 文件迁移 journal（独立 JSON，原子写）。`resume`/`rollback` 的唯一状态依据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    pub schema_version: i64,
    /// 本次迁移 id（进入 marker 内容，便于核对）
    pub id: String,
    pub phase: Phase,
    pub entries: Vec<EntryState>,
    /// marker 文件落点（phase=Committed 的持久标志物）
    pub marker: PathBuf,
}

/// 生成迁移计划并原子落盘 journal（先记意图后执行）。调用方可传显式条目
/// （支持任意旧→新布局映射），或先用 [`discover_copy_plan`] 做同构目录 1:1 映射。
pub fn plan(
    id: &str,
    entries: Vec<PlanEntry>,
    marker: PathBuf,
    journal_path: &Path,
) -> anyhow::Result<Journal> {
    anyhow::ensure!(!entries.is_empty(), "file migration plan is empty");
    let journal_tmp = atomic_temp_path(journal_path);
    let marker_tmp = atomic_temp_path(&marker);
    let reserved = [
        (journal_path, "journal"),
        (journal_tmp.as_path(), "journal temp"),
        (marker.as_path(), "marker"),
        (marker_tmp.as_path(), "marker temp"),
    ];
    for (index, (path, label)) in reserved.iter().enumerate() {
        for (other, other_label) in reserved.iter().skip(index + 1) {
            anyhow::ensure!(
                path_key(path) != path_key(other),
                "{label} path collides with {other_label}: {}",
                path.display()
            );
        }
    }
    let mut sources = HashSet::new();
    let mut targets = HashSet::new();
    for entry in &entries {
        anyhow::ensure!(
            path_key(&entry.source) != path_key(&entry.target),
            "plan entry source == target: {}",
            entry.source.display()
        );
        anyhow::ensure!(
            sources.insert(path_key(&entry.source)),
            "duplicate migration source: {}",
            entry.source.display()
        );
        anyhow::ensure!(
            targets.insert(path_key(&entry.target)),
            "duplicate migration target: {}",
            entry.target.display()
        );
        // journal / marker 是本次迁移的专用产物：不得与任何源或 staging 目标
        // 同文件（否则 journal/marker 落盘会覆盖源文件或顶掉迁移产物）
        for (path, label) in reserved {
            anyhow::ensure!(
                path_key(&entry.source) != path_key(path),
                "{label} path must not be a migration source: {}",
                path.display()
            );
            anyhow::ensure!(
                path_key(&entry.target) != path_key(path),
                "{label} path must not collide with a staging target: {}",
                path.display()
            );
        }
    }
    // A target must not replace another source: the source tree is the
    // rollback boundary and remains untouched for the whole migration.
    for entry in &entries {
        anyhow::ensure!(
            !sources.contains(&path_key(&entry.target)),
            "migration target collides with a source: {}",
            entry.target.display()
        );
    }
    let journal = Journal {
        schema_version: JOURNAL_SCHEMA_VERSION,
        id: id.to_string(),
        phase: Phase::Planned,
        entries: entries
            .into_iter()
            .map(|e| EntryState {
                source: e.source,
                target: e.target,
                sha256: None,
            })
            .collect(),
        marker,
    };
    save_journal(&journal, journal_path)?;
    Ok(journal)
}

/// Stable comparison key for paths that may not exist yet. Windows file
/// lookups are case-insensitive, so a lower-cased lexical key catches target
/// collisions before staging creates either file; non-Windows keeps the host's
/// case-sensitive semantics.
fn path_key(path: &Path) -> String {
    #[cfg(windows)]
    {
        path.to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().into_owned()
    }
}

fn atomic_temp_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.tmp", path.display()))
}

/// 同构目录映射：把 `old_root` 下全部文件（递归）映射为 `new_root` 下同相对
/// 路径的 plan 条目。异构布局（改名/合并）由调用方自行构造条目。
pub fn discover_copy_plan(old_root: &Path, new_root: &Path) -> anyhow::Result<Vec<PlanEntry>> {
    let mut entries = Vec::new();
    walk_files(old_root, old_root, new_root, &mut entries)?;
    Ok(entries)
}

fn walk_files(
    root: &Path,
    dir: &Path,
    new_root: &Path,
    out: &mut Vec<PlanEntry>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_files(root, &path, new_root, out)?;
        } else if path.is_file() {
            let target = new_root.join(path.strip_prefix(root)?);
            out.push(PlanEntry {
                source: path,
                target,
            });
        }
    }
    Ok(())
}

/// 读取既有 journal（resume/rollback 的入口检查）
pub fn load_journal(journal_path: &Path) -> anyhow::Result<Journal> {
    let raw = std::fs::read(journal_path)
        .with_context(|| format!("read journal {}", journal_path.display()))?;
    let journal: Journal = serde_json::from_slice(&raw)
        .with_context(|| format!("parse journal {}", journal_path.display()))?;
    anyhow::ensure!(
        journal.schema_version == JOURNAL_SCHEMA_VERSION,
        "journal schema {} unsupported (expected {JOURNAL_SCHEMA_VERSION}): {}",
        journal.schema_version,
        journal_path.display()
    );
    Ok(journal)
}

/// 恢复/推进迁移：从 journal 记录的边界继续，走完 copy → validate → marker。
/// - Committed：复验通过即 no-op（幂等）；
/// - RolledBack：拒绝（应另起新迁移）；
/// - 其余：逐文件补拷（已有 sha 的条目跳过，不重复复制）→ 全量复验 → marker。
pub fn resume(journal_path: &Path) -> anyhow::Result<Journal> {
    let mut journal = load_journal(journal_path)?;
    match journal.phase {
        Phase::Committed => {
            // 幂等：marker 在位且逐文件校验通过 → no-op
            verify_all(&journal)?;
            anyhow::ensure!(
                journal.marker.is_file(),
                "journal {} is committed but its marker is missing ({}); treat as suspicious and inspect manually",
                journal_path.display(),
                journal.marker.display()
            );
            return Ok(journal);
        }
        Phase::RolledBack => {
            bail!(
                "journal {} was rolled back; start a new migration instead of resuming it",
                journal_path.display()
            );
        }
        Phase::Planned | Phase::Copying | Phase::Validated => {}
    }

    journal.phase = Phase::Copying;
    save_journal(&journal, journal_path)?;

    // 阶段 2：staging copy——源不动；每个文件边界原子推进 journal
    for index in 0..journal.entries.len() {
        if journal.entries[index].sha256.is_some() {
            continue; // 此前边界后恢复：已复制条目不重复复制
        }
        copy_one(&mut journal.entries[index])?;
        save_journal(&journal, journal_path)?;
    }

    // 阶段 3：全量复验（含历史边界）；失败禁止写 marker（混合布局不得误标成功）
    verify_all(&journal)?;
    journal.phase = Phase::Validated;
    save_journal(&journal, journal_path)?;

    // 阶段 4：marker 原子提交，随后 journal 推进 Committed
    write_marker(&journal)?;
    journal.phase = Phase::Committed;
    save_journal(&journal, journal_path)?;
    Ok(journal)
}

/// 回滚：清掉 staging 产物与 marker，journal 记 RolledBack。源文件全程未被
/// 改动，无需恢复。已 Committed 的迁移拒绝由此回滚（post-commit 走快照恢复
/// 路径，契约 §6.2）。
pub fn rollback(journal_path: &Path) -> anyhow::Result<Journal> {
    let mut journal = load_journal(journal_path)?;
    if journal.phase == Phase::Committed {
        bail!(
            "journal {} is committed; automatic rollback no longer applies (restore the pre-upgrade snapshot instead)",
            journal_path.display()
        );
    }
    let mut failures = Vec::new();
    if journal.marker.exists() {
        if let Err(e) = std::fs::remove_file(&journal.marker) {
            failures.push(format!("remove marker: {e}"));
        }
    }
    for temp in [
        atomic_temp_path(journal_path),
        atomic_temp_path(&journal.marker),
    ] {
        if temp.exists() {
            if let Err(e) = std::fs::remove_file(&temp) {
                failures.push(format!("remove {}: {e}", temp.display()));
            }
        }
    }
    for entry in &journal.entries {
        if entry.target.exists() {
            if let Err(e) = std::fs::remove_file(&entry.target) {
                failures.push(format!("remove {}: {e}", entry.target.display()));
            }
        }
    }
    if !failures.is_empty() {
        bail!(
            "rollback of {} left staging files behind: {}",
            journal_path.display(),
            failures.join("; ")
        );
    }
    journal.phase = Phase::RolledBack;
    save_journal(&journal, journal_path)?;
    Ok(journal)
}

fn copy_one(entry: &mut EntryState) -> anyhow::Result<()> {
    if !entry.source.is_file() {
        bail!(
            "source file is missing (sources must never be lost): {}",
            entry.source.display()
        );
    }
    if let Some(parent) = entry.target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create staging dir {}", parent.display()))?;
    }
    if entry.target.exists() {
        // 幂等恢复：目标已在且内容与源一致 → 视作已复制；不一致即混合状态，
        // 拒绝覆盖（不静默改写既有产物）
        let existing = sha256_file(&entry.target)?;
        let source = sha256_file(&entry.source)?;
        anyhow::ensure!(
            existing == source,
            "staging target {} already exists with different content; refusing to overwrite (mixed layout)",
            entry.target.display()
        );
    } else {
        std::fs::copy(&entry.source, &entry.target).with_context(|| {
            format!(
                "staging copy {} -> {}",
                entry.source.display(),
                entry.target.display()
            )
        })?;
    }
    entry.sha256 = Some(sha256_file(&entry.target)?);
    Ok(())
}

/// 阶段 3 校验：全部条目已复制且目标内容逐文件命中记录的 SHA-256
fn verify_all(journal: &Journal) -> anyhow::Result<()> {
    for entry in &journal.entries {
        let Some(expected) = &entry.sha256 else {
            bail!(
                "entry not copied yet (mixed layout must not be marked committed): {}",
                entry.target.display()
            );
        };
        let actual = sha256_file(&entry.target)?;
        anyhow::ensure!(
            &actual == expected,
            "hash mismatch for {}: journal records {expected}, file has {actual}",
            entry.target.display()
        );
    }
    Ok(())
}

fn write_marker(journal: &Journal) -> anyhow::Result<()> {
    if let Some(parent) = journal.marker.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create marker dir {}", parent.display()))?;
        }
    }
    let body = format!("{}\nentries={}\n", journal.id, journal.entries.len());
    write_atomic(&journal.marker, body.as_bytes())
}

/// journal / marker 的原子写：同目录临时文件 + 落盘 flush + rename 替换
/// （半截文件不可能顶替正式内容）
fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = atomic_temp_path(path);
    let mut temp_created = false;
    let result = (|| -> anyhow::Result<()> {
        let mut file =
            File::create(&tmp).with_context(|| format!("create temp file {}", tmp.display()))?;
        temp_created = true;
        std::io::Write::write_all(&mut file, bytes)
            .with_context(|| format!("write temp file {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("flush temp file {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("atomic replace {}", path.display()))?;
        Ok(())
    })();
    if temp_created && result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn save_journal(journal: &Journal, journal_path: &Path) -> anyhow::Result<()> {
    let mut bytes =
        serde_json::to_vec_pretty(journal).context("serialize file migration journal")?;
    bytes.push(b'\n');
    write_atomic(journal_path, &bytes)
}

/// 流式 SHA-256（小写 hex）
fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("open {} for hashing", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; COPY_BUFFER_SIZE];
    loop {
        let read = std::io::Read::read(&mut file, &mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 中文 + 空格临时目录（验收要求：中文路径下全流程可用）
    fn temp_dir_cn(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gamer-filemig-中文 目录-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(path: &Path, content: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    struct Fixture {
        old_root: PathBuf,
        new_root: PathBuf,
        journal_path: PathBuf,
        marker: PathBuf,
    }

    fn fixture(tag: &str) -> Fixture {
        let base = temp_dir_cn(tag);
        let old_root = base.join("旧布局 数据");
        let new_root = base.join("新布局 数据");
        let journal_path = base.join("file-migration-journal.json");
        let marker = base.join("迁移 完成.marker");
        std::fs::create_dir_all(&old_root).unwrap();
        Fixture {
            old_root,
            new_root,
            journal_path,
            marker,
        }
    }

    fn standard_plan(fx: &Fixture) -> Vec<PlanEntry> {
        // 三个文件：根目录、中文文件名、深层嵌套
        vec![
            PlanEntry {
                source: fx.old_root.join("脚本一.yaml"),
                target: fx.new_root.join("脚本一.yaml"),
            },
            PlanEntry {
                source: fx.old_root.join("资源包").join("模板 图片.png"),
                target: fx.new_root.join("资源包").join("模板 图片.png"),
            },
            PlanEntry {
                source: fx.old_root.join("a").join("b").join("deep.yaml"),
                target: fx.new_root.join("a").join("b").join("deep.yaml"),
            },
        ]
    }

    fn seed_sources(fx: &Fixture) {
        write_file(&fx.old_root.join("脚本一.yaml"), b"steps: []\n");
        write_file(
            &fx.old_root.join("资源包").join("模板 图片.png"),
            &[0u8, 159, 146, 150, 1, 2, 3, 255],
        );
        write_file(
            &fx.old_root.join("a").join("b").join("deep.yaml"),
            b"params: {}\n",
        );
    }

    fn snapshot_sources(entries: &[PlanEntry]) -> Vec<(PathBuf, Vec<u8>)> {
        entries
            .iter()
            .map(|entry| (entry.source.clone(), std::fs::read(&entry.source).unwrap()))
            .collect()
    }

    fn assert_sources_unchanged(snapshot: &[(PathBuf, Vec<u8>)]) {
        for (path, expected) in snapshot {
            assert_eq!(
                std::fs::read(path).unwrap(),
                *expected,
                "source file changed: {}",
                path.display()
            );
        }
    }

    fn drop_fixture(fx: &Fixture) {
        let base = fx.journal_path.parent().unwrap().to_path_buf();
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn plan_records_intent_atomically_before_any_copy() {
        let fx = fixture("plan");
        seed_sources(&fx);
        let entries = standard_plan(&fx);
        let journal = plan("mig-1", entries, fx.marker.clone(), &fx.journal_path).unwrap();
        assert_eq!(journal.phase, Phase::Planned);
        assert!(journal.entries.iter().all(|e| e.sha256.is_none()));
        // journal 已落盘（先记意图），且尚未产生任何目标文件
        let loaded = load_journal(&fx.journal_path).unwrap();
        assert_eq!(loaded, journal);
        assert!(!fx.new_root.exists());
        assert!(!fx.marker.exists());
        drop_fixture(&fx);
    }

    #[test]
    fn plan_rejects_empty_and_self_overlapping_entries() {
        let fx = fixture("plan-guard");
        let err = plan("mig", vec![], fx.marker.clone(), &fx.journal_path).unwrap_err();
        assert!(err.to_string().contains("empty"));
        let src = fx.old_root.join("x.yaml");
        write_file(&src, b"data");
        let err = plan(
            "mig",
            vec![PlanEntry {
                source: src.clone(),
                target: src.clone(),
            }],
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap_err();
        assert!(err.to_string().contains("source == target"));
        // journal / marker 与源或 staging 目标同文件 → 拒绝（journal 落盘
        // 不得覆盖源文件，也不得顶掉迁移产物）
        let target = fx.new_root.join("x.yaml");
        let err = plan(
            "mig",
            vec![PlanEntry {
                source: src.clone(),
                target: target.clone(),
            }],
            fx.marker.clone(),
            &target,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("collide with a staging target"),
            "{err}"
        );
        let err = plan(
            "mig",
            vec![PlanEntry {
                source: src.clone(),
                target,
            }],
            fx.marker.clone(),
            &src,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("not be a migration source"),
            "{err}"
        );
        drop_fixture(&fx);
    }

    #[test]
    fn plan_rejects_duplicate_targets_sources_and_cross_tree_collisions() {
        let fx = fixture("plan-collisions");
        seed_sources(&fx);
        let first = fx.old_root.join("脚本一.yaml");
        let second = fx.old_root.join("资源包").join("模板 图片.png");
        let target = fx.new_root.join("same.bin");

        let err = plan(
            "collision-target",
            vec![
                PlanEntry {
                    source: first.clone(),
                    target: target.clone(),
                },
                PlanEntry {
                    source: second.clone(),
                    target: target.clone(),
                },
            ],
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("duplicate migration target"),
            "{err}"
        );
        assert!(!fx.journal_path.exists(), "拒绝计划不得落盘 journal");

        let err = plan(
            "collision-source",
            vec![
                PlanEntry {
                    source: first.clone(),
                    target: target.clone(),
                },
                PlanEntry {
                    source: first.clone(),
                    target: fx.new_root.join("other.bin"),
                },
            ],
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("duplicate migration source"),
            "{err}"
        );

        // 目标指向另一条源会在复制时毁掉 rollback 源，必须在 plan 阶段拒绝。
        let err = plan(
            "collision-cross-tree",
            vec![
                PlanEntry {
                    source: first.clone(),
                    target: fx.new_root.join("first.bin"),
                },
                PlanEntry {
                    source: second.clone(),
                    target: first.clone(),
                },
            ],
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap_err();
        assert!(err.to_string().contains("collides with a source"), "{err}");

        let err = plan(
            "collision-reserved",
            vec![PlanEntry {
                source: first,
                target,
            }],
            fx.journal_path.clone(),
            &fx.journal_path,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("journal path collides with marker"),
            "{err}"
        );
        drop_fixture(&fx);
    }

    #[test]
    fn preexisting_different_target_is_rejected_without_losing_source() {
        let fx = fixture("target-collision");
        seed_sources(&fx);
        let target = fx.new_root.join("脚本一.yaml");
        write_file(&target, b"another migration already owns this path");
        plan(
            "collision-existing",
            vec![PlanEntry {
                source: fx.old_root.join("脚本一.yaml"),
                target: target.clone(),
            }],
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap();
        let source = fx.old_root.join("脚本一.yaml");
        let original = std::fs::read(&source).unwrap();
        let err = resume(&fx.journal_path).unwrap_err();
        assert!(err.to_string().contains("different content"), "{err}");
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert!(!fx.marker.exists());
        let journal = load_journal(&fx.journal_path).unwrap();
        assert_eq!(journal.phase, Phase::Copying);
        drop_fixture(&fx);
    }

    #[test]
    fn n_minus_one_to_n_supports_renamed_files_and_keeps_old_layout() {
        let fx = fixture("n-minus-one-to-n");
        seed_sources(&fx);
        let entries = vec![
            PlanEntry {
                source: fx.old_root.join("脚本一.yaml"),
                target: fx.new_root.join("yaml").join("main.yaml"),
            },
            PlanEntry {
                source: fx.old_root.join("资源包").join("模板 图片.png"),
                target: fx.new_root.join("tmpl").join("main.png"),
            },
            PlanEntry {
                source: fx.old_root.join("a").join("b").join("deep.yaml"),
                target: fx.new_root.join("func").join("library.yaml"),
            },
        ];
        let before = snapshot_sources(&entries);
        plan(
            "layout-v1-to-v2",
            entries.clone(),
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap();

        let journal = resume(&fx.journal_path).unwrap();
        assert_eq!(journal.phase, Phase::Committed);
        assert!(fx.marker.is_file());
        for entry in &journal.entries {
            assert_eq!(
                std::fs::read(&entry.target).unwrap(),
                std::fs::read(&entry.source).unwrap(),
                "migrated content differs: {}",
                entry.target.display()
            );
        }
        assert_sources_unchanged(&before);
        drop_fixture(&fx);
    }

    #[test]
    fn copy_failure_is_retryable_and_does_not_change_sources() {
        let fx = fixture("copy-failure");
        seed_sources(&fx);
        let entries = standard_plan(&fx);
        let before = snapshot_sources(&entries);
        plan(
            "copy-failure-retry",
            entries,
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap();

        // The destination root is a file, so creating the first staging parent fails.
        write_file(&fx.new_root, b"destination obstruction");
        let err = resume(&fx.journal_path).unwrap_err();
        assert!(err.to_string().contains("create staging dir"), "{err}");
        assert_eq!(
            load_journal(&fx.journal_path).unwrap().phase,
            Phase::Copying
        );
        assert!(!fx.marker.exists());
        assert_sources_unchanged(&before);

        std::fs::remove_file(&fx.new_root).unwrap();
        let journal = resume(&fx.journal_path).unwrap();
        assert_eq!(journal.phase, Phase::Committed);
        assert_sources_unchanged(&before);
        drop_fixture(&fx);
    }

    #[test]
    fn hash_failure_is_retryable_and_never_commits_tampered_staging() {
        let fx = fixture("hash-failure");
        seed_sources(&fx);
        let entries = standard_plan(&fx);
        let before = snapshot_sources(&entries);
        plan(
            "hash-failure-retry",
            entries,
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap();

        // Simulate an interruption after copy + hash journal update, followed by
        // corruption of that staging file before the process resumes.
        let mut journal = load_journal(&fx.journal_path).unwrap();
        journal.phase = Phase::Copying;
        let first = &mut journal.entries[0];
        std::fs::create_dir_all(first.target.parent().unwrap()).unwrap();
        std::fs::copy(&first.source, &first.target).unwrap();
        first.sha256 = Some(sha256_file(&first.target).unwrap());
        save_journal(&journal, &fx.journal_path).unwrap();
        write_file(&journal.entries[0].target, b"corrupt staging");

        let err = resume(&fx.journal_path).unwrap_err();
        assert!(err.to_string().contains("hash mismatch"), "{err}");
        assert_eq!(
            load_journal(&fx.journal_path).unwrap().phase,
            Phase::Copying
        );
        assert!(!fx.marker.exists());
        assert_sources_unchanged(&before);

        // Once the staging file is restored, resume can finish without copying
        // the already-journaled entry again.
        std::fs::copy(&journal.entries[0].source, &journal.entries[0].target).unwrap();
        let journal = resume(&fx.journal_path).unwrap();
        assert_eq!(journal.phase, Phase::Committed);
        assert_sources_unchanged(&before);
        drop_fixture(&fx);
    }

    #[test]
    fn marker_rename_failure_is_retryable_and_cleans_atomic_temp() {
        let fx = fixture("rename-failure");
        seed_sources(&fx);
        let entries = standard_plan(&fx);
        let before = snapshot_sources(&entries);
        plan(
            "rename-failure-retry",
            entries,
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap();

        // A directory at the marker destination makes the final temp->marker
        // rename fail after validation has already been durably recorded.
        std::fs::create_dir_all(&fx.marker).unwrap();
        let err = resume(&fx.journal_path).unwrap_err();
        assert!(err.to_string().contains("atomic replace"), "{err}");
        assert_eq!(
            load_journal(&fx.journal_path).unwrap().phase,
            Phase::Validated
        );
        assert!(!PathBuf::from(format!("{}.tmp", fx.marker.display())).exists());
        assert_sources_unchanged(&before);

        std::fs::remove_dir_all(&fx.marker).unwrap();
        let journal = resume(&fx.journal_path).unwrap();
        assert_eq!(journal.phase, Phase::Committed);
        assert_sources_unchanged(&before);
        drop_fixture(&fx);
    }

    #[test]
    fn marker_write_failure_is_retryable_and_preserves_sources() {
        let mut fx = fixture("marker-failure");
        seed_sources(&fx);
        let entries = standard_plan(&fx);
        let before = snapshot_sources(&entries);
        let marker_parent = fx.journal_path.parent().unwrap().join("marker-parent");
        write_file(&marker_parent, b"not a directory");
        fx.marker = marker_parent.join("done.marker");
        plan(
            "marker-failure-retry",
            entries,
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap();

        let err = resume(&fx.journal_path).unwrap_err();
        assert!(err.to_string().contains("create marker dir"), "{err}");
        assert_eq!(
            load_journal(&fx.journal_path).unwrap().phase,
            Phase::Validated
        );
        assert!(!fx.marker.exists());
        assert_sources_unchanged(&before);

        std::fs::remove_file(&marker_parent).unwrap();
        let journal = resume(&fx.journal_path).unwrap();
        assert_eq!(journal.phase, Phase::Committed);
        assert_sources_unchanged(&before);
        drop_fixture(&fx);
    }

    #[test]
    fn resume_copies_validates_and_commits() {
        let fx = fixture("resume-full");
        seed_sources(&fx);
        plan(
            "mig-2",
            standard_plan(&fx),
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap();
        let journal = resume(&fx.journal_path).unwrap();
        assert_eq!(journal.phase, Phase::Committed);
        assert!(fx.marker.is_file());
        // 目标内容逐文件一致；源文件原样保留
        for entry in &journal.entries {
            assert!(entry.sha256.is_some());
            let copied = std::fs::read(&entry.target).unwrap();
            let source = std::fs::read(&entry.source).unwrap();
            assert_eq!(copied, source, "{}", entry.target.display());
            assert!(entry.source.is_file(), "源不得丢失");
        }
        drop_fixture(&fx);
    }

    #[test]
    fn resume_after_commit_is_noop() {
        let fx = fixture("resume-idempotent");
        seed_sources(&fx);
        plan(
            "mig-3",
            standard_plan(&fx),
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap();
        let first = resume(&fx.journal_path).unwrap();
        let mtime = std::fs::metadata(fx.new_root.join("脚本一.yaml"))
            .unwrap()
            .modified()
            .unwrap();
        let again = resume(&fx.journal_path).unwrap();
        assert_eq!(again.phase, Phase::Committed);
        assert_eq!(first.entries, again.entries);
        let mtime_after = std::fs::metadata(fx.new_root.join("脚本一.yaml"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(mtime, mtime_after, "Committed 后 resume 不得重写文件");
        drop_fixture(&fx);
    }

    #[test]
    fn resume_skips_already_copied_entries() {
        let fx = fixture("resume-partial");
        seed_sources(&fx);
        plan(
            "mig-4",
            standard_plan(&fx),
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap();

        // 模拟崩溃点：第一个文件已复制并记录 sha，其余未动（phase=Copying）
        let mut journal = load_journal(&fx.journal_path).unwrap();
        journal.phase = Phase::Copying;
        let first_target = journal.entries[0].target.clone();
        std::fs::create_dir_all(first_target.parent().unwrap()).unwrap();
        std::fs::copy(&journal.entries[0].source, &first_target).unwrap();
        journal.entries[0].sha256 = Some(sha256_file(&first_target).unwrap());
        save_journal(&journal, &fx.journal_path).unwrap();

        // 恢复推进：已复制条目不得重拷（改写源内容后目标必须保持旧内容）
        let before = std::fs::read(&first_target).unwrap();
        write_file(&journal.entries[0].source, b"steps: [changed]\n");
        let journal = resume(&fx.journal_path).unwrap();
        assert_eq!(journal.phase, Phase::Committed);
        let after = std::fs::read(&first_target).unwrap();
        assert_eq!(before, after, "已复制条目被重复复制");
        // 其余条目按新内容补齐
        assert!(
            std::fs::read(&journal.entries[1].target).unwrap()
                == std::fs::read(&journal.entries[1].source).unwrap()
        );
        drop_fixture(&fx);
    }

    #[test]
    fn tampered_staging_blocks_marker_and_keeps_in_progress() {
        let fx = fixture("tamper");
        seed_sources(&fx);
        plan(
            "mig-5",
            standard_plan(&fx),
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap();

        // 全部复制完成后篡改一个目标文件（模拟混合/损坏布局）
        let mut journal = load_journal(&fx.journal_path).unwrap();
        journal.phase = Phase::Copying;
        for entry in &mut journal.entries {
            std::fs::create_dir_all(entry.target.parent().unwrap()).unwrap();
            std::fs::copy(&entry.source, &entry.target).unwrap();
            entry.sha256 = Some(sha256_file(&entry.target).unwrap());
        }
        save_journal(&journal, &fx.journal_path).unwrap();
        write_file(&journal.entries[1].target, b"tampered");

        let err = resume(&fx.journal_path).unwrap_err();
        assert!(err.to_string().contains("hash mismatch"), "{err}");
        // 混合布局不得误标成功：无 marker、journal 停留在 Copying、源不丢
        assert!(!fx.marker.exists());
        let journal = load_journal(&fx.journal_path).unwrap();
        assert_eq!(journal.phase, Phase::Copying);
        assert!(journal.entries.iter().all(|e| e.source.is_file()));
        drop_fixture(&fx);
    }

    #[test]
    fn rollback_removes_staging_but_keeps_sources() {
        let fx = fixture("rollback");
        seed_sources(&fx);
        plan(
            "mig-6",
            standard_plan(&fx),
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap();
        // 推进到部分复制后回滚
        let mut journal = load_journal(&fx.journal_path).unwrap();
        journal.phase = Phase::Copying;
        let first_target = journal.entries[0].target.clone();
        std::fs::create_dir_all(first_target.parent().unwrap()).unwrap();
        std::fs::copy(&journal.entries[0].source, &first_target).unwrap();
        journal.entries[0].sha256 = Some(sha256_file(&first_target).unwrap());
        save_journal(&journal, &fx.journal_path).unwrap();
        // 回滚也要清掉原子写在崩溃边界留下的临时文件。
        let journal_tmp = atomic_temp_path(&fx.journal_path);
        let marker_tmp = atomic_temp_path(&fx.marker);
        write_file(&journal_tmp, b"partial journal");
        write_file(&marker_tmp, b"partial marker");

        let journal = rollback(&fx.journal_path).unwrap();
        assert_eq!(journal.phase, Phase::RolledBack);
        assert!(!first_target.exists(), "staging 产物应清理");
        assert!(!fx.marker.exists());
        assert!(!journal_tmp.exists(), "journal 临时文件应清理");
        assert!(!marker_tmp.exists(), "marker 临时文件应清理");
        for entry in &journal.entries {
            assert!(entry.source.is_file(), "源文件必须原样保留");
        }
        // 回滚后的 journal 拒绝继续 resume
        let err = resume(&fx.journal_path).unwrap_err();
        assert!(err.to_string().contains("rolled back"));
        drop_fixture(&fx);
    }

    #[test]
    fn rollback_refuses_after_commit() {
        let fx = fixture("rollback-after-commit");
        seed_sources(&fx);
        plan(
            "mig-7",
            standard_plan(&fx),
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap();
        resume(&fx.journal_path).unwrap();
        let err = rollback(&fx.journal_path).unwrap_err();
        assert!(err.to_string().contains("committed"), "{err}");
        // Committed 产物不受影响
        assert!(fx.marker.is_file());
        drop_fixture(&fx);
    }

    #[test]
    fn discover_copy_plan_maps_relative_layout_one_to_one() {
        let fx = fixture("discover");
        seed_sources(&fx);
        let entries = discover_copy_plan(&fx.old_root, &fx.new_root).unwrap();
        assert_eq!(entries.len(), 3);
        for entry in &entries {
            let relative = entry.source.strip_prefix(&fx.old_root).unwrap();
            assert_eq!(entry.target, fx.new_root.join(relative));
        }
        drop_fixture(&fx);
    }

    #[test]
    fn journal_atomic_write_leaves_no_half_files() {
        let fx = fixture("atomic");
        seed_sources(&fx);
        plan(
            "mig-8",
            standard_plan(&fx),
            fx.marker.clone(),
            &fx.journal_path,
        )
        .unwrap();
        let journal = resume(&fx.journal_path).unwrap();
        // 正式 journal 可解析、无残留 .tmp
        let loaded = load_journal(&fx.journal_path).unwrap();
        assert_eq!(loaded, journal);
        let tmp = PathBuf::from(format!("{}.tmp", fx.journal_path.display()));
        assert!(!tmp.exists(), "原子写不得残留临时文件");
        // marker 内容携带迁移 id
        let marker = std::fs::read_to_string(&fx.marker).unwrap();
        assert!(marker.contains("mig-8"));
        drop_fixture(&fx);
    }
}
