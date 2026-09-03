//! WASM runtime seam.
//!
//! The default implementation is deliberately unavailable. This lets the
//! extension store and lifecycle be tested without making a server with zero
//! installed extensions initialize a compiler or an execution engine.

use async_trait::async_trait;
use uuid::Uuid;

use super::error::{ExtensionError, ExtensionResult};
use super::host_api::HostApi;
use super::model::{ExtensionId, ExtensionVersion};

#[derive(Clone)]
pub(crate) struct WasmStartRequest {
    pub(crate) id: ExtensionId,
    pub(crate) version: ExtensionVersion,
    pub(crate) wasm: Vec<u8>,
    pub(crate) host: HostApi,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct WasmInstanceHandle(Uuid);

impl WasmInstanceHandle {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[async_trait]
pub(crate) trait WasmRuntime: Send + Sync {
    async fn start(&self, request: WasmStartRequest) -> ExtensionResult<WasmInstanceHandle>;

    async fn stop(&self, instance: WasmInstanceHandle) -> ExtensionResult<()>;

    fn is_available(&self) -> bool;
}

/// Runtime used by the default server build. It never compiles, instantiates,
/// or executes untrusted bytes.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NoWasmRuntime;

#[async_trait]
impl WasmRuntime for NoWasmRuntime {
    async fn start(&self, _request: WasmStartRequest) -> ExtensionResult<WasmInstanceHandle> {
        Err(ExtensionError::RuntimeUnavailable(
            "未启用 wasm-runtime feature",
        ))
    }

    async fn stop(&self, _instance: WasmInstanceHandle) -> ExtensionResult<()> {
        Err(ExtensionError::RuntimeUnavailable(
            "未启用 wasm-runtime feature",
        ))
    }

    fn is_available(&self) -> bool {
        false
    }
}

/// Lazy adapter boundary for Wasmtime.
///
/// This phase only validates a core WASM artifact after `start` is requested.
/// It intentionally does not instantiate a component, bind the WIT imports,
/// call an extension entrypoint, or report a running instance. The adapter is
/// therefore safe to ship behind the opt-in feature while those contracts are
/// completed in the next phase.
#[cfg(feature = "wasm-runtime")]
#[derive(Debug, Default)]
pub(crate) struct LazyWasmtimeRuntime {
    engine: std::sync::OnceLock<wasmtime::Engine>,
}

#[cfg(feature = "wasm-runtime")]
impl LazyWasmtimeRuntime {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn is_initialized(&self) -> bool {
        self.engine.get().is_some()
    }
}

#[cfg(feature = "wasm-runtime")]
#[async_trait]
impl WasmRuntime for LazyWasmtimeRuntime {
    async fn start(&self, request: WasmStartRequest) -> ExtensionResult<WasmInstanceHandle> {
        let engine = self.engine.get_or_init(wasmtime::Engine::default);
        wasmtime::Module::new(engine, &request.wasm)
            .map_err(|error| ExtensionError::Runtime(error.to_string()))?;
        Err(ExtensionError::RuntimeUnavailable(
            "WIT Host bindings 与插件入口尚未接入",
        ))
    }

    async fn stop(&self, _instance: WasmInstanceHandle) -> ExtensionResult<()> {
        Err(ExtensionError::RuntimeUnavailable(
            "WIT Host bindings 与插件入口尚未接入",
        ))
    }

    fn is_available(&self) -> bool {
        true
    }
}
