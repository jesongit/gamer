//! Generic extension-world fixture guest: exports `run` + `call` so the
//! declarative `plugin.call` host path can be exercised end to end.
//!
//! `call` echoes the action and the parsed values JSON back to the host,
//! proving the action/values wire contract without touching any capability.

wit_bindgen::generate!({
    path: "../../wit/gamer",
    world: "extension-host",
});

use exports::gamer::host::extension::Guest;

struct Echo;

impl Guest for Echo {
    fn run() {}

    fn call(action: String, values_json: String) -> Result<String, String> {
        let values = serde_json::from_str::<serde_json::Value>(&values_json)
            .unwrap_or(serde_json::Value::Null);
        Ok(serde_json::json!({
            "echo": { "action": action, "values": values }
        })
        .to_string())
    }
}

export!(Echo);
