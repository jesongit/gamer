//! YAML v3 的纯数据前端：Surface YAML -> small AST。
//!
//! 这个模块故意不依赖任何设备实现或存储视图。它只负责把用户友好的 YAML 语法
//! 收敛成少量控制流节点和通用 capability invocation。执行在 `yaml_extension`
//! 中完成，因而 Core 不需要认识 YAML。

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value as YamlValue};

pub const YAML_V3: u64 = 3;

/// v3 内置 timing 兜底（契约 §1/§4）：顶层 `defaults.timing` 未声明时生效。
const DEFAULT_AFTER_TAP_MS: u64 = 300;
const DEFAULT_AFTER_MATCH_MS: u64 = 200;
/// find/check 轮询间隔缺省（原 DEFAULT_POLL_MS 常量路径正式化为可配置项）。
const DEFAULT_POLL_MS: u64 = 100;
/// find / find.verify 的 timeout 缺省：30min（对齐 v2，不再无限轮询）。
const DEFAULT_FIND_TIMEOUT_MS: u64 = 30 * 60_000;

/// splitmix64 —— wait 随机区间的 PRNG（方案 (a)：host 注入 run 级 nonce 进
/// program JSON，guest 内本地 PRNG，无 WIT 变更）。guest 解释器
/// `server/guests/yaml-guest/src/lib.rs` 有逐字拷贝；算法或常量改动必须两处
/// 同步（`splitmix64_test_vectors` 锁定测试向量）。
pub(crate) fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A user-facing loader/lowering diagnostic. Paths use a stable dotted shape so
/// the front-end can display the same diagnostic for raw and visual editors.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub path: String,
    pub message: String,
}

impl Diagnostic {
    fn new(code: &str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            path: path.into(),
            message: message.into(),
        }
    }
}

/// Values deliberately cover the values needed by automation and plugin
/// capabilities without becoming a general expression language.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Duration(u64),
    Color(String),
    Coordinate([f64; 2]),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
    /// Opaque handles are only used by the dynamic Host wire. The id is never
    /// a pointer or a host path; it is scoped to the current guest instance.
    Handle {
        kind: String,
        id: u64,
    },
}

impl Value {
    pub fn truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(value) => *value,
            Self::Int(value) => *value != 0,
            Self::Float(value) => *value != 0.0,
            Self::String(value) | Self::Color(value) => !value.is_empty(),
            Self::Duration(value) => *value != 0,
            Self::Coordinate(_) | Self::Handle { .. } => true,
            Self::List(value) => !value.is_empty(),
            Self::Map(value) => value
                .get("found")
                .map(Self::truthy)
                .unwrap_or(!value.is_empty()),
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(value) | Self::Color(value) => Some(value),
            _ => None,
        }
    }

    pub fn duration_ms(&self) -> Option<u64> {
        match self {
            Self::Duration(value) => Some(*value),
            Self::Int(value) if *value >= 0 => Some(*value as u64),
            _ => None,
        }
    }

    /// JSON 出口（roundtrip 测试消费；生产 WASM 边界直接 serde 序列化）。
    #[allow(dead_code)]
    pub fn into_json(self) -> serde_json::Value {
        serde_json::to_value(self).expect("yaml vnext values are JSON representable")
    }

    /// Accept both the typed wire format emitted by `Value` and ordinary JSON
    /// values supplied by a third-party capability guest.
    /// 消费方在 wasm-runtime 侧（guest 输入/输出 JSON 边界）。
    #[cfg_attr(not(feature = "wasm-runtime"), allow(dead_code))]
    pub fn from_json(value: serde_json::Value) -> Result<Self, String> {
        match serde_json::from_value::<Self>(value.clone()) {
            Ok(value) => Ok(value),
            Err(_) => Self::from_plain_json(value),
        }
    }

    fn from_plain_json(value: serde_json::Value) -> Result<Self, String> {
        Ok(match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(value) => Self::Bool(value),
            serde_json::Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    Self::Int(value)
                } else {
                    Self::Float(value.as_f64().ok_or("JSON 数字不是有限浮点数")?)
                }
            }
            serde_json::Value::String(value) => Self::String(value),
            // 数组元素优先按 typed wire 形态还原（guest 小 AST 解释器的容器
            // 不加 wrap、元素是 typed 形态，见 yaml-guest evaluate）。
            serde_json::Value::Array(values) => Self::List(
                values
                    .into_iter()
                    .map(Self::from_json)
                    .collect::<Result<_, _>>()?,
            ),
            serde_json::Value::Object(values) => Self::Map(
                values
                    .into_iter()
                    .map(|(key, value)| Ok((key, Self::from_json(value)?)))
                    .collect::<Result<_, String>>()?,
            ),
        })
    }
}

/// A deliberately small expression form: literal, variable/path reference,
/// or a value embedded in a collection/map. No arithmetic or user functions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "expr", content = "value", rename_all = "snake_case")]
pub enum Expr {
    Literal(Value),
    Ref(String),
    List(Vec<Expr>),
    Map(BTreeMap<String, Expr>),
}

impl Expr {
    pub fn reference(name: impl Into<String>) -> Self {
        Self::Ref(name.into())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "condition", rename_all = "snake_case")]
pub enum Condition {
    Truthy { value: Expr },
    Equals { left: Expr, right: Expr },
    Not { value: Box<Condition> },
}

impl Condition {
    fn truthy(value: Expr) -> Self {
        Self::Truthy { value }
    }
}

/// surface step 的运行身份（P12.6 / ADR-YAML-03）：lower 期为每个 surface step
/// 生成稳定 path（`steps[0].then[1]`，与前端编辑器 commands 寻址同语法）与中文
/// desc（kind + 关键参数摘要）。挂在被标注步产出的 [`SmallStep::Step`] 包装上，
/// guest / 原生解释器据此发 `step_start` / `step_end` 事件；lower 展开物
/// （timing sleep、find/check 轮询体）不带 label，天然静默。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepLabel {
    pub path: String,
    pub desc: String,
}

/// The only nodes allowed after lowering. Actions are represented by the
/// generic `invoke` node; this is the important boundary that keeps YAML
/// policy out of Core capabilities.
///
/// [`SmallStep::Step`] 是 P12.6 的运行身份包装：不改变预算语义（包装步就是原
/// 逻辑步，不额外计数），仅携带 [`StepLabel`] 供解释器发事件。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SmallStep {
    Step {
        label: StepLabel,
        step: Box<SmallStep>,
    },
    Invoke {
        capability: String,
        args: BTreeMap<String, Expr>,
        save: Option<String>,
    },
    If {
        cond: Condition,
        then_steps: Vec<SmallStep>,
        else_steps: Vec<SmallStep>,
    },
    Loop {
        /// `None` means infinite; the body must contain a terminating branch
        /// or cancellation is the only exit.
        times: Option<Expr>,
        body: Vec<SmallStep>,
    },
    Break,
    Call {
        target: String,
        args: BTreeMap<String, Expr>,
        save: Option<String>,
    },
    Return {
        value: Expr,
    },
    Throw {
        message: Expr,
    },
    Set {
        name: String,
        value: Expr,
    },
    /// `- wait: {min, max}` 随机区间等待（契约 §4）。时长由解释器以 run nonce
    /// 播种的 splitmix64 在 [min, max] 内取值后调 `runtime.sleep`（取消可达）。
    WaitRandom {
        min: Expr,
        max: Expr,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamDecl {
    pub name: String,
    pub ty: String,
    /// 参数备注：字符串形态第 3 段 / 映射形态 `remark` 键（透出到参数 schema
    /// description；不参与 psig1 签名）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remark: Option<String>,
    pub default: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub version: u64,
    pub params: Vec<ParamDecl>,
    pub steps: Vec<SmallStep>,
    /// 运行级随机 nonce（wait 随机区间的 PRNG 种子，方案 (a)）。lower 产出
    /// None；生产 WASM 链路由 wasm_host 注入每 run 随机值，原生参考解释器
    /// 直接消费该字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<u64>,
}

/// 顶层 `defaults:`（契约 §1/§4）：vision 阈值兜底与 timing 兜底。解析期校验、
/// lower 期消费——threshold 三级优先（step > defaults > Runtime 内置 0.80）在
/// lower 期解析注入 invoke args；timing 展开为显式 `runtime.sleep` invoke
/// （tap 后 after_tap、find/check/match_first 命中后 after_match、轮询
/// poll_interval），取消语义复用 runtime.sleep 既有取消路径。
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SurfaceDefaults {
    pub vision_threshold: Option<f64>,
    pub after_tap_ms: Option<u64>,
    pub after_match_ms: Option<u64>,
    pub poll_interval_ms: Option<u64>,
}

/// Parsed but not lowered surface syntax. Keeping this type visible makes the
/// two-phase contract testable and lets the editor show the original feature.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceProgram {
    pub version: u64,
    pub params: Vec<ParamDecl>,
    pub defaults: SurfaceDefaults,
    pub steps: Vec<SurfaceStep>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceStep {
    Tap {
        at: Expr,
    },
    Swipe {
        from: Expr,
        to: Expr,
        duration: Expr,
    },
    Key {
        key: Expr,
        action: String,
    },
    Text {
        value: Expr,
    },
    /// `- wait: 300ms` 固定等待；`- wait: {min, max}` 随机区间（lower 成
    /// [`SmallStep::WaitRandom`]）。min/max 必须同给且 min ≤ max。
    Wait {
        duration: Expr,
        max: Option<Expr>,
    },
    If {
        cond: Expr,
        then_steps: Vec<SurfaceStep>,
        else_steps: Vec<SurfaceStep>,
    },
    Loop {
        times: Option<Expr>,
        steps: Vec<SurfaceStep>,
    },
    Break,
    Call {
        target: String,
        args: BTreeMap<String, Expr>,
        save: Option<String>,
    },
    Return {
        value: Expr,
    },
    Throw {
        message: Expr,
    },
    Set {
        name: String,
        value: Expr,
    },
    Invoke {
        capability: String,
        args: BTreeMap<String, Expr>,
        save: Option<String>,
    },
    Log {
        level: String,
        message: Expr,
    },
    AppStart {
        package: Option<Expr>,
    },
    AppStop {
        package: Option<Expr>,
    },
    /// find（ADR-YAML-03 / 契约 §3）：轮询模板至命中 → save/`$match` 固化 →
    /// sleep(after_match) → then → verify；超时走 else、无 else 抛
    /// `FIND_TIMEOUT: <template>`。timeout 缺省 30min。click 字段已删除。
    Find {
        template: Expr,
        timeout: Option<Expr>,
        threshold: Option<f64>,
        region: Option<Expr>,
        save: Option<String>,
        then_steps: Vec<SurfaceStep>,
        else_steps: Vec<SurfaceStep>,
        verify: Option<FindVerifySurface>,
    },
    Check {
        template: Expr,
        timeout: Option<Expr>,
        threshold: Option<f64>,
        message: Option<Expr>,
    },
    MatchFirst {
        candidates: Vec<MatchCandidateSurface>,
        else_steps: Vec<SurfaceStep>,
    },
}

/// `find.verify`（ADR-YAML-03）：then 执行完后在 timeout 内二次验证模板，
/// 不命中抛 `VERIFY_FAILED: <template>`（不走 else——verify 语义是确认操作
/// 生效，静默降级会掩盖异常）。
#[derive(Clone, Debug, PartialEq)]
pub struct FindVerifySurface {
    pub template: Expr,
    pub timeout: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatchCandidateSurface {
    pub template: Expr,
    /// 候选级 threshold override（缺省回落 defaults.vision.threshold → 内置）。
    pub threshold: Option<f64>,
    pub steps: Vec<SurfaceStep>,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}: {}", self.code, self.path, self.message)
    }
}

pub fn load(source: &str) -> Result<Program, Vec<Diagnostic>> {
    let surface = parse_surface(source)?;
    lower(&surface)
}

/// `call` 目标的显式命名空间（契约 §2 / ADR-YAML-02）。
#[derive(Clone, Debug, PartialEq)]
pub enum CallTarget {
    /// `script:<资源 id>`：分区内 `scripts/` 相对路径，`.yaml` 后缀可省略。
    Script(String),
    /// `function:<文件短路径>/<函数名>`：文件短路径按最后一个 `/` 分割、可含目录。
    Function {
        file: String,
        function: String,
    },
}

const CALL_NAMESPACE_HINT: &str =
    "call target 必须带命名空间前缀：script:<脚本id> 或 function:<文件短路径>/<函数名>（如 script:daily/login、function:工具/月卡领取）";

