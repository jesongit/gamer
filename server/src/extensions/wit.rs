//! Checked-in WIT contract metadata. Code generation is intentionally not
//! wired in this phase; the Rust Host facade and this file must evolve
//! together before the optional runtime starts executing components.

pub(crate) const WIT_PACKAGE: &str = include_str!("../../wit/gamer/host.wit");
pub(crate) const WIT_PACKAGE_VERSION: &str = "1.0.0";
