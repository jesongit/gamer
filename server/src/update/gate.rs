//! candidate activation gate（OPS-004 / 计划 §6.8）。
//!
//! 环境变量 `GAMER_ACTIVATION_GATE=1` 时 server 以**闸内形态**启动：绑端口但
//! 仅放行 `/health/ready`（ready:false 契约 not-ready 形态）、`/health/shutdown`
//! 与 `POST /api/system/activate`；scheduler / 设备扫描 / watchdog /
//! idle_power_loop 全部不启动、DeviceManager 不初始化；业务读写 API 统一
//! 503 `update_not_ready`。activate 成功（令牌校验通过、仅回环）→ 主流程
//! 完成完整初始化 → stage=ready（/health/ready 翻转 200）。
//!
//! 激活令牌：`X-Launcher-Token` 必须等于 `GAMER_LAUNCHER_IPC_TOKEN` 注入值
//! （launcher 会话令牌，与 IPC 帧的 `auth` 同源）；令牌缺失时闸永久无法激活
//! （fail closed，只记日志）。
//!
//! startup.stage 取值遵循 system-api-v1 §2.1 冻结枚举
//! `starting | maintenance_gate | ready`：闸内报 `maintenance_gate`，业务路由
//! 打开后报 `ready`（任务描述的 gate/activating/active 与契约枚举冲突，以契约为准）。
//!
//! 无 `GAMER_ACTIVATION_GATE` 时启动流程逐字节不变（stage 恒 ready）。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

/// stage 原子取值（契约 §2.1 冻结枚举的进程级投影）
pub const STAGE_STARTING: u8 = 0;
pub const STAGE_MAINTENANCE_GATE: u8 = 1;
pub const STAGE_READY: u8 = 2;

static STAGE: AtomicU8 = AtomicU8::new(STAGE_READY);

/// 当前启动阶段字符串（/api/system/info 的 startup.stage 字段）
pub fn stage_str() -> &'static str {
    match STAGE.load(Ordering::SeqCst) {
        STAGE_STARTING => "starting",
        STAGE_MAINTENANCE_GATE => "maintenance_gate",
        _ => "ready",
    }
}

/// 设置启动阶段（main 启动路径与 gate 测试使用）
pub fn set_stage(stage: u8) {
    STAGE.store(stage, Ordering::SeqCst);
}

/// 激活请求的拒绝原因（API 映射 403）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivateReject {
    /// 令牌缺失或不匹配
    BadToken,
    /// 来源非回环（activate 仅允许 launcher 本机调用）
    NotLoopback,
}

impl ActivateReject {
    pub fn message(self) -> &'static str {
        match self {
            ActivateReject::BadToken => "activation token mismatch",
            ActivateReject::NotLoopback => "activation only allowed from loopback",
        }
    }
}

/// 启动闸（进程级单例；main 装配，gate 路由与初始化任务共享）
pub struct StartupGate {
    enabled: bool,
    token: Option<String>,
    activated: AtomicBool,
    signal: Notify,
}

impl StartupGate {
    /// 生产装配：`GAMER_ACTIVATION_GATE=1` 开闸；激活令牌取
    /// `GAMER_LAUNCHER_IPC_TOKEN`（与 IPC 帧令牌同源）。开闸但无令牌 →
    /// fail closed（无法激活，日志告警）。
    pub fn from_env() -> Arc<Self> {
        let enabled = std::env::var("GAMER_ACTIVATION_GATE")
            .map(|v| v.trim() == "1")
            .unwrap_or(false);
        let token = std::env::var("GAMER_LAUNCHER_IPC_TOKEN")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        if enabled && token.is_none() {
            tracing::error!(
                "GAMER_ACTIVATION_GATE=1 but GAMER_LAUNCHER_IPC_TOKEN is missing; \
                 activation will be rejected (fail closed)"
            );
        }
        Arc::new(Self::new(enabled, token))
    }

    pub fn new(enabled: bool, token: Option<String>) -> Self {
        Self {
            enabled,
            token,
            activated: AtomicBool::new(false),
            signal: Notify::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// 闸是否已放行（未开闸 = 恒放行；业务初始化照常）
    pub fn is_active(&self) -> bool {
        !self.enabled || self.activated.load(Ordering::SeqCst)
    }

    /// 校验一次 activate 请求（令牌 + 回环）
    pub fn verify(
        &self,
        remote: Option<SocketAddr>,
        header_token: Option<&str>,
    ) -> Result<(), ActivateReject> {
        // 仅回环：launcher 与 server 同机
        match remote {
            Some(addr) if addr.ip().is_loopback() => {}
            _ => return Err(ActivateReject::NotLoopback),
        }
        let expected = self.token.as_deref().unwrap_or("");
        let provided = header_token.map(str::trim).unwrap_or("");
        if expected.is_empty() || provided.is_empty() || !constant_time_eq(expected, provided) {
            return Err(ActivateReject::BadToken);
        }
        Ok(())
    }

    /// 标记激活并唤醒初始化任务（幂等：已激活时 no-op）
    pub fn activate(&self) -> bool {
        if !self.enabled {
            return true;
        }
        if self
            .activated
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.signal.notify_waiters();
            true
        } else {
            false
        }
    }

    /// 等待激活信号（main 的初始化任务挂在这里）
    pub async fn wait_activation(&self) {
        if self.is_active() {
            return;
        }
        loop {
            let notified = self.signal.notified();
            if self.is_active() {
                return;
            }
            notified.await;
        }
    }
}

/// 长度恒定的令牌比较（时序侧信息最小化；令牌为高熵会话随机数）
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate() -> StartupGate {
        StartupGate::new(true, Some("tok-123".into()))
    }

