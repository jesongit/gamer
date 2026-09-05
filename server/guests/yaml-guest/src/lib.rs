wit_bindgen::generate!({
    path: "../../wit/gamer",
    world: "yaml-extension-host",
});

use exports::gamer::host::automation::Guest;
use gamer::host::{capability, programs};

/// gamer.yaml 官方产品 guest（与测试同源：yaml_extension.rs 测试链路现场构建
/// 本 crate，官方市场包由 tools/build-plugins.ps1 打包同一份源码）。宿主下发
/// lower 后的小 AST JSON；本 guest 解释控制流并经 WIT capability.invoke 转发
/// 原语调用。刻意无 WASI import、不触达 Gamer 内部。
///
/// 顶层可选 `start_index`（契约 §8）：跳过其前的**顶层**步骤（「从此运行」；
/// 嵌套分支 / 循环体不受影响——lower 后顶层小 AST 步与 surface 步 1:1 对应）。
///
/// ExecutionBudget（ADR-YAML-04 / 契约 §5）：步数与调用深度由 guest 本地计数，
/// 不经 WIT 透传给宿主。每个逻辑步执行前计数 +1（顶层、loop 体每轮每个子步、
/// if 分支体、call 目标程序体全计，loop 每轮迭代本身也计——空转体死循环同受
/// 约束）；call 进入被调方深度 +1、返回 -1。超限返回以机器可读码开头的错误
/// 文本（`STEP_BUDGET_EXCEEDED` / `CALL_DEPTH_EXCEEDED`），该文本经宿主原样
/// 进入 RunRecord 错误信息与运行日志。
///
/// 运行可视化事件（P12.6 / 契约 §6 / ADR-YAML-03）：经私有
/// `capability.invoke("__event", …)` 通道发射（宿主拦截转发 EventSink，不进
/// 权限声明）：`run_start` / `run_end` / `step_start` / `step_end` /
/// `call_start` / `budget`。发射是尽力而为——宿主无 sink 或拒绝时忽略返回值
/// 继续执行；lower 展开物（timing sleep / 轮询体）不带 label（`op: step`
/// 包装），天然静默。
struct YamlGuest;

impl Guest for YamlGuest {
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
        emit_event(serde_json::json!({ "ev": "run_start" }));
        let mut budget = ExecutionBudget::default();
        // wait 随机区间（契约 §4，方案 (a)）：宿主注入的 run 级 nonce 作
        // splitmix64 种子（算法与宿主侧 yaml_vnext::splitmix64 逐字一致，
        // 测试向量锁定在两侧）。
        budget.rng = program.get("nonce").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let outcome =
            execute_steps(&all_steps[start_index..], &mut values, &mut budget);
        match outcome {
            // 顶层 break 与既有语义一致（忽略，等价 Continue 收尾）
            Ok(Flow::Continue | Flow::Return | Flow::Break) => {
                emit_event(serde_json::json!({ "ev": "run_end", "ok": true }));
            }
            Err(error) => {
                // 预算/取消错误在 run_end 之前先发 budget 终止原因（ADR-YAML-04
                // 错误码：必须进 Run Event 可观察）
                let kind = budget_kind(&error);
                if let Some(kind) = kind {
                    emit_event(serde_json::json!({ "ev": "budget", "kind": kind }));
                }
                emit_event(serde_json::json!({
                    "ev": "run_end", "ok": false, "error": error
                }));
                return Err(error);
            }
        }
        Ok(
            serde_json::to_string(&values.remove("__return").unwrap_or(serde_json::Value::Null))
                .map_err(|error| error.to_string())?,
        )
    }
}

/// 运行事件发射（best-effort）：私有 `__event` capability，宿主拦截转发；
/// 失败静默——可视化事件不影响运行结果。
fn emit_event(event: serde_json::Value) {
    if let Ok(args) = serde_json::to_string(&event) {
        let _ = capability::invoke("__event", &args);
    }
}

/// 预算/取消错误码 → budget 事件 kind（与 ADR-YAML-04 对应）。
fn budget_kind(error: &str) -> Option<&'static str> {
    if error.starts_with("STEP_BUDGET_EXCEEDED") {
        Some("STEP_BUDGET_EXCEEDED")
    } else if error.starts_with("CALL_DEPTH_EXCEEDED") {
        Some("CALL_DEPTH_EXCEEDED")
    } else if error.starts_with("CANCELLED") || error.contains("kind=cancelled") {
        Some("CANCELLED")
    } else {
        None
    }
}

/// 执行预算（ADR-YAML-04）。常量必须与宿主侧原生参考解释器
/// （server/src/extensions/gamer_yaml/yaml_extension.rs 的 MAX_STEPS /
/// MAX_CALL_DEPTH）保持一致；两处各自独立编译，无法共享代码。
const MAX_STEPS: u64 = 100_000;
const MAX_CALL_DEPTH: u32 = 32;

struct ExecutionBudget {
    steps: u64,
    call_depth: u32,
    /// wait 随机区间的 splitmix64 状态（run nonce 播种，跨步骤连续推进）。
    rng: u64,
}

impl Default for ExecutionBudget {
    fn default() -> Self {
        Self {
            steps: 0,
            call_depth: 0,
            rng: 0,
        }
    }
}

