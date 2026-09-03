//! Feature-gated WASM Component Model runtime.
//!
//! The default build uses [`NoWasmRuntime`]. The Wasmtime implementation is
//! only compiled with `wasm-runtime`, creates no engine until the first start,
//! and links only the checked-in Gamer WIT world. In particular, no WASI
//! preview imports are installed, so a guest cannot obtain filesystem,
//! network, shell, or process-spawn access accidentally.

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
    pub(crate) app_context: Option<crate::core::AppContext>,
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

#[cfg(feature = "wasm-runtime")]
mod wasmtime_runtime {
    use std::collections::HashMap;
    use std::future::Future;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;

    use async_trait::async_trait;
    use sha2::{Digest, Sha256};
    use tokio::sync::{oneshot, Mutex};
    use wasmtime::component::{Component, HasSelf, Linker};
    use wasmtime::{Engine, Store};

    use crate::capabilities::{
        AppId, ColorSample, DeviceHandle, DeviceId, FrameHandle, FramePoint, KeyAction, KeyCode,
        KeyInput, LogLevel, LogRecord, MatchOptions, MatchOutcome as CoreMatchOutcome,
        ResourceHandle, ResourceId, RunHandle, RunRequest, RunStatus, RuntimeService, SwipeGesture,
        TemplateQuery, TextInput, TouchHandle, TouchPoint,
    };

    use super::super::error::{ExtensionError, ExtensionResult};
    use super::super::host_api::HostApi;
    use super::super::model::ExtensionId;
    use super::super::permissions::Permission;
    use super::super::wit;
    use super::{WasmInstanceHandle, WasmRuntime, WasmStartRequest};

    type Bindings = wit::ExtensionHost;

    use wit::gamer::host::{context, device, input, log, resources, run, runtime, touch, vision};

    type WitError = wit::gamer::host::types::HostError;
    type WitErrorKind = wit::gamer::host::types::HostErrorKind;

    /// Per-instance state. Numeric WIT handles are scoped to one guest and
    /// map to native opaque capability handles; no host path crosses WIT.
    pub(crate) struct HostState {
        host: HostApi,
        cancelled: Arc<AtomicBool>,
        runtime: Arc<dyn RuntimeService>,
        app_context: Option<crate::core::AppContext>,
        next_handle: u64,
        devices: HashMap<u64, DeviceHandle>,
        frames: HashMap<u64, FrameHandle>,
        resources: HashMap<u64, ResourceHandle>,
        touches: HashMap<u64, TouchHandle>,
        runs: HashMap<u64, RunHandle>,
    }

    impl HostState {
        pub(crate) fn new(
            host: HostApi,
            cancelled: Arc<AtomicBool>,
            app_context: Option<crate::core::AppContext>,
        ) -> Self {
            let runtime: Arc<dyn RuntimeService> = Arc::new(
                crate::capabilities::adapters::RuntimeAdapter::new(cancelled.clone()),
            );
            Self {
                host,
                cancelled,
                runtime,
                app_context,
                next_handle: 1,
                devices: HashMap::new(),
                frames: HashMap::new(),
                resources: HashMap::new(),
                touches: HashMap::new(),
                runs: HashMap::new(),
            }
        }

        fn issue(&mut self) -> u64 {
            let handle = self.next_handle;
            self.next_handle = self.next_handle.wrapping_add(1).max(1);
            handle
        }

        fn error(kind: WitErrorKind, message: impl Into<String>) -> WitError {
            WitError {
                kind,
                message: message.into(),
            }
        }

        fn extension_error(error: ExtensionError) -> WitError {
            match error {
                ExtensionError::Permission(error) => {
                    Self::error(WitErrorKind::Denied, error.to_string())
                }
                other => Self::error(WitErrorKind::Failed, other.to_string()),
            }
        }

