//! 参数绑定门禁（V1）：脚本/函数 `params` Schema ↔ 稀疏实参的统一绑定。
//!
//! 语义（计划 Phase 4.2）：任务参数快照按**当前声明**重绑——存活值保留、
//! 新增参数取默认值、被删参数丢弃、必填缺失结构化报错；旧 v3 的 psig1
//! 参数签名门禁随旧语法一并删除（开发阶段不兼容旧任务快照）。
//!
//! 手动运行（`POST /api/runs`）走严格模式：未知实参即报错；任务路径宽松：
//! 未知键静默丢弃（历史快照可能含已删参数）。
//!
//! 日志约束沿用：运行链路只记录参数名列表，**绝不记录参数值**（text 防
//! 泄露）。

use serde_json::{Map as JsonMap, Value};

use crate::extensions::gamer_yaml::error::{
    ScriptError, PARAM_ARGS_MISSING_REQUIRED, PARAM_ARGS_TYPE_MISMATCH, PARAM_ARGS_UNKNOWN,
};
use crate::extensions::gamer_yaml::syntax::{check_type, ParamDecl};

/// 绑定结果：resolved = 默认值 ∪ 已校验覆盖（供 `resolved_args` 展示）。
#[derive(Debug, Clone)]
pub struct BoundArgs {
    pub resolved: JsonMap<String, Value>,
}

/// 稀疏实参绑定：`overrides` 键 → 覆盖值（原始 JSON 形态，类型按声明校验）。
///
/// `strict_unknown`：true（手动运行）未知键报 `param.args.unknown`；false
/// （任务快照重绑）未知键丢弃。
pub fn bind_entry_args(
    resource: &str,
    decls: &[ParamDecl],
    overrides: &JsonMap<String, Value>,
    strict_unknown: bool,
) -> Result<BoundArgs, Vec<ScriptError>> {
    let mut errors: Vec<ScriptError> = Vec::new();
    let mut resolved = JsonMap::new();
    for decl in decls {
        match overrides.get(&decl.name) {
            Some(value) if !value.is_null() => {
                if let Err(message) = check_type(decl.ty, value) {
                    errors.push(
                        ScriptError::new(
                            PARAM_ARGS_TYPE_MISMATCH,
                            format!(
                                "参数 {} 与类型 {} 不符: {message}",
                                decl.name,
                                decl.ty.canonical()
                            ),
                            resource,
                        )
                        .at("args", &decl.name),
                    );
                    continue;
                }
                resolved.insert(decl.name.clone(), value.clone());
            }
            _ => {
                if let Some(default) = &decl.default {
                    resolved.insert(decl.name.clone(), default.clone());
                } else if decl.required {
                    errors.push(
                        ScriptError::new(
                            PARAM_ARGS_MISSING_REQUIRED,
                            format!("必填参数 {} 未提供", decl.name),
                            resource,
                        )
                        .at("args", &decl.name),
                    );
                }
            }
        }
    }
    if strict_unknown {
        for name in overrides.keys() {
            if !decls.iter().any(|decl| &decl.name == name) {
                errors.push(
                    ScriptError::new(
                        PARAM_ARGS_UNKNOWN,
                        format!("未知参数 {name}——按当前声明该参数不存在"),
                        resource,
                    )
                    .at("args", name),
                );
            }
        }
    }
    if errors.is_empty() {
        Ok(BoundArgs { resolved })
    } else {
        Err(errors)
    }
}

/// 参数声明 → descriptor schema JSON（`GET /api/runners/:id/entrypoint`）。
pub fn decls_schema_json(decls: &[ParamDecl]) -> Value {
    Value::Array(
        decls
            .iter()
            .map(|decl| {
                serde_json::json!({
                    "name": decl.name,
                    "type": decl.ty.canonical(),
                    "required": decl.required,
                    "default": decl.default,
                    "desc": decl.desc,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::gamer_yaml::syntax::parse_script;
    use serde_json::json;

    fn decls(source: &str) -> Vec<ParamDecl> {
        parse_script(source).unwrap().params
    }

    fn bind(
        decls: &[ParamDecl],
        overrides: Value,
    ) -> Result<JsonMap<String, Value>, Vec<ScriptError>> {
        bind_entry_args(
            "test/脚本.yaml",
            decls,
            &overrides.as_object().cloned().unwrap_or_default(),
            true,
        )
        .map(|bound| bound.resolved)
    }

    #[test]
    fn defaults_fill_missing_and_overrides_validate() {
        let decls = decls(
            "params:\n  retry:\n    type: integer\n    default: 3\n  tag:\n    type: string\n    default: \"\"\nrun: []\n",
        );
        let resolved = bind(&decls, json!({"retry": 5})).unwrap();
        assert_eq!(resolved["retry"], json!(5));
        assert_eq!(resolved["tag"], json!(""));

        let errors = bind(&decls, json!({"retry": 1.5})).unwrap_err();
        assert_eq!(errors[0].code, PARAM_ARGS_TYPE_MISMATCH);
        assert_eq!(errors[0].resource, "test/脚本.yaml");
    }

    #[test]
    fn unknown_args_strict_for_manual_and_lenient_for_tasks() {
        let decls = decls("params:\n  a:\n    type: string\nrun: []\n");
        let errors = bind(&decls, json!({"ghost": 1})).unwrap_err();
        assert_eq!(errors[0].code, PARAM_ARGS_UNKNOWN);
        // 宽松（任务重绑）：未知键丢弃
        let lenient = bind_entry_args(
            "t",
            &decls,
            &json!({"ghost": 1}).as_object().unwrap().clone(),
            false,
        )
        .unwrap();
        assert!(lenient.resolved.is_empty());
    }

    #[test]
    fn required_missing_reports_structured_diagnostic() {
        let decls = decls("params:\n  who:\n    type: string\n    required: true\nrun: []\n");
        let errors = bind(&decls, json!({})).unwrap_err();
        assert_eq!(errors[0].code, PARAM_ARGS_MISSING_REQUIRED);
        assert_eq!(errors[0].field_str(), "who");
    }

    #[test]
    fn domain_types_accept_both_raw_forms() {
        let decls = decls(
            "params:\n  wait:\n    type: duration\n  at:\n    type: point\n  key:\n    type: key\nrun: []\n",
        );
        let resolved = bind(
            &decls,
            json!({"wait": "1.5s", "at": [0.5, 0.8], "key": "HOME"}),
        )
        .unwrap();
        assert_eq!(resolved["wait"], json!("1.5s"));
        assert_eq!(resolved["at"], json!([0.5, 0.8]));
        let errors = bind(&decls, json!({"wait": "abc"})).unwrap_err();
        assert_eq!(errors[0].code, PARAM_ARGS_TYPE_MISMATCH);
    }
}
