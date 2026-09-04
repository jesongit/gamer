//! 运行源码快照：运行开始时把当前分区 `scripts/` 与 `functions/`
//! 的全部源文件内容整体读入内存（本次运行不可变）；call/func 在执行期从
//! 快照**懒解析**并按运行实例缓存——运行中修改文件不影响已开始的实例，
//! 下一次运行生效（plan §12.2）。
//!
//! 脚本/函数源码经 Core [`crate::resources::ResourceStore`] 的 composite 解析缝取
//! 三层合并结果：**本地编辑区（分区目录）→ user-overrides → active App
//! Package**（与模板/键位顺序一致）；包内 `scripts/` 只进脚本索引、包内
//! `functions/` 只进函数索引（Wave 1 起两类索引彻底分离）。
//!
//! 模板是二进制资源，不进快照：校验期可用性、匹配期路径均经 composite
//! 三层解析（`ResourceStore::composite().template` / `resolve_template_path`）落盘解析。
//!
//! 资源 id 形态（与 script_v2 校验的 provider 契约一致）：
//! - 脚本 = 分区内相对路径（含 `.yaml`，与 call 目标书写一致），缺扩展名自动
//!   补 `.yaml`（`.yml` 存量同理）；
//! - 函数文件 = 短路径（去 `.yaml`）。
//!
//! 引擎内统一以「去扩展名」的归一 id 做解析缓存键；parse 的 `resource` 也用
//! 归一 id——call 自引用/跨文件环比较（`validate::normalize_id`）由此保持一致。

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::app_packages::TemplateLookup;
use crate::core::AppContext;
use crate::extensions::gamer_yaml::script_v2::error::codes;
use crate::extensions::gamer_yaml::script_v2::validate::TemplateAvail as ProviderTemplateAvail;
use crate::extensions::gamer_yaml::script_v2::validate::{
    normalize_id, try_build_function_file, ResourceProvider,
};
use crate::extensions::gamer_yaml::script_v2::{FunctionFile, ScriptError, ScriptFile};
use crate::resources::ResourceStore;

/// 一次运行的分区源码快照（构建后不可变）。
pub(crate) struct RunSnapshot {
    /// `scripts/` 全部脚本源码：key = 分区内相对路径（含扩展名）
    scripts: BTreeMap<String, String>,
    /// `functions/` 全部函数库源码：key = 文件短路径（去 `.yaml`）
    functions: BTreeMap<String, String>,
}

