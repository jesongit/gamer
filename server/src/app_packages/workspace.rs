//! 本地编辑区（workspace）元数据与统计。
//!
//! 工作区 = `data/<android 包名>/`（scripts/functions/templates/keymaps/
//! presets/resources 六个资源目录的本地编辑现场）。`package.toml` 记录该
//! 工作区导出为 App Package 时使用的身份元数据，**schema 与包内
//! manifest.toml V2 完全一致**：解析直接复用 [`super::manifest::parse_manifest`]
//! （同一套规则，不存在宽松副本），序列化字段顺序固定为
//! `format_version, id, version, name?, revision?, [android].packages`。

use std::path::{Path, PathBuf};

use crate::core::fs::atomic_write;

use super::error::{AppPackageError, AppPackageResult};
use super::manifest::{parse_manifest, PackageManifest, MANIFEST_FORMAT_VERSION};
use super::model::AndroidPackageName;

/// 工作区元数据文件名（位于工作区根，不在任何资源目录内，导出不会收集它）。
pub(crate) const WORKSPACE_METADATA_FILE: &str = "package.toml";

/// 工作区目录：`data/<android 包名>/`。
pub(crate) fn workspace_dir(data_root: &Path, android: &AndroidPackageName) -> PathBuf {
    data_root.join(android.as_str())
}

pub(crate) fn metadata_path(dir: &Path) -> PathBuf {
    dir.join(WORKSPACE_METADATA_FILE)
}

/// TOML basic string 字面量（id/version/revision 已被校验限制为 ASCII 安全名，
/// name 可能含引号/换行等字符，这里做完整转义保证可往返）。
fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04X}", other as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// [`PackageManifest`] → 固定字段顺序的 TOML 文本（package.toml 与包内
/// manifest.toml 共用同一序列化器，保证两处字节形状一致）。
pub(crate) fn serialize_manifest_toml(manifest: &PackageManifest) -> String {
    let mut out = String::new();
    out.push_str(&format!("format_version = {MANIFEST_FORMAT_VERSION}\n"));
    out.push_str(&format!("id = {}\n", toml_string(manifest.id().as_str())));
    out.push_str(&format!(
        "version = {}\n",
        toml_string(manifest.version().as_str())
    ));
    if let Some(name) = manifest.name() {
        out.push_str(&format!("name = {}\n", toml_string(name)));
    }
    if let Some(revision) = manifest.revision() {
        out.push_str(&format!("revision = {}\n", toml_string(revision)));
    }
    out.push_str("\n[android]\npackages = [");
    for (index, package) in manifest.android_packages().iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&toml_string(package.as_str()));
    }
    out.push_str("]\n");
    out
}

/// 解析并校验 package.toml 字节（与包内 manifest 同一套规则）。错误带
/// `package.toml` 文件语境（`InvalidWorkspaceMetadata`），避免误报为
/// "manifest.toml 无效"。
pub(crate) fn parse_workspace_metadata(bytes: &[u8]) -> AppPackageResult<PackageManifest> {
    parse_manifest(bytes)
        .map_err(|error| AppPackageError::InvalidWorkspaceMetadata(error.to_string()))
}

/// PUT 工作区元数据的字段 → 校验并构造 [`PackageManifest`]。
///
/// 字段先序列化为固定顺序 TOML 再回灌 [`parse_workspace_metadata`]，
/// 保证 id/version/android 包名/name 的校验规则与 manifest V2 完全同源
/// （无第二套宽松实现）。空串 `name` 视为未提供。
pub(crate) fn metadata_from_parts(
    id: &str,
    version: &str,
    name: Option<&str>,
    android_packages: &[String],
) -> AppPackageResult<PackageManifest> {
    let mut text = String::new();
    text.push_str(&format!("format_version = {MANIFEST_FORMAT_VERSION}\n"));
    text.push_str(&format!("id = {}\n", toml_string(id.trim())));
    text.push_str(&format!("version = {}\n", toml_string(version.trim())));
    if let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) {
        text.push_str(&format!("name = {}\n", toml_string(name)));
    }
    text.push_str("\n[android]\npackages = [");
    for (index, package) in android_packages.iter().enumerate() {
        if index > 0 {
            text.push_str(", ");
        }
        text.push_str(&toml_string(package.trim()));
    }
    text.push_str("]\n");
    parse_workspace_metadata(text.as_bytes())
}

/// 读取工作区元数据；`package.toml` 不存在 → `Ok(None)`（尚未初始化）。
pub(crate) fn read_metadata(dir: &Path) -> AppPackageResult<Option<PackageManifest>> {
    let bytes = match std::fs::read(metadata_path(dir)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    parse_workspace_metadata(&bytes).map(Some)
}

/// 原子写 package.toml（tmp + rename）。写入文本先回灌
/// [`parse_workspace_metadata`] 复核——序列化产物永远满足同一套校验规则。
pub(crate) fn write_metadata(dir: &Path, manifest: &PackageManifest) -> AppPackageResult<()> {
    let text = serialize_manifest_toml(manifest);
    let _ = parse_workspace_metadata(text.as_bytes())?;
    atomic_write(&metadata_path(dir), text.as_bytes())
        .map_err(|error| AppPackageError::Io(std::io::Error::other(error.to_string())))?;
    Ok(())
}

/// 工作区资源统计：缺失目录计 0。scripts/functions/keymaps 只数 .yaml/.yml，
/// templates/presets/resources 数全部文件；递归统计，隐藏文件（`.` 开头）跳过。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkspaceStats {
    pub(crate) scripts: usize,
    pub(crate) functions: usize,
    pub(crate) templates: usize,
    pub(crate) keymaps: usize,
    pub(crate) presets: usize,
    pub(crate) resources: usize,
}

