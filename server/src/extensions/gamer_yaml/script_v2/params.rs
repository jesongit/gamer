//! 参数声明解析与类型化字面量（CONTRACT §3.3 / plan §6.1，规则全部冻结）。
//!
//! 声明格式 `类型:变量名:备注[:默认值]`：`splitn(4, ':')`，第三个冒号后整段为
//! 默认值尾串（text 默认值可含冒号）；类型/变量名/备注不得含半角冒号（由切分
//! 规则天然保证）；「整条单引号」样式校验在 loader 层完成（本模块只管内容）。

use std::time::Duration;

use super::error::codes;
use super::model::{ParamDecl, ParamType, TypedValue};

/// 参数声明解析错误（code 取自 CONTRACT §5.3 param 域）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclError {
    pub code: &'static str,
    pub field: &'static str,
    pub message: String,
}

fn decl_err(code: &'static str, field: &'static str, message: impl Into<String>) -> DeclError {
    DeclError {
        code,
        field,
        message: message.into(),
    }
}

/// 保留变量名：布尔/空字面量词与引擎内部 `gb_` 前缀。
const RESERVED_NAMES: &[&str] = &["true", "false", "null"];

/// 变量名 `[A-Za-z_][A-Za-z0-9_]*`。
pub fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 解析声明串（已保证为单引号标量的内容）。
pub fn parse_param_decl(raw: &str) -> Result<ParamDecl, DeclError> {
    let parts: Vec<&str> = raw.splitn(4, ':').collect();
    if parts.len() < 3 || parts[0].is_empty() || parts[1].is_empty() || parts[2].is_empty() {
        return Err(decl_err(
            codes::PARAM_DECL_FORMAT,
            "declaration",
            format!("声明 {raw:?} 不是 类型:变量名:备注[:默认值] 四段式，类型/变量名/备注不得为空"),
        ));
    }
    let Some(ty) = ParamType::parse(parts[0]) else {
        return Err(decl_err(
            codes::PARAM_DECL_FORMAT,
            "declaration",
            format!(
                "未知参数类型 {:?}，类型必须是 tmpl/coord/color/time/key/text/bool",
                parts[0]
            ),
        ));
    };
    let name = parts[1];
    if !is_valid_name(name) {
        return Err(decl_err(
            codes::PARAM_DECL_NAME_INVALID,
            "name",
            format!("变量名 {name:?} 不符合 [A-Za-z_][A-Za-z0-9_]*"),
        ));
    }
    if RESERVED_NAMES.contains(&name) || name.starts_with("gb_") {
        return Err(decl_err(
            codes::PARAM_DECL_NAME_INVALID,
            "name",
            format!("变量名 {name:?} 是保留名（true/false/null/gb_ 前缀）"),
        ));
    }
    let default = match parts.get(3) {
        None => None,
        Some(&"") => {
            return Err(decl_err(
                codes::PARAM_DEFAULT_EMPTY,
                "default",
                "空默认值：第四段尾串为空，不等价于没有默认值（text 空串须写 \"\"）",
            ));
        }
        Some(tail) => match parse_typed_default(ty, tail) {
            Ok(v) => Some(v),
            Err(message) => {
                return Err(decl_err(
                    codes::PARAM_DEFAULT_INVALID,
                    "default",
                    format!("默认值 {tail:?} 不能按类型 {} 解析：{message}", ty.as_str()),
                ));
            }
        },
    };
    Ok(ParamDecl {
        ty,
        name: name.to_string(),
        remark: parts[2].to_string(),
        default,
    })
}