/// 解析 `call` target 命名空间并做穿越校验。
///
/// 裸 target 与未知前缀在解析期拒绝（错误码 `yaml.v3.call.namespace`，错误信息
/// 含 target 原文与合法形态示例）；`function:` 路径形态错误为
/// `yaml.v3.call.function_path`，穿越（`..`/绝对路径/反斜杠/空段）为
/// `yaml.v3.call.target`。语法同 v2 `split_func_path` 的穿越校验并推广到
/// `script:` id。
pub fn split_call_target(target: &str) -> Result<CallTarget, Vec<Diagnostic>> {
    let trimmed = target.trim();
    if let Some(rest) = trimmed.strip_prefix("script:") {
        let id = rest.trim();
        if id.is_empty() {
            return Err(vec![Diagnostic::new(
                "yaml.v3.call.namespace",
                "target",
                format!("script: 后缺少脚本资源 id；{CALL_NAMESPACE_HINT}（原文 {target:?}）"),
            )]);
        }
        reject_resource_traversal(id)?;
        Ok(CallTarget::Script(id.to_string()))
    } else if let Some(rest) = trimmed.strip_prefix("function:") {
        let rest = rest.trim();
        reject_resource_traversal(rest)?;
        let Some((file, function)) = rest.rsplit_once('/') else {
            return Err(vec![Diagnostic::new(
                "yaml.v3.call.function_path",
                "target",
                format!(
                    "function: 目标 {target:?} 必须是 <文件短路径>/<函数名>（如 function:工具/月卡领取）"
                ),
            )]);
        };
        if file.is_empty() || function.is_empty() {
            return Err(vec![Diagnostic::new(
                "yaml.v3.call.function_path",
                "target",
                format!("function: 目标 {target:?} 的文件短路径与函数名均不能为空"),
            )]);
        }
        Ok(CallTarget::Function {
            file: file.to_string(),
            function: function.to_string(),
        })
    } else {
        Err(vec![Diagnostic::new(
            "yaml.v3.call.namespace",
            "target",
            format!("裸 call target {target:?} 不再接受；{CALL_NAMESPACE_HINT}"),
        )])
    }
}

/// 资源路径穿越校验：拒绝反斜杠、绝对路径、空段与 `..` 段。
fn reject_resource_traversal(path: &str) -> Result<(), Vec<Diagnostic>> {
    if path.contains('\\')
        || path.starts_with('/')
        || path.split('/').any(|segment| segment.is_empty() || segment == "..")
    {
        return Err(vec![Diagnostic::new(
            "yaml.v3.call.target",
            "target",
            format!("call 资源路径 {path:?} 含 ..、绝对路径、反斜杠或空段"),
        )]);
    }
    Ok(())
}

/// v3 函数库（functions/ 资源）：bare-map `{<函数名>: {params, steps}}`。
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionDecl {
    pub name: String,
    pub params: Vec<ParamDecl>,
    pub steps: Vec<SurfaceStep>,
}

/// v3 函数名保留字（动作键 / 结构键 / `$match` 上下文变量名）。与 v2
/// `RESERVED_FUNCTION_NAMES` 同口径并补 v3 新增键——函数经
/// `function:<文件短路径>/<函数名>` 调用，名字与步骤键或上下文变量重叠会
/// 造成不可读的遮蔽。
const RESERVED_FUNCTION_NAMES: &[&str] = &[
    "log", "key", "text", "tap", "swipe", "find", "match", "match_first", "check", "color",
    "loop", "break", "call", "throw", "set", "invoke", "str_app", "cls_app", "wait", "return",
    "then", "else", "steps", "times", "block", "verify", "timeout", "config", "func", "params",
    "args", "expect", "candidates", "click", "if", "until", "version", "defaults",
];

