use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use zip::ZipArchive;

#[cfg(unix)]
use std::fs::File;

use crate::core::fs::archive_validation::{
    IMPORT_MAX_ARCHIVE_BYTES, IMPORT_MAX_ENTRIES, IMPORT_MAX_TOTAL_BYTES,
};

use super::error::{AppPackageError, AppPackageResult};
use super::manifest::parse_manifest;
use super::model::ResourcePath;

pub(crate) const MAX_PACKAGE_ARCHIVE_BYTES: usize = IMPORT_MAX_ARCHIVE_BYTES;
pub(crate) const MAX_PACKAGE_TOTAL_BYTES: usize = IMPORT_MAX_TOTAL_BYTES;
pub(crate) const MAX_PACKAGE_ENTRIES: usize = IMPORT_MAX_ENTRIES;
pub(crate) const MAX_PACKAGE_FILE_BYTES: usize = 10 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 256 * 1024;

#[derive(Debug)]
struct CentralEntry {
    uncompressed_size: u64,
}

pub(crate) fn validate_and_read_manifest(bytes: &[u8]) -> AppPackageResult<Vec<u8>> {
    if bytes.len() > MAX_PACKAGE_ARCHIVE_BYTES {
        return Err(AppPackageError::ArchiveTooLarge {
            actual: bytes.len(),
            limit: MAX_PACKAGE_ARCHIVE_BYTES,
        });
    }
    let entries = parse_central_directory(bytes)?;
    if entries.is_empty() {
        return Err(AppPackageError::InvalidArchive("归档不能为空".to_string()));
    }
    let declared_total = entries
        .iter()
        .try_fold(0u64, |total, entry| {
            total.checked_add(entry.uncompressed_size)
        })
        .ok_or_else(|| AppPackageError::InvalidArchive("解压总大小溢出".to_string()))?;
    if declared_total > MAX_PACKAGE_TOTAL_BYTES as u64 {
        return Err(AppPackageError::InvalidArchive(format!(
            "声明解压总大小 {declared_total} 字节超过上限 {MAX_PACKAGE_TOTAL_BYTES} 字节"
        )));
    }

    let mut archive = ZipArchive::new(io::Cursor::new(bytes))?;
    if archive.len() != entries.len() {
        return Err(AppPackageError::InvalidArchive(
            "ZIP 中央目录条目数与读取器不一致".to_string(),
        ));
    }
    let mut manifest = None;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.encrypted() {
            return Err(AppPackageError::InvalidArchive(format!(
                "不允许加密条目: {}",
                entry.name()
            )));
        }
        if entry.name() == "manifest.toml" {
            if entry.is_dir() {
                return Err(AppPackageError::InvalidArchive(
                    "manifest.toml 必须是文件".to_string(),
                ));
            }
            let size = usize::try_from(entry.size()).map_err(|_| {
                AppPackageError::InvalidArchive("manifest.toml 大小溢出".to_string())
            })?;
            if size > MAX_MANIFEST_BYTES {
                return Err(AppPackageError::InvalidArchive(format!(
                    "manifest.toml 超过 {MAX_MANIFEST_BYTES} 字节上限"
                )));
            }
            let mut content = Vec::with_capacity(size);
            entry.read_to_end(&mut content)?;
            // Parse before staging anything; a malformed package cannot create a partial install.
            parse_manifest(&content)?;
            manifest = Some(content);
        }
    }
    manifest
        .ok_or_else(|| AppPackageError::InvalidArchive("归档根目录缺少 manifest.toml".to_string()))
}