/// 默认值尾串 → 类型化字面量。`Err` 携带原因（调用方统一报 param.default.invalid）。
pub fn parse_typed_default(ty: ParamType, tail: &str) -> Result<TypedValue, String> {
    match ty {
        ParamType::Tmpl => Ok(TypedValue::Tmpl(tail.to_string())),
        ParamType::Coord => {
            let inner = tail
                .trim()
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .ok_or_else(|| format!("coord 默认值必须是 [x, y]，得到 {tail:?}"))?;
            let mut nums = Vec::new();
            for part in inner.split(',') {
                let x: f64 = part
                    .trim()
                    .parse()
                    .map_err(|_| format!("coord 默认值含非法数字 {part:?}"))?;
                nums.push(x);
            }
            if nums.len() != 2 {
                return Err(format!("coord 默认值必须恰好两个数字，得到 {tail:?}"));
            }
            let [x, y] = [nums[0], nums[1]];
            if !coord_in_range(x) || !coord_in_range(y) {
                return Err(format!("coord 默认值分量必须在 0~1，得到 {tail:?}"));
            }
            Ok(TypedValue::Coord([x, y]))
        }
        ParamType::Color => {
            if tail.len() != 6 || !tail.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(format!("color 默认值必须是 6 位十六进制，得到 {tail:?}"));
            }
            Ok(TypedValue::Color(tail.to_string()))
        }
        ParamType::Time => {
            parse_time_ms(tail).ok_or_else(|| {
                format!("time 默认值必须带单位（ms/s/m/min/h/d）且 > 0，得到 {tail:?}")
            })?;
            Ok(TypedValue::Time(tail.to_string()))
        }
        ParamType::Key => {
            if !is_valid_key(tail) {
                return Err(invalid_key_reason(tail));
            }
            Ok(TypedValue::Key(tail.to_string()))
        }
        ParamType::Text => {
            // 双引号包裹形式：剥离外层引号并反转义（与规范序列化对称）。
            if tail.starts_with('"') && tail.ends_with('"') && tail.len() >= 2 {
                let inner = &tail[1..tail.len() - 1];
                let unescaped = unescape_double_quoted(inner)
                    .ok_or_else(|| format!("text 默认值 {tail:?} 的转义序列非法"))?;
                Ok(TypedValue::Text(unescaped))
            } else {
                Ok(TypedValue::Text(tail.to_string()))
            }
        }
        ParamType::Bool => match tail {
            "true" => Ok(TypedValue::Bool(true)),
            "false" => Ok(TypedValue::Bool(false)),
            other => Err(format!(
                "bool 默认值必须是字面 true/false（字符串 {other:?} 非法）"
            )),
        },
    }
}

/// 坐标分量取值域 0~1。
pub fn coord_in_range(x: f64) -> bool {
    x.is_finite() && (0.0..=1.0).contains(&x)
}

/// 6 位十六进制颜色（无 #）。
pub fn is_valid_color(s: &str) -> bool {
    s.len() == 6 && s.chars().all(|c| c.is_ascii_hexdigit())
}

// ---------------------------------------------------------------------------
// 按键
// ---------------------------------------------------------------------------

/// key 类型具名按键枚举（大小写不敏感；含 `engine::exec::key_code` 认可的别名
/// 拼写，与前端 schema.KEY_ENUM、docs/reference/YAML.md §5.1 保持一致）。
pub const KEY_NAMES: &[&str] = &[
    "HOME",
    "BACK",
    "APP_SWITCH",
    "RECENTS",
    "MENU",
    "VOL_UP",
    "VOLUME_UP",
    "VOL_DOWN",
    "VOLUME_DOWN",
    "POWER",
    "ENTER",
    "DEL",
    "BACKSPACE",
    "TAB",
    "SPACE",
    "ESC",
    "SEARCH",
    "CAMERA",
    "FOCUS",
    "NOTIFICATION",
    "SETTINGS",
    "MUTE",
    "HEADSETHOOK",
    "WAKEUP",
    "SLEEP",
];

/// key 值合法：具名枚举（大小写不敏感）或纯数字 Android keycode（与
/// `engine::exec::key_code` 的数字透传同规则，须能解析为 u32）。
pub fn is_valid_key(s: &str) -> bool {
    s.parse::<u32>().is_ok() || KEY_NAMES.contains(&s.to_ascii_uppercase().as_str())
}

/// 非法 key 值的统一报错原因（默认值 / 步骤字面量 / args 共用文案基调）。
pub fn invalid_key_reason(s: &str) -> String {
    format!(
        "未知按键 {s:?}（受支持枚举见 docs/reference/YAML.md §5.1：HOME/BACK/ESC 等具名键，或纯数字 Android keycode）"
    )
}

// ---------------------------------------------------------------------------
// 时间
// ---------------------------------------------------------------------------

/// 时间书写串 → 毫秒数。单位 ms/s/m/min/h/d（m≡min，可小数），必须 > 0。
pub fn parse_time_ms(raw: &str) -> Option<f64> {
    parse_time_ms_impl(raw, false)
}

