//! UpdateController（SYS-003 / release/contracts/ipc-v1.md §7、system-api-v1 §4.1）。
//!
//! 三个实现对应契约的部署形态：
//! - [`LauncherController`]：launcher 便携托管（managed）。tokio Windows named
//!   pipe **客户端**连 `GAMER_LAUNCHER_PIPE` 注入的完整 pipe 名（server 不自行
//!   拼接 installation-id），逐帧携带 `GAMER_LAUNCHER_IPC_TOKEN` 令牌；长操作
//!   **受理即回**，进展以 `status` 轮询（建议值：活跃 1s / 空闲 5s，由协调器驱动）。
//! - [`UnsupportedController`]：直跑（unsupported）。从不创建 IPC 连接，全部
//!   动作 `update_not_managed`（HTTP 409），capability 全 false。
//! - [`DockerController`]：容器（external strategy 适配器）。行为同
//!   unsupported——Docker 升级 = 宿主机换镜像，不经升级器。
//!
//! launcher 进程死亡 / pipe 消失：LauncherController 的动作与 status 返回
//! `launcher_unreachable`（502），server 不退出、不自动拉起 launcher（契约 §7）。

use async_trait::async_trait;

use super::ipc::{DependencyKind, LauncherClient, LauncherUpdateStatus, UpdateError};
use super::model::{UpdateErrorCode, UpdateState};
use super::pipe::PipeTransport;

/// 四能力布尔（system-api-v1 §2.1 冻结四键；仅由部署形态决定）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub check: bool,
    pub download: bool,
    pub install: bool,
    pub rollback: bool,
}

impl Capabilities {
    /// managed（launcher + IPC 通道建立）→ 全 true
    pub const MANAGED: Self = Self {
        check: true,
        download: true,
        install: true,
        rollback: true,
    };
    /// external / unsupported → 全 false
    pub const NONE: Self = Self {
        check: false,
        download: false,
        install: false,
        rollback: false,
    };
}

/// 长操作受理结果（ipc-v1 §4.2：`accepted:true` + operation + update_id + state）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acceptance {
    pub update_id: Option<String>,
    pub state: Option<UpdateState>,
}

/// 升级控制器抽象（server 侧唯一 IPC 出口；HTTP API 与协调器都经它）。
/// 方法一一对应 ipc-v1 §4 的 6 个操作枚举 + `status` 聚合查询 + capability。
#[async_trait]
pub trait UpdateController: Send + Sync {
    /// update_strategy 取值（managed | external | unsupported，契约 §2.1 冻结映射）
    fn strategy(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    /// launcher journal 快照（`status` 只读操作）；降级实现返回空状态（11 态
    /// 语义上的 idle）。通道不通 → `launcher_unreachable`。
    async fn status(&self) -> Result<LauncherUpdateStatus, UpdateError>;
    async fn check(&self) -> Result<Acceptance, UpdateError>;
    async fn download(&self) -> Result<Acceptance, UpdateError>;
    /// 安装前整备（复验 staging 完整性、标记可切换）。受理成功后 launcher 侧
    /// 接管停机切换（它调 `POST /api/shutdown` 优雅 drain 本进程）。
    async fn prepare_install(&self) -> Result<Acceptance, UpdateError>;
    /// committed 之前的自动回滚（恢复 previous + 已验证快照）。
    async fn rollback(&self) -> Result<Acceptance, UpdateError>;
    /// 依赖修复编排（inventory→seed/cache→remote→probe）；scrcpy 不可修。
    /// server 暂无自动调用路径（修复编排消费方为 launcher）——契约面完整性保留
    #[allow(dead_code)]
    async fn repair_dependency(
        &self,
        dependency: DependencyKind,
    ) -> Result<Acceptance, UpdateError>;
}

/// launcher 托管实现：单条长连接 + 失败即断、下次交换重建（ipc-v1 §1.3）。
pub struct LauncherController {
    client: LauncherClient<PipeTransport>,
}

impl LauncherController {
    /// 生产装配：pipe 名与令牌都来自 launcher 注入的环境变量；任一缺失即
    /// 视为未托管（防御：正常 launcher 启动必然同时注入两个变量）。
    pub fn from_env() -> Option<Self> {
        let pipe = non_empty_env("GAMER_LAUNCHER_PIPE")?;
        let token = non_empty_env("GAMER_LAUNCHER_IPC_TOKEN")?;
        Some(Self::new(pipe, token))
    }

    pub fn new(pipe_name: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            client: LauncherClient::new(PipeTransport::new(pipe_name), token.into()),
        }
    }

