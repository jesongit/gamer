//! Legacy YAML task adapter for Timer Core.
//!
//! This is the only timer-side module that knows the current ScriptStore,
//! typed parameter snapshot, and `RunTarget::Script`.  It translates the
//! generic Task payload into the existing RunManager request so YAML runs
//! scheduled through the unified task API remain compatible.
//!
//! P11.1（ADR-12）：Task 的 `runner.payload` 是 runner 私有不透明值。本 runner
//! 约定 `payload = {args: <稀疏或全量参数>}`；旧数据里可能还带
//! `param_signature`（有则继续做过期门禁），新保存路径不带签名——运行时按
//! 脚本当前声明重绑参数（存活值保留、新参数取默认值、必填缺失报错）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::core::RunRequest;
use crate::run_manager::{FinishHook, RunManager, RunOutcome, RunSource, StartError};
use crate::scripts::ScriptStore;
use crate::store::Db;
use crate::task_params::{self, GateError};
use crate::timer_core::{
    TimerCompletion, TimerOutcome, TimerRun, TimerRunner, TimerRunnerError, TimerRunnerFactory,
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

/// YAML runner 的不透明 payload 视图：`{args, param_signature?}`。
struct YamlPayload {
    script_id: String,
    args: Value,
    /// 旧数据携带的 psig1 快照签名；新保存路径为 None（不做过期门禁）。
    param_signature: Option<String>,
}

/// Translate the generic RunRequest into the YAML runner payload view only at
/// the YAML runner boundary. The Timer Core and Scheduler never inspect it.
fn payload_from_request(request: &RunRequest) -> Result<YamlPayload, String> {
    let payload = request
        .payload
        .as_value()
        .as_object()
        .ok_or_else(|| "YAML runner payload must be an object".to_string())?;
    let args = payload
        .get("args")
        .cloned()
        .ok_or_else(|| "YAML runner payload misses args".to_string())?;
    Ok(YamlPayload {
        script_id: request.entrypoint.clone(),
        args,
        param_signature: payload
            .get("param_signature")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
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
        let payload = payload_from_request(&request).map_err(TimerRunnerError::Invalid)?;
        let task_args = match task_params::gate_task(
            &self.scripts,
            &payload.script_id,
            &payload.args,
            payload.param_signature.as_deref(),
        ) {
            Ok(args) => args,
            Err(error) => {
                tracing::warn!(
                    task = %task_id,
                    script = %payload.script_id,
                    reason = %error.reason(),
                    detail = %error.message(),
                    "YAML timer runner rejected task parameters"
                );
                return Err(map_gate_error(error));
            }
        };
        tracing::info!(
            task = %task_id,
            script = %payload.script_id,
            params = %task_args.names.join(","),
            signature = %task_args.signature,
            signature_short = %task_params::signature_short_code(&task_args.signature),
            "YAML timer task parameters confirmed"
        );
        let req = crate::engine::yaml_start_request(
            request.app.clone(),
            crate::engine::RunTarget::Script {
                script_id: payload.script_id.clone(),
                start_index: 0,
            },
            if scheduled_at.is_some() {
                RunSource::Scheduled
            } else {
                RunSource::TaskNow
            },
            Some(task_id.to_string()),
            scheduled_at,
            task_args.overrides,
            false,
        )
        .map_err(|error| TimerRunnerError::Invalid(error.to_string()))?;
        let hook = yaml_finish_hook(
            self.db.clone(),
            request.app.device_id.to_string(),
            payload.script_id.clone(),
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
        GateError::SignatureMismatch { stored, current } => TimerRunnerError::ParamStale(
            GateError::SignatureMismatch {
                stored: stored.clone(),
                current: current.clone(),
            }
            .message(),
        ),
    }
}

fn map_start_error(error: StartError) -> TimerRunnerError {
    match error {
        StartError::Conflict(record) => TimerRunnerError::Conflict(record),
        StartError::ShuttingDown => TimerRunnerError::ShuttingDown,
    }
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
