//! Secure `.gplugin` ZIP inspection and extraction.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};

use zip::ZipArchive;

use crate::core::fs::archive_validation::{
    IMPORT_MAX_ARCHIVE_BYTES, IMPORT_MAX_ENTRIES, IMPORT_MAX_TOTAL_BYTES,
};

use super::error::{ExtensionError, ExtensionResult};
use super::manifest::{parse_manifest, ExtensionManifest, MANIFEST_FILE_NAME};
use super::model::ExtensionPath;

const MAX_EXTENSION_FILE_BYTES: usize = 10 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 256 * 1024;

#[derive(Debug)]
struct ArchiveEntry {
    name: String,
    is_dir: bool,
    declared_size: u64,
    archive_index: usize,
}

/// Validate all central-directory and extraction constraints before creating
/// any files, then return the parsed manifest for compatibility checks.
pub(crate) fn inspect_archive(bytes: &[u8]) -> ExtensionResult<ExtensionManifest> {
    let (entries, manifest_bytes) = scan_archive(bytes)?;
    let manifest = parse_manifest(&manifest_bytes)?;
    let entry = manifest.entry().as_str();
    let entry_info = entries.iter().find(|candidate| candidate.name == entry);
    let Some(entry_info) = entry_info else {
        return Err(ExtensionError::InvalidArchive(format!(
            "manifest entry 不存在: {entry}"
        )));
    };
    if entry_info.is_dir {
        return Err(ExtensionError::InvalidArchive(format!(
            "manifest entry 不能是目录: {entry}"
        )));
    }
    if entry_info.declared_size < 4 {
        return Err(ExtensionError::InvalidArchive(format!(
            "WASM entry 太小: {entry}"
        )));
    }

    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut wasm = archive.by_name(entry)?;
    let mut magic = [0u8; 4];
    wasm.read_exact(&mut magic)?;
    if magic != *b"\0asm" {
        return Err(ExtensionError::InvalidArchive(format!(
            "entry 不是 WASM 二进制: {entry}"
        )));
    }
    Ok(manifest)
}

pub(crate) fn extract_archive(bytes: &[u8], staging: &Path) -> ExtensionResult<ExtensionManifest> {
    let manifest = inspect_archive(bytes)?;
    let (entries, _) = scan_archive(bytes)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    fs::create_dir_all(staging)?;

    let mut extracted_total = 0usize;
    for candidate in entries {
        let mut entry = archive.by_index(candidate.archive_index)?;
        let destination = append_path(staging, &ExtensionPath::parse(&candidate.name)?);
        if entry.is_dir() {
            fs::create_dir_all(destination)?;
            continue;
        }
        if candidate.declared_size > MAX_EXTENSION_FILE_BYTES as u64 {
            return Err(ExtensionError::InvalidArchive(format!(
                "文件 {} 超过单文件上限 {} 字节",
                candidate.name, MAX_EXTENSION_FILE_BYTES
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
                .ok_or_else(|| ExtensionError::InvalidArchive("文件大小溢出".to_string()))?;
            extracted_total = extracted_total
                .checked_add(read)
                .ok_or_else(|| ExtensionError::InvalidArchive("解压总大小溢出".to_string()))?;
            if copied > MAX_EXTENSION_FILE_BYTES || extracted_total > IMPORT_MAX_TOTAL_BYTES {
                return Err(ExtensionError::InvalidArchive(
                    "实际解压大小超过上限".to_string(),
                ));
            }
            output.write_all(&buffer[..read])?;
        }
        output.flush()?;
        output.sync_all()?;
    }
    sync_directory(staging)?;
    Ok(manifest)
}

fn scan_archive(bytes: &[u8]) -> ExtensionResult<(Vec<ArchiveEntry>, Vec<u8>)> {
    if bytes.len() > IMPORT_MAX_ARCHIVE_BYTES {
        return Err(ExtensionError::ArchiveTooLarge {
            actual: bytes.len(),
            limit: IMPORT_MAX_ARCHIVE_BYTES,
        });
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    if archive.is_empty() {
        return Err(ExtensionError::InvalidArchive("归档不能为空".to_string()));
    }
    if archive.len() > IMPORT_MAX_ENTRIES {
        return Err(ExtensionError::InvalidArchive(format!(
            "条目数 {} 超过上限 {}",
            archive.len(),
            IMPORT_MAX_ENTRIES
        )));
    }

    let mut entries = Vec::with_capacity(archive.len());
    let mut seen = HashSet::with_capacity(archive.len());
    let mut declared_total = 0u64;
    let mut manifest_bytes = None;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.encrypted() || entry.is_symlink() {
            return Err(ExtensionError::InvalidArchive(format!(
                "不允许加密或符号链接条目: {}",
                entry.name()
            )));
        }
        let is_dir = entry.is_dir() || entry.name().ends_with('/');
        let name = normalize_archive_path(entry.name(), is_dir)?;
        if !seen.insert(name.clone()) {
            return Err(ExtensionError::InvalidArchive(format!(
                "归档存在重复路径: {name}"
            )));
        }
        declared_total = declared_total
            .checked_add(entry.size())
            .ok_or_else(|| ExtensionError::InvalidArchive("解压总大小溢出".to_string()))?;
        if declared_total > IMPORT_MAX_TOTAL_BYTES as u64 {
            return Err(ExtensionError::InvalidArchive(format!(
                "声明解压总大小超过上限 {} 字节",
                IMPORT_MAX_TOTAL_BYTES
            )));
        }
        if !is_dir && entry.size() > MAX_EXTENSION_FILE_BYTES as u64 {
            return Err(ExtensionError::InvalidArchive(format!(
                "文件 {} 超过单文件上限 {} 字节",
                name, MAX_EXTENSION_FILE_BYTES
            )));
        }
        if name == MANIFEST_FILE_NAME {
            if is_dir {
                return Err(ExtensionError::InvalidArchive(
                    "manifest.toml 必须是文件".to_string(),
                ));
            }
            let size = usize::try_from(entry.size()).map_err(|_| {
                ExtensionError::InvalidArchive("manifest.toml 大小溢出".to_string())
            })?;
            if size > MAX_MANIFEST_BYTES {
                return Err(ExtensionError::InvalidArchive(format!(
                    "manifest.toml 超过 {MAX_MANIFEST_BYTES} 字节上限"
                )));
            }
            let mut content = Vec::with_capacity(size);
            entry.read_to_end(&mut content)?;
            manifest_bytes = Some(content);
        }
        entries.push(ArchiveEntry {
            name,
            is_dir,
            declared_size: entry.size(),
            archive_index: index,
        });
    }
    let manifest_bytes = manifest_bytes.ok_or_else(|| {
        ExtensionError::InvalidArchive("归档根目录缺少 manifest.toml".to_string())
    })?;
    Ok((entries, manifest_bytes))
}

fn normalize_archive_path(name: &str, is_dir: bool) -> ExtensionResult<String> {
    if name.trim() != name {
        return Err(ExtensionError::InvalidArchive(format!(
            "归档路径不能包含首尾空白: {name:?}"
        )));
    }
    let normalized = if is_dir {
        name.trim_end_matches('/')
    } else {
        name
    };
    if normalized == MANIFEST_FILE_NAME {
        return Ok(normalized.to_string());
    }
    Ok(ExtensionPath::parse(normalized)?.as_str().to_string())
}

fn append_path(root: &Path, relative: &ExtensionPath) -> PathBuf {
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
