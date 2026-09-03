//! Compatibility export for the old engine event name.
//!
//! The event model now lives in `core`; this alias keeps the YAML engine and
//! existing tests source-compatible while removing all viewer knowledge from
//! the runner.

pub use crate::core::RuntimeEventKind as ScriptEvent;

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