impl RunSnapshot {
    /// 经 composite 解析缝递归读取 `scripts/` 与 `functions/` 全部源文件
    ///（三层合并：本地编辑区 → override → active 包；目录缺失视为空层，
    /// 不报错）。包内 `scripts/` 只进脚本索引、包内 `functions/` 只进函数库
    /// 索引（去扩展名短路径 key），两类索引互不混入。
    pub fn capture(store: &ResourceStore, pkg: &str) -> anyhow::Result<Self> {
        let scripts = store.composite().script_sources(pkg)?;
        let mut functions = BTreeMap::new();
        for (key, content) in store.composite().function_sources(pkg)? {
            let short = key
                .strip_suffix(".yaml")
                .or_else(|| key.strip_suffix(".yml"))
                .map(str::to_string)
                .unwrap_or_else(|| key.clone());
            functions.insert(short, content);
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

/// 快照版 [`ResourceProvider`]：脚本/函数内容取自快照，模板可用性落盘查询。
/// `pkg` 自有（运行实例生命周期一致，`Ctx` 持有；分区名在运行期间不变）。
pub(crate) struct RunResources<'a> {
    snapshot: &'a RunSnapshot,
    store: &'a ResourceStore,
    app: AppContext,
}

impl<'a> RunResources<'a> {
    pub fn new(snapshot: &'a RunSnapshot, store: &'a ResourceStore, app: AppContext) -> Self {
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

    fn resolve_template(&self, short_name: &str) -> ProviderTemplateAvail {
        self.app
            .content_package
            .as_ref()
            .map(|pkg| template_avail(self.store, pkg.as_str(), short_name))
            .unwrap_or(ProviderTemplateAvail::NotFound)
    }
}

/// 模板短名可用性（composite 三层，与 `resolve_template_path` 完全一致）：
/// 唯一存在 / 缺失 / 同短名多个 `#` 后缀候选（歧义）。
pub(crate) fn template_avail(
    store: &ResourceStore,
    pkg: &str,
    short: &str,
) -> ProviderTemplateAvail {
    match store.composite().template(pkg, short) {
        TemplateLookup::Found(_) => ProviderTemplateAvail::Found,
        TemplateLookup::Ambiguous { .. } => ProviderTemplateAvail::Ambiguous,
        TemplateLookup::NotFound => ProviderTemplateAvail::NotFound,
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
    crate::extensions::gamer_yaml::script_v2::parse_script_file(content, resource, provider)
}

fn script_v2_parse_function(
    resource: &str,
    content: &str,
    provider: &dyn ResourceProvider,
) -> Result<FunctionFile, Vec<ScriptError>> {
    crate::extensions::gamer_yaml::script_v2::parse_function_file(content, resource, provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::path::Path;

    /// 安装一个 active 包：scripts/dup.yaml（与分区/override 同名不同内容）、
    /// scripts/lib/helpers.yaml（子目录脚本）、functions/common.yaml（函数库，
    /// 与分区同名不同内容）、functions/dup.yaml（与包内脚本同名的函数库，
    /// 用于证明两个索引互不混入）。
    async fn install_package(data_dir: &Path) {
        let manifest = br#"format_version = 2
id = "official.test"
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
            zw.start_file("functions/common.yaml", opts).unwrap();
            zw.write_all(b"noop:\n  steps: [] # package\n").unwrap();
            zw.start_file("functions/dup.yaml", opts).unwrap();
            zw.write_all(b"dup:\n  steps: []\n").unwrap();
            zw.finish().unwrap();
        }
        let packages = crate::app_packages::AppPackageStore::new(data_dir);
        packages.install_and_activate(&archive, None).await.unwrap();
    }

    /// RunSnapshot composite 三层优先级：**本地编辑区 → override → 包**；
    /// 包内 `scripts/` 与 `functions/` 分别只进脚本（含扩展名）与函数库
    ///（去扩展名短路径）索引。
    #[tokio::test]
    async fn snapshot_merges_editable_override_and_package_sources_with_priority() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let store = ResourceStore::open(&cfg).unwrap();
        let pkg = "com.test.app";

        // 本地编辑区（分区）层
        std::fs::create_dir_all(store.kind_dir(pkg, crate::resources::ResourceKind::Scripts))
            .unwrap();
        std::fs::write(
            store
                .kind_dir(pkg, crate::resources::ResourceKind::Scripts)
                .join("dup.yaml"),
            b"steps: [] # local",
        )
        .unwrap();
        std::fs::write(
            store
                .kind_dir(pkg, crate::resources::ResourceKind::Scripts)
                .join("local-only.yaml"),
            b"steps: [] # local-only",
        )
        .unwrap();
        std::fs::create_dir_all(store.kind_dir(pkg, crate::resources::ResourceKind::Functions))
            .unwrap();
        std::fs::write(
            store
                .kind_dir(pkg, crate::resources::ResourceKind::Functions)
                .join("common.yaml"),
            b"noop:\n  steps: [] # local\n",
        )
        .unwrap();

        // 装包：本地编辑区同名文件必须继续胜出，包内独有文件可解析
        install_package(dir.path()).await;
        let snapshot = RunSnapshot::capture(&store, pkg).unwrap();
        assert_eq!(
            snapshot.script("dup.yaml"),
            Some("steps: [] # local"),
            "本地编辑区脚本必须胜过包内同名文件"
        );
        assert_eq!(
            snapshot.script("lib/helpers.yaml"),
            Some("steps: [] # helpers")
        );
        assert_eq!(
            snapshot.script("local-only.yaml"),
            Some("steps: [] # local-only"),
            "包内/override 未覆盖的本地脚本继续可见"
        );
        // 包内 functions/dup.yaml 进入函数库索引，包内 scripts/dup.yaml 的脚本
        // 内容不再混入（Script/Function 索引分离）
        assert_eq!(snapshot.function_file("dup"), Some("dup:\n  steps: []\n"));
        // 包内 functions/ 未覆盖分区同名函数库时，本地层胜出
        assert_eq!(
            snapshot.function_file("common"),
            Some("noop:\n  steps: [] # local\n")
        );
        // 包内 functions/ 不进入脚本索引
        assert_eq!(snapshot.script("common.yaml"), None);

        // 删掉本地 dup.yaml 后，包内同名脚本浮现
        std::fs::remove_file(
            store
                .kind_dir(pkg, crate::resources::ResourceKind::Scripts)
                .join("dup.yaml"),
        )
        .unwrap();
        let snapshot = RunSnapshot::capture(&store, pkg).unwrap();
        assert_eq!(
            snapshot.script("dup.yaml"),
            Some("steps: [] # package"),
            "本地编辑区删除后必须回落到包内脚本"
        );

        // user override 插入后胜过包内同名脚本与函数库（仍低于本地编辑区）
        let override_root = dir.path().join("user-overrides").join(pkg);
        std::fs::create_dir_all(override_root.join("scripts")).unwrap();
        std::fs::write(
            override_root.join("scripts").join("dup.yaml"),
            b"steps: [] # override",
        )
        .unwrap();
        std::fs::create_dir_all(override_root.join("functions")).unwrap();
        std::fs::write(
            override_root.join("functions").join("dup.yaml"),
            b"dup:\n  steps: [] # override\n",
        )
        .unwrap();
        let snapshot = RunSnapshot::capture(&store, pkg).unwrap();
        assert_eq!(
            snapshot.script("dup.yaml"),
            Some("steps: [] # override"),
            "override 必须优先于包内与本地编辑区之下的层"
        );
        assert_eq!(
            snapshot.function_file("dup"),
            Some("dup:\n  steps: [] # override\n"),
            "override 函数库必须优先于包内"
        );

        // 本地编辑区重新出现同名 → 再次胜过 override
        std::fs::write(
            store
                .kind_dir(pkg, crate::resources::ResourceKind::Scripts)
                .join("dup.yaml"),
            b"steps: [] # local",
        )
        .unwrap();
        let snapshot = RunSnapshot::capture(&store, pkg).unwrap();
        assert_eq!(
            snapshot.script("dup.yaml"),
            Some("steps: [] # local"),
            "本地编辑区必须优先于 override 与包"
        );
    }
}
