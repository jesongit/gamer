//! gamer-launcher：GameBot 便携安装启动器/升级器。
//!
//! 批次 1 骨架（LCH-001/002/003 + QA-001）：
//! - CLI（start/status/doctor/repair，upgrade 预留）
//! - 安装根单实例锁与 `state/` 原子读写（current.json / update-journal.json）
//! - release manifest v1 解析 + Ed25519 分离签名验签 + 语义/路径安全校验
//!
//! 契约来源：`docs/UPDATE_CONTRACT.md`、`release/contracts/manifest-v1.md`。

pub mod cli;
pub mod commands;
pub mod layout;
pub mod logging;
pub mod manifest;
pub mod state;
