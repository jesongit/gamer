//! Gamer YAML V1 权威解释器（Phase 1/2，`docs/plans/gamer_v1_simplification_plan.md`）。
//!
//! 全仓只有这一份 YAML 执行逻辑：
//!
//! - 生产执行：`gamer-yaml` 官方 guest（`plugins/gamer-yaml/guest`，WASM
//!   Component）链接本 crate，经 `capability.invoke("__fn", …)` 调宿主函数、
//!   `capability.invoke("__event", …)` 发运行事件；
//! - 测试执行：server 侧以 dev-dependency 原生编译本 crate，用假 HostFunctions
//!   直接驱动同一解释器（计划 Phase 2：不重新编写参考解释器）。
//!
//! 解释器只认识 V1 最小语法（计划 §1.1）：`run` 步骤 = 函数调用 / `if` /
//! `repeat` / `return`；`tap`、`find`、`sleep` 等都不是语法关键字，而是宿主
//! 注册的函数。表达式只有字面量与 `$name.field` 引用，无算术、无插值、无 eval。
//!
//! wire 形态（宿主解析/校验/lowering 的产出，见 gamer_yaml `syntax` 模块）：
//!
//! ```json
//! {
//!   "vars":  {"timeout": "15s"},
//!   "run":   [{"op":"fn","fn":"tap","args":{"expr":"lit","value":[0.5,0.8]},
//!              "as":"hit","path":"run[0]","desc":"…"}],
//!   "functions": {"claim_daily": {"params":[…],"vars":{…},"run":[…]}},
//!   "start_index": 2
//! }
//! ```
//!
//! 执行预算（ADR-YAML-04 语义沿用）：每逻辑步 +1、Package 函数调用深度 +1，
//! 超限返回机器可读码前缀错误（`STEP_BUDGET_EXCEEDED` / `CALL_DEPTH_EXCEEDED`）；
//! 取消经 [`HostFunctions::cancelled`] 轮询 + 宿主函数错误（`kind=cancelled`）。

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

/// 步数预算：每个逻辑步（顶层、分支体、repeat 体每轮每子步、函数体全计）
/// 执行前 +1，超限即终止。
pub const MAX_STEPS: u64 = 100_000;

/// Package 函数调用深度上限（本地解释递归；原生函数是叶节点不占深度）。
pub const MAX_CALL_DEPTH: u32 = 32;

/// `return` 步写入帧的保留键。
const RETURN_KEY: &str = "__return";

// ---------------------------------------------------------------------------
// 宿主边界
// ---------------------------------------------------------------------------

/// 宿主函数调用错误（与 WIT host-error 同构；guest 侧原样转 HostError）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostErrorKind {
    Denied,
    Unavailable,
    InvalidRequest,
    NotFound,
    Cancelled,
    Failed,
}

impl HostErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::Unavailable => "unavailable",
            Self::InvalidRequest => "invalid-request",
            Self::NotFound => "not-found",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct HostError {
    pub kind: HostErrorKind,
    pub message: String,
}

impl HostError {
    pub fn new(kind: HostErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "kind={}; message={}", self.kind.as_str(), self.message)
    }
}

/// 宿主函数通道：解释器遇到 `functions` 表之外的函数名时调用。
///
/// 生产实现（wasm guest 胶水）把调用转发到 `capability.invoke("__fn", …)`，
/// 宿主侧按当前运行上下文与插件权限执行原生函数；测试实现用假表驱动。
pub trait HostFunctions: Send + Sync {
    fn invoke(&self, name: &str, args: Value) -> Result<Value, HostError>;

    /// 取消轮询（每逻辑步执行前调用；wasm 路径恒 false，取消由 epoch
    /// interruption 与宿主函数错误承担）。
    fn cancelled(&self) -> bool {
        false
    }
}

/// 运行结构事件汇（`{"ev":…}` JSON，词表与 Core RuntimeEventKind 对齐）：
/// `run_start` / `run_end` / `step_start` / `step_end` / `call_start` /
/// `budget`。发射是尽力而为，失败不影响执行结果。
pub trait EventSink: Send + Sync {
    fn emit(&self, event: Value);
}

