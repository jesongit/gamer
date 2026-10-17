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

    /// Declarative UI backend entry (`plugin.call`). The default runtime
    /// rejects calls; only runtimes that keep a live instance can dispatch.
    async fn call(
        &self,
        instance: WasmInstanceHandle,
        action: &str,
        values_json: &str,
    ) -> ExtensionResult<String> {
        let _ = (instance, action, values_json);
        Err(ExtensionError::RuntimeUnavailable(
            "当前运行时不支持插件调用",
        ))
    }

    fn is_available(&self) -> bool;
    /// Scoped external calls use a fresh guest instance and bounded execution.
    /// Other runtimes must opt in after implementing those guarantees.
    fn supports_scoped_external(&self) -> bool {
        false
    }
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
    use tokio::sync::{mpsc, oneshot, Mutex};
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
        /// 调用方扩展 id：插件资源隔离的 plugin 维度来源（资源解析恒被限制
        /// 在 `packages/<pkg>/plugins/<本扩展 id>/` 前缀内）。
        extension_id: ExtensionId,
        cancelled: Arc<AtomicBool>,
        runtime: Arc<dyn RuntimeService>,
        app_context: Option<crate::core::AppContext>,
        io_runtime: Option<tokio::runtime::Handle>,
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
            extension_id: ExtensionId,
            cancelled: Arc<AtomicBool>,
            app_context: Option<crate::core::AppContext>,
        ) -> Self {
            let runtime: Arc<dyn RuntimeService> = Arc::new(
                crate::capabilities::adapters::RuntimeAdapter::new(cancelled.clone()),
            );
            Self {
                host,
                extension_id,
                cancelled,
                runtime,
                app_context,
                io_runtime: tokio::runtime::Handle::try_current().ok(),
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
            self.host.check_external().map_err(Self::extension_error)?;
            let device = self.devices.get(&handle).cloned().ok_or_else(|| {
                Self::error(
                    WitErrorKind::InvalidRequest,
                    format!("未知 device-handle: {handle}"),
                )
            })?;
            self.host
                .authorize_device_id(device.id().as_str())
                .map_err(Self::extension_error)?;
            Ok(device)
        }

        fn require_frame(&self, handle: u64) -> Result<FrameHandle, WitError> {
            self.host.check_external().map_err(Self::extension_error)?;
            self.frames.get(&handle).copied().ok_or_else(|| {
                Self::error(
                    WitErrorKind::InvalidRequest,
                    format!("未知 frame-handle: {handle}"),
                )
            })
        }

        fn require_resource(&self, handle: u64) -> Result<ResourceHandle, WitError> {
            self.host.check_external().map_err(Self::extension_error)?;
            self.resources.get(&handle).copied().ok_or_else(|| {
                Self::error(
                    WitErrorKind::InvalidRequest,
                    format!("未知 resource-handle: {handle}"),
                )
            })
        }

        fn require_touch(&self, handle: u64) -> Result<TouchHandle, WitError> {
            self.host.check_external().map_err(Self::extension_error)?;
            self.touches.get(&handle).copied().ok_or_else(|| {
                Self::error(
                    WitErrorKind::InvalidRequest,
                    format!("未知 touch-handle: {handle}"),
                )
            })
        }

        fn require_run(&self, handle: u64) -> Result<RunHandle, WitError> {
            self.host.check_external().map_err(Self::extension_error)?;
            self.runs.get(&handle).copied().ok_or_else(|| {
                Self::error(
                    WitErrorKind::InvalidRequest,
                    format!("未知 run-handle: {handle}"),
                )
            })
        }
    }

    impl wit::gamer::host::types::Host for HostState {}

    /// Called only from the instance's blocking OS thread. Capability I/O
    /// remains on the server runtime; no Wasmtime fiber or per-call runtime
    /// is entered or torn down around guest execution.
    fn block_on_host<T>(
        runtime: Option<tokio::runtime::Handle>,
        future: impl Future<Output = Result<T, WitError>>,
    ) -> Result<T, WitError> {
        let runtime = runtime.ok_or_else(|| {
            HostState::error(WitErrorKind::Unavailable, "Host I/O runtime is unavailable")
        })?;
        runtime.block_on(future)
    }

    impl device::Host for HostState {
        fn resolve(&mut self, id: String) -> Result<u64, WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
                self.authorize(Permission::DeviceRead)?;
                self.host
                    .authorize_device_id(id.trim())
                    .map_err(Self::extension_error)?;
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
            })
        }

        fn start_app(&mut self, device: u64, app: String) -> Result<(), WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
                self.authorize(Permission::DeviceApp)?;
                let device = self.require_device(device)?;
                let service = self.host.registry().device().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "device capability 未注册")
                })?;
                service
                    .start_app(&device, &AppId::new(app))
                    .await
                    .map_err(Self::capability_error)
            })
        }

        fn stop_app(&mut self, device: u64, app: String) -> Result<(), WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
                self.authorize(Permission::DeviceApp)?;
                let device = self.require_device(device)?;
                let service = self.host.registry().device().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "device capability 未注册")
                })?;
                service
                    .stop_app(&device, &AppId::new(app))
                    .await
                    .map_err(Self::capability_error)
            })
        }
    }

    impl vision::Host for HostState {
        fn match_template(
            &mut self,
            frame: u64,
            template: u64,
        ) -> Result<vision::MatchOutcome, WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
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
            })
        }

        fn capture(&mut self, device: u64) -> Result<u64, WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
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
            })
        }

        fn sample_color(
            &mut self,
            frame: u64,
            point: vision::Point,
        ) -> Result<(u8, u8, u8), WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
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
            })
        }
    }

    impl input::Host for HostState {
        fn tap(&mut self, device: u64, point: input::Point) -> Result<(), WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
                self.authorize(Permission::InputTap)?;
                let device = self.require_device(device)?;
                let service = self.host.registry().input().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "input capability 未注册")
                })?;
                service
                    .tap(&device, TouchPoint::new(point.x, point.y, 1.0))
                    .await
                    .map_err(Self::capability_error)
            })
        }

        fn swipe(
            &mut self,
            device: u64,
            start: input::Point,
            end: input::Point,
            duration_ms: u64,
        ) -> Result<(), WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
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
            })
        }

        fn key(&mut self, device: u64, code: u32, action: String) -> Result<(), WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
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
            })
        }

        fn text(&mut self, device: u64, value: String) -> Result<(), WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
                self.authorize(Permission::InputText)?;
                let device = self.require_device(device)?;
                let service = self.host.registry().input().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "input capability 未注册")
                })?;
                service
                    .text(&device, TextInput::new(value))
                    .await
                    .map_err(Self::capability_error)
            })
        }
    }

    impl touch::Host for HostState {
        fn begin(&mut self, device: u64, point: touch::Point) -> Result<u64, WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
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
            })
        }

        fn move_(&mut self, touch: u64, point: touch::Point) -> Result<(), WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
                self.authorize(Permission::Touch)?;
                let touch = self.require_touch(touch)?;
                let service = self.host.registry().touch().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "touch capability 未注册")
                })?;
                service
                    .move_touch(&touch, TouchPoint::new(point.x, point.y, point.pressure))
                    .await
                    .map_err(Self::capability_error)
            })
        }

        fn end(&mut self, touch: u64) -> Result<(), WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
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
            })
        }
    }

    impl resources::Host for HostState {
        fn resolve(&mut self, namespace: String, name: String) -> Result<u64, WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
                self.authorize(Permission::ResourceRead)?;
                self.host
                    .authorize_package_id(&namespace)
                    .map_err(Self::extension_error)?;
                let service = self.host.registry().resource().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "resource capability 未注册")
                })?;
                // 插件数据隔离（plan §14）：plugin 维度由宿主注入调用方自身扩展 id，
                // guest 无法寻址其他插件的 `plugins/<plugin>/` 前缀。
                let resource = service
                    .resolve(
                        &ResourceId::new(namespace, self.extension_id.as_str(), name).map_err(
                            |e| {
                                Self::error(
                                    wit::gamer::host::types::HostErrorKind::InvalidRequest,
                                    e.to_string(),
                                )
                            },
                        )?,
                    )
                    .await
                    .map_err(Self::capability_error)?;
                let token = self.issue();
                self.resources.insert(token, resource);
                Ok(token)
            })
        }

        fn open(&mut self, handle: u64) -> Result<u64, WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
                self.authorize(Permission::ResourceRead)?;
                let handle = self.require_resource(handle)?;
                let service = self.host.registry().resource().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "resource capability 未注册")
                })?;
                let lease = service.open(handle).await.map_err(Self::capability_error)?;
                Ok(lease.byte_len().unwrap_or(0))
            })
        }
    }

    impl run::Host for HostState {
        fn submit(&mut self, device: u64, entry: u64) -> Result<u64, WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
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
            })
        }

        fn cancel(&mut self, run: u64) -> Result<(), WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
                self.authorize(Permission::RunControl)?;
                let run_handle = self.require_run(run)?;
                let service = self.host.registry().run().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "run capability 未注册")
                })?;
                service
                    .cancel(run_handle)
                    .await
                    .map_err(Self::capability_error)
            })
        }

        fn status(&mut self, run: u64) -> Result<String, WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
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
            })
        }
    }

    impl runtime::Host for HostState {
        fn sleep(&mut self, milliseconds: u64) -> Result<(), WitError> {
            let executor = self.io_runtime.clone();
            block_on_host(executor, async {
                let runtime = self.runtime.clone();
                self.authorize(Permission::RuntimeSleep)?;
                if self.cancelled.load(Ordering::Relaxed) || runtime.cancelled() {
                    return Err(Self::error(WitErrorKind::Cancelled, "插件运行已取消"));
                }
                let sleep = runtime.sleep(Duration::from_millis(milliseconds.min(3_600_000)));
                let host = self.host.clone();
                let cancelled = self.cancelled.clone();
                tokio::select! {
                    result=sleep=>result.map_err(Self::capability_error)?,
                    _=async move {while !cancelled.load(Ordering::Acquire) && host.check_external().is_ok(){tokio::time::sleep(Duration::from_millis(50)).await;}}=>return Err(Self::error(WitErrorKind::Cancelled,"Guest call stopped, expired or was revoked")),
                }
                if self.cancelled.load(Ordering::Relaxed) || runtime.cancelled() {
                    Err(Self::error(WitErrorKind::Cancelled, "插件运行已取消"))
                } else {
                    Ok(())
                }
            })
        }

        fn cancelled(&mut self) -> bool {
            let cancelled = self.cancelled.clone();
            cancelled.load(Ordering::Relaxed) || self.host.check_external().is_err()
        }
    }

    impl log::Host for HostState {
        fn write(
            &mut self,
            level: String,
            message: String,
            device: Option<u64>,
            run: Option<u64>,
        ) -> Result<(), WitError> {
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
            let service =
                self.host.registry().log().ok_or_else(|| {
                    Self::error(WitErrorKind::Unavailable, "log capability 未注册")
                })?;
            service.write(record).map_err(Self::capability_error)
        }
    }

    impl context::Host for HostState {
        fn get(&mut self) -> context::AppContext {
            if let Some(external) = self.host.external_context() {
                return context::AppContext {
                    device_id: external.device_id.clone(),
                    android_package: None,
                    content_package: external.package_id.clone(),
                };
            }
            let app_context = self.app_context.clone();
            context::AppContext {
                device_id: app_context
                    .as_ref()
                    .map(|context| context.device_id.as_str().to_string()),
                android_package: app_context.as_ref().and_then(|context| {
                    context
                        .android_package
                        .as_ref()
                        .map(|p| p.as_str().to_string())
                }),
                content_package: app_context.as_ref().and_then(|context| {
                    context
                        .content_package
                        .as_ref()
                        .map(|package| package.as_str().to_string())
                }),
            }
        }
    }

    #[derive(Debug)]
    struct RunningInstance {
        commands: mpsc::Sender<RuntimeCommand>,
        /// The synchronous Component, Store and host handles remain on this
        /// thread until actual cleanup finishes. Its result is never ignored.
        task: std::thread::JoinHandle<ExtensionResult<()>>,
        cancelled: Arc<AtomicBool>,
        completed: Arc<AtomicBool>,
    }

    /// Commands served by the instance task after its `run` entrypoint has
    /// returned. A declarative plugin's entrypoint typically returns
    /// immediately, so the task parks here waiting for `plugin.call`s.
    enum RuntimeCommand {
        Call {
            action: String,
            values_json: String,
            reply: oneshot::Sender<ExtensionResult<String>>,
        },
        Stop(oneshot::Sender<()>),
    }

    fn join_instance_thread(
        task: std::thread::JoinHandle<ExtensionResult<()>>,
    ) -> ExtensionResult<()> {
        task.join().map_err(|_| {
            ExtensionError::Runtime(
                "WASM instance thread panicked during execution or cleanup".into(),
            )
        })?
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
                // The WIT bridge uses synchronous Component calls. Capability
                // futures are driven separately on the captured host runtime.
                let mut config = wasmtime::Config::new();
                config.consume_fuel(true);
                config.epoch_interruption(true);
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
            let state = HostState::new(
                request.host,
                request.id.clone(),
                cancelled.clone(),
                request.app_context,
            );
            let engine = self.engine().clone();
            let completed_for_task = completed.clone();
            let (commands_tx, mut commands_rx) = mpsc::channel::<RuntimeCommand>(16);
            let (ready_tx, ready_rx) = oneshot::channel();
            // The complete Store and guest lifetime belongs to one ordinary
            // blocking OS thread. Host I/O alone enters the captured Tokio
            // Handle; synchronous Component calls never create a fiber.
            let task = std::thread::Builder::new()
                .name(format!("wasm-ext-{}", request.id))
                .spawn(move || -> ExtensionResult<()> {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let host_runtime = state.io_runtime.clone();
                        let _runtime_context = host_runtime.as_ref().map(|runtime| runtime.enter());
                        let mut store = Store::new(&engine, state);
                        store.set_epoch_deadline(1);
                        store.epoch_deadline_callback(|store| {
                            if store.data().cancelled.load(Ordering::Acquire) {
                                Err(wasmtime::format_err!("WASM instance stopped"))
                            } else {
                                Ok(wasmtime::UpdateDeadline::Continue(1))
                            }
                        });
                        let fuel = if store.data().host.external_context().is_some() {
                            20_000_000
                        } else {
                            u64::MAX
                        };
                        if let Err(error) = store.set_fuel(fuel) {
                            let _ = ready_tx.send(Err(error.to_string()));
                            return Err(ExtensionError::Runtime(error.to_string()));
                        }
                        let instance = match Bindings::instantiate(&mut store, &component, &linker)
                        {
                            Ok(instance) => {
                                let _ = ready_tx.send(Ok(()));
                                instance
                            }
                            Err(error) => {
                                let _ = ready_tx.send(Err(error.to_string()));
                                return Err(ExtensionError::Runtime(format!(
                                    "组件实例化失败: {error}"
                                )));
                            }
                        };
                        let entry = instance.gamer_host_extension().call_run(&mut store);
                        completed_for_task.store(true, Ordering::Release);
                        if let Err(error) = entry {
                            if store.data().cancelled.load(Ordering::Acquire) {
                                return Ok(());
                            }
                            return Err(ExtensionError::Runtime(format!(
                                "插件 entry trap: {error}"
                            )));
                        }
                        while let Some(command) = commands_rx.blocking_recv() {
                            match command {
                                RuntimeCommand::Call {
                                    action,
                                    values_json,
                                    reply,
                                } => {
                                    let result = instance
                                        .gamer_host_extension()
                                        .call_call(&mut store, &action, &values_json)
                                        .map_err(|error| {
                                            ExtensionError::Runtime(format!(
                                                "插件 call trap: {error}"
                                            ))
                                        })
                                        .and_then(|result| {
                                            result.map_err(|error| {
                                                ExtensionError::Runtime(format!(
                                                    "插件 call 失败: {error}"
                                                ))
                                            })
                                        });
                                    let _ = reply.send(result);
                                }
                                RuntimeCommand::Stop(reply) => {
                                    let _ = reply.send(());
                                    break;
                                }
                            }
                        }
                        // stop joins this thread before reporting success, so
                        // Store/host handles have actually finished dropping.
                        drop(store);
                        Ok(())
                    }))
                    .unwrap_or_else(|_| {
                        Err(ExtensionError::Runtime(
                            "WASM instance panicked during execution or cleanup".into(),
                        ))
                    })
                })
                .map_err(|error| ExtensionError::Runtime(format!("插件线程启动失败: {error}")))?;

            match ready_rx.await {
                Ok(Ok(())) => {
                    let handle = WasmInstanceHandle::new();
                    self.instances.lock().await.insert(
                        handle,
                        RunningInstance {
                            commands: commands_tx,
                            task,
                            cancelled,
                            completed,
                        },
                    );
                    tracing::info!(extension = %request.id, version = %request.version, "WASM extension component started");
                    Ok(handle)
                }
                Ok(Err(error)) => {
                    join_instance_thread(task)?;
                    Err(ExtensionError::Runtime(format!("组件实例化失败: {error}")))
                }
                Err(_) => {
                    join_instance_thread(task)?;
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

            // Interrupt this store's CPU loop as well as cooperative imports.
            // Other stores observe the epoch but continue unless their own
            // cancellation flag is set. Never detach a still executing guest.
            running.cancelled.store(true, Ordering::Release);
            self.engine().increment_epoch();
            let (ack_tx, ack_rx) = oneshot::channel();
            if running
                .commands
                .send(RuntimeCommand::Stop(ack_tx))
                .await
                .is_ok()
            {
                let _ = ack_rx.await;
            }
            tokio::task::spawn_blocking(move || join_instance_thread(running.task))
                .await
                .map_err(|_| ExtensionError::Runtime("WASM cleanup join task failed".into()))?
        }

        async fn call(
            &self,
            instance: WasmInstanceHandle,
            action: &str,
            values_json: &str,
        ) -> ExtensionResult<String> {
            let commands = self
                .instances
                .lock()
                .await
                .get(&instance)
                .map(|running| running.commands.clone())
                .ok_or(ExtensionError::RuntimeUnavailable("WASM 实例不存在"))?;
            let (reply, wait) = oneshot::channel();
            commands
                .send(RuntimeCommand::Call {
                    action: action.to_string(),
                    values_json: values_json.to_string(),
                    reply,
                })
                .await
                .map_err(|_| ExtensionError::RuntimeUnavailable("WASM 实例已退出"))?;
            wait.await
                .map_err(|_| ExtensionError::RuntimeUnavailable("WASM 实例已退出"))?
        }

        fn is_available(&self) -> bool {
            true
        }
        fn supports_scoped_external(&self) -> bool {
            true
        }
    }

    #[cfg(test)]
    mod external_grant_tests {
        use super::*;
        use crate::extensions::{parse_manifest, HostApiCatalog, PermissionSet};
        use crate::mcp::{
            AllowedObjects, CreateTokenRequest, CredentialStore, InvocationContext, ReceiptStore,
            ToolEffect,
        };

        #[test]
        fn actual_worker_cleanup_errors_are_never_reported_as_success() {
            let task = std::thread::spawn(|| {
                Err(ExtensionError::Runtime("synthetic cleanup failure".into()))
            });
            assert!(join_instance_thread(task).is_err());
        }

        #[test]
        fn fresh_handle_tables_and_existing_handles_recheck_active_grants() {
            let directory = tempfile::tempdir().unwrap();
            let credentials = Arc::new(CredentialStore::open(directory.path()).unwrap());
            let receipts = Arc::new(ReceiptStore::open(directory.path()).unwrap());
            let token = credentials
                .create(CreateTokenRequest {
                    label: "fixture".into(),
                    scopes: ["fixture.read".into()].into_iter().collect(),
                    allowed: AllowedObjects {
                        devices: ["device-a".into(), "device-b".into()].into_iter().collect(),
                        packages: ["package-a".into(), "package-b".into()]
                            .into_iter()
                            .collect(),
                        ..Default::default()
                    },
                    expires_in_seconds: 60,
                })
                .unwrap();
            let mut context = InvocationContext::new(
                credentials.authenticate(&token.token).unwrap(),
                "fixture.read".into(),
                None,
                credentials.clone(),
                receipts,
            );
            context.device_id = Some("device-a".into());
            context.package_id = Some("package-a".into());
            let manifest = parse_manifest(b"manifest_version=2\nid='com.example.grant'\nname='Grant'\nversion='1.0.0'\nentry='plugin.wasm'\npermissions=['device.read','resource.read','resource.write','input.tap','run.submit']\n").unwrap();
            let host = HostApi::for_manifest(
                crate::capabilities::CapabilityRegistry::default(),
                HostApiCatalog::default(),
                &manifest,
            )
            .unwrap()
            .for_external(
                context,
                PermissionSet::parse([
                    "device.read",
                    "resource.read",
                    "resource.write",
                    "input.tap",
                    "run.submit",
                ])
                .unwrap(),
                ToolEffect::Read,
            );
            assert!(host.authorize(Permission::ResourceRead).is_ok());
            assert!(
                host.authorize(Permission::ResourceWrite).is_err(),
                "Read effect cannot write even if both manifest permission sets allow it"
            );
            assert!(
                host.authorize_device_id("device-b").is_err(),
                "Invocation selector further narrows the token allowlist"
            );
            assert!(host.authorize_package_id("package-b").is_err());
            let mut old = HostState::new(
                host.clone(),
                manifest.id().clone(),
                Arc::new(AtomicBool::new(false)),
                None,
            );
            old.devices
                .insert(1, DeviceHandle::new(DeviceId::new("device-a")));
            old.frames.insert(2, FrameHandle::new());
            old.resources.insert(3, ResourceHandle::new());
            old.touches.insert(4, TouchHandle::new());
            old.runs.insert(5, RunHandle::new());
            assert!(old.require_device(1).is_ok());
            assert!(old.require_resource(3).is_ok());
            let fresh = HostState::new(
                host,
                manifest.id().clone(),
                Arc::new(AtomicBool::new(false)),
                None,
            );
            assert!(fresh.require_device(1).is_err());
            assert!(fresh.require_frame(2).is_err());
            assert!(fresh.require_resource(3).is_err());
            assert!(fresh.require_touch(4).is_err());
            assert!(fresh.require_run(5).is_err());
            credentials.revoke(&token.metadata.token_id).unwrap();
            assert!(old.require_device(1).is_err());
            assert!(old.require_frame(2).is_err());
            assert!(old.require_resource(3).is_err());
            assert!(old.require_touch(4).is_err());
            assert!(old.require_run(5).is_err());
        }
    }
}

