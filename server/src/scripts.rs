//! 脚本/函数库/模板文件存储：按应用分区 `data/<pkg>/scripts/` + `data/<pkg>/functions/` + `data/<pkg>/templates/`
//!
//! 分区 = 设备配置的应用包名（如 com.miHoYo.hkrpg），无 default 兜底；
//! 脚本 id = `<pkg>/<name>.yaml`（含 `/`，前端拼 URL 必须整体 encodeURIComponent）。
//! 旧 `package <名字>` YAML 指令已废除（引擎直接解析 YAML，残留指令行 = 解析报错），
//! 更老的数据布局（含旧分区子目录名 yaml/func/tmpl）不自动迁移；
//! `ScriptStore::open` 检测到 data 根级旧布局时直接失败。
//!
//! 路径解析（目录即类型）：resolve_script_path / resolve_function_path /
//! resolve_template_path 三套拆开——拒绝绝对路径、反斜杠、空段、`.`、`..`、跨分区与
//! 扩展名错配；不回退（脚本只认 scripts/、函数库只认 functions/、模板只认 templates/
//! 现存文件）、不做内容推断。契约见 docs/SCRIPT_EDITOR_CONTRACT.md §3.1。

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Serialize, Serializer};

use crate::config::Config;
#[cfg(test)]
use crate::core::fs::atomic_write_with_replace_err;
use crate::core::fs::safe_name as sanitize_part;
use crate::core::fs::{atomic_write, content_version, is_windows_reserved_name};

fn format_script_errors(errors: &[crate::script_v2::ScriptError]) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("；")
}

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

/// 磁盘上的一个函数库文件（data/<pkg>/functions/<文件短路径>.yaml；顶层键 = 函数名）
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

