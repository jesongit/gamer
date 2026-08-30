//! 运行源码快照（阶段 2 交付 3）：运行开始时把当前分区 `yaml/` 与 `func/`
//! 的全部源文件内容整体读入内存（本次运行不可变）；call/func 在执行期从
//! 快照**懒解析**并按运行实例缓存——运行中修改文件不影响已开始的实例，
//! 下一次运行生效（plan §12.2）。
//!
//! 模板是二进制资源，不进快照：校验期可用性经
//! [`crate::scripts::ScriptStore::template_avail`]、匹配期路径经
//! [`crate::scripts::ScriptStore::resolve_template_path`] 落盘解析。
//!
//! 资源 id 形态（与 script_v2 校验的 provider 契约一致）：
//! - 脚本 = 分区内相对路径（含 `.yaml`，与 call 目标书写一致），缺扩展名自动
//!   补 `.yaml`（`.yml` 存量同理）；
//! - 函数文件 = 短路径（去 `.yaml`）。
//!
//! 引擎内统一以「去扩展名」的归一 id 做解析缓存键；parse 的 `resource` 也用
//! 归一 id——call 自引用/跨文件环比较（`validate::normalize_id`）由此保持一致。

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use crate::script_v2::error::codes;
use crate::script_v2::validate::{
    normalize_id, try_build_function_file, ResourceProvider, TemplateAvail,
};
use crate::script_v2::{FunctionFile, ScriptError, ScriptFile};
use crate::scripts::ScriptStore;

/// 一次运行的分区源码快照（构建后不可变）。
pub(crate) struct RunSnapshot {
    /// `yaml/` 全部脚本源码：key = 分区内相对路径（含扩展名）
    scripts: BTreeMap<String, String>,
    /// `func/` 全部函数库源码：key = 文件短路径（去 `.yaml`）
    functions: BTreeMap<String, String>,
}

impl RunSnapshot {
    /// 递归读取分区 `yaml/` 与 `func/` 下全部源文件（目录缺失视为空快照）。
    pub fn capture(store: &ScriptStore, pkg: &str) -> anyhow::Result<Self> {
        Ok(Self {
            scripts: read_sources(&store.yaml_dir(pkg), false)?,
            functions: read_sources(&store.func_dir(pkg), true)?,
        })
    }

    /// 脚本源码：精确名优先，缺 `.yaml`/`.yml` 扩展名自动补全。
    fn script(&self, resource_id: &str) -> Option<&str> {
        let id = resource_id.trim().trim_start_matches("./");
        if id.is_empty() {
            return None;
        }
        for key in [id.to_string(), format!("{id}.yaml"), format!("{id}.yml")] {
            if let Some(c) = self.scripts.get(&key) {
                return Some(c);
            }
        }
        None
    }

    /// 函数库源码：短路径（容忍误带 `.yaml` 后缀）。
    fn function_file(&self, file_short: &str) -> Option<&str> {
        let short = file_short.trim();
        let short = short
            .strip_suffix(".yaml")
            .or_else(|| short.strip_suffix(".yml"))
            .unwrap_or(short);
        if short.is_empty() {
            return None;
        }
        self.functions.get(short).map(String::as_str)
    }
}

/// 递归读取目录下全部 `.yaml`/`.yml` 源文件（`strip_ext=true` 时 key 去扩展名）。
fn read_sources(dir: &Path, strip_ext: bool) -> anyhow::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current)? {
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
            let rel = path.strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
            let key = if strip_ext {
                rel.rsplit_once('.')
                    .map(|(base, _)| base.to_string())
                    .unwrap_or(rel)
            } else {
                rel
            };
            let content = std::fs::read_to_string(&path)?;
            out.insert(key, content);
        }
    }
    Ok(out)
}

/// 快照版 [`ResourceProvider`]：脚本/函数内容取自快照，模板可用性落盘查询。
/// `pkg` 自有（运行实例生命周期一致，`Ctx` 持有；分区名在运行期间不变）。
pub(crate) struct RunResources<'a> {
    snapshot: &'a RunSnapshot,
    store: &'a ScriptStore,
    pkg: String,
}

