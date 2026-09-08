//! gamer.yaml 的 Timer Core 任务适配器（V1）。
//!
//! This is the only timer-side module that knows the current PackageStore
//! and `RunTarget::Script`.  It translates the generic Task payload into the
//! existing RunManager request so YAML runs scheduled through the unified
//! task API remain compatible.
//!
//! P11.1（ADR-12）：Task 的 `runner.payload` 是 runner 私有不透明值。本 runner
//! 约定 `payload = {args: <稀疏参数覆盖>}`；运行时按脚本当前 Schema 在执行
//! 边界绑定（存活值保留、新参数取默认值、必填缺失报错）——旧 v3 的 psig1
//! 参数签名门禁已随 V1 简化删除。
//!
//! P11.6（POST /api/runs 统一执行入口）：手动/函数测试运行经同一 runner。
//! `task_id` 为空 = 手动 ad-hoc 运行：`entrypoint` = `<pkg>/<脚本>.yaml` 或
//! `<pkg>/<文件短路径>.yaml#<函数名>`，payload = `{args?, start_index?}`。
//!
//! P11.2（ADR-13）：runner 注册由扩展生命周期驱动（[`YamlTimerRunnerRegistrar`]）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::core::RunRequest;
use crate::extensions::gamer_yaml::resources::{function_entry, script_entry};
use crate::extensions::gamer_yaml::YAML_EXTENSION_ID;
use crate::resources::PackageStore;
use crate::run_manager::{FinishHook, RunManager, RunOutcome, RunSource, StartError};
use crate::store::Db;
use crate::timer_core::{TimerCompletion, TimerOutcome, TimerRun, TimerRunner, TimerRunnerError};

// P12.3：entrypoint 参数 schema 描述（契约 §7）。本模块声明挂载，
// 物理文件为 gamer_yaml/entrypoint_descriptor.rs。
#[path = "entrypoint_descriptor.rs"]
pub(crate) mod entrypoint_descriptor;

pub(crate) struct YamlTimerRunner {
    db: Db,
    runs: Arc<RunManager>,
    scripts: Arc<PackageStore>,
}

impl YamlTimerRunner {
    pub(crate) fn new(db: Db, runs: Arc<RunManager>, scripts: Arc<PackageStore>) -> Self {
        Self { db, runs, scripts }
    }

    /// P12.3（契约 §7）：本 runner 名下 entrypoint 的参数 schema 描述器
    /// （`GET /api/runners/:runner_id/entrypoint` 数据源；Core 经窄 trait 消费）。
    pub(crate) fn entrypoint_describer(&self) -> Arc<dyn crate::scheduler::EntrypointDescriber> {
        Arc::new(entrypoint_descriptor::StoreEntrypointDescriber::new(
            self.scripts.clone(),
        ))
    }
}

/// YAML runner 的不透明 payload 视图：`{args?}`（稀疏原始覆盖）。
struct YamlPayload {
    script_id: String,
    args: serde_json::Map<String, Value>,
}

/// Translate the generic RunRequest into the YAML runner payload view only at
/// the YAML runner boundary. The Timer Core and Scheduler never inspect it.
fn payload_from_request(request: &RunRequest) -> Result<YamlPayload, String> {
    let payload = request
        .payload
        .as_value()
        .as_object()
        .ok_or_else(|| "YAML runner payload must be an object".to_string())?;
    let args = payload.get("args").cloned().unwrap_or(Value::Null);
    Ok(YamlPayload {
        script_id: request.entrypoint.clone(),
        args: args.as_object().cloned().unwrap_or_default(),
    })
}

fn script_exists(scripts: &PackageStore, script_id: &str) -> Result<bool, String> {
    script_entry(scripts, script_id)
        .map(|entry| entry.is_some())
        .map_err(|error| error.to_string())
}

