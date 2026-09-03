//! 业务空闲摘要（OPS-005）：update 安装门禁与 auto 协调器的统一输入。
//!
//! 聚合四个维度：
//! - `active_runs`：RunManager 中 starting/running/stopping 的活动运行数；
//! - `viewers`：活跃 WebRTC viewer 数（auto 安装的软门禁——等待不强制）；
//! - `update_transactions`：进行中的升级/回滚/备份/迁移事务（当前形态 =
//!   update 协调器自身事务标志；文件迁移框架为纯库代码，接线后在此并入）；
//! - `next_cron_secs`：距下一次**启用** cron 任务触发的秒数（禁用任务不计；
//!   None = 无启用任务）。
//!
//! 暴露两条路：① [`Workload::install_blockings`] 产出 §4.3 门禁 `blocking`
//! 数组（SYS-004 手动 install 409 详情）；② [`Workload::auto_install_ready`]
//! 供 auto 协调器判定"维护窗口内 + 全空闲 + 距下一 cron > 冻结窗口"。

use std::sync::Arc;

use crate::run_manager::RunManager;
use crate::scheduler::Scheduler;
use crate::update::model::InstallBlocking;
use crate::webrtc::ViewerMap;

/// 一次空闲快照（纯数据，单测直接构造合成各维度）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Workload {
    pub active_runs: usize,
    pub viewers: usize,
    pub update_transactions: usize,
    /// 距下一次启用 cron 触发的秒数；None = 无启用任务
    pub next_cron_secs: Option<i64>,
}

impl Workload {
    /// §4.3 install 门禁中由 workload 判定的子集（staging/launcher/space
    /// 来自状态机与 controller，由调用方并集）。`freeze_minutes` = 策略冻结
    /// 窗口；门禁要求距下一次启用 cron 触发**大于**该值（§6：≤ 即阻塞）。
    pub fn install_blockings(&self, freeze_minutes: i64) -> Vec<InstallBlocking> {
        let mut out = Vec::new();
        if self.active_runs > 0 {
            out.push(InstallBlocking::ActiveRun);
        }
        if self.update_transactions > 0 {
            out.push(InstallBlocking::UpdateTransaction);
        }
        if cron_in_freeze(self.next_cron_secs, freeze_minutes) {
            out.push(InstallBlocking::CronFreezeWindow);
        }
        out
    }

    /// auto 协调器安装判定的空闲部分（viewer 在线也等待——软门禁，绝不强制）。
    /// `in_window` = 当前是否处于维护窗口（策略判定，由调用方传入）。
    pub fn auto_install_ready(&self, in_window: bool, freeze_minutes: i64) -> bool {
        in_window
            && self.active_runs == 0
            && self.viewers == 0
            && self.update_transactions == 0
            && !cron_in_freeze(self.next_cron_secs, freeze_minutes)
    }
}

/// 距下一次启用 cron 触发 ≤ 冻结窗口 → 阻塞（无 cron 任务 = 永不阻塞）
fn cron_in_freeze(next_cron_secs: Option<i64>, freeze_minutes: i64) -> bool {
    match next_cron_secs {
        None => false,
        Some(secs) => secs <= freeze_minutes * 60,
    }
}

/// 生产装配的快照来源：从 RunManager / viewers / 协调器 / Scheduler 取实时值。
#[derive(Clone)]
pub struct WorkloadSource {
    runs: Arc<RunManager>,
    viewers: ViewerMap,
    scheduler: Arc<Scheduler>,
    /// 协调器自身的升级事务标志（Some(busy) 的克隆闭包）
    update_busy: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl WorkloadSource {
    pub fn new(
        runs: Arc<RunManager>,
        viewers: ViewerMap,
        scheduler: Arc<Scheduler>,
        update_busy: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self {
            runs,
            viewers,
            scheduler,
            update_busy,
        }
    }

    /// 采集当前快照（下一次 cron 时间以本地时区的 `now` 为基准计算）
    pub fn snapshot(&self) -> Workload {
        Workload {
            active_runs: self.runs.active_count(),
            viewers: self.viewers.lock().map(|v| v.len()).unwrap_or_default(),
            update_transactions: usize::from((self.update_busy)()),
            next_cron_secs: self.scheduler.next_enabled_trigger_in_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{ActivityLease, RunContext, RunRequest};
    use crate::run_manager::{RunExecutor, RunSource, RunState, StartRequest};
    use crate::store::{Db, Store, Task};
    use crate::update::service::UpdateTxn;
    use futures_util::future::BoxFuture;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex as StdMutex;

    struct HangingExecutor;

    impl RunExecutor for HangingExecutor {
        fn prepare<'a>(
            &'a self,
            _context: &'a RunContext,
            _request: &'a RunRequest,
        ) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn execute<'a>(
            &'a self,
            _context: &'a RunContext,
            _request: &'a RunRequest,
            _realtime_logs: bool,
            stop: Arc<AtomicBool>,
        ) -> BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
            Box::pin(async move {
                while !stop.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
                Ok(Vec::new())
            })
        }

        fn acquire(&self, _context: &RunContext) -> anyhow::Result<Box<dyn ActivityLease>> {
            Ok(Box::new(crate::core::NoopLease))
        }
    }

