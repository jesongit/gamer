wit_bindgen::generate!({
    path: "../../wit/gamer",
    world: "yaml-extension-host",
});

use exports::gamer::host::automation::Guest;
use gamer::host::{capability, programs};

/// A real Component guest fixture. The host sends the lowered small AST as
/// JSON; the fixture interprets control flow and forwards primitive invocations
/// through the WIT capability.invoke import. It intentionally has no WASI
/// imports and no access to Gamer internals.
///
/// 顶层可选 `start_index`（契约 §8）：跳过其前的**顶层**步骤（「从此运行」；
/// 嵌套分支 / 循环体不受影响——lower 后顶层小 AST 步与 surface 步 1:1 对应）。
/// `depth` 为当前调用深度（顶层 = 0）：`call` 进入被调方前 +1 并经
/// `programs.resolve(depth)` 透传给宿主做递归深度守卫（超限宿主回
/// CALL_DEPTH_EXCEEDED，本 guest 原样向上传播）。
struct Fixture;

impl Guest for Fixture {
    fn run(program_json: String) -> Result<String, String> {
        let program: serde_json::Value = serde_json::from_str(&program_json)
            .map_err(|error| format!("program JSON 无效: {error}"))?;
        let mut values = program
            .get("args")
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        apply_defaults(&program, &mut values);
        let all_steps = program
            .get("steps")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "program 缺少 steps".to_string())?;
        let start_index = program
            .get("start_index")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        if start_index > all_steps.len() {
            return Err(format!(
                "start_index {start_index} 超过顶层步数 {}",
                all_steps.len()
            ));
        }
        execute_steps(&all_steps[start_index..], &mut values, 0)?;
        Ok(
            serde_json::to_string(&values.remove("__return").unwrap_or(serde_json::Value::Null))
                .map_err(|error| error.to_string())?,
        )
    }
}

fn execute_steps(
    steps: &[serde_json::Value],
    values: &mut serde_json::Map<String, serde_json::Value>,
    depth: u32,
) -> Result<Flow, String> {
    for step in steps {
        match execute_step(step, values, depth)? {
            Flow::Continue => {}
            flow => return Ok(flow),
        }
    }
    Ok(Flow::Continue)
}

fn execute_step(
    step: &serde_json::Value,
    values: &mut serde_json::Map<String, serde_json::Value>,
    depth: u32,
) -> Result<Flow, String> {
    let op = step
        .get("op")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "small AST step 缺少 op".to_string())?;
    match op {
        "invoke" => {
            let capability_name = step
                .get("capability")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "invoke 缺少 capability".to_string())?;
            let args = evaluate_map(step.get("args"), values)?;
            let args_json = serde_json::to_string(&args).map_err(|error| error.to_string())?;
            let result =
                capability::invoke(capability_name, &args_json).map_err(format_host_error)?;
            let result: serde_json::Value = serde_json::from_str(&result)
                .map_err(|error| format!("capability 返回值无效: {error}"))?;
            if let Some(save) = step.get("save").and_then(serde_json::Value::as_str) {
                values.insert(save.to_string(), result);
            }
            Ok(Flow::Continue)
        }
        "set" => {
            let name = step
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "set 缺少 name".to_string())?;
            values.insert(name.to_string(), evaluate(step.get("value"), values)?);
            Ok(Flow::Continue)
        }
        "if" => {
            let condition = evaluate_condition(step.get("cond"), values)?;
            let key = if condition {
                "then_steps"
            } else {
                "else_steps"
            };
            match execute_steps(
                step.get(key)
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| format!("if 缺少 {key}"))?,
                values,
                depth,
            )? {
                Flow::Continue => Ok(Flow::Continue),
                flow => Ok(flow),
            }
        }
        "loop" => {
            let times = step
                .get("times")
                .and_then(|value| evaluate_value(value, values).ok());
            let limit = times.as_ref().and_then(|value| match value {
                serde_json::Value::Object(value)
                    if value.get("type") == Some(&serde_json::Value::String("int".into())) =>
                {
                    value.get("value").and_then(serde_json::Value::as_u64)
                }
                serde_json::Value::Object(value)
                    if value.get("type") == Some(&serde_json::Value::String("duration".into())) =>
                {
                    value
                        .get("value")
                        .and_then(serde_json::Value::as_u64)
                        .map(|value| (value / 100).max(1))
                }
                _ => None,
            });
            let body = step
                .get("body")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| "loop 缺少 body".to_string())?;
            let mut count = 0;
            loop {
                if let Some(limit) = limit {
                    if count >= limit {
                        break;
                    }
                }
                count += 1;
                match execute_steps(body, values, depth)? {
                    Flow::Continue => {}
                    Flow::Break => break,
                    flow => return Ok(flow),
                }
            }
            Ok(Flow::Continue)
        }
        "break" => Ok(Flow::Break),
        "return" => {
            let value = evaluate(step.get("value"), values)?;
            values.insert("__return".to_string(), value.clone());
            Ok(Flow::Return(value))
        }
        "throw" => Err(evaluate(step.get("message"), values)
            .ok()
            .and_then(|value| value.get("value").cloned())
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "YAML guest throw".to_string())),
        "call" => {
            let target = step
                .get("target")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "call 缺少 target".to_string())?;
            let args = evaluate_map(step.get("args"), values)?;
            let args_json = serde_json::to_string(&args).map_err(|error| error.to_string())?;
            // 被调方深度 = 当前 + 1；超限由宿主 resolver 侧守卫拒绝。
            let callee_json = programs::resolve(target, &args_json, depth + 1)
                .map_err(|error| error)?;
            let callee: serde_json::Value = serde_json::from_str(&callee_json)
                .map_err(|error| format!("call 目标不是有效程序: {error}"))?;
            let callee_steps = callee
                .get("steps")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| "call 目标缺少 steps".to_string())?;
            let mut child_values = args;
            apply_defaults(&callee, &mut child_values);
            match execute_steps(callee_steps, &mut child_values, depth + 1)? {
                Flow::Continue | Flow::Return(_) => {
                    if let Some(save) = step.get("save").and_then(serde_json::Value::as_str) {
                        values.insert(
                            save.to_string(),
                            child_values
                                .remove("__return")
                                .unwrap_or(serde_json::Value::Null),
                        );
                    }
                    Ok(Flow::Continue)
                }
                Flow::Break => Err("call 目标不能把 break 泄漏到调用方".to_string()),
            }
        }
        other => Err(format!("未知 small AST op: {other}")),
    }
}

