//! 更新运行时服务（SYS-004 / SYS-006）：HTTP API 与后台协调器的共享状态层。
//!
//! 职责：
//! - **状态聚合**：`status`（launcher journal 经 IPC）+ 本地缓存 → 合成
//!   `GET /api/system/update` 契约体（11 态跨重启由 launcher journal 保证，
//!   server 不持久化状态机）；
//! - **动作受理**：§4.2 状态×动作矩阵（[`model::admit`]）+ §4.3 install 门禁
//!   （workload/staging 完好性）+ 单事务门禁（两 install 并发只受理一个）；
//! - **策略存储**：`PUT policy` 热生效 + state/ JSON 持久化（[`policy::PolicyStore`]）；
//! - **审计**：install 受理（置 installing 后、prepare_install 前）与被拒写
//!   运行日志（SYS-006），消息不含路径/token/用户名。
//!
//! 泄露禁令（契约 §1.3）：错误 message 来自固定文案或 launcher 无泄露描述。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::controller::{Capabilities, UpdateController};
use super::ipc::{LastErrorCodeMessage, LauncherUpdateStatus, UpdateError};
use super::model::{admit, Admission, InstallBlocking, UpdateAction, UpdateErrorCode, UpdateState};
use super::policy::{PolicyStore, PolicyValidationError, UpdatePolicy};
use super::workload::Workload;
use crate::store::Db;

/// status 刷新节流：GET 高频轮询不至于每次都打一帧 IPC（ipc-v1 §5.2 建议活跃
/// 1s / 空闲 5s；动作端点经 force 绕过节流拿到准实时状态）
const STATUS_REFRESH_THROTTLE: Duration = Duration::from_millis(500);

/// 单事务门禁：install/rollback 非幂等——并发第二个请求必须 409 `update_busy`
/// （契约 §7；计划 §11.4：两个 install 只有一个取得事务）。
#[derive(Default)]
pub struct UpdateTxn {
    active: AtomicBool,
}

impl UpdateTxn {
    /// 取得事务；false = 已有事务进行中
    pub fn try_begin(&self) -> bool {
        !self.active.swap(true, Ordering::SeqCst)
    }

    pub fn end(&self) {
        self.active.store(false, Ordering::SeqCst);
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }
}

/// 状态缓存：launcher journal 的最近一次快照 + 状态最后变更时间
#[derive(Default)]
struct StatusCache {
    status: LauncherUpdateStatus,
    updated_at: Option<String>,
    last_refresh: Option<std::time::Instant>,
}

impl StatusCache {
    fn same_state(a: &LauncherUpdateStatus, b: &LauncherUpdateStatus) -> bool {
        a.state == b.state
            && a.detail == b.detail
            && a.update_id == b.update_id
            && a.candidate == b.candidate
            && a.progress == b.progress
            && a.last_error == b.last_error
    }
}

type SharedCache = Arc<Mutex<StatusCache>>;

/// workload 快照提供者（生产 = [`workload::WorkloadSource`]；测试注入合成值）
pub type WorkloadProvider = Arc<dyn Fn() -> Workload + Send + Sync>;

/// docker/direct 模式的固定拒绝文案（契约 §7 update_not_managed）
pub const NOT_MANAGED_MESSAGE: &str =
    "当前部署模式不受升级器托管（Docker 升级请在宿主机更换镜像，直跑模式请手动替换程序）";

pub struct UpdateService {
    controller: Arc<dyn UpdateController>,
    policy: Arc<PolicyStore>,
    txn: Arc<UpdateTxn>,
    cache: SharedCache,
    workload: WorkloadProvider,
    /// 审计日志落点（运行日志表；device_id=system / script_id=update）
    db: Db,
}

impl UpdateService {
    pub fn new(
        controller: Arc<dyn UpdateController>,
        policy: Arc<PolicyStore>,
        txn: Arc<UpdateTxn>,
        workload: WorkloadProvider,
        db: Db,
    ) -> Self {
        Self {
            controller,
            policy,
            txn,
            cache: Arc::new(Mutex::new(StatusCache::default())),
            workload,
            db,
        }
    }

    /// 是否受升级器托管（launcher 模式）；docker/direct 下动作端点一律
    /// 409 `update_not_managed`（契约 §4.1）
    pub fn managed(&self) -> bool {
        self.controller.capabilities() != Capabilities::NONE
    }

    fn audit(&self, level: &str, msg: &str) {
        if let Err(e) = self.db.add_log("system", "update", level, msg) {
            tracing::warn!(error = %e, "update audit log write failed");
        }
    }

    // ---------- 状态聚合 ----------

    /// 当前展示态（缓存优先；launcher journal 缺 state 时合成 idle）
    pub fn cached_state(&self) -> UpdateState {
        self.cache
            .lock()
            .unwrap()
            .status
            .state
            .unwrap_or(UpdateState::Idle)
    }

