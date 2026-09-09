//! gamer.yaml 原生（插件）函数注册表——V1 函数库两种来源之一（计划 Phase 3.2）。
//!
//! 原生函数在宿主（扩展边界）内以 Rust 实现，组合 Core capability 原语；
//! 解释器经 `__fn` 通道按名派发（`tap`、`find`、`sleep` 等不是语法关键字）。
//! Schema（名称/描述/参数/返回/权限）是执行校验、entrypoint 提示与前端
//! 参数表单的共同来源；新增函数只改本表 + 一个 handler，不动解释器。
//!
//! Package 函数（`automations/_function*.yaml`，简化计划 Phase 1 前缀识别）
//! 是另一种来源，由 YAML 写成、解释器本地执行；两种来源调用语法一致，
//! 同名即冲突（组合期拒绝）。

use serde_json::{json, Value};

use crate::extensions::permissions::Permission;

use super::syntax::ParamType;

/// 参数 Schema（静态表形态）。
pub struct ParamSchema {
    pub name: &'static str,
    pub ty: ParamType,
    pub required: bool,
    pub default: Option<Value>,
    pub desc: &'static str,
}

/// 原生函数声明。
pub struct NativeFunction {
    pub name: &'static str,
    pub description: &'static str,
    pub params: Vec<ParamSchema>,
    pub returns: &'static str,
    /// 运行前必须通过的插件权限（解释器调用不绕过权限）。
    pub permissions: &'static [Permission],
}

fn p(
    name: &'static str,
    ty: ParamType,
    required: bool,
    default: Option<Value>,
    desc: &'static str,
) -> ParamSchema {
    ParamSchema {
        name,
        ty,
        required,
        default,
        desc,
    }
}

const RETURN_NULL: &str = "null";
const RETURN_MATCH: &str = "match?";
const RETURN_BOOL: &str = "boolean";

