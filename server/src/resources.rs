//! Core 通用资源存储（P11.3 / P11.6 / ADR-11）。
//!
//! ScriptStore / KeymapStore 消解后的内容无关资源层：六目录寻址
//! （`data/<pkg>/{scripts,functions,templates,keymaps,presets,resources}/`，
//! 目录即类型、跨目录不解析不回退）+ composite 三层解析
//! （EditableLocal → UserOverride → InstalledPackage，复用
//! `app_packages::CompositeResolver`）+ 内容版本短码 + 原子写。
//!
//! 本层只懂「目录类别 + 字节/文本 + 内容版本短码 + 原子写」；**内容语义
//! （YAML 解析/校验、模板引用重写、方案元数据注记）经
//! [`ResourceKindHandler`] 注册表回调给扩展**（参照 B2 TimerRunnerRegistrar
//! 的注入模式，gamer_yaml / gamer.keymap 在组合根注册）。未注册 handler 的
//! kind 保存不做内容校验——裸 Core 也可启动并存取资源字节（§8.9 验收）。
//!
//! id 形态与旧存储一致：`<pkg>/<相对路径>`（含 `/`，HTTP 层必须整体
//! encodeURIComponent）。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serializer;
use serde_json::Value;

use crate::config::Config;
use crate::core::fs::{
    atomic_write, content_version, is_windows_reserved_name, safe_name as sanitize_part,
};

/// 六个资源目录类别（plan §11.2：Core 知道分类，不懂内容语义）。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ResourceKind {
    Scripts,
    Functions,
    Templates,
    Keymaps,
    Presets,
    Resources,
}

pub use ResourceKind::*;

impl ResourceKind {
    pub const ALL: [ResourceKind; 6] = [Scripts, Functions, Templates, Keymaps, Presets, Resources];

    pub fn as_str(self) -> &'static str {
        match self {
            Scripts => "scripts",
            Functions => "functions",
            Templates => "templates",
            Keymaps => "keymaps",
            Presets => "presets",
            Resources => "resources",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == value)
    }

    /// 文本 kind（YAML 等 UTF-8 资源）；Templates / Resources 为字节 kind。
    pub fn is_text(self) -> bool {
        matches!(self, Scripts | Functions | Keymaps | Presets)
    }

    fn rule(self) -> KindRule {
        match self {
            Scripts => KindRule {
                exts: &["yaml", "yml"],
                allow_nested: true,
                list_via_composite: false,
                get_via_composite: false,
                same_base_conflict: false,
            },
            Functions => KindRule {
                exts: &["yaml"],
                // P12.5：放开嵌套目录（T2 遗留）——`function:common/lib/fn`
                // 形态要求 functions/ 支持子目录文件（`<文件短路径>` 含 `/`）。
                allow_nested: true,
                list_via_composite: false,
                get_via_composite: false,
                same_base_conflict: false,
            },
            Keymaps => KindRule {
                exts: &["yaml", "yml"],
                allow_nested: false,
                list_via_composite: true,
                get_via_composite: true,
                same_base_conflict: false,
            },
            Presets => KindRule {
                exts: &["yaml", "yml"],
                allow_nested: false,
                list_via_composite: false,
                get_via_composite: false,
                same_base_conflict: false,
            },
            Templates | Resources => KindRule {
                exts: &[],
                allow_nested: false,
                list_via_composite: false,
                get_via_composite: false,
                same_base_conflict: self == Templates,
            },
        }
    }
}

struct KindRule {
    /// 文本 kind 允许的扩展名（小写，不含点）；字节 kind 为空。
    exts: &'static [&'static str],
    allow_nested: bool,
    /// 列表是否合并 override / 包层（keymaps 语义：下层方案可见）。
    list_via_composite: bool,
    /// 读取是否经 composite 三层（keymaps：本地无副本时读 override/包层）。
    get_via_composite: bool,
    /// 创建时同基名冲突检查（templates §11.7：短名引用靠基名 + `#` 后缀唯一
    /// 候选消歧，放行第二个同基名文件会制造歧义）。
    same_base_conflict: bool,
}

/// 相对短路径分段校验（资源路径 resolver 共用）：拒绝空串、反斜杠、空段、
/// `.`、`..`、前导点与非法字符（逐段过 [`sanitize_part`]，含 Windows 保留名）；
/// 绝对路径（`/x`、`C:x`）被空段/非法字符规则覆盖。
pub fn sanitize_rel_segments(rel: &str) -> anyhow::Result<Vec<String>> {
    if rel.contains('\\') {
        anyhow::bail!("路径不允许反斜杠: {rel:?}");
    }
    if rel.is_empty() {
        anyhow::bail!("路径不能为空");
    }
    rel.split('/')
        .map(|seg| {
            sanitize_part(seg).ok_or_else(|| {
                anyhow::anyhow!("路径段非法: {seg:?}（拒绝空段 / . / .. / 非法字符）")
            })
        })
        .collect()
}

/// 校验模板文件名：允许 unicode 字母数字与 `. - _ #`（模板名可带 # 区域后缀
/// 与 #1 颜色标记）、空格；拒绝空串、前导/尾随点与 Windows 保留名。
pub fn sanitize_template_name(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty()
        || t == "."
        || t == ".."
        || t.starts_with('.')
        || t.ends_with('.')
        || is_windows_reserved_name(t)
    {
        return None;
    }
    if t.chars()
        .any(|c| !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '#' | ' ')))
    {
        return None;
    }
    Some(t.to_string())
}

// ---------------------------------------------------------------------------
// 保存期内容校验 / 注记 / 重命名钩子（扩展注册；Core 不懂内容语义）
// ---------------------------------------------------------------------------

