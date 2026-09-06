//! Package 归档（.gamerpkg = zip）：导入校验提取 + 可复现导出打包。
//!
//! 导入校验流程（plan §8）：字节上限 → ZIP 中央目录解析（条目数/重复路径/
//! 穿越名/加密条目拒绝）→ 解压到临时 staging → 验证 package.toml → 验证 id
//! （= 目标目录名）→ 原子安装/替换。归档布局白名单：`package.toml`（自然
//! 第一）+ `shared/**` + `plugins/<plugin-id>/**`，其余顶层条目一律拒绝。
//!
//! 导出（plan §11）：内容 = package.toml + shared/ + plugins/，条目按相对
//! 路径排序、UTF-8 文件名、Deflated、固定 mtime（2000-01-01）→ 相同输入产生
//! 逐字节相同归档（相同 SHA-256）。导出后自检：归档重走导入侧校验 + 条目
//! 集合核对。

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use zip::ZipArchive;

use crate::core::fs::archive_validation::{
    IMPORT_MAX_ARCHIVE_BYTES, IMPORT_MAX_ENTRIES, IMPORT_MAX_TOTAL_BYTES,
};

use crate::resources::archive_limits;
use crate::resources::{parse_package_toml, validate_scope_id, PackageManifest, PackageStore};

