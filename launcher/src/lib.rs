//! gamer-launcher：GameBot 便携安装启动器/升级器。
//!
//! 批次 1（LCH-001/002/003 + QA-001）：CLI、安装根单实例锁、`state/` 原子读写、
//! release manifest v1 验签/语义/路径安全校验。
//! 批次 2（LCH-004~008 + OPS-003 + QA-002）：组件库存深检、seed/cache/remote
//! 下载、安全解压与原子安装、repair 修复编排、server supervisor（env 注入 +
//! 句柄等待 + /health/ready 就绪探测）。
//! 批次 3（LCH-009~012 + QA-004）：named pipe IPC server（ipc-v1 契约）、
//! 升级状态机编排（§6.6 全链路 + 启动恢复）、离线快照与恢复（LCH-011）、
//! 候选启动/提交/回滚（LCH-012）、journal 断电矩阵（QA-004）。
//!
//! 契约来源：`docs/UPDATE_CONTRACT.md`、`release/contracts/manifest-v1.md`、
//! `release/contracts/ipc-v1.md`、`docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md` §6.6-6.8。

pub mod archive;
pub mod cli;
pub mod commands;
pub mod digest;
pub mod fetch;
pub mod installation;
pub mod inventory;
pub mod ipc;
pub mod layout;
pub mod logging;
pub mod manifest;
pub mod repair;
pub mod state;
pub mod supervisor;
pub mod upgrade;
pub mod winutil;