// ---------------------------------------------------------------------------
// wire 类型（宿主 lowering 产出）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct Program {
    #[serde(default)]
    pub trace: Value,
    /// 入口帧初始值（已绑定参数 + vars 字面量，宿主绑定产出）。
    #[serde(default)]
    pub vars: serde_json::Map<String, Value>,
    #[serde(default)]
    pub run: Vec<Step>,
    /// 当前 Package 函数定义（运行开始时冻结；计划 Phase 3.4）。
    #[serde(default)]
    pub functions: BTreeMap<String, serde_json::Value>,
    /// 「从此运行」：跳过入口 run 前 N 个顶层步。
    #[serde(default)]
    pub start_index: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FunctionDef {
    #[serde(default)]
    pub params: Vec<ParamDecl>,
    #[serde(default)]
    pub vars: serde_json::Map<String, Value>,
    #[serde(default)]
    pub run: Vec<Step>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ParamDecl {
    pub name: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Step {
    #[serde(flatten)]
    pub kind: StepKind,
    /// 稳定步骤路径（`run[0]` / `run[0].then[1]` / `<函数名>.run[2]`）。
    pub path: String,
    #[serde(default)]
    pub desc: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StepKind {
    MatchTemplates {
        args: Expr,
        cases: Vec<TemplateBranch>,
        #[serde(default, rename = "else")]
        else_steps: Vec<Step>,
    },
    /// 函数调用：先查 `functions`（Package 函数，本地解释），未命中走宿主
    /// [`HostFunctions`]（原生函数）。`as` 接收返回值。
    Fn {
        #[serde(rename = "fn")]
        name: String,
        #[serde(default)]
        args: Option<Expr>,
        #[serde(default, rename = "as")]
        save_as: Option<String>,
    },
    If {
        cond: Expr,
        #[serde(default, rename = "then")]
        then_steps: Vec<Step>,
        #[serde(default, rename = "else")]
        else_steps: Vec<Step>,
    },
    Repeat {
        times: Expr,
        #[serde(default, rename = "do")]
        body: Vec<Step>,
    },
    Break,
    Return {
        value: Expr,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct TemplateBranch {
    #[serde(default, rename = "as")]
    pub save_as: Option<String>,
    #[serde(rename = "do")]
    pub body: Vec<Step>,
}

/// 表达式：字面量或 `$name.field` 引用（无第三种）；字面量容器（数组/映射）
/// 的叶子仍可以是引用——`swipe: {from: $a, to: $b}` 的参数树。
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "expr", rename_all = "snake_case")]
pub enum Expr {
    Lit {
        value: Value,
    },
    Ref {
        path: String,
    },
    List {
        value: Vec<Expr>,
    },
    Map {
        value: std::collections::BTreeMap<String, Expr>,
    },
}

enum Flow {
    Continue,
    Break,
    Return(Value),
}

// ---------------------------------------------------------------------------
// 解释器
// ---------------------------------------------------------------------------

/// 预算/取消错误 → `budget{kind}` 事件 kind（ADR-YAML-04 错误码）。
pub fn budget_kind(error: &str) -> Option<&'static str> {
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

/// 一次性运行入口：返回值为入口 `return` 值（未显式返回时 null）。
///
/// `functions` 表已由宿主解析为 [`FunctionDef`]；单个函数定义损坏（不可能经
/// 正常 lowering 产生）报结构化错误终止运行。
pub fn run(
    program: &Program,
    host: &dyn HostFunctions,
    events: Option<&dyn EventSink>,
) -> Result<Value, String> {
    let mut functions = BTreeMap::new();
    for (name, def) in &program.functions {
        let def: FunctionDef = serde_json::from_value(def.clone())
            .map_err(|error| format!("函数 {name} 定义无效: {error}"))?;
        functions.insert(name.clone(), def);
    }
    let mut interp = Interpreter {
        host,
        events,
        functions,
        steps: 0,
        call_depth: 0,
        trace: program.trace.clone(),
        frame_id: 0,
        next_frame_id: 0,
        parent_frame_id: None,
        function: None,
    };
    interp.run(program)
}

struct Interpreter<'a> {
    host: &'a dyn HostFunctions,
    events: Option<&'a dyn EventSink>,
    functions: BTreeMap<String, FunctionDef>,
    steps: u64,
    call_depth: u32,
    trace: Value,
    frame_id: u64,
    next_frame_id: u64,
    parent_frame_id: Option<u64>,
    function: Option<String>,
}

impl<'a> Interpreter<'a> {
    fn run(&mut self, program: &Program) -> Result<Value, String> {
        if program.start_index > program.run.len() {
            return Err(format!(
                "start_index {} 超过顶层步数 {}",
                program.start_index,
                program.run.len()
            ));
        }
        self.emit(serde_json::json!({ "ev": "run_start" }));
        let mut values = program.vars.clone();
        let steps = &program.run[program.start_index..];
        let outcome = self
            .run_steps(steps, &mut values)
            .and_then(|flow| match flow {
                Flow::Break => {
                    Err("yaml.break.outside_loop: break 只能在当前函数的 repeat 循环内使用".into())
                }
                flow => Ok(flow),
            });
        match outcome {
            Ok(flow) => {
                self.emit(serde_json::json!({ "ev": "run_end", "ok": true }));
                Ok(match flow {
                    Flow::Return(value) => value,
                    Flow::Break => unreachable!("break is rejected above"),
                    Flow::Continue => values.remove(RETURN_KEY).unwrap_or(Value::Null),
                })
            }
            Err(error) => {
                if let Some(kind) = budget_kind(&error) {
                    self.emit(serde_json::json!({ "ev": "budget", "kind": kind }));
                }
                self.emit(serde_json::json!({
                    "ev": "run_end", "ok": false, "error": error
                }));
                Err(error)
            }
        }
    }

    fn emit(&self, mut event: Value) {
        if self.trace.is_object() {
            let source = self
                .function
                .as_ref()
                .and_then(|name| self.trace.get("functions")?.get(name))
                .or_else(|| self.trace.get("entry"));
            event["trace"] = serde_json::json!({ "run_id": self.trace["run_id"], "source": source,
                "frame_id": self.frame_id, "parent_frame_id": self.parent_frame_id });
        }
        if let Some(sink) = self.events {
            sink.emit(event);
        }
    }

    fn detail(&self, name: &str, data: Value) {
        self.emit(serde_json::json!({"ev":"detail", "name":name, "data":data}));
    }

    fn emit_step_start(&self, step: &Step, values: &serde_json::Map<String, Value>) {
        let display_name = match &step.kind {
            StepKind::Fn {
                args: Some(Expr::Map { value }),
                ..
            } => value
                .get("name")
                .and_then(|expr| self.eval(Some(expr), values).ok())
                .and_then(|value| value.as_str().map(str::to_owned)),
            _ => None,
        };
        self.emit(serde_json::json!({
            "ev": "step_start", "path": step.path, "desc": display_name.as_deref().unwrap_or(&step.desc),
        }));
    }

    fn emit_step_end(&self, step: &Step, ok: bool, error: Option<&str>) {
        let mut event = serde_json::json!({
            "ev": "step_end", "path": step.path, "ok": ok,
        });
        if let (Some(error), Some(object)) = (error, event.as_object_mut()) {
            object.insert("error".into(), Value::from(error));
        }
        self.emit(event);
    }

    /// 每个逻辑步执行前：取消检查 + 步数预算 +1。
    fn begin_step(&mut self) -> Result<(), String> {
        if self.host.cancelled() {
            return Err("CANCELLED: 运行已取消".to_string());
        }
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err(format!(
                "STEP_BUDGET_EXCEEDED: consumed={} max={MAX_STEPS}",
                self.steps
            ));
        }
        Ok(())
    }

    fn run_steps(
        &mut self,
        steps: &[Step],
        values: &mut serde_json::Map<String, Value>,
    ) -> Result<Flow, String> {
        for step in steps {
            self.begin_step()?;
            self.emit_step_start(step, values);
            let outcome = self.run_step(step, values);
            match &outcome {
                Ok(_) => self.emit_step_end(step, true, None),
                Err(error) => self.emit_step_end(step, false, Some(error)),
            }
            match outcome? {
                Flow::Continue => {}
                flow => return Ok(flow),
            }
        }
        Ok(Flow::Continue)
    }

    fn run_step(
        &mut self,
        step: &Step,
        values: &mut serde_json::Map<String, Value>,
    ) -> Result<Flow, String> {
        match &step.kind {
            StepKind::MatchTemplates {
                args,
                cases,
                else_steps,
            } => {
                let args = self.eval(Some(args), values)?;
                self.detail(
                    "arguments",
                    serde_json::json!({"path":step.path,"function":"find_any","args":args}),
                );
                let matched = self
                    .host
                    .invoke("find_any", args)
                    .map_err(|e| e.to_string())?;
                self.detail("branch", serde_json::json!({"path":step.path,"kind":"match_templates","selected":matched.get("index").cloned().unwrap_or(Value::String("else".into())),"result":matched}));
                if matched.is_null() {
                    return self.run_steps(else_steps, values);
                }
                let index = matched
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|i| usize::try_from(i).ok())
                    .ok_or("匹配结果缺少有效分支 index")?;
                let branch = cases.get(index).ok_or("匹配分支 index 越界")?;
                let previous = branch
                    .save_as
                    .as_ref()
                    .map(|name| values.insert(name.clone(), matched));
                let outcome = self.run_steps(&branch.body, values);
                if let (Some(name), Some(previous)) = (&branch.save_as, previous) {
                    match previous {
                        Some(value) => {
                            values.insert(name.clone(), value);
                        }
                        None => {
                            values.remove(name);
                        }
                    }
                }
                outcome
            }
            StepKind::Fn {
                name,
                args,
                save_as,
            } => {
                let args = self.eval(args.as_ref(), values)?;
                if args.get("name").is_some_and(|value| !value.is_string()) {
                    return Err("name 必须是字符串".to_string());
                }
                self.detail(
                    "arguments",
                    serde_json::json!({"path":step.path,"function":name,"args":args}),
                );
                if self.functions.contains_key(name) {
                    self.call_package_function(name, args, save_as, values)
                } else {
                    self.call_host_function(name, args, save_as, values)
                }
            }
            StepKind::If {
                cond,
                then_steps,
                else_steps,
            } => {
                let value = self.eval(Some(cond), values)?;
                self.detail("branch", serde_json::json!({"path":step.path,"kind":"if","condition":value,"selected":if is_truthy(&value) {"then"} else {"else"}}));
                let branch = if is_truthy(&value) {
                    then_steps
                } else {
                    else_steps
                };
                self.run_steps(branch, values)
            }
            StepKind::Repeat { times, body } => {
                let times = self.eval(Some(times), values)?;
                let count = times
                    .as_u64()
                    .ok_or_else(|| format!("repeat 次数必须是零或正整数，得到 {times}"))?;
                for iteration in 0..count {
                    self.detail(
                        "iteration",
                        serde_json::json!({"path":step.path,"iteration":iteration+1,"total":count}),
                    );
                    // 每轮迭代本身也是逻辑步：空转体同样受预算约束终止。
                    self.begin_step()?;
                    match self.run_steps(body, values)? {
                        Flow::Continue => {}
                        Flow::Break => break,
                        flow => return Ok(flow),
                    }
                }
                Ok(Flow::Continue)
            }
            StepKind::Break => Ok(Flow::Break),
            StepKind::Return { value } => {
                let value = self.eval(Some(value), values)?;
                values.insert(RETURN_KEY.to_string(), value.clone());
                Ok(Flow::Return(value))
            }
        }
    }

    fn call_host_function(
        &mut self,
        name: &str,
        args: Value,
        save_as: &Option<String>,
        values: &mut serde_json::Map<String, Value>,
    ) -> Result<Flow, String> {
        let result = self
            .host
            .invoke(name, args)
            .map_err(|error| error.to_string())?;
        self.detail(
            "result",
            serde_json::json!({"function":name,"value":result,"as":save_as}),
        );
        if let Some(save_as) = save_as {
            values.insert(save_as.clone(), result);
        }
        Ok(Flow::Continue)
    }

    /// Package 函数：独立局部作用域（参数显式传入 + 函数 vars 字面量），
    /// 深度本地计数，返回值经 `as` 接收（缺省丢弃）。
    fn call_package_function(
        &mut self,
        name: &str,
        args: Value,
        save_as: &Option<String>,
        values: &mut serde_json::Map<String, Value>,
    ) -> Result<Flow, String> {
        self.call_depth += 1;
        if self.call_depth > MAX_CALL_DEPTH {
            self.call_depth -= 1;
            return Err(format!(
                "CALL_DEPTH_EXCEEDED: depth={} max={MAX_CALL_DEPTH}",
                self.call_depth
            ));
        }
        let previous = (self.frame_id, self.parent_frame_id, self.function.clone());
        self.next_frame_id += 1;
        self.parent_frame_id = Some(self.frame_id);
        self.frame_id = self.next_frame_id;
        self.function = Some(name.to_string());
        self.emit(
            serde_json::json!({ "ev": "call_start", "target": name, "depth": self.call_depth }),
        );
        let outcome = self.run_package_function(name, args, save_as, values);
        (self.frame_id, self.parent_frame_id, self.function) = previous;
        self.call_depth -= 1;
        outcome
    }

    fn run_package_function(
        &mut self,
        name: &str,
        args: Value,
        save_as: &Option<String>,
        values: &mut serde_json::Map<String, Value>,
    ) -> Result<Flow, String> {
        let def = self
            .functions
            .get(name)
            .cloned()
            .ok_or_else(|| format!("未定义函数 {name}"))?;
        let mut frame = def.vars.clone();
        let args = args.as_object().cloned().unwrap_or_default();
        for param in &def.params {
            match args.get(&param.name) {
                Some(value) => {
                    frame.insert(param.name.clone(), value.clone());
                }
                None => {
                    if let Some(default) = &param.default {
                        frame.insert(param.name.clone(), default.clone());
                    } else if param.required {
                        return Err(format!("函数 {name} 缺少必填参数 {}", param.name));
                    }
                }
            }
        }
        let bound: serde_json::Map<String, Value> = def.params.iter().filter_map(|p| frame.get(&p.name).map(|v| (p.name.clone(), v.clone()))).collect();
        self.detail("effective_args", serde_json::json!({"function":name,"args":bound}));
        let return_value = match self.run_steps(&def.run, &mut frame)? {
            Flow::Return(value) => value,
            Flow::Continue => Value::Null,
            Flow::Break => return Err("yaml.break.outside_loop: break 不能跳出调用方的循环".into()),
        };
        self.detail(
            "result",
            serde_json::json!({"function":name,"value":return_value,"as":save_as}),
        );
        if let Some(save_as) = save_as {
            values.insert(save_as.clone(), return_value);
        }
        Ok(Flow::Continue)
    }

    fn eval(
        &self,
        expr: Option<&Expr>,
        values: &serde_json::Map<String, Value>,
    ) -> Result<Value, String> {
        let Some(expr) = expr else {
            return Ok(Value::Null);
        };
        match expr {
            Expr::Lit { value } => Ok(value.clone()),
            Expr::Ref { path } => {
                lookup_path(values, path).ok_or_else(|| format!("未定义变量 ${path}"))
            }
            Expr::List { value } => {
                let mut items = Vec::with_capacity(value.len());
                for item in value {
                    items.push(self.eval(Some(item), values)?);
                }
                Ok(Value::Array(items))
            }
            Expr::Map { value } => {
                let mut map = serde_json::Map::with_capacity(value.len());
                for (key, item) in value {
                    map.insert(key.clone(), self.eval(Some(item), values)?);
                }
                Ok(Value::Object(map))
            }
        }
    }
}