    /// Refresh the launcher journal before a coordinator decision. The
    /// coordinator must not make an automatic decision from the process-start
    /// default (`idle`) when the launcher already has a staged/downloaded
    /// candidate. On an unavailable launcher the last cached state remains the
    /// safe fallback and the caller can retry on its next tick.
    pub async fn refresh_state(&self) -> Result<UpdateState, UpdateError> {
        let status = self.refresh(true).await?;
        Ok(status.state.unwrap_or(UpdateState::Idle))
    }

    /// 实时空闲快照（协调器评估用；§4.3 门禁与 auto 判定的统一输入）
    pub fn workload_snapshot(&self) -> Workload {
        (self.workload)()
    }

    /// 从 controller 拉一次 launcher journal 并入缓存。通道不通时保留缓存并
    /// 返回 Err（GET 走缓存降级展示；动作端点把 Err 映射 502 launcher_unreachable）
    async fn refresh(&self, force: bool) -> Result<LauncherUpdateStatus, UpdateError> {
        {
            let cache = self.cache.lock().unwrap();
            if !force
                && cache
                    .last_refresh
                    .is_some_and(|t| t.elapsed() < STATUS_REFRESH_THROTTLE)
            {
                return Ok(cache.status.clone());
            }
        }
        let status = self.controller.status().await?;
        let mut cache = self.cache.lock().unwrap();
        if !StatusCache::same_state(&cache.status, &status) || cache.updated_at.is_none() {
            cache.updated_at = Some(now_rfc3339());
        }
        cache.status = status.clone();
        cache.last_refresh = Some(std::time::Instant::now());
        Ok(status)
    }

    /// `GET /api/system/update` 契约体（先尽力刷新再合成；launcher 暂不可达时
    /// 用最近缓存降级展示，永不 5xx）
    pub async fn status_body(&self) -> Value {
        let status = match self.refresh(false).await {
            Ok(s) => s,
            Err(_) => self.cache.lock().unwrap().status.clone(),
        };
        let policy = self.policy.get().await;
        let updated_at = self.cache.lock().unwrap().updated_at.clone();
        status_json(&status, &policy, &updated_at.unwrap_or_else(now_rfc3339))
    }

    // ---------- 动作端点（受理路径） ----------

    /// POST check（幂等，202 形态）
    pub async fn request_check(&self) -> Result<Value, UpdateError> {
        self.request_long_op(UpdateAction::Check, Op::Check).await
    }

    /// POST download（幂等，202 形态；staged 态 no-op 由矩阵给出 staged）
    pub async fn request_download(&self) -> Result<Value, UpdateError> {
        self.request_long_op(UpdateAction::Download, Op::Download)
            .await
    }

    /// POST rollback（非幂等，202 形态；受理后 launcher 侧恢复旧版本并重启）
    pub async fn request_rollback(&self) -> Result<Value, UpdateError> {
        // 非幂等：先取事务再发 IPC（并发第二个 409 update_busy）
        if !self.txn.try_begin() {
            return Err(UpdateError::new(UpdateErrorCode::UpdateBusy, BUSY_MESSAGE));
        }
        let result = self
            .request_long_op(UpdateAction::Rollback, Op::Rollback)
            .await;
        if result.is_err() {
            self.txn.end();
        }
        // 受理成功后事务标记同样释放：rolling_back 态由矩阵（全 busy）兜底，
        // 避免重启未发生时事务标志悬挂
        self.txn.end();
        result
    }

