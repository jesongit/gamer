//! 统一停机协调器（OPS-001 / 计划 §7.2）。
//!
//! 三种触发源共用同一条 drain 入口：
//! - API 请求（`POST /api/shutdown`，`api::system::api_shutdown`）；
//! - Ctrl+C（`tokio::signal::ctrl_c`，全平台）；
//! - SIGTERM（`cfg(unix)`，容器 `docker stop` 路径）。
//!
//! drain 序列即原 `/api/shutdown` 内联的会话拆解（[`drain_sessions`]）：
//! RunManager drain（拒绝新 run → 等待/超时强停活动任务）→ 踢全部 viewer
//! （关 WebRTC peer，让 ws 循环退出，axum graceful 才等得到收尾）→ 拆全部
//! scrcpy 会话/清 reverse 隧道（防孤儿 adb）。
//!
//! 一次性语义：首次触发执行完整 drain（会话拆解完成后才点亮 watch 停机
//! 信号——axum graceful 与周期任务随之收尾）；后续触发不再重复执行，而是
//! 等待首次完成后以 Finished 返回。状态机 `Running → Draining → Finished`
//! 可随时查询（状态查询/幂等暴露属 OPS-002 扩展面，本批次交付协调器本体）。

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::sync::{watch, Mutex};
use tracing::info;

use crate::device::DeviceManager;
use crate::run_manager::RunManager;
use crate::webrtc::ViewerMap;

/// RunManager drain 的会话宽限（与原 /api/shutdown 一致：活动脚本短则提前返回）
const DRAIN_GRACE: Duration = Duration::from_secs(10);

/// 停机状态机
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownState {
    /// 正常运行
    Running,
    /// drain 进行中（会话拆解未完成）
    Draining,
    /// drain 完成（watch 停机信号已点亮）
    Finished,
}

impl ShutdownState {
    /// 状态查询暴露（OPS-002：GET /health/shutdown 匿名轻量端点消费）
    pub fn as_str(self) -> &'static str {
        match self {
            ShutdownState::Running => "running",
            ShutdownState::Draining => "draining",
            ShutdownState::Finished => "finished",
        }
    }
}

/// 会话拆解序列（原 `/api/shutdown` 内联逻辑抽出，协调器统一驱动）：
/// ① RunManager drain；② 踢全部 viewer；③ 拆全部 scrcpy 会话/清 reverse 隧道。
pub async fn drain_sessions(
    runs: Arc<RunManager>,
    viewers: ViewerMap,
    devices: Arc<DeviceManager>,
) {
    // ① RunManager drain（宽限 10s；拒绝新 run，等待活动任务结束，超时强停）
    runs.begin_shutdown(DRAIN_GRACE).await;
    // ② 踢 viewer：关 WebRTC peer（ws 循环随 peer_closed 退出），否则常驻 WS
    //    连接会让 axum 的 graceful drain 一直等不到收尾。只关 peer 不发
    //    taken_over——那是"被顶替"信号会让页面放弃自动重连；普通断开页面
    //    会在服务重启后自动重连。
    let viewers = viewers.lock().unwrap().clone();
    for (id, vh) in &viewers {
        info!(device = %id, "shutdown: closing viewer peer");
        vh.running.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(peer) = vh.peer.upgrade() {
            let _ = peer.close().await;
        }
    }
    // ③ 拆所有 scrcpy 会话/清 reverse 隧道（防孤儿 adb 楔死后续连接）
    devices.shutdown_all().await;
}

/// drain 闭包：返回 boxed future（捕获 runs/viewers/devices 克隆）。
/// 协调器只在首次触发时调用一次。
pub type DrainFn = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

/// 统一停机协调器：唯一 drain 入口 + 一次性语义 + 状态查询。
pub struct ShutdownCoordinator {
    /// 停机信号（drain 完成后 send(true)；axum graceful 与周期任务挂在其
    /// receiver 上，语义与既有 watch 通道完全一致）
    shutdown_tx: watch::Sender<bool>,
    drain: DrainFn,
    /// 串行化 drain：并发触发只有一个执行，其余在锁上等待首次完成后 no-op
    drain_slot: Mutex<()>,
    drained: AtomicBool,
    state: AtomicU8,
}

const STATE_RUNNING: u8 = 0;
const STATE_DRAINING: u8 = 1;
const STATE_FINISHED: u8 = 2;

impl ShutdownCoordinator {
    /// `drain` 即 [`drain_sessions`] 的参数化包装（由 main 装配具体依赖）
    pub fn new(drain: DrainFn) -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        Self {
            shutdown_tx,
            drain,
            drain_slot: Mutex::new(()),
            drained: AtomicBool::new(false),
            state: AtomicU8::new(STATE_RUNNING),
        }
    }

    /// 当前停机状态（可观测查询）
    pub fn state(&self) -> ShutdownState {
        match self.state.load(Ordering::SeqCst) {
            STATE_DRAINING => ShutdownState::Draining,
            STATE_FINISHED => ShutdownState::Finished,
            _ => ShutdownState::Running,
        }
    }

    /// 订阅停机信号（drain 完成后收到 `true`）
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    /// 触发停机：首个调用执行完整 drain 并返回 Finished；后续（并发或重复）
    /// 触发等待首次完成后直接返回（no-op，不重复拆解）。
    pub async fn request(&self) -> ShutdownState {
        let _slot = self.drain_slot.lock().await;
        if self.drained.load(Ordering::SeqCst) {
            return self.state();
        }
        self.state.store(STATE_DRAINING, Ordering::SeqCst);
        info!("shutdown coordinator: draining (runs, viewers, device sessions)");
        (self.drain)().await;
        // 与既有语义一致：会话拆解完成后才点亮 watch 信号
        let _ = self.shutdown_tx.send(true);
        self.drained.store(true, Ordering::SeqCst);
        self.state.store(STATE_FINISHED, Ordering::SeqCst);
        info!("shutdown coordinator: finished");
        self.state()
    }
}

