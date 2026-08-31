//! LCH-006：组件压缩包安全解压与原子安装。
//!
//! 拒绝：zip-slip（路径穿越）、解压炸弹（条目声明 + 实际写入双重上限）、
//! 符号链接条目、ADS 冒号、Windows 保留名、大小写碰撞、重复条目、
//! required_files 白名单之外的条目；解压完成后逐文件（size+sha256）校验，
//! 全过才允许 `install_staged` rename 进 `runtime/<id>/<version>/`
//! （目标已存在 = fail，不原地覆盖；跨卷 move 不在此路径，staging 与安装根同卷）。
//!
//! 选型说明：zip crate 的 `name()` 返回条目原始名（不清洗 `..`/绝对路径），
//! 本模块对每个条目名独立跑 manifest 同源 pathsafe 检查，不使用 `enclosed_name`
//! 的「清洗」语义——只拒绝、不改写。zip crate 在 Windows 上不会把 unix mode
//! 的符号链接落成 reparse point，但解压完成后仍全树扫描 reparse point 兜底。

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::digest::{verify_file, VerifyFileError};
use crate::manifest::model::RequiredFile;
use crate::manifest::pathsafe;
use crate::state::atomic::rename_with_retry;

const READ_BUF: usize = 64 * 1024;
/// NTFS reparse point 属性位（symlink/junction/mount point 共用）。
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

#[derive(Debug, Clone)]
pub struct ExtractOptions {
    /// zip 内所有文件条目未压缩字节总和上限（防解压炸弹；条目声明与实际写入双查）。
    pub max_total_uncompressed: u64,
    /// 单条目未压缩字节上限。
    pub max_file_uncompressed: u64,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        // 与 manifest Limits 对齐：单压缩包 2 GiB / 单文件 1 GiB
        Self {
            max_total_uncompressed: 2_147_483_648,
            max_file_uncompressed: 1_073_741_824,
        }
    }
}

#[derive(Debug)]
pub enum ArchiveError {
    Open(std::io::Error),
    ZipFormat(String),
    /// 条目名违反路径安全（reason = manifest 同源错误码）。
    DangerousEntry {
        entry: String,
        reason: &'static str,
    },
    SymlinkEntry {
        entry: String,
    },
    CaseCollision {
        first: String,
        second: String,
    },
    DuplicateEntry {
        entry: String,
    },
    UnexpectedEntry {
        entry: String,
    },
    BombTotal {
        declared: u64,
        limit: u64,
    },
    BombFile {
        entry: String,
        declared: u64,
        limit: u64,
    },
    /// 条目实际解压字节数与声明不符（谎报 header）。
    SizeCorrupt {
        entry: String,
        declared: u64,
        actual: u64,
    },
    /// 解压产物中出现 symlink/reparse point。
    ReparsePoint {
        path: PathBuf,
    },
    RequiredFileMissing {
        path: String,
    },
    RequiredFileSize {
        path: String,
        actual: u64,
        expected: u64,
    },
    RequiredFileHash {
        path: String,
        actual: String,
        expected: String,
    },
    StagingInvalid {
        path: PathBuf,
        reason: String,
    },
    TargetExists {
        path: PathBuf,
    },
    Io(std::io::Error),
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArchiveError::Open(e) => write!(f, "打开压缩包失败: {e}"),
            ArchiveError::ZipFormat(e) => write!(f, "压缩包格式非法: {e}"),
            ArchiveError::DangerousEntry { entry, reason } => {
                write!(f, "条目路径不安全（{reason}）: {entry:?}")
            }
            ArchiveError::SymlinkEntry { entry } => {
                write!(f, "条目为符号链接: {entry:?}")
            }
            ArchiveError::CaseCollision { first, second } => {
                write!(
                    f,
                    "大小写碰撞: {first:?} 与 {second:?}（Windows 大小写不敏感）"
                )
            }
            ArchiveError::DuplicateEntry { entry } => write!(f, "条目重复: {entry:?}"),
            ArchiveError::UnexpectedEntry { entry } => {
                write!(f, "条目不在 required_files 白名单内: {entry:?}")
            }
            ArchiveError::BombTotal { declared, limit } => {
                write!(f, "解压炸弹：条目未压缩总声明 {declared} 超过上限 {limit}")
            }
            ArchiveError::BombFile {
                entry,
                declared,
                limit,
            } => {
                write!(
                    f,
                    "解压炸弹：条目 {entry:?} 声明 {declared} 超过单文件上限 {limit}"
                )
            }
            ArchiveError::SizeCorrupt {
                entry,
                declared,
                actual,
            } => {
                write!(
                    f,
                    "条目 {entry:?} 实际解压 {actual} 字节与声明 {declared} 不符"
                )
            }
            ArchiveError::ReparsePoint { path } => {
                write!(f, "解压产物包含符号链接/reparse point: {}", path.display())
            }
            ArchiveError::RequiredFileMissing { path } => {
                write!(f, "required_files 缺失: {path:?}")
            }
            ArchiveError::RequiredFileSize {
                path,
                actual,
                expected,
            } => {
                write!(
                    f,
                    "required_files {path:?} size 不符（实际 {actual}，声明 {expected}）"
                )
            }
            ArchiveError::RequiredFileHash {
                path,
                actual,
                expected,
            } => {
                write!(
                    f,
                    "required_files {path:?} sha256 不符（实际 {actual}，声明 {expected}）"
                )
            }
            ArchiveError::StagingInvalid { path, reason } => {
                write!(f, "staging 目录不可用 {}: {reason}", path.display())
            }
            ArchiveError::TargetExists { path } => {
                write!(f, "目标目录已存在（不原地覆盖）: {}", path.display())
            }
            ArchiveError::Io(e) => write!(f, "IO 失败: {e}"),
        }
    }
}

