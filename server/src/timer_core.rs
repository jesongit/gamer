//! Generic timer infrastructure.
//!
//! This module owns the timer lifecycle and persistence boundary.  A schedule
//! is an opaque extension value and a runner is an injected implementation;
//! consequently the core does not parse cron, load YAML, or resolve app
//! resources.  The current YAML/cron path lives in `timer_yaml` and is kept as
//! an adapter for the existing HTTP API.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Notify;

use crate::core::{AndroidPackageName, AppContext, AppPackageId, DeviceId};
use crate::metrics::SchedulerEvent;
use crate::run_manager::RunRecord;
use crate::store::{Db, TimerTaskStorage};

/// An extension-owned schedule.  `kind` selects the extension and `value` is
/// interpreted only by that extension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleSpec {
    pub kind: String,
    pub value: Value,
}

impl ScheduleSpec {
    pub fn new(kind: impl Into<String>, value: Value) -> anyhow::Result<Self> {
        let kind = kind.into();
        if kind.trim().is_empty() {
            anyhow::bail!("schedule kind must not be empty");
        }
        Ok(Self { kind, value })
    }
}

/// User-owned task lifecycle.  Suspended tasks remain persisted and can carry
/// a dependency reason (for example, an uninstalled app package).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerTaskState {
    Active,
    Suspended,
    Cancelled,
}

impl TimerTaskState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "suspended" => Ok(Self::Suspended),
            "cancelled" => Ok(Self::Cancelled),
            other => anyhow::bail!("unknown timer task state: {other}"),
        }
    }
}

/// A persisted user task.  All runner-specific input is kept in `payload`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimerTask {
    pub id: String,
    pub name: String,
    pub app: AppContext,
    pub runner_id: String,
    pub entrypoint: String,
    pub payload: Value,
    pub schedule: ScheduleSpec,
    pub state: TimerTaskState,
    /// Compatibility flag for the legacy task API.  New callers should use
    /// `state`; false always makes a task non-schedulable.
    pub enabled: bool,
    pub next_wakeup: Option<DateTime<Utc>>,
    pub last_result: Option<String>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The preset that created this task, if any.  It is deliberately not a
    /// foreign key: removing a package must suspend a user task, not delete it.
    pub preset_id: Option<String>,
    pub suspend_reason: Option<String>,
}

impl TimerTask {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        app: AppContext,
        runner_id: impl Into<String>,
        entrypoint: impl Into<String>,
        payload: Value,
        schedule: ScheduleSpec,
    ) -> anyhow::Result<Self> {
        let now = Utc::now();
        let task = Self {
            id: id.into(),
            name: name.into(),
            app,
            runner_id: runner_id.into(),
            entrypoint: entrypoint.into(),
            payload,
            schedule,
            state: TimerTaskState::Active,
            enabled: true,
            next_wakeup: None,
            last_result: None,
            last_run_at: None,
            created_at: now,
            updated_at: now,
            preset_id: None,
            suspend_reason: None,
        };
        task.validate()?;
        Ok(task)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        for (field, value) in [
            ("task id", self.id.as_str()),
            ("task name", self.name.as_str()),
            ("runner_id", self.runner_id.as_str()),
            ("entrypoint", self.entrypoint.as_str()),
        ] {
            anyhow::ensure!(!value.trim().is_empty(), "{field} must not be empty");
            anyhow::ensure!(
                !value.chars().any(char::is_control),
                "{field} contains a control character"
            );
        }
        Ok(())
    }

    pub fn is_schedulable(&self) -> bool {
        self.enabled && self.state == TimerTaskState::Active
    }

    pub(crate) fn from_storage(row: TimerTaskStorage) -> anyhow::Result<Self> {
        let device_id = DeviceId::new(row.device_id)?;
        let android_package = AndroidPackageName::new(row.android_package)?;
        let content_package = row.content_package.map(AppPackageId::new).transpose()?;
        let payload = serde_json::from_str(&row.payload_json)?;
        let schedule: ScheduleSpec = serde_json::from_str(&row.schedule_json)?;
        let state = TimerTaskState::parse(&row.state)?;
        let last_run_at = row.last_run_at.map(parse_timestamp).transpose()?;
        let created_at = parse_timestamp(row.created_at)?;
        let updated_at = parse_timestamp(row.updated_at)?;
        let next_wakeup = row
            .next_wakeup
            .and_then(|value| DateTime::<Utc>::from_timestamp(value, 0));
        let task = Self {
            id: row.id,
            name: row.name,
            app: AppContext::new(device_id, android_package, content_package),
            runner_id: row.runner_id,
            entrypoint: row.entrypoint,
            payload,
            schedule,
            state,
            enabled: row.enabled,
            next_wakeup,
            last_result: row.last_result,
            last_run_at,
            created_at,
            updated_at,
            preset_id: row.preset_id,
            suspend_reason: row.suspend_reason,
        };
        task.validate()?;
        Ok(task)
    }
}

