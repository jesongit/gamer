//! script_v2 严格 AST（docs/SCRIPT_EDITOR_CONTRACT.md §3 五方对照的 Rust 侧）。
//!
//! 字段名与 golden JSON（前端 Model / API JSON）对齐：`Cell` 序列化为
//! `{"lit": …}` / `{"ref": …}`，`Step` 以 `kind` 判别。时间字面量保留书写串
//! （"800ms"），仅 config.interval 解析为 `Duration`（引擎轮询直接可用）。

use std::time::Duration;

use serde::{Serialize, Serializer};

use super::params;

// ---------------------------------------------------------------------------
// 参数模型
// ---------------------------------------------------------------------------

/// 参数类型七类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParamType {
    Tmpl,
    Coord,
    Color,
    Time,
    Key,
    Text,
    Bool,
}

impl ParamType {
    pub fn as_str(self) -> &'static str {
        match self {
            ParamType::Tmpl => "tmpl",
            ParamType::Coord => "coord",
            ParamType::Color => "color",
            ParamType::Time => "time",
            ParamType::Key => "key",
            ParamType::Text => "text",
            ParamType::Bool => "bool",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "tmpl" => ParamType::Tmpl,
            "coord" => ParamType::Coord,
            "color" => ParamType::Color,
            "time" => ParamType::Time,
            "key" => ParamType::Key,
            "text" => ParamType::Text,
            "bool" => ParamType::Bool,
            _ => return None,
        })
    }
}

impl Serialize for ParamType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// 类型化字面量。变体与参数类型一一对应；步骤字段位置约束可用变体
/// （coord 字段只能是 [`TypedValue::Coord`]，以此类推）。
#[derive(Debug, Clone, PartialEq)]
pub enum TypedValue {
    /// 模板短名（如 `account.png`）。
    Tmpl(String),
    /// 0~1 相对坐标 [x, y]。
    Coord([f64; 2]),
    /// 6 位十六进制颜色（无 #，保留书写大小写；比较时统一小写）。
    Color(String),
    /// 时间书写串（带单位，>0，如 "800ms"；"数值保持书写形式"）。
    Time(String),
    /// 按键名（如 "ESC"）。
    Key(String),
    /// 文本。
    Text(String),
    Bool(bool),
}

impl TypedValue {
    /// 该字面量对应的参数类型。
    pub fn param_type(&self) -> ParamType {
        match self {
            TypedValue::Tmpl(_) => ParamType::Tmpl,
            TypedValue::Coord(_) => ParamType::Coord,
            TypedValue::Color(_) => ParamType::Color,
            TypedValue::Time(_) => ParamType::Time,
            TypedValue::Key(_) => ParamType::Key,
            TypedValue::Text(_) => ParamType::Text,
            TypedValue::Bool(_) => ParamType::Bool,
        }
    }
}

impl Serialize for TypedValue {
    /// Model/API JSON 形态：字符串类 → string，coord → 数组，bool → 布尔。
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        match self {
            TypedValue::Tmpl(s)
            | TypedValue::Color(s)
            | TypedValue::Time(s)
            | TypedValue::Key(s)
            | TypedValue::Text(s) => serializer.serialize_str(s),
            TypedValue::Coord([x, y]) => {
                let mut seq = serializer.serialize_seq(Some(2))?;
                seq.serialize_element(x)?;
                seq.serialize_element(y)?;
                seq.end()
            }
            TypedValue::Bool(b) => serializer.serialize_bool(*b),
        }
    }
}

/// 参数声明（`类型:变量名:备注[:默认值]` 的解析结果）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParamDecl {
    #[serde(rename = "type")]
    pub ty: ParamType,
    pub name: String,
    pub remark: String,
    /// `None` = 必填（无默认值）。
    pub default: Option<TypedValue>,
}

// ---------------------------------------------------------------------------
// 取值单元格与步骤
// ---------------------------------------------------------------------------

/// 字段级取值单元格：类型化字面量或 `$name` 完整值引用。
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Lit(TypedValue),
    Ref(String),
}

impl Serialize for Cell {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            Cell::Lit(v) => map.serialize_entry("lit", v)?,
            Cell::Ref(name) => map.serialize_entry("ref", name)?,
        }
        map.end()
    }
}

/// match 候选（有序，单键映射 `模板: [分支步骤]` 的解析结果；候选值也可为
/// `{click: true, steps: [...]}` 映射——命中后点击模板中心，规范序列化时
/// `click: false` 还原为列表形态、`click: true` 恒为映射形态）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MatchCandidate {
    pub template: Cell,
    pub click: bool,
    pub steps: Vec<Step>,
}

/// color 候选分支（有序列表项，单键映射 `颜色: [分支步骤]`；click 语义同
/// match 候选，命中后点击取样点）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ColorBranch {
    pub color: Cell,
    pub click: bool,
    pub steps: Vec<Step>,
}

