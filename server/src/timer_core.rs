//! Generic timer infrastructure.
//!
//! This module owns the timer lifecycle and persistence boundary.  A schedule
//! is an opaque extension value and a runner is an injected implementation;
//! consequently the core does not parse cron, load YAML, or resolve app
//! resources.  The current YAML/cron path lives in `timer_yaml` and is kept as
//! an adapter for the existing HTTP API.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Notify;

use crate::core::{AndroidPackageName, AppContext, AppPackageId, DeviceId, RunPayload, RunRequest};
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

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.kind.trim().is_empty(),
            "schedule kind must not be empty"
        );
        anyhow::ensure!(
            !self.kind.chars().any(char::is_control),
            "schedule kind contains a control character"
        );
        Ok(())
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
        self.schedule.validate()?;
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
        self.schedule.validate()?;
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

/// Schedule extension registry. Timer Core asks this registry for instants;
/// individual extensions own parsing and schedule semantics.
pub struct ScheduleRegistry {
    extensions: std::sync::RwLock<HashMap<String, Arc<dyn ScheduleExtension>>>,
}

impl Default for ScheduleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ScheduleRegistry {
    pub fn new() -> Self {
        Self {
            extensions: std::sync::RwLock::new(HashMap::new()),
        }
    }

    pub fn register(
        &self,
        kind: impl Into<String>,
        extension: Arc<dyn ScheduleExtension>,
    ) -> anyhow::Result<()> {
        let kind = kind.into();
        anyhow::ensure!(
            !kind.trim().is_empty(),
            "schedule extension kind must not be empty"
        );
        let mut extensions = self.extensions.write().expect("schedule registry poisoned");
        anyhow::ensure!(
            !extensions.contains_key(&kind),
            "schedule extension already registered: {kind}"
        );
        extensions.insert(kind, extension);
        Ok(())
    }

    pub fn contains(&self, kind: &str) -> bool {
        self.extensions
            .read()
            .expect("schedule registry poisoned")
            .contains_key(kind)
    }

    fn get(&self, kind: &str) -> Option<Arc<dyn ScheduleExtension>> {
        self.extensions
            .read()
            .expect("schedule registry poisoned")
            .get(kind)
            .cloned()
    }
}

impl ScheduleExtension for ScheduleRegistry {
    fn next_after(
        &self,
        schedule: &ScheduleSpec,
        after: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, String> {
        self.get(&schedule.kind)
            .ok_or_else(|| format!("schedule extension unavailable: {}", schedule.kind))?
            .next_after(schedule, after)
    }

    fn latest_due(
        &self,
        schedule: &ScheduleSpec,
        now: DateTime<Utc>,
        lookback: Duration,
    ) -> Result<Option<DateTime<Utc>>, String> {
        self.get(&schedule.kind)
            .ok_or_else(|| format!("schedule extension unavailable: {}", schedule.kind))?
            .latest_due(schedule, now, lookback)
    }
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

    /// A registry/multiplexer can accept more than one runner id while a
    /// concrete runner keeps the strict one-id default.
    fn supports(&self, runner_id: &str) -> bool {
        self.runner_id() == runner_id
    }

    async fn submit(
        &self,
        request: RunRequest,
        task_id: &str,
        scheduled_at: Option<i64>,
        on_complete: TimerCompletionHook,
    ) -> Result<TimerRun, TimerRunnerError>;

    #[allow(dead_code, reason = "used by the task cancellation lifecycle API")]
    async fn cancel(&self, run_id: &str) -> Result<(), TimerRunnerError>;
}

/// In-process runner registry used by Timer Core. It is the extension-facing
/// registration point: adding a runner does not change scheduling or task
/// persistence, and a missing id is reported when the task is triggered.
pub struct TimerRunnerRegistry {
    runners: std::sync::RwLock<HashMap<String, Arc<dyn TimerRunner>>>,
}

impl Default for TimerRunnerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TimerRunnerRegistry {
    pub fn new() -> Self {
        Self {
            runners: std::sync::RwLock::new(HashMap::new()),
        }
    }

    pub fn register(&self, runner: Arc<dyn TimerRunner>) -> anyhow::Result<()> {
        let runner_id = runner.runner_id().trim();
        anyhow::ensure!(!runner_id.is_empty(), "runner id must not be empty");
        let mut runners = self
            .runners
            .write()
            .expect("timer runner registry poisoned");
        anyhow::ensure!(
            !runners.contains_key(runner_id),
            "runner already registered: {runner_id}"
        );
        runners.insert(runner_id.to_string(), runner);
        Ok(())
    }

