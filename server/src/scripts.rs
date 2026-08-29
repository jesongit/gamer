//! 脚本/函数库/模板文件存储：按应用分区 `data/<pkg>/yaml/` + `data/<pkg>/func/` + `data/<pkg>/tmpl/`
//!
//! 分区 = 设备配置的应用包名（如 com.miHoYo.hkrpg），无 default 兜底；
//! 脚本 id = `<pkg>/<name>.yaml`（含 `/`，前端拼 URL 必须整体 encodeURIComponent）。
//! 旧 `package <名字>` YAML 指令已废除（引擎直接解析 YAML，残留指令行 = 解析报错），
//! 旧目录布局由 migrate_fs_layout 启动时一次性迁移并顺手剥离指令行。
//!
//! 分区快照 zip = 导出/导入同构（导出为整分区全量，不再按单个脚本收集依赖闭包）：
//!   yaml/<name>.yaml   分区内全部脚本
//!   func/<name>.yaml   分区内全部函数库文件（顶层键 = 函数名）
//!   tmpl/<模板名>       分区内全部模板图片
//! 导入必须显式指定目标分区（?pkg=）；三目录均可缺省。
//!
//! 路径解析（阶段 1，目录即类型）：resolve_script_path / resolve_function_path /
//! resolve_template_path 三套拆开——拒绝绝对路径、反斜杠、空段、`.`、`..`、跨分区与
//! 扩展名错配；不回退（脚本只认 yaml/、函数库只认 func/、模板只认 tmpl/ 现存文件）、
//! 不做内容推断。契约见 docs/SCRIPT_EDITOR_CONTRACT.md §3.1。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Serialize, Serializer};

use crate::config::Config;

/// 将一个文件以“同目录临时文件 + flush/sync + replace”方式写入。
///
/// 临时文件和目标文件必须位于同一文件系统，这样最后的替换才是原子的；
/// 写入或替换失败时只清理临时文件，不触碰已有目标文件。
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    atomic_write_with(path, bytes, replace_file)
}

#[cfg(test)]
fn atomic_write_with_replace_err(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    atomic_write_with(path, bytes, |_temp, _path| {
        Err(std::io::Error::other("injected replace failure"))
    })
}

