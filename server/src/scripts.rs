//! 脚本/函数库/模板文件存储：按应用分区 `data/<pkg>/yaml/` + `data/<pkg>/func/` + `data/<pkg>/tmpl/`
//!
//! 分区 = 设备配置的应用包名（如 com.miHoYo.hkrpg），无 default 兜底；
//! 脚本 id = `<pkg>/<name>.yaml`（含 `/`，前端拼 URL 必须整体 encodeURIComponent）。
//! 旧 `package <名字>` YAML 指令已废除（引擎直接解析 YAML，残留指令行 = 解析报错），
//! 旧目录布局不会自动迁移；`ScriptStore::open` 检测到旧目录时直接失败。
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

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Serialize, Serializer};

use crate::config::Config;

fn format_script_errors(errors: &[crate::script_v2::ScriptError]) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("；")
}

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

/// 模板短名匹配结果（resolve_template_path / template_avail 共用内核）。
enum TplMatch {
    Found(PathBuf),
    NotFound {
        name: String,
        path: PathBuf,
    },
    Ambiguous {
        name: String,
        candidates: Vec<String>,
    },
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
        store.reject_legacy_layout()?;
        store.cleanup_staging();
        Ok(store)
    }

    /// 旧版 scripts/ 与 templates/ 目录属于已删除的数据布局。启动时只
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
                "检测到已废弃的资源目录布局：{}；请备份后删除旧目录并重建开发数据",
                found.join(", ")
            )
        }
    }

    /// 为 script_v2 严格 loader 提供当前分区的源码与模板视图。保存和导入
    /// 会在此视图中覆盖待写文件，使引用校验与运行时使用同一套资源寻址。
    pub fn resources(&self, pkg: &str) -> PartitionResources<'_> {
        PartitionResources::new(self, pkg)
    }

    pub fn parse_script_content(
        &self,
        pkg: &str,
        resource: &str,
        content: &str,
    ) -> Result<crate::script_v2::ScriptFile, Vec<crate::script_v2::ScriptError>> {
        let mut resources = self.resources(pkg);
        resources.add_script(resource, content);
        crate::script_v2::parse_script_file(content, resource, &resources)
    }

    pub fn parse_function_content(
        &self,
        pkg: &str,
        resource: &str,
        content: &str,
    ) -> Result<crate::script_v2::FunctionFile, Vec<crate::script_v2::ScriptError>> {
        let mut resources = self.resources(pkg);
        resources.add_function(resource, content);
        crate::script_v2::parse_function_file(content, resource, &resources)
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

    /// 保存前的版本冲突检测：将被覆盖脚本的当前内容版本（文件不存在 → None）。
    /// 编辑器按 old_id 加载（重命名场景以 old_id 文件为准），否则按目标 pkg+name。
    pub fn script_version_for_save(
        &self,
        old_id: Option<&str>,
        pkg: &str,
        name: &str,
    ) -> anyhow::Result<Option<String>> {
        let id = match old_id {
            Some(id) => id.to_string(),
            None => {
                let package = sanitize_part(pkg).ok_or_else(|| {
                    anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}")
                })?;
                let name = normalize_script_name(name)?;
                format!("{package}/{name}")
            }
        };
        Ok(self.get(&id)?.map(|s| s.version()))
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
        let path = dir.join(&name);
        if let Some((old_pkg, old_name)) = &old {
            if old_pkg != &package {
                anyhow::bail!("脚本更新不得跨分区移动: {old_id:?} -> {package}/{name}");
            }
            let old_path = self.yaml_dir(old_pkg).join(old_name);
            if !old_path.is_file() {
                anyhow::bail!("脚本不存在: {old_id:?}");
            }
            if old_path != path && path.exists() {
                anyhow::bail!("脚本已存在: {}/{}", package, name);
            }
        }
        std::fs::create_dir_all(&dir)?;
        atomic_write(&path, content.as_bytes())?;
        let new_id = format!("{}/{}", package, name);
        if let Some((opkg, oname)) = old {
            let old_id = format!("{}/{}", opkg, oname);
            if old_id != new_id {
                let old_path = self.yaml_dir(&opkg).join(&oname);
                if old_path != path && old_path.is_file() {
                    if let Err(err) = std::fs::remove_file(&old_path) {
                        let _ = std::fs::remove_file(&path);
                        return Err(err.into());
                    }
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
    /// 精确名优先；否则按「基名 + `#` 后缀 + 同扩展名」唯一匹配（短名消歧语义
    /// 与 script_v2 校验一致）；零候选/多候选均报错，不猜测、不跨目录回退。
    pub fn resolve_template_path(&self, pkg: &str, short: &str) -> anyhow::Result<PathBuf> {
        match self.match_template_in_partition(pkg, short) {
            TplMatch::Found(path) => Ok(path),
            TplMatch::NotFound { name, path } => {
                anyhow::bail!("模板 {name} 不存在 (path={})", path.display())
            }
            TplMatch::Ambiguous { name, candidates } => anyhow::bail!(
                "模板 {name} 匹配到多个候选：{}，请用完整文件名指定",
                candidates.join("、")
            ),
        }
    }

    /// 模板短名可用性（script_v2 校验 ResourceProvider 消费）：
    /// 唯一存在 / 缺失 / 同短名多个 `#` 后缀候选（歧义）。
    pub fn template_avail(&self, pkg: &str, short: &str) -> crate::script_v2::TemplateAvail {
        match self.match_template_in_partition(pkg, short) {
            TplMatch::Found(_) => crate::script_v2::TemplateAvail::Found,
            TplMatch::NotFound { .. } => crate::script_v2::TemplateAvail::NotFound,
            TplMatch::Ambiguous { .. } => crate::script_v2::TemplateAvail::Ambiguous,
        }
    }

    /// 短名/完整名 → 磁盘文件的统一匹配内核（resolve_template_path 与
    /// template_avail 共用）：精确名优先，短名在同扩展名文件中唯一匹配。
    fn match_template_in_partition(&self, pkg: &str, short: &str) -> TplMatch {
        let Some(package) = sanitize_part(pkg) else {
            return TplMatch::NotFound {
                name: short.to_string(),
                path: self.tmpl_dir(pkg),
            };
        };
        if short.contains('\\') || short.contains('/') {
            return TplMatch::NotFound {
                name: short.to_string(),
                path: self.tmpl_dir(&package),
            };
        }
        let Some(name) = sanitize_template_name(short) else {
            return TplMatch::NotFound {
                name: short.to_string(),
                path: self.tmpl_dir(&package),
            };
        };
        let dir = self.tmpl_dir(&package);
        let exact = dir.join(&name);
        if exact.is_file() {
            return TplMatch::Found(exact);
        }
        let Some((base, ext)) = name.rsplit_once('.') else {
            return TplMatch::NotFound { name, path: exact };
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
            1 => TplMatch::Found(dir.join(&candidates[0])),
            0 => TplMatch::NotFound { name, path: exact },
            _ => TplMatch::Ambiguous { name, candidates },
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

    /// 创建函数库文件：严格 loader 校验通过后再原子写盘；已有目标拒绝覆盖。
    pub fn save_function(
        &self,
        pkg: &str,
        name: &str,
        content: &str,
    ) -> anyhow::Result<FunctionFile> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let (file, rel) = Self::normalize_function_rel(name)?;
        let parsed = self
            .parse_function_content(&package, &rel, content)
            .map_err(|errors| anyhow::anyhow!("{}", format_script_errors(&errors)))?;
        let path = self.func_dir(&package).join(&rel);
        if path.exists() {
            anyhow::bail!("函数文件已存在: {}/{}", package, rel);
        }
        atomic_write(&path, content.as_bytes())?;
        Ok(FunctionFile {
            id: format!("{package}/{rel}"),
            pkg: package,
            file,
            content: content.to_string(),
            version: content_version(content),
            functions: parsed.functions.iter().map(|f| f.name.clone()).collect(),
            updated_at: Self::fmt_mtime(&path),
        })
    }

    /// 更新现有函数库文件，可选地同时重命名；目标文件必须不存在或就是源文件。
    pub fn update_function(
        &self,
        id: &str,
        new_name: Option<&str>,
        content: &str,
    ) -> anyhow::Result<FunctionFile> {
        let Some((pkg, old_rel)) = id.split_once('/') else {
            anyhow::bail!("非法函数文件 id: {id}");
        };
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let old_path = self
            .resolve_function_path(&package, old_rel)
            .map_err(|_| anyhow::anyhow!("非法函数文件 id: {id}"))?;
        if !old_path.is_file() {
            anyhow::bail!("函数文件不存在: {id}");
        }
        let old_rel = old_path
            .strip_prefix(self.func_dir(&package))
            .map_err(|_| anyhow::anyhow!("函数文件路径不在分区内"))?
            .to_string_lossy()
            .replace('\\', "/");
        let target_input = new_name.unwrap_or(&old_rel);
        let (file, rel) = Self::normalize_function_rel(target_input)?;
        let new_path = self.func_dir(&package).join(&rel);
        if new_path != old_path && new_path.exists() {
            anyhow::bail!("函数文件已存在: {}/{}", package, rel);
        }
        let parsed = self
            .parse_function_content(&package, &rel, content)
            .map_err(|errors| anyhow::anyhow!("{}", format_script_errors(&errors)))?;
        atomic_write(&new_path, content.as_bytes())?;
        if new_path != old_path {
            if let Err(err) = std::fs::remove_file(&old_path) {
                let _ = std::fs::remove_file(&new_path);
                return Err(err.into());
            }
            self.cleanup_partition(&package);
        }
        Ok(FunctionFile {
            id: format!("{package}/{rel}"),
            pkg: package,
            file,
            content: content.to_string(),
            version: content_version(content),
            functions: parsed.functions.iter().map(|f| f.name.clone()).collect(),
            updated_at: Self::fmt_mtime(&new_path),
        })
    }

    fn load_function_at(&self, pkg: &str, rel: &str, file: &str) -> Option<FunctionFile> {
        let p = self.func_dir(pkg).join(rel);
        if !p.is_file() {
            return None;
        }
        let content = std::fs::read_to_string(&p).ok()?;
        let version = content_version(&content);
        let functions = crate::script_v2::validate::try_build_function_file(&content)
            .map(|file| file.functions.iter().map(|f| f.name.clone()).collect())
            .unwrap_or_default();
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

    /// 导出整分区快照 zip：yaml/ 全部脚本 + func/ 全部函数库 + tmpl/ 全部模板 → zip 字节。
    /// 三个目录条目始终写入，允许目录为空（契约 §3.1 / plan §13.1）。
    /// 返回（建议文件名, zip 字节）。
    pub fn export_partition(&self, pkg: &str) -> anyhow::Result<(String, Vec<u8>)> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {}", pkg))?;
        // 收集规则与导入校验一致：yaml/func 只认 .yaml/.yml（func 从严仅 .yaml 也
        // 兼容收录存量 .yml；导入侧严格 loader 仍只接受当前函数库语法，
        // tmpl 全部非隐藏文件
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
        let mut func_files: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.func_dir(&package)) {
            for f in rd.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                let low = name.to_lowercase();
                if f.path().is_file() && sanitize_part(&name).is_some() && low.ends_with(".yaml") {
                    func_files.push(name);
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
        yaml_files.sort();
        func_files.sort();
        tmpl_files.sort();
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            // 目录条目始终存在：空目录也是合法快照形态
            zw.add_directory("yaml", opts)?;
            zw.add_directory("func", opts)?;
            zw.add_directory("tmpl", opts)?;
            for name in &yaml_files {
                zw.start_file(format!("yaml/{}", name), opts)?;
                zw.write_all(&std::fs::read(self.yaml_dir(&package).join(name))?)?;
            }
            for name in &func_files {
                zw.start_file(format!("func/{}", name), opts)?;
                zw.write_all(&std::fs::read(self.func_dir(&package).join(name))?)?;
            }
            for name in &tmpl_files {
                zw.start_file(format!("tmpl/{}", name), opts)?;
                zw.write_all(&std::fs::read(self.tmpl_dir(&package).join(name))?)?;
            }
            zw.finish()?;
        }
        Ok((format!("{}.zip", package), buf))
    }

    /// 导入分区快照 zip 到指定应用分区。confirm=false 为 dry-run：解析 + 严格校验，
    /// 返回 {scripts, functions, templates} 三类资源各自的 add/overwrite/invalid
    /// 报告、不落盘；confirm=true 时报告内有任何 invalid 则整体拒绝（不半写入），
    /// 否则原子提交（同名替换，staging + 备份回滚）。只认 yaml/、func/、tmpl/ 布局。
    ///
    /// 资源硬限（阶段 2 SEC-004，传输层另有 20MiB body 闸门）：
    /// - 条目数 ≤ [`IMPORT_MAX_ENTRIES`]；
    /// - 总解压量 ≤ [`IMPORT_MAX_TOTAL_BYTES`]——条目声明尺寸预检 + 实际读取计数
    ///   双保险，防"声明造假"（压缩炸弹以小博大）；
    /// - 单 YAML/函数库 ≤ 1MiB、单模板 ≤ 10MiB（按实际读取字节判定，声明只做预检参考）；
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
        // 全部解析到内存（zip-slip 防护 + 布局校验 + 实际读取计数），
        // 无错才考虑落盘。zip-slip/布局外路径/重复条目/模板炸弹/体积超限 = 硬错误；
        // 文件名非法/扩展名错配/YAML 语法错/顶层结构不合规 = 报告 invalid 条目
        // （dry-run 只报告；confirm 见到任何 invalid 整体拒绝，不半写入）。
        let mut actual_total: usize = 0;
        let mut materialized_total: usize = 0;
        let mut seen_paths = std::collections::HashSet::new();
        let mut files: Vec<(ImportKind, String, PathBuf, Vec<u8>)> = Vec::new();
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
            let (kind, cap, zip_path, dest): (ImportKind, usize, String, PathBuf) =
                match comps.as_slice() {
                    [y, name] if y == "yaml" => {
                        let mut bad = |message: String| {
                            let path = format!("yaml/{name}");
                            rep.scripts
                                .invalid
                                .push(invalid_import_entry(&path, message))
                        };
                        let Some(name) = sanitize_part(name) else {
                            bad(format!("脚本名非法: {name}"));
                            continue;
                        };
                        let low = name.to_lowercase();
                        if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
                            bad(format!("yaml/ 下只支持 .yaml/.yml 脚本: {name}"));
                            continue;
                        }
                        (
                            ImportKind::Script,
                            IMPORT_MAX_YAML_BYTES,
                            format!("yaml/{name}"),
                            self.yaml_dir(&package).join(&name),
                        )
                    }
                    [d, name] if d == "func" => {
                        let mut bad = |message: String| {
                            let path = format!("func/{name}");
                            rep.functions
                                .invalid
                                .push(invalid_import_entry(&path, message))
                        };
                        let Some(name) = sanitize_part(name) else {
                            bad(format!("函数文件名非法: {name}"));
                            continue;
                        };
                        if !name.to_lowercase().ends_with(".yaml") {
                            bad(format!("func/ 下只支持 .yaml 函数库文件: {name}"));
                            continue;
                        }
                        (
                            ImportKind::Func,
                            IMPORT_MAX_YAML_BYTES,
                            format!("func/{name}"),
                            self.func_dir(&package).join(&name),
                        )
                    }
                    [t, name] if t == "tmpl" => {
                        let Some(name) = sanitize_template_name(name) else {
                            let path = format!("tmpl/{name}");
                            rep.templates
                                .invalid
                                .push(invalid_import_entry(&path, format!("模板名非法: {name}")));
                            continue;
                        };
                        (
                            ImportKind::Tmpl,
                            IMPORT_MAX_TMPL_BYTES,
                            format!("tmpl/{name}"),
                            self.tmpl_dir(&package).join(&name),
                        )
                    }
                    _ => anyhow::bail!(
                        "包内路径需为 yaml/<脚本>、func/<函数库> 或 tmpl/<模板>: {}",
                        f.name()
                    ),
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
            // 用同一套字节/尺寸/像素限额解码；旧格式归一化为灰度 PNG，
            // 文件名带 #1 的彩色模板保留颜色通道。
            // 否则一个 10MiB 以内的像素炸弹会在后续匹配时才触发高额分配。
            let buf = if zip_path.starts_with("tmpl/") {
                crate::matcher::reencode_template_png(
                    &buf,
                    !crate::matcher::template_color_from_name(&zip_path),
                )
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
            files.push((kind, zip_path, dest, buf));
        }
        // 导入预检和保存/运行共用同一套严格 loader。先将整个 ZIP 的候选
        // 资源放入分区视图，再逐文件解析，因而同一快照内的 call/func/模板
        // 引用也按最终布局校验；任一诊断只进入报告，不提前落盘。
        let mut resources = self.resources(&package);
        for (kind, zip_path, _dest, buf) in &files {
            match kind {
                ImportKind::Script => {
                    if let Ok(text) = std::str::from_utf8(buf) {
                        let name = zip_path.strip_prefix("yaml/").unwrap_or(zip_path);
                        resources.add_script(name, text);
                    }
                }
                ImportKind::Func => {
                    if let Ok(text) = std::str::from_utf8(buf) {
                        let name = zip_path
                            .strip_prefix("func/")
                            .unwrap_or(zip_path)
                            .trim_end_matches(".yaml");
                        resources.add_function(name, text);
                    }
                }
                ImportKind::Tmpl => {
                    let name = zip_path.strip_prefix("tmpl/").unwrap_or(zip_path);
                    resources.add_template(name);
                }
            }
        }
        let mut valid_files = Vec::with_capacity(files.len());
        for (kind, zip_path, dest, buf) in files {
            let diagnostics = match kind {
                ImportKind::Script => match std::str::from_utf8(&buf) {
                    Ok(text) => {
                        let name = zip_path.strip_prefix("yaml/").unwrap_or(&zip_path);
                        crate::script_v2::parse_script_file(text, name, &resources).map(|_| ())
                    }
                    Err(err) => Err(vec![crate::script_v2::ScriptError::new(
                        crate::script_v2::error::codes::YAML_SYNTAX_ERROR,
                        format!("内容不是合法 UTF-8 文本: {err}"),
                        zip_path.clone(),
                    )
                    .at("", "yaml")]),
                },
                ImportKind::Func => match std::str::from_utf8(&buf) {
                    Ok(text) => {
                        let name = zip_path.strip_prefix("func/").unwrap_or(&zip_path);
                        crate::script_v2::parse_function_file(text, name, &resources).map(|_| ())
                    }
                    Err(err) => Err(vec![crate::script_v2::ScriptError::new(
                        crate::script_v2::error::codes::YAML_SYNTAX_ERROR,
                        format!("内容不是合法 UTF-8 文本: {err}"),
                        zip_path.clone(),
                    )
                    .at("", "yaml")]),
                },
                ImportKind::Tmpl => Ok(()),
            };
            match diagnostics {
                Ok(()) => valid_files.push((kind, zip_path, dest, buf)),
                Err(diagnostics) => {
                    report_bucket(&mut rep, kind)
                        .invalid
                        .push(ImportInvalidEntry {
                            path: zip_path,
                            diagnostics,
                        })
                }
            }
        }
        let files = valid_files;
        if files.is_empty() && !rep.any_invalid() {
            anyhow::bail!("包内没有可导入的文件");
        }
        if !confirm {
            // dry-run：不落盘，报告三类资源各自的新增/覆盖/非法条目
            for (kind, zip_path, dest, _) in &files {
                let b = report_bucket(&mut rep, *kind);
                if dest.exists() {
                    b.overwrite.push(zip_path.clone());
                } else {
                    b.add.push(zip_path.clone());
                }
            }
            return Ok(rep);
        }
        if rep.any_invalid() {
            anyhow::bail!(
                "导入被拒绝：{} 个条目未通过严格校验（整体未写入）：{}",
                rep.invalid_count(),
                rep.invalid_summary()
            );
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
            for (kind, zip_path, dest, buf) in files {
                let relative = dest
                    .strip_prefix(&self.root)
                    .map_err(|_| anyhow::anyhow!("导入目标路径不在数据目录内"))?;
                let stage_path = stage_data.join(relative);
                atomic_write(&stage_path, &buf)?;
                staged.push((kind, zip_path, dest, stage_path));
            }
            Ok(())
        })();
        if let Err(e) = stage_result {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }

        let mut committed: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
        for (kind, zip_path, dest, stage_path) in staged {
            let was_existing = dest.exists();
            let backup = if was_existing {
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
            let b = report_bucket(&mut rep, kind);
            if was_existing {
                b.overwrite.push(zip_path);
            } else {
                b.add.push(zip_path);
            }
        }
        if let Err(e) = std::fs::remove_dir_all(&staging) {
            tracing::warn!(error = %e, "导入 staging 清理失败，将在下次启动时清理");
        }
        Ok(rep)
    }
} // impl ScriptStore

/// 分区快照 zip 内的资源类别（目录即类型）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportKind {
    Script,
    Func,
    Tmpl,
}

/// 一个分区的 loader 资源视图，可叠加尚未落盘的脚本、函数库和模板名。
pub struct PartitionResources<'a> {
    store: &'a ScriptStore,
    pkg: String,
    script_overrides: HashMap<String, String>,
    function_overrides: HashMap<String, String>,
    template_overrides: HashSet<String>,
}

impl<'a> PartitionResources<'a> {
    fn new(store: &'a ScriptStore, pkg: &str) -> Self {
        Self {
            store,
            pkg: pkg.to_string(),
            script_overrides: HashMap::new(),
            function_overrides: HashMap::new(),
            template_overrides: HashSet::new(),
        }
    }

    pub fn add_script(&mut self, resource: &str, content: &str) {
        let key = crate::script_v2::validate::normalize_id(resource.trim());
        self.script_overrides.insert(key, content.to_string());
    }

    pub fn add_function(&mut self, file_short: &str, content: &str) {
        let key = file_short
            .trim()
            .trim_end_matches(".yaml")
            .trim_end_matches(".yml")
            .to_string();
        self.function_overrides.insert(key, content.to_string());
    }

    pub fn add_template(&mut self, name: &str) {
        self.template_overrides.insert(name.to_string());
    }

    fn script_content_override(&self, resource: &str) -> Option<String> {
        self.script_overrides
            .get(&crate::script_v2::validate::normalize_id(resource.trim()))
            .cloned()
    }

    fn function_content_override(&self, file_short: &str) -> Option<String> {
        let key = file_short
            .trim()
            .trim_end_matches(".yaml")
            .trim_end_matches(".yml");
        self.function_overrides.get(key).cloned()
    }

    fn template_available(&self, short_name: &str) -> crate::script_v2::TemplateAvail {
        if self.template_overrides.contains(short_name) {
            return crate::script_v2::TemplateAvail::Found;
        }
        if let Some((base, ext)) = short_name.rsplit_once('.') {
            let prefix = format!("{}#", base.to_ascii_lowercase());
            let suffix = format!(".{}", ext.to_ascii_lowercase());
            let candidates = self
                .template_overrides
                .iter()
                .filter(|name| {
                    let lower = name.to_ascii_lowercase();
                    lower.starts_with(&prefix) && lower.ends_with(&suffix)
                })
                .count();
            match candidates {
                1 => return crate::script_v2::TemplateAvail::Found,
                n if n > 1 => return crate::script_v2::TemplateAvail::Ambiguous,
                _ => {}
            }
        }
        self.store.template_avail(&self.pkg, short_name)
    }
}

impl crate::script_v2::validate::ResourceProvider for PartitionResources<'_> {
    fn script_exists(&self, resource_id: &str) -> bool {
        self.script_content(resource_id).is_some()
    }

    fn script_content(&self, resource_id: &str) -> Option<String> {
        if let Some(content) = self.script_content_override(resource_id) {
            return Some(content);
        }
        let key = crate::script_v2::validate::normalize_id(resource_id.trim());
        let candidates = [key.clone(), format!("{key}.yaml")];
        candidates
            .iter()
            .find_map(|candidate| {
                self.store
                    .get(&format!("{}/{}", self.pkg, candidate))
                    .ok()
                    .flatten()
            })
            .map(|script| script.content)
    }

    fn function_file_content(&self, file_short: &str) -> Option<String> {
        if let Some(content) = self.function_content_override(file_short) {
            return Some(content);
        }
        let rel = format!("{}.yaml", file_short.trim().trim_end_matches(".yaml"));
        let path = self.store.resolve_function_path(&self.pkg, &rel).ok()?;
        std::fs::read_to_string(path).ok()
    }

    fn function_exists(&self, file_short: &str, function: &str) -> bool {
        self.function_file_content(file_short)
            .and_then(|content| crate::script_v2::validate::try_build_function_file(&content))
            .is_some_and(|file| file.find(function).is_some())
    }

    fn resolve_template(&self, short_name: &str) -> crate::script_v2::TemplateAvail {
        self.template_available(short_name)
    }
}

