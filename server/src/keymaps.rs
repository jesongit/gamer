//! Keymap YAML storage, validation, canonical serialization, and partition snapshots.
//!
//! Keymaps deliberately use a schema separate from the script-v2 loader.  They are
//! stored below `data/<package>/keymap/` and never fall back to another package.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_yaml::{Mapping, Value};

use crate::scripts::{atomic_write, content_version, sanitize_part};

pub const MAX_KEYMAP_YAML_BYTES: usize = 1024 * 1024;
pub const MAX_KEYMAP_ARCHIVE_BYTES: usize = crate::scripts::IMPORT_MAX_ARCHIVE_BYTES;
pub const MAX_KEYMAP_TOTAL_BYTES: usize = crate::scripts::IMPORT_MAX_TOTAL_BYTES;
pub const MAX_KEYMAP_ARCHIVE_ENTRIES: usize = crate::scripts::IMPORT_MAX_ENTRIES;
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

#[derive(Debug, Clone, Serialize, Default)]
pub struct KeymapImportReport {
    pub add: Vec<String>,
    pub overwrite: Vec<String>,
    pub invalid: Vec<KeymapImportInvalid>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeymapImportInvalid {
    pub path: String,
    pub diagnostics: Vec<KeymapDiagnostic>,
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
            Some(Value::String(value)) if valid_key_code(value) && known_keyboard_code(value) => {
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
    let pkg = sanitize_part(pkg).ok_or_else(|| anyhow::anyhow!("应用包名非法: {pkg}"))?;
    let name = normalize_keymap_name(name)?;
    Ok((pkg, name))
}

pub struct KeymapStore {
    root: PathBuf,
}

impl KeymapStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn keymap_dir(&self, pkg: &str) -> PathBuf {
        sanitize_part(pkg)
            .map(|pkg| self.root.join(pkg).join("keymap"))
            .unwrap_or_else(|| self.root.join(".gamer-invalid-partition").join("keymap"))
    }

    pub fn create(&self, pkg: &str, name: &str, keymap: &Keymap) -> anyhow::Result<KeymapFile> {
        let package = sanitize_part(pkg).ok_or_else(|| anyhow::anyhow!("应用包名非法: {pkg}"))?;
        let name = normalize_keymap_name(name)?;
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
            anyhow::bail!("映射方案不存在: {id}");
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
        let path = self.keymap_dir(&package).join(&name);
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(self.load_file(&package, &name)?))
    }

