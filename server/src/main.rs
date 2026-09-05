//! GameBot 游戏自动化助手服务端
//!
//! 架构：设备通过 adb 接入，服务端作为 scrcpy 客户端（官方 scrcpy-server）
//! 采集 H.264 视频 + 注入控制；视频流经 WebRTC 转推浏览器；
//! 模板匹配 / YAML 自动化 / 定时任务全部在服务端执行。
//!
//! 启动序列（OPS-004 activation gate）：
//! - 常规路径（无 `GAMER_ACTIVATION_GATE`）：config/store/鉴权 → 设备/调度/
//!   更新栈 → HTTP 服务，行为与历史版本一致；
//! - 闸内路径（`GAMER_ACTIVATION_GATE=1`，候选进程启动形态）：只初始化
//!   「gate 前必需」（config/store/鉴权/停机协调器）即绑端口放行探针与激活
//!   端点；scheduler / 设备扫描 / watchdog / idle_power_loop / DeviceManager
//!   全部延后到 `POST /api/system/activate` 校验通过后执行，完成后换入完整
//!   路由并置 startup.stage=ready（/health/ready 翻转 200）。

mod api;
mod app_packages;
mod build_info;
pub(crate) mod capabilities;
mod config;
mod core;
mod cron_extension;
mod deps_probe;
mod device;
mod extensions;
mod logging;
mod maintenance;
mod matcher;
mod metrics;
mod migrations;
mod resources;
mod run_manager;
mod scheduler;
mod shutdown;
mod store;
mod timer_core;
mod update;
mod webrtc;

// Phase 0 兼容护栏只在测试构建挂载，不改变服务运行时模块图。
#[cfg(test)]
mod phase0_tests;

// P11.9 架构守卫测试（ADR-11/ADR-13 边界 + 隔离集成）同样只在测试构建挂载。
#[cfg(test)]
mod architecture_guard_tests;

use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tracing::{info, warn};