fn format_host_error(error: gamer::host::types::HostError) -> String {
    use gamer::host::types::HostErrorKind;

    let kind = match error.kind {
        HostErrorKind::Denied => "denied",
        HostErrorKind::Unavailable => "unavailable",
        HostErrorKind::InvalidRequest => "invalid-request",
        HostErrorKind::NotFound => "not-found",
        HostErrorKind::Cancelled => "cancelled",
        HostErrorKind::Failed => "failed",
    };
    format!("kind={kind}; message={}", error.message)
}

fn evaluate_map(
    value: Option<&serde_json::Value>,
    values: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    value
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "invoke args 必须是 map".to_string())?
        .iter()
        .map(|(key, value)| Ok((key.clone(), evaluate(Some(value), values)?)))
        .collect()
}

fn evaluate(
    value: Option<&serde_json::Value>,
    values: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let value = value.ok_or_else(|| "表达式缺失".to_string())?;
    let expr = value
        .get("expr")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "表达式缺少 expr".to_string())?;
    match expr {
        "literal" => Ok(value
            .get("value")
            .cloned()
            .ok_or_else(|| "literal 缺少 value".to_string())?),
        "ref" => {
            let path = value
                .get("value")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "ref 缺少 value".to_string())?;
            lookup_path(values, path.trim_start_matches('$'))
                .ok_or_else(|| format!("未定义变量 {path}"))
        }
        "list" => value
            .get("value")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "list 缺少 value".to_string())
            .and_then(|items| {
                items
                    .iter()
                    .map(|item| evaluate(Some(item), values))
                    .collect::<Result<Vec<_>, _>>()
                    .map(serde_json::Value::Array)
            }),
        "map" => evaluate_map(value.get("value"), values).map(serde_json::Value::Object),
        other => Err(format!("未知表达式: {other}")),
    }
}

fn evaluate_value(
    value: &serde_json::Value,
    values: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value, String> {
    evaluate(Some(value), values)
}

fn apply_defaults(
    program: &serde_json::Value,
    values: &mut serde_json::Map<String, serde_json::Value>,
) {
    let Some(params) = program.get("params").and_then(serde_json::Value::as_array) else {
        return;
    };
    for param in params {
        let Some(name) = param.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if !values.contains_key(name) {
            if let Some(default) = param.get("default") {
                values.insert(name.to_string(), default.clone());
            }
        }
    }
}

fn evaluate_condition(
    value: Option<&serde_json::Value>,
    values: &serde_json::Map<String, serde_json::Value>,
) -> Result<bool, String> {
    let value = value.ok_or_else(|| "condition 缺失".to_string())?;
    let condition = value
        .get("condition")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "condition 缺少 condition".to_string())?;
    match condition {
        "truthy" => Ok(truthy(&evaluate(value.get("value"), values)?)),
        "equals" => Ok(values_equal(
            &evaluate(value.get("left"), values)?,
            &evaluate(value.get("right"), values)?,
        )),
        "not" => Ok(!evaluate_condition(value.get("value"), values)?),
        other => Err(format!("未知 condition: {other}")),
    }
}