/// 函数名规则：unicode 字母/下划线开头，后续字母/数字/下划线（支持中文），
/// 不以数字开头——与 v2 `valid_function_name` 同口径。
fn valid_function_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {
            chars.all(|c| c.is_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

/// 解析 v3 函数库文件：顶层键全部是函数名（无 `version` 键，目录即类型），
/// 每个函数记录只允许 `params` / `steps`，`steps` 必需。函数名由映射键承载
/// （唯一；字符串标量、合法字符集、非保留字），结构非法给
/// `yaml.v3.function.*` 结构化诊断。
pub fn parse_function_library(source: &str) -> Result<Vec<FunctionDecl>, Vec<Diagnostic>> {
    let root: YamlValue = serde_yaml::from_str(source)
        .map_err(|error| vec![Diagnostic::new("yaml.v3.syntax", "", error.to_string())])?;
    let map = as_map(&root, "", "函数库必须是 {函数名: {params, steps}} 映射")?;
    if map.is_empty() {
        return Err(vec![Diagnostic::new(
            "yaml.v3.function.file",
            "",
            "函数库必须至少声明一个函数",
        )]);
    }
    let mut functions = Vec::with_capacity(map.len());
    for (key, value) in map {
        let Some(name) = key.as_str().map(str::trim).filter(|name| !name.is_empty()) else {
            return Err(vec![Diagnostic::new(
                "yaml.v3.function.name",
                "",
                "顶层键不是字符串标量，不能作为函数名",
            )]);
        };
        if !valid_function_name(name) {
            return Err(vec![Diagnostic::new(
                "yaml.v3.function.name",
                name,
                format!("函数名 {name} 只允许 unicode 字母/数字/下划线（支持中文），且不能以数字开头"),
            )]);
        }
        let def = as_map(value, name, "函数定义必须是映射")?;
        // 记录形状合法后才做保留字裁决：`version: 3` 这类 v2 脚本误存
        // functions/ 的文件优先报「定义必须是映射」（双形态保存边界的 v2
        // 回落口径依赖该形状诊断，见 resources::validate_function_library_file）。
        if RESERVED_FUNCTION_NAMES.contains(&name) {
            return Err(vec![Diagnostic::new(
                "yaml.v3.function.name",
                name,
                format!("函数名 {name} 是保留字（动作键 / 结构键）"),
            )]);
        }
        let name = name.to_string();
        for key in def.keys() {
            let key = key.as_str().unwrap_or_default();
            if !matches!(key, "params" | "steps") {
                return Err(vec![Diagnostic::new(
                    "yaml.v3.function.unknown_key",
                    format!("{name}.{key}"),
                    format!("不支持函数字段 {key:?}"),
                )]);
            }
        }
        let params = def
            .get("params")
            .map(parse_params)
            .transpose()?
            .unwrap_or_default();
        let steps = required_steps(def, "steps", &name)?;
        functions.push(FunctionDecl {
            name,
            params,
            steps,
        });
    }
    Ok(functions)
}

/// 从函数库源码装载指定函数并 lower 成可执行 Program（version 固定 3）。
/// 函数库无 defaults 块（bare-map 结构），timing/threshold 走内置兜底。
pub fn load_function(source: &str, function: &str) -> Result<Program, Vec<Diagnostic>> {
    let library = parse_function_library(source)?;
    let decl = library
        .iter()
        .find(|decl| decl.name == function)
        .ok_or_else(|| {
            vec![Diagnostic::new(
                "yaml.v3.function.not_found",
                function,
                format!("函数库中不存在函数 {function:?}"),
            )]
        })?;
    let mut lowerer = Lowerer::with_defaults(&SurfaceDefaults::default());
    Ok(Program {
        version: YAML_V3,
        params: decl.params.clone(),
        steps: lowerer.steps(&decl.steps, "steps")?,
        nonce: None,
    })
}

/// Identify a v3 source before invoking the legacy loader. A syntactically
/// valid `version: 3` document is v3 even when its later fields are invalid,
/// which keeps diagnostics on the correct compatibility path.
pub(crate) fn is_v3_source(source: &str) -> bool {
    serde_yaml::from_str::<YamlValue>(source)
        .ok()
        .and_then(|value| value.get("version").and_then(YamlValue::as_u64))
        == Some(YAML_V3)
}

pub fn parse_surface(source: &str) -> Result<SurfaceProgram, Vec<Diagnostic>> {
    let root: YamlValue = serde_yaml::from_str(source)
        .map_err(|error| vec![Diagnostic::new("yaml.v3.syntax", "", error.to_string())])?;
    let map = as_map(&root, "", "脚本顶层必须是映射")?;
    let version = match map.get("version") {
        Some(YamlValue::Number(value)) => value.as_u64().ok_or_else(|| {
            vec![Diagnostic::new(
                "yaml.v3.version",
                "version",
                "version 必须是整数 3",
            )]
        })?,
        Some(_) => {
            return Err(vec![Diagnostic::new(
                "yaml.v3.version",
                "version",
                "version 必须是整数 3",
            )])
        }
        None => {
            return Err(vec![Diagnostic::new(
                "yaml.v3.version.missing",
                "version",
                "v3 脚本必须声明 version: 3",
            )])
        }
    };
    if version != YAML_V3 {
        return Err(vec![Diagnostic::new(
            "yaml.v3.version",
            "version",
            "当前只支持 version: 3",
        )]);
    }
    for key in map.keys() {
        let key = key.as_str().unwrap_or_default();
        if !matches!(key, "version" | "params" | "defaults" | "steps") {
            return Err(vec![Diagnostic::new(
                "yaml.v3.top_level.unknown_key",
                key,
                format!("不支持顶层字段 {key:?}（只允许 version/params/defaults/steps）"),
            )]);
        }
    }
    let params = map
        .get("params")
        .map(parse_params)
        .transpose()?
        .unwrap_or_default();
    let defaults = match map.get("defaults") {
        Some(value) => parse_defaults(value)?,
        None => SurfaceDefaults::default(),
    };
    let steps = match map.get("steps") {
        Some(value) => parse_steps(value, "steps")?,
        None => {
            return Err(vec![Diagnostic::new(
                "yaml.v3.steps.missing",
                "steps",
                "脚本必须包含 steps",
            )])
        }
    };
    Ok(SurfaceProgram {
        version,
        params,
        defaults,
        steps,
    })
}

/// 顶层 `defaults:`（契约 §1/§4）：只允许 `vision.threshold` 与
/// `timing.{after_tap,after_match,poll_interval}`，未知键/类型/区间报
/// `yaml.v3.defaults.*` 结构化诊断。
fn parse_defaults(value: &YamlValue) -> Result<SurfaceDefaults, Vec<Diagnostic>> {
    let map = as_map(value, "defaults", "defaults 必须是映射（vision/timing）")?;
    let mut out = SurfaceDefaults::default();
    for key in map.keys() {
        let key = key.as_str().unwrap_or_default();
        let value = map.get(key).expect("map key has value");
        match key {
            "vision" => {
                let vision = as_map(
                    value,
                    "defaults.vision",
                    "defaults.vision 必须是映射（threshold）",
                )?;
                for vkey in vision.keys() {
                    let vkey = vkey.as_str().unwrap_or_default();
                    if vkey != "threshold" {
                        return Err(vec![Diagnostic::new(
                            "yaml.v3.defaults.unknown_key",
                            format!("defaults.vision.{vkey}"),
                            format!("defaults.vision 不支持字段 {vkey:?}（只允许 threshold）"),
                        )]);
                    }
                    out.vision_threshold = Some(parse_threshold_number(
                        vision.get(vkey).expect("map key has value"),
                        "defaults.vision.threshold",
                    )?);
                }
            }
            "timing" => {
                let timing = as_map(
                    value,
                    "defaults.timing",
                    "defaults.timing 必须是映射（after_tap/after_match/poll_interval）",
                )?;
                for tkey in timing.keys() {
                    let tkey = tkey.as_str().unwrap_or_default();
                    let path = format!("defaults.timing.{tkey}");
                    if !matches!(tkey, "after_tap" | "after_match" | "poll_interval") {
                        return Err(vec![Diagnostic::new(
                            "yaml.v3.defaults.unknown_key",
                            path.clone(),
                            format!(
                                "defaults.timing 不支持字段 {tkey:?}（只允许 after_tap/after_match/poll_interval）"
                            ),
                        )]);
                    }
                    let ms = duration_ms_literal(
                        timing.get(tkey).expect("map key has value"),
                        &path,
                    )?;
                    match tkey {
                        "after_tap" => out.after_tap_ms = Some(ms),
                        "after_match" => out.after_match_ms = Some(ms),
                        _ => out.poll_interval_ms = Some(ms),
                    }
                }
            }
            other => {
                return Err(vec![Diagnostic::new(
                    "yaml.v3.defaults.unknown_key",
                    format!("defaults.{other}"),
                    format!("defaults 不支持字段 {other:?}，只允许 vision/timing"),
                )]);
            }
        }
    }
    Ok(out)
}

/// defaults.timing 值：只能是字面时长（`300ms`/`2s` 或非负整数毫秒）——
/// timing 兜底在 lower 期展开为显式 sleep，不接受 `$var` 引用。
fn duration_ms_literal(value: &YamlValue, path: &str) -> Result<u64, Vec<Diagnostic>> {
    if let Some(raw) = value.as_str() {
        return parse_duration_ms(raw).ok_or_else(|| {
            vec![Diagnostic::new(
                "yaml.v3.defaults.type",
                path,
                "timing 项必须是字面时长（如 300ms/2s）",
            )]
        });
    }
    if let Some(value) = value.as_u64() {
        return Ok(value);
    }
    Err(vec![Diagnostic::new(
        "yaml.v3.defaults.type",
        path,
        "timing 项必须是带单位时间串或非负整数毫秒",
    )])
}

/// threshold 数值：0.0~1.0 的有限数（与前端编辑器同口径）。
fn parse_threshold_number(value: &YamlValue, path: &str) -> Result<f64, Vec<Diagnostic>> {
    let raw = value.as_f64().ok_or_else(|| {
        vec![Diagnostic::new(
            "yaml.v3.defaults.type",
            path,
            "threshold 必须是 0~1 的数字",
        )]
    })?;
    if !(0.0..=1.0).contains(&raw) {
        return Err(vec![Diagnostic::new(
            "yaml.v3.defaults.range",
            path,
            "threshold 必须在 0~1 之间",
        )]);
    }
    Ok(raw)
}

fn parse_params(value: &YamlValue) -> Result<Vec<ParamDecl>, Vec<Diagnostic>> {
    let items = match value {
        YamlValue::Sequence(items) => items,
        _ => {
            return Err(vec![Diagnostic::new(
                "yaml.v3.params.type",
                "params",
                "params 必须是列表",
            )])
        }
    };
    let mut result = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let path = format!("params[{index}]");
        match item {
            YamlValue::String(raw) => {
                let mut parts = raw.splitn(4, ':');
                let ty = parts.next().unwrap_or_default().trim();
                let name = parts.next().unwrap_or_default().trim();
                if ty.is_empty() || name.is_empty() {
                    return Err(vec![Diagnostic::new(
                        "yaml.v3.params.invalid",
                        path,
                        "参数必须是 type:name[:remark[:default]]",
                    )]);
                }
                let remark = parts
                    .next()
                    .map(str::trim)
                    .filter(|remark| !remark.is_empty())
                    .map(str::to_string);
                let default = parts.next().map(|value| Value::String(value.to_string()));
                result.push(ParamDecl {
                    name: name.to_string(),
                    ty: ty.to_string(),
                    remark,
                    default,
                });
            }
            YamlValue::Mapping(map) => {
                let name = required_string(map, "name", &path)?;
                let ty = map
                    .get("type")
                    .and_then(YamlValue::as_str)
                    .unwrap_or("value")
                    .to_string();
                let default = map
                    .get("default")
                    .map(|value| value_from_yaml(value, &format!("{path}.default")))
                    .transpose()?;
                for key in map.keys() {
                    let key = key.as_str().unwrap_or_default();
                    if !matches!(key, "name" | "type" | "default" | "remark") {
                        return Err(vec![Diagnostic::new(
                            "yaml.v3.params.unknown_key",
                            format!("{path}.{key}"),
                            "不支持参数字段",
                        )]);
                    }
                }
                let remark = map
                    .get("remark")
                    .and_then(YamlValue::as_str)
                    .map(str::trim)
                    .filter(|remark| !remark.is_empty())
                    .map(str::to_string);
                result.push(ParamDecl {
                    name,
                    ty,
                    remark,
                    default,
                });
            }
            _ => {
                return Err(vec![Diagnostic::new(
                    "yaml.v3.params.invalid",
                    path,
                    "参数声明必须是字符串或映射",
                )])
            }
        }
    }
    Ok(result)
}

fn parse_steps(value: &YamlValue, path: &str) -> Result<Vec<SurfaceStep>, Vec<Diagnostic>> {
    let items = match value {
        YamlValue::Sequence(items) => items,
        _ => {
            return Err(vec![Diagnostic::new(
                "yaml.v3.steps.type",
                path,
                "steps 必须是列表",
            )])
        }
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| parse_step(item, &format!("{path}[{index}]")))
        .collect()
}

fn parse_step(value: &YamlValue, path: &str) -> Result<SurfaceStep, Vec<Diagnostic>> {
    // Bare control/lifecycle actions are useful in compact YAML (`- break`,
    // `- app.start`) and have no payload. Other actions still require the
    // normal single-key mapping shape.
    if let Some(action) = value.as_str() {
        return match action {
            "break" => Ok(SurfaceStep::Break),
            "app.start" => Ok(SurfaceStep::AppStart { package: None }),
            "app.stop" => Ok(SurfaceStep::AppStop { package: None }),
            _ => Err(vec![Diagnostic::new(
                "yaml.v3.step.shape",
                path,
                "步骤必须是单键映射",
            )]),
        };
    }
    let map = as_map(value, path, "步骤必须是单键映射")?;
    if map.len() != 1 {
        return Err(vec![Diagnostic::new(
            "yaml.v3.step.shape",
            path,
            "每个步骤必须恰好包含一个动作键",
        )]);
    }
    let (action, value) = map.iter().next().expect("non-empty step");
    let action = action.as_str().unwrap_or_default();
    let value_path = format!("{path}.{action}");
    match action {
        "tap" => Ok(SurfaceStep::Tap {
            at: point_expr(value, &value_path)?,
        }),
        "swipe" => {
            let map = as_map(value, &value_path, "swipe 必须是映射")?;
            reject_unknown(map, &["from", "to", "duration", "time"], &value_path)?;
            Ok(SurfaceStep::Swipe {
                from: field_expr(map, "from", &value_path)?,
                to: field_expr(map, "to", &value_path)?,
                duration: duration_expr(
                    map.get("duration")
                        .or_else(|| map.get("time"))
                        .ok_or_else(|| {
                            vec![Diagnostic::new(
                                "yaml.v3.field.missing",
                                format!("{value_path}.duration"),
                                "swipe 缺少 duration",
                            )]
                        })?,
                    &format!("{value_path}.duration"),
                )?,
            })
        }
        "key" => {
            let (key, action) = match value {
                YamlValue::Mapping(map) => {
                    reject_unknown(map, &["key", "action"], &value_path)?;
                    (
                        field_expr(map, "key", &value_path)?,
                        map.get("action")
                            .and_then(YamlValue::as_str)
                            .unwrap_or("press")
                            .to_string(),
                    )
                }
                _ => (expr_from_yaml(value, &value_path)?, "press".to_string()),
            };
            if !matches!(action.as_str(), "down" | "up" | "press") {
                return Err(vec![Diagnostic::new(
                    "yaml.v3.key.action",
                    format!("{value_path}.action"),
                    "action 只能是 down/up/press",
                )]);
            }
            Ok(SurfaceStep::Key { key, action })
        }
        "text" => Ok(SurfaceStep::Text {
            value: map_or_value_expr(value, "value", &value_path)?,
        }),
        "wait" => {
            // 标量/`duration` 键 = 固定等待；`min`/`max` = 随机区间（契约 §4，
            // 两者必须同给且 min ≤ max，与前端编辑器同口径）。
            if let Some(sub) = value.as_mapping() {
                if sub.contains_key("min") || sub.contains_key("max") {
                    reject_unknown(sub, &["min", "max"], &value_path)?;
                    let min = duration_expr(
                        sub.get("min").ok_or_else(|| {
                            vec![Diagnostic::new(
                                "yaml.v3.field.missing",
                                format!("{value_path}.min"),
                                "wait 随机区间需要 min/max 同给",
                            )]
                        })?,
                        &format!("{value_path}.min"),
                    )?;
                    let max = duration_expr(
                        sub.get("max").ok_or_else(|| {
                            vec![Diagnostic::new(
                                "yaml.v3.field.missing",
                                format!("{value_path}.max"),
                                "wait 随机区间需要 min/max 同给",
                            )]
                        })?,
                        &format!("{value_path}.max"),
                    )?;
                    check_wait_range(&min, &max, &value_path)?;
                    return Ok(SurfaceStep::Wait {
                        duration: min,
                        max: Some(max),
                    });
                }
                reject_unknown(sub, &["duration", "time"], &value_path)?;
            }
            Ok(SurfaceStep::Wait {
                duration: duration_expr(
                    map_value(value, "duration", "time", &value_path)?,
                    &value_path,
                )?,
                max: None,
            })
        }
        "if" => {
            let map = as_map(value, &value_path, "if 必须是映射")?;
            reject_unknown(map, &["cond", "then", "else"], &value_path)?;
            Ok(SurfaceStep::If {
                cond: field_expr(map, "cond", &value_path)?,
                then_steps: parse_optional_steps(map.get("then"), &format!("{value_path}.then"))?,
                else_steps: parse_optional_steps(map.get("else"), &format!("{value_path}.else"))?,
            })
        }
        "loop" => {
            let map = as_map(value, &value_path, "loop 必须是映射")?;
            reject_unknown(map, &["times", "steps"], &value_path)?;
            let times = map
                .get("times")
                .map(|value| expr_from_yaml(value, &format!("{value_path}.times")))
                .transpose()?;
            let steps = required_steps(map, "steps", &value_path)?;
            Ok(SurfaceStep::Loop { times, steps })
        }
        "break" => Ok(SurfaceStep::Break),
        "call" => {
            let map = as_map(value, &value_path, "call 必须是映射")?;
            reject_unknown(map, &["target", "args", "with", "save"], &value_path)?;
            let target = required_string(map, "target", &value_path)?;
            split_call_target(&target).map_err(|mut diagnostics| {
                for diagnostic in &mut diagnostics {
                    diagnostic.path = format!("{value_path}.target");
                }
                diagnostics
            })?;
            Ok(SurfaceStep::Call {
                target,
                args: args_map(map, &value_path)?,
                save: optional_string(map, "save", &value_path)?,
            })
        }
        "return" => Ok(SurfaceStep::Return {
            value: expr_from_yaml(value, &value_path)?,
        }),
        "throw" => Ok(SurfaceStep::Throw {
            message: map_or_value_expr(value, "message", &value_path)?,
        }),
        "set" => {
            let map = as_map(value, &value_path, "set 必须是映射")?;
            if map.contains_key("name") {
                reject_unknown(map, &["name", "value"], &value_path)?;
                Ok(SurfaceStep::Set {
                    name: required_string(map, "name", &value_path)?,
                    value: field_expr(map, "value", &value_path)?,
                })
            } else if map.len() == 1 {
                let (name, value) = map.iter().next().expect("one set entry");
                Ok(SurfaceStep::Set {
                    name: name.as_str().unwrap_or_default().to_string(),
                    value: expr_from_yaml(value, &value_path)?,
                })
            } else {
                Err(vec![Diagnostic::new(
                    "yaml.v3.set.shape",
                    &value_path,
                    "set 使用 {name, value} 或单键映射",
                )])
            }
        }
        "invoke" => {
            let map = as_map(value, &value_path, "invoke 必须是映射")?;
            reject_unknown(map, &["capability", "with", "args", "save"], &value_path)?;
            Ok(SurfaceStep::Invoke {
                capability: required_string(map, "capability", &value_path)?,
                args: args_map(map, &value_path)?,
                save: optional_string(map, "save", &value_path)?,
            })
        }
        "log" => {
            let (level, message) = match value {
                YamlValue::Mapping(map) => {
                    reject_unknown(map, &["level", "message"], &value_path)?;
                    (
                        map.get("level")
                            .and_then(YamlValue::as_str)
                            .unwrap_or("info")
                            .to_string(),
                        field_expr(map, "message", &value_path)?,
                    )
                }
                _ => ("info".to_string(), expr_from_yaml(value, &value_path)?),
            };
            Ok(SurfaceStep::Log { level, message })
        }
        "app.start" | "app.stop" => {
            let package = match value {
                YamlValue::Null => None,
                YamlValue::Mapping(map) => {
                    reject_unknown(map, &["package", "app"], &value_path)?;
                    map.get("package")
                        .or_else(|| map.get("app"))
                        .map(|value| expr_from_yaml(value, &value_path))
                        .transpose()?
                }
                _ => Some(expr_from_yaml(value, &value_path)?),
            };
            if action == "app.start" {
                Ok(SurfaceStep::AppStart { package })
            } else {
                Ok(SurfaceStep::AppStop { package })
            }
        }
        "find" => parse_find(value, &value_path),
        "check" => {
            let map = as_map(value, &value_path, "check 必须是映射")?;
            reject_unknown(map, &["template", "timeout", "threshold", "throw"], &value_path)?;
            Ok(SurfaceStep::Check {
                template: field_expr(map, "template", &value_path)?,
                timeout: map
                    .get("timeout")
                    .map(|v| duration_expr(v, &format!("{value_path}.timeout")))
                    .transpose()?,
                threshold: map
                    .get("threshold")
                    .map(|v| parse_threshold_number(v, &format!("{value_path}.threshold")))
                    .transpose()?,
                message: map
                    .get("throw")
                    .map(|v| expr_from_yaml(v, &format!("{value_path}.throw")))
                    .transpose()?,
            })
        }
        "retry" | "wait_for" | "click_when" | "color_branch" => Err(vec![Diagnostic::new(
            "yaml.v3.step.removed",
            &value_path,
            removed_step_message(action),
        )]),
        "match_first" => parse_match_first(value, &value_path),
        _ => Err(vec![Diagnostic::new(
            "yaml.v3.step.unknown",
            &value_path,
            format!("未知 v3 动作 {action:?}"),
        )]),
    }
}

/// 已删除步骤/字段（ADR-YAML-03 click 语法移除 + 契约 §3 收口）的迁移提示。
fn removed_step_message(action: &str) -> String {
    match action {
        "click_when" => format!(
            "click_when 已删除（ADR-YAML-03 click 语法全面移除）：用 find 的 then 分支 + tap: {{point: $match.center}} 表达"
        ),
        "wait_for" => "wait_for 已删除：与 find 同义，用 find 表达（then 为命中分支、else 为超时分支）".to_string(),
        "retry" => "retry 已删除：用 loop 表达（loop: {times: N, steps: [...]}）".to_string(),
        "color_branch" => {
            "color_branch 已删除：用 invoke（capability: vision.sample_color）+ if 按 $<变量>.hex 分支表达".to_string()
        }
        "click" => format!(
            "find.click 已删除（ADR-YAML-03）：命中后动作用 then 分支 + tap: {{point: $match.center}} 表达"
        ),
        other => format!("步骤 {other:?} 已删除"),
    }
}

/// wait 随机区间：字面时长可比较时校验 min ≤ max（引用留待运行期，区间
/// 退化为 min）。与前端 `checkWaitRange` 同口径。
fn check_wait_range(min: &Expr, max: &Expr, path: &str) -> Result<(), Vec<Diagnostic>> {
    if let (Expr::Literal(Value::Duration(min)), Expr::Literal(Value::Duration(max))) = (min, max)
    {
        if min > max {
            return Err(vec![Diagnostic::new(
                "yaml.v3.wait.range",
                format!("{path}.max"),
                format!("随机区间起点 {min}ms 大于终点 {max}ms"),
            )]);
        }
    }
    Ok(())
}

fn parse_find(value: &YamlValue, path: &str) -> Result<SurfaceStep, Vec<Diagnostic>> {
    let map = as_map(value, path, "find 必须是映射")?;
    // click 是被 ADR-YAML-03 整族移除的字段：给专属迁移提示而非泛化未知键。
    if map.contains_key("click") {
        return Err(vec![Diagnostic::new(
            "yaml.v3.field.removed",
            format!("{path}.click"),
            removed_step_message("click"),
        )]);
    }
    reject_unknown(
        map,
        &[
            "template",
            "timeout",
            "threshold",
            "region",
            "save",
            "then",
            "else",
            "verify",
        ],
        path,
    )?;
    let verify = match map.get("verify") {
        None | Some(YamlValue::Null) => None,
        Some(value) => {
            let vmap = as_map(value, &format!("{path}.verify"), "verify 必须是映射")?;
            reject_unknown(vmap, &["template", "timeout"], &format!("{path}.verify"))?;
            Some(FindVerifySurface {
                template: field_expr(vmap, "template", &format!("{path}.verify"))?,
                timeout: vmap
                    .get("timeout")
                    .map(|v| duration_expr(v, &format!("{path}.verify.timeout")))
                    .transpose()?,
            })
        }
    };
    Ok(SurfaceStep::Find {
        template: field_expr(map, "template", path)?,
        timeout: map
            .get("timeout")
            .map(|v| duration_expr(v, &format!("{path}.timeout")))
            .transpose()?,
        threshold: map
            .get("threshold")
            .map(|v| parse_threshold_number(v, &format!("{path}.threshold")))
            .transpose()?,
        region: map
            .get("region")
            .map(|v| expr_from_yaml(v, &format!("{path}.region")))
            .transpose()?,
        save: optional_string(map, "save", path)?,
        then_steps: parse_optional_steps(map.get("then"), &format!("{path}.then"))?,
        else_steps: parse_optional_steps(map.get("else"), &format!("{path}.else"))?,
        verify,
    })
}

fn parse_match_first(value: &YamlValue, path: &str) -> Result<SurfaceStep, Vec<Diagnostic>> {
    let map = as_map(value, path, "match_first 必须是映射")?;
    // `then` 不再是 match_first 的键（契约 §3：候选各自携带 steps；顶层只有
    // candidates/else，与前端编辑器一致）。
    if map.contains_key("then") {
        return Err(vec![Diagnostic::new(
            "yaml.v3.field.removed",
            format!("{path}.then"),
            "match_first 不再支持顶层 then：命中步骤写在每个候选的 steps 里",
        )]);
    }
    reject_unknown(map, &["templates", "candidates", "else"], path)?;
    let raw = map
        .get("candidates")
        .or_else(|| map.get("templates"))
        .ok_or_else(|| {
            vec![Diagnostic::new(
                "yaml.v3.field.missing",
                path,
                "match_first 缺少 templates/candidates",
            )]
        })?;
    let items = match raw {
        YamlValue::Sequence(items) => items,
        _ => {
            return Err(vec![Diagnostic::new(
                "yaml.v3.match_first.type",
                path,
                "templates/candidates 必须是列表",
            )])
        }
    };
    let mut candidates = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let item_path = format!("{path}.candidates[{index}]");
        if let YamlValue::Mapping(item_map) = item {
            // click 同样从候选里移除（ADR-YAML-03）
            if item_map.contains_key("click") {
                return Err(vec![Diagnostic::new(
                    "yaml.v3.field.removed",
                    format!("{item_path}.click"),
                    removed_step_message("click"),
                )]);
            }
            reject_unknown(item_map, &["template", "threshold", "steps"], &item_path)?;
            candidates.push(MatchCandidateSurface {
                template: field_expr(item_map, "template", &item_path)?,
                threshold: item_map
                    .get("threshold")
                    .map(|v| parse_threshold_number(v, &format!("{item_path}.threshold")))
                    .transpose()?,
                steps: parse_optional_steps(item_map.get("steps"), &format!("{item_path}.steps"))?,
            });
        } else {
            candidates.push(MatchCandidateSurface {
                template: expr_from_yaml(item, &item_path)?,
                threshold: None,
                steps: Vec::new(),
            });
        }
    }
    Ok(SurfaceStep::MatchFirst {
        candidates,
        else_steps: parse_optional_steps(map.get("else"), &format!("{path}.else"))?,
    })
}

