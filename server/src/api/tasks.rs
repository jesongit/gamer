//! Scheduled task CRUD and immediate trigger endpoints.

use std::time::Instant;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::common::{err_response, run_blocking_api, validate_text_field};
use super::{ApiError, AppState};
use crate::scheduler::next_run;
use crate::store::Task;

pub(super) fn validate_task_req(req: &SaveTaskReq) -> Result<(), ApiError> {
    validate_text_field(&req.name, "任务名称", 255)?;
    validate_text_field(&req.cron, "cron", 256)?;
    validate_text_field(&req.script_id, "script_id", 512)?;
    validate_text_field(&req.device_id, "device_id", 255)?;
    Ok(())
}

// ---------- 定时任务 ----------

pub(super) async fn api_list_tasks(State(st): State<AppState>) -> Response {
    let db = st.db.clone();
    let tasks = match run_blocking_api(move || {
        db.list_tasks()
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(tasks) => tasks,
        Err(err) => return err.into_response(),
    };
    let out: Vec<serde_json::Value> = tasks
        .into_iter()
        .map(|t| {
            let next = if t.enabled {
                next_run(&t.cron)
                    .map(|x| x.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| "-".into())
            } else {
                "-".into()
            };
            serde_json::json!({
                "id": t.id, "name": t.name, "cron": t.cron, "script_id": t.script_id,
                "device_id": t.device_id, "enabled": t.enabled, "last_result": t.last_result,
                "last_run_at": t.last_run_at, "next_run": next
            })
        })
        .collect();
    Json(out).into_response()
}

#[derive(Deserialize)]
pub(super) struct SaveTaskReq {
    pub(super) id: Option<String>,
    pub(super) name: String,
    pub(super) cron: String,
    pub(super) script_id: String,
    pub(super) device_id: String,
    pub(super) enabled: Option<bool>,
}

pub(super) async fn api_save_task(
    State(st): State<AppState>,
    Json(req): Json<SaveTaskReq>,
) -> Response {
    // 校验 cron（5/6/7 字段）
    if !crate::scheduler::validate_cron(&req.cron) {
        return err_response(StatusCode::BAD_REQUEST, "cron 表达式无效");
    }
    if let Err(err) = validate_task_req(&req) {
        return err.into_response();
    }
    let id = req
        .id
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let db = st.db.clone();
    let task = match run_blocking_api(move || {
        let existing = db
            .list_tasks()
            .map_err(|e| ApiError::internal(e.to_string()))?
            .into_iter()
            .find(|t| t.id == id);
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
        };
        db.upsert_task(&task)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(task)
    })
    .await
    {
        Ok(task) => task,
        Err(err) => return err.into_response(),
    };
    Json(serde_json::json!({"ok": true, "id": task.id})).into_response()
}

pub(super) async fn api_delete_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let db = st.db.clone();
    match run_blocking_api(move || {
        db.delete_task(&id)
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(err) => err.into_response(),
    }
}

/// 立即运行定时任务（RUN-002 契约）：202 {run_id} 提交即返回，不占用 HTTP
/// 连接等任务完成；设备冲突 409 device_busy；停机 drain 中 503。
pub(super) async fn api_run_task_now(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    use crate::scheduler::RunNowError;
    let trigger_started = Instant::now();
    let db = st.db.clone();
    let tasks = match run_blocking_api(move || {
        db.list_tasks()
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(tasks) => tasks,
        Err(err) => return err.into_response(),
    };
    let Some(task) = tasks.into_iter().find(|t| t.id == id) else {
        return ApiError::not_found("任务不存在").into_response();
    };
    match st.scheduler.run_now(&task).await {
        Ok(run_id) => {
            st.metrics
                .record_scheduler_trigger(trigger_started.elapsed().as_millis() as u64);
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "run_id": run_id })),
            )
                .into_response()
        }
        Err(RunNowError::Start(crate::run_manager::StartError::Conflict(busy))) => {
            st.metrics
                .record_scheduler_trigger(trigger_started.elapsed().as_millis() as u64);
            st.metrics
                .record_scheduler_event(crate::metrics::SchedulerEvent::Conflict);
            (StatusCode::CONFLICT, Json(busy.busy_payload())).into_response()
        }
        Err(RunNowError::Start(crate::run_manager::StartError::ShuttingDown)) => {
            st.metrics
                .record_scheduler_trigger(trigger_started.elapsed().as_millis() as u64);
            st.metrics
                .record_scheduler_event(crate::metrics::SchedulerEvent::Skipped);
            err_response(StatusCode::SERVICE_UNAVAILABLE, "shutting_down")
        }
        Err(RunNowError::ScriptMissing | RunNowError::Io) => {
            st.metrics
                .record_scheduler_trigger(trigger_started.elapsed().as_millis() as u64);
            st.metrics
                .record_scheduler_event(crate::metrics::SchedulerEvent::Failed);
            err_response(StatusCode::BAD_REQUEST, "脚本不存在或读取失败")
        }
    }
}