impl WorkspaceStats {
    pub(crate) fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "scripts": self.scripts,
            "functions": self.functions,
            "templates": self.templates,
            "keymaps": self.keymaps,
            "presets": self.presets,
            "resources": self.resources,
        })
    }
}

/// 文件是否计入（yaml 类目录只认 .yaml/.yml；其余目录全量计数）。
fn counts_as_file(yaml_only: bool, file_name: &str) -> bool {
    if !yaml_only {
        return true;
    }
    let lower = file_name.to_ascii_lowercase();
    lower.ends_with(".yaml") || lower.ends_with(".yml")
}

/// 递归统计目录内文件数（目录缺失 → 0；隐藏文件/目录跳过）。
fn count_dir_files(dir: &Path, yaml_only: bool) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut count = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            count += count_dir_files(&path, yaml_only);
        } else if path.is_file() && counts_as_file(yaml_only, name) {
            count += 1;
        }
    }
    count
}

pub(crate) fn compute_stats(dir: &Path) -> WorkspaceStats {
    const YAML_KINDS: [&str; 3] = ["scripts", "functions", "keymaps"];
    const ALL_KINDS: [&str; 3] = ["templates", "presets", "resources"];
    let mut stats = WorkspaceStats::default();
    for kind in YAML_KINDS {
        let value = count_dir_files(&dir.join(kind), true);
        match kind {
            "scripts" => stats.scripts = value,
            "functions" => stats.functions = value,
            "keymaps" => stats.keymaps = value,
            _ => unreachable!("YAML_KINDS 全部被枚举"),
        }
    }
    for kind in ALL_KINDS {
        let value = count_dir_files(&dir.join(kind), false);
        match kind {
            "templates" => stats.templates = value,
            "presets" => stats.presets = value,
            "resources" => stats.resources = value,
            _ => unreachable!("ALL_KINDS 全部被枚举"),
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn manifest_text() -> String {
        String::from(
            "format_version = 2\nid = \"official.demo\"\nversion = \"1.0.0\"\nname = \"演示包\"\n\n[android]\npackages = [\"com.example.game\"]\n",
        )
    }

    #[test]
    fn metadata_round_trips_with_manifest_rules() {
        let parsed = parse_workspace_metadata(manifest_text().as_bytes()).unwrap();
        let serialized = serialize_manifest_toml(&parsed);
        assert_eq!(serialized, manifest_text(), "序列化字段顺序必须固定");
        let reparsed = parse_workspace_metadata(serialized.as_bytes()).unwrap();
        assert_eq!(parsed.id().as_str(), reparsed.id().as_str());
        assert_eq!(parsed.name(), reparsed.name());

        // 同一套规则拒绝非法输入（缺 format_version）
        let bad = "id = \"official.demo\"\nversion = \"1.0.0\"\n[android]\npackages = [\"com.example.game\"]\n";
        assert!(matches!(
            parse_workspace_metadata(bad.as_bytes()),
            Err(AppPackageError::InvalidWorkspaceMetadata(_))
        ));
    }

    #[test]
    fn read_write_metadata_reports_missing_file() {
        let temp = TempDir::new().unwrap();
        assert!(read_metadata(temp.path()).unwrap().is_none());

        let manifest = parse_workspace_metadata(manifest_text().as_bytes()).unwrap();
        write_metadata(temp.path(), &manifest).unwrap();
        let round = read_metadata(temp.path()).unwrap().unwrap();
        assert_eq!(round.id().as_str(), "official.demo");
        assert_eq!(round.version().as_str(), "1.0.0");
        assert_eq!(round.name(), Some("演示包"));
        // 原子写产物即固定顺序序列化
        assert_eq!(
            std::fs::read_to_string(metadata_path(temp.path())).unwrap(),
            manifest_text()
        );
    }

    #[test]
    fn stats_counts_yaml_kinds_and_all_files_recursively() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("scripts/nested")).unwrap();
        std::fs::write(root.join("scripts/a.yaml"), b"steps: []").unwrap();
        std::fs::write(root.join("scripts/nested/b.yml"), b"steps: []").unwrap();
        std::fs::write(root.join("scripts/notes.txt"), b"ignored").unwrap();
        std::fs::write(root.join("scripts/.hidden.yaml"), b"ignored").unwrap();
        std::fs::create_dir_all(root.join("templates/sub")).unwrap();
        std::fs::write(root.join("templates/icon.png"), b"png").unwrap();
        std::fs::write(root.join("templates/sub/raw.bin"), b"bin").unwrap();
        std::fs::create_dir_all(root.join("keymaps")).unwrap();
        std::fs::write(root.join("keymaps/wasd.yaml"), b"keymap").unwrap();
        std::fs::create_dir_all(root.join("resources")).unwrap();
        std::fs::write(root.join("resources/blob.txt"), b"x").unwrap();
        // functions / presets 目录缺失 → 0

        let stats = compute_stats(root);
        assert_eq!(stats.scripts, 2, "只数 yaml/yml，跳过隐藏与其它扩展名");
        assert_eq!(stats.templates, 2, "templates 计全部文件（含子目录）");
        assert_eq!(stats.keymaps, 1);
        assert_eq!(stats.functions, 0);
        assert_eq!(stats.presets, 0);
        assert_eq!(stats.resources, 1);
    }
}
