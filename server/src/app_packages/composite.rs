//! Composite 资源解析缝（Phase 4「务实范围」接线）。
//!
//! 解析顺序固定为：**user-overrides → active App Package → legacy 分区目录**。
//! 前两层在本模块实现；legacy 分区目录由调用方（scripts.rs / keymaps.rs 的
//! 既有分区逻辑）兜底。
//!
//! 覆盖范围：
//! - 模板：`find`/`match` 匹配路径与 script_v2 校验可用性共用
//!   （`ScriptStore::resolve_template_path` / `template_avail`）；
//! - 按键映射：`KeymapStore::get` / `list` 可见包内置方案；
//! - 脚本/函数库：运行快照（engine/snapshot.rs）分别合并包内 `scripts/` 与
//!   `functions/`（对应分区 scripts/ + functions/ 语义），override 优先、分区兜底。
//!
//! override 目录沿用 `user-overrides/<android-package>/<资源根>/<路径>` 布局
//! （与 [`super::store::AppPackageStore::write_user_override`] 一致）；模板
//! override 只认精确文件名，包内与分区一致支持「基名 + `#` 后缀」唯一消歧。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::manifest::parse_manifest;
use super::model::{
    parse_android_package_name, parse_app_package_id, AndroidPackageName, AppPackageId,
    InstalledVersion,
};

/// 命中资源的来源层（诊断与测试用）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CompositeSource {
    UserOverride,
    InstalledPackage {
        app_package: AppPackageId,
        version: InstalledVersion,
    },
}

/// 一次 composite 命中：宿主路径 + 来源。
#[derive(Clone, Debug)]
pub(crate) struct CompositeHit {
    pub(crate) path: PathBuf,
    pub(crate) source: CompositeSource,
}

/// 模板短名解析结果：与 legacy 分区语义对齐（零候选 / 多候选均明确区分）。
#[derive(Clone, Debug)]
pub(crate) enum TemplateLookup {
    Found(CompositeHit),
    NotFound,
    Ambiguous {
        name: String,
        candidates: Vec<String>,
    },
}

/// 当前对某 Android package 生效的 active App Package 版本。
#[derive(Clone, Debug)]
pub(crate) struct ActivePackage {
    pub(crate) id: AppPackageId,
    pub(crate) version: InstalledVersion,
    /// 已安装版本的根目录（含 manifest.toml）。
    pub(crate) root: PathBuf,
}

impl ActivePackage {
    /// 包内模板短名解析：精确名优先，否则同扩展名「基名#」唯一匹配。
    pub(crate) fn template(&self, short: &str) -> TemplateLookup {
        let dir = self.root.join("templates");
        match match_short_name(&dir, short) {
            ShortMatch::Found(path) => TemplateLookup::Found(CompositeHit {
                path,
                source: CompositeSource::InstalledPackage {
                    app_package: self.id.clone(),
                    version: self.version.clone(),
                },
            }),
            ShortMatch::NotFound => TemplateLookup::NotFound,
            ShortMatch::Ambiguous { name, candidates } => {
                TemplateLookup::Ambiguous { name, candidates }
            }
        }
    }

    /// 包内脚本源码（`scripts/` 递归，key = 包内相对路径含扩展名，分隔符统一
    /// `/`）。对应分区 scripts/（call 目标）。函数库在包内 `functions/`，
    /// 经 [`ActivePackage::function_sources`] 提供，两类索引互不混入。
    pub(crate) fn script_sources(&self) -> std::io::Result<BTreeMap<String, String>> {
        read_yaml_sources(&self.root.join("scripts"))
    }

    /// 包内函数库源码（`functions/` 递归，key = 包内相对路径含扩展名；运行
    /// 快照按去扩展名短路径索引）。对应分区 functions/ 语义。
    pub(crate) fn function_sources(&self) -> std::io::Result<BTreeMap<String, String>> {
        read_yaml_sources(&self.root.join("functions"))
    }

    /// 包内按键映射：`keymaps/<名称>` 精确文件名。
    pub(crate) fn keymap(&self, name: &str) -> Option<PathBuf> {
        let path = self.root.join("keymaps").join(name);
        is_regular_file(&path).ok().filter(|found| *found)?;
        Some(path)
    }

