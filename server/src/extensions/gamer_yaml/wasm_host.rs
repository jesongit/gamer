//! YAML world 的 Wasmtime 宿主（`feature = "wasm-runtime"`）。
//!
//! 自 gamer_yaml 扩展边界导出（P11.3）：guest 的 capability.invoke 经
//! [`yaml_extension::NativeYamlHost`] 落到 Core capability registry，
//! programs.resolve 走调用方注入的 [`YamlProgramResolver`]。通用扩展 world
//! 的宿主仍在 `crate::extensions::wasm`，YAML 专用状态与 runtime 不进入
//! Core 扩展机制模块。

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
    NativeYamlHost, YamlProgramResolver, YamlWasmRunRequest, YamlWasmRunResult, YamlWasmRuntime,
    EVENT_CAPABILITY,
};
use super::yaml_vnext;
use crate::core::events::{RuntimeEvent, RuntimeEventKind};
use crate::extensions::host_api::HostApi;
use crate::extensions::wit;

/// Request/response Component runtime for YAML v3. Unlike the generic
/// lifecycle runtime this invokes a supplied lowered program and does not
/// compile the legacy Rust Engine into WASM.
///
/// P12.4（ADR-YAML-04）：Engine 开启 epoch interruption 作为取消兜底——guest
/// 纯计算死循环不经过 capability 边界，stop 标志只能靠 epoch 检查点打断。
/// epoch 仅服务取消，不做 host 超时强杀（预算语义全部由 guest 的
/// ExecutionBudget 承载，见 tests/yaml-guest）。
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

/// epoch ticker：Engine 级全局单例线程（P12.4）。
///
/// `increment_epoch` 对该 Engine 的所有并发 store 生效，因此线程按 Engine
/// 唯一、绝不每 run 一个。生命周期：
///
/// - 生产环境 `LazyYamlWasmtimeRuntime` 是进程单例（`yaml_runtime()`），
///   ticker 线程随首个 run 按需拉起、空闲（无在飞 run）后自行退出，下次
///   run 再拉起——不留常驻 100Hz 空转线程；
/// - `enter` 先累加活动计数再在 `spawned` 锁内决定是否拉起线程，线程退出
///   判定与拉起判定互斥于同一把锁并对 `active` 复查，不存在「有在飞 run
///   但没有 ticker」的窗口；
/// - 即使 ticker 意外缺失，guest 步预算（STEP_BUDGET_EXCEEDED）仍保证终止，
///   只是取消延迟退化为「跑到预算耗尽」。
///
/// tick 周期 ~10ms：`cancelled` 置位后的下一个 epoch 检查点（≤ ~10ms）由
/// store 侧 `epoch_deadline_callback` 转成 CANCELLED 错误。
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

/// The YAML world has a separate state type. This keeps its resolver and
/// source-oriented call behavior out of the generic extension HostState.
///
/// `sink`（P12.6）：v3 运行事件汇——`__event` 私有 capability 拦截转发 +
/// vision/input capability 的宿主侧补发（经 NativeYamlHost）都走它。
struct YamlHostState {
    host: HostApi,
    cancelled: Arc<AtomicBool>,
    app_context: Option<crate::core::AppContext>,
    yaml_programs: Option<Arc<dyn YamlProgramResolver>>,
    sink: Option<Arc<dyn crate::core::events::EventSink>>,
}

impl YamlHostState {
    fn new(
        host: HostApi,
        cancelled: Arc<AtomicBool>,
        app_context: crate::core::AppContext,
        yaml_programs: Option<Arc<dyn YamlProgramResolver>>,
        sink: Option<Arc<dyn crate::core::events::EventSink>>,
    ) -> Self {
        Self {
            host,
            cancelled,
            app_context: Some(app_context),
            yaml_programs,
            sink,
        }
    }

