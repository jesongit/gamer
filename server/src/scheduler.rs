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
                let tasks = match db.list_tasks() {
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
                            info!(task = %task2.name, "scheduled run triggered");
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

    /// 立即运行任务（手动触发）：202 契约 —— 提交即返回 run_id，不等完成
    pub async fn run_now(&self, task: &Task) -> Result<String, RunNowError> {
        info!(task = %task.name, "manual trigger (task now)");
        let content = match self.scripts.get(&task.script_id) {
            Ok(Some(s)) => s.content,
            Ok(None) => {
                warn!(task = %task.name, "task now rejected: script not found");
                record_scheduler_failure(&self.db.metrics());
                return Err(RunNowError::ScriptMissing);
            }
            Err(e) => {
                warn!(task = %task.name, err = %e, "task now rejected: read script failed");
                record_scheduler_failure(&self.db.metrics());
                return Err(RunNowError::Io(e.to_string()));
            }
        };
        let result = submit_run(&self.runs, &self.db, task, None, content)
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
            Err(RunNowError::ScriptMissing | RunNowError::Io(_)) => {
                record_scheduler_failure(&self.db.metrics());
            }
        }
        result
    }
}

/// 立即运行的错误面（API 映射：ScriptMissing/Io→4xx、Start→409/503）
#[derive(Debug)]
pub enum RunNowError {
    ScriptMissing,
    Io(String),
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
        match db.claim_scheduled_run(&task.id, scheduled_at) {
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
    let content = match scripts.get(&task.script_id) {
        Ok(Some(s)) => s.content,
        Ok(None) => {
            warn!(
                task = %task.name,
                script = %task.script_id,
                "scheduled skip: script not found"
            );
            record_scheduler_event(&metrics, SchedulerEvent::Failed);
            finish_scheduled_run(db, task, scheduled_at, "failed", None, Some("脚本不存在"));
            mark_task_result(db, task, "失败", Some("任务执行失败: 脚本不存在"));
            return;
        }
        Err(e) => {
            warn!(task = %task.name, err = %e, "scheduled skip: read script failed");
            record_scheduler_event(&metrics, SchedulerEvent::Failed);
            finish_scheduled_run(db, task, scheduled_at, "failed", None, Some("读取脚本失败"));
            mark_task_result(db, task, "失败", Some("任务执行失败: 读脚本失败"));
            return;
        }
    };
    match submit_run(runs, db, task, trigger, content).await {
        Ok(run_id) => {
            debug!(task = %task.name, %run_id, "scheduled run submitted");
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
            finish_scheduled_run(db, task, scheduled_at, "skipped", None, Some("设备忙"));
            mark_task_result(
                db,
                task,
                "失败",
                Some("任务执行失败: 设备忙（跳过本次触发）"),
            );
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
            );
        }
    }
}