    /// 包内按键映射文件名清单（合法 .yaml/.yml，字典序）。
    pub(crate) fn keymap_names(&self) -> Vec<String> {
        let dir = self.root.join("keymaps");
        let mut names: Vec<String> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| {
                let lower = name.to_ascii_lowercase();
                lower.ends_with(".yaml") || lower.ends_with(".yml")
            })
            .collect();
        names.sort();
        names
    }
}

/// 无状态 composite 解析器：全部事实（overrides、active 注册表、安装包）
/// 都落在 `data_root` 文件系统上，可按需随处构造。
#[derive(Clone, Debug)]
pub(crate) struct CompositeResolver {
    data_root: PathBuf,
}

impl CompositeResolver {
    pub(crate) fn new(data_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
        }
    }

    fn overrides_root(&self, android: &AndroidPackageName) -> PathBuf {
        self.data_root.join("user-overrides").join(android.as_str())
    }

    /// 查找对某 Android package 生效的 active App Package（读 active 注册表
    /// 并逐个校验 manifest 的 android targets）。
    pub(crate) fn active_package(&self, android: &str) -> Option<ActivePackage> {
        let android = parse_android_package_name(android).ok()?;
        let registry = super::store::ActiveRegistry::load(&self.data_root).ok()?;
        for (id, version) in registry.iter() {
            let Ok(id) = parse_app_package_id(id) else {
                continue;
            };
            let Ok(version) = InstalledVersion::parse(version) else {
                continue;
            };
            let root = self
                .data_root
                .join("app-packages")
                .join(id.as_str())
                .join(version.as_str());
            let Ok(bytes) = fs::read(root.join("manifest.toml")) else {
                continue;
            };
            let Ok(manifest) = parse_manifest(&bytes) else {
                continue;
            };
            if manifest.id() == &id
                && manifest.version() == &version
                && manifest.supports_android_package(&android)
            {
                return Some(ActivePackage { id, version, root });
            }
        }
        None
    }

    /// 模板短名 composite 解析（override 精确名 → active 包短名消歧）。
    /// 两层都未命中时返回 [`TemplateLookup::NotFound`]，由调用方回退 legacy 分区。
    pub(crate) fn template(&self, android: &str, short: &str) -> TemplateLookup {
        let Some(android) = parse_android_package_name(android).ok() else {
            return TemplateLookup::NotFound;
        };
        if let Some(name) = crate::scripts::sanitize_template_name(short) {
            let path = self.overrides_root(&android).join("templates").join(&name);
            if is_regular_file(&path).unwrap_or(false) {
                return TemplateLookup::Found(CompositeHit {
                    path,
                    source: CompositeSource::UserOverride,
                });
            }
        }
        match self.active_package(android.as_str()) {
            Some(active) => active.template(short),
            None => TemplateLookup::NotFound,
        }
    }

    /// 按键映射 composite 解析（override 精确名 → active 包）。
    pub(crate) fn keymap(&self, android: &str, name: &str) -> Option<CompositeHit> {
        let android = parse_android_package_name(android).ok()?;
        let override_path = self.overrides_root(&android).join("keymaps").join(name);
        if is_regular_file(&override_path).unwrap_or(false) {
            return Some(CompositeHit {
                path: override_path,
                source: CompositeSource::UserOverride,
            });
        }
        let active = self.active_package(android.as_str())?;
        let path = active.keymap(name)?;
        Some(CompositeHit {
            path,
            source: CompositeSource::InstalledPackage {
                app_package: active.id,
                version: active.version,
            },
        })
    }

    /// 脚本 composite 源码：override（`user-overrides/<android>/scripts/`）
    /// 覆盖 active 包 `scripts/`；分区 scripts/ 由运行快照兜底（优先级最低）。
    pub(crate) fn script_sources(
        &self,
        android: &str,
    ) -> std::io::Result<BTreeMap<String, String>> {
        let mut merged = BTreeMap::new();
        if let Some(active) = self.active_package(android) {
            merged.extend(active.script_sources()?);
        }
        if let Ok(android) = parse_android_package_name(android) {
            merged.extend(read_yaml_sources(
                &self.overrides_root(&android).join("scripts"),
            )?);
        }
        Ok(merged)
    }

    /// 函数库 composite 源码：override（`user-overrides/<android>/functions/`）
    /// 覆盖 active 包 `functions/`；分区 functions/ 由运行快照兜底。
    pub(crate) fn function_sources(
        &self,
        android: &str,
    ) -> std::io::Result<BTreeMap<String, String>> {
        let mut merged = BTreeMap::new();
        if let Some(active) = self.active_package(android) {
            merged.extend(active.function_sources()?);
        }
        if let Ok(android) = parse_android_package_name(android) {
            merged.extend(read_yaml_sources(
                &self.overrides_root(&android).join("functions"),
            )?);
        }
        Ok(merged)
    }

    /// 按键映射文件名 union（override 优先、其后包内；均为字典序）。
    pub(crate) fn keymap_names(&self, android: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        if let Ok(android) = parse_android_package_name(android) {
            if let Ok(entries) = fs::read_dir(self.overrides_root(&android).join("keymaps")) {
                for entry in entries.flatten() {
                    if let Ok(name) = entry.file_name().into_string() {
                        let lower = name.to_ascii_lowercase();
                        if entry.path().is_file()
                            && (lower.ends_with(".yaml") || lower.ends_with(".yml"))
                        {
                            names.push(name);
                        }
                    }
                }
            }
        }
        if let Some(active) = self.active_package(android) {
            for name in active.keymap_names() {
                if !names
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(&name))
                {
                    names.push(name);
                }
            }
        }
        names.sort();
        names
    }
}

