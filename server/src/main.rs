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
mod scheduler;
mod scripts;
mod store;
mod webrtc;

use std::sync::Arc;

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

    // 脚本/模板按应用分区存储（data/<pkg>/yaml|tmpl）+ 旧目录布局一次性迁移
    let scripts = Arc::new(scripts::ScriptStore::open(&cfg)?);
    scripts::migrate_fs_layout(&db, &scripts)?;

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

    // 调度器：cron 定时任务
    let scheduler = Arc::new(scheduler::Scheduler::new(
        db.clone(),
        devices.clone(),
        viewers.clone(),
        scripts.clone(),
    ));
    scheduler.start().await;

    // HTTP + WebSocket API；优雅停机信号（POST /api/shutdown 拆完会话后触发）
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let app = api::build_router(
        db,
        devices,
        scheduler,
        cfg.clone(),
        viewers,
        scripts,
        shutdown_tx,
    );
    let listener = TcpListener::bind(cfg.listen_addr()).await?;
    info!("GameBot server ready on http://{}", cfg.listen_addr());
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.changed().await;
            info!("graceful shutdown: http server draining");
        })
        .await?;
    info!("server exited");

    Ok(())
}