pub(crate) fn native_functions() -> &'static [NativeFunction] {
    static FUNCTIONS: std::sync::LazyLock<Vec<NativeFunction>> = std::sync::LazyLock::new(|| {
        vec![
            NativeFunction {
                name: "tap",
                description: "点击相对坐标（0..1；可传 match 的 center）",
                params: vec![p(
                    "position",
                    ParamType::Point,
                    true,
                    None,
                    "目标点 [x, y] 或 {x, y}",
                )],
                returns: RETURN_NULL,
                permissions: &[Permission::InputTap],
            },
            NativeFunction {
                name: "swipe",
                description: "从起点滑动到终点",
                params: vec![
                    p("from", ParamType::Point, true, None, "起点"),
                    p("to", ParamType::Point, true, None, "终点"),
                    p(
                        "duration",
                        ParamType::Duration,
                        false,
                        Some(json!("300ms")),
                        "滑动时长",
                    ),
                ],
                returns: RETURN_NULL,
                permissions: &[Permission::InputSwipe],
            },
            NativeFunction {
                name: "key",
                description: "发送按键（HOME/BACK/…或数字 keycode）",
                params: vec![
                    p("key", ParamType::Key, true, None, "按键名或 keycode"),
                    p(
                        "action",
                        ParamType::String,
                        false,
                        Some(json!("press")),
                        "press/down/up",
                    ),
                ],
                returns: RETURN_NULL,
                permissions: &[Permission::InputKey],
            },
            NativeFunction {
                name: "input_text",
                description: "向设备输入文本",
                params: vec![p("text", ParamType::String, true, None, "文本内容")],
                returns: RETURN_NULL,
                permissions: &[Permission::InputText],
            },
            NativeFunction {
                name: "launch",
                description: "冷启动应用（缺省为设备配置的应用）",
                params: vec![p(
                    "package",
                    ParamType::String,
                    false,
                    None,
                    "Android 包名，缺省用设备配置",
                )],
                returns: RETURN_NULL,
                permissions: &[Permission::DeviceApp],
            },
            NativeFunction {
                name: "stop_app",
                description: "停止应用（缺省为设备配置的应用）",
                params: vec![p(
                    "package",
                    ParamType::String,
                    false,
                    None,
                    "Android 包名，缺省用设备配置",
                )],
                returns: RETURN_NULL,
                permissions: &[Permission::DeviceApp],
            },
            NativeFunction {
                name: "sleep",
                description: "等待指定时长（取消可达）",
                params: vec![p(
                    "duration",
                    ParamType::Duration,
                    true,
                    None,
                    "如 500ms / 1.5s / 2min",
                )],
                returns: RETURN_NULL,
                permissions: &[Permission::RuntimeSleep],
            },
            NativeFunction {
                name: "log",
                description: "写运行日志（非字符串值自动转 JSON 文本）",
                params: vec![
                    p("message", ParamType::Any, true, None, "日志内容"),
                    p(
                        "level",
                        ParamType::String,
                        false,
                        Some(json!("info")),
                        "info/debug/warn/error",
                    ),
                ],
                returns: RETURN_NULL,
                permissions: &[Permission::LogWrite],
            },
            NativeFunction {
                name: "find",
                description: "单次模板匹配；未找到返回 null（等待轮询用 wait_find）",
                params: vec![
                    p("template", ParamType::Template, true, None, "模板短名"),
                    p(
                        "threshold",
                        ParamType::Number,
                        false,
                        Some(json!(0.8)),
                        "匹配阈值 0..1",
                    ),
                    p(
                        "region",
                        ParamType::List,
                        false,
                        None,
                        "搜索区域 [x, y, w, h]（相对坐标）",
                    ),
                ],
                returns: RETURN_MATCH,
                permissions: &[Permission::VisionMatch, Permission::ResourceRead],
            },
            NativeFunction {
                name: "wait_find",
                description: "等待模板出现；超时返回 null",
                params: vec![
                    p("template", ParamType::Template, true, None, "模板短名"),
                    p(
                        "threshold",
                        ParamType::Number,
                        false,
                        Some(json!(0.8)),
                        "匹配阈值 0..1",
                    ),
                    p(
                        "timeout",
                        ParamType::Duration,
                        false,
                        Some(json!("30s")),
                        "等待上限",
                    ),
                    p(
                        "interval",
                        ParamType::Duration,
                        false,
                        Some(json!("250ms")),
                        "轮询间隔",
                    ),
                    p(
                        "region",
                        ParamType::List,
                        false,
                        None,
                        "搜索区域 [x, y, w, h]（相对坐标）",
                    ),
                ],
                returns: RETURN_MATCH,
                permissions: &[Permission::VisionMatch, Permission::ResourceRead],
            },
            NativeFunction {
                name: "tap_template",
                description: "查找模板并点击其中心（未找到不点击，返回 null）",
                params: vec![
                    p("template", ParamType::Template, true, None, "模板短名"),
                    p(
                        "threshold",
                        ParamType::Number,
                        false,
                        Some(json!(0.8)),
                        "匹配阈值 0..1",
                    ),
                    p(
                        "timeout",
                        ParamType::Duration,
                        false,
                        Some(json!("0ms")),
                        "轮询上限；0 = 只试一次",
                    ),
                    p(
                        "interval",
                        ParamType::Duration,
                        false,
                        Some(json!("100ms")),
                        "轮询间隔",
                    ),
                    p(
                        "region",
                        ParamType::List,
                        false,
                        None,
                        "搜索区域 [x, y, w, h]（相对坐标）",
                    ),
                ],
                returns: RETURN_MATCH,
                permissions: &[
                    Permission::VisionMatch,
                    Permission::ResourceRead,
                    Permission::InputTap,
                ],
            },
            NativeFunction {
                name: "wait_disappear",
                description: "等待模板消失；超时仍存在返回 false",
                params: vec![
                    p("template", ParamType::Template, true, None, "模板短名"),
                    p(
                        "threshold",
                        ParamType::Number,
                        false,
                        Some(json!(0.8)),
                        "匹配阈值 0..1",
                    ),
                    p(
                        "timeout",
                        ParamType::Duration,
                        false,
                        Some(json!("30s")),
                        "等待上限",
                    ),
                    p(
                        "interval",
                        ParamType::Duration,
                        false,
                        Some(json!("250ms")),
                        "轮询间隔",
                    ),
                    p(
                        "region",
                        ParamType::List,
                        false,
                        None,
                        "搜索区域 [x, y, w, h]（相对坐标）",
                    ),
                ],
                returns: RETURN_BOOL,
                permissions: &[Permission::VisionMatch, Permission::ResourceRead],
            },
            NativeFunction {
                name: "eq",
                description: "相等比较（数字跨整型/浮点，其余按值）",
                params: vec![
                    p("a", ParamType::Any, true, None, "左值"),
                    p("b", ParamType::Any, true, None, "右值"),
                ],
                returns: RETURN_BOOL,
                permissions: &[],
            },
            NativeFunction {
                name: "ne",
                description: "不等比较",
                params: vec![
                    p("a", ParamType::Any, true, None, "左值"),
                    p("b", ParamType::Any, true, None, "右值"),
                ],
                returns: RETURN_BOOL,
                permissions: &[],
            },
            NativeFunction {
                name: "gt",
                description: "大于（仅数字）",
                params: vec![
                    p("a", ParamType::Number, true, None, "左值"),
                    p("b", ParamType::Number, true, None, "右值"),
                ],
                returns: RETURN_BOOL,
                permissions: &[],
            },
            NativeFunction {
                name: "ge",
                description: "大于等于（仅数字）",
                params: vec![
                    p("a", ParamType::Number, true, None, "左值"),
                    p("b", ParamType::Number, true, None, "右值"),
                ],
                returns: RETURN_BOOL,
                permissions: &[],
            },
            NativeFunction {
                name: "lt",
                description: "小于（仅数字）",
                params: vec![
                    p("a", ParamType::Number, true, None, "左值"),
                    p("b", ParamType::Number, true, None, "右值"),
                ],
                returns: RETURN_BOOL,
                permissions: &[],
            },
            NativeFunction {
                name: "le",
                description: "小于等于（仅数字）",
                params: vec![
                    p("a", ParamType::Number, true, None, "左值"),
                    p("b", ParamType::Number, true, None, "右值"),
                ],
                returns: RETURN_BOOL,
                permissions: &[],
            },
        ]
    });
    &FUNCTIONS
}

/// 按名查原生函数。
pub fn native_function(name: &str) -> Option<&'static NativeFunction> {
    native_functions().iter().find(|func| func.name == name)
}

/// 原生函数名集合（函数注册表组合用）。
pub fn native_names() -> std::collections::BTreeSet<String> {
    native_functions()
        .iter()
        .map(|func| func.name.to_string())
        .collect()
}

/// 原生函数 Schema → descriptor JSON（`GET /api/extensions` 函数清单与
/// entrypoint 提示共用形态）。
pub fn native_schema_json(func: &NativeFunction) -> Value {
    json!({
        "name": func.name,
        "description": func.description,
        "source": "plugin",
        "params": func.params.iter().map(|param| json!({
            "name": param.name,
            "type": param.ty.canonical(),
            "required": param.required,
            "default": param.default,
            "desc": param.desc,
        })).collect::<Vec<_>>(),
        "returns": func.returns,
    })
}
