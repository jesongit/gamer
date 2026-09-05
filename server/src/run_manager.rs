//! 统一运行管理（OPTIMIZATION_PLAN 阶段 3 RUN-001）：
//! 手动脚本 / 定时任务 / 立即运行共用一张以 `run_id`（UUID v4）为主键的注册表，
//! 设备级互斥（一设备至多一个活动 run，冲突直接 409 不排队）。
//!
//! 状态机：
//!
//! ```text
//! submit ──► starting ──prepare 失败──► failed
//!               │ cancel（starting 阶段短路，不进执行器）
//!               │                        ▲
//!               ▼                        │ 执行报错/异常终止
//!           running ◄──（执行器就绪）────┘
//!            │  │  └──正常完成──► success
//!            │  └──────────────► failed
//!            └─cancel──► stopping ──► cancelled
//! ```
//!
//! 退出路径保障（RAII）：run 任务体内持两把 guard——
//! - [`ActivityLease`]：prepare 成功后由执行器取得，drop 时释放具体活动资源；
//!   panic 展开同样触发。
//! - [`FinishGuard`]：整个任务体作用域，drop 时从注册表摘除 + 写终态档案；
//!   正常路径已显式置终态则按显式值归档，残留非终态（panic/忘记置位）判 failed，
//!   取消请求在册判 cancelled。tokio spawn 任务 panic 时 future 被 drop，
//!   guard 的 Drop 在展开路径上必然执行（无需 catch_unwind）。
//!
//! 停止语义复用引擎现有停止通道（`stop: Arc<AtomicBool>`，Runner::run 消费），
//! 按 run_id 精确定位，不依赖脚本名称猜测路径。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use futures_util::future::BoxFuture;
use serde::Serialize;
use tracing::{debug, info, warn};

use crate::core::{ActivityLease, RunContext, RunId, RunRequest};

/// 运行来源
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunSource {
    Manual,
    Scheduled,
    TaskNow,
}

/// 运行状态（对外 JSON 字符串即变体小写下划线形式）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Starting,
    Running,
    Stopping,
    Success,
    Failed,
    Cancelled,
}

impl RunState {
    fn is_terminal(self) -> bool {
        matches!(
            self,
            RunState::Success | RunState::Failed | RunState::Cancelled
        )
    }
    /// 运行中的状态集合：任何非终态都仍占用设备运行槽。
    pub fn is_active(self) -> bool {
        !self.is_terminal()
    }
}

/// 一次执行的完整记录（HTTP 序列化形状与前端契约逐字段一致）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunRecord {
    pub run_id: String,
    pub device_id: String,
    /// Generic runner identity.  `script_id` below is retained as the legacy
    /// display field for the existing HTTP contract.
    pub runner_id: String,
    pub entrypoint: String,
    /// Compatibility display label; this is not interpreted by RunManager.
    pub script_id: String,
    pub source: RunSource,
    /// 关联定时任务 id（manual 为 null）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// 计划触发时刻（unix 秒 UTC；manual/task_now 为 null）
    #[serde(serialize_with = "ser_opt_ts", skip_serializing_if = "Option::is_none")]
    pub scheduled_at: Option<i64>,
    pub state: RunState,
    /// 开始时刻（ISO8601 UTC）
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn ser_opt_ts<S: serde::Serializer>(v: &Option<i64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(ts) => {
            let dt = chrono::DateTime::<Utc>::from_timestamp(*ts, 0).unwrap_or_else(Utc::now);
            s.serialize_some(&dt)
        }
        None => s.serialize_none(),
    }
}

impl RunRecord {
    /// 409 冲突响应摘要（与前端 device_busy 弹窗消费的字段钉死一致）
    pub fn busy_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "error": "device_busy",
            "run_id": self.run_id,
            "script_id": self.script_id,
            "runner_id": self.runner_id,
            "entrypoint": self.entrypoint,
            "source": self.source,
            "started_at": self.started_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        })
    }
}

/// 提交给执行器的一次运行请求
#[derive(Debug, Clone)]
pub struct StartRequest {
    /// Runner-agnostic request.  Runner-specific target decoding belongs to
    /// the selected executor adapter, never to RunManager.
    pub request: RunRequest,
    pub source: RunSource,
    /// 关联定时任务 id（manual 为 null）
    pub task_id: Option<String>,
    /// 计划触发点 unix 秒 UTC（scheduled 来源携带）
    pub scheduled_at: Option<i64>,
    /// 实时逐条落库日志（Console 是；调度批量为 false，由完成钩子批量写）
    pub realtime_logs: bool,
}

impl StartRequest {
    pub fn device_id(&self) -> &str {
        self.request.device_id.as_str()
    }

    pub fn display_label(&self) -> &str {
        self.request.entrypoint.as_str()
    }
}

/// 运行终态结果
#[derive(Debug, Clone)]
pub enum RunOutcome {
    Success(Vec<(String, String)>),
    Failed(String, Vec<(String, String)>),
    Cancelled(Vec<(String, String)>),
}

impl RunOutcome {
    pub fn logs(&self) -> &[(String, String)] {
        match self {
            RunOutcome::Success(l) | RunOutcome::Failed(_, l) | RunOutcome::Cancelled(l) => l,
        }
    }
}

/// 完成钩子：终态落定后同步回调（调度器借此更新任务 last_result/last_run_at）
pub type FinishHook = Arc<dyn Fn(&RunRecord, &RunOutcome) + Send + Sync>;

/// 执行器接缝：RunManager 只传递通用请求和运行上下文；具体 runner 通过
/// adapter 解释 payload，并返回一个自有的 activity lease。
pub trait RunExecutor: Send + Sync + 'static {
    /// 启动前准备（例如连接采集会话）。失败 → run 直接 failed。
    fn prepare<'a>(
        &'a self,
        context: &'a RunContext,
        request: &'a RunRequest,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    /// 执行主体。取消经 stop 标志传递给具体 runner。
    fn execute<'a>(
        &'a self,
        context: &'a RunContext,
        request: &'a RunRequest,
        realtime_logs: bool,
        stop: Arc<AtomicBool>,
    ) -> BoxFuture<'a, anyhow::Result<Vec<(String, String)>>>;
    /// Acquire a generic lease.  RunManager owns the returned guard until the
    /// execution task ends, so panic/cancel paths release activity as well.
    fn acquire(&self, context: &RunContext) -> anyhow::Result<Box<dyn ActivityLease>>;
}