fn atomic_write_with(
    path: &Path,
    bytes: &[u8],
    replace: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("目标路径没有父目录: {}", path.display()))?;
    std::fs::create_dir_all(parent)?;

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("目标文件名无效: {}", path.display()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mut temp = None;
    let mut file = None;
    for attempt in 0..16u32 {
        let candidate = parent.join(format!(
            ".{name}.tmp-{}-{nonce}-{attempt}",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(f) => {
                temp = Some(candidate);
                file = Some(f);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let temp = temp.ok_or_else(|| anyhow::anyhow!("无法创建临时文件: {}", path.display()))?;
    let mut file = file.expect("临时文件句柄必须与路径同时创建");
    let result = (|| -> anyhow::Result<()> {
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        #[cfg(windows)]
        let _guard = replace_lock().lock().unwrap();
        replace(&temp, path)?;
        sync_parent(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    std::fs::rename(temp, path)
}

#[cfg(windows)]
fn replace_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }

    let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
    let ok = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> std::io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(windows)]
fn replace_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

/// ZIP 导入资源硬限（阶段 2 SEC-004；传输层另有 20MiB body 闸门）
pub const IMPORT_MAX_ARCHIVE_BYTES: usize = 20 * 1024 * 1024; // 压缩包 ≤20MiB
pub const IMPORT_MAX_TOTAL_BYTES: usize = 100 * 1024 * 1024; // 总解压量 ≤100MiB
pub const IMPORT_MAX_ENTRIES: usize = 500; // 条目数 ≤500
pub const IMPORT_MAX_YAML_BYTES: usize = 1024 * 1024; // 单 YAML ≤1MiB
pub const IMPORT_MAX_TMPL_BYTES: usize = 10 * 1024 * 1024; // 单模板 ≤10MiB

/// 磁盘上的一个脚本文件（id = `<pkg>/<name>`，name 含 .yaml/.yml 扩展名；package 字段 = 应用分区）
#[derive(Debug, Clone)]
pub struct ScriptFile {
    pub id: String,
    pub package: String,
    pub name: String,
    pub content: String,
    pub updated_at: String,
}

impl ScriptFile {
    /// 内容版本短码（内容哈希）——GET 返回、保存接口 expected_version 冲突检测依据
    pub fn version(&self) -> String {
        content_version(&self.content)
    }
}

/// 手写 Serialize 追加派生字段 version（内容哈希短码）；不改结构体形状，
/// 兼容既有构造点（如 engine.rs 测试里的 FakeResolver）
impl Serialize for ScriptFile {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("ScriptFile", 6)?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("package", &self.package)?;
        st.serialize_field("name", &self.name)?;
        st.serialize_field("content", &self.content)?;
        st.serialize_field("updated_at", &self.updated_at)?;
        st.serialize_field("version", &self.version())?;
        st.end()
    }
}

/// 磁盘上的一个函数库文件（data/<pkg>/func/<文件短路径>.yaml；顶层键 = 函数名）
#[derive(Debug, Clone, Serialize)]
pub struct FunctionFile {
    /// `<pkg>/<文件短路径>.yaml`（含 `/`，前端拼 URL 必须整体 encodeURIComponent）
    pub id: String,
    /// 应用分区
    pub pkg: String,
    /// 文件短路径（不含 .yaml 扩展名，如 `common`；契约 FunctionLibraryModel.file）
    pub file: String,
    pub content: String,
    /// 内容版本短码（内容哈希，PUT/POST expected_version 冲突检测依据）
    pub version: String,
    /// 顶层函数名清单（按书写顺序；文件 YAML 非法时为空——列表仍可见便于修复）
    pub functions: Vec<String>,
    pub updated_at: String,
}

/// 内容版本短码：SHA-256 前 12 位 hex（ETag 语义；内容不变 → 版本不变）
pub fn content_version(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(content.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..12].to_string()
}

/// 校验路径部件（应用包名 / 脚本文件名）：
/// 允许 unicode 字母数字与 `. - _`；禁止空、路径分隔符、`..`、前导点
pub fn sanitize_part(s: &str) -> Option<String> {
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
        .any(|c| !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_')))
    {
        return None;
    }
    Some(t.to_string())
}

/// 校验模板文件名：允许 unicode 字母数字与 `. - _ #`（模板名可带 #x1_y1_x2_y2 区域后缀）、空格
fn sanitize_template_name(s: &str) -> Option<String> {
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

/// Windows 会把这些名字（包括带扩展名的形式）解析为设备文件；统一拒绝
/// 可移植存储中对应的 basename，避免 Linux 上创建后在 Windows 产生歧义。
fn is_windows_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn parse_script_id(id: &str) -> Option<(String, String)> {
    let (pkg, name) = id.split_once('/')?;
    Some((sanitize_part(pkg)?, sanitize_part(name)?))
}

/// 规范化脚本文件名（save 与版本冲突检测共用）：trim + sanitize + 缺扩展名补 .yaml
fn normalize_script_name(name_raw: &str) -> anyhow::Result<String> {
    let mut name = sanitize_part(name_raw.trim())
        .ok_or_else(|| anyhow::anyhow!("脚本名非法（只允许字母数字 . _ -）: {name_raw}"))?;
    let low = name.to_lowercase();
    if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
        name.push_str(".yaml");
    }
    Ok(name)
}

/// 函数名合法保留字清单：动作键 + 结构键 + 已删除的旧动作名（与 engine.rs
/// func 段的 RESERVED 对齐，并纳入新契约 v2 新增动作/结构键——函数调用写
/// `- 函数名:` 与步骤动作键同空间，撞名会让脚本无法解析）
const RESERVED_FUNCTION_NAMES: [&str; 32] = [
    "log",
    "key",
    "text",
    "tap",
    "swipe",
    "find",
    "match",
    "color",
    "loop",
    "call",
    "throw",
    "str_app",
    "cls_app",
    "wait",
    "return",
    "then",
    "else",
    "steps",
    "times",
    "block",
    "verify",
    "timeout",
    "config",
    "func",
    "params",
    "args",
    "expect",
    "candidates",
    "if",
    "until",
    "cond",
    "exit",
];

/// 函数名合法性：`[A-Za-z_][A-Za-z0-9_]*` 且非保留字（完整校验归阶段 2）
pub fn validate_function_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let ok = match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    };
    if !ok {
        return Err(format!("函数名 {name} 不符合 [A-Za-z_][A-Za-z0-9_]*"));
    }
    if RESERVED_FUNCTION_NAMES.contains(&name) {
        return Err(format!("函数名 {name} 是保留字（动作键 / 结构键）"));
    }
    Ok(())
}

/// 函数文件浅校验（阶段 1）：合法 YAML、顶层为映射、每个顶层键是合法函数名。
/// 返回按书写顺序的函数名清单。函数记录内部结构（params/steps）归阶段 2 严格校验。
pub fn validate_function_file(content: &str) -> Result<Vec<String>, String> {
    let v: serde_yaml::Value =
        serde_yaml::from_str(content).map_err(|e| format!("YAML 语法错误: {e}"))?;
    let serde_yaml::Value::Mapping(map) = v else {
        return Err("函数文件顶层必须是映射（顶层键 = 函数名）".to_string());
    };
    if map.is_empty() {
        return Err("函数文件没有定义任何函数".to_string());
    }
    let mut names = Vec::with_capacity(map.len());
    for (k, _) in &map {
        let name = k
            .as_str()
            .ok_or_else(|| format!("顶层键 {k:?} 不是字符串标量，不能作为函数名"))?
            .trim();
        validate_function_name(name)?;
        names.push(name.to_string());
    }
    Ok(names)
}

/// 脚本文件浅校验（分区导入 dry-run 用）：合法 YAML + 顶层结构。
/// 新契约顶层键 = params/config/steps（CONTRACT §3.2）；过渡期兼容 v1 引擎的
/// func 段与顶层序列简写（存量分区脚本仍在 v1 语法上运行，阶段 2 收紧）。
pub fn validate_script_file_shallow(content: &str) -> Result<(), String> {
    let v: serde_yaml::Value =
        serde_yaml::from_str(content).map_err(|e| format!("YAML 语法错误: {e}"))?;
    match &v {
        // v1 简写：顶层序列 = steps
        serde_yaml::Value::Sequence(_) => Ok(()),
        serde_yaml::Value::Mapping(map) => {
            for (k, _) in map {
                let key = k.as_str().unwrap_or_default();
                if !matches!(key, "params" | "config" | "steps" | "func") {
                    return Err(format!(
                        "未知顶层键 {key:?}（只允许 params/config/steps；过渡期兼容 func）"
                    ));
                }
            }
            Ok(())
        }
        _ => Err("脚本顶层必须是映射或步骤序列".to_string()),
    }
}

/// 相对短路径分段校验（脚本/函数路径 resolver 共用）：
/// 拒绝空串、反斜杠、空段（`a//b`、前导/尾随 `/`）、`.`、`..`、前导点与非法字符
/// （逐段过 [`sanitize_part`]，含 Windows 保留名）；绝对路径（`/x`、`C:x`）被
/// 空段/非法字符规则覆盖。
pub(crate) fn sanitize_rel_segments(rel: &str) -> anyhow::Result<Vec<String>> {
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

pub struct ScriptStore {
    /// 数据根目录（data/），一级子目录 = 应用分区（内含 yaml/ 与 tmpl/）
    root: PathBuf,
}

impl ScriptStore {
    pub fn open(cfg: &Config) -> anyhow::Result<Self> {
        let store = Self {
            root: cfg.data_dir.clone(),
        };
        store.cleanup_staging();
        Ok(store)
    }

    /// 清理上次进程异常退出留下的导入 staging 目录。目录名带随机 UUID，
    /// 只匹配本服务自己的前缀，不触碰用户数据目录中的其他内容。
    fn cleanup_staging(&self) {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".gamer-staging-") && entry.path().is_dir() {
                if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                    tracing::warn!(staging = %name, error = %e, "清理残留导入 staging 失败");
                }
            }
        }
    }

    /// 分区 yaml 脚本目录
    pub fn yaml_dir(&self, pkg: &str) -> PathBuf {
        self.partition_dir(pkg).join("yaml")
    }

    /// 分区函数库目录（阶段 1 新增：data/<pkg>/func/，顶层键 = 函数名）
    pub fn func_dir(&self, pkg: &str) -> PathBuf {
        self.partition_dir(pkg).join("func")
    }

    /// 分区模板目录
    pub fn tmpl_dir(&self, pkg: &str) -> PathBuf {
        self.partition_dir(pkg).join("tmpl")
    }

    /// 返回位于数据根内的分区目录。公共路径构造器没有 Result 返回值，故对
    /// 非法分区名映射到不可枚举的哨兵目录，避免任何调用方意外逃出 root。
    fn partition_dir(&self, pkg: &str) -> PathBuf {
        sanitize_part(pkg)
            .map(|pkg| self.root.join(pkg))
            .unwrap_or_else(|| self.root.join(".gamer-invalid-partition"))
    }

    /// 磁盘上全部分区名（存在 yaml/ func/ tmpl/ 子目录的一级目录，字典序）
    pub fn partitions(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.root) {
            for d in rd.flatten() {
                let p = d.path();
                let Some(name) = sanitize_part(&d.file_name().to_string_lossy()) else {
                    continue;
                };
                if p.is_dir()
                    && (p.join("yaml").is_dir()
                        || p.join("func").is_dir()
                        || p.join("tmpl").is_dir())
                {
                    out.push(name);
                }
            }
        }
        out.sort();
        out
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

    fn load_file(&self, pkg: &str, name: &str) -> Option<ScriptFile> {
        let p = self.yaml_dir(pkg).join(name);
        if !p.is_file() {
            return None;
        }
        let content = std::fs::read_to_string(&p).ok()?;
        Some(ScriptFile {
            id: format!("{pkg}/{name}"),
            package: pkg.to_string(),
            name: name.to_string(),
            content,
            updated_at: Self::fmt_mtime(&p),
        })
    }

    /// 列出全部脚本（按修改时间倒序，与旧 DB 版行为一致）
    pub fn list(&self) -> anyhow::Result<Vec<ScriptFile>> {
        let mut out = Vec::new();
        for pkg in self.partitions() {
            let Ok(rd) = std::fs::read_dir(self.yaml_dir(&pkg)) else {
                continue;
            };
            for f in rd.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                let low = name.to_lowercase();
                if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
                    continue;
                }
                if let Some(s) = self.load_file(&pkg, &name) {
                    out.push(s);
                }
            }
        }
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(out)
    }

    pub fn get(&self, id: &str) -> anyhow::Result<Option<ScriptFile>> {
        let Some((pkg, rel)) = id.split_once('/') else {
            return Ok(None);
        };
        // 非法路径（穿越/扩展名错配等）与文件不存在同样返回 None
        let Ok(path) = self.resolve_script_path(pkg, rel) else {
            return Ok(None);
        };
        if !path.is_file() {
            return Ok(None);
        }
        let name = path
            .strip_prefix(self.yaml_dir(pkg))
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        Ok(self.load_file(pkg, &name))
    }

    /// 保存脚本到指定应用分区，name 缺扩展名时补 .yaml；
    /// old_id 存在且归档位置变化时移动（删旧文件）。返回落盘后的脚本。
    pub fn save(
        &self,
        old_id: Option<&str>,
        pkg: &str,
        name: &str,
        content: &str,
    ) -> anyhow::Result<ScriptFile> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {}", pkg))?;
        let name = normalize_script_name(name)?;
        let old = match old_id {
            Some(id) => {
                Some(parse_script_id(id).ok_or_else(|| anyhow::anyhow!("非法脚本 id: {}", id))?)
            }
            None => None,
        };
        let dir = self.yaml_dir(&package);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(&name);
        atomic_write(&path, content.as_bytes())?;
        let new_id = format!("{}/{}", package, name);
        if let Some((opkg, oname)) = old {
            let old_id = format!("{}/{}", opkg, oname);
            if old_id != new_id {
                let old_path = self.yaml_dir(&opkg).join(&oname);
                if old_path != path && old_path.is_file() {
                    std::fs::remove_file(&old_path)?;
                    self.cleanup_partition(&opkg);
                }
            }
        }
        Ok(ScriptFile {
            id: new_id,
            package,
            name,
            content: content.to_string(),
            updated_at: Self::fmt_mtime(&path),
        })
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        let Some((pkg, rel)) = id.split_once('/') else {
            anyhow::bail!("非法脚本 id: {}", id);
        };
        let path = self
            .resolve_script_path(pkg, rel)
            .map_err(|_| anyhow::anyhow!("非法脚本 id: {}", id))?;
        std::fs::remove_file(&path)
            .map_err(|e| anyhow::anyhow!("删除失败: {} ({})", e, path.display()))?;
        self.cleanup_partition(pkg);
        Ok(())
    }

    // ---------- 三套路径解析（阶段 1：目录即类型，互不回退、不做内容推断） ----------
    //
    // 契约 §3.1：yaml/ 可执行脚本、func/ 函数库、tmpl/ 模板；跨分区一律不解析、
    // 不回退；模板短名只允许 tmpl/ 现存文件。解析失败统一 Err（不猜测、不回退）。

    /// 脚本相对路径 → 磁盘路径（分区 `yaml/` 内，.yaml/.yml——.yml 为存量存档扩展名，
    /// 阶段 2 收紧为 .yaml 时只改此处）。拒绝绝对路径/反斜杠/空段/`.`/`..`/扩展名错配；
    /// func/、tmpl/ 下的同名文件对脚本解析不可见。
    pub fn resolve_script_path(&self, pkg: &str, rel: &str) -> anyhow::Result<PathBuf> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let segs = sanitize_rel_segments(rel)?;
        let last = segs.last().expect("分段结果非空");
        let low = last.to_lowercase();
        if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
            anyhow::bail!("脚本必须是 .yaml/.yml 且位于分区 yaml/ 目录: {rel}");
        }
        let mut p = self.yaml_dir(&package);
        for s in &segs {
            p.push(s);
        }
        Ok(p)
    }

    /// 函数文件相对路径 → 磁盘路径（分区 `func/` 内，严格 .yaml——新契约函数库
    /// 无 .yml 形态）。规则同 resolve_script_path。
    pub fn resolve_function_path(&self, pkg: &str, rel: &str) -> anyhow::Result<PathBuf> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let segs = sanitize_rel_segments(rel)?;
        let last = segs.last().expect("分段结果非空");
        if !last.to_lowercase().ends_with(".yaml") {
            anyhow::bail!("函数文件必须是 .yaml 且位于分区 func/ 目录: {rel}");
        }
        let mut p = self.func_dir(&package);
        for s in &segs {
            p.push(s);
        }
        Ok(p)
    }

    /// 模板短名/完整名 → 分区 `tmpl/` 内**现存**文件的路径。
    /// 精确名优先；否则按「基名 + `#` 后缀 + 同扩展名」唯一匹配（与引擎
    /// resolve_template_file 同语义）；零候选/多候选均报错，不猜测、不跨目录回退。
    pub fn resolve_template_path(&self, pkg: &str, short: &str) -> anyhow::Result<PathBuf> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        if short.contains('\\') || short.contains('/') {
            anyhow::bail!("模板名不允许路径分隔符: {short:?}");
        }
        let name =
            sanitize_template_name(short).ok_or_else(|| anyhow::anyhow!("模板名非法: {short}"))?;
        let dir = self.tmpl_dir(&package);
        let exact = dir.join(&name);
        if exact.is_file() {
            return Ok(exact);
        }
        let Some((base, ext)) = name.rsplit_once('.') else {
            anyhow::bail!("模板 {name} 不存在 (path={})", exact.display());
        };
        let prefix = format!("{}#", base.to_ascii_lowercase());
        let dotted = format!(".{}", ext.to_ascii_lowercase());
        let mut candidates: Vec<String> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| {
                let lower = n.to_ascii_lowercase();
                lower.starts_with(&prefix) && lower.ends_with(&dotted)
            })
            .collect();
        candidates.sort();
        match candidates.len() {
            1 => Ok(dir.join(&candidates[0])),
            0 => anyhow::bail!("模板 {name} 不存在 (path={})", exact.display()),
            _ => anyhow::bail!(
                "模板 {name} 匹配到多个候选：{}，请用完整文件名指定",
                candidates.join("、")
            ),
        }
    }

    // ---------- 函数库存储（阶段 1：data/<pkg>/func/，文件顶层键 = 函数名） ----------

    /// 规范化函数文件短路径（save 与版本冲突检测共用）：trim，去掉尾部 .yaml/.yml
    /// 后统一补 .yaml；返回（文件短路径, 相对 func/ 的磁盘路径）。
    /// 阶段 1 存储扁平（列表/导出均单层），短路径不支持子目录。
    fn normalize_function_rel(name_raw: &str) -> anyhow::Result<(String, String)> {
        let t = name_raw.trim();
        let lower = t.to_lowercase();
        let base = if lower.ends_with(".yaml") {
            &t[..t.len() - 5]
        } else if lower.ends_with(".yml") {
            &t[..t.len() - 4]
        } else {
            t
        };
        let rel = format!("{base}.yaml");
        let segs = sanitize_rel_segments(&rel)?;
        if segs.len() > 1 {
            anyhow::bail!("函数文件短路径暂不支持子目录: {name_raw}");
        }
        Ok((base.to_string(), rel))
    }

    /// 保存函数库文件（upsert，语义对齐脚本 save）：先浅校验内容（合法 YAML +
    /// 顶层键全是合法函数名），失败不落盘；再同目录临时文件原子写盘。
    pub fn save_function(
        &self,
        pkg: &str,
        name: &str,
        content: &str,
    ) -> anyhow::Result<FunctionFile> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let functions = validate_function_file(content).map_err(anyhow::Error::msg)?;
        let (file, rel) = Self::normalize_function_rel(name)?;
        let path = self.func_dir(&package).join(&rel);
        atomic_write(&path, content.as_bytes())?;
        Ok(FunctionFile {
            id: format!("{package}/{rel}"),
            pkg: package,
            file,
            content: content.to_string(),
            version: content_version(content),
            functions,
            updated_at: Self::fmt_mtime(&path),
        })
    }

    fn load_function_at(&self, pkg: &str, rel: &str, file: &str) -> Option<FunctionFile> {
        let p = self.func_dir(pkg).join(rel);
        if !p.is_file() {
            return None;
        }
        let content = std::fs::read_to_string(&p).ok()?;
        let version = content_version(&content);
        // 列表/读取对浅校验失败保持可见（functions 置空），便于编辑器打开修复
        let functions = validate_function_file(&content).unwrap_or_default();
        Some(FunctionFile {
            id: format!("{pkg}/{rel}"),
            pkg: pkg.to_string(),
            file: file.to_string(),
            content,
            version,
            functions,
            updated_at: Self::fmt_mtime(&p),
        })
    }

    /// 列出分区全部函数库文件（文件短路径 + 顶层函数名清单；按修改时间倒序，
    /// 与脚本列表一致）。只认 func/ 下 .yaml 文件，不与脚本/模板混列。
    pub fn list_functions(&self, pkg: &str) -> anyhow::Result<Vec<FunctionFile>> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.func_dir(&package)) {
            for f in rd.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                if !(f.path().is_file() && name.to_lowercase().ends_with(".yaml")) {
                    continue;
                }
                let file = name[..name.len() - 5].to_string(); // 去掉 ".yaml"
                if let Some(v) = self.load_function_at(&package, &name, &file) {
                    out.push(v);
                }
            }
        }
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(out)
    }

    pub fn get_function(&self, id: &str) -> anyhow::Result<Option<FunctionFile>> {
        let Some((pkg, rel)) = id.split_once('/') else {
            return Ok(None);
        };
        // 非法路径与文件不存在同样返回 None
        let Ok(path) = self.resolve_function_path(pkg, rel) else {
            return Ok(None);
        };
        if !path.is_file() {
            return Ok(None);
        }
        let name = path
            .strip_prefix(self.func_dir(pkg))
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        let file = name.strip_suffix(".yaml").unwrap_or(&name).to_string();
        Ok(self.load_function_at(pkg, &name, &file))
    }

    /// 保存前冲突检测：目标函数文件当前内容版本（不存在 → None）
    pub fn function_version_for_save(
        &self,
        pkg: &str,
        name: &str,
    ) -> anyhow::Result<Option<String>> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let (file, rel) = Self::normalize_function_rel(name)?;
        Ok(self
            .load_function_at(&package, &rel, &file)
            .map(|f| f.version))
    }

    pub fn delete_function(&self, id: &str) -> anyhow::Result<()> {
        let Some((pkg, rel)) = id.split_once('/') else {
            anyhow::bail!("非法函数文件 id: {}", id);
        };
        let path = self
            .resolve_function_path(pkg, rel)
            .map_err(|_| anyhow::anyhow!("非法函数文件 id: {}", id))?;
        std::fs::remove_file(&path)
            .map_err(|e| anyhow::anyhow!("删除失败: {} ({})", e, path.display()))?;
        self.cleanup_partition(pkg);
        Ok(())
    }

    /// 旧分区 yaml/func/tmpl 都已空时删掉分区目录（避免残留空目录被当成有效分区）
    pub fn cleanup_partition(&self, pkg: &str) {
        let _ = std::fs::remove_dir(self.yaml_dir(pkg)); // 非空时失败，忽略
        let _ = std::fs::remove_dir(self.func_dir(pkg));
        let _ = std::fs::remove_dir(self.tmpl_dir(pkg));
        let _ = std::fs::remove_dir(self.root.join(pkg));
    }

    /// call 子脚本按名解析：优先调用者同分区，其次跨分区；
    /// 名字缺 .yaml/.yml 扩展名时自动补全再试（call 写 `子脚本` 或 `子脚本.yml` 均可）
    pub fn resolve_call(&self, caller_pkg: &str, name: &str) -> anyhow::Result<Option<ScriptFile>> {
        let all = self.list()?;
        if let Some(i) = all
            .iter()
            .position(|s| s.package == caller_pkg && s.name == name)
        {
            let mut all = all;
            return Ok(Some(all.swap_remove(i)));
        }
        if let Some(i) = all.iter().position(|s| s.name == name) {
            let mut all = all;
            return Ok(Some(all.swap_remove(i)));
        }
        let low = name.to_lowercase();
        if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
            for ext in [".yaml", ".yml"] {
                let with_ext = format!("{}{}", name, ext);
                if let Some(i) = all.iter().position(|s| s.name == with_ext) {
                    let mut all = all;
                    return Ok(Some(all.swap_remove(i)));
                }
            }
        }
        Ok(None)
    }

    /// 导出整分区快照 zip：yaml/ 全部脚本 + tmpl/ 全部模板 → zip 字节。
    /// 分区没有任何可导出文件时报错。返回（建议文件名, zip 字节）。
    pub fn export_partition(&self, pkg: &str) -> anyhow::Result<(String, Vec<u8>)> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {}", pkg))?;
        // 收集规则与导入校验一致：yaml 只认 .yaml/.yml，tmpl 全部非隐藏文件
        let mut yaml_files: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.yaml_dir(&package)) {
            for f in rd.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                let low = name.to_lowercase();
                if f.path().is_file()
                    && sanitize_part(&name).is_some()
                    && (low.ends_with(".yaml") || low.ends_with(".yml"))
                {
                    yaml_files.push(name);
                }
            }
        }
        let mut tmpl_files: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.tmpl_dir(&package)) {
            for f in rd.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                if f.path().is_file()
                    && !name.starts_with('.')
                    && sanitize_template_name(&name).is_some()
                {
                    tmpl_files.push(name);
                }
            }
        }
        if yaml_files.is_empty() && tmpl_files.is_empty() {
            anyhow::bail!("分区 {} 没有可导出的脚本/模板", package);
        }
        yaml_files.sort();
        tmpl_files.sort();
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for name in &yaml_files {
                zw.start_file(format!("yaml/{}", name), opts)?;
                zw.write_all(&std::fs::read(self.yaml_dir(&package).join(name))?)?;
            }
            for name in &tmpl_files {
                zw.start_file(format!("tmpl/{}", name), opts)?;
                zw.write_all(&std::fs::read(self.tmpl_dir(&package).join(name))?)?;
            }
            zw.finish()?;
        }
        Ok((format!("{}.zip", package), buf))
    }

    /// 导入分区快照 zip 到指定应用分区。confirm=false 时只解析并报告同名冲突
    /// （前端二次确认），confirm=true 时落盘（同名替换）。只认 yaml/ 与 tmpl/ 布局。
    ///
    /// 资源硬限（阶段 2 SEC-004，传输层另有 20MiB body 闸门）：
    /// - 条目数 ≤ [`IMPORT_MAX_ENTRIES`]；
    /// - 总解压量 ≤ [`IMPORT_MAX_TOTAL_BYTES`]——条目声明尺寸预检 + 实际读取计数
    ///   双保险，防"声明造假"（压缩炸弹以小博大）；
    /// - 单 YAML ≤ 1MiB、单模板 ≤ 10MiB（按实际读取字节判定，声明只做预检参考）；
    /// - zip-slip 由 `enclosed_name` 拒绝绝对路径与 `..`；目录条目不计入限额。
    pub fn import(&self, bytes: &[u8], pkg: &str, confirm: bool) -> anyhow::Result<ImportReport> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {}", pkg))?;
        if bytes.len() > IMPORT_MAX_ARCHIVE_BYTES {
            anyhow::bail!(
                "压缩包 {} 字节超过上限 {} MiB",
                bytes.len(),
                IMPORT_MAX_ARCHIVE_BYTES / (1024 * 1024)
            );
        }
        let mut rep = ImportReport::default();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        // 预检 ① 条目数（含目录条目——给攻击者的预算更紧，合法包不受影响）
        if archive.len() > IMPORT_MAX_ENTRIES {
            anyhow::bail!("包内条目数 {} 超过上限 {IMPORT_MAX_ENTRIES}", archive.len());
        }
        // 预检 ② 声明解压总量（不可信值，仅作快速拒绝大头；真实防线在实际读取计数）
        let mut declared_total: u64 = 0;
        for i in 0..archive.len() {
            declared_total = declared_total.saturating_add(archive.by_index(i)?.size());
            if declared_total > IMPORT_MAX_TOTAL_BYTES as u64 {
                anyhow::bail!(
                    "声明解压总量超过上限（>{} MiB），疑似压缩炸弹",
                    IMPORT_MAX_TOTAL_BYTES / (1024 * 1024)
                );
            }
        }
        // 全部解析到内存（zip-slip 防护 + 布局校验 + 实际读取计数），无错才考虑落盘
        let mut actual_total: usize = 0;
        let mut materialized_total: usize = 0;
        let mut seen_paths = std::collections::HashSet::new();
        let mut files: Vec<(String, PathBuf, Vec<u8>)> = Vec::new();
        for i in 0..archive.len() {
            let mut f = archive.by_index(i)?;
            if f.is_dir() {
                continue;
            }
            // enclosed_name 拒绝绝对路径与 ..（zip-slip）
            let Some(rel) = f.enclosed_name() else {
                anyhow::bail!("包内路径非法: {}", f.name());
            };
            let comps: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect();
            let (cap, zip_path, dest): (usize, String, PathBuf) = match comps.as_slice() {
                [y, name] if y == "yaml" => {
                    let name = sanitize_part(name)
                        .ok_or_else(|| anyhow::anyhow!("脚本名非法: {}", name))?;
                    let low = name.to_lowercase();
                    if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
                        anyhow::bail!("yaml/ 下只支持 .yaml/.yml 文件: {}", name);
                    }
                    (
                        IMPORT_MAX_YAML_BYTES,
                        format!("yaml/{}", name),
                        self.yaml_dir(&package).join(&name),
                    )
                }
                [t, name] if t == "tmpl" => {
                    let name = sanitize_template_name(name)
                        .ok_or_else(|| anyhow::anyhow!("模板名非法: {}", name))?;
                    (
                        IMPORT_MAX_TMPL_BYTES,
                        format!("tmpl/{}", name),
                        self.tmpl_dir(&package).join(&name),
                    )
                }
                _ => anyhow::bail!("包内路径需为 yaml/<脚本> 或 tmpl/<模板>: {}", f.name()),
            };
            if f.size() > cap as u64 {
                anyhow::bail!(
                    "{zip_path} 声明解压后 {} 字节超限（该类文件上限 {} MiB）",
                    f.size(),
                    cap / (1024 * 1024)
                );
            }
            if !seen_paths.insert(zip_path.to_ascii_lowercase()) {
                anyhow::bail!("包内存在重复文件: {zip_path}");
            }
            // 双保险之实际读取计数：按 cap 截读，多读 1 字节即暴露超限/声明造假
            let cap_display_mib = cap / (1024 * 1024);
            let mut buf = Vec::new();
            (&mut f).take(cap as u64 + 1).read_to_end(&mut buf)?;
            if buf.len() > cap {
                anyhow::bail!(
                    "{zip_path} 解压后 {bytes} 字节超限（该类文件上限 {cap_display_mib} MiB）",
                    bytes = buf.len()
                );
            }
            actual_total = actual_total.saturating_add(buf.len());
            if actual_total > IMPORT_MAX_TOTAL_BYTES {
                anyhow::bail!(
                    "总解压量超过上限（>{} MiB），中止导入",
                    IMPORT_MAX_TOTAL_BYTES / (1024 * 1024)
                );
            }
            // ZIP 内模板不能绕过 HTTP 上传的图片安全闸门：在任何落盘前
            // 用同一套字节/尺寸/像素限额解码，并统一归一化为灰度 PNG。
            // 否则一个 10MiB 以内的像素炸弹会在后续匹配时才触发高额分配。
            let buf = if zip_path.starts_with("tmpl/") {
                crate::matcher::reencode_template_gray_png(&buf)
                    .map_err(|e| anyhow::anyhow!("{zip_path} 模板校验失败: {e}"))?
            } else {
                buf
            };
            if buf.len() > cap {
                anyhow::bail!(
                    "{zip_path} 归一化后 {bytes} 字节超限（该类文件上限 {cap_display_mib} MiB）",
                    bytes = buf.len()
                );
            }
            materialized_total = materialized_total.saturating_add(buf.len());
            if materialized_total > IMPORT_MAX_TOTAL_BYTES {
                anyhow::bail!(
                    "归一化后总数据量超过上限（>{} MiB），中止导入",
                    IMPORT_MAX_TOTAL_BYTES / (1024 * 1024)
                );
            }
            files.push((zip_path, dest, buf));
        }
        if files.is_empty() {
            anyhow::bail!("包内没有可导入的文件");
        }
        for (zip_path, dest, _) in &files {
            rep.entries.push(zip_path.clone());
            if dest.exists() {
                rep.conflicts.push(zip_path.clone());
            }
        }
        if !confirm {
            return Ok(rep);
        }
        // 先把全部内容写入同文件系统 staging，再逐文件提交；提交失败时利用备份
        // 回滚已经替换的文件，避免留下半导入状态。staging 目录由 open() 清理残留。
        let staging = self
            .root
            .join(format!(".gamer-staging-{}", uuid::Uuid::new_v4().simple()));
        let stage_data = staging.join("data");
        let stage_backup = staging.join("backup");
        let mut staged = Vec::with_capacity(files.len());
        let stage_result = (|| -> anyhow::Result<()> {
            for (zip_path, dest, buf) in files {
                let relative = dest
                    .strip_prefix(&self.root)
                    .map_err(|_| anyhow::anyhow!("导入目标路径不在数据目录内"))?;
                let stage_path = stage_data.join(relative);
                atomic_write(&stage_path, &buf)?;
                staged.push((zip_path, dest, stage_path));
            }
            Ok(())
        })();
        if let Err(e) = stage_result {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }

        let mut committed: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
        for (zip_path, dest, stage_path) in staged {
            let backup = if dest.exists() {
                let relative = dest
                    .strip_prefix(&self.root)
                    .map_err(|_| anyhow::anyhow!("导入目标路径不在数据目录内"))?;
                let path = stage_backup.join(relative);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                if let Err(e) = std::fs::rename(&dest, &path) {
                    rollback_import(&committed);
                    let _ = std::fs::remove_dir_all(&staging);
                    return Err(e.into());
                }
                Some(path)
            } else {
                None
            };
            let data = match std::fs::read(&stage_path) {
                Ok(data) => data,
                Err(e) => {
                    restore_import_file(&dest, backup.as_deref());
                    rollback_import(&committed);
                    let _ = std::fs::remove_dir_all(&staging);
                    return Err(e.into());
                }
            };
            if let Err(e) = atomic_write(&dest, &data) {
                restore_import_file(&dest, backup.as_deref());
                rollback_import(&committed);
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
            committed.push((dest, backup));
            if rep.conflicts.iter().any(|c| c == &zip_path) {
                rep.replaced.push(zip_path);
            } else {
                rep.imported.push(zip_path);
            }
        }
        if let Err(e) = std::fs::remove_dir_all(&staging) {
            tracing::warn!(error = %e, "导入 staging 清理失败，将在下次启动时清理");
        }
        Ok(rep)
    }
} // impl ScriptStore