/// `if` 条件真值：`false` 与 `null` 为假，非空结果为真（计划 §1.5，不做
/// 数字/字符串等隐式转换）。
fn is_truthy(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false))
}

/// `$name.field.sub` 引用求值：仅点号字段访问（V1 无动态索引）。
fn lookup_path(values: &serde_json::Map<String, Value>, path: &str) -> Option<Value> {
    let mut segments = path.split('.');
    let mut current = values.get(segments.next()?)?.clone();
    for segment in segments {
        let object = current.as_object()?;
        current = object.get(segment)?.clone();
    }
    Some(current)
}

#[cfg(test)]
mod template_branch_tests;

#[cfg(test)]
mod break_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeHost {
        calls: Mutex<Vec<(String, Value)>>,
        results: BTreeMap<String, Value>,
        cancelled: std::sync::atomic::AtomicBool,
    }

    impl FakeHost {
        fn with(name: &str, value: Value) -> Self {
            let mut host = Self::default();
            host.results.insert(name.to_string(), value);
            host
        }
    }

    impl HostFunctions for FakeHost {
        fn invoke(&self, name: &str, args: Value) -> Result<Value, HostError> {
            self.calls
                .lock()
                .unwrap()
                .push((name.to_string(), args.clone()));
            match self.results.get(name) {
                Some(value) => Ok(value.clone()),
                None => Ok(Value::Null),
            }
        }

        fn cancelled(&self) -> bool {
            self.cancelled.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    #[derive(Default)]
    struct Collect {
        events: Mutex<Vec<Value>>,
    }

    impl EventSink for Collect {
        fn emit(&self, event: Value) {
            self.events.lock().unwrap().push(event);
        }
    }

    fn lit(value: Value) -> Expr {
        Expr::Lit { value }
    }

    fn ref_to(path: &str) -> Expr {
        Expr::Ref {
            path: path.to_string(),
        }
    }

    fn fn_step(name: &str, args: Option<Expr>, save_as: Option<&str>, path: &str) -> Step {
        Step {
            kind: StepKind::Fn {
                name: name.to_string(),
                args,
                save_as: save_as.map(str::to_string),
            },
            path: path.to_string(),
            desc: String::new(),
        }
    }

    fn program(run: Vec<Step>) -> Program {
        Program {
            trace: Value::Null,
            vars: Default::default(),
            run,
            functions: Default::default(),
            start_index: 0,
        }
    }

    #[test]
    fn nested_error_events_keep_source_version_and_call_frame() {
        let program: Program = serde_json::from_value(serde_json::json!({
            "trace": { "run_id": "r1", "entry": {"path":"automations/main.yaml","version":"a"},
                "functions": {"失败函数":{"path":"automations/_function_extra.yaml","version":"b","function":"失败函数"}} },
            "run": [{"op":"fn","fn":"失败函数","path":"run[0]"}],
            "functions": {"失败函数":{"run":[{"op":"return","value":{"expr":"ref","path":"missing"},"path":"失败函数.run[0]"}]}}
        })).unwrap();
        let events = Collect::default();
        assert!(run(&program, &FakeHost::default(), Some(&events)).is_err());
        let events = events.events.lock().unwrap();
        let failures: Vec<_> = events
            .iter()
            .filter(|event| event["ev"] == "step_end" && event["ok"] == false)
            .collect();
        assert_eq!(failures[0]["trace"]["source"]["version"], "b");
        assert_eq!(failures[0]["trace"]["frame_id"], 1);
        assert_eq!(failures[0]["trace"]["parent_frame_id"], 0);
        assert_eq!(
            failures[1]["trace"]["source"]["path"],
            "automations/main.yaml"
        );
        assert_eq!(failures[1]["trace"]["frame_id"], 0);
    }

    #[test]
    fn call_name_reference_is_used_in_events_and_checked_at_runtime() {
        let mut program: Program = serde_json::from_value(serde_json::json!({
            "vars": {"label": "点击登录"},
            "run": [{"op":"fn", "fn":"tap", "path":"run[0]", "desc":"点击",
                "args":{"expr":"map", "value":{
                    "name":{"expr":"ref", "path":"label"},
                    "position":{"expr":"lit", "value":[0.5,0.8]}
                }}}]
        }))
        .unwrap();
        let events = Collect::default();
        let host = FakeHost::default();
        run(&program, &host, Some(&events)).unwrap();
        assert_eq!(events.events.lock().unwrap()[1]["desc"], "点击登录");
        assert_eq!(host.calls.lock().unwrap()[0].0, "tap");
        program.vars.insert("label".into(), Value::Bool(false));
        assert!(run(&program, &host, None)
            .unwrap_err()
            .contains("name 必须是字符串"));
    }

    #[test]
    fn host_functions_receive_args_and_save_results() {
        let host = FakeHost::with(
            "find",
            serde_json::json!({"center": {"x": 0.5, "y": 0.5}, "score": 0.9}),
        );
        let program = program(vec![
            fn_step(
                "find",
                Some(lit(serde_json::json!({"template": "home"}))),
                Some("home"),
                "run[0]",
            ),
            Step {
                kind: StepKind::Fn {
                    name: "tap".into(),
                    args: Some(ref_to("home.center")),
                    save_as: None,
                },
                path: "run[1]".into(),
                desc: String::new(),
            },
        ]);
        let value = run(&program, &host, None).unwrap();
        assert_eq!(value, Value::Null);
        let calls = host.calls.lock().unwrap();
        assert_eq!(calls[0].0, "find");
        assert_eq!(calls[1].0, "tap");
        assert_eq!(
            calls[1].1,
            serde_json::json!({"x": 0.5, "y": 0.5}),
            "字段引用取真实类型传递"
        );
    }

    #[test]
    fn unknown_function_surfaces_host_error_text() {
        let program = program(vec![fn_step("nope", None, None, "run[0]")]);
        struct Reject;
        impl HostFunctions for Reject {
            fn invoke(&self, name: &str, _args: Value) -> Result<Value, HostError> {
                Err(HostError::new(
                    HostErrorKind::NotFound,
                    format!("未知函数: {name}"),
                ))
            }
        }
        let error = run(&program, &Reject, None).unwrap_err();
        assert!(error.contains("kind=not-found"), "{error}");
    }

    #[test]
    fn if_takes_bool_and_null_only() {
        let host = FakeHost::default();
        let program = Program {
            trace: Value::Null,
            vars: serde_json::Map::from_iter([
                ("flag".into(), Value::Bool(true)),
                ("miss".into(), Value::Null),
            ]),
            run: vec![
                Step {
                    kind: StepKind::If {
                        cond: ref_to("flag"),
                        then_steps: vec![fn_step(
                            "log",
                            Some(lit(Value::String("yes".into()))),
                            None,
                            "run[0].then[0]",
                        )],
                        else_steps: vec![],
                    },
                    path: "run[0]".into(),
                    desc: String::new(),
                },
                Step {
                    kind: StepKind::If {
                        cond: ref_to("miss"),
                        then_steps: vec![fn_step(
                            "log",
                            Some(lit(Value::String("bad".into()))),
                            None,
                            "run[1].then[0]",
                        )],
                        else_steps: vec![fn_step(
                            "log",
                            Some(lit(Value::String("no".into()))),
                            None,
                            "run[1].else[0]",
                        )],
                    },
                    path: "run[1]".into(),
                    desc: String::new(),
                },
            ],
            functions: Default::default(),
            start_index: 0,
        };
        run(&program, &host, None).unwrap();
        let calls = host.calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "then 分支与 else 分支各执行一次 log");
        assert_eq!(calls[0].1, Value::String("yes".into()));
        assert_eq!(calls[1].1, Value::String("no".into()));
    }

    #[test]
    fn repeat_counts_iterations() {
        let host = FakeHost::default();
        let program = program(vec![Step {
            kind: StepKind::Repeat {
                times: lit(Value::from(3)),
                body: vec![fn_step(
                    "tap",
                    Some(lit(serde_json::json!([0.5, 0.5]))),
                    None,
                    "run[0].do[0]",
                )],
            },
            path: "run[0]".into(),
            desc: String::new(),
        }]);
        run(&program, &host, None).unwrap();
        assert_eq!(host.calls.lock().unwrap().len(), 3);
    }

    #[test]
    fn empty_repeat_body_is_bounded_by_step_budget() {
        let host = FakeHost::default();
        let big = program(vec![Step {
            kind: StepKind::Repeat {
                times: lit(Value::from(u64::MAX)),
                body: vec![],
            },
            path: "run[0]".into(),
            desc: String::new(),
        }]);
        let error = run(&big, &host, None).unwrap_err();
        assert!(error.starts_with("STEP_BUDGET_EXCEEDED"), "{error}");
    }

    #[test]
    fn repeat_rejects_non_integer_counts() {
        let host = FakeHost::default();
        let program = program(vec![Step {
            kind: StepKind::Repeat {
                times: lit(Value::from(1.5)),
                body: vec![],
            },
            path: "run[0]".into(),
            desc: String::new(),
        }]);
        let error = run(&program, &host, None).unwrap_err();
        assert!(error.contains("repeat"), "{error}");
    }

    #[test]
    fn package_functions_run_in_local_scope_with_defaults_and_required() {
        let host = FakeHost::default();
        let mut functions = BTreeMap::new();
        functions.insert(
            "claim".to_string(),
            serde_json::json!({
                "params": [
                    {"name": "timeout", "required": false, "default": "5s"},
                    {"name": "mode", "required": true}
                ],
                "vars": {"tag": "local"},
                "run": [
                    {"op":"fn","fn":"log","args":{"expr":"map","value":{
                        "message":{"expr":"ref","path":"tag"},
                        "extra":{"expr":"ref","path":"timeout"}}},
                     "path":"claim.run[0]","desc":""},
                    {"op":"return","value":{"expr":"lit","value":true},"path":"claim.run[1]","desc":""}
                ]
            }),
        );
        let program = Program {
            trace: Value::Null,
            vars: Default::default(),
            run: vec![
                fn_step(
                    "claim",
                    Some(lit(serde_json::json!({"mode": "daily"}))),
                    Some("ok"),
                    "run[0]",
                ),
                Step {
                    kind: StepKind::Return {
                        value: ref_to("ok"),
                    },
                    path: "run[1]".into(),
                    desc: String::new(),
                },
            ],
            functions,
            start_index: 0,
        };
        let value = run(&program, &host, None).unwrap();
        assert_eq!(value, Value::Bool(true), "return 值经 as 传给调用方");
        // call_start 事件 + 深度 1
        let events = Collect::default();
        let _ = run(&program, &host, Some(&events));
        let first = events.events.lock().unwrap();
        assert!(first.iter().any(|event| {
            event["ev"] == "call_start" && event["target"] == "claim" && event["depth"] == 1
        }));
    }

    #[test]
    fn missing_required_package_param_fails() {
        let host = FakeHost::default();
        let mut functions = BTreeMap::new();
        functions.insert(
            "need".to_string(),
            serde_json::json!({
                "params": [{"name": "who", "required": true}],
                "run": []
            }),
        );
        let program = Program {
            trace: Value::Null,
            vars: Default::default(),
            run: vec![fn_step(
                "need",
                Some(lit(serde_json::json!({}))),
                None,
                "run[0]",
            )],
            functions,
            start_index: 0,
        };
        let error = run(&program, &host, None).unwrap_err();
        assert!(error.contains("必填参数 who"), "{error}");
    }

    #[test]
    fn recursion_is_bounded_by_call_depth() {
        let host = FakeHost::default();
        let mut functions = BTreeMap::new();
        functions.insert(
            "loop".to_string(),
            serde_json::json!({
                "run": [
                    {"op":"fn","fn":"loop","path":"loop.run[0]","desc":""}
                ]
            }),
        );
        let program = Program {
            trace: Value::Null,
            vars: Default::default(),
            run: vec![fn_step("loop", None, None, "run[0]")],
            functions,
            start_index: 0,
        };
        let error = run(&program, &host, None).unwrap_err();
        assert!(error.starts_with("CALL_DEPTH_EXCEEDED"), "{error}");
    }

    #[test]
    fn undefined_variable_reference_fails_with_path() {
        let host = FakeHost::default();
        let program = program(vec![fn_step(
            "tap",
            Some(ref_to("home.center")),
            None,
            "run[0]",
        )]);
        let error = run(&program, &host, None).unwrap_err();
        assert!(error.contains("未定义变量 $home.center"), "{error}");
    }

    #[test]
    fn start_index_skips_top_level_steps_only() {
        let host = FakeHost::default();
        let mut program = program(vec![
            fn_step(
                "log",
                Some(lit(Value::String("first".into()))),
                None,
                "run[0]",
            ),
            Step {
                kind: StepKind::If {
                    cond: lit(Value::Bool(true)),
                    then_steps: vec![fn_step(
                        "log",
                        Some(lit(Value::String("skipped-branch".into()))),
                        None,
                        "run[1].then[0]",
                    )],
                    else_steps: vec![],
                },
                path: "run[1]".into(),
                desc: String::new(),
            },
            fn_step(
                "log",
                Some(lit(Value::String("third".into()))),
                None,
                "run[2]",
            ),
        ]);
        program.start_index = 2;
        run(&program, &host, None).unwrap();
        let calls = host.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, Value::String("third".into()));
    }

    #[test]
    fn cancel_polling_stops_before_next_step() {
        let host = FakeHost::with("tap", Value::Null);
        host.cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let program = program(vec![fn_step("tap", None, None, "run[0]")]);
        let error = run(&program, &host, None).unwrap_err();
        assert!(error.starts_with("CANCELLED"), "{error}");
    }

    #[test]
    fn events_cover_step_lifecycle_and_run_end_error() {
        let events = Collect::default();
        let host = FakeHost::default();
        let program = program(vec![fn_step("tap", None, None, "run[0]")]);
        run(&program, &host, Some(&events)).unwrap();
        let kinds: Vec<String> = events
            .events
            .lock()
            .unwrap()
            .iter()
            .map(|event| event["ev"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            kinds,
            vec![
                "run_start",
                "step_start",
                "detail",
                "detail",
                "step_end",
                "run_end"
            ]
        );

        struct Fail;
        impl HostFunctions for Fail {
            fn invoke(&self, _name: &str, _args: Value) -> Result<Value, HostError> {
                Err(HostError::new(HostErrorKind::Failed, "设备断开"))
            }
        }
        let events = Collect::default();
        let error = run(&program, &Fail, Some(&events)).unwrap_err();
        assert!(error.contains("设备断开"));
        let last = events.events.lock().unwrap().last().unwrap().clone();
        assert_eq!(last["ev"], "run_end");
        assert_eq!(last["ok"], false);
        assert_eq!(last["error"], error);
    }
}
