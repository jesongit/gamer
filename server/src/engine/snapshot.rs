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

use crate::core::AppContext;
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
    /// 递归读取分区 `yaml/` 与 `func/` 下全部源文件（目录缺失视为空快照），
    /// 并合并 active App Package `scripts/` 与用户 override 的同名资源：
    /// 优先级 **override → 包 → 分区**（与模板/键位 composite 顺序一致）。
    /// 包内 `scripts/` 同时承载脚本（call 目标，含扩展名 key）与函数库
    /// （func 目标，去扩展名短路径 key）两种语义。
    pub fn capture(store: &ScriptStore, pkg: &str) -> anyhow::Result<Self> {
        let mut scripts = read_sources(&store.yaml_dir(pkg), false)?;
        let mut functions = read_sources(&store.func_dir(pkg), true)?;
        for (key, content) in store.composite_script_sources(pkg)? {
            functions.insert(
                key.strip_suffix(".yaml")
                    .or_else(|| key.strip_suffix(".yml"))
                    .map(str::to_string)
                    .unwrap_or_else(|| key.clone()),
                content.clone(),
            );
            scripts.insert(key, content);
        }
        Ok(Self { scripts, functions })
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
    app: AppContext,
}

impl<'a> RunResources<'a> {
    pub fn new(snapshot: &'a RunSnapshot, store: &'a ScriptStore, app: AppContext) -> Self {
        Self {
            snapshot,
            store,
            app,
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
        self.app
            .content_package
            .as_ref()
            .map(|pkg| self.store.template_avail(pkg.as_str(), short_name))
            .unwrap_or(TemplateAvail::NotFound)
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
                format!(
                    "脚本 {resource_id:?} 不存在（分区 {}）",
                    resources
                        .app
                        .content_package
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "<none>".to_string())
                ),
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
                format!(
                    "函数文件 {file_short:?} 不存在（分区 {}）",
                    resources
                        .app
                        .content_package
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "<none>".to_string())
                ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// 安装一个 active 包：scripts/dup.yaml（与分区/override 同名不同内容）、
    /// scripts/lib/helpers.yaml（子目录脚本）。
    async fn install_package(data_dir: &Path) {
        let manifest = br#"id = "official.test"
version = "1.0.0"

[android]
packages = ["com.test.app"]
"#;
        let mut archive = Vec::new();
        {
            use std::io::Write as _;
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut archive));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zw.start_file("manifest.toml", opts).unwrap();
            zw.write_all(manifest).unwrap();
            zw.start_file("scripts/dup.yaml", opts).unwrap();
            zw.write_all(b"steps: [] # package").unwrap();
            zw.start_file("scripts/lib/helpers.yaml", opts).unwrap();
            zw.write_all(b"steps: [] # helpers").unwrap();
            zw.finish().unwrap();
        }
        let packages = crate::app_packages::AppPackageStore::new(data_dir);
        packages.install_and_activate(&archive, None).await.unwrap();
    }

    /// RunSnapshot composite 优先级：override → 包 → 分区；包内 `scripts/`
    /// 同时进入脚本（含扩展名）与函数库（去扩展名短路径）两个索引。
    #[tokio::test]
    async fn snapshot_merges_package_and_override_sources_with_priority() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let store = ScriptStore::open(&cfg).unwrap();
        let pkg = "com.test.app";

        // 分区兜底层
        std::fs::create_dir_all(store.yaml_dir(pkg)).unwrap();
        std::fs::write(
            store.yaml_dir(pkg).join("dup.yaml"),
            b"steps: [] # partition",
        )
        .unwrap();
        std::fs::write(
            store.yaml_dir(pkg).join("partition-only.yaml"),
            b"steps: [] # partition-only",
        )
        .unwrap();
        std::fs::create_dir_all(store.func_dir(pkg)).unwrap();
        std::fs::write(
            store.func_dir(pkg).join("common.yaml"),
            b"noop:\n  steps: []\n",
        )
        .unwrap();

        // 包内 scripts/：分区同名被包覆盖，子目录脚本可解析
        install_package(dir.path()).await;
        let snapshot = RunSnapshot::capture(&store, pkg).unwrap();
        assert_eq!(
            snapshot.script("dup.yaml"),
            Some("steps: [] # package"),
            "包内脚本必须覆盖分区同名文件"
        );
        assert_eq!(
            snapshot.script("lib/helpers.yaml"),
            Some("steps: [] # helpers")
        );
        assert_eq!(
            snapshot.script("partition-only.yaml"),
            Some("steps: [] # partition-only"),
            "包内/override 未覆盖的脚本继续由分区兜底"
        );
        // 包内脚本同时可作函数库（短路径 key），分区 func/ 不受影响
        assert_eq!(snapshot.function_file("dup"), Some("steps: [] # package"));
        assert_eq!(
            snapshot.function_file("common"),
            Some("noop:\n  steps: []\n")
        );

        // user override 再覆盖包内同名脚本
        let override_dir = dir.path().join("user-overrides").join(pkg).join("scripts");
        std::fs::create_dir_all(&override_dir).unwrap();
        std::fs::write(override_dir.join("dup.yaml"), b"steps: [] # override").unwrap();
        let snapshot = RunSnapshot::capture(&store, pkg).unwrap();
        assert_eq!(
            snapshot.script("dup.yaml"),
            Some("steps: [] # override"),
            "override 必须优先于包内与分区"
        );
        assert_eq!(snapshot.function_file("dup"), Some("steps: [] # override"));
    }
}