/// 校验模板文件名：允许 unicode 字母数字与 `. - _ #`（模板名可带 #x1_y1_x2_y2 区域后缀）、空格
pub(crate) fn sanitize_template_name(s: &str) -> Option<String> {
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

/// 与前端模板短名规则保持一致：去掉颜色标记 `#1` 和搜索区域 `#...`，
/// 保留扩展名。脚本通常引用短名，重命名模板时需要同时迁移这种引用。
fn template_short_name(name: &str) -> String {
    let mut value = name.to_string();
    let lower = value.to_ascii_lowercase();
    for extension in [".jpeg", ".jpg", ".png"] {
        let suffix = format!("#1{extension}");
        if lower.ends_with(&suffix) {
            let stem_end = value.len() - extension.len();
            let prefix_end = value.len() - suffix.len();
            value = format!("{}{}", &value[..prefix_end], &value[stem_end..]);
            break;
        }
    }
    let lower = value.to_ascii_lowercase();
    let ext_len = [".jpeg", ".jpg", ".png"]
        .iter()
        .find(|ext| lower.ends_with(**ext))
        .map(|ext| ext.len());
    let Some(ext_len) = ext_len else {
        return value;
    };
    let stem_end = value.len() - ext_len;
    let stem = &value[..stem_end];
    match stem.rfind('#') {
        Some(index) if index + 1 < stem.len() => {
            format!("{}{}", &stem[..index], &value[stem_end..])
        }
        _ => value,
    }
}

fn rename_template_value(
    value: &mut crate::script_v2::TypedValue,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
) -> bool {
    let crate::script_v2::TypedValue::Tmpl(current) = value else {
        return false;
    };
    let replacement = if current == old_name {
        Some(new_name)
    } else if current == old_short {
        Some(new_short)
    } else {
        None
    };
    let Some(replacement) = replacement else {
        return false;
    };
    *current = replacement.to_string();
    true
}

fn rename_template_cell(
    cell: &mut crate::script_v2::Cell,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
) -> usize {
    let crate::script_v2::Cell::Lit(value) = cell else {
        return 0;
    };
    usize::from(rename_template_value(
        value, old_name, old_short, new_name, new_short,
    ))
}

fn rename_template_steps(
    steps: &mut [crate::script_v2::Step],
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
) -> usize {
    use crate::script_v2::Step;

    let mut changed = 0;
    for step in steps {
        changed += match step {
            Step::StrApp | Step::ClsApp | Step::Break | Step::Throw { .. } => 0,
            Step::Tap { at } => rename_template_cell(at, old_name, old_short, new_name, new_short),
            Step::Swipe { from, to, time } => {
                rename_template_cell(from, old_name, old_short, new_name, new_short)
                    + rename_template_cell(to, old_name, old_short, new_name, new_short)
                    + rename_template_cell(time, old_name, old_short, new_name, new_short)
            }
            Step::Key { key } => {
                rename_template_cell(key, old_name, old_short, new_name, new_short)
            }
            Step::Text { value } => {
                rename_template_cell(value, old_name, old_short, new_name, new_short)
            }
            Step::Log { message } => {
                rename_template_cell(message, old_name, old_short, new_name, new_short)
            }
            Step::Wait {
                duration,
                duration_max,
            } => {
                let mut n =
                    rename_template_cell(duration, old_name, old_short, new_name, new_short);
                if let Some(max) = duration_max {
                    n += rename_template_cell(max, old_name, old_short, new_name, new_short);
                }
                n
            }
            Step::Find {
                template,
                block,
                then,
                r#else,
                ..
            } => {
                let mut n =
                    rename_template_cell(template, old_name, old_short, new_name, new_short);
                for cell in block {
                    n += rename_template_cell(cell, old_name, old_short, new_name, new_short);
                }
                n + rename_template_steps(then, old_name, old_short, new_name, new_short)
                    + rename_template_steps(r#else, old_name, old_short, new_name, new_short)
            }
            Step::Match {
                candidates, r#else, ..
            } => {
                let mut n = 0;
                for candidate in candidates {
                    n += rename_template_cell(
                        &mut candidate.template,
                        old_name,
                        old_short,
                        new_name,
                        new_short,
                    );
                    n += rename_template_steps(
                        &mut candidate.steps,
                        old_name,
                        old_short,
                        new_name,
                        new_short,
                    );
                }
                n + rename_template_steps(r#else, old_name, old_short, new_name, new_short)
            }
            Step::Check {
                template, timeout, ..
            } => {
                let mut n =
                    rename_template_cell(template, old_name, old_short, new_name, new_short);
                if let Some(timeout) = timeout {
                    n += rename_template_cell(timeout, old_name, old_short, new_name, new_short);
                }
                n
            }
            Step::Color { at, expect, r#else } => {
                let mut n = rename_template_cell(at, old_name, old_short, new_name, new_short);
                for branch in expect {
                    n += rename_template_cell(
                        &mut branch.color,
                        old_name,
                        old_short,
                        new_name,
                        new_short,
                    );
                    n += rename_template_steps(
                        &mut branch.steps,
                        old_name,
                        old_short,
                        new_name,
                        new_short,
                    );
                }
                n + rename_template_steps(r#else, old_name, old_short, new_name, new_short)
            }
            Step::If { cond, then, r#else } => {
                rename_template_cell(cond, old_name, old_short, new_name, new_short)
                    + rename_template_steps(then, old_name, old_short, new_name, new_short)
                    + rename_template_steps(r#else, old_name, old_short, new_name, new_short)
            }
            Step::Loop { steps, .. } => {
                rename_template_steps(steps, old_name, old_short, new_name, new_short)
            }
            Step::Call { args, .. } => args
                .iter_mut()
                .map(|arg| {
                    rename_template_cell(&mut arg.value, old_name, old_short, new_name, new_short)
                })
                .sum(),
            Step::Func {
                args, then, r#else, ..
            } => {
                let n: usize = args
                    .iter_mut()
                    .map(|arg| {
                        rename_template_cell(
                            &mut arg.value,
                            old_name,
                            old_short,
                            new_name,
                            new_short,
                        )
                    })
                    .sum();
                n + rename_template_steps(then, old_name, old_short, new_name, new_short)
                    + rename_template_steps(r#else, old_name, old_short, new_name, new_short)
            }
            Step::Return { value } => {
                rename_template_cell(value, old_name, old_short, new_name, new_short)
            }
        };
    }
    changed
}

fn rename_template_in_params(
    params: &mut [crate::script_v2::ParamDecl],
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
) -> usize {
    params
        .iter_mut()
        .filter_map(|param| param.default.as_mut())
        .map(|value| rename_template_value(value, old_name, old_short, new_name, new_short))
        .map(usize::from)
        .sum()
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
    /// 数据根目录（data/），一级子目录 = 应用分区（内含 scripts/ 与 templates/）
    root: PathBuf,
    /// Composite 资源解析缝（user-overrides → active App Package → 本分区兜底）。
    /// 仅模板解析接入（find/match 与 script_v2 校验共用）；脚本/函数库快照仍
    /// 以分区目录为唯一来源，包内脚本待后续波次。
    composite: crate::app_packages::CompositeResolver,
}

impl ScriptStore {
    pub fn open(cfg: &Config) -> anyhow::Result<Self> {
        let store = Self {
            root: cfg.data_dir.clone(),
            composite: crate::app_packages::CompositeResolver::new(cfg.data_dir.clone()),
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

    /// 为 script_v2 严格 loader 提供当前分区的源码与模板视图。保存会在此
    /// 视图中覆盖待写文件，使引用校验与运行时使用同一套资源寻址。
    pub fn resources(&self, pkg: &str) -> PartitionResources<'_> {
        PartitionResources::new(self, pkg)
    }

    /// Composite 脚本源码（engine RunSnapshot 合并用）：override → active 包
    /// `scripts/`；分区目录由调用方自行兜底。
    pub(crate) fn composite_script_sources(
        &self,
        pkg: &str,
    ) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
        Ok(self.composite.script_sources(pkg)?)
    }

    /// Composite 函数库源码（engine RunSnapshot 合并用）：override → active 包
    /// `functions/`；分区目录由调用方自行兜底。
    pub(crate) fn composite_function_sources(
        &self,
        pkg: &str,
    ) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
        Ok(self.composite.function_sources(pkg)?)
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

    /// 分区脚本目录
    pub fn script_dir(&self, pkg: &str) -> PathBuf {
        self.partition_dir(pkg).join("scripts")
    }

    /// 分区函数库目录（data/<pkg>/functions/，顶层键 = 函数名）
    pub fn functions_dir(&self, pkg: &str) -> PathBuf {
        self.partition_dir(pkg).join("functions")
    }

    /// 分区模板目录
    pub fn templates_dir(&self, pkg: &str) -> PathBuf {
        self.partition_dir(pkg).join("templates")
    }

    /// 返回位于数据根内的分区目录。公共路径构造器没有 Result 返回值，故对
    /// 非法分区名映射到不可枚举的哨兵目录，避免任何调用方意外逃出 root。
    fn partition_dir(&self, pkg: &str) -> PathBuf {
        sanitize_part(pkg)
            .map(|pkg| self.root.join(pkg))
            .unwrap_or_else(|| self.root.join(".gamer-invalid-partition"))
    }

    /// 磁盘上全部分区名（存在 scripts/ functions/ templates/ keymaps/ presets/
    /// resources/ 子目录之一的一级目录，字典序）。不把 package.toml 之类标志
    /// 文件计入：本模块没有任何路径会写它，App Package 清单（manifest.toml）
    /// 位于 data/app-packages/ 下、经 composite 解析，与本地分区无关；以资源
    /// 子目录为准可避免杂散文件在分区列表里制造幻影分区。
    pub fn partitions(&self) -> Vec<String> {
        const PARTITION_RESOURCE_DIRS: [&str; 6] = [
            "scripts",
            "functions",
            "templates",
            "keymaps",
            "presets",
            "resources",
        ];
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.root) {
            for d in rd.flatten() {
                let p = d.path();
                let Some(name) = sanitize_part(&d.file_name().to_string_lossy()) else {
                    continue;
                };
                if p.is_dir()
                    && PARTITION_RESOURCE_DIRS
                        .iter()
                        .any(|dir| p.join(dir).is_dir())
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
        let p = self.script_dir(pkg).join(name);
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
            let Ok(rd) = std::fs::read_dir(self.script_dir(&pkg)) else {
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
            .strip_prefix(self.script_dir(pkg))
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
        let dir = self.script_dir(&package);
        let path = dir.join(&name);
        if let Some((old_pkg, old_name)) = &old {
            if old_pkg != &package {
                anyhow::bail!("脚本更新不得跨分区移动: {old_id:?} -> {package}/{name}");
            }
            let old_path = self.script_dir(old_pkg).join(old_name);
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
                let old_path = self.script_dir(&opkg).join(&oname);
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

    /// 重命名模板文件，并同步改写当前分区 scripts/ 与 functions/ 中的模板引用。
    ///
    /// 引用迁移走严格 AST，不做全局文本替换，避免误改日志/文本内容；同时处理
    /// 模板参数默认值、步骤字段、match/color 候选与 call/func 实参。所有资源先
    /// 解析并生成新内容，再开始落盘，写入失败时回滚已改写的资源。
    pub fn rename_template(
        &self,
        pkg: &str,
        old_name: &str,
        new_name: &str,
    ) -> anyhow::Result<usize> {
        let package = sanitize_part(pkg).ok_or_else(|| anyhow::anyhow!("应用包名非法"))?;
        let old_name = sanitize_template_name(old_name)
            .ok_or_else(|| anyhow::anyhow!("模板名非法: {old_name}"))?;
        let new_name = sanitize_template_name(new_name)
            .ok_or_else(|| anyhow::anyhow!("模板名非法: {new_name}"))?;
        let old_path = self.templates_dir(&package).join(&old_name);
        let new_path = self.templates_dir(&package).join(&new_name);
        if !old_path.is_file() {
            anyhow::bail!("模板不存在");
        }
        if new_path.exists() {
            anyhow::bail!("已存在同名模板");
        }
        let template_bytes = std::fs::read(&old_path)?;
        let old_short = template_short_name(&old_name);
        let new_short = template_short_name(&new_name);
        let mut rewrites: Vec<(PathBuf, String, String)> = Vec::new();

        for script in self.list()?.into_iter().filter(|s| s.package == package) {
            if crate::yaml_vnext::is_v3_source(&script.content) {
                if let Some((rewritten, _changed)) = crate::yaml_vnext::rename_template_source(
                    &script.content,
                    &old_name,
                    &old_short,
                    &new_name,
                    &new_short,
                )
                .map_err(|diagnostics| {
                    anyhow::anyhow!(
                        "v3 脚本模板引用无法重写: {}",
                        diagnostics
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("；")
                    )
                })? {
                    rewrites.push((
                        self.script_dir(&package).join(&script.name),
                        script.content,
                        rewritten,
                    ));
                }
                continue;
            }
            let mut parsed = self
                .parse_script_content(&package, &script.name, &script.content)
                .map_err(|errors| anyhow::anyhow!(format_script_errors(&errors)))?;
            let changed = rename_template_in_params(
                &mut parsed.params,
                &old_name,
                &old_short,
                &new_name,
                &new_short,
            ) + rename_template_steps(
                &mut parsed.steps,
                &old_name,
                &old_short,
                &new_name,
                &new_short,
            );
            if changed > 0 {
                rewrites.push((
                    self.script_dir(&package).join(&script.name),
                    script.content,
                    crate::script_v2::serialize_script(&parsed),
                ));
            }
        }

        for function in self.list_functions(&package)? {
            let mut parsed = self
                .parse_function_content(&package, &function.file, &function.content)
                .map_err(|errors| anyhow::anyhow!(format_script_errors(&errors)))?;
            let mut changed = 0;
            for declaration in &mut parsed.functions {
                changed += rename_template_in_params(
                    &mut declaration.params,
                    &old_name,
                    &old_short,
                    &new_name,
                    &new_short,
                );
                changed += rename_template_steps(
                    &mut declaration.steps,
                    &old_name,
                    &old_short,
                    &new_name,
                    &new_short,
                );
            }
            if changed > 0 {
                rewrites.push((
                    self.functions_dir(&package)
                        .join(format!("{}.yaml", function.file)),
                    function.content,
                    crate::script_v2::serialize_function_file(&parsed),
                ));
            }
        }

        for (written, (path, _, content)) in rewrites.iter().enumerate() {
            if let Err(error) = atomic_write(path, content.as_bytes()) {
                for (rollback_path, original, _) in rewrites[..written].iter().rev() {
                    let _ = atomic_write(rollback_path, original.as_bytes());
                }
                return Err(error);
            }
        }

        if let Err(error) = atomic_write(&new_path, &template_bytes) {
            for (rollback_path, original, _) in rewrites.iter().rev() {
                let _ = atomic_write(rollback_path, original.as_bytes());
            }
            return Err(error);
        }
        if let Err(error) = std::fs::remove_file(&old_path) {
            let _ = std::fs::remove_file(&new_path);
            for (rollback_path, original, _) in rewrites.iter().rev() {
                let _ = atomic_write(rollback_path, original.as_bytes());
            }
            return Err(error.into());
        }
        Ok(rewrites.len())
    }

    // ---------- 三套路径解析（阶段 1：目录即类型，互不回退、不做内容推断） ----------
    //
    // 契约 §3.1：scripts/ 可执行脚本、functions/ 函数库、templates/ 模板；跨分区一律
    // 不解析、不回退；模板短名只允许 templates/ 现存文件。解析失败统一 Err（不猜测、不回退）。

    /// 脚本相对路径 → 磁盘路径（分区 `scripts/` 内，.yaml/.yml——.yml 为存量存档扩展名，
    /// 收紧为 .yaml 时只改此处）。拒绝绝对路径/反斜杠/空段/`.`/`..`/扩展名错配；
    /// functions/、templates/ 下的同名文件对脚本解析不可见。
    pub fn resolve_script_path(&self, pkg: &str, rel: &str) -> anyhow::Result<PathBuf> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let segs = sanitize_rel_segments(rel)?;
        let last = segs.last().expect("分段结果非空");
        let low = last.to_lowercase();
        if !(low.ends_with(".yaml") || low.ends_with(".yml")) {
            anyhow::bail!("脚本必须是 .yaml/.yml 且位于分区 scripts/ 目录: {rel}");
        }
        let mut p = self.script_dir(&package);
        for s in &segs {
            p.push(s);
        }
        Ok(p)
    }

    /// 函数文件相对路径 → 磁盘路径（分区 `functions/` 内，严格 .yaml——新契约函数库
    /// 无 .yml 形态）。规则同 resolve_script_path。
    pub fn resolve_function_path(&self, pkg: &str, rel: &str) -> anyhow::Result<PathBuf> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let segs = sanitize_rel_segments(rel)?;
        let last = segs.last().expect("分段结果非空");
        if !last.to_lowercase().ends_with(".yaml") {
            anyhow::bail!("函数文件必须是 .yaml 且位于分区 functions/ 目录: {rel}");
        }
        let mut p = self.functions_dir(&package);
        for s in &segs {
            p.push(s);
        }
        Ok(p)
    }

    /// 模板短名/完整名 → **现存**文件路径。composite 顺序：user override →
    /// active App Package → 分区 `templates/`（兜底层）。
    /// 精确名优先；否则按「基名 + `#` 后缀 + 同扩展名」唯一匹配（短名消歧语义
    /// 与 script_v2 校验一致）；零候选/多候选均报错，不猜测、不跨目录回退。
    pub fn resolve_template_path(&self, pkg: &str, short: &str) -> anyhow::Result<PathBuf> {
        match self.composite.template(pkg, short) {
            crate::app_packages::TemplateLookup::Found(hit) => Ok(hit.path),
            crate::app_packages::TemplateLookup::Ambiguous { name, candidates } => anyhow::bail!(
                "模板 {name} 匹配到多个候选：{}，请用完整文件名指定",
                candidates.join("、")
            ),
            crate::app_packages::TemplateLookup::NotFound => {
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
        }
    }

    /// 模板短名可用性（script_v2 校验 ResourceProvider 消费）：
    /// 唯一存在 / 缺失 / 同短名多个 `#` 后缀候选（歧义）。
    /// 解析顺序与 resolve_template_path 完全一致（override → 包 → 分区）。
    pub fn template_avail(&self, pkg: &str, short: &str) -> crate::script_v2::TemplateAvail {
        match self.composite.template(pkg, short) {
            crate::app_packages::TemplateLookup::Found(_) => crate::script_v2::TemplateAvail::Found,
            crate::app_packages::TemplateLookup::Ambiguous { .. } => {
                crate::script_v2::TemplateAvail::Ambiguous
            }
            crate::app_packages::TemplateLookup::NotFound => {
                match self.match_template_in_partition(pkg, short) {
                    TplMatch::Found(_) => crate::script_v2::TemplateAvail::Found,
                    TplMatch::NotFound { .. } => crate::script_v2::TemplateAvail::NotFound,
                    TplMatch::Ambiguous { .. } => crate::script_v2::TemplateAvail::Ambiguous,
                }
            }
        }
    }

    /// 短名/完整名 → 磁盘文件的统一匹配内核（resolve_template_path 与
    /// template_avail 共用）：精确名优先，短名在同扩展名文件中唯一匹配。
    fn match_template_in_partition(&self, pkg: &str, short: &str) -> TplMatch {
        let Some(package) = sanitize_part(pkg) else {
            return TplMatch::NotFound {
                name: short.to_string(),
                path: self.templates_dir(pkg),
            };
        };
        if short.contains('\\') || short.contains('/') {
            return TplMatch::NotFound {
                name: short.to_string(),
                path: self.templates_dir(&package),
            };
        }
        let Some(name) = sanitize_template_name(short) else {
            return TplMatch::NotFound {
                name: short.to_string(),
                path: self.templates_dir(&package),
            };
        };
        let dir = self.templates_dir(&package);
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

    // ---------- 函数库存储（data/<pkg>/functions/，文件顶层键 = 函数名） ----------

    /// 规范化函数文件短路径（save 与版本冲突检测共用）：trim，去掉尾部 .yaml/.yml
    /// 后统一补 .yaml；返回（文件短路径, 相对 functions/ 的磁盘路径）。
    /// 存储扁平（列表单层），短路径不支持子目录。
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
        let path = self.functions_dir(&package).join(&rel);
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
            .strip_prefix(self.functions_dir(&package))
            .map_err(|_| anyhow::anyhow!("函数文件路径不在分区内"))?
            .to_string_lossy()
            .replace('\\', "/");
        let target_input = new_name.unwrap_or(&old_rel);
        let (file, rel) = Self::normalize_function_rel(target_input)?;
        let new_path = self.functions_dir(&package).join(&rel);
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
        let p = self.functions_dir(pkg).join(rel);
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
    /// 与脚本列表一致）。只认 functions/ 下 .yaml 文件，不与脚本/模板混列。
    pub fn list_functions(&self, pkg: &str) -> anyhow::Result<Vec<FunctionFile>> {
        let package = sanitize_part(pkg)
            .ok_or_else(|| anyhow::anyhow!("应用包名非法（只允许字母数字 . _ -）: {pkg}"))?;
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.functions_dir(&package)) {
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
            .strip_prefix(self.functions_dir(pkg))
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

    /// 分区 scripts/functions/templates 都已空时删掉分区目录（避免残留空目录被当成有效分区）
    pub fn cleanup_partition(&self, pkg: &str) {
        let _ = std::fs::remove_dir(self.script_dir(pkg)); // 非空时失败，忽略
        let _ = std::fs::remove_dir(self.functions_dir(pkg));
        let _ = std::fs::remove_dir(self.templates_dir(pkg));
        let _ = std::fs::remove_dir(self.root.join(pkg));
    }
} // impl ScriptStore

/// 一个分区的 loader 资源视图，可叠加尚未落盘的脚本与函数库。
pub struct PartitionResources<'a> {
    store: &'a ScriptStore,
    pkg: String,
    script_overrides: HashMap<String, String>,
    function_overrides: HashMap<String, String>,
}

impl<'a> PartitionResources<'a> {
    fn new(store: &'a ScriptStore, pkg: &str) -> Self {
        Self {
            store,
            pkg: pkg.to_string(),
            script_overrides: HashMap::new(),
            function_overrides: HashMap::new(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn atomic_write_replaces_existing_file_without_leftover_temp_files() {
        let (store, dir) = temp_store("atomic");
        let path = dir.join("com.test.app").join("scripts").join("main.yaml");
        atomic_write(&path, b"first\n").unwrap();
        atomic_write(&path, b"second\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second\n");
        let scripts_dir = store.script_dir("com.test.app");
        let leftovers: Vec<_> = std::fs::read_dir(scripts_dir)
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
        let path = dir.join("com.test.app").join("scripts").join("main.yaml");
        atomic_write(&path, b"old\n").unwrap();

        let err = atomic_write_with_replace_err(&path, b"new\n").unwrap_err();
        assert!(err.to_string().contains("replace failure"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old\n");

        let scripts_dir = store.script_dir("com.test.app");
        let leftovers: Vec<_> = std::fs::read_dir(scripts_dir)
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
        let path = dir.join("com.test.app").join("scripts").join("main.yaml");
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

        let scripts_dir = store.script_dir("com.test.app");
        let leftovers: Vec<_> = std::fs::read_dir(scripts_dir)
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
        assert!(err.to_string().contains("已废弃的数据根级目录布局"));
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

    // ---------- 测试脚手架 ----------

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
        let store = ScriptStore {
            root: dir.clone(),
            composite: crate::app_packages::CompositeResolver::new(dir.clone()),
        };
        (store, dir)
    }

    // ---------- 三套路径解析（路径安全 + 目录即类型 + 不回退） ----------

    #[test]
    fn resolve_script_path_accepts_only_yaml_files_under_partition() {
        let (store, dir) = temp_store("rspscript");
        let p = store
            .resolve_script_path("com.test.app", "main.yaml")
            .unwrap();
        assert_eq!(
            p,
            dir.join("com.test.app").join("scripts").join("main.yaml")
        );
        // 嵌套短路径（契约 call 目标 sub/inner.yaml 形态）按段解析
        let p = store
            .resolve_script_path("com.test.app", "sub/inner.yaml")
            .unwrap();
        assert_eq!(
            p,
            dir.join("com.test.app")
                .join("scripts")
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
    fn resolve_function_path_strict_yaml_and_stays_in_functions() {
        let (store, dir) = temp_store("rspfunc");
        let p = store
            .resolve_function_path("com.test.app", "common.yaml")
            .unwrap();
        assert_eq!(
            p,
            dir.join("com.test.app")
                .join("functions")
                .join("common.yaml")
        );
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
        let (store, dir) = temp_store("rsptpl");
        let templates_dir = dir.join("com.test.app").join("templates");
        std::fs::create_dir_all(&templates_dir).unwrap();
        std::fs::write(templates_dir.join("login#907_160_973_717.png"), b"png").unwrap();
        std::fs::write(templates_dir.join("full.png"), b"png").unwrap();
        // 短名唯一匹配 # 后缀候选
        assert_eq!(
            store
                .resolve_template_path("com.test.app", "login.png")
                .unwrap(),
            templates_dir.join("login#907_160_973_717.png")
        );
        // 精确完整名优先
        assert_eq!(
            store
                .resolve_template_path("com.test.app", "full.png")
                .unwrap(),
            templates_dir.join("full.png")
        );
        // 不存在 → 错误；歧义 → 错误；路径分隔符 → 错误
        assert!(store
            .resolve_template_path("com.test.app", "nope.png")
            .is_err());
        std::fs::write(templates_dir.join("login#a.png"), b"png").unwrap();
        std::fs::write(templates_dir.join("login#b.png"), b"png").unwrap();
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
        // 只认 templates/ 现存文件：scripts/ 下同名文件不影响模板解析（不回退、不内容推断）
        std::fs::create_dir_all(dir.join("com.test.app").join("scripts")).unwrap();
        std::fs::write(
            dir.join("com.test.app").join("scripts").join("shop.png"),
            b"not a template",
        )
        .unwrap();
        assert!(store
            .resolve_template_path("com.test.app", "shop.png")
            .is_err());
        // 跨分区不可见
        std::fs::create_dir_all(dir.join("com.other.app").join("templates")).unwrap();
        std::fs::write(
            dir.join("com.other.app").join("templates").join("x.png"),
            b"png",
        )
        .unwrap();
        assert!(store
            .resolve_template_path("com.test.app", "x.png")
            .is_err());
    }

    #[test]
    fn rename_template_updates_script_and_function_references() {
        let (store, dir) = temp_store("rename-template");
        let templates_dir = store.templates_dir("com.test.app");
        std::fs::create_dir_all(&templates_dir).unwrap();
        std::fs::write(templates_dir.join("old.png"), b"png").unwrap();

        store
            .save(
                None,
                "com.test.app",
                "main.yaml",
                "steps:\n  - check: old.png\n    timeout: 0s\n  - log: old.png 文本不应改\n",
            )
            .unwrap();
        store
            .save_function(
                "com.test.app",
                "common",
                "login:\n  steps:\n    - find: old.png\n",
            )
            .unwrap();

        assert_eq!(
            store
                .rename_template("com.test.app", "old.png", "new.png")
                .unwrap(),
            2
        );
        assert!(!templates_dir.join("old.png").exists());
        assert_eq!(
            std::fs::read(templates_dir.join("new.png")).unwrap(),
            b"png"
        );
        let script = std::fs::read_to_string(dir.join("com.test.app/scripts/main.yaml")).unwrap();
        assert!(script.contains("check: new.png"));
        assert!(script.contains("old.png 文本不应改"));
        let function =
            std::fs::read_to_string(dir.join("com.test.app/functions/common.yaml")).unwrap();
        assert!(function.contains("find: new.png"));
    }

    #[test]
    fn rename_template_updates_v3_surface_references_without_touching_text() {
        let (store, dir) = temp_store("rename-template-v3");
        let templates_dir = store.templates_dir("com.test.app");
        std::fs::create_dir_all(&templates_dir).unwrap();
        std::fs::write(templates_dir.join("old.png"), b"png").unwrap();
        store
            .save(
                None,
                "com.test.app",
                "main.yaml",
                "version: 3\nsteps:\n  - find:\n      template: old.png\n      then:\n        - log: old.png 文本不应改\n  - match_first:\n      candidates: [old.png]\n",
            )
            .unwrap();

        let renamed = store
            .rename_template("com.test.app", "old.png", "new.png")
            .unwrap();
        assert_eq!(renamed, 1);
        let script = std::fs::read_to_string(dir.join("com.test.app/scripts/main.yaml")).unwrap();
        assert!(script.contains("template: new.png"));
        assert!(script.contains("- new.png"));
        assert!(script.contains("old.png 文本不应改"));
    }

    #[test]
    fn resolvers_never_fall_back_across_resource_directories() {
        let (store, dir) = temp_store("rspfallback");
        let pkg = dir.join("com.test.app");
        // functions/ 里存在 common.yaml，脚本解析不得回退命中（目录即类型）
        std::fs::create_dir_all(pkg.join("functions")).unwrap();
        std::fs::write(
            pkg.join("functions").join("common.yaml"),
            "login:\n  steps: []",
        )
        .unwrap();
        assert!(store
            .resolve_script_path("com.test.app", "common.yaml")
            .is_ok());
        assert!(!store
            .resolve_script_path("com.test.app", "common.yaml")
            .unwrap()
            .is_file());
        // scripts/ 里存在 main.yaml，函数解析不得回退命中
        std::fs::create_dir_all(pkg.join("scripts")).unwrap();
        std::fs::write(pkg.join("scripts").join("main.yaml"), "steps: []").unwrap();
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
        assert!(!dir.join("com.test.app/functions/bad.yaml").exists());
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

        // 脚本列表只有 scripts/ 脚本，functions/ 文件绝不混入
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
