//! Package 本地包存储（Package Resource 模型，plan §2-§15）。
//!
//! 数据一级作用域 = **Package ID**（不再是 Android 包名分区）。数据根
//! `<data>/packages/<package-id>/`：
//!
//! ```text
//! packages/<package-id>/
//! ├── package.toml          # manifest（id/name/version/author/targets/plugins 依赖）
//! ├── shared/               # 跨插件保留区（本波只随包导出，无插件写入口）
//! └── plugins/<plugin-id>/  # 插件数据（内部子目录语义归插件定义，Core 不解释）
//! ```
//!
//! package-id / plugin-id 都是单路径段（`[a-z0-9][a-z0-9._-]*`，禁 `.`/`..`/
//! 路径分隔符），严格校验 + path traversal 防护。资源寻址 =
//! [`PackageResource`]`(package_id, plugin_id, path)`，文本/字节统一存取；
//! 乐观并发：资源级内容版本短码（`PUT` 必须带 `expected_version` 或显式
//! `force`）+ 包级 `revision` 计数（元数据编辑条件）。
//!
//! **插件数据隔离**：资源 API 把插件限制在自己的 `plugins/<plugin-id>/` 前缀
//! 内——不能读写其他插件目录、不能写 shared/。Core 不解释插件目录内部结构；
//! 保存期内容校验/列表注记经 [`ResourceHandler`] 注册表回调给扩展（按
//! plugin-id 注册；未注册 = 裸 Core 语义，不做内容校验）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::config::Config;
use crate::core::fs::{atomic_write, content_version, is_windows_reserved_name};

/// 文本资源内容上限（与归档侧 YAML 上限同源）。
pub const TEXT_RESOURCE_MAX_BYTES: usize =
    crate::core::fs::archive_validation::IMPORT_MAX_YAML_BYTES;

// ---------------------------------------------------------------------------
// 标识与路径校验（package-id / plugin-id / 资源相对路径）
// ---------------------------------------------------------------------------

/// package-id / plugin-id 最大长度（单路径段，留足可读性）。
pub const MAX_SCOPE_ID_LEN: usize = 100;

/// 单路径段语法：`[a-z0-9][a-z0-9._-]*`；禁止 `.` / `..` / 纯点 / 路径分隔符。
/// package-id 与 plugin-id 共用同一规则。
pub fn validate_scope_id(kind: &str, value: &str) -> anyhow::Result<()> {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit() => {}
        _ => anyhow::bail!("{kind} 必须以小写字母或数字开头: {value:?}"),
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
    {
        anyhow::bail!("{kind} 只允许小写字母、数字与 . _ -（禁止路径分隔符与大写）: {value:?}");
    }
    if value == "." || value == ".." || value.matches('.').count() == value.len() {
        anyhow::bail!("{kind} 不能是纯点: {value:?}");
    }
    if value.len() > MAX_SCOPE_ID_LEN {
        anyhow::bail!("{kind} 超过 {MAX_SCOPE_ID_LEN} 字节: {value:?}");
    }
    if is_windows_reserved_name(value) {
        anyhow::bail!("{kind} 不能是 Windows 保留名: {value:?}");
    }
    Ok(())
}

/// 快速判定（同 [`validate_scope_id`] 规则）。
pub fn is_valid_scope_id(value: &str) -> bool {
    validate_scope_id("id", value).is_ok()
}

/// 资源路径分段校验：拒绝空串、反斜杠、空段、`.`、`..`、前导点与 Windows
/// 保留名；允许 `#` 与空格（插件命名惯例，如模板 `#区域` 后缀）——Core 只把
/// 它们当普通文件名字符，不解释语义。绝对路径（`/x`、`C:x`）被空段/非法
/// 字符规则覆盖。
pub fn sanitize_rel_path(rel: &str) -> anyhow::Result<Vec<String>> {
    if rel.contains('\\') {
        anyhow::bail!("资源路径不允许反斜杠: {rel:?}");
    }
    if rel.is_empty() {
        anyhow::bail!("资源路径不能为空");
    }
    rel.split('/')
        .map(|seg| {
            sanitize_segment(seg).ok_or_else(|| {
                anyhow::anyhow!(
                    "资源路径段非法: {seg:?}（拒绝空段 / . / .. / 前导点 / Windows 保留名）"
                )
            })
        })
        .collect()
}

fn sanitize_segment(seg: &str) -> Option<String> {
    if seg.is_empty()
        || seg == "."
        || seg == ".."
        || seg.starts_with('.')
        || seg.ends_with('.')
        || is_windows_reserved_name(seg)
    {
        return None;
    }
    if seg
        .chars()
        .any(|c| !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '#' | ' ')))
    {
        return None;
    }
    Some(seg.to_string())
}

// ---------------------------------------------------------------------------
// package.toml manifest
// ---------------------------------------------------------------------------

/// `[plugins."<plugin-id>"]` 依赖声明（允许声明当前未安装的插件）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginDependency {
    pub required: bool,
}

/// Package manifest（package.toml 的解析形态）。`revision` 是 Core 管理的
/// 元数据编辑计数（乐观并发条件），每次 manifest 写入自增。
#[derive(Clone, Debug, PartialEq)]
pub struct PackageManifest {
    pub id: String,
    pub name: Option<String>,
    pub version: String,
    pub author: Option<String>,
    /// Android 兼容目标（0 个 = 通用包，允许多个）。仅作运行目标声明，
    /// 不参与任何资源寻址。
    pub android_targets: Vec<String>,
    pub plugins: BTreeMap<String, PluginDependency>,
    pub revision: u64,
}

/// 新建/编辑 manifest 的输入（`version` 缺省 "0.1.0"）。
#[derive(Clone, Debug, Default)]
pub struct PackageInput {
    pub id: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub android_targets: Vec<String>,
    pub plugins: BTreeMap<String, bool>,
}

impl PackageInput {
    fn into_manifest(self, revision: u64) -> anyhow::Result<PackageManifest> {
        validate_scope_id("package id", &self.id)?;
        if let Some(name) = &self.name {
            let name = name.trim();
            anyhow::ensure!(!name.is_empty(), "name 不能为空");
            anyhow::ensure!(name.len() <= 200, "name 超过 200 字节");
        }
        let version = self
            .version
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or("0.1.0")
            .to_string();
        anyhow::ensure!(
            !version
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\')),
            "version 含非法字符: {version:?}"
        );
        anyhow::ensure!(version.len() <= 64, "version 超过 64 字节");
        if let Some(author) = &self.author {
            let author = author.trim();
            anyhow::ensure!(!author.is_empty(), "author 不能为空");
            anyhow::ensure!(author.len() <= 200, "author 超过 200 字节");
        }
        let mut targets = Vec::new();
        for target in &self.android_targets {
            let target = target.trim();
            validate_android_target(target)?;
            if !targets.iter().any(|existing| existing == target) {
                targets.push(target.to_string());
            }
        }
        let mut plugins = BTreeMap::new();
        for (plugin, required) in &self.plugins {
            validate_scope_id("plugin id", plugin)?;
            plugins.insert(
                plugin.clone(),
                PluginDependency {
                    required: *required,
                },
            );
        }
        Ok(PackageManifest {
            id: self.id,
            name: self.name.map(|n| n.trim().to_string()),
            version,
            author: self.author.map(|a| a.trim().to_string()),
            android_targets: targets,
            plugins,
            revision,
        })
    }
}

