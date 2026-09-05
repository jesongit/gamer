//! YAML world 的 Wasmtime 宿主（`feature = "wasm-runtime"`）。
//!
//! 自 gamer_yaml 扩展边界导出（P11.3）：guest 的 capability.invoke 经
//! [`yaml_extension::NativeYamlHost`] 落到 Core capability registry，
//! programs.resolve 走调用方注入的 [`YamlProgramResolver`]。通用扩展 world
//! 的宿主仍在 `crate::extensions::wasm`，YAML 专用状态与 runtime 不进入
//! Core 扩展机制模块。

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Engine, Store};

use super::yaml_extension::{
    NativeYamlHost, YamlProgramResolver, YamlWasmRunRequest, YamlWasmRunResult, YamlWasmRuntime,
};
use super::yaml_vnext;
use crate::extensions::host_api::HostApi;
use crate::extensions::wit;

/// Request/response Component runtime for YAML v3. Unlike the generic
/// lifecycle runtime this invokes a supplied lowered program and does not
/// compile the legacy Rust Engine into WASM.
#[derive(Debug)]
pub(crate) struct LazyYamlWasmtimeRuntime {
    engine: OnceLock<Engine>,
    components: Mutex<HashMap<[u8; 32], Arc<Component>>>,
}

impl LazyYamlWasmtimeRuntime {
    pub(crate) fn new() -> Self {
        Self {
            engine: OnceLock::new(),
            components: Mutex::new(HashMap::new()),
        }
    }

    fn engine(&self) -> &Engine {
        self.engine.get_or_init(|| {
            let config = wasmtime::Config::new();
            Engine::new(&config).expect("Wasmtime engine config is valid")
        })
    }
}

/// The YAML world has a separate state type. This keeps its resolver and
/// source-oriented call behavior out of the generic extension HostState.
struct YamlHostState {
    host: HostApi,
    cancelled: Arc<AtomicBool>,
    app_context: Option<crate::core::AppContext>,
    yaml_programs: Option<Arc<dyn YamlProgramResolver>>,
}

impl YamlHostState {
    fn new(
        host: HostApi,
        cancelled: Arc<AtomicBool>,
        app_context: crate::core::AppContext,
        yaml_programs: Option<Arc<dyn YamlProgramResolver>>,
    ) -> Self {
        Self {
            host,
            cancelled,
            app_context: Some(app_context),
            yaml_programs,
        }
    }
}

// `bindgen!` generates one copy of the imported package for each world.
// Keep the YAML adapter explicit rather than weakening the generic Host
// API with YAML-specific types.
impl wit::yaml::gamer::host::types::Host for YamlHostState {}

impl wit::yaml::gamer::host::capability::Host for YamlHostState {
    fn invoke(
        &mut self,
        capability: String,
        args_json: String,
    ) -> Result<String, wit::yaml::gamer::host::types::HostError> {
        let host = self.host.clone();
        let context = self.app_context.clone();
        let cancelled = self.cancelled.clone();
        let result = block_on_yaml(async move {
            let context =
                context.ok_or_else(|| anyhow::anyhow!("capability.invoke 需要 AppContext"))?;
            let value =
                NativeYamlHost::invoke_json(host, context, cancelled, &capability, &args_json)
                    .await?;
            Ok::<_, anyhow::Error>(serde_json::to_string(&value)?)
        });
        result.map_err(|error| yaml_capability_error(&error))
    }
}

impl wit::yaml::gamer::host::programs::Host for YamlHostState {
    fn resolve(&mut self, target: String, args_json: String, depth: u32) -> Result<String, String> {
        let resolver = self
            .yaml_programs
            .clone()
            .ok_or_else(|| "YAML call resolver 未配置".to_string())?;
        super::yaml_extension::check_call_depth(depth).map_err(|error| error.to_string())?;
        let args = serde_json::from_str::<serde_json::Value>(&args_json)
            .map_err(|error| format!("call 参数不是 JSON: {error}"))?;
        let args = yaml_vnext::Value::from_json(args)
            .map_err(|error| format!("call 参数无效: {error}"))?;
        let yaml_vnext::Value::Map(args) = args else {
            return Err("call 参数必须是 map".to_string());
        };
        let program = resolver
            .resolve(&target, &args, depth)
            .map_err(|error| error.to_string())?;
        serde_json::to_string(&program).map_err(|error| error.to_string())
    }
}