/// 递归读取目录下全部 `.yaml`/`.yml` 文件（key = 相对路径含扩展名，`/` 分隔）。
fn read_yaml_sources(root: &Path) -> std::io::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    if !root.is_dir() {
        return Ok(out);
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            if !matches!(ext.as_deref(), Some("yaml") | Some("yml")) {
                continue;
            }
            let key = match path.strip_prefix(root) {
                Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            let content = fs::read_to_string(&path)?;
            out.insert(key, content);
        }
    }
    Ok(out)
}

enum ShortMatch {
    Found(PathBuf),
    NotFound,
    Ambiguous {
        name: String,
        candidates: Vec<String>,
    },
}

/// 精确名优先；否则按「基名 + `#` 后缀 + 同扩展名」唯一匹配（与
/// scripts.rs 分区内核同规则，短名消歧语义在包内保持一致）。
fn match_short_name(dir: &Path, short: &str) -> ShortMatch {
    let Some(name) = crate::scripts::sanitize_template_name(short) else {
        return ShortMatch::NotFound;
    };
    let exact = dir.join(&name);
    if is_regular_file(&exact).unwrap_or(false) {
        return ShortMatch::Found(exact);
    }
    let Some((base, ext)) = name.rsplit_once('.') else {
        return ShortMatch::NotFound;
    };
    let prefix = format!("{}#", base.to_ascii_lowercase());
    let dotted = format!(".{}", ext.to_ascii_lowercase());
    let mut candidates: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|candidate| {
            let lower = candidate.to_ascii_lowercase();
            lower.starts_with(&prefix) && lower.ends_with(&dotted)
        })
        .collect();
    candidates.sort();
    match candidates.len() {
        1 => ShortMatch::Found(dir.join(&candidates[0])),
        0 => ShortMatch::NotFound,
        _ => ShortMatch::Ambiguous { name, candidates },
    }
}

