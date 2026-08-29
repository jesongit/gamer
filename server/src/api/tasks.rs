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
//! - 列表/详情带 `param_stale`（签名过期或无快照，前端展示"参数已过期"）；
//! - 「立即运行」走已存快照（过同一签名门禁，过期明确失败不空跑）。

use std::time::Instant;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use super::common::{err_response, run_blocking_api, validate_text_field};
use super::runs::diagnostics_response;
use super::{ApiError, AppState};
use crate::scheduler::next_run;
use crate::store::Task;
use crate::task_params::{self, GateError};

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
    stored: Option<&str>,
    current: &str,
    message: &str,
) -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "code": task_params::CODE_SIGNATURE_CONFLICT,
            "reason": if stored.is_none() {
                task_params::REASON_NO_SNAPSHOT
            } else {
                task_params::REASON_SIGNATURE_MISMATCH
            },
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
fn task_json(t: &Task, next: &str, param_stale: bool, with_args: bool) -> Value {
    let mut v = serde_json::json!({
        "id": t.id, "name": t.name, "cron": t.cron, "script_id": t.script_id,
        "device_id": t.device_id, "enabled": t.enabled, "last_result": t.last_result,
        "last_run_at": t.last_run_at, "next_run": next,
        "param_stale": param_stale,
        "has_args": t.args_json.is_some(),
        "param_signature": t.param_signature,
    });
    if with_args {
        let args = t
            .args_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .unwrap_or(Value::Null);
        v["args"] = args;
    }
    v
}

/// 逐任务计算 param_stale：脚本缺失/解析失败/无快照/签名不一致都算过期
/// （统一口径：不能按快照安全运行的任务都提示重新确认）。
fn param_stale_of(scripts: &crate::scripts::ScriptStore, t: &Task) -> bool {
    match task_params::probe_script_signature(scripts, &t.script_id) {
        Ok((_, current)) => t.param_signature.as_deref() != Some(current.as_str()),
        Err(_) => true,
    }
}

// ---------- 定时任务 ----------

