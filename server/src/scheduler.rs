//! 定时任务调度：cron 表达式 + tokio 后台调度（Docker 内 7×24 运行）
//!
//! 阶段 3 起（RUN-001/002）执行全部经 RunManager 统一仲裁：
//! 触发命中 → `RunManager::submit(source=scheduled)` → run_id；设备被手动运行
//! 占用时按策略记 skipped/conflict 并落任务结果，绝不注入控制。
//! 本文件只负责"何时触发"（触发判重的持久化幂等与 misfire 策略属 RUN-004）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, TimeZone, Utc};
use cron::Schedule;
use std::str::FromStr;

use tracing::{debug, error, info, warn};

use crate::metrics::{Metrics, SchedulerEvent};
use crate::run_manager::{
    FinishHook, RunManager, RunOutcome, RunSource, RunState, StartError, StartRequest,
};
use crate::scripts::ScriptStore;
use crate::store::{Db, Task};
use crate::task_params::{self, GateError, TaskArgs};

/// misfire 窗口沿用原调度器的一小时回看范围：恢复时只补最近一次，窗口外不补跑。
const MISFIRE_WINDOW_SECS: i64 = 60 * 60;

/// 将 5 字段标准 cron（分 时 日 月 周）规范化为 cron crate 的 7 字段格式
pub fn normalize_cron(expr: &str) -> String {
    let expr = expr.trim();
    if expr.starts_with('@') {
        return expr.to_string(); // @daily/@hourly 等
    }
    let parts: Vec<&str> = expr.split_whitespace().collect();
    match parts.len() {
        5 => format!("0 {} *", expr), // 秒=0，年=*
        6 => format!("0 {}", expr),   // 秒=0
        _ => expr.to_string(),
    }
}

/// 校验 cron 表达式（5/6/7 字段均可）
pub fn validate_cron(expr: &str) -> bool {
    Schedule::from_str(&normalize_cron(expr)).is_ok()
}

/// 任务触发的进程内占位，持久化 `(task_id, scheduled_at)` 才是最终幂等边界。
type TriggerMap = Arc<tokio::sync::Mutex<HashMap<String, bool>>>;

pub struct Scheduler {
    db: Db,
    /// 统一运行管理：所有定时/立即执行经此仲裁（冲突 409 语义 / RAII 清理 / cancel）
    runs: Arc<RunManager>,
    /// 脚本文件存储：按 script_id（package/name）取脚本内容
    scripts: Arc<ScriptStore>,
    triggers: TriggerMap,
}

