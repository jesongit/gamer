//! 参数绑定共用的标量解析与校验（v3 参数链：声明默认值规整、实参收纳、
//! entrypoint schema 的 key 枚举）。
//!
//! 这些纯函数原属 v2 params 模块；v2 删除后由任务参数门禁
//! （[`crate::extensions::gamer_yaml::task_params`]）与 entrypoint 描述器
//! 共同消费，语义不变。

/// key 类型具名按键枚举（大小写不敏感；与前端 schema.KEY_ENUM、
/// docs/yaml-v3/steps.md 的 key 步骤枚举保持一致）。
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

/// key 值合法：具名枚举（大小写不敏感）或纯数字 Android keycode（须能解析为
/// u32）。
pub fn is_valid_key(s: &str) -> bool {
    s.parse::<u32>().is_ok() || KEY_NAMES.contains(&s.to_ascii_uppercase().as_str())
}

/// 坐标分量取值域 0~1。
pub fn coord_in_range(x: f64) -> bool {
    x.is_finite() && (0.0..=1.0).contains(&x)
}

/// 6 位十六进制颜色（无 #）。
pub fn is_valid_color(s: &str) -> bool {
    s.len() == 6 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// 时间书写串 → 毫秒数。单位 ms/s/m/min/h/d（m≡min，可小数），必须 > 0。
pub fn parse_time_ms(raw: &str) -> Option<f64> {
    let lower = raw.to_ascii_lowercase();
    // "min" 必须先于 "m" 尝试剥离。
    for unit in ["min", "ms", "s", "m", "h", "d"] {
        if let Some(num) = lower.strip_suffix(unit) {
            let x: f64 = num.parse().ok()?;
            if !x.is_finite() || x <= 0.0 {
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

/// 数字最短十进制表示（整数不带小数点）。
pub fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

/// YAML 双引号标量风格反转义（`\`、`"` 与常见控制字符）；悬空反斜杠返回
/// `None`。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_enum_accepts_names_case_insensitively_and_numeric_codes() {
        assert!(is_valid_key("home"));
        assert!(is_valid_key("APP_SWITCH"));
        assert!(is_valid_key("123"));
        assert!(!is_valid_key("nope"));
    }

    #[test]
    fn time_requires_unit_and_positive_value() {
        assert_eq!(parse_time_ms("500ms"), Some(500.0));
        assert_eq!(parse_time_ms("2min"), Some(120_000.0));
        assert_eq!(parse_time_ms("1.5s"), Some(1500.0));
        assert_eq!(parse_time_ms("0s"), None);
        assert_eq!(parse_time_ms("500"), None);
    }

    #[test]
    fn fmt_num_prefers_integer_form() {
        assert_eq!(fmt_num(3.0), "3");
        assert_eq!(fmt_num(0.5), "0.5");
    }

    #[test]
    fn unescape_roundtrip_rejects_dangling_escape() {
        assert_eq!(unescape_double_quoted(r#"a\"b"#), Some("a\"b".into()));
        assert_eq!(unescape_double_quoted("a\\x"), None);
    }
}