/// 保存期内容校验请求。`store` 供实现方构建「当前分区视图 + 待写覆盖」
/// （call/func 引用解析与运行时同源）。
pub struct SaveValidation<'a> {
    pub app: &'a str,
    pub kind: ResourceKind,
    /// 目标资源相对路径（含扩展名，相对 kind 目录）。
    pub id: &'a str,
    pub content: &'a str,
    pub store: &'a ResourceStore,
}

/// 单 kind 资源的内容钩子。扩展在组合根注册（gamer_yaml → scripts/functions/
/// templates；gamer.keymap → keymaps）；未注册 = 该 kind 无内容校验/注记。
pub trait ResourceKindHandler: Send + Sync {
    /// 保存前内容校验；Err = 结构化诊断 JSON（HTTP 400 透传，格式由扩展定）。
    fn validate_save(&self, _req: SaveValidation<'_>) -> Result<(), Value> {
        Ok(())
    }

    /// 列表/读取注记：entries = (id, content)；返回 id → 顶层附加字段
    /// （如函数名清单、方案显示名/binding 数）。Core 只做透明合并。
    fn annotate(&self, _entries: &[(String, String)]) -> serde_json::Map<String, Value> {
        Default::default()
    }

    /// 字节 kind 重命名前钩子（templates：同步改写分区脚本/函数中的模板
    /// 引用；实现方保证失败时不动任何文件）。
    fn before_rename(
        &self,
        _store: &ResourceStore,
        _app: &str,
        _old: &str,
        _new: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// 包构建/提取 preflight 的「staged 集合」校验（跨文件 call/func 引用视图以
/// staged 内容自身为最高优先）。扩展（gamer_yaml）在组合根注册；未注册时
/// staged YAML 不做内容校验。
pub trait StagedResourceValidator: Send + Sync {
    /// entries = (kind, kind 内相对路径, 文本内容)；返回问题行列表。
    fn validate_staged(
        &self,
        store: &ResourceStore,
        app: &str,
        entries: &[(ResourceKind, String, String)],
    ) -> Vec<String>;
}

/// 一个资源条目（文本 kind）。手写 Serialize：基础字段在前、注记字段
/// （meta）随后覆盖同名字段（如 keymap 显示名 `name`）。
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    /// `<pkg>/<rel>`（rel 含扩展名）。
    pub id: String,
    pub package: String,
    /// rel 路径（含扩展名；文件短路径去扩展名见 [`ResourceEntry::file`]）。
    pub name: String,
    pub content: String,
    pub updated_at: String,
    /// 字节 kind 的文件大小（文本 kind 为 content 字节数）。
    pub size: u64,
    /// 文件修改时间（unix 秒；模板列表排序沿用）。
    pub mtime: u64,
    /// 注记字段（handler.annotate；序列化时展开进顶层）。
    pub meta: serde_json::Map<String, Value>,
}

impl ResourceEntry {
    /// 内容版本短码（内容哈希）——GET 返回、保存 expected_version 冲突检测依据。
    pub fn version(&self) -> String {
        content_version(&self.content)
    }
}

impl serde::Serialize for ResourceEntry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let file_short = self.name.strip_suffix(".yaml").unwrap_or(&self.name);
        let count = 10 + self.meta.len();
        let mut ser = serializer.serialize_map(Some(count))?;
        ser.serialize_entry("id", &self.id)?;
        ser.serialize_entry("package", &self.package)?;
        ser.serialize_entry("pkg", &self.package)?;
        ser.serialize_entry("name", &self.name)?;
        ser.serialize_entry("file", file_short)?;
        ser.serialize_entry("content", &self.content)?;
        ser.serialize_entry("version", &self.version())?;
        ser.serialize_entry("updated_at", &self.updated_at)?;
        ser.serialize_entry("size", &self.size)?;
        ser.serialize_entry("mtime", &self.mtime)?;
        for (key, value) in &self.meta {
            ser.serialize_entry(key, value)?;
        }
        ser.end()
    }
}

/// 字节资源条目（templates / resources kind 的列表项；读取返回原始字节）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct BinaryEntry {
    pub id: String,
    pub package: String,
    pub pkg: String,
    pub name: String,
    pub size: u64,
    pub mtime: u64,
    pub updated_at: String,
}

fn fmt_mtime(p: &std::path::Path) -> String {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .map(|t| {
            let dt: chrono::DateTime<chrono::Local> = t.into();
            dt.format("%Y-%m-%d %H:%M:%S").to_string()
        })
        .unwrap_or_default()
}

fn mtime_secs(p: &std::path::Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 结构化诊断 JSON 的通用展示格式（`code: message (step_path)` 分号连接；
/// 非对象条目原样字符串化）。包构建/提取 preflight 与日志侧共用。
pub fn format_diagnostics_value(value: &Value) -> String {
    match value {
        Value::Array(items) => items
            .iter()
            .map(format_diagnostics_value)
            .collect::<Vec<_>>()
            .join("; "),
        Value::Object(map) => {
            let code = map.get("code").and_then(|v| v.as_str()).unwrap_or("");
            let message = map
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| map.get("error").and_then(|v| v.as_str()).unwrap_or(""));
            let step_path = map
                .get("step_path")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if code.is_empty() {
                message.to_string()
            } else {
                format!("{code}: {message} ({step_path})")
            }
        }
        other => other.to_string(),
    }
}