impl Scheduler {
    pub fn new(db: Db, scripts: Arc<ScriptStore>, runs: Arc<RunManager>) -> Self {
        Self {
            db,
            runs,
            scripts,
            triggers: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    /// 启动调度循环：每 10s 扫描一次所有启用任务
    pub async fn start(&self) {
        let db = self.db.clone();
        let runs = self.runs.clone();
        let scripts = self.scripts.clone();
        let triggers = self.triggers.clone();
        info!("scheduler started");
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(10));
            loop {
                tick.tick().await;
                let tasks = match db.list_tasks_async().await {
                    Ok(t) => t,
                    Err(e) => {
                        error!("list tasks failed: {}", e);
                        continue;
                    }
                };
                let now = Local::now();
                for task in tasks {
                    if !task.enabled {
                        continue;
                    }
                    // 只取窗口内最近一个触发点：恢复时不批量补跑，窗口外的历史
                    // 触发自然被跳过；scheduled_at 以 Unix 秒保存为 UTC 规范值。
                    let trigger = match Schedule::from_str(&normalize_cron(&task.cron)) {
                        Ok(sched) => latest_due_trigger(&sched, now),
                        Err(e) => {
                            warn!("invalid cron {}: {}", task.cron, e);
                            None
                        }
                    };
                    if let Some(trigger_time) = trigger {
                        let mut map = triggers.lock().await;
                        let entry = map.entry(task.id.clone()).or_insert(false);
                        // 同一任务的上一触发还在跑：本触发点也标已处理（不叠加排队，
                        // 10s tick 内重复命中同一点由判重挡住）
                        if *entry {
                            continue;
                        }
                        *entry = true;
                        drop(map);
                        let db2 = db.clone();
                        let runs2 = runs.clone();
                        let scripts2 = scripts.clone();
                        let triggers2 = triggers.clone();
                        let task2 = task.clone();
                        tokio::spawn(async move {
                            // OBS-002 关联字段：task_id + device_id 随触发日志落盘
                            info!(task = %task2.name, task_id = %task2.id, device = %task2.device_id, "scheduled run triggered");
                            dispatch(&db2, &runs2, &scripts2, &task2, Some(trigger_time)).await;
                            // 复位占位；同一触发点的跨 tick/重启判重由 scheduled_runs
                            // 唯一索引负责。
                            let mut map = triggers2.lock().await;
                            if let Some(entry) = map.get_mut(&task2.id) {
                                *entry = false;
                            }
                        });
                    }
                }
            }
        });
    }

    /// 立即运行任务（手动触发）：202 契约 —— 提交即返回 run_id，不等完成。
    /// 参数门禁与计划触发同口径：脚本缺失/解析失败 → 4xx；签名过期 →
    /// ParamStale（API 映射 409 param_signature_conflict，要求重新确认）。
    pub async fn run_now(&self, task: &Task) -> Result<String, RunNowError> {
        info!(task = %task.name, task_id = %task.id, device = %task.device_id, "manual trigger (task now)");
        let task_args = match task_params::gate_task(&self.scripts, task) {
            Ok(ok) => ok,
            Err(err) => {
                warn!(
                    task = %task.name,
                    script = %task.script_id,
                    reason = %err.reason(),
                    detail = %err.message(),
                    "task now rejected: param gate failed"
                );
                record_scheduler_failure(&self.db.metrics());
                return Err(match &err {
                    GateError::ScriptMissing => RunNowError::ScriptMissing,
                    GateError::ScriptInvalid(_) => RunNowError::ScriptInvalid(err.message()),
                    GateError::SignatureMismatch { .. } => RunNowError::ParamStale(err),
                });
            }
        };
        log_task_params("task now", task, &task_args);
        let result = submit_run(&self.runs, &self.db, task, None, task_args)
            .await
            .map_err(RunNowError::Start);
        match &result {
            Ok(_) => {}
            Err(RunNowError::Start(StartError::Conflict(_))) => {
                record_scheduler_event(&self.db.metrics(), SchedulerEvent::Conflict);
            }
            Err(RunNowError::Start(StartError::ShuttingDown)) => {
                record_scheduler_event(&self.db.metrics(), SchedulerEvent::Skipped);
            }
            Err(RunNowError::ScriptMissing | RunNowError::ScriptInvalid(_)) => {
                record_scheduler_failure(&self.db.metrics());
            }
            Err(RunNowError::ParamStale(_)) => {
                record_scheduler_failure(&self.db.metrics());
            }
        }
        result
    }

    /// 距下一次**启用** cron 任务触发的秒数（OPS-005：install 冻结窗口门禁的
    /// 时间维度；禁用任务不计、非法表达式跳过、无启用任务返回 None）。
    /// 本地时区口径，与调度触发判定一致。
    pub fn next_enabled_trigger_in_secs(&self) -> Option<i64> {
        let tasks = self.db.list_tasks().ok()?;
        next_enabled_trigger_in_secs_from(&tasks, Local::now())
    }
}

/// 纯函数（可注入 now 单测）：启用任务的最近下一次触发距 `now` 的秒数
pub fn next_enabled_trigger_in_secs_from(
    tasks: &[Task],
    now: chrono::DateTime<Local>,
) -> Option<i64> {
    let mut best: Option<i64> = None;
    for task in tasks {
        if !task.enabled {
            continue;
        }
        let Ok(sched) = Schedule::from_str(&normalize_cron(&task.cron)) else {
            continue;
        };
        // `Schedule::after` is strictly exclusive. Probe one nanosecond before
        // `now` so a trigger exactly at the current instant is reported as 0s,
        // which keeps the cron freeze gate closed at the boundary.
        let probe = now - chrono::Duration::nanoseconds(1);
        let Some(next) = sched.after(&probe).next() else {
            continue;
        };
        let secs = (next - now).num_seconds().max(0);
        best = Some(match best {
            Some(b) => b.min(secs),
            None => secs,
        });
    }
    best
}

/// 参数确认日志：只记参数名列表与签名（短码 + 全串），**不记参数值**
/// （text 参数防泄露）。
fn log_task_params(scene: &str, task: &Task, args: &TaskArgs) {
    info!(
        scene,
        task = %task.name,
        task_id = %task.id,
        script = %task.script_id,
        params = args.names.join(","),
        signature = %args.signature,
        signature_short = %task_params::signature_short_code(&args.signature),
        "task params confirmed"
    );
}

/// 立即运行的错误面（API 映射：ScriptMissing/ScriptInvalid→400、
/// ParamStale→409 param_signature_conflict、Start→409 device_busy/503）
#[derive(Debug)]
pub enum RunNowError {
    ScriptMissing,
    /// 脚本读取/严格解析失败（携带人类可读摘要）
    ScriptInvalid(String),
    /// 参数门禁未过：签名过期，需重新确认
    ParamStale(GateError),
    Start(StartError),
}