    fn wl(runs: usize, viewers: usize, txns: usize, cron: Option<i64>) -> Workload {
        Workload {
            active_runs: runs,
            viewers,
            update_transactions: txns,
            next_cron_secs: cron,
        }
    }

    #[test]
    fn install_blockings_cover_each_dimension_independently() {
        let empty = wl(0, 0, 0, None);
        assert!(empty.install_blockings(30).is_empty());

        let runs_only = wl(2, 0, 0, None);
        assert_eq!(
            runs_only.install_blockings(30),
            vec![InstallBlocking::ActiveRun]
        );

        let txn_only = wl(0, 3, 1, None);
        assert_eq!(
            txn_only.install_blockings(30),
            vec![InstallBlocking::UpdateTransaction]
        );

        let cron_only = wl(0, 0, 0, Some(30 * 60));
        assert_eq!(
            cron_only.install_blockings(30),
            vec![InstallBlocking::CronFreezeWindow]
        );

        // 全部命中：blocking 列出全部未满足项（§4.3）
        let all = wl(1, 1, 1, Some(60));
        assert_eq!(
            all.install_blockings(30),
            vec![
                InstallBlocking::ActiveRun,
                InstallBlocking::UpdateTransaction,
                InstallBlocking::CronFreezeWindow,
            ]
        );
    }

    #[test]
    fn cron_freeze_boundary_is_less_than_or_equal() {
        // 契约：安装门禁要求距下一次启用 cron 触发 > 冻结窗口（§6）
        // 恰好等于窗口 → 阻塞；大 1 秒 → 放行；无 cron → 放行
        let edge = wl(0, 0, 0, Some(30 * 60));
        assert!(edge
            .install_blockings(30)
            .contains(&InstallBlocking::CronFreezeWindow));
        let clear = wl(0, 0, 0, Some(30 * 60 + 1));
        assert!(!clear
            .install_blockings(30)
            .contains(&InstallBlocking::CronFreezeWindow));
        let none = wl(0, 0, 0, None);
        assert!(!none
            .install_blockings(30)
            .contains(&InstallBlocking::CronFreezeWindow));
        // freeze=0：只有已经到点的触发（0s）阻塞；未来 1s 的触发不在窗口内。
        let scheduled = wl(0, 0, 0, Some(0));
        assert!(scheduled
            .install_blockings(0)
            .contains(&InstallBlocking::CronFreezeWindow));
    }

    #[test]
    fn auto_install_ready_requires_window_and_full_idle() {
        // 窗口外一律不安装
        assert!(!wl(0, 0, 0, None).auto_install_ready(false, 30));
        // 窗口内全空闲 + 无 cron 临近 → 就绪
        assert!(wl(0, 0, 0, None).auto_install_ready(true, 30));
        // viewer 在线即等待（软门禁：等待不强制，绝不打断）
        assert!(!wl(0, 1, 0, None).auto_install_ready(true, 30));
        // run / 事务 / cron 临边任一存在 → 等待
        assert!(!wl(1, 0, 0, None).auto_install_ready(true, 30));
        assert!(!wl(0, 0, 1, None).auto_install_ready(true, 30));
        assert!(!wl(0, 0, 0, Some(29 * 60)).auto_install_ready(true, 30));
        assert!(wl(0, 0, 0, Some(31 * 60)).auto_install_ready(true, 30));
    }

    #[test]
    fn manual_install_gate_ignores_viewers() {
        // viewer 不是硬门禁（§4.3 六项枚举不含 viewer）：手动 install 不因
        // viewer 在线而拒绝，由协调器经优雅停机链路处理
        let busy_viewer = wl(0, 2, 0, None);
        assert!(busy_viewer.install_blockings(30).is_empty());
    }

