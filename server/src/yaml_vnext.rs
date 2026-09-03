//! YAML v3 的纯数据前端：Surface YAML -> small AST。
//!
//! 这个模块故意不依赖 `script_v2`、`engine`、`ScriptStore` 或任何设备实现。
//! 它只负责把用户友好的 YAML 语法收敛成少量控制流节点和通用 capability
//! invocation。执行在 `yaml_extension` 中完成，因而 Core 不需要认识 YAML。

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value as YamlValue};

pub const YAML_V3: u64 = 3;
const DEFAULT_POLL_MS: u64 = 100;

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

    pub fn map() -> BTreeMap<String, Value> {
        BTreeMap::new()
    }

    pub fn into_json(self) -> serde_json::Value {
        serde_json::to_value(self).expect("yaml vnext values are JSON representable")
    }

    /// Accept both the typed wire format emitted by `Value` and ordinary JSON
    /// values supplied by a third-party capability guest.
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
            serde_json::Value::Array(values) => Self::List(
                values
                    .into_iter()
                    .map(Self::from_plain_json)
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

/// The only nodes allowed after lowering. Actions are represented by the
/// generic `invoke` node; this is the important boundary that keeps YAML
/// policy out of Core capabilities.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SmallStep {
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamDecl {
    pub name: String,
    pub ty: String,
    pub default: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub version: u64,
    pub params: Vec<ParamDecl>,
    pub steps: Vec<SmallStep>,
}

/// Parsed but not lowered surface syntax. Keeping this type visible makes the
/// two-phase contract testable and lets the editor show the original feature.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceProgram {
    pub version: u64,
    pub params: Vec<ParamDecl>,
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
    Wait {
        duration: Expr,
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
    Find {
        template: Expr,
        timeout: Option<Expr>,
        region: Option<Expr>,
        click: bool,
        then_steps: Vec<SurfaceStep>,
        else_steps: Vec<SurfaceStep>,
    },
    Check {
        template: Expr,
        timeout: Option<Expr>,
        message: Option<Expr>,
    },
    Retry {
        times: Expr,
        steps: Vec<SurfaceStep>,
    },
    WaitFor {
        template: Expr,
        timeout: Option<Expr>,
        then_steps: Vec<SurfaceStep>,
        else_steps: Vec<SurfaceStep>,
    },
    ClickWhen {
        template: Expr,
        timeout: Option<Expr>,
        then_steps: Vec<SurfaceStep>,
        else_steps: Vec<SurfaceStep>,
    },
    MatchFirst {
        candidates: Vec<MatchCandidateSurface>,
        then_steps: Vec<SurfaceStep>,
        else_steps: Vec<SurfaceStep>,
    },
    ColorBranch {
        at: Expr,
        branches: Vec<ColorBranchSurface>,
        else_steps: Vec<SurfaceStep>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatchCandidateSurface {
    pub template: Expr,
    pub steps: Vec<SurfaceStep>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColorBranchSurface {
    pub color: Expr,
    pub click: bool,
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
        if !matches!(key, "version" | "params" | "steps") {
            return Err(vec![Diagnostic::new(
                "yaml.v3.top_level.unknown_key",
                key,
                format!("不支持顶层字段 {key:?}"),
            )]);
        }
    }
    let params = map
        .get("params")
        .map(parse_params)
        .transpose()?
        .unwrap_or_default();
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
        steps,
    })
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
                let _remark = parts.next();
                let default = parts.next().map(|value| Value::String(value.to_string()));
                result.push(ParamDecl {
                    name: name.to_string(),
                    ty: ty.to_string(),
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
                result.push(ParamDecl { name, ty, default });
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
        "wait" => Ok(SurfaceStep::Wait {
            duration: duration_expr(
                map_value(value, "duration", "time", &value_path)?,
                &value_path,
            )?,
        }),
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
            Ok(SurfaceStep::Call {
                target: required_string(map, "target", &value_path)?,
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
            reject_unknown(map, &["template", "timeout", "throw"], &value_path)?;
            Ok(SurfaceStep::Check {
                template: field_expr(map, "template", &value_path)?,
                timeout: map
                    .get("timeout")
                    .map(|v| duration_expr(v, &format!("{value_path}.timeout")))
                    .transpose()?,
                message: map
                    .get("throw")
                    .map(|v| expr_from_yaml(v, &format!("{value_path}.throw")))
                    .transpose()?,
            })
        }
        "retry" => {
            let map = as_map(value, &value_path, "retry 必须是映射")?;
            reject_unknown(map, &["times", "steps"], &value_path)?;
            Ok(SurfaceStep::Retry {
                times: field_expr(map, "times", &value_path)?,
                steps: required_steps(map, "steps", &value_path)?,
            })
        }
        "wait_for" | "click_when" => {
            let map = as_map(value, &value_path, "wait_for/click_when 必须是映射")?;
            reject_unknown(map, &["template", "timeout", "then", "else"], &value_path)?;
            let step = if action == "wait_for" {
                SurfaceStep::WaitFor {
                    template: field_expr(map, "template", &value_path)?,
                    timeout: map
                        .get("timeout")
                        .map(|v| duration_expr(v, &format!("{value_path}.timeout")))
                        .transpose()?,
                    then_steps: parse_optional_steps(
                        map.get("then"),
                        &format!("{value_path}.then"),
                    )?,
                    else_steps: parse_optional_steps(
                        map.get("else"),
                        &format!("{value_path}.else"),
                    )?,
                }
            } else {
                SurfaceStep::ClickWhen {
                    template: field_expr(map, "template", &value_path)?,
                    timeout: map
                        .get("timeout")
                        .map(|v| duration_expr(v, &format!("{value_path}.timeout")))
                        .transpose()?,
                    then_steps: parse_optional_steps(
                        map.get("then"),
                        &format!("{value_path}.then"),
                    )?,
                    else_steps: parse_optional_steps(
                        map.get("else"),
                        &format!("{value_path}.else"),
                    )?,
                }
            };
            Ok(step)
        }
        "match_first" => parse_match_first(value, &value_path),
        "color_branch" => parse_color_branch(value, &value_path),
        _ => Err(vec![Diagnostic::new(
            "yaml.v3.step.unknown",
            &value_path,
            format!("未知 v3 动作 {action:?}"),
        )]),
    }
}

fn parse_find(value: &YamlValue, path: &str) -> Result<SurfaceStep, Vec<Diagnostic>> {
    let map = as_map(value, path, "find 必须是映射")?;
    reject_unknown(
        map,
        &["template", "timeout", "region", "click", "then", "else"],
        path,
    )?;
    Ok(SurfaceStep::Find {
        template: field_expr(map, "template", path)?,
        timeout: map
            .get("timeout")
            .map(|v| duration_expr(v, &format!("{path}.timeout")))
            .transpose()?,
        region: map
            .get("region")
            .map(|v| expr_from_yaml(v, &format!("{path}.region")))
            .transpose()?,
        click: map
            .get("click")
            .and_then(YamlValue::as_bool)
            .unwrap_or(false),
        then_steps: parse_optional_steps(map.get("then"), &format!("{path}.then"))?,
        else_steps: parse_optional_steps(map.get("else"), &format!("{path}.else"))?,
    })
}

fn parse_match_first(value: &YamlValue, path: &str) -> Result<SurfaceStep, Vec<Diagnostic>> {
    let map = as_map(value, path, "match_first 必须是映射")?;
    reject_unknown(map, &["templates", "candidates", "then", "else"], path)?;
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
            reject_unknown(item_map, &["template", "steps"], &item_path)?;
            candidates.push(MatchCandidateSurface {
                template: field_expr(item_map, "template", &item_path)?,
                steps: parse_optional_steps(item_map.get("steps"), &format!("{item_path}.steps"))?,
            });
        } else {
            candidates.push(MatchCandidateSurface {
                template: expr_from_yaml(item, &item_path)?,
                steps: Vec::new(),
            });
        }
    }
    Ok(SurfaceStep::MatchFirst {
        candidates,
        then_steps: parse_optional_steps(map.get("then"), &format!("{path}.then"))?,
        else_steps: parse_optional_steps(map.get("else"), &format!("{path}.else"))?,
    })
}

fn parse_color_branch(value: &YamlValue, path: &str) -> Result<SurfaceStep, Vec<Diagnostic>> {
    let map = as_map(value, path, "color_branch 必须是映射")?;
    reject_unknown(map, &["at", "branches", "expect", "else"], path)?;
    let raw = map
        .get("branches")
        .or_else(|| map.get("expect"))
        .ok_or_else(|| {
            vec![Diagnostic::new(
                "yaml.v3.field.missing",
                path,
                "color_branch 缺少 branches/expect",
            )]
        })?;
    let items = match raw {
        YamlValue::Sequence(items) => items,
        _ => {
            return Err(vec![Diagnostic::new(
                "yaml.v3.color_branch.type",
                path,
                "branches/expect 必须是列表",
            )])
        }
    };
    let mut branches = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let item_path = format!("{path}.branches[{index}]");
        let item_map = as_map(item, &item_path, "颜色分支必须是单键映射")?;
        if item_map.len() != 1 {
            return Err(vec![Diagnostic::new(
                "yaml.v3.color_branch.shape",
                item_path,
                "每个颜色分支必须只有一个颜色键",
            )]);
        }
        let (color, branch) = item_map.iter().next().expect("non-empty color branch");
        let color = color_expr(color, &item_path)?;
        let (click, steps) = match branch {
            YamlValue::Mapping(branch_map) => {
                reject_unknown(branch_map, &["click", "steps"], &item_path)?;
                (
                    branch_map
                        .get("click")
                        .and_then(YamlValue::as_bool)
                        .unwrap_or(false),
                    parse_optional_steps(branch_map.get("steps"), &format!("{item_path}.steps"))?,
                )
            }
            _ => (false, parse_steps(branch, &format!("{item_path}.steps"))?),
        };
        branches.push(ColorBranchSurface {
            color,
            click,
            steps,
        });
    }
    Ok(SurfaceStep::ColorBranch {
        at: field_expr(map, "at", path)?,
        branches,
        else_steps: parse_optional_steps(map.get("else"), &format!("{path}.else"))?,
    })
}

