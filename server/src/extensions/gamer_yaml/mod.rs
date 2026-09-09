//! `gamer.yaml` 扩展边界（ADR-11 / ADR-14 / V1 简化计划）。
//!
//! 本目录物理收编 YAML 自动化栈的全部内容语义：
//!
//! - [`error`]：扩展侧 REST 结构化诊断载体（五元组）；
//! - [`syntax`]：V1 纯数据前端（surface 解析/校验 + wire 降线 + 模板引用
//!   改写 + 确定性序列化）；
//! - [`native_funcs`]：原生（插件）函数注册表（Schema 唯一声明点）；
//! - [`yaml_extension`]：原生函数宿主（`__fn` 后端）、WASM runtime 契约与
//!   官方 manifest 常量；
//! - [`runner_adapter`]：V1 执行器（EngineExecutor）——运行前组合函数注册表
//!   （原生 + 当前 Package）并绑定参数；
//! - [`timer_yaml`]：Timer Core 的 gamer.yaml runner + 扩展生命周期注册器；
//! - [`task_params`]：任务/手动运行参数绑定（按当前 Schema 重绑，无签名门禁）；
//! - [`video_draft`]：视频工作台草稿动作（`automation.create_draft`）；
//! - [`wasm_host`]：YAML world 的 Wasmtime 宿主（feature = "wasm-runtime"）。
//!
//! 执行权威在 `yaml-interp` crate（WASM guest 与 server 测试同源；计划
//! Phase 2：只维护一份解释器）。
//!
//! 依赖方向：本模块 → Core（device / matcher / capabilities / timer_core /
//! run_manager）单向；Core 侧不得 import 本目录内部符号，只能走 Core 定义的
//! 窄 trait（`TimerRunner`、`ResourceHandler` 等）与本文件显式导出的门面。

pub(crate) mod actions;
pub(crate) mod error;
pub(crate) mod native_funcs;
pub(crate) mod resources;
pub(crate) mod run_target;
pub(crate) mod runner_adapter;
pub(crate) mod syntax;
pub(crate) mod task_params;
pub(crate) mod timer_yaml;
pub(crate) mod video_draft;
pub(crate) mod yaml_extension;

/// native_call_action 缝的分发入口在 [`actions`]（版本化公开动作清单：
/// 草稿生成/保存、模板帧上创建；清单 ↔ 实现由测试双向锁死）。
pub(crate) use actions::{
    is_public_native_action, native_action_caller_permissions, native_action_expected_caller,
    native_action_required_permissions, native_action_requires_package_context, native_call_action,
};

/// 公开动作目录（简化计划 Phase 4 能力发现读端）：gamer.yaml 声明的全部
/// 版本化动作（含 Native/Rest/Frontend surface）；非 gamer.yaml 返回空表。
pub(crate) fn public_action_catalog(extension_id: &str) -> Vec<serde_json::Value> {
    if extension_id != YAML_EXTENSION_ID {
        return Vec::new();
    }
    actions::PUBLIC_ACTIONS
        .iter()
        .map(|action| {
            serde_json::json!({
                "action": action.name,
                "version": action.version,
                "surface": match action.surface {
                    actions::ActionSurface::Native => "native",
                    actions::ActionSurface::Rest => "rest",
                    actions::ActionSurface::Frontend => "frontend",
                },
                "summary": action.summary,
                "caller": action.caller,
                "permissions": action
                    .required_permissions
                    .iter()
                    .map(|permission| permission.as_str())
                    .collect::<Vec<_>>(),
            })
        })
        .collect()
}
pub(crate) use resources::register_resource_handlers;
pub(crate) use runner_adapter::{yaml_start_request, EngineExecutor};
pub(crate) use timer_yaml::{YamlTimerRunner, YamlTimerRunnerRegistrar};
pub(crate) use yaml_extension::{YAML_EXTENSION_ID, YAML_EXTENSION_MANIFEST_TOML};

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

/// Execute a lowered YAML V1 program in the installed `gamer.yaml` Component
/// guest. Extension → Core direction only: the guest bytes and host API come
/// from the generic [`crate::extensions::ExtensionService`] lookup; the YAML
/// runtime itself lives behind this boundary.
///
/// `program` = [`syntax::build_program`] 产出的 wire JSON（含冻结函数表与
/// 绑定参数）；`sink` = 运行可视化事件汇（`None` = 静默）。
pub(crate) async fn run_yaml_program(
    service: &crate::extensions::ExtensionService,
    program: serde_json::Value,
    context: crate::core::AppContext,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    sink: Option<std::sync::Arc<dyn crate::core::events::EventSink>>,
) -> Result<serde_json::Value, crate::extensions::ExtensionError> {
    use crate::extensions::ExtensionId;
    let id = ExtensionId::parse(YAML_EXTENSION_ID).expect("built-in YAML extension id is valid");
    service
        .with_guest_for_run(&id, move |wasm, host| async move {
            yaml_runtime()
                .run(yaml_extension::YamlWasmRunRequest {
                    wasm,
                    program,
                    host,
                    context,
                    stop,
                    sink,
                })
                .await
                .map(|result| result.value)
                .map_err(|error| crate::extensions::ExtensionError::Runtime(error.to_string()))
        })
        .await
}

#[cfg(feature = "wasm-runtime")]
pub(crate) mod wasm_host;
