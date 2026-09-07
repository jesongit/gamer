//! 录制 REST（视频工作台 V1）。实施合同：`docs/plans/gamer_video_workbench_contracts.md` §2。
//!
//! 路由与处理器由录制服务实施者填充；认证与限额组装配在
//! [`super::build_router_with_extensions`]。录制是服务端权威：
//! 浏览器断开不中断录制，重复 stop/cancel 幂等。

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use super::{ApiError, AppState};
use crate::recording::{service, FailureKind, RecordingFailure, RecordingId, RecordingStartReq};

/// 录制路由组（挂进受保护组）。
/// 合同端点：
/// - `POST /api/recording/start`（{device_id} → 202 session）
/// - `POST /api/recording/:id/stop | /cancel`（→ session 终态；幂等）
/// - `GET  /api/recording/:id`（状态）
/// - `GET  /api/recording/active?device_id=`（设备活动会话或 404）
/// - `GET  /api/recording/:id/events`（{schema_version, events:[...]}）
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/recording/start", post(api_start))
        .route("/api/recording/active", get(api_active))
        .route("/api/recording/:id/stop", post(api_stop))
        .route("/api/recording/:id/cancel", post(api_cancel))
        .route("/api/recording/:id", get(api_status))
        .route("/api/recording/:id/events", get(api_events))
}

/// 服务错误 → HTTP：结构化 [`RecordingFailure`] 按 kind 映射状态码
/// （404/409/400），其余按 500；错误体对齐 `{"error": "<msg>"}`。
fn map_failure(err: anyhow::Error) -> Response {
    match err.downcast_ref::<RecordingFailure>() {
        Some(failure) => {
            let status = match failure.kind {
                FailureKind::NotFound => StatusCode::NOT_FOUND,
                FailureKind::Busy | FailureKind::DeviceOffline => StatusCode::CONFLICT,
                FailureKind::Invalid => StatusCode::BAD_REQUEST,
                FailureKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            };
            ApiError::new(status, failure.message.clone()).into_response()
        }
        None => ApiError::internal(err.to_string()).into_response(),
    }
}

/// `POST /api/recording/start`：202 + 会话元数据（服务端权威异步起录）。
async fn api_start(State(st): State<AppState>, Json(req): Json<RecordingStartReq>) -> Response {
    match service(&st.cfg).start(&st.devices, &req) {
        Ok(meta) => (StatusCode::ACCEPTED, Json(meta)).into_response(),
        Err(err) => map_failure(err),
    }
}

/// `POST /api/recording/:id/stop`：200 终态 session（幂等）。
async fn api_stop(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match service(&st.cfg).stop(&RecordingId(id)) {
        Ok(meta) => Json(meta).into_response(),
        Err(err) => map_failure(err),
    }
}

/// `POST /api/recording/:id/cancel`：200 终态（已落盘部分保留为素材）。
async fn api_cancel(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match service(&st.cfg).cancel(&RecordingId(id)) {
        Ok(meta) => Json(meta).into_response(),
        Err(err) => map_failure(err),
    }
}

/// `GET /api/recording/:id`：会话状态；404 `{"error":"recording_not_found"}`。
async fn api_status(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match service(&st.cfg).status(&RecordingId(id)) {
        Ok(meta) => Json(meta).into_response(),
        Err(err) => map_failure(err),
    }
}

/// `GET /api/recording/active?device_id=`：设备活动会话或 404。
async fn api_active(
    State(st): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(device_id) = q
        .get("device_id")
        .map(|s| s.trim())
        .filter(|d| !d.is_empty())
    else {
        return ApiError::bad_request("缺少 device_id 查询参数").into_response();
    };
    match service(&st.cfg).active_for_device(device_id) {
        Some(meta) => Json(meta).into_response(),
        None => ApiError::not_found("recording_not_found").into_response(),
    }
}

/// `GET /api/recording/:id/events`：操作事件（时间轴升序）。
async fn api_events(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match service(&st.cfg).events(&RecordingId(id)) {
        Ok(events) => Json(json!({ "schema_version": 1, "events": events })).into_response(),
        Err(err) => map_failure(err),
    }
}