pub fn lower(surface: &SurfaceProgram) -> Result<Program, Vec<Diagnostic>> {
    let mut lowerer = Lowerer { serial: 0 };
    let steps = lowerer.steps(&surface.steps)?;
    Ok(Program {
        version: YAML_V3,
        params: surface.params.clone(),
        steps,
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
        "find" | "check" | "wait_for" | "click_when" => {
            if let Some(body) = body.as_mapping_mut() {
                if let Some(template) = body.get_mut("template") {
                    replace_template_scalar(
                        template, old_name, old_short, new_name, new_short, changed,
                    );
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
}

impl Lowerer {
    fn temp(&mut self, prefix: &str) -> String {
        self.serial += 1;
        format!("__yaml_{prefix}_{}", self.serial)
    }

    fn steps(&mut self, steps: &[SurfaceStep]) -> Result<Vec<SmallStep>, Vec<Diagnostic>> {
        steps.iter().map(|step| self.step(step)).collect()
    }

    fn step(&mut self, step: &SurfaceStep) -> Result<SmallStep, Vec<Diagnostic>> {
        Ok(match step {
            SurfaceStep::Tap { at } => invoke("input.tap", map([("point", at.clone())]), None),
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
            SurfaceStep::Wait { duration } => {
                invoke("runtime.sleep", map([("duration", duration.clone())]), None)
            }
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
                then_steps: self.steps(then_steps)?,
                else_steps: self.steps(else_steps)?,
            },
            SurfaceStep::Loop { times, steps } => SmallStep::Loop {
                times: times.clone(),
                body: self.steps(steps)?,
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
            SurfaceStep::Retry { times, steps } => SmallStep::Loop {
                times: Some(times.clone()),
                body: self.steps(steps)?,
            },
            SurfaceStep::Find {
                template,
                timeout,
                region,
                click,
                then_steps,
                else_steps,
            } => self.poll_match(
                template.clone(),
                timeout.clone(),
                region.clone(),
                *click,
                then_steps,
                else_steps,
            )?,
            SurfaceStep::WaitFor {
                template,
                timeout,
                then_steps,
                else_steps,
            } => self.poll_match(
                template.clone(),
                timeout.clone(),
                None,
                false,
                then_steps,
                else_steps,
            )?,
            SurfaceStep::ClickWhen {
                template,
                timeout,
                then_steps,
                else_steps,
            } => self.poll_match(
                template.clone(),
                timeout.clone(),
                None,
                true,
                then_steps,
                else_steps,
            )?,
            SurfaceStep::Check {
                template,
                timeout,
                message,
            } => {
                let result = self.temp("check");
                let mut body = vec![invoke(
                    "vision.match",
                    map([("template", template.clone())]),
                    Some(result.clone()),
                )];
                let fail = message
                    .clone()
                    .unwrap_or_else(|| lit(Value::String("check 未命中".to_string())));
                body.push(SmallStep::If {
                    cond: Condition::truthy(Expr::reference(format!("{result}.found"))),
                    then_steps: vec![SmallStep::Break],
                    else_steps: vec![SmallStep::Throw { message: fail }],
                });
                SmallStep::Loop {
                    times: timeout.clone(),
                    body,
                }
            }
            SurfaceStep::MatchFirst {
                candidates,
                then_steps,
                else_steps,
            } => {
                let result = self.temp("match_many");
                let templates = Expr::List(
                    candidates
                        .iter()
                        .map(|item| item.template.clone())
                        .collect(),
                );
                let mut branches = self.steps(else_steps)?;
                for (index, candidate) in candidates.iter().enumerate().rev() {
                    let branch = if candidate.steps.is_empty() {
                        self.steps(then_steps)?
                    } else {
                        self.steps(&candidate.steps)?
                    };
                    branches = vec![SmallStep::If {
                        cond: Condition::truthy(Expr::reference(format!(
                            "{result}.matches[{index}].found"
                        ))),
                        then_steps: branch,
                        else_steps: branches,
                    }];
                }
                let mut steps = vec![invoke(
                    "vision.match_many",
                    map([("templates", templates)]),
                    Some(result),
                )];
                steps.extend(branches);
                // The nested candidate conditions already provide the no-hit
                // path; keeping the invoke unconditional preserves the native
                // match_many one-frame contract.
                SmallStep::If {
                    cond: Condition::truthy(lit(Value::Bool(true))),
                    then_steps: steps,
                    else_steps: Vec::new(),
                }
            }
            SurfaceStep::ColorBranch {
                at,
                branches,
                else_steps,
            } => {
                let result = self.temp("color");
                let mut lowered = self.steps(else_steps)?;
                for branch in branches.iter().rev() {
                    let mut then_steps = Vec::new();
                    if branch.click {
                        then_steps.push(invoke("input.tap", map([("point", at.clone())]), None));
                    }
                    then_steps.extend(self.steps(&branch.steps)?);
                    lowered = vec![SmallStep::If {
                        cond: Condition::Equals {
                            left: Expr::reference(result.clone()),
                            right: branch.color.clone(),
                        },
                        then_steps,
                        else_steps: lowered,
                    }];
                }
                let mut steps = vec![invoke(
                    "vision.sample_color",
                    map([("point", at.clone())]),
                    Some(result),
                )];
                steps.extend(lowered);
                SmallStep::If {
                    cond: Condition::truthy(lit(Value::Bool(true))),
                    then_steps: steps,
                    else_steps: Vec::new(),
                }
            }
        })
    }

    fn poll_match(
        &mut self,
        template: Expr,
        timeout: Option<Expr>,
        region: Option<Expr>,
        click: bool,
        then_steps: &[SurfaceStep],
        else_steps: &[SurfaceStep],
    ) -> Result<SmallStep, Vec<Diagnostic>> {
        let result = self.temp("match");
        let done = self.temp("done");
        let mut body = vec![invoke(
            "vision.match",
            {
                let mut args = map([("template", template)]);
                if let Some(region) = region {
                    args.insert("region".to_string(), region);
                }
                args
            },
            Some(result.clone()),
        )];
        let mut found = vec![SmallStep::Set {
            name: done.clone(),
            value: lit(Value::Bool(true)),
        }];
        if click {
            found.push(invoke(
                "input.tap",
                map([("point", Expr::reference(format!("{result}.center")))]),
                None,
            ));
        }
        found.extend(self.steps(then_steps)?);
        found.push(SmallStep::Break);
        body.push(SmallStep::If {
            cond: Condition::truthy(Expr::reference(format!("{result}.found"))),
            then_steps: found,
            else_steps: vec![invoke(
                "runtime.sleep",
                map([("duration", lit(Value::Duration(DEFAULT_POLL_MS)))]),
                None,
            )],
        });
        let mut result_steps = vec![
            SmallStep::Set {
                name: done.clone(),
                value: lit(Value::Bool(false)),
            },
            SmallStep::Loop {
                times: timeout,
                body,
            },
        ];
        result_steps.push(SmallStep::If {
            cond: Condition::Equals {
                left: Expr::reference(done),
                right: lit(Value::Bool(false)),
            },
            then_steps: self.steps(else_steps)?,
            else_steps: Vec::new(),
        });
        Ok(SmallStep::If {
            cond: Condition::truthy(lit(Value::Bool(true))),
            then_steps: result_steps,
            else_steps: Vec::new(),
        })
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
        field_expr(map, "at", path)
    } else {
        expr_from_yaml(value, path)
    }
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
    Some(number.checked_mul(match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => return None,
    })?)
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

fn color_expr(value: &YamlValue, path: &str) -> Result<Expr, Vec<Diagnostic>> {
    let expr = expr_from_yaml(value, path)?;
    match expr {
        Expr::Literal(Value::String(value)) => {
            let normalized = value.trim_start_matches('#');
            if (normalized.len() == 3 || normalized.len() == 6)
                && normalized.chars().all(|ch| ch.is_ascii_hexdigit())
            {
                Ok(Expr::Literal(Value::Color(normalized.to_ascii_lowercase())))
            } else {
                Ok(Expr::Literal(Value::String(value)))
            }
        }
        other => Ok(other),
    }
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
            "version: 3\nsteps:\n  - find:\n      template: login\n      timeout: 1s\n      click: true\n  - check:\n      template: ready\n  - retry:\n      times: 3\n      steps:\n        - log: retry\n  - wait_for:\n      template: loaded\n  - click_when:\n      template: play\n  - match_first:\n      candidates: [a, b]\n  - color_branch:\n      at: [0.5, 0.5]\n      branches:\n        - ff0000: [{log: red}]\n",
        )
        .unwrap();
        let json = serde_json::to_string(&program).unwrap();
        for sugar in [
            "find",
            "check",
            "retry",
            "wait_for",
            "click_when",
            "match_first",
            "color_branch",
        ] {
            assert!(
                !json.contains(&format!("\"op\":\"{sugar}\"")),
                "sugar leaked: {sugar}"
            );
        }
        assert!(json.contains("vision.match"));
        assert!(json.contains("vision.match_many"));
        assert!(json.contains("vision.sample_color"));
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
            "version: 3\nsteps:\n  - set: {ready: true}\n  - if:\n      cond: $ready\n      then:\n        - loop:\n            times: 2\n            steps:\n              - break\n      else: []\n  - call:\n      target: helper\n      with: {arg: 1}\n      save: result\n  - invoke:\n      capability: plugin.value\n      with: {value: $result}\n      save: output\n  - throw: failed\n  - return: $output\n",
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
    fn color_branch_hex_literal_is_typed_for_native_color_records() {
        let program = load(
            "version: 3\nsteps:\n  - color_branch:\n      at: [0.5, 0.5]\n      branches:\n        - '#ff0000': [{log: red}]\n",
        )
        .unwrap();
        let json = serde_json::to_string(&program).unwrap();
        assert!(json.contains("\"type\":\"color\""));
    }
}