/// Android 兼容目标轻校验：非空、无分隔符/控制字符（Android 包名允许大写，
/// 与 package-id 的严格小写规则刻意不同——它只是运行目标字符串）。`*` 是
/// 合法目标值，表示通用（全部应用）。
pub(crate) fn validate_android_target(value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!value.is_empty(), "Android 目标包名不能为空");
    anyhow::ensure!(
        value.len() <= 200,
        "Android 目标包名超过 200 字节: {value:?}"
    );
    anyhow::ensure!(
        !value
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\')),
        "Android 目标包名含非法字符: {value:?}"
    );
    Ok(())
}

/// Android Target 匹配语义（package.toml 与插件 manifest 的
/// `[targets.android].packages` 共用）：空声明 = 通用（恒命中），`*` = 通用
/// （恒命中），其余按 Android 包名精确比较（区分大小写）。
pub fn android_targets_match(targets: &[String], android_package: &str) -> bool {
    if targets.is_empty() {
        return true;
    }
    let app = android_package.trim();
    targets.iter().any(|target| target == "*" || target == app)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestToml {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    revision: Option<u64>,
    #[serde(default)]
    targets: Option<TargetsToml>,
    #[serde(default, rename = "plugins")]
    plugin_deps: BTreeMap<String, PluginDepToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetsToml {
    #[serde(default)]
    android: Option<AndroidTargetsToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AndroidTargetsToml {
    #[serde(default)]
    packages: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginDepToml {
    required: bool,
}

use serde::Deserialize;

/// 严格解析 package.toml（未知字段 / 类型错配 / id 非法一律报错）。
pub fn parse_package_toml(bytes: &[u8]) -> anyhow::Result<PackageManifest> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| anyhow::anyhow!("package.toml 必须是 UTF-8: {e}"))?;
    let raw: ManifestToml =
        toml::from_str(text).map_err(|e| anyhow::anyhow!("package.toml 解析失败: {e}"))?;
    let input = PackageInput {
        id: raw.id,
        name: raw.name,
        version: raw.version,
        author: raw.author,
        android_targets: raw
            .targets
            .and_then(|t| t.android)
            .map(|a| a.packages)
            .unwrap_or_default(),
        plugins: raw
            .plugin_deps
            .into_iter()
            .map(|(plugin, dep)| (plugin, dep.required))
            .collect(),
    };
    input.into_manifest(raw.revision.unwrap_or(1))
}

/// 序列化为 package.toml 文本（固定字段顺序，可复现打包依赖此稳定性）。
pub fn serialize_package_toml(manifest: &PackageManifest) -> String {
    let mut out = String::new();
    out.push_str(&format!("id = {}\n", toml_quote(&manifest.id)));
    if let Some(name) = &manifest.name {
        out.push_str(&format!("name = {}\n", toml_quote(name)));
    }
    out.push_str(&format!("version = {}\n", toml_quote(&manifest.version)));
    if let Some(author) = &manifest.author {
        out.push_str(&format!("author = {}\n", toml_quote(author)));
    }
    out.push_str(&format!("revision = {}\n", manifest.revision));
    if !manifest.android_targets.is_empty() {
        out.push_str("\n[targets.android]\npackages = [");
        out.push_str(
            &manifest
                .android_targets
                .iter()
                .map(|t| toml_quote(t))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push_str("]\n");
    }
    for (plugin, dep) in &manifest.plugins {
        out.push_str(&format!(
            "\n[plugins.{}]\nrequired = {}\n",
            toml_quote(plugin),
            dep.required
        ));
    }
    out
}

fn toml_quote(value: &str) -> String {
    // 基本字符串：转义反斜杠与双引号（控制字符已被上游校验拒绝）
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// 资源条目
// ---------------------------------------------------------------------------

/// 文本资源条目。
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    pub package: String,
    pub plugin: String,
    /// 相对 `plugins/<plugin>/` 的路径（含扩展名）。
    pub path: String,
    pub content: String,
    pub updated_at: String,
    pub size: u64,
    pub mtime: u64,
    /// 注记字段（handler.annotate；序列化时展开进顶层）。
    pub meta: serde_json::Map<String, Value>,
}

impl ResourceEntry {
    /// 内容版本短码（内容哈希）——GET 返回、PUT expected_version 冲突检测依据。
    pub fn version(&self) -> String {
        content_version(&self.content)
    }
}

impl Serialize for ResourceEntry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut ser = serializer.serialize_map(Some(9 + self.meta.len()))?;
        ser.serialize_entry("package", &self.package)?;
        ser.serialize_entry("plugin", &self.plugin)?;
        ser.serialize_entry("path", &self.path)?;
        ser.serialize_entry("content", &self.content)?;
        ser.serialize_entry("version", &self.version())?;
        ser.serialize_entry("updated_at", &self.updated_at)?;
        ser.serialize_entry("size", &self.size)?;
        ser.serialize_entry("mtime", &self.mtime)?;
        ser.serialize_entry("text", &true)?;
        for (key, value) in &self.meta {
            ser.serialize_entry(key, value)?;
        }
        ser.end()
    }
}

/// 列表条目（递归列表统一形态：文本条目带 content/version，字节条目只有
/// 元数据；`text` 标记区分）。
#[derive(Debug, Clone, Serialize)]
pub struct ListEntry {
    pub package: String,
    pub plugin: String,
    pub path: String,
    pub size: u64,
    pub mtime: u64,
    pub updated_at: String,
    pub text: bool,
    /// 文本条目的内容（UTF-8 且 ≤ [`TEXT_RESOURCE_MAX_BYTES`]）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// 文本条目的内容版本短码。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(flatten)]
    pub meta: serde_json::Map<String, Value>,
}

fn fmt_mtime(p: &Path) -> String {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .map(|t| {
            let dt: chrono::DateTime<chrono::Local> = t.into();
            dt.format("%Y-%m-%d %H:%M:%S").to_string()
        })
        .unwrap_or_default()
}

fn mtime_secs(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn bytes_version(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut version = String::with_capacity(12);
    for byte in digest.iter().take(6) {
        version.push_str(&format!("{byte:02x}"));
    }
    version
}

// ---------------------------------------------------------------------------
// 内容钩子（按 plugin-id 注册；Core 不懂内容语义）
// ---------------------------------------------------------------------------

/// 保存期内容校验请求。`store` 供实现方构建「当前包视图 + 待写覆盖」。
pub struct SaveValidation<'a> {
    pub package: &'a str,
    pub plugin: &'a str,
    /// 目标资源相对路径（相对 `plugins/<plugin>/`，含扩展名）。
    pub path: &'a str,
    pub content: &'a str,
    pub store: &'a PackageStore,
}

/// 字节资源保存期内容校验请求（REST 字节 PUT 路径；归档导入不经过）。
/// 字段形状与 [`SaveValidation`] 对称——完整上下文交给 handler，个别字段
/// 暂无读取方（扩展后续按需消费）。
#[allow(
    dead_code,
    reason = "request shape mirrors SaveValidation; fields are consumed by handlers as needed"
)]
pub struct SaveBinaryValidation<'a> {
    pub package: &'a str,
    pub plugin: &'a str,
    /// 目标资源相对路径（相对 `plugins/<plugin>/`，含扩展名）。
    pub path: &'a str,
    pub bytes: &'a [u8],
    pub store: &'a PackageStore,
}

/// 单插件资源内容钩子。扩展在组合根注册（gamer.yaml / gamer.keymap）；
/// 未注册 = 该插件资源保存不做内容校验（裸 Core 语义）。
pub trait ResourceHandler: Send + Sync {
    /// 保存前内容校验；Err = 结构化诊断 JSON（HTTP 400 透传，格式由扩展定）。
    fn validate_save(&self, _req: SaveValidation<'_>) -> Result<(), Value> {
        Ok(())
    }

