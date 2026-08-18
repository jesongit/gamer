//! GameBot 游戏自动化助手服务端
//!
//! 架构：设备通过 adb 接入，服务端作为 scrcpy 客户端（官方 scrcpy-server）
//! 采集 H.264 视频 + 注入控制；视频流经 WebRTC 转推浏览器；
//! 模板匹配 / YAML 自动化 / 定时任务全部在服务端执行。

mod api;
mod config;
mod device;
mod engine;
mod matcher;
mod scheduler;
mod store;
mod webrtc;

use std::sync::Arc;

use tokio::net::TcpListener;
use tracing::info;

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

    // 日志：默认 stdout；设置 GB_LOG=<文件路径> 时写入文件（追加模式）。
    // 文件模式用于生产部署——不依赖 shell 重定向管道，
    // 避免"重定向句柄异常导致进程假死/日志丢失"的问题。
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info".into());
    if let Ok(path) = std::env::var("GB_LOG") {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("cannot open GB_LOG {}: {}", path, e));
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::sync::Mutex::new(file))
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    let cfg = config::Config::load()?;
    info!("GameBot server v{} starting...", env!("CARGO_PKG_VERSION"));
    info!("listen: {}", cfg.listen_addr());
    info!("data dir: {}", cfg.data_dir.display());

    let db = Arc::new(store::Store::open(&cfg)?);

    // 设备管理器：负责 adb 发现 + scrcpy 会话
    let devices = Arc::new(device::DeviceManager::new(db.clone(), cfg.clone()));
    devices.start().await?;

    // 调度器：cron 定时任务
    let scheduler = Arc::new(scheduler::Scheduler::new(db.clone(), devices.clone()));
    scheduler.start().await;

    // HTTP + WebSocket API
    let app = api::build_router(db, devices, scheduler, cfg.clone());
    let listener = TcpListener::bind(cfg.listen_addr()).await?;
    info!("GameBot server ready on http://{}", cfg.listen_addr());
    axum::serve(listener, app).await?;

    Ok(())
}
