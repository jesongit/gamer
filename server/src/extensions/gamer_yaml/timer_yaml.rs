//! gamer.yaml 的 Timer Core 任务适配器（v3-only）。
//!
//! This is the only timer-side module that knows the current ResourceStore,
//! typed parameter snapshot, and `RunTarget::Script`.  It translates the
//! generic Task payload into the existing RunManager request so YAML runs
//! scheduled through the unified task API remain compatible.
//!
//! P11.1（ADR-12）：Task 的 `runner.payload` 是 runner 私有不透明值。本 runner
//! 约定 `payload = {args: <稀疏或全量参数>}`；旧数据里可能还带
//! `param_signature`（有则继续做过期门禁），新保存路径不带签名——运行时按
//! 脚本当前声明重绑参数（存活值保留、新参数取默认值、必填缺失报错）。
//!
//! P11.6（POST /api/runs 统一执行入口）：手动/函数测试运行经同一 runner。
//! `task_id` 为空 = 手动 ad-hoc 运行：`entrypoint` = `<pkg>/<脚本>.yaml` 或
//! `<pkg>/<文件短路径>.yaml#<函数名>`，payload = `{args?, start_index?,
//! function?}`（稀疏参数按声明解析并合并默认值，诊断 → `InvalidDetail`）。
//!
//! P11.2（ADR-13）：runner 注册由扩展生命周期驱动。本文件另提供
//! [`YamlTimerRunnerRegistrar`]——`TimerRunnerRegistrar` 钩子的 YAML 侧实现：
//! gamer.yaml 扩展 start → 构造并注册 runner（owner=扩展 id），stop/disable/
//! uninstall → 注销并挂起其名下任务。按扩展 id 特判 gamer.yaml 是本 Wave 的
//! 过渡缝（钩子接口本身通用），Wave3 把 runner 构造移进扩展边界后本类型随
//! YAML 栈一并迁移。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::core::RunRequest;
use crate::extensions::gamer_yaml::task_params::{self, GateError};
use crate::resources::ResourceKind as RK;
use crate::resources::ResourceStore;
use crate::run_manager::{FinishHook, RunManager, RunOutcome, RunSource, StartError};
use crate::store::Db;
use crate::timer_core::{TimerCompletion, TimerOutcome, TimerRun, TimerRunner, TimerRunnerError};

// P12.3：entrypoint 参数 schema 描述（契约 §7）。本模块声明挂载（gamer_yaml
// 的 mod.rs 属并行任务地盘），物理文件为 gamer_yaml/entrypoint_descriptor.rs。
#[path = "entrypoint_descriptor.rs"]
pub(crate) mod entrypoint_descriptor;

pub(crate) struct YamlTimerRunner {
    db: Db,
    runs: Arc<RunManager>,
    scripts: Arc<ResourceStore>,
}

impl YamlTimerRunner {
    pub(crate) fn new(db: Db, runs: Arc<RunManager>, scripts: Arc<ResourceStore>) -> Self {
        Self { db, runs, scripts }
    }