/// 计划触发的执行入口：取脚本 → 提交 RunManager。
/// 冲突（设备正被手动运行占用）→ 按策略记 skipped/conflict，不注入控制。
async fn dispatch(
    db: &Db,
    runs: &Arc<RunManager>,
    scripts: &Arc<ScriptStore>,
    task: &Task,
    trigger: Option<DateTime<Local>>,
) {
    let scheduled_at = trigger.map(|t| t.timestamp());
    let metrics = db.metrics();
    if let Some(scheduled_at) = scheduled_at {
        record_scheduler_trigger_latency(&metrics, scheduled_at);
    }
    if let Some(scheduled_at) = scheduled_at {
        match db.claim_scheduled_run_async(&task.id, scheduled_at).await {
            Ok(true) => {}
            Ok(false) => {
                debug!(
                    task = %task.name,
                    scheduled_at,
                    "scheduled trigger already claimed"
                );
                record_scheduler_event(&metrics, SchedulerEvent::Skipped);
                return;
            }
            Err(e) => {
                error!(
                    task = %task.name,
                    scheduled_at,
                    err = %e,
                    "scheduled trigger claim failed"
                );
                record_scheduler_event(&metrics, SchedulerEvent::Failed);
                return;
            }
        }
    }
    // 参数门禁（plan §12.3 阶段 5）：脚本存在性 / 严格解析 / 快照签名统一
    // 口径——任何失败都明确落失败结果，绝不空跑、绝不静默继承新默认值。
    let task_args = match task_params::gate_task(scripts, task) {
        Ok(ok) => ok,
        Err(GateError::ScriptMissing) => {
            warn!(
                task = %task.name,
                script = %task.script_id,
                "scheduled skip: script not found"
            );
            record_scheduler_event(&metrics, SchedulerEvent::Failed);
            finish_scheduled_run(db, task, scheduled_at, "failed", None, Some("脚本不存在")).await;
            mark_task_result(db, task, "失败", Some("任务执行失败: 脚本不存在")).await;
            return;
        }
        Err(err @ GateError::ScriptInvalid(_)) => {
            warn!(
                task = %task.name,
                script = %task.script_id,
                detail = %err.message(),
                "scheduled skip: script invalid"
            );
            record_scheduler_event(&metrics, SchedulerEvent::Failed);
            finish_scheduled_run(db, task, scheduled_at, "failed", None, Some("脚本解析失败"))
                .await;
            mark_task_result(db, task, "失败", Some("任务执行失败: 脚本解析失败")).await;
            return;
        }
        Err(ref err @ GateError::SignatureMismatch { ref stored, .. }) => {
            // 签名过期：日志带期望 vs 实际签名短码（不含参数值）
            warn!(
                task = %task.name,
                task_id = %task.id,
                script = %task.script_id,
                reason = %err.reason(),
                actual_signature = %task_params::signature_short_code(stored),
                "scheduled skip: task params stale, reconfirm required"
            );
            record_scheduler_event(&metrics, SchedulerEvent::Failed);
            finish_scheduled_run(db, task, scheduled_at, "failed", None, Some(&err.message()))
                .await;
            mark_task_result(
                db,
                task,
                "失败",
                Some(&format!("任务执行失败: {}", err.message())),
            )
            .await;
            return;
        }
    };
    log_task_params("scheduled", task, &task_args);
    match submit_run(runs, db, task, trigger, task_args).await {
        Ok(run_id) => {
            debug!(task = %task.name, task_id = %task.id, device = %task.device_id, %run_id, "scheduled run submitted");
        }
        Err(StartError::Conflict(busy)) => {
            // 设备正忙（大概率手动运行中）：第一版策略不排队——记 skipped、更新任务
            // 结果，不向对方会话注入任何控制
            info!(
                task = %task.name,
                busy_run = %busy.run_id,
                busy_script = %busy.script_id,
                skipped = "conflict",
                "scheduled trigger skipped: device busy"
            );
            record_scheduler_event(&metrics, SchedulerEvent::Conflict);
            finish_scheduled_run(db, task, scheduled_at, "skipped", None, Some("设备忙")).await;
            mark_task_result(
                db,
                task,
                "失败",
                Some("任务执行失败: 设备忙（跳过本次触发）"),
            )
            .await;
        }
        Err(StartError::ShuttingDown) => {
            warn!(task = %task.name, "scheduled trigger dropped: server draining");
            record_scheduler_event(&metrics, SchedulerEvent::Skipped);
            finish_scheduled_run(
                db,
                task,
                scheduled_at,
                "skipped",
                None,
                Some("服务正在关闭"),
            )
            .await;
        }
    }
}