fn report_bucket(rep: &mut ImportReport, kind: ImportKind) -> &mut ImportResourceReport {
    match kind {
        ImportKind::Script => &mut rep.scripts,
        ImportKind::Func => &mut rep.functions,
        ImportKind::Tmpl => &mut rep.templates,
    }
}

fn invalid_import_entry(path: &str, message: impl Into<String>) -> ImportInvalidEntry {
    ImportInvalidEntry {
        path: path.to_string(),
        diagnostics: vec![crate::script_v2::ScriptError::new(
            crate::script_v2::error::codes::RESOURCE_IMPORT_INVALID,
            message,
            path,
        )
        .at("", "path")],
    }
}

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
/// 单类资源的导入报告（add/overwrite 为 zip 相对路径）
#[derive(Debug, Default, Serialize)]
pub struct ImportResourceReport {
    /// dry-run=将新增；confirm=实际新增
    pub add: Vec<String>,
    /// dry-run=将覆盖（分区已有同名文件）；confirm=实际覆盖
    pub overwrite: Vec<String>,
    /// 未通过严格 loader 的条目（含结构、语义与引用诊断）
    pub invalid: Vec<ImportInvalidEntry>,
}

/// 一个未通过严格 loader 校验的导入条目
#[derive(Debug, Serialize)]
pub struct ImportInvalidEntry {
    /// zip 内相对路径
    pub path: String,
    /// 严格 loader 诊断（五元组）
    pub diagnostics: Vec<crate::script_v2::ScriptError>,
}

