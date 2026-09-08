//! Entrypoint 参数 schema 描述（V1，契约 §7 形态保留）。
//!
//! `GET /api/runners/:runner_id/entrypoint` 的 gamer.yaml 数据源：
//! entrypoint = `<pkg>/<脚本>.yaml`（脚本）或
//! `<pkg>/<文件短路径>.yaml#<函数名>`（函数，缺省函数名 = 文件第一个）。
//! 内层载荷 `{kind, format:"yaml-params-v1", schema}`——schema 即 V1
//! `params` 声明（名称/类型/必填/默认值/说明），前端据此渲染参数表单，
//! 不解析 YAML。旧 v3 的 psig1 签名字段已删除。

use std::sync::Arc;

use serde_json::Value;

use crate::extensions::gamer_yaml::resources::{function_entry, script_entry};
use crate::extensions::gamer_yaml::syntax::{parse_function_library, parse_script};
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

/// 描述一个 entrypoint：`<pkg>/<脚本>.yaml`（脚本）或
/// `<pkg>/<文件>.yaml#<函数名>`（函数库内函数）。返回契约 §7 内层载荷
/// `{kind, format, schema}`（API 层补 runner_id/entrypoint 外壳）。
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

fn describe_function(
    scripts: &PackageStore,
    base: &str,
    func: &str,
    entrypoint: &str,
) -> Result<Value, DescribeError> {
    let target = normalize_file_target(base);
    let content = match function_entry(scripts, &target) {
        Ok(Some(entry)) => entry.content,
        Ok(None) => {
            return Err(DescribeError::NotFound {
                resource: target.clone(),
            })
        }
        Err(error) => {
            return Err(DescribeError::invalid_diagnostic(
                "yaml.read_failed",
                format!("读取函数文件失败: {error:#}"),
            ))
        }
    };
    let library =
        parse_function_library(&content).map_err(|diagnostics| DescribeError::Invalid {
            diagnostics: serde_json::to_value(&diagnostics).unwrap_or_default(),
        })?;
    let name = if func.is_empty() {
        library
            .first()
            .map(|(name, _)| name.clone())
            .ok_or_else(|| {
                DescribeError::invalid_diagnostic(
                    "resource.func.not_found",
                    format!("函数文件 {target} 未定义任何函数"),
                )
            })?
    } else {
        func.to_string()
    };
    let decls = library
        .iter()
        .find(|(entry, _)| entry == &name)
        .map(|(_, def)| decls_schema_json(&def.params))
        .ok_or_else(|| {
            DescribeError::from_script_errors(&[
                crate::extensions::gamer_yaml::error::ScriptError::new(
                    "resource.func.not_found",
                    format!("函数 {name} 不在文件 {target} 中"),
                    entrypoint,
                ),
            ])
        })?;
    Ok(schema_payload("function", &decls))
}

/// `<pkg>/<文件>.yaml[.yml]` 短路径归一（后缀可省略）。
fn normalize_file_target(base: &str) -> String {
    let lower = base.to_ascii_lowercase();
    if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        base.to_string()
    } else {
        format!("{base}.yaml")
    }
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
            "functions",
            "common.yaml",
            "functions:\n  claim:\n    params:\n      timeout:\n        type: duration\n        default: 5s\n    run:\n      - log: hi\n",
        );
        let store = PackageStore::open(&cfg).unwrap();
        let payload = describe_entrypoint(&store, "com.test.app/common.yaml#claim").unwrap();
        assert_eq!(payload["kind"], "function");
        assert_eq!(payload["schema"][0]["type"], "duration");

        // 缺省函数名 = 文件第一个
        let payload = describe_entrypoint(&store, "com.test.app/common.yaml#").unwrap();
        assert_eq!(payload["kind"], "function");

        // 目标函数不存在
        let error = describe_entrypoint(&store, "com.test.app/common.yaml#missing").unwrap_err();
        assert!(matches!(error, DescribeError::Invalid { .. }));
        // 文件不存在
        let error = describe_entrypoint(&store, "com.test.app/none.yaml#a").unwrap_err();
        assert!(matches!(error, DescribeError::NotFound { .. }));
    }
}