    pub fn list(&self, pkg: &str) -> anyhow::Result<Vec<KeymapSummary>> {
        let package = sanitize_part(pkg).ok_or_else(|| anyhow::anyhow!("应用包名非法: {pkg}"))?;
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
                    file,
                    name,
                    version,
                    binding_count,
                    updated_at: fmt_mtime(&path),
                    valid,
                    diagnostics,
                });
            }
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

    pub fn export_partition(&self, pkg: &str) -> anyhow::Result<(String, Vec<u8>)> {
        let package = sanitize_part(pkg).ok_or_else(|| anyhow::anyhow!("应用包名非法: {pkg}"))?;
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(self.keymap_dir(&package)) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                let lower = name.to_ascii_lowercase();
                if path.is_file()
                    && normalize_keymap_name(&name).ok().as_deref() == Some(name.as_str())
                    && (lower.ends_with(".yaml") || lower.ends_with(".yml"))
                {
                    files.push(name);
                }
            }
        }
        files.sort();
        let mut bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.add_directory("keymap", options)?;
            for name in files {
                writer.start_file(format!("keymap/{name}"), options)?;
                writer.write_all(&std::fs::read(self.keymap_dir(&package).join(name))?)?;
            }
            writer.finish()?;
        }
        Ok((format!("{package}-keymaps.zip"), bytes))
    }

    pub fn import_partition(
        &self,
        bytes: &[u8],
        pkg: &str,
        confirm: bool,
    ) -> anyhow::Result<KeymapImportReport> {
        let package = sanitize_part(pkg).ok_or_else(|| anyhow::anyhow!("应用包名非法: {pkg}"))?;
        if bytes.len() > MAX_KEYMAP_ARCHIVE_BYTES {
            anyhow::bail!(
                "压缩包超过 {} MiB",
                MAX_KEYMAP_ARCHIVE_BYTES / (1024 * 1024)
            );
        }
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        if archive.len() > MAX_KEYMAP_ARCHIVE_ENTRIES {
            anyhow::bail!("包内条目数超过上限 {MAX_KEYMAP_ARCHIVE_ENTRIES}");
        }
        let mut declared_total = 0u64;
        for index in 0..archive.len() {
            declared_total = declared_total.saturating_add(archive.by_index(index)?.size());
            if declared_total > MAX_KEYMAP_TOTAL_BYTES as u64 {
                anyhow::bail!("声明解压总量超过上限");
            }
        }

        let mut report = KeymapImportReport::default();
        let mut seen = HashSet::new();
        let mut files: Vec<(String, Vec<u8>, String)> = Vec::new();
        let mut actual_total = 0usize;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            if entry.is_dir() {
                continue;
            }
            let zip_path = entry.name().to_string();
            if zip_path.contains('\\') {
                anyhow::bail!("包内路径非法: {zip_path}");
            }
            let Some(rest) = zip_path.strip_prefix("keymap/") else {
                anyhow::bail!("包内路径需为 keymap/<映射方案>: {zip_path}");
            };
            if rest.is_empty() || rest.contains('/') {
                anyhow::bail!("包内路径需为 keymap/<映射方案>: {zip_path}");
            }
            let name = match normalize_keymap_name(rest) {
                Ok(name) if name == rest => name,
                Ok(_) => {
                    report.invalid.push(KeymapImportInvalid {
                        path: zip_path.clone(),
                        diagnostics: vec![diag(
                            "keymap.filename.invalid",
                            "导入文件必须显式使用 .yaml 或 .yml 扩展名",
                            &zip_path,
                            "",
                            "filename",
                        )],
                    });
                    continue;
                }
                Err(error) => {
                    report.invalid.push(KeymapImportInvalid {
                        path: zip_path.clone(),
                        diagnostics: vec![diag(
                            "keymap.filename.invalid",
                            error.to_string(),
                            &zip_path,
                            "",
                            "filename",
                        )],
                    });
                    continue;
                }
            };
            if !seen.insert(name.to_ascii_lowercase()) {
                anyhow::bail!("包内存在重复文件: {zip_path}");
            }
            if entry.size() > MAX_KEYMAP_YAML_BYTES as u64 {
                anyhow::bail!("{zip_path} 解压后超过 1 MiB");
            }
            let mut content = Vec::new();
            (&mut entry)
                .take(MAX_KEYMAP_YAML_BYTES as u64 + 1)
                .read_to_end(&mut content)?;
            if content.len() > MAX_KEYMAP_YAML_BYTES {
                anyhow::bail!("{zip_path} 解压后超过 1 MiB");
            }
            actual_total = actual_total.saturating_add(content.len());
            if actual_total > MAX_KEYMAP_TOTAL_BYTES {
                anyhow::bail!("总解压量超过上限");
            }
            let text = match std::str::from_utf8(&content) {
                Ok(text) => text,
                Err(error) => {
                    report.invalid.push(KeymapImportInvalid {
                        path: zip_path.clone(),
                        diagnostics: vec![diag(
                            "keymap.yaml.utf8",
                            format!("内容不是合法 UTF-8 文本: {error}"),
                            &zip_path,
                            "",
                            "yaml",
                        )],
                    });
                    continue;
                }
            };
            match parse_keymap_content(text, &zip_path) {
                Ok(keymap) => {
                    let canonical = serialize_keymap(&keymap)?;
                    files.push((name, canonical.into_bytes(), zip_path));
                }
                Err(diagnostics) => report.invalid.push(KeymapImportInvalid {
                    path: zip_path,
                    diagnostics,
                }),
            }
        }
        if files.is_empty() && report.invalid.is_empty() {
            anyhow::bail!("包内没有可导入的 keymap YAML 文件");
        }
        for (name, _, _) in &files {
            let destination = self.keymap_dir(&package).join(name);
            if destination.exists() {
                report.overwrite.push(format!("keymap/{name}"));
            } else {
                report.add.push(format!("keymap/{name}"));
            }
        }
        if !confirm {
            return Ok(report);
        }
        if !report.invalid.is_empty() {
            anyhow::bail!("导入被拒绝：{} 个条目未通过严格校验", report.invalid.len());
        }

        let staging = self.root.join(format!(
            ".gamer-keymap-staging-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let backup_dir = staging.join("backup");
        std::fs::create_dir_all(&backup_dir)?;
        let mut committed: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
        let result = (|| -> anyhow::Result<()> {
            for (name, content, _) in files {
                let destination = self.keymap_dir(&package).join(name);
                let backup = if destination.exists() {
                    let backup = backup_dir.join(destination.file_name().unwrap());
                    std::fs::rename(&destination, &backup)?;
                    Some(backup)
                } else {
                    None
                };
                if let Err(error) = atomic_write(&destination, &content) {
                    restore_file(&destination, backup.as_deref());
                    rollback_files(&committed);
                    return Err(error);
                }
                committed.push((destination, backup));
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
        if let Err(error) = std::fs::remove_dir_all(&staging) {
            tracing::warn!(error = %error, "keymap 导入 staging 清理失败，将在下次启动时清理");
        }
        Ok(report)
    }

    fn load_file(&self, package: &str, name: &str) -> anyhow::Result<KeymapFile> {
        let path = self.keymap_dir(package).join(name);
        let content = std::fs::read_to_string(&path)?;
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
            updated_at: fmt_mtime(&path),
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

fn restore_file(destination: &Path, backup: Option<&Path>) {
    if let Some(backup) = backup {
        let _ = std::fs::remove_file(destination);
        let _ = std::fs::rename(backup, destination);
    } else {
        let _ = std::fs::remove_file(destination);
    }
}

fn rollback_files(committed: &[(PathBuf, Option<PathBuf>)]) {
    for (destination, backup) in committed.iter().rev() {
        restore_file(destination, backup.as_deref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "version: 1\nname: 战斗方案\nbindings:\n  - key: Space\n    action:\n      type: tap\n      at: [0.72, 0.86]\n  - key: KeyE\n    action:\n      type: swipe\n      from: [0.4, 0.8]\n      to: [0.6, 0.8]\n      duration_ms: 300\n";

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
        assert!(dir.path().join("com.test.app/keymap/combat.yaml").is_file());
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
