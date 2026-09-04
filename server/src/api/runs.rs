//! 统一执行入口（P11.6 / plan §11.3）与运行生命周期端点。
//!
//! - `POST /api/runs` body `{runner_id, entrypoint, device_id, payload?,
//!   content_package?}`：Core 只做通用分发——按 `runner_id` 在
//!   [`crate::timer_core::TimerRunnerRegistry`] 查找已注册 runner 并转发；
//!   entrypoint/payload 语义属于注册该 runner 的扩展（gamer.yaml 的契约见
//!   `extensions::gamer_yaml::timer_yaml`）。`task_id` 为空 = 手动 ad-hoc
//!   运行（任务路径复用同一 runner，`/api/tasks/:id/run`）。
//! - `GET /api/runs/:run_id` / `POST /api/runs/:run_id/cancel`：
//!   RunManager 统一 run_id 注册表（活动 + 终态档案均可查）。
//! - `GET /api/devices/:id/run`：设备当前运行（前端刷新恢复运行态）。
//!
//! 原 `/api/scripts/:id/run`、`/api/functions/:id/run` 已删除（ADR-14 零兼容）。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::common::{err_response, validate_text_field};
use super::{ApiError, AppState};
use crate::core::{AppContext, AppPackageId, DeviceId, RunPayload, RunRequest};
use crate::timer_core::{TimerCompletion, TimerRunnerError};

/// POST /api/runs 请求形态（plan §11.3）。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DispatchRunReq {
    pub(super) runner_id: String,
    pub(super) entrypoint: String,
    pub(super) device_id: String,
    #[serde(default)]
    pub(super) payload: Option<serde_json::Value>,
    /// 内容分区（runner 资源解析域）；缺省取 entrypoint 首段
    /// （`<content_package>/<path>` 约定）。
    #[serde(default)]
    pub(super) content_package: Option<String>,
}

/// POST /api/runs：统一执行分发（202 提交即返回；RUN-002 契约）。
pub(super) async fn api_dispatch_run(
    State(st): State<AppState>,
    Json(req): Json<DispatchRunReq>,
) -> Response {
    if let Err(err) = validate_text_field(&req.runner_id, "runner_id", 255) {
        return err.into_response();
    }
    if let Err(err) = validate_text_field(&req.entrypoint, "entrypoint", 1024) {
        return err.into_response();
    }
    if let Err(err) = validate_text_field(&req.device_id, "device_id", 255) {
        return err.into_response();
    }
    if let Some(payload) = &req.payload {
        if !payload.is_object() {
            return ApiError::bad_request("payload 必须是对象").into_response();
        }
    }
    // 内容分区：显式 content_package 优先，缺省按 entrypoint 首段约定解析
    let content_package = match &req.content_package {
        Some(pkg) => pkg.clone(),
        None => req
            .entrypoint
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string(),
    };
    let Ok(content) = AppPackageId::new(&content_package) else {
        return ApiError::bad_request(format!(
            "内容分区非法（只允许字母数字 . _ -）: {content_package}"
        ))
        .into_response();
    };
    let android_package = st
        .devices
        .snapshot(&req.device_id)
        .and_then(|(device, _, _)| device.pkg)
        .unwrap_or_else(|| content_package.clone());
    let device_id = match DeviceId::new(&req.device_id) {
        Ok(id) => id,
        Err(error) => return ApiError::bad_request(error.to_string()).into_response(),
    };
    let android = match crate::core::AndroidPackageName::new(&android_package) {
        Ok(name) => name,
        Err(error) => return ApiError::bad_request(error.to_string()).into_response(),
    };
    let app = AppContext::new(device_id, android, Some(content));
    let request = match RunRequest::for_app(
        app,
        req.runner_id.clone(),
        req.entrypoint.clone(),
        RunPayload::new(req.payload.unwrap_or(serde_json::json!({}))),
    ) {
        Ok(request) => request,
        Err(error) => return ApiError::bad_request(error.to_string()).into_response(),
    };
    // 手动运行终态摘要行由 gamer.yaml runner 落库（实时日志 realtime 入库）；
    // 完成回调当前无 TimerCore 记账需求，传 no-op。
    let hook: crate::timer_core::TimerCompletionHook = Arc::new(|_completion: TimerCompletion| {});
    use crate::timer_core::TimerRunner as _;
    match st
        .scheduler
        .runner_registry()
        .submit(request, "", None, hook)
        .await
    {
        Ok(run) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "run_id": run.run_id,
                "state": "starting",
                "resolved_args": run.detail.and_then(|detail| detail.get("resolved_args").cloned()),
            })),
        )
            .into_response(),
        Err(error) => dispatch_error_response(error),
    }
}