pub(super) async fn api_list_tasks(State(st): State<AppState>) -> Response {
    let db = st.db.clone();
    let scripts = st.scripts.clone();
    let out = match run_blocking_api(move || {
        let tasks = db
            .list_tasks()
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(tasks
            .into_iter()
            .map(|t| {
                let next = if t.enabled {
                    next_run(&t.cron)
                        .map(|x| x.format("%Y-%m-%d %H:%M:%S").to_string())
                        .unwrap_or_else(|| "-".into())
                } else {
                    "-".into()
                };
                let stale = param_stale_of(&scripts, &t);
                task_json(&t, &next, stale, false)
            })
            .collect::<Vec<_>>())
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
    let db = st.db.clone();
    let scripts = st.scripts.clone();
    match run_blocking_api(move || {
        let task = db
            .list_tasks()
            .map_err(|e| ApiError::internal(e.to_string()))?
            .into_iter()
            .find(|t| t.id == id);
        let Some(t) = task else {
            return Err(ApiError::not_found("任务不存在"));
        };
        let stale = param_stale_of(&scripts, &t);
        let next = if t.enabled {
            next_run(&t.cron)
                .map(|x| x.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "-".into())
        } else {
            "-".into()
        };
        Ok(task_json(&t, &next, stale, true))
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
    if !crate::scheduler::validate_cron(&req.cron) {
        return err_response(StatusCode::BAD_REQUEST, "cron 表达式无效");
    }
    if let Err(err) = validate_task_req(&req) {
        return err.into_response();
    }
    let id = req
        .id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let db = st.db.clone();
    let scripts = st.scripts.clone();
    // 快照解析需要分区快照（磁盘 IO + 严格解析），整体放 blocking 池。
    // inner 直接产出 Response（409 冲突体与结构化诊断 400 均非 ApiError 文本形状）。
    let result = run_blocking_api(move || -> Result<Response, ApiError> {
        Ok(save_task_inner(db, scripts, id, req))
    })
    .await;
    match result {
        Ok(resp) => resp,
        Err(err) => err.into_response(),
    }
}

fn save_task_inner(
    db: crate::store::Db,
    scripts: std::sync::Arc<crate::scripts::ScriptStore>,
    id: String,
    req: SaveTaskReq,
) -> Response {
    use crate::engine::RunTarget;
    let existing = match db.list_tasks() {
        Ok(list) => list.into_iter().find(|t| t.id == id),
        Err(e) => return ApiError::internal(e.to_string()).into_response(),
    };
    // 脚本当前声明 + psig1 签名（缺失 → 404；解析失败 → 400 结构化诊断）
    let (decls, current_sig) = match task_params::probe_script_signature(&scripts, &req.script_id) {
        Ok(v) => v,
        Err(GateError::ScriptMissing) => {
            return ApiError::not_found("脚本不存在，无法保存任务参数").into_response();
        }
        Err(GateError::ScriptInvalid(diags)) => return diagnostics_response(&diags),
        Err(_) => return ApiError::internal("参数探测失败").into_response(),
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
            Err(diags) => return diagnostics_response(&diags),
        },
        None => match existing.as_ref() {
            Some(prev)
                if prev.args_json.is_some()
                    && prev.param_signature.as_deref() == Some(current_sig.as_str()) =>
            {
                // 声明未变：原快照原签名原样保留
                (
                    prev.args_json.clone().unwrap_or_default(),
                    prev.param_signature.clone().unwrap_or_default(),
                )
            }
            Some(prev) if req.reconfirm => {
                // 不带 args 的重新确认：存活参数保留原值、新参数取当前默认值、
                // 已删参数丢弃；必填缺失仍走 400 结构化诊断
                let old = prev.args_json.as_deref().unwrap_or("{}");
                match task_params::rebind_snapshot(&decls, old, &req.script_id) {
                    Ok(bound) => (
                        task_params::typed_pairs_to_json(bound).to_string(),
                        current_sig,
                    ),
                    Err(diags) => return diagnostics_response(&diags),
                }
            }
            Some(prev) => {
                // 签名不一致（或无快照）且未 reconfirm → 409，前端弹重新确认
                let (message, stored) = match prev.param_signature.as_deref() {
                    None => (GateError::NoSnapshot.message(), None),
                    Some(stored) => (
                        GateError::SignatureMismatch {
                            stored: stored.to_string(),
                            current: current_sig.clone(),
                        }
                        .message(),
                        Some(stored),
                    ),
                };
                return signature_conflict_response(
                    &prev.id,
                    &req.script_id,
                    stored,
                    &current_sig,
                    &message,
                );
            }
            None => {
                // 新建不带 args：纯当前默认值打底（必填缺失 → 400 结构化诊断）
                match crate::engine::resolve_entry_args(&scripts, &target, &serde_json::Map::new())
                {
                    Ok(bound) => (bound.resolved.to_string(), bound.param_signature),
                    Err(diags) => return diagnostics_response(&diags),
                }
            }
        },
    };
    let parsed_args: Value = serde_json::from_str(&args_json).unwrap_or(Value::Null);
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
        args_json: Some(args_json),
        param_signature: Some(signature.clone()),
    };
    if let Err(e) = db.upsert_task(&task) {
        return ApiError::internal(e.to_string()).into_response();
    }
    Json(serde_json::json!({
        "ok": true,
        "id": task.id,
        "args": parsed_args,
        "param_signature": signature,
    }))
    .into_response()
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
/// 连接等任务完成；设备冲突 409 device_busy；参数门禁未过 409
/// param_signature_conflict；停机 drain 中 503。
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
        Err(RunNowError::ParamStale(err)) => {
            st.metrics
                .record_scheduler_trigger(trigger_started.elapsed().as_millis() as u64);
            st.metrics
                .record_scheduler_event(crate::metrics::SchedulerEvent::Failed);
            signature_conflict_response(
                &task.id,
                &task.script_id,
                task.param_signature.as_deref(),
                // expected 只在签名可比对时给出；无快照场景无期望值
                match &err {
                    GateError::SignatureMismatch { current, .. } => current.as_str(),
                    _ => "",
                },
                &err.message(),
            )
        }
        Err(RunNowError::ScriptMissing) => {
            st.metrics
                .record_scheduler_trigger(trigger_started.elapsed().as_millis() as u64);
            st.metrics
                .record_scheduler_event(crate::metrics::SchedulerEvent::Failed);
            err_response(StatusCode::BAD_REQUEST, "脚本不存在")
        }
        Err(RunNowError::ScriptInvalid(message)) => {
            st.metrics
                .record_scheduler_trigger(trigger_started.elapsed().as_millis() as u64);
            st.metrics
                .record_scheduler_event(crate::metrics::SchedulerEvent::Failed);
            err_response(StatusCode::BAD_REQUEST, &format!("脚本解析失败: {message}"))
        }
    }
}