fn zip_err(e: zip::result::ZipError) -> ArchiveError {
    ArchiveError::ZipFormat(e.to_string())
}

/// 把组件压缩包安全解压到 `staging_dir`（须为空目录，不存在则创建）。
/// `required` 为白名单 + 逐文件校验清单：条目名必须与白名单完全一致（区分大小写）。
pub fn extract_component_zip(
    zip_path: &Path,
    staging_dir: &Path,
    required: &[RequiredFile],
    opts: &ExtractOptions,
) -> Result<(), ArchiveError> {
    prepare_fresh_staging(staging_dir)?;
    let mut file = fs::File::open(zip_path).map_err(ArchiveError::Open)?;
    // 读侧前置防线：zip crate 的读取器按条目名建索引（IndexMap），重复条目会被
    // 静默折叠——先独立清点 central directory 原始条目数，与折叠后不一致即拒绝。
    let raw_entries = raw_central_entry_count(&mut file).map_err(ArchiveError::Io)?;
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(ArchiveError::Io)?;
    let mut archive = zip::ZipArchive::new(file).map_err(zip_err)?;
    if let Some(raw) = raw_entries {
        if raw != archive.len() {
            return Err(ArchiveError::DuplicateEntry {
                entry: format!(
                    "<central directory 声明 {raw} 项，读取侧折叠为 {} 项（存在重复条目）>",
                    archive.len()
                ),
            });
        }
    }

    // ---- pass 1：全条目命名/结构校验（先整体校验，再写任何字节） ----
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut declared_total: u64 = 0;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(zip_err)?;
        let name = entry.name().to_string();
        let clean = name.trim_end_matches('/');
        if let Some(reason) = pathsafe::check_single_path(clean) {
            return Err(ArchiveError::DangerousEntry {
                entry: name.clone(),
                reason,
            });
        }
        if entry.is_symlink() {
            return Err(ArchiveError::SymlinkEntry {
                entry: name.clone(),
            });
        }
        // 文件条目与目录条目共用同一命名空间（Windows 大小写不敏感）
        let key = clean.to_lowercase();
        if let Some(prev) = seen.get(&key) {
            return Err(if prev == clean {
                ArchiveError::DuplicateEntry {
                    entry: name.clone(),
                }
            } else {
                ArchiveError::CaseCollision {
                    first: prev.clone(),
                    second: name.clone(),
                }
            });
        }
        seen.insert(key, clean.to_string());

        if entry.is_dir() {
            if !required
                .iter()
                .any(|rf| rf.path.starts_with(&format!("{clean}/")))
            {
                return Err(ArchiveError::UnexpectedEntry {
                    entry: name.clone(),
                });
            }
        } else {
            if !required.iter().any(|rf| rf.path == clean) {
                return Err(ArchiveError::UnexpectedEntry {
                    entry: name.clone(),
                });
            }
            if entry.size() > opts.max_file_uncompressed {
                return Err(ArchiveError::BombFile {
                    entry: name.clone(),
                    declared: entry.size(),
                    limit: opts.max_file_uncompressed,
                });
            }
            declared_total += entry.size();
        }
    }
    if declared_total > opts.max_total_uncompressed {
        return Err(ArchiveError::BombTotal {
            declared: declared_total,
            limit: opts.max_total_uncompressed,
        });
    }

    // ---- pass 2：解压（实际写入字节数守门，防谎报 header） ----
    let mut actual_total: u64 = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(zip_err)?;
        let name = entry.name().to_string();
        if entry.is_dir() {
            fs::create_dir_all(staging_dir.join(name.trim_end_matches('/')))
                .map_err(ArchiveError::Io)?;
            continue;
        }
        let target = staging_dir.join(&name);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(ArchiveError::Io)?;
        }
        let mut out = fs::File::create(&target).map_err(ArchiveError::Io)?;
        let declared = entry.size();
        let mut written: u64 = 0;
        let mut buf = vec![0u8; READ_BUF];
        loop {
            let n = entry.read(&mut buf).map_err(ArchiveError::Io)?;
            if n == 0 {
                break;
            }
            written += n as u64;
            actual_total += n as u64;
            if written > opts.max_file_uncompressed || actual_total > opts.max_total_uncompressed {
                let _ = fs::remove_file(&target);
                return Err(ArchiveError::BombFile {
                    entry: name.clone(),
                    declared: written,
                    limit: opts.max_file_uncompressed,
                });
            }
            out.write_all(&buf[..n]).map_err(ArchiveError::Io)?;
        }
        if written != declared {
            return Err(ArchiveError::SizeCorrupt {
                entry: name,
                declared,
                actual: written,
            });
        }
    }

    // ---- pass 3：全树扫描 reparse point（symlink/junction 兜底） ----
    scan_no_reparse(staging_dir)?;

    // ---- pass 4：required_files 逐文件（size+sha256）校验 ----
    for rf in required {
        let p = staging_dir.join(&rf.path);
        let expected_size = u64::try_from(rf.size).map_err(|_| ArchiveError::RequiredFileSize {
            path: rf.path.clone(),
            actual: 0,
            expected: u64::MAX,
        })?;
        match verify_file(&p, &rf.sha256, expected_size) {
            Ok(()) => {}
            Err(VerifyFileError::NotFound) => {
                return Err(ArchiveError::RequiredFileMissing {
                    path: rf.path.clone(),
                });
            }
            Err(VerifyFileError::SizeMismatch { actual, expected }) => {
                return Err(ArchiveError::RequiredFileSize {
                    path: rf.path.clone(),
                    actual,
                    expected,
                });
            }
            Err(VerifyFileError::HashMismatch { actual, expected }) => {
                return Err(ArchiveError::RequiredFileHash {
                    path: rf.path.clone(),
                    actual,
                    expected,
                });
            }
            Err(VerifyFileError::Io(e)) => return Err(ArchiveError::Io(e)),
        }
    }
    Ok(())
}

