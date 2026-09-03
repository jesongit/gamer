//! Timer Core composition and the Cron schedule extension.
//!
//! `Scheduler` is kept as the compatibility façade used by the existing
//! REST/update code.  It does not own a script store or parse runner payloads:
//! those concerns are supplied by a `TimerRunnerFactory` adapter.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, Local, Utc};
use cron::Schedule;
use tracing::info;

use crate::metrics::{Metrics, SchedulerEvent};
use crate::run_manager::{RunManager, RunState, StartError};
use crate::store::{Db, Task};
use crate::task_params::GateError;
use crate::timer_core::{
    ScheduleExtension, ScheduleSpec, TimerCore, TimerRunner, TimerRunnerError, TimerRunnerFactory,
};

#[allow(unused_imports)]
pub(crate) use crate::timer_yaml::dispatch;

/// Misfire window retained for YAML/Cron compatibility.  Timer Core receives
/// this as an extension policy instead of knowing Cron semantics.
#[allow(
    dead_code,
    reason = "legacy one-hour misfire helper remains for compatibility"
)]
const MISFIRE_WINDOW_SECS: i64 = 60 * 60;

/// Normalize standard 5/6-field Cron into the seven-field form accepted by
/// the current Cron extension.
pub fn normalize_cron(expr: &str) -> String {
    let expr = expr.trim();
    if expr.starts_with('@') {
        return expr.to_string();
    }
    let parts: Vec<&str> = expr.split_whitespace().collect();
    match parts.len() {
        5 => format!("0 {} *", expr),
        6 => format!("0 {}", expr),
        _ => expr.to_string(),
    }
}

pub fn validate_cron(expr: &str) -> bool {
    Schedule::from_str(&normalize_cron(expr)).is_ok()
}

#[derive(Debug, Default)]
pub(crate) struct CronExtension;

impl ScheduleExtension for CronExtension {
    fn next_after(
        &self,
        schedule: &ScheduleSpec,
        after: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, String> {
        let expression = cron_expression(schedule)?;
        let schedule = Schedule::from_str(&normalize_cron(expression))
            .map_err(|error| format!("invalid cron schedule: {error}"))?;
        let local_after = after.with_timezone(&Local);
        Ok(schedule
            .after(&local_after)
            .next()
            .and_then(|value| DateTime::<Utc>::from_timestamp(value.timestamp(), 0)))
    }

    fn latest_due(
        &self,
        schedule: &ScheduleSpec,
        now: DateTime<Utc>,
        lookback: std::time::Duration,
    ) -> Result<Option<DateTime<Utc>>, String> {
        let expression = cron_expression(schedule)?;
        let schedule = Schedule::from_str(&normalize_cron(expression))
            .map_err(|error| format!("invalid cron schedule: {error}"))?;
        let local_now = now.with_timezone(&Local);
        let window_start = local_now
            - chrono::Duration::from_std(lookback)
                .map_err(|error| format!("invalid Cron lookback: {error}"))?;
        Ok(schedule
            .after(&window_start)
            .take_while(|value| *value <= local_now)
            .last()
            .and_then(|value| DateTime::<Utc>::from_timestamp(value.timestamp(), 0)))
    }
}

fn cron_expression(schedule: &ScheduleSpec) -> Result<&str, String> {
    if schedule.kind != "cron" {
        return Err(format!("unsupported schedule extension: {}", schedule.kind));
    }
    schedule
        .value
        .get("expression")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "cron schedule misses expression".into())
}

pub struct Scheduler {
    core: Arc<TimerCore>,
    runner: Arc<dyn TimerRunner>,
    cron: Arc<CronExtension>,
}

/// Legacy error surface retained for the task REST adapter. Timer Core uses
/// `TimerRunnerError`; this enum keeps callers from depending on the generic
/// runner boundary while the YAML adapter remains the active runner.
#[derive(Debug)]
pub enum RunNowError {
    ScriptMissing,
    ScriptInvalid(String),
    ParamStale(GateError),
    Start(StartError),
}

