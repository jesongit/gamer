//! Entrypoint 参数 schema 描述（V1，契约 §7 形态保留）。
//!
//! `GET /api/runners/:runner_id/entrypoint` 的 gamer.yaml 数据源：
//! entrypoint = `<pkg>/<脚本>.yaml`（脚本）或 `<pkg>#<函数名>`（函数，
//! 简化计划 Phase 1：统一命名空间按名寻址，函数从当前 Package 全部
//! `_function*.yaml` 组合出的注册表解析——定义文件可拆分/移动）。
//! 内层载荷 `{kind, format:"yaml-params-v1", schema}`——schema 即 V1
//! `params` 声明（名称/类型/必填/默认值/说明），前端据此渲染参数表单，
//! 不解析 YAML。旧 v3 的 psig1 签名字段已删除。

use std::sync::Arc;

use serde_json::Value;

use crate::extensions::gamer_yaml::resources::{is_function_library_path, script_entry};
use crate::extensions::gamer_yaml::runner_adapter::compose_function_library;
use crate::extensions::gamer_yaml::syntax::parse_script;
use crate::extensions::gamer_yaml::task_params::decls_schema_json;
use crate::resources::PackageStore;

#[derive(Debug)]
pub(crate) enum DescribeError {
    NotFound { resource: String },
    Invalid { diagnostics: Value },
}

impl DescribeError {
    fn from_script_errors(
        diagnostics: &[crate::extensions::gamer_yaml::error::ScriptError],
    ) -> Self {
        Self::Invalid {
            diagnostics: serde_json::to_value(diagnostics).unwrap_or_default(),
        }
    }

    fn invalid_diagnostic(code: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            diagnostics: serde_json::json!([
                { "code": code, "message": message.into(), "resource": "", "step_path": "", "field": "" }
            ]),
        }
    }
}

/// [`crate::scheduler::EntrypointDescriber`] 的 gamer.yaml 实现（资源存储视图）。
pub(crate) struct StoreEntrypointDescriber {
    scripts: Arc<PackageStore>,
}

impl StoreEntrypointDescriber {
    pub(crate) fn new(scripts: Arc<PackageStore>) -> Self {
        Self { scripts }
    }
}

impl crate::scheduler::EntrypointDescriber for StoreEntrypointDescriber {
    fn describe(
        &self,
        entrypoint: &str,
    ) -> Result<Value, crate::scheduler::EntrypointDescribeError> {
        describe_entrypoint(&self.scripts, entrypoint).map_err(|error| match error {
            DescribeError::NotFound { resource } => {
                crate::scheduler::EntrypointDescribeError::NotFound { resource }
            }
            DescribeError::Invalid { diagnostics } => {
                crate::scheduler::EntrypointDescribeError::Invalid { diagnostics }
            }
        })
    }
}

/// 描述一个 entrypoint：`<pkg>/<脚本>.yaml`（脚本）或 `<pkg>#<函数名>`
/// （函数）。返回契约 §7 内层载荷 `{kind, format, schema}`（API 层补
/// runner_id/entrypoint 外壳）。
pub(crate) fn describe_entrypoint(
    scripts: &PackageStore,
    entrypoint: &str,
) -> Result<Value, DescribeError> {
    let entrypoint = entrypoint.trim();
    if let Some((base, func)) = entrypoint.rsplit_once('#') {
        describe_function(scripts, base.trim(), func.trim(), entrypoint)
    } else {
        describe_script(scripts, entrypoint)
    }
}

fn describe_script(scripts: &PackageStore, entrypoint: &str) -> Result<Value, DescribeError> {
    let rel = entrypoint
        .split_once('/')
        .map(|(_, rel)| rel)
        .unwrap_or(entrypoint);
    if is_function_library_path(rel) {
        return Err(DescribeError::invalid_diagnostic(
            "yaml.function_library.not_script",
            "函数库文件（automations/_function*.yaml）不能作为脚本描述；函数请以 <pkg>#<函数名> 寻址",
        ));
    }
    let content = match script_entry(scripts, entrypoint) {
        Ok(Some(entry)) => entry.content,
        Ok(None) => {
            return Err(DescribeError::NotFound {
                resource: entrypoint.to_string(),
            })
        }
        Err(error) => {
            return Err(DescribeError::invalid_diagnostic(
                "yaml.read_failed",
                format!("读取脚本失败: {error:#}"),
            ))
        }
    };
    let script = parse_script(&content).map_err(|diagnostics| DescribeError::Invalid {
        diagnostics: serde_json::to_value(&diagnostics).unwrap_or_default(),
    })?;
    Ok(schema_payload("script", &decls_schema_json(&script.params)))
}

