//! PackageBuilder：本地编辑区（`data/<android 包名>/`）→ 可安装 `.gamerpkg`
//! 归档的导出流水线。
//!
//! 流水线（业务逻辑收在这里，HTTP handler 只做参数解析与响应装配）：
//!
//! ```text
//! PackageBuilder
//! ├── load_metadata()      // 读 package.toml（缺失 → WorkspaceNotFound）
//! ├── validate_source()    // Preflight：收集全部问题而非首失败即停
//! ├── collect_resources()  // 六目录递归收集（排序、跳过隐藏文件）
//! ├── build_archive()      // manifest.toml（自然第一）+ 按路径排序的资源条目
//! └── verify_archive()     // 复用安装侧归档校验 + 条目集合核对 + SHA-256
//! ```
//!
//! 可复现打包：条目按相对路径排序、路径统一 `/`、UTF-8 文件名、Deflated、
//! 固定 mtime（2000-01-01，DOS 日期范围 1980–2107 内任选的常量，使相同输入
//! 产生逐字节相同的归档 → 相同 SHA-256）、无 zip 注释/额外字段。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use zip::write::SimpleFileOptions;

use crate::matcher;
use crate::resources::{ResourceKind as CoreKind, ResourceStore};

use super::archive::{
    validate_and_read_manifest, MAX_PACKAGE_ENTRIES, MAX_PACKAGE_FILE_BYTES,
    MAX_PACKAGE_TOTAL_BYTES,
};
use super::error::{AppPackageError, AppPackageResult};
use super::manifest::PackageManifest;
use super::model::{parse_android_package_name, AndroidPackageName, ResourcePath};
use super::{presets, workspace};

/// 六个合法资源根（与 `ResourceKind` 一致；显式列出用于递归扫描入口，
/// edit 提取链路复用同一清单）。
pub(crate) const RESOURCE_ROOTS: [&str; 6] = [
    "scripts",
    "functions",
    "templates",
    "keymaps",
    "presets",
    "resources",
];

/// 固定 mtime：2000-01-01 00:00:00（可复现打包，见模块注释）。
fn fixed_mtime() -> zip::DateTime {
    zip::DateTime::from_date_and_time(2000, 1, 1, 0, 0, 0).expect("常量时间在 DOS 日期范围内")
}

/// 一个待打包资源文件：包内相对路径（`ResourcePath` 已校验）+ 磁盘绝对路径。
#[derive(Debug, Clone)]
pub(crate) struct CollectedFile {
    pub(crate) path: ResourcePath,
    pub(crate) absolute: PathBuf,
    pub(crate) size: usize,
}

/// 导出产物：归档字节 + 自检摘要。
#[derive(Debug)]
pub(crate) struct BuiltPackage {
    pub(crate) manifest: PackageManifest,
    pub(crate) archive: Vec<u8>,
    pub(crate) sha256: String,
    pub(crate) entries: Vec<String>,
}

pub(crate) struct PackageBuilder {
    data_root: PathBuf,
    android: AndroidPackageName,
    resources: Arc<ResourceStore>,
}

impl PackageBuilder {
    pub(crate) fn new(
        data_root: impl Into<PathBuf>,
        android: AndroidPackageName,
        resources: Arc<ResourceStore>,
    ) -> Self {
        Self {
            data_root: data_root.into(),
            android,
            resources,
        }
    }

    fn dir(&self) -> PathBuf {
        workspace::workspace_dir(&self.data_root, &self.android)
    }

    /// 读工作区元数据（package.toml；与 manifest V2 同一套校验规则）。
    pub(crate) fn load_metadata(&self) -> AppPackageResult<PackageManifest> {
        let dir = self.dir();
        match workspace::read_metadata(&dir)? {
            Some(metadata) => Ok(metadata),
            None => Err(AppPackageError::WorkspaceNotFound(format!(
                "{}（先用 PUT /api/workspace/{} 初始化元数据）",
                workspace::metadata_path(&dir).display(),
                self.android
            ))),
        }
    }