/// 组装 StartRequest 并提交（trigger 有值=Scheduled，无=TaskNow）。
/// `task_args` = 已过签名门禁的完整参数快照（签名 + 参数名 + 全量类型化
/// 覆盖）；快照是全量，天然不静默继承脚本新默认值（plan §12.3）。
async fn submit_run(
    runs: &Arc<RunManager>,
    db: &Db,
    task: &Task,
    trigger: Option<DateTime<Local>>,
    task_args: TaskArgs,
) -> Result<String, StartError> {
    let scheduled_at = trigger.map(|t| t.timestamp());
    let hook = task_finish_hook(db.clone(), task.id.clone(), scheduled_at);
    let req = StartRequest {
        device_id: task.device_id.clone(),
        target: crate::engine::RunTarget::Script {
            script_id: task.script_id.clone(),
            start_index: 0,
        },
        source: if trigger.is_some() {
            RunSource::Scheduled
        } else {
            RunSource::TaskNow
        },
        task_id: Some(task.id.clone()),
        scheduled_at,
        args: task_args.overrides,
        realtime_logs: false,
    };
    let rec = runs.submit(req, Some(hook))?;
    if let Some(scheduled_at) = scheduled_at {
        let runs = runs.clone();
        let db = db.clone();
        let task = task.clone();
        let run_id = rec.run_id.clone();
        tokio::spawn(async move {
            watch_scheduled_completion(runs, db, task, scheduled_at, run_id).await;
        });
    }
    Ok(rec.run_id)
}

/// 任务结果落库：更新 last_result / last_run_at + 可选摘要日志
async fn mark_task_result(db: &Db, task: &Task, result: &str, summary_log: Option<&str>) {
    if let Some(msg) = summary_log {
        let _ = db
            .add_log_async(&task.device_id, &task.script_id, "error", msg)
            .await;
    }
    upsert_task_result(db, &task.id, |t| {
        t.last_result = Some(result.to_string());
        t.last_run_at = Some(now_utc_string());
    })
    .await;
}

fn now_utc_string() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

async fn upsert_task_result(db: &Db, task_id: &str, apply: impl FnOnce(&mut Task)) {
    let mut t = match db.list_tasks_async().await {
        Ok(ts) => match ts.into_iter().find(|x| x.id == task_id) {
            Some(task) => task,
            None => return,
        },
        Err(e) => {
            error!(task = %task_id, err = %e, "load task result target failed");
            return;
        }
    };
    apply(&mut t);
    if let Err(e) = db.upsert_task_async(&t).await {
        error!(task = %task_id, err = %e, "update task result failed");
    }
}

/// 任务级完成钩子：终态写回 last_result/last_run_at 并批量落库引擎日志
/// （调度批量模式 realtime_logs=false，引擎日志未实时入库，由这里统一写入）
fn task_finish_hook(db: Db, task_id: String, scheduled_at: Option<i64>) -> FinishHook {
    Arc::new(move |rec, outcome| {
        let logs = outcome.logs().to_vec();
        let has_error =
            matches!(outcome, RunOutcome::Failed(_, _)) || logs.iter().any(|(l, _)| l == "error");
        let label = match outcome {
            RunOutcome::Cancelled(_) => "取消",
            _ if has_error => "失败",
            _ => "成功",
        };
        let rid = rec.run_id.clone();
        let db = db.clone();
        let device_id = rec.device_id.clone();
        let script_id = rec.script_id.clone();
        let run_id = rec.run_id.clone();
        let scheduled_at = scheduled_at;
        let task_id_for_log = task_id.clone();
        let result_label = label.to_string();
        let scheduled_state = match outcome {
            RunOutcome::Cancelled(_) => "skipped",
            _ if has_error => "failed",
            _ => "success",
        };
        let scheduled_error = match outcome {
            RunOutcome::Failed(msg, _) => Some(msg.clone()),
            RunOutcome::Cancelled(_) => Some("运行被取消".to_string()),
            RunOutcome::Success(_) => None,
        };
        tokio::spawn(async move {
            for (level, msg) in logs {
                let _ = db.add_log_async(&device_id, &script_id, &level, &msg).await;
            }
            if let Some(scheduled_at) = scheduled_at {
                let _ = db
                    .finish_scheduled_run_async(
                        &task_id_for_log,
                        scheduled_at,
                        scheduled_state,
                        Some(&run_id),
                        scheduled_error.as_deref(),
                    )
                    .await;
            }
            upsert_task_result(&db, &task_id_for_log, |t| {
                t.last_result = Some(result_label);
                t.last_run_at = Some(now_utc_string());
            })
            .await;
            debug!(%task_id_for_log, %rid, result = label, "task finish hook applied");
        });
    })
}

/// 计算 cron 下次执行时间（用于 API 预览）
pub fn next_run(cron_expr: &str) -> Option<DateTime<Local>> {
    let sched = Schedule::from_str(&normalize_cron(cron_expr)).ok()?;
    sched
        .after(&Local::now())
        .next()
        .map(|t| Local.timestamp_opt(t.timestamp(), 0).unwrap())
}