/// 导入结果报告（dry-run 与 confirm 同构；契约 plan §13.1）
#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    /// yaml/ 可执行脚本
    pub scripts: ImportResourceReport,
    /// func/ 函数库
    pub functions: ImportResourceReport,
    /// tmpl/ 模板
    pub templates: ImportResourceReport,
}

impl ImportReport {
    fn any_invalid(&self) -> bool {
        !self.scripts.invalid.is_empty()
            || !self.functions.invalid.is_empty()
            || !self.templates.invalid.is_empty()
    }

    fn invalid_count(&self) -> usize {
        self.scripts.invalid.len() + self.functions.invalid.len() + self.templates.invalid.len()
    }

    /// 前 3 条 invalid 的 "path: diagnostic" 摘要（整体拒绝时的错误消息）
    fn invalid_summary(&self) -> String {
        let all = self
            .scripts
            .invalid
            .iter()
            .chain(&self.functions.invalid)
            .chain(&self.templates.invalid)
            .map(|e| {
                let detail = e
                    .diagnostics
                    .first()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "未知诊断".to_string());
                format!("{}（{}）", e.path, detail)
            });
        all.take(3).collect::<Vec<_>>().join("；")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

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
    fn script_store_open_fails_fast_on_legacy_layout() {
        let dir = std::env::temp_dir().join(format!(
            "gamer-legacy-layout-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(dir.join("scripts/com.test.app")).unwrap();
        std::fs::write(
            dir.join("scripts/com.test.app/main.yaml"),
            "package old\nsteps: []\n",
        )
        .unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let err = match ScriptStore::open(&cfg) {
            Ok(_) => panic!("旧布局必须 fail-fast"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("已废弃的资源目录布局"));
        assert!(dir.join("scripts/com.test.app/main.yaml").is_file());
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
            ("yaml/main.yaml".into(), b"steps:\n  - log: ok\n".to_vec()),
            (
                "func/common.yaml".into(),
                b"login:\n  steps:\n    - return: true\n".to_vec(),
            ),
            ("tmpl/a#0_0_10_10.png".into(), valid_template_png()),
        ]);
        // dry-run：报告三类资源、不落盘
        let rep = store.import(&z, "com.test.app", false).unwrap();
        assert_eq!(rep.scripts.add, vec!["yaml/main.yaml"]);
        assert_eq!(rep.functions.add, vec!["func/common.yaml"]);
        assert_eq!(rep.templates.add, vec!["tmpl/a#0_0_10_10.png"]);
        assert!(!rep.any_invalid());
        assert!(!dir.join("com.test.app/yaml/main.yaml").exists());
        // confirm：落盘，报告 add
        let rep = store.import(&z, "com.test.app", true).unwrap();
        assert_eq!(rep.scripts.add.len(), 1);
        assert_eq!(rep.functions.add.len(), 1);
        assert_eq!(rep.templates.add.len(), 1);
        let root: PathBuf = dir.clone();
        assert!(root.join("com.test.app/yaml/main.yaml").is_file());
        assert!(root.join("com.test.app/func/common.yaml").is_file());
        assert!(root.join("com.test.app/tmpl/a#0_0_10_10.png").is_file());
        // 再导入一次（confirm 覆盖同名）：全部进 overwrite
        let rep2 = store.import(&z, "com.test.app", true).unwrap();
        assert_eq!(rep2.scripts.overwrite.len(), 1);
        assert_eq!(rep2.functions.overwrite.len(), 1);
        assert_eq!(rep2.templates.overwrite.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_dry_run_reports_invalid_and_confirm_rejects_atomically() {
        let (store, dir) = temp_store("import-invalid");
        let z = craft_zip(vec![
            // 合法脚本 + 非法函数库（函数名保留字）+ 语法错脚本 + 未知顶层键脚本
            ("yaml/ok.yaml".into(), b"steps: []\n".to_vec()),
            (
                "yaml/bad-syntax.yaml".into(),
                b"steps: [unclosed\n".to_vec(),
            ),
            (
                "yaml/bad-key.yaml".into(),
                b"name: legacy\nsteps: []\n".to_vec(),
            ),
            (
                "func/bad-name.yaml".into(),
                b"match:\n  steps: []\n".to_vec(),
            ),
        ]);
        let rep = store.import(&z, "com.test.app", false).unwrap();
        assert_eq!(rep.scripts.add, vec!["yaml/ok.yaml"]);
        assert_eq!(rep.scripts.invalid.len(), 2, "{rep:?}");
        assert!(rep
            .scripts
            .invalid
            .iter()
            .any(|e| e.path == "yaml/bad-syntax.yaml"));
        assert!(rep
            .scripts
            .invalid
            .iter()
            .any(|e| e.path == "yaml/bad-key.yaml"));
        assert_eq!(rep.functions.invalid.len(), 1);
        assert_eq!(rep.functions.invalid[0].path, "func/bad-name.yaml");
        assert!(rep.functions.add.is_empty());
        // dry-run 不落盘
        assert!(!dir.join("com.test.app/yaml/ok.yaml").exists());
        // confirm：任一 invalid → 整体拒绝，合法条目也不写入
        let err = store.import(&z, "com.test.app", true).unwrap_err();
        assert!(err.to_string().contains("整体未写入"), "{err}");
        assert!(!dir.join("com.test.app/yaml/ok.yaml").exists());
        assert!(!dir.join("com.test.app/func").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_accepts_snapshot_without_func_dir() {
        let (store, dir) = temp_store("import-no-func");
        // 当前布局快照可以省略 func/ 目录；脚本使用严格 v2 语法。
        let z = craft_zip(vec![
            ("yaml/old.yaml".into(), b"steps:\n  - log: x\n".to_vec()),
            ("tmpl/b.png".into(), valid_template_png()),
        ]);
        let rep = store.import(&z, "com.test.app", true).unwrap();
        assert_eq!(rep.scripts.add, vec!["yaml/old.yaml"]);
        assert_eq!(rep.templates.add, vec!["tmpl/b.png"]);
        assert!(rep.functions.add.is_empty() && rep.functions.invalid.is_empty());
        assert!(dir.join("com.test.app/yaml/old.yaml").is_file());
        let _ = std::fs::remove_dir_all(&dir);

        // 顶层旧语法/顶层序列不再被导入接受，并返回结构化诊断
        let z = craft_zip(vec![
            (
                "yaml/v1func.yaml".into(),
                b"func:\n  - f1:\n    - log: a\nsteps:\n  - log: x\n".to_vec(),
            ),
            ("yaml/v1seq.yaml".into(), b"- log: seq\n".to_vec()),
        ]);
        let rep = store.import(&z, "com.test.app", false).unwrap();
        assert_eq!(rep.scripts.invalid.len(), 2, "{rep:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_import_roundtrip_is_lossless() {
        let (store, dir) = temp_store("roundtrip");
        store
            .save(None, "com.a", "main.yaml", "steps:\n  - log: x\n")
            .unwrap();
        store
            .save(None, "com.a", "legacy.yml", "steps: []\n")
            .unwrap();
        store.save_function("com.a", "common", FUNC_OK).unwrap();
        let tmpl_dir = dir.join("com.a").join("tmpl");
        std::fs::create_dir_all(&tmpl_dir).unwrap();
        // 磁盘模板与导入侧同为灰度归一化字节，保证往返逐字节可比
        let png = crate::matcher::reencode_template_gray_png(&valid_template_png()).unwrap();
        std::fs::write(tmpl_dir.join("icon#0_0_10_10.png"), &png).unwrap();

        // 导出：含 yaml/ func/ tmpl/ 三个目录
        let (filename, zip_bytes) = store.export_partition("com.a").unwrap();
        assert_eq!(filename, "com.a.zip");
        {
            let mut ar = zip::ZipArchive::new(std::io::Cursor::new(&zip_bytes)).unwrap();
            let names: Vec<String> = (0..ar.len())
                .map(|i| ar.by_index(i).unwrap().name().to_string())
                .collect();
            for d in ["yaml/", "func/", "tmpl/"] {
                assert!(names.iter().any(|n| n == d), "缺目录条目 {d}: {names:?}");
            }
            assert!(names.contains(&"yaml/main.yaml".to_string()));
            assert!(names.contains(&"yaml/legacy.yml".to_string()));
            assert!(names.contains(&"func/common.yaml".to_string()));
            assert!(names.contains(&"tmpl/icon#0_0_10_10.png".to_string()));
        }

        // 导入到另一分区 → 列表零差异（内容逐字节一致）
        store.import(&zip_bytes, "com.b", true).unwrap();
        let a_scripts = store.list().unwrap();
        let pair = |pkg: &str, name: &str| {
            a_scripts
                .iter()
                .find(|s| s.package == pkg && s.name == name)
                .map(|s| s.content.clone())
                .unwrap_or_default()
        };
        assert_eq!(pair("com.a", "main.yaml"), pair("com.b", "main.yaml"));
        assert_eq!(pair("com.a", "legacy.yml"), pair("com.b", "legacy.yml"));
        let a_func = store.list_functions("com.a").unwrap();
        let b_func = store.list_functions("com.b").unwrap();
        assert_eq!(a_func.len(), b_func.len());
        assert_eq!(a_func[0].file, b_func[0].file);
        assert_eq!(a_func[0].content, b_func[0].content);
        assert_eq!(a_func[0].functions, b_func[0].functions);
        assert_eq!(
            std::fs::read(tmpl_dir.join("icon#0_0_10_10.png")).unwrap(),
            std::fs::read(dir.join("com.b/tmpl/icon#0_0_10_10.png")).unwrap()
        );
        // 导入分区隔离：com.a 不受影响（同名覆盖只发生在目标分区）
        assert_eq!(store.list_functions("com.a").unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_allows_empty_directories() {
        let (store, _dir) = temp_store("export-empty");
        // 只有函数库的分区：yaml/ tmpl/ 为空目录也进快照
        store.save_function("com.a", "common", FUNC_OK).unwrap();
        let (_name, zip_bytes) = store.export_partition("com.a").unwrap();
        let mut ar = zip::ZipArchive::new(std::io::Cursor::new(&zip_bytes)).unwrap();
        let names: Vec<String> = (0..ar.len())
            .map(|i| ar.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.iter().all(|n| n != "yaml/main.yaml"));
        assert!(names.iter().any(|n| n == "func/common.yaml"));
        // 完全空分区也产出合法快照（三个空目录）
        let (_name, empty_zip) = store.export_partition("com.empty").unwrap();
        let ar = zip::ZipArchive::new(std::io::Cursor::new(&empty_zip)).unwrap();
        assert_eq!(ar.len(), 3, "仅三个目录条目");
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
            .update_function(
                "com.test.app/common.yaml",
                None,
                "only:\n  steps:\n    - return: true\n",
            )
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
    fn function_save_uses_strict_loader_before_writing() {
        let (store, dir) = temp_store("func-invalid");
        let cases: Vec<(&str, &str)> = vec![
            ("yaml.syntax_error", "login: [unclosed"),
            ("函数文件顶层必须是映射", "- login\n- logout\n"),
            ("没有定义任何函数", "{}\n"),
            ("只允许 unicode 字母", "1abc:\n  steps: []\n"),
            ("只允许 unicode 字母", "带 空 格:\n  steps: []\n"),
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
        // 中文函数名合法（顶层键 = 函数名，序列化 plain 往返）
        let f = store
            .save_function("com.test.app", "ok.yaml", "登录确认:\n  steps: []\n")
            .unwrap();
        assert_eq!(f.functions, vec!["登录确认".to_string()]);
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