/// 提交失败
#[derive(Debug, Clone)]
pub enum StartError {
    /// 设备已有活动运行（携带对方记录，映射 HTTP 409 device_busy）
    Conflict(Box<RunRecord>),
    /// 服务停机 drain 中拒绝新任务（映射 HTTP 503）
    ShuttingDown,
}

/// 取消请求结果（API 据此映射：Active→202 / NotFound→404 / 终态→409）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelOutcome {
    Accepted,
    NotFound,
    AlreadyFinished(RunState),
}

struct ActiveRun {
    record: RunRecord,
    /// 引擎停止标志（engine Ctx.stop 同源通道）
    stop: Arc<AtomicBool>,
    cancel_requested: bool,
}

const HISTORY_CAP: usize = 256;

pub struct RunManager {
    executor: Arc<dyn RunExecutor>,
    /// 设备级互斥：device_id → 活动 run_id
    active_by_device: Mutex<HashMap<String, String>>,
    /// run_id → 活动条目（终态后摘入 history）
    runs: Mutex<HashMap<String, ActiveRun>>,
    /// 终态档案（FIFO 限长，供 GET /api/runs/:id 终态查询与取消确认）
    history: Mutex<VecDeque<RunRecord>>,
    /// drain 中：拒绝一切新提交
    draining: AtomicBool,
    /// 进行中的 spawn 任务数（测试 settle 判定用；生产无读者）
    inflight: AtomicUsize,
    /// 状态/归档变化通知；测试等待状态时使用，避免依赖固定 sleep。
    state_changed: tokio::sync::Notify,
}

impl RunManager {
    pub fn new(executor: Arc<dyn RunExecutor>) -> Self {
        Self {
            executor,
            active_by_device: Mutex::new(HashMap::new()),
            runs: Mutex::new(HashMap::new()),
            history: Mutex::new(VecDeque::new()),
            draining: AtomicBool::new(false),
            inflight: AtomicUsize::new(0),
            state_changed: tokio::sync::Notify::new(),
        }
    }

    // ---------- 查询 ----------