/// 返回 misfire 窗口内不晚于 now 的最新触发点。
fn latest_due_trigger(sched: &Schedule, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let window_start = now - chrono::Duration::seconds(MISFIRE_WINDOW_SECS);
    sched.after(&window_start).take_while(|t| *t <= now).last()
}

async fn finish_scheduled_run(
    db: &Db,
    task: &Task,
    scheduled_at: Option<i64>,
    state: &str,
    run_id: Option<&str>,
    error: Option<&str>,
) {
    if let Some(scheduled_at) = scheduled_at {
        if let Err(e) = db
            .finish_scheduled_run_async(&task.id, scheduled_at, state, run_id, error)
            .await
        {
            error!(task = %task.name, scheduled_at, err = %e, "finish scheduled run failed");
        }
    }
}

/// `RunManager` 的 FinishHook 不覆盖 starting 阶段取消/prepare 失败，因此提交成功后
/// 额外观察终态；只有持久记录仍为 running 时才会由观察器接管，避免与 hook 双写覆盖。
async fn watch_scheduled_completion(
    runs: Arc<RunManager>,
    db: Db,
    task: Task,
    scheduled_at: i64,
    run_id: String,
) {
    loop {
        let Some(rec) = runs.get_run(&run_id) else {
            return;
        };
        let Some(state) = persisted_scheduled_state(rec.state) else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        let changed = match db
            .finish_scheduled_run_async(
                &task.id,
                scheduled_at,
                state,
                Some(&run_id),
                rec.error.as_deref(),
            )
            .await
        {
            Ok(changed) => changed,
            Err(e) => {
                error!(task = %task.name, %run_id, err = %e, "scheduled completion persistence failed");
                return;
            }
        };
        if changed {
            let label = match rec.state {
                RunState::Success => "成功",
                RunState::Failed => "失败",
                RunState::Cancelled => "取消",
                _ => unreachable!("non-terminal run state mapped as terminal"),
            };
            upsert_task_result(&db, &task.id, |t| {
                t.last_result = Some(label.to_string());
                t.last_run_at = Some(now_utc_string());
            })
            .await;
        }
        return;
    }
}

fn record_scheduler_trigger_latency(metrics: &Metrics, scheduled_at: i64) {
    let now = Utc::now().timestamp();
    metrics.record_scheduler_trigger(now.saturating_sub(scheduled_at) as u64);
}

fn record_scheduler_event(metrics: &Metrics, event: SchedulerEvent) {
    metrics.record_scheduler_event(event);
}

fn record_scheduler_failure(metrics: &Metrics) {
    metrics.record_scheduler_event(SchedulerEvent::Failed);
}

