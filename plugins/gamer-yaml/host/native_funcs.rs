//! gamer-yaml 原生（插件）函数注册表——V1 函数库两种来源之一（计划 Phase 3.2）。
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
    pub item_type: Option<ParamType>,
}

/// 原生函数声明。
pub struct NativeFunction {
    pub name: &'static str,
    pub display_name: &'static str,
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
        item_type: None,
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
        let mut functions = vec![
            NativeFunction {
                name: "tap",
                display_name: "点击",
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
                display_name: "滑动",
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
                display_name: "按键",
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
                display_name: "输入文本",
                description: "向设备输入文本",
                params: vec![p("text", ParamType::String, true, None, "文本内容")],
                returns: RETURN_NULL,
                permissions: &[Permission::InputText],
            },
            NativeFunction {
                name: "launch",
                display_name: "启动应用",
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
                display_name: "停止应用",
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
                display_name: "等待",
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
                display_name: "日志",
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
                display_name: "查找模板",
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
                name: "find_any",
                display_name: "查找首个模板",
                description:
                    "共用一帧按顺序匹配，返回首个命中及 index，不点击；全部未命中返回 null",
                params: vec![
                    ParamSchema {
                        item_type: Some(ParamType::Template),
                        ..p(
                            "templates",
                            ParamType::List,
                            true,
                            None,
                            "按优先级排序的模板列表（1..64 项）",
                        )
                    },
                    p(
                        "threshold",
                        ParamType::Number,
                        false,
                        Some(json!(0.8)),
                        "匹配阈值 0..1",
                    ),
                ],
                returns: RETURN_MATCH,
                permissions: &[Permission::VisionMatch, Permission::ResourceRead],
            },
            NativeFunction {
                name: "wait_find",
                display_name: "等待模板出现",
                description: "等待模板出现，默认点击命中中心（click: false 仅等待）；超时返回 null",
                params: vec![
                    p("template", ParamType::Template, true, None, "模板短名"),
                    ParamSchema {
                        item_type: Some(ParamType::Template),
                        ..p(
                            "obstacles",
                            ParamType::List,
                            false,
                            Some(json!([])),
                            "障碍模板：按顺序命中首个就点击，下轮重新检查；计入总超时",
                        )
                    },
                    p(
                        "click",
                        ParamType::Boolean,
                        false,
                        Some(json!(true)),
                        "命中后是否点击模板中心（前后延迟使用自动化设置）",
                    ),
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
                        Some(json!(format!("{}s", super::settings::DEFAULT_TIMEOUT_SECS))),
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
                display_name: "点击模板",
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
                        Some(json!(format!("{}s", super::settings::DEFAULT_TIMEOUT_SECS))),
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
                display_name: "等待模板消失",
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
                        Some(json!(format!("{}s", super::settings::DEFAULT_TIMEOUT_SECS))),
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
                display_name: "等于",
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
                display_name: "不等于",
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
                display_name: "大于",
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
                display_name: "大于等于",
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
                display_name: "小于",
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
                display_name: "小于等于",
                description: "小于等于（仅数字）",
                params: vec![
                    p("a", ParamType::Number, true, None, "左值"),
                    p("b", ParamType::Number, true, None, "右值"),
                ],
                returns: RETURN_BOOL,
                permissions: &[],
            },
        ];
        for func in &mut functions {
            func.params.push(p(
                "name",
                ParamType::String,
                false,
                Some(json!(func.display_name)),
                "可视化显示名称",
            ));
        }
        functions
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
        "params": func.params.iter().map(|param| {
            let mut schema = json!({
            "name": param.name,
            "type": param.ty.canonical(),
            "required": param.required,
            "default": param.default,
            "desc": param.desc,
            });
            if let Some(item_type) = &param.item_type {
                schema["items"] = json!({"type": item_type.canonical()});
            }
            schema
        }).collect::<Vec<_>>(),
        "returns": func.returns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_functions_have_display_names_and_ten_second_timeouts() {
        let mut timeouts = 0;
        for function in native_functions() {
            let names: Vec<_> = function
                .params
                .iter()
                .filter(|p| p.name == "name")
                .collect();
            assert_eq!(names.len(), 1, "{}", function.name);
            assert_eq!(names[0].default, Some(json!(function.display_name)));
            assert_eq!(names[0].ty, ParamType::String);
            assert!(!names[0].required);
            assert_ne!(function.params[0].name, "name", "位置简写不能变");
            for param in function.params.iter().filter(|p| p.name == "timeout") {
                assert_eq!(param.default, Some(json!("10s")), "{}", function.name);
                timeouts += 1;
            }
        }
        assert_eq!(timeouts, 3);
    }
}
