//! gamer.yaml 的资源内容钩子（P11.3 / P11.6）。
//!
//! Core [`crate::resources::ResourceStore`] 只懂目录类别 + 字节/文本 + 内容
//! 版本短码 + 原子写；本模块把 YAML 内容语义挂回通用层：
//!
//! - [`YamlScriptValidator`]（scripts kind）：保存/更新前的 v2/v3 双格式校验
//!   （v2 走严格 loader + call/func 引用视图，v3 走 yaml_vnext）；
//! - [`YamlFunctionValidator`]（functions kind）：函数库文件严格校验 +
//!   顶层函数名清单注记（`functions` 字段，列表/读取透传给前端）；
//! - [`YamlTemplateHandler`]（templates kind）：重命名前同步改写分区脚本/
//!   函数中的模板引用（严格 AST，不做全局文本替换，失败整体回滚）；
//! - [`YamlStagedValidator`]：App Package 导出/提取 preflight 的 staged 集合
//!   校验（跨文件 call/func 引用以 staged 内容自身为最高优先视图）。
//!
//! 组合根引导期调用 [`register_resource_handlers`]；未注册时 Core 保存不做
//! 内容校验（裸 Core 语义，§8.9 验收锚点）。

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use crate::extensions::gamer_yaml::script_v2::error::codes;
use crate::extensions::gamer_yaml::script_v2::validate::{
    normalize_id, try_build_function_file, ResourceProvider,
};
use crate::extensions::gamer_yaml::script_v2::{
    parse_function_file, parse_script_file, Cell, ParamDecl, ScriptError, Step, TypedValue,
};
use crate::extensions::gamer_yaml::yaml_vnext;
use crate::resources::{
    ResourceKind, ResourceKindHandler, ResourceStore, SaveValidation, StagedResourceValidator,
    Templates,
};

/// 注册 gamer.yaml 的全部资源内容钩子（组合根引导期调用）。
pub fn register_resource_handlers(store: &ResourceStore) {
    store.register_handler(ResourceKind::Scripts, Arc::new(YamlScriptValidator));
    store.register_handler(ResourceKind::Functions, Arc::new(YamlFunctionValidator));
    store.register_handler(Templates, Arc::new(YamlTemplateHandler));
    store.set_staged_validator(Arc::new(YamlStagedValidator));
}

// ---------------------------------------------------------------------------
// ResourceStore 视图：当前分区内容 + 待写覆盖（v2 严格 loader 引用解析）
// ---------------------------------------------------------------------------

/// 一个分区的 loader 资源视图，可叠加尚未落盘的脚本与函数库。
pub struct StoreView<'a> {
    store: &'a ResourceStore,
    pkg: String,
    script_overrides: HashMap<String, String>,
    function_overrides: HashMap<String, String>,
}

impl<'a> StoreView<'a> {
    pub fn new(store: &'a ResourceStore, pkg: &str) -> Self {
        Self {
            store,
            pkg: pkg.to_string(),
            script_overrides: HashMap::new(),
            function_overrides: HashMap::new(),
        }
    }

    pub fn add_script(&mut self, resource: &str, content: &str) {
        let key = normalize_id(resource.trim());
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
            .get(&normalize_id(resource.trim()))
            .cloned()
    }

    fn function_content_override(&self, file_short: &str) -> Option<String> {
        let key = file_short
            .trim()
            .trim_end_matches(".yaml")
            .trim_end_matches(".yml");
        self.function_overrides.get(key).cloned()
    }
}