fn function_file_exists(scripts: &PackageStore, target: &str) -> Result<bool, String> {
    function_entry(scripts, target)
        .map(|entry| entry.is_some())
        .map_err(|error| error.to_string())
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
        // 存在性先行：脚本缺失 → 依赖缺失（任务保留 enabled 原意）。
        // 参数按当前 Schema 宽松重绑（计划 Phase 4.2）：存活值保留、新增参数
        // 取默认值、被删参数丢弃、必填缺失/类型不符结构化报错（psig1 签名
        // 门禁已随 V1 删除）。
        match script_exists(&self.scripts, &payload.script_id) {
            Ok(true) => {}
            Ok(false) => {
                return Err(TimerRunnerError::DependencyMissing("脚本不存在".into()));
            }
            Err(error) => return Err(TimerRunnerError::Invalid(error)),
        }
        let scripts = self.scripts.clone();
        let script_id = payload.script_id.clone();
        let args_owned = payload.args.clone();
        let bound = tokio::task::spawn_blocking(move || {
            let content = script_entry(&scripts, &script_id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "脚本不存在".to_string())?
                .content;
            let script = crate::extensions::gamer_yaml::syntax::parse_script(&content).map_err(
                |diagnostics| {
                    diagnostics
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("；")
                },
            )?;
            crate::extensions::gamer_yaml::task_params::bind_entry_args(
                &script_id,
                &script.params,
                &args_owned,
                false,
            )
            .map(|bound| bound.resolved)
            .map_err(|errors| {
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("；")
            })
        })
        .await
        .map_err(|error| TimerRunnerError::Invalid(format!("参数解析任务失败: {error}")))?;
        let resolved = match bound {
            Ok(resolved) => resolved,
            Err(message) => {
                tracing::warn!(
                    task = %task_id,
                    script = %payload.script_id,
                    detail = %message,
                    "YAML timer runner rejected task parameters"
                );
                return Err(TimerRunnerError::Invalid(message));
            }
        };
        tracing::info!(
            task = %task_id,
            script = %payload.script_id,
            params = %resolved.keys().cloned().collect::<Vec<_>>().join(","),
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
            resolved,
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

/// 手动路径早期绑定失败（存在性 / 脚本解析 / 参数绑定三类，400 语义分流）。
enum EarlyBindError {
    NotFound(String),
    Parse(Vec<crate::extensions::gamer_yaml::syntax::Diagnostic>),
    Bind(Vec<crate::extensions::gamer_yaml::error::ScriptError>),
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
        // 存在性先行（与运行端点的 404 语义对齐，统一为结构化失败：
        // 手动运行无任务可挂起）+ 早期严格绑定（缺必填/未知键/类型不符在
        // 提交时即 400 invalid_args；resolved_args 供 202 响应展示）。
        let scripts = self.scripts.clone();
        let target_for_bind = target.clone();
        let args_owned = args.clone();
        let bound = tokio::task::spawn_blocking(move || {
            use crate::extensions::gamer_yaml::run_target::RunTarget as T;
            match &target_for_bind {
                T::Script { script_id, .. } => {
                    let content = script_entry(&scripts, script_id)
                        .map_err(|error| EarlyBindError::NotFound(error.to_string()))?
                        .ok_or_else(|| EarlyBindError::NotFound("脚本不存在".into()))?
                        .content;
                    let script = crate::extensions::gamer_yaml::syntax::parse_script(&content)
                        .map_err(EarlyBindError::Parse)?;
                    crate::extensions::gamer_yaml::task_params::bind_entry_args(
                        script_id,
                        &script.params,
                        &args_owned,
                        true,
                    )
                    .map(|bound| bound.resolved)
                    .map_err(EarlyBindError::Bind)
                }
                T::Function {
                    pkg,
                    file,
                    function,
                    ..
                } => {
                    let target_id = format!("{pkg}/{file}.yaml");
                    let content = function_entry(&scripts, &target_id)
                        .map_err(|error| EarlyBindError::NotFound(error.to_string()))?
                        .ok_or_else(|| EarlyBindError::NotFound("函数文件不存在".into()))?
                        .content;
                    let library =
                        crate::extensions::gamer_yaml::syntax::parse_function_library(&content)
                            .map_err(EarlyBindError::Parse)?;
                    let name = match function {
                        Some(name) => name.clone(),
                        None => library
                            .first()
                            .map(|(name, _)| name.clone())
                            .ok_or_else(|| {
                                EarlyBindError::NotFound(format!(
                                    "函数文件 {target_id} 未定义任何函数"
                                ))
                            })?,
                    };
                    let def = library
                        .iter()
                        .find(|(entry, _)| entry == &name)
                        .map(|(_, def)| def)
                        .ok_or_else(|| {
                            EarlyBindError::NotFound(format!("函数 {name} 不在文件 {target_id} 中"))
                        })?;
                    crate::extensions::gamer_yaml::task_params::bind_entry_args(
                        &format!("{target_id}#{name}"),
                        &def.params,
                        &args_owned,
                        true,
                    )
                    .map(|bound| bound.resolved)
                    .map_err(EarlyBindError::Bind)
                }
            }
        })
        .await;
        let bound: Result<serde_json::Map<String, Value>, EarlyBindError> = match bound {
            Ok(inner) => inner,
            Err(error) => Err(EarlyBindError::NotFound(format!(
                "参数解析任务失败: {error}"
            ))),
        };
        let resolved = match bound {
            Ok(resolved) => resolved,
            Err(EarlyBindError::NotFound(message)) => {
                return Err(invalid_detail(
                    message,
                    serde_json::json!({ "error": "not_found" }),
                ));
            }
            Err(EarlyBindError::Parse(diagnostics)) => {
                let text = diagnostics
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("；");
                return Err(invalid_detail(
                    text,
                    serde_json::json!({ "error": "invalid_script", "diagnostics": diagnostics }),
                ));
            }
            Err(EarlyBindError::Bind(diagnostics)) => {
                return Err(invalid_detail(
                    "参数解析失败",
                    serde_json::json!({ "error": "invalid_args", "diagnostics": diagnostics }),
                ));
            }
        };
        let start_request = crate::extensions::gamer_yaml::yaml_start_request(
            app,
            target,
            RunSource::Manual,
            None,
            None,
            resolved.clone(),
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
        // resolved_args = 默认值 ∪ 已校验覆盖（提交边界按当前 Schema 绑定；
        // 执行边界重绑结果一致——同一次运行的声明/覆盖已冻结在请求里）。
        Ok(TimerRun {
            run_id: record.run_id,
            detail: Some(serde_json::json!({ "resolved_args": Value::Object(resolved) })),
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
    scripts: Arc<PackageStore>,
}

impl YamlTimerRunnerRegistrar {
    pub(crate) fn new(
        scheduler: Arc<crate::scheduler::Scheduler>,
        db: Db,
        runs: Arc<RunManager>,
        scripts: Arc<PackageStore>,
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
        if extension_id != YAML_EXTENSION_ID {
            return Ok(());
        }
        let runner = Arc::new(YamlTimerRunner::new(
            self.db.clone(),
            self.runs.clone(),
            self.scripts.clone(),
        ));
        self.scheduler
            .register_extension_runner(YAML_EXTENSION_ID, extension_id, runner.clone())
            .await?;
        // P12.3：entrypoint 参数 schema 描述器与 runner 同生命周期注册/注销
        self.scheduler.register_entrypoint_describer(
            YAML_EXTENSION_ID,
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
        extension_id == YAML_EXTENSION_ID
    }
}
