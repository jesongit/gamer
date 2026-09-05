//! `gamer.yaml` 扩展边界（P11.3 / ADR-11 / ADR-14）。
//!
//! 本目录物理收编 YAML 自动化栈的全部内容语义：
//!
//! - [`error`]：扩展侧 REST 结构化诊断载体（五元组）；
//! - [`params`]：参数绑定共用的标量解析与校验；
//! - [`run_target`]：运行请求描述（RunTarget / RunSpec / TypedValue wire）；
//! - [`yaml_vnext`]：v3 纯数据前端（version:3 判别 + 小 AST wire 形态）；
//! - [`yaml_extension`]：v3 原生参考解释器、capability invoker、保存/导入
//!   校验入口与 WASM runtime 契约 trait；
//! - [`runner_adapter`]：v3-only 执行器（EngineExecutor）——非 `version: 3`
//!   脚本统一报版本错误，无 fallback；
//! - [`timer_yaml`]：Timer Core 的 gamer.yaml runner + 扩展生命周期注册器；
//! - [`task_params`]：定时任务参数快照与 psig1 签名门禁（v3 参数桥）；
//! - [`wasm_host`]：YAML world 的 Wasmtime 宿主（feature = "wasm-runtime"）。
//!
//! 依赖方向（§16）：本模块 → Core（device / matcher / capabilities / timer_core /
//! run_manager / app_packages composite）单向；Core 侧（api / store / timer_core /
//! scheduler / webrtc / capabilities）不得 import 本目录内部符号，只能走 Core 定义的
//! 窄 trait（`TimerRunner`、`ResourceKindHandler` 等）与本文件显式导出的门面。

pub(crate) mod error;
pub(crate) mod params;
pub(crate) mod resources;
pub(crate) mod run_target;
pub(crate) mod runner_adapter;
pub(crate) mod task_params;
pub(crate) mod timer_yaml;
pub(crate) mod yaml_extension;
pub(crate) mod yaml_vnext;

pub(crate) use runner_adapter::{yaml_app_context, yaml_start_request, EngineExecutor};
pub(crate) use resources::register_resource_handlers;
pub(crate) use timer_yaml::{YamlTimerRunner, YamlTimerRunnerRegistrar};
pub(crate) use yaml_extension::{
    YamlProgramResolver, YAML_EXTENSION_ID, YAML_EXTENSION_MANIFEST_TOML,
};

/// gamer.yaml 的进程级 WASM runtime（feature 选择 Lazy / No 实现）。
pub(crate) fn yaml_runtime() -> std::sync::Arc<dyn yaml_extension::YamlWasmRuntime> {
    #[cfg(feature = "wasm-runtime")]
    {
        use std::sync::OnceLock;
        static RUNTIME: OnceLock<std::sync::Arc<dyn yaml_extension::YamlWasmRuntime>> =
            OnceLock::new();
        RUNTIME
            .get_or_init(|| std::sync::Arc::new(wasm_host::LazyYamlWasmtimeRuntime::new()))
            .clone()
    }
    #[cfg(not(feature = "wasm-runtime"))]
    {
        std::sync::Arc::new(yaml_extension::NoYamlWasmRuntime)
    }
}

/// Execute a lowered YAML v3 program in the installed `gamer.yaml` Component
/// guest. Extension → Core direction only: the guest bytes and host API come
/// from the generic [`crate::extensions::ExtensionService`] lookup; the YAML
/// runtime itself lives behind this boundary.
///
/// `start_index`（契约 §8）：顶层可选「从此运行」步序号，经
/// [`yaml_extension::YamlWasmRunRequest`] 透传给 guest 注入 program JSON；
/// `None` = 从头执行。
/// `sink`（P12.6）：运行可视化事件汇（`__event` 私有通道拦截 + 宿主侧
/// vision/input 补发）；`None` = 静默。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_yaml_vnext(
    service: &crate::extensions::ExtensionService,
    program: yaml_vnext::Program,
    context: crate::core::AppContext,
    args: std::collections::BTreeMap<String, yaml_vnext::Value>,
    resolver: Option<std::sync::Arc<dyn YamlProgramResolver>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    start_index: Option<usize>,
    sink: Option<std::sync::Arc<dyn crate::core::events::EventSink>>,
) -> Result<yaml_vnext::Value, crate::extensions::ExtensionError> {
    use crate::extensions::ExtensionId;
    let id = ExtensionId::parse(YAML_EXTENSION_ID).expect("built-in YAML extension id is valid");
    let (wasm, host) = service.guest_for_run(&id).await?;
    yaml_runtime()
        .run(yaml_extension::YamlWasmRunRequest {
            wasm,
            program,
            args,
            resolver,
            start_index,
            host,
            context,
            stop,
            sink,
        })
        .await
        .map(|result| result.value)
        .map_err(|error| crate::extensions::ExtensionError::Runtime(error.to_string()))
}

#[cfg(feature = "wasm-runtime")]
pub(crate) mod wasm_host;
