//! Package 归档（.gamerpkg = zip）：导入校验提取 + 可复现导出打包。
//!
//! 导入校验流程（plan §8）：字节上限 → ZIP 中央目录解析（条目数/重复路径/
//! 穿越名/加密条目拒绝）→ 解压到临时 staging → 验证 package.toml → 验证 id
//! （= 目标目录名）→ 原子安装/替换。归档布局白名单：`package.toml`（自然
//! 第一）+ `shared/**` + `plugins/<plugin-id>/**` + `media/**`（Phase 8 §11.1
//! 媒体分发：`media/index.json` 引用登记 + `media/files/<sha256>` 素材字节，
//! 其余形态拒绝），其余顶层条目一律拒绝。**布局白名单扩展是破坏性调整**：
//! 旧归档（无 media/）仍可导入；旧版本服务端会拒绝含 media/ 的新归档。
//!
//! 导出（plan §11）：内容 = package.toml + shared/ + plugins/ + media/，条目
//! 按相对路径排序、UTF-8 文件名、Deflated、固定 mtime（2000-01-01）→ 相同
//! 输入产生逐字节相同归档（相同 SHA-256）。默认导出**不含**原始素材字节
//! （只有 `media/index.json` 引用登记）；`include_media = true` 时追加
//! `media/files/<sha256>`。导出后自检：归档重走导入侧校验 + 条目集合核对。

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::core::fs::archive_validation::{
    IMPORT_MAX_ARCHIVE_BYTES, IMPORT_MAX_ENTRIES, IMPORT_MAX_TOTAL_BYTES,
};

use crate::media::{MediaProbeMeta, MediaService};
use crate::resources::archive_limits;
use crate::resources::{parse_package_toml, validate_scope_id, PackageManifest, PackageStore};

use crate::resources::archive_limits::MAX_FILE_BYTES as MAX_PACKAGE_FILE_BYTES;
/// 媒体素材单文件上限（媒体是内容寻址 + 索引声明大小的受控目录，不走
/// 插件资源 10 MiB 单文件上限；仍受归档总量 100 MiB 预算约束）。
pub const MAX_MEDIA_FILE_BYTES: usize = 90 * 1024 * 1024;
/// media/index.json 上限（防御：条目数另有上限）。
const MAX_MEDIA_INDEX_BYTES: usize = 1024 * 1024;
/// 媒体索引条目上限（100 MiB 预算下实际不可能触达，纯防御）。
const MAX_MEDIA_ENTRIES: usize = 1024;
const MANIFEST_BYTES_LIMIT: usize = 256 * 1024;
/// 归档内媒体索引路径与素材目录（白名单受控形态）。
pub const MEDIA_INDEX_ENTRY: &str = "media/index.json";
pub const MEDIA_FILES_PREFIX: &str = "media/files/";
/// 固定 mtime：2000-01-01 00:00:00（DOS 日期范围 1980–2107 内，可复现打包）。
fn fixed_mtime() -> zip::DateTime {
    zip::DateTime::from_date_and_time(2000, 1, 1, 0, 0, 0).expect("常量时间在 DOS 日期范围内")
}

/// 归档校验失败（含机器可判定的变体；HTTP 层映射 400/413）。
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("归档超过大小上限（{actual} > {limit} 字节）")]
    ArchiveTooLarge { actual: usize, limit: usize },
    #[error("归档非法: {0}")]
    Invalid(String),
    #[error("package.toml 非法: {0}")]
    Manifest(String),
    #[error("配置不存在: {0}")]
    NotFound(String),
    #[error("IO 错误: {0}")]
    Io(#[from] io::Error),
    #[error("ZIP 错误: {0}")]
    Zip(#[from] zip::result::ZipError),
}

/// 归档内的一个条目（仅需未压缩大小做总量预算校验）。
#[derive(Debug)]
struct CentralEntry {
    uncompressed_size: u64,
}

/// 校验归档整体安全并读取 package.toml 字节（解压前先行，坏包不落盘）。
pub fn validate_and_read_manifest(bytes: &[u8]) -> Result<Vec<u8>, ArchiveError> {
    if bytes.len() > IMPORT_MAX_ARCHIVE_BYTES {
        return Err(ArchiveError::ArchiveTooLarge {
            actual: bytes.len(),
            limit: IMPORT_MAX_ARCHIVE_BYTES,
        });
    }
    let entries = parse_central_directory(bytes)?;
    if entries.is_empty() {
        return Err(ArchiveError::Invalid("归档不能为空".to_string()));
    }
    let declared_total = entries
        .iter()
        .try_fold(0u64, |total, entry| {
            total.checked_add(entry.uncompressed_size)
        })
        .ok_or_else(|| ArchiveError::Invalid("解压总大小溢出".to_string()))?;
    if declared_total > IMPORT_MAX_TOTAL_BYTES as u64 {
        return Err(ArchiveError::Invalid(format!(
            "声明解压总大小 {declared_total} 字节超过上限 {IMPORT_MAX_TOTAL_BYTES} 字节"
        )));
    }
    let mut archive = ZipArchive::new(io::Cursor::new(bytes))?;
    if archive.len() != entries.len() {
        return Err(ArchiveError::Invalid(
            "ZIP 中央目录条目数与读取器不一致".to_string(),
        ));
    }
    let mut manifest = None;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.encrypted() {
            return Err(ArchiveError::Invalid(format!(
                "不允许加密条目: {}",
                entry.name()
            )));
        }
        if entry.name() == archive_limits::MANIFEST_NAME {
            if entry.is_dir() {
                return Err(ArchiveError::Invalid("package.toml 必须是文件".to_string()));
            }
            let size = usize::try_from(entry.size())
                .map_err(|_| ArchiveError::Invalid("package.toml 大小溢出".to_string()))?;
            if size > MANIFEST_BYTES_LIMIT {
                return Err(ArchiveError::Invalid(format!(
                    "package.toml 超过 {MANIFEST_BYTES_LIMIT} 字节上限"
                )));
            }
            let mut content = Vec::with_capacity(size);
            entry.read_to_end(&mut content)?;
            parse_package_toml(&content).map_err(|e| ArchiveError::Manifest(e.to_string()))?;
            manifest = Some(content);
        }
    }
    manifest.ok_or_else(|| ArchiveError::Invalid("归档根目录缺少 package.toml".to_string()))
}

