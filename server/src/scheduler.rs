//! Timer Core composition.
//!
//! Scheduling policy and runner implementations are registered adapters. The
//! scheduler itself only composes the generic timer service and exposes the
//! small compatibility façade used by the existing REST/update callers.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use tracing::info;

use crate::cron_extension::{next_enabled_trigger_in_secs, CronExtension};
#[allow(unused_imports)]
pub use crate::cron_extension::{next_run, normalize_cron, validate_cron};
use crate::run_manager::RunManager;
use crate::store::Db;
use crate::timer_core::{
    ScheduleExtension, ScheduleRegistry, TimerCore, TimerRunner, TimerRunnerFactory,
    TimerRunnerRegistry, TimerTask,
};

pub struct Scheduler {
    core: Arc<TimerCore>,
    runners: Arc<TimerRunnerRegistry>,
    schedules: Arc<ScheduleRegistry>,
}

impl Scheduler {
    pub(crate) fn new<A: TimerRunnerFactory>(db: Db, adapter: A, runs: Arc<RunManager>) -> Self {
        let runners = Arc::new(TimerRunnerRegistry::new());
        let runner = adapter.into_timer_runner(db.clone(), runs);
        runners
            .register(runner)
            .expect("the built-in timer runner must have a non-empty unique id");
        let schedules = Arc::new(ScheduleRegistry::new());
        schedules
            .register("cron", CronExtension::new())
            .expect("the built-in Cron schedule extension must register");
        Self {
            core: TimerCore::new(db),
            runners,
            schedules,
        }
    }

    /// Register an extension runner before or after the timer loop starts.
    /// Duplicate ids are rejected without changing the active runner.
    /// 扩展 runner/schedule 注册缝（Phase 9/10 预留）：当前内置 runner/Cron
    /// 已在 new() 直接注册，公开包装由后续扩展消费者使用。
    #[allow(dead_code)]
    pub fn register_runner(&self, runner: Arc<dyn TimerRunner>) -> anyhow::Result<()> {
        self.runners.register(runner)
    }

    #[allow(dead_code)]
    pub fn register_schedule(
        &self,
        kind: impl Into<String>,
        extension: Arc<dyn ScheduleExtension>,
    ) -> anyhow::Result<()> {
        self.schedules.register(kind, extension)
    }

    pub async fn start(&self) {
        info!("timer core started with registered schedule and runner extensions");
        self.core
            .start(self.schedules.clone(), self.runners.clone());
    }

    /// Submit a generic user task immediately. No task payload or runner
    /// semantics are decoded in this composition layer.
    pub async fn run_now(
        &self,
        task: &TimerTask,
    ) -> Result<String, crate::timer_core::TimerRunnerError> {
        self.core
            .submit_now(task.clone(), self.runners.clone())
            .await
            .map(|run| run.run_id)
    }

    pub async fn cancel_task(&self, task_id: &str) -> anyhow::Result<()> {
        self.core.cancel_task(task_id, self.runners.clone()).await
    }

    pub async fn suspend_task(&self, task_id: &str, reason: &str) -> anyhow::Result<()> {
        self.core.suspend_task(task_id, reason).await
    }

    pub async fn resume_task(&self, task_id: &str) -> anyhow::Result<()> {
        self.core
            .resume_task(task_id, self.schedules.as_ref())
            .await
    }

    pub async fn on_app_package_uninstalled(&self, package: &str) -> anyhow::Result<usize> {
        self.core.on_app_package_uninstalled(package).await
    }

    /// 下次唤醒时间查询（诊断/编排预读用；调度循环自身不经过它）。
    #[allow(dead_code)]
    pub async fn next_wakeup(&self) -> anyhow::Result<Option<DateTime<Utc>>> {
        self.core.next_wakeup().await
    }

    pub fn notify_tasks_changed(&self) {
        self.core.notify_changed();
    }

    /// Existing update/install workload gate, now calculated from generic
    /// TimerTask rows that Store keeps synchronized with the legacy view.
    pub fn next_enabled_trigger_in_secs(&self) -> Option<i64> {
        let tasks = self.core.db().list_timer_tasks().ok()?;
        next_enabled_trigger_in_secs(&tasks, Utc::now())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn scheduler_composes_timer_core_without_runner_or_payload_knowledge() {
        let source = include_str!("scheduler.rs");
        assert!(!source.contains(&["Script", "Store"].concat()));
        assert!(!source.contains(&["timer_", "yaml"].concat()));
        assert!(!source.contains(&["Run", "Target"].concat()));
    }
}