pub fn lower(surface: &SurfaceProgram) -> Result<Program, Vec<Diagnostic>> {
    let mut lowerer = Lowerer::with_defaults(&surface.defaults);
    let steps = lowerer.steps(&surface.steps, "steps")?;
    Ok(Program {
        version: YAML_V3,
        params: surface.params.clone(),
        steps,
        nonce: None,
    })
}

/// Rewrite only v3 fields that are semantically template references. This is
/// used by the existing template rename API without making that API understand
/// the v3 AST or performing an unsafe global text replacement.
pub(crate) fn rename_template_source(
    source: &str,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
) -> Result<Option<(String, usize)>, Vec<Diagnostic>> {
    if !is_v3_source(source) {
        return Ok(None);
    }
    parse_surface(source)?;
    let mut root: YamlValue = serde_yaml::from_str(source)
        .map_err(|error| vec![Diagnostic::new("yaml.v3.syntax", "", error.to_string())])?;
    let mut changed = 0;
    if let Some(params) = root.get_mut("params") {
        rewrite_params(
            params,
            old_name,
            old_short,
            new_name,
            new_short,
            &mut changed,
        );
    }
    if let Some(steps) = root.get_mut("steps") {
        rewrite_steps(
            steps,
            old_name,
            old_short,
            new_name,
            new_short,
            &mut changed,
        );
    }
    if changed == 0 {
        return Ok(None);
    }
    let rewritten = serde_yaml::to_string(&root)
        .map_err(|error| vec![Diagnostic::new("yaml.v3.serialize", "", error.to_string())])?;
    Ok(Some((rewritten, changed)))
}

/// Rewrite template references inside a v3 function library (bare-map, no
/// `version` key). Used by the template rename API for functions/ files whose
/// content is v3 step syntax; returns `None` when nothing changed.
pub(crate) fn rename_template_in_function_library(
    source: &str,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
) -> Result<Option<(String, usize)>, Vec<Diagnostic>> {
    // 先按 v3 函数库解析校验（失败交由调用方回落 v2 路径）
    parse_function_library(source)?;
    let mut root: YamlValue = serde_yaml::from_str(source)
        .map_err(|error| vec![Diagnostic::new("yaml.v3.syntax", "", error.to_string())])?;
    let Some(map) = root.as_mapping_mut() else {
        return Ok(None);
    };
    let mut changed = 0;
    for (_name, def) in map.iter_mut() {
        let Some(def) = def.as_mapping_mut() else {
            continue;
        };
        if let Some(params) = def.get_mut("params") {
            rewrite_params(
                params,
                old_name,
                old_short,
                new_name,
                new_short,
                &mut changed,
            );
        }
        if let Some(steps) = def.get_mut("steps") {
            rewrite_steps(
                steps,
                old_name,
                old_short,
                new_name,
                new_short,
                &mut changed,
            );
        }
    }
    if changed == 0 {
        return Ok(None);
    }
    let rewritten = serde_yaml::to_string(&root)
        .map_err(|error| vec![Diagnostic::new("yaml.v3.serialize", "", error.to_string())])?;
    Ok(Some((rewritten, changed)))
}

fn rewrite_params(
    value: &mut YamlValue,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
    changed: &mut usize,
) {
    let Some(items) = value.as_sequence_mut() else {
        return;
    };
    for item in items {
        if let Some(map) = item.as_mapping_mut() {
            let is_template = map
                .get("type")
                .and_then(YamlValue::as_str)
                .is_some_and(|value| matches!(value, "tmpl" | "template"));
            if is_template {
                if let Some(default) = map.get_mut("default") {
                    replace_template_scalar(
                        default, old_name, old_short, new_name, new_short, changed,
                    );
                }
            }
        }
    }
}

fn rewrite_steps(
    value: &mut YamlValue,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
    changed: &mut usize,
) {
    let Some(items) = value.as_sequence_mut() else {
        return;
    };
    for item in items {
        rewrite_step(item, old_name, old_short, new_name, new_short, changed);
    }
}

fn rewrite_step(
    value: &mut YamlValue,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
    changed: &mut usize,
) {
    let Some(map) = value.as_mapping_mut() else {
        return;
    };
    let Some(action) = map
        .keys()
        .next()
        .and_then(YamlValue::as_str)
        .map(str::to_string)
    else {
        return;
    };
    let Some(body) = map.get_mut(&action) else {
        return;
    };
    match action.as_str() {
        "find" | "check" => {
            if let Some(body) = body.as_mapping_mut() {
                if let Some(template) = body.get_mut("template") {
                    replace_template_scalar(
                        template, old_name, old_short, new_name, new_short, changed,
                    );
                }
                // find.verify.template 同为模板引用
                if let Some(verify) = body.get_mut("verify").and_then(YamlValue::as_mapping_mut) {
                    if let Some(template) = verify.get_mut("template") {
                        replace_template_scalar(
                            template, old_name, old_short, new_name, new_short, changed,
                        );
                    }
                }
            }
        }
        "match_first" => {
            rewrite_match_first(body, old_name, old_short, new_name, new_short, changed)
        }
        "invoke" => rewrite_invoke(body, old_name, old_short, new_name, new_short, changed),
        _ => {}
    }
    if let Some(body) = body.as_mapping_mut() {
        for key in ["then", "else", "steps"] {
            if let Some(steps) = body.get_mut(key) {
                rewrite_steps(steps, old_name, old_short, new_name, new_short, changed);
            }
        }
    }
}

fn rewrite_match_first(
    value: &mut YamlValue,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
    changed: &mut usize,
) {
    let Some(map) = value.as_mapping_mut() else {
        return;
    };
    let candidates = if map.contains_key("candidates") {
        map.get_mut("candidates")
    } else {
        map.get_mut("templates")
    };
    let Some(candidates) = candidates else {
        return;
    };
    let Some(items) = candidates.as_sequence_mut() else {
        return;
    };
    for item in items {
        if let Some(item_map) = item.as_mapping_mut() {
            if let Some(template) = item_map.get_mut("template") {
                replace_template_scalar(
                    template, old_name, old_short, new_name, new_short, changed,
                );
            }
            if let Some(steps) = item_map.get_mut("steps") {
                rewrite_steps(steps, old_name, old_short, new_name, new_short, changed);
            }
        } else {
            replace_template_scalar(item, old_name, old_short, new_name, new_short, changed);
        }
    }
}

fn rewrite_invoke(
    value: &mut YamlValue,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
    changed: &mut usize,
) {
    let Some(map) = value.as_mapping_mut() else {
        return;
    };
    let is_match = map
        .get("capability")
        .and_then(YamlValue::as_str)
        .is_some_and(|value| matches!(value, "vision.match" | "vision.match_template"));
    let is_many = map
        .get("capability")
        .and_then(YamlValue::as_str)
        .is_some_and(|value| value == "vision.match_many");
    let args = if map.contains_key("args") {
        map.get_mut("args")
    } else {
        map.get_mut("with")
    };
    let Some(args) = args.and_then(YamlValue::as_mapping_mut) else {
        return;
    };
    if is_match {
        if let Some(template) = args.get_mut("template") {
            replace_template_scalar(template, old_name, old_short, new_name, new_short, changed);
        }
    } else if is_many {
        if let Some(templates) = args.get_mut("templates") {
            if let Some(items) = templates.as_sequence_mut() {
                for item in items {
                    replace_template_scalar(
                        item, old_name, old_short, new_name, new_short, changed,
                    );
                }
            }
        }
    }
}