/// 解压归档到 staging 目录（目录安全已在中央目录解析期完成；entry 名 →
/// staging 内相对路径逐段重建）。staging 必须为空目录。
pub fn extract_archive(bytes: &[u8], staging: &Path) -> Result<PackageManifest, ArchiveError> {
    let entries = parse_central_directory(bytes)?;
    let mut archive = ZipArchive::new(io::Cursor::new(bytes))?;
    if archive.len() != entries.len() {
        return Err(ArchiveError::Invalid(
            "ZIP 中央目录条目数与读取器不一致".to_string(),
        ));
    }
    fs::create_dir_all(staging)?;
    let mut extracted_total = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.encrypted() || entry.is_symlink() {
            return Err(ArchiveError::Invalid(format!(
                "不允许加密或符号链接条目: {}",
                entry.name()
            )));
        }
        let is_dir = entry.is_dir();
        let relative = archive_relative_path(entry.name(), is_dir)?;
        let destination = append_relative_path(staging, &relative);
        if is_dir {
            fs::create_dir_all(destination)?;
            continue;
        }
        let limit = per_file_limit(&relative);
        if entry.size() > limit as u64 {
            return Err(ArchiveError::Invalid(format!(
                "文件 {} 超过单文件上限 {limit} 字节",
                entry.name()
            )));
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)?;
        let mut copied = 0usize;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = entry.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            copied = copied
                .checked_add(read)
                .ok_or_else(|| ArchiveError::Invalid("文件大小溢出".to_string()))?;
            extracted_total = extracted_total
                .checked_add(read)
                .ok_or_else(|| ArchiveError::Invalid("解压总大小溢出".to_string()))?;
            if copied > limit || extracted_total > IMPORT_MAX_TOTAL_BYTES {
                return Err(ArchiveError::Invalid("实际解压大小超过上限".to_string()));
            }
            output.write_all(&buffer[..read])?;
        }
        output.flush()?;
        output.sync_all()?;
    }
    sync_directory(staging)?;
    // manifest 验证（id 与目标目录名的一致性由调用方核对——staging 阶段先
    // 解析出 manifest 返回）
    let manifest_bytes = fs::read(staging.join(archive_limits::MANIFEST_NAME))?;
    parse_package_toml(&manifest_bytes).map_err(|e| ArchiveError::Manifest(e.to_string()))
}

/// 顶层布局白名单：package.toml / shared/** / plugins/<plugin-id>/** /
/// media/index.json + media/files/<64hex>（媒体分发受控目录，Phase 8 §11.1）。
fn archive_relative_path(name: &str, is_dir: bool) -> Result<Vec<String>, ArchiveError> {
    let normalized = if is_dir {
        name.trim_end_matches('/')
    } else {
        name
    };
    if normalized == archive_limits::MANIFEST_NAME {
        return Ok(vec![normalized.to_string()]);
    }
    let mut segments = normalized.split('/').collect::<Vec<_>>();
    if segments.is_empty() {
        return Err(ArchiveError::Invalid(format!("归档路径为空: {name:?}")));
    }
    let top = segments.remove(0);
    match top {
        "shared" => {}
        "plugins" => {
            let Some(plugin) = segments.first() else {
                return Err(ArchiveError::Invalid(
                    "plugins/ 下必须带 plugin-id 目录".to_string(),
                ));
            };
            validate_scope_id("plugin id", plugin)
                .map_err(|e| ArchiveError::Invalid(e.to_string()))?;
        }
        "media" => {
            // 受控形态：media/index.json 与 media/files/<sha256>，其余拒绝
            match segments.first().copied() {
                Some("index.json") if segments.len() == 1 => {}
                Some("files") if segments.len() == 2 => {
                    if !is_sha256_segment(segments[1]) {
                        return Err(ArchiveError::Invalid(format!(
                            "media/files/ 条目必须是 64 位 hex 内容哈希: {name:?}"
                        )));
                    }
                }
                _ => {
                    return Err(ArchiveError::Invalid(format!(
                        "media/ 下只允许 index.json 与 files/<sha256>，拒绝: {name:?}"
                    )));
                }
            }
        }
        other => {
            return Err(ArchiveError::Invalid(format!(
                "归档顶层条目 {other:?} 不在白名单内（只允许 package.toml / shared/ / plugins/ / media/）"
            )));
        }
    }
    let mut out = vec![top.to_string()];
    for seg in segments {
        let seg = super::resources::sanitize_rel_path(seg)
            .map_err(|e| ArchiveError::Invalid(format!("归档路径段 {seg:?} 非法: {e}")))?
            .pop()
            .expect("单段解析结果非空");
        out.push(seg);
    }
    Ok(out)
}

/// 单文件上限：media/files/** 素材走媒体专用上限，其余走插件资源上限。
fn per_file_limit(relative: &[String]) -> usize {
    if relative.first() == Some(&"media".to_string()) {
        MAX_MEDIA_FILE_BYTES
    } else {
        MAX_PACKAGE_FILE_BYTES
    }
}

/// sha256 hex 段校验（media/files/<sha256>，64 位 hex；小写归一由调用方做）。
fn is_sha256_segment(seg: &str) -> bool {
    crate::media::is_sha256_hex(seg)
}

// ---------------------------------------------------------------------------
// 媒体索引（media/index.json）：逻辑 id → 内容 sha256 映射（Phase 8 §11.1）
// ---------------------------------------------------------------------------

