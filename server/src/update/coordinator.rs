//! 更新策略协调器（SYS-005 / 计划 §6.5）。
//!
//! 后台周期评估当前策略与空闲状况：
//! - `off`：不检查（capability 不变，仅无任何自动行为——契约 §6）；
//! - `notify`（产品默认）：自动检查 + 后台下载，**只下载不装**（安装等用户确认）；
//! - `auto`：检查 + 下载；候选 staged 后在**维护窗口内 + 全空闲**（无活动运行、
//!   无 viewer、无升级事务、距下一次启用 cron 触发 > 冻结窗口）时触发安装；
//!   活动运行 / viewer / cron 临边 / busy → 只等待，绝不中断业务（QA-006）。
//!
//! 时钟可注入（单测：窗口内外 / 跨午夜 / cron 临边 / busy 等待）。检查节奏
//! 有内部下限（不冻结于契约；launcher 侧才是真正的远端检查执行者）。

use std::sync::Arc;
use std::time::Duration;

use chrono::Timelike;

use super::model::UpdateState;
use super::policy::UpdateStrategy;
use super::service::UpdateService;
use super::workload::Workload;

/// 评估周期（30s：窗口/空闲判定粒度；cron 临边安全边际由冻结窗口分钟级吸收）
pub const EVAL_INTERVAL: Duration = Duration::from_secs(30);
/// 自动检查最短间隔（防止每 30s 打一次 launcher check；launcher 侧有自己的
/// 远端节奏，这里只是不上频的保守下限）
pub const AUTO_CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// 可注入时钟（本地时区的「当日分钟数」即维护窗口判定的全部时间需求）
pub trait Clock: Send + Sync {
    fn now_minutes_of_day(&self) -> i64;
    /// 单调基准（自动检查节流用；测试可注入固定值）
    fn monotonic(&self) -> std::time::Instant {
        std::time::Instant::now()
    }
}

/// 生产时钟：本地时间（维护窗口为本地 HH:MM，契约 §6 冻结语义）
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_minutes_of_day(&self) -> i64 {
        let t = chrono::Local::now().time();
        t.hour() as i64 * 60 + t.minute() as i64
    }
}

/// 一次评估的决策（纯函数输出；执行在 [`Coordinator::tick`]）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    Noop,
    Check,
    Download,
    Install,
}

/// 策略评估（纯函数）：窗口/空闲/cron 临边/busy 全部在此收敛。
/// `in_window` 由策略对象判定（[`UpdatePolicy::in_maintenance_window`]），
/// `state` 为当前缓存状态，`workload` 为实时空闲快照。
pub fn decide(
    strategy: UpdateStrategy,
    in_window: bool,
    state: UpdateState,
    workload: &Workload,
    freeze_minutes: i64,
) -> Tick {
    match strategy {
        // off：不检查（更不下载/安装）
        UpdateStrategy::Off => Tick::Noop,
        UpdateStrategy::Notify => match state {
            // 自动检查 + 后台下载；安装只等用户确认
            UpdateState::Idle | UpdateState::Failed => Tick::Check,
            UpdateState::Available => Tick::Download,
            _ => Tick::Noop,
        },
        UpdateStrategy::Auto => match state {
            UpdateState::Idle | UpdateState::Failed => Tick::Check,
            UpdateState::Available => Tick::Download,
            // staged/waiting → 窗口内 + 全空闲才安装；其余只等待
            UpdateState::Staged | UpdateState::Waiting => {
                if workload.auto_install_ready(in_window, freeze_minutes) {
                    Tick::Install
                } else {
                    Tick::Noop
                }
            }
            _ => Tick::Noop,
        },
    }
}

/// 协调器：后台周期任务 + 可测试的单步评估（[`Coordinator::tick`]）
pub struct Coordinator {
    service: Arc<UpdateService>,
    clock: Arc<dyn Clock>,
    last_check: parking_lot::Mutex<Option<std::time::Instant>>,
}

