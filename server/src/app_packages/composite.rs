//! Composite 资源解析缝（三层统一）。
//!
//! 解析顺序固定为：**EditableLocal（本地编辑区）→ user-overrides → active
//! App Package**，对所有资源类型（模板/按键映射/脚本/函数库）一致；同名资源
//! 高优先层遮蔽低优先层。三层都在本模块实现，调用方不再各自兜底。
//!
//! 覆盖范围：
//! - 模板：`find`/`match` 匹配路径与 script_v2 校验可用性共用
//!   （`ScriptStore::resolve_template_path` / `template_avail`）；
//! - 按键映射：`KeymapStore::get` / `list` 可见全部三层方案；
//! - 脚本/函数库：运行快照（engine/snapshot.rs）分别合并 `scripts/` 与
//!   `functions/` 三层同名源码（对应分区 scripts/ + functions/ 语义）。
//!
//! 各层布局（目录即类型）：
//! - EditableLocal = 本地编辑区，即 server 数据根下的分区目录
//!   `<data_root>/<android_package>/<资源根>/…`（用户可直接编辑，写入接口
//!   也只写这一层）；
//! - override 沿用 `user-overrides/<android-package>/<资源根>/<路径>` 布局
//!   （与 [`super::store::AppPackageStore::write_user_override`] 一致）；
//! - active App Package 为安装目录内不可变内容。
//!
//! 任务预设（presets/）明确不接 composite：预设只在包激活时发布为任务预设，
//! 本地 presets/ 仅随包搬运，不参与运行时解析。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::manifest::parse_manifest;
use super::model::{
    parse_android_package_name, parse_app_package_id, AndroidPackageName, AppPackageId,
    InstalledVersion,
};

/// 命中资源的来源层（诊断与测试可断言「这资源来自哪一层」）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CompositeSource {
    /// 本地编辑区分区目录（`<data_root>/<android_package>/…`），最高优先。
    EditableLocal,
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

/// 模板短名解析结果：与本地编辑区分区语义对齐（零候选 / 多候选均明确区分）。
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