/// Core 通用资源存储。见模块级文档。
pub struct ResourceStore {
    /// 数据根目录（data/），一级子目录 = 应用分区（内含六资源目录）
    root: PathBuf,
    /// Composite 资源解析缝（EditableLocal → user-overrides → active App
    /// Package）。模板解析与 keymaps 读取经此寻址；本地编辑区即 `root` 下
    /// 的分区目录。
    composite: crate::app_packages::CompositeResolver,
    handlers: std::sync::RwLock<BTreeMap<ResourceKind, Arc<dyn ResourceKindHandler>>>,
    staged: std::sync::RwLock<Option<Arc<dyn StagedResourceValidator>>>,
}

impl ResourceStore {
    pub fn open(cfg: &Config) -> anyhow::Result<Self> {
        let store = Self {
            root: cfg.data_dir.clone(),
            composite: crate::app_packages::CompositeResolver::new(cfg.data_dir.clone()),
            handlers: std::sync::RwLock::new(BTreeMap::new()),
            staged: std::sync::RwLock::new(None),
        };
        store.reject_legacy_layout()?;
        Ok(store)
    }

    /// data **根级**的 scripts/ 与 templates/ 目录属于更早的单层布局（分区机制
    /// 引入之前），与分区内的同名子目录（data/<pkg>/scripts/）无关。启动时只
    /// 报错并要求重建/清理开发数据，绝不自动移动或改写其中的文件。
    fn reject_legacy_layout(&self) -> anyhow::Result<()> {
        let legacy = [self.root.join("scripts"), self.root.join("templates")];
        let found: Vec<String> = legacy
            .iter()
            .filter(|path| path.exists())
            .map(|path| path.display().to_string())
            .collect();
        if found.is_empty() {
            Ok(())
        } else {
            anyhow::bail!(
                "检测到已废弃的数据根级目录布局：{}（旧单层布局，不是分区内目录）；请备份后删除旧目录并重建开发数据",
                found.join(", ")
            )
        }
    }

