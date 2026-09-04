//! YAML 自动化脚本执行引擎（2026-08 阶段 2 后半重写：严格 AST 执行，取代
//! v1 serde_yaml 动态解析；旧语法在装载层给出结构化诊断，不再兼容）。
//!
//! 语法与执行语义的权威定义：docs/reference/SCRIPT_EDITOR_CONTRACT.md +
//! docs/plans/archive/SCRIPT_EDITOR_REDESIGN_PLAN.md §7/§12.2/§13.3。装载/校验在
//! `crate::script_v2`（parse_script_file / parse_function_file，
//! `Result<_, Vec<ScriptError>>`）；本模块只做执行与运行编排：
//!
//! - [`exec`]：`RunTarget`（脚本 / 函数测试二选一）→ 快照 → 严格解析 →
//!   参数绑定（声明默认值 → args 覆盖）→ AST 步骤执行（find/match/color/if/
//!   loop/break/call/func/throw/return 等 19 类；10 万步 guard + 32 层嵌套 + 取消
//!   轮询；tap/swipe/hit/miss 可视化事件经 control DataChannel 反向推送）。
//! - [`snapshot`]：运行开始时整体捕获分区 `scripts/`+`functions/` 源码，call/func 从
//!   快照懒解析并按运行实例缓存——运行中改文件不影响已开始的实例。
//! - [`ports`]：截图源 / 设备控制 / 模板匹配三个窄 trait；生产在
//!   `Runner::new` 装配 adapter 转发 DeviceManager / matcher，测试注入 fake。
//!
//! 可视化事件（v1 语义保持）：tap/swipe/匹配命中/未命中时推送给该设备当前
//! viewer（emit → 注入的 EventSink；WebRTC adapter 无 viewer 时静默丢弃）。

#[path = "engine_events.rs"]
mod events;

pub mod exec;
pub mod ports;
pub mod runner_adapter;
pub mod snapshot;

// bin crate：部分再导出仅被 API/测试/未来阶段消费，未在本 crate 内使用
#[allow(unused_imports)]
pub use events::ScriptEvent;
#[allow(unused_imports)]
pub use exec::{
    key_code, load_entry_param_decls, resolve_entry_args, BoundEntryArgs, RunSpec, RunTarget,
    Runner,
};
pub use runner_adapter::{yaml_app_context, yaml_start_request, EngineExecutor};
