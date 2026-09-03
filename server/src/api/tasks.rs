//! Scheduled task CRUD and immediate trigger endpoints.
//!
//! 阶段 5（plan §12.3 / CONTRACT §4.3–4.5）：任务保存**完整类型化参数快照**
//! （args_json = 七类 TypedValue 的 JSON 形态对象，与 run API args 同构）+
//! 保存时脚本的 psig1 签名：
//! - 创建/更新接受稀疏或全量 `args`，服务端按脚本当前声明解析为完整快照存储；
//!   缺必填/未知参数 → 400 结构化诊断；
//! - 不带 `args` 且已存快照签名与脚本当前声明不一致 → 409
//!   `param_signature_conflict`（`reconfirm:true` 强制重确认：带 args 全量重拍，
//!   不带 args 存活参数保留原值、新参数取当前默认值）；
//! - 列表/详情带 `param_stale`（签名过期或脚本无效，前端展示"参数已过期"）；
//! - 「立即运行」走已存快照（过同一签名门禁，过期明确失败不空跑）。

use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use super::common::{err_response, run_blocking_api, validate_text_field};
use super::runs::diagnostics_response;
use super::{ApiError, AppState};
use crate::cron_extension::{next_run, validate_cron};
use crate::store::Task;
use crate::task_params::{self, GateError};
use crate::timer_core::{ScheduleSpec, TaskPreset, TimerRunnerError, TimerTask, TimerTaskState};
use crate::{core::AppContext, timer_yaml};

#[derive(Debug)]
enum LegacyRunNowError {
    ScriptMissing,
    ScriptInvalid(String),
    ParamStale(GateError),
    Start(crate::run_manager::StartError),
}

fn map_legacy_run_now_error(error: TimerRunnerError) -> LegacyRunNowError {
    match error {
        TimerRunnerError::DependencyMissing(message) if message == "脚本不存在" => {
            LegacyRunNowError::ScriptMissing
        }
        TimerRunnerError::DependencyMissing(message) => LegacyRunNowError::ScriptInvalid(message),
        TimerRunnerError::Invalid(message) | TimerRunnerError::Other(message) => {
            LegacyRunNowError::ScriptInvalid(message)
        }
        TimerRunnerError::ParamStale {
            stored, current, ..
        } => LegacyRunNowError::ParamStale(GateError::SignatureMismatch { stored, current }),
        TimerRunnerError::Conflict(record) => {
            LegacyRunNowError::Start(crate::run_manager::StartError::Conflict(record))
        }
        TimerRunnerError::ShuttingDown => {
            LegacyRunNowError::Start(crate::run_manager::StartError::ShuttingDown)
        }
    }
}

pub(super) fn validate_task_req(req: &SaveTaskReq) -> Result<(), ApiError> {
    validate_text_field(&req.name, "任务名称", 255)?;
    validate_text_field(&req.cron, "cron", 256)?;
    validate_text_field(&req.script_id, "script_id", 512)?;
    validate_text_field(&req.device_id, "device_id", 255)?;
    Ok(())
}

// ---------- 响应形状 ----------

/// 409 参数签名冲突（保存时不带 reconfirm / 立即运行时门禁未过共用）。
/// CONTRACT §5.2 错误码表未列 `param_signature_conflict`（契约缺口，按任务
/// 约定使用 snake_case；`reason`/`expected`/`actual` 供前端区分细分原因）。
pub(super) fn signature_conflict_response(
    task_id: &str,
    script_id: &str,
    stored: &str,
    current: &str,
    message: &str,
) -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "code": task_params::CODE_SIGNATURE_CONFLICT,
            "reason": task_params::REASON_SIGNATURE_MISMATCH,
            "message": message,
            "resource": script_id,
            "task_id": task_id,
            "expected": current,
            "actual": stored,
        })),
    )
        .into_response()
}

