//! gamer.yaml 的资源内容钩子（V1）。
//!
//! Core [`crate::resources::PackageStore`] 只懂 PackageResource 三元组 +
//! 字节/文本 + 内容版本短码 + 原子写；本模块把 YAML 内容语义挂回通用层
//! （gamer.yaml 的插件数据根 = `packages/<pkg>/plugins/gamer.yaml/`，内部
//! 子目录布局 automations/ templates/ 归插件定义）：
//!
//! - [`YamlResourceHandler`]（按 plugin-id 注册）：保存/更新前的 V1 结构校验
//!   （`automations/` → 文件名 `_function` 前缀 = `syntax::parse_function_library`，
//!   其余 = `syntax::parse_script`；`templates/` → 字节侧 8-bit 灰度 PNG
//!   归一化）+ 函数名清单注记 + 模板重命名前的引用同步改写（AST 改写，
//!   失败整体回滚；不可解析的存量源跳过不阻塞重命名）。
//!
//! 函数库与自动化共用 automations/ 资源空间（简化计划 Phase 1）：文件名以
//! `_function` 开头且以 `.yaml` 结尾 = 函数库（[`is_function_library_path`]），
//! 其余 `.yaml` = 自动化。旧 `functions/` 专属目录已删除，保存钩子显式拒绝。
//!
//! 保存边界只做**结构**校验；函数存在性/参数匹配在运行前的注册表组合期
//! 校验（runner_adapter::compose_function_library：执行前明确提示）。
//!
//! 组合根引导期调用 [`register_resource_handlers`]；未注册时 Core 保存不做
//! 内容校验（裸 Core 语义）。
//!
//! 资源 id 形态（全扩展统一）：`<package-id>/<名>`（首段 = Package id，目录
//! 由本模块按资源类别补全——自动化脚本/函数库寻址不含目录段，模板显式带
//! `templates/`）；REST 侧资源路径 = 插件目录内相对路径（含目录段）。

use std::borrow::Cow;
use std::sync::Arc;

use serde_json::json;

use crate::extensions::gamer_yaml::syntax;
use crate::extensions::gamer_yaml::yaml_extension::YAML_EXTENSION_ID;
use crate::resources::{
    PackageStore, ResourceEntry, ResourceHandler, SaveBinaryValidation, SaveValidation,
};

/// 函数库文件识别（简化计划 Phase 1 §3.2）：文件名（basename）以小写
/// `_function` 开头且以 `.yaml` 结尾 → 函数库；其余 `.yaml` → 自动化。
/// 只用于资源发现，不新增 `kind` 字段、不区分大小写别名、不接受 `.yml`。
pub(crate) fn is_function_library_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.starts_with("_function") && name.ends_with(".yaml")
}

/// 旧 `functions/` 专属目录已随 Phase 1 删除；写路径显式拒绝（无兼容层）。
pub(crate) fn is_removed_functions_dir(path: &str) -> bool {
    path == "functions" || path.starts_with("functions/")
}

/// 注册 gamer.yaml 的资源内容钩子（组合根引导期调用）。
pub fn register_resource_handlers(store: &PackageStore) {
    store.register_handler(YAML_EXTENSION_ID, Arc::new(YamlResourceHandler));
}

// ---------------------------------------------------------------------------
// 读取助手：`<pkg>/<rel>` 资源 id → automations/ 插件路径
//（runner_adapter / task_params / entrypoint_descriptor / timer_yaml 共用）
// ---------------------------------------------------------------------------

/// 拆分 `<pkg>/<rel>` 形态的资源 id（首段 = package id）。
fn split_resource_id(id: &str) -> Option<(String, String)> {
    let (pkg, rel) = id.split_once('/')?;
    Some((pkg.trim().to_string(), rel.trim().to_string()))
}