use update::gate::StartupGate;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // DATA-005：维护子命令最前分支（schema-policy §7 零后台服务）——inspect /
    // migrate 在任何 adb / scheduler / HTTP / 设备扫描 / DeviceManager 初始化
    // 之前执行完即退出；无子命令时走既有启动流程，行为逐字节不变
    let argv: Vec<String> = std::env::args().collect();
    match maintenance::parse_args(&argv) {
        Ok(Some(command)) => std::process::exit(maintenance::run_cli(command)),
        Ok(None) => {}
        Err(usage) => {
            eprintln!("{usage}");
            std::process::exit(2);
        }
    }

    // Windows 定时器分辨率：默认 15.6ms 粒度会让 tokio::time::sleep(16ms) 实际睡
    // ~31ms，pusher 的帧率上限（60fps → 16ms 间隔）被砍半 → 设备帧爆发时队列积压
    // （内容滞后 + 画面跳动）。多媒体应用（OBS/游戏）都会调用 timeBeginPeriod(1)
    // 把全局定时器精度提到 1ms；进程退出时系统自动恢复，无需 timeEndPeriod。
    // 零依赖 FFI（winmm.dll），仅 Windows 需要。
    #[cfg(target_os = "windows")]
    {
        #[link(name = "winmm")]
        extern "system" {
            fn timeBeginPeriod(uPeriod: u32) -> u32;
        }
        unsafe {
            timeBeginPeriod(1);
        }
    }

    // 配置加载（OPS-004：文件存在但解析失败/校验不过 → 带位置与清单直接退出；
    // 文件缺失时 dev 放行默认值、prod 报错；此时还没有 logger，失败原因走 stderr）。
    // 滚动日志保留天数同源于此。加载失败经 main 返回 anyhow → 非零退出码。
    let loaded = config::Config::load()?;
    let cfg = loaded.cfg;

    // 日志（OPS-003）：GB_LOG 未设置/留空/="stdout" → 纯 stdout，容器部署天然处于
    // 此形态，轮转与保留交给容器日志驱动；其余值视作基准路径，按天滚动写出
    // <路径>.YYYY-MM-DD 并统一经非阻塞 worker 落盘——guard 绑定在 main 栈帧上，
    // 进程退出时 drop 冲刷残余日志。旧"单文件无限追加"模式已移除。
    let (log_target, _log_guard) = logging::init(cfg.log_retain_days)?;
    if let logging::LogTarget::RollingFile { dir, .. } = &log_target {
        info!(dir = %dir.display(), "file logging with daily rotation enabled");
    }

    info!("GameBot server v{} starting...", env!("CARGO_PKG_VERSION"));
    // 最终生效配置来源 + 非敏感摘要（敏感项如 password 绝不进入日志）
    info!(
        source = %loaded.source,
        profile = loaded.profile.as_str(),
        "config loaded"
    );
    info!("effective config: {}", cfg.non_sensitive_summary());

    // scrcpy-server jar 存在性必检：缺失即退出（没有它连不上任何设备）
    if let Err(e) = cfg.check_scrcpy_jar() {
        tracing::error!("{e:#}");
        return Err(e);
    }
    // adb / ffmpeg 只探测记录不阻断启动（readiness 端点属阶段 4 OBS-001，探测函数已预留）
    for tool in cfg.probe_external_tools() {
        match tool.status {
            Ok(()) => info!(tool = tool.name, path = %tool.path, "external tool ready"),
            Err(reason) => warn!(
                tool = tool.name,
                path = %tool.path,
                reason = %reason,
                "external tool NOT ready (startup continues)"
            ),
        }
    }

    let db = Arc::new(store::Store::open(&cfg)?);
    // 进程级共享指标（OBS-003）：webrtc pusher / 帧缓存 / 设备帧消费等采集点
    // 远离 AppState，经 metrics::global() 取同一实例；未安装时惰性兜底，
    // 采集失败/缺失不影响业务行为（观测为旁路）
    metrics::install_global(db.metrics());

    // 鉴权状态（阶段 2）：凭据链路解析 + 回环管理通道令牌 + 会话治理参数
    // 【gate 前必需】
    let credential = api::auth::resolve_credential(&cfg);
    let setup_required = loaded.profile == config::Profile::Dev
        && cfg.auth.password_hash.trim().is_empty()
        && !std::env::var("GAMER_ADMIN_PASSWORD")
            .ok()
            .is_some_and(|password| !password.trim().is_empty());
    let admin_token = api::auth::resolve_admin_token(loaded.profile);
    let auth = Arc::new(api::auth::AuthState::new_with_setup(
        credential,
        cfg.auth.clone(),
        loaded.profile == config::Profile::Prod,
        admin_token,
        setup_required,
    ));
    info!(
        source = %auth.credential_source(),
        secure_cookies = auth.secure_cookies(),
        setup_required,
        "auth enabled (session cookies; /api/** requires login)"
    );

    // activation gate（OPS-004）：GAMER_ACTIVATION_GATE=1 → 闸内启动
    let gate = StartupGate::from_env();
    let gate_shared = Arc::new(api::gate::GateShared::default());

    // 统一停机协调器（OPS-001）：drain 依赖（runs/viewers/devices）在闸内路径
    // 尚不存在 → 经 DrainSlot 延后装配（闸内无会话可拆 = no-op；完整初始化后
    // 换入真实 drain），/api/shutdown 与 Ctrl+C / SIGTERM 共用同一入口。
    let drain_slot: DrainSlot = Arc::new(std::sync::RwLock::new(None));
    let shutdown = Arc::new(shutdown::ShutdownCoordinator::new(slot_drain(
        drain_slot.clone(),
    )));
    shutdown::spawn_signal_listener(shutdown.clone());

    if gate.enabled() {
        info!("activation gate enabled (GAMER_ACTIVATION_GATE=1): booting behind maintenance gate");
        let app = api::gate::build_gate_router(
            cfg.clone(),
            db.clone(),
            shutdown.clone(),
            gate.clone(),
            gate_shared.clone(),
        );
        // 激活任务：等待 activate → 完整初始化（【activate 后】序列）→ 换入完整路由
        let init_cfg = cfg.clone();
        let init_db = db.clone();
        let init_auth = auth.clone();
        let init_shutdown = shutdown.clone();
        let init_slot = drain_slot.clone();
        let init_gate = gate.clone();
        let init_shared = gate_shared.clone();
        tokio::spawn(async move {
            init_gate.wait_activation().await;
            info!("activation received: completing full initialization");
            match RuntimeServices::start(
                &init_cfg,
                init_db.clone(),
                init_auth.clone(),
                init_slot.clone(),
            )
            .await
            {
                Ok(ctx) => {
                    let update = spawn_update_stack(&init_cfg, init_db.clone(), &ctx);
                    // 运行日志保留（DATA-004）随完整初始化启动
                    if init_cfg.log_retain_days > 0 {
                        let retention_db = init_db.clone();
                        let retain_days = init_cfg.log_retain_days;
                        tokio::spawn(run_log_retention(
                            retention_db,
                            retain_days,
                            init_shutdown.subscribe(),
                        ));
                    }
                    update::gate::set_stage(update::gate::STAGE_READY);
                    let router =
                        ctx.router(init_db, init_cfg.clone(), init_shutdown, init_auth, update);
                    init_shared.set(router);
                    info!("full initialization complete: business routes open (stage=ready)");
                }
                Err(e) => {
                    tracing::error!(err = %e, "post-activation initialization failed; staying gated");
                }
            }
        });
        serve(cfg, app, shutdown.clone()).await?;
        info!("server exited");
        return Ok(());
    }

    // ---- 常规启动路径（无 gate：行为与历史版本一致） ----
    // 与激活路径共用同一个 composition root，避免两条启动路径的依赖图漂移。
    let ctx = RuntimeServices::start(&cfg, db.clone(), auth.clone(), drain_slot.clone()).await?;

    // 更新子系统（批次 3）：controller 按部署形态装配 + 策略协调器后台任务
    let update = spawn_update_stack(&cfg, db.clone(), &ctx);

    // 运行日志保留策略（DATA-004）：启动时已做一次清理，这个低频任务负责长期
    // 运行实例。SQLite 调用放入 blocking 池，不占用 Tokio 核心线程；每次只删除小批量。
    if cfg.log_retain_days > 0 {
        let retention_db = db.clone();
        let retain_days = cfg.log_retain_days;
        tokio::spawn(run_log_retention(
            retention_db,
            retain_days,
            shutdown.subscribe(),
        ));
    }

    // HTTP + WebSocket API（停机经协调器：/api/shutdown 与信号同路径）
    let mut shutdown_rx = shutdown.subscribe();
    let app = ctx.router(db, cfg.clone(), shutdown.clone(), auth, update);
    let listener = TcpListener::bind(cfg.listen_addr()).await?;
    info!("GameBot server ready on http://{}", cfg.listen_addr());
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown_rx.changed().await;
        info!("graceful shutdown: http server draining");
    })
    .await?;
    info!("server exited");

    Ok(())
}