fn parse_timestamp(value: String) -> anyhow::Result<DateTime<Utc>> {
    if let Ok(timestamp) = value.parse::<DateTime<Utc>>() {
        return Ok(timestamp);
    }
    let naive = NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S")?;
    Ok(Local
        .from_local_datetime(&naive)
        .single()
        .ok_or_else(|| anyhow::anyhow!("ambiguous local timestamp: {value}"))?
        .with_timezone(&Utc))
}

/// A package-provided task template.  Presets are not user schedules and can
/// be removed/reinstalled independently from `TimerTask` rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskPreset {
    pub id: String,
    pub app_package: String,
    pub name: String,
    pub runner_id: String,
    pub entrypoint: String,
    pub payload: Value,
    pub schedule: ScheduleSpec,
    pub created_at: DateTime<Utc>,
}

impl TaskPreset {
    pub fn validate(&self) -> anyhow::Result<()> {
        for (field, value) in [
            ("preset id", self.id.as_str()),
            ("preset name", self.name.as_str()),
            ("runner_id", self.runner_id.as_str()),
            ("entrypoint", self.entrypoint.as_str()),
            ("app_package", self.app_package.as_str()),
        ] {
            anyhow::ensure!(!value.trim().is_empty(), "{field} must not be empty");
            anyhow::ensure!(
                !value.chars().any(char::is_control),
                "{field} contains a control character"
            );
        }
        Ok(())
    }
}

/// System wall clock boundary.  Tests can inject a deterministic clock.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Schedule extension boundary.  The timer core only asks for instants.
pub trait ScheduleExtension: Send + Sync {
    fn next_after(
        &self,
        schedule: &ScheduleSpec,
        after: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, String>;

    fn latest_due(
        &self,
        schedule: &ScheduleSpec,
        now: DateTime<Utc>,
        lookback: Duration,
    ) -> Result<Option<DateTime<Utc>>, String>;
}

#[derive(Debug, Clone)]
pub enum TimerRunnerError {
    DependencyMissing(String),
    Invalid(String),
    ParamStale {
        stored: String,
        current: String,
        message: String,
    },
    Conflict(Box<RunRecord>),
    ShuttingDown,
    #[allow(
        dead_code,
        reason = "adapter-specific cancellation failures use this catch-all"
    )]
    Other(String),
}

impl std::fmt::Display for TimerRunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DependencyMissing(message) => write!(f, "dependency unavailable: {message}"),
            Self::Invalid(message) => f.write_str(message),
            Self::ParamStale { message, .. } => f.write_str(message),
            Self::Conflict(record) => write!(f, "device busy: {}", record.run_id),
            Self::ShuttingDown => f.write_str("server is shutting down"),
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for TimerRunnerError {}

#[derive(Debug, Clone)]
pub enum TimerOutcome {
    Success,
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct TimerCompletion {
    pub task_id: String,
    pub scheduled_at: Option<i64>,
    pub run_id: String,
    pub outcome: TimerOutcome,
}

pub type TimerCompletionHook = Arc<dyn Fn(TimerCompletion) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimerRun {
    pub run_id: String,
}

/// Runner implementation boundary.  Completion is pushed by the runner via
/// the callback; the core never polls a run registry.
#[async_trait]
pub trait TimerRunner: Send + Sync {
    fn runner_id(&self) -> &str;

    async fn submit(
        &self,
        task: TimerTask,
        scheduled_at: Option<i64>,
        on_complete: TimerCompletionHook,
    ) -> Result<TimerRun, TimerRunnerError>;

    #[allow(dead_code, reason = "used by the task cancellation lifecycle API")]
    async fn cancel(&self, run_id: &str) -> Result<(), TimerRunnerError>;
}

/// Compatibility construction seam used by the existing three-argument
/// `Scheduler::new(db, adapter, runs)` calls.  The scheduler only sees this
/// generic factory, while the YAML adapter owns the legacy ScriptStore.
pub trait TimerRunnerFactory: Send + 'static {
    fn into_timer_runner(
        self,
        db: Db,
        runs: Arc<crate::run_manager::RunManager>,
    ) -> Arc<dyn TimerRunner>;
}