fn is_regular_file(path: &Path) -> std::io::Result<bool> {
    Ok(fs::symlink_metadata(path)?.file_type().is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_packages::store::ActiveRegistry;

    fn write_manifest(root: &Path, id: &str, version: &str, android: &str) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(
            root.join("manifest.toml"),
            format!(
                "format_version = 2\nid = \"{id}\"\nversion = \"{version}\"\n[android]\npackages = [\"{android}\"]\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn active_package_requires_manifest_target_match() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        write_manifest(
            &root.join("app-packages/official.a/1.0.0"),
            "official.a",
            "1.0.0",
            "com.example.game",
        );
        ActiveRegistry::from_iter([("official.a".to_string(), "1.0.0".to_string())])
            .save(root)
            .unwrap();

        let resolver = CompositeResolver::new(root);
        let active = resolver.active_package("com.example.game").unwrap();
        assert_eq!(active.id.as_str(), "official.a");
        assert!(resolver.active_package("com.other.game").is_none());
        assert!(resolver.active_package("not an android pkg").is_none());
    }

    #[test]
    fn template_lookup_prefers_override_then_package() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let package_root = root.join("app-packages/official.a/1.0.0");
        write_manifest(&package_root, "official.a", "1.0.0", "com.example.game");
        std::fs::create_dir_all(package_root.join("templates")).unwrap();
        std::fs::write(package_root.join("templates/icon#1_2_3_4.png"), b"package").unwrap();

        ActiveRegistry::from_iter([("official.a".to_string(), "1.0.0".to_string())])
            .save(root)
            .unwrap();
        let resolver = CompositeResolver::new(root);

        let from_package = resolver.template("com.example.game", "icon.png");
        match from_package {
            TemplateLookup::Found(hit) => {
                assert_eq!(hit.path.file_name().unwrap(), "icon#1_2_3_4.png");
                assert_eq!(
                    hit.source,
                    CompositeSource::InstalledPackage {
                        app_package: parse_app_package_id("official.a").unwrap(),
                        version: InstalledVersion::parse("1.0.0").unwrap(),
                    }
                );
            }
            other => panic!("expected package hit, got {other:?}"),
        }

        let override_file = root
            .join("user-overrides/com.example.game/templates")
            .join("icon.png");
        std::fs::create_dir_all(override_file.parent().unwrap()).unwrap();
        std::fs::write(&override_file, b"override").unwrap();
        match resolver.template("com.example.game", "icon.png") {
            TemplateLookup::Found(hit) => {
                assert_eq!(hit.source, CompositeSource::UserOverride);
                assert_eq!(std::fs::read(&hit.path).unwrap(), b"override");
            }
            other => panic!("expected override hit, got {other:?}"),
        }

        assert!(matches!(
            resolver.template("com.example.game", "missing"),
            TemplateLookup::NotFound
        ));
        assert!(matches!(
            resolver.template("com.example.game", "../escape"),
            TemplateLookup::NotFound
        ));
    }

    /// 包内 scripts/ 与 functions/ 两个索引互不混入；override 函数库覆盖包内同名。
    #[test]
    fn script_and_function_sources_stay_in_their_own_roots() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let package_root = root.join("app-packages/official.a/1.0.0");
        write_manifest(&package_root, "official.a", "1.0.0", "com.example.game");
        std::fs::create_dir_all(package_root.join("scripts")).unwrap();
        std::fs::write(package_root.join("scripts/daily.yaml"), b"package script").unwrap();
        std::fs::create_dir_all(package_root.join("functions")).unwrap();
        std::fs::write(
            package_root.join("functions/common.yaml"),
            b"package function",
        )
        .unwrap();

        ActiveRegistry::from_iter([("official.a".to_string(), "1.0.0".to_string())])
            .save(root)
            .unwrap();
        let resolver = CompositeResolver::new(root);

        let scripts = resolver.script_sources("com.example.game").unwrap();
        assert_eq!(
            scripts.get("daily.yaml").map(String::as_str),
            Some("package script")
        );
        assert!(
            !scripts.contains_key("common.yaml"),
            "包内 functions/ 不得混入脚本索引"
        );

        let functions = resolver.function_sources("com.example.game").unwrap();
        assert_eq!(
            functions.get("common.yaml").map(String::as_str),
            Some("package function")
        );
        assert!(
            !functions.contains_key("daily.yaml"),
            "包内 scripts/ 不得混入函数库索引"
        );

        let override_dir = root.join("user-overrides/com.example.game/functions");
        std::fs::create_dir_all(&override_dir).unwrap();
        std::fs::write(override_dir.join("common.yaml"), b"override function").unwrap();
        let functions = resolver.function_sources("com.example.game").unwrap();
        assert_eq!(
            functions.get("common.yaml").map(String::as_str),
            Some("override function"),
            "override 必须覆盖包内同名函数库"
        );
    }
}
