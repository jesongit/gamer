//! Checked-in WIT contract and Wasmtime-generated bindings.

// Guest execution stays on a dedicated ordinary OS thread. Synchronous host
// bindings bridge capability futures to the captured server Tokio Handle,
// matching the YAML/keymap execution boundary. This avoids Wasmtime Windows
// fiber TLS teardown corrupting Tokio's entered-runtime guard. Both imports
// and exports are synchronous together: no async-required Store is created.
// These are Rust calling conventions only; the WIT ABI and guest package
// version remain unchanged.
#[cfg(feature = "wasm-runtime")]
wasmtime::component::bindgen!({
    path: "wit/gamer",
    world: "extension-host",
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

/// YAML v3 uses a request/response export so the app package can provide a
/// lowered program for each run. It shares the generic capability.invoke host
/// import; no YAML type enters the Core capability layer.
#[cfg(feature = "wasm-runtime")]
pub(crate) mod yaml {
    wasmtime::component::bindgen!({
        path: "wit/gamer",
        world: "yaml-extension-host",
    });
}

pub(crate) const WIT_PACKAGE: &str = include_str!("../../wit/gamer/host.wit");
pub(crate) const WIT_PACKAGE_VERSION: &str = "1.0.0";