/// Timer Core service.  All state transitions are persisted before the next
/// wakeup is awaited, so a process restart can reconstruct its schedule.
pub struct TimerCore {
    db: Db,
    clock: Arc<dyn Clock>,
    wakeup: Notify,
    started: AtomicBool,
    active_runs: Arc<Mutex<HashMap<String, String>>>,
}

#[allow(
    dead_code,
    reason = "public lifecycle methods are consumed by task/package adapters incrementally"
)]
impl TimerCore {
    pub fn new(db: Db) -> Arc<Self> {
        Self::with_clock(db, Arc::new(SystemClock))
    }

    pub fn with_clock(db: Db, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            db,
            clock,
            wakeup: Notify::new(),
            started: AtomicBool::new(false),
            active_runs: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    pub fn notify_changed(&self) {
        self.wakeup.notify_one();
    }

    pub async fn save_task(&self, task: &TimerTask) -> anyhow::Result<()> {
        self.db.upsert_timer_task_async(task).await?;
        self.notify_changed();
        Ok(())
    }

    pub async fn delete_task(&self, task_id: &str) -> anyhow::Result<()> {
        self.db.delete_timer_task_async(task_id).await?;
        self.notify_changed();
        Ok(())
    }

    pub fn start(
        self: &Arc<Self>,
        extension: Arc<dyn ScheduleExtension>,
        runner: Arc<dyn TimerRunner>,
    ) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let core = self.clone();
        tokio::spawn(async move {
            core.run_loop(extension, runner).await;
        });
    }

    async fn run_loop(
        self: Arc<Self>,
        extension: Arc<dyn ScheduleExtension>,
        runner: Arc<dyn TimerRunner>,
    ) {
        const LOOKBACK: Duration = Duration::from_secs(60 * 60);
        loop {
            let now = self.clock.now();
            let oldest_misfire = now
                - chrono::Duration::from_std(LOOKBACK)
                    .expect("Timer Core lookback must fit chrono duration");
            let tasks = match self.db.list_timer_tasks_async().await {
                Ok(tasks) => tasks,
                Err(error) => {
                    tracing::error!(%error, "timer core task restore failed");
                    self.wait_for(Duration::from_secs(1)).await;
                    continue;
                }
            };
            let mut next_wakeup: Option<DateTime<Utc>> = None;
            for mut task in tasks {
                if !task.is_schedulable() {
                    continue;
                }
                let due = match task.next_wakeup {
                    Some(value) if value <= now && value >= oldest_misfire => Some(value),
                    // A persisted cursor can outlive the process for longer
                    // than the misfire window. Recompute through the schedule
                    // extension so restart recovery does not replay stale work.
                    Some(value) if value < oldest_misfire => {
                        match extension.latest_due(&task.schedule, now, LOOKBACK) {
                            Ok(Some(value)) => Some(value),
                            Ok(None) => match extension.next_after(&task.schedule, now) {
                                Ok(next) => {
                                    if let Some(next) = next {
                                        let _ = self
                                            .db
                                            .set_timer_task_wakeup_async(
                                                &task.id,
                                                Some(next.timestamp()),
                                            )
                                            .await;
                                        next_wakeup = min_instant(next_wakeup, next);
                                    }
                                    None
                                }
                                Err(error) => {
                                    tracing::warn!(task = %task.id, %error, "timer schedule extension rejected task");
                                    None
                                }
                            },
                            Err(error) => {
                                tracing::warn!(task = %task.id, %error, "timer schedule extension rejected task");
                                None
                            }
                        }
                    }
                    Some(value) => {
                        next_wakeup = min_instant(next_wakeup, value);
                        None
                    }
                    None => match extension.latest_due(&task.schedule, now, LOOKBACK) {
                        Ok(Some(value)) => Some(value),
                        Ok(None) => match extension.next_after(&task.schedule, now) {
                            Ok(value) => {
                                if let Some(value) = value {
                                    let _ = self
                                        .db
                                        .set_timer_task_wakeup_async(
                                            &task.id,
                                            Some(value.timestamp()),
                                        )
                                        .await;
                                    next_wakeup = min_instant(next_wakeup, value);
                                }
                                None
                            }
                            Err(error) => {
                                tracing::warn!(task = %task.id, %error, "timer schedule extension rejected task");
                                None
                            }
                        },
                        Err(error) => {
                            tracing::warn!(task = %task.id, %error, "timer schedule extension rejected task");
                            None
                        }
                    },
                };
                let Some(due) = due else { continue };
                if due > now {
                    next_wakeup = min_instant(next_wakeup, due);
                    continue;
                }
                // Advance the persisted cursor before submitting.  A crash
                // after this point is recovered by scheduled_runs' claim.
                match extension.next_after(&task.schedule, now) {
                    Ok(next) => {
                        task.next_wakeup = next;
                        if let Err(error) = self
                            .db
                            .set_timer_task_wakeup_async(
                                &task.id,
                                next.map(|value| value.timestamp()),
                            )
                            .await
                        {
                            tracing::error!(task = %task.id, %error, "timer wakeup persistence failed");
                            continue;
                        }
                        if let Some(next) = next {
                            next_wakeup = min_instant(next_wakeup, next);
                        }
                    }
                    Err(error) => {
                        tracing::warn!(task = %task.id, %error, "timer task has no next wakeup");
                    }
                }
                self.dispatch(task, Some(due.timestamp()), runner.clone())
                    .await;
            }
            let wait = next_wakeup
                .and_then(|value| (value - self.clock.now()).to_std().ok())
                .unwrap_or(Duration::from_secs(10));
            self.wait_for(wait.min(Duration::from_secs(60))).await;
        }
    }