        fn capability_error(error: crate::capabilities::CapabilityError) -> WitError {
            use crate::capabilities::CapabilityError;
            let kind = match error {
                CapabilityError::Unavailable(_) => WitErrorKind::Unavailable,
                CapabilityError::InvalidRequest(_) => WitErrorKind::InvalidRequest,
                CapabilityError::NotFound(_) => WitErrorKind::NotFound,
                CapabilityError::Cancelled => WitErrorKind::Cancelled,
                CapabilityError::Failed(_) => WitErrorKind::Failed,
            };
            Self::error(kind, error.to_string())
        }

        fn authorize(&self, permission: Permission) -> Result<(), WitError> {
            self.host
                .authorize(permission)
                .map_err(Self::extension_error)
        }

        fn require_device(&self, handle: u64) -> Result<DeviceHandle, WitError> {
            self.devices.get(&handle).cloned().ok_or_else(|| {
                Self::error(
                    WitErrorKind::InvalidRequest,
                    format!("未知 device-handle: {handle}"),
                )
            })
        }

        fn require_frame(&self, handle: u64) -> Result<FrameHandle, WitError> {
            self.frames.get(&handle).copied().ok_or_else(|| {
                Self::error(
                    WitErrorKind::InvalidRequest,
                    format!("未知 frame-handle: {handle}"),
                )
            })
        }

        fn require_resource(&self, handle: u64) -> Result<ResourceHandle, WitError> {
            self.resources.get(&handle).copied().ok_or_else(|| {
                Self::error(
                    WitErrorKind::InvalidRequest,
                    format!("未知 resource-handle: {handle}"),
                )
            })
        }

        fn require_touch(&self, handle: u64) -> Result<TouchHandle, WitError> {
            self.touches.get(&handle).copied().ok_or_else(|| {
                Self::error(
                    WitErrorKind::InvalidRequest,
                    format!("未知 touch-handle: {handle}"),
                )
            })
        }

        fn require_run(&self, handle: u64) -> Result<RunHandle, WitError> {
            self.runs.get(&handle).copied().ok_or_else(|| {
                Self::error(
                    WitErrorKind::InvalidRequest,
                    format!("未知 run-handle: {handle}"),
                )
            })
        }
    }

    impl wit::gamer::host::types::Host for HostState {}

