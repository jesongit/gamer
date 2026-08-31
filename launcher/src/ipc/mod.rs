//! LCH-009：Windows named pipe IPC server（契约：release/contracts/ipc-v1.md，冻结）。
//!
//! - pipe：`\\.\pipe\gamebot-launcher-<installation-id>`，DACL 仅当前用户 + SYSTEM
//!   （ipc/dacl.rs），字节模式，一请求一响应；
//! - 帧：u32 LE 长度前缀 + UTF-8 JSON，单帧上限 1 MiB（超限立即断开不回帧），
//!   单帧交换超时 30s；
//! - 幂等：request_id 去重窗口 10 分钟，重发回原受理帧，绝不二次触发事务；
//! - 操作枚举（冻结 6 个）：status/check/download/prepare_install/rollback/
//!   repair_dependency → 内部枚举，转 Engine 阶段方法或 repair。

pub mod dacl;
pub mod dispatch;
pub mod frames;
// `server.rs` was part of the pre-existing batch-3 working tree and contains
// invalid UTF-8 bytes. Keep it untouched as user-owned work; compile the
// corrected implementation from a clean source file instead.
#[path = "server_v1.rs"]
pub mod server;

#[cfg(test)]
mod lch009_tests;

pub use dispatch::Dispatcher;
pub use server::{run_server, IpcServerConfig};

/// 协议版本（冻结：恒为 1）。
pub const PROTOCOL_VERSION: u32 = 1;
/// 单帧上限 1 MiB（建议值冻结于 ipc-v1 §9）。
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
/// 单帧交换超时（建议值）。
pub const FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// request_id 去重窗口（建议值）。
pub const DEDUP_WINDOW: std::time::Duration = std::time::Duration::from_secs(600);