    /// Preflight：全量收集问题（路径/大小/上限 + 每类资源过各自现有校验器；
    /// package.toml 字段已在 load_metadata 经 parse_manifest 全量校验），
    /// 有问题 → `PreflightFailed{problems}`；通过 → 返回排序后的收集结果。
    pub(crate) fn validate_source(&self) -> AppPackageResult<Vec<CollectedFile>> {
        self.validate_dir(&self.dir())
    }

    /// Preflight 任意目录（edit 提取链路对 staging 目录复用同一套校验器）。
    /// 目录形状与工作区一致（六个资源根 + 可选 package.toml）；资源校验语境
    /// （脚本/函数分区名、模板解析兜底）仍取 `self.android`；脚本/函数跨文件
    /// 引用（call/func）以被校验目录自身内容为最高优先视图（见函数体注释）。
    pub(crate) fn validate_dir(&self, dir: &Path) -> AppPackageResult<Vec<CollectedFile>> {
        let mut problems: Vec<String> = Vec::new();
        let files = Self::collect_resources(dir, &mut problems);
        // 上限护栏：条目数（含 manifest.toml）与解压总量对齐安装侧预算
        if files.len() + 1 > MAX_PACKAGE_ENTRIES {
            problems.push(format!(
                "资源条目数 {} 超过上限 {MAX_PACKAGE_ENTRIES}",
                files.len()
            ));
        }
        let total: usize = files.iter().map(|file| file.size).sum();
        if total > MAX_PACKAGE_TOTAL_BYTES {
            problems.push(format!(
                "解压总量 {total} 字节超过上限 {MAX_PACKAGE_TOTAL_BYTES} 字节"
            ));
        }

        // 脚本/函数跨文件引用（call/func）的自洽校验：以「被校验目录自身」的
        // scripts/functions 内容为最高优先视图（扩展经 StagedResourceValidator
        // 回调）。edit 提取校验的是 staging 快照——本地编辑区此时可能为空或
        // 不同源，若只读本地目录，「提取到空工作区」会永远 preflight 失败；
        // 导出路径目录即工作区，注入内容与本地一致，行为不变。keymaps 内容
        // 校验经各自 kind 的 ResourceKindHandler 逐文件回调。模板可用性不在
        // 此注入。
        let mut staged: Vec<(CoreKind, String, String)> = Vec::new();
        for file in &files {
            let name = file
                .path
                .as_str()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !(name.ends_with(".yaml") || name.ends_with(".yml")) {
                continue;
            }
            let kind = match file.path.kind() {
                super::model::ResourceKind::Scripts => CoreKind::Scripts,
                super::model::ResourceKind::Functions => CoreKind::Functions,
                super::model::ResourceKind::Keymaps => CoreKind::Keymaps,
                _ => continue,
            };
            let Ok(content) = read_utf8(&file.absolute) else {
                continue; // 读取失败由下方逐文件校验记入 problems
            };
            staged.push((kind, trim_root_for_kind(file.path.as_str(), kind), content));
        }
        // scripts/functions 走跨文件引用视图（staged 内容自身为最高优先）；
        // keymaps 等无引用语义的 kind 逐条经各自 handler 校验。
        let (script_func, standalone): (Vec<_>, Vec<_>) = staged
            .into_iter()
            .partition(|(kind, ..)| matches!(kind, CoreKind::Scripts | CoreKind::Functions));
        for problem in self
            .resources
            .validate_staged(self.android.as_str(), &script_func)
        {
            problems.push(problem);
        }
        for (kind, rel, content) in standalone {
            if let Err(diagnostics) =
                self.resources
                    .validate_content(kind, self.android.as_str(), &rel, &content)
            {
                problems.push(format!(
                    "{}/{}: {}",
                    kind.as_str(),
                    rel,
                    crate::resources::format_diagnostics_value(&diagnostics)
                ));
            }
        }
        // 逐文件校验（templates PNG 解码 / presets 解析器）
        for file in &files {
            self.validate_file(file, &mut problems);
        }

        if !problems.is_empty() {
            return Err(AppPackageError::preflight_failed(problems));
        }
        Ok(files)
    }