fn replace_template_scalar(
    value: &mut YamlValue,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
    changed: &mut usize,
) {
    let Some(current) = value.as_str() else {
        return;
    };
    let replacement = if current == old_name {
        Some(new_name)
    } else if current == old_short {
        Some(new_short)
    } else {
        None
    };
    if let Some(replacement) = replacement {
        *value = YamlValue::String(replacement.to_string());
        *changed += 1;
    }
}

struct Lowerer {
    serial: u64,
    /// timing 兜底（契约 §4，defaults 覆盖内置值），lower 期展开为显式
    /// `runtime.sleep` invoke——取消语义复用 runtime.sleep 既有取消路径。
    after_tap_ms: u64,
    after_match_ms: u64,
    poll_ms: u64,
    /// vision threshold 兜底（step 值 > defaults.vision.threshold > Runtime
    /// 内置 0.80；前两级在 lower 期解析注入 invoke args，缺省省略字段）。
    vision_threshold: Option<f64>,
}

impl Lowerer {
    fn with_defaults(defaults: &SurfaceDefaults) -> Self {
        Self {
            serial: 0,
            after_tap_ms: defaults.after_tap_ms.unwrap_or(DEFAULT_AFTER_TAP_MS),
            after_match_ms: defaults.after_match_ms.unwrap_or(DEFAULT_AFTER_MATCH_MS),
            poll_ms: defaults.poll_interval_ms.unwrap_or(DEFAULT_POLL_MS),
            vision_threshold: defaults.vision_threshold,
        }
    }

    fn temp(&mut self, prefix: &str) -> String {
        self.serial += 1;
        format!("__yaml_{prefix}_{}", self.serial)
    }

    /// threshold 三级优先的 lower 期解析：step 值 > defaults.vision.threshold；
    /// 都缺省 → 省略字段（Runtime 内置 0.80 兜底，见 matcher）。
    fn threshold_expr(&self, step: Option<f64>) -> Option<Expr> {
        let threshold = step.or(self.vision_threshold)?;
        Some(lit(Value::Float(threshold)))
    }

    /// 固定时长 sleep（timing defaults 的 lower 期展开形态）。0ms 省略。
    fn sleep(&self, ms: u64) -> Vec<SmallStep> {
        if ms == 0 {
            Vec::new()
        } else {
            vec![invoke(
                "runtime.sleep",
                map([("duration", lit(Value::Duration(ms)))]),
                None,
            )]
        }
    }

    /// find/check/verify 的轮询次数：字面 timeout 按 poll_interval 折算成
    /// 迭代上限（ceil，轮询体每轮 sleep poll_interval）；非字面（$var）保留
    /// duration 形态（解释器按 100ms/轮近似折算，动态 timeout 的已知近似）。
    fn poll_times(&self, timeout: Option<Expr>) -> Option<Expr> {
        match timeout {
            None => None,
            Some(Expr::Literal(Value::Duration(ms))) if self.poll_ms > 0 => {
                let iterations = ms.div_ceil(self.poll_ms).max(1);
                Some(lit(Value::Int(iterations as i64)))
            }
            Some(other) => Some(other),
        }
    }

    /// 把多条小步包成单个顶层小 AST 步（truthy(true) 容器）：保持「lower 后
    /// 顶层小 AST 步与 surface 步 1:1」不变量，start_index（契约 §8）才成立。
    fn container(&self, steps: Vec<SmallStep>) -> SmallStep {
        if steps.len() == 1 {
            steps.into_iter().next().expect("len == 1")
        } else {
            SmallStep::If {
                cond: Condition::truthy(lit(Value::Bool(true))),
                then_steps: steps,
                else_steps: Vec::new(),
            }
        }
    }

    fn steps(&mut self, steps: &[SurfaceStep], base: &str) -> Result<Vec<SmallStep>, Vec<Diagnostic>> {
        steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                // P12.6：每个 surface step 的稳定路径在 lower 期生成（`steps[0]`、
                // `steps[2].then[1]`……），挂在它产出的唯一小 AST 步包装上。
                let path = format!("{base}[{index}]");
                let inner = self.step(step, &path)?;
                Ok(labeled(&path, surface_desc(step), inner))
            })
            .collect()
    }

    fn step(&mut self, step: &SurfaceStep, path: &str) -> Result<SmallStep, Vec<Diagnostic>> {
        Ok(match step {
            SurfaceStep::Tap { at } => {
                // tap 后 sleep(after_tap)（契约 §4；容器保持顶层 1:1）
                let mut steps = vec![invoke("input.tap", map([("point", at.clone())]), None)];
                steps.extend(self.sleep(self.after_tap_ms));
                self.container(steps)
            }
            SurfaceStep::Swipe { from, to, duration } => invoke(
                "input.swipe",
                map([
                    ("from", from.clone()),
                    ("to", to.clone()),
                    ("duration", duration.clone()),
                ]),
                None,
            ),
            SurfaceStep::Key { key, action } => invoke(
                "input.key",
                map([
                    ("key", key.clone()),
                    ("action", lit(Value::String(action.clone()))),
                ]),
                None,
            ),
            SurfaceStep::Text { value } => {
                invoke("input.text", map([("value", value.clone())]), None)
            }
            SurfaceStep::Wait { duration, max: None } => {
                invoke("runtime.sleep", map([("duration", duration.clone())]), None)
            }
            SurfaceStep::Wait {
                duration: min,
                max: Some(max),
            } => SmallStep::WaitRandom {
                min: min.clone(),
                max: max.clone(),
            },
            SurfaceStep::Invoke {
                capability,
                args,
                save,
            } => invoke(capability, args.clone(), save.clone()),
            SurfaceStep::Log { level, message } => invoke(
                "log.write",
                map([
                    ("level", lit(Value::String(level.clone()))),
                    ("message", message.clone()),
                ]),
                None,
            ),
            SurfaceStep::AppStart { package } => {
                invoke("app.start", optional_map("package", package.clone()), None)
            }
            SurfaceStep::AppStop { package } => {
                invoke("app.stop", optional_map("package", package.clone()), None)
            }
            SurfaceStep::If {
                cond,
                then_steps,
                else_steps,
            } => SmallStep::If {
                cond: Condition::truthy(cond.clone()),
                then_steps: self.steps(then_steps, &format!("{path}.then"))?,
                else_steps: self.steps(else_steps, &format!("{path}.else"))?,
            },
            SurfaceStep::Loop { times, steps } => SmallStep::Loop {
                times: times.clone(),
                body: self.steps(steps, &format!("{path}.steps"))?,
            },
            SurfaceStep::Break => SmallStep::Break,
            SurfaceStep::Call { target, args, save } => SmallStep::Call {
                target: target.clone(),
                args: args.clone(),
                save: save.clone(),
            },
            SurfaceStep::Return { value } => SmallStep::Return {
                value: value.clone(),
            },
            SurfaceStep::Throw { message } => SmallStep::Throw {
                message: message.clone(),
            },
            SurfaceStep::Set { name, value } => SmallStep::Set {
                name: name.clone(),
                value: value.clone(),
            },
            SurfaceStep::Find {
                template,
                timeout,
                threshold,
                region,
                save,
                then_steps,
                else_steps,
                verify,
            } => self.find(
                template.clone(),
                timeout.clone(),
                *threshold,
                region.clone(),
                save.clone(),
                then_steps,
                else_steps,
                verify.as_ref(),
                path,
            )?,
            SurfaceStep::Check {
                template,
                timeout,
                threshold,
                message,
            } => self.check(
                template.clone(),
                timeout.clone(),
                *threshold,
                message.clone(),
            ),
            SurfaceStep::MatchFirst {
                candidates,
                else_steps,
            } => self.match_first(candidates, else_steps, path)?,
        })
    }

    /// find（ADR-YAML-03 / 契约 §3）：轮询 vision.match 至命中 → save/`$match`
    /// 固化 → sleep(after_match) → then → verify；超时走 else、无 else 抛
    /// `FIND_TIMEOUT: <template>`。块结束后 `$match` 复位 null（上下文以块为界，
    /// 不跨块泄漏；save 的命名变量跨步可用）。
    #[allow(clippy::too_many_arguments)]
    fn find(
        &mut self,
        template: Expr,
        timeout: Option<Expr>,
        threshold: Option<f64>,
        region: Option<Expr>,
        save: Option<String>,
        then_steps: &[SurfaceStep],
        else_steps: &[SurfaceStep],
        verify: Option<&FindVerifySurface>,
        path: &str,
    ) -> Result<SmallStep, Vec<Diagnostic>> {
        let found = self.temp("found");
        let mut match_args = map([("template", template.clone())]);
        if let Some(threshold) = self.threshold_expr(threshold) {
            match_args.insert("threshold".to_string(), threshold);
        }
        if let Some(region) = region {
            match_args.insert("region".to_string(), region);
        }
        let mut hit = Vec::new();
        if let Some(save) = save {
            hit.push(SmallStep::Set {
                name: save,
                value: Expr::reference("match"),
            });
        }
        hit.extend(self.sleep(self.after_match_ms));
        hit.extend(self.steps(then_steps, &format!("{path}.then"))?);
        if let Some(verify) = verify {
            hit.extend(self.verify(verify, threshold)?);
        }
        hit.push(SmallStep::Set {
            name: found.clone(),
            value: lit(Value::Bool(true)),
        });
        hit.push(SmallStep::Break);
        let body = vec![
            invoke("vision.match", match_args, Some("match".to_string())),
            SmallStep::If {
                cond: Condition::truthy(Expr::reference("match.found")),
                then_steps: hit,
                else_steps: self.sleep(self.poll_ms),
            },
        ];
        let mut steps = vec![
            SmallStep::Set {
                name: found.clone(),
                value: lit(Value::Bool(false)),
            },
            SmallStep::Loop {
                times: self.poll_times(Some(timeout.unwrap_or_else(|| {
                    lit(Value::Duration(DEFAULT_FIND_TIMEOUT_MS))
                }))),
                body,
            },
        ];
        let on_timeout = if else_steps.is_empty() {
            vec![SmallStep::Throw {
                message: lit(Value::String(format!(
                    "FIND_TIMEOUT: {}",
                    template_name(&template)
                ))),
            }]
        } else {
            self.steps(else_steps, &format!("{path}.else"))?
        };
        steps.push(SmallStep::If {
            cond: Condition::Equals {
                left: Expr::reference(found),
                right: lit(Value::Bool(false)),
            },
            then_steps: on_timeout,
            else_steps: Vec::new(),
        });
        steps.push(SmallStep::Set {
            name: "match".to_string(),
            value: lit(Value::Null),
        });
        Ok(self.container(steps))
    }

    /// find.verify：在 verify.timeout（缺省 30min）内轮询验证模板；不命中抛
    /// `VERIFY_FAILED: <template>`（裁决：verify 是确认操作生效，静默走 else
    /// 会掩盖异常，故一律抛运行错误）。
    fn verify(
        &mut self,
        verify: &FindVerifySurface,
        threshold: Option<f64>,
    ) -> Result<Vec<SmallStep>, Vec<Diagnostic>> {
        let done = self.temp("verified");
        let result = self.temp("verify_match");
        let mut match_args = map([("template", verify.template.clone())]);
        if let Some(threshold) = self.threshold_expr(threshold) {
            match_args.insert("threshold".to_string(), threshold);
        }
        let body = vec![
            invoke("vision.match", match_args, Some(result.clone())),
            SmallStep::If {
                cond: Condition::truthy(Expr::reference(format!("{result}.found"))),
                then_steps: vec![
                    SmallStep::Set {
                        name: done.clone(),
                        value: lit(Value::Bool(true)),
                    },
                    SmallStep::Break,
                ],
                else_steps: self.sleep(self.poll_ms),
            },
        ];
        Ok(vec![
            SmallStep::Set {
                name: done.clone(),
                value: lit(Value::Bool(false)),
            },
            SmallStep::Loop {
                times: self.poll_times(Some(verify.timeout.clone().unwrap_or_else(|| {
                    lit(Value::Duration(DEFAULT_FIND_TIMEOUT_MS))
                }))),
                body,
            },
            SmallStep::If {
                cond: Condition::Equals {
                    left: Expr::reference(done),
                    right: lit(Value::Bool(false)),
                },
                then_steps: vec![SmallStep::Throw {
                    message: lit(Value::String(format!(
                        "VERIFY_FAILED: {}",
                        template_name(&verify.template)
                    ))),
                }],
                else_steps: Vec::new(),
            },
        ])
    }

    /// check（契约 §3：轮询至出现，超时 throw）：命中 → sleep(after_match) →
    /// break；未命中 → sleep(poll_interval)；超时（或一次性未命中后的超时）
    /// 抛 `throw` 文案（缺省「check 未命中」）。
    fn check(
        &mut self,
        template: Expr,
        timeout: Option<Expr>,
        threshold: Option<f64>,
        message: Option<Expr>,
    ) -> SmallStep {
        let done = self.temp("check_done");
        let result = self.temp("check");
        let mut match_args = map([("template", template)]);
        if let Some(threshold) = self.threshold_expr(threshold) {
            match_args.insert("threshold".to_string(), threshold);
        }
        let fail = message.unwrap_or_else(|| lit(Value::String("check 未命中".to_string())));
        let mut hit = self.sleep(self.after_match_ms);
        hit.push(SmallStep::Set {
            name: done.clone(),
            value: lit(Value::Bool(true)),
        });
        hit.push(SmallStep::Break);
        let body = vec![
            invoke("vision.match", match_args, Some(result.clone())),
            SmallStep::If {
                cond: Condition::truthy(Expr::reference(format!("{result}.found"))),
                then_steps: hit,
                else_steps: self.sleep(self.poll_ms),
            },
        ];
        let steps = vec![
            SmallStep::Set {
                name: done.clone(),
                value: lit(Value::Bool(false)),
            },
            SmallStep::Loop {
                times: self.poll_times(timeout),
                body,
            },
            SmallStep::If {
                cond: Condition::Equals {
                    left: Expr::reference(done),
                    right: lit(Value::Bool(false)),
                },
                then_steps: vec![SmallStep::Throw { message: fail }],
                else_steps: Vec::new(),
            },
        ];
        self.container(steps)
    }

    /// match_first（契约 §3）：单帧 match_many（args 支持与 templates 平行的
    /// thresholds 列表做候选级 threshold）→ 首个命中候选执行自己的 steps
    /// （体内 `$match` = 该候选结果）；全未中有 else 走 else、无 else 静默继续
    /// （保持既有 v3 行为）。
    fn match_first(
        &mut self,
        candidates: &[MatchCandidateSurface],
        else_steps: &[SurfaceStep],
        path: &str,
    ) -> Result<SmallStep, Vec<Diagnostic>> {
        let result = self.temp("match_many");
        let templates = Expr::List(candidates.iter().map(|c| c.template.clone()).collect());
        let mut args = map([("templates", templates)]);
        let thresholds: Vec<Expr> = candidates
            .iter()
            .map(|c| self.threshold_expr(c.threshold).unwrap_or(lit(Value::Null)))
            .collect();
        if candidates
            .iter()
            .any(|c| self.threshold_expr(c.threshold).is_some())
        {
            args.insert("thresholds".to_string(), Expr::List(thresholds));
        }
        let mut branches = self.steps(else_steps, &format!("{path}.else"))?;
        for (index, candidate) in candidates.iter().enumerate().rev() {
            let mut branch = vec![SmallStep::Set {
                name: "match".to_string(),
                value: Expr::reference(format!("{result}.matches[{index}]")),
            }];
            branch.extend(self.sleep(self.after_match_ms));
            branch.extend(
                self.steps(
                    &candidate.steps,
                    &format!("{path}.candidates[{index}].steps"),
                )?,
            );
            branches = vec![SmallStep::If {
                cond: Condition::truthy(Expr::reference(format!(
                    "{result}.matches[{index}].found"
                ))),
                then_steps: branch,
                else_steps: branches,
            }];
        }
        let mut steps = vec![
            invoke("vision.match_many", args, Some(result.clone())),
            // 候选全未中时 else 体内的 `$match` = 整体结果 {found, matches}
            SmallStep::Set {
                name: "match".to_string(),
                value: Expr::reference(result),
            },
        ];
        steps.extend(branches);
        steps.push(SmallStep::Set {
            name: "match".to_string(),
            value: lit(Value::Null),
        });
        Ok(self.container(steps))
    }
}