/// 媒体索引 schema 版本。
pub const MEDIA_INDEX_SCHEMA_VERSION: u32 = 1;

/// 归档媒体索引（默认导出即携带：引用登记/缺失标注；include_media 时同名
/// 条目附 `media/files/<sha256>` 字节）。`plugin_id`/`kind` 描述引用方。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaIndex {
    pub schema_version: u32,
    pub entries: Vec<MediaIndexEntry>,
}

/// 索引条目。`included` 与 `media/files/<sha256>` 字节存在性必须一致；
/// `probe` 是导出侧探测元数据（导入原样恢复，不重跑 ffprobe）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaIndexEntry {
    /// 逻辑媒体 id（导入尽量原样保留，使包内项目引用导入即有效）。
    pub id: String,
    /// 展示名（原始文件名）。
    pub name: String,
    /// 内容身份（64 位 hex）。
    pub sha256: String,
    /// 原始字节数。
    pub size: u64,
    /// 引用方插件 id。
    pub plugin_id: String,
    /// 引用用途（如 `project`）。
    pub kind: String,
    /// 归档是否携带素材字节。
    pub included: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<MediaProbeMeta>,
}

/// 解析并校验媒体索引（形状/语法校验；与归档字节的一致性由导入方核对）。
pub fn parse_media_index(bytes: &[u8]) -> Result<MediaIndex, ArchiveError> {
    if bytes.len() > MAX_MEDIA_INDEX_BYTES {
        return Err(ArchiveError::Invalid(format!(
            "media/index.json 超过 {MAX_MEDIA_INDEX_BYTES} 字节上限"
        )));
    }
    let index: MediaIndex = serde_json::from_slice(bytes)
        .map_err(|e| ArchiveError::Invalid(format!("media/index.json 解析失败: {e}")))?;
    if index.schema_version != MEDIA_INDEX_SCHEMA_VERSION {
        return Err(ArchiveError::Invalid(format!(
            "media/index.json schema_version 不受支持: {}",
            index.schema_version
        )));
    }
    if index.entries.len() > MAX_MEDIA_ENTRIES {
        return Err(ArchiveError::Invalid(format!(
            "media/index.json 条目数 {} 超过上限 {MAX_MEDIA_ENTRIES}",
            index.entries.len()
        )));
    }
    let mut seen = HashSet::new();
    for entry in &index.entries {
        if !crate::media::is_valid_media_id(&entry.id) {
            return Err(ArchiveError::Invalid(format!(
                "media/index.json 条目 id 非法: {:?}",
                entry.id
            )));
        }
        if !seen.insert(entry.id.clone()) {
            return Err(ArchiveError::Invalid(format!(
                "media/index.json 存在重复逻辑 id: {}",
                entry.id
            )));
        }
        let name = entry.name.trim();
        if name.is_empty() || name.len() > 200 || name.chars().any(char::is_control) {
            return Err(ArchiveError::Invalid(format!(
                "media/index.json 条目 name 非法: {:?}",
                entry.name
            )));
        }
        if !crate::media::is_sha256_hex(&entry.sha256) {
            return Err(ArchiveError::Invalid(format!(
                "media/index.json 条目 {}/sha256 必须是 64 位 hex",
                entry.id
            )));
        }
        if entry.included && entry.size > MAX_MEDIA_FILE_BYTES as u64 {
            return Err(ArchiveError::Invalid(format!(
                "媒体素材 {} 超过单文件上限 {MAX_MEDIA_FILE_BYTES} 字节",
                entry.id
            )));
        }
        validate_scope_id("media entry plugin_id", &entry.plugin_id)
            .map_err(|e| ArchiveError::Invalid(format!("media/index.json: {e}")))?;
        let kind = entry.kind.trim();
        if kind.is_empty() || kind.len() > 64 || kind.chars().any(char::is_control) {
            return Err(ArchiveError::Invalid(format!(
                "media/index.json 条目 {}/kind 非法",
                entry.id
            )));
        }
    }
    Ok(index)
}

/// 索引条目对应的归档内素材路径。
pub fn media_file_entry(sha256: &str) -> String {
    format!("{MEDIA_FILES_PREFIX}{}", sha256.to_ascii_lowercase())
}