    /// 字节资源保存前钩子：校验 + 可选内容归一化。返回 `Cow::Borrowed` =
    /// 原样落盘，`Cow::Owned` = 归一化后的落盘内容；Err = 结构化诊断 JSON
    /// （HTTP 400 透传）。默认透传（裸 Core 语义）。
    fn validate_save_binary<'a>(
        &self,
        _req: SaveBinaryValidation<'a>,
    ) -> Result<std::borrow::Cow<'a, [u8]>, Value> {
        Ok(std::borrow::Cow::Borrowed(_req.bytes))
    }

    /// 列表/读取注记：entries = (path, content)；返回 path → 顶层附加字段。
    /// Core 只做透明合并。
    fn annotate(&self, _entries: &[(String, String)]) -> serde_json::Map<String, Value> {
        Default::default()
    }

    /// 重命名前钩子（如模板引用同步改写；实现方保证失败时不动任何文件）。
    /// T2a 备注：与 [`PackageStore::rename_resource`] 同为模板重命名迁移缝。
    #[allow(dead_code)]
    fn before_rename(
        &self,
        _store: &PackageStore,
        _package: &str,
        _plugin: &str,
        _old_path: &str,
        _new_path: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PackageStore
// ---------------------------------------------------------------------------

/// 包统计（`GET /api/packages/:pkg`）。
#[derive(Debug, Serialize)]
pub struct PackageStats {
    pub files: u64,
    pub bytes: u64,
    pub plugins: Vec<PluginStats>,
}

/// 单插件目录统计。
#[derive(Debug, Serialize)]
pub struct PluginStats {
    pub plugin: String,
    pub files: u64,
    pub bytes: u64,
}

/// 包不存在（HTTP 404 语义）。
#[derive(Debug, thiserror::Error)]
#[error("配置不存在: {0}")]
pub struct PackageNotFound(pub String);

/// 默认配置包 id（包存储为空时由 [`PackageStore::ensure_default_package`]
/// 播种；Android Targets = `*`、零插件依赖）。
pub const DEFAULT_PACKAGE_ID: &str = "default";

/// Core Package 本地包存储。见模块级文档。
pub struct PackageStore {
    /// 数据根（`<data>/packages`），一级子目录 = package-id。
    root: PathBuf,
    handlers: std::sync::RwLock<BTreeMap<String, std::sync::Arc<dyn ResourceHandler>>>,
}

impl PackageStore {
    pub fn open(cfg: &Config) -> anyhow::Result<Self> {
        let root = cfg.data_dir.join("packages");
        if cfg.data_dir.exists() && !root.starts_with(&cfg.data_dir) {
            anyhow::bail!("packages 数据根解析异常: {}", root.display());
        }
        std::fs::create_dir_all(&root)?;
        let store = Self {
            root,
            handlers: std::sync::RwLock::new(BTreeMap::new()),
        };
        store.reject_foreign_layout()?;
        Ok(store)
    }

    /// 数据根下不允许出现文件形态的 `packages` 条目之外的保留名（防误把
    /// 旧布局文件当包目录）。目录名不合法的子目录在列表时被跳过。
    fn reject_foreign_layout(&self) -> anyhow::Result<()> {
        if self.root.is_file() {
            anyhow::bail!(
                "packages 数据根被文件占用: {}（请备份后移除该文件）",
                self.root.display()
            );
        }
        Ok(())
    }

    // ---------- 目录解析 ----------

    /// 包目录（package-id 严格校验，非法 id 直接报错而非映射哨兵——所有
    /// 调用方都应显式处理非法输入）。
    pub fn package_dir(&self, pkg: &str) -> anyhow::Result<PathBuf> {
        validate_scope_id("package id", pkg)?;
        Ok(self.root.join(pkg))
    }

    /// 插件数据目录 `plugins/<plugin-id>/`（package/plugin 双重校验）。
    pub fn plugin_dir(&self, pkg: &str, plugin: &str) -> anyhow::Result<PathBuf> {
        validate_scope_id("plugin id", plugin)?;
        Ok(self.package_dir(pkg)?.join("plugins").join(plugin))
    }

    /// 资源磁盘路径：`plugins/<plugin>/<path>`（逐段校验防穿越）。
    pub fn resource_path(&self, pkg: &str, plugin: &str, path: &str) -> anyhow::Result<PathBuf> {
        let segs = sanitize_rel_path(path)?;
        let mut p = self.plugin_dir(pkg, plugin)?;
        for seg in &segs {
            p.push(seg);
        }
        Ok(p)
    }

    // ---------- 包生命周期 ----------

    /// 新建包：写 package.toml + 建 shared/、plugins/ 目录。已存在 → 报错。
    pub fn create_package(&self, input: PackageInput) -> anyhow::Result<PackageManifest> {
        let manifest = input.into_manifest(1)?;
        let dir = self.package_dir(&manifest.id)?;
        anyhow::ensure!(!dir.exists(), "配置已存在: {}", manifest.id);
        std::fs::create_dir_all(dir.join("shared"))?;
        std::fs::create_dir_all(dir.join("plugins"))?;
        atomic_write(
            &dir.join("package.toml"),
            serialize_package_toml(&manifest).as_bytes(),
        )?;
        Ok(manifest)
    }

    /// 包存储为空时播种「默认配置」包：Android Targets = `*`（通用）、零插件
    /// 依赖，保证全新安装开箱即用。已有任何包（含与默认包无关的损坏条目）则
    /// 不播种；`default` 目录存在但缺 package.toml（损坏）时创建失败，错误交
    /// 调用方记日志——不阻断启动。返回是否实际播种。
    pub fn ensure_default_package(&self) -> anyhow::Result<bool> {
        if !self.list_packages()?.is_empty() {
            return Ok(false);
        }
        self.create_package(PackageInput {
            id: DEFAULT_PACKAGE_ID.to_string(),
            name: Some("默认配置".to_string()),
            android_targets: vec!["*".to_string()],
            ..PackageInput::default()
        })?;
        Ok(true)
    }

    /// 磁盘上全部包（按 id 字典序）。目录名不合法或缺 package.toml 的条目
    /// 跳过（不炸整个列表）。
    pub fn list_packages(&self) -> anyhow::Result<Vec<PackageManifest>> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.root) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !is_valid_scope_id(&name) || !entry.path().is_dir() {
                    continue;
                }
                if let Some(manifest) = self.try_manifest(&name)? {
                    out.push(manifest);
                }
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// 读取 manifest；包不存在 → `PackageNotFound`。
    pub fn manifest(&self, pkg: &str) -> anyhow::Result<PackageManifest> {
        self.try_manifest(pkg)?
            .ok_or_else(|| anyhow::Error::new(PackageNotFound(pkg.to_string())))
    }

    /// 读取 manifest；缺文件/解析失败 → None（列表容错语义）。
    pub fn try_manifest(&self, pkg: &str) -> anyhow::Result<Option<PackageManifest>> {
        let path = self.package_dir(pkg)?.join("package.toml");
        if !path.is_file() {
            return Ok(None);
        }
        match std::fs::read(&path) {
            Ok(bytes) => match parse_package_toml(&bytes) {
                Ok(manifest) => {
                    if manifest.id != pkg {
                        tracing::warn!(package = pkg, "package.toml id 与目录名不一致，跳过");
                        return Ok(None);
                    }
                    Ok(Some(manifest))
                }
                Err(error) => {
                    tracing::warn!(package = pkg, %error, "package.toml 解析失败，跳过");
                    Ok(None)
                }
            },
            Err(error) => {
                tracing::warn!(package = pkg, %error, "package.toml 读取失败，跳过");
                Ok(None)
            }
        }
    }

    /// 编辑 manifest 元数据。`expected_revision` 条件更新（None + !force =
    /// 要求提供）；成功后 revision 自增。id 不可变（必须与路径一致）。
    pub fn update_manifest(
        &self,
        pkg: &str,
        input: PackageInput,
        expected_revision: Option<u64>,
        force: bool,
    ) -> anyhow::Result<PackageManifest> {
        anyhow::ensure!(
            input.id == pkg,
            "package id 不可变（{} ≠ 路径 {}）",
            input.id,
            pkg
        );
        let current = self.manifest(pkg)?;
        if !force {
            match expected_revision {
                None => {
                    anyhow::bail!("更新 manifest 必须提供 expected_revision，或显式 force:true")
                }
                Some(expected) if expected != current.revision => anyhow::bail!(
                    "manifest 已被其他页面修改（expected {expected} ≠ 当前 {}），请重新加载",
                    current.revision
                ),
                Some(_) => {}
            }
        }
        let manifest = input.into_manifest(current.revision + 1)?;
        let path = self.package_dir(pkg)?.join("package.toml");
        atomic_write(&path, serialize_package_toml(&manifest).as_bytes())?;
        Ok(manifest)
    }

    /// 删除包（整目录递归删除）。返回是否发生了删除。
    pub fn delete_package(&self, pkg: &str) -> anyhow::Result<bool> {
        let dir = self.package_dir(pkg)?;
        if !dir.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(&dir)
            .map_err(|e| anyhow::anyhow!("删除配置失败: {} ({})", e, dir.display()))?;
        Ok(true)
    }

    /// 复制包为新包（深拷贝 shared/ + plugins/；manifest 换 id、revision 归 1）。
    pub fn duplicate_package(&self, src: &str, new_id: &str) -> anyhow::Result<PackageManifest> {
        validate_scope_id("package id", new_id)?;
        let source = self.manifest(src)?;
        let target_dir = self.package_dir(new_id)?;
        anyhow::ensure!(!target_dir.exists(), "配置已存在: {new_id}");
        let manifest = PackageManifest {
            id: new_id.to_string(),
            revision: 1,
            ..source
        };
        copy_dir_contents(&self.package_dir(src)?, &target_dir)?;
        atomic_write(
            &target_dir.join("package.toml"),
            serialize_package_toml(&manifest).as_bytes(),
        )?;
        Ok(manifest)
    }

    /// 包统计（总文件/字节 + 每插件目录统计）。
    pub fn stats(&self, pkg: &str) -> anyhow::Result<PackageStats> {
        let dir = self.package_dir(pkg)?;
        anyhow::ensure!(dir.is_dir(), "配置不存在: {pkg}");
        let mut stats = PackageStats {
            files: 0,
            bytes: 0,
            plugins: Vec::new(),
        };
        let plugins_dir = dir.join("plugins");
        if let Ok(rd) = std::fs::read_dir(&plugins_dir) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !is_valid_scope_id(&name) || !entry.path().is_dir() {
                    continue;
                }
                let (files, bytes) = dir_totals(&entry.path());
                stats.plugins.push(PluginStats {
                    plugin: name,
                    files,
                    bytes,
                });
            }
        }
        stats.plugins.sort_by(|a, b| a.plugin.cmp(&b.plugin));
        let (files, bytes) = dir_totals(&dir);
        stats.files = files;
        stats.bytes = bytes;
        Ok(stats)
    }

    // ---------- 资源：文本 ----------

    /// 读取文本资源；不存在/路径非法 → None。
    pub fn read_text(
        &self,
        pkg: &str,
        plugin: &str,
        path: &str,
    ) -> anyhow::Result<Option<ResourceEntry>> {
        let Ok(disk) = self.resource_path(pkg, plugin, path) else {
            return Ok(None);
        };
        if !disk.is_file() {
            return Ok(None);
        }
        match std::fs::read_to_string(&disk) {
            Ok(content) => Ok(Some(ResourceEntry {
                package: pkg.to_string(),
                plugin: plugin.to_string(),
                path: normalize_written_path(path)?,
                updated_at: fmt_mtime(&disk),
                size: content.len() as u64,
                mtime: mtime_secs(&disk),
                content,
                meta: serde_json::Map::new(),
            })),
            // 非 UTF-8 = 非文本资源，按不存在语义返回（调用方读 binary）
            Err(_) => Ok(None),
        }
    }

    /// 写文本资源（创建或条件更新）。已存在时必须 `force` 或 `expected_version`
    /// 与当前内容版本一致；目标父目录自动创建。
    pub fn write_text(
        &self,
        pkg: &str,
        plugin: &str,
        path: &str,
        content: &str,
        expected_version: Option<&str>,
        force: bool,
    ) -> anyhow::Result<ResourceEntry> {
        let normalized = normalize_written_path(path)?;
        let disk = self.resource_path(pkg, plugin, &normalized)?;
        if disk.is_file() {
            if !force {
                let current = std::fs::read_to_string(&disk)
                    .map(|c| content_version(&c))
                    .unwrap_or_default();
                match expected_version {
                    None => anyhow::bail!(
                        "version_required: 更新资源必须提供 expected_version，或显式 force:true"
                    ),
                    Some(expected) if expected != current => anyhow::bail!(
                        "version_conflict: 资源已被其他页面修改（expected {expected} ≠ 当前 {current}），请重新加载后再保存"
                    ),
                    Some(_) => {}
                }
            }
        } else if expected_version.is_some() && !force {
            anyhow::bail!("version_conflict: 资源不存在，不能带 expected_version 创建");
        }
        atomic_write(&disk, content.as_bytes())?;
        Ok(ResourceEntry {
            package: pkg.to_string(),
            plugin: plugin.to_string(),
            path: normalized,
            updated_at: fmt_mtime(&disk),
            size: content.len() as u64,
            mtime: mtime_secs(&disk),
            content: content.to_string(),
            meta: serde_json::Map::new(),
        })
    }

    /// 直接覆盖写文本资源（不经版本门禁；导入/引用改写等受控场景使用——
    /// 调用方负责回滚）。
    pub fn write_text_unchecked(
        &self,
        pkg: &str,
        plugin: &str,
        path: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        let disk = self.resource_path(pkg, plugin, path)?;
        atomic_write(&disk, content.as_bytes())
    }

    // ---------- 资源：字节 ----------

    /// 读取字节资源；不存在/路径非法 → None。
    pub fn read_binary(
        &self,
        pkg: &str,
        plugin: &str,
        path: &str,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let Ok(disk) = self.resource_path(pkg, plugin, path) else {
            return Ok(None);
        };
        if !disk.is_file() {
            return Ok(None);
        }
        Ok(std::fs::read(&disk).ok())
    }

    /// 写字节资源（创建或条件更新；语义同 [`PackageStore::write_text`]，
    /// 版本 = 字节内容哈希）。
    pub fn write_binary(
        &self,
        pkg: &str,
        plugin: &str,
        path: &str,
        bytes: &[u8],
        expected_version: Option<&str>,
        force: bool,
    ) -> anyhow::Result<ListEntry> {
        let normalized = normalize_written_path(path)?;
        let disk = self.resource_path(pkg, plugin, &normalized)?;
        if disk.is_file() {
            if !force {
                let current = std::fs::read(&disk)
                    .map(|b| bytes_version(&b))
                    .unwrap_or_default();
                match expected_version {
                    None => anyhow::bail!(
                        "version_required: 更新资源必须提供 expected_version，或显式 force:true"
                    ),
                    Some(expected) if expected != current => anyhow::bail!(
                        "version_conflict: 资源已被其他页面修改（expected {expected} ≠ 当前 {current}），请重新加载后再保存"
                    ),
                    Some(_) => {}
                }
            }
        } else if expected_version.is_some() && !force {
            anyhow::bail!("version_conflict: 资源不存在，不能带 expected_version 创建");
        }
        atomic_write(&disk, bytes)?;
        Ok(ListEntry {
            package: pkg.to_string(),
            plugin: plugin.to_string(),
            path: normalized,
            size: bytes.len() as u64,
            mtime: mtime_secs(&disk),
            updated_at: fmt_mtime(&disk),
            text: false,
            content: None,
            version: Some(bytes_version(bytes)),
            meta: serde_json::Map::new(),
        })
    }

    /// 删除资源文件（不存在 → 报错）；返回被删磁盘路径。删除后向上清理
    /// 空目录（不超过插件目录本身）。
    pub fn delete_resource(&self, pkg: &str, plugin: &str, path: &str) -> anyhow::Result<PathBuf> {
        let disk = self.resource_path(pkg, plugin, path)?;
        if !disk.is_file() {
            anyhow::bail!("资源不存在: {pkg}/{plugin}/{path}");
        }
        std::fs::remove_file(&disk)
            .map_err(|e| anyhow::anyhow!("删除失败: {} ({})", e, disk.display()))?;
        let plugin_root = self.plugin_dir(pkg, plugin)?;
        let mut parent = disk.parent();
        while let Some(dir) = parent {
            if dir == plugin_root || !dir.starts_with(&plugin_root) {
                break;
            }
            if std::fs::remove_dir(dir).is_err() {
                break; // 非空即停
            }
            parent = dir.parent();
        }
        Ok(disk)
    }

    /// 重命名/移动资源（同插件内）。先经 handler.before_rename 钩子（如模板
    /// 引用改写），钩子失败则不动文件。T2a 备注：模板重命名 REST 面随六目录
    /// API 退役，本方法与钩子是为模板引用改写语义保留的迁移缝。
    // T2a 迁移缝：REST 面暂缺，测试经下方 rename 用例覆盖钩子语义
    #[allow(dead_code)]
    pub fn rename_resource(
        &self,
        pkg: &str,
        plugin: &str,
        old_path: &str,
        new_path: &str,
    ) -> anyhow::Result<()> {
        let old_normalized = normalize_written_path(old_path)?;
        let new_normalized = normalize_written_path(new_path)?;
        if old_normalized == new_normalized {
            anyhow::bail!("名称未变化");
        }
        let old_disk = self.resource_path(pkg, plugin, &old_normalized)?;
        let new_disk = self.resource_path(pkg, plugin, &new_normalized)?;
        if !old_disk.is_file() {
            anyhow::bail!("资源不存在: {pkg}/{plugin}/{old_normalized}");
        }
        anyhow::ensure!(
            !new_disk.exists(),
            "资源已存在: {pkg}/{plugin}/{new_normalized}"
        );
        if let Some(handler) = self.handler(plugin) {
            handler.before_rename(self, pkg, plugin, &old_normalized, &new_normalized)?;
        }
        if let Some(parent) = new_disk.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&old_disk, &new_disk).map_err(|e| anyhow::anyhow!("重命名失败: {e}"))?;
        Ok(())
    }

    // ---------- 资源：列表 ----------

    /// 递归列出插件目录（`prefix` 可选限定子目录，空串 = 全部）。文本探测：
    /// UTF-8 可解码且 ≤ [`TEXT_RESOURCE_MAX_BYTES`]；文本条目合并 handler 注记。
    /// 按 path 字典序。
    pub fn list(&self, pkg: &str, plugin: &str, prefix: &str) -> anyhow::Result<Vec<ListEntry>> {
        let plugin_root = self.plugin_dir(pkg, plugin)?;
        let root = if prefix.trim().is_empty() {
            plugin_root.clone()
        } else {
            self.resource_path(pkg, plugin, prefix.trim_end_matches('/'))?
        };
        let mut out = Vec::new();
        if root.is_dir() {
            // path 恒相对插件目录（PackageResource path 空间），与 prefix 无关
            Self::collect(&root, &plugin_root, pkg, plugin, &mut out);
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        // 文本条目注记透明合并
        if let Some(handler) = self.handler(plugin) {
            let pairs: Vec<(String, String)> = out
                .iter()
                .filter_map(|e| e.content.clone().map(|c| (e.path.clone(), c)))
                .collect();
            let meta = handler.annotate(&pairs);
            for entry in out.iter_mut() {
                if let Some(value) = meta.get(&entry.path) {
                    entry.meta = value.as_object().cloned().unwrap_or_default();
                }
            }
        }
        Ok(out)
    }

    fn collect(dir: &Path, root: &Path, pkg: &str, plugin: &str, out: &mut Vec<ListEntry>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let name = match entry.file_name().into_string() {
                Ok(name) => name,
                Err(_) => continue,
            };
            if name.starts_with('.') {
                continue; // 隐藏文件 / 临时文件不进列表
            }
            let path = entry.path();
            if path.is_dir() {
                Self::collect(&path, root, pkg, plugin, out);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let rel = match path.strip_prefix(root) {
                Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            let meta = entry.metadata().ok();
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let (content, version) = if size <= TEXT_RESOURCE_MAX_BYTES as u64 {
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        let version = content_version(&text);
                        (Some(text), Some(version))
                    }
                    Err(_) => (None, None),
                }
            } else {
                (None, None)
            };
            out.push(ListEntry {
                package: pkg.to_string(),
                plugin: plugin.to_string(),
                path: rel,
                size,
                mtime: mtime_secs(&path),
                updated_at: fmt_mtime(&path),
                text: content.is_some(),
                content,
                version,
                meta: serde_json::Map::new(),
            });
        }
    }

    /// 文本条目注记合并：entries 内容按 path → meta 透明并入条目顶层（单条
    /// 读取路径复用列表注记语义）。
    pub fn annotate_text(&self, plugin: &str, entries: &mut [ResourceEntry]) {
        let Some(handler) = self.handler(plugin) else {
            return;
        };
        let pairs: Vec<(String, String)> = entries
            .iter()
            .map(|entry| (entry.path.clone(), entry.content.clone()))
            .collect();
        let meta = handler.annotate(&pairs);
        for entry in entries.iter_mut() {
            if let Some(value) = meta.get(&entry.path) {
                entry.meta = value.as_object().cloned().unwrap_or_default();
            }
        }
    }

    // ---------- 模板短名消歧（vision 链路机械迁移；`#` 后缀命名约定） ----------

    /// 精确路径优先；否则按「基名 + `#` 后缀 + 同扩展名」在同目录内唯一匹配
    /// （模板短名引用约定：`icon.png` → `icon#1_2_3_4.png`）。零候选/多候选
    /// 均报错。Core 只做文件名消歧，不解释模板内容语义。
    pub fn resolve_short_path(
        &self,
        pkg: &str,
        plugin: &str,
        path: &str,
    ) -> anyhow::Result<PathBuf> {
        let disk = self.resource_path(pkg, plugin, path)?;
        if disk.is_file() {
            return Ok(disk);
        }
        let dir = disk
            .parent()
            .ok_or_else(|| anyhow::anyhow!("资源路径没有父目录: {path}"))?
            .to_path_buf();
        let name = disk
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("资源路径文件名无效: {path}"))?;
        let Some((base, ext)) = name.rsplit_once('.') else {
            anyhow::bail!("资源不存在: {pkg}/{plugin}/{path}");
        };
        let prefix = format!("{}#", base.to_ascii_lowercase());
        let dotted = format!(".{}", ext.to_ascii_lowercase());
        let mut candidates: Vec<String> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|candidate| {
                let lower = candidate.to_ascii_lowercase();
                lower.starts_with(&prefix) && lower.ends_with(&dotted)
            })
            .collect();
        candidates.sort();
        match candidates.len() {
            1 => Ok(dir.join(&candidates[0])),
            0 => anyhow::bail!("资源不存在: {pkg}/{plugin}/{path}"),
            _ => anyhow::bail!(
                "资源 {path} 匹配到多个候选：{}，请用完整文件名指定",
                candidates.join("、")
            ),
        }
    }

    // ---------- 钩子注册 ----------

    /// 注册插件的内容钩子（组合根引导期调用；同插件重复注册 = 替换）。
    pub fn register_handler(&self, plugin: &str, handler: std::sync::Arc<dyn ResourceHandler>) {
        self.handlers
            .write()
            .expect("resource handler registry poisoned")
            .insert(plugin.to_string(), handler);
    }

    fn handler(&self, plugin: &str) -> Option<std::sync::Arc<dyn ResourceHandler>> {
        self.handlers
            .read()
            .expect("resource handler registry poisoned")
            .get(plugin)
            .cloned()
    }

    /// 保存前内容校验：分发到已注册 handler；未注册 = 通过（裸 Core 语义）。
    pub fn validate_save(&self, req: SaveValidation<'_>) -> Result<(), Value> {
        let handler = self.handler(req.plugin);
        match handler {
            Some(handler) => handler.validate_save(req),
            None => Ok(()),
        }
    }

    /// 字节资源保存前钩子分发：返回落盘内容（hook 可归一化）；未注册
    /// handler = 原样透传（裸 Core 语义）。实现侧 Cow 归一化在此收口为
    /// owned 字节，借用不逃出本调用。
    pub fn validate_save_binary(&self, req: SaveBinaryValidation<'_>) -> Result<Vec<u8>, Value> {
        let handler = self.handler(req.plugin);
        match handler {
            Some(handler) => handler
                .validate_save_binary(req)
                .map(|cow| cow.into_owned()),
            None => Ok(req.bytes.to_vec()),
        }
    }

    /// 归档目录安全提取后的 staging 根（导入流程专用；Core 管理的临时目录）。
    pub(crate) fn staging_root(&self) -> PathBuf {
        self.root.join(".staging")
    }
}

