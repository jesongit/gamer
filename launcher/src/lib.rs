//! gamer-launcher：GameBot 便携安装启动器/升级器。
//!
//! 批次 1（LCH-001/002/003 + QA-001）：CLI、安装根单实例锁、`state/` 原子读写、
//! release manifest v1 验签/语义/路径安全校验。
//! 批次 2（LCH-004~008 + OPS-003 + QA-002）：组件库存深检、seed/cache/remote
//! 下载、安全解压与原子安装、repair 修复编排、server supervisor（env 注入 +
//! 句柄等待 + /health/ready 就绪探测）。
//!
//! 契约来源：`docs/UPDATE_CONTRACT.md`、`release/contracts/manifest-v1.md`。

pub mod archive;
pub mod cli;
pub mod commands;
pub mod digest;
pub mod fetch;
pub mod inventory;
pub mod layout;
pub mod logging;
pub mod manifest;
pub mod repair;
pub mod state;
pub mod supervisor;
