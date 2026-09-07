//! echo-minimal：**零 import** 的最小第三方插件 guest。
//!
//! 本组件不导入 `gamer:host` 的任何域（manifest 因此也不声明任何权限），
//! 只导出 `gamer:host/extension@1.0.0` 的 `run` + `call`。它是能在当前
//! 服务端基线上完整走通 安装→启动→调用→停止→卸载 的最小模板，也是
//! 「纯 declarative 面板 + guest 内计算」类插件（表单校验、文本处理、
//! 配置生成等）的正确起点。
//!
//! 需要调用 Host API（设备/视觉/输入/资源/日志/上下文）时，请改用
//! `hello` / `vision-probe` 示例的写法：在 manifest 声明权限与 host_api 域，
//! guest 内 `use gamer::host::...` 直接调用受控函数。

wit_bindgen::generate!({
    // 契约快照：gamer:host@1.0.0。只导出 world 要求的 extension 接口，
    // 不导入任何域。
    path: "wit/gamer",
    world: "extension-host",
});

use exports::gamer::host::extension::Guest;

struct EchoMinimal;

impl Guest for EchoMinimal {
    /// declarative 插件的入口通常立即返回；实例停在命令循环等 `call`。
    fn run() {}

    fn call(action: String, values_json: String) -> Result<String, String> {
        let values = serde_json::from_str::<serde_json::Value>(&values_json)
            .unwrap_or(serde_json::Value::Null);
        match action.as_str() {
            // 回显：证明 action/values 线上契约与实例存活。
            "echo" => Ok(serde_json::json!({
                "ok": true,
                "action": action,
                "echo": values,
            })
            .to_string()),
            // 纯 guest 计算示例：无任何宿主往返。
            "sum" => {
                let a = values.get("a").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let b = values.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0);
                Ok(serde_json::json!({
                    "ok": true,
                    "action": action,
                    "sum": a + b,
                })
                .to_string())
            }
            other => Err(serde_json::json!({
                "ok": false,
                "error": "unknown_action",
                "action": other,
            })
            .to_string()),
        }
    }
}

export!(EchoMinimal);
