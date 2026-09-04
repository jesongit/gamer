//! Unified Task API（P11.1 / ADR-12：Task = 任意 ScheduleProvider + 任意 Runner）。
//!
//! 唯一任务资源端点组：`/api/tasks`（原 `/api/user-tasks` 收口升级）。
//! - `schedule = {provider_id, config}`：调度语义由已注册 ScheduleProvider 解释；
//!   未注册 provider 保存时放行（未来扩展可先存任务），触发时由 TimerCore 进入
//!   `dependency_missing` 状态（任务保留，不删除）。
//! - `runner = {runner_id, entrypoint, payload}`：执行语义由 Runner 解释，
//!   payload 为 runner 私有不透明值；允许保存未知 runner_id。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use super::common::validate_text_field;
use super::{ApiError, AppState};
use crate::core::AppContext;
use crate::timer_core::{
    ScheduleRegistry, Task, TaskPreset, TaskSchedule, TaskState, TimerRunnerError,
};

// ---------- JSON DTO（ADR-12 嵌套形状；持久层列保持平铺） ----------

/// API 层的 runner 声明（Rust 模型与 SQLite 列保持 runner_id/entrypoint/payload
/// 平铺，仅 HTTP JSON 嵌套表达）。
#[derive(Debug, Clone, Deserialize)]
pub(super) struct RunnerSpecDto {
    pub(super) runner_id: String,
    pub(super) entrypoint: String,
    #[serde(default = "empty_payload")]
    pub(super) payload: Value,
}

fn empty_payload() -> Value {
    Value::Object(serde_json::Map::new())
}

/// JSON shape for the timer-owned task. The runner is nested
/// (`runner: {runner_id, entrypoint, payload}`) and the schedule is
/// `{provider_id, config}` per ADR-12.
fn task_json(task: &Task) -> Value {
    serde_json::json!({
        "id": task.id,
        "name": task.name,
        "app": task.app,
        "runner": {
            "runner_id": task.runner_id,
            "entrypoint": task.entrypoint,
            "payload": task.payload,
        },
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
        "runner": {
            "runner_id": preset.runner_id,
            "entrypoint": preset.entrypoint,
            "payload": preset.payload,
        },
        "schedule": preset.schedule,
        "created_at": preset.created_at,
    })
}

// ---------- UI 支撑只读端点 ----------

/// GET /api/runners：已注册 runner 列表（含 owner 扩展，TaskBoard 执行器下拉
/// 数据源）。裸 Core（无扩展 start）时为空数组。
pub(super) async fn api_list_runners(State(st): State<AppState>) -> Response {
    let runners = st.scheduler.runners();
    Json(
        runners
            .into_iter()
            .map(|runner| {
                serde_json::json!({
                    "runner_id": runner.runner_id,
                    "owner_extension_id": runner.owner_extension_id,
                })
            })
            .collect::<Vec<_>>(),
    )
    .into_response()
}

/// GET /api/schedule-providers：已注册 schedule provider 列表。
pub(super) async fn api_list_schedule_providers(State(st): State<AppState>) -> Response {
    let ids = st.scheduler.schedule_provider_ids();
    Json(
        ids.into_iter()
            .map(|provider_id| serde_json::json!({ "provider_id": provider_id }))
            .collect::<Vec<_>>(),
    )
    .into_response()
}

// ---------- 定时任务 ----------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SaveTaskReq {
    #[serde(default)]
    pub(super) id: Option<String>,
    pub(super) name: String,
    #[serde(alias = "app_context")]
    pub(super) app: AppContext,
    pub(super) runner: RunnerSpecDto,
    pub(super) schedule: TaskSchedule,
    #[serde(default)]
    pub(super) enabled: Option<bool>,
    #[serde(default)]
    pub(super) preset_id: Option<String>,
}

/// 通用 schedule 校验：已注册 provider 必须接受该 config（cron 表达式错误在
/// 保存边界即 400）；未注册 provider 保存时放行，触发时由 TimerCore 进入
/// dependency_missing（既有语义：未来扩展允许先存任务、后装 provider）。
fn validate_schedule(registry: &ScheduleRegistry, schedule: &TaskSchedule) -> Result<(), ApiError> {
    schedule
        .validate()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    registry.probe(schedule).map_err(ApiError::bad_request)
}

pub(super) fn build_task(
    registry: &ScheduleRegistry,
    id: String,
    req: SaveTaskReq,
    existing: Option<Task>,
) -> Result<Task, ApiError> {
    validate_text_field(&req.name, "任务名称", 255)?;
    validate_text_field(&req.runner.runner_id, "runner_id", 255)?;
    validate_text_field(&req.runner.entrypoint, "entrypoint", 1024)?;
    if let Some(preset_id) = &req.preset_id {
        validate_text_field(preset_id, "preset_id", 255)?;
    }
    validate_schedule(registry, &req.schedule)?;
    let enabled = req
        .enabled
        .or_else(|| existing.as_ref().map(|task| task.enabled))
        .unwrap_or(true);
    let mut task = Task::new(
        id,
        req.name,
        req.app,
        req.runner.runner_id,
        req.runner.entrypoint,
        req.runner.payload,
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
        TaskState::Active
    } else {
        TaskState::Suspended
    };
    task.suspend_reason = (!enabled).then(|| "disabled".to_string());
    Ok(task)
}

