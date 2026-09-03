//! Checked-in WIT contract and Wasmtime-generated bindings.

#[cfg(feature = "wasm-runtime")]
wasmtime::component::bindgen!({
    path: "wit/gamer",
    world: "extension-host",
    imports: { default: async },
});

/// Keymap has a deliberately separate world. The generic extension world
/// remains the Phase 6 lifecycle contract; the keymap world adds the typed
/// event/result entrypoint without changing other extensions.
#[cfg(feature = "wasm-runtime")]
pub(crate) mod keymap {
    wasmtime::component::bindgen!({
        path: "wit/keymap",
        world: "keymap-host",
    });
}

pub(crate) const WIT_PACKAGE: &str = include_str!("../../wit/gamer/host.wit");
pub(crate) const WIT_PACKAGE_VERSION: &str = "1.0.0";