/// 组装 StartRequest 并提交（trigger 有值=Scheduled，无=TaskNow）
async fn submit_run(
    runs: &Arc<RunManager>,
    db: &Db,
    task: &Task,
    trigger: Option<DateTime<Local>>,
    content: String,
) -> Result<String, StartError> {
    let scheduled_at = trigger.map(|t| t.timestamp());
    let hook = task_finish_hook(db.clone(), task.id.clone(), task.name.clone(), scheduled_at);
    let req = StartRequest {
        run_id: String::new(),
        device_id: task.device_id.clone(),
        script_id: task.script_id.clone(),
        content,
        source: if trigger.is_some() {
            RunSource::Scheduled
        } else {
            RunSource::TaskNow
        },
        task_id: Some(task.id.clone()),
        scheduled_at,
        start_index: 0,
        run_func: None,
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
fn mark_task_result(db: &Db, task: &Task, result: &str, summary_log: Option<&str>) {
    if let Some(msg) = summary_log {
        let _ = db.add_log(&task.device_id, &task.script_id, "error", msg);
    }
    upsert_task_result(db, &task.id, task, |t| {
        t.last_result = Some(result.to_string());
        t.last_run_at = Some(now_utc_string());
    });
}

fn now_utc_string() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn upsert_task_result(db: &Db, task_id: &str, fallback: &Task, apply: impl FnOnce(&mut Task)) {
    let mut t = match db.list_tasks() {
        Ok(ts) => ts.into_iter().find(|x| x.id == task_id),
        Err(_) => None,
    }
    .unwrap_or_else(|| fallback.clone());
    apply(&mut t);
    if let Err(e) = db.upsert_task(&t) {
        error!(task = %task_id, err = %e, "update task result failed");
    }
}

/// 任务级完成钩子：终态写回 last_result/last_run_at 并批量落库引擎日志
/// （调度批量模式 realtime_logs=false，引擎日志未实时入库，由这里统一写入）
fn task_finish_hook(
    db: Db,
    task_id: String,
    task_name: String,
    scheduled_at: Option<i64>,
) -> FinishHook {
    Arc::new(move |rec, outcome| {
        let logs = outcome.logs();
        for (level, msg) in logs {
            let _ = db.add_log(&rec.device_id, &rec.script_id, level, msg);
        }
        let has_error =
            matches!(outcome, RunOutcome::Failed(_, _)) || logs.iter().any(|(l, _)| l == "error");
        let label = match outcome {
            RunOutcome::Cancelled(_) => "取消",
            _ if has_error => "失败",
            _ => "成功",
        };
        if let Some(scheduled_at) = scheduled_at {
            let state = match outcome {
                RunOutcome::Cancelled(_) => "skipped",
                _ if has_error => "failed",
                _ => "success",
            };
            let error = match outcome {
                RunOutcome::Failed(msg, _) => Some(msg.as_str()),
                RunOutcome::Cancelled(_) => Some("运行被取消"),
                RunOutcome::Success(_) => None,
            };
            let _ =
                db.finish_scheduled_run(&task_id, scheduled_at, state, Some(&rec.run_id), error);
        }
        let rid = rec.run_id.clone();
        upsert_task_result(
            &db,
            &task_id,
            &fallback_task(&task_id, &task_name, rec),
            |t| {
                t.last_result = Some(label.to_string());
                t.last_run_at = Some(now_utc_string());
            },
        );
        debug!(%task_id, %rid, result = label, "task finish hook applied");
    })
}

fn fallback_task(task_id: &str, task_name: &str, rec: &crate::run_manager::RunRecord) -> Task {
    Task {
        id: task_id.to_string(),
        name: task_name.to_string(),
        cron: String::new(),
        script_id: rec.script_id.clone(),
        device_id: rec.device_id.clone(),
        enabled: true,
        last_result: None,
        last_run_at: None,
        created_at: String::new(),
    }
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

fn finish_scheduled_run(
    db: &Db,
    task: &Task,
    scheduled_at: Option<i64>,
    state: &str,
    run_id: Option<&str>,
    error: Option<&str>,
) {
    if let Some(scheduled_at) = scheduled_at {
        if let Err(e) = db.finish_scheduled_run(&task.id, scheduled_at, state, run_id, error) {
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
        let changed = match db.finish_scheduled_run(
            &task.id,
            scheduled_at,
            state,
            Some(&run_id),
            rec.error.as_deref(),
        ) {
            Ok(changed) => changed,
            Err(e) => {
                error!(task = %task.name, %run_id, err = %e, "scheduled completion fallback failed");
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
            upsert_task_result(&db, &task.id, &task, |t| {
                t.last_result = Some(label.to_string());
                t.last_run_at = Some(now_utc_string());
            });
        }
        return;
    }
}

fn record_scheduler_trigger(metrics: &Metrics, trigger_started: std::time::Instant) {
    metrics.record_scheduler_trigger(trigger_started.elapsed().as_millis() as u64);
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

    #[test]
    fn utc_last_run_format_is_parseable_and_ends_in_z() {
        let value = now_utc_string();
        assert!(value.ends_with('Z'));
        assert!(chrono::DateTime::parse_from_rfc3339(&value).is_ok());
    }

    #[test]
    fn scheduler_metrics_helpers_update_low_cardinality_counters() {
        let trigger_metrics = Metrics::default();
        record_scheduler_trigger(&trigger_metrics, std::time::Instant::now());
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
