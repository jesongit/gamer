//! Script execution events emitted to the active WebRTC viewer.
//!
//! Keeping the wire-facing event model separate from the runner makes the
//! execution engine independent from the viewer registry implementation while
//! preserving the existing `engine::ScriptEvent` export.

use serde::Serialize;

/// Script execution visualization event (server → browser).
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum ScriptEvent {
    /// Engine tap in device pixel coordinates.
    Tap { x: u32, y: u32 },
    /// Engine swipe in device pixel coordinates.
    Swipe { x1: u32, y1: u32, x2: u32, y2: u32 },
    /// Template match hit in device pixel coordinates.
    Hit {
        tpl: String,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        score: f32,
    },
    /// Search area shown when a template did not match.
    Miss {
        tpl: String,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::ScriptEvent;

    #[test]
    fn events_keep_the_data_channel_wire_shape() {
        let event = serde_json::to_value(ScriptEvent::Tap { x: 12, y: 34 }).unwrap();
        assert_eq!(event["ev"], "tap");
        assert_eq!(event["x"], 12);
        assert_eq!(event["y"], 34);
    }
}