    #[test]
    fn disabled_gate_is_always_active() {
        let g = StartupGate::new(false, None);
        assert!(!g.enabled());
        assert!(g.is_active());
        assert!(g.activate());
        // 未开闸时 verify 仍会按令牌规则拒绝（调用方在未开闸路径不应到达这里，
        // 但行为保守：无令牌配置一律拒）
        assert_eq!(
            g.verify(Some("127.0.0.1:9000".parse().unwrap()), None),
            Err(ActivateReject::BadToken)
        );
    }

    #[test]
    fn verify_requires_loopback_and_matching_token() {
        let g = gate();
        let loopback: SocketAddr = "127.0.0.1:51000".parse().unwrap();
        let lan: SocketAddr = "192.168.1.9:51000".parse().unwrap();

        assert!(g.verify(Some(loopback), Some("tok-123")).is_ok());
        // 令牌错 / 缺失 → 拒绝
        assert_eq!(
            g.verify(Some(loopback), Some("wrong")),
            Err(ActivateReject::BadToken)
        );
        assert_eq!(
            g.verify(Some(loopback), None),
            Err(ActivateReject::BadToken)
        );
        assert_eq!(
            g.verify(Some(loopback), Some("  ")),
            Err(ActivateReject::BadToken)
        );
        // 非回环 → 拒绝（即使令牌正确）
        assert_eq!(
            g.verify(Some(lan), Some("tok-123")),
            Err(ActivateReject::NotLoopback)
        );
        assert_eq!(
            g.verify(None, Some("tok-123")),
            Err(ActivateReject::NotLoopback)
        );
        // 开闸但无令牌配置 → fail closed
        let tokenless = StartupGate::new(true, None);
        assert_eq!(
            tokenless.verify(Some(loopback), Some("anything")),
            Err(ActivateReject::BadToken)
        );
    }

    #[test]
    fn activate_is_idempotent_and_flips_active() {
        let g = Arc::new(gate());
        assert!(!g.is_active());
        assert!(g.activate());
        assert!(g.is_active());
        // 重复 activate 幂等（返回 false = 重复信号）
        assert!(!g.activate());
        assert!(g.is_active());
    }

    #[tokio::test]
    async fn wait_activation_resolves_after_signal() {
        let g = Arc::new(gate());
        let waiter = tokio::spawn({
            let g = g.clone();
            async move {
                g.wait_activation().await;
                true
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!waiter.is_finished(), "激活前等待不得返回");
        g.activate();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
                .await
                .expect("activation waiter")
                .unwrap()
        );
    }

    #[test]
    fn stage_projection_covers_contract_enum() {
        // 契约 §2.1 冻结枚举：starting | maintenance_gate | ready
        set_stage(STAGE_STARTING);
        assert_eq!(stage_str(), "starting");
        set_stage(STAGE_MAINTENANCE_GATE);
        assert_eq!(stage_str(), "maintenance_gate");
        set_stage(STAGE_READY);
        assert_eq!(stage_str(), "ready");
    }

    /// OPS-004：候选版本 commit 前只运行最小闸内路由；调度触发、设备扫描
    /// 与业务写入都必须在 HTTP 层统一返回 503，不得触及真实 handler/DB。
    #[tokio::test]
    async fn candidate_gate_rejects_scheduler_scan_and_business_writes() {
        use crate::api::gate::{build_gate_router, GateShared};
        use crate::config::Config;
        use crate::shutdown::ShutdownCoordinator;
        use crate::store::{Db, Store};
        use axum::body::Body;
        use axum::http::{Method, Request, StatusCode};
        use tower::ServiceExt;

        let dir = std::env::temp_dir().join(format!(
            "gamer-gate-ops004-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let db: Db = std::sync::Arc::new(Store::open(&cfg).unwrap());
        let shutdown = std::sync::Arc::new(ShutdownCoordinator::new(std::sync::Arc::new(|| {
            Box::pin(async {})
        })));
        let gate = std::sync::Arc::new(StartupGate::new(true, Some("tok".into())));
        let app = build_gate_router(
            cfg,
            db.clone(),
            shutdown,
            gate.clone(),
            std::sync::Arc::new(GateShared::default()),
        );

        assert_eq!(stage_str(), "maintenance_gate");
        for (method, uri, body) in [
            // scheduler/task-now submission
            (Method::POST, "/api/tasks/task-1/run", "{}"),
            // adb scan side effect
            (Method::POST, "/api/devices/scan", ""),
            // ordinary device write
            (Method::POST, "/api/devices", "{}"),
            // script write is also a business mutation
            (Method::POST, "/api/scripts", "{}"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(&method)
                        .uri(uri)
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{method} {uri}"
            );
            let json = axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&json).unwrap();
            assert_eq!(json["code"], "update_not_ready", "{method} {uri}");
        }

        assert!(!gate.is_active());
        assert!(db.list_devices().unwrap().is_empty());
        assert!(db.list_timer_tasks().unwrap().is_empty());
        set_stage(STAGE_READY);
        let _ = std::fs::remove_dir_all(dir);
    }
}