#[allow(
    dead_code,
    reason = "Timer Core lifecycle façade is consumed by task/package adapters incrementally"
)]
impl Scheduler {
    pub(crate) fn new<A: TimerRunnerFactory>(db: Db, adapter: A, runs: Arc<RunManager>) -> Self {
        Self {
            core: TimerCore::new(db.clone()),
            runner: adapter.into_timer_runner(db, runs),
            cron: Arc::new(CronExtension),
        }
    }

    pub async fn start(&self) {
        info!("timer core started with cron extension");
        self.core.start(self.cron.clone(), self.runner.clone());
    }

    /// Compatibility façade for `POST /api/tasks/:id/run`.
    pub async fn run_now(&self, task: &Task) -> Result<String, RunNowError> {
        let generic = crate::timer_yaml::timer_from_legacy(task)
            .map_err(|error| RunNowError::ScriptInvalid(error.to_string()))?;
        self.core
            .submit_now(generic, self.runner.clone())
            .await
            .map_err(map_run_now_error)
            .map(|run| run.run_id)
    }

    pub async fn cancel_task(&self, task_id: &str) -> anyhow::Result<()> {
        self.core.cancel_task(task_id, self.runner.clone()).await
    }

    pub async fn suspend_task(&self, task_id: &str, reason: &str) -> anyhow::Result<()> {
        self.core.suspend_task(task_id, reason).await
    }

    pub async fn resume_task(&self, task_id: &str) -> anyhow::Result<()> {
        self.core.resume_task(task_id, self.cron.as_ref()).await
    }

    pub async fn on_app_package_uninstalled(&self, package: &str) -> anyhow::Result<usize> {
        self.core.on_app_package_uninstalled(package).await
    }

    pub async fn next_wakeup(&self) -> anyhow::Result<Option<DateTime<Utc>>> {
        self.core.next_wakeup().await
    }

    pub fn notify_tasks_changed(&self) {
        self.core.notify_changed();
    }

    /// Existing update/install workload gate.  It remains a Cron extension
    /// query over the legacy adapter rows, not a Timer Core responsibility.
    pub fn next_enabled_trigger_in_secs(&self) -> Option<i64> {
        let tasks = self.core.db().list_tasks().ok()?;
        next_enabled_trigger_in_secs_from(&tasks, Local::now())
    }
}

fn map_run_now_error(error: TimerRunnerError) -> RunNowError {
    match error {
        TimerRunnerError::DependencyMissing(message) if message == "脚本不存在" => {
            RunNowError::ScriptMissing
        }
        TimerRunnerError::DependencyMissing(message) => RunNowError::ScriptInvalid(message),
        TimerRunnerError::Invalid(message) | TimerRunnerError::Other(message) => {
            RunNowError::ScriptInvalid(message)
        }
        TimerRunnerError::ParamStale {
            stored, current, ..
        } => RunNowError::ParamStale(GateError::SignatureMismatch { stored, current }),
        TimerRunnerError::Conflict(record) => RunNowError::Start(StartError::Conflict(record)),
        TimerRunnerError::ShuttingDown => RunNowError::Start(StartError::ShuttingDown),
    }
}

#[allow(
    dead_code,
    reason = "legacy scheduler tests and observability adapters retain this helper"
)]
fn now_utc_string() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[allow(
    dead_code,
    reason = "legacy scheduler metrics adapter retained for compatibility"
)]
fn record_scheduler_trigger_latency(metrics: &Metrics, scheduled_at: i64) {
    let now_millis = Utc::now().timestamp_millis();
    let scheduled_millis = scheduled_at.saturating_mul(1_000);
    metrics.record_scheduler_trigger(now_millis.saturating_sub(scheduled_millis).max(0) as u64);
}

#[allow(
    dead_code,
    reason = "legacy scheduler metrics adapter retained for compatibility"
)]
fn record_scheduler_event(metrics: &Metrics, event: SchedulerEvent) {
    metrics.record_scheduler_event(event);
}