    /// composite 三层解析器（模板短名消歧 / keymaps 跨层；生产链路内部直走
    /// 字段，公开访问器当前仅测试探针消费）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn composite(&self) -> &crate::app_packages::CompositeResolver {
        &self.composite
    }

    /// 注册 kind 的内容钩子（组合根引导期调用；同 kind 重复注册 = 替换）。
    pub fn register_handler(&self, kind: ResourceKind, handler: Arc<dyn ResourceKindHandler>) {
        self.handlers
            .write()
            .expect("resource handler registry poisoned")
            .insert(kind, handler);
    }

    pub fn set_staged_validator(&self, validator: Arc<dyn StagedResourceValidator>) {
        *self.staged.write().expect("staged validator slot poisoned") = Some(validator);
    }

    /// 保存前内容校验：分发到已注册 handler；未注册 = 通过（裸 Core 语义）。
    pub fn validate_save(&self, req: SaveValidation<'_>) -> Result<(), Value> {
        let handler = self
            .handlers
            .read()
            .expect("resource handler registry poisoned")
            .get(&req.kind)
            .cloned();
        match handler {
            Some(handler) => handler.validate_save(req),
            None => Ok(()),
        }
    }

    /// 单条资源内容校验（经 kind handler；未注册 = 通过）。包构建/提取
    /// preflight 对无跨文件引用语义的 kind（keymaps 等）逐条回调。
    pub fn validate_content(
        &self,
        kind: ResourceKind,
        app: &str,
        id: &str,
        content: &str,
    ) -> Result<(), Value> {
        let handler = self.handler(kind);
        match handler {
            Some(handler) => handler.validate_save(SaveValidation {
                app,
                kind,
                id,
                content,
                store: self,
            }),
            None => Ok(()),
        }
    }

    /// staged 集合校验（包导出/提取 preflight）；未注册 = 无问题。
    pub fn validate_staged(
        &self,
        app: &str,
        entries: &[(ResourceKind, String, String)],
    ) -> Vec<String> {
        let validator = self
            .staged
            .read()
            .expect("staged validator slot poisoned")
            .clone();
        match validator {
            Some(validator) => validator.validate_staged(self, app, entries),
            None => Vec::new(),
        }
    }

    fn handler(&self, kind: ResourceKind) -> Option<Arc<dyn ResourceKindHandler>> {
        self.handlers
            .read()
            .expect("resource handler registry poisoned")
            .get(&kind)
            .cloned()
    }

    // ---------- 目录与分区 ----------

    /// kind 目录（分区 `<pkg>/` 下的六目录之一）。非法分区名映射到不可枚举
    /// 的哨兵目录，避免任何调用方意外逃出 root。
    pub fn kind_dir(&self, pkg: &str, kind: ResourceKind) -> PathBuf {
        self.partition_dir(pkg).join(kind.as_str())
    }

    fn partition_dir(&self, pkg: &str) -> PathBuf {
        sanitize_part(pkg)
            .map(|pkg| self.root.join(pkg))
            .unwrap_or_else(|| self.root.join(".gamer-invalid-partition"))
    }

    /// 磁盘上全部分区名（存在六资源目录之一的一级目录，字典序）。不把
    /// package.toml 之类标志文件计入：以资源子目录为准可避免杂散文件在
    /// 分区列表里制造幻影分区。
    pub fn partitions(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.root) {
            for d in rd.flatten() {
                let p = d.path();
                let Some(name) = sanitize_part(&d.file_name().to_string_lossy()) else {
                    continue;
                };
                if p.is_dir()
                    && ResourceKind::ALL
                        .iter()
                        .any(|kind| p.join(kind.as_str()).is_dir())
                {
                    out.push(name);
                }
            }
        }
        out.sort();
        out
    }

    /// 分区六目录都空时删掉分区目录（避免残留空目录被当成有效分区）。
    pub fn cleanup_partition(&self, pkg: &str) {
        for kind in ResourceKind::ALL {
            let _ = std::fs::remove_dir(self.kind_dir(pkg, kind)); // 非空时失败，忽略
        }
        let _ = std::fs::remove_dir(self.partition_dir(pkg));
    }

    // ---------- 路径解析（目录即类型，互不回退、不做内容推断） ----------

    /// 文本/字节资源相对路径 → 分区 kind 目录内的磁盘路径。拒绝扩展名错配、
    /// 越层嵌套（kind 规则）与非法分段；不回退。
    pub fn resolve_path(
        &self,
        pkg: &str,
        kind: ResourceKind,
        rel: &str,
    ) -> anyhow::Result<PathBuf> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let segs = sanitize_rel_segments(rel)?;
        let rule = kind.rule();
        if !rule.allow_nested && segs.len() > 1 {
            anyhow::bail!("{} 资源不支持子目录路径: {rel}", kind.as_str());
        }
        if !rule.exts.is_empty() {
            let last = segs.last().expect("分段结果非空");
            let low = last.to_lowercase();
            if !low.contains('.') || !rule.exts.iter().any(|e| low.ends_with(&format!(".{e}"))) {
                anyhow::bail!(
                    "{} 资源必须是 .{} 且位于分区 {} 目录: {rel}",
                    kind.as_str(),
                    rule.exts.join("/."),
                    kind.as_str()
                );
            }
        }
        let mut p = self.kind_dir(&package, kind);
        for s in &segs {
            p.push(s);
        }
        Ok(p)
    }

    /// 模板短名/完整名 → **现存**文件路径。composite 三层统一顺序：本地编辑区
    /// → user override → active App Package，逐层解析。精确名优先；否则按
    /// 「基名 + `#` 后缀 + 同扩展名」唯一匹配；零候选/多候选均报错。
    pub fn resolve_template_path(&self, pkg: &str, short: &str) -> anyhow::Result<PathBuf> {
        match self.composite.template(pkg, short) {
            crate::app_packages::TemplateLookup::Found(hit) => Ok(hit.path),
            crate::app_packages::TemplateLookup::Ambiguous { name, candidates } => anyhow::bail!(
                "模板 {name} 匹配到多个候选：{}，请用完整文件名指定",
                candidates.join("、")
            ),
            crate::app_packages::TemplateLookup::NotFound => anyhow::bail!(
                "模板 {short} 不存在 (path={})",
                self.kind_dir(pkg, Templates).display()
            ),
        }
    }

    // ---------- 文本 kind：CRUD ----------

    fn load_text_at(
        &self,
        pkg: &str,
        _kind: ResourceKind,
        rel: &str,
        path: &std::path::Path,
    ) -> Option<ResourceEntry> {
        let content = std::fs::read_to_string(path).ok()?;
        Some(ResourceEntry {
            id: format!("{pkg}/{rel}"),
            package: pkg.to_string(),
            name: rel.to_string(),
            updated_at: fmt_mtime(path),
            size: content.len() as u64,
            mtime: mtime_secs(path),
            content,
            meta: serde_json::Map::new(),
        })
    }

    /// 列出**一个分区**的文本资源（本地编辑区；keymaps 另合并 override/包层，
    /// 同名以本地优先）。返回按 updated_at 倒序。
    pub fn list_text(&self, pkg: &str, kind: ResourceKind) -> anyhow::Result<Vec<ResourceEntry>> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let rule = kind.rule();
        let mut out: Vec<ResourceEntry> = Vec::new();
        let dir = self.kind_dir(&package, kind);
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for f in rd.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                let low = name.to_lowercase();
                if !f.path().is_file()
                    || !rule
                        .exts
                        .iter()
                        .any(|ext| low.ends_with(&format!(".{ext}")))
                {
                    continue;
                }
                if let Some(entry) = self.load_text_at(&package, kind, &name, &f.path()) {
                    out.push(entry);
                }
            }
        }
        if rule.list_via_composite {
            // 追加 override/包内置方案：分区（本地编辑区）已列出的文件名不重复
            // 展示——composite.keymap_names 为三层并集，本地副本优先可编辑，
            // 下层方案的元数据以其文件为准。
            let listed: std::collections::HashSet<String> = out
                .iter()
                .map(|entry| entry.name.to_ascii_lowercase())
                .collect();
            for name in self.composite.keymap_names(&package) {
                if listed.contains(&name.to_ascii_lowercase()) {
                    continue;
                }
                let Some(path) = self.composite.keymap(&package, &name).map(|hit| hit.path) else {
                    continue;
                };
                if let Some(entry) = self.load_text_at(&package, kind, &name, &path) {
                    out.push(entry);
                }
            }
        }
        out.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    /// 读取单个文本资源（id = `<pkg>/<rel>`）。keymaps 经 composite 三层；
    /// 其余 kind 只读本地编辑区。非法 id / 不存在 → None。
    pub fn get_text(&self, kind: ResourceKind, id: &str) -> anyhow::Result<Option<ResourceEntry>> {
        let Some((pkg, rel)) = id.split_once('/') else {
            return Ok(None);
        };
        if kind.rule().get_via_composite {
            let name = rel.trim();
            match self.composite.keymap(pkg, name) {
                Some(hit) => {
                    // composite 命中下层时磁盘文件名可能大小写不同，用请求名
                    return Ok(self.load_text_at(pkg, kind, name, &hit.path));
                }
                None => return Ok(None),
            }
        }
        // 非法路径（穿越/扩展名错配等）与文件不存在同样返回 None
        let Ok(path) = self.resolve_path(pkg, kind, rel) else {
            return Ok(None);
        };
        if !path.is_file() {
            return Ok(None);
        }
        Ok(self.load_text_at(pkg, kind, rel, &path))
    }

    /// 保存前冲突检测：目标资源当前内容版本（不存在 → None）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn text_version(
        &self,
        kind: ResourceKind,
        pkg: &str,
        name: &str,
    ) -> anyhow::Result<Option<String>> {
        let rel = normalize_rel_name(kind, name)?;
        Ok(self
            .get_text(kind, &format!("{pkg}/{rel}"))?
            .map(|entry| entry.version()))
    }

    /// 保存文本资源到指定分区。`old_id` 存在 = 更新/同分区重命名（源必须
    /// 存在、目标不得与他文件冲突）；None = 创建（目标已存在 → 报错）。
    /// 不做内容校验——调用方先过 [`ResourceStore::validate_save`]。
    pub fn save_text(
        &self,
        kind: ResourceKind,
        old_id: Option<&str>,
        pkg: &str,
        name: &str,
        content: &str,
    ) -> anyhow::Result<ResourceEntry> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {}", pkg))?;
        let name = normalize_rel_name(kind, name)?;
        let dir = self.kind_dir(&package, kind);
        let path = dir.join(&name);
        if let Some(old_id) = old_id {
            let Some((old_pkg, old_rel)) = old_id.split_once('/') else {
                anyhow::bail!("非法资源 id: {old_id}");
            };
            let old_rel = normalize_rel_name(kind, old_rel)?;
            if old_pkg != package {
                anyhow::bail!("资源更新不得跨分区移动: {old_id:?} -> {package}/{name}");
            }
            let old_path = self.resolve_path(old_pkg, kind, &old_rel)?;
            if !old_path.is_file() {
                anyhow::bail!("资源不存在: {old_id}");
            }
            if old_path != path && path.exists() {
                anyhow::bail!("资源已存在: {}/{}", package, name);
            }
        } else if path.exists() {
            anyhow::bail!("资源已存在: {}/{}", package, name);
        }
        std::fs::create_dir_all(&dir)?;
        atomic_write(&path, content.as_bytes())?;
        if let Some(old_id) = old_id {
            let (old_pkg, old_rel) = old_id.split_once('/').expect("上面已校验");
            let old_rel = normalize_rel_name(kind, old_rel)?;
            let new_id = format!("{package}/{name}");
            let old_full = format!("{old_pkg}/{old_rel}");
            if old_full != new_id {
                let old_path = self.resolve_path(old_pkg, kind, &old_rel)?;
                if old_path != path && old_path.is_file() {
                    if let Err(err) = std::fs::remove_file(&old_path) {
                        let _ = std::fs::remove_file(&path);
                        return Err(err.into());
                    }
                    self.cleanup_partition(old_pkg);
                }
            }
        }
        Ok(ResourceEntry {
            id: format!("{package}/{name}"),
            package,
            name,
            updated_at: fmt_mtime(&path),
            size: content.len() as u64,
            mtime: mtime_secs(&path),
            content: content.to_string(),
            meta: serde_json::Map::new(),
        })
    }

    /// 直接覆盖写分区文本资源（不经校验/版本门禁；扩展内部引用重写等受控
    /// 场景使用——调用方负责回滚）。
    pub fn write_text_direct(
        &self,
        kind: ResourceKind,
        pkg: &str,
        rel: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        let path = self.resolve_path(pkg, kind, rel)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        atomic_write(&path, content.as_bytes())
    }

    /// 删除文本资源（id = `<pkg>/<rel>`；不存在 → 报错）。
    pub fn delete_text(&self, kind: ResourceKind, id: &str) -> anyhow::Result<()> {
        let Some((pkg, rel)) = id.split_once('/') else {
            anyhow::bail!("非法资源 id: {id}");
        };
        let path = self
            .resolve_path(pkg, kind, rel)
            .map_err(|_| anyhow::anyhow!("非法资源 id: {id}"))?;
        std::fs::remove_file(&path)
            .map_err(|e| anyhow::anyhow!("删除失败: {} ({})", e, path.display()))?;
        self.cleanup_partition(pkg);
        Ok(())
    }

    // ---------- 字节 kind（templates / resources）：CRUD ----------

    /// 列出一个分区的字节资源（本地编辑区；非隐藏文件）。
    pub fn list_binary(&self, pkg: &str, kind: ResourceKind) -> anyhow::Result<Vec<BinaryEntry>> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let mut out = Vec::new();
        let dir = self.kind_dir(&package, kind);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if e.path().is_file() && !name.starts_with('.') {
                    let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                    out.push(BinaryEntry {
                        id: format!("{package}/{name}"),
                        package: package.clone(),
                        pkg: package.clone(),
                        mtime: mtime_secs(&e.path()),
                        updated_at: fmt_mtime(&e.path()),
                        size,
                        name,
                    });
                }
            }
        }
        out.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    /// 读取字节资源（本地编辑区；不存在 → None）。
    pub fn get_binary(&self, kind: ResourceKind, id: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let Some((pkg, rel)) = id.split_once('/') else {
            return Ok(None);
        };
        let Ok(path) = self.resolve_path(pkg, kind, rel) else {
            return Ok(None);
        };
        if !path.is_file() {
            return Ok(None);
        }
        Ok(std::fs::read(&path).ok())
    }

    /// 创建字节资源。templates kind 施加同基名冲突检查（§11.7）。
    pub fn create_binary(
        &self,
        kind: ResourceKind,
        pkg: &str,
        name: &str,
        bytes: &[u8],
    ) -> anyhow::Result<PathBuf> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let name = normalize_binary_name(kind, name)?;
        let dir = self.kind_dir(&package, kind);
        let path = dir.join(&name);
        if kind.rule().same_base_conflict && same_base_conflict(&dir, &name) {
            anyhow::bail!("同名资源已存在（基名冲突，不会覆盖）: {}/{}", package, name);
        }
        if path.exists() {
            anyhow::bail!("资源已存在: {}/{}", package, name);
        }
        std::fs::create_dir_all(&dir)?;
        atomic_write(&path, bytes)?;
        Ok(path)
    }

    /// 覆盖已有字节资源（不存在 → 报错）；返回磁盘路径。
    pub fn replace_binary(
        &self,
        kind: ResourceKind,
        pkg: &str,
        name: &str,
        bytes: &[u8],
    ) -> anyhow::Result<PathBuf> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let name = normalize_binary_name(kind, name)?;
        let dir = self.kind_dir(&package, kind);
        let path = dir.join(&name);
        if !path.is_file() {
            anyhow::bail!("资源不存在: {}/{}", package, name);
        }
        atomic_write(&path, bytes)?;
        Ok(path)
    }

    /// 字节资源重命名（同分区内）。templates kind 先经 before_rename 钩子
    /// （模板引用重写），钩子失败则不动文件。
    pub fn rename_binary(
        &self,
        kind: ResourceKind,
        pkg: &str,
        old_name: &str,
        new_name: &str,
    ) -> anyhow::Result<()> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let old_name = normalize_binary_name(kind, old_name)?;
        let new_name = normalize_binary_name(kind, new_name)?;
        if old_name == new_name {
            anyhow::bail!("名称未变化");
        }
        let dir = self.kind_dir(&package, kind);
        let old_path = dir.join(&old_name);
        let new_path = dir.join(&new_name);
        if !old_path.is_file() {
            anyhow::bail!("资源不存在: {package}/{old_name}");
        }
        if new_path.exists() {
            anyhow::bail!("资源已存在: {package}/{new_name}");
        }
        if let Some(handler) = self.handler(kind) {
            handler.before_rename(self, &package, &old_name, &new_name)?;
        }
        std::fs::rename(&old_path, &new_path).map_err(|e| anyhow::anyhow!("重命名失败: {e}"))?;
        self.cleanup_partition(&package);
        Ok(())
    }

    /// 删除字节资源（不存在 → 报错）；返回被删的磁盘路径（缓存失效用）。
    pub fn delete_binary(
        &self,
        kind: ResourceKind,
        pkg: &str,
        name: &str,
    ) -> anyhow::Result<PathBuf> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let name = normalize_binary_name(kind, name)?;
        let path = self.kind_dir(&package, kind).join(&name);
        std::fs::remove_file(&path)
            .map_err(|e| anyhow::anyhow!("删除失败: {} ({})", e, path.display()))?;
        self.cleanup_partition(&package);
        Ok(path)
    }

    // ---------- 注记 ----------

    /// 列表注记合并：entries 内容按 id → meta 透明并入条目顶层。
    pub fn annotate(&self, kind: ResourceKind, app: &str, entries: &mut [ResourceEntry]) {
        let Some(handler) = self.handler(kind) else {
            return;
        };
        let pairs: Vec<(String, String)> = entries
            .iter()
            .map(|entry| (entry.name.clone(), entry.content.clone()))
            .collect();
        let meta = handler.annotate(&pairs);
        for entry in entries.iter_mut() {
            if let Some(value) = meta.get(&entry.name) {
                entry.meta = value.as_object().cloned().unwrap_or_default();
            }
        }
        let _ = app;
    }
}