/// 给 surface step 产出的小 AST 步套运行身份包装（P12.6）。包装不改变预算
/// 语义：解释器把包装步视为原逻辑步，不额外计数。
fn labeled(path: &str, desc: String, step: SmallStep) -> SmallStep {
    SmallStep::Step {
        label: StepLabel {
            path: path.to_string(),
            desc,
        },
        step: Box::new(step),
    }
}

/// surface step 的中文可读摘要（desc）：kind 关键字 + 关键参数，如
/// `find 登录按钮`、`tap 0.5,0.3`、`call script:daily/login`、`wait 300ms`。
/// 只取表达式摘要（字面量原样、`$var` 引用原样），不做求值。
fn surface_desc(step: &SurfaceStep) -> String {
    match step {
        SurfaceStep::Tap { at } => format!("tap {}", expr_desc(at)),
        SurfaceStep::Swipe { from, to, .. } => {
            format!("swipe {} → {}", expr_desc(from), expr_desc(to))
        }
        SurfaceStep::Key { key, action } => {
            if action == "press" {
                format!("key {}", expr_desc(key))
            } else {
                format!("key {} {action}", expr_desc(key))
            }
        }
        SurfaceStep::Text { value } => format!("text {}", expr_desc(value)),
        SurfaceStep::Wait { duration, max } => match max {
            Some(max) => format!("wait {}~{}", expr_desc(duration), expr_desc(max)),
            None => format!("wait {}", expr_desc(duration)),
        },
        SurfaceStep::If { cond, .. } => format!("if {}", expr_desc(cond)),
        SurfaceStep::Loop { times, .. } => match times {
            Some(times) => format!("loop {} 次", expr_desc(times)),
            None => "loop".to_string(),
        },
        SurfaceStep::Break => "break".to_string(),
        SurfaceStep::Call { target, .. } => format!("call {target}"),
        SurfaceStep::Return { .. } => "return".to_string(),
        SurfaceStep::Throw { message } => format!("throw {}", expr_desc(message)),
        SurfaceStep::Set { name, .. } => format!("set {name}"),
        SurfaceStep::Invoke { capability, .. } => format!("invoke {capability}"),
        SurfaceStep::Log { message, .. } => format!("log {}", expr_desc(message)),
        SurfaceStep::AppStart { package } => match package {
            Some(package) => format!("app.start {}", expr_desc(package)),
            None => "app.start".to_string(),
        },
        SurfaceStep::AppStop { package } => match package {
            Some(package) => format!("app.stop {}", expr_desc(package)),
            None => "app.stop".to_string(),
        },
        SurfaceStep::Find { template, .. } => format!("find {}", expr_desc(template)),
        SurfaceStep::Check { template, .. } => format!("check {}", expr_desc(template)),
        SurfaceStep::MatchFirst { candidates, .. } => {
            format!("match_first {} 选 1", candidates.len())
        }
    }
}

/// 表达式摘要：字面量紧凑渲染、`$var` 引用带前缀、容器退化为占位。
fn expr_desc(expr: &Expr) -> String {
    match expr {
        Expr::Literal(value) => match value {
            Value::Null => "null".to_string(),
            Value::Bool(value) => value.to_string(),
            Value::Int(value) => value.to_string(),
            Value::Float(value) => format!("{value}"),
            Value::String(value) | Value::Color(value) => value.clone(),
            Value::Duration(ms) => duration_desc(*ms),
            Value::Coordinate([x, y]) => format!("{x},{y}"),
            Value::List(_) => "[…]".to_string(),
            Value::Map(_) => "{…}".to_string(),
            Value::Handle { .. } => "<handle>".to_string(),
        },
        Expr::Ref(name) => format!("${name}"),
        Expr::List(_) => "[…]".to_string(),
        Expr::Map(_) => "{…}".to_string(),
    }
}

/// 时长摘要：整秒用 `2s`，其余毫秒（契约示例 `wait 300ms` 形态）。
fn duration_desc(ms: u64) -> String {
    if ms != 0 && ms % 1000 == 0 {
        format!("{}s", ms / 1000)
    } else {
        format!("{ms}ms")
    }
}

/// find/verify 超时文案里的模板名：字面模板用原名，动态表达式退化为占位。
fn template_name(template: &Expr) -> String {
    match template {
        Expr::Literal(Value::String(name)) => name.clone(),
        _ => "template".to_string(),
    }
}

fn lit(value: Value) -> Expr {
    Expr::Literal(value)
}
fn map<const N: usize>(items: [(&str, Expr); N]) -> BTreeMap<String, Expr> {
    items
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}
fn optional_map(key: &str, value: Option<Expr>) -> BTreeMap<String, Expr> {
    value.map(|value| map([(key, value)])).unwrap_or_default()
}
fn invoke(capability: &str, args: BTreeMap<String, Expr>, save: Option<String>) -> SmallStep {
    SmallStep::Invoke {
        capability: capability.to_string(),
        args,
        save,
    }
}

fn as_map<'a>(
    value: &'a YamlValue,
    path: &str,
    message: &str,
) -> Result<&'a Mapping, Vec<Diagnostic>> {
    value
        .as_mapping()
        .ok_or_else(|| vec![Diagnostic::new("yaml.v3.type", path, message)])
}

fn reject_unknown(map: &Mapping, allowed: &[&str], path: &str) -> Result<(), Vec<Diagnostic>> {
    for key in map.keys() {
        let key = key.as_str().unwrap_or_default();
        if !allowed.contains(&key) {
            return Err(vec![Diagnostic::new(
                "yaml.v3.field.unknown",
                format!("{path}.{key}"),
                format!("不支持字段 {key:?}"),
            )]);
        }
    }
    Ok(())
}

fn required_string(map: &Mapping, key: &str, path: &str) -> Result<String, Vec<Diagnostic>> {
    map.get(key)
        .and_then(YamlValue::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            vec![Diagnostic::new(
                "yaml.v3.field.string",
                format!("{path}.{key}"),
                format!("{key} 必须是字符串"),
            )]
        })
}

fn optional_string(
    map: &Mapping,
    key: &str,
    path: &str,
) -> Result<Option<String>, Vec<Diagnostic>> {
    match map.get(key) {
        None | Some(YamlValue::Null) => Ok(None),
        Some(YamlValue::String(value)) if !value.trim().is_empty() => Ok(Some(value.to_string())),
        _ => Err(vec![Diagnostic::new(
            "yaml.v3.field.string",
            format!("{path}.{key}"),
            format!("{key} 必须是非空字符串"),
        )]),
    }
}

fn field_expr(map: &Mapping, key: &str, path: &str) -> Result<Expr, Vec<Diagnostic>> {
    map.get(key)
        .map(|value| expr_from_yaml(value, &format!("{path}.{key}")))
        .transpose()?
        .ok_or_else(|| {
            vec![Diagnostic::new(
                "yaml.v3.field.missing",
                format!("{path}.{key}"),
                format!("缺少字段 {key}"),
            )]
        })
}

fn map_value<'a>(
    value: &'a YamlValue,
    primary: &str,
    alias: &str,
    path: &str,
) -> Result<&'a YamlValue, Vec<Diagnostic>> {
    if let Some(map) = value.as_mapping() {
        map.get(primary).or_else(|| map.get(alias)).ok_or_else(|| {
            vec![Diagnostic::new(
                "yaml.v3.field.missing",
                path,
                format!("缺少字段 {primary}"),
            )]
        })
    } else {
        Ok(value)
    }
}

fn map_or_value_expr(value: &YamlValue, key: &str, path: &str) -> Result<Expr, Vec<Diagnostic>> {
    if let Some(map) = value.as_mapping() {
        field_expr(map, key, path)
    } else {
        expr_from_yaml(value, path)
    }
}

fn point_expr(value: &YamlValue, path: &str) -> Result<Expr, Vec<Diagnostic>> {
    if let Some(map) = value.as_mapping() {
        // `point` / `at` 双键（ADR-YAML-03 示例用 point；与前端编辑器同口径）
        let key = if map.contains_key("point") {
            "point"
        } else {
            "at"
        };
        return field_expr(map, key, path);
    }
    expr_from_yaml(value, path)
}

fn duration_expr(value: &YamlValue, path: &str) -> Result<Expr, Vec<Diagnostic>> {
    if let Some(raw) = value.as_str() {
        if let Some(name) = raw.strip_prefix('$').filter(|name| !name.is_empty()) {
            return Ok(Expr::reference(name));
        }
        return Ok(lit(Value::Duration(parse_duration_ms(raw).ok_or_else(
            || {
                vec![Diagnostic::new(
                    "yaml.v3.duration",
                    path,
                    "时间必须是如 100ms/2s/1m 的正值",
                )]
            },
        )?)));
    }
    if let Some(value) = value.as_u64() {
        return Ok(lit(Value::Duration(value)));
    }
    Err(vec![Diagnostic::new(
        "yaml.v3.duration",
        path,
        "时间必须是字符串或非负整数毫秒",
    )])
}

fn parse_duration_ms(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    let (number, unit) = raw
        .chars()
        .position(|char| !char.is_ascii_digit())
        .map(|index| raw.split_at(index))?;
    let number = number.parse::<u64>().ok()?;
    if number == 0 {
        return Some(0);
    }
    number.checked_mul(match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => return None,
    })
}

fn args_map(map: &Mapping, path: &str) -> Result<BTreeMap<String, Expr>, Vec<Diagnostic>> {
    let value = map.get("with").or_else(|| map.get("args"));
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let value = as_map(value, &format!("{path}.with"), "with/args 必须是映射")?;
    value
        .iter()
        .map(|(key, value)| {
            Ok((
                key.as_str().unwrap_or_default().to_string(),
                expr_from_yaml(
                    value,
                    &format!("{path}.with.{}", key.as_str().unwrap_or_default()),
                )?,
            ))
        })
        .collect()
}

