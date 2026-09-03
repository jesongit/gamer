//! Keymap WASM fixture guest (gamer.keymap plugin.wasm).
//!
//! Profile 通道（gamer-keymap@1）：`start(profile)` 携带当前分区的 keymap
//! YAML 原文（与 `data/<pkg>/keymap/*.yaml` 完全相同的 schema，由 host 侧
//! `KeymapStore` 写入时校验）：
//!
//! ```yaml
//! version: 1
//! name: 可选显示名
//! bindings:
//!   - key: KeyW            # 按键 selector（KeyboardEvent.code）
//!     action:
//!       type: tap          # tap | swipe | raw_key | hold
//!       at: [0.40, 0.60]   # 归一化坐标 [0..1]
//! ```
//!
//! 规则语义与 Native KeymapRunner 对齐：
//! - `tap`：按下时在 `at` 归一化点执行一次点击；
//! - `swipe`：按下时执行 `from → to`、`duration_ms` 的滑动；
//! - `hold`：按下时在 `at` 处 touch-begin（guest 自有 slot），抬起时 touch-end；
//! - `raw_key`：按下发 Android keycode down，抬起发 up（`code` 名字或
//!   `keycode` 数字均可）。
//!
//! 内置默认规则即原 WASD fixture（KeyW/KeyA 按住、Space 点击、KeyE 滑动、
//! 鼠标左键拖动、手柄按键/摇杆），profile 中同 key 的绑定覆盖内置规则；
//! 未映射的输入全部 pass-through（consume=false）。

wit_bindgen::generate!({
    path: "../../wit/keymap",
    world: "keymap-host",
});

use exports::gamer::keymap::keymap::{
    DeviceAction, EventKind, Guest, InputEvent, InputResult, KeyAction,
};
use std::sync::{Mutex, OnceLock};

/// 内置默认规则的按键集合（原 WASD fixture 行为）。
const BUILTIN_KEYS: &[&str] = &["KeyW", "KeyA", "Space", "KeyE"];

#[derive(Default)]
struct GuestState {
    mouse_left: bool,
    gamepad_axis: bool,
    /// profile 解析结果：key selector → 规则。None = 未提供 profile。
    rules: Option<std::collections::HashMap<String, Rule>>,
    /// hold 规则当前按下的 slot。
    held_slots: std::collections::HashSet<u64>,
    /// raw_key 规则当前按下的 selector。
    active_raw: std::collections::HashSet<String>,
}

fn state() -> &'static Mutex<GuestState> {
    static STATE: OnceLock<Mutex<GuestState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(GuestState::default()))
}

/// 一条 profile 规则：按键 → 设备动作（InputResult/DeviceAction 形态的最小映射）。
#[derive(Debug, Clone)]
enum Rule {
    Tap([f64; 2]),
    Swipe {
        from: [f64; 2],
        to: [f64; 2],
        duration_ms: u64,
    },
    RawKey(u32),
    /// guest 自有 slot 由规则序号决定，保证按下/抬起配对稳定。
    Hold { slot: u64, at: [f64; 2] },
}

#[derive(serde::Deserialize)]
struct ProfileDocument {
    #[serde(default = "default_version")]
    #[allow(dead_code)]
    version: u32,
    #[serde(default)]
    #[allow(dead_code)]
    name: String,
    #[serde(default)]
    bindings: Vec<ProfileBinding>,
}

fn default_version() -> u32 {
    1
}

#[derive(serde::Deserialize)]
struct ProfileBinding {
    key: String,
    action: ProfileAction,
}

#[derive(serde::Deserialize)]
struct ProfileAction {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    at: Option<[f64; 2]>,
    #[serde(default)]
    from: Option<[f64; 2]>,
    #[serde(default)]
    to: Option<[f64; 2]>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    keycode: Option<u32>,
}

fn parse_coordinate(value: Option<[f64; 2]>, field: &str) -> Result<[f64; 2], String> {
    let point = value.ok_or_else(|| format!("keymap profile 缺少坐标 {field}"))?;
    if point.len() != 2 || !(0.0..=1.0).contains(&point[0]) || !(0.0..=1.0).contains(&point[1]) {
        return Err(format!("keymap profile 坐标 {field} 超出 0..1: {point:?}"));
    }
    Ok(point)
}