fn persisted_scheduled_state(state: RunState) -> Option<&'static str> {
    match state {
        RunState::Success => Some("success"),
        RunState::Failed => Some("failed"),
        RunState::Cancelled => Some("skipped"),
        RunState::Starting | RunState::Running | RunState::Stopping => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn misfire_uses_latest_due_trigger_in_one_hour_window() {
        let sched = Schedule::from_str("0 * * * * * *").unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-27T12:34:56+00:00")
            .unwrap()
            .with_timezone(&Local);
        let trigger = latest_due_trigger(&sched, now).unwrap();
        assert_eq!(trigger.timestamp(), 1_787_834_040);
    }

    /// OPS-005：下一次启用触发秒数——启用任务取最小、禁用不计、非法表达式
    /// 跳过、无启用任务 None（更新安装冻结窗口门禁的时间维度）
    #[test]
    fn next_enabled_trigger_secs_picks_minimum_of_enabled_tasks() {
        // Construct local wall-clock values explicitly: production scheduling
        // is local-time based, and the test must be stable on hosts outside
        // UTC (e.g. Asia/Shanghai).
        let now = Local
            .with_ymd_and_hms(2026, 8, 31, 10, 0, 20)
            .single()
            .unwrap();
        let task = |id: &str, cron: &str, enabled: bool| Task {
            id: id.into(),
            name: id.into(),
            cron: cron.into(),
            script_id: "com.x/y.yaml".into(),
            device_id: "d".into(),
            enabled,
            last_result: None,
            last_run_at: None,
            created_at: "2026-08-29T00:00:00Z".into(),
            args_json: "{}".into(),
            param_signature: "psig1|".into(),
        };
        // 每 5 分钟触发：下一次 10:05:00 → 280s
        let every5 = task("a", "*/5 * * * *", true);
        assert_eq!(
            next_enabled_trigger_in_secs_from(std::slice::from_ref(&every5), now),
            Some(280)
        );
        // 禁用任务不计
        let mut disabled = every5.clone();
        disabled.enabled = false;
        assert_eq!(next_enabled_trigger_in_secs_from(&[disabled], now), None);
        // 多任务取最小；禁用/非法表达式被跳过
        let hourly = task("b", "0 11 * * *", true); // 11:00 → 3580s
        let broken = task("c", "not a cron", true);
        assert_eq!(
            next_enabled_trigger_in_secs_from(&[hourly, broken.clone(), every5], now),
            Some(280)
        );
        // 只有非法表达式 → None（不 panic、不误报临近）
        assert_eq!(next_enabled_trigger_in_secs_from(&[broken], now), None);
        // 恰在触发点：0s（冻结窗口门禁语义下必然阻塞）
        let at_trigger = task("d", "*/5 * * * *", true);
        let exact = Local
            .with_ymd_and_hms(2026, 8, 31, 10, 5, 0)
            .single()
            .unwrap();
        assert_eq!(
            next_enabled_trigger_in_secs_from(&[at_trigger], exact),
            Some(0)
        );
    }

    #[test]
    fn utc_last_run_format_is_parseable_and_ends_in_z() {
        let value = now_utc_string();
        assert!(value.ends_with('Z'));
        assert!(chrono::DateTime::parse_from_rfc3339(&value).is_ok());
    }

    #[test]
    fn scheduler_metrics_helpers_update_low_cardinality_counters() {
        let trigger_metrics = Metrics::default();
        record_scheduler_trigger_latency(&trigger_metrics, Utc::now().timestamp());
        let trigger_snapshot = trigger_metrics.snapshot();
        assert_eq!(trigger_snapshot.scheduler_triggers_total, 1);

        let latency_metrics = Metrics::default();
        let scheduled_at = Utc::now().timestamp().saturating_sub(12);
        record_scheduler_trigger_latency(&latency_metrics, scheduled_at);
        let latency_snapshot = latency_metrics.snapshot();
        assert!(latency_snapshot.scheduler_triggers_total >= 1);
        assert!(latency_snapshot.scheduler_trigger_latency_ms_total >= 12);

        let event_metrics = Metrics::default();
        record_scheduler_event(&event_metrics, SchedulerEvent::Conflict);
        record_scheduler_event(&event_metrics, SchedulerEvent::Skipped);
        record_scheduler_failure(&event_metrics);
        let event_snapshot = event_metrics.snapshot();
        assert_eq!(event_snapshot.scheduler_conflicts_total, 1);
        assert_eq!(event_snapshot.scheduler_skipped_total, 1);
        assert_eq!(event_snapshot.scheduler_failures_total, 1);
    }

    #[test]
    fn dst_fallback_instants_remain_distinct_after_utc_normalization() {
        let first = chrono::DateTime::parse_from_rfc3339("2026-11-01T01:30:00-04:00")
            .unwrap()
            .timestamp();
        let second = chrono::DateTime::parse_from_rfc3339("2026-11-01T01:30:00-05:00")
            .unwrap()
            .timestamp();
        assert_eq!(second - first, 3600);
    }

    #[test]
    fn terminal_states_have_unambiguous_persisted_mapping() {
        assert_eq!(
            persisted_scheduled_state(RunState::Success),
            Some("success")
        );
        assert_eq!(persisted_scheduled_state(RunState::Failed), Some("failed"));
        assert_eq!(
            persisted_scheduled_state(RunState::Cancelled),
            Some("skipped")
        );
        assert_eq!(persisted_scheduled_state(RunState::Running), None);
    }
}

/// 阶段 5：调度/立即运行的参数快照与签名门禁测试。
/// 真脚本走 ScriptStore（临时目录直写 yaml 文件），RunManager 挂捕获假执行器。
#[cfg(test)]
mod task_param_tests {
    use super::*;
    use crate::config::Config;
    use crate::run_manager::RunExecutor;
    use futures_util::future::BoxFuture;
    use std::sync::atomic::AtomicBool;

    /// 带默认值的参数脚本（v12 形态子集；不引模板，免去模板存在性校验）。
    const SCRIPT: &str = "\
params:
  - 'bool:enable:是否启用:true'
  - 'text:message:提示文本:\"hello\"'
steps:
  - log: 'ok'
";

    /// text 参数的敏感标记值：任何日志/落库出现即失败。
    const SECRET_TEXT: &str = "TOPSECRET-TEXT-9a3f";

    struct CaptureExec {
        requests: parking_lot::Mutex<Vec<StartRequest>>,
    }

