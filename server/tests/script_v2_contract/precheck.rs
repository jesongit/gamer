//! 阶段 0 最小预校验：只实现非法 fixture 期望覆盖的错误面，证明非法样例可被
//! 「保存前预校验」以结构化错误（code + step_path + field）拒绝，而非解析中文文案。
//!
//! 阶段 2 扩展点（对照 docs/SCRIPT_EDITOR_REDESIGN_PLAN.md §13.2 六层校验、
//! docs/SCRIPT_EDITOR_CONTRACT.md 第 5 节错误码）：
//! 1. 迁入 `server/src`，成为 parse_script_file()/parse_function_file() 的入口校验层；
//! 2. 补齐步骤字段互斥/上下文限制、类型化引用、args 绑定、同分区资源引用校验；
//! 3. call 跨文件调用环：本阶段只查自引用，跨文件需要阶段 1 的资源解析器提供引用图；
//! 4. Diagnostic 增加 resource 字段与 span（saphyr-parser 的 Span 事件已携带行列信息）。

use crate::model::ACTION_KEYS;
use crate::yaml_loader::{self, Node};
use serde::Serialize;
use saphyr_parser::ScalarStyle;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    pub step_path: String,
    pub field: String,
}

fn diag(code: &str, message: impl Into<String>, step_path: &str, field: &str) -> Diagnostic {
    Diagnostic {
        code: code.to_string(),
        message: message.into(),
        step_path: step_path.to_string(),
        field: field.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Script,
    FunctionLibrary,
}

/// 旧语法顶层键（出现即报 legacy_format，给前端「旧格式迁移」提示位）。
const LEGACY_TOP_KEYS: &[&str] = &[
    "func", "name", "action_wait", "default_threshold", "package", "until", "cond",
];

pub fn precheck(resource_id: &str, kind: ResourceKind, source: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let root = match yaml_loader::load(source) {
        Ok(n) => n,
        Err(e) => {
            out.push(Diagnostic {
                code: "yaml.syntax_error".into(),
                message: e.to_string(),
                step_path: String::new(),
                field: "yaml".into(),
            });
            return out;
        }
    };
    let Some(entries) = root.as_map() else {
        out.push(diag("script.root_type", "根节点必须是映射", "", "yaml"));
        return out;
    };
    match kind {
        ResourceKind::Script => precheck_script(resource_id, entries, &mut out),
        ResourceKind::FunctionLibrary => precheck_function_library(entries, &mut out),
    }
    out
}

fn precheck_script(resource_id: &str, entries: &[(String, Node)], out: &mut Vec<Diagnostic>) {
    // 第 1 层：顶层键。
    let mut top_errors = Vec::new();
    for (k, _) in entries {
        if LEGACY_TOP_KEYS.contains(&k.as_str()) {
            top_errors.push(diag(
                "script.top_level.legacy_format",
                format!("顶层键 {k:?} 属于旧语法（旧 func:/name:/action_wait 等不再支持）"),
                "",
                k,
            ));
        } else if !matches!(k.as_str(), "params" | "config" | "steps") {
            top_errors.push(diag(
                "script.top_level.unknown_key",
                format!("未知顶层键 {k:?}，只允许 params/config/steps"),
                "",
                k,
            ));
        }
    }
    if !top_errors.is_empty() {
        // 顶层结构已破坏，不再下钻，避免错误风暴。
        out.extend(top_errors);
        return;
    }
    // 第 2 层：参数声明。
    if let Some((_, params)) = entries.iter().find(|(k, _)| k == "params") {
        check_params(params, out);
    }
    // 第 3~4 层的最小子集：步骤结构（match 形态）、call 自引用、func 路径穿越。
    if let Some((_, steps)) = entries.iter().find(|(k, _)| k == "steps") {
        check_steps(resource_id, steps, "steps", out);
    }
}

fn precheck_function_library(entries: &[(String, Node)], out: &mut Vec<Diagnostic>) {
    for (name, record) in entries {
        let Some(record_map) = record.as_map() else {
            out.push(diag(
                "func.record_type",
                format!("函数 {name} 的记录必须是映射"),
                name,
                "steps",
            ));
            continue;
        };
        for (k, v) in record_map {
            match k.as_str() {
                "params" => check_params(v, out),
                "steps" => check_steps("", v, &format!("{name}.steps"), out),
                other => out.push(diag(
                    "func.record_unknown_key",
                    format!("函数 {name} 记录含非法键 {other:?}，只允许 params/steps"),
                    name,
                    other,
                )),
            }
        }
    }
}

fn check_params(node: &Node, out: &mut Vec<Diagnostic>) {
    let Some(items) = node.as_seq() else {
        out.push(diag("param.decl.format", "params 必须是列表", "params", "params"));
        return;
    };
    let mut seen: Vec<String> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let path = format!("params[{i}]");
        let Some(raw) = item.as_str() else {
            out.push(diag(
                "param.decl.format",
                "params 项必须是标量（整条单引号）",
                &path,
                "declaration",
            ));
            continue;
        };
        // 「整条单引号」契约：必须写作单引号标量，否则与普通字符串无法区分，
        // 且备注/默认值中的特殊字符有歧义（serde_yaml 0.9 反序列化后样式丢失）。
        if item.scalar_style() != Some(ScalarStyle::SingleQuoted) {
            out.push(diag(
                "param.decl.quote_style",
                "params 项必须整条用单引号包裹，如 'bool:enable:开关:true'",
                &path,
                "style",
            ));
            continue;
        }
        let parts: Vec<&str> = raw.splitn(4, ':').collect();
        if parts.len() < 3 || parts[0].is_empty() || parts[1].is_empty() || parts[2].is_empty() {
            out.push(diag(
                "param.decl.format",
                "声明必须是 类型:变量名:备注[:默认值]，类型/变量名/备注不得为空",
                &path,
                "declaration",
            ));
            continue;
        }
        let name = parts[1].to_string();
        if seen.contains(&name) {
            out.push(diag(
                "param.decl.name_duplicate",
                format!("参数名 {name:?} 在同一参数表内重复"),
                &path,
                "name",
            ));
        } else {
            seen.push(name);
        }
        if let Some(tail) = parts.get(3) {
            if tail.is_empty() {
                out.push(diag(
                    "param.default.empty",
                    "空默认值：第四段尾串为空，不等价于没有默认值（空字符串须写成 \"\"）",
                    &path,
                    "default",
                ));
            } else if crate::model::typed_default(parts[0], tail).is_err() {
                out.push(diag(
                    "param.default.invalid",
                    format!("默认值 {tail:?} 不能按类型 {} 解析", parts[0]),
                    &path,
                    "default",
                ));
            }
        }
    }
}

