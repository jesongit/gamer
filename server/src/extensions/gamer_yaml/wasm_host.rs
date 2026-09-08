//! YAML world 的 Wasmtime 宿主（`feature = "wasm-runtime"`）。
//!
//! guest 的两个私有通道都落在扩展边界：
//! - `capability.invoke("__event", …)`：运行结构事件 → [`EventSink`]；
//! - `capability.invoke("__fn", …)`：原生函数派发 → [`NativeYamlHost`]。
//!
//! 通用扩展 world 的宿主仍在 `crate::extensions::wasm`，YAML 专用状态与
//! runtime 不进入 Core 扩展机制模块。

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as AsyncMutex;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Engine, Store, StoreContextMut, UpdateDeadline};

use super::yaml_extension::{
    NativeYamlHost, YamlWasmRunRequest, YamlWasmRunResult, YamlWasmRuntime, EVENT_CAPABILITY,
    FN_CAPABILITY,
};
use crate::core::events::{RuntimeEvent, RuntimeEventKind};
use crate::extensions::host_api::HostApi;
use crate::extensions::wit;

/// Request/response Component runtime for YAML V1. Unlike the generic
/// lifecycle runtime this invokes a supplied lowered program and does not
/// compile the interpreter source into the host process.
///
/// （ADR-YAML-04）Engine 开启 epoch interruption 作为取消兜底——guest 纯计算
/// 死循环不经过 capability 边界，stop 标志只能靠 epoch 检查点打断。epoch 仅
/// 服务取消，不做 host 超时强杀（步预算语义由 yaml-interp 的执行预算承载）。
#[derive(Debug)]
pub(crate) struct LazyYamlWasmtimeRuntime {
    engine: OnceLock<Engine>,
    /// 与 engine 同生命周期创建的 epoch ticker（见 [`EpochTicker`]）。
    ticker: OnceLock<Arc<EpochTicker>>,
    components: AsyncMutex<HashMap<[u8; 32], Arc<Component>>>,
}

impl LazyYamlWasmtimeRuntime {
    pub(crate) fn new() -> Self {
        Self {
            engine: OnceLock::new(),
            ticker: OnceLock::new(),
            components: AsyncMutex::new(HashMap::new()),
        }
    }

    fn engine(&self) -> &Engine {
        self.engine.get_or_init(|| {
            let mut config = wasmtime::Config::new();
            config.epoch_interruption(true);
            let engine = Engine::new(&config).expect("Wasmtime engine config is valid");
            let ticker = Arc::new(EpochTicker::new(engine.clone()));
            self.ticker
                .set(ticker)
                .expect("epoch ticker only initialized once");
            engine
        })
    }

    fn ticker(&self) -> &Arc<EpochTicker> {
        self.engine();
        self.ticker.get().expect("ticker created with engine")
    }
}

/// epoch ticker：Engine 级全局单例线程。
///
/// `increment_epoch` 对该 Engine 的所有并发 store 生效，因此线程按 Engine
/// 唯一、绝不每 run 一个。生命周期：生产环境 runtime 是进程单例，ticker 线程
/// 随首个 run 按需拉起、空闲后自行退出；即使 ticker 意外缺失，解释器步预算
/// 仍保证终止，只是取消延迟退化为「跑到预算耗尽」。tick 周期 ~10ms。
#[derive(Debug)]
struct EpochTicker {
    engine: Engine,
    /// 在飞 wasm run 数；>0 时线程才推进 epoch。
    active: AtomicUsize,
    /// ticker 线程存活标记（与 `active` 的读写顺序见 `enter`/`thread_loop`）。
    spawned: Mutex<bool>,
}

impl EpochTicker {
    const TICK: Duration = Duration::from_millis(10);

    fn new(engine: Engine) -> Self {
        Self {
            engine,
            active: AtomicUsize::new(0),
            spawned: Mutex::new(false),
        }
    }