fn append_relative_path(root: &Path, relative: &[String]) -> PathBuf {
    relative
        .iter()
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

fn parse_central_directory(bytes: &[u8]) -> Result<Vec<CentralEntry>, ArchiveError> {
    let eocd = find_end_of_central_directory(bytes)?;
    let disk = read_u16(bytes, eocd + 4)?;
    let central_disk = read_u16(bytes, eocd + 6)?;
    let disk_entries = read_u16(bytes, eocd + 8)?;
    let total_entries = read_u16(bytes, eocd + 10)?;
    let central_size = usize::try_from(read_u32(bytes, eocd + 12)?).unwrap_or(usize::MAX);
    let central_offset = usize::try_from(read_u32(bytes, eocd + 16)?).unwrap_or(usize::MAX);
    if disk != 0 || central_disk != 0 || disk_entries != total_entries {
        return Err(ArchiveError::Invalid("不支持多磁盘 ZIP".to_string()));
    }
    if total_entries == u16::MAX
        || central_size == u32::MAX as usize
        || central_offset == u32::MAX as usize
    {
        return Err(ArchiveError::Invalid("不支持 ZIP64 归档".to_string()));
    }
    let count = usize::from(total_entries);
    if count > IMPORT_MAX_ENTRIES {
        return Err(ArchiveError::Invalid(format!(
            "条目数 {count} 超过上限 {IMPORT_MAX_ENTRIES}"
        )));
    }
    let central_end = central_offset
        .checked_add(central_size)
        .ok_or_else(|| ArchiveError::Invalid("中央目录范围溢出".to_string()))?;
    if central_end != eocd || central_end > bytes.len() {
        return Err(ArchiveError::Invalid("中央目录范围无效".to_string()));
    }
    let mut entries = Vec::with_capacity(count);
    let mut seen = HashSet::with_capacity(count);
    let mut cursor = central_offset;
    for _ in 0..count {
        if read_u32(bytes, cursor)? != 0x0201_4b50 {
            return Err(ArchiveError::Invalid("中央目录条目标头无效".to_string()));
        }
        let uncompressed_size = u64::from(read_u32(bytes, cursor + 24)?);
        let name_len = usize::from(read_u16(bytes, cursor + 28)?);
        let extra_len = usize::from(read_u16(bytes, cursor + 30)?);
        let comment_len = usize::from(read_u16(bytes, cursor + 32)?);
        let header_end = cursor
            .checked_add(46)
            .and_then(|end| end.checked_add(name_len))
            .and_then(|end| end.checked_add(extra_len))
            .and_then(|end| end.checked_add(comment_len))
            .ok_or_else(|| ArchiveError::Invalid("中央目录条目范围溢出".to_string()))?;
        if header_end > central_end {
            return Err(ArchiveError::Invalid("中央目录条目越界".to_string()));
        }
        let name_start = cursor + 46;
        let name = std::str::from_utf8(&bytes[name_start..name_start + name_len])
            .map_err(|_| ArchiveError::Invalid("归档路径必须是 UTF-8".to_string()))?;
        let is_dir = name.ends_with('/');
        let normalized = if is_dir {
            name.trim_end_matches('/')
        } else {
            name
        };
        // 穿越与绝对路径拒绝（白名单检查前先做粗筛，错误信息更直白）
        if normalized.contains('\\')
            || normalized.contains("..")
            || normalized.starts_with('/')
            || normalized.contains(':')
        {
            return Err(ArchiveError::Invalid(format!("归档路径非法: {name:?}")));
        }
        if !seen.insert(normalized.to_string()) {
            return Err(ArchiveError::Invalid(format!(
                "归档存在重复路径: {normalized}"
            )));
        }
        // 顶层布局白名单在中央目录解析期统一执行（validate 与 extract 同一语义）
        let is_dir_entry = name.ends_with('/');
        archive_relative_path(normalized, is_dir_entry)?;
        entries.push(CentralEntry { uncompressed_size });
        cursor = header_end;
    }
    if cursor != central_end {
        return Err(ArchiveError::Invalid(
            "中央目录包含无法解析的尾部数据".to_string(),
        ));
    }
    Ok(entries)
}

fn find_end_of_central_directory(bytes: &[u8]) -> Result<usize, ArchiveError> {
    if bytes.len() < 22 {
        return Err(ArchiveError::Invalid("缺少 ZIP 结束目录".to_string()));
    }
    let first = bytes.len().saturating_sub(22 + u16::MAX as usize);
    for position in (first..=bytes.len() - 22).rev() {
        if read_u32(bytes, position)? != 0x0605_4b50 {
            continue;
        }
        let comment_len = usize::from(read_u16(bytes, position + 20)?);
        if position + 22 + comment_len == bytes.len() {
            return Ok(position);
        }
    }
    Err(ArchiveError::Invalid("缺少有效 ZIP 结束目录".to_string()))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ArchiveError> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| ArchiveError::Invalid("ZIP 字段越界".to_string()))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ArchiveError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| ArchiveError::Invalid("ZIP 字段越界".to_string()))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 导出（可复现打包）
// ---------------------------------------------------------------------------

/// 导出产物：归档字节 + manifest + SHA-256。
#[derive(Debug)]
pub struct BuiltPackage {
    pub manifest: PackageManifest,
    pub archive: Vec<u8>,
    pub sha256: String,
}

/// 包目录内的一个待打包文件：归档内相对路径 + 磁盘绝对路径。
struct CollectedFile {
    name: String,
    absolute: PathBuf,
}

/// 从包目录递归收集全部文件（排序、跳过隐藏文件/目录）。`sub` 限定子目录
/// （空串 = 整包）。顶层 `media/` 不收集——媒体索引与素材字节由导出器按
/// 当前媒体库引用状态重新生成（包目录内的 index.json 可能已过期）。
fn collect_dir(root: &Path, prefix: &str, out: &mut Vec<CollectedFile>) {
    let dir = if prefix.is_empty() {
        root.to_path_buf()
    } else {
        root.join(prefix)
    };
    let Ok(rd) = fs::read_dir(&dir) else {
        return;
    };
    for entry in rd.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        if name.starts_with('.') || name == archive_limits::MANIFEST_NAME {
            // 隐藏文件/临时文件不进包；manifest 由打包器显式写入（防重复条目）
            continue;
        }
        if prefix.is_empty() && name == "media" {
            continue; // 媒体分发目录由导出器重建
        }
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let path = entry.path();
        if path.is_dir() {
            collect_dir(root, &rel, out);
        } else if path.is_file() {
            out.push(CollectedFile {
                name: rel,
                absolute: path,
            });
        }
    }
}