    /// P12.3（契约 §7）：本 runner 名下 entrypoint 的参数 schema 描述器
    /// （`GET /api/runners/:runner_id/entrypoint` 数据源；Core 经窄 trait 消费）。
    pub(crate) fn entrypoint_describer(
        &self,
    ) -> Arc<dyn crate::scheduler::EntrypointDescriber> {
        Arc::new(entrypoint_descriptor::StoreEntrypointDescriber::new(
            self.scripts.clone(),
        ))
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
        if task_id.is_empty() {
            return self.submit_manual(request, on_complete).await;
        }
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
        let req = crate::extensions::gamer_yaml::yaml_start_request(
            request.app.clone(),
            crate::extensions::gamer_yaml::run_target::RunTarget::Script {
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
        Ok(TimerRun::new(record.run_id))
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

/// 手动运行 payload 视图：`{args?, start_index?, function?}`。
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
struct ManualPayload {
    #[serde(default)]
    args: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    start_index: Option<usize>,
    #[serde(default)]
    function: Option<String>,
}

fn invalid_detail(message: impl Into<String>, detail: serde_json::Value) -> TimerRunnerError {
    TimerRunnerError::InvalidDetail {
        message: message.into(),
        detail,
    }
}

impl YamlTimerRunner {
    /// POST /api/runs 手动路径（task_id 为空）：entrypoint + 稀疏 args 在
    /// 本 runner 边界内翻译为 RunTarget::Script / Function 并交 RunManager。
    async fn submit_manual(
        &self,
        request: RunRequest,
        on_complete: Arc<dyn Fn(TimerCompletion) + Send + Sync>,
    ) -> Result<TimerRun, TimerRunnerError> {
        let payload: ManualPayload = if request.payload.as_value().is_null() {
            ManualPayload::default()
        } else {
            serde_json::from_value(request.payload.as_value().clone()).map_err(|error| {
                invalid_detail(
                    "gamer.yaml payload 无效",
                    serde_json::json!({
                        "error": "invalid_payload",
                        "message": error.to_string(),
                    }),
                )
            })?
        };
        let entrypoint = request.entrypoint.clone();
        let app = request.app.clone();
        let args: serde_json::Map<String, Value> = payload.args.clone().unwrap_or_default();
        let target = if let Some((base, func)) = entrypoint.clone().rsplit_once('#') {
            let (pkg, file) = base.split_once('/').ok_or_else(|| {
                invalid_detail(
                    "非法函数目标 entrypoint",
                    serde_json::json!({
                        "error": "invalid_payload", "entrypoint": entrypoint,
                    }),
                )
            })?;
            let file = file
                .trim()
                .trim_end_matches(".yaml")
                .trim_end_matches(".yml")
                .to_string();
            crate::extensions::gamer_yaml::run_target::RunTarget::Function {
                pkg: pkg.to_string(),
                file,
                function: Some(payload.function.clone().unwrap_or_else(|| func.to_string())),
                start_index: payload.start_index.unwrap_or(0),
            }
        } else {
            crate::extensions::gamer_yaml::run_target::RunTarget::Script {
                script_id: entrypoint.clone(),
                start_index: payload.start_index.unwrap_or(0),
            }
        };
        // 存在性先行（与旧运行端点的 404 语义对齐，此处统一为结构化失败：
        // 手动运行无任务可挂起）
        match &target {
            crate::extensions::gamer_yaml::run_target::RunTarget::Script { script_id, .. } => {
                let exists = self
                    .scripts
                    .get_text(RK::Scripts, script_id)
                    .map_err(|error| invalid_detail(error.to_string(), serde_json::json!([])))?
                    .is_some();
                if !exists {
                    return Err(invalid_detail(
                        "脚本不存在",
                        serde_json::json!({ "error": "not_found", "resource": script_id }),
                    ));
                }
            }
            crate::extensions::gamer_yaml::run_target::RunTarget::Function { pkg, file, .. } => {
                let rel = format!("{pkg}/{file}.yaml");
                let exists = self
                    .scripts
                    .get_text(RK::Functions, &rel)
                    .map_err(|error| invalid_detail(error.to_string(), serde_json::json!([])))?
                    .is_some();
                if !exists {
                    return Err(invalid_detail(
                        "函数文件不存在",
                        serde_json::json!({ "error": "not_found", "resource": rel }),
                    ));
                }
            }
        }
        // 稀疏 args → 按声明解析 + 默认值合并（blocking 池内做磁盘快照 + 严格
        // 解析）。P12.3：version:3 脚本与 v2 兼容失败后的 v3 函数库在此分流，
        // v3 参数绑定/缺必填校验与 v2 同口径（invalid_args 诊断）。
        let scripts = self.scripts.clone();
        let bound = {
            let target = target.clone();
            let args_owned = args.clone();
            tokio::task::spawn_blocking(move || {
                task_params::resolve_manual_entry_args(&scripts, &target, &args_owned)
            })
            .await
            .map_err(|error| {
                invalid_detail(format!("参数解析任务失败: {error}"), serde_json::json!([]))
            })?
            .map_err(|diagnostics| {
                invalid_detail(
                    "参数解析失败",
                    serde_json::json!({
                        "error": "invalid_args",
                        "diagnostics": diagnostics,
                    }),
                )
            })?
        };
        let start_request = crate::extensions::gamer_yaml::yaml_start_request(
            app,
            target,
            RunSource::Manual,
            None,
            None,
            bound.overrides,
            true,
        )
        .map_err(|error| invalid_detail(error.to_string(), serde_json::json!([])))?;
        let db = self.db.clone();
        let hook: FinishHook = Arc::new(move |record, outcome| {
            write_manual_terminal_log(&db, record, outcome);
            on_complete(TimerCompletion {
                task_id: String::new(),
                scheduled_at: None,
                run_id: record.run_id.clone(),
                outcome: match outcome {
                    RunOutcome::Success(_) => TimerOutcome::Success,
                    RunOutcome::Failed(message, _) => TimerOutcome::Failed(message.clone()),
                    RunOutcome::Cancelled(_) => TimerOutcome::Cancelled,
                },
            });
        });
        let record = self
            .runs
            .submit(start_request, Some(hook))
            .map_err(map_start_error)?;
        Ok(TimerRun {
            run_id: record.run_id,
            detail: Some(serde_json::json!({ "resolved_args": bound.resolved })),
        })
    }
}

/// 手动运行终态摘要行落库（realtime 模式引擎日志已实时入库，只补终局提示，
/// 与统一执行入口的手动语义对齐）。
fn write_manual_terminal_log(
    db: &Db,
    record: &crate::run_manager::RunRecord,
    outcome: &RunOutcome,
) {
    let (level, message) = match outcome {
        RunOutcome::Success(_) => ("success", "脚本执行完成".to_string()),
        RunOutcome::Failed(message, _) => ("error", format!("脚本执行失败: {message}")),
        RunOutcome::Cancelled(_) => ("info", "脚本已停止".to_string()),
    };
    let db = db.clone();
    let device_id = record.device_id.clone();
    let script_id = record.script_id.clone();
    tokio::spawn(async move {
        let _ = db
            .add_log_async(&device_id, &script_id, level, &message)
            .await;
    });
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

/// P11.2 / ADR-13：ExtensionService 生命周期回调的 YAML 侧绑定。扩展边界在
/// 此自声明两件事：本扩展拥有的 runner 如何构造（`extension_started`），以及
/// 本扩展的执行模型是按调用惰性实例化（`executes_without_instance`——`start`
/// 只表示 runner 提供方在线，不启动常驻实例）。注销路径对任意 owner 通用
/// （owner 名下没有 runner 时为幂等 no-op）。
pub(crate) struct YamlTimerRunnerRegistrar {
    scheduler: Arc<crate::scheduler::Scheduler>,
    db: Db,
    runs: Arc<RunManager>,
    scripts: Arc<ResourceStore>,
}

impl YamlTimerRunnerRegistrar {
    pub(crate) fn new(
        scheduler: Arc<crate::scheduler::Scheduler>,
        db: Db,
        runs: Arc<RunManager>,
        scripts: Arc<ResourceStore>,
    ) -> Self {
        Self {
            scheduler,
            db,
            runs,
            scripts,
        }
    }
}

#[async_trait]
impl crate::extensions::TimerRunnerRegistrar for YamlTimerRunnerRegistrar {
    async fn extension_started(&self, extension_id: &str) -> anyhow::Result<()> {
        if extension_id != crate::extensions::gamer_yaml::yaml_extension::YAML_EXTENSION_ID {
            return Ok(());
        }
        let runner = Arc::new(YamlTimerRunner::new(
            self.db.clone(),
            self.runs.clone(),
            self.scripts.clone(),
        ));
        self.scheduler
            .register_extension_runner(
                crate::extensions::gamer_yaml::yaml_extension::YAML_EXTENSION_ID,
                extension_id,
                runner.clone(),
            )
            .await?;
        // P12.3：entrypoint 参数 schema 描述器与 runner 同生命周期注册/注销
        self.scheduler.register_entrypoint_describer(
            crate::extensions::gamer_yaml::yaml_extension::YAML_EXTENSION_ID,
            extension_id,
            runner.entrypoint_describer(),
        );
        Ok(())
    }

    async fn extension_stopped(&self, extension_id: &str) -> anyhow::Result<()> {
        self.scheduler
            .unregister_extension_owner(extension_id)
            .await
            .map(|_| ())
    }

    fn executes_without_instance(&self, extension_id: &str) -> bool {
        extension_id == crate::extensions::gamer_yaml::yaml_extension::YAML_EXTENSION_ID
    }
}