/// 运行时上下文：闸内路径激活后与常规路径共用的完整业务依赖集合
struct RuntimeServices {
    resources: Arc<resources::ResourceStore>,
    viewers: webrtc::ViewerMap,
    devices: Arc<device::DeviceManager>,
    runs: Arc<run_manager::RunManager>,
    scheduler: Arc<scheduler::Scheduler>,
    extensions: Arc<extensions::ExtensionService>,
}

impl RuntimeServices {
    /// Build the HTTP graph from one fully initialized dependency set. Both
    /// normal startup and activation-gate startup use this composition root;
    /// the gate itself remains a deliberately smaller router.
    fn router(
        &self,
        db: Arc<store::Store>,
        cfg: config::Config,
        shutdown: Arc<shutdown::ShutdownCoordinator>,
        auth: Arc<api::auth::AuthState>,
        update: Arc<update::service::UpdateService>,
    ) -> axum::Router {
        api::build_router_with_extensions(
            db,
            self.devices.clone(),
            self.runs.clone(),
            self.scheduler.clone(),
            cfg,
            self.viewers.clone(),
            self.resources.clone(),
            shutdown,
            auth,
            update,
            self.extensions.clone(),
        )
    }
}

impl RuntimeServices {
    /// Complete service graph for both normal startup and activation-gate
    /// startup. Device and scheduler background work begins only after the
    /// drain slot is populated. Background lifecycles (video watchdog, auth
    /// session sweeper) are also started here so that router assembly stays
    /// pure route registration.
    async fn start(
        cfg: &config::Config,
        db: Arc<store::Store>,
        auth: Arc<api::auth::AuthState>,
        drain_slot: DrainSlot,
    ) -> anyhow::Result<Self> {
        let resources = Arc::new(resources::ResourceStore::open(cfg)?);
        // P11.3：扩展内容钩子注册（组合根引导期）——gamer.yaml 的脚本/函数/
        // 模板校验与 gamer.keymap 的方案校验。裸 Core（不注册）时保存不做
        // 内容校验（§8.9 验收锚点）。
        extensions::gamer_yaml::register_resource_handlers(&resources);
        extensions::register_resource_handlers(&resources);
        let viewers: webrtc::ViewerMap =
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let devices = Arc::new(device::DeviceManager::new(db.clone(), cfg.clone()));
        // P12.9：v3-only 执行器——非 `version: 3` 脚本统一报版本错误，无 fallback
        let executor = Arc::new(extensions::gamer_yaml::runner_adapter::EngineExecutor::new(
            devices.clone(),
            db.clone(),
        ));
        let runs = Arc::new(run_manager::RunManager::new(executor.clone()));
        // ADR-13：裸 Core 组合——Scheduler 不再预置任何 runner；gamer.yaml 的
        // 定时 runner 由扩展 start 生命周期经 registrar 钩子注册。
        let scheduler = Arc::new(scheduler::Scheduler::new(db.clone()));
        let capabilities = capabilities::adapters::build_registry(
            devices.clone(),
            resources.clone(),
            db.clone(),
            runs.clone(),
        );
        let runner_registrar = Arc::new(
            extensions::gamer_yaml::timer_yaml::YamlTimerRunnerRegistrar::new(
                scheduler.clone(),
                db.clone(),
                runs.clone(),
                resources.clone(),
            ),
        );
        let extensions = Arc::new(
            extensions::ExtensionService::for_data_root(cfg.data_dir.clone(), capabilities)
                .with_runner_registrar(runner_registrar),
        );
        let ctx = Self {
            resources,
            viewers,
            devices,
            runs,
            scheduler,
            extensions,
        };
        // 与常规路径一致：在任何设备扫描/保活启动前接入统一 drain，避免
        // activation 后初始化窗口收到 SIGTERM 时漏掉已创建的运行依赖。
        install_drain(&drain_slot, &ctx);
        // P12.6：v3 运行可视化事件走同一 viewer DataChannel（复用 ViewerEventSink）；
        // 无 viewer 时事件自然丢弃。
        executor.attach_yaml_vnext(
            ctx.resources.clone(),
            ctx.extensions.clone(),
            Some(Arc::new(webrtc::ViewerEventSink::new(ctx.viewers.clone()))),
        );
        // 后台生命周期统一在组合根启动：视频静默看门狗（devices/viewers/metrics
        // 三依赖）+ 会话过期清扫（小时级）。路由组装（api::build_router_*）只注册路由。
        api::system::spawn_watchdog(ctx.devices.clone(), ctx.viewers.clone(), db.metrics());
        api::auth::spawn_sweeper(auth);
        ctx.devices.start().await?;
        // P11.7 启动对账：恢复重启前遗留 Running 的扩展（实例 + runner 注册 +
        // UI 贡献），必须在 Scheduler 启动前完成——恢复出的 runner 要立即可
        // 派发任务；失败降级 Enabled，不阻塞启动。
        ctx.extensions.reconcile_startup().await;
        ctx.scheduler.start().await;
        Ok(ctx)
    }
}