#[cfg(feature = "wasm-runtime")]
pub(crate) use wasmtime_runtime::LazyWasmtimeRuntime;

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

    // The world now requires both exports. `call` returns a fixed JSON string:
    // the canonical ABI hands string params in through an exported `realloc`
    // and returns the variant via a caller-provided return area (disc=0 ok,
    // then ptr/len of the payload).
    const ENTRY_COMPONENT: &str = r#"(component
      (core module $m
        (memory (export "memory") 1)
        (global $heap (mut i32) (i32.const 1024))
        (func $realloc (export "realloc")
          (param $old i32) (param $old_size i32) (param $align i32) (param $new_size i32) (result i32)
          (local $ptr i32)
          (local.set $ptr
            (i32.and
              (i32.add (global.get $heap) (i32.sub (local.get $align) (i32.const 1)))
              (i32.xor (i32.sub (local.get $align) (i32.const 1)) (i32.const -1))))
          (global.set $heap (i32.add (local.get $ptr) (local.get $new_size)))
          (local.get $ptr)
        )
        (func (export "run"))
        (func $call (export "call")
          (param $action-ptr i32) (param $action-len i32)
          (param $values-ptr i32) (param $values-len i32)
          ;; canon lift(lift 侧)扁平结果超过 1 个时，核心函数返回结果区指针：
          ;; 区内为 [disc=0(ok), ptr, len]。
          (result i32)
          (i32.store (i32.const 512) (i32.const 0))
          (i32.store (i32.add (i32.const 512) (i32.const 4)) (i32.const 256))
          (i32.store (i32.add (i32.const 512) (i32.const 8)) (i32.const 13))
          (i32.const 512)
        )
        (data (i32.const 256) "{\"echo\":true}")
      )
      (core instance $i (instantiate $m))
      (alias core export $i "memory" (core memory $mem))
      (alias core export $i "realloc" (core func $realloc))
      (alias core export $i "run" (core func $run))
      (alias core export $i "call" (core func $call))
      (type $run-type (func))
      (func $run (type $run-type) (canon lift (core func $run)))
      (type $call-type (func (param "action" string) (param "values-json" string)
        (result (result string (error string)))))
      (func $call (type $call-type) (canon lift (core func $call)
        (memory $mem) (realloc (core func $realloc))))
      (instance $ext (export "run" (func $run)) (export "call" (func $call)))
      (export "gamer:host/extension@1.0.0" (instance $ext))
    )"#;

    fn test_host() -> HostApi {
        let manifest = parse_manifest(
            br#"manifest_version = 2
id = "com.example.entry"
version = "1.0.0"
name = "Entry test"
entry = "plugin.wasm"
permissions = ["runtime.sleep"]
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

    #[tokio::test(flavor = "current_thread")]
    async fn plugin_call_dispatches_to_live_instance_and_stop_ends_it() {
        use WasmRuntime as _;
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

        let result = runtime.call(handle, "refresh", "{\"k\":1}").await.unwrap();
        assert_eq!(result, "{\"echo\":true}");

        // 已完成 entry 的实例经命令通道优雅停止，句柄移除后调用不可达。
        runtime.stop(handle).await.unwrap();
        assert!(matches!(
            runtime.call(handle, "refresh", "{}").await,
            Err(crate::extensions::ExtensionError::RuntimeUnavailable(_))
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stop_interrupts_cpu_only_entry_and_waits_for_actual_thread_exit() {
        let runtime = LazyWasmtimeRuntime::new();
        let component = ENTRY_COMPONENT.replace(
            "(func (export \"run\"))",
            "(func (export \"run\") (loop $again (br $again)))",
        );
        let handle = runtime
            .start(WasmStartRequest {
                id: ExtensionId::parse("com.example.entry").unwrap(),
                version: ExtensionVersion::parse("1.0.0").unwrap(),
                wasm: wat::parse_str(component).unwrap(),
                host: test_host(),
                app_context: None,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), runtime.stop(handle))
            .await
            .expect("A CPU-only guest must not outlive stop")
            .unwrap();
        assert!(runtime.call(handle, "refresh", "{}").await.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn generated_host_binding_denies_unlisted_capability() {
        use super::wasmtime_runtime::HostState;
        use crate::extensions::wit::gamer::host::device::Host as _;

        let mut state = HostState::new(
            test_host(),
            ExtensionId::parse("com.example.entry").unwrap(),
            Arc::new(AtomicBool::new(false)),
            None,
        );
        let error = tokio::task::spawn_blocking(move || state.resolve("device-1".to_string()))
            .await
            .unwrap()
            .unwrap_err();
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
            crate::core::AppContext::for_test("device-1", "com.example.game").unwrap();
        let mut state = HostState::new(
            test_host(),
            ExtensionId::parse("com.example.entry").unwrap(),
            Arc::new(AtomicBool::new(false)),
            Some(app_context),
        );
        let context = state.get();
        assert_eq!(context.device_id.as_deref(), Some("device-1"));
        assert_eq!(context.android_package.as_deref(), Some("com.example.game"));
        assert_eq!(context.content_package.as_deref(), Some("com.example.game"));
    }

    /// 现场构建带 host import 的最小 guest 组件（`tests/import-guest`，
    /// wit-bindgen 生成 + wit-component 编码；与 call-guest 同一工程法）。
    /// 产物落在 `target/import-guest/`（首次冷构建约 1 分钟，之后增量）。
    fn import_guest_component() -> Vec<u8> {
        use std::path::PathBuf;
        use std::process::Command;
        use std::sync::OnceLock;

        static COMPONENT: OnceLock<Vec<u8>> = OnceLock::new();
        COMPONENT
            .get_or_init(|| {
                let server_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                let guest_dir = server_dir.join("tests").join("import-guest");
                let target_dir = server_dir.join("target").join("import-guest");
                let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
                let run = |args: &[&str]| {
                    let mut command = Command::new(&cargo);
                    command
                        .current_dir(&guest_dir)
                        .args(args)
                        .arg("--target-dir")
                        .arg(&target_dir);
                    let output = command.output().unwrap_or_else(|error| {
                        panic!("无法启动 import guest cargo 子进程: {error}")
                    });
                    assert!(
                        output.status.success(),
                        "import guest 构建失败: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                };
                run(&[
                    "build",
                    "--locked",
                    "--quiet",
                    "--release",
                    "--lib",
                    "--target",
                    "wasm32-unknown-unknown",
                ]);
                let module = target_dir
                    .join("wasm32-unknown-unknown")
                    .join("release")
                    .join("gamer_import_fixture.wasm");
                let component = target_dir.join("plugin.component.wasm");
                // core module → WASM Component（fixture 自带 componentize bin）。
                let mut command = Command::new(&cargo);
                command
                    .current_dir(&guest_dir)
                    .args(["run", "--quiet", "--release", "--bin", "componentize", "--"])
                    .arg(&module)
                    .arg(&component)
                    .arg("--target-dir")
                    .arg(&target_dir);
                let output = command
                    .output()
                    .expect("无法启动 import guest componentize");
                assert!(
                    output.status.success(),
                    "import guest componentize 失败: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                std::fs::read(&component).expect("import guest component 不存在")
            })
            .clone()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn external_guest_observes_current_token_context_and_fresh_memory() {
        use crate::extensions::PermissionSet;
        use crate::mcp::{
            AllowedObjects, CreateTokenRequest, CredentialStore, InvocationContext, ReceiptStore,
            ToolEffect,
        };
        let directory = tempfile::tempdir().unwrap();
        let credentials = Arc::new(CredentialStore::open(directory.path()).unwrap());
        let receipts = Arc::new(ReceiptStore::open(directory.path()).unwrap());
        let runtime = LazyWasmtimeRuntime::new();
        let component = import_guest_component();
        for suffix in ["a", "b"] {
            let device = format!("device-{suffix}");
            let package = format!("package-{suffix}");
            let token = credentials
                .create(CreateTokenRequest {
                    label: suffix.into(),
                    scopes: ["fixture.read".into()].into_iter().collect(),
                    allowed: AllowedObjects {
                        devices: [device.clone()].into_iter().collect(),
                        packages: [package.clone()].into_iter().collect(),
                        ..Default::default()
                    },
                    expires_in_seconds: 60,
                })
                .unwrap();
            let mut context = InvocationContext::new(
                credentials.authenticate(&token.token).unwrap(),
                "fixture.read".into(),
                None,
                credentials.clone(),
                receipts.clone(),
            );
            context.device_id = Some(device.clone());
            context.package_id = Some(package.clone());
            let handle = runtime
                .start(WasmStartRequest {
                    id: ExtensionId::parse("com.example.import").unwrap(),
                    version: ExtensionVersion::parse("1.0.0").unwrap(),
                    wasm: component.clone(),
                    host: test_host().for_external(
                        context,
                        PermissionSet::parse(["runtime.sleep"]).unwrap(),
                        ToolEffect::Read,
                    ),
                    // An external call must never inherit this UI startup context.
                    app_context: Some(
                        crate::core::AppContext::for_test("device-decoy", "com.example.decoy")
                            .unwrap(),
                    ),
                })
                .await
                .unwrap();
            let value: serde_json::Value =
                serde_json::from_str(&runtime.call(handle, "sleep", "{}").await.unwrap()).unwrap();
            assert_eq!(value["context"]["device_id"], device);
            assert_eq!(value["context"]["content_package"], package);
            assert!(value["context"]["android_package"].is_null());
            assert_eq!(
                value["invocation_counter"], 1,
                "Guest globals must not persist across external tokens"
            );
            runtime.stop(handle).await.unwrap();
        }
    }

    /// Real imported guest coverage: start, context/log import, a capability
    /// future through the host runtime bridge, and actual joined cleanup.
    /// A caught worker panic is an error from stop and fails this test.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn guest_with_host_imports_starts_calls_and_stops_without_trap() {
        use WasmRuntime as _;

        let manifest = parse_manifest(
            br#"manifest_version = 2
id = "com.example.import"
version = "1.0.0"
name = "Import test"
entry = "plugin.wasm"
permissions = ["log.write", "runtime.sleep"]
"#,
        )
        .unwrap();
        let runtime = LazyWasmtimeRuntime::new();
        let handle = runtime
            .start(WasmStartRequest {
                id: ExtensionId::parse("com.example.import").unwrap(),
                version: ExtensionVersion::parse("1.0.0").unwrap(),
                wasm: import_guest_component(),
                host: HostApi::for_manifest(
                    CapabilityRegistry::default(),
                    HostApiCatalog::default(),
                    &manifest,
                )
                .unwrap(),
                app_context: Some(
                    crate::core::AppContext::for_test("device-import", "com.example.game").unwrap(),
                ),
            })
            .await
            .expect("带 import 的插件必须能 start");

        while !runtime.entry_completed(handle).await {
            tokio::task::yield_now().await;
        }

        // call 经 guest 的 host import（context.get）回读 AppContext：证明
        // The real guest import round trip also exercises the blocking I/O
        // bridge without creating Wasmtime async fibers.
        let result = runtime.call(handle, "ping", "{}").await.unwrap();
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(json["action"], "ping");
        assert_eq!(json["context"]["device_id"], "device-import");
        assert_eq!(json["context"]["android_package"], "com.example.game");
        let slept: serde_json::Value =
            serde_json::from_str(&runtime.call(handle, "sleep", "{}").await.unwrap()).unwrap();
        assert_eq!(slept["action"], "sleep");

        runtime.stop(handle).await.unwrap();
        assert!(matches!(
            runtime.call(handle, "ping", "{}").await,
            Err(crate::extensions::ExtensionError::RuntimeUnavailable(_))
        ));
    }
}