    /// 长操作统一受理路径：传输/业务失败按 ipc-v1 §6 两类归一
    async fn accept(&self, op: super::ipc::Operation) -> Result<Acceptance, UpdateError> {
        match self.client.accept(op).await {
            Ok(accepted) => Ok(Acceptance {
                update_id: accepted.update_id,
                state: accepted.state,
            }),
            Err(failure) => Err(failure.into_update_error(op)),
        }
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[async_trait]
impl UpdateController for LauncherController {
    fn strategy(&self) -> &'static str {
        "managed"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::MANAGED
    }

    async fn status(&self) -> Result<LauncherUpdateStatus, UpdateError> {
        // status 为同步只读操作；业务错误帧属规格外（ipc-v1 §4 无此形态），
        // 与传输损伤同样按通道不可达处置
        self.client.status().await.map_err(UpdateError::from_frame)
    }

    async fn check(&self) -> Result<Acceptance, UpdateError> {
        self.accept(super::ipc::Operation::Check).await
    }

    async fn download(&self) -> Result<Acceptance, UpdateError> {
        self.accept(super::ipc::Operation::Download).await
    }

    async fn prepare_install(&self) -> Result<Acceptance, UpdateError> {
        self.accept(super::ipc::Operation::PrepareInstall).await
    }

    async fn rollback(&self) -> Result<Acceptance, UpdateError> {
        self.accept(super::ipc::Operation::Rollback).await
    }

    async fn repair_dependency(
        &self,
        dependency: DependencyKind,
    ) -> Result<Acceptance, UpdateError> {
        let op = super::ipc::Operation::RepairDependency(dependency);
        self.accept(op).await
    }
}

/// 直跑降级（ipc-v1 §7 冻结）：无 launcher、从不创建 IPC 连接。
pub struct UnsupportedController;

#[async_trait]
impl UpdateController for UnsupportedController {
    fn strategy(&self) -> &'static str {
        "unsupported"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::NONE
    }

    async fn status(&self) -> Result<LauncherUpdateStatus, UpdateError> {
        Ok(LauncherUpdateStatus::default())
    }

    async fn check(&self) -> Result<Acceptance, UpdateError> {
        Err(not_managed())
    }

    async fn download(&self) -> Result<Acceptance, UpdateError> {
        Err(not_managed())
    }

    async fn prepare_install(&self) -> Result<Acceptance, UpdateError> {
        Err(not_managed())
    }

    async fn rollback(&self) -> Result<Acceptance, UpdateError> {
        Err(not_managed())
    }

    async fn repair_dependency(
        &self,
        _dependency: DependencyKind,
    ) -> Result<Acceptance, UpdateError> {
        Err(not_managed())
    }
}

/// Docker external 适配器（ipc-v1 §7）：行为同 unsupported，strategy 枚举不同。
/// Docker 升级 = 宿主机更换镜像；server 不发起任何升级事务。
pub struct DockerController;

#[async_trait]
impl UpdateController for DockerController {
    fn strategy(&self) -> &'static str {
        "external"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::NONE
    }

    async fn status(&self) -> Result<LauncherUpdateStatus, UpdateError> {
        Ok(LauncherUpdateStatus::default())
    }

    async fn check(&self) -> Result<Acceptance, UpdateError> {
        Err(not_managed())
    }

    async fn download(&self) -> Result<Acceptance, UpdateError> {
        Err(not_managed())
    }

    async fn prepare_install(&self) -> Result<Acceptance, UpdateError> {
        Err(not_managed())
    }

    async fn rollback(&self) -> Result<Acceptance, UpdateError> {
        Err(not_managed())
    }

    async fn repair_dependency(
        &self,
        _dependency: DependencyKind,
    ) -> Result<Acceptance, UpdateError> {
        Err(not_managed())
    }
}

fn not_managed() -> UpdateError {
    UpdateError::new(
        UpdateErrorCode::UpdateNotManaged,
        "当前部署模式不受升级器托管（Docker 升级请在宿主机更换镜像，直跑模式请手动替换程序）",
    )
}

/// 按部署形态装配生产控制器（main 启动路径唯一入口）
pub fn build_for_mode(mode: crate::deps_probe::Mode) -> std::sync::Arc<dyn UpdateController> {
    match mode {
        crate::deps_probe::Mode::Launcher => LauncherController::from_env()
            .map(|c| std::sync::Arc::new(c) as std::sync::Arc<dyn UpdateController>)
            .unwrap_or_else(|| {
                tracing::warn!(
                    "launcher mode without IPC env injection; update controller degraded to unsupported"
                );
                std::sync::Arc::new(UnsupportedController)
            }),
        crate::deps_probe::Mode::Docker => std::sync::Arc::new(DockerController),
        crate::deps_probe::Mode::Direct => std::sync::Arc::new(UnsupportedController),
    }
}