    pub fn get_run(&self, run_id: &str) -> Option<RunRecord> {
        if let Some(e) = self.runs.lock().unwrap().get(run_id) {
            return Some(e.record.clone());
        }
        self.history
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.run_id == run_id)
            .cloned()
    }

    /// Wait for a run to reach a terminal state without polling the registry.
    /// The notification is registered before each snapshot, so a completion
    /// racing with the snapshot cannot be missed.  Terminal records remain in
    /// the bounded history and are returned just like active records.
    #[allow(
        dead_code,
        reason = "completion consumers adopt this event boundary incrementally"
    )]
    pub async fn wait_terminal(&self, run_id: &str) -> Option<RunRecord> {
        loop {
            let notified = self.state_changed.notified();
            match self.get_run(run_id) {
                Some(record) if record.state.is_terminal() => return Some(record),
                Some(_) => {}
                None => return None,
            }
            notified.await;
        }
    }

    /// 设备当前活动运行（无则 None；前端刷新恢复用）。
    pub fn active_for_device(&self, device_id: &str) -> Option<RunRecord> {
        let rid = self
            .active_by_device
            .lock()
            .unwrap()
            .get(device_id)
            .cloned()?;
        self.runs
            .lock()
            .unwrap()
            .get(&rid)
            .map(|e| e.record.clone())
    }

    pub fn active_count(&self) -> usize {
        self.runs.lock().unwrap().len()
    }

    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    // ---------- 生命周期 ----------

    /// 提交一次运行：先抢设备互斥槽（409 语义在此产生），随后 spawn 执行任务立即返回。
    pub fn submit(
        self: &Arc<Self>,
        req: StartRequest,
        on_finish: Option<FinishHook>,
    ) -> Result<RunRecord, StartError> {
        if self.draining.load(Ordering::SeqCst) {
            return Err(StartError::ShuttingDown);
        }
        // 锁序恒定：active_by_device 先于 runs（finalize 只做两次独立短锁，无嵌套）
        let mut dev = self.active_by_device.lock().unwrap();
        if let Some(existing_rid) = dev.get(req.device_id()) {
            let cur = self
                .runs
                .lock()
                .unwrap()
                .get(existing_rid)
                .map(|e| e.record.clone());
            return match cur {
                Some(rec) => Err(StartError::Conflict(Box::new(rec))),
                None => {
                    // 幽灵条目（活动表指向已消失记录——理论不可达，防御性清理后放行）
                    warn!(device = %req.device_id(), "stale device slot without run entry, reclaimed");
                    dev.remove(req.device_id());
                    self.submit_inner(req, on_finish, &mut dev)
                }
            };
        }
        self.submit_inner(req, on_finish, &mut dev)
    }

    fn submit_inner(
        self: &Arc<Self>,
        req: StartRequest,
        on_finish: Option<FinishHook>,
        dev: &mut HashMap<String, String>,
    ) -> Result<RunRecord, StartError> {
        let run_id = uuid::Uuid::new_v4().to_string();
        let record = RunRecord {
            run_id: run_id.clone(),
            device_id: req.device_id().to_string(),
            runner_id: req.request.runner_id.clone(),
            entrypoint: req.request.entrypoint.clone(),
            script_id: req.display_label().to_string(),
            source: req.source,
            task_id: req.task_id.clone(),
            scheduled_at: req.scheduled_at,
            state: RunState::Starting,
            started_at: Utc::now(),
            finished_at: None,
            error: None,
        };
        info!(
            run_id = %run_id,
            device = %record.device_id,
            runner = %record.runner_id,
            entrypoint = %record.entrypoint,
            source = ?record.source,
            task_id = record.task_id.as_deref().unwrap_or("-"),
            "run accepted"
        );
        self.runs.lock().unwrap().insert(
            run_id.clone(),
            ActiveRun {
                record: record.clone(),
                stop: Arc::new(AtomicBool::new(false)),
                cancel_requested: false,
            },
        );
        dev.insert(req.device_id().to_string(), run_id.clone());
        self.state_changed.notify_waiters();

        let mgr = self.clone();
        tokio::spawn(async move {
            mgr.run_task(Arc::new(req), run_id, on_finish).await;
        });
        Ok(record)
    }

    /// run 任务体：guard 组合下所有退出路径收敛到 finalize
    async fn run_task(
        self: Arc<Self>,
        req: Arc<StartRequest>,
        run_id: String,
        on_finish: Option<FinishHook>,
    ) {
        debug!(run_id = %run_id, "run task started");
        // inflight 计数同样走 RAII（panic 展开不会漏减，wait_settled 才不假死）
        let _inflight = CountGuard::new(&self.inflight);
        let mut finish = FinishGuard {
            mgr: self.clone(),
            run_id: run_id.clone(),
            on_finish,
        };
        let context = RunContext::new(
            RunId::new(run_id.clone()).expect("run ids are generated UUIDs"),
            req.request.app.clone(),
        );
        // starting 阶段取消短路：不入执行器，但仍需调用完成钩子，
        // 否则 scheduled_runs 会一直停在 running。
        if self.is_cancelled(&run_id) {
            finish.complete(&run_id, RunOutcome::Cancelled(vec![]));
            drop(finish);
            return;
        }
        let prepare = self.executor.prepare(&context, &req.request).await;
        if let Err(e) = prepare {
            warn!(run_id = %run_id, err = %format!("{e:#}"), "run prepare (connect) failed");
            if self.is_cancelled(&run_id) {
                finish.complete(&run_id, RunOutcome::Cancelled(vec![]));
            } else {
                finish.complete(
                    &run_id,
                    RunOutcome::Failed(format!("连接失败: {e:#}"), vec![]),
                );
            }
            drop(finish);
            return;
        }
        // 活动租约：RAII 配对 release，
        // panic 展开时 Drop 必然归还
        let _lease = match self.executor.acquire(&context) {
            Ok(lease) => lease,
            Err(e) => {
                finish.complete(
                    &run_id,
                    RunOutcome::Failed(format!("设备活动租约获取失败: {e:#}"), vec![]),
                );
                return;
            }
        };
        self.mark_state(&run_id, RunState::Running, None);

        // starting→running 竞态取消：置位后再补一次检查
        if self.is_cancelled(&run_id) {
            finish.complete(&run_id, RunOutcome::Cancelled(vec![]));
            return; // _lease → finish → _inflight 逆序 drop
        }

        let outcome = self.execute(req, context, run_id.clone()).await;
        finish.complete(&run_id, outcome);
        // finish / lease / inflight 依声明逆序自动释放
    }

    /// 执行主体（独立函数便于阅读）：入口已过取消闸，出口把结果归类
    async fn execute(
        self: &Arc<Self>,
        req: Arc<StartRequest>,
        context: RunContext,
        run_id: String,
    ) -> RunOutcome {
        let entry_stop = self
            .runs
            .lock()
            .unwrap()
            .get(&run_id)
            .map(|e| e.stop.clone());
        let Some(stop) = entry_stop else {
            return RunOutcome::Failed("注册表条目丢失".into(), vec![]);
        };
        let was_cancelled = || self.cancel_requested(&run_id);
        match self
            .executor
            .execute(&context, &req.request, req.realtime_logs, stop.clone())
            .await
        {
            Ok(logs) => {
                if was_cancelled() {
                    RunOutcome::Cancelled(logs)
                } else {
                    RunOutcome::Success(logs)
                }
            }
            Err(e) => {
                if was_cancelled() {
                    RunOutcome::Cancelled(vec![])
                } else {
                    RunOutcome::Failed(format!("{e:#}"), vec![])
                }
            }
        }
    }

    /// 停止一次运行：定位到 ctx 停止通道，state → stopping，终态经查询确认。
    pub fn cancel(&self, run_id: &str) -> CancelOutcome {
        let mut runs = self.runs.lock().unwrap();
        let active_outcome = match runs.get_mut(run_id) {
            None => None,
            Some(entry) if entry.record.state.is_terminal() => {
                Some(CancelOutcome::AlreadyFinished(entry.record.state))
            }
            Some(entry) => {
                info!(run_id = %run_id, "cancel requested");
                entry.cancel_requested = true;
                entry.record.state = RunState::Stopping;
                entry.stop.store(true, Ordering::SeqCst);
                Some(CancelOutcome::Accepted)
            }
        };
        let outcome = match active_outcome {
            Some(outcome) => outcome,
            None => {
                drop(runs);
                self.history
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|record| record.run_id == run_id)
                    .map(|record| CancelOutcome::AlreadyFinished(record.state))
                    .unwrap_or(CancelOutcome::NotFound)
            }
        };
        self.state_changed.notify_waiters();
        outcome
    }

    // ---------- 内部状态操作（全部短锁，不跨 await） ----------

    fn with_entry<R>(&self, run_id: &str, f: impl FnOnce(&mut ActiveRun) -> R) -> Option<R> {
        let mut runs = self.runs.lock().unwrap();
        runs.get_mut(run_id).map(f)
    }

    fn is_cancelled(&self, run_id: &str) -> bool {
        self.with_entry(run_id, |e| e.cancel_requested)
            .unwrap_or(false)
    }

    fn cancel_requested(&self, run_id: &str) -> bool {
        self.is_cancelled(run_id)
    }

    fn mark_state(&self, run_id: &str, state: RunState, error: Option<String>) {
        self.with_entry(run_id, |e| {
            e.record.state = state;
            e.record.error = error;
            if state.is_terminal() {
                e.record.finished_at = Some(Utc::now());
            }
        });
        self.state_changed.notify_waiters();
    }

    fn mark_terminal_checked(&self, run_id: &str, class: RunOutcomeClass, error: Option<String>) {
        let mut runs = self.runs.lock().unwrap();
        let Some(entry) = runs.get_mut(run_id) else {
            return;
        };
        // 显式终态不回退；这里只允许 running/stopping 收敛到终态
        if entry.record.state.is_terminal() {
            return;
        }
        let state = match class {
            RunOutcomeClass::Success => RunState::Success,
            RunOutcomeClass::Failed => RunState::Failed,
            RunOutcomeClass::Cancelled => RunState::Cancelled,
        };
        entry.record.state = state;
        entry.record.error = error;
        entry.record.finished_at = Some(Utc::now());
        self.state_changed.notify_waiters();
    }

    fn snapshot_or_placeholder(&self, run_id: &str) -> RunRecord {
        self.get_run(run_id).unwrap_or_else(|| RunRecord {
            run_id: run_id.to_string(),
            device_id: String::new(),
            runner_id: String::new(),
            entrypoint: String::new(),
            script_id: String::new(),
            source: RunSource::Manual,
            task_id: None,
            scheduled_at: None,
            state: RunState::Failed,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            error: Some("record vanished".into()),
        })
    }

    /// FinishGuard Drop 出口：非终态兜底分类 + 注册表摘除 + 档案归档。
    /// 兜底规则：cancelled 在册 → cancelled；其余残留（panic/ forgotten set）→ failed。
    fn finalize(&self, run_id: &str) -> Option<RunRecord> {
        let rec = {
            let mut runs = self.runs.lock().unwrap();
            let entry = runs.get_mut(run_id)?;
            if !entry.record.state.is_terminal() {
                let cancelled = entry.cancel_requested;
                entry.record.state = if cancelled {
                    RunState::Cancelled
                } else {
                    RunState::Failed
                };
                if entry.record.error.is_none() {
                    entry.record.error = if cancelled {
                        Some("已停止".into())
                    } else {
                        Some("执行异常终止（panic 或未正常收尾）".into())
                    };
                }
                entry.record.finished_at = Some(Utc::now());
            }
            runs.remove(run_id).unwrap().record
        };
        // 摘除设备槽（仅当仍归属本 run：期间新 run 可能已占位？不可能——
        // 本 run 尚未摘除前新 run 会撞 409；所以匹配即删，防御性再验一次）
        {
            let mut dev = self.active_by_device.lock().unwrap();
            if dev.get(&rec.device_id).map(|s| s.as_str()) == Some(run_id) {
                dev.remove(&rec.device_id);
            }
        }
        let mut hist = self.history.lock().unwrap();
        if hist.len() >= HISTORY_CAP {
            hist.pop_front();
        }
        info!(
            run_id = %rec.run_id,
            device = %rec.device_id,
            script = %rec.script_id,
            task_id = rec.task_id.as_deref().unwrap_or("-"),
            state = ?rec.state,
            elapsed_ms = (Utc::now() - rec.started_at).num_milliseconds(),
            "run finished"
        );
        let finished = rec.clone();
        hist.push_back(rec);
        self.state_changed.notify_waiters();
        Some(finished)
    }

    /// 停机 drain：先关闸（新提交一律 ShuttingDown），等待活动运行自然结束，
    /// 超时仍未结束的强制置停止标志并标记取消。
    pub async fn begin_shutdown(self: &Arc<Self>, wait_timeout: std::time::Duration) {
        let _ = self.is_draining();
        self.draining.store(true, Ordering::SeqCst);
        let deadline = tokio::time::Instant::now() + wait_timeout;
        loop {
            if self.active_count() == 0 {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                let ids: Vec<(String, String)> = {
                    let mut runs = self.runs.lock().unwrap();
                    runs.values_mut()
                        .filter(|e| e.record.state.is_active())
                        .map(|e| (e.record.run_id.clone(), e.record.device_id.clone()))
                        .collect()
                };
                for (rid, _) in &ids {
                    self.cancel(rid);
                }
                warn!(
                    forced = ids.len(),
                    "shutdown timeout: force-cancelling active runs"
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        // 给强停任务一点收尾窗口（guard drop/日志冲刷），不等全量完成
        let settle = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
        while tokio::time::Instant::now() < settle && self.inflight.load(Ordering::SeqCst) > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    // ---------- 测试辅助 ----------

    #[cfg(test)]
    async fn wait_settled(&self, timeout: std::time::Duration) {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if self.active_count() == 0 && self.inflight.load(Ordering::SeqCst) == 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("runs did not settle within {:?}", timeout);
    }

    #[cfg(test)]
    async fn wait_for_state(
        &self,
        run_id: &str,
        predicate: impl Fn(&RunRecord) -> bool,
        timeout: std::time::Duration,
    ) -> RunRecord {
        let wait = async {
            loop {
                let notified = self.state_changed.notified();
                if let Some(rec) = self.get_run(run_id) {
                    if predicate(&rec) {
                        return rec;
                    }
                }
                notified.await;
            }
        };
        tokio::time::timeout(timeout, wait)
            .await
            .unwrap_or_else(|_| panic!("run {run_id} did not reach expected state"))
    }
}

enum RunOutcomeClass {
    Success,
    Failed,
    Cancelled,
}

/// inflight 计数 RAII guard（panic 展开同样归还）
struct CountGuard<'a>(&'a AtomicUsize);

impl<'a> CountGuard<'a> {
    fn new(c: &'a AtomicUsize) -> Self {
        c.fetch_add(1, Ordering::SeqCst);
        CountGuard(c)
    }
}

impl Drop for CountGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 注册表终态 guard：Drop 保证任何退出路径（含 panic 展开）都收敛入档并释放设备槽
struct FinishGuard {
    mgr: Arc<RunManager>,
    run_id: String,
    on_finish: Option<FinishHook>,
}

impl FinishGuard {
    fn complete(&mut self, run_id: &str, outcome: RunOutcome) {
        let error = match &outcome {
            RunOutcome::Failed(msg, _) => Some(msg.clone()),
            RunOutcome::Success(_) | RunOutcome::Cancelled(_) => None,
        };
        let class = match &outcome {
            RunOutcome::Success(_) => RunOutcomeClass::Success,
            RunOutcome::Failed(_, _) => RunOutcomeClass::Failed,
            RunOutcome::Cancelled(_) => RunOutcomeClass::Cancelled,
        };
        self.mgr.mark_terminal_checked(run_id, class, error);
        if let Some(hook) = self.on_finish.take() {
            hook(&self.mgr.snapshot_or_placeholder(run_id), &outcome);
        }
    }
}

impl Drop for FinishGuard {
    fn drop(&mut self) {
        let Some(rec) = self.mgr.finalize(&self.run_id) else {
            return;
        };
        // execute/prepare panic 或其它异常展开时没有机会走 complete；仍调用一次
        // 完成钩子，使 scheduled_runs 和任务结果不会永久停在 running。
        if let Some(hook) = self.on_finish.take() {
            let outcome = match rec.state {
                RunState::Success => RunOutcome::Success(vec![]),
                RunState::Cancelled => RunOutcome::Cancelled(vec![]),
                RunState::Failed => RunOutcome::Failed(
                    rec.error
                        .clone()
                        .unwrap_or_else(|| "执行异常终止（panic 或未正常收尾）".into()),
                    vec![],
                ),
                RunState::Starting | RunState::Running | RunState::Stopping => {
                    RunOutcome::Failed("执行异常终止（panic 或未正常收尾）".into(), vec![])
                }
            };
            hook(&rec, &outcome);
        }
    }
}

// ---------------------------------------------------------------------------
// 测试：仲裁层全场景走假执行器（连接/执行可控、可观测）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as PlMutex;
    use std::sync::atomic::AtomicUsize;

    /// 可编程假执行器：prepare 成败 / execute 行为 / 计数与并发观测
    struct FakeExecutor {
        prepare_ok: bool,
        hang_until_cancel: bool,
        step_error: Option<String>,
        prepare_gate: Option<Arc<tokio::sync::Notify>>,
        prepare_started: Arc<tokio::sync::Notify>,
        state: Arc<PlMutex<FakeState>>,
    }

    struct FakeState {
        prepare_calls: usize,
        occupies: usize,
        releases: usize,
        executes: usize,
        /// 并发进入 execute 的峰值
        cur_in_exec: usize,
        max_concurrent: usize,
        last_stops: Vec<Arc<AtomicBool>>,
    }

    struct FakeLease(Arc<PlMutex<FakeState>>);

    impl ActivityLease for FakeLease {}

    impl Drop for FakeLease {
        fn drop(&mut self) {
            self.0.lock().releases += 1;
        }
    }

    impl Default for FakeExecutor {
        fn default() -> Self {
            Self {
                prepare_ok: true,
                hang_until_cancel: false,
                step_error: None,
                prepare_gate: None,
                prepare_started: Arc::new(tokio::sync::Notify::new()),
                state: Arc::new(PlMutex::new(FakeState {
                    prepare_calls: 0,
                    occupies: 0,
                    releases: 0,
                    executes: 0,
                    cur_in_exec: 0,
                    max_concurrent: 0,
                    last_stops: vec![],
                })),
            }
        }
    }

    impl FakeExecutor {
        fn hanging() -> Self {
            Self {
                hang_until_cancel: true,
                ..Default::default()
            }
        }
        fn connect_fail() -> Self {
            Self {
                prepare_ok: false,
                ..Default::default()
            }
        }

        fn starting() -> (Self, Arc<tokio::sync::Notify>) {
            let gate = Arc::new(tokio::sync::Notify::new());
            let executor = Self {
                prepare_gate: Some(gate.clone()),
                ..Default::default()
            };
            (executor, gate)
        }
        fn stats<T>(&self, f: impl FnOnce(&FakeState) -> T) -> T {
            f(&self.state.lock())
        }
    }

    impl RunExecutor for FakeExecutor {
        fn prepare<'a>(
            &'a self,
            _context: &'a RunContext,
            _request: &'a RunRequest,
        ) -> BoxFuture<'a, anyhow::Result<()>> {
            let gate = self.prepare_gate.clone();
            let started = self.prepare_started.clone();
            Box::pin(async move {
                started.notify_one();
                if let Some(gate) = gate {
                    gate.notified().await;
                }
                self.state.lock().prepare_calls += 1;
                if self.prepare_ok {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("device offline (fake)"))
                }
            })
        }

        fn execute<'a>(
            &'a self,
            _context: &'a RunContext,
            _request: &'a RunRequest,
            _realtime_logs: bool,
            stop: Arc<AtomicBool>,
        ) -> BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
            Box::pin(async move {
                {
                    let mut st = self.state.lock();
                    st.executes += 1;
                    st.cur_in_exec += 1;
                    st.max_concurrent = st.max_concurrent.max(st.cur_in_exec);
                    st.last_stops.push(stop.clone());
                }
                // 并发计数 RAII：任何 return / panic 路径都归还
                struct Dec<'a>(&'a FakeExecutor);
                impl Drop for Dec<'_> {
                    fn drop(&mut self) {
                        self.0.state.lock().cur_in_exec -= 1;
                    }
                }
                let _dec = Dec(self);
                if let Some(msg) = self.step_error.clone() {
                    return Err(anyhow::anyhow!(msg));
                }
                if self.hang_until_cancel {
                    // 假挂起：等停止标志（真实引擎是轮询退出，语义一致）
                    while !stop.load(Ordering::SeqCst) {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                }
                Ok(vec![("info".into(), "fake done".into())])
            })
        }

        fn acquire(&self, _context: &RunContext) -> anyhow::Result<Box<dyn ActivityLease>> {
            self.state.lock().occupies += 1;
            Ok(Box::new(FakeLease(self.state.clone())))
        }
    }

    fn req(device_id: &str, script_id: &str, source: RunSource) -> StartRequest {
        let package = script_id.split('/').next().unwrap_or("legacy");
        let app = crate::core::AppContext::for_test(device_id, package).unwrap();
        let request = RunRequest::for_app(
            app,
            "test.runner",
            script_id,
            crate::core::RunPayload::empty(),
        )
        .unwrap();
        StartRequest {
            request,
            source,
            task_id: None,
            scheduled_at: None,
            realtime_logs: false,
        }
    }

    async fn settled(mgr: &Arc<RunManager>) {
        mgr.wait_settled(std::time::Duration::from_secs(5)).await;
    }

    // 九项必测之一：同设备两个不同脚本，第二个 409
    #[tokio::test]
    async fn second_start_on_same_device_conflicts_with_current_record() {
        let fake = Arc::new(FakeExecutor::hanging());
        let mgr = Arc::new(RunManager::new(fake.clone()));
        let a = mgr
            .submit(req("d1", "pkg/a.yaml", RunSource::Manual), None)
            .unwrap();
        let b = mgr.submit(req("d1", "pkg/b.yaml", RunSource::Scheduled), None);
        match b {
            Err(StartError::Conflict(busy)) => {
                assert_eq!(busy.run_id, a.run_id);
                assert_eq!(busy.script_id, "pkg/a.yaml");
                assert_eq!(busy.source, RunSource::Manual);
                let payload = busy.busy_payload();
                assert_eq!(payload["error"], "device_busy");
                assert!(payload["started_at"].as_str().unwrap().contains('T'));
            }
            other => panic!("expected conflict, got {:?}", other.map(|r| r.run_id)),
        }
        // 另一台设备不受影响
        let other = mgr
            .submit(req("d2", "pkg/a.yaml", RunSource::Manual), None)
            .unwrap();
        assert_eq!(mgr.cancel(&a.run_id), CancelOutcome::Accepted);
        assert_eq!(mgr.cancel(&other.run_id), CancelOutcome::Accepted);
        settled(&mgr).await;
    }
    // 两台设备可并行：并发峰值 ≥ 2
    #[tokio::test]
    async fn two_devices_execute_in_parallel() {
        /// 两任务在 barrier 相会 ⇒ 进入区并发计数必为 2（确定性重叠证明）
        #[derive(Default)]
        struct ParExec {
            max: AtomicUsize,
            cur: AtomicUsize,
            barrier: std::sync::OnceLock<Arc<tokio::sync::Barrier>>,
        }
        impl RunExecutor for ParExec {
            fn prepare<'a>(
                &'a self,
                _: &'a RunContext,
                _: &'a RunRequest,
            ) -> BoxFuture<'a, anyhow::Result<()>> {
                Box::pin(async { Ok(()) })
            }
            fn execute<'a>(
                &'a self,
                _: &'a RunContext,
                _: &'a RunRequest,
                _realtime_logs: bool,
                _stop: Arc<AtomicBool>,
            ) -> BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
                Box::pin(async move {
                    let cur = self.cur.fetch_add(1, Ordering::SeqCst) + 1;
                    self.max.fetch_max(cur, Ordering::SeqCst);
                    let bar = self
                        .barrier
                        .get_or_init(|| Arc::new(tokio::sync::Barrier::new(2)))
                        .clone();
                    let _ = bar.wait().await;
                    self.cur.fetch_sub(1, Ordering::SeqCst);
                    Ok(vec![])
                })
            }
            fn acquire(&self, _: &RunContext) -> anyhow::Result<Box<dyn ActivityLease>> {
                Ok(Box::new(crate::core::NoopLease))
            }
        }
        let par = Arc::new(ParExec::default());
        let mgr = Arc::new(RunManager::new(par.clone()));
        let r1 = mgr
            .submit(req("dA", "p/s.yaml", RunSource::Manual), None)
            .unwrap();
        let r2 = mgr
            .submit(req("dB", "p/s.yaml", RunSource::Manual), None)
            .unwrap();
        settled(&mgr).await;
        assert!(par.max.load(Ordering::SeqCst) >= 2, "must overlap");
        assert_eq!(mgr.get_run(&r1.run_id).unwrap().state, RunState::Success);
        assert_eq!(mgr.get_run(&r2.run_id).unwrap().state, RunState::Success);
    }

    // 连接失败锁释放：不占用计数、终态 failed、设备槽清空
    #[tokio::test]
    async fn prepare_failure_releases_device_slot_and_marks_failed() {
        let fake = Arc::new(FakeExecutor::connect_fail());
        let mgr = Arc::new(RunManager::new(fake.clone()));
        let r = mgr
            .submit(req("dX", "p/s.yaml", RunSource::TaskNow), None)
            .unwrap();
        assert_eq!(
            mgr.active_for_device("dX").map(|x| x.run_id),
            Some(r.run_id.clone())
        );
        settled(&mgr).await;
        let rec = mgr.get_run(&r.run_id).unwrap();
        assert_eq!(rec.state, RunState::Failed);
        assert!(rec.error.unwrap().contains("连接失败"));
        assert_eq!(mgr.active_for_device("dX"), None, "slot freed");
        assert_eq!(fake.stats(|s| s.occupies), 0, "never occupied");
        assert_eq!(fake.stats(|s| s.releases), 0);
        assert_eq!(fake.stats(|s| s.executes), 0, "execute not reached");
    }

    // cancel 终态归零：状态 stopping→cancelled，注册表清空，release 配对
    #[tokio::test]
    async fn cancel_reaches_cancelled_and_frees_registry() {
        let fake = Arc::new(FakeExecutor::hanging());
        let mgr = Arc::new(RunManager::new(fake.clone()));
        let r = mgr
            .submit(req("d1", "p/s.yaml", RunSource::Manual), None)
            .unwrap();
        mgr.wait_for_state(
            &r.run_id,
            |rec| rec.state == RunState::Running,
            std::time::Duration::from_secs(5),
        )
        .await;
        assert_eq!(mgr.cancel(&r.run_id), CancelOutcome::Accepted);
        assert_eq!(
            mgr.cancel(&r.run_id),
            CancelOutcome::Accepted,
            "repeating cancellation while stopping is idempotent"
        );
        assert_eq!(
            mgr.get_run(&r.run_id).unwrap().state,
            RunState::Stopping,
            "cancel flips to stopping immediately"
        );
        settled(&mgr).await;
        assert_eq!(mgr.get_run(&r.run_id).unwrap().state, RunState::Cancelled);
        assert_eq!(mgr.active_count(), 0, "run count 归零");
        assert_eq!(fake.stats(|s| s.releases), 1, "occupy/release 严格配对");
        assert_eq!(fake.stats(|s| s.occupies), 1);

        // 二次取消：already finished
        assert_eq!(
            mgr.cancel(&r.run_id),
            CancelOutcome::AlreadyFinished(RunState::Cancelled)
        );
        // 未知 run
        assert_eq!(mgr.cancel("nope"), CancelOutcome::NotFound);
    }

    // starting 阶段取消短路：不进执行器、终态 cancelled
    #[tokio::test]
    async fn cancel_during_starting_short_circuits_execution() {
        let (fake_value, release_prepare) = FakeExecutor::starting();
        let started = fake_value.prepare_started.clone();
        let started_wait = started.notified();
        let fake = Arc::new(fake_value);
        let mgr = Arc::new(RunManager::new(fake.clone()));
        let r = mgr
            .submit(req("d1", "p/s.yaml", RunSource::Manual), None)
            .unwrap();
        started_wait.await;
        assert_eq!(mgr.cancel(&r.run_id), CancelOutcome::Accepted);
        release_prepare.notify_one();
        settled(&mgr).await;
        assert_eq!(mgr.get_run(&r.run_id).unwrap().state, RunState::Cancelled);
        assert_eq!(fake.stats(|s| s.executes), 0, "must not enter executor");
    }

    // 执行报错终态 failed；终态记录进档案可通过 get_run 查询
    #[tokio::test]
    async fn execution_error_marks_failed_and_archives() {
        let fake = Arc::new(FakeExecutor {
            step_error: Some("模板找不到 (fake)".into()),
            ..Default::default()
        });
        let mgr = Arc::new(RunManager::new(fake));
        let r = mgr
            .submit(req("d9", "p/dead.yaml", RunSource::Scheduled), None)
            .unwrap();
        settled(&mgr).await;
        let rec = mgr.get_run(&r.run_id).unwrap();
        assert_eq!(rec.state, RunState::Failed);
        assert_eq!(rec.scheduled_at, None);
        assert!(rec.finished_at.is_some());
        assert_eq!(mgr.active_for_device("d9"), None);
    }

    // 停机 drain：拒绝新提交 + 等待/超时强停
    #[tokio::test]
    async fn shutdown_drains_rejects_new_and_force_cancels_on_timeout() {
        let fake = Arc::new(FakeExecutor::hanging());
        let mgr = Arc::new(RunManager::new(fake.clone()));
        let r = mgr
            .submit(req("d1", "p/s.yaml", RunSource::Manual), None)
            .unwrap();
        let guard = mgr.clone();
        tokio::spawn(async move {
            guard
                .begin_shutdown(std::time::Duration::from_millis(250))
                .await;
        });
        // 关闸立刻生效
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(matches!(
            mgr.submit(req("d2", "p/x.yaml", RunSource::Manual), None),
            Err(StartError::ShuttingDown)
        ));
        // 挂起 run 被超时强停
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert_eq!(mgr.get_run(&r.run_id).unwrap().state, RunState::Cancelled);
        assert_eq!(fake.stats(|s| s.releases), 1);
    }

    // 完成钩子收到终态与日志（调度 bookkeeping 依赖此接缝）
    #[tokio::test]
    async fn finish_hook_receives_outcome() {
        let mgr = Arc::new(RunManager::new(Arc::new(FakeExecutor::default())));
        let seen: Arc<PlMutex<Vec<(String, usize)>>> = Arc::new(PlMutex::new(vec![]));
        let sink = seen.clone();
        let hook: FinishHook = Arc::new(move |rec, out| {
            sink.lock().push((rec.run_id.clone(), out.logs().len()));
        });
        let r = mgr
            .submit(req("d1", "p/s.yaml", RunSource::Scheduled), Some(hook))
            .unwrap();
        settled(&mgr).await;
        let calls = seen.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, r.run_id);
        assert_eq!(calls[0].1, 1);
    }

    #[tokio::test]
    async fn wait_terminal_resolves_from_state_notification() {
        let mgr = Arc::new(RunManager::new(Arc::new(FakeExecutor::default())));
        let run = mgr
            .submit(req("d1", "p/s.yaml", RunSource::Scheduled), None)
            .unwrap();
        let record = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            mgr.wait_terminal(&run.run_id),
        )
        .await
        .expect("terminal notification timed out")
        .expect("run record should remain queryable");
        assert_eq!(record.run_id, run.run_id);
        assert_eq!(record.state, RunState::Success);
    }

    #[tokio::test]
    async fn finish_hook_runs_for_starting_cancel_and_prepare_failure() {
        let (fake_value, release_prepare) = FakeExecutor::starting();
        let started = fake_value.prepare_started.clone();
        let started_wait = started.notified();
        let fake = Arc::new(fake_value);
        let mgr = Arc::new(RunManager::new(fake));
        let seen: Arc<PlMutex<Vec<RunState>>> = Arc::new(PlMutex::new(vec![]));
        let sink = seen.clone();
        let hook: FinishHook = Arc::new(move |rec, _| sink.lock().push(rec.state));
        let cancelled = mgr
            .submit(req("d1", "p/cancel.yaml", RunSource::Scheduled), Some(hook))
            .unwrap();
        started_wait.await;
        assert_eq!(mgr.cancel(&cancelled.run_id), CancelOutcome::Accepted);
        release_prepare.notify_one();
        settled(&mgr).await;

        let failed = Arc::new(RunManager::new(Arc::new(FakeExecutor::connect_fail())));
        let sink = seen.clone();
        let hook: FinishHook = Arc::new(move |rec, _| sink.lock().push(rec.state));
        let failed_run = failed
            .submit(req("d2", "p/fail.yaml", RunSource::Scheduled), Some(hook))
            .unwrap();
        settled(&failed).await;

        assert_eq!(
            mgr.get_run(&cancelled.run_id).unwrap().state,
            RunState::Cancelled
        );
        assert_eq!(
            failed.get_run(&failed_run.run_id).unwrap().state,
            RunState::Failed
        );
        assert_eq!(*seen.lock(), vec![RunState::Cancelled, RunState::Failed]);
    }

    // panic 路径：guard drop 兜底终态 failed + 槽释放 + release 配对
    #[tokio::test]
    async fn panic_inside_execute_is_caught_by_guards() {
        struct Panicky;
        impl RunExecutor for Panicky {
            fn prepare<'a>(
                &'a self,
                _: &'a RunContext,
                _: &'a RunRequest,
            ) -> BoxFuture<'a, anyhow::Result<()>> {
                Box::pin(async { Ok(()) })
            }
            fn execute<'a>(
                &'a self,
                _: &'a RunContext,
                _: &'a RunRequest,
                _realtime_logs: bool,
                _: Arc<AtomicBool>,
            ) -> BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
                Box::pin(async {
                    panic!("boom inside executor");
                })
            }
            fn acquire(&self, _: &RunContext) -> anyhow::Result<Box<dyn ActivityLease>> {
                Ok(Box::new(crate::core::NoopLease))
            }
        }
        // 借助包装执行器断言 release 被调用（panic 展开 guard 生效）
        struct WrapProxy {
            inner: Arc<Panicky>,
            releases: Arc<AtomicUsize>,
        }
        struct CountedLease(Arc<AtomicUsize>);
        impl ActivityLease for CountedLease {}
        impl Drop for CountedLease {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        impl RunExecutor for WrapProxy {
            fn prepare<'a>(
                &'a self,
                context: &'a RunContext,
                request: &'a RunRequest,
            ) -> BoxFuture<'a, anyhow::Result<()>> {
                self.inner.prepare(context, request)
            }
            fn execute<'a>(
                &'a self,
                context: &'a RunContext,
                request: &'a RunRequest,
                realtime_logs: bool,
                stop: Arc<AtomicBool>,
            ) -> BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
                self.inner.execute(context, request, realtime_logs, stop)
            }
            fn acquire(&self, _: &RunContext) -> anyhow::Result<Box<dyn ActivityLease>> {
                Ok(Box::new(CountedLease(self.releases.clone())))
            }
        }
        let releases = Arc::new(AtomicUsize::new(0));
        let exec = Arc::new(WrapProxy {
            inner: Arc::new(Panicky),
            releases: releases.clone(),
        });
        let mgr = Arc::new(RunManager::new(exec));
        let seen: Arc<PlMutex<Vec<RunState>>> = Arc::new(PlMutex::new(vec![]));
        let sink = seen.clone();
        let hook: FinishHook = Arc::new(move |rec, _| sink.lock().push(rec.state));
        let r = mgr
            .submit(req("dp", "p/s.yaml", RunSource::Manual), Some(hook))
            .unwrap();
        mgr.wait_settled(std::time::Duration::from_secs(5)).await;
        let rec = mgr.get_run(&r.run_id).unwrap();
        assert_eq!(rec.state, RunState::Failed, "panic must land in failed");
        assert!(rec.error.unwrap().contains("panic"));
        assert_eq!(
            mgr.active_for_device("dp"),
            None,
            "slot released after panic"
        );
        assert_eq!(
            releases.load(Ordering::SeqCst),
            1,
            "occupy paired even on unwind"
        );
        assert_eq!(*seen.lock(), vec![RunState::Failed]);
    }
    // 统一 RunTarget：函数测试目标与脚本目标同样受设备互斥 + 可取消
    #[tokio::test]
    async fn function_run_target_conflicts_and_cancels() {
        let fake = Arc::new(FakeExecutor::hanging());
        let mgr = Arc::new(RunManager::new(fake.clone()));
        let app = crate::core::AppContext::for_test("d1", "com.test.app").unwrap();
        let request = RunRequest::for_app(
            app,
            "gamer.yaml",
            "com.test.app/common.yaml#login",
            crate::core::RunPayload::empty(),
        )
        .unwrap();
        let req = StartRequest {
            request,
            source: RunSource::Manual,
            task_id: None,
            scheduled_at: None,
            realtime_logs: false,
        };
        let a = mgr.submit(req.clone(), None).unwrap();
        assert_eq!(a.script_id, "com.test.app/common.yaml#login", "展示标签");
        // 同设备第二个函数运行 → 409 携带在册记录（展示标签一致）
        match mgr.submit(req, None) {
            Err(StartError::Conflict(busy)) => {
                assert_eq!(busy.script_id, "com.test.app/common.yaml#login");
            }
            other => panic!("expected conflict, got {:?}", other.map(|r| r.run_id)),
        }
        // 取消 → stopping → cancelled，设备槽释放（等进入 running 再取消，
        // 避免踩 starting 短路路径——该路径本就不 occupy/release）
        mgr.wait_for_state(
            &a.run_id,
            |rec| rec.state == RunState::Running,
            std::time::Duration::from_secs(5),
        )
        .await;
        assert_eq!(mgr.cancel(&a.run_id), CancelOutcome::Accepted);
        settled(&mgr).await;
        assert_eq!(mgr.get_run(&a.run_id).unwrap().state, RunState::Cancelled);
        assert_eq!(mgr.active_for_device("d1"), None);
        assert_eq!(fake.stats(|s| s.releases), 1);
    }
}
