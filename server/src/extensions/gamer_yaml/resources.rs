//! gamer.yaml 的资源内容钩子（P11.3 / P11.6，v3-only）。
//!
//! Core [`crate::resources::ResourceStore`] 只懂目录类别 + 字节/文本 + 内容
//! 版本短码 + 原子写；本模块把 YAML 内容语义挂回通用层：
//!
//! - [`YamlScriptValidator`]（scripts kind）：保存/更新前的 v3 校验
//!   （`version: 3` 判别 + surface 解析/lowering；非 v3 源报版本门禁诊断）；
//! - [`YamlFunctionValidator`]（functions kind）：函数库 bare-map 严格校验 +
//!   顶层函数名清单注记（`functions` 字段，列表/读取透传给前端）；
//! - [`YamlTemplateHandler`]（templates kind）：重命名前同步改写分区脚本/
//!   函数中的模板引用（v3 AST 改写，不做全局文本替换，失败整体回滚；
//!   非 v3 存量源不可解析 → 跳过不阻塞重命名）；
//! - [`YamlStagedValidator`]：App Package 导出/提取 preflight 的 staged 集合
//!   校验。
//!
//! 组合根引导期调用 [`register_resource_handlers`]；未注册时 Core 保存不做
//! 内容校验（裸 Core 语义，§8.9 验收锚点）。

use std::sync::Arc;

use serde_json::json;

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
// 保存/更新校验（scripts / functions kind，v3-only）
// ---------------------------------------------------------------------------

/// v3 脚本校验：`yaml_vnext::load`（version 门禁 + surface 解析 + lowering）。
/// 非 `version: 3` 源（含 v2 存量形态）报 `yaml.v3.version` 诊断，无 fallback。
fn validate_v3_script(source: &str) -> Result<(), serde_json::Value> {
    yaml_vnext::load(source)
        .map(|_| ())
        .map_err(|diagnostics| serde_json::to_value(diagnostics).unwrap_or_default())
}

/// scripts kind 校验器（v3-only，与运行链路同源）。
struct YamlScriptValidator;

impl ResourceKindHandler for YamlScriptValidator {
    fn validate_save(&self, req: SaveValidation<'_>) -> Result<(), serde_json::Value> {
        validate_v3_script(req.content)
    }
}

/// 函数库文件校验（v3 bare-map；保存边界与导出/提取 preflight 共用）。
/// 校验含：函数名唯一（映射键承载）+ 合法字符集/非保留字、记录只允许
/// params/steps、steps 合法 v3 语法（call 裸 target 在解析期报错，与运行前
/// 一致）。
pub(crate) fn validate_function_library_file(
    _store: &ResourceStore,
    _app: &str,
    _id: &str,
    content: &str,
) -> Result<(), serde_json::Value> {
    yaml_vnext::parse_function_library(content)
        .map(|_| ())
        .map_err(|diagnostics| serde_json::to_value(diagnostics).unwrap_or_default())
}

/// functions kind 校验器：v3 bare-map 验收 + 函数名清单注记。
struct YamlFunctionValidator;

impl ResourceKindHandler for YamlFunctionValidator {
    fn validate_save(&self, req: SaveValidation<'_>) -> Result<(), serde_json::Value> {
        validate_function_library_file(req.store, req.app, req.id, req.content)
    }