/// 停机 drain 的延后装配槽：闸内启动时 drain 依赖不存在，完整初始化后写入
type DrainSlot = Arc<std::sync::RwLock<Option<shutdown::DrainFn>>>;

/// drain 闭包工厂：读槽取真实 drain；槽为空（闸内未激活）时无会话可拆 = no-op
fn slot_drain(slot: DrainSlot) -> shutdown::DrainFn {
    Arc::new(move || {
        let slot = slot.clone();
        Box::pin(async move {
            let drain = slot.read().unwrap().clone();
            if let Some(drain) = drain {
                drain().await;
            }
        }) as futures_util::future::BoxFuture<'static, ()>
    })
}

/// 完整初始化后装配真实 drain（RunManager drain → 踢 viewer → 拆 scrcpy 会话）
fn install_drain(slot: &DrainSlot, ctx: &RuntimeServices) {
    let runs = ctx.runs.clone();
    let viewers = ctx.viewers.clone();
    let devices = ctx.devices.clone();
    *slot.write().unwrap() = Some(Arc::new(move || {
        let runs = runs.clone();
        let viewers = viewers.clone();
        let devices = devices.clone();
        Box::pin(shutdown::drain_sessions(runs, viewers, devices))
            as futures_util::future::BoxFuture<'static, ()>
    }));
}