    pub fn contains(&self, runner_id: &str) -> bool {
        self.runners
            .read()
            .expect("timer runner registry poisoned")
            .contains_key(runner_id)
    }

    fn get(&self, runner_id: &str) -> Option<Arc<dyn TimerRunner>> {
        self.runners
            .read()
            .expect("timer runner registry poisoned")
            .get(runner_id)
            .cloned()
    }
}

#[async_trait]
impl TimerRunner for TimerRunnerRegistry {
    fn runner_id(&self) -> &str {
        "<timer-runner-registry>"
    }

    fn supports(&self, runner_id: &str) -> bool {
        self.contains(runner_id)
    }

    async fn submit(
        &self,
        request: RunRequest,
        task_id: &str,
        scheduled_at: Option<i64>,
        on_complete: TimerCompletionHook,
    ) -> Result<TimerRun, TimerRunnerError> {
        let Some(runner) = self.get(&request.runner_id) else {
            return Err(TimerRunnerError::DependencyMissing(format!(
                "runner unavailable: {}",
                request.runner_id
            )));
        };
        runner
            .submit(request, task_id, scheduled_at, on_complete)
            .await
    }

    async fn cancel(&self, run_id: &str) -> Result<(), TimerRunnerError> {
        let runners = self
            .runners
            .read()
            .expect("timer runner registry poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut last_error = None;
        for runner in runners {
            match runner.cancel(run_id).await {
                Ok(()) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            TimerRunnerError::DependencyMissing(format!("runner unavailable for run: {run_id}"))
        }))
    }
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
    completion_notify: Arc<Notify>,
    completion_generation: Arc<AtomicU64>,
    started: AtomicBool,
    active_runs: Arc<Mutex<HashMap<String, String>>>,
    /// A runner is allowed to complete synchronously from `submit`. Keep a
    /// small hand-off set so that such a completion cannot be lost between
    /// the callback and the active-run registration below.
    completed_before_registration: Arc<Mutex<HashSet<String>>>,
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
            completion_notify: Arc::new(Notify::new()),
            completion_generation: Arc::new(AtomicU64::new(0)),
            started: AtomicBool::new(false),
            active_runs: Arc::new(Mutex::new(HashMap::new())),
            completed_before_registration: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    pub fn notify_changed(&self) {
        self.wakeup.notify_one();
    }

    /// Return a monotonic completion event cursor for consumers that need to
    /// await bookkeeping without polling task or run state.
    pub fn completion_generation(&self) -> u64 {
        self.completion_generation.load(Ordering::Acquire)
    }