    /// `__event` 私有通道拦截（P12.6，方案 (a) 零 WIT 变更）：guest 把
    /// `{"ev":...}` 事件 JSON 发到 `capability.invoke("__event", …)`，这里
    /// **先于**权限校验/NativeYamlHost 解析成 [`RuntimeEventKind`]（serde
    /// tag="ev" 白名单即事件词表），补 run 维度的 device 作用域后转发 sink。
    /// 解析失败 / 无 sink / 发射失败一律静默——可视化事件不影响运行结果，
    /// 也不要求扩展声明任何权限。
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
        // P12.6 私有事件通道：不进 CapabilityRegistry、不做权限校验（方案 (a)）
        if capability == EVENT_CAPABILITY {
            return self.emit_run_event(&args_json);
        }
        let host = self.host.clone();
        let context = self.app_context.clone();
        let cancelled = self.cancelled.clone();
        let sink = self.sink.clone();
        let result = block_on_yaml(async move {
            let context =
                context.ok_or_else(|| anyhow::anyhow!("capability.invoke 需要 AppContext"))?;
            let value = NativeYamlHost::invoke_json(
                host,
                context,
                cancelled,
                sink,
                &capability,
                &args_json,
            )
            .await?;
            Ok::<_, anyhow::Error>(serde_json::to_string(&value)?)
        });
        result.map_err(|error| yaml_capability_error(&error))
    }
}

impl wit::yaml::gamer::host::programs::Host for YamlHostState {
    fn resolve(&mut self, target: String, args_json: String) -> Result<String, String> {
        let resolver = self
            .yaml_programs
            .clone()
            .ok_or_else(|| "YAML call resolver 未配置".to_string())?;
        let args = serde_json::from_str::<serde_json::Value>(&args_json)
            .map_err(|error| format!("call 参数不是 JSON: {error}"))?;
        let args = yaml_vnext::Value::from_json(args)
            .map_err(|error| format!("call 参数无效: {error}"))?;
        let yaml_vnext::Value::Map(args) = args else {
            return Err("call 参数必须是 map".to_string());
        };
        // 调用深度由 guest 本地 ExecutionBudget 计数（ADR-YAML-04），resolver
        // 只负责按命名空间定位目标程序，不再做深度守卫。
        let program = resolver.resolve(&target, &args).map_err(|error| error.to_string())?;
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

/// 每 run 随机 nonce（wait 随机区间种子）：系统时钟纳秒 + 进程 id 混合。
/// 只需 run 级不可预测性，不追求密码学强度。
fn run_nonce() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ ((std::process::id() as u64) << 32)
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
            request.resolver,
            request.sink.clone(),
        );
        let mut store = Store::new(self.engine(), state);
        // epoch 取消兜底（P12.4 / ADR-YAML-04）：deadline 以 1 tick 为步进，
        // 每次 tick 到点回调里复查 stop 标志——未取消则续期继续执行（全局
        // epoch 推进对并发 run 一视同仁，续期保证非取消 run 不被打断），
        // 已取消则以 CANCELLED 错误终止 guest。epoch 只服务取消，不做
        // host 超时强杀。instantiate 可能执行组件 start 代码，deadline 与
        // 回调必须在 instantiate 之前就位（epoch interruption 开启后 deadline
        // 缺省为 0，会立即 trap）。
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
        let mut program = serde_json::to_value(&request.program)?;
        if let serde_json::Value::Object(ref mut program) = program {
            program.insert("args".to_string(), serde_json::to_value(request.args)?);
            // wait 随机区间（契约 §4，方案 (a)）：每 run 注入 nonce 作 guest 内
            // splitmix64 种子；不新增 WIT 能力（T3 刚稳定 ABI）。
            program.insert(
                "nonce".to_string(),
                serde_json::Value::from(run_nonce()),
            );
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
        // ticker 只在 wasm 执行窗口内推进 epoch（见 EpochTicker 生命周期）。
        // RAII guard：call 异常展开时也要回退活动计数，避免 ticker 永不退出。
        let ticker = self.ticker();
        ticker.enter();
        let call_result = {
            let _guard = TickerGuard(&*ticker);
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