    impl device::Host for HostState {
        fn resolve(&mut self, id: String) -> impl Future<Output = Result<u64, WitError>> + Send {
            async move {
                self.authorize(Permission::DeviceRead)?;
                let service = self.host.registry().device().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "device capability 未注册")
                })?;
                let handle = service
                    .resolve(&DeviceId::new(id.trim()))
                    .await
                    .map_err(Self::capability_error)?;
                let token = self.issue();
                self.devices.insert(token, handle);
                Ok(token)
            }
        }

        fn start_app(
            &mut self,
            device: u64,
            app: String,
        ) -> impl Future<Output = Result<(), WitError>> + Send {
            async move {
                self.authorize(Permission::DeviceApp)?;
                let device = self.require_device(device)?;
                let service = self.host.registry().device().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "device capability 未注册")
                })?;
                service
                    .start_app(&device, &AppId::new(app))
                    .await
                    .map_err(Self::capability_error)
            }
        }

        fn stop_app(
            &mut self,
            device: u64,
            app: String,
        ) -> impl Future<Output = Result<(), WitError>> + Send {
            async move {
                self.authorize(Permission::DeviceApp)?;
                let device = self.require_device(device)?;
                let service = self.host.registry().device().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "device capability 未注册")
                })?;
                service
                    .stop_app(&device, &AppId::new(app))
                    .await
                    .map_err(Self::capability_error)
            }
        }
    }

    impl vision::Host for HostState {
        fn match_template(
            &mut self,
            frame: u64,
            template: u64,
        ) -> impl Future<Output = Result<vision::MatchOutcome, WitError>> + Send {
            async move {
                self.authorize(Permission::VisionMatch)?;
                let frame = self.require_frame(frame)?;
                let template = self.require_resource(template)?;
                let service = self.host.registry().vision().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "vision capability 未注册")
                })?;
                let outcome = service
                    .match_template(frame, TemplateQuery::new(template, MatchOptions::default()))
                    .await
                    .map_err(Self::capability_error)?;
                Ok(match outcome {
                    CoreMatchOutcome::Found(found) => {
                        vision::MatchOutcome::Found(vision::MatchBox {
                            x: found.x,
                            y: found.y,
                            width: found.width,
                            height: found.height,
                            score: found.score,
                        })
                    }
                    CoreMatchOutcome::NotFound => vision::MatchOutcome::NotFound,
                })
            }
        }

        fn capture(&mut self, device: u64) -> impl Future<Output = Result<u64, WitError>> + Send {
            async move {
                self.authorize(Permission::VisionMatch)?;
                let device = self.require_device(device)?;
                let service = self.host.registry().frame().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "frame capability 未注册")
                })?;
                let frame = service
                    .capture(&device)
                    .await
                    .map_err(Self::capability_error)?;
                let token = self.issue();
                self.frames.insert(token, frame);
                Ok(token)
            }
        }

        fn sample_color(
            &mut self,
            frame: u64,
            point: vision::Point,
        ) -> impl Future<Output = Result<(u8, u8, u8), WitError>> + Send {
            async move {
                self.authorize(Permission::VisionColor)?;
                let frame = self.require_frame(frame)?;
                let service = self.host.registry().vision().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "vision capability 未注册")
                })?;
                let ColorSample { red, green, blue } = service
                    .sample_color(frame, FramePoint::new(point.x, point.y))
                    .await
                    .map_err(Self::capability_error)?;
                Ok((red, green, blue))
            }
        }
    }

    impl input::Host for HostState {
        fn tap(
            &mut self,
            device: u64,
            point: input::Point,
        ) -> impl Future<Output = Result<(), WitError>> + Send {
            async move {
                self.authorize(Permission::InputTap)?;
                let device = self.require_device(device)?;
                let service = self.host.registry().input().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "input capability 未注册")
                })?;
                service
                    .tap(&device, TouchPoint::new(point.x, point.y, 1.0))
                    .await
                    .map_err(Self::capability_error)
            }
        }

        fn swipe(
            &mut self,
            device: u64,
            start: input::Point,
            end: input::Point,
            duration_ms: u64,
        ) -> impl Future<Output = Result<(), WitError>> + Send {
            async move {
                self.authorize(Permission::InputSwipe)?;
                let device = self.require_device(device)?;
                let service = self.host.registry().input().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "input capability 未注册")
                })?;
                service
                    .swipe(
                        &device,
                        SwipeGesture::new(
                            TouchPoint::new(start.x, start.y, 1.0),
                            TouchPoint::new(end.x, end.y, 1.0),
                            Duration::from_millis(duration_ms.min(60_000)),
                        ),
                    )
                    .await
                    .map_err(Self::capability_error)
            }
        }

        fn key(
            &mut self,
            device: u64,
            code: u32,
            action: String,
        ) -> impl Future<Output = Result<(), WitError>> + Send {
            async move {
                self.authorize(Permission::InputKey)?;
                let action = match action.trim().to_ascii_lowercase().as_str() {
                    "down" => KeyAction::Down,
                    "up" => KeyAction::Up,
                    "press" => KeyAction::Press,
                    other => {
                        return Err(Self::error(
                            WitErrorKind::InvalidRequest,
                            format!("未知 key action: {other}"),
                        ));
                    }
                };
                let device = self.require_device(device)?;
                let service = self.host.registry().input().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "input capability 未注册")
                })?;
                service
                    .key(&device, KeyInput::new(KeyCode::new(code), action))
                    .await
                    .map_err(Self::capability_error)
            }
        }

        fn text(
            &mut self,
            device: u64,
            value: String,
        ) -> impl Future<Output = Result<(), WitError>> + Send {
            async move {
                self.authorize(Permission::InputText)?;
                let device = self.require_device(device)?;
                let service = self.host.registry().input().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "input capability 未注册")
                })?;
                service
                    .text(&device, TextInput::new(value))
                    .await
                    .map_err(Self::capability_error)
            }
        }
    }

    impl touch::Host for HostState {
        fn begin(
            &mut self,
            device: u64,
            point: touch::Point,
        ) -> impl Future<Output = Result<u64, WitError>> + Send {
            async move {
                self.authorize(Permission::Touch)?;
                let device = self.require_device(device)?;
                let service = self.host.registry().touch().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "touch capability 未注册")
                })?;
                let touch = service
                    .begin(&device, TouchPoint::new(point.x, point.y, point.pressure))
                    .await
                    .map_err(Self::capability_error)?;
                let token = self.issue();
                self.touches.insert(token, touch);
                Ok(token)
            }
        }

        fn move_(
            &mut self,
            touch: u64,
            point: touch::Point,
        ) -> impl Future<Output = Result<(), WitError>> + Send {
            async move {
                self.authorize(Permission::Touch)?;
                let touch = self.require_touch(touch)?;
                let service = self.host.registry().touch().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "touch capability 未注册")
                })?;
                service
                    .move_touch(&touch, TouchPoint::new(point.x, point.y, point.pressure))
                    .await
                    .map_err(Self::capability_error)
            }
        }

        fn end(&mut self, touch: u64) -> impl Future<Output = Result<(), WitError>> + Send {
            async move {
                self.authorize(Permission::Touch)?;
                let touch_handle = self.require_touch(touch)?;
                let service = self.host.registry().touch().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "touch capability 未注册")
                })?;
                service
                    .end(&touch_handle)
                    .await
                    .map_err(Self::capability_error)?;
                self.touches.remove(&touch);
                Ok(())
            }
        }
    }

    impl resources::Host for HostState {
        fn resolve(
            &mut self,
            namespace: String,
            name: String,
        ) -> impl Future<Output = Result<u64, WitError>> + Send {
            async move {
                self.authorize(Permission::ResourceRead)?;
                let service = self.host.registry().resource().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "resource capability 未注册")
                })?;
                let resource = service
                    .resolve(&ResourceId::new(namespace, name))
                    .await
                    .map_err(Self::capability_error)?;
                let token = self.issue();
                self.resources.insert(token, resource);
                Ok(token)
            }
        }

        fn open(&mut self, handle: u64) -> impl Future<Output = Result<u64, WitError>> + Send {
            async move {
                self.authorize(Permission::ResourceRead)?;
                let handle = self.require_resource(handle)?;
                let service = self.host.registry().resource().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "resource capability 未注册")
                })?;
                let lease = service.open(handle).await.map_err(Self::capability_error)?;
                Ok(lease.byte_len().unwrap_or(0))
            }
        }
    }

    impl run::Host for HostState {
        fn submit(
            &mut self,
            device: u64,
            entry: u64,
        ) -> impl Future<Output = Result<u64, WitError>> + Send {
            async move {
                self.authorize(Permission::RunSubmit)?;
                let device = self.require_device(device)?;
                let entry = self.require_resource(entry)?;
                let service = self.host.registry().run().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "run capability 未注册")
                })?;
                let run = service
                    .submit(RunRequest::new(device, entry))
                    .await
                    .map_err(Self::capability_error)?;
                let token = self.issue();
                self.runs.insert(token, run);
                Ok(token)
            }
        }

        fn cancel(&mut self, run: u64) -> impl Future<Output = Result<(), WitError>> + Send {
            async move {
                self.authorize(Permission::RunControl)?;
                let run_handle = self.require_run(run)?;
                let service = self.host.registry().run().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "run capability 未注册")
                })?;
                service
                    .cancel(run_handle)
                    .await
                    .map_err(Self::capability_error)
            }
        }

        fn status(&mut self, run: u64) -> impl Future<Output = Result<String, WitError>> + Send {
            async move {
                self.authorize(Permission::RunControl)?;
                let run_handle = self.require_run(run)?;
                let service = self.host.registry().run().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "run capability 未注册")
                })?;
                let status = service
                    .status(run_handle)
                    .await
                    .map_err(Self::capability_error)?;
                Ok(match status {
                    RunStatus::Queued => "queued",
                    RunStatus::Running => "running",
                    RunStatus::Succeeded => "succeeded",
                    RunStatus::Failed => "failed",
                    RunStatus::Cancelled => "cancelled",
                }
                .to_string())
            }
        }
    }

    impl runtime::Host for HostState {
        fn sleep(
            &mut self,
            milliseconds: u64,
        ) -> impl Future<Output = Result<(), WitError>> + Send {
            let runtime = self.runtime.clone();
            async move {
                self.authorize(Permission::RuntimeSleep)?;
                if self.cancelled.load(Ordering::Relaxed) || runtime.cancelled() {
                    return Err(Self::error(WitErrorKind::Cancelled, "插件运行已取消"));
                }
                runtime
                    .sleep(Duration::from_millis(milliseconds.min(3_600_000)))
                    .await
                    .map_err(Self::capability_error)?;
                if self.cancelled.load(Ordering::Relaxed) || runtime.cancelled() {
                    Err(Self::error(WitErrorKind::Cancelled, "插件运行已取消"))
                } else {
                    Ok(())
                }
            }
        }

        fn cancelled(&mut self) -> impl Future<Output = bool> + Send {
            let cancelled = self.cancelled.clone();
            async move { cancelled.load(Ordering::Relaxed) }
        }
    }

    impl log::Host for HostState {
        fn write(
            &mut self,
            level: String,
            message: String,
            device: Option<u64>,
            run: Option<u64>,
        ) -> impl Future<Output = Result<(), WitError>> + Send {
            async move {
                self.authorize(Permission::LogWrite)?;
                let level = match level.trim().to_ascii_lowercase().as_str() {
                    "trace" => LogLevel::Trace,
                    "debug" => LogLevel::Debug,
                    "info" => LogLevel::Info,
                    "warn" | "warning" => LogLevel::Warn,
                    "error" => LogLevel::Error,
                    other => {
                        return Err(Self::error(
                            WitErrorKind::InvalidRequest,
                            format!("未知 log level: {other}"),
                        ));
                    }
                };
                let mut record = LogRecord::new(level, message);
                if let Some(device) = device {
                    record = record.with_device(self.require_device(device)?);
                }
                if let Some(run) = run {
                    record = record.with_run(self.require_run(run)?);
                }
                let service = self.host.registry().log().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "log capability 未注册")
                })?;
                service.write(record).map_err(Self::capability_error)
            }
        }
    }

    /// The YAML world has a separate state type. This keeps its resolver and
    /// source-oriented call behavior out of the generic extension HostState.
    struct YamlHostState {
        host: HostApi,
        cancelled: Arc<AtomicBool>,
        app_context: Option<crate::core::AppContext>,
        yaml_programs: Option<Arc<dyn crate::yaml_extension::YamlProgramResolver>>,
    }

    impl YamlHostState {
        fn new(
            host: HostApi,
            cancelled: Arc<AtomicBool>,
            app_context: crate::core::AppContext,
            yaml_programs: Option<Arc<dyn crate::yaml_extension::YamlProgramResolver>>,
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
    impl crate::extensions::wit::yaml::gamer::host::types::Host for YamlHostState {}

    impl crate::extensions::wit::yaml::gamer::host::capability::Host for YamlHostState {
        fn invoke(
            &mut self,
            capability: String,
            args_json: String,
        ) -> Result<String, crate::extensions::wit::yaml::gamer::host::types::HostError> {
            let host = self.host.clone();
            let context = self.app_context.clone();
            let cancelled = self.cancelled.clone();
            let result = block_on_yaml(async move {
                let context =
                    context.ok_or_else(|| anyhow::anyhow!("capability.invoke 需要 AppContext"))?;
                let value = crate::yaml_extension::NativeYamlHost::invoke_json(
                    host,
                    context,
                    cancelled,
                    &capability,
                    &args_json,
                )
                .await?;
                Ok::<_, anyhow::Error>(serde_json::to_string(&value)?)
            });
            result.map_err(|error| yaml_capability_error(&error))
        }
    }

    impl crate::extensions::wit::yaml::gamer::host::programs::Host for YamlHostState {
        fn resolve(&mut self, target: String, args_json: String) -> Result<String, String> {
            let resolver = self
                .yaml_programs
                .clone()
                .ok_or_else(|| "YAML call resolver 未配置".to_string())?;
            let args = serde_json::from_str::<serde_json::Value>(&args_json)
                .map_err(|error| format!("call 参数不是 JSON: {error}"))?;
            let args = crate::yaml_vnext::Value::from_json(args)
                .map_err(|error| format!("call 参数无效: {error}"))?;
            let crate::yaml_vnext::Value::Map(args) = args else {
                return Err("call 参数必须是 map".to_string());
            };
            let program = resolver
                .resolve(&target, &args)
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
        kind: crate::extensions::wit::yaml::gamer::host::types::HostErrorKind,
        message: impl Into<String>,
    ) -> crate::extensions::wit::yaml::gamer::host::types::HostError {
        crate::extensions::wit::yaml::gamer::host::types::HostError {
            kind,
            message: message.into(),
        }
    }

    fn yaml_capability_error(
        error: &anyhow::Error,
    ) -> crate::extensions::wit::yaml::gamer::host::types::HostError {
        use crate::capabilities::CapabilityError;
        use crate::extensions::error::ExtensionError;
        use crate::extensions::wit::yaml::gamer::host::types::HostErrorKind;

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

    impl context::Host for HostState {
        fn get(&mut self) -> impl Future<Output = context::AppContext> + Send {
            let app_context = self.app_context.clone();
            async move {
                context::AppContext {
                    device_id: app_context
                        .as_ref()
                        .map(|context| context.device_id.as_str().to_string()),
                    android_package: app_context
                        .as_ref()
                        .map(|context| context.android_package.as_str().to_string()),
                    content_package: app_context.as_ref().and_then(|context| {
                        context
                            .content_package
                            .as_ref()
                            .map(|package| package.as_str().to_string())
                    }),
                }
            }
        }
    }

    #[derive(Debug)]
    struct RunningInstance {
        task: tokio::task::JoinHandle<()>,
        cancelled: Arc<AtomicBool>,
        completed: Arc<AtomicBool>,
    }

    #[derive(Debug)]
    pub(crate) struct LazyWasmtimeRuntime {
        engine: OnceLock<Engine>,
        components: Mutex<HashMap<[u8; 32], Arc<Component>>>,
        instances: Arc<Mutex<HashMap<WasmInstanceHandle, RunningInstance>>>,
    }

    impl LazyWasmtimeRuntime {
        pub(crate) fn new() -> Self {
            Self {
                engine: OnceLock::new(),
                components: Mutex::new(HashMap::new()),
                instances: Arc::new(Mutex::new(HashMap::new())),
            }
        }

        pub(crate) fn is_initialized(&self) -> bool {
            self.engine.get().is_some()
        }

        /// Returns whether the guest entrypoint has returned (successfully or
        /// with a trap). This is primarily useful for lifecycle diagnostics;
        /// a running extension may deliberately keep the entrypoint pending.
        pub(crate) async fn entry_completed(&self, instance: WasmInstanceHandle) -> bool {
            self.instances
                .lock()
                .await
                .get(&instance)
                .is_some_and(|running| running.completed.load(Ordering::Acquire))
        }

        fn engine(&self) -> &Engine {
            self.engine.get_or_init(|| {
                let config = wasmtime::Config::new();
                Engine::new(&config).expect("Wasmtime engine config is valid")
            })
        }
    }

    #[async_trait]
    impl WasmRuntime for LazyWasmtimeRuntime {
        async fn start(&self, request: WasmStartRequest) -> ExtensionResult<WasmInstanceHandle> {
            let mut digest = [0u8; 32];
            digest.copy_from_slice(Sha256::digest(&request.wasm).as_slice());
            let component = {
                let mut components = self.components.lock().await;
                if let Some(component) = components.get(&digest).cloned() {
                    component
                } else {
                    let component =
                        Arc::new(Component::new(self.engine(), &request.wasm).map_err(
                            |error| ExtensionError::Runtime(format!("组件编译失败: {error}")),
                        )?);
                    components.insert(digest, component.clone());
                    component
                }
            };
            let mut linker = Linker::new(self.engine());
            Bindings::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state).map_err(
                |error| ExtensionError::Runtime(format!("WIT linker 初始化失败: {error}")),
            )?;

            let cancelled = Arc::new(AtomicBool::new(false));
            let completed = Arc::new(AtomicBool::new(false));
            let state = HostState::new(request.host, cancelled.clone(), request.app_context);
            let engine = self.engine().clone();
            let completed_for_task = completed.clone();
            let (ready_tx, ready_rx) = oneshot::channel();
            let task = tokio::spawn(async move {
                let mut store = Store::new(&engine, state);
                let instance =
                    match Bindings::instantiate_async(&mut store, &component, &linker).await {
                        Ok(instance) => {
                            let _ = ready_tx.send(Ok(()));
                            instance
                        }
                        Err(error) => {
                            let _ = ready_tx.send(Err(error.to_string()));
                            return;
                        }
                    };
                match instance.gamer_host_extension().call_run(&mut store) {
                    Ok(()) => tracing::debug!("WASM extension entrypoint returned"),
                    Err(error) => {
                        tracing::error!(error = %error, "WASM extension entrypoint trapped")
                    }
                }
                completed_for_task.store(true, Ordering::Release);
            });

            match ready_rx.await {
                Ok(Ok(())) => {
                    let handle = WasmInstanceHandle::new();
                    self.instances.lock().await.insert(
                        handle,
                        RunningInstance {
                            task,
                            cancelled,
                            completed,
                        },
                    );
                    tracing::info!(extension = %request.id, version = %request.version, "WASM extension component started");
                    Ok(handle)
                }
                Ok(Err(error)) => {
                    task.abort();
                    Err(ExtensionError::Runtime(format!("组件实例化失败: {error}")))
                }
                Err(_) => {
                    task.abort();
                    Err(ExtensionError::Runtime("组件启动任务意外退出".to_string()))
                }
            }
        }

        async fn stop(&self, instance: WasmInstanceHandle) -> ExtensionResult<()> {
            let running = self
                .instances
                .lock()
                .await
                .remove(&instance)
                .ok_or(ExtensionError::RuntimeUnavailable("WASM 实例不存在"))?;

            // Once the entrypoint returned, let the task finish its Store and
            // Component cleanup normally. Aborting a task while Wasmtime is
            // unwinding an async component fiber is not safe on all Tokio
            // schedulers (notably the Windows GNU target).
            if running.completed.load(Ordering::Acquire) {
                let _ = running.task.await;
                return Ok(());
            }

            running.cancelled.store(true, Ordering::Relaxed);
            running.task.abort();
            let _ = running.task.await;
            Ok(())
        }

        fn is_available(&self) -> bool {
            true
        }
    }

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

    #[async_trait]
    impl crate::yaml_extension::YamlWasmRuntime for LazyYamlWasmtimeRuntime {
        async fn run(
            &self,
            request: crate::yaml_extension::YamlWasmRunRequest,
        ) -> Result<crate::yaml_extension::YamlWasmRunResult, anyhow::Error> {
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
            crate::extensions::wit::yaml::YamlExtensionHost::add_to_linker::<_, HasSelf<_>>(
                &mut linker,
                |state| state,
            )
            .map_err(|error| anyhow::anyhow!("YAML WIT linker 初始化失败: {error}"))?;
            let state = YamlHostState::new(
                request.host,
                request.stop,
                request.context,
                request.resolver,
            );
            let mut store = Store::new(self.engine(), state);
            let instance = crate::extensions::wit::yaml::YamlExtensionHost::instantiate(
                &mut store, &component, &linker,
            )
            .map_err(|error| anyhow::anyhow!("YAML 组件实例化失败: {error}"))?;
            let mut program = serde_json::to_value(&request.program)?;
            if let serde_json::Value::Object(ref mut program) = program {
                program.insert("args".to_string(), serde_json::to_value(request.args)?);
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
            let value = crate::yaml_vnext::Value::from_json(result)
                .map_err(|error| anyhow::anyhow!("YAML guest 返回值无效: {error}"))?;
            Ok(crate::yaml_extension::YamlWasmRunResult { value })
        }

        fn is_available(&self) -> bool {
            true
        }
    }
}

#[cfg(feature = "wasm-runtime")]
pub(crate) use wasmtime_runtime::LazyWasmtimeRuntime;
#[cfg(feature = "wasm-runtime")]
pub(crate) use wasmtime_runtime::LazyYamlWasmtimeRuntime;

#[cfg(all(test, feature = "wasm-runtime"))]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    use super::wasmtime_runtime::LazyWasmtimeRuntime;
    use super::{WasmRuntime, WasmStartRequest};
    use crate::capabilities::CapabilityRegistry;
    use crate::extensions::{
        parse_manifest, ExtensionId, ExtensionVersion, HostApi, HostApiCatalog,
    };

    const ENTRY_COMPONENT: &str = r#"(component
      (core module $m
        (func (export "run"))
      )
      (core instance $i (instantiate $m))
      (alias core export $i "run" (core func $run))
      (type $run-type (func))
      (func $run (type $run-type) (canon lift (core func $run)))
      (instance $ext (export "run" (func $run)))
      (export "gamer:host/extension@1.0.0" (instance $ext))
    )"#;

    fn test_host() -> HostApi {
        let manifest = parse_manifest(
            br#"manifest_version = 1
id = "com.example.entry"
version = "1.0.0"
name = "Entry test"
entry = "plugin.wasm"
"#,
        )
        .unwrap();
        HostApi::for_manifest(
            CapabilityRegistry::default(),
            HostApiCatalog::default(),
            &manifest,
        )
        .unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn component_is_instantiated_and_exported_entrypoint_is_called() {
        let runtime = LazyWasmtimeRuntime::new();
        let handle = runtime
            .start(WasmStartRequest {
                id: ExtensionId::parse("com.example.entry").unwrap(),
                version: ExtensionVersion::parse("1.0.0").unwrap(),
                wasm: wat::parse_str(ENTRY_COMPONENT).unwrap(),
                host: test_host(),
                app_context: None,
            })
            .await
            .unwrap();

        while !runtime.entry_completed(handle).await {
            tokio::task::yield_now().await;
        }
        runtime.stop(handle).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn generated_host_binding_denies_unlisted_capability() {
        use super::wasmtime_runtime::HostState;
        use crate::extensions::wit::gamer::host::device::Host as _;

        let mut state = HostState::new(test_host(), Arc::new(AtomicBool::new(false)), None);
        let error = state.resolve("device-1".to_string()).await.unwrap_err();
        assert_eq!(
            error.kind,
            crate::extensions::wit::gamer::host::types::HostErrorKind::Denied
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn generated_host_binding_exposes_app_context_without_native_objects() {
        use super::wasmtime_runtime::HostState;
        use crate::extensions::wit::gamer::host::context::Host as _;

        let app_context =
            crate::core::AppContext::from_legacy_package("device-1", "com.example.game").unwrap();
        let mut state = HostState::new(
            test_host(),
            Arc::new(AtomicBool::new(false)),
            Some(app_context),
        );
        let context = state.get().await;
        assert_eq!(context.device_id.as_deref(), Some("device-1"));
        assert_eq!(context.android_package.as_deref(), Some("com.example.game"));
        assert_eq!(context.content_package.as_deref(), Some("com.example.game"));
    }
}
