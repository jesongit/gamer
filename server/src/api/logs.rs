//! Log query and cleanup endpoints.

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::common::run_blocking_api;
use super::{ApiError, AppState};

// ---------- 日志 ----------

#[derive(Deserialize)]
pub(super) struct LogQuery {
    device_id: Option<String>,
    level: Option<String>,
    limit: Option<i64>,
}

/// 日志查询条数钳制：1..=1000（阶段 2 SEC-004），缺省 200。
/// 非法值钳进合法区间而非报错——前端只需要"少拿点"，不存在语义歧义
pub(super) fn clamp_log_limit(limit: Option<i64>) -> i64 {
    match limit {
        Some(n) if n < 1 => 1,
        Some(n) => n.min(1000),
        None => 200,
    }
}

pub(super) async fn api_list_logs(
    State(st): State<AppState>,
    Query(q): Query<LogQuery>,
) -> Response {
    let db = st.db.clone();
    let device_id = q.device_id;
    let level = q.level;
    let limit = clamp_log_limit(q.limit);
    match run_blocking_api(move || {
        db.list_logs(device_id.as_deref(), level.as_deref(), limit)
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(logs) => Json(logs).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(super) async fn api_clear_logs(State(st): State<AppState>) -> Response {
    let db = st.db.clone();
    match run_blocking_api(move || {
        db.clear_logs()
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(err) => err.into_response(),
    }
}