    /// run 入口：登记在飞计数并确保 ticker 存活。
    fn enter(self: &Arc<Self>) {
        self.active.fetch_add(1, Ordering::Relaxed);
        let mut spawned = self.spawned.lock().unwrap();
        if !*spawned {
            *spawned = true;
            let ticker = self.clone();
            drop(spawned);
            std::thread::Builder::new()
                .name("yaml-wasm-epoch-ticker".into())
                .spawn(move || ticker.thread_loop())
                .expect("yaml epoch ticker 线程启动失败");
        }
    }

    /// run 出口：回退在飞计数（经 [`TickerGuard`] 在 drop 时调用）。
    fn leave(&self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
    }

    fn thread_loop(self: Arc<Self>) {
        loop {
            std::thread::sleep(Self::TICK);
            let mut spawned = self.spawned.lock().unwrap();
            if self.active.load(Ordering::Relaxed) == 0 {
                // 空闲退出；下一次 enter 会重新拉起（判定同锁互斥）。
                *spawned = false;
                return;
            }
            drop(spawned);
            self.engine.increment_epoch();
        }
    }
}

/// run 期间持有：drop 时回退 ticker 活动计数（含异常展开路径）。
struct TickerGuard<'a>(&'a EpochTicker);

impl Drop for TickerGuard<'_> {
    fn drop(&mut self) {
        self.0.leave();
    }
}

/// The YAML world has a separate state type. This keeps its function-dispatch
/// behavior out of the generic extension HostState.
struct YamlHostState {
    host: HostApi,
    cancelled: Arc<AtomicBool>,
    app_context: Option<crate::core::AppContext>,
    sink: Option<Arc<dyn crate::core::events::EventSink>>,
}

impl YamlHostState {
    fn new(
        host: HostApi,
        cancelled: Arc<AtomicBool>,
        app_context: crate::core::AppContext,
        sink: Option<Arc<dyn crate::core::events::EventSink>>,
    ) -> Self {
        Self {
            host,
            cancelled,
            app_context: Some(app_context),
            sink,
        }
    }