/// GET /api/tasks 列表项与 GET /api/tasks/:id 详情共用的 JSON 形状
/// （`args` 仅详情返回——列表不带参数值，text 防泄露）。
fn task_json(t: &Task, next: &str, param_stale: bool, with_args: bool) -> Result<Value, ApiError> {
    let args = serde_json::from_str::<Value>(&t.args_json)
        .map_err(|e| ApiError::internal(format!("任务参数快照无效: {e}")))?;
    if !args.is_object() {
        return Err(ApiError::internal("任务参数快照必须是 JSON 对象"));
    }
    let mut v = serde_json::json!({
        "id": t.id, "name": t.name, "cron": t.cron, "script_id": t.script_id,
        "device_id": t.device_id, "enabled": t.enabled, "last_result": t.last_result,
        "last_run_at": t.last_run_at, "next_run": next,
        "param_stale": param_stale,
        "has_args": true,
        "param_signature": t.param_signature,
    });
    if with_args {
        v["args"] = args;
    }
    Ok(v)
}

/// 逐任务计算 param_stale：脚本缺失/解析失败/签名不一致都算过期
/// （统一口径：不能按快照安全运行的任务都提示重新确认）。
fn param_stale_of(scripts: &crate::scripts::ScriptStore, t: &Task) -> bool {
    match task_params::probe_script_signature(scripts, &t.script_id) {
        Ok((_, current)) => t.param_signature != current,
        Err(_) => true,
    }
}

/// next_run 序列化：服务端本地墙钟 + 时区偏移（`%:z` → `2026-09-01 10:00:00+08:00`）。
/// 前端 task-tz.js 靠该偏移推导「服务端时区 UTC+08:00」标签（SYS-001 禁止
/// /api/system/info 暴露 timezone）；无偏移的旧形态会让标签一直处于兜底态。
fn format_next_run(next: chrono::DateTime<chrono::Local>) -> String {
    next.format("%Y-%m-%d %H:%M:%S%:z").to_string()
}

// ---------- 定时任务 ----------

