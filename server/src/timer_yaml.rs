//! Legacy YAML task adapter for Timer Core.
//!
//! This is the only timer-side module that knows the current ScriptStore,
//! typed parameter snapshot, and `RunTarget::Script`.  It translates the
//! generic TimerTask payload into the existing RunManager request so the
//! public task API and existing YAML runs remain compatible.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Local, Utc};
use serde_json::Value;

use crate::core::{AppContext, RunRequest};
use crate::run_manager::{FinishHook, RunManager, RunOutcome, RunSource, StartError};
use crate::scripts::ScriptStore;
use crate::store::{Db, Task};
use crate::task_params::{self, GateError};
use crate::timer_core::{
    ScheduleSpec, TimerCompletion, TimerCore, TimerOutcome, TimerRun, TimerRunner,
    TimerRunnerError, TimerRunnerFactory, TimerTask, TimerTaskState,
};

pub(crate) struct YamlTimerRunner {
    db: Db,
    runs: Arc<RunManager>,
    scripts: Arc<ScriptStore>,
}

impl YamlTimerRunner {
    fn new(db: Db, runs: Arc<RunManager>, scripts: Arc<ScriptStore>) -> Self {
        Self { db, runs, scripts }
    }
}

impl TimerRunnerFactory for Arc<ScriptStore> {
    fn into_timer_runner(self, db: Db, runs: Arc<RunManager>) -> Arc<dyn TimerRunner> {
        Arc::new(YamlTimerRunner::new(db, runs, self))
    }
}

#[async_trait]
impl TimerRunner for YamlTimerRunner {
    fn runner_id(&self) -> &str {
        "gamer.yaml"
    }

    async fn submit(
        &self,
        request: RunRequest,
        task_id: &str,
        scheduled_at: Option<i64>,
        on_complete: Arc<dyn Fn(TimerCompletion) + Send + Sync>,
    ) -> Result<TimerRun, TimerRunnerError> {
        let legacy = legacy_from_request(&request, task_id).map_err(TimerRunnerError::Invalid)?;
        let task = legacy.clone();
        let task_args = match task_params::gate_task(&self.scripts, &legacy) {
            Ok(args) => args,
            Err(error) => {
                tracing::warn!(
                    task = %task.id,
                    script = %task.script_id,
                    reason = %error.reason(),
                    detail = %error.message(),
                    "YAML timer runner rejected task parameters"
                );
                return Err(map_gate_error(error));
            }
        };
        tracing::info!(
            task = %task.id,
            script = %task.script_id,
            params = %task_args.names.join(","),
            signature = %task_args.signature,
            signature_short = %task_params::signature_short_code(&task_args.signature),
            "YAML timer task parameters confirmed"
        );
        let req = crate::engine::yaml_start_request(
            request.app.clone(),
            crate::engine::RunTarget::Script {
                script_id: request.entrypoint.clone(),
                start_index: 0,
            },
            if scheduled_at.is_some() {
                RunSource::Scheduled
            } else {
                RunSource::TaskNow
            },
            Some(task.id.clone()),
            scheduled_at,
            task_args.overrides,
            false,
        )
        .map_err(|error| TimerRunnerError::Invalid(error.to_string()))?;
        let hook = yaml_finish_hook(
            self.db.clone(),
            request.app.device_id.to_string(),
            request.entrypoint.clone(),
            task_id.to_string(),
            scheduled_at,
            on_complete,
        );
        let record = self.runs.submit(req, Some(hook)).map_err(map_start_error)?;
        Ok(TimerRun {
            run_id: record.run_id,
        })
    }

    async fn cancel(&self, run_id: &str) -> Result<(), TimerRunnerError> {
        match self.runs.cancel(run_id) {
            crate::run_manager::CancelOutcome::Accepted => Ok(()),
            crate::run_manager::CancelOutcome::NotFound => {
                Err(TimerRunnerError::Other(format!("run not found: {run_id}")))
            }
            crate::run_manager::CancelOutcome::AlreadyFinished(_) => Ok(()),
        }
    }
}

fn map_gate_error(error: GateError) -> TimerRunnerError {
    match error {
        GateError::ScriptMissing => TimerRunnerError::DependencyMissing("脚本不存在".into()),
        GateError::ScriptInvalid(diagnostics) => {
            TimerRunnerError::Invalid(GateError::ScriptInvalid(diagnostics).message())
        }
        GateError::SignatureMismatch { stored, current } => TimerRunnerError::ParamStale {
            message: GateError::SignatureMismatch {
                stored: stored.clone(),
                current: current.clone(),
            }
            .message(),
            stored,
            current,
        },
    }
}

fn map_start_error(error: StartError) -> TimerRunnerError {
    match error {
        StartError::Conflict(record) => TimerRunnerError::Conflict(record),
        StartError::ShuttingDown => TimerRunnerError::ShuttingDown,
    }
}

/// Translate the generic RunRequest into the legacy YAML task shape only at
/// the YAML runner boundary. The Timer Core and Scheduler never inspect it.
fn legacy_from_request(request: &RunRequest, task_id: &str) -> Result<Task, String> {
    let payload = request
        .payload
        .as_value()
        .as_object()
        .ok_or_else(|| "YAML runner payload must be an object".to_string())?;
    let args = payload
        .get("args")
        .cloned()
        .ok_or_else(|| "YAML runner payload misses args".to_string())?;
    let args_json = serde_json::to_string(&args).map_err(|error| error.to_string())?;
    let signature = payload
        .get("param_signature")
        .and_then(Value::as_str)
        .ok_or_else(|| "YAML runner payload misses param_signature".to_string())?;
    Ok(Task {
        id: task_id.to_string(),
        name: request.entrypoint.clone(),
        cron: String::new(),
        script_id: request.entrypoint.clone(),
        device_id: request.app.device_id.to_string(),
        enabled: true,
        last_result: None,
        last_run_at: None,
        created_at: Utc::now().to_rfc3339(),
        args_json,
        param_signature: signature.to_string(),
    })
}