/// 自动化脚本/函数库资源读取（`automations/<rel>`；两类文件共用同一空间）。
pub(crate) fn script_entry(
    store: &PackageStore,
    id: &str,
) -> anyhow::Result<Option<ResourceEntry>> {
    match split_resource_id(id) {
        Some((pkg, rel)) => store.read_text(&pkg, YAML_EXTENSION_ID, &format!("automations/{rel}")),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// 保存/更新校验（automations / functions 路径前缀，V1 结构校验）
// ---------------------------------------------------------------------------

/// V1 脚本校验：结构解析（顶层字段、步骤形态、表达式形态）。旧 v3 源因
/// `version` 字段/未知顶层字段直接被拒（`yaml.version.removed`）。
fn validate_v1_script(source: &str) -> Result<(), serde_json::Value> {
    syntax::parse_script(source)
        .map(|_| ())
        .map_err(|diagnostics| serde_json::to_value(diagnostics).unwrap_or_default())
}

/// 函数库文件校验（V1 `functions:` 包装结构；保存边界与 preflight 共用）。
pub(crate) fn validate_function_library_file(
    _store: &PackageStore,
    _package: &str,
    _path: &str,
    content: &str,
) -> Result<(), serde_json::Value> {
    syntax::parse_function_library(content)
        .map(|_| ())
        .map_err(|diagnostics| serde_json::to_value(diagnostics).unwrap_or_default())
}

/// gamer.yaml 插件资源的统一内容钩子：按路径前缀分发到 V1 校验器。
struct YamlResourceHandler;

impl ResourceHandler for YamlResourceHandler {
    fn validate_save(&self, req: SaveValidation<'_>) -> Result<(), serde_json::Value> {
        if let Some(rel) = req.path.strip_prefix("automations/") {
            // 前缀识别（Phase 1）：`_function*.yaml` = 函数库（functions: 包装），
            // 其余 = 自动化脚本。识别只看文件名，无 kind 字段。
            return if is_function_library_path(rel) {
                validate_function_library_file(req.store, req.package, req.path, req.content)
            } else {
                validate_v1_script(req.content)
            };
        }
        if is_removed_functions_dir(req.path) {
            // 旧专属目录已删除：显式结构化拒绝（无静默兼容、无自动迁移）。
            return Err(json!([
                {
                    "code": "yaml.functions.dir.removed",
                    "path": req.path,
                    "message": "functions/ 专属目录已删除：函数库请保存为 automations/_function.yaml（或 _function*.yaml 手动拆分）",
                }
            ]));
        }
        // templates/ 等其余路径不做文本内容校验（字节内容由下方二进制钩子归一化）
        Ok(())
    }

    fn validate_save_binary<'a>(
        &self,
        req: SaveBinaryValidation<'a>,
    ) -> Result<Cow<'a, [u8]>, serde_json::Value> {
        // templates/ = 本插件的字节语义区：统一归一化为 8-bit 灰度 PNG
        // （匹配链路只消费灰度；`#1` 颜色复核标记由文件名承载，归一化不改名）。
        // 非法图片字节报结构化诊断（HTTP 400）。其余路径不解释。
        if req.path.strip_prefix("templates/").is_some() {
            return match crate::matcher::reencode_template_png(req.bytes, true) {
                Ok(normalized) => Ok(Cow::Owned(normalized)),
                Err(error) => Err(json!([
                    {
                        "code": "template.png.invalid",
                        "path": req.path,
                        "message": error.to_string(),
                    }
                ])),
            };
        }
        Ok(Cow::Borrowed(req.bytes))
    }

    fn annotate(&self, entries: &[(String, String)]) -> serde_json::Map<String, serde_json::Value> {
        // 只注记 automations/ 内的函数库文件（函数名清单 + 文件短路径）
        let mut out = serde_json::Map::new();
        for (path, content) in entries {
            let Some(rel) = path.strip_prefix("automations/") else {
                continue;
            };
            if !is_function_library_path(rel) {
                continue;
            }
            let short = rel
                .trim()
                .trim_end_matches(".yaml")
                .trim_end_matches(".yml")
                .to_string();
            let functions = syntax::parse_function_library(content)
                .ok()
                .map(|library| {
                    library
                        .into_iter()
                        .map(|(name, _)| name)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            out.insert(
                path.clone(),
                json!({ "functions": functions, "file": short }),
            );
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
        // 模板重命名 → 仅同步改写当前包 automations/（脚本 + 函数库）中的模板
        // 引用；模板文件本身的移动由 PackageStore::rename_resource 在钩子之后
        // 原子执行。非模板路径不处理。
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
/// 保留扩展名。脚本通常引用短名，重命名模板时需要同时迁移这种引用；
/// 消费方还有 actions.rs（模板动作的短名冲突检测）。
pub(crate) fn template_short_name(name: &str) -> String {
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

/// 重命名模板前，同步改写当前包 automations/（脚本 + `_function*.yaml` 函数库）
/// 中的模板引用（仅引用，模板文件本身由调用方 [`PackageStore::rename_resource`]
/// 移动）。
///
/// 引用迁移走 V1 AST 改写（`syntax::rename_template_source` /
/// `syntax::rename_template_in_function_library`），不做全局文本替换，
/// 避免误改日志/文本内容。不可解析的存量源跳过——它们本就无法运行，
/// 不阻塞重命名；改写失败（语法损坏）则整体报错。所有资源先生成新内容，
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

    for entry in store.list(package, YAML_EXTENSION_ID, "automations")? {
        let Some(content) = entry.content.as_deref() else {
            continue; // 非 UTF-8 附件不参与引用改写
        };
        let rel = entry
            .path
            .strip_prefix("automations/")
            .unwrap_or(&entry.path);
        let rewritten = if is_function_library_path(rel) {
            // 不可解析的存量函数库解析失败 → 跳过（与脚本侧 skip 语义一致）
            syntax::rename_template_in_function_library(
                content, old_name, &old_short, new_name, &new_short,
            )
            .ok()
            .flatten()
        } else {
            // 不可解析的存量源（旧 v3/坏语法）跳过——它们本就无法运行，不阻塞重命名
            syntax::rename_template_source(content, old_name, &old_short, new_name, &new_short)
                .ok()
                .flatten()
        };
        if let Some((content, _changed)) = rewritten {
            rewrites.push((entry.path.clone(), entry.content.clone().unwrap(), content));
        }
    }

    // 先写全部引用改写（任一失败回滚已写内容——模板文件此时未动，调用方
    // rename_resource 的 fs::rename 尚未发生）
    let mut written: Vec<(String, String)> = Vec::new();
    for (path, original, content) in &rewrites {
        if let Err(error) = store.write_text_unchecked(package, YAML_EXTENSION_ID, path, content) {
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

    /// V1 脚本 + 函数库中的模板引用经 AST 同步改写；文本字面量不动。
    #[test]
    fn rename_template_updates_script_and_function_references() {
        let (store, dir) = temp_store("v1");
        let templates = plugin_root(&dir).join("templates");
        std::fs::create_dir_all(&templates).unwrap();
        std::fs::write(templates.join("old.png"), b"png").unwrap();

        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "automations/main.yaml",
                "run:\n  - find:\n      template: old.png\n      region: [0, 0, 1, 1]\n    as: hit\n  - log: old.png 文本不应改\n",
                None,
                false,
            )
            .unwrap();
        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "automations/_function.yaml",
                "functions:\n  login:\n    run:\n      - wait_find:\n          template: old.png\n",
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
        assert!(
            !templates.join("old.png").exists(),
            "rename_resource 负责移动模板文件"
        );
        assert_eq!(std::fs::read(templates.join("new.png")).unwrap(), b"png");
        let script =
            std::fs::read_to_string(plugin_root(&dir).join("automations/main.yaml")).unwrap();
        assert!(script.contains("template: new"), "脚本引用改写为短名");
        assert!(script.contains("old.png 文本不应改"));
        let function =
            std::fs::read_to_string(plugin_root(&dir).join("automations/_function.yaml")).unwrap();
        assert!(function.contains("template: new"));
    }

    /// 不可解析的存量源 → 跳过（不阻塞重命名）。
    #[test]
    fn rename_template_skips_unparsable_legacy_sources() {
        let (store, dir) = temp_store("legacy");
        let templates = plugin_root(&dir).join("templates");
        std::fs::create_dir_all(&templates).unwrap();
        std::fs::write(templates.join("old.png"), b"png").unwrap();
        // 旧 v3 形态存量自动化脚本（保存边界已拒收，只可能来自历史盘上数据）
        let automations = plugin_root(&dir).join("automations");
        std::fs::create_dir_all(&automations).unwrap();
        std::fs::write(
            automations.join("legacy.yaml"),
            b"version: 3\nsteps:\n  - check: old.png\n",
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
        let legacy = std::fs::read_to_string(automations.join("legacy.yaml")).unwrap();
        assert!(legacy.contains("old.png"), "不可解析的存量源保持原样");
    }

    /// 保存边界：V1 直存；旧 v3 源报 yaml.version.removed；automations/ 内
    /// `_function*.yaml` 按函数库（functions: 包装）校验；旧 functions/ 目录
    /// 显式拒绝。
    #[test]
    fn saves_are_v1_only() {
        let (store, _dir) = temp_store("save");
        store
            .validate_save(crate::resources::SaveValidation {
                package: "com.test.app",
                plugin: YAML_EXTENSION_ID,
                path: "automations/daily.yaml",
                content: "run:\n  - log: ok\n",
                store: &store,
            })
            .expect("V1 脚本必须通过");
        let err = store
            .validate_save(crate::resources::SaveValidation {
                package: "com.test.app",
                plugin: YAML_EXTENSION_ID,
                path: "automations/daily.yaml",
                content: "version: 3\nsteps: []\n",
                store: &store,
            })
            .unwrap_err();
        assert_eq!(err[0]["code"], "yaml.version.removed");

        // _function 前缀 → 函数库校验（必须有 functions: 包装）
        let err = store
            .validate_save(crate::resources::SaveValidation {
                package: "com.test.app",
                plugin: YAML_EXTENSION_ID,
                path: "automations/_function.yaml",
                content: "greet:\n  run: []\n",
                store: &store,
            })
            .unwrap_err();
        assert_eq!(err[0]["code"], "yaml.functions.missing", "{err}");

        // 旧 functions/ 专属目录 → 显式拒绝（已删除，无兼容层）
        let err = store
            .validate_save(crate::resources::SaveValidation {
                package: "com.test.app",
                plugin: YAML_EXTENSION_ID,
                path: "functions/lib.yaml",
                content: "functions:\n  greet:\n    run: []\n",
                store: &store,
            })
            .unwrap_err();
        assert_eq!(err[0]["code"], "yaml.functions.dir.removed", "{err}");

        // 函数名清单注记（automations/ 内的函数库文件）
        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "automations/_function.yaml",
                "functions:\n  greet:\n    run:\n      - return: true\n",
                None,
                false,
            )
            .unwrap();
        let list = store
            .list("com.test.app", YAML_EXTENSION_ID, "automations")
            .unwrap();
        let library = list
            .iter()
            .find(|e| e.path.ends_with("_function.yaml"))
            .unwrap();
        assert_eq!(library.meta["functions"][0], "greet");
        assert_eq!(library.meta["file"], "_function");
        // 普通脚本不注记函数清单
        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "automations/daily.yaml",
                "run:\n  - log: ok\n",
                None,
                false,
            )
            .unwrap();
        let list = store
            .list("com.test.app", YAML_EXTENSION_ID, "automations")
            .unwrap();
        let script = list
            .iter()
            .find(|e| e.path.ends_with("daily.yaml"))
            .unwrap();
        assert!(
            script.meta.get("functions").is_none(),
            "自动化不注记函数清单"
        );
    }

    /// 字节钩子：templates/ 上传彩色 PNG → 落盘 8-bit 灰度归一化；
    /// 非模板路径字节原样透传；垃圾字节 → 结构化诊断（HTTP 400 载荷）。
    #[test]
    fn binary_hook_normalizes_templates_to_grayscale_and_rejects_garbage() {
        let (store, _dir) = temp_store("binary-hook");
        store.register_handler(YAML_EXTENSION_ID, Arc::new(YamlResourceHandler));

        // 彩色 PNG（非灰度）夹具
        let mut color = image::RgbaImage::new(4, 3);
        for (x, _y, pixel) in color.enumerate_pixels_mut() {
            *pixel = image::Rgba([if x % 2 == 0 { 10 } else { 240 }, 90, 160, 255]);
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(color)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();

        let normalized = store
            .validate_save_binary(crate::resources::SaveBinaryValidation {
                package: "com.test.app",
                plugin: YAML_EXTENSION_ID,
                path: "templates/icon.png",
                bytes: &bytes,
                store: &store,
            })
            .expect("合法 PNG 必须通过归一化");
        assert_eq!(
            image::load_from_memory(&normalized).unwrap().color(),
            image::ColorType::L8,
            "templates/ 上传必须归一化为 8-bit 灰度 PNG"
        );
        store
            .write_binary(
                "com.test.app",
                YAML_EXTENSION_ID,
                "templates/icon.png",
                &normalized,
                None,
                false,
            )
            .unwrap();
        let on_disk = store
            .read_binary("com.test.app", YAML_EXTENSION_ID, "templates/icon.png")
            .unwrap()
            .unwrap();
        assert_eq!(on_disk, normalized);

        // 非模板路径：字节原样透传（不解释）
        let passthrough = store
            .validate_save_binary(crate::resources::SaveBinaryValidation {
                package: "com.test.app",
                plugin: YAML_EXTENSION_ID,
                path: "assets/blob.bin",
                bytes: b"\x00\x01not a png",
                store: &store,
            })
            .unwrap();
        assert_eq!(passthrough, b"\x00\x01not a png".to_vec());

        // 垃圾字节 → 结构化诊断
        let err = store
            .validate_save_binary(crate::resources::SaveBinaryValidation {
                package: "com.test.app",
                plugin: YAML_EXTENSION_ID,
                path: "templates/broken.png",
                bytes: b"definitely not an image",
                store: &store,
            })
            .unwrap_err();
        assert_eq!(err[0]["code"], "template.png.invalid", "{err}");
        assert_eq!(err[0]["path"], "templates/broken.png");
    }
}
