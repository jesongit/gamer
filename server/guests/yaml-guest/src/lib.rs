wit_bindgen::generate!({
    path: "../../wit/gamer",
    world: "yaml-extension-host",
});

use exports::gamer::host::automation::Guest;
use gamer::host::capability;
use yaml_interp::{EventSink, HostError, HostErrorKind, HostFunctions};

/// gamer.yaml 官方产品 guest（计划 Phase 2：唯一权威解释器在 `yaml-interp`
/// crate，与宿主测试链路共享同一份源码；本 crate 只是 WIT 胶水）。宿主下发
/// 解析/校验/绑定后的 V1 程序 JSON（含运行开始时冻结的当前 Package 函数表）；
/// guest 解释控制流（函数调用 / if / repeat / return），原生函数经私有
/// `__fn` 通道转发宿主，运行结构事件经 `__event` 通道发射（均不经权限声明）。
///
/// 执行预算（MAX_STEPS / MAX_CALL_DEPTH）与取消兜底语义在 `yaml-interp` 内
/// 实现；epoch interruption 取消归宿主（wasm_host.rs）。
struct YamlGuest;

/// 宿主函数通道：`capability.invoke("__fn", {"name", "args"})`。
struct GuestHost;

impl HostFunctions for GuestHost {
    fn invoke(&self, name: &str, args: serde_json::Value) -> Result<serde_json::Value, HostError> {
        let payload = serde_json::json!({ "name": name, "args": args });
        let result = capability::invoke("__fn", &payload.to_string())
            .map_err(map_wit_error)?;
        serde_json::from_str(&result)
            .map_err(|error| HostError::new(HostErrorKind::Failed, format!("宿主函数 {name} 返回值无效: {error}")))
    }
}

/// 运行结构事件通道（尽力而为，失败静默）：`capability.invoke("__event", …)`。
struct GuestSink;

impl EventSink for GuestSink {
    fn emit(&self, event: serde_json::Value) {
        if let Ok(args) = serde_json::to_string(&event) {
            let _ = capability::invoke("__event", &args);
        }
    }
}

fn map_wit_error(error: gamer::host::types::HostError) -> HostError {
    use gamer::host::types::HostErrorKind as Wit;
    let kind = match error.kind {
        Wit::Denied => HostErrorKind::Denied,
        Wit::Unavailable => HostErrorKind::Unavailable,
        Wit::InvalidRequest => HostErrorKind::InvalidRequest,
        Wit::NotFound => HostErrorKind::NotFound,
        Wit::Cancelled => HostErrorKind::Cancelled,
        Wit::Failed => HostErrorKind::Failed,
    };
    HostError::new(kind, error.message)
}

impl Guest for YamlGuest {
    fn run(program_json: String) -> Result<String, String> {
        let program: yaml_interp::Program = serde_json::from_str(&program_json)
            .map_err(|error| format!("program JSON 无效: {error}"))?;
        let value = yaml_interp::run(&program, &GuestHost, Some(&GuestSink))?;
        serde_json::to_string(&value).map_err(|error| error.to_string())
    }
}

export!(YamlGuest);