fn required_steps(
    map: &Mapping,
    key: &str,
    path: &str,
) -> Result<Vec<SurfaceStep>, Vec<Diagnostic>> {
    map.get(key)
        .ok_or_else(|| {
            vec![Diagnostic::new(
                "yaml.v3.field.missing",
                format!("{path}.{key}"),
                format!("缺少字段 {key}"),
            )]
        })
        .and_then(|value| parse_steps(value, &format!("{path}.{key}")))
}

fn parse_optional_steps(
    value: Option<&YamlValue>,
    path: &str,
) -> Result<Vec<SurfaceStep>, Vec<Diagnostic>> {
    value
        .map(|value| parse_steps(value, path))
        .transpose()
        .map(|value| value.unwrap_or_default())
}

fn expr_from_yaml(value: &YamlValue, path: &str) -> Result<Expr, Vec<Diagnostic>> {
    Ok(match value {
        YamlValue::Null => lit(Value::Null),
        YamlValue::Bool(value) => lit(Value::Bool(*value)),
        YamlValue::Number(value) => {
            if let Some(value) = value.as_i64() {
                lit(Value::Int(value))
            } else {
                lit(Value::Float(value.as_f64().ok_or_else(|| {
                    vec![Diagnostic::new(
                        "yaml.v3.number",
                        path,
                        "数字必须是有限数值",
                    )]
                })?))
            }
        }
        YamlValue::String(value)
            if value.strip_prefix('$').is_some_and(|name| !name.is_empty()) =>
        {
            Expr::reference(value.trim_start_matches('$'))
        }
        YamlValue::String(value) => lit(Value::String(value.clone())),
        YamlValue::Sequence(items)
            if items.len() == 2 && items.iter().all(|item| item.as_f64().is_some()) =>
        {
            lit(Value::Coordinate([
                items[0].as_f64().unwrap(),
                items[1].as_f64().unwrap(),
            ]))
        }
        YamlValue::Sequence(items) => Expr::List(
            items
                .iter()
                .enumerate()
                .map(|(index, item)| expr_from_yaml(item, &format!("{path}[{index}]")))
                .collect::<Result<_, _>>()?,
        ),
        YamlValue::Mapping(map) => Expr::Map(
            map.iter()
                .map(|(key, value)| {
                    Ok((
                        key.as_str().unwrap_or_default().to_string(),
                        expr_from_yaml(
                            value,
                            &format!("{path}.{}", key.as_str().unwrap_or_default()),
                        )?,
                    ))
                })
                .collect::<Result<_, Vec<Diagnostic>>>()?,
        ),
        _ => {
            return Err(vec![Diagnostic::new(
                "yaml.v3.value",
                path,
                "不支持的 YAML 值",
            )])
        }
    })
}