/// runner 分发错误 → HTTP（与 /api/tasks/:id/run 同口径；参数诊断 400 透传）。
fn dispatch_error_response(error: TimerRunnerError) -> Response {
    match error {
        TimerRunnerError::Conflict(busy) => {
            (StatusCode::CONFLICT, Json(busy.busy_payload())).into_response()
        }
        TimerRunnerError::ShuttingDown => {
            err_response(StatusCode::SERVICE_UNAVAILABLE, "shutting_down")
        }
        TimerRunnerError::DependencyMissing(message) => (
            StatusCode::FAILED_DEPENDENCY,
            Json(serde_json::json!({
                "code": "dependency_unavailable",
                "message": message,
            })),
        )
            .into_response(),
        TimerRunnerError::ParamStale(message) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "code": "signature_mismatch",
                "message": message,
            })),
        )
            .into_response(),
        TimerRunnerError::InvalidDetail { message, detail } => (
            StatusCode::BAD_REQUEST,
            Json(match detail {
                serde_json::Value::Null => {
                    serde_json::json!({ "error": "invalid_args", "message": message })
                }
                value if value.is_object() && value.get("error").is_some() => {
                    let mut map = value.as_object().cloned().unwrap_or_default();
                    map.entry("message".to_string())
                        .or_insert(serde_json::json!(message));
                    serde_json::Value::Object(map)
                }
                value => serde_json::json!({
                    "error": "invalid_args",
                    "message": message,
                    "diagnostics": value
                }),
            }),
        )
            .into_response(),
        TimerRunnerError::Invalid(message) | TimerRunnerError::Other(message) => {
            ApiError::bad_request(message).into_response()
        }
    }
}

/// 设备当前运行查询（前端刷新恢复运行态）：
/// 新契约固定为 active:true + 嵌套完整 RunRecord，或 active:false。
pub(super) async fn api_device_run(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match st.runs.active_for_device(&id) {
        Some(rec) => Json(serde_json::json!({"active": true, "run": rec})).into_response(),
        None => Json(serde_json::json!({"active": false})).into_response(),
    }
}

/// GET /api/runs/:run_id → 完整 RunRecord（活动在册 + 终态档案均可查；未知 404）
pub(super) async fn api_get_run(
    State(st): State<AppState>,
    Path(run_id): Path<String>,
) -> Response {
    match st.runs.get_run(&run_id) {
        Some(rec) => Json(serde_json::to_value(&rec).unwrap_or_else(|_| serde_json::json!({})))
            .into_response(),
        None => err_response(StatusCode::NOT_FOUND, "run_not_found"),
    }
}

/// POST /api/runs/:run_id/cancel → 202 {"cancelling":true}；
/// 终态由客户端随后 GET /api/runs/:id 确认（cancelled/success/failed）
pub(super) async fn api_cancel_run(
    State(st): State<AppState>,
    Path(run_id): Path<String>,
) -> Response {
    use crate::run_manager::CancelOutcome;
    match st.runs.cancel(&run_id) {
        CancelOutcome::Accepted => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"cancelling": true})),
        )
            .into_response(),
        CancelOutcome::NotFound => err_response(StatusCode::NOT_FOUND, "run_not_found"),
        CancelOutcome::AlreadyFinished(state) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "already_finished",
                "state": serde_json::to_value(state).unwrap_or_default(),
            })),
        )
            .into_response(),
    }
}