impl Coordinator {
    pub fn new(service: Arc<UpdateService>, clock: Arc<dyn Clock>) -> Self {
        Self {
            service,
            clock,
            last_check: parking_lot::Mutex::new(None),
        }
    }

    /// 生产装配：tokio 后台任务（跟随进程生命周期；install 受理本身有门禁与
    /// 事务兜底，drain 期间多跑一轮评估无副作用）
    pub fn spawn(service: Arc<UpdateService>) {
        let coordinator = Arc::new(Self::new(service, Arc::new(SystemClock)));
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(EVAL_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                coordinator.tick().await;
            }
        });
    }

    /// 单步评估（可测试）：策略 → 决策 → 执行。返回实际决策（诊断/断言用）。
    pub async fn tick(&self) -> Tick {
        let policy = self.service.current_policy().await;
        let state = match self.service.refresh_state().await {
            Ok(state) => state,
            Err(error) => {
                // Keep the last known journal state when the launcher is
                // temporarily unreachable. Action requests still surface
                // launcher_unreachable, while the next evaluation retries.
                tracing::debug!(code = error.code.as_str(), "update status refresh failed");
                self.service.cached_state()
            }
        };
        let workload = self.service.workload_snapshot();
        let decision = decide(
            policy.strategy,
            policy.in_maintenance_window(self.clock.now_minutes_of_day()),
            state,
            &workload,
            policy.freeze_minutes,
        );
        match decision {
            Tick::Noop => {}
            Tick::Check => {
                // 检查节奏下限：空闲期不上频打 launcher
                let due = {
                    let mut last = self.last_check.lock();
                    let due = match *last {
                        Some(t) => t.elapsed() >= AUTO_CHECK_INTERVAL,
                        None => true,
                    };
                    if due {
                        *last = Some(self.clock.monotonic());
                    }
                    due
                };
                if !due {
                    return Tick::Noop;
                }
                if let Err(e) = self.service.request_check().await {
                    tracing::debug!(
                        code = e.code.as_str(),
                        "auto check not accepted (will retry)"
                    );
                }
            }
            Tick::Download => {
                if let Err(e) = self.service.request_download().await {
                    tracing::debug!(
                        code = e.code.as_str(),
                        "auto download not accepted (will retry)"
                    );
                }
            }
            // auto 安装：与手动 install 同一条门禁/事务路径；被 busy/门禁拒绝
            // 都是「等待」语义（绝不中断业务），下一轮再评
            Tick::Install => match self.service.request_install().await {
                Ok(_) => {
                    tracing::info!(
                        "auto strategy: maintenance install accepted, handing off to launcher"
                    );
                }
                Err(e) => {
                    tracing::debug!(
                        code = e.code.as_str(),
                        "auto install gate rejected (waiting)"
                    );
                    return Tick::Noop;
                }
            },
        }
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Db;
    use crate::update::controller::mock::MockController;
    use crate::update::ipc::{Candidate, LauncherUpdateStatus};
    use crate::update::policy::{PolicyStore, UpdatePolicy};
    use crate::update::service::{UpdateService, UpdateTxn, WorkloadProvider};
    use crate::update::workload::Workload;

    fn wl(runs: usize, viewers: usize, txns: usize, cron: Option<i64>) -> Workload {
        Workload {
            active_runs: runs,
            viewers,
            update_transactions: txns,
            next_cron_secs: cron,
        }
    }

    fn candidate() -> Candidate {
        Candidate {
            version: "0.3.0".into(),
            channel: "stable".into(),
            published_at: None,
            size_bytes: None,
            release_notes_url: None,
        }
    }

    fn staged_status() -> LauncherUpdateStatus {
        LauncherUpdateStatus {
            state: Some(UpdateState::Staged),
            detail: Some("staged".into()),
            update_id: Some("upd-coord-1".into()),
            candidate: Some(candidate()),
            progress: None,
            last_error: None,
        }
    }

    struct FixedClock(i64);
    impl Clock for FixedClock {
        fn now_minutes_of_day(&self) -> i64 {
            self.0
        }
    }

    async fn service_with(
        controller: Arc<MockController>,
        policy: UpdatePolicy,
    ) -> (Arc<UpdateService>, Arc<UpdateTxn>, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("gamer-coord-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = crate::config::Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let db: Db = Arc::new(crate::store::Store::open(&cfg).unwrap());
        let policy_store = PolicyStore::load_blocking(&cfg.data_dir, policy);
        let workload: WorkloadProvider = Arc::new(|| wl(0, 0, 0, None));
        let txn = Arc::new(UpdateTxn::default());
        (
            Arc::new(UpdateService::new(
                controller,
                policy_store,
                txn.clone(),
                workload,
                db,
            )),
            txn,
            dir,
        )
    }

    fn auto_policy() -> UpdatePolicy {
        UpdatePolicy {
            strategy: UpdateStrategy::Auto,
            ..Default::default()
        }
    }

    fn notify_policy() -> UpdatePolicy {
        UpdatePolicy {
            strategy: UpdateStrategy::Notify,
            ..Default::default()
        }
    }

    // ---------- decide 纯函数矩阵（SYS-005 注入时钟场景） ----------

    #[test]
    fn off_strategy_never_acts() {
        let idle = wl(0, 0, 0, None);
        for state in [
            UpdateState::Idle,
            UpdateState::Available,
            UpdateState::Staged,
            UpdateState::Waiting,
        ] {
            assert_eq!(
                decide(UpdateStrategy::Off, true, state, &idle, 30),
                Tick::Noop
            );
        }
    }

    #[test]
    fn notify_checks_and_downloads_but_never_installs() {
        let idle = wl(0, 0, 0, None);
        assert_eq!(
            decide(UpdateStrategy::Notify, false, UpdateState::Idle, &idle, 30),
            Tick::Check
        );
        assert_eq!(
            decide(
                UpdateStrategy::Notify,
                false,
                UpdateState::Failed,
                &idle,
                30
            ),
            Tick::Check
        );
        assert_eq!(
            decide(
                UpdateStrategy::Notify,
                false,
                UpdateState::Available,
                &idle,
                30
            ),
            Tick::Download
        );
        // 窗口内全空闲也不安装（notify 只下载不装）
        assert_eq!(
            decide(UpdateStrategy::Notify, true, UpdateState::Staged, &idle, 30),
            Tick::Noop
        );
        assert_eq!(
            decide(
                UpdateStrategy::Notify,
                true,
                UpdateState::Waiting,
                &idle,
                30
            ),
            Tick::Noop
        );
    }

    #[test]
    fn auto_install_requires_window_and_full_idle() {
        let idle = wl(0, 0, 0, None);
        // 窗口外不装
        assert_eq!(
            decide(UpdateStrategy::Auto, false, UpdateState::Staged, &idle, 30),
            Tick::Noop
        );
        // 窗口内全空闲 → 安装
        assert_eq!(
            decide(UpdateStrategy::Auto, true, UpdateState::Staged, &idle, 30),
            Tick::Install
        );
        assert_eq!(
            decide(UpdateStrategy::Auto, true, UpdateState::Waiting, &idle, 30),
            Tick::Install
        );
        // viewer / run / 事务任一存在 → 等待
        assert_eq!(
            decide(
                UpdateStrategy::Auto,
                true,
                UpdateState::Staged,
                &wl(0, 1, 0, None),
                30
            ),
            Tick::Noop
        );
        assert_eq!(
            decide(
                UpdateStrategy::Auto,
                true,
                UpdateState::Staged,
                &wl(1, 0, 0, None),
                30
            ),
            Tick::Noop
        );
        assert_eq!(
            decide(
                UpdateStrategy::Auto,
                true,
                UpdateState::Staged,
                &wl(0, 0, 1, None),
                30
            ),
            Tick::Noop
        );
    }

    #[test]
    fn auto_cron_edge_blocks_inside_freeze_window() {
        // 距下一次启用 cron 触发 ≤ 冻结窗口 → 等待；> 冻结窗口 → 安装
        assert_eq!(
            decide(
                UpdateStrategy::Auto,
                true,
                UpdateState::Staged,
                &wl(0, 0, 0, Some(30 * 60)),
                30
            ),
            Tick::Noop,
            "cron 临边（恰等于冻结窗口）不安装"
        );
        assert_eq!(
            decide(
                UpdateStrategy::Auto,
                true,
                UpdateState::Staged,
                &wl(0, 0, 0, Some(31 * 60)),
                30
            ),
            Tick::Install
        );
    }

    // ---------- tick 执行链（mock controller + 注入时钟） ----------

    #[tokio::test]
    async fn tick_installs_when_window_idle_and_staged() {
        let controller = Arc::new(MockController::new());
        controller.set_status(staged_status());
        let (service, _txn, dir) = service_with(controller.clone(), auto_policy()).await;
        let coordinator = Coordinator::new(service, Arc::new(FixedClock(3 * 60))); // 03:00 窗口内
        assert_eq!(coordinator.tick().await, Tick::Install);
        // install is deliberately handed to a background task so the HTTP
        // endpoint can return 202 before launcher preparation begins.
        tokio::task::yield_now().await;
        assert!(controller.calls().contains(&"prepare_install".to_string()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn tick_waits_outside_window_even_if_staged() {
        let controller = Arc::new(MockController::new());
        controller.set_status(staged_status());
        let (service, _txn, dir) = service_with(controller.clone(), auto_policy()).await;
        let coordinator = Coordinator::new(service, Arc::new(FixedClock(12 * 60))); // 正午窗口外
        assert_eq!(coordinator.tick().await, Tick::Noop);
        assert!(!controller.calls().contains(&"prepare_install".to_string()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn tick_waits_while_busy_transaction_held() {
        // busy（事务被占用）→ install 被拒 → 等待，不重复取得事务（单受理）
        let controller = Arc::new(MockController::new());
        controller.set_status(staged_status());
        let (service, txn, dir) = service_with(controller.clone(), auto_policy()).await;
        assert!(txn.try_begin(), "预占升级事务");
        let coordinator = Coordinator::new(service, Arc::new(FixedClock(3 * 60)));
        assert_eq!(coordinator.tick().await, Tick::Noop);
        assert!(
            !controller.calls().contains(&"prepare_install".to_string()),
            "busy 等待绝不触发第二次 prepare_install"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn tick_notify_downloads_available_never_installs_staged() {
        let controller = Arc::new(MockController::new());
        let (service, _txn, dir) = service_with(controller.clone(), notify_policy()).await;
        let coordinator = Coordinator::new(service, Arc::new(FixedClock(3 * 60)));

        // available → 自动下载
        controller.set_status(LauncherUpdateStatus {
            state: Some(UpdateState::Available),
            detail: Some("checked".into()),
            update_id: Some("upd-n1".into()),
            candidate: Some(candidate()),
            progress: None,
            last_error: None,
        });
        assert_eq!(coordinator.tick().await, Tick::Download);
        assert!(controller.calls().contains(&"download".to_string()));

        // staged → 不安装
        controller.set_status(staged_status());
        assert_eq!(coordinator.tick().await, Tick::Noop);
        assert!(!controller.calls().contains(&"prepare_install".to_string()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn tick_check_respects_pacing_floor() {
        let controller = Arc::new(MockController::new());
        let (service, _txn, dir) = service_with(controller.clone(), notify_policy()).await;
        let coordinator = Coordinator::new(service, Arc::new(FixedClock(3 * 60)));
        // 第一轮：idle → Check 决策，执行检查
        assert_eq!(coordinator.tick().await, Tick::Check);
        assert!(controller.calls().contains(&"check".to_string()));
        // 第二轮（紧邻）：决策仍 Check 但被节奏下限吸收
        assert_eq!(coordinator.tick().await, Tick::Noop);
        assert_eq!(
            controller.calls().iter().filter(|c| *c == "check").count(),
            1
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