    fn annotate(&self, entries: &[(String, String)]) -> serde_json::Map<String, serde_json::Value> {
        let mut out = serde_json::Map::new();
        for (id, content) in entries {
            let short = id
                .trim()
                .trim_end_matches(".yaml")
                .trim_end_matches(".yml")
                .to_string();
            let functions = yaml_vnext::parse_function_library(content)
                .ok()
                .map(|library| {
                    library
                        .into_iter()
                        .map(|decl| decl.name)
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

/// 重命名模板文件，并同步改写当前分区 scripts/ 与 functions/ 中的模板引用。
///
/// 引用迁移走 v3 AST 改写（[`yaml_vnext::rename_template_source`] /
/// [`yaml_vnext::rename_template_in_function_library`]），不做全局文本替换，
/// 避免误改日志/文本内容。非 v3 存量源（不可解析）跳过——它们本就无法运行，
/// 不阻塞重命名；v3 源改写失败（语法损坏）则整体报错。所有资源先生成新内容，
/// 再开始落盘，写入失败时回滚已改写的资源。
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
        let rewritten = yaml_vnext::rename_template_source(
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
        })?;
        if let Some((content, _changed)) = rewritten {
            rewrites.push((
                ResourceKind::Scripts,
                script.name.clone(),
                script.content.clone(),
                content,
            ));
        }
    }

    for function in store.list_text(package, ResourceKind::Functions)? {
        // 非 v3 存量函数库解析失败 → 跳过（与脚本侧 skip 语义一致）
        let rewritten =
            yaml_vnext::rename_template_in_function_library(
                &function.content,
                old_name,
                &old_short,
                new_name,
                &new_short,
            )
            .ok()
            .flatten();
        if let Some((content, _changed)) = rewritten {
            rewrites.push((
                ResourceKind::Functions,
                function.name.clone(),
                function.content.clone(),
                content,
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
// 包导出/提取 preflight：staged 集合校验
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
        for (kind, rel, content) in entries {
            let result = match kind {
                ResourceKind::Scripts => validate_v3_script(content),
                // 函数库：v3 bare-map（与保存边界同源）
                ResourceKind::Functions => {
                    validate_function_library_file(store, app, rel, content)
                }
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

    /// v3 脚本 + 函数库中的模板引用经 AST 同步改写；文本字面量不动。
    #[test]
    fn rename_template_updates_script_and_function_references() {
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
                "version: 3\nsteps:\n  - find:\n      template: old.png\n      then:\n        - log: old.png 文本不应改\n",
            )
            .unwrap();
        store
            .save_text(
                ResourceKind::Functions,
                None,
                "com.test.app",
                "common.yaml",
                "login:\n  steps:\n    - find:\n        template: old.png\n",
            )
            .unwrap();

        assert_eq!(
            rename_template_references(&store, "com.test.app", "old.png", "new.png").unwrap(),
            2
        );
        assert!(!templates.join("old.png").exists());
        assert_eq!(std::fs::read(templates.join("new.png")).unwrap(), b"png");
        let script = std::fs::read_to_string(dir.join("com.test.app/scripts/main.yaml")).unwrap();
        assert!(script.contains("template: new.png"));
        assert!(script.contains("old.png 文本不应改"));
        let function =
            std::fs::read_to_string(dir.join("com.test.app/functions/common.yaml")).unwrap();
        assert!(function.contains("template: new.png"));
    }

    /// v3 源面引用改写覆盖 find.then / match_first 候选，文本不误改。
    #[test]
    fn rename_template_updates_v3_surface_references_without_touching_text() {
        let (store, dir) = temp_store("surface");
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

    /// 非 v3 存量源不可解析 → 跳过（不阻塞重命名）；v3 引用继续改写。
    #[test]
    fn rename_template_skips_unparsable_legacy_sources() {
        let (store, dir) = temp_store("legacy");
        let templates = dir.join("com.test.app").join("templates");
        std::fs::create_dir_all(&templates).unwrap();
        atomic_write(&templates.join("old.png"), b"png").unwrap();
        // v2 形态存量脚本（legacy：保存边界已拒收，只可能来自历史盘上数据）
        let scripts = dir.join("com.test.app").join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("legacy.yaml"), b"steps:\n  - check: old.png\n").unwrap();
        store
            .save_text(
                ResourceKind::Scripts,
                None,
                "com.test.app",
                "main.yaml",
                "version: 3\nsteps:\n  - find:\n      template: old.png\n",
            )
            .unwrap();

        assert_eq!(
            rename_template_references(&store, "com.test.app", "old.png", "new.png").unwrap(),
            1,
            "只有 v3 脚本计入改写"
        );
        let script = std::fs::read_to_string(dir.join("com.test.app/scripts/main.yaml")).unwrap();
        assert!(script.contains("template: new.png"));
        let legacy = std::fs::read_to_string(scripts.join("legacy.yaml")).unwrap();
        assert!(legacy.contains("old.png"), "不可解析的存量源保持原样");
    }

    /// 保存边界 v3-only：v3 直存、非 v3 源报版本门禁诊断（yaml.v3.version）。
    #[test]
    fn function_file_save_is_v3_only_with_version_gate() {
        let (store, _dir) = temp_store("dual");
        // v3 bare-map 函数库
        let v3_library = "领取奖励:\n  params:\n    - 'int:times:次数:2'\n  steps:\n    - log: $times\n    - if:\n        cond: $times > 0\n        then:\n          - log: ok\n";
        validate_function_library_file(&store, "com.test.app", "common.yaml", v3_library)
            .expect("v3 函数库必须通过保存校验");
        // v3 嵌套文件短路径（functions allow_nested，function:<短路径>/<函数名>）
        validate_function_library_file(
            &store,
            "com.test.app",
            "sub/common.yaml",
            v3_library,
        )
        .unwrap();
        // v2 形态存量函数文件 → 版本门禁拒绝（v3 解析对 `- find: x` 标量步报错，
        // 但错误必须带 yaml.v3.* 码——存量文件不可再经 v2 loader 落盘）
        let legacy = "login:\n  steps:\n    - find: old.png\n";
        assert!(validate_function_library_file(&store, "com.test.app", "legacy.yaml", legacy).is_err());
        // 双失败口径统一：坏 v3 → v3 诊断（非法 call 裸 target）
        let broken_v3 = "bad:\n  steps:\n    - call:\n        target: login\n";
        let diagnostics =
            validate_function_library_file(&store, "com.test.app", "broken.yaml", broken_v3)
                .unwrap_err();
        let text = diagnostics.to_string();
        assert!(
            text.contains("yaml.v3.call") || text.contains("命名空间"),
            "坏 v3 call 目标必须报 v3 诊断: {text}"
        );
    }
}