/// 步骤十八类。分支子列表递归为 `Vec<Step>`；空分支/默认字段在 AST 中
/// 显式存在（序列化规范 YAML 时按契约省略）。
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    StrApp,
    ClsApp,
    Tap {
        at: Cell,
    },
    Swipe {
        from: Cell,
        to: Cell,
        time: Cell,
    },
    Key {
        key: Cell,
    },
    Text {
        value: Cell,
    },
    Log {
        message: Cell,
    },
    Wait {
        duration: Cell,
        /// 随机区间上界（`- wait: [1s, 3s]`）；定值形式为 `None`。
        duration_max: Option<Cell>,
    },
    Find {
        template: Cell,
        block: Vec<Cell>,
        verify: bool,
        timeout: Option<Cell>,
        then: Vec<Step>,
        r#else: Vec<Step>,
    },
    Match {
        candidates: Vec<MatchCandidate>,
        r#else: Vec<Step>,
        timeout: Option<Cell>,
    },
    /// check：单帧匹配模板（不点击、不轮询、无分支），未命中按 throw 文案结束运行。
    Check {
        template: Cell,
        /// 未命中时的终止原因（必填，loader 保证非空）。
        r#throw: String,
    },
    Color {
        at: Cell,
        expect: Vec<ColorBranch>,
        r#else: Vec<Step>,
    },
    If {
        cond: Cell,
        then: Vec<Step>,
        r#else: Vec<Step>,
    },
    Loop {
        /// `None` = 无限循环（times 省略）。
        times: Option<u64>,
        steps: Vec<Step>,
    },
    Call {
        target: String,
        args: Vec<ArgAssign>,
    },
    Func {
        target: String,
        args: Vec<ArgAssign>,
        then: Vec<Step>,
        r#else: Vec<Step>,
    },
    Throw {
        /// 裸写 `- throw` 为 `None`。
        message: Option<String>,
    },
    Return {
        value: Cell,
    },
}

/// args 具名实参：保留书写顺序，并保留源标量的引号样式（规范 YAML 原样回写：
/// 文本实参 `"字面量消息"` 保持双引号，`30s` 等保持裸写）。
#[derive(Debug, Clone, PartialEq)]
pub struct ArgAssign {
    pub name: String,
    pub value: Cell,
    /// 标量实参在源 YAML 中是否带引号（引用/序列实参忽略该标记）。
    pub quoted: bool,
}

/// args 实参列表的序列化视图（Model JSON：`{"name": {"lit"/"ref": …}}`）。
#[derive(Debug, Clone, PartialEq)]
pub struct ArgsRef<'a>(pub &'a [ArgAssign]);

impl<'a> Serialize for ArgsRef<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for a in self.0 {
            map.serialize_entry(&a.name, &a.value)?;
        }
        map.end()
    }
}

impl Step {
    /// API JSON 的 `kind` 判别名。
    pub fn kind(&self) -> &'static str {
        match self {
            Step::StrApp => "str_app",
            Step::ClsApp => "cls_app",
            Step::Tap { .. } => "tap",
            Step::Swipe { .. } => "swipe",
            Step::Key { .. } => "key",
            Step::Text { .. } => "text",
            Step::Log { .. } => "log",
            Step::Wait { .. } => "wait",
            Step::Find { .. } => "find",
            Step::Match { .. } => "match",
            Step::Check { .. } => "check",
            Step::Color { .. } => "color",
            Step::If { .. } => "if",
            Step::Loop { .. } => "loop",
            Step::Call { .. } => "call",
            Step::Func { .. } => "func",
            Step::Throw { .. } => "throw",
            Step::Return { .. } => "return",
        }
    }
}