/// check.timeout 专用时间解析：与普通时间相同，但允许 0 表示只检测一次。
pub fn parse_time_ms_allow_zero(raw: &str) -> Option<f64> {
    parse_time_ms_impl(raw, true)
}

fn parse_time_ms_impl(raw: &str, allow_zero: bool) -> Option<f64> {
    let lower = raw.to_ascii_lowercase();
    if allow_zero && lower == "0" {
        return Some(0.0);
    }
    // "min" 必须先于 "m" 尝试剥离。
    for unit in ["min", "ms", "s", "m", "h", "d"] {
        if let Some(num) = lower.strip_suffix(unit) {
            let x: f64 = num.parse().ok()?;
            if !x.is_finite() || (!allow_zero && x <= 0.0) || (allow_zero && x < 0.0) {
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

/// 时间书写串 → Duration（供 config.interval 与阶段 2 引擎使用）。
pub fn parse_time_duration(raw: &str) -> Option<Duration> {
    let ms = parse_time_ms(raw)?;
    if ms.fract() == 0.0 && ms <= u64::MAX as f64 {
        Some(Duration::from_millis(ms as u64))
    } else {
        Duration::try_from_secs_f64(ms / 1000.0).ok()
    }
}

/// check.timeout 专用 Duration 解析，允许零时长。
pub fn parse_time_duration_allow_zero(raw: &str) -> Option<Duration> {
    let ms = parse_time_ms_allow_zero(raw)?;
    if ms.fract() == 0.0 && ms <= u64::MAX as f64 {
        Some(Duration::from_millis(ms as u64))
    } else {
        Duration::try_from_secs_f64(ms / 1000.0).ok()
    }
}

/// Duration → 规范书写串：取能整除的最大单位（ms/s/m/h/d，"min" 归一为 "m"）。
pub fn fmt_duration(d: &Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms % 1000.0 == 0.0 {
        let s = ms / 1000.0;
        if s % 60.0 == 0.0 {
            let m = s / 60.0;
            if m % 60.0 == 0.0 {
                let h = m / 60.0;
                if h % 24.0 == 0.0 {
                    return format!("{}d", fmt_num(h / 24.0));
                }
                return format!("{}h", fmt_num(h));
            }
            return format!("{}m", fmt_num(m));
        }
        return format!("{}s", fmt_num(s));
    }
    format!("{}ms", fmt_num(ms))
}

/// 数字最短十进制表示（整数不带小数点）。
pub fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

// ---------------------------------------------------------------------------
// text 双引号转义（声明默认值与规范序列化共用，保证往返对称）
// ---------------------------------------------------------------------------

/// YAML 双引号标量风格转义（`\`、`"` 与常见控制字符）。
pub fn escape_double_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// [`escape_double_quoted`] 的逆操作；悬空反斜杠返回 `None`。
pub fn unescape_double_quoted(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match chars.next()? {
            '\\' => '\\',
            '"' => '"',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            _ => return None,
        });
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// args 绑定（CONTRACT §4.3：声明默认值 → 显式覆盖，绑定后类型校验）
// ---------------------------------------------------------------------------

/// 稀疏 args 合并：以声明默认值打底，显式覆盖；必填缺失报错。
///
/// `overrides` 应为已经过校验的类型化值（validate 层负责 `$name` 引用解析与
/// 字面量按目标类型重定型）；此处做绑定后最终类型校验，供阶段 2 引擎与
/// 任务快照解析共用。
pub fn merge_args(
    decls: &[ParamDecl],
    overrides: impl IntoIterator<Item = (String, TypedValue)>,
    resource: &str,
) -> Result<Vec<(String, TypedValue)>, Vec<super::error::ScriptError>> {
    use super::error::ScriptError;
    let overrides: Vec<(String, TypedValue)> = overrides.into_iter().collect();
    let mut errors = Vec::new();
    let mut bound: Vec<(String, TypedValue)> = Vec::with_capacity(decls.len());
    for decl in decls {
        let explicit = overrides.iter().find(|(name, _)| *name == decl.name);
        let value = match explicit {
            Some((_, v)) => {
                if v.param_type() != decl.ty {
                    errors.push(
                        ScriptError::new(
                            codes::PARAM_ARGS_TYPE_MISMATCH,
                            format!(
                                "参数 {} 的实参类型与声明 {} 不符",
                                decl.name,
                                decl.ty.as_str()
                            ),
                            resource,
                        )
                        .at(format!("args.{}", decl.name), "args"),
                    );
                    continue;
                }
                v.clone()
            }
            None => match &decl.default {
                Some(v) => v.clone(),
                None => {
                    errors.push(
                        ScriptError::new(
                            codes::PARAM_ARGS_MISSING_REQUIRED,
                            format!("必填参数 {} 未提供", decl.name),
                            resource,
                        )
                        .at("args", "args"),
                    );
                    continue;
                }
            },
        };
        bound.push((decl.name.clone(), value));
    }
    for (name, _) in &overrides {
        if !decls.iter().any(|d| &d.name == name) {
            errors.push(
                ScriptError::new(
                    codes::PARAM_ARGS_UNKNOWN,
                    format!("args 键 {name:?} 不是目标参数"),
                    resource,
                )
                .at(format!("args.{name}"), "args"),
            );
        }
    }
    if errors.is_empty() {
        Ok(bound)
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// API JSON args（手动运行 / 函数测试 / 阶段 5 任务快照）
// ---------------------------------------------------------------------------

/// 七类参数值从 API JSON 解析为类型化字面量：bool=布尔、coord=[x,y] 数组
/// （0~1）、其余五类=字符串（time 须带单位且 >0，color 须 6 位十六进制，
/// tmpl 非空、key 须在按键枚举内或为纯数字 keycode）。形态不符返回 `None`
/// （调用方报 param.args.type_mismatch）。
pub fn parse_json_arg(ty: ParamType, value: &serde_json::Value) -> Option<TypedValue> {
    match (ty, value) {
        (ParamType::Tmpl, serde_json::Value::String(s)) => {
            (!s.is_empty()).then(|| TypedValue::Tmpl(s.clone()))
        }
        (ParamType::Coord, serde_json::Value::Array(items)) if items.len() == 2 => {
            let x = items[0].as_f64()?;
            let y = items[1].as_f64()?;
            (coord_in_range(x) && coord_in_range(y)).then_some(TypedValue::Coord([x, y]))
        }
        (ParamType::Color, serde_json::Value::String(s)) => {
            is_valid_color(s).then(|| TypedValue::Color(s.clone()))
        }
        (ParamType::Time, serde_json::Value::String(s)) => {
            parse_time_ms(s).map(|_| TypedValue::Time(s.clone()))
        }
        (ParamType::Key, serde_json::Value::String(s)) => {
            is_valid_key(s).then(|| TypedValue::Key(s.clone()))
        }
        (ParamType::Text, serde_json::Value::String(s)) => Some(TypedValue::Text(s.clone())),
        (ParamType::Bool, serde_json::Value::Bool(b)) => Some(TypedValue::Bool(*b)),
        _ => None,
    }
}

/// 稀疏 JSON args（键 → 任意 JSON 值）按声明解析为类型化稀疏覆盖：
/// 未知键 → `param.args.unknown`；形态与声明类型不符 → `param.args.type_mismatch`。
pub fn parse_json_args(
    decls: &[ParamDecl],
    args: &serde_json::Map<String, serde_json::Value>,
    resource: &str,
) -> Result<Vec<(String, TypedValue)>, Vec<super::error::ScriptError>> {
    use super::error::ScriptError;
    let mut errors = Vec::new();
    let mut out = Vec::new();
    for (name, value) in args {
        let Some(decl) = decls.iter().find(|d| &d.name == name) else {
            errors.push(
                ScriptError::new(
                    codes::PARAM_ARGS_UNKNOWN,
                    format!("args 键 {name:?} 不是目标参数"),
                    resource,
                )
                .at(format!("args.{name}"), "args"),
            );
            continue;
        };
        match parse_json_arg(decl.ty, value) {
            Some(v) => out.push((name.clone(), v)),
            None => errors.push(
                ScriptError::new(
                    codes::PARAM_ARGS_TYPE_MISMATCH,
                    format!(
                        "参数 {name} 的实参 {value} 与声明类型 {} 不符",
                        decl.ty.as_str()
                    ),
                    resource,
                )
                .at(format!("args.{name}"), "args"),
            ),
        }
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}
