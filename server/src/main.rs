//! GameBot 游戏自动化助手服务端
//!
//! 架构：设备通过 adb 接入，服务端作为 scrcpy 客户端（官方 scrcpy-server）
//! 采集 H.264 视频 + 注入控制；视频流经 WebRTC 转推浏览器；
//! 模板匹配 / YAML 自动化 / 定时任务全部在服务端执行。

mod api;
mod config;
mod device;
mod engine;
mod logging;
mod matcher;
mod metrics;
mod run_manager;
mod scheduler;
mod script_v2;
mod scripts;
mod store;
mod task_params;
mod webrtc;

use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
    // 优雅停机信号（POST /api/shutdown 拆完会话后触发）；先于周期任务创建，
    // 运行日志保留任务同挂此信号——服务关闭时随之结束
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    // 运行日志保留策略（DATA-004）：启动时已做一次清理，这个低频任务负责长期
    // 运行实例。SQLite 调用放入 blocking 池，不占用 Tokio 核心线程；每次只删除小批量。
    if cfg.log_retain_days > 0 {
        let retention_db = db.clone();
        let retain_days = cfg.log_retain_days;
        tokio::spawn(run_log_retention(
            retention_db,
            retain_days,
            shutdown_rx.clone(),
        ));
    }

    // 鉴权状态（阶段 2）：凭据链路解析 + 回环管理通道令牌 + 会话治理参数
    let credential = api::auth::resolve_credential(&cfg);
    let admin_token = api::auth::resolve_admin_token(loaded.profile);
    let auth = Arc::new(api::auth::AuthState::new(
        credential,
        cfg.auth.clone(),
        loaded.profile == config::Profile::Prod,
        admin_token,
    ));
    info!(
        source = %auth.credential_source(),
        secure_cookies = auth.secure_cookies(),
        "auth enabled (session cookies; /api/** requires login)"
    );

    // 脚本/模板按应用分区存储（data/<pkg>/yaml|tmpl）；旧目录布局不再自动迁移
    let scripts = Arc::new(scripts::ScriptStore::open(&cfg)?);

    // 每设备活跃 viewer 注册表：AppState / Scheduler / DeviceManager（空闲断开守卫）共享
    // （引擎经 control DataChannel 反向推送脚本可视化事件，定时任务运行时同样生效）
    let viewers: webrtc::ViewerMap =
        Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    // 设备管理器：负责 adb 发现 + scrcpy 会话（start 内含启动扫描自举 + WiFi adb 保活）
    let devices = Arc::new(device::DeviceManager::new(
        db.clone(),
        cfg.clone(),
        viewers.clone(),
    ));
    devices.start().await?;

    // 统一运行管理（阶段 3 RUN-001）：手动 / 定时 / 立即运行共用 run_id 注册表，
    // 生产装配 EngineExecutor 直连 Runner + DeviceManager
    let runner = Arc::new(engine::Runner::new(
        devices.clone(),
        viewers.clone(),
        scripts.clone(),
    ));
    let executor = Arc::new(run_manager::EngineExecutor::new(
        runner,
        devices.clone(),
        db.clone(),
    ));
    let runs = Arc::new(run_manager::RunManager::new(executor));

    // 调度器：cron 定时任务（执行经 RunManager 统一仲裁）
    let scheduler = Arc::new(scheduler::Scheduler::new(
        db.clone(),
        scripts.clone(),
        runs.clone(),
    ));
    scheduler.start().await;

    // HTTP + WebSocket API（停机信号已在上方创建，由 /api/shutdown 触发）
    let app = api::build_router(
        db,
        devices,
        runs,
        scheduler,
        cfg.clone(),
        viewers,
        scripts,
        shutdown_tx,
        auth,
    );
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
                let db = db.clone();
                match tokio::task::spawn_blocking(move || db.prune_logs(retain_days)).await {
                    Ok(Ok(deleted)) if deleted > 0 => {
                        info!(deleted, retain_days, "periodic run log cleanup removed expired rows");
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => warn!(error = %e, "periodic run log cleanup failed"),
                    Err(e) => warn!(error = %e, "run log cleanup worker failed"),
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
