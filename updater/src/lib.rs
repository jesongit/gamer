//! Internal portable update engine: verified staging, offline snapshot, restart and recovery.
//! Built into Gamer; no standalone launcher or named-pipe server.

pub mod app_inventory;
pub mod archive;
pub mod bootstrap;
pub mod digest;
pub mod distribution;
pub mod fetch;
pub mod installation;
pub mod inventory;
pub mod ipc;
pub mod layout;
pub mod manifest;
pub mod official_plugins;
pub mod repair;
pub mod state;
pub mod supervisor;
pub mod transfer;
pub mod upgrade;
pub mod verification;
pub mod winutil;

pub mod portable;

pub mod tray;