/// 导出一个 Package 为 .gamerpkg 归档（package.toml 自然第一 + shared/ +
/// plugins/ + media/ 按路径排序；固定 mtime 可复现）。包不存在 → NotFound。
///
/// `media` 给出时查询该包名下的媒体引用并写入 `media/index.json`（引用
/// 登记/缺失标注）；`include_media = true` 时追加 `media/files/<sha256>`
/// 素材字节（默认不含原始大视频）。素材文件缺失/状态非 Ready → 该条目
/// `included=false`（缺失标注，不阻断导出）。
pub fn export_package(
    store: &PackageStore,
    pkg: &str,
    media: Option<&MediaService>,
    include_media: bool,
) -> Result<BuiltPackage, ArchiveError> {
    let manifest = store
        .try_manifest(pkg)
        .map_err(|e| ArchiveError::Invalid(e.to_string()))?
        .ok_or_else(|| ArchiveError::NotFound(pkg.to_string()))?;
    let dir = store
        .package_dir(pkg)
        .map_err(|e| ArchiveError::Invalid(e.to_string()))?;
    let mut files = Vec::new();
    collect_dir(&dir, "", &mut files);

    // 媒体引用 → 索引条目（+ 可选素材字节）
    let mut index = MediaIndex {
        schema_version: MEDIA_INDEX_SCHEMA_VERSION,
        entries: Vec::new(),
    };
    let mut media_files: Vec<(String, PathBuf)> = Vec::new(); // (sha256, 原文件路径)
    let mut media_bytes_total: u64 = 0;
    if let Some(media) = media {
        let entries = media
            .refs_for_package(pkg)
            .map_err(|e| ArchiveError::Invalid(format!("查询媒体引用失败: {e}")))?;
        let mut seen_files: HashSet<String> = HashSet::new();
        for entry in entries {
            let mut included = false;
            if include_media && entry.media.state == crate::media::MediaState::Ready {
                let path = media
                    .file_path(&entry.media.id)
                    .map_err(|e| ArchiveError::Invalid(format!("读取素材失败: {e}")))?;
                let size = fs::metadata(&path)
                    .map(|m| m.len())
                    .map_err(|e| ArchiveError::Invalid(format!("读取素材大小失败: {e}")))?;
                if size != entry.media.size {
                    return Err(ArchiveError::Invalid(format!(
                        "素材 {} 实际大小 {size} 与元数据 {} 不一致",
                        entry.media.id.0, entry.media.size
                    )));
                }
                if size > MAX_MEDIA_FILE_BYTES as u64 {
                    return Err(ArchiveError::Invalid(format!(
                        "素材 {}（{size} 字节）超过单文件上限 {MAX_MEDIA_FILE_BYTES} 字节",
                        entry.media.id.0
                    )));
                }
                if seen_files.insert(entry.media.sha256.clone()) {
                    media_files.push((entry.media.sha256.clone(), path));
                    media_bytes_total += size;
                }
                included = true;
            }
            index.entries.push(MediaIndexEntry {
                id: entry.media.id.0.clone(),
                name: entry.media.name.clone(),
                sha256: entry.media.sha256.clone(),
                size: entry.media.size,
                plugin_id: entry.plugin_id,
                kind: entry.kind,
                included,
                probe: Some(MediaProbeMeta {
                    container: entry.media.container.clone(),
                    codec: entry.media.codec.clone(),
                    width: entry.media.width,
                    height: entry.media.height,
                    rotation: entry.media.rotation,
                    duration_us: entry.media.duration_us,
                }),
            });
        }
    }
    // 同一素材被多个引用条目共享时 included 一致（都是 true 或都 false 之前
    // 已经按 sha 去重写入一次）；索引条目按 (id, plugin_id, kind) 排序保证
    // 可复现。
    index
        .entries
        .sort_by(|a, b| (&a.id, &a.plugin_id, &a.kind).cmp(&(&b.id, &b.plugin_id, &b.kind)));

    files.sort_by(|a, b| a.name.cmp(&b.name));
    let index_bytes = serde_json::to_vec_pretty(&index)
        .map_err(|e| ArchiveError::Invalid(format!("媒体索引序列化失败: {e}")))?;
    let extra_entries = files.len() + 1 + media_files.len(); // + media/index.json
    if extra_entries + 1 > IMPORT_MAX_ENTRIES {
        return Err(ArchiveError::Invalid(format!(
            "资源条目数 {} 超过上限 {IMPORT_MAX_ENTRIES}",
            extra_entries
        )));
    }
    let total: u64 = files
        .iter()
        .map(|f| fs::metadata(&f.absolute).map(|m| m.len()).unwrap_or(0))
        .sum::<u64>()
        + index_bytes.len() as u64
        + media_bytes_total;
    if total > IMPORT_MAX_TOTAL_BYTES as u64 {
        return Err(ArchiveError::Invalid(format!(
            "解压总量 {total} 字节超过上限 {IMPORT_MAX_TOTAL_BYTES} 字节"
        )));
    }
    let archive = build_archive(&manifest, &dir, &files, &index_bytes, &media_files)?;
    let sha256 = verify_archive(&archive, pkg)?;
    Ok(BuiltPackage {
        manifest,
        sha256,
        archive,
    })
}

fn build_archive(
    manifest: &PackageManifest,
    dir: &Path,
    files: &[CollectedFile],
    media_index_bytes: &[u8],
    media_files: &[(String, PathBuf)],
) -> Result<Vec<u8>, ArchiveError> {
    let mut bytes = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(fixed_mtime());
        writer
            .start_file(archive_limits::MANIFEST_NAME, options)
            .map_err(ArchiveError::Zip)?;
        writer
            .write_all(crate::resources::serialize_package_toml(manifest).as_bytes())
            .map_err(ArchiveError::Io)?;
        // 其余条目（包内容 + 媒体索引 + 素材）按归档路径排序，保证可复现
        let mut ordered: Vec<(String, SourceBytes)> = files
            .iter()
            .map(|f| (f.name.clone(), SourceBytes::File(f.absolute.clone())))
            .collect();
        ordered.push((
            MEDIA_INDEX_ENTRY.to_string(),
            SourceBytes::Bytes(media_index_bytes.to_vec()),
        ));
        for (sha, path) in media_files {
            ordered.push((media_file_entry(sha), SourceBytes::File(path.clone())));
        }
        ordered.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, source) in ordered {
            writer.start_file(name.as_str(), options)?;
            match source {
                SourceBytes::File(path) => {
                    let content = fs::read(&path)?;
                    writer.write_all(&content)?;
                }
                SourceBytes::Bytes(payload) => {
                    writer.write_all(&payload)?;
                }
            }
        }
        writer.finish().map_err(ArchiveError::Zip)?;
    }
    let _ = dir;
    Ok(bytes)
}