fn restore_import_file(dest: &Path, backup: Option<&Path>) {
    let _ = std::fs::remove_file(dest);
    if let Some(backup) = backup {
        let _ = std::fs::rename(backup, dest);
    }
}

fn rollback_import(committed: &[(PathBuf, Option<PathBuf>)]) {
    for (dest, backup) in committed.iter().rev() {
        restore_import_file(dest, backup.as_deref());
    }
}
/// 导入结果报告
#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    /// 包内全部条目（zip 相对路径）
    pub entries: Vec<String>,
    /// 与现有文件同名、将被替换的条目（confirm=false 时的提示依据）
    pub conflicts: Vec<String>,
    /// 实际新增（confirm=true）
    pub imported: Vec<String>,
    /// 实际替换（confirm=true）
    pub replaced: Vec<String>,
}

/// 剥离旧 `package <名字>` 指令行（仅 migrate_fs_layout 清理旧文件残留用，
/// 运行时已不识别该指令——残留行会导致 YAML 解析报错）
fn strip_directive_line(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut stripped = false;
    for line in content.lines() {
        let t = line.trim();
        let is_directive = !stripped
            && !t.is_empty()
            && !t.starts_with('#')
            && t.strip_prefix("package")
                .map(|r| r.starts_with(' ') || r.starts_with('\t'))
                .unwrap_or(false);
        if is_directive {
            stripped = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// 目录内（含子目录）是否有文件
fn dir_has_any_file(dir: &std::path::Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    for e in rd.flatten() {
        if e.path().is_file() {
            return true;
        }
        if e.path().is_dir() && dir_has_any_file(&e.path()) {
            return true;
        }
    }
    false
}

/// 一次性迁移：旧布局（data/scripts/<package>/ + data/templates/）→ 应用分区
/// （data/<目标pkg>/yaml|tmpl）。目标 = DB 首个配置了应用包名的设备；
/// 无设备 pkg 或目标分区已有数据时跳过（旧目录保留）。脚本内容顺手剥离旧指令行。
pub fn migrate_fs_layout(db: &crate::store::Store, store: &ScriptStore) -> anyhow::Result<()> {
    let old_scripts = store.root.join("scripts");
    let old_templates = store.root.join("templates");
    if !dir_has_any_file(&old_scripts) && !dir_has_any_file(&old_templates) {
        return Ok(());
    }
    let Some(pkg) = db
        .list_devices()?
        .into_iter()
        .filter_map(|d| d.pkg)
        .map(|p| p.trim().to_string())
        .find(|p| !p.is_empty())
    else {
        tracing::warn!("存在旧布局脚本/模板但无设备配置应用包名，跳过迁移（旧目录保留）");
        return Ok(());
    };
    let target_yaml = store.yaml_dir(&pkg);
    let target_tmpl = store.tmpl_dir(&pkg);
    if dir_has_any_file(&target_yaml) || dir_has_any_file(&target_tmpl) {
        tracing::warn!(pkg = %pkg, "目标分区已有数据，跳过旧布局迁移（旧目录保留）");
        return Ok(());
    }
    std::fs::create_dir_all(&target_yaml)?;
    std::fs::create_dir_all(&target_tmpl)?;
    if old_scripts.is_dir() {
        for pkg_dir in std::fs::read_dir(&old_scripts)?.flatten() {
            if !pkg_dir.path().is_dir() {
                continue;
            }
            for f in std::fs::read_dir(pkg_dir.path())?.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                let low = name.to_lowercase();
                if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
                    continue;
                }
                let dest = target_yaml.join(&name);
                if dest.exists() {
                    tracing::warn!(name = %name, "目标已存在同名脚本，跳过（旧文件保留）");
                    continue;
                }
                let content = std::fs::read_to_string(f.path())?;
                atomic_write(&dest, strip_directive_line(&content).as_bytes())?;
                let _ = std::fs::remove_file(f.path());
                tracing::info!(from = %f.path().display(), to = %dest.display(), "脚本迁移至应用分区");
            }
            let _ = std::fs::remove_dir(pkg_dir.path()); // 非空（有跳过文件）时失败，忽略
        }
        let _ = std::fs::remove_dir(&old_scripts);
    }
    if old_templates.is_dir() {
        for f in std::fs::read_dir(&old_templates)?.flatten() {
            if !f.path().is_file() {
                continue;
            }
            let name = f.file_name().to_string_lossy().to_string();
            let dest = target_tmpl.join(&name);
            if dest.exists() {
                tracing::warn!(name = %name, "目标已存在同名模板，跳过（旧文件保留）");
                continue;
            }
            let content = std::fs::read(f.path())?;
            atomic_write(&dest, &content)?;
            let _ = std::fs::remove_file(f.path());
            tracing::info!(from = %f.path().display(), to = %dest.display(), "模板迁移至应用分区");
        }
        let _ = std::fs::remove_dir(&old_templates);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn strip_legacy_directive() {
        assert_eq!(
            strip_directive_line("package test\nsteps:\n  - log x\n"),
            "steps:\n  - log x\n"
        );
        assert_eq!(
            strip_directive_line("# 注释\n\npackage foo\nsteps: []"),
            "# 注释\n\nsteps: []\n"
        );
        // 无指令行原样（补尾部换行）
        assert_eq!(strip_directive_line("steps: []"), "steps: []\n");
    }

    #[test]
    fn atomic_write_replaces_existing_file_without_leftover_temp_files() {
        let (store, dir) = temp_store("atomic");
        let path = dir.join("com.test.app").join("yaml").join("main.yaml");
        atomic_write(&path, b"first\n").unwrap();
        atomic_write(&path, b"second\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second\n");
        let yaml_dir = store.yaml_dir("com.test.app");
        let leftovers: Vec<_> = std::fs::read_dir(yaml_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "临时文件未清理: {leftovers:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn atomic_write_failure_keeps_old_content_and_cleans_temp_file() {
        let (store, dir) = temp_store("atomic-fail");
        let path = dir.join("com.test.app").join("yaml").join("main.yaml");
        atomic_write(&path, b"old\n").unwrap();

        let err = atomic_write_with_replace_err(&path, b"new\n").unwrap_err();
        assert!(err.to_string().contains("replace failure"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old\n");

        let yaml_dir = store.yaml_dir("com.test.app");
        let leftovers: Vec<_> = std::fs::read_dir(yaml_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "失败后临时文件未清理: {leftovers:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn atomic_write_concurrent_writers_replace_with_whole_files_only() {
        let (store, dir) = temp_store("atomic-race");
        let path = dir.join("com.test.app").join("yaml").join("main.yaml");
        atomic_write(&path, b"seed\n").unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let payload_a = b"alpha\nalpha\n".to_vec();
        let payload_b = b"beta\nbeta\nbeta\n".to_vec();

        let mut handles = Vec::new();
        for payload in [payload_a.clone(), payload_b.clone()] {
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
            seen.iter().any(|payload| *payload == content.as_bytes()),
            "并发写入后内容应完整来自某个写者，实际 {content:?}"
        );
        assert!(!content.contains("seed"));
        assert!(!content.contains("alpha\nbeta"));

        let yaml_dir = store.yaml_dir("com.test.app");
        let leftovers: Vec<_> = std::fs::read_dir(yaml_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "并发写入后临时文件未清理: {leftovers:?}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn script_store_open_cleans_stale_import_staging() {
        let dir = std::env::temp_dir().join(format!(
            "gamer-staging-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(dir.join(".gamer-staging-old/data")).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let _store = ScriptStore::open(&cfg).unwrap();
        assert!(!dir.join(".gamer-staging-old").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sanitize() {
        assert_eq!(
            sanitize_part("com.miHoYo.hkrpg").as_deref(),
            Some("com.miHoYo.hkrpg")
        );
        assert_eq!(sanitize_part(""), None);
        assert_eq!(sanitize_part(".."), None);
        assert_eq!(sanitize_part("a/b"), None);
        assert_eq!(sanitize_part("测试_1-2"), Some("测试_1-2".into()));
    }

    // ---------- 导入资源硬限（阶段 2 SEC-004） ----------

    fn temp_store(tag: &str) -> (ScriptStore, std::path::PathBuf) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "gamer-scripttest-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = ScriptStore { root: dir.clone() };
        (store, dir)
    }

    /// 内存构造 zip（Deflated）：name → 原始字节
    fn craft_zip(entries: Vec<(String, Vec<u8>)>) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, data) in entries {
                zw.start_file(name, opts).unwrap();
                zw.write_all(&data).unwrap();
            }
            zw.finish().unwrap();
        }
        buf
    }

    fn valid_template_png() -> Vec<u8> {
        let mut img = image::GrayImage::new(8, 8);
        for (x, y, p) in img.enumerate_pixels_mut() {
            p.0[0] = if (x + y) % 2 == 0 { 32 } else { 224 };
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

    fn pixel_bomb_png(width: u32, height: u32) -> Vec<u8> {
        fn crc32(data: &[u8]) -> u32 {
            let mut crc: u32 = 0xFFFF_FFFF;
            for &b in data {
                crc ^= b as u32;
                for _ in 0..8 {
                    let mask = (crc & 1).wrapping_neg();
                    crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
                }
            }
            !crc
        }
        let mut out = vec![137, 80, 78, 71, 13, 10, 26, 10];
        let ihdr = [
            13u32.to_be_bytes().as_slice(),
            b"IHDR".as_slice(),
            &width.to_be_bytes()[..],
            &height.to_be_bytes()[..],
            &[8u8, 0, 0, 0, 0],
        ]
        .concat();
        out.extend_from_slice(&ihdr);
        out.extend_from_slice(&crc32(&ihdr[4..]).to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(b"IEND");
        out.extend_from_slice(&crc32(b"IEND").to_be_bytes());
        out
    }

    fn expect_import_err(store: &ScriptStore, zip_bytes: &[u8], marker: &str, context: &str) {
        let err = store.import(zip_bytes, "com.test.app", false).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(marker),
            "{context}: 期望错误含 {marker:?}，实际 {msg}"
        );
    }

    #[test]
    fn import_rejects_entry_count_over_500() {
        let (store, _dir) = temp_store("count");
        let entries: Vec<(String, Vec<u8>)> = (0..501)
            .map(|i| (format!("yaml/s{i}.yaml"), b"steps: []\n".to_vec()))
            .collect();
        let z = craft_zip(entries);
        expect_import_err(&store, &z, "条目数", "501 个条目应被拒");
    }

    #[test]
    fn import_rejects_single_yaml_over_1mib_actual_bytes() {
        let (store, _dir) = temp_store("yamlcap");
        // 全零压缩后很小——模拟"声明小/传输小但解压大"的压缩炸弹形态
        let big = vec![0u8; IMPORT_MAX_YAML_BYTES + 1024];
        let z = craft_zip(vec![("yaml/big.yaml".into(), big)]);
        expect_import_err(&store, &z, "超限", "单 YAML 实际解压超 1MiB 应被拒");
    }

    #[test]
    fn import_rejects_single_template_over_10mib_actual_bytes() {
        let (store, _dir) = temp_store("tmplcap");
        let big = vec![0u8; IMPORT_MAX_TMPL_BYTES + 4096];
        let z = craft_zip(vec![("tmpl/bomb.png".into(), big)]);
        expect_import_err(&store, &z, "超限", "单模板实际解压超 10MiB 应被拒");
    }

    #[test]
    fn import_rejects_total_decompressed_budget_breach_mid_read() {
        let (store, _dir) = temp_store("totalcap");
        // 多个 <1MiB YAML 叠加越过 100MiB 总预算：声明总量预检与实际计数
        // 双保险，任一都会拦下；断言不 panic 且报预算类错误即可
        let per = IMPORT_MAX_YAML_BYTES / 4; // 256KiB ×400 = 100MiB+
        let count = 420;
        let small = vec![b'a'; per];
        let entries: Vec<(String, Vec<u8>)> = (0..count)
            .map(|i| (format!("yaml/p{i}.yaml"), small.clone()))
            .collect();
        let z = craft_zip(entries);
        expect_import_err(&store, &z, "上限", "总解压量超预算应被拒");
    }

    #[test]
    fn import_happy_path_under_limits_and_report() {
        let (store, dir) = temp_store("happy");
        let z = craft_zip(vec![
            ("yaml/main.yaml".into(), b"steps:\n  - log ok\n".to_vec()),
            ("tmpl/a#0_0_10_10.png".into(), valid_template_png()),
        ]);
        let rep = store.import(&z, "com.test.app", true).unwrap();
        assert_eq!(rep.imported.len(), 2);
        assert_eq!(rep.conflicts.len(), 0);
        let root: PathBuf = dir.clone();
        assert!(root.join("com.test.app/yaml/main.yaml").is_file());
        assert!(root.join("com.test.app/tmpl/a#0_0_10_10.png").is_file());
        // 再导入一次（confirm 覆盖同名）：全部进 replaced
        let rep2 = store.import(&z, "com.test.app", true).unwrap();
        assert_eq!(rep2.replaced.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_rejects_template_pixel_bomb_before_commit() {
        let (store, dir) = temp_store("tmplpixelbomb");
        let bomb = pixel_bomb_png(30_000, 30_000);
        let z = craft_zip(vec![("tmpl/bomb.png".into(), bomb)]);
        expect_import_err(&store, &z, "像素", "ZIP 模板像素炸弹应在落盘前拒绝");
        assert!(!dir.join("com.test.app/tmpl/bomb.png").exists());
    }

    // ---------- 三套路径解析（阶段 1：路径安全 + 目录即类型 + 不回退） ----------

    #[test]
    fn resolve_script_path_accepts_only_yaml_under_partition() {
        let (store, dir) = temp_store("rspscript");
        let p = store
            .resolve_script_path("com.test.app", "main.yaml")
            .unwrap();
        assert_eq!(p, dir.join("com.test.app").join("yaml").join("main.yaml"));
        // 嵌套短路径（契约 call 目标 sub/inner.yaml 形态）按段解析
        let p = store
            .resolve_script_path("com.test.app", "sub/inner.yaml")
            .unwrap();
        assert_eq!(
            p,
            dir.join("com.test.app")
                .join("yaml")
                .join("sub")
                .join("inner.yaml")
        );
        // .yml 为存量存档扩展名，仍可解析
        assert!(store.resolve_script_path("com.test.app", "a.yml").is_ok());
    }

    #[test]
    fn resolve_script_path_rejects_traversal_and_bad_segments() {
        let (store, _dir) = temp_store("rspscript-bad");
        let bad = [
            "",                   // 空路径
            "/abs.yaml",          // 绝对路径（前导 / = 空段）
            "..",                 // ..
            "../escape.yaml",     // 穿越
            "a/../../b.yaml",     // 中段穿越
            "a//b.yaml",          // 空段
            "a/.yaml",            // 空基名
            ".hidden.yaml",       // 前导点
            "a\\..\\escape.yaml", // 反斜杠
            "main.png",           // 扩展名错配
            "C:/x.yaml",          // Windows 盘符
        ];
        for rel in bad {
            assert!(
                store.resolve_script_path("com.test.app", rel).is_err(),
                "{rel:?} 必须被拒绝"
            );
        }
        // 非法分区名拒绝（不得落到哨兵目录成功返回）
        assert!(store.resolve_script_path("../escape", "a.yaml").is_err());
        assert!(store.resolve_script_path("", "a.yaml").is_err());
    }

    #[test]
    fn resolve_function_path_strict_yaml_and_stays_in_func() {
        let (store, dir) = temp_store("rspfunc");
        let p = store
            .resolve_function_path("com.test.app", "common.yaml")
            .unwrap();
        assert_eq!(p, dir.join("com.test.app").join("func").join("common.yaml"));
        // 函数库严格 .yaml（无 .yml 形态）
        assert!(store
            .resolve_function_path("com.test.app", "a.yml")
            .is_err());
        for rel in ["../escape.yaml", "a\\b.yaml", "a//b.yaml", "common.png", ""] {
            assert!(
                store.resolve_function_path("com.test.app", rel).is_err(),
                "{rel:?} 必须被拒绝"
            );
        }
    }

    #[test]
    fn resolve_template_path_requires_existing_file_in_tmpl_only() {
        let (store, dir) = temp_store("rsptmpl");
        let tmpl_dir = dir.join("com.test.app").join("tmpl");
        std::fs::create_dir_all(&tmpl_dir).unwrap();
        std::fs::write(tmpl_dir.join("login#907_160_973_717.png"), b"png").unwrap();
        std::fs::write(tmpl_dir.join("full.png"), b"png").unwrap();
        // 短名唯一匹配 # 后缀候选
        assert_eq!(
            store
                .resolve_template_path("com.test.app", "login.png")
                .unwrap(),
            tmpl_dir.join("login#907_160_973_717.png")
        );
        // 精确完整名优先
        assert_eq!(
            store
                .resolve_template_path("com.test.app", "full.png")
                .unwrap(),
            tmpl_dir.join("full.png")
        );
        // 不存在 → 错误；歧义 → 错误；路径分隔符 → 错误
        assert!(store
            .resolve_template_path("com.test.app", "nope.png")
            .is_err());
        std::fs::write(tmpl_dir.join("login#a.png"), b"png").unwrap();
        std::fs::write(tmpl_dir.join("login#b.png"), b"png").unwrap();
        let err = store
            .resolve_template_path("com.test.app", "login.png")
            .unwrap_err();
        assert!(err.to_string().contains("多个候选"), "{err}");
        assert!(store
            .resolve_template_path("com.test.app", "a/b.png")
            .is_err());
        assert!(store
            .resolve_template_path("com.test.app", "a\\b.png")
            .is_err());
        // 只认 tmpl/ 现存文件：yaml/ 下同名脚本不影响模板解析（不回退、不内容推断）
        std::fs::create_dir_all(dir.join("com.test.app").join("yaml")).unwrap();
        std::fs::write(
            dir.join("com.test.app").join("yaml").join("shop.png"),
            b"not a template",
        )
        .unwrap();
        assert!(store
            .resolve_template_path("com.test.app", "shop.png")
            .is_err());
        // 跨分区不可见
        std::fs::create_dir_all(dir.join("com.other.app").join("tmpl")).unwrap();
        std::fs::write(dir.join("com.other.app").join("tmpl").join("x.png"), b"png").unwrap();
        assert!(store
            .resolve_template_path("com.test.app", "x.png")
            .is_err());
    }

    #[test]
    fn resolvers_never_fall_back_across_resource_directories() {
        let (store, dir) = temp_store("rspfallback");
        let pkg = dir.join("com.test.app");
        // func/ 里存在 common.yaml，脚本解析不得回退命中（目录即类型）
        std::fs::create_dir_all(pkg.join("func")).unwrap();
        std::fs::write(pkg.join("func").join("common.yaml"), "login:\n  steps: []").unwrap();
        assert!(store
            .resolve_script_path("com.test.app", "common.yaml")
            .is_ok());
        assert!(!store
            .resolve_script_path("com.test.app", "common.yaml")
            .unwrap()
            .is_file());
        // yaml/ 里存在 main.yaml，函数解析不得回退命中
        std::fs::create_dir_all(pkg.join("yaml")).unwrap();
        std::fs::write(pkg.join("yaml").join("main.yaml"), "steps: []").unwrap();
        assert!(!store
            .resolve_function_path("com.test.app", "main.yaml")
            .unwrap()
            .is_file());
    }

    // ---------- 函数库存储 CRUD ----------

    const FUNC_OK: &str =
        "login:\n  steps:\n    - return: true\n\nis_enabled:\n  steps:\n    - return: true\n";

    #[test]
    fn function_crud_roundtrip_with_version_and_names() {
        let (store, _dir) = temp_store("func-crud");
        // create（缺扩展名补 .yaml）
        let f = store
            .save_function("com.test.app", "common", FUNC_OK)
            .unwrap();
        assert_eq!(f.id, "com.test.app/common.yaml");
        assert_eq!(f.file, "common");
        assert_eq!(f.functions, vec!["login", "is_enabled"]); // 顶层键按书写顺序
        assert_eq!(f.version, content_version(FUNC_OK));
        // get
        let g = store
            .get_function("com.test.app/common.yaml")
            .unwrap()
            .unwrap();
        assert_eq!(g.content, FUNC_OK);
        assert_eq!(g.version, f.version);
        // update 覆盖 → version 变化
        let updated = store
            .save_function("com.test.app", "common.yaml", "only:\n  steps: []")
            .unwrap();
        assert_eq!(updated.functions, vec!["only"]);
        assert_ne!(updated.version, f.version);
        // list
        let list = store.list_functions("com.test.app").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].file, "common");
        // delete；再取不存在
        store.delete_function("com.test.app/common.yaml").unwrap();
        assert!(store
            .get_function("com.test.app/common.yaml")
            .unwrap()
            .is_none());
        assert!(store.list_functions("com.test.app").unwrap().is_empty());
        // 删除不存在 → 报错（语义对齐脚本 delete）
        assert!(store.delete_function("com.test.app/common.yaml").is_err());
        assert!(store
            .get_function("com.test.app/missing.yaml")
            .unwrap()
            .is_none());
    }

    #[test]
    fn function_save_shallow_validates_before_writing() {
        let (store, dir) = temp_store("func-invalid");
        let cases: Vec<(&str, &str)> = vec![
            ("YAML 语法错误", "login: [unclosed"),
            ("函数文件顶层必须是映射", "- login\n- logout\n"),
            ("没有定义任何函数", "{}\n"),
            ("不符合 [A-Za-z_][A-Za-z0-9_]*", "1abc:\n  steps: []\n"),
            ("不符合 [A-Za-z_][A-Za-z0-9_]*", "带 空 格:\n  steps: []\n"),
            ("是保留字", "match:\n  steps: []\n"),
            ("是保留字", "return:\n  steps: []\n"),
            ("不是字符串标量", "123:\n  steps: []\n"),
        ];
        for (marker, content) in cases {
            let err = store
                .save_function("com.test.app", "bad.yaml", content)
                .unwrap_err();
            assert!(
                err.to_string().contains(marker),
                "{content:?}: 期望含 {marker:?}，实际 {err}"
            );
        }
        // 失败不留半个文件
        assert!(!dir.join("com.test.app/func/bad.yaml").exists());
        // 非法文件名 / 子目录拒绝
        assert!(store
            .save_function("com.test.app", "../bad.yaml", FUNC_OK)
            .is_err());
        assert!(store
            .save_function("com.test.app", "sub/common.yaml", FUNC_OK)
            .is_err());
        assert!(store
            .save_function("../evil", "common.yaml", FUNC_OK)
            .is_err());
    }

    #[test]
    fn functions_are_partition_scoped_and_hidden_from_script_list() {
        let (store, _dir) = temp_store("func-scope");
        store.save_function("com.a", "common", FUNC_OK).unwrap();
        store
            .save(None, "com.a", "main.yaml", "steps: []\n")
            .unwrap();
        store.save_function("com.b", "common", FUNC_OK).unwrap();

        // 脚本列表只有 yaml/ 脚本，func 文件绝不混入
        let scripts = store.list().unwrap();
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].name, "main.yaml");
        // 函数列表分区隔离
        assert_eq!(store.list_functions("com.a").unwrap().len(), 1);
        assert_eq!(store.list_functions("com.b").unwrap().len(), 1);
        assert!(store.list_functions("com.c").unwrap().is_empty());
        // 跨分区 get 不可见
        assert!(store
            .get_function("com.b/../com.a/common.yaml")
            .unwrap()
            .is_none());
        assert!(store.get_function("com.c/common.yaml").unwrap().is_none());
        // 非法 id
        assert!(store.get_function("no-slash").unwrap().is_none());
        assert!(store.get_function("com.a/common.yml").unwrap().is_none());
    }

    #[test]
    fn content_version_is_stable_short_hash() {
        let v = content_version("steps: []\n");
        assert_eq!(v.len(), 12);
        assert!(v.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(v, content_version("steps: []\n"));
        assert_ne!(v, content_version("steps: []\r\n"));
    }

    #[test]
    fn script_shallow_validation_accepts_contract_and_legacy_v1() {
        // 新契约顶层键
        assert!(validate_script_file_shallow(
            "params: []\nconfig:\n  interval: 500ms\nsteps: []\n"
        )
        .is_ok());
        assert!(validate_script_file_shallow("steps:\n  - log: x\n").is_ok());
        // 过渡期兼容 v1：func 段 / 顶层序列简写
        assert!(validate_script_file_shallow("func:\n  - f1:\n    - log: a\nsteps: []\n").is_ok());
        assert!(validate_script_file_shallow("- log: 顶层序列简写\n").is_ok());
        // 未知顶层键 / 语法错误 / 标量顶层
        assert!(validate_script_file_shallow("steps: []\nname: x\n").is_err());
        assert!(validate_script_file_shallow("action_wait: 1s\nsteps: []\n").is_err());
        assert!(validate_script_file_shallow("steps: [unclosed").is_err());
        assert!(validate_script_file_shallow("42\n").is_err());
    }

    #[test]
    fn function_version_for_save_detects_overwrite_target() {
        let (store, _dir) = temp_store("func-vers");
        let f = store
            .save_function("com.test.app", "common", FUNC_OK)
            .unwrap();
        // 目标存在 → 返回当前版本
        assert_eq!(
            store
                .function_version_for_save("com.test.app", "common")
                .unwrap(),
            Some(f.version)
        );
        // 目标不存在 → None
        assert_eq!(
            store
                .function_version_for_save("com.test.app", "newfile")
                .unwrap(),
            None
        );
    }
}