pub(super) async fn api_list_tasks(State(st): State<AppState>) -> Response {
    let tasks = match st.db.list_tasks_async().await {
        Ok(tasks) => tasks,
        Err(err) => return ApiError::internal(err.to_string()).into_response(),
    };
    let scripts = st.scripts.clone();
    let out = match run_blocking_api(move || {
        let tasks = tasks
            .into_iter()
            .map(|t| {
                let next = if t.enabled {
                    next_run(&t.cron)
                        .map(format_next_run)
                        .unwrap_or_else(|| "-".into())
                } else {
                    "-".into()
                };
                let stale = param_stale_of(&scripts, &t);
                task_json(&t, &next, stale, false)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tasks)
    })
    .await
    {
        Ok(out) => out,
        Err(err) => return err.into_response(),
    };
    Json(out).into_response()
}

/// GET /api/tasks/:id：详情（比列表多 `args` 解析视图——重新确认对话框的
/// 「任务原快照」来源）。
pub(super) async fn api_get_task(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let task = match st.db.get_task_async(&id).await {
        Ok(task) => task,
        Err(err) => return ApiError::internal(err.to_string()).into_response(),
    };
    let scripts = st.scripts.clone();
    match run_blocking_api(move || {
        let Some(t) = task else {
            return Err(ApiError::not_found("任务不存在"));
        };
        let stale = param_stale_of(&scripts, &t);
        let next = if t.enabled {
            next_run(&t.cron)
                .map(format_next_run)
                .unwrap_or_else(|| "-".into())
        } else {
            "-".into()
        };
        task_json(&t, &next, stale, true)
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(err) => err.into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct SaveTaskReq {
    pub(super) id: Option<String>,
    pub(super) name: String,
    pub(super) cron: String,
    pub(super) script_id: String,
    pub(super) device_id: String,
    pub(super) enabled: Option<bool>,
    /// 稀疏或全量参数覆盖（键 = 参数名，值按七类解析）。缺省时：已存快照
    /// 签名一致则原样保留；否则需 `reconfirm:true`（不带 args 的 reconfirm =
    /// 存活参数保留原值 + 新参数取当前默认值）。
    #[serde(default)]
    pub(super) args: Option<serde_json::Map<String, Value>>,
    /// 重新确认：按脚本当前声明重算签名并更新快照。
    #[serde(default)]
    pub(super) reconfirm: bool,
}

pub(super) async fn api_save_task(
    State(st): State<AppState>,
    Json(req): Json<SaveTaskReq>,
) -> Response {
    // 校验 cron（5/6/7 字段）
    if !validate_cron(&req.cron) {
        return err_response(StatusCode::BAD_REQUEST, "cron 表达式无效");
    }
    if let Err(err) = validate_task_req(&req) {
        return err.into_response();
    }
    let id = req
        .id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let existing = match st.db.get_task_async(&id).await {
        Ok(existing) => existing,
        Err(err) => return ApiError::internal(err.to_string()).into_response(),
    };
    let scripts = st.scripts.clone();
    // 快照解析需要分区快照（磁盘 IO + 严格解析），整体放 blocking 池。
    // inner 返回 Task 或现有的结构化 Response；数据库写入在异步 worker RPC 完成。
    let result = run_blocking_api(move || Ok(save_task_inner(scripts, id, req, existing))).await;
    let task = match result {
        Ok(Ok(task)) => task,
        Ok(Err(resp)) => return resp,
        Err(err) => return err.into_response(),
    };
    if let Err(err) = st.db.upsert_task_async(&task).await {
        return ApiError::internal(err.to_string()).into_response();
    }
    st.scheduler.notify_tasks_changed();
    let parsed_args = match serde_json::from_str::<Value>(&task.args_json) {
        Ok(Value::Object(args)) => Value::Object(args),
        Ok(_) => return ApiError::internal("任务参数快照必须是 JSON 对象").into_response(),
        Err(_) => return ApiError::internal("任务参数快照不是合法 JSON").into_response(),
    };
    Json(serde_json::json!({
        "ok": true,
        "id": task.id,
        "args": parsed_args,
        "param_signature": task.param_signature,
    }))
    .into_response()
}

fn save_task_inner(
    scripts: std::sync::Arc<crate::scripts::ScriptStore>,
    id: String,
    req: SaveTaskReq,
    existing: Option<Task>,
) -> Result<Task, Response> {
    use crate::engine::RunTarget;
    // 脚本当前声明 + psig1 签名（缺失 → 404；解析失败 → 400 结构化诊断）
    let (decls, current_sig) = match task_params::probe_script_signature(&scripts, &req.script_id) {
        Ok(v) => v,
        Err(GateError::ScriptMissing) => {
            return Err(ApiError::not_found("脚本不存在，无法保存任务参数").into_response());
        }
        Err(GateError::ScriptInvalid(diags)) => return Err(diagnostics_response(&diags)),
        Err(_) => return Err(ApiError::internal("参数探测失败").into_response()),
    };
    let target = RunTarget::Script {
        script_id: req.script_id.clone(),
        start_index: 0,
    };
    // 快照决策（plan §12.3）：显式 args 全量重拍 > 签名一致保留原快照 >
    // reconfirm 重绑定 > 409 冲突 / 新建取纯默认值
    let (args_json, signature): (String, String) = match req.args {
        Some(args) => match crate::engine::resolve_entry_args(&scripts, &target, &args) {
            Ok(bound) => (bound.resolved.to_string(), bound.param_signature),
            Err(diags) => return Err(diagnostics_response(&diags)),
        },
        None => match existing.as_ref() {
            Some(prev) if prev.param_signature == current_sig => {
                // 声明未变：原快照原签名原样保留
                (prev.args_json.clone(), prev.param_signature.clone())
            }
            Some(prev) if req.reconfirm => {
                // 不带 args 的重新确认：存活参数保留原值、新参数取当前默认值、
                // 已删参数丢弃；必填缺失仍走 400 结构化诊断
                match task_params::rebind_snapshot(&decls, &prev.args_json, &req.script_id) {
                    Ok(bound) => (
                        task_params::typed_pairs_to_json(bound).to_string(),
                        current_sig,
                    ),
                    Err(diags) => return Err(diagnostics_response(&diags)),
                }
            }
            Some(prev) => {
                // 签名不一致且未 reconfirm → 409，前端弹重新确认
                let message = GateError::SignatureMismatch {
                    stored: prev.param_signature.clone(),
                    current: current_sig.clone(),
                }
                .message();
                return Err(signature_conflict_response(
                    &prev.id,
                    &req.script_id,
                    &prev.param_signature,
                    &current_sig,
                    &message,
                ));
            }
            None => {
                // 新建不带 args：纯当前默认值打底（必填缺失 → 400 结构化诊断）
                match crate::engine::resolve_entry_args(&scripts, &target, &serde_json::Map::new())
                {
                    Ok(bound) => (bound.resolved.to_string(), bound.param_signature),
                    Err(diags) => return Err(diagnostics_response(&diags)),
                }
            }
        },
    };
    let task = Task {
        id,
        name: req.name,
        cron: req.cron,
        script_id: req.script_id,
        device_id: req.device_id,
        enabled: req
            .enabled
            .unwrap_or(existing.as_ref().map(|t| t.enabled).unwrap_or(true)),
        last_result: existing.as_ref().and_then(|t| t.last_result.clone()),
        last_run_at: existing.as_ref().and_then(|t| t.last_run_at.clone()),
        created_at: existing
            .as_ref()
            .map(|t| t.created_at.clone())
            .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()),
        args_json,
        param_signature: signature,
    };
    Ok(task)
}

pub(super) async fn api_delete_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match st.db.delete_task_async(&id).await {
        Ok(_) => {
            st.scheduler.notify_tasks_changed();
            Json(serde_json::json!({"ok": true})).into_response()
        }
        Err(err) => ApiError::internal(err.to_string()).into_response(),
    }
}

/// 立即运行定时任务（RUN-002 契约）：202 {run_id} 提交即返回，不占用 HTTP
/// 连接等任务完成；设备冲突 409 device_busy；参数门禁未过 409
/// param_signature_conflict；停机 drain 中 503。
pub(super) async fn api_run_task_now(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let trigger_started = Instant::now();
    let tasks = match st.db.list_tasks_async().await {
        Ok(tasks) => tasks,
        Err(err) => return ApiError::internal(err.to_string()).into_response(),
    };
    let Some(task) = tasks.into_iter().find(|t| t.id == id) else {
        return ApiError::not_found("任务不存在").into_response();
    };
    let timer = match timer_yaml::timer_from_legacy(&task) {
        Ok(timer) => timer,
        Err(error) => {
            return err_response(StatusCode::BAD_REQUEST, &error.to_string());
        }
    };
    match st
        .scheduler
        .run_now(&timer)
        .await
        .map_err(map_legacy_run_now_error)
    {
        Ok(run_id) => {
            st.metrics
                .record_scheduler_trigger(trigger_started.elapsed().as_millis() as u64);
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "run_id": run_id })),
            )
                .into_response()
        }
        Err(LegacyRunNowError::Start(crate::run_manager::StartError::Conflict(busy))) => {
            st.metrics
                .record_scheduler_trigger(trigger_started.elapsed().as_millis() as u64);
            st.metrics
                .record_scheduler_event(crate::metrics::SchedulerEvent::Conflict);
            (StatusCode::CONFLICT, Json(busy.busy_payload())).into_response()
        }
        Err(LegacyRunNowError::Start(crate::run_manager::StartError::ShuttingDown)) => {
            st.metrics
                .record_scheduler_trigger(trigger_started.elapsed().as_millis() as u64);
            st.metrics
                .record_scheduler_event(crate::metrics::SchedulerEvent::Skipped);
            err_response(StatusCode::SERVICE_UNAVAILABLE, "shutting_down")
        }
        Err(LegacyRunNowError::ParamStale(err)) => {
            st.metrics
                .record_scheduler_trigger(trigger_started.elapsed().as_millis() as u64);
            st.metrics
                .record_scheduler_event(crate::metrics::SchedulerEvent::Failed);
            signature_conflict_response(
                &task.id,
                &task.script_id,
                &task.param_signature,
                match &err {
                    GateError::SignatureMismatch { current, .. } => current,
                    _ => unreachable!("ParamStale only contains signature mismatches"),
                },
                &err.message(),
            )
        }
        Err(LegacyRunNowError::ScriptMissing) => {
            st.metrics
                .record_scheduler_trigger(trigger_started.elapsed().as_millis() as u64);
            st.metrics
                .record_scheduler_event(crate::metrics::SchedulerEvent::Failed);
            err_response(StatusCode::BAD_REQUEST, "脚本不存在")
        }
        Err(LegacyRunNowError::ScriptInvalid(message)) => {
            st.metrics
                .record_scheduler_trigger(trigger_started.elapsed().as_millis() as u64);
            st.metrics
                .record_scheduler_event(crate::metrics::SchedulerEvent::Failed);
            err_response(StatusCode::BAD_REQUEST, &format!("脚本解析失败: {message}"))
        }
    }
}

