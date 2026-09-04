//! Keymap YAML storage, validation, and canonical serialization.
//!
//! Keymaps deliberately use a schema separate from the script-v2 loader.  They are
//! stored below `data/<package>/keymaps/` and never fall back to another package.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_yaml::{Mapping, Value};

use crate::core::fs::{atomic_write, content_version, safe_name};

pub const MAX_KEYMAP_YAML_BYTES: usize = 1024 * 1024;
const MAX_SWIPE_DURATION_MS: u32 = 60_000;
const MAX_ANDROID_KEYCODE: u32 = 1_000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Keymap {
    pub version: u32,
    pub name: String,
    pub bindings: Vec<KeymapBinding>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct KeymapBinding {
    pub key: String,
    pub action: KeymapAction,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum KeymapAction {
    #[serde(rename = "tap")]
    Tap { at: [f64; 2] },
    #[serde(rename = "swipe")]
    Swipe {
        from: [f64; 2],
        to: [f64; 2],
        duration_ms: u32,
    },
    #[serde(rename = "raw_key")]
    RawKey {
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        keycode: Option<u32>,
    },
    /// A stateful single-point touch binding.  The pointer id is assigned at
    /// runtime and is intentionally not part of the persisted keymap schema.
    #[serde(rename = "hold")]
    Hold { at: [f64; 2] },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KeymapDiagnostic {
    pub code: String,
    pub message: String,
    pub resource: String,
    pub step_path: String,
    pub field: String,
}

impl KeymapDiagnostic {
    fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        resource: &str,
        step_path: impl Into<String>,
        field: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            resource: resource.to_string(),
            step_path: step_path.into(),
            field: field.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct KeymapSummary {
    pub id: String,
    pub package: String,
    pub pkg: String,
    /// Stored filename, including `.yaml`/`.yml`.
    pub file: String,
    /// Display name from the YAML document.  Invalid files use the filename stem.
    pub name: String,
    pub version: String,
    pub binding_count: usize,
    pub updated_at: String,
    pub valid: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<KeymapDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeymapFile {
    pub id: String,
    pub package: String,
    pub pkg: String,
    /// Stored filename, including `.yaml`.
    pub file: String,
    pub name: String,
    pub content: String,
    pub version: String,
    pub binding_count: usize,
    pub keymap: Keymap,
    pub updated_at: String,
}

/// Parse a keymap YAML document and return the typed model only after all
/// schema, field, coordinate, duration, and duplicate-key checks pass.
pub fn parse_keymap_content(
    content: &str,
    resource: &str,
) -> Result<Keymap, Vec<KeymapDiagnostic>> {
    if content.len() > MAX_KEYMAP_YAML_BYTES {
        return Err(vec![diag(
            "keymap.yaml.too_large",
            format!(
                "映射 YAML 超过 {} MiB",
                MAX_KEYMAP_YAML_BYTES / (1024 * 1024)
            ),
            resource,
            "",
            "",
        )]);
    }
    let root = match serde_yaml::from_str::<Value>(content) {
        Ok(value) => value,
        Err(error) => {
            return Err(vec![diag(
                "keymap.yaml.syntax",
                format!("YAML 语法错误: {error}"),
                resource,
                "",
                "",
            )])
        }
    };
    let Some(root_map) = root.as_mapping() else {
        return Err(vec![diag(
            "keymap.root.type",
            "映射配置根节点必须是对象",
            resource,
            "",
            "",
        )]);
    };

    let mut diagnostics = Vec::new();
    check_unknown_fields(
        root_map,
        &["version", "name", "bindings"],
        resource,
        "",
        "",
        "keymap.top_level.unknown_key",
        &mut diagnostics,
    );
    let version = match value_for(root_map, "version") {
        Some(value) => match value.as_u64() {
            Some(1) => 1,
            Some(other) => {
                diagnostics.push(diag(
                    "keymap.version.invalid",
                    format!("version 必须是 1，得到 {other}"),
                    resource,
                    "",
                    "version",
                ));
                1
            }
            None => {
                diagnostics.push(diag(
                    "keymap.version.invalid",
                    "version 必须是整数 1",
                    resource,
                    "",
                    "version",
                ));
                1
            }
        },
        None => {
            diagnostics.push(diag(
                "keymap.version.missing",
                "缺少必需字段 version",
                resource,
                "",
                "version",
            ));
            1
        }
    };

    let name = match value_for(root_map, "name") {
        Some(Value::String(value)) if valid_display_name(value) => value.trim().to_string(),
        Some(Value::String(_)) => {
            diagnostics.push(diag(
                "keymap.name.invalid",
                "name 不能为空、不能含控制字符且长度不能超过 255 字节",
                resource,
                "",
                "name",
            ));
            String::new()
        }
        Some(_) => {
            diagnostics.push(diag(
                "keymap.name.invalid",
                "name 必须是字符串",
                resource,
                "",
                "name",
            ));
            String::new()
        }
        None => {
            diagnostics.push(diag(
                "keymap.name.missing",
                "缺少必需字段 name",
                resource,
                "",
                "name",
            ));
            String::new()
        }
    };

    let bindings = match value_for(root_map, "bindings") {
        Some(Value::Sequence(items)) => parse_bindings(items, resource, &mut diagnostics),
        Some(_) => {
            diagnostics.push(diag(
                "keymap.bindings.type",
                "bindings 必须是列表",
                resource,
                "bindings",
                "bindings",
            ));
            Vec::new()
        }
        None => {
            diagnostics.push(diag(
                "keymap.bindings.missing",
                "缺少必需字段 bindings",
                resource,
                "bindings",
                "bindings",
            ));
            Vec::new()
        }
    };

    if diagnostics.is_empty() {
        Ok(Keymap {
            version,
            name,
            bindings,
        })
    } else {
        Err(diagnostics)
    }
}

pub fn serialize_keymap(keymap: &Keymap) -> anyhow::Result<String> {
    let content = serde_yaml::to_string(keymap)?;
    Ok(if content.ends_with('\n') {
        content
    } else {
        format!("{content}\n")
    })
}

fn parse_bindings(
    items: &[Value],
    resource: &str,
    diagnostics: &mut Vec<KeymapDiagnostic>,
) -> Vec<KeymapBinding> {
    let mut bindings = Vec::with_capacity(items.len());
    let mut seen_keys = HashSet::new();
    for (index, item) in items.iter().enumerate() {
        let path = format!("bindings[{index}]");
        let Some(map) = item.as_mapping() else {
            diagnostics.push(diag(
                "keymap.binding.type",
                "绑定项必须是对象",
                resource,
                &path,
                &path,
            ));
            continue;
        };
        check_unknown_fields(
            map,
            &["key", "action"],
            resource,
            &path,
            &path,
            "keymap.binding.unknown_key",
            diagnostics,
        );
        let key = match value_for(map, "key") {
            Some(Value::String(value)) if valid_key_code(value) && known_input_code(value) => {
                value.clone()
            }
            Some(Value::String(_)) => {
                diagnostics.push(diag(
                    "keymap.binding.key.invalid",
                    "key 必须是受支持的 KeyboardEvent.code（仅允许已知物理按键）",
                    resource,
                    &path,
                    format!("{path}.key"),
                ));
                String::new()
            }
            Some(_) => {
                diagnostics.push(diag(
                    "keymap.binding.key.invalid",
                    "key 必须是字符串",
                    resource,
                    &path,
                    format!("{path}.key"),
                ));
                String::new()
            }
            None => {
                diagnostics.push(diag(
                    "keymap.binding.key.missing",
                    "绑定缺少必需字段 key",
                    resource,
                    &path,
                    format!("{path}.key"),
                ));
                String::new()
            }
        };
        if !key.is_empty() && !seen_keys.insert(key.clone()) {
            diagnostics.push(diag(
                "keymap.binding.duplicate_key",
                format!("按键 {key} 重复绑定；同一方案内每个物理按键只能出现一次"),
                resource,
                &path,
                format!("{path}.key"),
            ));
        }

        let action = match value_for(map, "action") {
            Some(action) => parse_action(action, resource, &path, diagnostics),
            None => {
                diagnostics.push(diag(
                    "keymap.action.missing",
                    "绑定缺少必需字段 action",
                    resource,
                    &path,
                    format!("{path}.action"),
                ));
                None
            }
        };
        if let Some(action) = action {
            bindings.push(KeymapBinding { key, action });
        }
    }
    bindings
}

fn parse_action(
    value: &Value,
    resource: &str,
    binding_path: &str,
    diagnostics: &mut Vec<KeymapDiagnostic>,
) -> Option<KeymapAction> {
    let field_path = format!("{binding_path}.action");
    let Some(map) = value.as_mapping() else {
        diagnostics.push(diag(
            "keymap.action.type",
            "action 必须是对象",
            resource,
            binding_path,
            &field_path,
        ));
        return None;
    };
    let action_type = match value_for(map, "type") {
        Some(Value::String(value)) => value.as_str(),
        Some(_) => {
            diagnostics.push(diag(
                "keymap.action.type.invalid",
                "action.type 必须是字符串",
                resource,
                binding_path,
                format!("{field_path}.type"),
            ));
            return None;
        }
        None => {
            diagnostics.push(diag(
                "keymap.action.type.missing",
                "action 缺少必需字段 type",
                resource,
                binding_path,
                format!("{field_path}.type"),
            ));
            return None;
        }
    };

    let allowed: &[&str] = match action_type {
        "tap" => &["type", "at"],
        "swipe" => &["type", "from", "to", "duration_ms"],
        "raw_key" => &["type", "code", "keycode"],
        "hold" => &["type", "at"],
        other => {
            diagnostics.push(diag(
                "keymap.action.type.unknown",
                format!("未知动作类型 {other}（支持 tap、swipe、raw_key、hold）"),
                resource,
                binding_path,
                format!("{field_path}.type"),
            ));
            return None;
        }
    };
    check_unknown_fields(
        map,
        allowed,
        resource,
        binding_path,
        &field_path,
        "keymap.action.unknown_key",
        diagnostics,
    );

    match action_type {
        "tap" => parse_coord_field(map, "at", resource, binding_path, diagnostics)
            .map(|at| KeymapAction::Tap { at }),
        "hold" => parse_hold(map, resource, binding_path, diagnostics),
        "swipe" => {
            let from = parse_coord_field(map, "from", resource, binding_path, diagnostics);
            let to = parse_coord_field(map, "to", resource, binding_path, diagnostics);
            let duration_ms = parse_duration(map, resource, binding_path, diagnostics);
            from.zip(to)
                .zip(duration_ms)
                .map(|((from, to), duration_ms)| KeymapAction::Swipe {
                    from,
                    to,
                    duration_ms,
                })
        }
        "raw_key" => parse_raw_key(map, resource, binding_path, diagnostics),
        _ => None,
    }
}

fn parse_coord_field(
    map: &Mapping,
    field: &str,
    resource: &str,
    binding_path: &str,
    diagnostics: &mut Vec<KeymapDiagnostic>,
) -> Option<[f64; 2]> {
    let path = format!("{binding_path}.action.{field}");
    let Some(value) = value_for(map, field) else {
        diagnostics.push(diag(
            "keymap.coordinate.missing",
            format!("动作缺少必需字段 {field}"),
            resource,
            binding_path,
            &path,
        ));
        return None;
    };
    let Some(items) = value.as_sequence() else {
        diagnostics.push(diag(
            "keymap.coordinate.invalid",
            format!("{field} 必须是包含两个数字的列表"),
            resource,
            binding_path,
            &path,
        ));
        return None;
    };
    if items.len() != 2 {
        diagnostics.push(diag(
            "keymap.coordinate.invalid",
            format!("{field} 必须恰好包含 [x, y] 两个值"),
            resource,
            binding_path,
            &path,
        ));
        return None;
    }
    let Some(x) = items[0].as_f64() else {
        diagnostics.push(diag(
            "keymap.coordinate.invalid",
            format!("{field}[0] 必须是数字"),
            resource,
            binding_path,
            &path,
        ));
        return None;
    };
    let Some(y) = items[1].as_f64() else {
        diagnostics.push(diag(
            "keymap.coordinate.invalid",
            format!("{field}[1] 必须是数字"),
            resource,
            binding_path,
            &path,
        ));
        return None;
    };
    if !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        diagnostics.push(diag(
            "keymap.coordinate.out_of_range",
            format!("{field} 坐标必须在 0~1 范围内"),
            resource,
            binding_path,
            &path,
        ));
        return None;
    }
    Some([x, y])
}

fn parse_raw_key(
    map: &Mapping,
    resource: &str,
    binding_path: &str,
    diagnostics: &mut Vec<KeymapDiagnostic>,
) -> Option<KeymapAction> {
    let code_path = format!("{binding_path}.action.code");
    let code = match value_for(map, "code") {
        Some(Value::String(value)) if known_keyboard_code(value) => Some(value.clone()),
        Some(Value::String(value)) => {
            diagnostics.push(diag(
                "keymap.raw_key_code",
                format!("无法映射 KeyboardEvent.code：{value}"),
                resource,
                binding_path,
                &code_path,
            ));
            None
        }
        Some(_) => {
            diagnostics.push(diag(
                "keymap.raw_key_code",
                "code 必须是受支持的 KeyboardEvent.code 字符串",
                resource,
                binding_path,
                &code_path,
            ));
            None
        }
        None => None,
    };
    let keycode = parse_optional_keycode(map, resource, binding_path, diagnostics);
    if code.is_none() && keycode.is_none() {
        diagnostics.push(diag(
            "keymap.raw_key",
            "raw_key 必须提供有效的 code 或 keycode",
            resource,
            binding_path,
            format!("{binding_path}.action"),
        ));
        return None;
    }
    Some(KeymapAction::RawKey { code, keycode })
}

fn parse_optional_keycode(
    map: &Mapping,
    resource: &str,
    binding_path: &str,
    diagnostics: &mut Vec<KeymapDiagnostic>,
) -> Option<u32> {
    let path = format!("{binding_path}.action.keycode");
    let value = value_for(map, "keycode")?;
    let Some(keycode) = value.as_u64() else {
        diagnostics.push(diag(
            "keymap.raw_keycode",
            "keycode 必须是 1~1000 的整数",
            resource,
            binding_path,
            &path,
        ));
        return None;
    };
    if !(1..=MAX_ANDROID_KEYCODE as u64).contains(&keycode) {
        diagnostics.push(diag(
            "keymap.raw_keycode",
            "keycode 必须在 1~1000 范围内",
            resource,
            binding_path,
            &path,
        ));
        return None;
    }
    Some(keycode as u32)
}

fn parse_hold(
    map: &Mapping,
    resource: &str,
    binding_path: &str,
    diagnostics: &mut Vec<KeymapDiagnostic>,
) -> Option<KeymapAction> {
    parse_coord_field(map, "at", resource, binding_path, diagnostics)
        .map(|at| KeymapAction::Hold { at })
}

/// Return whether a value is a supported browser keyboard physical code.
///
/// `raw_key.code` deliberately continues to use this narrower predicate;
/// binding selectors additionally accept mouse buttons and gamepad controls.
fn known_keyboard_code(code: &str) -> bool {
    if let Some(letter) = code.strip_prefix("Key") {
        return letter.len() == 1 && letter.as_bytes()[0].is_ascii_uppercase();
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        return digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit();
    }
    matches!(
        code,
        "ArrowUp"
            | "ArrowDown"
            | "ArrowLeft"
            | "ArrowRight"
            | "Home"
            | "End"
            | "PageUp"
            | "PageDown"
            | "Insert"
            | "Delete"
            | "Space"
            | "Enter"
            | "NumpadEnter"
            | "Tab"
            | "Escape"
            | "Backspace"
            | "AltLeft"
            | "AltRight"
            | "ShiftLeft"
            | "ShiftRight"
            | "ControlLeft"
            | "ControlRight"
            | "MetaLeft"
            | "MetaRight"
            | "CapsLock"
            | "NumLock"
            | "ScrollLock"
            | "PrintScreen"
            | "Pause"
            | "ContextMenu"
            | "Backquote"
            | "Minus"
            | "Equal"
            | "BracketLeft"
            | "BracketRight"
            | "Backslash"
            | "IntlBackslash"
            | "Semicolon"
            | "Quote"
            | "Comma"
            | "Period"
            | "Slash"
            | "F1"
            | "F2"
            | "F3"
            | "F4"
            | "F5"
            | "F6"
            | "F7"
            | "F8"
            | "F9"
            | "F10"
            | "F11"
            | "F12"
            | "Numpad0"
            | "Numpad1"
            | "Numpad2"
            | "Numpad3"
            | "Numpad4"
            | "Numpad5"
            | "Numpad6"
            | "Numpad7"
            | "Numpad8"
            | "Numpad9"
            | "NumpadDivide"
            | "NumpadMultiply"
            | "NumpadSubtract"
            | "NumpadAdd"
            | "NumpadDecimal"
            | "NumpadComma"
            | "NumpadEqual"
            | "NumpadParenLeft"
            | "NumpadParenRight"
    )
}

/// Binding selectors are intentionally closed.  They use the same names as
/// the normalized InputEvent gateway, so a YAML keymap cannot accidentally
/// become a free-form event filter.
fn known_input_code(code: &str) -> bool {
    if known_keyboard_code(code) {
        return true;
    }
    if matches!(
        code,
        "MouseLeft" | "MouseMiddle" | "MouseRight" | "MouseBack" | "MouseForward" | "MouseMove"
    ) {
        return true;
    }
    let (prefix, max) = if let Some(rest) = code.strip_prefix("GamepadButton") {
        (rest, 31)
    } else if let Some(rest) = code.strip_prefix("GamepadAxis") {
        (rest, 7)
    } else {
        return false;
    };
    prefix.parse::<u8>().ok().is_some_and(|index| index <= max)
}

fn parse_duration(
    map: &Mapping,
    resource: &str,
    binding_path: &str,
    diagnostics: &mut Vec<KeymapDiagnostic>,
) -> Option<u32> {
    let path = format!("{binding_path}.action.duration_ms");
    let Some(value) = value_for(map, "duration_ms") else {
        diagnostics.push(diag(
            "keymap.duration.missing",
            "swipe 缺少必需字段 duration_ms",
            resource,
            binding_path,
            &path,
        ));
        return None;
    };
    let Some(duration) = value.as_u64() else {
        diagnostics.push(diag(
            "keymap.duration.invalid",
            "duration_ms 必须是正整数",
            resource,
            binding_path,
            &path,
        ));
        return None;
    };
    if duration == 0 || duration > MAX_SWIPE_DURATION_MS as u64 {
        diagnostics.push(diag(
            "keymap.duration.invalid",
            format!("duration_ms 必须在 1~{MAX_SWIPE_DURATION_MS} 毫秒范围内"),
            resource,
            binding_path,
            &path,
        ));
        return None;
    }
    Some(duration as u32)
}

fn check_unknown_fields(
    map: &Mapping,
    allowed: &[&str],
    resource: &str,
    step_path: &str,
    field_path: &str,
    code: &str,
    diagnostics: &mut Vec<KeymapDiagnostic>,
) {
    for key in map.keys() {
        let Some(name) = key.as_str() else {
            diagnostics.push(diag(
                code,
                "YAML 对象字段名必须是字符串",
                resource,
                step_path,
                field_path,
            ));
            continue;
        };
        if !allowed.contains(&name) {
            diagnostics.push(diag(
                code,
                format!("未知字段 {name}；允许字段: {}", allowed.join(", ")),
                resource,
                step_path,
                if field_path.is_empty() {
                    name.to_string()
                } else {
                    format!("{field_path}.{name}")
                },
            ));
        }
    }
}

fn value_for<'a>(map: &'a Mapping, key: &str) -> Option<&'a Value> {
    map.get(Value::String(key.to_string()))
}

fn valid_display_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 255 && !value.chars().any(char::is_control)
}

fn valid_key_code(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn diag(
    code: &str,
    message: impl Into<String>,
    resource: &str,
    step_path: impl Into<String>,
    field: impl Into<String>,
) -> KeymapDiagnostic {
    KeymapDiagnostic::new(code, message, resource, step_path, field)
}

fn normalize_keymap_name(name: &str) -> anyhow::Result<String> {
    let trimmed = name.trim();
    let upper_stem = trimmed
        .split('.')
        .next()
        .unwrap_or(trimmed)
        .to_ascii_uppercase();
    let reserved = matches!(
        upper_stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.starts_with('.')
        || trimmed.ends_with('.')
        || reserved
        || trimmed.chars().any(|ch| {
            ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        || !trimmed
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_' | '#' | ' '))
    {
        anyhow::bail!("映射方案文件名非法（允许 Unicode 字母数字、空格、. _ - #）: {name}");
    }
    let mut name = trimmed.to_string();
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".yaml") && !lower.ends_with(".yml") {
        name.push_str(".yaml");
    }
    Ok(name)
}

fn parse_keymap_id(id: &str) -> anyhow::Result<(String, String)> {
    let (pkg, name) = id
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("非法映射方案 id: {id}"))?;
    let pkg = safe_name(pkg).ok_or_else(|| anyhow::anyhow!("应用包名非法: {pkg}"))?;
    let name = normalize_keymap_name(name)?;
    Ok((pkg, name))
}

pub struct KeymapStore {
    root: PathBuf,
    /// Composite 资源解析缝（EditableLocal 本地编辑区 → user-overrides →
    /// active App Package，三层统一）。读取（get/list）覆盖全部三层；写入
    /// （create/update）只落地本地编辑区（分区目录）——本地副本在该层可见且
    /// 优先，删除本地副本后下层内容重新可见（分层语义）。
    composite: crate::app_packages::CompositeResolver,
}

impl KeymapStore {
    pub fn new(root: PathBuf) -> Self {
        let composite = crate::app_packages::CompositeResolver::new(root.clone());
        Self { root, composite }
    }

    pub fn keymap_dir(&self, pkg: &str) -> PathBuf {
        safe_name(pkg)
            .map(|pkg| self.root.join(pkg).join("keymaps"))
            .unwrap_or_else(|| self.root.join(".gamer-invalid-partition").join("keymaps"))
    }

    pub fn create(&self, pkg: &str, name: &str, keymap: &Keymap) -> anyhow::Result<KeymapFile> {
        let package = safe_name(pkg).ok_or_else(|| anyhow::anyhow!("应用包名非法: {pkg}"))?;
        let name = normalize_keymap_name(name)?;
        // 本地编辑区是最高优先层：同名 override/包内置方案存在时也允许创建，
        // 本地副本立即可见并遮蔽下层（删除本地副本后下层内容重新可见）。
        let path = self.keymap_dir(&package).join(&name);
        if path.exists() {
            anyhow::bail!("映射方案已存在: {package}/{name}");
        }
        let content = serialize_keymap(keymap)?;
        atomic_write(&path, content.as_bytes())?;
        self.load_file(&package, &name)
    }

    pub fn update(
        &self,
        id: &str,
        new_name: Option<&str>,
        keymap: &Keymap,
        expected_version: Option<&str>,
        force: bool,
    ) -> anyhow::Result<KeymapFile> {
        let (package, old_name) = parse_keymap_id(id)?;
        let old_path = self.keymap_dir(&package).join(&old_name);
        if !old_path.is_file() {
            // 本地无副本但 override/包层提供同名方案 → 首次物化：把新内容写为
            // 本地编辑区副本（用户改的就是本地副本，立即可见生效）。物化视为
            // 首次写入，不做 expected_version 版本比较（force 与否均一致）。
            if self.composite.keymap(&package, &old_name).is_none() {
                anyhow::bail!("映射方案不存在: {id}");
            }
            let target_name = match new_name {
                Some(name) => normalize_keymap_name(name)?,
                None => old_name.clone(),
            };
            let target_path = self.keymap_dir(&package).join(&target_name);
            if target_path.exists() {
                anyhow::bail!("映射方案已存在: {package}/{target_name}");
            }
            let content = serialize_keymap(keymap)?;
            atomic_write(&target_path, content.as_bytes())?;
            return self.load_file(&package, &target_name);
        }
        let old_content = std::fs::read_to_string(&old_path)?;
        if !force {
            let expected = expected_version.ok_or_else(|| {
                anyhow::anyhow!("更新映射方案必须提供 expected_version，或显式 force:true")
            })?;
            if expected != content_version(&old_content) {
                anyhow::bail!("映射方案版本冲突: {id}");
            }
        }
        let target_name = match new_name {
            Some(name) => normalize_keymap_name(name)?,
            None => old_name.clone(),
        };
        let target_path = self.keymap_dir(&package).join(&target_name);
        if target_path != old_path && target_path.exists() {
            anyhow::bail!("映射方案已存在: {package}/{target_name}");
        }
        let content = serialize_keymap(keymap)?;
        atomic_write(&target_path, content.as_bytes())?;
        if target_path != old_path {
            if let Err(error) = std::fs::remove_file(&old_path) {
                let _ = std::fs::remove_file(&target_path);
                return Err(error.into());
            }
        }
        self.load_file(&package, &target_name)
    }

    pub fn get(&self, id: &str) -> anyhow::Result<Option<KeymapFile>> {
        let (package, name) = match parse_keymap_id(id) {
            Ok(parts) => parts,
            Err(_) => return Ok(None),
        };
        // composite 三层顺序：本地编辑区（分区）→ user override → active App Package。
        match self.composite.keymap(&package, &name) {
            Some(hit) => self.load_file_at(&package, &name, &hit.path).map(Some),
            None => Ok(None),
        }
    }

    pub fn list(&self, pkg: &str) -> anyhow::Result<Vec<KeymapSummary>> {
        let package = safe_name(pkg).ok_or_else(|| anyhow::anyhow!("应用包名非法: {pkg}"))?;
        let mut out = Vec::new();
        let dir = self.keymap_dir(&package);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let file = entry.file_name().to_string_lossy().to_string();
                let lower = file.to_ascii_lowercase();
                if !path.is_file()
                    || (!lower.ends_with(".yaml") && !lower.ends_with(".yml"))
                    || normalize_keymap_name(&file).ok().as_deref() != Some(file.as_str())
                {
                    continue;
                }
                let content = match std::fs::read_to_string(&path) {
                    Ok(content) => content,
                    Err(_) => continue,
                };
                let version = content_version(&content);
                let (name, binding_count, valid, diagnostics) =
                    match parse_keymap_content(&content, &format!("{package}/{file}")) {
                        Ok(keymap) => (keymap.name, keymap.bindings.len(), true, Vec::new()),
                        Err(diagnostics) => (
                            file.strip_suffix(".yaml")
                                .or_else(|| file.strip_suffix(".yml"))
                                .unwrap_or(&file)
                                .to_string(),
                            0,
                            false,
                            diagnostics,
                        ),
                    };
                out.push(KeymapSummary {
                    id: format!("{package}/{file}"),
                    package: package.clone(),
                    pkg: package.clone(),
                    file: file.clone(),
                    name,
                    version,
                    binding_count,
                    updated_at: fmt_mtime(&path),
                    valid,
                    diagnostics,
                });
            }
        }
        // 追加 override/包内置方案：分区（本地编辑区）已列出的文件名不重复
        // 展示——composite.keymap_names 为三层并集，本地副本优先可编辑，
        // 下层方案的元数据以其文件为准。
        let listed: std::collections::HashSet<String> = out
            .iter()
            .map(|summary| summary.file.to_ascii_lowercase())
            .collect();
        for file in self.composite.keymap_names(&package) {
            if listed.contains(&file.to_ascii_lowercase()) {
                continue;
            }
            if normalize_keymap_name(&file).ok().as_deref() != Some(file.as_str()) {
                continue;
            }
            let path = self
                .composite
                .keymap(&package, &file)
                .map(|hit| hit.path)
                .unwrap_or_else(|| dir.join(&file));
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let version = content_version(&content);
            let (name, binding_count, valid, diagnostics) =
                match parse_keymap_content(&content, &format!("{package}/{file}")) {
                    Ok(keymap) => (keymap.name, keymap.bindings.len(), true, Vec::new()),
                    Err(diagnostics) => (
                        file.strip_suffix(".yaml")
                            .or_else(|| file.strip_suffix(".yml"))
                            .unwrap_or(&file)
                            .to_string(),
                        0,
                        false,
                        diagnostics,
                    ),
                };
            out.push(KeymapSummary {
                id: format!("{package}/{file}"),
                package: package.clone(),
                pkg: package.clone(),
                file,
                name,
                version,
                binding_count,
                updated_at: fmt_mtime(&path),
                valid,
                diagnostics,
            });
        }
        out.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        let (package, name) = parse_keymap_id(id)?;
        let path = self.keymap_dir(&package).join(&name);
        std::fs::remove_file(&path)
            .map_err(|error| anyhow::anyhow!("删除映射方案失败: {error}"))?;
        let _ = std::fs::remove_dir(self.keymap_dir(&package));
        let _ = std::fs::remove_dir(self.root.join(&package));
        Ok(())
    }

    fn load_file(&self, package: &str, name: &str) -> anyhow::Result<KeymapFile> {
        let path = self.keymap_dir(package).join(name);
        self.load_file_at(package, name, &path)
    }

    /// 从任意宿主路径装载（分区文件或 composite 命中的 override/包内文件）。
    fn load_file_at(&self, package: &str, name: &str, path: &Path) -> anyhow::Result<KeymapFile> {
        let content = std::fs::read_to_string(path)?;
        let keymap = parse_keymap_content(&content, &format!("{package}/{name}"))
            .map_err(|diagnostics| anyhow::anyhow!(format_diagnostics(&diagnostics)))?;
        Ok(KeymapFile {
            id: format!("{package}/{name}"),
            package: package.to_string(),
            pkg: package.to_string(),
            file: name.to_string(),
            name: keymap.name.clone(),
            content: content.clone(),
            version: content_version(&content),
            binding_count: keymap.bindings.len(),
            keymap,
            updated_at: fmt_mtime(path),
        })
    }
}

fn format_diagnostics(diagnostics: &[KeymapDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("；")
}

fn fmt_mtime(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(|time| {
            let datetime: chrono::DateTime<chrono::Local> = time.into();
            datetime.format("%Y-%m-%d %H:%M:%S").to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "version: 1\nname: 战斗方案\nbindings:\n  - key: Space\n    action:\n      type: tap\n      at: [0.72, 0.86]\n  - key: KeyE\n    action:\n      type: swipe\n      from: [0.4, 0.8]\n      to: [0.6, 0.8]\n      duration_ms: 300\n";

    /// composite 三层（本地编辑区 → override → 包）：包内置方案对 get/list
    /// 可见；同名 create 落地本地副本并立即遮蔽包内方案；update 直接改写本地
    /// 副本；本地无副本时 update 首次物化；delete 后下层内容重新可见。
    #[tokio::test]
    async fn package_keymaps_visible_and_local_editable_layer_wins() {
        use crate::app_packages::AppPackageStore;
        use zip::write::SimpleFileOptions;

        let dir = std::env::temp_dir().join(format!(
            "gamer-keymap-composite-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let store = KeymapStore::new(dir.clone());
        let packages = AppPackageStore::new(dir.clone());
        let manifest = br#"format_version = 2
id = "official.km"
version = "1.0.0"

[android]
packages = ["com.test.app"]
"#;
        let package_keymap = "version: 1\nname: 包内方案\nbindings:\n  - key: Space\n    action:\n      type: tap\n      at: [0.5, 0.5]\n";
        let mut archive = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut archive));
            let opts =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            zw.start_file("manifest.toml", opts).unwrap();
            std::io::Write::write_all(&mut zw, manifest).unwrap();
            zw.start_file("keymaps/default.yaml", opts).unwrap();
            std::io::Write::write_all(&mut zw, package_keymap.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        packages.install_and_activate(&archive, None).await.unwrap();

        // 包内置方案经 get 可读
        let loaded = store.get("com.test.app/default.yaml").unwrap().unwrap();
        assert_eq!(loaded.file, "default.yaml");
        assert_eq!(loaded.keymap.name, "包内方案");

        // list 合并展示包内置方案
        let summaries = store.list("com.test.app").unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].file, "default.yaml");

        // 同名 create 成功：本地副本落地并立即可见（遮蔽包内方案）
        let local_keymap =
            parse_keymap_content("version: 1\nname: 本地方案\nbindings: []\n", "local").unwrap();
        let created = store
            .create("com.test.app", "default.yaml", &local_keymap)
            .unwrap();
        assert!(dir.join("com.test.app/keymaps/default.yaml").is_file());
        let local_version = created.version.clone();
        let loaded = store.get("com.test.app/default.yaml").unwrap().unwrap();
        assert_eq!(loaded.keymap.name, "本地方案", "本地副本必须优先于包内方案");
        let summaries = store.list("com.test.app").unwrap();
        assert_eq!(summaries.len(), 1, "同名本地副本不得在 list 中重复出现");
        assert_eq!(summaries[0].name, "本地方案");

        // update 改写本地副本：expected_version 版本守卫照常生效
        let updated = store
            .update(
                "com.test.app/default.yaml",
                None,
                &local_keymap,
                Some(&local_version),
                false,
            )
            .unwrap();
        assert_eq!(updated.keymap.name, "本地方案");
        assert!(store
            .update(
                "com.test.app/default.yaml",
                None,
                &local_keymap,
                Some("stale"),
                false
            )
            .is_err());

        // 删除本地副本 → 包内方案重新可见
        store.delete("com.test.app/default.yaml").unwrap();
        let loaded = store.get("com.test.app/default.yaml").unwrap().unwrap();
        assert_eq!(loaded.keymap.name, "包内方案");

        // 本地无副本时 update = 首次物化：不做版本比较，新内容落地本地副本
        let materialized = store
            .update(
                "com.test.app/default.yaml",
                None,
                &local_keymap,
                None,
                false,
            )
            .unwrap();
        assert_eq!(materialized.keymap.name, "本地方案");
        assert!(dir.join("com.test.app/keymaps/default.yaml").is_file());
        let loaded = store.get("com.test.app/default.yaml").unwrap().unwrap();
        assert_eq!(loaded.keymap.name, "本地方案");

        // 本地不存在的方案 update 仍然报不存在
        assert!(store
            .update("com.test.app/missing.yaml", None, &local_keymap, None, true)
            .is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn strict_loader_accepts_all_first_release_actions() {
        let keymap = parse_keymap_content(VALID, "com.test.app/combat.yaml").unwrap();
        assert_eq!(keymap.version, 1);
        assert_eq!(keymap.bindings.len(), 2);
        assert!(matches!(
            keymap.bindings[0].action,
            KeymapAction::Tap { .. }
        ));

        let content = "version: 1\nname: Reserved\nbindings:\n  - key: KeyW\n    action:\n      type: hold\n      at: [0, 1]\n  - key: F1\n    action:\n      type: raw_key\n      keycode: 131\n";
        let keymap = parse_keymap_content(content, "reserved.yaml").unwrap();
        assert!(matches!(
            keymap.bindings[0].action,
            KeymapAction::Hold { at: [0.0, 1.0] }
        ));
        assert!(matches!(
            keymap.bindings[1].action,
            KeymapAction::RawKey {
                keycode: Some(131),
                ..
            }
        ));
    }

    #[test]
    fn strict_loader_accepts_closed_mouse_and_gamepad_binding_selectors() {
        let content = "version: 1\nname: Inputs\nbindings:\n  - key: MouseLeft\n    action:\n      type: hold\n      at: [0.1, 0.2]\n  - key: GamepadButton0\n    action:\n      type: tap\n      at: [0.3, 0.4]\n  - key: GamepadAxis7\n    action:\n      type: hold\n      at: [0.5, 0.6]\n";
        let keymap = parse_keymap_content(content, "inputs.yaml").unwrap();
        assert_eq!(
            keymap
                .bindings
                .iter()
                .map(|binding| binding.key.as_str())
                .collect::<Vec<_>>(),
            vec!["MouseLeft", "GamepadButton0", "GamepadAxis7"]
        );
        for key in ["MouseSide", "GamepadButton32", "GamepadAxis8"] {
            let invalid = format!(
                "version: 1\nname: bad\nbindings:\n  - key: {key}\n    action:\n      type: tap\n      at: [0, 0]\n"
            );
            assert!(
                parse_keymap_content(&invalid, "inputs.yaml").is_err(),
                "{key}"
            );
        }
    }

    #[test]
    fn hold_requires_single_point_and_does_not_serialize_runtime_fields() {
        let content = "version: 1\nname: Hold\nbindings:\n  - key: KeyW\n    action:\n      type: hold\n      at: [0.25, 0.75]\n";
        let keymap = parse_keymap_content(content, "hold.yaml").unwrap();
        assert_eq!(
            keymap.bindings[0].action,
            KeymapAction::Hold { at: [0.25, 0.75] }
        );

        let serialized = serialize_keymap(&keymap).unwrap();
        assert!(serialized.contains("type: hold"));
        assert!(serialized.contains("at:"));
        assert!(serialized.contains("0.25"));
        assert!(serialized.contains("0.75"));
        assert!(!serialized.contains("from:"));
        assert!(!serialized.contains("to:"));
        assert!(!serialized.contains("pointer_id:"));
    }

    #[test]
    fn hold_rejects_drag_coordinates_and_pointer_id() {
        for field in ["from", "to", "pointer_id"] {
            let content = format!(
                "version: 1\nname: Hold\nbindings:\n  - key: KeyW\n    action:\n      type: hold\n      at: [0.25, 0.75]\n      {field}: {}\n",
                if field == "pointer_id" {
                    "1".to_string()
                } else {
                    "[0.1, 0.2]".to_string()
                }
            );
            let diagnostics = parse_keymap_content(&content, "hold-invalid.yaml").unwrap_err();
            assert!(diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "keymap.action.unknown_key"
                    && diagnostic.field == format!("bindings[0].action.{field}")
            }));
        }
    }

    #[test]
    fn strict_loader_reports_unknown_fields_coordinates_duration_and_duplicates() {
        let content = "version: 1\nname: bad\nextra: true\nbindings:\n  - key: Space\n    action:\n      type: tap\n      at: [1.1, 0.2]\n      extra: true\n  - key: Space\n    action:\n      type: swipe\n      from: [0, 0]\n      to: [1, 1]\n      duration_ms: 0\n";
        let diagnostics = parse_keymap_content(content, "bad.yaml").unwrap_err();
        assert!(diagnostics
            .iter()
            .any(|d| d.code == "keymap.top_level.unknown_key"));
        assert!(diagnostics
            .iter()
            .any(|d| d.code == "keymap.action.unknown_key"));
        assert!(diagnostics
            .iter()
            .any(|d| d.code == "keymap.coordinate.out_of_range"));
        assert!(diagnostics
            .iter()
            .any(|d| d.code == "keymap.duration.invalid"));
        assert!(diagnostics
            .iter()
            .any(|d| d.code == "keymap.binding.duplicate_key"));
    }

    #[test]
    fn canonical_serialization_roundtrips() {
        let parsed = parse_keymap_content(VALID, "test.yaml").unwrap();
        let serialized = serialize_keymap(&parsed).unwrap();
        let reparsed = parse_keymap_content(&serialized, "test.yaml").unwrap();
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn storage_is_partitioned_and_version_guarded() {
        let dir = tempfile::tempdir().unwrap();
        let store = KeymapStore::new(dir.path().to_path_buf());
        let keymap = parse_keymap_content(VALID, "test.yaml").unwrap();
        let created = store.create("com.test.app", "combat", &keymap).unwrap();
        assert!(dir
            .path()
            .join("com.test.app/keymaps/combat.yaml")
            .is_file());
        assert!(store.list("com.other.app").unwrap().is_empty());
        assert!(store
            .update(
                "com.test.app/combat.yaml",
                None,
                &keymap,
                Some("stale"),
                false
            )
            .is_err());
        let updated = store
            .update(&created.id, None, &keymap, Some(&created.version), false)
            .unwrap();
        assert_eq!(updated.id, created.id);
    }
}