pub(super) async fn api_list_tasks(State(st): State<AppState>) -> Response {
    match st.db.list_timer_tasks_async().await {
        Ok(tasks) => Json(tasks.iter().map(task_json).collect::<Vec<_>>()).into_response(),
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

pub(super) async fn api_get_task(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match st.db.get_timer_task_async(&id).await {
        Ok(Some(task)) => Json(task_json(&task)).into_response(),
        Ok(None) => ApiError::not_found("任务不存在").into_response(),
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

pub(super) async fn api_save_task(
    State(st): State<AppState>,
    Json(req): Json<SaveTaskReq>,
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
    let task = match build_task(st.scheduler.schedules().as_ref(), id, req, existing) {
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
    (status, Json(task_json(&task))).into_response()
}

pub(super) async fn api_update_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(mut req): Json<SaveTaskReq>,
) -> Response {
    if req.id.as_deref().is_some_and(|body_id| body_id != id) {
        return ApiError::bad_request("路径任务 id 与请求体 id 不一致").into_response();
    }
    req.id = Some(id);
    api_save_task(State(st), Json(req)).await
}

pub(super) async fn api_delete_task(
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

pub(super) async fn api_suspend_task(
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
            Ok(Some(task)) => Json(task_json(&task)).into_response(),
            Ok(None) => ApiError::not_found("任务不存在").into_response(),
            Err(error) => ApiError::internal(error.to_string()).into_response(),
        },
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

pub(super) async fn api_resume_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match st.scheduler.resume_task(&id).await {
        Ok(()) => match st.db.get_timer_task_async(&id).await {
            Ok(Some(task)) => Json(task_json(&task)).into_response(),
            Ok(None) => ApiError::not_found("任务不存在").into_response(),
            Err(error) => ApiError::internal(error.to_string()).into_response(),
        },
        Err(error) if error.to_string().contains("timer task not found") => {
            ApiError::not_found("任务不存在").into_response()
        }
        Err(error) => ApiError::bad_request(error.to_string()).into_response(),
    }
}

/// POST /api/tasks/:id/enable：启用调度（= resume：重算唤醒游标、清 reason）。
pub(super) async fn api_enable_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    api_resume_task(State(st), Path(id)).await
}

/// POST /api/tasks/:id/disable：停用调度（挂起 + "disabled" 原因，任务保留）。
pub(super) async fn api_disable_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match st.scheduler.suspend_task(&id, "disabled").await {
        Ok(()) => match st.db.get_timer_task_async(&id).await {
            Ok(Some(task)) => Json(task_json(&task)).into_response(),
            Ok(None) => ApiError::not_found("任务不存在").into_response(),
            Err(error) => ApiError::internal(error.to_string()).into_response(),
        },
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

pub(super) async fn api_cancel_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match st.scheduler.cancel_task(&id).await {
        Ok(()) => match st.db.get_timer_task_async(&id).await {
            Ok(Some(task)) => Json(task_json(&task)).into_response(),
            Ok(None) => ApiError::not_found("任务不存在").into_response(),
            Err(error) => ApiError::internal(error.to_string()).into_response(),
        },
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

/// 立即运行（RUN-002 契约）：202 {run_id} 提交即返回；设备冲突 409 device_busy；
/// 运行依赖缺失（runner/schedule provider/脚本不存在）424 dependency_unavailable
/// 且任务进入 dependency_missing 状态；停机 drain 中 503。
pub(super) async fn api_run_task_now(
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
        Err(TimerRunnerError::Conflict(busy)) => {
            (StatusCode::CONFLICT, Json(busy.busy_payload())).into_response()
        }
        Err(TimerRunnerError::ShuttingDown) => {
            super::common::err_response(StatusCode::SERVICE_UNAVAILABLE, "shutting_down")
        }
        Err(TimerRunnerError::DependencyMissing(message)) => (
            StatusCode::FAILED_DEPENDENCY,
            Json(serde_json::json!({
                "code": "dependency_unavailable",
                "message": message,
                "task_id": task.id,
                "runner_id": task.runner_id,
                "state": "dependency_missing"
            })),
        )
            .into_response(),
        Err(TimerRunnerError::Invalid(message))
        | Err(TimerRunnerError::InvalidDetail { message, .. })
        | Err(TimerRunnerError::Other(message))
        | Err(TimerRunnerError::ParamStale(message)) => {
            ApiError::bad_request(message).into_response()
        }
    }
}

// ---------- Task Preset API ----------

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
    pub(super) runner: RunnerSpecDto,
    pub(super) schedule: TaskSchedule,
}

pub(super) async fn api_save_task_preset(
    State(st): State<AppState>,
    Json(req): Json<SaveTaskPresetReq>,
) -> Response {
    if let Err(error) = validate_text_field(&req.app_package, "app_package", 255)
        .and_then(|_| validate_text_field(&req.name, "任务预设名称", 255))
        .and_then(|_| validate_text_field(&req.runner.runner_id, "runner_id", 255))
        .and_then(|_| validate_text_field(&req.runner.entrypoint, "entrypoint", 1024))
    {
        return error.into_response();
    }
    if let Err(error) = validate_schedule(st.scheduler.schedules().as_ref(), &req.schedule) {
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
        runner_id: req.runner.runner_id,
        entrypoint: req.runner.entrypoint,
        payload: req.runner.payload,
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
    let mut task = match Task::new(
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
        TaskState::Active
    } else {
        TaskState::Suspended
    };
    task.suspend_reason = (!enabled).then(|| "disabled".to_string());
    if let Err(error) = st.db.upsert_timer_task_async(&task).await {
        return ApiError::internal(error.to_string()).into_response();
    }
    st.scheduler.notify_tasks_changed();
    (StatusCode::CREATED, Json(task_json(&task))).into_response()
}