// ---------- 通用 User Task / Task Preset API ----------

/// JSON shape for the timer-owned task. Unlike the legacy `/api/tasks` shape,
/// this endpoint carries an explicit AppContext and opaque runner payload.
fn timer_task_json(task: &TimerTask) -> Value {
    serde_json::json!({
        "id": task.id,
        "name": task.name,
        "app": task.app,
        "runner_id": task.runner_id,
        "entrypoint": task.entrypoint,
        "payload": task.payload,
        "schedule": task.schedule,
        "state": task.state,
        "enabled": task.enabled,
        "next_wakeup": task.next_wakeup,
        "last_result": task.last_result,
        "last_run_at": task.last_run_at,
        "created_at": task.created_at,
        "updated_at": task.updated_at,
        "preset_id": task.preset_id,
        "suspend_reason": task.suspend_reason,
    })
}

fn preset_json(preset: &TaskPreset) -> Value {
    serde_json::json!({
        "id": preset.id,
        "app_package": preset.app_package,
        "name": preset.name,
        "runner_id": preset.runner_id,
        "entrypoint": preset.entrypoint,
        "payload": preset.payload,
        "schedule": preset.schedule,
        "created_at": preset.created_at,
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SaveUserTaskReq {
    #[serde(default)]
    pub(super) id: Option<String>,
    pub(super) name: String,
    #[serde(alias = "app_context")]
    pub(super) app: AppContext,
    pub(super) runner_id: String,
    pub(super) entrypoint: String,
    #[serde(default = "empty_payload")]
    pub(super) payload: Value,
    pub(super) schedule: ScheduleSpec,
    #[serde(default)]
    pub(super) enabled: Option<bool>,
    #[serde(default)]
    pub(super) preset_id: Option<String>,
}

fn empty_payload() -> Value {
    Value::Object(serde_json::Map::new())
}

fn validate_schedule(schedule: &ScheduleSpec) -> Result<(), ApiError> {
    schedule
        .validate()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    if schedule.kind == crate::cron_extension::CRON_SCHEDULE_KIND {
        let expression = schedule
            .value
            .get("expression")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::bad_request("cron schedule misses expression"))?;
        if !validate_cron(expression) {
            return Err(ApiError::bad_request("cron 表达式无效"));
        }
    }
    Ok(())
}

fn build_user_task(
    id: String,
    req: SaveUserTaskReq,
    existing: Option<TimerTask>,
) -> Result<TimerTask, ApiError> {
    validate_text_field(&req.name, "任务名称", 255)?;
    validate_text_field(&req.runner_id, "runner_id", 255)?;
    validate_text_field(&req.entrypoint, "entrypoint", 1024)?;
    if let Some(preset_id) = &req.preset_id {
        validate_text_field(preset_id, "preset_id", 255)?;
    }
    validate_schedule(&req.schedule)?;
    let enabled = req
        .enabled
        .or_else(|| existing.as_ref().map(|task| task.enabled))
        .unwrap_or(true);
    let mut task = TimerTask::new(
        id,
        req.name,
        req.app,
        req.runner_id,
        req.entrypoint,
        req.payload,
        req.schedule,
    )
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    if let Some(previous) = existing {
        task.created_at = previous.created_at;
        task.last_result = previous.last_result;
        task.last_run_at = previous.last_run_at;
        task.preset_id = req.preset_id.or(previous.preset_id);
        task.updated_at = chrono::Utc::now();
    } else {
        task.preset_id = req.preset_id;
    }
    task.enabled = enabled;
    task.state = if enabled {
        TimerTaskState::Active
    } else {
        TimerTaskState::Suspended
    };
    task.suspend_reason = (!enabled).then(|| "disabled".to_string());
    Ok(task)
}

pub(super) async fn api_list_user_tasks(State(st): State<AppState>) -> Response {
    match st.db.list_timer_tasks_async().await {
        Ok(tasks) => Json(tasks.iter().map(timer_task_json).collect::<Vec<_>>()).into_response(),
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

pub(super) async fn api_get_user_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match st.db.get_timer_task_async(&id).await {
        Ok(Some(task)) => Json(timer_task_json(&task)).into_response(),
        Ok(None) => ApiError::not_found("任务不存在").into_response(),
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

pub(super) async fn api_save_user_task(
    State(st): State<AppState>,
    Json(req): Json<SaveUserTaskReq>,
) -> Response {
    let id = req
        .id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let existing = match st.db.get_timer_task_async(&id).await {
        Ok(existing) => existing,
        Err(error) => return ApiError::internal(error.to_string()).into_response(),
    };
    let is_new = existing.is_none();
    let task = match build_user_task(id, req, existing) {
        Ok(task) => task,
        Err(error) => return error.into_response(),
    };
    if let Err(error) = st.db.upsert_timer_task_async(&task).await {
        return ApiError::internal(error.to_string()).into_response();
    }
    st.scheduler.notify_tasks_changed();
    let status = if is_new {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    (status, Json(timer_task_json(&task))).into_response()
}

pub(super) async fn api_update_user_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(mut req): Json<SaveUserTaskReq>,
) -> Response {
    if req.id.as_deref().is_some_and(|body_id| body_id != id) {
        return ApiError::bad_request("路径任务 id 与请求体 id 不一致").into_response();
    }
    req.id = Some(id);
    api_save_user_task(State(st), Json(req)).await
}

pub(super) async fn api_delete_user_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let exists = match st.db.get_timer_task_async(&id).await {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(error) => return ApiError::internal(error.to_string()).into_response(),
    };
    if !exists {
        return ApiError::not_found("任务不存在").into_response();
    }
    match st.db.delete_timer_task_async(&id).await {
        Ok(()) => {
            st.scheduler.notify_tasks_changed();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

pub(super) async fn api_suspend_user_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let reason = if body.is_empty() {
        "suspended".to_string()
    } else {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct SuspendReq {
            reason: String,
        }
        match serde_json::from_slice::<SuspendReq>(&body) {
            Ok(req) => req.reason,
            Err(error) => return ApiError::bad_request(error.to_string()).into_response(),
        }
    };
    if reason.trim().is_empty() || reason.chars().any(char::is_control) {
        return ApiError::bad_request("挂起原因无效").into_response();
    }
    match st.scheduler.suspend_task(&id, &reason).await {
        Ok(()) => match st.db.get_timer_task_async(&id).await {
            Ok(Some(task)) => Json(timer_task_json(&task)).into_response(),
            Ok(None) => ApiError::not_found("任务不存在").into_response(),
            Err(error) => ApiError::internal(error.to_string()).into_response(),
        },
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

pub(super) async fn api_resume_user_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match st.scheduler.resume_task(&id).await {
        Ok(()) => match st.db.get_timer_task_async(&id).await {
            Ok(Some(task)) => Json(timer_task_json(&task)).into_response(),
            Ok(None) => ApiError::not_found("任务不存在").into_response(),
            Err(error) => ApiError::internal(error.to_string()).into_response(),
        },
        Err(error) if error.to_string().contains("timer task not found") => {
            ApiError::not_found("任务不存在").into_response()
        }
        Err(error) => ApiError::bad_request(error.to_string()).into_response(),
    }
}

pub(super) async fn api_cancel_user_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match st.scheduler.cancel_task(&id).await {
        Ok(()) => match st.db.get_timer_task_async(&id).await {
            Ok(Some(task)) => Json(timer_task_json(&task)).into_response(),
            Ok(None) => ApiError::not_found("任务不存在").into_response(),
            Err(error) => ApiError::internal(error.to_string()).into_response(),
        },
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

pub(super) async fn api_run_user_task_now(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let task = match st.db.get_timer_task_async(&id).await {
        Ok(Some(task)) => task,
        Ok(None) => return ApiError::not_found("任务不存在").into_response(),
        Err(error) => return ApiError::internal(error.to_string()).into_response(),
    };
    match st.scheduler.run_now(&task).await {
        Ok(run_id) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "run_id": run_id })),
        )
            .into_response(),
        Err(crate::timer_core::TimerRunnerError::Conflict(busy)) => {
            (StatusCode::CONFLICT, Json(busy.busy_payload())).into_response()
        }
        Err(crate::timer_core::TimerRunnerError::ShuttingDown) => {
            err_response(StatusCode::SERVICE_UNAVAILABLE, "shutting_down")
        }
        Err(crate::timer_core::TimerRunnerError::DependencyMissing(message)) => (
            StatusCode::FAILED_DEPENDENCY,
            Json(serde_json::json!({
                "code": "dependency_unavailable",
                "message": message,
                "task_id": task.id,
                "runner_id": task.runner_id,
                "state": "suspended"
            })),
        )
            .into_response(),
        Err(crate::timer_core::TimerRunnerError::Invalid(message))
        | Err(crate::timer_core::TimerRunnerError::Other(message))
        | Err(crate::timer_core::TimerRunnerError::ParamStale { message, .. }) => {
            ApiError::bad_request(message).into_response()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct TaskPresetQuery {
    pub(super) app_package: Option<String>,
}

pub(super) async fn api_list_task_presets(
    State(st): State<AppState>,
    Query(query): Query<TaskPresetQuery>,
) -> Response {
    match st
        .db
        .list_task_presets_async(query.app_package.as_deref())
        .await
    {
        Ok(presets) => Json(presets.iter().map(preset_json).collect::<Vec<_>>()).into_response(),
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

pub(super) async fn api_get_task_preset(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match st.db.get_task_preset_async(&id).await {
        Ok(Some(preset)) => Json(preset_json(&preset)).into_response(),
        Ok(None) => ApiError::not_found("任务预设不存在").into_response(),
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SaveTaskPresetReq {
    #[serde(default)]
    pub(super) id: Option<String>,
    #[serde(alias = "content_package")]
    pub(super) app_package: String,
    pub(super) name: String,
    pub(super) runner_id: String,
    pub(super) entrypoint: String,
    #[serde(default = "empty_payload")]
    pub(super) payload: Value,
    pub(super) schedule: ScheduleSpec,
}

pub(super) async fn api_save_task_preset(
    State(st): State<AppState>,
    Json(req): Json<SaveTaskPresetReq>,
) -> Response {
    if let Err(error) = validate_text_field(&req.app_package, "app_package", 255)
        .and_then(|_| validate_text_field(&req.name, "任务预设名称", 255))
        .and_then(|_| validate_text_field(&req.runner_id, "runner_id", 255))
        .and_then(|_| validate_text_field(&req.entrypoint, "entrypoint", 1024))
    {
        return error.into_response();
    }
    if let Err(error) = validate_schedule(&req.schedule) {
        return error.into_response();
    }
    let id = req
        .id
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let previous = match st.db.get_task_preset_async(&id).await {
        Ok(previous) => previous,
        Err(error) => return ApiError::internal(error.to_string()).into_response(),
    };
    let existed = previous.is_some();
    let preset = TaskPreset {
        id,
        app_package: req.app_package,
        name: req.name,
        runner_id: req.runner_id,
        entrypoint: req.entrypoint,
        payload: req.payload,
        schedule: req.schedule,
        created_at: previous
            .map(|preset| preset.created_at)
            .unwrap_or_else(chrono::Utc::now),
    };
    if let Err(error) = preset.validate() {
        return ApiError::bad_request(error.to_string()).into_response();
    }
    match st.db.upsert_task_preset_async(&preset).await {
        Ok(()) => (
            if existed {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            },
            Json(preset_json(&preset)),
        )
            .into_response(),
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

pub(super) async fn api_update_task_preset(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(mut req): Json<SaveTaskPresetReq>,
) -> Response {
    if req.id.as_deref().is_some_and(|body_id| body_id != id) {
        return ApiError::bad_request("路径预设 id 与请求体 id 不一致").into_response();
    }
    req.id = Some(id);
    api_save_task_preset(State(st), Json(req)).await
}

pub(super) async fn api_delete_task_preset(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match st.db.delete_task_preset_async(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => ApiError::not_found("任务预设不存在").into_response(),
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InstantiatePresetReq {
    #[serde(alias = "app_context")]
    pub(super) app: AppContext,
    #[serde(default)]
    pub(super) id: Option<String>,
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) enabled: Option<bool>,
}

pub(super) async fn api_instantiate_task_preset(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<InstantiatePresetReq>,
) -> Response {
    let preset = match st.db.get_task_preset_async(&id).await {
        Ok(Some(preset)) => preset,
        Ok(None) => return ApiError::not_found("任务预设不存在").into_response(),
        Err(error) => return ApiError::internal(error.to_string()).into_response(),
    };
    let expected_package = match crate::core::AppPackageId::new(preset.app_package.clone()) {
        Ok(package) => package,
        Err(error) => return ApiError::bad_request(error.to_string()).into_response(),
    };
    if req.app.content_package.as_ref() != Some(&expected_package) {
        return ApiError::bad_request("AppContext.content_package 与任务预设不匹配")
            .into_response();
    }
    let enabled = req.enabled.unwrap_or(true);
    let mut task = match TimerTask::new(
        req.id
            .unwrap_or_else(|| Uuid::new_v4().simple().to_string()),
        req.name.unwrap_or_else(|| preset.name.clone()),
        req.app,
        preset.runner_id.clone(),
        preset.entrypoint.clone(),
        preset.payload.clone(),
        preset.schedule.clone(),
    ) {
        Ok(task) => task,
        Err(error) => return ApiError::bad_request(error.to_string()).into_response(),
    };
    task.preset_id = Some(preset.id);
    task.enabled = enabled;
    task.state = if enabled {
        TimerTaskState::Active
    } else {
        TimerTaskState::Suspended
    };
    task.suspend_reason = (!enabled).then(|| "disabled".to_string());
    if let Err(error) = st.db.upsert_timer_task_async(&task).await {
        return ApiError::internal(error.to_string()).into_response();
    }
    st.scheduler.notify_tasks_changed();
    (StatusCode::CREATED, Json(timer_task_json(&task))).into_response()
}