fn truthy(value: &serde_json::Value) -> bool {
    if let Some(object) = value.as_object() {
        if let Some(kind) = object.get("type").and_then(serde_json::Value::as_str) {
            let payload = object.get("value").unwrap_or(&serde_json::Value::Null);
            return match kind {
                "null" => false,
                "bool" => payload.as_bool().unwrap_or(false),
                "int" | "float" => payload.as_f64().is_some_and(|value| value != 0.0),
                "string" | "color" => payload.as_str().is_some_and(|value| !value.is_empty()),
                "duration" => payload.as_u64().is_some_and(|value| value != 0),
                "coordinate" | "handle" => true,
                "list" => payload.as_array().is_some_and(|value| !value.is_empty()),
                "map" => payload
                    .as_object()
                    .and_then(|value| value.get("found"))
                    .map(truthy)
                    .unwrap_or_else(|| payload.as_object().is_some_and(|value| !value.is_empty())),
                _ => true,
            };
        }
    }
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        serde_json::Value::String(value) => !value.is_empty(),
        serde_json::Value::Array(value) => !value.is_empty(),
        serde_json::Value::Object(value) => {
            value.get("found").map(truthy).unwrap_or(!value.is_empty())
        }
    }
}

fn lookup_path(
    values: &serde_json::Map<String, serde_json::Value>,
    path: &str,
) -> Option<serde_json::Value> {
    let mut segments = path.split('.');
    let mut current = values.get(segments.next()?)?.clone();
    for segment in segments {
        let (name, indexes) = parse_segment(segment);
        if !name.is_empty() {
            current = typed_map_get(&current, name)?;
        }
        for index in indexes {
            current = typed_list_get(&current, index)?;
        }
    }
    Some(current)
}

fn typed_map_get(value: &serde_json::Value, key: &str) -> Option<serde_json::Value> {
    let object = value.as_object()?;
    if object.get("type").and_then(serde_json::Value::as_str) == Some("map") {
        object.get("value")?.as_object()?.get(key).cloned()
    } else {
        object.get(key).cloned()
    }
}

fn typed_list_get(value: &serde_json::Value, index: usize) -> Option<serde_json::Value> {
    let object = value.as_object()?;
    if object.get("type").and_then(serde_json::Value::as_str) == Some("list") {
        object.get("value")?.as_array()?.get(index).cloned()
    } else {
        value.as_array()?.get(index).cloned()
    }
}

fn parse_segment(segment: &str) -> (&str, Vec<usize>) {
    let name = segment.split('[').next().unwrap_or(segment);
    let mut indexes = Vec::new();
    let mut rest = segment.strip_prefix(name).unwrap_or_default();
    while let Some(rest_value) = rest.strip_prefix('[') {
        let Some(end) = rest_value.find(']') else {
            break;
        };
        let Ok(index) = rest_value[..end].parse() else {
            break;
        };
        indexes.push(index);
        rest = &rest_value[end + 1..];
    }
    (name, indexes)
}

fn values_equal(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    if left == right {
        return true;
    }
    let left_kind = left.get("type").and_then(serde_json::Value::as_str);
    let right_kind = right.get("type").and_then(serde_json::Value::as_str);
    if left_kind == Some("int") && right_kind == Some("float")
        || left_kind == Some("float") && right_kind == Some("int")
    {
        return left.get("value").and_then(serde_json::Value::as_f64)
            == right.get("value").and_then(serde_json::Value::as_f64);
    }
    if left_kind == Some("color") || right_kind == Some("color") {
        let color = |value: &serde_json::Value| -> Option<String> {
            if value.get("type").and_then(serde_json::Value::as_str) == Some("color") {
                value
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            } else {
                value
                    .get("value")?
                    .get("hex")?
                    .get("value")?
                    .as_str()
                    .map(str::to_string)
            }
        };
        return color(left).zip(color(right)).is_some_and(|(left, right)| {
            left.trim_start_matches('#')
                .eq_ignore_ascii_case(right.trim_start_matches('#'))
        });
    }
    false
}

enum Flow {
    Continue,
    Break,
    Return(serde_json::Value),
}

export!(Fixture);
