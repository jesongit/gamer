//! gamer.yaml 的资源内容钩子（P11.3 / P11.6，v3-only）。
//!
//! Core [`crate::resources::PackageStore`] 只懂 PackageResource 三元组 +
//! 字节/文本 + 内容版本短码 + 原子写；本模块把 YAML 内容语义挂回通用层
//! （gamer.yaml 的插件数据根 = `packages/<pkg>/plugins/gamer.yaml/`，内部
//! 子目录布局 scripts/ functions/ templates/ 归插件定义）：
//!
//! - [`YamlResourceHandler`]（按 plugin-id 注册）：保存/更新前的 v3 校验
//!   （`scripts/` → `version: 3` 判别 + surface 解析/lowering，非 v3 源报版本
//!   门禁诊断；`functions/` → 函数库 bare-map 严格校验）+ 函数名清单注记 +
//!   模板重命名前的引用同步改写（v3 AST 改写，失败整体回滚；非 v3 存量源
//!   跳过不阻塞重命名）。
//!
//! 组合根引导期调用 [`register_resource_handlers`]；未注册时 Core 保存不做
//! 内容校验（裸 Core 语义，§8.9 验收锚点）。
//!
//! 资源 id 形态（全扩展统一）：`<package-id>/<名>`（首段 = Package id，目录
//! 由本模块按资源类别补全——脚本/函数寻址不含目录段，模板显式带
//! `templates/`）；REST 侧资源路径 = 插件目录内相对路径（含目录段）。

use std::sync::Arc;

use serde_json::json;

use crate::extensions::gamer_yaml::yaml_extension::YAML_EXTENSION_ID;
use crate::extensions::gamer_yaml::yaml_vnext;
use crate::resources::{
    PackageStore, ResourceEntry, ResourceHandler, SaveValidation,
};

/// 注册 gamer.yaml 的资源内容钩子（组合根引导期调用）。
pub fn register_resource_handlers(store: &PackageStore) {
    store.register_handler(YAML_EXTENSION_ID, Arc::new(YamlResourceHandler));
}

// ---------------------------------------------------------------------------
// 读取助手：`<pkg>/<rel>` 资源 id → scripts/ / functions/ 插件路径
//（runner_adapter / task_params / entrypoint_descriptor / timer_yaml 共用）
// ---------------------------------------------------------------------------

/// 拆分 `<pkg>/<rel>` 形态的资源 id（首段 = package id）。
fn split_resource_id(id: &str) -> Option<(String, String)> {
    let (pkg, rel) = id.split_once('/')?;
    Some((pkg.trim().to_string(), rel.trim().to_string()))
}

/// 脚本资源读取（`scripts/<rel>`）。
pub(crate) fn script_entry(
    store: &PackageStore,
    id: &str,
) -> anyhow::Result<Option<ResourceEntry>> {
    match split_resource_id(id) {
        Some((pkg, rel)) => store.read_text(&pkg, YAML_EXTENSION_ID, &format!("scripts/{rel}")),
        None => Ok(None),
    }
}