/// 安装信号监听（main 启动期调用一次）：Ctrl+C 全平台，SIGTERM 仅 unix。
/// 首个信号经协调器触发 drain；重复信号由协调器一次性语义吸收。
pub fn spawn_signal_listener(coordinator: Arc<ShutdownCoordinator>) {
    tokio::spawn(async move {
        wait_for_signal().await;
        info!("shutdown signal received; requesting coordinated drain");
        coordinator.request().await;
    });
}

/// 等待首个停机信号。unix 上同时监听 SIGTERM；SIGTERM handler 安装失败时
/// 降级为仅 Ctrl+C（不阻断启动）。
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::SignalKind;
        match tokio::signal::unix::signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => info!("shutdown signal: ctrl+c"),
                    _ = sigterm.recv() => info!("shutdown signal: SIGTERM"),
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot install SIGTERM handler; ctrl+c only");
                let _ = tokio::signal::ctrl_c().await;
                info!("shutdown signal: ctrl+c");
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        info!("shutdown signal: ctrl+c");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn counting_coordinator(counter: Arc<AtomicUsize>, delay: Duration) -> ShutdownCoordinator {
        ShutdownCoordinator::new(Arc::new(move || {
            let counter = counter.clone();
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                counter.fetch_add(1, Ordering::SeqCst);
            })
        }))
    }

    #[tokio::test]
    async fn state_transitions_running_draining_finished() {
        let counter = Arc::new(AtomicUsize::new(0));
        let coordinator = Arc::new(counting_coordinator(
            counter.clone(),
            Duration::from_millis(200),
        ));
        assert_eq!(coordinator.state(), ShutdownState::Running);

        // drain 进行中可观测为 Draining（通过并发任务采样）
        let sampler = coordinator.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            sampler.state()
        });
        let done = coordinator.request().await;
        assert_eq!(done, ShutdownState::Finished);
        let sampled = handle.await.unwrap();
        assert_eq!(sampled, ShutdownState::Draining, "drain 进行中应可观测");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn repeated_and_concurrent_triggers_are_idempotent() {
        let counter = Arc::new(AtomicUsize::new(0));
        let coordinator = Arc::new(counting_coordinator(
            counter.clone(),
            Duration::from_millis(80),
        ));

        // 并发三路触发：drain 只执行一次
        let c1 = coordinator.clone();
        let c2 = coordinator.clone();
        let c3 = coordinator.clone();
        let handles = [
            tokio::spawn(async move { c1.request().await }),
            tokio::spawn(async move { c2.request().await }),
            tokio::spawn(async move { c3.request().await }),
        ];
        for handle in handles {
            assert_eq!(handle.await.unwrap(), ShutdownState::Finished);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1, "drain 必须只执行一次");

        // 完成后再次触发：no-op
        coordinator.request().await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert_eq!(coordinator.state(), ShutdownState::Finished);
    }

    /// OPS-002：draining 状态下的重复触发——不拒绝、不重入、不报错，等待
    /// 首次 drain 完成后以 Finished 返回；状态机全程可观测
    #[tokio::test]
    async fn request_during_drain_waits_without_reentering() {
        let counter = Arc::new(AtomicUsize::new(0));
        let coordinator = Arc::new(counting_coordinator(
            counter.clone(),
            Duration::from_millis(150),
        ));

        let first = tokio::spawn({
            let coordinator = coordinator.clone();
            async move { coordinator.request().await }
        });
        // 等首次触发进入 drain
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(coordinator.state(), ShutdownState::Draining);
        assert_eq!(coordinator.state().as_str(), "draining");

        // draining 期间的重复请求被一次性语义吸收（挂起等待而非并发执行）
        let second = tokio::spawn({
            let coordinator = coordinator.clone();
            async move { coordinator.request().await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "drain 未完成前不得重复执行或并行进入"
        );
        assert_eq!(coordinator.state(), ShutdownState::Draining);

        assert_eq!(second.await.unwrap(), ShutdownState::Finished);
        assert_eq!(first.await.unwrap(), ShutdownState::Finished);
        assert_eq!(counter.load(Ordering::SeqCst), 1, "drain 全程只执行一次");
        assert_eq!(coordinator.state(), ShutdownState::Finished);
        assert_eq!(coordinator.state().as_str(), "finished");
    }

    #[tokio::test]
    async fn watch_signal_fires_only_after_drain_completes() {
        let counter = Arc::new(AtomicUsize::new(0));
        let coordinator = counting_coordinator(counter, Duration::from_millis(60));
        let mut rx = coordinator.subscribe();
        let requester = tokio::spawn(async move { coordinator.request().await });

        // drain 完成前信号必须未点亮
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!*rx.borrow(), "drain 完成前不得点亮停机信号");
        let changed = tokio::time::timeout(Duration::from_secs(2), rx.changed()).await;
        assert!(changed.is_ok(), "drain 完成后应点亮停机信号");
        assert!(*rx.borrow_and_update());
        assert_eq!(requester.await.unwrap(), ShutdownState::Finished);
    }
}
