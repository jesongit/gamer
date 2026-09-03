//! Checked-in WIT contract and Wasmtime-generated bindings.

#[cfg(feature = "wasm-runtime")]
wasmtime::component::bindgen!({
    path: "wit/gamer",
    world: "extension-host",
    imports: { default: async },
});

pub(crate) const WIT_PACKAGE: &str = include_str!("../../wit/gamer/host.wit");
pub(crate) const WIT_PACKAGE_VERSION: &str = "1.0.0";
