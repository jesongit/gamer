//! Log query and cleanup endpoints.

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

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
    let device_id = q.device_id;
    let level = q.level;
    let limit = clamp_log_limit(q.limit);
    match st
        .db
        .list_logs_async(device_id.as_deref(), level.as_deref(), limit)
        .await
    {
        Ok(logs) => Json(logs).into_response(),
        Err(err) => ApiError::internal(err.to_string()).into_response(),
    }
}

pub(super) async fn api_clear_logs(State(st): State<AppState>) -> Response {
    match st.db.clear_logs_async().await {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(err) => ApiError::internal(err.to_string()).into_response(),
    }
}