/// 解析 profile YAML 并构建 selector → Rule 表。失败即 start 失败，
/// 由 host 以 keymap start 错误呈现，不静默降级为 pass-through。
fn parse_profile(profile: &str) -> Result<std::collections::HashMap<String, Rule>, String> {
    let document: ProfileDocument = serde_yaml::from_str(profile)
        .map_err(|error| format!("keymap profile YAML 无效: {error}"))?;
    if document.version != 1 {
        return Err(format!(
            "keymap profile version 必须是 1，得到 {}",
            document.version
        ));
    }
    let mut rules = std::collections::HashMap::new();
    for (index, binding) in document.bindings.iter().enumerate() {
        if binding.key.trim().is_empty() {
            return Err("keymap profile binding.key 不能为空".to_string());
        }
        let rule = match binding.action.kind.as_str() {
            "tap" => Rule::Tap(parse_coordinate(binding.action.at, "at")?),
            "swipe" => Rule::Swipe {
                from: parse_coordinate(binding.action.from, "from")?,
                to: parse_coordinate(binding.action.to, "to")?,
                duration_ms: binding.action.duration_ms.unwrap_or(30).clamp(1, 60_000),
            },
            "raw_key" => {
                let keycode = binding
                    .action
                    .keycode
                    .map(Ok)
                    .unwrap_or_else(|| {
                        let code = binding
                            .action
                            .code
                            .as_deref()
                            .ok_or("keymap profile raw_key 需要 code 或 keycode")?;
                        android_keycode(code)
                            .ok_or_else(|| format!("keymap profile 未知 Android key: {code}"))
                    })?;
                if !(1..=1000).contains(&keycode) {
                    return Err(format!("keymap profile keycode 超出范围: {keycode}"));
                }
                Rule::RawKey(keycode)
            }
            "hold" => Rule::Hold {
                // 与内置 slot（1-4）隔离的 guest 自有 slot 段。
                slot: 100 + index as u64,
                at: parse_coordinate(binding.action.at, "at")?,
            },
            other => {
                return Err(format!(
                    "keymap profile 不支持的动作类型: {other}（tap|swipe|raw_key|hold）"
                ))
            }
        };
        rules.insert(binding.key.trim().to_string(), rule);
    }
    Ok(rules)
}

fn normalized(point: [f64; 2]) -> exports::gamer::keymap::keymap::Point {
    exports::gamer::keymap::keymap::Point {
        x: point[0].clamp(0.0, 1.0) as f32,
        y: point[1].clamp(0.0, 1.0) as f32,
    }
}

/// raw_key `code` 名 → Android keycode（与 host 侧 android_keycode 同表的精简版）。
fn android_keycode(code: &str) -> Option<u32> {
    if let Some(letter) = code.strip_prefix("Key") {
        let byte = letter.as_bytes().first().copied()?;
        if letter.len() == 1 && byte.is_ascii_uppercase() {
            return Some(29 + u32::from(byte - b'A'));
        }
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        let byte = digit.as_bytes().first().copied()?;
        if digit.len() == 1 && byte.is_ascii_digit() {
            return Some(7 + u32::from(byte - b'0'));
        }
    }
    Some(match code {
        "ArrowUp" => 19,
        "ArrowDown" => 20,
        "ArrowLeft" => 21,
        "ArrowRight" => 22,
        "Home" => 3,
        "Back" => 4,
        "Space" => 62,
        "Enter" => 66,
        "NumpadEnter" => 160,
        "Tab" => 61,
        "Escape" => 111,
        "Backspace" => 67,
        "Delete" => 112,
        _ => return None,
    })
}

/// profile 规则的按下/抬起动作。返回 None 表示该 selector 没有 profile 规则。
fn profile_action(
    guest_state: &mut GuestState,
    selector: &str,
    press: bool,
) -> Option<Vec<DeviceAction>> {
    let rules = guest_state.rules.as_ref()?;
    let rule = rules.get(selector)?;
    if press {
        match rule {
            Rule::Tap(at) => Some(vec![DeviceAction::Tap(normalized(*at))]),
            Rule::Swipe { from, to, duration_ms } => {
                Some(vec![DeviceAction::Swipe(
                    exports::gamer::keymap::keymap::Swipe {
                        from_point: normalized(*from),
                        to: normalized(*to),
                        duration_ms: *duration_ms,
                    },
                )])
            }
            Rule::RawKey(keycode) => {
                if guest_state.active_raw.contains(selector) {
                    Some(Vec::new())
                } else {
                    guest_state.active_raw.insert(selector.to_string());
                    Some(vec![DeviceAction::Key(KeyAction {
                        code: *keycode,
                        action: "down".to_string(),
                    })])
                }
            }
            Rule::Hold { slot, at } => {
                if guest_state.held_slots.contains(slot) {
                    Some(Vec::new())
                } else {
                    guest_state.held_slots.insert(*slot);
                    Some(vec![DeviceAction::TouchBegin(
                        exports::gamer::keymap::keymap::TouchBegin {
                            slot: *slot,
                            point: normalized(*at),
                        },
                    )])
                }
            }
        }
    } else {
        match rule {
            Rule::Hold { slot, .. } => {
                if guest_state.held_slots.remove(slot) {
                    Some(vec![DeviceAction::TouchEnd(*slot)])
                } else {
                    Some(Vec::new())
                }
            }
            Rule::RawKey(keycode) => {
                if guest_state.active_raw.remove(selector) {
                    Some(vec![DeviceAction::Key(KeyAction {
                        code: *keycode,
                        action: "up".to_string(),
                    })])
                } else {
                    Some(Vec::new())
                }
            }
            Rule::Tap(_) | Rule::Swipe { .. } => Some(Vec::new()),
        }
    }
}