    impl CaptureExec {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                requests: parking_lot::Mutex::new(Vec::new()),
            })
        }
        fn captured(&self) -> Vec<StartRequest> {
            self.requests.lock().clone()
        }
    }

    impl RunExecutor for CaptureExec {
        fn prepare<'a>(&'a self, _req: &'a StartRequest) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn execute<'a>(
            &'a self,
            req: &'a StartRequest,
            _stop: Arc<AtomicBool>,
        ) -> BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
            Box::pin(async move {
                self.requests.lock().push(req.clone());
                Ok(vec![("info".into(), "fake done".into())])
            })
        }
        fn occupy(&self, _device_id: &str) {}
        fn release(&self, _device_id: &str) {}
    }

    struct Rig {
        db: Db,
        scripts: Arc<ScriptStore>,
        runs: Arc<RunManager>,
        exec: Arc<CaptureExec>,
        scheduler: Scheduler,
        dir: std::path::PathBuf,
    }

    fn rig(tag: &str) -> Rig {
        let dir = std::env::temp_dir().join(format!(
            "gamer-sched-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let db: Db = Arc::new(crate::store::Store::open(&cfg).unwrap());
        let scripts = Arc::new(ScriptStore::open(&cfg).unwrap());
        let exec = CaptureExec::new();
        let runs = Arc::new(RunManager::new(exec.clone()));
        let scheduler = Scheduler::new(db.clone(), scripts.clone(), runs.clone());
        Rig {
            db,
            scripts,
            runs,
            exec,
            scheduler,
            dir,
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn write_script(rig: &Rig, name: &str, content: &str) {
        let dir = rig.dir.join("com.test.app").join("yaml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), content).unwrap();
    }

    fn mk_task(script_id: &str, args_json: &str, sig: &str) -> Task {
        Task {
            id: "task-1".into(),
            name: "Daily".into(),
            cron: "0 * * * * *".into(),
            script_id: script_id.into(),
            device_id: "dev-1".into(),
            enabled: true,
            last_result: None,
            last_run_at: None,
            created_at: "2026-08-29T00:00:00Z".into(),
            args_json: args_json.to_string(),
            param_signature: sig.to_string(),
        }
    }

    /// 等待所有 run 收敛（RunManager wait_settled 是模块私有，这里轮询公开计数）
    async fn settle(runs: &Arc<RunManager>) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while runs.active_count() > 0 {
            assert!(std::time::Instant::now() < deadline, "runs did not settle");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        // 完成钩子是同步回调，收敛即已写回
    }

    #[tokio::test]
    async fn scheduled_dispatch_passes_full_snapshot_args() {
        let rig = rig("dispatch-ok");
        write_script(&rig, "daily.yaml", SCRIPT);
        let (_, signature) =
            task_params::probe_script_signature(&rig.scripts, "com.test.app/daily.yaml").unwrap();
        let snapshot = serde_json::json!({
            "enable": false,
            "message": SECRET_TEXT,
        });
        let snapshot_json = snapshot.to_string();
        let task = mk_task("com.test.app/daily.yaml", &snapshot_json, &signature);
        rig.db.upsert_task(&task).unwrap();

        let trigger = Local::now();
        dispatch(&rig.db, &rig.runs, &rig.scripts, &task, Some(trigger)).await;
        settle(&rig.runs).await;

        let captured = rig.exec.captured();
        assert_eq!(captured.len(), 1, "门禁通过必须恰好提交一次");
        let req = &captured[0];
        assert_eq!(
            req.args,
            vec![
                (
                    "enable".to_string(),
                    crate::script_v2::TypedValue::Bool(false)
                ),
                (
                    "message".to_string(),
                    crate::script_v2::TypedValue::Text(SECRET_TEXT.into())
                ),
            ],
            "调度必须把完整快照作为 args 传入 StartRequest"
        );
        assert!(matches!(req.source, RunSource::Scheduled));
        assert_eq!(rig.db.scheduled_run_count(&task.id, trigger.timestamp()), 1);
        assert_eq!(
            rig.db.scheduled_run_state(&task.id, trigger.timestamp()),
            "success"
        );
    }

    #[tokio::test]
    async fn stale_signature_task_is_not_dispatched_and_marks_failure() {
        let rig = rig("dispatch-stale");
        write_script(&rig, "daily.yaml", SCRIPT);
        let trigger = Local::now();
        let task = mk_task(
            "com.test.app/daily.yaml",
            r#"{"enable":false,"message":"x"}"#,
            "psig1|stale",
        );
        rig.db.upsert_task(&task).unwrap();

        dispatch(&rig.db, &rig.runs, &rig.scripts, &task, Some(trigger)).await;
        settle(&rig.runs).await;

        assert!(rig.exec.captured().is_empty(), "签名过期绝不调度");
        let stored = rig
            .db
            .list_tasks()
            .unwrap()
            .into_iter()
            .find(|t| t.id == "task-1")
            .unwrap();
        assert_eq!(
            stored.last_result.as_deref(),
            Some("失败"),
            "明确失败而非空跑"
        );
        assert!(
            stored.last_run_at.is_some(),
            "失败也要更新 last_run_at（UI 可见）"
        );
    }

    #[tokio::test]
    async fn missing_script_task_is_not_dispatched() {
        let rig = rig("dispatch-missing");
        // 探测不存在的脚本应报 ScriptMissing（保证门禁与既有 404 口径一致）
        match task_params::probe_script_signature(&rig.scripts, "com.test.app/missing.yaml") {
            Err(GateError::ScriptMissing) => {}
            other => panic!("expected ScriptMissing, got ok={}", other.is_ok()),
        }
        let task = mk_task("com.test.app/missing.yaml", "{}", "psig1|");
        rig.db.upsert_task(&task).unwrap();

        dispatch(&rig.db, &rig.runs, &rig.scripts, &task, Some(Local::now())).await;
        settle(&rig.runs).await;

        assert!(rig.exec.captured().is_empty());
        let stored = rig
            .db
            .list_tasks()
            .unwrap()
            .into_iter()
            .find(|t| t.id == "task-1")
            .unwrap();
        assert_eq!(stored.last_result.as_deref(), Some("失败"));
    }

    #[tokio::test]
    async fn run_now_rejects_stale_task_and_passes_snapshot_when_fresh() {
        let rig = rig("run-now");
        write_script(&rig, "daily.yaml", SCRIPT);
        let (_, signature) =
            task_params::probe_script_signature(&rig.scripts, "com.test.app/daily.yaml").unwrap();
        let snapshot = serde_json::json!({ "enable": false, "message": SECRET_TEXT });

        // 过期：409 语义（ParamStale），不提交
        let stale = mk_task("com.test.app/daily.yaml", "{}", "psig1|old");
        match rig.scheduler.run_now(&stale).await {
            Err(RunNowError::ParamStale(GateError::SignatureMismatch { stored, current })) => {
                assert_eq!(stored, "psig1|old");
                assert!(current.starts_with("psig1|"));
            }
            other => panic!("expected ParamStale, got ok={}", other.is_ok()),
        }
        assert!(rig.exec.captured().is_empty());

        // 新鲜快照：run_now 通过并携带完整 args
        let snapshot_json = snapshot.to_string();
        let fresh = mk_task("com.test.app/daily.yaml", &snapshot_json, &signature);
        let run_id = rig.scheduler.run_now(&fresh).await.unwrap();
        settle(&rig.runs).await;
        let rec = rig.runs.get_run(&run_id).unwrap();
        assert_eq!(rec.state, RunState::Success);
        let captured = rig.exec.captured();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].args.len(), 2, "全量快照覆盖");
        assert_eq!(
            captured[0].args[1],
            (
                "message".to_string(),
                crate::script_v2::TypedValue::Text(SECRET_TEXT.into())
            )
        );
    }

    /// 日志防泄露：text 参数值绝不进入运行链路日志；参数名与签名短码必须在。
    #[tokio::test]
    async fn run_logs_contain_signature_and_names_never_values() {
        use tracing::instrument::WithSubscriber as _;

        struct CapturedLogs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl CapturedLogs {
            fn new() -> Self {
                Self(std::sync::Arc::new(std::sync::Mutex::new(Vec::new())))
            }
            fn text(&self) -> String {
                String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
            }
        }
        impl Clone for CapturedLogs {
            fn clone(&self) -> Self {
                Self(self.0.clone())
            }
        }
        struct CapturedWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for CapturedWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
            type Writer = CapturedWriter;
            fn make_writer(&'a self) -> Self::Writer {
                CapturedWriter(self.0.clone())
            }
        }

        let rig = rig("log-leak");
        write_script(&rig, "daily.yaml", SCRIPT);
        let (_, signature) =
            task_params::probe_script_signature(&rig.scripts, "com.test.app/daily.yaml").unwrap();
        let snapshot = serde_json::json!({ "enable": false, "message": SECRET_TEXT });
        let snapshot_json = snapshot.to_string();
        let task = mk_task("com.test.app/daily.yaml", &snapshot_json, &signature);
        rig.db.upsert_task(&task).unwrap();

        let capture = CapturedLogs::new();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(capture.clone())
            .finish();
        dispatch(&rig.db, &rig.runs, &rig.scripts, &task, None)
            .with_subscriber(subscriber)
            .await;
        settle(&rig.runs).await;

        let logs = capture.text();
        assert!(logs.contains("task params confirmed"), "{logs}");
        assert!(
            logs.contains("enable,message"),
            "参数名列表必须出现在确认日志: {logs}"
        );
        assert!(
            logs.contains("signature_short"),
            "签名短码必须出现在确认日志: {logs}"
        );
        assert!(!logs.contains(SECRET_TEXT), "text 参数值泄露进日志: {logs}");
        // 落库日志同样不得含参数值（调度批量日志经完成钩子入库）
        let db_logs = rig.db.list_logs(None, None, 100).unwrap();
        for log in db_logs {
            assert!(
                !log.msg.contains(SECRET_TEXT),
                "text 参数值泄露进运行日志表: {}",
                log.msg
            );
        }
    }
}