impl Serialize for Step {
    /// Model/API JSON：`{"kind": …}` + 分支字段；空列表/None 照实输出
    /// （Model 中显式存在空列表，Option → null）。
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("kind", self.kind())?;
        match self {
            Step::StrApp | Step::ClsApp => {}
            Step::Tap { at } => map.serialize_entry("at", at)?,
            Step::Swipe { from, to, time } => {
                map.serialize_entry("from", from)?;
                map.serialize_entry("to", to)?;
                map.serialize_entry("time", time)?;
            }
            Step::Key { key } => map.serialize_entry("key", key)?,
            Step::Text { value } => map.serialize_entry("value", value)?,
            Step::Log { message } => map.serialize_entry("message", message)?,
            Step::Wait {
                duration,
                duration_max,
            } => {
                map.serialize_entry("duration", duration)?;
                map.serialize_entry("duration_max", duration_max)?;
            }
            Step::Find {
                template,
                block,
                verify,
                timeout,
                then,
                r#else,
            } => {
                map.serialize_entry("template", template)?;
                map.serialize_entry("block", block)?;
                map.serialize_entry("verify", verify)?;
                map.serialize_entry("timeout", timeout)?;
                map.serialize_entry("then", then)?;
                map.serialize_entry("else", r#else)?;
            }
            Step::Match {
                candidates,
                r#else,
                timeout,
            } => {
                map.serialize_entry("candidates", candidates)?;
                map.serialize_entry("else", r#else)?;
                map.serialize_entry("timeout", timeout)?;
            }
            Step::Check { template, r#throw } => {
                map.serialize_entry("template", template)?;
                map.serialize_entry("throw", r#throw)?;
            }
            Step::Color { at, expect, r#else } => {
                map.serialize_entry("at", at)?;
                map.serialize_entry("expect", expect)?;
                map.serialize_entry("else", r#else)?;
            }
            Step::If { cond, then, r#else } => {
                map.serialize_entry("cond", cond)?;
                map.serialize_entry("then", then)?;
                map.serialize_entry("else", r#else)?;
            }
            Step::Loop { times, steps } => {
                map.serialize_entry("times", times)?;
                map.serialize_entry("steps", steps)?;
            }
            Step::Call { target, args } => {
                map.serialize_entry("target", target)?;
                map.serialize_entry("args", &ArgsRef(args))?;
            }
            Step::Func {
                target,
                args,
                then,
                r#else,
            } => {
                map.serialize_entry("target", target)?;
                map.serialize_entry("args", &ArgsRef(args))?;
                map.serialize_entry("then", then)?;
                map.serialize_entry("else", r#else)?;
            }
            Step::Throw { message } => map.serialize_entry("message", message)?,
            Step::Return { value } => map.serialize_entry("value", value)?,
        }
        map.end()
    }
}

// ---------------------------------------------------------------------------
// 文件级模型
// ---------------------------------------------------------------------------

/// 脚本运行配置（整体省略 = 使用 config.toml 运行时默认）。
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptConfig {
    pub interval: Duration,
    pub threshold: f64,
    pub log_level: LogLevel,
}

impl Serialize for ScriptConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("ScriptConfig", 3)?;
        st.serialize_field("interval", &params::fmt_duration(&self.interval))?;
        st.serialize_field("threshold", &self.threshold)?;
        st.serialize_field("log_level", &self.log_level)?;
        st.end()
    }
}

/// 日志等级（CONTRACT §3.6，success 视同 info 的 v1 语义不再存在）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "debug" => LogLevel::Debug,
            "info" => LogLevel::Info,
            "warn" => LogLevel::Warn,
            "error" => LogLevel::Error,
            _ => return None,
        })
    }
}

impl Serialize for LogLevel {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// 可执行脚本（yaml/）：顶层只允许 params/config/steps。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScriptFile {
    pub params: Vec<ParamDecl>,
    /// `None` = 未配置（运行时取 config.toml 同名键）。
    pub config: Option<ScriptConfig>,
    pub steps: Vec<Step>,
}

/// 函数声明（func/ 文件中的一个顶层记录）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FunctionDecl {
    pub name: String,
    pub params: Vec<ParamDecl>,
    pub steps: Vec<Step>,
}

/// 函数库文件（func/）：顶层键 = 函数名，保持书写顺序；无文件级 config。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FunctionFile {
    pub functions: Vec<FunctionDecl>,
}

impl FunctionFile {
    pub fn find(&self, name: &str) -> Option<&FunctionDecl> {
        self.functions.iter().find(|f| f.name == name)
    }
}

// ---------------------------------------------------------------------------
// 任务参数签名（CONTRACT §4.5 psig1，冻结算法）
// ---------------------------------------------------------------------------

/// 按声明顺序覆盖类型/名称/必填性/默认值的规范化签名串，用于任务过期检测。
pub fn param_signature(params: &[ParamDecl]) -> String {
    let entries: Vec<String> = params.iter().map(canonical_param_entry).collect();
    format!("psig1|{}", entries.join("|"))
}

fn canonical_param_entry(p: &ParamDecl) -> String {
    let (required, canon) = match &p.default {
        None => ("1", String::new()),
        Some(v) => ("0", canonical_default_value(&p.ty, v)),
    };
    format!("{},{},{},{}", p.ty.as_str(), p.name, required, canon)
}

fn canonical_default_value(ty: &ParamType, v: &TypedValue) -> String {
    match (ty, v) {
        (ParamType::Bool, TypedValue::Bool(b)) => b.to_string(),
        (ParamType::Coord, TypedValue::Coord([x, y])) => {
            format!("[{},{}]", params::fmt_num(*x), params::fmt_num(*y))
        }
        (ParamType::Color, TypedValue::Color(s)) => s.to_ascii_lowercase(),
        (ParamType::Key, TypedValue::Key(s)) => s.to_ascii_uppercase(),
        (ParamType::Time, TypedValue::Time(s)) => {
            // 小写；"min" 归一为 "m"；数值保持书写形式。
            let lower = s.to_ascii_lowercase();
            match lower.strip_suffix("min") {
                Some(num) => format!("{num}m"),
                None => lower,
            }
        }
        (ParamType::Text, TypedValue::Text(s)) => s
            .replace('\\', "\\\\")
            .replace(',', "\\,")
            .replace('|', "\\|"),
        (ParamType::Tmpl, TypedValue::Tmpl(s)) => s.clone(),
        // 校验保证不会到达；宽裕处理为原串避免 panic。
        _ => String::new(),
    }
}