    /// `__event` 私有通道拦截：guest 把 `{"ev":...}` 事件 JSON 发到
    /// `capability.invoke("__event", …)`，这里**先于**权限校验解析成
    /// [`RuntimeEventKind`]（serde tag="ev" 白名单即事件词表），补 run 维度的
    /// device 作用域后转发 sink。解析失败 / 无 sink / 发射失败一律静默。
    fn emit_run_event(
        &mut self,
        args_json: &str,
    ) -> Result<String, wit::yaml::gamer::host::types::HostError> {
        let sink = self.sink.clone();
        let context = self.app_context.clone();
        let args_json = args_json.to_string();
        let future = async move {
            let Some(sink) = sink else {
                return Ok("null".to_string());
            };
            let context =
                context.ok_or_else(|| anyhow::anyhow!("capability.invoke 需要 AppContext"))?;
            // 非法事件静默丢弃（serde tag="ev" 解析即白名单校验）
            let Ok(kind) = serde_json::from_str::<RuntimeEventKind>(&args_json) else {
                return Ok("null".to_string());
            };
            sink.emit(RuntimeEvent::new(context.device_id.clone(), kind))
                .await?;
            Ok("null".to_string())
        };
        // 发射失败只记 debug：可视化事件不影响运行结果
        match block_on_yaml(future) {
            Ok(payload) => Ok(payload),
            Err(error) => {
                tracing::debug!(%error, "yaml run event emit failed");
                Ok("null".to_string())
            }
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
        // 私有事件通道：不进 CapabilityRegistry、不做权限校验
        if capability == EVENT_CAPABILITY {
            return self.emit_run_event(&args_json);
        }
        // 原生函数派发通道：Schema 校验与权限检查在 NativeYamlHost 内完成。
        if capability == FN_CAPABILITY {
            let host = self.host.clone();
            let context = self.app_context.clone();
            let cancelled = self.cancelled.clone();
            let sink = self.sink.clone();
            let result = block_on_yaml(async move {
                let context =
                    context.ok_or_else(|| anyhow::anyhow!("capability.invoke 需要 AppContext"))?;
                let (name, args) = serde_json::from_str::<serde_json::Value>(&args_json)
                    .map_err(|error| anyhow::anyhow!("__fn 参数不是合法 JSON: {error}"))
                    .and_then(|value| {
                        let name = value
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| anyhow::anyhow!("__fn 缺少 name"))?
                            .to_string();
                        let args = value
                            .get("args")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        Ok((name, args))
                    })?;
                let value = NativeYamlHost::call_function_json(
                    host,
                    context,
                    cancelled,
                    sink,
                    &name,
                    &serde_json::to_string(&args)?,
                )
                .await?;
                Ok::<_, anyhow::Error>(serde_json::to_string(&value)?)
            });
            return result.map_err(|error| yaml_capability_error(&error));
        }
        Err(yaml_error(
            wit::yaml::gamer::host::types::HostErrorKind::InvalidRequest,
            format!("未知 capability 通道: {capability}"),
        ))
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

/// 仅测试：锁定 capability 错误 → WIT host-error kind 的映射（epoch 取消
/// 兜底与 capability 边界取消并行，见 ADR-YAML-04 与对应 e2e 测试注释）。
#[cfg(test)]
pub(crate) fn yaml_capability_error_for_test(
    error: &anyhow::Error,
) -> wit::yaml::gamer::host::types::HostError {
    yaml_capability_error(error)
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
            request.stop.clone(),
            request.context,
            request.sink.clone(),
        );
        let mut store = Store::new(self.engine(), state);
        // epoch 取消兜底（ADR-YAML-04）：deadline 以 1 tick 为步进，每次 tick
        // 到点回调里复查 stop 标志——未取消则续期继续执行，已取消则以
        // CANCELLED 错误终止 guest。deadline 必须在 instantiate 之前就位。
        store.set_epoch_deadline(1);
        store.epoch_deadline_callback(
            |context: StoreContextMut<'_, YamlHostState>| -> wasmtime::Result<UpdateDeadline> {
                if context.data().cancelled.load(Ordering::Relaxed) {
                    return Err(wasmtime::Error::msg(
                        "CANCELLED: 宿主取消（stop 标志已置位，epoch 中断）",
                    ));
                }
                Ok(UpdateDeadline::Continue(1))
            },
        );
        let instance = wit::yaml::YamlExtensionHost::instantiate(&mut store, &component, &linker)
            .map_err(|error| anyhow::anyhow!("YAML 组件实例化失败: {error}"))?;
        let program = serde_json::to_string(&request.program)?;
        // ticker 只在 wasm 执行窗口内推进 epoch（见 EpochTicker 生命周期）。
        // RAII guard：call 异常展开时也要回退活动计数，避免 ticker 永不退出。
        let ticker = self.ticker();
        ticker.enter();
        let call_result = {
            let _guard = TickerGuard(ticker);
            instance
                .gamer_host_automation()
                .func_run()
                .call(&mut store, (&program,))
        };
        let (result,) = match call_result {
            Ok(result) => result,
            Err(error) => {
                if request.stop.load(Ordering::Relaxed) {
                    // epoch trap 取消：与 capability 边界的 Cancelled 同形，
                    // 错误文本带机器可读码。
                    anyhow::bail!("CANCELLED: guest 执行被宿主取消打断（epoch trap）");
                }
                // 非取消类 trap（栈溢出等）映射为运行失败，保留 trap 摘要。
                anyhow::bail!("YAML guest 执行失败: {error:#}");
            }
        };
        let result = result.map_err(|error| anyhow::anyhow!("YAML guest 返回错误: {error}"))?;
        let value = serde_json::from_str::<serde_json::Value>(&result)
            .map_err(|error| anyhow::anyhow!("YAML guest 返回值不是 JSON: {error}"))?;
        Ok(YamlWasmRunResult { value })
    }

    fn is_available(&self) -> bool {
        true
    }
}