fn yaml_finish_hook(
    db: Db,
    device_id: String,
    script_id: String,
    task_id: String,
    scheduled_at: Option<i64>,
    on_complete: Arc<dyn Fn(TimerCompletion) + Send + Sync>,
) -> FinishHook {
    Arc::new(move |record, outcome| {
        let logs = outcome.logs().to_vec();
        let log_error = logs
            .iter()
            .find(|(level, _)| level == "error")
            .map(|(_, message)| message.clone());
        let db = db.clone();
        let device_id = device_id.clone();
        let script_id = script_id.clone();
        tokio::spawn(async move {
            for (level, message) in logs {
                let _ = db
                    .add_log_async(&device_id, &script_id, &level, &message)
                    .await;
            }
        });
        let timer_outcome = match outcome {
            RunOutcome::Success(_) if log_error.is_none() => TimerOutcome::Success,
            RunOutcome::Success(_) => {
                TimerOutcome::Failed(log_error.unwrap_or_else(|| "执行日志包含错误".to_string()))
            }
            RunOutcome::Failed(message, _) => TimerOutcome::Failed(message.clone()),
            RunOutcome::Cancelled(_) => TimerOutcome::Cancelled,
        };
        on_complete(TimerCompletion {
            task_id: task_id.clone(),
            scheduled_at,
            run_id: record.run_id.clone(),
            outcome: timer_outcome,
        });
    })
}

/// Convert the current legacy task row to the generic Timer Core model.
pub(crate) fn timer_from_legacy(task: &Task) -> anyhow::Result<TimerTask> {
    let package = task
        .script_id
        .split_once('/')
        .map(|(package, _)| package)
        .unwrap_or("legacy");
    let app = AppContext::from_legacy_package(&task.device_id, package)?;
    let args: Value = serde_json::from_str(&task.args_json)?;
    let payload = serde_json::json!({
        "args": args,
        "param_signature": task.param_signature,
    });
    let schedule = ScheduleSpec::new("cron", serde_json::json!({"expression": task.cron}))?;
    let now = Utc::now();
    let mut timer = TimerTask::new(
        task.id.clone(),
        task.name.clone(),
        app,
        "gamer.yaml",
        task.script_id.clone(),
        payload,
        schedule,
    )?;
    timer.enabled = task.enabled;
    timer.state = if task.enabled {
        TimerTaskState::Active
    } else {
        TimerTaskState::Suspended
    };
    timer.suspend_reason = (!task.enabled).then(|| "disabled".to_string());
    timer.last_result = task.last_result.clone();
    timer.last_run_at = task
        .last_run_at
        .as_deref()
        .and_then(|value| value.parse::<DateTime<Utc>>().ok());
    timer.created_at = task.created_at.parse::<DateTime<Utc>>().unwrap_or(now);
    timer.updated_at = now;
    Ok(timer)
}

fn legacy_from_timer(task: &TimerTask) -> Result<Task, String> {
    let payload = task
        .payload
        .as_object()
        .ok_or_else(|| "YAML runner payload must be an object".to_string())?;
    let args = payload
        .get("args")
        .cloned()
        .ok_or_else(|| "YAML runner payload misses args".to_string())?;
    let args_json = serde_json::to_string(&args).map_err(|error| error.to_string())?;
    let signature = payload
        .get("param_signature")
        .and_then(Value::as_str)
        .ok_or_else(|| "YAML runner payload misses param_signature".to_string())?;
    let expression = task
        .schedule
        .value
        .get("expression")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok(Task {
        id: task.id.clone(),
        name: task.name.clone(),
        cron: expression,
        script_id: task.entrypoint.clone(),
        device_id: task.app.device_id.to_string(),
        enabled: task.enabled,
        last_result: task.last_result.clone(),
        last_run_at: task.last_run_at.map(|value| value.to_rfc3339()),
        created_at: task.created_at.to_rfc3339(),
        args_json,
        param_signature: signature.to_string(),
    })
}

/// Compatibility helper retained for the existing scheduler tests and callers.
#[allow(
    dead_code,
    reason = "legacy callers are retained while the Timer Core adapter rolls out"
)]
pub(crate) async fn dispatch(
    db: &Db,
    runs: &Arc<RunManager>,
    scripts: &Arc<ScriptStore>,
    task: &Task,
    trigger: Option<DateTime<Local>>,
) {
    let timer = match timer_from_legacy(task) {
        Ok(timer) => timer,
        Err(error) => {
            tracing::error!(task = %task.id, %error, "legacy task conversion failed");
            return;
        }
    };
    let core = TimerCore::new(db.clone());
    let runner = Arc::new(YamlTimerRunner::new(
        db.clone(),
        runs.clone(),
        scripts.clone(),
    ));
    core.dispatch(timer, trigger.map(|value| value.timestamp()), runner)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_task_round_trips_generic_runner_payload() {
        let task = Task {
            id: "task".into(),
            name: "Task".into(),
            cron: "0 * * * * *".into(),
            script_id: "com.example/daily.yaml".into(),
            device_id: "device".into(),
            enabled: true,
            last_result: None,
            last_run_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            args_json: "{}".into(),
            param_signature: "psig1|".into(),
        };
        let generic = timer_from_legacy(&task).unwrap();
        let back = legacy_from_timer(&generic).unwrap();
        assert_eq!(back.script_id, task.script_id);
        assert_eq!(back.args_json, task.args_json);
        assert_eq!(back.param_signature, task.param_signature);
    }
}
