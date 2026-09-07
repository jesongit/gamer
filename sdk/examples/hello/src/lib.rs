//! Hello 最小第三方插件 guest。
//!
//! 目标：证明写一个 Gamer 插件不需要复制 YAML/Keymap 的任何代码——
//! 一个 manifest v2 + 一个导出 `gamer:host/extension@1.0.0`（`run` + `call`）
//! 的 WASM Component 就够了。
//!
//! 生命周期模型（宿主 `LazyWasmtimeRuntime`）：
//! - `run` 在实例化后被调用一次；返回后实例停在命令循环等待 `call`。
//!   本示例把初始化（读运行上下文 + 写日志）放在 `run` 里，然后返回。
//! - `call(action, values-json)` 是 declarative UI 面板按钮的后端入口；
//!   宿主在派发前校验：插件必须 Running 且 action 在 manifest declarative
//!   按钮集合内（`CallRejected` → 400）。返回值必须是 JSON 字符串。
//!
//! 权限闭集：本插件只声明 `resource.read` + `log.write`。任何越权调用
//! （见 `probe_denied`）都会在宿主 capability 边界收到 `kind=denied`。

wit_bindgen::generate!({
    // 契约快照：gamer:host@1.0.0（与宿主 HOST_API_VERSION 对应）。
    // 独立项目里这份 wit/ 随示例走；ABI 变更（宿主升版本）时需同步。
    path: "wit/gamer",
    world: "extension-host",
});

use exports::gamer::host::extension::Guest;
use gamer::host::types::{HostError, HostErrorKind};
use gamer::host::{context, device, log, resources};

/// 本插件在 Package 内的私有数据路径（宿主强制限定在
/// `packages/<content_package>/plugins/com.example.hello/` 前缀内，
/// plugin 维度由宿主注入调用方身份，guest 无法寻址其他插件目录）。
const STATE_PATH: &str = "data/state.json";

struct HelloPlugin;

/// 写插件日志（log.write 已声明；失败静默——示例不因日志通道问题中断业务）。
fn write_log(level: &str, message: &str) {
    let _ = log::write(level, message, None, None);
}

fn kind_str(kind: &HostErrorKind) -> &'static str {
    match kind {
        HostErrorKind::Denied => "denied",
        HostErrorKind::Unavailable => "unavailable",
        HostErrorKind::InvalidRequest => "invalid-request",
        HostErrorKind::NotFound => "not-found",
        HostErrorKind::Cancelled => "cancelled",
        HostErrorKind::Failed => "failed",
    }
}

fn err_json(error: &HostError) -> String {
    serde_json::json!({
        "ok": false,
        "kind": kind_str(&error.kind),
        "message": error.message,
    })
    .to_string()
}

/// 运行上下文（Device/App/Package 三层；Plugin 层由宿主身份承载不进上下文）。
/// `context.get()` 不需要权限，随实例注入。
fn log_app_context() {
    let ctx = context::get();
    write_log(
        "info",
        format!(
            "hello: app_context = device_id={:?} android_package={:?} content_package={:?}",
            ctx.device_id, ctx.android_package, ctx.content_package
        )
        .as_str(),
    );
}

/// action=greet：读 declarative 表单值 → 写插件日志 → 原样回显。
fn greet(values: &serde_json::Value) -> Result<String, String> {
    let message = values
        .get("message")
        .and_then(|value| value.as_str())
        .unwrap_or("你好，Gamer");
    let requested = values
        .get("level")
        .and_then(|value| value.as_str())
        .unwrap_or("info");
    let level = match requested {
        "debug" | "info" | "warn" => requested,
        _ => "info",
    };
    write_log(level, format!("hello: {message}").as_str());
    Ok(serde_json::json!({
        "ok": true,
        "action": "greet",
        "echo": message,
        "logged_at_level": level,
    })
    .to_string())
}

/// action=check_resource：在自身插件私有前缀内解析 `data/state.json`。
///
/// 边界说明（host API 1.0.0 现状）：WIT `resources` 域只暴露
/// `resolve`（逻辑名 → 句柄，文件必须存在）与 `open`（字节长度），
/// 字节内容读取/写入尚未开放给 guest——写侧目前走 Package 资源 REST
/// （`PUT /api/packages/:pkg/plugins/com.example.hello/resources/...`），
/// 读取字节内容同理。本动作验证的是「私有数据定位 + 存在性 + 大小」链路
/// 与插件数据隔离（宿主把 plugin 维度钉死为调用方 id）。
fn check_resource() -> Result<String, String> {
    let ctx = context::get();
    let Some(package) = ctx.content_package else {
        return Ok(serde_json::json!({
            "ok": false,
            "action": "check_resource",
            "error": "no_content_package",
            "hint": "实例启动未携带 app_context.content_package；用带 app_context 的 start 指定数据上下文",
        })
        .to_string());
    };
    match resources::resolve(&package, STATE_PATH) {
        Ok(handle) => {
            let bytes = resources::open(handle).map_err(|error| err_json(&error))?;
            Ok(serde_json::json!({
                "ok": true,
                "action": "check_resource",
                "found": true,
                "package": package,
                "path": STATE_PATH,
                "bytes": bytes,
            })
            .to_string())
        }
        // 资源不存在是正常业务态（还没写过数据），不算插件错误。
        Err(error) if matches!(error.kind, HostErrorKind::NotFound) => Ok(serde_json::json!({
            "ok": true,
            "action": "check_resource",
            "found": false,
            "package": package,
            "path": STATE_PATH,
        })
        .to_string()),
        Err(error) => Err(err_json(&error)),
    }
}

/// action=probe_denied：权限闭集自检。
///
/// 本插件**没有**声明 `device.read`，宿主必须在 capability 派发前拒绝
/// `device.resolve`（`kind=denied`）。把声明移出 manifest 再装一次，
/// 这个动作就会从 denied 变成 not-found——这就是权限声明真实生效的证据。
fn probe_denied() -> Result<String, String> {
    let outcome = match device::resolve("selftest-nonexistent") {
        Ok(_) => serde_json::json!({
            "ok": true,
            "action": "probe_denied",
            "denied": false,
            "note": "unexpected：未声明 device.read 却通过了宿主权限检查",
        }),
        Err(error) => serde_json::json!({
            "ok": true,
            "action": "probe_denied",
            "denied": matches!(error.kind, HostErrorKind::Denied),
            "kind": kind_str(&error.kind),
            "message": error.message,
        }),
    };
    Ok(outcome.to_string())
}

impl Guest for HelloPlugin {
    /// 实例化后调用一次。做初始化然后返回；返回后实例停在该实例线程的
    /// 命令循环里等 `call`（declarative 面板按钮），stop 时被优雅收尾。
    fn run() {
        write_log("info", "hello: 插件实例已启动（gamer:host/extension@1.0.0）");
        log_app_context();
        write_log("info", "hello: run() 返回，等待 declarative 面板 call");
    }

    fn call(action: String, values_json: String) -> Result<String, String> {
        let values = serde_json::from_str::<serde_json::Value>(&values_json)
            .unwrap_or(serde_json::Value::Null);
        match action.as_str() {
            "greet" => greet(&values),
            "check_resource" => check_resource(),
            "probe_denied" => probe_denied(),
            other => Err(serde_json::json!({
                "ok": false,
                "error": "unknown_action",
                "action": other,
            })
            .to_string()),
        }
    }
}

export!(HelloPlugin);