/// 函数库文件读取（`functions/<rel>`）。
pub(crate) fn function_entry(
    store: &PackageStore,
    id: &str,
) -> anyhow::Result<Option<ResourceEntry>> {
    match split_resource_id(id) {
        Some((pkg, rel)) => {
            store.read_text(&pkg, YAML_EXTENSION_ID, &format!("functions/{rel}"))
        }
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// 保存/更新校验（scripts / functions 路径前缀，v3-only）
// ---------------------------------------------------------------------------

/// v3 脚本校验：`yaml_vnext::load`（version 门禁 + surface 解析 + lowering）。
/// 非 `version: 3` 源（含 v2 存量形态）报 `yaml.v3.version` 诊断，无 fallback。
fn validate_v3_script(source: &str) -> Result<(), serde_json::Value> {
    yaml_vnext::load(source)
        .map(|_| ())
        .map_err(|diagnostics| serde_json::to_value(diagnostics).unwrap_or_default())
}

/// 函数库文件校验（v3 bare-map；保存边界与 preflight 共用）。
/// 校验含：函数名唯一（映射键承载）+ 合法字符集/非保留字、记录只允许
/// params/steps、steps 合法 v3 语法（call 裸 target 在解析期报错，与运行前
/// 一致）。
pub(crate) fn validate_function_library_file(
    _store: &PackageStore,
    _package: &str,
    _path: &str,
    content: &str,
) -> Result<(), serde_json::Value> {
    yaml_vnext::parse_function_library(content)
        .map(|_| ())
        .map_err(|diagnostics| serde_json::to_value(diagnostics).unwrap_or_default())
}

/// gamer.yaml 插件资源的统一内容钩子：按路径前缀分发到 v3 校验器。
struct YamlResourceHandler;

impl ResourceHandler for YamlResourceHandler {
    fn validate_save(&self, req: SaveValidation<'_>) -> Result<(), serde_json::Value> {
        if let Some(_rel) = req.path.strip_prefix("scripts/") {
            return validate_v3_script(req.content);
        }
        if let Some(_rel) = req.path.strip_prefix("functions/") {
            return validate_function_library_file(req.store, req.package, req.path, req.content);
        }
        // templates/ 等其余路径不做内容校验（字节内容由运行链路解码校验）
        Ok(())
    }

    fn annotate(&self, entries: &[(String, String)]) -> serde_json::Map<String, serde_json::Value> {
        // 只注记函数库文件（函数名清单 + 文件短路径）
        let mut out = serde_json::Map::new();
        for (path, content) in entries {
            let Some(rel) = path.strip_prefix("functions/") else {
                continue;
            };
            let short = rel
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
            out.insert(path.clone(), json!({ "functions": functions, "file": short }));
        }
        out
    }

    fn before_rename(
        &self,
        store: &PackageStore,
        package: &str,
        plugin: &str,
        old_path: &str,
        new_path: &str,
    ) -> anyhow::Result<()> {
        // 模板重命名 → 仅同步改写当前包 scripts/ 与 functions/ 中的模板引用；
        // 模板文件本身的移动由 PackageStore::rename_resource 在钩子之后原子执行。
        // 非模板路径不处理。
        let _ = plugin;
        if let (Some(old_name), Some(new_name)) = (
            old_path.strip_prefix("templates/"),
            new_path.strip_prefix("templates/"),
        ) {
            rewrite_template_references(store, package, old_name, new_name)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// templates 重命名：改写包内脚本/函数中的模板引用
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

/// 重命名模板前，同步改写当前包 scripts/ 与 functions/ 中的模板引用（仅引用，
/// 模板文件本身由调用方 [`PackageStore::rename_resource`] 移动）。
///
/// 引用迁移走 v3 AST 改写（[`yaml_vnext::rename_template_source`] /
/// [`yaml_vnext::rename_template_in_function_library`]），不做全局文本替换，
/// 避免误改日志/文本内容。非 v3 存量源（不可解析）跳过——它们本就无法运行，
/// 不阻塞重命名；v3 源改写失败（语法损坏）则整体报错。所有资源先生成新内容，
/// 再开始落盘，写入失败时回滚已改写的资源。
fn rewrite_template_references(
    store: &PackageStore,
    package: &str,
    old_name: &str,
    new_name: &str,
) -> anyhow::Result<usize> {
    let old_short = template_short_name(old_name);
    let new_short = template_short_name(new_name);
    // (path, 原内容, 新内容)
    let mut rewrites: Vec<(String, String, String)> = Vec::new();

    for script in store.list(package, YAML_EXTENSION_ID, "scripts")? {
        let Some(content) = script.content.as_deref() else {
            continue; // 非 UTF-8 附件不参与引用改写
        };
        let rewritten = yaml_vnext::rename_template_source(
            content,
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
            rewrites.push((script.path.clone(), script.content.clone().unwrap(), content));
        }
    }

    for function in store.list(package, YAML_EXTENSION_ID, "functions")? {
        let Some(content) = function.content.as_deref() else {
            continue;
        };
        // 非 v3 存量函数库解析失败 → 跳过（与脚本侧 skip 语义一致）
        let rewritten =
            yaml_vnext::rename_template_in_function_library(
                content,
                old_name,
                &old_short,
                new_name,
                &new_short,
            )
            .ok()
            .flatten();
        if let Some((content, _changed)) = rewritten {
            rewrites.push((function.path.clone(), function.content.clone().unwrap(), content));
        }
    }

    // 先写全部引用改写（任一失败回滚已写内容——模板文件此时未动，调用方
    // rename_resource 的 fs::rename 尚未发生）
    let mut written: Vec<(String, String)> = Vec::new();
    for (path, original, content) in &rewrites {
        if let Err(error) =
            store.write_text_unchecked(package, YAML_EXTENSION_ID, path, content)
        {
            for (path, original) in written.iter().rev() {
                let _ = store.write_text_unchecked(package, YAML_EXTENSION_ID, path, original);
            }
            return Err(error);
        }
        written.push((path.clone(), original.clone()));
    }
    Ok(rewrites.len())
}

#[cfg(test)]
mod rename_tests {
    use super::*;

    fn temp_store(tag: &str) -> (PackageStore, std::path::PathBuf) {
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
        let store = PackageStore::open(&cfg).unwrap();
        store
            .create_package(crate::resources::PackageInput {
                id: "com.test.app".into(),
                ..Default::default()
            })
            .unwrap();
        // 与生产组合根一致：注册 gamer.yaml 的内容钩子（rename_resource 经
        // handler.before_rename 改写模板引用）
        store.register_handler(YAML_EXTENSION_ID, Arc::new(YamlResourceHandler));
        (store, dir)
    }

    fn plugin_root(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("packages/com.test.app/plugins/gamer.yaml")
    }

    /// v3 脚本 + 函数库中的模板引用经 AST 同步改写；文本字面量不动。
    /// 文件移动由 PackageStore::rename_resource 完成（钩子只改引用）。
    #[test]
    fn rename_template_updates_script_and_function_references() {
        let (store, dir) = temp_store("v3");
        let templates = plugin_root(&dir).join("templates");
        std::fs::create_dir_all(&templates).unwrap();
        std::fs::write(templates.join("old.png"), b"png").unwrap();

        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "scripts/main.yaml",
                "version: 3\nsteps:\n  - find:\n      template: old.png\n      then:\n        - log: old.png 文本不应改\n",
                None,
                false,
            )
            .unwrap();
        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "functions/common.yaml",
                "login:\n  steps:\n    - find:\n        template: old.png\n",
                None,
                false,
            )
            .unwrap();

        store
            .rename_resource(
                "com.test.app",
                YAML_EXTENSION_ID,
                "templates/old.png",
                "templates/new.png",
            )
            .unwrap();
        assert!(!templates.join("old.png").exists(), "rename_resource 负责移动模板文件");
        assert_eq!(std::fs::read(templates.join("new.png")).unwrap(), b"png");
        let script =
            std::fs::read_to_string(plugin_root(&dir).join("scripts/main.yaml")).unwrap();
        assert!(script.contains("template: new.png"));
        assert!(script.contains("old.png 文本不应改"));
        let function =
            std::fs::read_to_string(plugin_root(&dir).join("functions/common.yaml")).unwrap();
        assert!(function.contains("template: new.png"));
    }

    /// v3 源面引用改写覆盖 find.then / match_first 候选，文本不误改。
    #[test]
    fn rename_template_updates_v3_surface_references_without_touching_text() {
        let (store, dir) = temp_store("surface");
        let templates = plugin_root(&dir).join("templates");
        std::fs::create_dir_all(&templates).unwrap();
        std::fs::write(templates.join("old.png"), b"png").unwrap();
        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "scripts/main.yaml",
                "version: 3\nsteps:\n  - find:\n      template: old.png\n      then:\n        - log: old.png 文本不应改\n  - match_first:\n      candidates: [old.png]\n",
                None,
                false,
            )
            .unwrap();

        store
            .rename_resource(
                "com.test.app",
                YAML_EXTENSION_ID,
                "templates/old.png",
                "templates/new.png",
            )
            .unwrap();
        let script =
            std::fs::read_to_string(plugin_root(&dir).join("scripts/main.yaml")).unwrap();
        assert!(script.contains("template: new.png"));
        assert!(script.contains("- new.png"));
        assert!(script.contains("old.png 文本不应改"));
    }

    /// 非 v3 存量源不可解析 → 跳过（不阻塞重命名）；v3 引用继续改写。
    #[test]
    fn rename_template_skips_unparsable_legacy_sources() {
        let (store, dir) = temp_store("legacy");
        let templates = plugin_root(&dir).join("templates");
        std::fs::create_dir_all(&templates).unwrap();
        std::fs::write(templates.join("old.png"), b"png").unwrap();
        // v2 形态存量脚本（legacy：保存边界已拒收，只可能来自历史盘上数据）
        let scripts = plugin_root(&dir).join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("legacy.yaml"), b"steps:\n  - check: old.png\n").unwrap();
        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "scripts/main.yaml",
                "version: 3\nsteps:\n  - find:\n      template: old.png\n",
                None,
                false,
            )
            .unwrap();

        store
            .rename_resource(
                "com.test.app",
                YAML_EXTENSION_ID,
                "templates/old.png",
                "templates/new.png",
            )
            .unwrap();
        let script =
            std::fs::read_to_string(plugin_root(&dir).join("scripts/main.yaml")).unwrap();
        assert!(script.contains("template: new.png"));
        let legacy = std::fs::read_to_string(scripts.join("legacy.yaml")).unwrap();
        assert!(legacy.contains("old.png"), "不可解析的存量源保持原样");
    }

    /// 引用改写整体原子性：v3 源改写失败（语法损坏）→ 整体重命名失败且不动文件。
    #[test]
    fn rename_template_fails_atomically_when_v3_rewrite_breaks() {
        let (store, dir) = temp_store("atomic-rename");
        let templates = plugin_root(&dir).join("templates");
        std::fs::create_dir_all(&templates).unwrap();
        std::fs::write(templates.join("old.png"), b"png").unwrap();
        // 坏 v3 源（模板引用可改写但整体解析失败）→ rename_resource 报错，
        // 模板文件保持原名原位
        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "scripts/broken.yaml",
                "version: 3\nsteps:\n  - find:\n      template: old.png\n",
                None,
                false,
            )
            .unwrap();
        let broken = plugin_root(&dir).join("scripts/broken.yaml");
        // 落盘后把它改成语法损坏的 v3 源（绕过保存校验，模拟历史盘上数据）
        std::fs::write(&broken, b"version: 3\nsteps:\n  - find:\n").unwrap();

        assert!(
            store
                .rename_resource(
                    "com.test.app",
                    YAML_EXTENSION_ID,
                    "templates/old.png",
                    "templates/new.png",
                )
                .is_err(),
            "v3 源改写失败必须整体失败"
        );
        assert!(templates.join("old.png").exists(), "失败时模板文件不动");
        assert!(!templates.join("new.png").exists());
    }

    /// 保存边界 v3-only：v3 直存、非 v3 源报版本门禁诊断（yaml.v3.version）。
    #[test]
    fn function_file_save_is_v3_only_with_version_gate() {
        let (store, _dir) = temp_store("dual");
        // v3 bare-map 函数库
        let v3_library = "领取奖励:\n  params:\n    - 'int:times:次数:2'\n  steps:\n    - log: $times\n    - if:\n        cond: $times > 0\n        then:\n          - log: ok\n";
        validate_function_library_file(&store, "com.test.app", "functions/common.yaml", v3_library)
            .expect("v3 函数库必须通过保存校验");
        // v3 嵌套文件短路径（functions 允许子目录，function:<短路径>/<函数名>）
        validate_function_library_file(
            &store,
            "com.test.app",
            "functions/sub/common.yaml",
            v3_library,
        )
        .unwrap();
        // v2 形态存量函数文件 → 版本门禁拒绝（v3 解析对 `- find: x` 标量步报错，
        // 但错误必须带 yaml.v3.* 码——存量文件不可再经 v2 loader 落盘）
        let legacy = "login:\n  steps:\n    - find: old.png\n";
        assert!(validate_function_library_file(&store, "com.test.app", "functions/legacy.yaml", legacy).is_err());
        // 双失败口径统一：坏 v3 → v3 诊断（非法 call 裸 target）
        let broken_v3 = "bad:\n  steps:\n    - call:\n        target: login\n";
        let diagnostics =
            validate_function_library_file(&store, "com.test.app", "functions/broken.yaml", broken_v3)
                .unwrap_err();
        let text = diagnostics.to_string();
        assert!(
            text.contains("yaml.v3.call") || text.contains("命名空间"),
            "坏 v3 call 目标必须报 v3 诊断: {text}"
        );
    }

    /// 保存钩子路径分发：scripts/ 走脚本校验，functions/ 走函数库校验，
    /// 其余路径（templates 等）放行。
    #[test]
    fn handler_validates_by_path_prefix() {
        let (store, _dir) = temp_store("dispatch");
        store.register_handler(YAML_EXTENSION_ID, Arc::new(YamlResourceHandler));
        // 脚本：v3 通过
        store
            .validate_save(SaveValidation {
                package: "com.test.app",
                plugin: YAML_EXTENSION_ID,
                path: "scripts/daily.yaml",
                content: "version: 3\nsteps:\n  - log: ok\n",
                store: &store,
            })
            .expect("v3 脚本必须通过");
        // 脚本：非 v3 → 版本门禁诊断
        let err = store
            .validate_save(SaveValidation {
                package: "com.test.app",
                plugin: YAML_EXTENSION_ID,
                path: "scripts/daily.yaml",
                content: "steps: []\n",
                store: &store,
            })
            .unwrap_err();
        assert_eq!(err[0]["code"], "yaml.v3.version.missing");
        // templates 路径不做校验
        store
            .validate_save(SaveValidation {
                package: "com.test.app",
                plugin: YAML_EXTENSION_ID,
                path: "templates/x.png",
                content: "随便什么",
                store: &store,
            })
            .expect("非 YAML 路径放行");
        // 函数名清单注记
        let meta = store.list("com.test.app", YAML_EXTENSION_ID, "").unwrap();
        assert!(meta.is_empty());
        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "functions/lib.yaml",
                "greet:\n  steps:\n    - return: true\n",
                None,
                false,
            )
            .unwrap();
        let list = store.list("com.test.app", YAML_EXTENSION_ID, "functions").unwrap();
        assert_eq!(list[0].meta["functions"][0], "greet");
        assert_eq!(list[0].meta["file"], "lib");
    }
}
