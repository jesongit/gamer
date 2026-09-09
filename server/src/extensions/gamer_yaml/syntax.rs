//! YAML V1 纯数据前端（计划 Phase 1/2：解析 → 校验 → 解释器 wire）。
//!
//! 本模块故意不依赖任何设备实现或存储视图。职责：
//!
//! - `parse_script` / `parse_function_library`：V1 surface 解析与结构校验
//!   （顶层 `name/params/vars/run`；步骤 = 函数调用 / `if` / `repeat` /
//!   `return`；表达式 = 字面量 / `$name.field`）；
//! - `build_program`：surface → [`yaml_interp`] wire 程序（步路径 + 中文
//!   desc 在此生成，事件与前端卡片寻址共用同一语法）；
//! - `collect_called_functions`：调用面收集（运行前/校验函数存在性）;
//! - `rename_template_source` / `rename_template_in_function_library`：
//!   模板重命名时的引用同步改写（AST 改写，非文本替换）。
//!
//! 执行权威在 `yaml-interp`（WASM guest 与测试同源），Core 不认识本模块。

use std::collections::BTreeSet;
use std::fmt;

use serde::Serialize;
use serde_json::{json, Map as JsonMap, Value};
use serde_yaml::{Mapping, Value as YamlValue};

/// 当前唯一支持的语法版本标识（文档/草稿来源标注用；V1 源内没有 version 字段）。
pub const SYNTAX_V1: u64 = 1;

/// 用户可见的解析/校验诊断。路径用稳定点号形态（`run[0].then[1]`），前端
/// raw/visual 编辑器展示同一诊断。
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub path: String,
    pub message: String,
}

impl Diagnostic {
    pub fn new(code: &str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}: {}", self.code, self.path, self.message)
    }
}

fn one_diagnostic(code: &str, path: &str, message: impl Into<String>) -> Vec<Diagnostic> {
    vec![Diagnostic::new(code, path, message)]
}

// ---------------------------------------------------------------------------
// 参数 Schema（脚本 params / 函数 params / entrypoint 描述共同来源）
// ---------------------------------------------------------------------------

/// V1 参数类型：基础七类 + 领域类型（绑定/派发时显式解析，解释器不感知单位）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamType {
    Any,
    Boolean,
    Integer,
    Number,
    String,
    List,
    Object,
    Duration,
    Point,
    Template,
    Key,
}

pub const PARAM_TYPE_NAMES: &[(&str, ParamType)] = &[
    ("any", ParamType::Any),
    ("boolean", ParamType::Boolean),
    ("bool", ParamType::Boolean),
    ("integer", ParamType::Integer),
    ("int", ParamType::Integer),
    ("number", ParamType::Number),
    ("float", ParamType::Number),
    ("string", ParamType::String),
    ("text", ParamType::String),
    ("list", ParamType::List),
    ("object", ParamType::Object),
    ("duration", ParamType::Duration),
    ("point", ParamType::Point),
    ("template", ParamType::Template),
    ("key", ParamType::Key),
];

impl ParamType {
    pub fn parse(name: &str) -> Option<Self> {
        let lower = name.trim().to_ascii_lowercase();
        PARAM_TYPE_NAMES
            .iter()
            .find(|(alias, _)| *alias == lower)
            .map(|(_, ty)| *ty)
    }

    /// 规范名（descriptor schema 输出用）。
    pub fn canonical(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::String => "string",
            Self::List => "list",
            Self::Object => "object",
            Self::Duration => "duration",
            Self::Point => "point",
            Self::Template => "template",
            Self::Key => "key",
        }
    }
}

impl Serialize for ParamType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.canonical())
    }
}

/// 参数声明（函数 Schema：类型、必填、默认值、说明——计划 Phase 3.2）。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParamDecl {
    pub name: String,
    pub ty: ParamType,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
}

// ---------------------------------------------------------------------------
// Surface 模型
// ---------------------------------------------------------------------------

/// 表达式：字面量或 `$name.field` 引用；容器叶子可含引用。
#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceExpr {
    Lit(Value),
    Ref(String),
    List(Vec<SurfaceExpr>),
    Map(Vec<(String, SurfaceExpr)>),
}

