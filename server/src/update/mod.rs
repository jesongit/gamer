//! 更新子系统（批次 3 Server/Data/API 轨道）：
//!
//! - [`model`]：11 态状态机 + 11 错误码 + §4.2 受理矩阵 + §4.3 门禁枚举
//!   （纯函数层，fixture 比对与矩阵测试直接驱动）；
//! - [`ipc`]：launcher IPC 客户端帧协议（u32 LE 前缀 JSON、1 MiB 上限、6 操作、
//!   受理/错误帧解析——ipc-v1 冻结契约）；
//! - [`pipe`]：Windows named pipe 客户端传输（连接 5s / 交换 30s 有界超时）；
//! - [`controller`]：UpdateController 三实现（launcher 托管 / 直跑 unsupported /
//!   Docker external）；
//! - [`policy`]：更新策略对象 + state/ JSON 持久化（PUT policy 热生效）；
//! - [`workload`]：业务空闲摘要（OPS-005；install 门禁 + auto 协调器输入）；
//! - [`service`]：HTTP API 与协调器的共享状态层（状态聚合/动作受理/审计）；
//! - [`coordinator`]：策略协调器（SYS-005；off/notify/auto 周期评估）；
//! - [`gate`]：candidate activation gate（OPS-004；startup.stage 投影）。

pub mod controller;
pub mod coordinator;
pub mod gate;
pub mod ipc;
pub mod model;
pub mod pipe;
pub mod policy;
pub mod service;
pub mod workload;