/// 导入/复制共用的目录深拷贝（跳过隐藏文件；保留相对结构）。
pub(crate) fn copy_dir_contents(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let target = dst.join(entry.file_name());
        let path = entry.path();
        if path.is_dir() {
            copy_dir_contents(&path, &target)?;
        } else if path.is_file() {
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

/// 目录资源统计（排除隐藏文件与 package.toml——manifest 是包身份元数据，
/// 不是插件资源）。
fn dir_totals(dir: &Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "package.toml" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                files += 1;
                bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    (files, bytes)
}

/// 写入路径规范化：trim + 分段校验（不补扩展名——Core 内容无关，扩展名
/// 语义归插件）。
fn normalize_written_path(path: &str) -> anyhow::Result<String> {
    let segs = sanitize_rel_path(path.trim())?;
    Ok(segs.join("/"))
}

// ---------------------------------------------------------------------------
// 导入导出归档工具（从旧 app_packages archive/builder 收编的 zip 安全逻辑）
// ---------------------------------------------------------------------------

/// 归档上限（对齐 archive_validation 预算）。
pub mod archive_limits {
    /// 单文件上限。
    pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
    pub const MANIFEST_NAME: &str = "package.toml";
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn temp_store(tag: &str) -> (PackageStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "gamer-pkgstore-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        (PackageStore::open(&cfg).unwrap(), dir)
    }

    fn input(id: &str) -> PackageInput {
        PackageInput {
            id: id.to_string(),
            ..Default::default()
        }
    }

    // ---------- id / 路径校验 ----------

    #[test]
    fn scope_id_validation_is_strict() {
        for ok in [
            "a",
            "gamer.yaml",
            "gamer.keymap",
            "official.hsr.daily",
            "9lives",
            "a-b_c.d",
        ] {
            assert!(is_valid_scope_id(ok), "{ok:?} 应合法");
        }
        for bad in [
            "",
            ".",
            "..",
            "...",
            ".hidden",
            "A.upper",
            "has space",
            "a/b",
            "a\\b",
            "-lead",
            "con",
            "aux.yaml",
        ] {
            assert!(!is_valid_scope_id(bad), "{bad:?} 必须被拒绝");
        }
        let long = format!("a{}", "b".repeat(MAX_SCOPE_ID_LEN));
        assert!(!is_valid_scope_id(&long));
    }

    #[test]
    fn rel_path_rejects_traversal_and_bad_segments() {
        let bad = [
            "",
            "/abs",
            "..",
            "../escape",
            "a/../../b",
            "a//b",
            ".hidden",
            "a\\b",
            "C:/x",
        ];
        for rel in bad {
            assert!(sanitize_rel_path(rel).is_err(), "{rel:?} 必须被拒绝");
        }
        // `#` 与空格是合法文件名字符（模板命名惯例）
        assert_eq!(
            sanitize_rel_path("templates/icon#001_002.png").unwrap(),
            vec!["templates".to_string(), "icon#001_002.png".to_string()]
        );
        // 中文文件名合法（Unicode 字母数字）；GBK 包文件名被 Latin-1 误读出的
        // 乱码（»/½ 等符号与 C1 控制符）必须拒绝——前端上传前先按 GBK 修正
        assert!(sanitize_rel_path("templates/登录.png").is_ok());
        let mojibake = "templates/ç\u{99}»å½\u{95}.png";
        assert!(
            sanitize_rel_path(mojibake).is_err(),
            "{mojibake:?} 必须被拒绝"
        );
    }

    // ---------- manifest 解析/序列化 ----------

    #[test]
    fn manifest_roundtrip_with_defaults_and_plugins() {
        let text = r#"
id = "official.hsr.daily"
name = "星穹铁道日常"
version = "1.2.0"

[targets.android]
packages = ["com.miHoYo.hkrpg", "com.HoYoverse.hkrpgoversea"]

[plugins."gamer.yaml"]
required = true

[plugins."gamer.keymap"]
required = false
"#;
        let manifest = parse_package_toml(text.as_bytes()).unwrap();
        assert_eq!(manifest.id, "official.hsr.daily");
        assert_eq!(manifest.version, "1.2.0");
        assert_eq!(manifest.revision, 1);
        assert_eq!(manifest.android_targets.len(), 2);
        assert_eq!(manifest.plugins.len(), 2);
        assert!(manifest.plugins["gamer.yaml"].required);
        assert!(!manifest.plugins["gamer.keymap"].required);

        // 序列化 → 再解析往返一致
        let reparsed = parse_package_toml(serialize_package_toml(&manifest).as_bytes()).unwrap();
        assert_eq!(manifest, reparsed);

        // 缺省：version → 0.1.0，revision → 1
        let minimal = parse_package_toml(b"id = \"a.b\"").unwrap();
        assert_eq!(minimal.version, "0.1.0");
        assert_eq!(minimal.android_targets, Vec::<String>::new());
    }

    #[test]
    fn manifest_strictly_rejects_unknown_fields_and_bad_ids() {
        let unknown = b"id = \"a.b\"\nwhat = 1\n";
        assert!(parse_package_toml(unknown).is_err());
        let bad_id = b"id = \"Bad/Id\"\n";
        assert!(parse_package_toml(bad_id).is_err());
        let empty = b"";
        assert!(parse_package_toml(empty).is_err());
        // 重复 android 目标去重
        let dup = b"id = \"a.b\"\n[targets.android]\npackages = [\"com.x\", \"com.x\"]\n";
        let manifest = parse_package_toml(dup).unwrap();
        assert_eq!(manifest.android_targets, vec!["com.x".to_string()]);
    }

    #[test]
    fn wildcard_is_a_valid_android_target_value() {
        validate_android_target("*").unwrap();
        validate_android_target(" com.example.Game ").unwrap();
    }

    #[test]
    fn android_targets_match_supports_wildcard_and_empty() {
        // 空 = 通用恒命中；`*` = 通用恒命中
        assert!(android_targets_match(&[], "com.any.app"));
        assert!(android_targets_match(&["*".to_string()], "com.any.app"));
        // 精确命中（区分大小写：Android 包名允许大写）
        let specific = vec!["com.example.Game".to_string(), "com.other".to_string()];
        assert!(android_targets_match(&specific, "com.example.Game"));
        assert!(!android_targets_match(&specific, "com.example.game"));
        assert!(!android_targets_match(&specific, "com.miss"));
        // `*` 混排恒命中
        let mixed = vec!["com.a".to_string(), "*".to_string()];
        assert!(android_targets_match(&mixed, "whatever"));
    }

    #[test]
    fn default_package_seeds_only_into_empty_store() {
        let (store, dir) = temp_store("default-seed");
        assert!(store.list_packages().unwrap().is_empty());
        // 空存储播种：targets = *、零插件依赖
        assert!(store.ensure_default_package().unwrap());
        let manifest = store.manifest(DEFAULT_PACKAGE_ID).unwrap();
        assert_eq!(manifest.android_targets, vec!["*".to_string()]);
        assert!(manifest.plugins.is_empty());
        assert_eq!(manifest.name.as_deref(), Some("默认配置"));
        assert_eq!(manifest.version, "0.1.0");
        assert!(dir.join("packages/default/package.toml").is_file());

        // 已有任意包（含删除默认包后剩余的）都不再播种
        assert!(!store.ensure_default_package().unwrap());
        store.create_package(input("official.demo")).unwrap();
        store.delete_package(DEFAULT_PACKAGE_ID).unwrap();
        assert!(!store.ensure_default_package().unwrap());

        // 存储重新清空 → 再次播种；损坏的 default 目录（缺 package.toml）使
        // 播种报错而非静默通过（调用方记 warn 不阻断启动）
        store.delete_package("official.demo").unwrap();
        assert!(store.ensure_default_package().unwrap());
        std::fs::remove_dir_all(dir.join("packages/default")).unwrap();
        std::fs::create_dir_all(dir.join("packages/default")).unwrap();
        assert!(store.ensure_default_package().is_err());
    }

    // ---------- 包生命周期 ----------

    #[test]
    fn package_lifecycle_create_list_duplicate_delete() {
        let (store, dir) = temp_store("lifecycle");
        let mut plugins = BTreeMap::new();
        plugins.insert("gamer.yaml".to_string(), true);
        let manifest = store
            .create_package(PackageInput {
                id: "official.demo".into(),
                name: Some("演示".into()),
                version: Some("1.0.0".into()),
                android_targets: vec!["com.example.game".into()],
                plugins,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(manifest.revision, 1);
        assert!(dir.join("packages/official.demo/package.toml").is_file());
        assert!(dir.join("packages/official.demo/shared").is_dir());
        assert!(dir.join("packages/official.demo/plugins").is_dir());

        // 重复创建 → 报错
        assert!(store.create_package(input("official.demo")).is_err());

        // 写一个插件资源后统计
        store
            .write_text(
                "official.demo",
                "gamer.yaml",
                "automations/daily.yaml",
                "version: 3\nsteps: []\n",
                None,
                false,
            )
            .unwrap();
        let stats = store.stats("official.demo").unwrap();
        assert_eq!(stats.files, 1);
        assert_eq!(stats.plugins.len(), 1);
        assert_eq!(stats.plugins[0].plugin, "gamer.yaml");

        // 复制为新包：资源随拷、manifest 换 id、revision 归 1
        let copy = store
            .duplicate_package("official.demo", "user.demo")
            .unwrap();
        assert_eq!(copy.id, "user.demo");
        assert_eq!(copy.revision, 1);
        assert_eq!(copy.name.as_deref(), Some("演示"));
        let copied = store
            .read_text("user.demo", "gamer.yaml", "automations/daily.yaml")
            .unwrap()
            .unwrap();
        assert!(copied.content.contains("version: 3"));

        // 列表字典序
        let ids: Vec<String> = store
            .list_packages()
            .unwrap()
            .iter()
            .map(|m| m.id.clone())
            .collect();
        assert_eq!(ids, vec!["official.demo", "user.demo"]);

        // 删除
        assert!(store.delete_package("user.demo").unwrap());
        assert!(!store.delete_package("user.demo").unwrap());
        assert!(!dir.join("packages/user.demo").exists());
    }

    #[test]
    fn manifest_update_is_revision_gated() {
        let (store, _dir) = temp_store("revision");
        store.create_package(input("a.b")).unwrap();
        // 无 expected_revision 且不 force → 拒绝
        assert!(store
            .update_manifest("a.b", input("a.b"), None, false)
            .is_err());
        // 错误 revision → 拒绝
        assert!(store
            .update_manifest("a.b", input("a.b"), Some(99), false)
            .is_err());
        // 正确 revision → 通过并自增
        let updated = store
            .update_manifest(
                "a.b",
                PackageInput {
                    id: "a.b".into(),
                    name: Some("renamed".into()),
                    ..Default::default()
                },
                Some(1),
                false,
            )
            .unwrap();
        assert_eq!(updated.revision, 2);
        assert_eq!(updated.name.as_deref(), Some("renamed"));
        // force 跳过门禁
        let forced = store
            .update_manifest("a.b", input("a.b"), None, true)
            .unwrap();
        assert_eq!(forced.revision, 3);
        // id 不可变
        assert!(store
            .update_manifest("a.b", input("c.d"), Some(3), false)
            .is_err());
    }

    // ---------- 资源 CRUD + 乐观并发 + 插件隔离 ----------

    #[test]
    fn resource_crud_roundtrip_with_version_gate() {
        let (store, dir) = temp_store("crud");
        store.create_package(input("a.b")).unwrap();
        let entry = store
            .write_text(
                "a.b",
                "gamer.yaml",
                "automations/main.yaml",
                "steps: []\n",
                None,
                false,
            )
            .unwrap();
        assert_eq!(entry.path, "automations/main.yaml");
        assert_eq!(entry.version().len(), 12);

        // 嵌套路径
        store
            .write_text(
                "a.b",
                "gamer.yaml",
                "automations/sub/inner.yaml",
                "x: 1\n",
                None,
                false,
            )
            .unwrap();

        // 创建后无门禁再写 → version_required；带错版本 → version_conflict
        let err = store
            .write_text(
                "a.b",
                "gamer.yaml",
                "automations/main.yaml",
                "x",
                None,
                false,
            )
            .unwrap_err();
        assert!(err.to_string().contains("version_required"), "{err}");
        let err = store
            .write_text(
                "a.b",
                "gamer.yaml",
                "automations/main.yaml",
                "x",
                Some("bad"),
                false,
            )
            .unwrap_err();
        assert!(err.to_string().contains("version_conflict"), "{err}");
        // 带对版本 → 通过
        let version = entry.version();
        store
            .write_text(
                "a.b",
                "gamer.yaml",
                "automations/main.yaml",
                "steps: []\n",
                Some(&version),
                false,
            )
            .unwrap();
        // force 跳过门禁
        store
            .write_text(
                "a.b",
                "gamer.yaml",
                "automations/main.yaml",
                "steps: []\n",
                None,
                true,
            )
            .unwrap();

        // 读取
        let read = store
            .read_text("a.b", "gamer.yaml", "automations/main.yaml")
            .unwrap()
            .unwrap();
        assert!(read.content.starts_with("steps:"));
        assert!(store
            .read_text("a.b", "gamer.yaml", "automations/missing.yaml")
            .unwrap()
            .is_none());
        assert!(store
            .read_text("a.b", "gamer.yaml", "../escape")
            .unwrap()
            .is_none());

        // 字节资源 + 条件更新
        let raw = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00];
        let written = store
            .write_binary("a.b", "gamer.yaml", "templates/icon.png", &raw, None, false)
            .unwrap();
        assert_eq!(written.size, 9);
        assert_eq!(
            store
                .read_binary("a.b", "gamer.yaml", "templates/icon.png")
                .unwrap(),
            Some(raw.to_vec())
        );
        assert!(store
            .write_binary("a.b", "gamer.yaml", "templates/icon.png", b"x", None, false)
            .is_err());

        // 列表（递归 + 文本探测）
        let list = store.list("a.b", "gamer.yaml", "").unwrap();
        let paths: Vec<String> = list.iter().map(|e| e.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                "automations/main.yaml".to_string(),
                "automations/sub/inner.yaml".to_string(),
                "templates/icon.png".to_string()
            ]
        );
        assert!(list[0].text);
        assert!(list[0].version.is_some());
        assert!(!list[2].text);
        assert!(list[2].content.is_none());

        // 子目录前缀列表
        let sub = store.list("a.b", "gamer.yaml", "automations/sub").unwrap();
        assert_eq!(sub.len(), 1);

        // 删除 + 空目录清理
        store
            .delete_resource("a.b", "gamer.yaml", "automations/sub/inner.yaml")
            .unwrap();
        assert!(!dir
            .join("packages/a.b/plugins/gamer.yaml/automations/sub")
            .exists());
        assert!(store
            .delete_resource("a.b", "gamer.yaml", "automations/sub/inner.yaml")
            .is_err());
    }

    #[test]
    fn resources_are_confined_to_own_plugin_directory() {
        let (store, _dir) = temp_store("isolation");
        store.create_package(input("a.b")).unwrap();
        // 任意穿越尝试（含逃向 shared/ 与其他插件目录）都落到校验失败
        for evil in ["../../shared/evil", "../other-plugin/x", "/abs", "a/../.."] {
            assert!(
                store.resource_path("a.b", "gamer.yaml", evil).is_err(),
                "{evil:?}"
            );
        }
        // 非法 plugin id 直接拒绝
        assert!(store.plugin_dir("a.b", "../other").is_err());
        assert!(store.plugin_dir("../escape", "gamer.yaml").is_err());
    }

    #[test]
    fn short_path_disambiguation_matches_unique_hash_suffix() {
        let (store, _dir) = temp_store("shortname");
        store.create_package(input("a.b")).unwrap();
        store
            .write_binary(
                "a.b",
                "gamer.yaml",
                "templates/icon#1_2_3_4.png",
                b"png",
                None,
                false,
            )
            .unwrap();
        // 短名 → 唯一 # 候选
        let hit = store
            .resolve_short_path("a.b", "gamer.yaml", "templates/icon.png")
            .unwrap();
        assert_eq!(
            hit.file_name().unwrap().to_string_lossy(),
            "icon#1_2_3_4.png"
        );
        // 精确名优先
        store
            .write_binary(
                "a.b",
                "gamer.yaml",
                "templates/full.png",
                b"png",
                None,
                false,
            )
            .unwrap();
        let hit = store
            .resolve_short_path("a.b", "gamer.yaml", "templates/full.png")
            .unwrap();
        assert_eq!(hit.file_name().unwrap().to_string_lossy(), "full.png");
        // 零候选
        assert!(store
            .resolve_short_path("a.b", "gamer.yaml", "templates/missing.png")
            .is_err());
        // 扩展名不参与跨类匹配
        assert!(store
            .resolve_short_path("a.b", "gamer.yaml", "automations/icon.yaml")
            .is_err());
    }

    // ---------- 验收锚点：裸 Core 无 handler 可存内容；注册后生效 ----------

    struct RejectingHandler;
    impl ResourceHandler for RejectingHandler {
        fn validate_save(&self, _req: SaveValidation<'_>) -> Result<(), Value> {
            Err(serde_json::json!([
                { "code": "yaml.bad", "message": "坏内容", "step_path": "" }
            ]))
        }
        fn annotate(&self, entries: &[(String, String)]) -> serde_json::Map<String, Value> {
            let mut out = serde_json::Map::new();
            for (path, content) in entries {
                out.insert(path.clone(), serde_json::json!({ "len": content.len() }));
            }
            out
        }
    }

    #[test]
    fn bare_core_saves_without_handler_and_hook_registration_changes_behavior() {
        let (store, _dir) = temp_store("barecore");
        store.create_package(input("a.b")).unwrap();
        // 未注册 handler：保存不做内容校验（裸 Core 语义）
        store
            .write_text(
                "a.b",
                "gamer.yaml",
                "automations/a.yaml",
                "不是 YAML 的内容",
                None,
                false,
            )
            .unwrap();
        // 注册后：同样内容被拒绝，诊断 JSON 原样透传
        store.register_handler("gamer.yaml", Arc::new(RejectingHandler));
        let err = store
            .validate_save(SaveValidation {
                package: "a.b",
                plugin: "gamer.yaml",
                path: "automations/b.yaml",
                content: "随便",
                store: &store,
            })
            .unwrap_err();
        assert_eq!(err[0]["code"], "yaml.bad");
        // 注记透明合并（列表）
        let list = store.list("a.b", "gamer.yaml", "").unwrap();
        assert_eq!(list[0].meta["len"], list[0].content.as_ref().unwrap().len());
    }

    // ---------- 原子写并发 ----------

    #[test]
    fn concurrent_writers_produce_whole_files_only() {
        let (store, _dir) = temp_store("atomic");
        store.create_package(input("a.b")).unwrap();
        let path = "automations/main.yaml";
        store
            .write_text("a.b", "gamer.yaml", path, "seed\n", None, true)
            .unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        let store = Arc::new(store);
        for payload in [
            "alpha\nalpha\n".to_string(),
            "beta\nbeta\nbeta\n".to_string(),
        ] {
            let barrier = barrier.clone();
            let store = store.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store
                    .write_text("a.b", "gamer.yaml", path, &payload, None, true)
                    .unwrap();
                payload
            }));
        }
        let mut seen = Vec::new();
        for handle in handles {
            seen.push(handle.join().unwrap());
        }
        let content = store
            .read_text("a.b", "gamer.yaml", path)
            .unwrap()
            .unwrap()
            .content;
        assert!(seen.contains(&content), "并发写入后内容应完整来自某个写者");
    }
}
