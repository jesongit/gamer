//! Pure YAML shape validation and migration diagnostics.

use serde_yaml::Value;

/// Validate an explicit top-level mapping before shorthand normalization.
/// Returns whether any explicit section key is present.
pub(super) fn validate_top_mapping(mapping: &serde_yaml::Mapping) -> anyhow::Result<bool> {
    for key in mapping.keys() {
        match key.as_str() {
            Some("action_wait") => anyhow::bail!(
                "顶层 action_wait 已删除：操作间隔统一为 config interval（仅轮询类等待，步骤间不再等待）"
            ),
            Some("log_level") => anyhow::bail!(
                "顶层 log_level 已删除：改用 config: 段（config.toml 可配全局默认）"
            ),
            Some("name") => anyhow::bail!("顶层 name 已删除（脚本名即文件名）"),
            _ => {}
        }
    }

    let has_section = mapping
        .keys()
        .any(|key| matches!(key.as_str(), Some("config" | "func" | "steps")));
    if has_section {
        for key in mapping.keys() {
            if !matches!(key.as_str(), Some("config" | "func" | "steps")) {
                anyhow::bail!(
                    "未知顶层键 {:?}（只支持 config / func / steps；单段简写：顶层序列 = steps，无段落键的顶层映射 = func）",
                    key.as_str()
                );
            }
        }
    } else {
        for key in mapping.keys() {
            if matches!(key.as_str(), Some("interval" | "threshold")) {
                anyhow::bail!(
                    "顶层 {:?} 是 config: 段参数（省略段落键的简写只支持纯 steps 序列或纯 func 函数定义，config 必须写 config: 键）",
                    key.as_str()
                );
            }
        }
    }
    Ok(has_section)
}

pub(super) fn ensure_only_keys(step: &Value, action: &str, allowed: &[&str]) -> anyhow::Result<()> {
    let mapping = step.as_mapping().unwrap();
    for key in mapping.keys() {
        let Some(name) = key.as_str() else {
            anyhow::bail!("{} 不支持非字符串参数键（旧数组键写法已删除）", action);
        };
        if !allowed.contains(&name) {
            anyhow::bail!(
                "{} 不支持参数 {}（可用：{}）",
                action,
                name,
                allowed.join(" / ")
            );
        }
    }
    Ok(())
}

pub(super) fn ensure_bare_value(step: &Value, action: &str) -> anyhow::Result<()> {
    match step.get(action) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(value)) if value.trim().is_empty() => Ok(()),
        Some(_) => anyhow::bail!(
            "{} 不支持参数：应用包名固定为设备分区（设备配置 pkg）",
            action
        ),
    }
}

/// Produce a targeted error for scalar steps such as `- throw reason` where
/// YAML requires a colon before the value.
pub(super) fn missing_colon_hint(names: &[String]) -> Option<String> {
    const VALUE_ACTIONS: [&str; 14] = [
        "log", "key", "text", "tap", "swipe", "find", "color", "loop", "call", "throw", "str_app",
        "cls_app", "wait", "return",
    ];
    for name in names {
        let trimmed = name.trim();
        for action in VALUE_ACTIONS {
            if let Some(rest) = trimmed.strip_prefix(action) {
                if rest.starts_with(char::is_whitespace) {
                    return Some(format!(
                        "\"{}\" 是标量步骤（YAML 把 \"- {}\" 解析成字符串）——带值/带原因的动作需写冒号：应为 \"- {}: {}\"（裸写仅限无参动作，如 - str_app / - throw）",
                        name,
                        name,
                        action,
                        rest.trim_start()
                    ));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_errors_remain_targeted() {
        let action_wait: Value = serde_yaml::from_str("action_wait: 1s").unwrap();
        let error = validate_top_mapping(action_wait.as_mapping().unwrap()).unwrap_err();
        assert!(error.to_string().contains("action_wait 已删除"));

        let mixed: Value = serde_yaml::from_str("steps: []\nother: true").unwrap();
        let error = validate_top_mapping(mixed.as_mapping().unwrap()).unwrap_err();
        assert!(error.to_string().contains("未知顶层键"));

        let bare_config: Value = serde_yaml::from_str("interval: 500ms").unwrap();
        let error = validate_top_mapping(bare_config.as_mapping().unwrap()).unwrap_err();
        assert!(error.to_string().contains("config: 段参数"));
    }

    #[test]
    fn action_shape_diagnostics_are_independent_from_execution() {
        let step: Value = serde_yaml::from_str("tap: [0.5, 0.5]\nwait: 1s").unwrap();
        let error = ensure_only_keys(&step, "tap", &["tap"]).unwrap_err();
        assert!(error.to_string().contains("tap 不支持参数 wait"));

        assert_eq!(
            missing_colon_hint(&["throw reason".into()]).unwrap(),
            "\"throw reason\" 是标量步骤（YAML 把 \"- throw reason\" 解析成字符串）——带值/带原因的动作需写冒号：应为 \"- throw: reason\"（裸写仅限无参动作，如 - str_app / - throw）"
        );
    }
}