use crate::resources::archive_limits::MAX_FILE_BYTES as MAX_PACKAGE_FILE_BYTES;
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
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
    #[error("Package 不存在: {0}")]
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
        .try_fold(0u64, |total, entry| total.checked_add(entry.uncompressed_size))
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
                return Err(ArchiveError::Invalid(
                    "package.toml 必须是文件".to_string(),
                ));
            }
            let size = usize::try_from(entry.size())
                .map_err(|_| ArchiveError::Invalid("package.toml 大小溢出".to_string()))?;
            if size > MAX_MANIFEST_BYTES {
                return Err(ArchiveError::Invalid(format!(
                    "package.toml 超过 {MAX_MANIFEST_BYTES} 字节上限"
                )));
            }
            let mut content = Vec::with_capacity(size);
            entry.read_to_end(&mut content)?;
            parse_package_toml(&content)
                .map_err(|e| ArchiveError::Manifest(e.to_string()))?;
            manifest = Some(content);
        }
    }
    manifest.ok_or_else(|| {
        ArchiveError::Invalid("归档根目录缺少 package.toml".to_string())
    })
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
        if entry.size() > MAX_PACKAGE_FILE_BYTES as u64 {
            return Err(ArchiveError::Invalid(format!(
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
                .ok_or_else(|| ArchiveError::Invalid("文件大小溢出".to_string()))?;
            extracted_total = extracted_total
                .checked_add(read)
                .ok_or_else(|| ArchiveError::Invalid("解压总大小溢出".to_string()))?;
            if copied > MAX_PACKAGE_FILE_BYTES || extracted_total > IMPORT_MAX_TOTAL_BYTES {
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

/// 顶层布局白名单：package.toml / shared/** / plugins/<plugin-id>/**。
fn archive_relative_path(name: &str, is_dir: bool) -> Result<Vec<String>, ArchiveError> {
    let normalized = if is_dir { name.trim_end_matches('/') } else { name };
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
        other => {
            return Err(ArchiveError::Invalid(format!(
                "归档顶层条目 {other:?} 不在白名单内（只允许 package.toml / shared/ / plugins/）"
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

fn append_relative_path(root: &Path, relative: &[String]) -> PathBuf {
    relative.iter().fold(root.to_path_buf(), |path, part| path.join(part))
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
            return Err(ArchiveError::Invalid(format!(
                "归档路径非法: {name:?}"
            )));
        }
        if !seen.insert(normalized.to_string()) {
            return Err(ArchiveError::Invalid(format!(
                "归档存在重复路径: {normalized}"
            )));
        }
        // 顶层布局白名单在中央目录解析期统一执行（validate 与 extract 同一语义）
        let is_dir_entry = name.ends_with('/');
        archive_relative_path(normalized, is_dir_entry)?;
        entries.push(CentralEntry {
            uncompressed_size,
        });
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
/// （空串 = 整包）。
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
/// plugins/ 按路径排序；固定 mtime 可复现）。包不存在 → NotFound。
pub fn export_package(store: &PackageStore, pkg: &str) -> Result<BuiltPackage, ArchiveError> {
    let manifest = store
        .try_manifest(pkg)
        .map_err(|e| ArchiveError::Invalid(e.to_string()))?
        .ok_or_else(|| ArchiveError::NotFound(pkg.to_string()))?;
    let dir = store
        .package_dir(pkg)
        .map_err(|e| ArchiveError::Invalid(e.to_string()))?;
    let mut files = Vec::new();
    collect_dir(&dir, "", &mut files);
    files.sort_by(|a, b| a.name.cmp(&b.name));
    if files.len() + 1 > IMPORT_MAX_ENTRIES {
        return Err(ArchiveError::Invalid(format!(
            "资源条目数 {} 超过上限 {IMPORT_MAX_ENTRIES}",
            files.len()
        )));
    }
    let total: u64 = files
        .iter()
        .map(|f| fs::metadata(&f.absolute).map(|m| m.len()).unwrap_or(0))
        .sum();
    if total > IMPORT_MAX_TOTAL_BYTES as u64 {
        return Err(ArchiveError::Invalid(format!(
            "解压总量 {total} 字节超过上限 {IMPORT_MAX_TOTAL_BYTES} 字节"
        )));
    }
    let archive = build_archive(&manifest, &dir, &files)?;
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
        for file in files {
            writer.start_file(file.name.as_str(), options)?;
            let content = fs::read(&file.absolute)?;
            writer.write_all(&content)?;
        }
        writer.finish().map_err(ArchiveError::Zip)?;
    }
    let _ = dir;
    Ok(bytes)
}

/// 导出自检：归档字节重走导入侧校验（limits + manifest 解析）并核对 manifest
/// 身份；返回归档 SHA-256。
fn verify_archive(archive: &[u8], expected_id: &str) -> Result<String, ArchiveError> {
    let manifest_bytes = validate_and_read_manifest(archive)?;
    let parsed = parse_package_toml(&manifest_bytes)
        .map_err(|e| ArchiveError::Manifest(e.to_string()))?;
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
            ("plugins/gamer.yaml/scripts/daily.yaml", b"version: 3\nsteps: []\n"),
            ("shared/notes.txt", b"shared bytes"),
        ]);
        let staging = store.staging_root().join("t1");
        let manifest = extract_archive(&package, &staging).unwrap();
        assert_eq!(manifest.id, "official.demo");
        assert!(staging.join("package.toml").is_file());
        assert!(staging.join("plugins/gamer.yaml/scripts/daily.yaml").is_file());
        assert!(staging.join("shared/notes.txt").is_file());

        // 原子安装：staging → packages/official.demo（再导入走替换路径）
        let final_dir = store.package_dir("official.demo").unwrap();
        std::fs::rename(&staging, &final_dir).unwrap();

        // 导出可复现 + 内容一致
        let first = export_package(&store, "official.demo").unwrap();
        let second = export_package(&store, "official.demo").unwrap();
        assert_eq!(first.archive, second.archive);
        assert_eq!(first.sha256, second.sha256);
        assert_eq!(first.manifest.id, "official.demo");
        assert_eq!(
            std::fs::read(
                store
                    .package_dir("official.demo")
                    .unwrap()
                    .join("plugins/gamer.yaml/scripts/daily.yaml")
            )
            .unwrap(),
            b"version: 3\nsteps: []\n"
        );

        // 导出字节可再导入（往返）
        let staging2 = store.staging_root().join("t2");
        let again = extract_archive(&first.archive, &staging2).unwrap();
        assert_eq!(again.id, "official.demo");
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
}