/// 更新子系统装配（两种启动路径共用）：controller 按部署形态选择（launcher
/// named pipe / Docker external / 直跑 unsupported）→ 策略存储（config 基线 +
/// state/ 持久化覆盖）→ workload 源（活跃运行/viewer/cron/升级事务）→ 服务 →
/// 协调器后台任务。
fn spawn_update_stack(
    cfg: &config::Config,
    db: Arc<store::Store>,
    ctx: &RuntimeServices,
) -> Arc<update::service::UpdateService> {
    let mode = deps_probe::Mode::detect();
    let controller = update::controller::build_for_mode(mode);
    let controller_strategy = controller.strategy();
    let policy = update::policy::PolicyStore::load_blocking(
        &cfg.data_dir,
        update::policy::UpdatePolicy::from_config(&cfg.update),
    );
    let txn = Arc::new(update::service::UpdateTxn::default());
    let runs = ctx.runs.clone();
    let viewers = ctx.viewers.clone();
    let scheduler = ctx.scheduler.clone();
    let txn_for_workload = txn.clone();
    let workload: update::service::WorkloadProvider = Arc::new(move || {
        update::workload::WorkloadSource::new(runs.clone(), viewers.clone(), scheduler.clone(), {
            let txn = txn_for_workload.clone();
            Arc::new(move || txn.is_active()) as Arc<dyn Fn() -> bool + Send + Sync>
        })
        .snapshot()
    });
    let service = Arc::new(update::service::UpdateService::new(
        controller, policy, txn, workload, db,
    ));
    update::coordinator::Coordinator::spawn(service.clone());
    info!(
        mode = mode.as_str(),
        strategy = controller_strategy,
        "update stack online (controller + policy + coordinator)"
    );
    service
}

/// 闸内/常规路径共用的 HTTP 服务装配：绑定端口 + 优雅停机挂协调器 watch
async fn serve(
    cfg: config::Config,
    app: axum::Router,
    shutdown: Arc<shutdown::ShutdownCoordinator>,
) -> anyhow::Result<()> {
    let mut shutdown_rx = shutdown.subscribe();
    let listener = TcpListener::bind(cfg.listen_addr()).await?;
    info!("GameBot server ready on http://{}", cfg.listen_addr());
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown_rx.changed().await;
        info!("graceful shutdown: http server draining");
    })
    .await?;
    Ok(())
}

/// 运行日志保留周期（DATA-004）：低频兜底清理即可，无需可配置——保留天数
/// 复用 config.toml 的 `log_retain_days`，这里只固定触发频率。
const LOG_RETENTION_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// 运行日志周期保留任务（DATA-004）：每 [`LOG_RETENTION_INTERVAL`] 调用一次
/// [`store::Store::prune_logs`]（分批删除，避免一次大事务长期占用数据库锁）。
/// 挂在 main 的 watch 停机信号上——服务关闭时任务随之结束，不阻塞优雅退出。
async fn run_log_retention(
    db: Arc<store::Store>,
    retain_days: u32,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(LOG_RETENTION_INTERVAL);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                match db.prune_logs_async(retain_days).await {
                    Ok(deleted) if deleted > 0 => {
                        info!(deleted, retain_days, "periodic run log cleanup removed expired rows");
                    }
                    Ok(_) => {}
                    Err(e) => warn!(error = %e, "periodic run log cleanup failed"),
                }
            }
            _ = shutdown.changed() => {
                info!("run log retention task stopped (shutdown)");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 周期保留任务随停机信号结束（DATA-004）：首轮 tick 即触发一次
    /// prune_logs（分批删除逻辑本体已在 store.rs 测试覆盖），收到停机
    /// 信号后任务必须在有限时间内退出，不悬挂阻塞优雅关机。
    #[tokio::test]
    async fn log_retention_task_exits_on_shutdown_signal() {
        let dir = std::env::temp_dir().join(format!(
            "gamer-retention-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = config::Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let db = Arc::new(store::Store::open(&cfg).unwrap());
        let (tx, rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(run_log_retention(db, 14, rx));

        tx.send(true).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), task).await;
        assert!(result.is_ok(), "保留任务未随停机信号退出");

        std::fs::remove_dir_all(dir).unwrap();
    }
}