/// 规范化文本资源名：trim + 分段校验 + 缺扩展名补默认扩展名（save 与版本
/// 冲突检测共用）。函数库（functions）严格 .yaml、P12.5 起允许嵌套目录。
pub fn normalize_rel_name(kind: ResourceKind, name_raw: &str) -> anyhow::Result<String> {
    let t = name_raw.trim();
    let rule = kind.rule();
    let segs = sanitize_rel_segments(t)?;
    if !rule.allow_nested && segs.len() > 1 {
        anyhow::bail!("{} 资源不支持子目录路径: {name_raw}", kind.as_str());
    }
    let mut last = segs.last().expect("分段结果非空").clone();
    let low = last.to_lowercase();
    let has_ext = rule
        .exts
        .iter()
        .any(|ext| low.ends_with(&format!(".{ext}")));
    if !has_ext {
        let default_ext = rule.exts.first().copied().unwrap_or("yaml");
        last = format!("{last}.{default_ext}");
    }
    if rule.allow_nested {
        let mut segs = segs;
        *segs.last_mut().expect("非空") = last;
        Ok(segs.join("/"))
    } else {
        Ok(last)
    }
}

/// 字节资源名规范化：trim + 单文件名校验。模板名合法字符集含 `#`（区域/
/// 颜色后缀），不能用 [`sanitize_rel_segments`]（其 safe_name 拒绝 `#`），
/// 走 [`sanitize_template_name`]；其余字节 kind 保持 safe_name 口径。
fn normalize_binary_name(kind: ResourceKind, name_raw: &str) -> anyhow::Result<String> {
    let t = name_raw.trim();
    if t.is_empty() || t.contains('/') || t.contains('\\') {
        anyhow::bail!("{} 资源名非法（单文件名）: {name_raw}", kind.as_str());
    }
    if kind == Templates {
        sanitize_template_name(t)
            .map(|_| t.to_string())
            .ok_or_else(|| anyhow::anyhow!("模板名非法: {name_raw}"))
    } else {
        sanitize_part(t)
            .map(|_| t.to_string())
            .ok_or_else(|| anyhow::anyhow!("{} 资源名非法: {name_raw}", kind.as_str()))
    }
}