/// 归档条目内容来源（包内文件路径或内存字节）。
enum SourceBytes {
    File(PathBuf),
    Bytes(Vec<u8>),
}

/// 导出自检：归档字节重走导入侧校验（limits + manifest 解析）并核对 manifest
/// 身份；返回归档 SHA-256。
fn verify_archive(archive: &[u8], expected_id: &str) -> Result<String, ArchiveError> {
    let manifest_bytes = validate_and_read_manifest(archive)?;
    let parsed =
        parse_package_toml(&manifest_bytes).map_err(|e| ArchiveError::Manifest(e.to_string()))?;
    if parsed.id != expected_id {
        return Err(ArchiveError::Invalid(format!(
            "自检失败：归档 manifest 身份（{}）与目标包（{expected_id}）不一致",
            parsed.id
        )));
    }
    Ok(sha256_hex(archive))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {

    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::config::Config;

    fn temp_store() -> (PackageStore, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let cfg = Config {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        (PackageStore::open(&cfg).unwrap(), dir)
    }

    fn archive(entries: Vec<(&str, &[u8])>) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            for (name, content) in entries {
                writer.start_file(name, options).unwrap();
                writer.write_all(content).unwrap();
            }
            writer.finish().unwrap();
        }
        bytes
    }

    fn manifest_bytes(id: &str) -> Vec<u8> {
        format!("id = \"{id}\"\nversion = \"1.0.0\"\n").into_bytes()
    }

    #[test]
    fn import_roundtrip_export_is_reproducible() {
        let (store, _dir) = temp_store();
        let package = archive(vec![
            ("package.toml", manifest_bytes("official.demo").as_slice()),
            (
                "plugins/gamer.yaml/automations/daily.yaml",
                b"version: 3\nsteps: []\n",
            ),
            ("shared/notes.txt", b"shared bytes"),
        ]);
        let staging = store.staging_root().join("t1");
        let manifest = extract_archive(&package, &staging).unwrap();
        assert_eq!(manifest.id, "official.demo");
        assert!(staging.join("package.toml").is_file());
        assert!(staging
            .join("plugins/gamer.yaml/automations/daily.yaml")
            .is_file());
        assert!(staging.join("shared/notes.txt").is_file());

        // 原子安装：staging → packages/official.demo（再导入走替换路径）
        let final_dir = store.package_dir("official.demo").unwrap();
        std::fs::rename(&staging, &final_dir).unwrap();

        // 导出可复现 + 内容一致
        let first = export_package(&store, "official.demo", None, false).unwrap();
        let second = export_package(&store, "official.demo", None, false).unwrap();
        assert_eq!(first.archive, second.archive);
        assert_eq!(first.sha256, second.sha256);
        assert_eq!(first.manifest.id, "official.demo");
        assert_eq!(
            std::fs::read(
                store
                    .package_dir("official.demo")
                    .unwrap()
                    .join("plugins/gamer.yaml/automations/daily.yaml")
            )
            .unwrap(),
            b"version: 3\nsteps: []\n"
        );

        // 导出字节可再导入（往返）
        let staging2 = store.staging_root().join("t2");
        let again = extract_archive(&first.archive, &staging2).unwrap();
        assert_eq!(again.id, "official.demo");
    }

    // 中文资源名（templates/登录.png 等）在归档里的往返：写入侧是 UTF-8 字节 +
    // UTF-8 标志（zip crate 对非 ASCII 名自动置位），解析侧中央目录强制合法
    // UTF-8，两端同标准 → 中文名包导出/导入/自检全链无损。
    #[test]
    fn chinese_entry_names_survive_export_import_roundtrip() {
        let (store, _dir) = temp_store();
        let package = archive(vec![
            ("package.toml", manifest_bytes("official.demo").as_slice()),
            (
                "plugins/gamer.yaml/templates/登录.png",
                b"\x89PNG fake bytes",
            ),
            (
                "plugins/gamer.yaml/automations/日常任务.yaml",
                b"version: 3\nsteps: []\n",
            ),
        ]);
        let staging = store.staging_root().join("zh1");
        extract_archive(&package, &staging).unwrap();
        assert!(staging.join("plugins/gamer.yaml/templates/登录.png").is_file());

        let final_dir = store.package_dir("official.demo").unwrap();
        std::fs::rename(&staging, &final_dir).unwrap();

        // 导出（内含自检重走中央目录解析）→ 再导入：中文名原样往返
        let exported = export_package(&store, "official.demo", None, false).unwrap();
        let staging2 = store.staging_root().join("zh2");
        extract_archive(&exported.archive, &staging2).unwrap();
        assert!(staging2
            .join("plugins/gamer.yaml/templates/登录.png")
            .is_file());
        assert!(staging2
            .join("plugins/gamer.yaml/automations/日常任务.yaml")
            .is_file());
    }

    /// 手工打包 stored 条目（显式控制文件名字节与标志位，zip crate 造不出无
    /// 标志的非 ASCII 名），返回 (归档字节, 中央目录起始偏移)。
    fn raw_zip(entries: Vec<(Vec<u8>, &[u8])>) -> Vec<u8> {
        let mut locals = Vec::new();
        let mut centrals = Vec::new();
        let mut offset = 0usize;
        let count = entries.len();
        for (name, data) in entries {
            let mut local = Vec::new();
            local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            local.extend_from_slice(&20u16.to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes()); // 无 UTF-8 标志
            local.extend_from_slice(&0u16.to_le_bytes()); // stored
            local.extend_from_slice(&0u32.to_le_bytes()); // time+date
            local.extend_from_slice(&0u32.to_le_bytes()); // crc
            local.extend_from_slice(&(data.len() as u32).to_le_bytes());
            local.extend_from_slice(&(data.len() as u32).to_le_bytes());
            local.extend_from_slice(&(name.len() as u16).to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes()); // extra len
            local.extend_from_slice(&name);
            local.extend_from_slice(data);

            let mut central = Vec::new();
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes()); // version made
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // 无 UTF-8 标志
            central.extend_from_slice(&0u16.to_le_bytes()); // stored
            central.extend_from_slice(&0u32.to_le_bytes()); // time+date
            central.extend_from_slice(&0u32.to_le_bytes()); // crc
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0u8; 12]); // extra/comment/disk/attrs（含外部属性 4 字节）
            central.extend_from_slice(&(offset as u32).to_le_bytes());
            central.extend_from_slice(&name);

            offset += local.len();
            locals.extend_from_slice(&local);
            centrals.extend_from_slice(&central);
        }
        let mut out = locals;
        let central_offset = out.len() as u32;
        out.extend_from_slice(&centrals);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // disk
        out.extend_from_slice(&0u16.to_le_bytes()); // central disk
        out.extend_from_slice(&(count as u16).to_le_bytes());
        out.extend_from_slice(&(count as u16).to_le_bytes());
        out.extend_from_slice(&(centrals.len() as u32).to_le_bytes());
        out.extend_from_slice(&central_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        out
    }

    /// GBK 文件名归档（中文 Windows 外部工具重打包形态）必须整包拒绝：
    /// 中央目录强制合法 UTF-8，绝不解出乱码名资源落盘（fail-closed）。
    #[test]
    fn gbk_named_archive_is_rejected() {
        // "登录" 的 GBK（CP936）字节：B5 C7 C2 BC
        let gbk_name = b"plugins/gamer.yaml/templates/\xB5\xC7\xC2\xBC.png".to_vec();
        let bytes = raw_zip(vec![
            (b"package.toml".to_vec(), manifest_bytes("official.demo").as_slice()),
            (gbk_name, b"png"),
        ]);
        let err = validate_and_read_manifest(&bytes).unwrap_err();
        assert!(
            err.to_string().contains("UTF-8"),
            "必须以「归档路径必须是 UTF-8」拒绝: {err}"
        );
        let staging = std::env::temp_dir().join(format!(
            "gamer-archive-gbk-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).unwrap();
        assert!(extract_archive(&bytes, &staging).is_err());
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn archive_rejects_traversal_duplicates_whitelist_and_limits() {
        let oversized = vec![0u8; IMPORT_MAX_ARCHIVE_BYTES + 1];
        assert!(matches!(
            validate_and_read_manifest(&oversized),
            Err(ArchiveError::ArchiveTooLarge { .. })
        ));

        // 穿越路径拒绝
        let traversal = archive(vec![
            ("package.toml", manifest_bytes("official.x").as_slice()),
            ("../outside.txt", b"nope"),
        ]);
        assert!(matches!(
            validate_and_read_manifest(&traversal),
            Err(ArchiveError::Invalid(_))
        ));

        // 顶层白名单外条目拒绝
        let rogue = archive(vec![
            ("package.toml", manifest_bytes("official.x").as_slice()),
            ("scripts/daily.yaml", b"steps: []\n"),
        ]);
        let staging = std::env::temp_dir().join(format!(
            "gamer-arch-rogue-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_dir_all(&staging);
        let err = extract_archive(&rogue, &staging).unwrap_err();
        assert!(err.to_string().contains("白名单"), "{err}");
        let _ = std::fs::remove_dir_all(&staging);

        // plugins/ 下 plugin-id 非法拒绝
        let bad_plugin = archive(vec![
            ("package.toml", manifest_bytes("official.x").as_slice()),
            ("plugins/Bad_Id/x.yaml", b"nope"),
        ]);
        assert!(validate_and_read_manifest(&bad_plugin).is_err());

        // 空 entries / 缺 manifest
        let empty = archive(vec![]);
        assert!(matches!(
            validate_and_read_manifest(&empty),
            Err(ArchiveError::Invalid(_))
        ));
        let no_manifest = archive(vec![("shared/x.txt", b"x")]);
        assert!(matches!(
            validate_and_read_manifest(&no_manifest),
            Err(ArchiveError::Invalid(_))
        ));

        // 坏 manifest（非法 id）在解压前拒绝
        let bad_manifest = archive(vec![("package.toml", b"id = \"BAD ID\"\n".as_slice())]);
        assert!(matches!(
            validate_and_read_manifest(&bad_manifest),
            Err(ArchiveError::Manifest(_))
        ));
    }

    const MEDIA_INDEX: &str = r#"{
  "schema_version": 1,
  "entries": [
    {
      "id": "clip01abc",
      "name": "clip.mp4",
      "sha256": "aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44",
      "size": 4,
      "plugin_id": "gamer.video",
      "kind": "project",
      "included": true
    }
  ]
}"#;

    /// media/ 白名单受控形态：index.json + files/<64hex> 放行，其余拒绝。
    #[test]
    fn media_layout_whitelist_accepts_controlled_shapes_only() {
        let ok = archive(vec![
            ("package.toml", manifest_bytes("official.media").as_slice()),
            (MEDIA_INDEX_ENTRY, MEDIA_INDEX.as_bytes()),
            (
                "media/files/aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44",
                b"clip",
            ),
        ]);
        let staging = std::env::temp_dir().join(format!(
            "gamer-arch-media-ok-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_dir_all(&staging);
        let manifest = extract_archive(&ok, &staging).unwrap();
        assert_eq!(manifest.id, "official.media");
        assert!(staging.join("media/index.json").is_file());
        assert!(staging
            .join("media/files/aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44")
            .is_file());
        let _ = std::fs::remove_dir_all(&staging);

        for bad in [
            "media/evil.json",
            "media/files/deeper/aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44",
            "media/files/not-hex",
            "media/files/short",
            "media/onlydir/",
        ] {
            let rogue = archive(vec![
                ("package.toml", manifest_bytes("official.media").as_slice()),
                (bad, b"x"),
            ]);
            let err = validate_and_read_manifest(&rogue).unwrap_err();
            assert!(
                err.to_string().contains("media/") || err.to_string().contains("白名单"),
                "{bad}: {err}"
            );
        }
    }

    /// 媒体索引解析与形状校验。
    #[test]
    fn media_index_parses_and_validates_entries() {
        let index = parse_media_index(MEDIA_INDEX.as_bytes()).unwrap();
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].id, "clip01abc");
        assert!(index.entries[0].included);
        assert!(index.entries[0].probe.is_none());

        let entry_json = r#"{
            "id": "clip01abc",
            "name": "clip.mp4",
            "sha256": "aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44",
            "size": 4,
            "plugin_id": "gamer.video",
            "kind": "project",
            "included": true
        }"#;
        let wrap = |entries: &str| format!("{{\"schema_version\": 1, \"entries\": [{entries}]}}");

        // schema 版本不识别
        let bad_version =
            wrap(entry_json).replace("\"schema_version\": 1", "\"schema_version\": 2");
        assert!(matches!(
            parse_media_index(bad_version.as_bytes()),
            Err(ArchiveError::Invalid(_))
        ));
        // 非法 id / 坏 sha / 重复条目 / 非法 plugin id / 非法 kind
        for (label, patch) in [
            ("bad id", entry_json.replace("clip01abc", "Bad Id")),
            (
                "bad sha",
                entry_json.replace(
                    "aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44",
                    "zz11",
                ),
            ),
            (
                "bad plugin",
                entry_json.replace("gamer.video", "Bad Plugin"),
            ),
            ("bad kind", entry_json.replace("project", "")),
        ] {
            let err = parse_media_index(wrap(&patch).as_bytes());
            assert!(err.is_err(), "{label} must be rejected");
        }
        let duplicated = wrap(&format!("{entry_json},{entry_json}"));
        assert!(
            parse_media_index(duplicated.as_bytes())
                .unwrap_err()
                .to_string()
                .contains("重复"),
            "重复逻辑 id 必须拒绝"
        );
        // included 条目超过媒体单文件上限
        let huge = entry_json.replace(
            "\"size\": 4",
            &format!("\"size\": {}", MAX_MEDIA_FILE_BYTES + 1),
        );
        assert!(parse_media_index(wrap(&huge).as_bytes()).is_err());
    }

    /// 带媒体导出：索引含引用条目 + 素材字节；默认导出只有索引（引用登记、
    /// 不含原始大视频）；导出产物可再导入。
    #[test]
    fn export_with_media_includes_index_and_optional_files() {
        use crate::media::{MediaId, MediaRef, MediaSource, MediaState};

        let dir = tempfile::TempDir::new().unwrap();
        let cfg = Config {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let store = PackageStore::open(&cfg).unwrap();
        store
            .create_package(crate::resources::PackageInput {
                id: "official.media".into(),
                ..Default::default()
            })
            .unwrap();

        // 手工落一个素材（无 ffmpeg 路径：直接写库目录 + metadata）
        let media_root = dir.path().join("media");
        let clip_dir = media_root.join("clip01abc");
        std::fs::create_dir_all(&clip_dir).unwrap();
        std::fs::write(clip_dir.join("original.mp4"), b"clip").unwrap();
        let meta = crate::media::MediaMetadata {
            id: MediaId("clip01abc".into()),
            name: "clip.mp4".into(),
            sha256: crate::package_archive::sha256_hex(b"clip"),
            size: 4,
            duration_us: Some(1_000),
            container: "mp4".into(),
            codec: "h264".into(),
            width: 64,
            height: 64,
            rotation: 0,
            source: MediaSource::Import,
            state: MediaState::Ready,
            created_at: "2026-09-07T00:00:00.000000Z".into(),
            refs: vec![MediaRef {
                package_id: "official.media".into(),
                plugin_id: "gamer.video".into(),
                kind: "project".into(),
            }],
        };
        crate::media::write_metadata_for_test(&clip_dir, &meta).unwrap();
        let media = MediaService::open(media_root, "gamer-no-such-ffmpeg".into()).unwrap();

        // 默认导出：索引有条目但 included=false，无 files/ 字节
        let bare = export_package(&store, "official.media", Some(&media), false).unwrap();
        let bare_index = read_archive_entry(&bare.archive, MEDIA_INDEX_ENTRY);
        let index: MediaIndex = serde_json::from_slice(&bare_index).unwrap();
        assert_eq!(index.entries.len(), 1);
        assert!(!index.entries[0].included);
        assert!(read_archive_entry_opt(&bare.archive, "media/files/").is_none());

        // include_media：素材字节进包 + included=true + 可复现
        let full1 = export_package(&store, "official.media", Some(&media), true).unwrap();
        let full2 = export_package(&store, "official.media", Some(&media), true).unwrap();
        assert_eq!(full1.archive, full2.archive, "可复现");
        let index: MediaIndex =
            serde_json::from_slice(&read_archive_entry(&full1.archive, MEDIA_INDEX_ENTRY)).unwrap();
        assert!(index.entries[0].included);
        let bytes = read_archive_entry(&full1.archive, &media_file_entry(&index.entries[0].sha256));
        assert_eq!(bytes, b"clip");

        // 导出产物可再导入（往返）
        let staging = store.staging_root().join("media-roundtrip");
        let manifest = extract_archive(&full1.archive, &staging).unwrap();
        assert_eq!(manifest.id, "official.media");
        assert!(staging.join("media/index.json").is_file());
    }

    fn read_archive_entry(archive: &[u8], name: &str) -> Vec<u8> {
        read_archive_entry_opt(archive, name).expect("entry must exist")
    }

    fn read_archive_entry_opt(archive: &[u8], prefix: &str) -> Option<Vec<u8>> {
        let mut reader = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
        for i in 0..reader.len() {
            let mut entry = reader.by_index(i).unwrap();
            if entry.name().starts_with(prefix) {
                let mut out = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut out).unwrap();
                return Some(out);
            }
        }
        None
    }
}