    /// POST install（非幂等，202 形态）：门禁全过 → 202 立即返回 → 后台
    /// prepare_install（SYS-006：202 先于停机；受理即置 installing + 审计日志；
    /// 被拒 → failed + last_error）。
    pub async fn request_install(&self) -> Result<Value, UpdateError> {
        if !self.managed() {
            return Err(UpdateError::new(
                UpdateErrorCode::UpdateNotManaged,
                NOT_MANAGED_MESSAGE,
            ));
        }
        let status = self.refresh(true).await?;
        let state = status.state.unwrap_or(UpdateState::Idle);

        // §4.2 矩阵先裁掉非法组合；install 的合法入口（staged/waiting/failed）
        // 全部落入 InstallGate 做门禁判定
        match admit(state, UpdateAction::Install) {
            Admission::Rejected(code) => {
                // available 态：staging 未就绪（§4.2 括注），按门禁形态给 blocking
                if code == UpdateErrorCode::UpdateNotReady {
                    return Err(UpdateError::new(
                        code,
                        "安装条件未满足：新组件尚未完整下载/验签并就位于 staging",
                    )
                    .with_details(json!({ "blocking": ["staging_not_ready"] })));
                }
                return Err(UpdateError::new(code, code_default_message(code)));
            }
            Admission::Accepted(_) => unreachable!("install never directly accepted"),
            Admission::InstallGate => {}
        }
        // §4.3 install 门禁：任一不满足 → 409 update_not_ready + blocking 全量
        // （viewer 在线不是硬门禁——由协调器经优雅停机链路处理）
        let policy = self.policy.get().await;
        let mut blocking: Vec<InstallBlocking> =
            (self.workload)().install_blockings(policy.freeze_minutes);
        // staging 完好性：staged/waiting 才视为就位；failed 态 staging 完整性
        // 未经验证（失败可能发生在下载/验签），保守要求重新下载
        if !matches!(state, UpdateState::Staged | UpdateState::Waiting) {
            blocking.push(InstallBlocking::StagingNotReady);
        }
        if !blocking.is_empty() {
            let names: Vec<&str> = blocking.iter().map(|b| b.as_str()).collect();
            return Err(UpdateError::new(
                UpdateErrorCode::UpdateNotReady,
                "安装条件未满足，详见 blocking 列表；满足后可重试",
            )
            .with_details(json!({ "blocking": names })));
        }

        // 单事务门禁：两 install 只有一个取得（§7 update_busy / §11.4）
        if !self.txn.try_begin() {
            return Err(UpdateError::new(UpdateErrorCode::UpdateBusy, BUSY_MESSAGE));
        }

        // 状态机置 installing + 审计日志（prepare_install 之前；SYS-006）
        let update_id = status.update_id.clone();
        {
            let mut cache = self.cache.lock().unwrap();
            cache.status.state = Some(UpdateState::Installing);
            cache.status.detail = Some("draining".into());
            cache.updated_at = Some(now_rfc3339());
        }
        self.audit(
            "info",
            &format!(
                "update install accepted (id={}): entering installing, handoff to launcher",
                update_id.as_deref().unwrap_or("-")
            ),
        );

        // 后台整备：202 已先行返回；launcher 受理后接管停机（它调 /api/shutdown
        // 优雅 drain 本进程 → 快照/迁移/切换/候选启动）。被拒 → failed + 错误码。
        let background = InstallBackground {
            controller: self.controller.clone(),
            txn: self.txn.clone(),
            cache: self.cache.clone(),
            db: self.db.clone(),
        };
        tokio::spawn(async move { background.run().await });

        Ok(json!({
            "update_id": update_id,
            "state": UpdateState::Installing.as_str(),
        }))
    }

    /// check/download/rollback 的公共受理路径：矩阵 → IPC 受理 → 缓存推进
    async fn request_long_op(&self, action: UpdateAction, op: Op) -> Result<Value, UpdateError> {
        if self.controller.capabilities() == Capabilities::NONE {
            return Err(UpdateError::new(
                UpdateErrorCode::UpdateNotManaged,
                NOT_MANAGED_MESSAGE,
            ));
        }
        let status = self.refresh(true).await?;
        let state = status.state.unwrap_or(UpdateState::Idle);
        let target = match admit(state, action) {
            Admission::Accepted(target) => target,
            Admission::Rejected(code) => {
                return Err(UpdateError::new(code, code_default_message(code)));
            }
            Admission::InstallGate => unreachable!("only install produces gate"),
        };
        let accepted = match op {
            Op::Check => self.controller.check().await?,
            Op::Download => self.controller.download().await?,
            Op::Rollback => self.controller.rollback().await?,
        };
        let new_state = accepted.state.unwrap_or(target);
        let update_id = {
            let mut cache = self.cache.lock().unwrap();
            cache.status.state = Some(new_state);
            cache.status.detail = Some(default_detail(new_state).to_string());
            if let Some(id) = accepted.update_id.clone() {
                cache.status.update_id = Some(id);
            }
            if new_state == UpdateState::Downloading {
                // 重试下载：清空旧失败（§5.1 failed → downloading 重试语义）
                cache.status.last_error = None;
            }
            cache.updated_at = Some(now_rfc3339());
            // 受理体 update_id：优先本次受理帧，其次既有事务 id，最后 null
            accepted
                .update_id
                .or_else(|| cache.status.update_id.clone())
                .map(Value::from)
                .unwrap_or(Value::Null)
        };
        Ok(json!({
            "update_id": update_id,
            "state": new_state.as_str(),
        }))
    }

    // ---------- 策略 ----------

    /// PUT policy：整对象替换（幂等）；校验失败 → invalid_argument + field
    pub async fn set_policy(&self, policy: UpdatePolicy) -> Result<Value, PolicyValidationError> {
        let saved = self.policy.replace(policy).await?;
        Ok(PolicyStore::to_json(&saved))
    }

    /// 当前生效策略（协调器合成用）
    pub async fn current_policy(&self) -> UpdatePolicy {
        self.policy.get().await
    }
}