    async fn wait_for(&self, duration: Duration) {
        tokio::select! {
            _ = tokio::time::sleep(duration) => {}
            _ = self.wakeup.notified() => {}
        }
    }

    pub async fn dispatch(
        &self,
        task: TimerTask,
        scheduled_at: Option<i64>,
        runner: Arc<dyn TimerRunner>,
    ) {
        if let Some(scheduled_at) = scheduled_at {
            record_trigger_latency(&self.db, scheduled_at);
            match self
                .db
                .claim_scheduled_run_async(&task.id, scheduled_at)
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    self.db
                        .metrics()
                        .record_scheduler_event(SchedulerEvent::Skipped);
                    return;
                }
                Err(error) => {
                    self.db
                        .metrics()
                        .record_scheduler_event(SchedulerEvent::Failed);
                    tracing::error!(task = %task.id, %scheduled_at, %error, "timer trigger claim failed");
                    return;
                }
            }
        }
        if runner.runner_id() != task.runner_id {
            let reason = format!("runner unavailable: {}", task.runner_id);
            self.db
                .metrics()
                .record_scheduler_event(SchedulerEvent::Failed);
            let _ = self.db.suspend_timer_task_async(&task.id, &reason).await;
            self.finish_rejected(&task, scheduled_at, "failed", None, Some(&reason))
                .await;
            return;
        }
        let run_id = task.id.clone();
        let completion = self.completion_hook(task.id.clone(), scheduled_at);
        match runner.submit(task.clone(), scheduled_at, completion).await {
            Ok(run) => {
                self.active_runs
                    .lock()
                    .expect("timer active run mutex poisoned")
                    .insert(run_id, run.run_id.clone());
                if let Some(scheduled_at) = scheduled_at {
                    if let Err(error) = self
                        .db
                        .attach_scheduled_run_async(&task.id, scheduled_at, &run.run_id)
                        .await
                    {
                        tracing::error!(task = %task.id, %error, "timer run attachment failed");
                    }
                }
            }
            Err(error) => self.handle_runner_error(&task, scheduled_at, error).await,
        }
    }

    async fn handle_runner_error(
        &self,
        task: &TimerTask,
        scheduled_at: Option<i64>,
        error: TimerRunnerError,
    ) {
        let (state, message) = match &error {
            TimerRunnerError::Conflict(_) => {
                self.db
                    .metrics()
                    .record_scheduler_event(SchedulerEvent::Conflict);
                ("skipped", "设备忙".to_string())
            }
            TimerRunnerError::ShuttingDown => {
                self.db
                    .metrics()
                    .record_scheduler_event(SchedulerEvent::Skipped);
                ("skipped", "服务正在关闭".to_string())
            }
            TimerRunnerError::DependencyMissing(message) => {
                self.db
                    .metrics()
                    .record_scheduler_event(SchedulerEvent::Failed);
                let _ = self.db.suspend_timer_task_async(&task.id, message).await;
                ("failed", message.clone())
            }
            TimerRunnerError::Invalid(message)
            | TimerRunnerError::Other(message)
            | TimerRunnerError::ParamStale { message, .. } => {
                self.db
                    .metrics()
                    .record_scheduler_event(SchedulerEvent::Failed);
                ("failed", message.clone())
            }
        };
        self.finish_rejected(task, scheduled_at, state, None, Some(&message))
            .await;
    }

    async fn finish_rejected(
        &self,
        task: &TimerTask,
        scheduled_at: Option<i64>,
        state: &str,
        run_id: Option<&str>,
        error: Option<&str>,
    ) {
        if let Some(scheduled_at) = scheduled_at {
            let _ = self
                .db
                .finish_scheduled_run_async(&task.id, scheduled_at, state, run_id, error)
                .await;
        }
        let label = if state == "success" {
            "成功"
        } else {
            "失败"
        };
        let _ = self
            .db
            .update_timer_task_result_async(&task.id, label, error)
            .await;
    }

    fn completion_hook(&self, _task_id: String, _scheduled_at: Option<i64>) -> TimerCompletionHook {
        let db = self.db.clone();
        let active_runs = self.active_runs.clone();
        Arc::new(move |completion| {
            let db = db.clone();
            let active_runs = active_runs.clone();
            // RunManager calls FinishHook synchronously from the run task.  DB
            // writes remain asynchronous and event-driven, never a poll loop.
            tokio::spawn(async move {
                let task_id = completion.task_id.clone();
                let scheduled_at = completion.scheduled_at;
                let (state, label, error) = match &completion.outcome {
                    TimerOutcome::Success => ("success", "成功", None),
                    TimerOutcome::Failed(message) => ("failed", "失败", Some(message.as_str())),
                    TimerOutcome::Cancelled => ("skipped", "取消", Some("运行被取消")),
                };
                if let Some(scheduled_at) = scheduled_at {
                    let _ = db
                        .finish_scheduled_run_async(
                            &task_id,
                            scheduled_at,
                            state,
                            Some(&completion.run_id),
                            error,
                        )
                        .await;
                }
                let _ = db
                    .update_timer_task_result_async(&task_id, label, error)
                    .await;
                active_runs
                    .lock()
                    .expect("timer active run mutex poisoned")
                    .remove(&task_id);
            });
        })
    }

    pub async fn submit_now(
        &self,
        task: TimerTask,
        runner: Arc<dyn TimerRunner>,
    ) -> Result<TimerRun, TimerRunnerError> {
        if runner.runner_id() != task.runner_id {
            return Err(TimerRunnerError::DependencyMissing(format!(
                "runner unavailable: {}",
                task.runner_id
            )));
        }
        let completion = self.completion_hook(task.id.clone(), None);
        let run = runner.submit(task.clone(), None, completion).await?;
        self.active_runs
            .lock()
            .expect("timer active run mutex poisoned")
            .insert(task.id, run.run_id.clone());
        Ok(run)
    }

    pub async fn cancel_task(
        &self,
        task_id: &str,
        runner: Arc<dyn TimerRunner>,
    ) -> anyhow::Result<()> {
        let run_id = self
            .active_runs
            .lock()
            .expect("timer active run mutex poisoned")
            .get(task_id)
            .cloned();
        if let Some(run_id) = run_id {
            runner
                .cancel(&run_id)
                .await
                .map_err(|error| anyhow::anyhow!(error))?;
        }
        let result = self
            .db
            .set_timer_task_state_async(task_id, TimerTaskState::Cancelled, false, None)
            .await;
        self.notify_changed();
        result
    }

    pub async fn suspend_task(&self, task_id: &str, reason: &str) -> anyhow::Result<()> {
        let result = self.db.suspend_timer_task_async(task_id, reason).await;
        self.notify_changed();
        result
    }

    pub async fn resume_task(
        &self,
        task_id: &str,
        extension: &dyn ScheduleExtension,
    ) -> anyhow::Result<()> {
        let task = self
            .db
            .get_timer_task_async(task_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("timer task not found: {task_id}"))?;
        let next = extension
            .next_after(&task.schedule, self.clock.now())
            .map_err(|error| anyhow::anyhow!(error))?;
        self.db
            .set_timer_task_state_async(task_id, TimerTaskState::Active, true, None)
            .await?;
        self.db
            .set_timer_task_wakeup_async(task_id, next.map(|v| v.timestamp()))
            .await?;
        self.notify_changed();
        Ok(())
    }

    /// App Package lifecycle hook.  It intentionally changes state only; the
    /// persisted user schedule remains available for a later resume.
    pub async fn on_app_package_uninstalled(&self, package: &str) -> anyhow::Result<usize> {
        let result = self
            .db
            .suspend_timer_tasks_for_package_async(package, "app package unavailable")
            .await;
        self.notify_changed();
        result
    }

    pub async fn next_wakeup(&self) -> anyhow::Result<Option<DateTime<Utc>>> {
        Ok(self
            .db
            .list_timer_tasks_async()
            .await?
            .into_iter()
            .filter(TimerTask::is_schedulable)
            .filter_map(|task| task.next_wakeup)
            .min())
    }
}