#[allow(
    dead_code,
    reason = "legacy scheduler metrics adapter retained for compatibility"
)]
fn record_scheduler_failure(metrics: &Metrics) {
    record_scheduler_event(metrics, SchedulerEvent::Failed);
}

#[allow(
    dead_code,
    reason = "legacy scheduled-run state mapping retained for adapters"
)]
fn persisted_scheduled_state(state: RunState) -> Option<&'static str> {
    match state {
        RunState::Success => Some("success"),
        RunState::Failed => Some("failed"),
        RunState::Cancelled => Some("skipped"),
        RunState::Starting | RunState::Running | RunState::Stopping => None,
    }
}

/// Cron preview retained for the existing task REST response.
pub fn next_run(cron_expr: &str) -> Option<DateTime<Local>> {
    let schedule = Schedule::from_str(&normalize_cron(cron_expr)).ok()?;
    schedule.after(&Local::now()).next()
}

/// Compatibility helper for callers that need the legacy one-hour misfire
/// policy. The Timer Core itself receives the lookback from its extension API.
#[allow(
    dead_code,
    reason = "legacy one-hour misfire helper remains for compatibility"
)]
pub(crate) fn latest_due_trigger(
    sched: &Schedule,
    now: DateTime<Local>,
) -> Option<DateTime<Local>> {
    let window_start = now - chrono::Duration::seconds(MISFIRE_WINDOW_SECS);
    sched
        .after(&window_start)
        .take_while(|value| *value <= now)
        .last()
}

/// Pure Cron extension helper used by the update workload gate.
pub fn next_enabled_trigger_in_secs_from(tasks: &[Task], now: DateTime<Local>) -> Option<i64> {
    let mut best: Option<i64> = None;
    for task in tasks {
        if !task.enabled {
            continue;
        }
        let Ok(schedule) = Schedule::from_str(&normalize_cron(&task.cron)) else {
            continue;
        };
        let probe = now - chrono::Duration::nanoseconds(1);
        let Some(next) = schedule.after(&probe).next() else {
            continue;
        };
        let seconds = (next - now).num_seconds().max(0);
        best = Some(best.map_or(seconds, |current| current.min(seconds)));
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    fn task(id: &str, cron: &str, enabled: bool) -> Task {
        Task {
            id: id.into(),
            name: id.into(),
            cron: cron.into(),
            script_id: "com.example/daily.yaml".into(),
            device_id: "d".into(),
            enabled,
            last_result: None,
            last_run_at: None,
            created_at: "2026-08-29T00:00:00Z".into(),
            args_json: "{}".into(),
            param_signature: "psig1|".into(),
        }
    }

    #[test]
    fn cron_extension_is_the_only_place_that_reads_expression() {
        let extension = CronExtension;
        let schedule =
            ScheduleSpec::new("cron", serde_json::json!({"expression": "*/5 * * * *"})).unwrap();
        let now = Local
            .with_ymd_and_hms(2026, 8, 31, 10, 0, 20)
            .single()
            .unwrap()
            .with_timezone(&Utc);
        let next = extension.next_after(&schedule, now).unwrap().unwrap();
        assert_eq!(next.with_timezone(&Local).minute(), 5);
    }

    #[test]
    fn next_enabled_trigger_preserves_legacy_gate_semantics() {
        let now = Local
            .with_ymd_and_hms(2026, 8, 31, 10, 0, 20)
            .single()
            .unwrap();
        assert_eq!(
            next_enabled_trigger_in_secs_from(&[task("a", "*/5 * * * *", true)], now),
            Some(280)
        );
        assert_eq!(
            next_enabled_trigger_in_secs_from(&[task("a", "*/5 * * * *", false)], now),
            None
        );
    }

    #[test]
    fn misfire_window_is_still_one_hour() {
        assert_eq!(MISFIRE_WINDOW_SECS, 3600);
    }
}