    /// Wait until at least one runner completion has been persisted by the
    /// Timer Core completion handler. The cursor makes a notification that
    /// raced with the caller's snapshot observable instead of lost.
    pub async fn wait_for_completion(&self, after: u64) -> u64 {
        loop {
            let notified = self.completion_notify.notified();
            let current = self.completion_generation();
            if current > after {
                return current;
            }
            notified.await;
        }
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
                                    self.reject_schedule(&task, error).await;
                                    None
                                }
                            },
                            Err(error) => {
                                self.reject_schedule(&task, error).await;
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
                                self.reject_schedule(&task, error).await;
                                None
                            }
                        },
                        Err(error) => {
                            self.reject_schedule(&task, error).await;
                            None
                        }
                    },
                };
                let Some(due) = due else { continue };
                if due > now {
                    next_wakeup = min_instant(next_wakeup, due);
                    continue;
                }
                // Advance the persisted cursor before submitting. The unique
                // scheduled-run claim below makes repeated dispatches
                // idempotent after a restart; the cursor itself prevents the
                // same in-memory loop from repeatedly seeing this occurrence.
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
                        self.reject_schedule(&task, error).await;
                        continue;
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
        if !runner.supports(&task.runner_id) {
            let reason = format!("runner unavailable: {}", task.runner_id);
            self.db
                .metrics()
                .record_scheduler_event(SchedulerEvent::Failed);
            let _ = self.db.suspend_timer_task_async(&task.id, &reason).await;
            self.finish_rejected(&task, scheduled_at, "failed", None, Some(&reason))
                .await;
            return;
        }
        let request = match RunRequest::for_app(
            task.app.clone(),
            task.runner_id.clone(),
            task.entrypoint.clone(),
            RunPayload::new(task.payload.clone()),
        ) {
            Ok(request) => request,
            Err(error) => {
                self.handle_runner_error(
                    &task,
                    scheduled_at,
                    TimerRunnerError::Invalid(error.to_string()),
                )
                .await;
                return;
            }
        };
        let run_id = task.id.clone();
        let completion = self.completion_hook(task.id.clone(), scheduled_at);
        match runner
            .submit(request, &task.id, scheduled_at, completion)
            .await
        {
            Ok(run) => {
                {
                    let mut active_runs_guard = self
                        .active_runs
                        .lock()
                        .expect("timer active run mutex poisoned");
                    let mut completed_before_registration = self
                        .completed_before_registration
                        .lock()
                        .expect("timer completed-run mutex poisoned");
                    if !completed_before_registration.remove(&run.run_id) {
                        active_runs_guard.insert(run_id, run.run_id.clone());
                    }
                }
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

    async fn reject_schedule(&self, task: &TimerTask, error: String) {
        let reason = format!("schedule unavailable: {error}");
        tracing::warn!(task = %task.id, %reason, "timer schedule rejected task");
        let _ = self.db.suspend_timer_task_async(&task.id, &reason).await;
        self.db
            .metrics()
            .record_scheduler_event(SchedulerEvent::Failed);
        self.finish_rejected(task, None, "failed", None, Some(&reason))
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
        let completed_before_registration = self.completed_before_registration.clone();
        let completion_generation = self.completion_generation.clone();
        let completion_notify = self.completion_notify.clone();
        Arc::new(move |completion| {
            let db = db.clone();
            let active_runs = active_runs.clone();
            let completed_before_registration = completed_before_registration.clone();
            let completion_generation = completion_generation.clone();
            let completion_notify = completion_notify.clone();
            let task_id = completion.task_id.clone();
            let run_id = completion.run_id.clone();
            let mut active_runs_guard =
                active_runs.lock().expect("timer active run mutex poisoned");
            let mut completed_before_registration_guard = completed_before_registration
                .lock()
                .expect("timer completed-run mutex poisoned");
            if active_runs_guard
                .get(&task_id)
                .is_some_and(|active_run_id| active_run_id == &run_id)
            {
                active_runs_guard.remove(&task_id);
            } else {
                completed_before_registration_guard.insert(run_id);
            }
            // RunManager calls FinishHook synchronously from the run task.  DB
            // writes remain asynchronous and event-driven, never a poll loop.
            tokio::spawn(async move {
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
                completion_generation.fetch_add(1, Ordering::Release);
                completion_notify.notify_waiters();
            });
        })
    }

    pub async fn submit_now(
        &self,
        task: TimerTask,
        runner: Arc<dyn TimerRunner>,
    ) -> Result<TimerRun, TimerRunnerError> {
        if !runner.supports(&task.runner_id) {
            let reason = format!("runner unavailable: {}", task.runner_id);
            self.db
                .suspend_timer_task_async(&task.id, &reason)
                .await
                .map_err(|error| TimerRunnerError::Other(error.to_string()))?;
            self.db
                .update_timer_task_result_async(&task.id, "失败", Some(&reason))
                .await
                .map_err(|error| TimerRunnerError::Other(error.to_string()))?;
            self.notify_changed();
            return Err(TimerRunnerError::DependencyMissing(reason));
        }
        if !task.is_schedulable() {
            return Err(TimerRunnerError::Invalid(
                "timer task is not active".to_string(),
            ));
        }
        let request = RunRequest::for_app(
            task.app.clone(),
            task.runner_id.clone(),
            task.entrypoint.clone(),
            RunPayload::new(task.payload.clone()),
        )
        .map_err(|error| TimerRunnerError::Invalid(error.to_string()))?;
        let completion = self.completion_hook(task.id.clone(), None);
        let run = runner.submit(request, &task.id, None, completion).await?;
        let mut active_runs_guard = self
            .active_runs
            .lock()
            .expect("timer active run mutex poisoned");
        let mut completed_before_registration = self
            .completed_before_registration
            .lock()
            .expect("timer completed-run mutex poisoned");
        if !completed_before_registration.remove(&run.run_id) {
            active_runs_guard.insert(task.id, run.run_id.clone());
        }
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
        anyhow::ensure!(
            task.state != TimerTaskState::Cancelled,
            "cancelled timer task cannot be resumed"
        );
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
        // Enumerate first so a repeated uninstall notification is a no-op and
        // cancelled tasks are not accidentally changed into suspended tasks.
        // The bulk Store helper remains available for older callers, while
        // this lifecycle boundary needs the stronger idempotent semantics.
        let tasks = self.db.list_timer_tasks_async().await?;
        let matching = tasks
            .into_iter()
            .filter(|task| task.is_schedulable())
            .filter(|task| {
                task.app
                    .content_package
                    .as_ref()
                    .is_some_and(|value| value.as_str() == package)
                    || task.app.android_package.as_str() == package
            })
            .collect::<Vec<_>>();
        for task in &matching {
            self.db
                .suspend_timer_task_async(&task.id, "app package unavailable")
                .await?;
        }
        self.notify_changed();
        Ok(matching.len())
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
        id: &'static str,
        submissions: AtomicUsize,
        cancellations: AtomicUsize,
        complete_immediately: bool,
        submit_error: Option<TimerRunnerError>,
    }

    #[async_trait]
    impl TimerRunner for FakeRunner {
        fn runner_id(&self) -> &str {
            self.id
        }

        async fn submit(
            &self,
            request: RunRequest,
            task_id: &str,
            scheduled_at: Option<i64>,
            on_complete: TimerCompletionHook,
        ) -> Result<TimerRun, TimerRunnerError> {
            self.submissions.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self.submit_error.clone() {
                return Err(error);
            }
            let run_id = format!("run-{}", self.submissions.load(Ordering::SeqCst));
            if self.complete_immediately {
                on_complete(TimerCompletion {
                    task_id: task_id.to_string(),
                    scheduled_at,
                    run_id: run_id.clone(),
                    outcome: TimerOutcome::Success,
                });
            }
            assert_eq!(request.runner_id, self.id);
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
            id: "fake.runner",
            submissions: AtomicUsize::new(0),
            cancellations: AtomicUsize::new(0),
            complete_immediately: true,
            submit_error: None,
        });
        let core = TimerCore::new(db.clone());
        let completion_before = core.completion_generation();

        core.dispatch(task.clone(), Some(42), runner.clone()).await;
        core.dispatch(task, Some(42), runner.clone()).await;
        core.wait_for_completion(completion_before).await;

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

    #[tokio::test]
    async fn synchronous_completion_does_not_retain_an_active_run() {
        let (db, dir) = test_db("sync-completion");
        let task = test_task();
        db.upsert_timer_task_async(&task).await.unwrap();
        let runner = Arc::new(FakeRunner {
            id: "fake.runner",
            submissions: AtomicUsize::new(0),
            cancellations: AtomicUsize::new(0),
            complete_immediately: true,
            submit_error: None,
        });
        let core = TimerCore::new(db.clone());
        let completion_before = core.completion_generation();
        let run = core.submit_now(task, runner.clone()).await.unwrap();
        core.wait_for_completion(completion_before).await;

        core.cancel_task("task-1", runner.clone()).await.unwrap();
        assert_eq!(runner.cancellations.load(Ordering::SeqCst), 0);
        assert_eq!(run.run_id, "run-1");
        drop(core);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn missing_runner_is_a_persisted_dependency_error_not_a_startup_failure() {
        let (db, dir) = test_db("missing-runner");
        let mut task = test_task();
        task.runner_id = "missing.runner".into();
        db.upsert_timer_task_async(&task).await.unwrap();
        let runner = Arc::new(FakeRunner {
            id: "fake.runner",
            submissions: AtomicUsize::new(0),
            cancellations: AtomicUsize::new(0),
            complete_immediately: false,
            submit_error: None,
        });
        TimerCore::new(db.clone())
            .dispatch(task, Some(7), runner)
            .await;

        let saved = db.get_timer_task_async("task-1").await.unwrap().unwrap();
        assert_eq!(saved.state, TimerTaskState::Suspended);
        assert_eq!(
            saved.suspend_reason.as_deref(),
            Some("runner unavailable: missing.runner")
        );
        assert_eq!(db.scheduled_run_state("task-1", 7), "failed");
        assert_eq!(saved.last_result.as_deref(), Some("失败"));
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn runner_failure_is_recovered_and_recorded_for_the_trigger() {
        let (db, dir) = test_db("runner-failure");
        let task = test_task();
        db.upsert_timer_task_async(&task).await.unwrap();
        let runner = Arc::new(FakeRunner {
            id: "fake.runner",
            submissions: AtomicUsize::new(0),
            cancellations: AtomicUsize::new(0),
            complete_immediately: false,
            submit_error: Some(TimerRunnerError::Invalid("runner rejected payload".into())),
        });
        TimerCore::new(db.clone())
            .dispatch(task, Some(8), runner)
            .await;

        assert_eq!(db.scheduled_run_state("task-1", 8), "failed");
        let saved = db.get_timer_task_async("task-1").await.unwrap().unwrap();
        assert_eq!(saved.state, TimerTaskState::Active);
        assert_eq!(saved.last_result.as_deref(), Some("失败"));
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn missing_schedule_extension_suspends_task_and_records_failure() {
        let (db, dir) = test_db("missing-schedule");
        let mut task = test_task();
        task.schedule = ScheduleSpec::new("missing.schedule", serde_json::json!({})).unwrap();
        db.upsert_timer_task_async(&task).await.unwrap();
        let schedules = ScheduleRegistry::new();
        let error = schedules
            .next_after(&task.schedule, Utc::now())
            .unwrap_err();
        assert_eq!(error, "schedule extension unavailable: missing.schedule");
        db.suspend_timer_task_async(&task.id, &error).await.unwrap();
        let saved = db.get_timer_task_async(&task.id).await.unwrap().unwrap();
        assert_eq!(saved.state, TimerTaskState::Suspended);
        assert_eq!(saved.suspend_reason.as_deref(), Some(error.as_str()));
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn registry_routes_the_same_generic_request_to_different_runners() {
        let (db, dir) = test_db("runner-registry");
        let mut task = test_task();
        task.runner_id = "second.runner".into();
        db.upsert_timer_task_async(&task).await.unwrap();
        let first = Arc::new(FakeRunner {
            id: "fake.runner",
            submissions: AtomicUsize::new(0),
            cancellations: AtomicUsize::new(0),
            complete_immediately: false,
            submit_error: None,
        });
        let second = Arc::new(FakeRunner {
            id: "second.runner",
            submissions: AtomicUsize::new(0),
            cancellations: AtomicUsize::new(0),
            complete_immediately: false,
            submit_error: None,
        });
        let registry = Arc::new(TimerRunnerRegistry::new());
        registry.register(first).unwrap();
        registry.register(second.clone()).unwrap();
        TimerCore::new(db.clone())
            .dispatch(task, Some(9), registry)
            .await;
        assert_eq!(second.submissions.load(Ordering::SeqCst), 1);
        assert_eq!(db.scheduled_run_state("task-1", 9), "running");
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn suspend_resume_preserves_schedule_and_repeated_transitions_are_safe() {
        let (db, dir) = test_db("suspend-resume");
        let mut task = test_task();
        task.schedule =
            ScheduleSpec::new("cron", serde_json::json!({"expression": "*/5 * * * *"})).unwrap();
        db.upsert_timer_task_async(&task).await.unwrap();
        let core = TimerCore::new(db.clone());
        core.suspend_task("task-1", "dependency unavailable")
            .await
            .unwrap();
        core.suspend_task("task-1", "dependency unavailable")
            .await
            .unwrap();
        assert_eq!(
            db.get_timer_task_async("task-1")
                .await
                .unwrap()
                .unwrap()
                .state,
            TimerTaskState::Suspended
        );
        core.resume_task("task-1", &crate::cron_extension::CronExtension)
            .await
            .unwrap();
        core.resume_task("task-1", &crate::cron_extension::CronExtension)
            .await
            .unwrap();
        let resumed = db.get_timer_task_async("task-1").await.unwrap().unwrap();
        assert_eq!(resumed.state, TimerTaskState::Active);
        assert!(resumed.next_wakeup.is_some());
        assert_eq!(resumed.schedule.kind, "cron");
        drop(core);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn persisted_wakeup_is_available_to_a_new_core_after_restart() {
        let (db, dir) = test_db("restore");
        let mut task = test_task();
        let wakeup = Utc::now() + chrono::Duration::minutes(5);
        task.next_wakeup = Some(wakeup);
        db.upsert_timer_task_async(&task).await.unwrap();
        drop(TimerCore::new(db.clone()));
        let expected = DateTime::<Utc>::from_timestamp(wakeup.timestamp(), 0);
        assert_eq!(
            TimerCore::new(db.clone()).next_wakeup().await.unwrap(),
            expected
        );
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
