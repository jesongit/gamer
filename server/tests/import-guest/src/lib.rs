//! Extension-world fixture guest **with host imports** (regression fixture for
//! the 2026-09 async-import defect): the generic runtime linked async host
//! imports but entered the guest synchronously, so any plugin that declared
//! imports trapped at start ("store configuration requires that *_async
//! functions are used instead") and the fiber teardown aborted the process.
//! Zero-import fixtures (WAT entry/call-guest) never triggered the link-time
//! `async_required` flag and thus missed the bug.
//!
//! This guest exercises the two cheapest import domains end to end:
//! - `context.get` (no permission needed, no capability service needed)
//! - `log.write` (needs the `log.write` permission + a registered log service;
//!   errors are swallowed so the fixture stays runnable on bare registries)
//!
//! `call("ping")` echoes the app context it observed via the host import, so a
//! host-side assertion on the returned JSON proves imports actually round-trip.

wit_bindgen::generate!({
    path: "../../wit/gamer",
    world: "extension-host",
});

use exports::gamer::host::extension::Guest;
use gamer::host::context;
use gamer::host::log;

fn context_json() -> String {
    let context = context::get();
    serde_json::json!({
        "device_id": context.device_id,
        "android_package": context.android_package,
        "content_package": context.content_package,
    })
    .to_string()
}

struct ImportEcho;

impl Guest for ImportEcho {
    fn run() {
        // 宿主 import 往返：读上下文 + 尽力而为写日志（无 log 服务时静默）。
        let _ = log::write("info", "import-guest run", None, None);
    }

    fn call(action: String, _values_json: String) -> Result<String, String> {
        match action.as_str() {
            "ping" => Ok(serde_json::json!({
                "action": action,
                "context": serde_json::from_str::<serde_json::Value>(&context_json())
                    .unwrap_or(serde_json::Value::Null),
            })
            .to_string()),
            other => Err(format!("未知 action: {other}")),
        }
    }
}

export!(ImportEcho);