/// templates §11.7：分区内存在同基名文件（任意扩展名，含 `#` 后缀变体，
/// 大小写不敏感对齐 Windows FS）即冲突。
fn same_base_conflict(dir: &std::path::Path, name: &str) -> bool {
    let Some((stem, _)) = name.rsplit_once('.') else {
        return dir.join(name).exists();
    };
    // 基名 = 去区域后缀（短名引用按「基名 + # 后缀唯一候选」消歧，第二个
    // 同基名文件——无论区域后缀如何——都会制造歧义）
    let base = stem.split('#').next().unwrap_or(stem).to_ascii_lowercase();
    let prefix = format!("{base}#");
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .any(|n| match n.rsplit_once('.') {
            Some((stem, _)) => {
                let stem = stem.to_ascii_lowercase();
                stem == base || stem.starts_with(&prefix)
            }
            None => false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn temp_store(tag: &str) -> (ResourceStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "gamer-restest-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        (ResourceStore::open(&cfg).unwrap(), dir)
    }

    // ---------- 目录即类型 + 路径安全（原 scripts.rs resolver 契约） ----------

    #[test]
    fn resolve_path_accepts_only_kind_files_under_partition() {
        let (store, dir) = temp_store("resolve");
        let p = store
            .resolve_path("com.test.app", Scripts, "main.yaml")
            .unwrap();
        assert_eq!(
            p,
            dir.join("com.test.app").join("scripts").join("main.yaml")
        );
        // 嵌套短路径（scripts 允许子目录）
        let p = store
            .resolve_path("com.test.app", Scripts, "sub/inner.yaml")
            .unwrap();
        assert_eq!(
            p,
            dir.join("com.test.app")
                .join("scripts")
                .join("sub")
                .join("inner.yaml")
        );
        // functions 严格 .yaml；P12.5 起允许嵌套目录（function:<文件短路径>/<函数名>）
        assert!(store
            .resolve_path("com.test.app", Functions, "a.yml")
            .is_err());
        let p = store
            .resolve_path("com.test.app", Functions, "sub/a.yaml")
            .unwrap();
        assert_eq!(
            p,
            dir.join("com.test.app")
                .join("functions")
                .join("sub")
                .join("a.yaml")
        );
        // 跨目录不解析、不回退
        std::fs::create_dir_all(dir.join("com.test.app/functions")).unwrap();
        std::fs::write(dir.join("com.test.app/functions/common.yaml"), b"x").unwrap();
        assert!(!store
            .resolve_path("com.test.app", Scripts, "common.yaml")
            .unwrap()
            .is_file());
    }

    #[test]
    fn resolve_path_rejects_traversal_and_bad_segments() {
        let (store, _dir) = temp_store("traversal");
        let bad = [
            "",
            "/abs.yaml",
            "..",
            "../escape.yaml",
            "a/../../b.yaml",
            "a//b.yaml",
            ".hidden.yaml",
            "a\\b.yaml",
            "main.png",
            "C:/x.yaml",
        ];
        for rel in bad {
            assert!(
                store.resolve_path("com.test.app", Scripts, rel).is_err(),
                "{rel:?} 必须被拒绝"
            );
        }
        assert!(store.resolve_path("../escape", Scripts, "a.yaml").is_err());
    }

    // ---------- 文本资源 CRUD + 乐观并发版本 ----------

    #[test]
    fn text_crud_roundtrip_with_version_and_rename() {
        let (store, dir) = temp_store("crud");
        let entry = store
            .save_text(Scripts, None, "com.test.app", "main.yaml", "steps: []\n")
            .unwrap();
        assert_eq!(entry.id, "com.test.app/main.yaml");
        assert_eq!(entry.version().len(), 12);
        // 创建后同名再创建 → 冲突
        assert!(store
            .save_text(Scripts, None, "com.test.app", "main.yaml", "x")
            .is_err());
        // 重命名（更新路径）
        store
            .save_text(
                Scripts,
                Some("com.test.app/main.yaml"),
                "com.test.app",
                "renamed.yaml",
                "steps: []\n",
            )
            .unwrap();
        assert!(!dir.join("com.test.app/scripts/main.yaml").exists());
        assert!(store
            .get_text(Scripts, "com.test.app/renamed.yaml")
            .unwrap()
            .is_some());
        // 版本门禁数据源：text_version
        let v = store
            .text_version(Scripts, "com.test.app", "renamed.yaml")
            .unwrap();
        assert!(v.is_some());
        // 删除后不可见
        store
            .delete_text(Scripts, "com.test.app/renamed.yaml")
            .unwrap();
        assert!(store
            .get_text(Scripts, "com.test.app/renamed.yaml")
            .unwrap()
            .is_none());
    }

    #[test]
    fn partitions_detected_by_resource_dirs_and_cleaned_up_when_empty() {
        let (store, dir) = temp_store("partitions");
        store
            .save_text(Scripts, None, "com.a", "main.yaml", "steps: []\n")
            .unwrap();
        store
            .save_text(Functions, None, "com.b", "common.yaml", "a:\n  steps: []\n")
            .unwrap();
        std::fs::create_dir_all(dir.join("com.c")).unwrap(); // 无资源目录 = 非分区
        let parts = store.partitions();
        assert_eq!(parts, vec!["com.a", "com.b"]);
        store.delete_text(Scripts, "com.a/main.yaml").unwrap();
        assert!(!dir.join("com.a").exists(), "删空后分区目录应被清理");
    }

    // ---------- 原子写并发（原 scripts.rs 契约迁移） ----------

    #[test]
    fn atomic_write_concurrent_writers_replace_with_whole_files_only() {
        let (_store, dir) = temp_store("atomic");
        let path = dir.join("com.test.app").join("scripts").join("main.yaml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        atomic_write(&path, b"seed\n").unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for payload in [b"alpha\nalpha\n".to_vec(), b"beta\nbeta\nbeta\n".to_vec()] {
            let barrier = barrier.clone();
            let path = path.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                atomic_write(&path, &payload).unwrap();
                payload
            }));
        }
        let mut seen = Vec::new();
        for handle in handles {
            seen.push(handle.join().unwrap());
        }
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            seen.iter().any(|p| *p == content.as_bytes()),
            "并发写入后内容应完整来自某个写者"
        );
    }

    #[test]
    fn store_open_fails_fast_on_legacy_layout() {
        let dir = std::env::temp_dir().join(format!(
            "gamer-legacy-layout-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(dir.join("scripts/com.test.app")).unwrap();
        std::fs::write(dir.join("scripts/com.test.app/main.yaml"), "steps: []\n").unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let err = match ResourceStore::open(&cfg) {
            Err(err) => err,
            Ok(_) => panic!("旧布局必须 fail-fast"),
        };
        assert!(err.to_string().contains("已废弃的数据根级目录布局"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    // ---------- 验收锚点 §8.9：裸 Core 无 validator 可存内容；注册后生效 ----------

    struct RejectingValidator;
    impl ResourceKindHandler for RejectingValidator {
        fn validate_save(&self, _req: SaveValidation<'_>) -> Result<(), Value> {
            Err(serde_json::json!([
                { "code": "yaml.bad", "message": "坏内容", "step_path": "" }
            ]))
        }
        fn annotate(&self, entries: &[(String, String)]) -> serde_json::Map<String, Value> {
            let mut out = serde_json::Map::new();
            for (id, content) in entries {
                out.insert(id.clone(), serde_json::json!({ "len": content.len() }));
            }
            out
        }
    }

    #[test]
    fn bare_core_saves_without_validator_and_hook_registers_change_behavior() {
        let (store, _dir) = temp_store("barecore");
        // 未注册 handler：保存不做内容校验（裸 Core 语义，§8.9）
        store
            .save_text(Scripts, None, "com.test.app", "a.yaml", "不是 YAML 的内容")
            .unwrap();
        // 注册后：同样内容被拒绝，诊断 JSON 原样透传
        store.register_handler(Scripts, Arc::new(RejectingValidator));
        let err = store
            .validate_save(SaveValidation {
                app: "com.test.app",
                kind: Scripts,
                id: "b.yaml",
                content: "随便",
                store: &store,
            })
            .unwrap_err();
        assert_eq!(err[0]["code"], "yaml.bad");
        // 注记透明合并
        let mut entries = vec![store
            .get_text(Scripts, "com.test.app/a.yaml")
            .unwrap()
            .unwrap()];
        store.annotate(Scripts, "com.test.app", &mut entries);
        assert_eq!(entries[0].meta["len"], entries[0].content.len());
    }

    #[test]
    fn templates_same_base_conflict_and_rename_hook() {
        use std::sync::Mutex;

        struct RenameHook(Mutex<Vec<(String, String)>>);
        impl ResourceKindHandler for RenameHook {
            fn before_rename(
                &self,
                _store: &ResourceStore,
                app: &str,
                old: &str,
                new: &str,
            ) -> anyhow::Result<()> {
                self.0
                    .lock()
                    .unwrap()
                    .push((format!("{app}/{old}"), new.to_string()));
                Ok(())
            }
        }

        let (store, dir) = temp_store("templates");
        let bytes = b"png";
        store
            .create_binary(Templates, "com.test.app", "icon#001_002_003_004.png", bytes)
            .unwrap();
        // 同基名（不同区域后缀）→ 冲突，不覆盖
        let err = store
            .create_binary(Templates, "com.test.app", "icon#005_006_007_008.png", bytes)
            .unwrap_err();
        assert!(err.to_string().contains("已存在"));
        // 不同基名 → 可创建
        store
            .create_binary(Templates, "com.test.app", "other.png", bytes)
            .unwrap();

        let hook = Arc::new(RenameHook(Mutex::new(Vec::new())));
        store.register_handler(Templates, hook.clone());
        store
            .rename_binary(Templates, "com.test.app", "other.png", "renamed.png")
            .unwrap();
        assert_eq!(hook.0.lock().unwrap().len(), 1);
        assert!(dir.join("com.test.app/templates/renamed.png").is_file());
        assert!(!dir.join("com.test.app/templates/other.png").is_file());
    }

    #[test]
    fn keymaps_list_merges_composite_layers_and_local_wins() {
        let (store, dir) = temp_store("keymapcomposite");
        store
            .save_text(
                Keymaps,
                None,
                "com.test.app",
                "wasd.yaml",
                "version: 1\nname: 本地\nbindings: []\n",
            )
            .unwrap();
        // override 层
        let override_root = dir
            .join("user-overrides")
            .join("com.test.app")
            .join("keymaps");
        std::fs::create_dir_all(&override_root).unwrap();
        std::fs::write(
            override_root.join("combat.yaml"),
            "version: 1
name: 覆盖
bindings: []
"
            .as_bytes(),
        )
        .unwrap();
        let list = store.list_text("com.test.app", Keymaps).unwrap();
        let names: Vec<String> = list.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&"wasd.yaml".to_string()));
        assert!(
            names.contains(&"combat.yaml".to_string()),
            "override 层方案必须并入列表"
        );
    }
}