const BUSY_MESSAGE: &str = "已有升级/回滚事务进行中，请等待其结束后再试";

/// 长操作种类（request_long_op 的静态分派键）
#[derive(Debug, Clone, Copy)]
enum Op {
    Check,
    Download,
    Rollback,
}

/// install 后台整备任务（SYS-006）：prepare_install 成功 → launcher 接管停机
/// （事务标志不清：进程即将被重启；被拒 → 释放事务 + failed + last_error + 审计）
struct InstallBackground {
    controller: Arc<dyn UpdateController>,
    txn: Arc<UpdateTxn>,
    cache: SharedCache,
    db: Db,
}

impl InstallBackground {
    async fn run(self) {
        match self.controller.prepare_install().await {
            Ok(_) => {
                let _ = self.db.add_log(
                    "system",
                    "update",
                    "info",
                    "update prepare_install accepted: launcher takes over (drain/snapshot/switch)",
                );
            }
            Err(e) => {
                self.txn.end();
                let _ = self.db.add_log(
                    "system",
                    "update",
                    "error",
                    &format!("update install failed: {} ({})", e.code.as_str(), e.message),
                );
                let mut cache = self.cache.lock().unwrap();
                cache.status.state = Some(UpdateState::Failed);
                cache.status.detail = Some("failed".into());
                cache.status.last_error = Some(LastErrorCodeMessage {
                    code: e.code.as_str().to_string(),
                    message: e.message.clone(),
                });
                cache.updated_at = Some(now_rfc3339());
            }
        }
    }
}

/// 各态的缺省 journal detail（§5.2 允许集内的驻留值；launcher 给出真值时覆盖）
pub fn default_detail(state: UpdateState) -> &'static str {
    match state {
        UpdateState::Idle => "idle",
        UpdateState::Checking => "checking",
        UpdateState::Available => "checked",
        UpdateState::Downloading => "downloading",
        UpdateState::Staged => "staged",
        UpdateState::Waiting => "waiting_idle",
        UpdateState::Installing => "draining",
        UpdateState::Restarting => "candidate_starting",
        UpdateState::Failed => "failed",
        UpdateState::RollingBack => "rolling_back",
        UpdateState::ManualRecovery => "manual_recovery_required",
    }
}

fn code_default_message(code: UpdateErrorCode) -> &'static str {
    match code {
        UpdateErrorCode::UpdateNotManaged => NOT_MANAGED_MESSAGE,
        UpdateErrorCode::UpdateBusy => BUSY_MESSAGE,
        UpdateErrorCode::UpdateNotAvailable => "当前没有可执行的更新候选，请先执行检查更新",
        UpdateErrorCode::RollbackUnavailable => {
            "没有可用的自动回滚点（自动回滚仅承诺提交之前的事务）"
        }
        UpdateErrorCode::ManualRecoveryRequired => "升级与自动回滚均失败，请按维护手册执行人工恢复",
        _ => "更新请求被拒绝",
    }
}

/// `GET /api/system/update` 契约体装配（纯函数；fixture 键集比对直接驱动）。
/// launcher 给出的 detail 若越出 §5.2 允许集，回退到该态缺省驻留值。
pub fn status_json(
    status: &LauncherUpdateStatus,
    policy: &UpdatePolicy,
    updated_at: &str,
) -> Value {
    let state = status.state.unwrap_or(UpdateState::Idle);
    let detail = match &status.detail {
        Some(d) if state.allows_detail(d) => d.clone(),
        _ => default_detail(state).to_string(),
    };
    json!({
        "state": state.as_str(),
        "detail": detail,
        "update_id": status.update_id.clone().map(Value::from).unwrap_or(Value::Null),
        "candidate": status.candidate.as_ref().map(|c| c.to_http_json()).unwrap_or(Value::Null),
        "progress": status.progress.as_ref().map(|p| json!({
            "bytes_done": p.bytes_done,
            "bytes_total": p.bytes_total,
        })).unwrap_or(Value::Null),
        "policy": PolicyStore::to_json(policy),
        "last_error": status.last_error.as_ref().map(|e| json!({
            "code": e.code,
            "message": e.message,
        })).unwrap_or(Value::Null),
        "updated_at": updated_at,
    })
}

/// 动作端点 202 受理体（契约 §4.1 冻结为 {update_id, state}；fixture 比对与
/// service 受理路径共用）
#[allow(dead_code)]
pub fn accepted_json(update_id: &str, state: UpdateState) -> Value {
    json!({ "update_id": update_id, "state": state.as_str() })
}

/// 业务错误体（契约 §1.2：{code, message, details?}；§1.3 无泄露）
pub fn error_json(err: &UpdateError) -> Value {
    let mut body = json!({
        "code": err.code.as_str(),
        "message": err.message,
    });
    if let Some(details) = &err.details {
        body["details"] = details.clone();
    }
    body
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