/// staging → 最终目录的原子安装；目标已存在 = fail（不原地覆盖，契约 §2）。
pub fn install_staged(staging: &Path, target: &Path) -> Result<(), ArchiveError> {
    if target.exists() {
        return Err(ArchiveError::TargetExists {
            path: target.to_path_buf(),
        });
    }
    rename_with_retry(staging, target).map_err(ArchiveError::Io)
}

/// 读侧前置防线：独立清点 zip central directory 的原始条目数。
/// 返回 `None` 表示找不到经典 EOCD（如 zip64）——交给 zip crate 处理，不做清点。
/// 实现：从文件尾部定位 EOCD（PK\x05\x06）→ 取 cd_offset/cd_size → 逐个
/// 按 46 字节固定头 + 变长字段走完 central directory，统计 PK\x01\x02 条目数。
fn raw_central_entry_count(file: &mut fs::File) -> std::io::Result<Option<usize>> {
    use std::io::{Seek, SeekFrom};
    let size = file.seek(SeekFrom::End(0))?;
    let tail_len = size.min(66_000) as usize;
    file.seek(SeekFrom::Start(size - tail_len as u64))?;
    let mut tail = vec![0u8; tail_len];
    file.read_exact(&mut tail)?;
    const EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    let Some(pos) = tail.windows(4).rposition(|w| w == EOCD_SIG) else {
        return Ok(None);
    };
    if tail.len() < pos + 22 {
        return Ok(None);
    }
    let read_u16 = |at: usize| u16::from_le_bytes([tail[at], tail[at + 1]]);
    let read_u32 =
        |at: usize| u32::from_le_bytes([tail[at], tail[at + 1], tail[at + 2], tail[at + 3]]);
    // EOCD 固定 22 字节 + 注释长度必须恰好耗尽缓冲（容忍前置垃圾）
    let comment_len = read_u16(pos + 20) as usize;
    if pos + 22 + comment_len != tail.len() {
        return Ok(None);
    }
    let cd_size = read_u32(pos + 12) as u64;
    let cd_offset = read_u32(pos + 16) as u64;
    if cd_offset == u64::from(u32::MAX) || cd_size == u64::from(u32::MAX) {
        return Ok(None); // zip64：交给 zip crate
    }
    file.seek(SeekFrom::Start(cd_offset))?;
    let mut cd = vec![0u8; cd_size as usize];
    file.read_exact(&mut cd)?;
    const CDH_SIG: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
    let mut count = 0usize;
    let mut at = 0usize;
    while at + 46 <= cd.len() {
        if cd[at..at + 4] != CDH_SIG {
            return Ok(None); // 结构不认识：不强行判定，交给 zip crate
        }
        let name_len = u16::from_le_bytes([cd[at + 28], cd[at + 29]]) as usize;
        let extra_len = u16::from_le_bytes([cd[at + 30], cd[at + 31]]) as usize;
        let comment_len = u16::from_le_bytes([cd[at + 32], cd[at + 33]]) as usize;
        at += 46 + name_len + extra_len + comment_len;
        count += 1;
    }
    if at != cd.len() {
        return Ok(None);
    }
    Ok(Some(count))
}

fn prepare_fresh_staging(staging: &Path) -> Result<(), ArchiveError> {
    if staging.exists() {
        if !staging.is_dir() {
            return Err(ArchiveError::StagingInvalid {
                path: staging.to_path_buf(),
                reason: "同名文件已存在".to_string(),
            });
        }
        let empty = fs::read_dir(staging)
            .map_err(ArchiveError::Io)?
            .next()
            .is_none();
        if !empty {
            return Err(ArchiveError::StagingInvalid {
                path: staging.to_path_buf(),
                reason: "目录非空".to_string(),
            });
        }
    } else {
        fs::create_dir_all(staging).map_err(ArchiveError::Io)?;
    }
    Ok(())
}

fn scan_no_reparse(dir: &Path) -> Result<(), ArchiveError> {
    for entry in fs::read_dir(dir).map_err(ArchiveError::Io)? {
        let entry = entry.map_err(ArchiveError::Io)?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(ArchiveError::Io)?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(ArchiveError::ReparsePoint { path });
        }
        if meta.is_dir() {
            scan_no_reparse(&path)?;
        }
    }
    Ok(())
}