    /// YAML 资源在跨文件引用视图里的注入键：(资源种类, scripts/ 相对资源 id
    /// 或 functions/ 文件短路径)。非 scripts/functions 的 YAML 返回 None。
    fn yaml_reference_entry(file: &CollectedFile) -> Option<(super::model::ResourceKind, &str)> {
        let relative = file.path.as_str();
        let name = relative.rsplit('/').next().unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".yaml") || lower.ends_with(".yml")) {
            return None;
        }
        match file.path.kind() {
            super::model::ResourceKind::Scripts => Some((
                super::model::ResourceKind::Scripts,
                trim_root(relative, "scripts"),
            )),
            super::model::ResourceKind::Functions => Some((
                super::model::ResourceKind::Functions,
                trim_root(relative, "functions"),
            )),
            _ => None,
        }
    }

    /// 六目录递归收集：排序、跳过隐藏文件/目录；路径非法与单文件超限记入
    /// problems（不中止），其余文件原样收集。
    fn collect_resources(dir: &Path, problems: &mut Vec<String>) -> Vec<CollectedFile> {
        let mut files = Vec::new();
        for kind in RESOURCE_ROOTS {
            Self::collect_kind(&dir.join(kind), kind.to_string(), &mut files, problems);
        }
        files.sort_by(|left, right| left.path.as_str().cmp(right.path.as_str()));
        files
    }

    fn collect_kind(
        dir: &Path,
        relative: String,
        files: &mut Vec<CollectedFile>,
        problems: &mut Vec<String>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return; // 目录缺失 = 该类资源为空
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                problems.push(format!("{relative}: 文件名必须是 UTF-8"));
                continue;
            };
            if name.starts_with('.') {
                continue; // 隐藏文件/目录不进包
            }
            let path = entry.path();
            let child_relative = format!("{relative}/{name}");
            if path.is_dir() {
                Self::collect_kind(&path, child_relative, files, problems);
                continue;
            }
            if !path.is_file() {
                continue; // 符号链接等非常规条目不进包
            }
            match ResourcePath::parse(&child_relative) {
                Ok(resource_path) => match entry.metadata() {
                    Ok(metadata) => {
                        let size = metadata.len() as usize;
                        if size > MAX_PACKAGE_FILE_BYTES {
                            problems.push(format!(
                                "{child_relative}: 单文件 {size} 字节超过上限 {MAX_PACKAGE_FILE_BYTES} 字节"
                            ));
                            continue;
                        }
                        files.push(CollectedFile {
                            path: resource_path,
                            absolute: path,
                            size,
                        });
                    }
                    Err(error) => {
                        problems.push(format!("{child_relative}: 读取文件元数据失败: {error}"))
                    }
                },
                Err(error) => problems.push(format!("{child_relative}: 路径非法（{error}）")),
            }
        }
    }

    /// 单文件按资源根分发到各自现有校验器（不写包专用 parser）。
    /// scripts/functions/keymaps 的内容校验已经过 staged 回调（见
    /// validate_dir），这里保留 templates（PNG 解码）与 presets（包安装侧
    /// 同一解析器）的逐文件校验。
    fn validate_file(&self, file: &CollectedFile, problems: &mut Vec<String>) {
        let relative = file.path.as_str();
        let name = file
            .path
            .as_str()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let is_yaml = name.ends_with(".yaml") || name.ends_with(".yml");
        match file.path.kind() {
            // 非 YAML 文件不进索引，放行为包内附件（staged 校验已跳过）
            super::model::ResourceKind::Scripts
            | super::model::ResourceKind::Functions
            | super::model::ResourceKind::Keymaps => {}
            super::model::ResourceKind::Templates => {
                // 内存中重编码即校验（不落盘）；`#1` 后缀保留颜色，其余灰度——
                // 与模板上传/替换链路同一语义
                let grayscale_only = !matcher::template_color_from_name(relative);
                let bytes = match std::fs::read(&file.absolute) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        problems.push(format!("{relative}: 读取失败: {error}"));
                        return;
                    }
                };
                if let Err(error) = matcher::reencode_template_png(&bytes, grayscale_only) {
                    problems.push(format!("{relative}: 模板无法解码: {error}"));
                }
            }
            super::model::ResourceKind::Presets => {
                if !is_yaml {
                    return;
                }
                let bytes = match std::fs::read(&file.absolute) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        problems.push(format!("{relative}: 读取失败: {error}"));
                        return;
                    }
                };
                // 包安装侧同一解析器；source 带包内路径语境，不放宽任何校验
                if let Err(error) = presets::parse_preset(&bytes, relative) {
                    problems.push(format!("{relative}: {error}"));
                }
            }
            super::model::ResourceKind::Resources => {
                // 只查路径合法与大小上限（collect_resources 已覆盖），无内容校验
            }
        }
    }

    /// 构建 manifest.toml 文本（固定字段顺序）。
    pub(crate) fn build_manifest(metadata: &PackageManifest) -> String {
        workspace::serialize_manifest_toml(metadata)
    }

    /// 打 zip 字节：manifest.toml 自然第一，其余条目按相对路径排序（调用方
    /// 传入已排序的 collect 结果），Deflated + 固定 mtime 保证可复现。
    pub(crate) fn build_archive(
        metadata: &PackageManifest,
        files: &[CollectedFile],
    ) -> AppPackageResult<Vec<u8>> {
        let mut bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .last_modified_time(fixed_mtime());
            writer.start_file("manifest.toml", options)?;
            writer.write_all(Self::build_manifest(metadata).as_bytes())?;
            for file in files {
                writer.start_file(file.path.as_str(), options)?;
                let content = std::fs::read(&file.absolute)?;
                writer.write_all(&content)?;
            }
            writer.finish()?;
        }
        Ok(bytes)
    }

    /// 自检：字节走既有安装侧归档校验（含 limits/manifest 解析）+ 条目集合与
    /// collect 结果一致 + 元数据身份一致 + 计算 SHA-256。
    pub(crate) fn verify_archive(
        archive: &[u8],
        files: &[CollectedFile],
        metadata: &PackageManifest,
    ) -> AppPackageResult<String> {
        let manifest_bytes = validate_and_read_manifest(archive)?;
        let parsed = super::manifest::parse_manifest(&manifest_bytes)?;
        if parsed.id() != metadata.id() || parsed.version() != metadata.version() {
            return Err(AppPackageError::PackageBuildFailed(format!(
                "自检失败：归档 manifest 身份（{}@{}）与工作区元数据（{}@{}）不一致",
                parsed.id(),
                parsed.version(),
                metadata.id(),
                metadata.version()
            )));
        }
        let mut reader = zip::ZipArchive::new(std::io::Cursor::new(archive))?;
        let mut archived: Vec<String> = (0..reader.len())
            .filter_map(|index| {
                reader
                    .by_index_raw(index)
                    .ok()
                    .map(|entry| entry.name().to_string())
            })
            .filter(|name| name != "manifest.toml")
            .collect();
        archived.sort();
        let mut expected: Vec<String> = files
            .iter()
            .map(|file| file.path.as_str().to_string())
            .collect();
        expected.sort();
        if archived != expected {
            return Err(AppPackageError::PackageBuildFailed(
                "自检失败：归档条目集合与收集结果不一致".to_string(),
            ));
        }
        Ok(sha256_hex(archive))
    }

    /// 完整导出流水线（load_metadata → validate_source → build → verify）。
    pub(crate) fn export(&self) -> AppPackageResult<BuiltPackage> {
        let metadata = self.load_metadata()?;
        let files = self.validate_source()?;
        let archive = Self::build_archive(&metadata, &files)?;
        let sha256 = Self::verify_archive(&archive, &files, &metadata)?;
        let entries = files
            .iter()
            .map(|file| file.path.as_str().to_string())
            .collect();
        Ok(BuiltPackage {
            manifest: metadata,
            archive,
            sha256,
            entries,
        })
    }
}