impl ExecutionBudget {
    /// 每个逻辑步执行前调用：计数 +1，超限报结构化错误。
    fn begin_step(&mut self) -> Result<(), String> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err(format!(
                "STEP_BUDGET_EXCEEDED: consumed={} max={MAX_STEPS}",
                self.steps
            ));
        }
        Ok(())
    }

    /// call 进入被调方：深度 +1，超限立即终止。
    fn enter_call(&mut self) -> Result<(), String> {
        self.call_depth += 1;
        if self.call_depth > MAX_CALL_DEPTH {
            return Err(format!(
                "CALL_DEPTH_EXCEEDED: depth={} max={MAX_CALL_DEPTH}",
                self.call_depth
            ));
        }
        Ok(())
    }

    /// call 返回：深度 -1（所有退出路径都必须配对调用）。
    fn exit_call(&mut self) {
        self.call_depth -= 1;
    }
}

/// wait 随机区间的 PRNG：splitmix64，与宿主侧
/// `server/src/extensions/gamer_yaml/yaml_vnext.rs` 的 `splitmix64` 逐字一致
///（测试向量锁定在两侧：seed=7 → 7191089600892374487, 309689372594955804,
/// 16616101746815609346）。算法/常量改动必须两处同步。
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

fn execute_steps(
    steps: &[serde_json::Value],
    values: &mut serde_json::Map<String, serde_json::Value>,
    budget: &mut ExecutionBudget,
) -> Result<Flow, String> {
    for step in steps {
        // 每个逻辑步执行前计数：顶层、loop 体每轮子步、if 分支体、call 目标
        // 程序体全计（ADR-YAML-04：外层 loop 包裹不得绕过预算）。
        budget.begin_step()?;
        match execute_step(step, values, budget)? {
            Flow::Continue => {}
            flow => return Ok(flow),
        }
    }
    Ok(Flow::Continue)
}

fn execute_step(
    step: &serde_json::Value,
    values: &mut serde_json::Map<String, serde_json::Value>,
    budget: &mut ExecutionBudget,
) -> Result<Flow, String> {
    let op = step
        .get("op")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "small AST step 缺少 op".to_string())?;
    match op {
        // P12.6 运行身份包装（lower 为每个 surface step 生成 label）：进入/
        // 完成/失败发 step 事件；包装步就是原逻辑步（不再额外计预算）。
        // 下层展开物（timing sleep / find 轮询体）无包装，天然静默。
        "step" => {
            let label = step.get("label").ok_or_else(|| "step 缺少 label".to_string())?;
            let path = label
                .get("path")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "step label 缺少 path".to_string())?;
            let desc = label
                .get("desc")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            emit_event(serde_json::json!({
                "ev": "step_start", "path": path, "desc": desc
            }));
            let inner = step
                .get("step")
                .ok_or_else(|| "step 缺少被包装节点".to_string())?;
            match execute_step(inner, values, budget) {
                Ok(flow) => {
                    emit_event(serde_json::json!({
                        "ev": "step_end", "path": path, "ok": true
                    }));
                    Ok(flow)
                }
                Err(error) => {
                    emit_event(serde_json::json!({
                        "ev": "step_end", "path": path, "ok": false, "error": error
                    }));
                    Err(error)
                }
            }
        }
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
        // wait 随机区间（契约 §4）：[min, max] 内取随机时长后经 runtime.sleep
        // 等待（取消语义与普通 sleep 一致）。
        "wait_random" => {
            let min = evaluate(step.get("min"), values)?
                .get("value")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| "wait min 必须是时间值".to_string())?;
            let max = evaluate(step.get("max"), values)?
                .get("value")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| "wait max 必须是时间值".to_string())?;
            let duration = if max > min {
                min + splitmix64(&mut budget.rng) % (max - min + 1)
            } else {
                min
            };
            let sleep_args = serde_json::json!({
                "duration": {"type": "duration", "value": duration}
            });
            let args_json =
                serde_json::to_string(&sleep_args).map_err(|error| error.to_string())?;
            capability::invoke("runtime.sleep", &args_json).map_err(format_host_error)?;
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
                budget,
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
                // 每轮迭代本身也是逻辑步：空转体（body 无子步）的无 times loop
                // 同样受预算约束终止，而不是永不退出。
                budget.begin_step()?;
                count += 1;
                match execute_steps(body, values, budget)? {
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
            Ok(Flow::Return)
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
            // 深度本地计数：进入被调方 +1、返回 -1，所有退出路径都配对回退；
            // 超限立即终止（ADR-YAML-04，不再经 WIT depth 透传宿主守卫）。
            budget.enter_call()?;
            // P12.6：宣告进入被调方帧（depth = 本地计数；被调方内部 step 事件
            // 的 path 保持 script-local 契约形态）
            emit_event(serde_json::json!({
                "ev": "call_start", "target": target, "depth": budget.call_depth
            }));
            let outcome = (|| -> Result<Flow, String> {
                let args = evaluate_map(step.get("args"), values)?;
                let args_json = serde_json::to_string(&args).map_err(|error| error.to_string())?;
                let callee_json = programs::resolve(target, &args_json)?;
                let callee: serde_json::Value = serde_json::from_str(&callee_json)
                    .map_err(|error| format!("call 目标不是有效程序: {error}"))?;
                let callee_steps = callee
                    .get("steps")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| "call 目标缺少 steps".to_string())?;
                let mut child_values = args;
                apply_defaults(&callee, &mut child_values);
                match execute_steps(callee_steps, &mut child_values, budget)? {
                    Flow::Continue | Flow::Return => {
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
            })();
            budget.exit_call();
            outcome
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
    Return,
}

export!(YamlGuest);
