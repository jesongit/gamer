wit_bindgen::generate!({
    path: "../../wit/keymap",
    world: "keymap-host",
});

use exports::gamer::keymap::keymap::{
    DeviceAction, EventKind, Guest, InputEvent, InputResult, KeyAction,
};
use std::sync::{Mutex, OnceLock};

#[derive(Default)]
struct GuestState {
    mouse_left: bool,
    gamepad_axis: bool,
}

fn state() -> &'static Mutex<GuestState> {
    static STATE: OnceLock<Mutex<GuestState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(GuestState::default()))
}

struct Fixture;

impl Guest for Fixture {
    fn start() -> Result<(), String> {
        *state()
            .lock()
            .map_err(|_| "guest state poisoned".to_string())? = GuestState::default();
        Ok(())
    }

    fn handle(event: InputEvent) -> Result<InputResult, String> {
        use exports::gamer::keymap::keymap::{Point, Swipe, TouchBegin, TouchMove};

        let mut guest_state = state()
            .lock()
            .map_err(|_| "guest state poisoned".to_string())?;
        let actions = match event.kind {
            // Two independent slots prove that WASM state can keep more than
            // one virtual finger alive without ever receiving a scrcpy id.
            EventKind::KeyDown => match event.code.as_deref() {
                Some("KeyW") => vec![DeviceAction::TouchBegin(TouchBegin {
                    slot: 1,
                    point: Point { x: 0.40, y: 0.60 },
                })],
                Some("KeyA") => vec![DeviceAction::TouchBegin(TouchBegin {
                    slot: 2,
                    point: Point { x: 0.20, y: 0.60 },
                })],
                Some("Space") => vec![DeviceAction::Tap(Point { x: 0.70, y: 0.80 })],
                Some("KeyE") => vec![DeviceAction::Swipe(Swipe {
                    from_point: Point { x: 0.30, y: 0.80 },
                    to: Point { x: 0.70, y: 0.80 },
                    duration_ms: 25,
                })],
                _ => Vec::new(),
            },
            EventKind::KeyUp => match event.code.as_deref() {
                Some("KeyW") => vec![DeviceAction::TouchEnd(1)],
                Some("KeyA") => vec![DeviceAction::TouchEnd(2)],
                _ => Vec::new(),
            },
            EventKind::MouseDown if event.button == 0 => {
                guest_state.mouse_left = true;
                vec![DeviceAction::TouchBegin(TouchBegin {
                    slot: 3,
                    point: Point { x: 0.25, y: 0.50 },
                })]
            }
            EventKind::MouseMove if guest_state.mouse_left => {
                vec![DeviceAction::TouchMove(TouchMove {
                    slot: 3,
                    point: Point { x: 0.50, y: 0.50 },
                })]
            }
            EventKind::MouseUp if event.button == 0 && guest_state.mouse_left => {
                guest_state.mouse_left = false;
                vec![DeviceAction::TouchEnd(3)]
            }
            EventKind::GamepadButton if event.index == 0 => vec![DeviceAction::Key(KeyAction {
                code: 62,
                action: if event.pressed { "down" } else { "up" }.to_string(),
            })],
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
                        vec![DeviceAction::TouchMove(TouchMove {
                            slot: 4,
                            point: Point { x, y: 0.30 },
                        })]
                    } else {
                        guest_state.gamepad_axis = true;
                        vec![DeviceAction::TouchBegin(TouchBegin {
                            slot: 4,
                            point: Point { x, y: 0.30 },
                        })]
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
