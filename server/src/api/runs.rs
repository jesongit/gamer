//! Manual script runs and run lifecycle compatibility endpoints.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::common::{err_response, run_blocking_api, validate_text_field};
use super::{ApiError, AppState};
use crate::store::Db;

pub(super) fn validate_run_script_req(req: &RunScriptReq) -> Result<(), ApiError> {
    validate_text_field(&req.device_id, "device_id", 255)?;
    if req.start_index.is_some_and(|index| index > 100_000) {
        return Err(ApiError::bad_request("start_index 超过脚本步数上限"));
    }
    if let Some(func) = req.func.as_deref().filter(|v| !v.trim().is_empty()) {
        validate_text_field(func, "func", 255)?;
    }
    Ok(())
}

#[derive(Deserialize)]
pub(super) struct RunScriptReq {
    pub(super) device_id: String,
    /// 从第几个 step 开始运行（0=从头；前端选中某个 "- " 逻辑行时传入）
    #[serde(default)]
    pub(super) start_index: Option<usize>,
    /// 直接运行指定函数体（Console 选中函数名行 / 函数体内的行时传入）；
    /// start_index 此时是函数体内的步骤序号——0（函数名行）先检查函数 cond，
    /// >0（体内行）跳过 cond 从该步执行
    #[serde(default)]
    pub(super) func: Option<String>,
}

/// 手动运行的完成钩子：终态摘要行落库（realtime 模式引擎日志已实时入库，
/// 这里只补一条终局提示，与旧实现的"脚本执行完成/失败"行语义对齐）
fn manual_finish_hook(db: Db) -> crate::run_manager::FinishHook {
    use crate::run_manager::RunOutcome;
    Arc::new(move |rec, outcome| match outcome {
        RunOutcome::Success(_) => {
            let _ = db.add_log(&rec.device_id, &rec.script_id, "success", "脚本执行完成");
        }
        RunOutcome::Failed(msg, _) => {
            let _ = db.add_log(
                &rec.device_id,
                &rec.script_id,
                "error",
                &format!("脚本执行失败: {}", msg),
            );
        }
        RunOutcome::Cancelled(_) => {
            let _ = db.add_log(&rec.device_id, &rec.script_id, "info", "脚本已停止");
        }
    })
}

pub(super) async fn api_run_script(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RunScriptReq>,
) -> Response {
    let script_id = id.clone();
    // 脚本存在性先校验（404 优先于设备冲突）
    let Some(script) = (match run_blocking_api(move || {
        st.scripts
            .get(&id)
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    }) else {
        return ApiError::not_found("脚本不存在").into_response();
    };
    if let Err(err) = validate_run_script_req(&req) {
        return err.into_response();
    }
    // RUN-002 契约：启动即返回 202 {run_id, state:"starting"}，不等脚本结束；
    // 设备级互斥冲突 → 409 {error:"device_busy", run_id, script_id, source, started_at}
    let rreq = crate::run_manager::StartRequest {
        run_id: String::new(),
        device_id: req.device_id.clone(),
        script_id,
        content: script.content.clone(),
        source: crate::run_manager::RunSource::Manual,
        task_id: None,
        scheduled_at: None,
        start_index: req.start_index.unwrap_or(0),
        run_func: req.func.filter(|s| !s.trim().is_empty()),
        realtime_logs: true,
    };
    match st
        .runs
        .submit(rreq, Some(manual_finish_hook(st.db.clone())))
    {
        Ok(rec) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "run_id": rec.run_id,
                "state": serde_json::to_value(rec.state).unwrap_or_default(),
            })),
        )
            .into_response(),
        Err(crate::run_manager::StartError::Conflict(busy)) => {
            (StatusCode::CONFLICT, Json(busy.busy_payload())).into_response()
        }
        Err(crate::run_manager::StartError::ShuttingDown) => {
            err_response(StatusCode::SERVICE_UNAVAILABLE, "shutting_down")
        }
    }
}

/// 旧停止端点（兼容窗口）：按 script_id 定位活动 run 并取消。
/// 同一脚本可能在不同设备各有一个实例——逐个取消。响应保持旧形状 {ok:true}。
pub(super) async fn api_stop_script(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    for run in st.runs.active_for_script(&id) {
        st.runs.cancel(&run.run_id);
    }
    Json(serde_json::json!({"ok": true})).into_response()
}

/// 旧脚本运行查询（兼容窗口）：内部经 RunManager 反查该脚本的任意活动实例
pub(super) async fn api_script_status(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let running = !st.runs.active_for_script(&id).is_empty();
    Json(serde_json::json!({"running": running})).into_response()
}

/// 设备当前运行查询（前端刷新恢复运行态）：
/// 新契约 active:true + 完整 RunRecord / active:false；
/// （旧 {running,script_id,script_name} 形状已随阶段 3 废弃）
pub(super) async fn api_device_run(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match st.runs.active_for_device(&id) {
        Some(rec) => {
            let mut v = serde_json::to_value(&rec).unwrap_or_else(|_| serde_json::json!({}));
            v["active"] = serde_json::json!(true);
            Json(v).into_response()
        }
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