fn check_steps(resource_id: &str, node: &Node, path: &str, out: &mut Vec<Diagnostic>) {
    let Some(items) = node.as_seq() else {
        out.push(diag("step.list_type", "步骤必须是列表", path, "steps"));
        return;
    };
    for (i, item) in items.iter().enumerate() {
        check_step(resource_id, item, &format!("{path}[{i}]"), out);
    }
}

fn check_step(resource_id: &str, item: &Node, path: &str, out: &mut Vec<Diagnostic>) {
    let Some(entries) = item.as_map() else {
        return; // 裸标量步骤与非法动作键归阶段 2 校验
    };
    let mut action: Option<(&str, &Node)> = None;
    let mut fields: Vec<(&str, &Node)> = Vec::new();
    for (k, v) in entries {
        if ACTION_KEYS.contains(&k.as_str()) {
            if action.is_some() {
                return; // 多动作键归阶段 2
            }
            action = Some((k.as_str(), v));
        } else {
            fields.push((k.as_str(), v));
        }
    }
    let Some((action, value)) = action else {
        return;
    };
    match action {
        "match" => check_match(resource_id, value, path, out),
        "call" => {
            if let Some(target) = value.as_str() {
                let normalized = target.strip_suffix(".yaml").unwrap_or(target);
                if normalized == resource_id {
                    out.push(diag(
                        "ref.call.self_cycle",
                        format!("call 目标 {target:?} 是脚本自身，形成调用环"),
                        path,
                        "target",
                    ));
                }
            }
        }
        "func" => {
            if let Some(target) = value.as_str() {
                if target.contains('\\')
                    || target.starts_with('/')
                    || target.split('/').any(|seg| seg == "..")
                {
                    out.push(diag(
                        "ref.func.path_traversal",
                        format!("函数路径 {target:?} 含 ..、绝对路径或反斜杠"),
                        path,
                        "target",
                    ));
                }
            }
        }
        _ => {}
    }
    // 递归下钻分支（then/else/steps 为兄弟键的步骤）。
    for (k, v) in fields {
        if matches!(k, "then" | "else" | "steps") {
            check_steps(resource_id, v, &format!("{path}.{k}"), out);
        }
    }
}

fn check_match(resource_id: &str, candidates_node: &Node, path: &str, out: &mut Vec<Diagnostic>) {
    let Some(cands) = candidates_node.as_seq() else {
        out.push(diag(
            "step.match.candidates_type",
            "match 值必须是候选列表（紧凑缩进）",
            path,
            "candidates",
        ));
        return;
    };
    let mut seen: Vec<String> = Vec::new();
    for (i, c) in cands.iter().enumerate() {
        let Some(entries) = c.as_map() else { continue };
        if entries.len() != 1 {
            continue; // 多键候选归阶段 2
        }
        let (key, steps_node) = &entries[0];
        if key == "else" || key == "timeout" {
            out.push(diag(
                "step.match.else_in_candidates",
                format!("{key:?} 写进了候选列表；else/timeout 必须是 match 步骤的兄弟键"),
                path,
                "candidates",
            ));
            continue;
        }
        if seen.iter().any(|s| s == key) {
            out.push(diag(
                "step.match.candidate_duplicate",
                format!("候选模板 {key:?} 重复"),
                path,
                "candidates",
            ));
        } else {
            seen.push(key.clone());
        }
        if steps_node.as_seq().is_some() {
            check_steps(resource_id, steps_node, &format!("{path}.candidates[{i}].steps"), out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// parse_function_file 入口的最小验证：函数记录只允许 params/steps。
    #[test]
    fn function_library_precheck_entry() {
        let src = "login:\n  params:\n    - 'tmpl:account:账号模板'\n  steps:\n    - return: true\n\nbad:\n  extra: 1\n";
        let diags = precheck("lib", ResourceKind::FunctionLibrary, src);
        assert_eq!(diags.len(), 1, "实际: {diags:?}");
        assert_eq!(diags[0].code, "func.record_unknown_key");
        assert_eq!(diags[0].step_path, "bad");
        assert_eq!(diags[0].field, "extra");
    }

    /// 函数库中的 params 同样受「整条单引号」约束。
    #[test]
    fn function_library_params_quote_style() {
        let src = "login:\n  params:\n    - bool:enable:开关:true\n  steps:\n    - return: true\n";
        let diags = precheck("lib", ResourceKind::FunctionLibrary, src);
        assert_eq!(diags.len(), 1, "实际: {diags:?}");
        assert_eq!(diags[0].code, "param.decl.quote_style");
    }
}