/// 无状态 composite 解析器：全部事实（本地编辑区分区、overrides、active 注册
/// 表、安装包）都落在 `data_root` 文件系统上，可按需随处构造。本地编辑区根 =
/// `data_root` 本身（分区即 `<data_root>/<android_package>/`），无需额外装配。
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

    /// 本地编辑区分区根：`<data_root>/<pkg>/`。分区名走与 ScriptStore /
    /// KeymapStore 相同的 [`crate::core::fs::safe_name`] 校验（分区名不必满足
    /// Android 包名文法），非法名 = 该层为空。
    fn editable_partition(&self, pkg: &str) -> Option<PathBuf> {
        crate::core::fs::safe_name(pkg).map(|name| self.data_root.join(name))
    }

    /// keymap 文件名防穿越/分隔符守卫（合法名由调用方规范化，这里只挡显式
    /// 路径拼接逃逸）。
    fn keymap_name_is_plain(name: &str) -> bool {
        !name.is_empty() && !name.contains(['/', '\\']) && !name.contains("..")
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

    /// 模板短名 composite 解析：**本地编辑区 → override → active 包**，逐层
    /// 解析（每层内先精确名、再「基名 + `#` 后缀」同扩展名唯一候选）。该层
    /// Found 即返回；Ambiguous 只在本层产生并直接返回（不跨层吞掉）；NotFound
    /// 落到下一层，三层都未命中返回 [`TemplateLookup::NotFound`]。
    pub(crate) fn template(&self, android: &str, short: &str) -> TemplateLookup {
        // 层 1：本地编辑区（分区 templates/）
        if let Some(partition) = self.editable_partition(android) {
            match match_short_name(&partition.join("templates"), short) {
                ShortMatch::Found(path) => {
                    return TemplateLookup::Found(CompositeHit {
                        path,
                        source: CompositeSource::EditableLocal,
                    });
                }
                ShortMatch::Ambiguous { name, candidates } => {
                    return TemplateLookup::Ambiguous { name, candidates };
                }
                ShortMatch::NotFound => {}
            }
        }
        // 层 2：user-overrides
        if let Ok(android) = parse_android_package_name(android) {
            match match_short_name(&self.overrides_root(&android).join("templates"), short) {
                ShortMatch::Found(path) => {
                    return TemplateLookup::Found(CompositeHit {
                        path,
                        source: CompositeSource::UserOverride,
                    });
                }
                ShortMatch::Ambiguous { name, candidates } => {
                    return TemplateLookup::Ambiguous { name, candidates };
                }
                ShortMatch::NotFound => {}
            }
        }
        // 层 3：active App Package
        match self.active_package(android) {
            Some(active) => active.template(short),
            None => TemplateLookup::NotFound,
        }
    }

    /// 按键映射 composite 解析：**本地编辑区 → override → active 包**。
    pub(crate) fn keymap(&self, android: &str, name: &str) -> Option<CompositeHit> {
        if !Self::keymap_name_is_plain(name) {
            return None;
        }
        // 层 1：本地编辑区（分区 keymaps/）
        if let Some(partition) = self.editable_partition(android) {
            let path = partition.join("keymaps").join(name);
            if is_regular_file(&path).unwrap_or(false) {
                return Some(CompositeHit {
                    path,
                    source: CompositeSource::EditableLocal,
                });
            }
        }
        // 层 2：user-overrides
        if let Ok(android) = parse_android_package_name(android) {
            let path = self.overrides_root(&android).join("keymaps").join(name);
            if is_regular_file(&path).unwrap_or(false) {
                return Some(CompositeHit {
                    path,
                    source: CompositeSource::UserOverride,
                });
            }
        }
        // 层 3：active App Package
        let active = self.active_package(android)?;
        let path = active.keymap(name)?;
        Some(CompositeHit {
            path,
            source: CompositeSource::InstalledPackage {
                app_package: active.id,
                version: active.version,
            },
        })
    }

    /// 脚本 composite 源码：三层 map 合并，同 key 高优先层覆盖低优先层
    ///（合并顺序 = 包 → override → 本地编辑区）。key = 资源根内相对路径
    /// 含扩展名。
    pub(crate) fn script_sources(
        &self,
        android: &str,
    ) -> std::io::Result<BTreeMap<String, String>> {
        Ok(self
            .layered_yaml_sources(android, "scripts")?
            .into_iter()
            .map(|(key, (content, _))| (key, content))
            .collect())
    }

    /// [`CompositeResolver::script_sources`] 的来源标注版（诊断/测试断言
    /// 「这资源来自哪一层」）。
    pub(crate) fn script_sources_with_source(
        &self,
        android: &str,
    ) -> std::io::Result<BTreeMap<String, (String, CompositeSource)>> {
        self.layered_yaml_sources(android, "scripts")
    }

    /// 函数库 composite 源码：三层 map 合并（包 → override → 本地编辑区）。
    /// key = 资源根内相对路径含扩展名（运行快照按去扩展名短路径索引）。
    pub(crate) fn function_sources(
        &self,
        android: &str,
    ) -> std::io::Result<BTreeMap<String, String>> {
        Ok(self
            .layered_yaml_sources(android, "functions")?
            .into_iter()
            .map(|(key, (content, _))| (key, content))
            .collect())
    }

    /// [`CompositeResolver::function_sources`] 的来源标注版。
    pub(crate) fn function_sources_with_source(
        &self,
        android: &str,
    ) -> std::io::Result<BTreeMap<String, (String, CompositeSource)>> {
        self.layered_yaml_sources(android, "functions")
    }

    /// 三层 YAML 源码合并内核：低优先层先 insert、高优先层后覆盖。
    /// 分区目录不存在 = 该层为空，不报错。
    fn layered_yaml_sources(
        &self,
        android: &str,
        subdir: &str,
    ) -> std::io::Result<BTreeMap<String, (String, CompositeSource)>> {
        let mut merged = BTreeMap::new();
        // 层 3（最低）：active App Package
        if let Some(active) = self.active_package(android) {
            let source = CompositeSource::InstalledPackage {
                app_package: active.id.clone(),
                version: active.version.clone(),
            };
            for (key, content) in read_yaml_sources(&active.root.join(subdir))? {
                merged.insert(key, (content, source.clone()));
            }
        }
        // 层 2：user-overrides
        if let Ok(android) = parse_android_package_name(android) {
            for (key, content) in read_yaml_sources(&self.overrides_root(&android).join(subdir))? {
                merged.insert(key, (content, CompositeSource::UserOverride));
            }
        }
        // 层 1（最高）：本地编辑区
        if let Some(partition) = self.editable_partition(android) {
            for (key, content) in read_yaml_sources(&partition.join(subdir))? {
                merged.insert(key, (content, CompositeSource::EditableLocal));
            }
        }
        Ok(merged)
    }

    /// 按键映射文件名三层并集（本地编辑区 → override → 包；按文件名去重、
    /// 字典序）。高优先层的名字拼写优先保留。
    pub(crate) fn keymap_names(&self, android: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let push_layer = |dir: PathBuf, names: &mut Vec<String>| {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    if let Ok(name) = entry.file_name().into_string() {
                        let lower = name.to_ascii_lowercase();
                        if entry.path().is_file()
                            && (lower.ends_with(".yaml") || lower.ends_with(".yml"))
                            && !names
                                .iter()
                                .any(|existing| existing.eq_ignore_ascii_case(&name))
                        {
                            names.push(name);
                        }
                    }
                }
            }
        };
        if let Some(partition) = self.editable_partition(android) {
            push_layer(partition.join("keymaps"), &mut names);
        }
        if let Ok(android) = parse_android_package_name(android) {
            push_layer(self.overrides_root(&android).join("keymaps"), &mut names);
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
    let Some(name) = crate::resources::sanitize_template_name(short) else {
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
    fn template_lookup_prefers_editable_then_override_then_package() {
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

        // override 层命中（含来源断言）
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

        // 本地编辑区最高优先，且遮蔽 override/包
        let local_file = root.join("com.example.game/templates").join("icon.png");
        std::fs::create_dir_all(local_file.parent().unwrap()).unwrap();
        std::fs::write(&local_file, b"editable").unwrap();
        match resolver.template("com.example.game", "icon.png") {
            TemplateLookup::Found(hit) => {
                assert_eq!(hit.source, CompositeSource::EditableLocal);
                assert_eq!(std::fs::read(&hit.path).unwrap(), b"editable");
            }
            other => panic!("expected editable hit, got {other:?}"),
        }

        // 删本地编辑区 → 回落 override；再删 override → 回落包
        std::fs::remove_file(&local_file).unwrap();
        match resolver.template("com.example.game", "icon.png") {
            TemplateLookup::Found(hit) => {
                assert_eq!(hit.source, CompositeSource::UserOverride);
            }
            other => panic!("expected override fallback, got {other:?}"),
        }
        std::fs::remove_file(&override_file).unwrap();
        match resolver.template("com.example.game", "icon.png") {
            TemplateLookup::Found(hit) => assert!(matches!(
                hit.source,
                CompositeSource::InstalledPackage { .. }
            )),
            other => panic!("expected package fallback, got {other:?}"),
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

    /// 模板歧义只在本层内产生并直接返回，不静默落到低优先层。
    #[test]
    fn template_ambiguity_does_not_fall_through_to_lower_layers() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let package_root = root.join("app-packages/official.a/1.0.0");
        write_manifest(&package_root, "official.a", "1.0.0", "com.example.game");
        std::fs::create_dir_all(package_root.join("templates")).unwrap();
        std::fs::write(package_root.join("templates/icon#only.png"), b"package").unwrap();
        ActiveRegistry::from_iter([("official.a".to_string(), "1.0.0".to_string())])
            .save(root)
            .unwrap();

        // 本地编辑区同短名两个 # 后缀候选 → Ambiguous（尽管包层有唯一命中）
        let local_dir = root.join("com.example.game/templates");
        std::fs::create_dir_all(&local_dir).unwrap();
        std::fs::write(local_dir.join("icon#a.png"), b"one").unwrap();
        std::fs::write(local_dir.join("icon#b.png"), b"two").unwrap();

        let resolver = CompositeResolver::new(root);
        match resolver.template("com.example.game", "icon.png") {
            TemplateLookup::Ambiguous { name, candidates } => {
                assert_eq!(name, "icon.png");
                assert_eq!(candidates, vec!["icon#a.png", "icon#b.png"]);
            }
            other => panic!("expected ambiguity in editable layer, got {other:?}"),
        }
    }

    /// 三层 scripts/functions 合并：editable > override > 包；两类索引互不
    /// 混入；来源标注可断言。
    #[test]
    fn script_and_function_sources_layer_editable_over_override_over_package() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let package_root = root.join("app-packages/official.a/1.0.0");
        write_manifest(&package_root, "official.a", "1.0.0", "com.example.game");
        std::fs::create_dir_all(package_root.join("scripts")).unwrap();
        std::fs::write(package_root.join("scripts/dup.yaml"), b"package script").unwrap();
        std::fs::create_dir_all(package_root.join("functions")).unwrap();
        std::fs::write(
            package_root.join("functions/common.yaml"),
            b"noop:\n  steps: [] # package\n",
        )
        .unwrap();

        ActiveRegistry::from_iter([("official.a".to_string(), "1.0.0".to_string())])
            .save(root)
            .unwrap();
        let resolver = CompositeResolver::new(root);

        let override_scripts = root.join("user-overrides/com.example.game/scripts");
        std::fs::create_dir_all(&override_scripts).unwrap();
        std::fs::write(override_scripts.join("dup.yaml"), b"override script").unwrap();
        let override_functions = root.join("user-overrides/com.example.game/functions");
        std::fs::create_dir_all(&override_functions).unwrap();
        std::fs::write(
            override_functions.join("common.yaml"),
            b"noop:\n  steps: [] # override\n",
        )
        .unwrap();

        let local_scripts = root.join("com.example.game/scripts");
        std::fs::create_dir_all(&local_scripts).unwrap();
        std::fs::write(local_scripts.join("dup.yaml"), b"local script").unwrap();
        let local_functions = root.join("com.example.game/functions");
        std::fs::create_dir_all(&local_functions).unwrap();
        std::fs::write(
            local_functions.join("common.yaml"),
            b"noop:\n  steps: [] # local\n",
        )
        .unwrap();

        let scripts = resolver.script_sources("com.example.game").unwrap();
        assert_eq!(
            scripts.get("dup.yaml").map(String::as_str),
            Some("local script")
        );
        assert!(
            !scripts.contains_key("common.yaml"),
            "functions/ 内容不得混入脚本索引"
        );
        let functions = resolver.function_sources("com.example.game").unwrap();
        assert_eq!(
            functions.get("common.yaml").map(String::as_str),
            Some("noop:\n  steps: [] # local\n")
        );
        assert!(
            !functions.contains_key("dup.yaml"),
            "scripts/ 内容不得混入函数库索引"
        );

        // 删本地编辑区 → 回落 override
        std::fs::remove_file(local_scripts.join("dup.yaml")).unwrap();
        std::fs::remove_file(local_functions.join("common.yaml")).unwrap();
        assert_eq!(
            resolver
                .script_sources("com.example.game")
                .unwrap()
                .get("dup.yaml")
                .map(String::as_str),
            Some("override script")
        );
        assert_eq!(
            resolver
                .function_sources("com.example.game")
                .unwrap()
                .get("common.yaml")
                .map(String::as_str),
            Some("noop:\n  steps: [] # override\n")
        );

        // 再删 override → 回落包
        std::fs::remove_file(override_scripts.join("dup.yaml")).unwrap();
        std::fs::remove_file(override_functions.join("common.yaml")).unwrap();
        assert_eq!(
            resolver
                .script_sources("com.example.game")
                .unwrap()
                .get("dup.yaml")
                .map(String::as_str),
            Some("package script")
        );
        assert_eq!(
            resolver
                .function_sources("com.example.game")
                .unwrap()
                .get("common.yaml")
                .map(String::as_str),
            Some("noop:\n  steps: [] # package\n")
        );

        // 来源标注
        let tagged = resolver
            .script_sources_with_source("com.example.game")
            .unwrap();
        assert_eq!(
            tagged.get("dup.yaml").map(|(_, s)| s),
            Some(&CompositeSource::InstalledPackage {
                app_package: parse_app_package_id("official.a").unwrap(),
                version: InstalledVersion::parse("1.0.0").unwrap(),
            })
        );
        std::fs::write(local_scripts.join("dup.yaml"), b"local script").unwrap();
        let tagged = resolver
            .script_sources_with_source("com.example.game")
            .unwrap();
        assert_eq!(
            tagged.get("dup.yaml").map(|(_, s)| s),
            Some(&CompositeSource::EditableLocal)
        );
    }

    /// 按键映射三层：editable > override > 包；keymap_names 为三层并集。
    #[test]
    fn keymaps_layer_editable_over_override_over_package() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let package_root = root.join("app-packages/official.a/1.0.0");
        write_manifest(&package_root, "official.a", "1.0.0", "com.example.game");
        std::fs::create_dir_all(package_root.join("keymaps")).unwrap();
        std::fs::write(package_root.join("keymaps/dup.yaml"), b"package").unwrap();
        std::fs::write(package_root.join("keymaps/pkg-only.yaml"), b"package").unwrap();
        ActiveRegistry::from_iter([("official.a".to_string(), "1.0.0".to_string())])
            .save(root)
            .unwrap();

        let resolver = CompositeResolver::new(root);
        let override_dir = root.join("user-overrides/com.example.game/keymaps");
        std::fs::create_dir_all(&override_dir).unwrap();
        std::fs::write(override_dir.join("dup.yaml"), b"override").unwrap();

        let local_dir = root.join("com.example.game/keymaps");
        std::fs::create_dir_all(&local_dir).unwrap();
        std::fs::write(local_dir.join("dup.yaml"), b"local").unwrap();

        let hit = resolver.keymap("com.example.game", "dup.yaml").unwrap();
        assert_eq!(hit.source, CompositeSource::EditableLocal);
        assert_eq!(std::fs::read(&hit.path).unwrap(), b"local");

        // 删本地 → override；再删 override → 包
        std::fs::remove_file(local_dir.join("dup.yaml")).unwrap();
        assert_eq!(
            resolver
                .keymap("com.example.game", "dup.yaml")
                .unwrap()
                .source,
            CompositeSource::UserOverride
        );
        std::fs::remove_file(override_dir.join("dup.yaml")).unwrap();
        let hit = resolver.keymap("com.example.game", "dup.yaml").unwrap();
        assert!(matches!(
            hit.source,
            CompositeSource::InstalledPackage { .. }
        ));

        // names 并集：dup.yaml 各层同名只出现一次，独有名可见
        let names = resolver.keymap_names("com.example.game");
        assert_eq!(names, vec!["dup.yaml", "pkg-only.yaml"]);

        assert!(resolver
            .keymap("com.example.game", "../escape.yaml")
            .is_none());
    }
}