/// `<pkg>#<函数名>`：从当前 Package 全部 `_function*.yaml` 组合出的注册表按名
/// 解析函数（定义文件可拆分/移动，不影响寻址）。空函数名 → NotFound 提示。
fn describe_function(
    scripts: &PackageStore,
    base: &str,
    func: &str,
    entrypoint: &str,
) -> Result<Value, DescribeError> {
    if func.is_empty() {
        return Err(DescribeError::invalid_diagnostic(
            "resource.func.not_found",
            "函数 entrypoint 缺少函数名（<pkg>#<函数名>）",
        ));
    }
    let library = compose_function_library(scripts, base).map_err(|error| {
        DescribeError::invalid_diagnostic("yaml.library.invalid", error.to_string())
    })?;
    let decls = library
        .iter()
        .find(|(name, _)| name == func)
        .map(|(_, def)| decls_schema_json(&def.params))
        .ok_or_else(|| {
            DescribeError::from_script_errors(&[
                crate::extensions::gamer_yaml::error::ScriptError::new(
                    "resource.func.not_found",
                    format!("函数 {func} 不在当前 Package（{base}）函数库中"),
                    entrypoint,
                ),
            ])
        })?;
    Ok(schema_payload("function", &decls))
}

fn schema_payload(kind: &str, schema: &Value) -> Value {
    serde_json::json!({
        "kind": kind,
        "format": "yaml-params-v1",
        "schema": schema,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::extensions::gamer_yaml::YAML_EXTENSION_ID;

    fn store_dir(tag: &str) -> (Config, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "gamer-epdesc-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        (cfg, dir)
    }

    fn write(cfg: &Config, kind_dir: &str, name: &str, content: &str) {
        let store = PackageStore::open(cfg).unwrap();
        let pkg = "com.test.app";
        let _ = store.create_package(crate::resources::PackageInput {
            id: pkg.into(),
            ..Default::default()
        });
        store
            .write_text(
                pkg,
                YAML_EXTENSION_ID,
                &format!("{kind_dir}/{name}"),
                content,
                None,
                false,
            )
            .unwrap();
    }

    #[test]
    fn describes_v1_scripts_with_schema() {
        let (cfg, _dir) = store_dir("script");
        write(
            &cfg,
            "automations",
            "daily.yaml",
            "params:\n  retry:\n    type: integer\n    default: 3\n    desc: 重试次数\nrun:\n  - log: hi\n",
        );
        let store = PackageStore::open(&cfg).unwrap();
        let payload = describe_entrypoint(&store, "com.test.app/daily.yaml").unwrap();
        assert_eq!(payload["kind"], "script");
        assert_eq!(payload["format"], "yaml-params-v1");
        assert_eq!(payload["schema"][0]["name"], "retry");
        assert_eq!(payload["schema"][0]["type"], "integer");
        assert_eq!(payload["schema"][0]["default"], 3);
        assert_eq!(payload["schema"][0]["desc"], "重试次数");
        assert!(payload.get("signature").is_none(), "V1 无签名字段");

        // 旧 v3 源 → 结构化 invalid 诊断（不再有 fallback）
        write(
            &cfg,
            "automations",
            "legacy.yaml",
            "version: 3\nsteps: []\n",
        );
        let error = describe_entrypoint(&store, "com.test.app/legacy.yaml").unwrap_err();
        match error {
            DescribeError::Invalid { diagnostics } => {
                assert!(
                    diagnostics.to_string().contains("yaml.version.removed"),
                    "{diagnostics}"
                );
            }
            other => panic!("期望 Invalid，得到 {other:?}"),
        }
    }

    #[test]
    fn describes_function_entrypoint_and_reports_missing() {
        let (cfg, _dir) = store_dir("func");
        write(
            &cfg,
            "automations",
            "_function.yaml",
            "functions:\n  claim:\n    params:\n      timeout:\n        type: duration\n        default: 5s\n    run:\n      - log: hi\n",
        );
        let store = PackageStore::open(&cfg).unwrap();
        let payload = describe_entrypoint(&store, "com.test.app#claim").unwrap();
        assert_eq!(payload["kind"], "function");
        assert_eq!(payload["schema"][0]["type"], "duration");

        // 手动拆分文件中的函数同样按名可寻（文件名不影响调用名）
        write(
            &cfg,
            "automations",
            "_function_battle.yaml",
            "functions:\n  attack:\n    run:\n      - return: true\n",
        );
        let payload = describe_entrypoint(&store, "com.test.app#attack").unwrap();
        assert_eq!(payload["kind"], "function");

        // 目标函数不存在
        let error = describe_entrypoint(&store, "com.test.app#missing").unwrap_err();
        assert!(matches!(error, DescribeError::Invalid { .. }));
        // 空函数名
        let error = describe_entrypoint(&store, "com.test.app#").unwrap_err();
        assert!(matches!(error, DescribeError::Invalid { .. }));
        // 包不存在
        let error = describe_entrypoint(&store, "com.none#a").unwrap_err();
        assert!(matches!(error, DescribeError::Invalid { .. }));

        // 函数库文件不能按脚本描述
        let error = describe_entrypoint(&store, "com.test.app/_function.yaml").unwrap_err();
        match error {
            DescribeError::Invalid { diagnostics } => assert!(
                diagnostics
                    .to_string()
                    .contains("yaml.function_library.not_script"),
                "{diagnostics}"
            ),
            other => panic!("期望 Invalid，得到 {other:?}"),
        }
    }
}