pub(crate) fn extract_archive(bytes: &[u8], staging: &Path) -> AppPackageResult<()> {
    let entries = parse_central_directory(bytes)?;
    let mut archive = ZipArchive::new(io::Cursor::new(bytes))?;
    if archive.len() != entries.len() {
        return Err(AppPackageError::InvalidArchive(
            "ZIP 中央目录条目数与读取器不一致".to_string(),
        ));
    }
    fs::create_dir_all(staging)?;
    let mut extracted_total = 0usize;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.encrypted() || entry.is_symlink() {
            return Err(AppPackageError::InvalidArchive(format!(
                "不允许加密或符号链接条目: {}",
                entry.name()
            )));
        }
        let destination = if entry.name() == "manifest.toml" {
            staging.join("manifest.toml")
        } else {
            let relative = archive_relative_path(entry.name(), entry.is_dir())?;
            append_relative_path(staging, &relative)
        };
        if entry.is_dir() {
            fs::create_dir_all(destination)?;
            continue;
        }
        if entry.size() > MAX_PACKAGE_FILE_BYTES as u64 {
            return Err(AppPackageError::InvalidArchive(format!(
                "文件 {} 超过单文件上限 {MAX_PACKAGE_FILE_BYTES} 字节",
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
                .ok_or_else(|| AppPackageError::InvalidArchive("文件大小溢出".to_string()))?;
            extracted_total = extracted_total
                .checked_add(read)
                .ok_or_else(|| AppPackageError::InvalidArchive("解压总大小溢出".to_string()))?;
            if copied > MAX_PACKAGE_FILE_BYTES || extracted_total > MAX_PACKAGE_TOTAL_BYTES {
                return Err(AppPackageError::InvalidArchive(
                    "实际解压大小超过上限".to_string(),
                ));
            }
            output.write_all(&buffer[..read])?;
        }
        output.flush()?;
        output.sync_all()?;
    }
    sync_directory(staging)?;
    Ok(())
}

fn parse_central_directory(bytes: &[u8]) -> AppPackageResult<Vec<CentralEntry>> {
    let eocd = find_end_of_central_directory(bytes)?;
    let disk = read_u16(bytes, eocd + 4)?;
    let central_disk = read_u16(bytes, eocd + 6)?;
    let disk_entries = read_u16(bytes, eocd + 8)?;
    let total_entries = read_u16(bytes, eocd + 10)?;
    let central_size = usize::try_from(read_u32(bytes, eocd + 12)?).unwrap_or(usize::MAX);
    let central_offset = usize::try_from(read_u32(bytes, eocd + 16)?).unwrap_or(usize::MAX);
    if disk != 0 || central_disk != 0 || disk_entries != total_entries {
        return Err(AppPackageError::InvalidArchive(
            "不支持多磁盘 ZIP".to_string(),
        ));
    }
    if total_entries == u16::MAX
        || central_size == u32::MAX as usize
        || central_offset == u32::MAX as usize
    {
        return Err(AppPackageError::InvalidArchive(
            "不支持 ZIP64 归档".to_string(),
        ));
    }
    let count = usize::from(total_entries);
    if count > MAX_PACKAGE_ENTRIES {
        return Err(AppPackageError::InvalidArchive(format!(
            "条目数 {count} 超过上限 {MAX_PACKAGE_ENTRIES}"
        )));
    }
    let central_end = central_offset
        .checked_add(central_size)
        .ok_or_else(|| AppPackageError::InvalidArchive("中央目录范围溢出".to_string()))?;
    if central_end != eocd || central_end > bytes.len() {
        return Err(AppPackageError::InvalidArchive(
            "中央目录范围无效".to_string(),
        ));
    }

    let mut entries = Vec::with_capacity(count);
    let mut seen = HashSet::with_capacity(count);
    let mut cursor = central_offset;
    for _ in 0..count {
        if read_u32(bytes, cursor)? != 0x0201_4b50 {
            return Err(AppPackageError::InvalidArchive(
                "中央目录条目标头无效".to_string(),
            ));
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
            .ok_or_else(|| AppPackageError::InvalidArchive("中央目录条目范围溢出".to_string()))?;
        if header_end > central_end {
            return Err(AppPackageError::InvalidArchive(
                "中央目录条目越界".to_string(),
            ));
        }
        let name_start = cursor + 46;
        let name = std::str::from_utf8(&bytes[name_start..name_start + name_len])
            .map_err(|_| AppPackageError::InvalidArchive("归档路径必须是 UTF-8".to_string()))?;
        let is_dir = name.ends_with('/');
        let normalized_name = normalized_archive_name(name, is_dir)?;
        if !seen.insert(normalized_name.clone()) {
            return Err(AppPackageError::InvalidArchive(format!(
                "归档存在重复路径: {normalized_name}"
            )));
        }
        entries.push(CentralEntry { uncompressed_size });
        cursor = header_end;
    }
    if cursor != central_end {
        return Err(AppPackageError::InvalidArchive(
            "中央目录包含无法解析的尾部数据".to_string(),
        ));
    }
    if entries
        .iter()
        .map(|entry| entry.uncompressed_size)
        .try_fold(0u64, |total, size| total.checked_add(size))
        .is_none()
    {
        return Err(AppPackageError::InvalidArchive("解压大小溢出".to_string()));
    }
    Ok(entries)
}

fn find_end_of_central_directory(bytes: &[u8]) -> AppPackageResult<usize> {
    if bytes.len() < 22 {
        return Err(AppPackageError::InvalidArchive(
            "缺少 ZIP 结束目录".to_string(),
        ));
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
    Err(AppPackageError::InvalidArchive(
        "缺少有效 ZIP 结束目录".to_string(),
    ))
}

fn read_u16(bytes: &[u8], offset: usize) -> AppPackageResult<u16> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| AppPackageError::InvalidArchive("ZIP 字段越界".to_string()))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> AppPackageResult<u32> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| AppPackageError::InvalidArchive("ZIP 字段越界".to_string()))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn normalized_archive_name(name: &str, is_dir: bool) -> AppPackageResult<String> {
    let normalized = if is_dir {
        name.trim_end_matches('/')
    } else {
        name
    };
    if normalized == "manifest.toml" {
        return Ok(normalized.to_string());
    }
    let path = ResourcePath::parse(normalized)?;
    if !is_dir && !normalized.contains('/') {
        return Err(AppPackageError::InvalidArchive(
            "资源文件必须位于受支持的资源目录下".to_string(),
        ));
    }
    Ok(path.as_str().to_string())
}

fn archive_relative_path(name: &str, is_dir: bool) -> AppPackageResult<ResourcePath> {
    let normalized = normalized_archive_name(name, is_dir)?;
    if normalized == "manifest.toml" {
        return Err(AppPackageError::InvalidArchive(
            "manifest.toml 不应作为资源路径处理".to_string(),
        ));
    }
    let path = ResourcePath::parse(&normalized)?;
    if !is_dir && path.kind().as_str() == normalized {
        return Err(AppPackageError::InvalidArchive(
            "资源根目录不能作为文件".to_string(),
        ));
    }
    Ok(path)
}

fn append_relative_path(root: &Path, relative: &ResourcePath) -> PathBuf {
    relative
        .components()
        .fold(root.to_path_buf(), |path, component| path.join(component))
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