impl ResourceProvider for StoreView<'_> {
    fn script_exists(&self, resource_id: &str) -> bool {
        self.script_content(resource_id).is_some()
    }

    fn script_content(&self, resource_id: &str) -> Option<String> {
        if let Some(content) = self.script_content_override(resource_id) {
            return Some(content);
        }
        let key = normalize_id(resource_id.trim());
        let candidates = [key.clone(), format!("{key}.yaml"), format!("{key}.yml")];
        candidates
            .iter()
            .find_map(|candidate| {
                self.store
                    .get_text(
                        ResourceKind::Scripts,
                        &format!("{}/{}", self.pkg, candidate),
                    )
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
        self.store
            .get_text(ResourceKind::Functions, &format!("{}/{}", self.pkg, rel))
            .ok()
            .flatten()
            .map(|file| file.content)
    }

    fn function_exists(&self, file_short: &str, function: &str) -> bool {
        self.function_file_content(file_short)
            .and_then(|content| try_build_function_file(&content))
            .is_some_and(|file| file.find(function).is_some())
    }

    fn resolve_template(
        &self,
        short_name: &str,
    ) -> crate::extensions::gamer_yaml::script_v2::validate::TemplateAvail {
        super::engine::snapshot::template_avail(self.store, &self.pkg, short_name)
    }
}

// ---------------------------------------------------------------------------
// 保存/更新校验（scripts / functions kind）
// ---------------------------------------------------------------------------

/// v2/v3 双格式校验结果（保存边界只用成败与诊断 JSON）。
/// v2/v3 判别（载荷当前仅作判别保留；调用方未消费 AST 本体）。
#[derive(Debug)]
pub(crate) enum CompatibleYamlSource {
    V2,
    #[allow(dead_code)]
    V3(yaml_vnext::Program),
}

#[derive(Debug)]
pub(crate) enum CompatibleYamlError {
    V2(Vec<ScriptError>),
    V3(Vec<yaml_vnext::Diagnostic>),
}

impl CompatibleYamlError {
    pub(crate) fn into_json(self) -> serde_json::Value {
        match self {
            Self::V2(diagnostics) => serde_json::to_value(diagnostics).unwrap_or_default(),
            Self::V3(diagnostics) => serde_json::to_value(diagnostics).unwrap_or_default(),
        }
    }
}

/// 保存边界校验：无目录覆盖层，call/func 引用解析 = 本地编辑区视图 + 待写
/// 文件自身。v2 与 v3 各自走独立 loader，不做版本猜测。
pub(crate) fn validate_compatible_script(
    store: &ResourceStore,
    package: &str,
    resource: &str,
    source: &str,
) -> Result<CompatibleYamlSource, CompatibleYamlError> {
    let mut view = StoreView::new(store, package);
    validate_compatible_script_in(resource, source, &mut view)
}

/// Same as [`validate_compatible_script`], but the caller supplies the v2
/// reference view (call/func targets). PackageBuilder preflight over a staged
/// directory snapshot pre-injects that directory's own scripts/functions so
/// extraction into an empty workspace validates self-consistently; save
/// boundaries pass a fresh view (equivalent to the plain variant).
pub(crate) fn validate_compatible_script_in(
    resource: &str,
    source: &str,
    view: &mut StoreView<'_>,
) -> Result<CompatibleYamlSource, CompatibleYamlError> {
    if yaml_vnext::is_v3_source(source) {
        yaml_vnext::load(source)
            .map(CompatibleYamlSource::V3)
            .map_err(CompatibleYamlError::V3)
    } else {
        view.add_script(resource, source);
        parse_script_file(source, resource, view)
            .map(|_| CompatibleYamlSource::V2)
            .map_err(CompatibleYamlError::V2)
    }
}

/// scripts kind 校验器（v2/v3 双格式，与保存/导入链路同源）。
struct YamlScriptValidator;

impl ResourceKindHandler for YamlScriptValidator {
    fn validate_save(&self, req: SaveValidation<'_>) -> Result<(), serde_json::Value> {
        match validate_compatible_script(req.store, req.app, req.id, req.content) {
            Ok(_) => Ok(()),
            Err(error) => Err(error.into_json()),
        }
    }
}

/// functions kind 校验器：严格 loader（顶层键 = 函数名）+ 函数名清单注记。
struct YamlFunctionValidator;

impl ResourceKindHandler for YamlFunctionValidator {
    fn validate_save(&self, req: SaveValidation<'_>) -> Result<(), serde_json::Value> {
        let mut view = StoreView::new(req.store, req.app);
        let rel = req
            .id
            .trim()
            .trim_end_matches(".yaml")
            .trim_end_matches(".yml");
        view.add_function(rel, req.content);
        parse_function_file(req.content, rel, &view)
            .map(|_| ())
            .map_err(|errors| serde_json::to_value(errors).unwrap_or_default())
    }

    fn annotate(&self, entries: &[(String, String)]) -> serde_json::Map<String, serde_json::Value> {
        let mut out = serde_json::Map::new();
        for (id, content) in entries {
            let short = id
                .trim()
                .trim_end_matches(".yaml")
                .trim_end_matches(".yml")
                .to_string();
            let functions = try_build_function_file(content)
                .map(|file| {
                    file.functions
                        .iter()
                        .map(|f| f.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            out.insert(id.clone(), json!({ "functions": functions, "file": short }));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// templates kind：重命名前改写分区脚本/函数中的模板引用
// ---------------------------------------------------------------------------

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
    value: &mut TypedValue,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
) -> bool {
    let TypedValue::Tmpl(current) = value else {
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
    cell: &mut Cell,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
) -> usize {
    let Cell::Lit(value) = cell else {
        return 0;
    };
    usize::from(rename_template_value(
        value, old_name, old_short, new_name, new_short,
    ))
}

fn rename_template_steps(
    steps: &mut [Step],
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
) -> usize {
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
    params: &mut [ParamDecl],
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

/// 重命名模板文件，并同步改写当前分区 scripts/ 与 functions/ 中的模板引用。
///
/// 引用迁移走严格 AST，不做全局文本替换，避免误改日志/文本内容；同时处理
/// 模板参数默认值、步骤字段、match/color 候选与 call/func 实参。所有资源先
/// 解析并生成新内容，再开始落盘，写入失败时回滚已改写的资源。
fn rename_template_references(
    store: &ResourceStore,
    package: &str,
    old_name: &str,
    new_name: &str,
) -> anyhow::Result<usize> {
    let old_path = store.kind_dir(package, Templates).join(old_name);
    let template_bytes = std::fs::read(&old_path)?;
    let old_short = template_short_name(old_name);
    let new_short = template_short_name(new_name);
    // (kind, rel, 原内容, 新内容)
    let mut rewrites: Vec<(ResourceKind, String, String, String)> = Vec::new();

    for script in store.list_text(package, ResourceKind::Scripts)? {
        if yaml_vnext::is_v3_source(&script.content) {
            if let Some((rewritten, _changed)) = yaml_vnext::rename_template_source(
                &script.content,
                old_name,
                &old_short,
                new_name,
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
                    ResourceKind::Scripts,
                    script.name.clone(),
                    script.content.clone(),
                    rewritten,
                ));
            }
            continue;
        }
        let mut view = StoreView::new(store, package);
        view.add_script(&script.name, &script.content);
        let mut parsed =
            parse_script_file(&script.content, &script.name, &view).map_err(|errors| {
                anyhow::anyhow!(errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("；"))
            })?;
        let changed = rename_template_in_params(
            &mut parsed.params,
            old_name,
            &old_short,
            new_name,
            &new_short,
        ) + rename_template_steps(
            &mut parsed.steps,
            old_name,
            &old_short,
            new_name,
            &new_short,
        );
        if changed > 0 {
            rewrites.push((
                ResourceKind::Scripts,
                script.name.clone(),
                script.content.clone(),
                crate::extensions::gamer_yaml::script_v2::serialize_script(&parsed),
            ));
        }
    }

    for function in store.list_text(package, ResourceKind::Functions)? {
        let mut view = StoreView::new(store, package);
        let rel = function.name.trim().trim_end_matches(".yaml").to_string();
        view.add_function(&rel, &function.content);
        let mut parsed = parse_function_file(&function.content, &rel, &view).map_err(|errors| {
            anyhow::anyhow!(errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("；"))
        })?;
        let mut changed = 0;
        for declaration in &mut parsed.functions {
            changed += rename_template_in_params(
                &mut declaration.params,
                old_name,
                &old_short,
                new_name,
                &new_short,
            );
            changed += rename_template_steps(
                &mut declaration.steps,
                old_name,
                &old_short,
                new_name,
                &new_short,
            );
        }
        if changed > 0 {
            rewrites.push((
                ResourceKind::Functions,
                function.name.clone(),
                function.content.clone(),
                crate::extensions::gamer_yaml::script_v2::serialize_function_file(&parsed),
            ));
        }
    }

    // 先写全部引用改写（失败回滚），最后写新模板 + 删旧模板（失败回滚）
    let mut written: Vec<(ResourceKind, String, String)> = Vec::new();
    for (kind, rel, original, content) in &rewrites {
        if let Err(error) = store.write_text_direct(*kind, package, rel, content) {
            for (kind, rel, original) in written.iter().rev() {
                let _ = store.write_text_direct(*kind, package, rel, original);
            }
            return Err(error);
        }
        written.push((*kind, rel.clone(), original.clone()));
    }

    let new_path = store.kind_dir(package, Templates).join(new_name);
    let dir = store.kind_dir(package, Templates);
    if let Some(parent) = new_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Err(error) = crate::core::fs::atomic_write(&new_path, &template_bytes) {
        for (kind, rel, original) in written.iter().rev() {
            let _ = store.write_text_direct(*kind, package, rel, original);
        }
        return Err(error);
    }
    if let Err(error) = std::fs::remove_file(&old_path) {
        let _ = std::fs::remove_file(&new_path);
        for (kind, rel, original) in written.iter().rev() {
            let _ = store.write_text_direct(*kind, package, rel, original);
        }
        return Err(error.into());
    }
    let _ = dir;
    Ok(rewrites.len())
}

struct YamlTemplateHandler;

impl ResourceKindHandler for YamlTemplateHandler {
    fn before_rename(
        &self,
        store: &ResourceStore,
        app: &str,
        old: &str,
        new: &str,
    ) -> anyhow::Result<()> {
        rename_template_references(store, app, old, new).map(|_| ())
    }
}

// ---------------------------------------------------------------------------
// 包导出/提取 preflight：staged 集合校验（跨文件引用自洽）
// ---------------------------------------------------------------------------

struct YamlStagedValidator;

impl StagedResourceValidator for YamlStagedValidator {
    fn validate_staged(
        &self,
        store: &ResourceStore,
        app: &str,
        entries: &[(ResourceKind, String, String)],
    ) -> Vec<String> {
        let mut problems = Vec::new();
        let mut view = StoreView::new(store, app);
        for (kind, rel, content) in entries {
            match kind {
                ResourceKind::Scripts => view.add_script(rel, content),
                ResourceKind::Functions => view.add_function(rel, content),
                _ => {}
            }
        }
        for (kind, rel, content) in entries {
            let result = match kind {
                ResourceKind::Scripts => validate_compatible_script_in(rel, content, &mut view)
                    .map(|_| ())
                    .map_err(|error| error.into_json()),
                ResourceKind::Functions => parse_function_file(content, rel, &view)
                    .map(|_| ())
                    .map_err(|errors| serde_json::to_value(errors).unwrap_or_default()),
                _ => Ok(()),
            };
            if let Err(diagnostics) = result {
                problems.push(format!(
                    "{}/{rel}: {}",
                    kind.as_str(),
                    crate::resources::format_diagnostics_value(&diagnostics)
                ));
            }
        }
        problems
    }
}

#[cfg(test)]
mod rename_tests {
    use super::*;
    use crate::core::fs::atomic_write;

    fn temp_store(tag: &str) -> (ResourceStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "gamer-yamlrename-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = crate::config::Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        (ResourceStore::open(&cfg).unwrap(), dir)
    }

    /// v2 脚本 + 函数库中的模板引用经严格 AST 同步改写；文本字面量不动。
    #[test]
    fn rename_template_updates_script_and_function_references() {
        let (store, dir) = temp_store("v2");
        let templates = dir.join("com.test.app").join("templates");
        std::fs::create_dir_all(&templates).unwrap();
        atomic_write(&templates.join("old.png"), b"png").unwrap();

        store
            .save_text(
                ResourceKind::Scripts,
                None,
                "com.test.app",
                "main.yaml",
                "steps:\n  - check: old.png\n    timeout: 0s\n  - log: old.png 文本不应改\n",
            )
            .unwrap();
        store
            .save_text(
                ResourceKind::Functions,
                None,
                "com.test.app",
                "common.yaml",
                "login:\n  steps:\n    - find: old.png\n",
            )
            .unwrap();

        assert_eq!(
            rename_template_references(&store, "com.test.app", "old.png", "new.png").unwrap(),
            2
        );
        assert!(!templates.join("old.png").exists());
        assert_eq!(std::fs::read(templates.join("new.png")).unwrap(), b"png");
        let script = std::fs::read_to_string(dir.join("com.test.app/scripts/main.yaml")).unwrap();
        assert!(script.contains("check: new.png"));
        assert!(script.contains("old.png 文本不应改"));
        let function =
            std::fs::read_to_string(dir.join("com.test.app/functions/common.yaml")).unwrap();
        assert!(function.contains("find: new.png"));
    }

    /// v3 源面引用同样改写（yaml_vnext 路径），文本不误改。
    #[test]
    fn rename_template_updates_v3_surface_references_without_touching_text() {
        let (store, dir) = temp_store("v3");
        let templates = dir.join("com.test.app").join("templates");
        std::fs::create_dir_all(&templates).unwrap();
        atomic_write(&templates.join("old.png"), b"png").unwrap();
        store
            .save_text(
                ResourceKind::Scripts,
                None,
                "com.test.app",
                "main.yaml",
                "version: 3\nsteps:\n  - find:\n      template: old.png\n      then:\n        - log: old.png 文本不应改\n  - match_first:\n      candidates: [old.png]\n",
            )
            .unwrap();

        let renamed =
            rename_template_references(&store, "com.test.app", "old.png", "new.png").unwrap();
        assert_eq!(renamed, 1);
        let script = std::fs::read_to_string(dir.join("com.test.app/scripts/main.yaml")).unwrap();
        assert!(script.contains("template: new.png"));
        assert!(script.contains("- new.png"));
        assert!(script.contains("old.png 文本不应改"));
    }
}
