//! Entrypoint 参数 schema 描述（P12.3 / 契约 §7）。
//!
//! 前端约束是「不得为取参数而解析 YAML」：本模块按 entrypoint 资源 id 读取
//! 分区资源并产出可渲染参数表单的 JSON schema——v2 七类与 v3（`version: 3`
//! 顶层 / 函数库 bare-map）双格式走同一端点。资源缺失 → 结构化 not_found；
//! 解析失败 / 未知参数类型 → 结构化 invalid（诊断与保存期同源）。本模块物理
//! 居于 gamer_yaml 扩展边界内（架构守卫：extensions 外禁现 yaml_vnext 引用），
//! Core 侧经 `scheduler::EntrypointDescriber` 窄 trait 透传，不感知本模块。

use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::extensions::gamer_yaml::engine::{load_entry_param_decls, RunTarget};
use crate::extensions::gamer_yaml::script_v2::params::KEY_NAMES;
use crate::extensions::gamer_yaml::task_params::{
    is_known_v3_type, normalize_v3_default_json, v3_decls_from_program, v3_param_signature,
    V3ParamDecl,
};
use crate::resources::{ResourceKind as RK, ResourceStore};

/// 描述失败（经 `scheduler::EntrypointDescribeError` 透传到 API 边界）。
#[derive(Debug, Clone)]
pub(crate) enum DescribeError {
    NotFound { resource: String },
    Invalid { diagnostics: Value },
}

impl DescribeError {
    fn invalid_diagnostic(code: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            diagnostics: json!([{ "code": code, "message": message.into() }]),
        }
    }
}

/// [`crate::scheduler::EntrypointDescriber`] 的 gamer.yaml 实现（资源存储视图）。
pub(crate) struct StoreEntrypointDescriber {
    scripts: Arc<ResourceStore>,
}