fn value_from_yaml(value: &YamlValue, path: &str) -> Result<Value, Vec<Diagnostic>> {
    match expr_from_yaml(value, path)? {
        Expr::Literal(value) => Ok(value),
        Expr::Ref(_) | Expr::List(_) | Expr::Map(_) => Err(vec![Diagnostic::new(
            "yaml.v3.params.default",
            path,
            "参数默认值必须是字面量",
        )]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v3_loader_rejects_v2_shape_and_unknown_top_level() {
        let error = load("steps:\n  - tap: [0.1, 0.2]\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.version.missing");
        let error = load("version: 3\nextra: true\nsteps: []\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.top_level.unknown_key");
    }

    /// P12.6：lower 为每个 surface step 生成稳定 path + 中文 desc（与前端
    /// 寻址同语法）；timing/轮询展开物不带 label（运行时天然静默）。
    #[test]
    fn lower_labels_surface_steps_with_stable_paths_and_desc() {
        let program = load(
            "version: 3\nsteps:\n  - log: start\n  - tap: [0.5, 0.3]\n  - find:\n      template: 登录按钮\n      timeout: 2s\n      then:\n        - wait: 300ms\n  - call:\n      target: script:daily/login\n",
        )
        .unwrap();
        let mut labels = Vec::new();
        fn collect(step: &SmallStep, labels: &mut Vec<(String, String, bool)>) {
            if let SmallStep::Step { label, step } = step {
                labels.push((label.path.clone(), label.desc.clone(), true));
                collect(step, labels);
                return;
            }
            match step {
                SmallStep::If {
                    then_steps,
                    else_steps,
                    ..
                } => {
                    for s in then_steps {
                        collect(s, labels);
                    }
                    for s in else_steps {
                        collect(s, labels);
                    }
                }
                SmallStep::Loop { body, .. } => {
                    for s in body {
                        collect(s, labels);
                    }
                }
                _ => {}
            }
        }
        for step in &program.steps {
            collect(step, &mut labels);
        }
        assert_eq!(
            labels,
            vec![
                ("steps[0]".to_string(), "log start".to_string(), true),
                ("steps[1]".to_string(), "tap 0.5,0.3".to_string(), true),
                (
                    "steps[2]".to_string(),
                    "find 登录按钮".to_string(),
                    true
                ),
                (
                    "steps[2].then[0]".to_string(),
                    "wait 300ms".to_string(),
                    true
                ),
                (
                    "steps[3]".to_string(),
                    "call script:daily/login".to_string(),
                    true
                ),
            ],
            "surface step 路径与 desc 摘要"
        );
        // tap 的 after_tap sleep 是 lower 展开物：无 label 包装
        let tap_container = &program.steps[1];
        let SmallStep::Step { step: inner, .. } = tap_container else {
            panic!("tap 顶层必须是 Step 包装");
        };
        let SmallStep::If { then_steps, .. } = inner.as_ref() else {
            panic!("tap 容器形态保持");
        };
        assert_eq!(then_steps.len(), 2, "invoke tap + after_tap sleep");
        assert!(
            matches!(&then_steps[1], SmallStep::Invoke { capability, .. } if capability == "runtime.sleep"),
            "展开物保持裸 invoke（无 label）"
        );
    }

    #[test]
    fn primitive_surface_lowers_to_generic_invokes() {
        let program = load(
            "version: 3\nsteps:\n  - tap: [0.1, 0.2]\n  - swipe:\n      from: [0.1, 0.2]\n      to: [0.8, 0.9]\n      duration: 250ms\n  - key: ESC\n  - text: hello\n  - wait: 1s\n  - app.start\n  - app.stop\n  - log: done\n",
        )
        .unwrap();
        let json = serde_json::to_string(&program).unwrap();
        assert!(json.contains("input.tap"));
        assert!(json.contains("input.swipe"));
        assert!(json.contains("input.key"));
        assert!(json.contains("runtime.sleep"));
        assert!(json.contains("app.start"));
        assert!(!json.contains("\"op\":\"tap\""));
    }

    #[test]
    fn all_high_level_sugars_lower_without_sugar_nodes() {
        let program = load(
            "version: 3\nsteps:\n  - find:\n      template: login\n      timeout: 1s\n      threshold: 0.9\n      save: hit\n      then:\n        - tap: {point: $hit.center}\n      verify:\n        template: home\n        timeout: 5s\n  - check:\n      template: ready\n      threshold: 0.85\n  - wait: {min: 300ms, max: 700ms}\n  - match_first:\n      candidates:\n        - template: a\n          threshold: 0.9\n          steps:\n            - log: a\n        - template: b\n",
        )
        .unwrap();
        let json = serde_json::to_string(&program).unwrap();
        for sugar in ["find", "check", "match_first"] {
            assert!(
                !json.contains(&format!("\"op\":\"{sugar}\"")),
                "sugar leaked: {sugar}"
            );
        }
        assert!(json.contains("vision.match"));
        assert!(json.contains("vision.match_many"));
        assert!(json.contains("runtime.sleep"));
        assert!(json.contains("\"op\":\"wait_random\""));
    }

    #[test]
    fn value_types_include_duration_coordinate_record_and_handle() {
        let value = Value::Map(BTreeMap::from([
            ("duration".to_string(), Value::Duration(12)),
            ("point".to_string(), Value::Coordinate([0.1, 0.2])),
            (
                "handle".to_string(),
                Value::Handle {
                    kind: "frame".to_string(),
                    id: 4,
                },
            ),
        ]));
        let round_trip = Value::from_json(value.clone().into_json()).unwrap();
        assert_eq!(round_trip, value);
    }

    #[test]
    fn v3_surface_covers_control_and_dynamic_nodes() {
        let program = load(
            "version: 3\nsteps:\n  - set: {ready: true}\n  - if:\n      cond: $ready\n      then:\n        - loop:\n            times: 2\n            steps:\n              - break\n      else: []\n  - call:\n      target: script:helper\n      with: {arg: 1}\n      save: result\n  - invoke:\n      capability: plugin.value\n      with: {value: $result}\n      save: output\n  - throw: failed\n  - return: $output\n",
        )
        .unwrap();
        let json = serde_json::to_string(&program).unwrap();
        for op in [
            "set", "if", "loop", "break", "call", "invoke", "throw", "return",
        ] {
            assert!(
                json.contains(&format!("\"op\":\"{op}\"")),
                "missing op: {op}"
            );
        }
    }

    #[test]
    fn removed_click_family_steps_get_migration_diagnostics() {
        let cases = [
            ("click_when", "click_when 已删除"),
            ("wait_for", "wait_for 已删除"),
            ("retry", "retry 已删除"),
            ("color_branch", "color_branch 已删除"),
        ];
        for (action, fragment) in cases {
            let error = load(&format!(
                "version: 3\nsteps:\n  - {action}:\n      template: x\n"
            ))
            .unwrap_err();
            assert_eq!(error[0].code, "yaml.v3.step.removed", "action={action}");
            assert!(error[0].message.contains(fragment));
        }
        // find.click 字段与 match 候选 click：专属迁移提示
        let error = load(
            "version: 3\nsteps:\n  - find:\n      template: x\n      click: true\n",
        )
        .unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.field.removed");
        assert!(error[0].message.contains("$match.center"));
        let error = load(
            "version: 3\nsteps:\n  - match_first:\n      candidates:\n        - template: a\n          click: true\n",
        )
        .unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.field.removed");
        // match_first 顶层 then 已删除（候选步骤归 steps）
        let error = load(
            "version: 3\nsteps:\n  - match_first:\n      candidates: [a]\n      then:\n        - log: x\n",
        )
        .unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.field.removed");
    }

    #[test]
    fn splitmix64_test_vectors() {
        // 与 server/guests/yaml-guest/src/lib.rs 的逐字拷贝锁定同一向量：
        // 宿主原生参考解释器与 WASM guest 必须产出相同随机序列。
        let mut state = 7u64;
        assert_eq!(super::splitmix64(&mut state), 7191089600892374487);
        assert_eq!(super::splitmix64(&mut state), 309689372594955804);
        assert_eq!(super::splitmix64(&mut state), 16616101746815609346);
    }

    #[test]
    fn defaults_parse_unknown_keys_and_range() {
        let surface = parse_surface(
            "version: 3\ndefaults:\n  vision:\n    threshold: 0.9\n  timing:\n    after_tap: 250ms\n    after_match: 1s\n    poll_interval: 200\nsteps:\n  - log: ok\n",
        )
        .unwrap();
        assert_eq!(surface.defaults.vision_threshold, Some(0.9));
        assert_eq!(surface.defaults.after_tap_ms, Some(250));
        assert_eq!(surface.defaults.after_match_ms, Some(1_000));
        assert_eq!(surface.defaults.poll_interval_ms, Some(200));

        let error = load("version: 3\ndefaults:\n  other: 1\nsteps: []\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.defaults.unknown_key");
        let error =
            load("version: 3\ndefaults:\n  vision:\n    contrast: 1\nsteps: []\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.defaults.unknown_key");
        let error =
            load("version: 3\ndefaults:\n  timing:\n    judge_delay: 5ms\nsteps: []\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.defaults.unknown_key");
        let error = load("version: 3\ndefaults:\n  vision:\n    threshold: 1.5\nsteps: []\n")
            .unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.defaults.range");
        let error =
            load("version: 3\ndefaults:\n  timing:\n    after_tap: $wait\nsteps: []\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.defaults.type");
        // 顶层白名单收口：defaults 之外的未知顶层键仍拒绝
        let error = load("version: 3\ndefaults: {}\nsteps: []\nextra: 1\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.top_level.unknown_key");
    }

    #[test]
    fn threshold_resolves_step_over_defaults_over_builtin() {
        // step 值优先
        let program = load(
            "version: 3\ndefaults:\n  vision:\n    threshold: 0.7\nsteps:\n  - check:\n      template: a\n      threshold: 0.95\n",
        )
        .unwrap();
        let json = serde_json::to_string(&program).unwrap();
        assert!(json.contains("\"threshold\":{\"expr\":\"literal\",\"value\":{\"type\":\"float\",\"value\":0.95}}"));

        // defaults 兜底
        let program = load(
            "version: 3\ndefaults:\n  vision:\n    threshold: 0.7\nsteps:\n  - check:\n      template: a\n",
        )
        .unwrap();
        let json = serde_json::to_string(&program).unwrap();
        assert!(json.contains("\"value\":0.7"));

        // 都缺省 → 省略 threshold 字段（Runtime 内置 0.80）
        let program = load("version: 3\nsteps:\n  - check:\n      template: a\n").unwrap();
        let json = serde_json::to_string(&program).unwrap();
        assert!(!json.contains("threshold"));

        // match_first 候选级 threshold：有候选设置时注入 thresholds 平行列表
        let program = load(
            "version: 3\ndefaults:\n  vision:\n    threshold: 0.7\nsteps:\n  - match_first:\n      candidates:\n        - template: a\n        - template: b\n          threshold: 0.6\n",
        )
        .unwrap();
        let json = serde_json::to_string(&program).unwrap();
        assert!(json.contains("thresholds"));
        assert!(json.contains("\"value\":0.6"));
        assert!(json.contains("\"value\":0.7"));
    }

    #[test]
    fn wait_random_range_validation_and_lowering() {
        let program = load(
            "version: 3\nsteps:\n  - wait: {min: 300ms, max: 700ms}\n  - wait: 500ms\n  - wait: {duration: 1s}\n",
        )
        .unwrap();
        let json = serde_json::to_string(&program).unwrap();
        assert!(json.contains("\"op\":\"wait_random\""));
        assert_eq!(
            serde_json::to_string(&program)
                .unwrap()
                .matches("runtime.sleep")
                .count(),
            2,
            "固定 wait 各展开为一次 runtime.sleep"
        );

        // min/max 必须同给
        let error = load("version: 3\nsteps:\n  - wait: {min: 100ms}\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.field.missing");
        // min ≤ max
        let error = load("version: 3\nsteps:\n  - wait: {min: 700ms, max: 300ms}\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.wait.range");
        // 混用 duration/min 拒绝
        let error =
            load("version: 3\nsteps:\n  - wait: {duration: 1s, min: 100ms}\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.field.unknown");
    }

    #[test]
    fn timing_defaults_expand_to_explicit_sleeps() {
        // 内置兜底：tap 后 300ms
        let program = load("version: 3\nsteps:\n  - tap: [0.5, 0.5]\n").unwrap();
        let json = serde_json::to_string(&program).unwrap();
        assert!(json.contains("input.tap"));
        assert!(json.contains("\"type\":\"duration\",\"value\":300"));

        // defaults 覆盖
        let program = load(
            "version: 3\ndefaults:\n  timing:\n    after_tap: 50ms\nsteps:\n  - tap: [0.5, 0.5]\n",
        )
        .unwrap();
        let json = serde_json::to_string(&program).unwrap();
        assert!(json.contains("\"type\":\"duration\",\"value\":50"));
        assert!(!json.contains("\"value\":300"));

        // find 轮询间隔与命中后 sleep：poll_interval=250ms、timeout 折算迭代数
        let program = load(
            "version: 3\ndefaults:\n  timing:\n    poll_interval: 250ms\nsteps:\n  - find:\n      template: a\n      timeout: 10s\n      then:\n        - log: hit\n",
        )
        .unwrap();
        let json = serde_json::to_string(&program).unwrap();
        assert!(json.contains("\"type\":\"duration\",\"value\":250"));
        assert!(json.contains("\"type\":\"duration\",\"value\":200"), "命中后 after_match=200ms 兜底");
        // timeout 10s / 250ms = 40 次迭代
        assert!(json.contains("\"type\":\"int\",\"value\":40"));

        // find timeout 缺省 = 30min（18000 次 @100ms）
        let program = load("version: 3\nsteps:\n  - find:\n      template: a\n").unwrap();
        let json = serde_json::to_string(&program).unwrap();
        assert!(json.contains("\"type\":\"int\",\"value\":18000"));
    }

    #[test]
    fn find_lowers_match_context_save_and_verify() {
        let program = load(
            "version: 3\nsteps:\n  - find:\n      template: reward\n      timeout: 10s\n      save: reward\n      then:\n        - tap: {point: $reward.center}\n        - log: $match.score\n      else:\n        - log: miss\n      verify:\n        template: home\n        timeout: 5s\n",
        )
        .unwrap();
        let json = serde_json::to_string(&program).unwrap();
        // match 结果以正式名 `match` 落变量（$match 上下文），save 镜像到命名变量
        assert!(json.contains("\"save\":\"match\""));
        assert!(json.contains("\"name\":\"reward\""));
        assert!(json.contains("\"op\":\"throw\""));
        assert!(json.contains("VERIFY_FAILED: home"));
        // 块结束 $match 复位 null（不跨块泄漏）
        assert!(json.contains("\"name\":\"match\""));
    }

    #[test]
    fn find_timeout_without_else_throws_find_timeout() {
        let program = load("version: 3\nsteps:\n  - find:\n      template: ghost\n      timeout: 2s\n").unwrap();
        let json = serde_json::to_string(&program).unwrap();
        assert!(json.contains("FIND_TIMEOUT: ghost"));
        // 有 else 则走 else 不抛
        let program = load(
            "version: 3\nsteps:\n  - find:\n      template: ghost\n      timeout: 2s\n      else:\n        - log: gone\n",
        )
        .unwrap();
        let json = serde_json::to_string(&program).unwrap();
        assert!(!json.contains("FIND_TIMEOUT"));
    }

    #[test]
    fn param_remark_survives_both_decl_forms() {
        let surface = parse_surface(
            "version: 3\nparams:\n  - 'text:msg:消息内容:\"默认\"'\n  - name: count\n    type: int\n    remark: 次数\n    default: 3\nsteps: []\n",
        )
        .unwrap();
        assert_eq!(surface.params[0].remark.as_deref(), Some("消息内容"));
        assert_eq!(surface.params[1].remark.as_deref(), Some("次数"));
        // 空 remark 归一为 None
        let surface =
            parse_surface("version: 3\nparams:\n  - 'text:msg::\"x\"'\nsteps: []\n").unwrap();
        assert_eq!(surface.params[0].remark, None);
        // 库函数解析同样透出
        let library = parse_function_library(
            "fn:\n  params:\n    - 'int:times:次数:2'\n  steps: []\n",
        )
        .unwrap();
        assert_eq!(library[0].params[0].remark.as_deref(), Some("次数"));
    }

    #[test]
    fn bare_call_targets_are_rejected_with_namespace_diagnostic() {
        let error = load("version: 3\nsteps:\n  - call:\n      target: helper\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.call.namespace");
        assert!(error[0].message.contains("helper"), "诊断必须含 target 原文");
        assert!(error[0].message.contains("script:"));
        assert!(error[0].message.contains("function:"));
        let error = load("version: 3\nsteps:\n  - call:\n      target: plugin:value\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.call.namespace");
        let error = load("version: 3\nsteps:\n  - call:\n      target: \"script:\"\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.call.namespace");
    }

    #[test]
    fn namespaced_call_targets_parse_and_keep_args_alias() {
        let surface = parse_surface(
            "version: 3\nsteps:\n  - call:\n      target: script:daily/login\n      args: {account: $user}\n      save: result\n  - call:\n      target: function:common/login/is_logged_in\n      with: {flag: true}\n",
        )
        .unwrap();
        assert_eq!(
            surface.steps[0],
            SurfaceStep::Call {
                target: "script:daily/login".into(),
                args: BTreeMap::from([("account".into(), Expr::reference("user"))]),
                save: Some("result".into()),
            }
        );
        assert_eq!(
            surface.steps[1],
            SurfaceStep::Call {
                target: "function:common/login/is_logged_in".into(),
                args: BTreeMap::from([("flag".into(), lit(Value::Bool(true)))]),
                save: None,
            }
        );
        let lowered = lower(&surface).unwrap();
        // P12.6：顶层小 AST 步是 Step 运行身份包装（path 指回 surface 步）
        assert!(matches!(
            &lowered.steps[1],
            SmallStep::Step { label, step }
                if label.path == "steps[1]"
                    && matches!(step.as_ref(), SmallStep::Call { target, .. } if target == "function:common/login/is_logged_in")
        ));
    }

    #[test]
    fn function_call_paths_reject_traversal_and_bad_shapes() {
        let cases = [
            ("function:../evil/fn", "yaml.v3.call.target"),
            ("function:a/b/../../../fn", "yaml.v3.call.target"),
            ("function:a\\b/fn", "yaml.v3.call.target"),
            ("function:/abs/fn", "yaml.v3.call.target"),
            ("function:a//fn", "yaml.v3.call.target"),
            ("function:file/", "yaml.v3.call.target"), // 尾随 / = 空段
            ("function:noslash", "yaml.v3.call.function_path"),
        ];
        for (target, code) in cases {
            let source = format!("version: 3\nsteps:\n  - call:\n      target: {target:?}\n");
            let error = load(&source).unwrap_err();
            assert_eq!(error[0].code, code, "target {target:?}");
        }
        // script id 同样拒绝穿越
        let error = load("version: 3\nsteps:\n  - call:\n      target: script:../x\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.call.target");
    }

    #[test]
    fn function_library_parses_bare_map_and_lowers_named_function() {
        let source = "fn1:\n  params:\n    - name: flag\n      type: bool\n      default: false\n  steps:\n    - if:\n        cond: $flag\n        then:\n          - return: {ok: true}\n    - return: {ok: false}\nfn2:\n  steps:\n    - return: [1, 2, 3]\n";
        let library = parse_function_library(source).unwrap();
        assert_eq!(
            library.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["fn1", "fn2"]
        );
        let program = load_function(source, "fn2").unwrap();
        assert_eq!(program.version, YAML_V3);
        assert!(program.params.is_empty());
        assert_eq!(program.steps.len(), 1);
        let program = load_function(source, "fn1").unwrap();
        assert_eq!(program.params.len(), 1);
        let error = load_function(source, "missing").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.function.not_found");
    }

    #[test]
    fn function_library_rejects_invalid_structures() {
        // 空文件（解析为 Null，顶层不是映射）与语法错误
        let error = parse_function_library("").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.type");
        let error = parse_function_library("fn: [unclosed\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.syntax");
        let error = parse_function_library("{}").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.function.file");
        // 顶层不是映射
        let error = parse_function_library("- a\n- b\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.type");
        // 函数定义必须是映射（version 键会被当作函数名，标量值非法）
        let error = parse_function_library("version: 3\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.type");
        // steps 缺失
        let error = parse_function_library("fn:\n  params: []\n").unwrap_err();
        assert_eq!(
            error[0].code, "yaml.v3.field.missing",
            "path={}", error[0].path
        );
        assert_eq!(error[0].path, "fn.steps");
        // 未知字段
        let error = parse_function_library("fn:\n  steps: []\n  extra: 1\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.function.unknown_key");
        // steps 非列表
        let error = parse_function_library("fn:\n  steps: 3\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.steps.type");
    }

    #[test]
    fn function_library_rejects_invalid_names() {
        // 非法字符集 / 数字开头
        let error = parse_function_library("my-fn:\n  steps: []\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.function.name");
        let error = parse_function_library("3fn:\n  steps: []\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.function.name");
        // 保留字（动作键 / 结构键 / $match）
        for reserved in ["tap", "find", "match", "then", "defaults"] {
            let error =
                parse_function_library(&format!("{reserved}:\n  steps: []\n")).unwrap_err();
            assert_eq!(error[0].code, "yaml.v3.function.name", "name={reserved}");
            assert!(error[0].message.contains("保留字"), "name={reserved}");
        }
        // 顶层键是非字符串标量
        let error = parse_function_library("3:\n  steps: []\n").unwrap_err();
        assert_eq!(error[0].code, "yaml.v3.function.name");
        // 中文函数名合法；同名键后者覆盖语义被唯一性承载（映射键本身唯一）
        let library =
            parse_function_library("领取奖励:\n  steps:\n    - log: ok\n").unwrap();
        assert_eq!(library[0].name, "领取奖励");
    }
}

#[cfg(test)]
mod probe_tests {
    use super::*;

    /// 返回值泛化（ADR-YAML-02）：guest 解释器的容器不加 typed wrap、元素是
    /// typed 形态，`Value::from_json` 必须递归还原嵌套容器里的 typed 值。
    #[test]
    fn from_json_rehydrates_typed_leaves_inside_raw_containers() {
        let guest_return = serde_json::json!({
            "ok": {"type": "bool", "value": true},
            "items": [{"type": "int", "value": 1}, {"type": "int", "value": 2}],
        });
        assert_eq!(
            Value::from_json(guest_return).unwrap(),
            Value::Map(BTreeMap::from([
                (
                    "items".to_string(),
                    Value::List(vec![Value::Int(1), Value::Int(2)])
                ),
                ("ok".to_string(), Value::Bool(true)),
            ]))
        );
        assert_eq!(
            Value::from_json(serde_json::json!([
                {"type": "int", "value": 7},
                {"type": "string", "value": "s"},
            ]))
            .unwrap(),
            Value::List(vec![Value::Int(7), Value::String("s".into())])
        );
    }
}