impl<'a> RunResources<'a> {
    pub fn new(snapshot: &'a RunSnapshot, store: &'a ScriptStore, pkg: impl Into<String>) -> Self {
        Self {
            snapshot,
            store,
            pkg: pkg.into(),
        }
    }

    pub fn as_provider(&self) -> &dyn ResourceProvider {
        self
    }
}

impl ResourceProvider for RunResources<'_> {
    fn script_exists(&self, resource_id: &str) -> bool {
        self.snapshot.script(resource_id).is_some()
    }

    fn script_content(&self, resource_id: &str) -> Option<String> {
        self.snapshot.script(resource_id).map(str::to_string)
    }

    fn function_file_content(&self, file_short: &str) -> Option<String> {
        self.snapshot.function_file(file_short).map(str::to_string)
    }

    fn function_exists(&self, file_short: &str, function: &str) -> bool {
        // 轻量构建（不进缓存）：仅判断函数名存在性，正式解析在 ResourceCache。
        self.snapshot
            .function_file(file_short)
            .and_then(try_build_function_file)
            .is_some_and(|ff| ff.find(function).is_some())
    }

    fn resolve_template(&self, short_name: &str) -> TemplateAvail {
        self.store.template_avail(&self.pkg, short_name)
    }
}

/// 运行实例内的解析缓存：call/func 目标首次触达时严格解析，之后复用
/// 同一 AST（Arc 共享）——运行中改文件不影响本实例。
#[derive(Default)]
pub(crate) struct ResourceCache {
    scripts: HashMap<String, Arc<ScriptFile>>,
    functions: HashMap<String, Arc<FunctionFile>>,
}

impl ResourceCache {
    /// 懒解析一个脚本（缓存键 = 归一 id，即去 `.yaml` 后缀）。
    /// 不存在 → `resource.script.not_found`；解析/校验失败 → 全部结构化诊断。
    pub fn script(
        &mut self,
        resources: &RunResources<'_>,
        resource_id: &str,
    ) -> Result<Arc<ScriptFile>, Vec<ScriptError>> {
        let key = normalize_id(resource_id.trim());
        if let Some(parsed) = self.scripts.get(&key) {
            return Ok(parsed.clone());
        }
        let Some(content) = resources.snapshot.script(resource_id) else {
            return Err(vec![ScriptError::new(
                codes::RESOURCE_SCRIPT_NOT_FOUND,
                format!("脚本 {resource_id:?} 不存在（分区 {}）", resources.pkg),
                key.clone(),
            )]);
        };
        let parsed = Arc::new(script_v2_parse(&key, content, resources.as_provider())?);
        self.scripts.insert(key, parsed.clone());
        Ok(parsed)
    }

    /// 懒解析一个函数库文件（缓存键 = 文件短路径）。
    pub fn function_file(
        &mut self,
        resources: &RunResources<'_>,
        file_short: &str,
    ) -> Result<Arc<FunctionFile>, Vec<ScriptError>> {
        let key = normalize_id(file_short.trim());
        if let Some(parsed) = self.functions.get(&key) {
            return Ok(parsed.clone());
        }
        let Some(content) = resources.snapshot.function_file(file_short) else {
            return Err(vec![ScriptError::new(
                codes::RESOURCE_FUNC_NOT_FOUND,
                format!("函数文件 {file_short:?} 不存在（分区 {}）", resources.pkg),
                key.clone(),
            )
            .at("", "file")]);
        };
        let parsed = Arc::new(script_v2_parse_function(
            &key,
            content,
            resources.as_provider(),
        )?);
        self.functions.insert(key, parsed.clone());
        Ok(parsed)
    }
}

/// 薄封装：统一 parse_script_file 调用点（错误类型透传）。
fn script_v2_parse(
    resource: &str,
    content: &str,
    provider: &dyn ResourceProvider,
) -> Result<ScriptFile, Vec<ScriptError>> {
    crate::script_v2::parse_script_file(content, resource, provider)
}

fn script_v2_parse_function(
    resource: &str,
    content: &str,
    provider: &dyn ResourceProvider,
) -> Result<FunctionFile, Vec<ScriptError>> {
    crate::script_v2::parse_function_file(content, resource, provider)
}