impl SurfaceExpr {
    /// wire JSON 形态（`{"expr":"lit","value":…}` / `{"expr":"ref","path":…}`）。
    pub fn to_wire(&self) -> Value {
        match self {
            Self::Lit(value) => json!({ "expr": "lit", "value": value }),
            Self::Ref(path) => json!({ "expr": "ref", "path": path }),
            Self::List(items) => json!({
                "expr": "list",
                "value": items.iter().map(Self::to_wire).collect::<Vec<_>>(),
            }),
            Self::Map(entries) => json!({
                "expr": "map",
                "value": entries
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_wire()))
                    .collect::<serde_json::Map<String, Value>>(),
            }),
        }
    }

    /// 条件/描述渲染。
    fn describe(&self) -> String {
        match self {
            Self::Lit(value) => compact_json(value),
            Self::Ref(path) => format!("${path}"),
            Self::List(items) => format!(
                "[{}]",
                items
                    .iter()
                    .map(Self::describe)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Map(entries) => format!(
                "{{{}}}",
                entries
                    .iter()
                    .map(|(key, value)| format!("{key}: {}", value.describe()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// 收集表达式引用到的全部顶层变量名。
    fn collect_refs(&self, out: &mut BTreeSet<String>) {
        match self {
            Self::Lit(_) => {}
            Self::Ref(path) => {
                if let Some((head, _)) = path.split_once('.') {
                    out.insert(head.to_string());
                } else {
                    out.insert(path.clone());
                }
            }
            Self::List(items) => items.iter().for_each(|item| item.collect_refs(out)),
            Self::Map(entries) => entries
                .iter()
                .for_each(|(_, value)| value.collect_refs(out)),
        }
    }
}

/// 步骤（解析后、未降线）。
#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceStep {
    Call {
        name: String,
        args: SurfaceExpr,
        save_as: Option<String>,
    },
    If {
        cond: SurfaceExpr,
        then_steps: Vec<SurfaceStep>,
        else_steps: Vec<SurfaceStep>,
    },
    Repeat {
        times: SurfaceExpr,
        body: Vec<SurfaceStep>,
    },
    Return {
        value: SurfaceExpr,
    },
}

impl SurfaceStep {
    /// 收集本步（含子步）调用的函数名与引用的变量。
    fn collect(&self, calls: &mut BTreeSet<String>, refs: &mut BTreeSet<String>) {
        match self {
            Self::Call { name, args, .. } => {
                calls.insert(name.clone());
                args.collect_refs(refs);
            }
            Self::If {
                cond,
                then_steps,
                else_steps,
            } => {
                cond.collect_refs(refs);
                for step in then_steps.iter().chain(else_steps) {
                    step.collect(calls, refs);
                }
            }
            Self::Repeat { times, body } => {
                times.collect_refs(refs);
                for step in body {
                    step.collect(calls, refs);
                }
            }
            Self::Return { value } => value.collect_refs(refs),
        }
    }
}

/// 脚本文档（`automations/`）。
#[derive(Clone, Debug)]
pub struct Script {
    pub name: Option<String>,
    pub params: Vec<ParamDecl>,
    /// vars：字面量表（不做引用解析）。
    pub vars: Vec<(String, Value)>,
    pub run: Vec<SurfaceStep>,
}

/// 函数定义（`automations/_function*.yaml` 函数库内单个函数，简化计划 Phase 1）。
#[derive(Clone, Debug)]
pub struct FunctionDef {
    pub description: Option<String>,
    pub params: Vec<ParamDecl>,
    pub vars: Vec<(String, Value)>,
    pub returns: Option<Value>,
    pub run: Vec<SurfaceStep>,
}

/// 函数库文件解析结果（保持文件内声明顺序；「第一个函数」缺省语义依赖它）。
pub type FunctionLibrary = Vec<(String, FunctionDef)>;

impl Script {
    /// 收集脚本调用的全部函数名。
    pub fn called_functions(&self) -> BTreeSet<String> {
        let mut calls = BTreeSet::new();
        let mut refs = BTreeSet::new();
        for step in &self.run {
            step.collect(&mut calls, &mut refs);
        }
        calls
    }

    /// 收集脚本引用的全部变量名（参数存在性快速校验用）。
    pub fn referenced_vars(&self) -> BTreeSet<String> {
        let mut calls = BTreeSet::new();
        let mut refs = BTreeSet::new();
        for step in &self.run {
            step.collect(&mut calls, &mut refs);
        }
        refs
    }
}

impl FunctionDef {
    pub fn called_functions(&self) -> BTreeSet<String> {
        let mut calls = BTreeSet::new();
        let mut refs = BTreeSet::new();
        for step in &self.run {
            step.collect(&mut calls, &mut refs);
        }
        calls
    }
}

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------

fn yaml_to_diagnostic(error: serde_yaml::Error) -> Vec<Diagnostic> {
    vec![Diagnostic::new(
        "yaml.syntax_error",
        "",
        format!("YAML 解析失败: {error}"),
    )]
}

/// 标识符规则：小写字母/下划线开头，仅小写字母、数字、下划线（函数名、
/// 参数名、变量名共用；禁点号保证 step path 无歧义）。
pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// 控制流关键字，不能作为函数名。
const RESERVED_WORDS: &[&str] = &["if", "repeat", "return"];

pub fn is_reserved(name: &str) -> bool {
    RESERVED_WORDS.contains(&name)
}

fn valid_ref_path(path: &str) -> bool {
    let mut segments = path.split('.');
    let head = segments.next().unwrap_or_default();
    if !is_identifier(head) {
        return false;
    }
    segments.all(is_identifier)
}

/// `$` 前缀字符串的解析：`$name.field` → 引用；`$$text` → 字面量 `$text`；
/// 其余 → 字面量。
fn expr_from_yaml(value: &YamlValue, path: &str) -> Result<SurfaceExpr, Vec<Diagnostic>> {
    match value {
        YamlValue::String(text) => {
            let Some(rest) = text.strip_prefix('$') else {
                return Ok(SurfaceExpr::Lit(Value::String(text.clone())));
            };
            if let Some(escaped) = rest.strip_prefix('$') {
                return Ok(SurfaceExpr::Lit(Value::String(format!("${escaped}"))));
            }
            if rest.is_empty() {
                return Ok(SurfaceExpr::Lit(Value::String(text.clone())));
            }
            if !valid_ref_path(rest) {
                return Err(one_diagnostic(
                    "yaml.expr.invalid",
                    path,
                    format!("非法变量引用 {text:?}——V1 只支持 $name 与 $name.field（$name 段为小写标识符）；字面量 $ 用 $$ 转义"),
                ));
            }
            Ok(SurfaceExpr::Ref(rest.to_string()))
        }
        YamlValue::Sequence(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                out.push(expr_from_yaml(item, &format!("{path}[{index}]"))?);
            }
            Ok(SurfaceExpr::List(out))
        }
        YamlValue::Mapping(map) => {
            let mut out = Vec::with_capacity(map.len());
            for (key, item) in map {
                let Some(key) = key.as_str() else {
                    return Err(one_diagnostic(
                        "yaml.expr.invalid",
                        path,
                        "映射参数键必须是字符串",
                    ));
                };
                out.push((
                    key.to_string(),
                    expr_from_yaml(item, &format!("{path}.{key}"))?,
                ));
            }
            Ok(SurfaceExpr::Map(out))
        }
        YamlValue::Null => Ok(SurfaceExpr::Lit(Value::Null)),
        YamlValue::Bool(value) => Ok(SurfaceExpr::Lit(Value::Bool(*value))),
        YamlValue::Number(number) => {
            let value = serde_json::to_value(number).map_err(|error| {
                one_diagnostic("yaml.expr.invalid", path, format!("数字无法表示: {error}"))
            })?;
            Ok(SurfaceExpr::Lit(value))
        }
        YamlValue::Tagged(tagged) => Err(one_diagnostic(
            "yaml.expr.invalid",
            path,
            format!("不支持 YAML 标签 !{}", tagged.tag),
        )),
    }
}

fn yaml_scalar_to_json(value: &YamlValue, path: &str) -> Result<Value, Vec<Diagnostic>> {
    match value {
        YamlValue::String(text) => Ok(Value::String(text.clone())),
        YamlValue::Bool(value) => Ok(Value::Bool(*value)),
        YamlValue::Number(number) => serde_json::to_value(number).map_err(|error| {
            one_diagnostic("yaml.expr.invalid", path, format!("数字无法表示: {error}"))
        }),
        YamlValue::Null => Ok(Value::Null),
        YamlValue::Sequence(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                out.push(yaml_scalar_to_json(item, &format!("{path}[{index}]"))?);
            }
            Ok(Value::Array(out))
        }
        YamlValue::Mapping(map) => {
            let mut out = JsonMap::new();
            for (key, item) in map {
                let Some(key) = key.as_str() else {
                    return Err(one_diagnostic(
                        "yaml.expr.invalid",
                        path,
                        "映射键必须是字符串",
                    ));
                };
                out.insert(
                    key.to_string(),
                    yaml_scalar_to_json(item, &format!("{path}.{key}"))?,
                );
            }
            Ok(Value::Object(out))
        }
        YamlValue::Tagged(tagged) => Err(one_diagnostic(
            "yaml.expr.invalid",
            path,
            format!("不支持 YAML 标签 !{}", tagged.tag),
        )),
    }
}

/// 关键字集合。
const KEYWORDS: &[&str] = &["if", "repeat", "return"];

fn parse_steps(steps: &YamlValue, path: &str) -> Result<Vec<SurfaceStep>, Vec<Diagnostic>> {
    let YamlValue::Sequence(items) = steps else {
        return Err(one_diagnostic(
            "yaml.step.list",
            path,
            "步骤必须是列表（- 开头）",
        ));
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        out.push(parse_step(item, &format!("{path}[{index}]"))?);
    }
    Ok(out)
}

/// 单步解析：恰好一个动作键（函数名或 if/repeat/return），`as` 为修饰字段。
fn parse_step(step: &YamlValue, path: &str) -> Result<SurfaceStep, Vec<Diagnostic>> {
    let YamlValue::Mapping(mapping) = step else {
        return Err(one_diagnostic(
            "yaml.step.shape",
            path,
            "步骤必须是映射（如 `- tap: [0.5, 0.5]`）",
        ));
    };
    if mapping.is_empty() {
        return Err(one_diagnostic(
            "yaml.step.missing",
            path,
            "步骤为空——需要一个函数调用或 if/repeat/return",
        ));
    }

    let mut keyword: Option<(&str, &YamlValue)> = None;
    let mut action: Option<(String, &YamlValue)> = None;
    let mut save_as: Option<String> = None;
    let mut extra_keys: Vec<String> = Vec::new();
    // if/repeat 的结构键（then/else/do）：由对应关键字分支消费；
    // 出现在无关键字步骤里 = 结构错误。
    let mut structural: Vec<&str> = Vec::new();

    for (key, value) in mapping {
        let Some(key) = key.as_str() else {
            extra_keys.push(format!("{key:?}"));
            continue;
        };
        match key {
            "as" => {
                let Some(name) = value.as_str() else {
                    return Err(one_diagnostic(
                        "yaml.as.invalid",
                        path,
                        "as 必须是变量名（小写标识符）",
                    ));
                };
                if !is_identifier(name) {
                    return Err(one_diagnostic(
                        "yaml.as.invalid",
                        path,
                        format!("as 变量名 {name:?} 非法——只允许小写字母、数字、下划线"),
                    ));
                }
                save_as = Some(name.to_string());
            }
            word if KEYWORDS.contains(&word) => {
                if keyword.is_some() {
                    return Err(one_diagnostic(
                        "yaml.step.multi",
                        path,
                        "一个步骤只能有一个控制流关键字（if/repeat/return）",
                    ));
                }
                keyword = Some((word, value));
            }
            "then" | "else" | "do" => structural.push(key),
            other => {
                if action.is_some() {
                    return Err(one_diagnostic(
                        "yaml.step.multi",
                        path,
                        format!("一个步骤只能有一个函数调用（本步同时出现 {other} 等）"),
                    ));
                }
                action = Some((other.to_string(), value));
            }
        }
    }

    if !extra_keys.is_empty() {
        return Err(one_diagnostic(
            "yaml.step.shape",
            path,
            format!("步骤键必须是非空字符串，得到 {extra_keys:?}"),
        ));
    }

    if !structural.is_empty() && keyword.is_none() {
        return Err(one_diagnostic(
            "yaml.step.multi",
            path,
            format!(
                "then/else/do 只能在 if/repeat 步骤内使用（本步出现了 {}）",
                structural
                    .iter()
                    .map(|key| format!("{key:?}"))
                    .collect::<Vec<_>>()
                    .join("、")
            ),
        ));
    }

    let as_rejected = |kind: &str| -> Vec<Diagnostic> {
        one_diagnostic(
            "yaml.as.invalid",
            path,
            format!("{kind} 步骤不支持 as（只有函数调用有返回值）"),
        )
    };

    match (keyword, action) {
        (Some((word, value)), None) => match word {
            "if" => {
                if save_as.is_some() {
                    return Err(as_rejected("if"));
                }
                let cond = expr_from_yaml(value, &format!("{path}.if"))?;
                let then_steps = match mapping.get(YamlValue::String("then".into())) {
                    Some(steps) => parse_steps(steps, &format!("{path}.then"))?,
                    None => {
                        return Err(one_diagnostic(
                            "yaml.if.then",
                            path,
                            "if 步骤缺少 then 分支",
                        ))
                    }
                };
                let else_steps = match mapping.get(YamlValue::String("else".into())) {
                    Some(steps) => parse_steps(steps, &format!("{path}.else"))?,
                    None => Vec::new(),
                };
                Ok(SurfaceStep::If {
                    cond,
                    then_steps,
                    else_steps,
                })
            }
            "repeat" => {
                if save_as.is_some() {
                    return Err(as_rejected("repeat"));
                }
                let times = expr_from_yaml(value, &format!("{path}.repeat"))?;
                if let SurfaceExpr::Lit(Value::Number(number)) = &times {
                    if number
                        .as_f64()
                        .is_none_or(|value| value < 0.0 || value.fract() != 0.0)
                    {
                        return Err(one_diagnostic(
                            "yaml.repeat.times",
                            path,
                            "repeat 次数必须是零或正整数",
                        ));
                    }
                }
                let body = match mapping.get(YamlValue::String("do".into())) {
                    Some(steps) => parse_steps(steps, &format!("{path}.do"))?,
                    None => {
                        return Err(one_diagnostic(
                            "yaml.repeat.do",
                            path,
                            "repeat 步骤缺少 do 循环体",
                        ))
                    }
                };
                Ok(SurfaceStep::Repeat { times, body })
            }
            "return" => {
                if save_as.is_some() {
                    return Err(as_rejected("return"));
                }
                let value = expr_from_yaml(value, &format!("{path}.return"))?;
                Ok(SurfaceStep::Return { value })
            }
            other => Err(one_diagnostic(
                "yaml.step.unknown",
                path,
                format!("未知控制流关键字 {other}"),
            )),
        },
        (None, Some((name, value))) => {
            if !is_identifier(&name) {
                return Err(one_diagnostic(
                    "yaml.name.invalid",
                    path,
                    format!(
                        "函数名 {name:?} 非法——只允许小写字母、数字、下划线（如 tap、wait_find）"
                    ),
                ));
            }
            let args = match value {
                YamlValue::Null => SurfaceExpr::Map(Vec::new()),
                other => expr_from_yaml(other, &format!("{path}.{name}"))?,
            };
            Ok(SurfaceStep::Call {
                name,
                args,
                save_as,
            })
        }
        (Some(_), Some((name, _))) => Err(one_diagnostic(
            "yaml.step.multi",
            path,
            format!("控制流关键字不能与函数调用 {name} 同时出现在一步"),
        )),
        (None, None) => Err(one_diagnostic(
            "yaml.step.missing",
            path,
            "步骤只有 as——需要一个函数调用或 if/repeat/return",
        )),
    }
}

fn parse_param_name_name(name: &str, path: &str) -> Result<(), Vec<Diagnostic>> {
    if is_identifier(name) {
        Ok(())
    } else {
        Err(one_diagnostic(
            "yaml.name.invalid",
            path,
            format!("参数名 {name:?} 非法——只允许小写字母、数字、下划线"),
        ))
    }
}

/// params 解析：`<名>: {type, required?, default?, desc?}`。
fn parse_params(value: &YamlValue, path: &str) -> Result<Vec<ParamDecl>, Vec<Diagnostic>> {
    let YamlValue::Mapping(mapping) = value else {
        return Err(one_diagnostic(
            "yaml.param.decl",
            path,
            "params 必须是映射（参数名 → 声明）",
        ));
    };
    let mut out = Vec::with_capacity(mapping.len());
    for (key, decl) in mapping {
        let Some(name) = key.as_str() else {
            return Err(one_diagnostic(
                "yaml.param.decl",
                path,
                "参数名必须是字符串",
            ));
        };
        parse_param_name_name(name, path)?;
        let YamlValue::Mapping(fields) = decl else {
            return Err(one_diagnostic(
                "yaml.param.decl",
                path,
                format!("参数 {name} 的声明必须是映射（type/required/default/desc）"),
            ));
        };
        let mut ty: Option<ParamType> = None;
        let mut required = false;
        let mut default: Option<Value> = None;
        let mut desc: Option<String> = None;
        for (field_key, field_value) in fields.iter() {
            let Some(field_key) = field_key.as_str() else {
                return Err(one_diagnostic(
                    "yaml.param.decl",
                    path,
                    format!("参数 {name} 声明键必须是字符串"),
                ));
            };
            match field_key {
                "type" => {
                    let Some(name_text) = field_value.as_str() else {
                        return Err(one_diagnostic(
                            "yaml.param.decl",
                            path,
                            format!("参数 {name} 的 type 必须是字符串"),
                        ));
                    };
                    ty = Some(ParamType::parse(name_text).ok_or_else(|| {
                        one_diagnostic(
                            "yaml.param.decl",
                            path,
                            format!(
                                "参数 {name} 的未知类型 {name_text:?}——支持 {}",
                                "any/boolean/integer/number/string/list/object/duration/point/template/key"
                            ),
                        )
                    })?);
                }
                "required" => {
                    required = field_value.as_bool().ok_or_else(|| {
                        one_diagnostic(
                            "yaml.param.decl",
                            path,
                            format!("参数 {name} 的 required 必须是布尔值"),
                        )
                    })?;
                }
                "default" => {
                    default = Some(yaml_scalar_to_json(field_value, &format!("{path}.{name}"))?);
                }
                "desc" => {
                    desc = Some(field_value.as_str().unwrap_or_default().to_string());
                }
                other => {
                    return Err(one_diagnostic(
                        "yaml.param.decl",
                        path,
                        format!(
                            "参数 {name} 声明不支持字段 {other:?}（type/required/default/desc）"
                        ),
                    ));
                }
            }
        }
        let Some(ty) = ty else {
            return Err(one_diagnostic(
                "yaml.param.decl",
                path,
                format!("参数 {name} 缺少 type"),
            ));
        };
        if let Some(default) = &default {
            check_type(ty, default).map_err(|message| {
                one_diagnostic(
                    "yaml.param.default.invalid",
                    path,
                    format!(
                        "参数 {name} 默认值与类型 {} 不符: {message}",
                        ty.canonical()
                    ),
                )
            })?;
        }
        out.push(ParamDecl {
            name: name.to_string(),
            ty,
            required,
            default,
            desc,
        });
    }
    Ok(out)
}

/// vars 解析：`<名>: 字面量`（不解析引用）。
fn parse_vars(value: &YamlValue, path: &str) -> Result<Vec<(String, Value)>, Vec<Diagnostic>> {
    let YamlValue::Mapping(mapping) = value else {
        return Err(one_diagnostic(
            "yaml.vars.shape",
            path,
            "vars 必须是映射（变量名 → 字面量）",
        ));
    };
    let mut out = Vec::with_capacity(mapping.len());
    for (key, item) in mapping {
        let Some(name) = key.as_str() else {
            return Err(one_diagnostic(
                "yaml.vars.shape",
                path,
                "变量名必须是字符串",
            ));
        };
        if !is_identifier(name) {
            return Err(one_diagnostic(
                "yaml.name.invalid",
                path,
                format!("变量名 {name:?} 非法——只允许小写字母、数字、下划线"),
            ));
        }
        out.push((
            name.to_string(),
            yaml_scalar_to_json(item, &format!("{path}.{name}"))?,
        ));
    }
    Ok(out)
}

/// 脚本解析：顶层 `name?` / `params?` / `vars?` / `run`。
pub fn parse_script(source: &str) -> Result<Script, Vec<Diagnostic>> {
    let doc: YamlValue = serde_yaml::from_str(source).map_err(yaml_to_diagnostic)?;
    let YamlValue::Mapping(mapping) = &doc else {
        return Err(one_diagnostic(
            "yaml.top.shape",
            "",
            "脚本顶层必须是映射（name/params/vars/run）",
        ));
    };

    for key in mapping.keys() {
        if let Some(key) = key.as_str() {
            if key == "version" {
                return Err(one_diagnostic(
                    "yaml.version.removed",
                    "",
                    "V1 语法不再使用 version 字段——请删除该行（旧 v3 脚本不兼容，需按新语法重写）",
                ));
            }
        }
    }

    let mut name: Option<String> = None;
    let mut params: Vec<ParamDecl> = Vec::new();
    let mut vars: Vec<(String, Value)> = Vec::new();
    let mut run: Option<Vec<SurfaceStep>> = None;

    for (key, value) in mapping {
        let Some(key) = key.as_str() else {
            return Err(one_diagnostic("yaml.top.shape", "", "顶层键必须是字符串"));
        };
        match key {
            "name" => {
                name = Some(value.as_str().unwrap_or_default().to_string());
            }
            "params" => params = parse_params(value, "params")?,
            "vars" => vars = parse_vars(value, "vars")?,
            "run" => run = Some(parse_steps(value, "run")?),
            other => {
                return Err(one_diagnostic(
                    "yaml.top.unknown",
                    "",
                    format!("未知顶层字段 {other:?}——V1 只支持 name/params/vars/run"),
                ));
            }
        }
    }

    let param_names: BTreeSet<_> = params.iter().map(|decl| decl.name.clone()).collect();
    for (var_name, _) in &vars {
        if param_names.contains(var_name) {
            return Err(one_diagnostic(
                "yaml.vars.conflict",
                "vars",
                format!("变量 {var_name} 与参数同名——参数与 vars 不得重名"),
            ));
        }
    }

    Ok(Script {
        name,
        params,
        vars,
        run: run.unwrap_or_default(),
    })
}

/// 函数库文件解析：顶层 `functions: {<名>: {description?, params?, vars?,
/// returns?, run}}`。
pub fn parse_function_library(source: &str) -> Result<FunctionLibrary, Vec<Diagnostic>> {
    let doc: YamlValue = serde_yaml::from_str(source).map_err(yaml_to_diagnostic)?;
    let YamlValue::Mapping(mapping) = &doc else {
        return Err(one_diagnostic(
            "yaml.top.shape",
            "",
            "函数文件顶层必须是映射（functions: {<函数名>: …}）",
        ));
    };
    let Some(functions) = mapping.get(YamlValue::String("functions".into())) else {
        let hint = if mapping.keys().any(|key| {
            key.as_str()
                .is_some_and(|key| key == "run" || key == "params")
        }) {
            "（顶层看起来是单个函数定义——需要包在 functions: 下）"
        } else {
            ""
        };
        return Err(one_diagnostic(
            "yaml.functions.missing",
            "",
            format!("函数文件缺少 functions: 顶层包装{hint}"),
        ));
    };
    let YamlValue::Mapping(functions) = functions else {
        return Err(one_diagnostic(
            "yaml.functions.shape",
            "functions",
            "functions 必须是映射（函数名 → 定义）",
        ));
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: FunctionLibrary = Vec::with_capacity(functions.len());
    for (key, def) in functions {
        let Some(name) = key.as_str() else {
            return Err(one_diagnostic(
                "yaml.functions.shape",
                "functions",
                "函数名必须是字符串",
            ));
        };
        if !is_identifier(name) {
            return Err(one_diagnostic(
                "yaml.name.invalid",
                "functions",
                format!("函数名 {name:?} 非法——只允许小写字母、数字、下划线（如 claim_daily）"),
            ));
        }
        if is_reserved(name) {
            return Err(one_diagnostic(
                "yaml.name.invalid",
                "functions",
                format!("函数名 {name} 是保留关键字（if/repeat/return）"),
            ));
        }
        if !seen.insert(name.to_string()) {
            return Err(one_diagnostic(
                "yaml.fn.duplicate",
                "functions",
                format!("函数 {name} 在同一文件中重复定义"),
            ));
        }
        let YamlValue::Mapping(fields) = def else {
            return Err(one_diagnostic(
                "yaml.functions.shape",
                "functions",
                format!("函数 {name} 的定义必须是映射"),
            ));
        };
        let mut description: Option<String> = None;
        let mut params: Vec<ParamDecl> = Vec::new();
        let mut vars: Vec<(String, Value)> = Vec::new();
        let mut returns: Option<Value> = None;
        let mut run: Option<Vec<SurfaceStep>> = None;
        for (field_key, field_value) in fields.iter() {
            let Some(field_key) = field_key.as_str() else {
                return Err(one_diagnostic(
                    "yaml.functions.shape",
                    "functions",
                    format!("函数 {name} 定义键必须是字符串"),
                ));
            };
            match field_key {
                "description" => {
                    description = Some(field_value.as_str().unwrap_or_default().to_string());
                }
                "params" => {
                    params = parse_params(field_value, &format!("functions.{name}.params"))?
                }
                "vars" => vars = parse_vars(field_value, &format!("functions.{name}.vars"))?,
                // returns 目前仅作文档/schema 提示，不做运行时校验。
                "returns" => {
                    returns = Some(yaml_scalar_to_json(
                        field_value,
                        &format!("functions.{name}.returns"),
                    )?)
                }
                "run" => run = Some(parse_steps(field_value, &format!("functions.{name}.run"))?),
                other => {
                    return Err(one_diagnostic(
                        "yaml.functions.shape",
                        "functions",
                        format!(
                            "函数 {name} 定义不支持字段 {other:?}（description/params/vars/returns/run）"
                        ),
                    ));
                }
            }
        }
        let Some(run) = run else {
            return Err(one_diagnostic(
                "yaml.functions.shape",
                "functions",
                format!("函数 {name} 缺少 run 步骤列表"),
            ));
        };
        let param_names: BTreeSet<_> = params.iter().map(|decl| decl.name.clone()).collect();
        for (var_name, _) in &vars {
            if param_names.contains(var_name) {
                return Err(one_diagnostic(
                    "yaml.vars.conflict",
                    "functions",
                    format!("函数 {name} 的变量 {var_name} 与参数同名"),
                ));
            }
        }
        out.push((
            name.to_string(),
            FunctionDef {
                description,
                params,
                vars,
                returns,
                run,
            },
        ));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// 类型检查（参数声明默认值 / 实参绑定共用）
// ---------------------------------------------------------------------------

/// 值是否满足参数类型（不做事转换；duration 字符串允许 0）。
pub fn check_type(ty: ParamType, value: &Value) -> Result<(), String> {
    let ok = match ty {
        ParamType::Any => true,
        ParamType::Boolean => value.is_boolean(),
        ParamType::Integer => value.is_i64() || value.is_u64(),
        ParamType::Number => value.is_number(),
        ParamType::String => value.is_string(),
        ParamType::List => value.is_array(),
        ParamType::Object => value.is_object(),
        ParamType::Duration => match value {
            Value::Number(number) => number.as_f64().is_some_and(|value| value >= 0.0),
            Value::String(text) => parse_duration_ms(text).is_some(),
            _ => false,
        },
        ParamType::Point => point_components(value).is_some(),
        ParamType::Template | ParamType::Key => {
            value.as_str().is_some_and(|text| !text.trim().is_empty())
        }
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "得到 {}",
            match value {
                Value::Null => "null",
                Value::Bool(_) => "布尔值",
                Value::Number(_) => "数字",
                Value::String(_) => "字符串",
                Value::Array(_) => "数组",
                Value::Object(_) => "对象",
            }
        ))
    }
}

/// point 值 → 相对坐标分量（数组 `[x, y]` 或对象 `{x, y}`，均 0..1）。
pub fn point_components(value: &Value) -> Option<[f64; 2]> {
    let (x, y) = match value {
        Value::Array(items) if items.len() == 2 => (items[0].as_f64()?, items[1].as_f64()?),
        Value::Object(map) => (map.get("x")?.as_f64()?, map.get("y")?.as_f64()?),
        _ => return None,
    };
    let in_range = |component: f64| component.is_finite() && (0.0..=1.0).contains(&component);
    if in_range(x) && in_range(y) {
        Some([x, y])
    } else {
        None
    }
}

/// 时间书写串 → 毫秒（ms/s/m/min/h/d；允许 0，可小数）。
pub fn parse_duration_ms(raw: &str) -> Option<f64> {
    let lower = raw.trim().to_ascii_lowercase();
    for unit in ["min", "ms", "s", "m", "h", "d"] {
        if let Some(num) = lower.strip_suffix(unit) {
            let x: f64 = num.trim().parse().ok()?;
            if !x.is_finite() || x < 0.0 {
                return None;
            }
            let scale = match unit {
                "min" | "m" => 60_000.0,
                "ms" => 1.0,
                "s" => 1_000.0,
                "h" => 3_600_000.0,
                "d" => 86_400_000.0,
                _ => return None,
            };
            return Some(x * scale);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 降线：surface → yaml_interp wire
// ---------------------------------------------------------------------------

fn compact_json(value: &Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    if text.chars().count() > 48 {
        let truncated: String = text.chars().take(45).collect();
        format!("{truncated}…")
    } else {
        text
    }
}

fn step_desc(step: &SurfaceStep) -> String {
    match step {
        SurfaceStep::Call { name, args, .. } => format!("{name} {}", args.describe()),
        SurfaceStep::If { cond, .. } => format!("如果 {}", cond.describe()),
        SurfaceStep::Repeat { times, .. } => format!("重复 {} 次", times.describe()),
        SurfaceStep::Return { value } => format!("返回 {}", value.describe()),
    }
}

fn wire_step(step: &SurfaceStep, path: &str) -> Value {
    let desc = step_desc(step);
    let mut wire = serde_json::Map::new();
    wire.insert("path".into(), Value::String(path.to_string()));
    wire.insert("desc".into(), Value::String(desc));
    match step {
        SurfaceStep::Call {
            name,
            args,
            save_as,
        } => {
            wire.insert("op".into(), Value::String("fn".into()));
            wire.insert("fn".into(), Value::String(name.clone()));
            wire.insert("args".into(), args.to_wire());
            if let Some(save_as) = save_as {
                wire.insert("as".into(), Value::String(save_as.clone()));
            }
        }
        SurfaceStep::If {
            cond,
            then_steps,
            else_steps,
        } => {
            wire.insert("op".into(), Value::String("if".into()));
            wire.insert("cond".into(), cond.to_wire());
            wire.insert(
                "then".into(),
                Value::Array(wire_steps(then_steps, &format!("{path}.then"))),
            );
            if !else_steps.is_empty() {
                wire.insert(
                    "else".into(),
                    Value::Array(wire_steps(else_steps, &format!("{path}.else"))),
                );
            }
        }
        SurfaceStep::Repeat { times, body } => {
            wire.insert("op".into(), Value::String("repeat".into()));
            wire.insert("times".into(), times.to_wire());
            wire.insert(
                "do".into(),
                Value::Array(wire_steps(body, &format!("{path}.do"))),
            );
        }
        SurfaceStep::Return { value } => {
            wire.insert("op".into(), Value::String("return".into()));
            wire.insert("value".into(), value.to_wire());
        }
    }
    Value::Object(wire)
}

fn wire_steps(steps: &[SurfaceStep], prefix: &str) -> Vec<Value> {
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| wire_step(step, &format!("{prefix}[{index}]")))
        .collect()
}

fn param_decls_wire(decls: &[ParamDecl]) -> Value {
    Value::Array(
        decls
            .iter()
            .map(|decl| {
                json!({
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

fn vars_wire(vars: &[(String, Value)]) -> Value {
    Value::Object(vars.iter().cloned().collect())
}

/// 脚本降线为入口 wire 程序。
///
/// `functions` = 运行开始时冻结的当前 Package 函数表（计划 Phase 3.4）；
/// `initial_vars` = 已绑定参数与 vars 合并后的入口帧初值。
pub fn build_program(
    script: &Script,
    functions: &FunctionLibrary,
    initial_vars: serde_json::Map<String, Value>,
    start_index: usize,
) -> Value {
    let functions_wire: serde_json::Map<String, Value> = functions
        .iter()
        .map(|(name, def)| {
            (
                name.clone(),
                json!({
                    "params": param_decls_wire(&def.params),
                    "vars": vars_wire(&def.vars),
                    "run": wire_steps(&def.run, &format!("{name}.run")),
                }),
            )
        })
        .collect();
    json!({
        "vars": Value::Object(initial_vars),
        "run": wire_steps(&script.run, "run"),
        "functions": Value::Object(functions_wire),
        "start_index": start_index,
    })
}

/// 函数个体测试降线：入口 = 函数体（`<函数名>.run` 前缀），函数表携带
/// 全部定义；「从此运行」跳过函数体顶层前 N 步。
pub fn build_function_program(
    name: &str,
    def: &FunctionDef,
    functions: &FunctionLibrary,
    initial_vars: serde_json::Map<String, Value>,
    start_index: usize,
) -> Value {
    let mut functions_wire: serde_json::Map<String, Value> = functions
        .iter()
        .map(|(entry_name, entry_def)| {
            (
                entry_name.clone(),
                json!({
                    "params": param_decls_wire(&entry_def.params),
                    "vars": vars_wire(&entry_def.vars),
                    "run": wire_steps(&entry_def.run, &format!("{entry_name}.run")),
                }),
            )
        })
        .collect();
    functions_wire.insert(
        name.to_string(),
        json!({
            "params": param_decls_wire(&def.params),
            "vars": vars_wire(&def.vars),
            "run": wire_steps(&def.run, &format!("{name}.run")),
        }),
    );
    json!({
        "vars": Value::Object(initial_vars),
        "run": wire_steps(&def.run, &format!("{name}.run")),
        "functions": Value::Object(functions_wire),
        "start_index": start_index,
    })
}

// ---------------------------------------------------------------------------
// 模板重命名引用改写
// ---------------------------------------------------------------------------

/// 需要同步改写 template 实参的函数（V1 首版原生视觉函数）。
const TEMPLATE_PARAM_FUNCTIONS: &[&str] = &["find", "wait_find", "tap_template", "wait_disappear"];

fn rewrite_args_template(
    args: &mut SurfaceExpr,
    old_name: &str,
    old_short: &str,
    _new_name: &str,
    new_short: &str,
    changed: &mut bool,
) {
    if let SurfaceExpr::Map(entries) = args {
        for (key, value) in entries.iter_mut() {
            if key == "template" {
                if let SurfaceExpr::Lit(Value::String(text)) = value {
                    let matches_old = text == old_short
                        || text == old_name
                        || std::path::Path::new(text)
                            .file_name()
                            .is_some_and(|file| file.to_string_lossy() == old_name);
                    if matches_old {
                        *value = SurfaceExpr::Lit(Value::String(new_short.to_string()));
                        *changed = true;
                    }
                }
            }
        }
    }
}

fn rewrite_step_template(
    step: &mut SurfaceStep,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
    changed: &mut bool,
) {
    match step {
        SurfaceStep::Call { name, args, .. } => {
            if TEMPLATE_PARAM_FUNCTIONS.contains(&name.as_str()) {
                rewrite_args_template(args, old_name, old_short, new_name, new_short, changed);
            }
        }
        SurfaceStep::If {
            then_steps,
            else_steps,
            ..
        } => {
            for child in then_steps.iter_mut().chain(else_steps.iter_mut()) {
                rewrite_step_template(child, old_name, old_short, new_name, new_short, changed);
            }
        }
        SurfaceStep::Repeat { body, .. } => {
            for child in body.iter_mut() {
                rewrite_step_template(child, old_name, old_short, new_name, new_short, changed);
            }
        }
        SurfaceStep::Return { .. } => {}
    }
}

/// 脚本源中的模板引用改写。返回 `None` = 无引用变化（调用方保留原文）。
pub fn rename_template_source(
    source: &str,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
) -> Result<Option<(String, bool)>, Vec<Diagnostic>> {
    let mut script = parse_script(source)?;
    let mut changed = false;
    for step in &mut script.run {
        rewrite_step_template(step, old_name, old_short, new_name, new_short, &mut changed);
    }
    if !changed {
        return Ok(None);
    }
    Ok(Some((serialize_script(&script), true)))
}

/// 函数库文件源中的模板引用改写（任一函数改写即整体重序列化）。
pub fn rename_template_in_function_library(
    source: &str,
    old_name: &str,
    old_short: &str,
    new_name: &str,
    new_short: &str,
) -> Result<Option<(String, bool)>, Vec<Diagnostic>> {
    let mut library = parse_function_library(source)?;
    let mut changed = false;
    for (_, def) in &mut library {
        for step in &mut def.run {
            rewrite_step_template(step, old_name, old_short, new_name, new_short, &mut changed);
        }
    }
    if !changed {
        return Ok(None);
    }
    Ok(Some((serialize_function_library(&library), true)))
}

// ---------------------------------------------------------------------------
// 确定性序列化（编辑器保存 / 引用改写落盘共用）
// ---------------------------------------------------------------------------

fn yaml_quote(text: &str) -> String {
    serde_yaml::to_string(&YamlValue::String(text.to_string()))
        .ok()
        .map(|dumped| dumped.trim_end_matches('\n').to_string())
        .unwrap_or_else(|| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"")))
}

fn yaml_value_text(value: &Value) -> String {
    match value {
        Value::String(text) => yaml_quote(text),
        Value::Bool(value) => value.to_string(),
        Value::Number(_) | Value::Null => serde_json::to_string(value).unwrap_or_default(),
        Value::Array(_) | Value::Object(_) => {
            serde_yaml::to_string(&serde_yaml::to_value(value).unwrap_or(YamlValue::Null))
                .unwrap_or_default()
                .trim_end_matches('\n')
                .to_string()
        }
    }
}

fn expr_yaml_lines(expr: &SurfaceExpr, indent: usize, out: &mut Vec<String>) {
    let pad = "  ".repeat(indent);
    match expr {
        SurfaceExpr::Lit(Value::Null) => out.push(String::new()),
        SurfaceExpr::Lit(Value::String(text)) => out.push(format!("{pad}{}", yaml_quote(text))),
        SurfaceExpr::Lit(value) => out.push(format!("{pad}{}", yaml_value_text(value))),
        SurfaceExpr::Ref(path) => out.push(format!("{pad}${path}")),
        SurfaceExpr::List(items) => {
            out.push(pad.to_string());
            for item in items {
                match item {
                    SurfaceExpr::Lit(Value::String(text)) => {
                        out.push(format!("{pad}- {}", yaml_quote(text)))
                    }
                    SurfaceExpr::Lit(value) => {
                        out.push(format!("{pad}- {}", yaml_value_text(value)))
                    }
                    SurfaceExpr::Ref(path) => out.push(format!("{pad}- ${path}")),
                    SurfaceExpr::List(_) | SurfaceExpr::Map(_) => {
                        out.push(format!("{pad}-"));
                        expr_yaml_lines(item, indent + 1, out);
                    }
                }
            }
        }
        SurfaceExpr::Map(entries) => {
            for (key, value) in entries {
                match value {
                    SurfaceExpr::Lit(Value::String(text)) => {
                        out.push(format!("{pad}{key}: {}", yaml_quote(text)))
                    }
                    SurfaceExpr::Lit(Value::Null) => out.push(format!("{pad}{key}:")),
                    SurfaceExpr::Lit(value) => {
                        out.push(format!("{pad}{key}: {}", yaml_value_text(value)))
                    }
                    SurfaceExpr::Ref(path) => out.push(format!("{pad}{key}: ${path}")),
                    SurfaceExpr::List(_) | SurfaceExpr::Map(_) => {
                        out.push(format!("{pad}{key}:"));
                        expr_yaml_lines(value, indent + 1, out);
                    }
                }
            }
        }
    }
}

/// 序列化一个步骤的键行（缩进 = indent；`as` 与调用键同列）。
fn step_yaml_lines(step: &SurfaceStep, indent: usize, out: &mut Vec<String>) {
    let pad = "  ".repeat(indent);
    match step {
        SurfaceStep::Call {
            name,
            args,
            save_as,
        } => {
            match args {
                SurfaceExpr::Map(entries) if entries.is_empty() => {
                    out.push(format!("{pad}{name}: {{}}"));
                }
                SurfaceExpr::Map(_) => {
                    out.push(format!("{pad}{name}:"));
                    expr_yaml_lines(args, indent + 1, out);
                }
                SurfaceExpr::Lit(Value::Null) => out.push(format!("{pad}{name}:")),
                SurfaceExpr::Lit(_) | SurfaceExpr::Ref(_) | SurfaceExpr::List(_) => {
                    let mut lines = Vec::new();
                    expr_yaml_lines(args, 0, &mut lines);
                    let inline = lines.join("\n").trim().to_string();
                    if inline.contains('\n') {
                        out.push(format!("{pad}{name}:"));
                        expr_yaml_lines(args, indent + 1, out);
                    } else {
                        out.push(format!("{pad}{name}: {inline}"));
                    }
                }
            }
            if let Some(save_as) = save_as {
                out.push(format!("{pad}as: {save_as}"));
            }
        }
        SurfaceStep::If {
            cond,
            then_steps,
            else_steps,
        } => {
            let mut cond_lines = Vec::new();
            expr_yaml_lines(cond, 0, &mut cond_lines);
            let cond_text = cond_lines.join("\n").trim().to_string();
            if cond_text.contains('\n') {
                out.push(format!("{pad}if:"));
                expr_yaml_lines(cond, indent + 1, out);
            } else {
                out.push(format!("{pad}if: {cond_text}"));
            }
            out.push(format!("{pad}then:"));
            for child in then_steps {
                push_dash_item(child, indent + 1, out);
            }
            if !else_steps.is_empty() {
                out.push(format!("{pad}else:"));
                for child in else_steps {
                    push_dash_item(child, indent + 1, out);
                }
            }
        }
        SurfaceStep::Repeat { times, body } => {
            let mut times_lines = Vec::new();
            expr_yaml_lines(times, 0, &mut times_lines);
            let times_text = times_lines.join("\n").trim().to_string();
            if times_text.contains('\n') {
                out.push(format!("{pad}repeat:"));
                expr_yaml_lines(times, indent + 1, out);
            } else {
                out.push(format!("{pad}repeat: {times_text}"));
            }
            out.push(format!("{pad}do:"));
            for child in body {
                push_dash_item(child, indent + 1, out);
            }
        }
        SurfaceStep::Return { value } => {
            let mut value_lines = Vec::new();
            expr_yaml_lines(value, 0, &mut value_lines);
            let value_text = value_lines.join("\n").trim().to_string();
            if value_text.contains('\n') {
                out.push(format!("{pad}return:"));
                expr_yaml_lines(value, indent + 1, out);
            } else {
                out.push(format!("{pad}return: {value_text}"));
            }
        }
    }
}

/// 序列化一个序列项：`- ` 与步骤首键同行，后续键与首键同列。
fn push_dash_item(step: &SurfaceStep, indent: usize, out: &mut Vec<String>) {
    let mut lines: Vec<String> = Vec::new();
    step_yaml_lines(step, indent + 1, &mut lines);
    let pad = "  ".repeat(indent);
    if let Some(first) = lines.first_mut() {
        let trimmed = first.trim_start().to_string();
        *first = format!("{pad}- {trimmed}");
    }
    out.extend(lines);
}

fn steps_yaml_lines(steps: &[SurfaceStep], indent: usize, out: &mut Vec<String>) {
    if steps.is_empty() {
        out.push(format!("{}run: []", "  ".repeat(indent)));
        return;
    }
    out.push(format!("{}run:", "  ".repeat(indent)));
    for step in steps {
        push_dash_item(step, indent + 1, out);
    }
}

fn params_yaml_lines(decls: &[ParamDecl], indent: usize, out: &mut Vec<String>) {
    let pad = "  ".repeat(indent);
    for decl in decls {
        out.push(format!("{pad}  {name}:", name = decl.name));
        out.push(format!("{pad}    type: {}", decl.ty.canonical()));
        if decl.required {
            out.push(format!("{pad}    required: true"));
        }
        if let Some(default) = &decl.default {
            out.push(format!(
                "{pad}    default: {}",
                yaml_value_text(default).trim()
            ));
        }
        if let Some(desc) = &decl.desc {
            out.push(format!("{pad}    desc: {}", yaml_quote(desc)));
        }
    }
}

fn vars_yaml_lines(vars: &[(String, Value)], indent: usize, out: &mut Vec<String>) {
    let pad = "  ".repeat(indent);
    for (name, value) in vars {
        out.push(format!("{pad}  {name}: {}", yaml_value_text(value).trim()));
    }
}

/// 脚本 → 确定性 YAML 文本（保存/改写落盘统一形态）。
pub fn serialize_script(script: &Script) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(name) = &script.name {
        lines.push(format!("name: {}", yaml_quote(name)));
    }
    if !script.params.is_empty() {
        lines.push("params:".into());
        params_yaml_lines(&script.params, 0, &mut lines);
    }
    if !script.vars.is_empty() {
        lines.push("vars:".into());
        vars_yaml_lines(&script.vars, 0, &mut lines);
    }
    steps_yaml_lines(&script.run, 0, &mut lines);
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// 函数库文件 → 确定性 YAML 文本。
pub fn serialize_function_library(library: &FunctionLibrary) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("functions:".into());
    for (name, def) in library {
        lines.push(format!("  {name}:"));
        if let Some(description) = &def.description {
            lines.push(format!("    description: {}", yaml_quote(description)));
        }
        if !def.params.is_empty() {
            lines.push("    params:".into());
            params_yaml_lines(&def.params, 2, &mut lines);
        }
        if !def.vars.is_empty() {
            lines.push("    vars:".into());
            vars_yaml_lines(&def.vars, 2, &mut lines);
        }
        if let Some(returns) = &def.returns {
            lines.push(format!("    returns: {}", yaml_value_text(returns).trim()));
        }
        if def.run.is_empty() {
            lines.push("    run: []".into());
        } else {
            lines.push("    run:".into());
            for step in &def.run {
                push_dash_item(step, 2, &mut lines);
            }
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试宿主：视觉函数返回 match 对象，其余返回 null。
    struct NullHost;

    impl yaml_interp::HostFunctions for NullHost {
        fn invoke(&self, name: &str, _args: Value) -> Result<Value, yaml_interp::HostError> {
            if matches!(name, "find" | "wait_find" | "tap_template") {
                return Ok(serde_json::json!({ "center": { "x": 0.5, "y": 0.5 } }));
            }
            Ok(Value::Null)
        }
    }

    fn run_script(source: &str) -> Result<Value, String> {
        let script = parse_script(source).map_err(|diagnostics| diagnostics[0].to_string())?;
        let library = parse_function_library("functions: {}\n").unwrap();
        let initial: serde_json::Map<String, Value> = script.vars.iter().cloned().collect();
        let wire = build_program(&script, &library, initial, 0);
        let program: yaml_interp::Program =
            serde_json::from_value(wire).map_err(|error| error.to_string())?;
        yaml_interp::run(&program, &NullHost, None)
    }

    #[test]
    fn parses_plan_example_script() {
        let source = r#"
name: 每日签到

params:
  retry:
    type: integer
    default: 3

vars:
  timeout: 15s

run:
  - launch: com.example.game

  - wait_find:
      template: home
      timeout: $timeout
    as: home

  - if: $home
    then:
      - claim_daily: {}
    else:
      - log: 未进入主页
"#;
        let script = parse_script(source).unwrap();
        assert_eq!(script.name.as_deref(), Some("每日签到"));
        assert_eq!(script.params.len(), 1);
        assert_eq!(script.params[0].ty, ParamType::Integer);
        assert_eq!(script.vars[0].1, Value::String("15s".into()));
        assert_eq!(script.run.len(), 3);
        assert_eq!(
            script.called_functions(),
            BTreeSet::from([
                "launch".to_string(),
                "wait_find".to_string(),
                "claim_daily".to_string(),
                "log".to_string(),
            ])
        );
        // 变量引用收集（$timeout）
        assert!(script.referenced_vars().contains("timeout"));
    }

    #[test]
    fn version_field_is_rejected_with_migration_hint() {
        let diagnostics = parse_script("version: 3\nrun: []\n").unwrap_err();
        assert_eq!(diagnostics[0].code, "yaml.version.removed");
        let legacy = parse_script("version: 3\nparams: []\ndefaults: {}\nsteps:\n  - log: hi\n")
            .unwrap_err();
        assert_eq!(legacy[0].code, "yaml.version.removed");
    }

    #[test]
    fn unknown_top_level_fields_are_rejected() {
        let diagnostics = parse_script("steps: []\n").unwrap_err();
        assert_eq!(diagnostics[0].code, "yaml.top.unknown");
    }

    #[test]
    fn shorthand_args_and_refs_parse_to_wire_shapes() {
        let script = parse_script(
            "run:\n  - tap: [0.5, 0.8]\n  - sleep: 1s\n  - find: login\n    as: button\n  - tap: $button.center\n  - log: $$price\n",
        )
        .unwrap();
        let wire = build_program(&script, &Vec::new(), Default::default(), 0);
        assert_eq!(wire["run"][0]["op"], "fn");
        assert_eq!(wire["run"][0]["fn"], "tap");
        assert_eq!(wire["run"][0]["args"]["expr"], "list");
        assert_eq!(wire["run"][0]["args"]["value"][0]["value"], 0.5);
        assert_eq!(wire["run"][0]["path"], "run[0]");
        assert_eq!(wire["run"][2]["as"], "button");
        assert_eq!(wire["run"][3]["args"]["expr"], "ref");
        assert_eq!(wire["run"][3]["args"]["path"], "button.center");
        // $$ 转义 → 字面量 $price
        assert_eq!(wire["run"][4]["args"]["value"], "$price");
    }

    #[test]
    fn invalid_refs_and_names_are_rejected() {
        assert_eq!(
            parse_script("run:\n  - tap: $Foo Bar\n").unwrap_err()[0].code,
            "yaml.expr.invalid"
        );
        assert_eq!(
            parse_script("run:\n  - Tap: [0.1, 0.1]\n").unwrap_err()[0].code,
            "yaml.name.invalid"
        );
        assert_eq!(
            parse_script("run:\n  - if: $x\n    as: y\n    then: []\n").unwrap_err()[0].code,
            "yaml.as.invalid"
        );
        assert_eq!(
            parse_script("run:\n  - tap: [0.1]\n    repeat: 2\n").unwrap_err()[0].code,
            "yaml.step.multi"
        );
        assert_eq!(
            parse_script("run:\n  - as: x\n").unwrap_err()[0].code,
            "yaml.step.missing"
        );
        assert_eq!(
            parse_script("run:\n  - if: $x\n").unwrap_err()[0].code,
            "yaml.if.then"
        );
        assert_eq!(
            parse_script("run:\n  - repeat: 3\n").unwrap_err()[0].code,
            "yaml.repeat.do"
        );
        assert_eq!(
            parse_script("run:\n  - repeat: -1\n    do: []\n").unwrap_err()[0].code,
            "yaml.repeat.times"
        );
    }

    #[test]
    fn nested_refs_inside_maps_and_lists() {
        let script =
            parse_script("run:\n  - swipe:\n      from: $start\n      to: [0.5, 0.2]\n").unwrap();
        let wire = build_program(&script, &Vec::new(), Default::default(), 0);
        let args = &wire["run"][0]["args"];
        assert_eq!(args["expr"], "map");
        assert_eq!(args["value"]["from"]["expr"], "ref");
        assert_eq!(args["value"]["from"]["path"], "start");
        assert_eq!(args["value"]["to"]["value"][1]["value"], 0.2);
    }

    #[test]
    fn if_repeat_return_roundtrip_through_wire_and_interp() {
        let source = r#"
vars:
  flag: true

run:
  - repeat: 2
    do:
      - log: tick
  - if: $flag
    then:
      - return: done
    else:
      - log: no
"#;
        let value = run_script(source).unwrap();
        assert_eq!(value, Value::String("done".into()));
    }

    #[test]
    fn function_library_parses_and_lowers() {
        let source = r#"
functions:
  claim_daily:
    description: 领取每日奖励
    params:
      timeout:
        type: duration
        default: 5s
    run:
      - tap_template:
          template: daily_button
          timeout: $timeout
      - return: true

  greet:
    run:
      - log: hi
"#;
        let library = parse_function_library(source).unwrap();
        assert_eq!(library.len(), 2);
        assert_eq!(library[0].0, "claim_daily");
        assert_eq!(library[0].1.params[0].ty, ParamType::Duration);
        assert_eq!(
            library[0].1.params[0].default,
            Some(Value::String("5s".into()))
        );
        assert_eq!(library[1].1.run.len(), 1);
        // 「文件第一个函数」缺省语义
        assert_eq!(
            library.first().map(|(name, _)| name.as_str()),
            Some("claim_daily")
        );

        let bad = parse_function_library("greet:\n  run: []\n").unwrap_err();
        assert_eq!(bad[0].code, "yaml.functions.missing");
        let reserved = parse_function_library("functions:\n  if:\n    run: []\n").unwrap_err();
        assert_eq!(reserved[0].code, "yaml.name.invalid");
        let no_run = parse_function_library("functions:\n  hi:\n    description: x\n").unwrap_err();
        assert_eq!(no_run[0].code, "yaml.functions.shape");
    }

    #[test]
    fn script_and_library_serialize_roundtrip() {
        let script_source = r#"
name: 每日签到
params:
  retry:
    type: integer
    default: 3
vars:
  timeout: 15s
run:
  - launch: com.example.game
  - wait_find:
      template: home
      timeout: $timeout
    as: home
  - if: $home
    then:
      - claim_daily: {}
    else:
      - log: 未进入主页
  - return: $home.center
"#;
        let script = parse_script(script_source).unwrap();
        let serialized = serialize_script(&script);
        let reparsed = parse_script(&serialized).unwrap();
        assert_eq!(serialize_script(&reparsed), serialized, "序列化必须幂等");
        // 序列化产物可执行（NullHost 下未知函数不会拦截，运行应成功返回）
        run_script(&serialized)
            .unwrap_or_else(|error| panic!("序列化产物必须可执行: {error}\n{serialized}"));

        let library_source = "functions:\n  claim_daily:\n    params:\n      timeout:\n        type: duration\n        default: 5s\n    run:\n      - tap_template:\n          template: daily_button\n          timeout: $timeout\n      - return: true\n";
        let library = parse_function_library(library_source).unwrap();
        let serialized = serialize_function_library(&library);
        let reparsed = parse_function_library(&serialized).unwrap();
        assert_eq!(serialize_function_library(&reparsed), serialized);
    }

    #[test]
    fn template_rename_rewrites_known_functions_only() {
        let source = r#"
run:
  - find:
      template: old.png
      region: [0, 0, 1, 1]
    as: hit
  - tap_template:
      template: old.png
  - log: old.png 文本不改
"#;
        let (rewritten, changed) =
            rename_template_source(source, "old.png", "old", "new.png", "new")
                .unwrap()
                .unwrap();
        assert!(changed);
        let script = parse_script(&rewritten).unwrap();
        let text = serialize_script(&script);
        assert!(text.contains("template: new"));
        assert!(text.contains("old.png 文本不改"), "日志文本不参与模板改写");

        // 无引用 → None（保留原文）
        let untouched =
            rename_template_source("run:\n  - log: x\n", "old.png", "old", "new.png", "new")
                .unwrap();
        assert!(untouched.is_none());

        let library =
            "functions:\n  login:\n    run:\n      - wait_find:\n          template: old.png\n";
        let (rewritten, changed) =
            rename_template_in_function_library(library, "old.png", "old", "new.png", "new")
                .unwrap()
                .unwrap();
        assert!(changed);
        assert!(rewritten.contains("template: new"));
    }

    #[test]
    fn params_default_type_is_checked() {
        let bad = parse_script("params:\n  n:\n    type: integer\n    default: 1.5\nrun: []\n")
            .unwrap_err();
        assert_eq!(bad[0].code, "yaml.param.default.invalid");
        let good =
            parse_script("params:\n  t:\n    type: duration\n    default: 0s\nrun: []\n").unwrap();
        assert_eq!(good.params[0].default, Some(Value::String("0s".into())));
    }

    #[test]
    fn vars_conflict_with_params_is_rejected() {
        let bad =
            parse_script("params:\n  a:\n    type: string\nvars:\n  a: x\nrun: []\n").unwrap_err();
        assert_eq!(bad[0].code, "yaml.vars.conflict");
    }
}