    /// OPS-005：生产 WorkloadSource 同时聚合 RunManager、viewer 注册表、升级
    /// 事务标志和 Scheduler 的启用 cron；禁用任务/非法 cron 不应污染摘要。
    #[tokio::test]
    async fn source_snapshot_aggregates_active_run_viewer_transaction_and_cron() {
        let dir = std::env::temp_dir().join(format!(
            "gamer-workload-ops005-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = crate::config::Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let db: Db = Arc::new(Store::open(&cfg).unwrap());
        let scripts = Arc::new(crate::scripts::ScriptStore::open(&cfg).unwrap());
        let runs = Arc::new(RunManager::new(Arc::new(HangingExecutor)));
        let scheduler = Arc::new(Scheduler::new(db.clone(), scripts, runs.clone()));

        let now = chrono::Local::now();
        let enabled = Task {
            id: "enabled".into(),
            name: "Enabled cron".into(),
            cron: "*/5 * * * *".into(),
            script_id: "pkg/script.yaml".into(),
            device_id: "device-1".into(),
            enabled: true,
            last_result: None,
            last_run_at: None,
            created_at: now.to_rfc3339(),
            args_json: "{}".into(),
            param_signature: "psig1|".into(),
        };
        let disabled = Task {
            id: "disabled".into(),
            name: "Disabled invalid cron".into(),
            cron: "not-a-cron".into(),
            enabled: false,
            ..enabled.clone()
        };
        db.upsert_task(&enabled).unwrap();
        db.upsert_task(&disabled).unwrap();

        let app = crate::core::AppContext::from_legacy_package("device-1", "pkg").unwrap();
        let request = RunRequest::for_app(
            app,
            "test.runner",
            "pkg/script.yaml",
            crate::core::RunPayload::empty(),
        )
        .unwrap();
        let run = runs
            .submit(
                StartRequest {
                    request,
                    source: RunSource::Scheduled,
                    task_id: Some("enabled".into()),
                    scheduled_at: Some(now.timestamp()),
                    realtime_logs: false,
                },
                None,
            )
            .unwrap();

        let viewers: ViewerMap = Arc::new(StdMutex::new(std::collections::HashMap::new()));
        let (control_tx, control_rx) = tokio::sync::mpsc::unbounded_channel();
        drop(control_rx);
        viewers.lock().unwrap().insert(
            "device-1".into(),
            crate::webrtc::ViewerHandle {
                running: Arc::new(AtomicBool::new(true)),
                peer: std::sync::Weak::new(),
                control_dc: Arc::new(Mutex::new(None)),
                viewer_id: "viewer-1".into(),
                last_serve: Arc::new(std::sync::atomic::AtomicI64::new(0)),
                notify: Arc::new(Mutex::new(None)),
                control_tx,
                activity_lease: None,
            },
        );
        let transaction_active = Arc::new(AtomicBool::new(true));
        let transaction_for_source = transaction_active.clone();
        let source = WorkloadSource::new(
            runs.clone(),
            viewers,
            scheduler,
            Arc::new(move || transaction_for_source.load(Ordering::SeqCst)),
        );

        let snapshot = source.snapshot();
        assert_eq!(snapshot.active_runs, 1);
        assert_eq!(snapshot.viewers, 1);
        assert_eq!(snapshot.update_transactions, 1);
        assert!(matches!(snapshot.next_cron_secs, Some(secs) if (0..=300).contains(&secs)));
        assert_eq!(
            runs.active_for_device("device-1").unwrap().state,
            RunState::Starting
        );

        runs.cancel(&run.run_id);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while runs.active_count() != 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "run cleanup did not settle"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        transaction_active.store(false, Ordering::SeqCst);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// QA-006：并发 install/rollback 共享同一个原子事务门禁，多个竞争者中只能
    /// 有一个持有事务，其余请求必须在受理前失败；持有者释放后才可再次受理。
    #[tokio::test]
    async fn update_transaction_gate_accepts_exactly_one_concurrent_holder() {
        const COMPETITORS: usize = 16;
        let txn = Arc::new(UpdateTxn::default());
        let barrier = Arc::new(tokio::sync::Barrier::new(COMPETITORS));
        let winners = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut tasks = Vec::with_capacity(COMPETITORS);

        for _ in 0..COMPETITORS {
            let txn = txn.clone();
            let barrier = barrier.clone();
            let winners = winners.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                let won = txn.try_begin();
                if won {
                    winners.fetch_add(1, Ordering::SeqCst);
                }
                // 让所有竞争者完成 try_begin 后再释放，避免测试本身制造第二个赢家。
                barrier.wait().await;
                if won {
                    txn.end();
                }
                won
            }));
        }

        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(winners.load(Ordering::SeqCst), 1);
        assert!(!txn.is_active());
        assert!(txn.try_begin());
        assert!(!txn.try_begin());
        txn.end();
    }
}