#[cfg(test)]
pub(crate) mod mock {
    //! 测试控制器：记录调用序列、可配置 status 结果、可阻塞 prepare_install
    //! （SYS-006 / QA-006 的 202-先于停机、busy 单受理场景驱动器）。

    use std::sync::Mutex;

    use super::*;
    use crate::update::ipc::LastErrorCodeMessage;

    #[derive(Default)]
    struct Inner {
        calls: Vec<String>,
        status: LauncherUpdateStatus,
        /// prepare_install 完成门闩：Some(rx) 时挂起直到收到信号
        hold_prepare: Option<tokio::sync::oneshot::Receiver<()>>,
        /// prepare_install 注入的同步失败（SYS-006 被拒 → failed + 错误码）
        prepare_error: Option<UpdateErrorCode>,
    }

    #[derive(Default)]
    pub struct MockController {
        inner: Mutex<Inner>,
    }

    impl MockController {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn set_status(&self, status: LauncherUpdateStatus) {
            self.inner.lock().unwrap().status = status;
        }

        pub fn fail_prepare_with(&self, code: UpdateErrorCode) {
            let mut inner = self.inner.lock().unwrap();
            inner.status.last_error = Some(LastErrorCodeMessage {
                code: code.as_str().to_string(),
                message: "mock failure".into(),
            });
            inner.prepare_error = Some(code);
        }

        /// 让下一次 prepare_install 挂起；返回信号端（收到 = 释放）
        pub fn hold_prepare(&self) -> tokio::sync::oneshot::Sender<()> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.inner.lock().unwrap().hold_prepare = Some(rx);
            tx
        }