/// 内置默认规则（原 WASD fixture）：仅当 selector 没有 profile 覆盖时使用。
fn builtin_actions(event: &InputEvent) -> Vec<DeviceAction> {
    use exports::gamer::keymap::keymap::{Point, Swipe, TouchBegin};
    let code = match &event.kind {
        EventKind::KeyDown | EventKind::KeyUp => event.code.as_deref().unwrap_or(""),
        _ => "",
    };
    if !BUILTIN_KEYS.contains(&code) {
        return Vec::new();
    }
    match event.kind {
        EventKind::KeyDown => match code {
            "KeyW" => vec![DeviceAction::TouchBegin(TouchBegin {
                slot: 1,
                point: Point { x: 0.40, y: 0.60 },
            })],
            "KeyA" => vec![DeviceAction::TouchBegin(TouchBegin {
                slot: 2,
                point: Point { x: 0.20, y: 0.60 },
            })],
            "Space" => vec![DeviceAction::Tap(Point { x: 0.70, y: 0.80 })],
            "KeyE" => vec![DeviceAction::Swipe(Swipe {
                from_point: Point { x: 0.30, y: 0.80 },
                to: Point { x: 0.70, y: 0.80 },
                duration_ms: 25,
            })],
            _ => Vec::new(),
        },
        EventKind::KeyUp => match code {
            "KeyW" => vec![DeviceAction::TouchEnd(1)],
            "KeyA" => vec![DeviceAction::TouchEnd(2)],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

struct Fixture;

impl Guest for Fixture {
    fn start(profile: Option<String>) -> Result<(), String> {
        let mut guest_state = state()
            .lock()
            .map_err(|_| "guest state poisoned".to_string())?;
        *guest_state = GuestState {
            mouse_left: false,
            gamepad_axis: false,
            rules: match profile.as_deref() {
                Some(content) if content.trim().is_empty() => None,
                Some(content) => Some(parse_profile(content)?),
                None => None,
            },
            held_slots: std::collections::HashSet::new(),
            active_raw: std::collections::HashSet::new(),
        };
        Ok(())
    }

    fn handle(event: InputEvent) -> Result<InputResult, String> {
        let mut guest_state = state()
            .lock()
            .map_err(|_| "guest state poisoned".to_string())?;
        let actions = match event.kind {
            EventKind::KeyDown | EventKind::KeyUp => {
                let selector = event.code.as_deref().unwrap_or_default();
                let press = event.kind == EventKind::KeyDown;
                let repeat = press && event.repeat;
                match profile_action(&mut guest_state, selector, press) {
                    // profile 覆盖：绑定过的按键一律消费（重复按下仅消费）。
                    Some(actions) if !repeat => actions,
                    Some(_) => Vec::new(),
                    None if repeat => Vec::new(),
                    // 内置默认规则 + 未映射按键 pass-through。
                    None => builtin_actions(&event),
                }
            }
            // 鼠标拖动 / 手柄保持内置默认行为。
            EventKind::MouseDown => {
                if event.button == 0 {
                    guest_state.mouse_left = true;
                    vec![DeviceAction::TouchBegin(
                        exports::gamer::keymap::keymap::TouchBegin {
                            slot: 3,
                            point: exports::gamer::keymap::keymap::Point {
                                x: 0.25,
                                y: 0.50,
                            },
                        },
                    )]
                } else {
                    Vec::new()
                }
            }
            EventKind::MouseMove if guest_state.mouse_left => {
                vec![DeviceAction::TouchMove(
                    exports::gamer::keymap::keymap::TouchMove {
                        slot: 3,
                        point: exports::gamer::keymap::keymap::Point { x: 0.50, y: 0.50 },
                    },
                )]
            }
            EventKind::MouseUp if event.button == 0 && guest_state.mouse_left => {
                guest_state.mouse_left = false;
                vec![DeviceAction::TouchEnd(3)]
            }
            EventKind::GamepadButton if event.index == 0 => {
                vec![DeviceAction::Key(KeyAction {
                    code: 62,
                    action: if event.pressed { "down" } else { "up" }.to_string(),
                })]
            }
            EventKind::GamepadAxis if event.index == 0 => {
                if event.value.abs() < 0.1 {
                    if guest_state.gamepad_axis {
                        guest_state.gamepad_axis = false;
                        vec![DeviceAction::TouchEnd(4)]
                    } else {
                        Vec::new()
                    }
                } else {
                    let x = ((event.value.clamp(-1.0, 1.0) + 1.0) / 2.0) as f32;
                    if guest_state.gamepad_axis {
                        vec![DeviceAction::TouchMove(
                            exports::gamer::keymap::keymap::TouchMove {
                                slot: 4,
                                point: exports::gamer::keymap::keymap::Point { x, y: 0.30 },
                            },
                        )]
                    } else {
                        guest_state.gamepad_axis = true;
                        vec![DeviceAction::TouchBegin(
                            exports::gamer::keymap::keymap::TouchBegin {
                                slot: 4,
                                point: exports::gamer::keymap::keymap::Point { x, y: 0.30 },
                            },
                        )]
                    }
                }
            }
            _ => Vec::new(),
        };
        Ok(InputResult {
            consume: !actions.is_empty(),
            actions,
        })
    }
}

export!(Fixture);