fn min_instant(current: Option<DateTime<Utc>>, candidate: DateTime<Utc>) -> Option<DateTime<Utc>> {
    Some(match current {
        Some(current) => current.min(candidate),
        None => candidate,
    })
}

fn record_trigger_latency(db: &Db, scheduled_at: i64) {
    let now_millis = Utc::now().timestamp_millis();
    let scheduled_millis = scheduled_at.saturating_mul(1_000);
    let latency = now_millis.saturating_sub(scheduled_millis).max(0) as u64;
    db.metrics().record_scheduler_trigger(latency);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    use async_trait::async_trait;

    struct FakeRunner {
        submissions: AtomicUsize,
        cancellations: AtomicUsize,
        complete_immediately: bool,
    }

    #[async_trait]
    impl TimerRunner for FakeRunner {
        fn runner_id(&self) -> &str {
            "fake.runner"
        }

        async fn submit(
            &self,
            task: TimerTask,
            scheduled_at: Option<i64>,
            on_complete: TimerCompletionHook,
        ) -> Result<TimerRun, TimerRunnerError> {
            self.submissions.fetch_add(1, Ordering::SeqCst);
            let run_id = format!("run-{}", self.submissions.load(Ordering::SeqCst));
            if self.complete_immediately {
                on_complete(TimerCompletion {
                    task_id: task.id,
                    scheduled_at,
                    run_id: run_id.clone(),
                    outcome: TimerOutcome::Success,
                });
            }
            Ok(TimerRun { run_id })
        }

        async fn cancel(&self, _run_id: &str) -> Result<(), TimerRunnerError> {
            self.cancellations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn test_db(name: &str) -> (Db, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("gamer-timer-core-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = crate::config::Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        (Arc::new(crate::store::Store::open(&cfg).unwrap()), dir)
    }

    fn test_task() -> TimerTask {
        TimerTask::new(
            "task-1",
            "Task",
            AppContext::from_legacy_package("device-1", "com.example").unwrap(),
            "fake.runner",
            "entry",
            serde_json::json!({"value": 1}),
            ScheduleSpec::new("opaque", serde_json::json!({"rule": "test"})).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn schedule_is_opaque_to_core_models() {
        let spec = ScheduleSpec::new("future-extension", serde_json::json!({"rule": "x"})).unwrap();
        assert_eq!(spec.kind, "future-extension");
        assert_eq!(spec.value["rule"], "x");
    }

    #[test]
    fn task_state_is_suspendable_without_losing_schedule() {
        let app = AppContext::from_legacy_package("d1", "com.example.game").unwrap();
        let schedule =
            ScheduleSpec::new("cron", serde_json::json!({"expression": "* * * * *"})).unwrap();
        let mut task = TimerTask::new(
            "task",
            "Task",
            app,
            "runner",
            "entry",
            Value::Null,
            schedule.clone(),
        )
        .unwrap();
        task.state = TimerTaskState::Suspended;
        task.suspend_reason = Some("app package unavailable".into());
        assert!(!task.is_schedulable());
        assert_eq!(task.schedule, schedule);
    }

    #[tokio::test]
    async fn dispatch_claims_once_and_completion_updates_persisted_task() {
        let (db, dir) = test_db("dispatch");
        let task = test_task();
        db.upsert_timer_task_async(&task).await.unwrap();
        let runner = Arc::new(FakeRunner {
            submissions: AtomicUsize::new(0),
            cancellations: AtomicUsize::new(0),
            complete_immediately: true,
        });
        let core = TimerCore::new(db.clone());

        core.dispatch(task.clone(), Some(42), runner.clone()).await;
        core.dispatch(task, Some(42), runner.clone()).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(runner.submissions.load(Ordering::SeqCst), 1);
        assert_eq!(db.scheduled_run_state("task-1", 42), "success");
        assert_eq!(
            db.get_timer_task_async("task-1")
                .await
                .unwrap()
                .unwrap()
                .last_result
                .as_deref(),
            Some("成功")
        );

        drop(runner);
        drop(core);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
