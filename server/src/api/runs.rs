//! 手动运行（脚本 / 函数测试）与运行生命周期端点。
//!
//! - `POST /api/scripts/:id/run` body `{device_id, start_index?, args?}`：
//!   统一 RunTarget::Script（CONTRACT §4.4）；`args` 为稀疏映射（§4.3），
//!   提交前按声明七类解析并合并默认值，诊断失败 → 400 {error:"invalid_args",
//!   diagnostics:[...]};；
//! - `POST /api/functions/:id/run` body `{device_id, function?, start_index?,
//!   args?}`：函数测试（RunTarget::Function），RunRecord.script_id 记
//!   `<pkg>/<file>.yaml[#函数]` 展示标签；函数库不进脚本运行接口，
//!   函数体内步骤定位 = function + start_index。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use super::common::{err_response, run_blocking_api, validate_text_field};
use super::{ApiError, AppState};
use crate::engine::RunTarget;
use crate::store::Db;

/// 稀疏 args 请求形态（手动运行 / 函数测试共用）。
#[derive(Deserialize)]
pub(super) struct RunReqArgs {
    pub(super) device_id: String,
    /// 从第几个顶层步骤开始运行（0=从头；「从某行运行」选中逻辑行时传入）
    #[serde(default)]
    pub(super) start_index: Option<usize>,
    /// 函数测试：目标函数名；缺省 = 文件第一个函数
    #[serde(default)]
    pub(super) function: Option<String>,
    /// 稀疏参数覆盖：键 = 参数名，值按七类解析（bool=布尔、coord=[x,y]、
    /// 其余五类=字符串）
    #[serde(default)]
    pub(super) args: Option<serde_json::Map<String, Value>>,
}

pub(super) fn validate_run_req(req: &RunReqArgs) -> Result<(), ApiError> {
    validate_text_field(&req.device_id, "device_id", 255)?;
    if req.start_index.is_some_and(|index| index > 100_000) {
        return Err(ApiError::bad_request("start_index 超过脚本步数上限"));
    }
    if let Some(func) = req.function.as_deref().filter(|v| !v.trim().is_empty()) {
        validate_text_field(func, "function", 255)?;
    }
    Ok(())
}

/// 结构化诊断 400 响应（CONTRACT §5.1 五元组列表，前端按 code/step_path 定位）。
/// 任务保存的 args 解析与脚本解析诊断共用同一形态。
pub(super) fn diagnostics_response(diagnostics: &[crate::script_v2::ScriptError]) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": "invalid_args",
            "diagnostics": diagnostics,
        })),
    )
        .into_response()
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

/// 统一提交入口：解析 args（blocking 池内做磁盘快照 + 严格解析）→
/// RunManager.submit → 202 {run_id, state, resolved_args}。
async fn submit_run(
    st: &AppState,
    device_id: String,
    target: RunTarget,
    args_json: Option<serde_json::Map<String, Value>>,
) -> Response {
    // args 解析需要分区快照（磁盘 IO + 严格解析），放 blocking 池。
    // 闭包返回 Result<绑定结果, ApiError> 以适配 run_blocking_api；诊断在
    // 内层 Result 里透传给 400 响应。
    let scripts = st.scripts.clone();
    let bound = {
        let target = target.clone();
        match run_blocking_api(move || {
            let r: Result<crate::engine::BoundEntryArgs, Vec<crate::script_v2::ScriptError>> =
                crate::engine::resolve_entry_args(
                    &scripts,
                    &target,
                    &args_json.unwrap_or_default(),
                );
            Ok(r)
        })
        .await
        {
            Ok(Ok(bound)) => bound,
            Ok(Err(diagnostics)) => return diagnostics_response(&diagnostics),
            Err(e) => return e.into_response(),
        }
    };
    let rreq = crate::run_manager::StartRequest {
        device_id,
        target,
        source: crate::run_manager::RunSource::Manual,
        task_id: None,
        scheduled_at: None,
        args: bound.overrides,
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
                "resolved_args": bound.resolved,
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

/// POST /api/scripts/:id/run（id = `<pkg>/<name>.yaml`，整体 encodeURIComponent）
pub(super) async fn api_run_script(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RunReqArgs>,
) -> Response {
    // 脚本存在性先校验（404 优先于设备冲突）
    let scripts = st.scripts.clone();
    let probe = id.clone();
    let exists = match run_blocking_api(move || {
        scripts
            .get(&probe)
            .map(|s| s.is_some())
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(ok) => ok,
        Err(e) => return e.into_response(),
    };
    if !exists {
        return ApiError::not_found("脚本不存在").into_response();
    }
    if let Err(err) = validate_run_req(&req) {
        return err.into_response();
    }
    // RUN-002 契约：启动即返回 202 {run_id, state:"starting"}，不等脚本结束；
    // 设备级互斥冲突 → 409 {error:"device_busy", run_id, script_id, source, started_at}
    let target = RunTarget::Script {
        script_id: id,
        start_index: req.start_index.unwrap_or(0),
    };
    submit_run(&st, req.device_id, target, req.args).await
}

/// POST /api/functions/:id/run（id = `<pkg>/<文件短路径>.yaml`）：
/// 函数测试运行（RunTarget::Function）。函数文件不存在 → 404；
/// 指定 function 不存在 → 400 诊断（resource.func.not_found）。
pub(super) async fn api_run_function(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RunReqArgs>,
) -> Response {
    // 函数文件存在性先校验（404 优先于设备冲突）；文件短路径去扩展名
    let scripts = st.scripts.clone();
    let probe = id.clone();
    let file_short = match run_blocking_api(move || {
        scripts
            .get_function(&probe)
            .map(|f| f.map(|f| f.file))
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(Some(file)) => file,
        Ok(None) => return ApiError::not_found("函数文件不存在").into_response(),
        Err(e) => return e.into_response(),
    };
    if let Err(err) = validate_run_req(&req) {
        return err.into_response();
    }
    let Some((pkg, _)) = id.split_once('/') else {
        return ApiError::not_found("函数文件不存在").into_response();
    };
    let function = req
        .function
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let target = RunTarget::Function {
        pkg: pkg.to_string(),
        file: file_short,
        function,
        start_index: req.start_index.unwrap_or(0),
    };
    submit_run(&st, req.device_id, target, req.args).await
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