impl StoreEntrypointDescriber {
    pub(crate) fn new(scripts: Arc<ResourceStore>) -> Self {
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
/// `{kind, format, schema, signature}`（API 层补 runner_id/entrypoint 外壳）。
pub(crate) fn describe_entrypoint(
    scripts: &ResourceStore,
    entrypoint: &str,
) -> Result<Value, DescribeError> {
    let entrypoint = entrypoint.trim();
    if let Some((base, func)) = entrypoint.rsplit_once('#') {
        describe_function(scripts, base.trim(), func.trim(), entrypoint)
    } else {
        describe_script(scripts, entrypoint)
    }
}

fn describe_script(scripts: &ResourceStore, entrypoint: &str) -> Result<Value, DescribeError> {
    let entry = match scripts.get_text(RK::Scripts, entrypoint) {
        Ok(Some(entry)) => entry,
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
    if crate::extensions::gamer_yaml::yaml_vnext::is_v3_source(&entry.content) {
        let program = crate::extensions::gamer_yaml::yaml_vnext::load(&entry.content)
            .map_err(|diagnostics| DescribeError::Invalid {
                diagnostics: serde_json::to_value(&diagnostics).unwrap_or_default(),
            })?;
        let decls = v3_decls_from_program(&program);
        return schema_payload("script", &decls);
    }
    // v2 存量脚本：服务端 v2 解析（快照级 composite，包内脚本亦可描述）
    let target = RunTarget::Script {
        script_id: entrypoint.to_string(),
        start_index: 0,
    };
    match load_entry_param_decls(scripts, &target) {
        Ok((decls, _)) => v2_schema_payload("script", &decls),
        Err(diagnostics) => Err(DescribeError::Invalid {
            diagnostics: serde_json::to_value(&diagnostics).unwrap_or_default(),
        }),
    }
}

fn describe_function(
    scripts: &ResourceStore,
    base: &str,
    function: &str,
    entrypoint: &str,
) -> Result<Value, DescribeError> {
    let Some((pkg, file)) = base.split_once('/') else {
        return Err(DescribeError::invalid_diagnostic(
            "entrypoint.invalid",
            format!("函数 entrypoint 缺少分区前缀：{entrypoint:?}（应为 <分区>/<文件>.yaml#<函数>）"),
        ));
    };
    let file = file
        .trim()
        .trim_end_matches(".yaml")
        .trim_end_matches(".yml");
    let rel = format!("{pkg}/{file}.yaml");
    match scripts.get_text(RK::Functions, &rel) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(DescribeError::NotFound { resource: rel }),
        Err(error) => {
            return Err(DescribeError::invalid_diagnostic(
                "yaml.read_failed",
                format!("读取函数库失败: {error:#}"),
            ))
        }
    }
    // v2 严格解析先行（保留 v2 口径的结构化诊断与七类强类型）
    let target = RunTarget::Function {
        pkg: pkg.to_string(),
        file: file.to_string(),
        function: Some(function.to_string()),
        start_index: 0,
    };
    match load_entry_param_decls(scripts, &target) {
        Ok((decls, _)) => v2_schema_payload("function", &decls),
        Err(v2_diagnostics) => {
            // v2 拒收（如 v3 扩展类型）→ 函数库 bare-map 宽松抽取目标函数 params
            match crate::extensions::gamer_yaml::task_params::probe_v3_function_decls(
                scripts,
                pkg,
                file,
                Some(function),
            ) {
                Ok(Some(decls)) => schema_payload("function", &decls),
                Ok(None) => Err(DescribeError::Invalid {
                    diagnostics: serde_json::to_value(&v2_diagnostics).unwrap_or_default(),
                }),
                Err(extra) => {
                    let mut all = v2_diagnostics;
                    all.extend(extra);
                    Err(DescribeError::Invalid {
                        diagnostics: serde_json::to_value(&all).unwrap_or_default(),
                    })
                }
            }
        }
    }
}

/// v2 声明（七类强类型）→ 契约 §7 载荷。
fn v2_schema_payload(
    kind: &str,
    decls: &[crate::extensions::gamer_yaml::script_v2::ParamDecl],
) -> Result<Value, DescribeError> {
    let generic: Vec<V3ParamDecl> = decls
        .iter()
        .map(|decl| V3ParamDecl {
            name: decl.name.clone(),
            ty: decl.ty.as_str().to_string(),
            remark: decl.remark.clone(),
            default: decl
                .default
                .as_ref()
                .map(|value| serde_json::to_value(value).unwrap_or(Value::Null)),
        })
        .collect();
    Ok(schema_payload_with(
        kind,
        &generic,
        crate::extensions::gamer_yaml::script_v2::model::param_signature(decls),
    ))
}

/// v3 声明 → 契约 §7 载荷（含当前 psig1 签名，前端可做过期预检）。
/// 未知类型声明 → invalid（schema 拒绝渲染未知形态；与运行期绑定同口径）。
fn schema_payload(kind: &str, decls: &[V3ParamDecl]) -> Result<Value, DescribeError> {
    for decl in decls {
        if !is_known_v3_type(&decl.ty) {
            return Err(DescribeError::invalid_diagnostic(
                "param.decl.format",
                format!(
                    "参数 {} 声明了未知类型 {:?}（可用：tmpl/coord/color/time/key/text/bool/string/int/number/value）",
                    decl.name, decl.ty
                ),
            ));
        }
    }
    Ok(schema_payload_with(kind, decls, v3_param_signature(decls)))
}

fn schema_payload_with(kind: &str, decls: &[V3ParamDecl], signature: String) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for decl in decls {
        let mut property = Map::new();
        property.insert(
            "type".into(),
            Value::String(schema_type(&decl.ty).to_string()),
        );
        if decl.ty.trim() == "coord" {
            property.insert(
                "items".into(),
                json!({ "type": "number", "minItems": 2, "maxItems": 2 }),
            );
        }
        if let Some(default) = &decl.default {
            property.insert(
                "default".into(),
                normalize_v3_default_json(&decl.ty, default),
            );
        } else {
            required.push(Value::String(decl.name.clone()));
        }
        if !decl.remark.is_empty() {
            property.insert("description".into(), Value::String(decl.remark.clone()));
        }
        if decl.ty.trim() == "key" {
            property.insert(
                "enum".into(),
                Value::Array(
                    KEY_NAMES
                        .iter()
                        .map(|name| Value::String((*name).to_string()))
                        .collect(),
                ),
            );
        }
        // 原始声明类型（time/coord 等 UI 形态由前端按此渲染；执行期 TypedValue
        // 行为不变，见契约 §7「v2 ty 名映射到上述类型」）
        property.insert(
            "param_type".into(),
            Value::String(decl.ty.trim().to_string()),
        );
        properties.insert(decl.name.clone(), Value::Object(property));
    }
    json!({
        "kind": kind,
        "format": "yaml-params-v1",
        "schema": {
            "type": "object",
            "properties": Value::Object(properties),
            "required": required,
        },
        "signature": signature,
    })
}

/// 声明类型 → JSON Schema 类型（契约 §7 参数类型集合：string/number/integer/
/// boolean/enum；coord 以二元数值数组表达，value 为任意 JSON）。
fn schema_type(ty: &str) -> &'static str {
    match ty.trim() {
        "bool" | "boolean" => "boolean",
        "int" | "integer" => "integer",
        "number" => "number",
        "coord" => "array",
        "value" => "any",
        // text/string/tmpl/color/time/key 均按字符串渲染（time 取值带单位书写，
        // 如 30s/500ms——执行期解析要求单位串，故不映射为 number）
        _ => "string",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::scheduler::EntrypointDescriber as _;

    fn store_dir(tag: &str) -> (Config, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "gamer-entrypoint-desc-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        (cfg, dir)
    }

    fn write(cfg: &Config, kind_dir: &str, name: &str, content: &str) {
        let dir = cfg.data_dir.join("com.test.app").join(kind_dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn describes_v2_and_v3_scripts_with_schema_and_signature() {
        let (cfg, dir) = store_dir("dual");
        write(
            &cfg,
            "scripts",
            "v2.yaml",
            "params:\n  - 'text:msg:消息:\"默认\"'\n  - 'bool:fast:快速'\nsteps:\n  - log: $msg\n",
        );
        write(
            &cfg,
            "scripts",
            "v3.yaml",
            "version: 3\nparams:\n  - 'text:msg:消息:\"默认\"'\n  - name: count\n    type: int\n    default: 3\nsteps:\n  - log: $msg\n",
        );
        let scripts = Arc::new(ResourceStore::open(&cfg).unwrap());

        let v2 = describe_entrypoint(&scripts, "com.test.app/v2.yaml").unwrap();
        assert_eq!(v2["kind"], "script");
        assert_eq!(v2["schema"]["type"], "object");
        assert_eq!(v2["schema"]["properties"]["msg"]["type"], "string");
        assert_eq!(v2["schema"]["properties"]["msg"]["default"], "默认");
        assert_eq!(v2["schema"]["properties"]["msg"]["description"], "消息");
        assert_eq!(v2["schema"]["properties"]["fast"]["type"], "boolean");
        assert_eq!(
            v2["schema"]["required"],
            serde_json::json!(["fast"]),
            "无默认值 = 必填"
        );
        assert!(v2["signature"]
            .as_str()
            .unwrap()
            .starts_with("psig1|text,msg,0,"));
        assert_eq!(v2["schema"]["properties"]["fast"]["param_type"], "bool");

        let v3 = describe_entrypoint(&scripts, "com.test.app/v3.yaml").unwrap();
        assert_eq!(v3["kind"], "script");
        assert_eq!(v3["schema"]["properties"]["count"]["type"], "integer");
        assert_eq!(v3["schema"]["properties"]["count"]["default"], 3);
        assert_eq!(v3["schema"]["properties"]["msg"]["param_type"], "text");
        assert_eq!(v3["schema"]["required"], serde_json::json!([]));
        assert_eq!(
            v3["signature"].as_str().unwrap(),
            "psig1|text,msg,0,默认|int,count,0,3"
        );
        drop(scripts);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn describes_v3_function_library_entrypoint_and_reports_missing_invalid() {
        let (cfg, dir) = store_dir("func");
        write(
            &cfg,
            "functions",
            "lib.yaml",
            "greet:\n  params:\n    - 'text:who:称呼:\"玩家\"'\n    - 'int:times:次数:2'\n  steps:\n    - log: $who\nfarewell:\n  steps:\n    - log: bye\n",
        );
        let scripts = Arc::new(ResourceStore::open(&cfg).unwrap());

        let greet = describe_entrypoint(&scripts, "com.test.app/lib.yaml#greet").unwrap();
        assert_eq!(greet["kind"], "function");
        assert_eq!(greet["schema"]["properties"]["times"]["type"], "integer");
        assert_eq!(greet["schema"]["properties"]["who"]["default"], "玩家");
        assert_eq!(
            greet["schema"]["required"],
            serde_json::json!([]),
            "字符串形态声明带默认值 → 非必填"
        );
        // 资源缺失 → 结构化 not_found
        match describe_entrypoint(&scripts, "com.test.app/nope.yaml#greet") {
            Err(DescribeError::NotFound { resource }) => {
                assert_eq!(resource, "com.test.app/nope.yaml")
            }
            other => panic!("expected not_found, got {:?}", other.is_ok()),
        }
        // 函数名不存在 → invalid（v2 结构化诊断定位到函数名）
        match describe_entrypoint(&scripts, "com.test.app/lib.yaml#ghost") {
            Err(DescribeError::Invalid { diagnostics }) => {
                let text = diagnostics.to_string();
                assert!(
                    text.contains("ghost") || text.contains("不存在"),
                    "诊断需定位缺失函数: {text}"
                );
            }
            other => panic!("expected invalid, got {:?}", other.is_ok()),
        }
        // v3 脚本解析失败 → 结构化 invalid（yaml.v3.* 诊断）
        write(&cfg, "scripts", "bad.yaml", "version: 3\nparams: []\n");
        match describe_entrypoint(&scripts, "com.test.app/bad.yaml") {
            Err(DescribeError::Invalid { diagnostics }) => {
                assert!(diagnostics.to_string().contains("yaml.v3"));
            }
            other => panic!("expected invalid, got {:?}", other.is_ok()),
        }
        // 未知类型声明 → invalid（与运行期绑定同口径）
        write(
            &cfg,
            "functions",
            "odd.yaml",
            "f:\n  params:\n    - 'flavor:x:备注:1'\n  steps:\n    - log: ok\n",
        );
        match describe_entrypoint(&scripts, "com.test.app/odd.yaml#f") {
            Err(DescribeError::Invalid { diagnostics }) => {
                assert!(diagnostics.to_string().contains("flavor"));
            }
            other => panic!("expected invalid, got {:?}", other.is_ok()),
        }
        // 缺分区前缀的函数 entrypoint → invalid
        match describe_entrypoint(&scripts, "lib.yaml#greet") {
            Err(DescribeError::Invalid { diagnostics }) => {
                assert!(diagnostics.to_string().contains("分区前缀"));
            }
            other => panic!("expected invalid, got {:?}", other.is_ok()),
        }
        drop(scripts);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn describer_impl_maps_errors_for_the_core_trait() {
        let (cfg, dir) = store_dir("trait");
        write(
            &cfg,
            "scripts",
            "v3.yaml",
            "version: 3\nsteps:\n  - log: ok\n",
        );
        let scripts = Arc::new(ResourceStore::open(&cfg).unwrap());
        let describer = StoreEntrypointDescriber::new(scripts);
        let ok = describer.describe("com.test.app/v3.yaml").unwrap();
        assert_eq!(ok["kind"], "script");
        assert_eq!(ok["format"], "yaml-params-v1");
        match describer.describe("com.test.app/missing.yaml") {
            Err(crate::scheduler::EntrypointDescribeError::NotFound { .. }) => {}
            other => panic!("expected NotFound, got {:?}", other.is_ok()),
        }
        drop(describer);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