fn block_on_yaml<T>(
    future: impl Future<Output = Result<T, anyhow::Error>> + Send + 'static,
) -> Result<T, anyhow::Error>
where
    T: Send + 'static,
{
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| anyhow::anyhow!("YAML capability runtime 初始化失败: {error}"))?
            .block_on(future)
    })
    .join()
    .map_err(|_| anyhow::anyhow!("YAML capability thread 异常退出"))?
}

fn yaml_error(
    kind: wit::yaml::gamer::host::types::HostErrorKind,
    message: impl Into<String>,
) -> wit::yaml::gamer::host::types::HostError {
    wit::yaml::gamer::host::types::HostError {
        kind,
        message: message.into(),
    }
}

fn yaml_capability_error(error: &anyhow::Error) -> wit::yaml::gamer::host::types::HostError {
    use crate::capabilities::CapabilityError;
    use crate::extensions::error::ExtensionError;
    use wit::yaml::gamer::host::types::HostErrorKind;

    let kind = if error
        .downcast_ref::<ExtensionError>()
        .is_some_and(|error| matches!(error, ExtensionError::Permission(_)))
    {
        HostErrorKind::Denied
    } else if let Some(error) = error.downcast_ref::<CapabilityError>() {
        match error {
            CapabilityError::Unavailable(_) => HostErrorKind::Unavailable,
            CapabilityError::InvalidRequest(_) => HostErrorKind::InvalidRequest,
            CapabilityError::NotFound(_) => HostErrorKind::NotFound,
            CapabilityError::Cancelled => HostErrorKind::Cancelled,
            CapabilityError::Failed(_) => HostErrorKind::Failed,
        }
    } else {
        HostErrorKind::Failed
    };
    yaml_error(kind, error.to_string())
}

#[async_trait]
impl YamlWasmRuntime for LazyYamlWasmtimeRuntime {
    async fn run(&self, request: YamlWasmRunRequest) -> Result<YamlWasmRunResult, anyhow::Error> {
        let mut digest = [0u8; 32];
        digest.copy_from_slice(Sha256::digest(&request.wasm).as_slice());
        let component = {
            let mut components = self.components.lock().await;
            if let Some(component) = components.get(&digest).cloned() {
                component
            } else {
                let component = Arc::new(
                    Component::new(self.engine(), &request.wasm)
                        .map_err(|error| anyhow::anyhow!("YAML 组件编译失败: {error}"))?,
                );
                components.insert(digest, component.clone());
                component
            }
        };
        let mut linker = Linker::new(self.engine());
        wit::yaml::YamlExtensionHost::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
            .map_err(|error| anyhow::anyhow!("YAML WIT linker 初始化失败: {error}"))?;
        let state = YamlHostState::new(
            request.host,
            request.stop,
            request.context,
            request.resolver,
        );
        let mut store = Store::new(self.engine(), state);
        let instance = wit::yaml::YamlExtensionHost::instantiate(&mut store, &component, &linker)
            .map_err(|error| anyhow::anyhow!("YAML 组件实例化失败: {error}"))?;
        let mut program = serde_json::to_value(&request.program)?;
        if let serde_json::Value::Object(ref mut program) = program {
            program.insert("args".to_string(), serde_json::to_value(request.args)?);
            // 手动运行「从此运行」：顶层可选 start_index，guest 只按顶层
            // surface 步序号跳步（契约 §8）；None = 从头执行（现状行为）。
            if let Some(start_index) = request.start_index {
                program.insert(
                    "start_index".to_string(),
                    serde_json::Value::from(start_index),
                );
            }
        }
        let program = serde_json::to_string(&program)?;
        let (result,) = instance
            .gamer_host_automation()
            .func_run()
            .call(&mut store, (&program,))
            .map_err(|error| anyhow::anyhow!("YAML guest 执行失败: {error}"))?;
        let result = result.map_err(|error| anyhow::anyhow!("YAML guest 返回错误: {error}"))?;
        let result = serde_json::from_str::<serde_json::Value>(&result)
            .map_err(|error| anyhow::anyhow!("YAML guest 返回值不是 JSON: {error}"))?;
        let value = yaml_vnext::Value::from_json(result)
            .map_err(|error| anyhow::anyhow!("YAML guest 返回值无效: {error}"))?;
        Ok(YamlWasmRunResult { value })
    }

    fn is_available(&self) -> bool {
        true
    }
}