        pub fn calls(&self) -> Vec<String> {
            self.inner.lock().unwrap().calls.clone()
        }
    }

    #[async_trait]
    impl UpdateController for MockController {
        fn strategy(&self) -> &'static str {
            "managed"
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::MANAGED
        }

        async fn status(&self) -> Result<LauncherUpdateStatus, UpdateError> {
            self.inner.lock().unwrap().calls.push("status".into());
            Ok(self.inner.lock().unwrap().status.clone())
        }

        async fn check(&self) -> Result<Acceptance, UpdateError> {
            self.inner.lock().unwrap().calls.push("check".into());
            Ok(Acceptance {
                update_id: Some("upd-mock-1".into()),
                state: Some(UpdateState::Checking),
            })
        }

        async fn download(&self) -> Result<Acceptance, UpdateError> {
            self.inner.lock().unwrap().calls.push("download".into());
            Ok(Acceptance {
                update_id: Some("upd-mock-1".into()),
                state: Some(UpdateState::Downloading),
            })
        }

        async fn prepare_install(&self) -> Result<Acceptance, UpdateError> {
            let hold = {
                let mut inner = self.inner.lock().unwrap();
                inner.calls.push("prepare_install".into());
                inner.hold_prepare.take()
            };
            if let Some(rx) = hold {
                let _ = rx.await;
            }
            if let Some(code) = self.inner.lock().unwrap().prepare_error {
                return Err(UpdateError::new(code, "mock prepare failure"));
            }
            Ok(Acceptance {
                update_id: Some("upd-mock-1".into()),
                state: Some(UpdateState::Installing),
            })
        }

        async fn rollback(&self) -> Result<Acceptance, UpdateError> {
            self.inner.lock().unwrap().calls.push("rollback".into());
            Ok(Acceptance {
                update_id: Some("upd-mock-1".into()),
                state: Some(UpdateState::RollingBack),
            })
        }

        async fn repair_dependency(
            &self,
            dependency: DependencyKind,
        ) -> Result<Acceptance, UpdateError> {
            self.inner
                .lock()
                .unwrap()
                .calls
                .push(format!("repair_dependency:{}", dependency.as_str()));
            Ok(Acceptance {
                update_id: None,
                state: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::ipc::DependencyKind as Dep;

    /// 三实现的 strategy/capability 冻结映射（契约 §2.1 / ipc-v1 §7）
    #[test]
    fn strategy_and_capabilities_follow_contract_mapping() {
        let launcher = LauncherController::new(r"\\.\pipe\gamebot-test", "token");
        assert_eq!(launcher.strategy(), "managed");
        assert_eq!(launcher.capabilities(), Capabilities::MANAGED);

        let unsupported = UnsupportedController;
        assert_eq!(unsupported.strategy(), "unsupported");
        assert_eq!(unsupported.capabilities(), Capabilities::NONE);

        let docker = DockerController;
        assert_eq!(docker.strategy(), "external");
        assert_eq!(docker.capabilities(), Capabilities::NONE);
    }

    /// 降级实现：status = 空状态（idle 语义）；四动作一律 update_not_managed 409
    #[tokio::test]
    async fn degraded_controllers_return_not_managed_for_every_action() {
        let unsupported: std::sync::Arc<dyn UpdateController> =
            std::sync::Arc::new(UnsupportedController);
        let docker: std::sync::Arc<dyn UpdateController> = std::sync::Arc::new(DockerController);
        for controller in [&unsupported, &docker] {
            let status = controller.status().await.unwrap();
            assert_eq!(status.state, None, "空状态由 service 合成为 idle");
            for err in [
                controller.check().await.unwrap_err(),
                controller.download().await.unwrap_err(),
                controller.prepare_install().await.unwrap_err(),
                controller.rollback().await.unwrap_err(),
                controller.repair_dependency(Dep::Adb).await.unwrap_err(),
            ] {
                assert_eq!(err.code, UpdateErrorCode::UpdateNotManaged);
                assert_eq!(err.code.http_status(), 409);
            }
        }
    }

    #[tokio::test]
    async fn launcher_controller_surfaces_launcher_unreachable_without_pipe() {
        // launcher 不在场：pipe 不存在 → 动作 502 launcher_unreachable（可重试），
        // server 不退出、不重连风暴（ipc-v1 §7）
        let controller = LauncherController::new(r"\\.\pipe\gamebot-ctl-test-absent", "token");
        let err = controller.check().await.unwrap_err();
        assert_eq!(err.code, UpdateErrorCode::LauncherUnreachable);
        let err = controller.status().await.unwrap_err();
        assert_eq!(err.code, UpdateErrorCode::LauncherUnreachable);
    }

    /// SYS-003：launcher adapter 透传完整 IPC 请求/受理帧；验证 server 使用
    /// 注入令牌与冻结 operation，而不是把长操作改成本地同步执行。
    #[cfg(windows)]
    #[tokio::test]
    async fn launcher_controller_forwards_token_and_acceptance_over_named_pipe() {
        use crate::update::ipc::{decode_payload, encode_frame};
        use serde_json::json;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::windows::named_pipe::ServerOptions;

        let pipe_name = format!(
            r"\\.\pipe\gamebot-controller-test-{}",
            uuid::Uuid::new_v4().simple()
        );
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();
        let serve = tokio::spawn(async move {
            let mut conn = server;
            conn.connect().await.unwrap();
            let mut prefix = [0u8; 4];
            conn.read_exact(&mut prefix).await.unwrap();
            let len = u32::from_le_bytes(prefix);
            let mut body = vec![0u8; len as usize];
            conn.read_exact(&mut body).await.unwrap();
            let req = decode_payload(&body).unwrap();
            assert_eq!(req["auth"], "controller-token");
            assert_eq!(req["operation"], "check");
            let response = json!({
                "protocol_version": 1,
                "request_id": req["request_id"],
                "ok": true,
                "result": {
                    "accepted": true,
                    "operation": "check",
                    "update_id": "upd-controller-1",
                    "state": "checking"
                }
            });
            conn.write_all(&encode_frame(&response)).await.unwrap();
            conn.flush().await.unwrap();
        });

        let controller = LauncherController::new(pipe_name, "controller-token");
        let accepted = tokio::time::timeout(std::time::Duration::from_secs(10), controller.check())
            .await
            .expect("launcher adapter exchange timed out")
            .unwrap();
        assert_eq!(accepted.update_id.as_deref(), Some("upd-controller-1"));
        assert_eq!(accepted.state, Some(UpdateState::Checking));
        serve.await.unwrap();
    }

    /// SYS-003：生产模式选择器将 direct/docker 映射到各自降级 adapter；这些
    /// 模式不应意外创建 launcher IPC 连接。
    #[tokio::test]
    async fn build_for_mode_selects_non_launcher_adapters_without_ipc() {
        for (mode, expected) in [
            (crate::deps_probe::Mode::Direct, "unsupported"),
            (crate::deps_probe::Mode::Docker, "external"),
        ] {
            let controller = build_for_mode(mode);
            assert_eq!(controller.strategy(), expected);
            assert_eq!(controller.capabilities(), Capabilities::NONE);
            assert_eq!(
                controller.check().await.unwrap_err().code,
                UpdateErrorCode::UpdateNotManaged
            );
        }
    }
}