fn read_utf8(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("读取失败: {error}"))?;
    String::from_utf8(bytes).map_err(|_| "必须是 UTF-8 文本".to_string())
}

/// 去掉资源根前缀（`scripts/daily.yaml` → `daily.yaml`），作为校验器 resource
/// 语境（与分区存储链路的资源命名一致）。
/// staged 条目的资源 id：scripts/ 内相对路径、functions/ 文件短路径、
/// keymaps 文件名（与各保存链路的资源命名一致）。
fn trim_root_for_kind(relative: &str, kind: CoreKind) -> String {
    match kind {
        CoreKind::Scripts => trim_root(relative, "scripts").to_string(),
        CoreKind::Functions => trim_root(relative, "functions").to_string(),
        CoreKind::Keymaps => trim_root(relative, "keymaps").to_string(),
        _ => relative.to_string(),
    }
}

fn trim_root<'a>(relative: &'a str, root: &str) -> &'a str {
    relative
        .strip_prefix(root)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(relative)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::TempDir;

    use super::*;
    use crate::config::Config;

    fn android() -> AndroidPackageName {
        parse_android_package_name("com.example.game").unwrap()
    }

    fn builder_with(data_root: &Path) -> PackageBuilder {
        let config = Config {
            data_dir: data_root.to_path_buf(),
            ..Default::default()
        };
        let resources = Arc::new(ResourceStore::open(&config).unwrap());
        // 与生产组合根一致：注册扩展内容校验钩子
        crate::extensions::gamer_yaml::register_resource_handlers(&resources);
        crate::extensions::register_resource_handlers(&resources);
        PackageBuilder::new(data_root.to_path_buf(), android(), resources)
    }

    fn write_metadata(data_root: &Path, id: &str, version: &str) -> PackageManifest {
        let text = format!(
            "format_version = 2\nid = \"{id}\"\nversion = \"{version}\"\n\n[android]\npackages = [\"com.example.game\"]\n"
        );
        let manifest = workspace::parse_workspace_metadata(text.as_bytes()).unwrap();
        workspace::write_metadata(&data_root.join("com.example.game"), &manifest).unwrap();
        manifest
    }

    fn valid_png() -> Vec<u8> {
        let mut img = image::GrayImage::new(8, 8);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            pixel.0[0] = if (x + y) % 2 == 0 { 32 } else { 224 };
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageLuma8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    fn seed_workspace(data_root: &Path) -> PathBuf {
        let ws = data_root.join("com.example.game");
        std::fs::create_dir_all(ws.join("scripts")).unwrap();
        std::fs::write(ws.join("scripts/daily.yaml"), b"steps: []\n").unwrap();
        std::fs::create_dir_all(ws.join("functions")).unwrap();
        std::fs::write(ws.join("functions/common.yaml"), b"login:\n  steps: []\n").unwrap();
        std::fs::create_dir_all(ws.join("templates")).unwrap();
        std::fs::write(ws.join("templates/main.png"), valid_png()).unwrap();
        std::fs::create_dir_all(ws.join("keymaps")).unwrap();
        std::fs::write(
            ws.join("keymaps/wasd.yaml"),
            b"version: 1\nname: wasd\nbindings: []\n",
        )
        .unwrap();
        std::fs::create_dir_all(ws.join("presets")).unwrap();
        std::fs::write(
            ws.join("presets/daily.yaml"),
            b"name: daily\nrunner_id: gamer.yaml\nentrypoint: run\npayload: {}\nschedule:\n  kind: cron\n  value:\n    expression: \"0 8 * * *\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(ws.join("resources")).unwrap();
        std::fs::write(ws.join("resources/config.json"), b"{}").unwrap();
        // 隐藏文件不进包
        std::fs::write(ws.join("scripts/.hidden.yaml"), b"steps: []\n").unwrap();
        ws
    }

    /// 平台相关地造一个 ResourcePath 必然拒绝的资源文件：
    /// Linux 文件名可含反斜杠；Windows 经 \\?\ 扩展路径创建保留名 con.txt。
    fn seed_invalid_resource_path(ws: &Path) {
        #[cfg(unix)]
        {
            std::fs::write(ws.join("resources/a\\b.txt"), b"odd").unwrap();
        }
        #[cfg(windows)]
        {
            let extended = format!("\\\\?\\{}", ws.join("resources").join("con.txt").display());
            std::fs::write(extended, b"odd").unwrap();
        }
    }

    #[test]
    fn reproducible_archive_bytes() {
        let temp = TempDir::new().unwrap();
        seed_workspace(temp.path());
        let metadata = write_metadata(temp.path(), "official.demo", "1.0.0");
        let builder = builder_with(temp.path());
        let files = builder.validate_source().unwrap();

        let first = PackageBuilder::build_archive(&metadata, &files).unwrap();
        let second = PackageBuilder::build_archive(&metadata, &files).unwrap();
        assert_eq!(first, second, "相同输入必须产生相同归档字节");
        assert_eq!(sha256_hex(&first), sha256_hex(&second));

        // manifest.toml 是第一个条目
        let mut reader = zip::ZipArchive::new(std::io::Cursor::new(&first)).unwrap();
        assert_eq!(reader.by_index_raw(0).unwrap().name(), "manifest.toml");
    }

    #[test]
    fn preflight_reports_all_problems_at_once() {
        let temp = TempDir::new().unwrap();
        let ws = seed_workspace(temp.path());
        // 坏脚本 + 坏函数库 + 坏模板 + 坏 keymap + 坏 preset + 非法 resources 路径
        std::fs::write(ws.join("scripts/bad.yaml"), b"steps: 42\n").unwrap();
        std::fs::write(ws.join("functions/badlib.yaml"), b"login:\n  extra: 1\n").unwrap();
        std::fs::write(ws.join("templates/broken.png"), b"not a png").unwrap();
        std::fs::write(ws.join("keymaps/bad.yaml"), b"version: 9\n").unwrap();
        std::fs::write(ws.join("presets/bad.yaml"), b"name: \"\"\nschedule: {}\n").unwrap();
        seed_invalid_resource_path(&ws);

        let builder = builder_with(temp.path());
        let error = builder.validate_source().unwrap_err();
        let AppPackageError::PreflightFailed { problems } = &error else {
            panic!("必须是 PreflightFailed: {error:?}");
        };
        for fragment in [
            "scripts/bad.yaml",
            "functions/badlib.yaml",
            "templates/broken.png",
            "keymaps/bad.yaml",
            "presets/bad.yaml",
            "resources/",
        ] {
            assert!(
                problems.contains(&fragment.to_string()),
                "缺少 {fragment}: {problems}"
            );
        }
    }

    #[test]
    fn load_metadata_requires_package_toml() {
        let temp = TempDir::new().unwrap();
        let builder = builder_with(temp.path());
        let error = builder.load_metadata().unwrap_err();
        assert!(matches!(error, AppPackageError::WorkspaceNotFound(_)));

        std::fs::create_dir_all(temp.path().join("com.example.game")).unwrap();
        std::fs::write(
            temp.path().join("com.example.game/package.toml"),
            b"id = \"official.demo\"\n",
        )
        .unwrap();
        let error = builder.load_metadata().unwrap_err();
        assert!(matches!(
            error,
            AppPackageError::InvalidWorkspaceMetadata(_)
        ));
    }

    /// round-trip：build → 真实 AppPackageStore::install_and_activate →
    /// 包内文件与工作区一致、manifest 字段正确。
    #[tokio::test]
    async fn exported_package_installs_and_matches_workspace() {
        let temp = TempDir::new().unwrap();
        seed_workspace(temp.path());
        write_metadata(temp.path(), "official.demo", "1.2.0");
        let builder = builder_with(temp.path());
        let built = builder.export().unwrap();
        assert_eq!(built.manifest.id().as_str(), "official.demo");
        assert_eq!(built.manifest.version().as_str(), "1.2.0");
        assert_eq!(
            built.manifest.android_packages()[0].as_str(),
            "com.example.game"
        );
        assert_eq!(built.entries.len(), 6, "六类资源各一个文件");

        let store = crate::app_packages::store::AppPackageStore::new(temp.path());
        let installed = store
            .install_and_activate(&built.archive, Some(built.sha256.as_str()))
            .await
            .unwrap();
        assert_eq!(installed.manifest().id().as_str(), "official.demo");

        // 包内文件集合与字节与工作区收集结果一致
        // （manifest.toml / install.json 是安装侧自产文件，不参与资源对比）
        let mut installed_files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        collect_files_recursively(installed.root(), installed.root(), &mut installed_files);
        installed_files.remove("manifest.toml");
        installed_files.remove("install.json");
        let mut expected_files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for file in builder.validate_source().unwrap() {
            expected_files.insert(
                file.path.as_str().to_string(),
                std::fs::read(&file.absolute).unwrap(),
            );
        }
        assert_eq!(installed_files, expected_files, "包内文件必须与工作区一致");
        assert!(installed.root().join("manifest.toml").is_file());
    }

    fn collect_files_recursively(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_files_recursively(root, &path, out);
            } else if path.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }

    #[test]
    fn verify_archive_detects_tampered_bytes() {
        let temp = TempDir::new().unwrap();
        seed_workspace(temp.path());
        let metadata = write_metadata(temp.path(), "official.demo", "1.0.0");
        let builder = builder_with(temp.path());
        let files = builder.validate_source().unwrap();
        let archive = PackageBuilder::build_archive(&metadata, &files).unwrap();
        let sha256 = PackageBuilder::verify_archive(&archive, &files, &metadata).unwrap();
        assert_eq!(sha256, sha256_hex(&archive));

        // 篡改一个字节 → 必须被检出：corrupt 到结构字段时自检直接拒绝，
        // corrupt 到数据区时条目集合不变但 SHA-256 变化，两种都算检出
        let mut tampered = archive.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;
        let detected = match PackageBuilder::verify_archive(&tampered, &files, &metadata) {
            Ok(sha) => sha != sha256,
            Err(_) => true,
        };
        assert!(detected, "篡改字节必须被 verify_archive 检出");

        // 归档缺一个条目 → 自检失败
        let mut fewer = files.clone();
        fewer.pop();
        let smaller = PackageBuilder::build_archive(&metadata, &fewer).unwrap();
        let error = PackageBuilder::verify_archive(&smaller, &files, &metadata).unwrap_err();
        assert!(matches!(error, AppPackageError::PackageBuildFailed(_)));
    }

    /// edit 提取 preflight 的目录自洽性：被校验目录（staging 快照）自身的
    /// scripts/functions 内容优先参与 call/func 引用解析，本地编辑区为空时
    /// 不得误报 resource.func.not_found；目录内部引用悬空仍必须报出。
    #[test]
    fn validate_dir_resolves_cross_references_from_directory_itself() {
        let temp = TempDir::new().unwrap();
        // 被校验目录（模拟 staging）：脚本引用同目录函数库，本地编辑区完全为空
        let staging = temp.path().join("staging");
        std::fs::create_dir_all(staging.join("scripts")).unwrap();
        std::fs::write(
            staging.join("scripts/daily.yaml"),
            b"steps:\n  - func: common/greet\n",
        )
        .unwrap();
        std::fs::create_dir_all(staging.join("functions")).unwrap();
        std::fs::write(
            staging.join("functions/common.yaml"),
            b"greet:\n  steps:\n    - return: true\n",
        )
        .unwrap();
        assert!(builder_with(temp.path()).validate_dir(&staging).is_ok());

        // 目录内引用悬空（函数文件缺 greet）→ preflight 必须失败
        std::fs::write(
            staging.join("functions/common.yaml"),
            b"other:\n  steps:\n    - return: true\n",
        )
        .unwrap();
        let error = builder_with(temp.path())
            .validate_dir(&staging)
            .unwrap_err();
        let AppPackageError::PreflightFailed { problems } = error else {
            panic!("期望 PreflightFailed，实际 {error:?}");
        };
        assert!(
            problems.contains("resource.func.not_found"),
            "悬空 func 引用必须报 not_found: {problems}"
        );
    }
}
