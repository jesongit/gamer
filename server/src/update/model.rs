//! 更新状态机模型（system-api-v1 §5 冻结的 11 态 + §7 错误码 + §4.2 状态×动作
//! 受理矩阵 + §4.3 install 门禁枚举）。
//!
//! 本模块是纯函数层：不含 IO、不含锁，fixture 比对与矩阵测试直接驱动。
//! 权威依据 `release/contracts/system-api-v1.md`（冻结）与
//! `release/contracts/ipc-v1.md`（11 个业务错误码与 HTTP API 共享）。

use serde::{Deserialize, Serialize};

/// 11 态展示枚举（system-api-v1 §5.1 冻结；前端业务分支只依赖此字段）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateState {
    Idle,
    Checking,
    Available,
    Downloading,
    Staged,
    Waiting,
    Installing,
    Restarting,
    Failed,
    RollingBack,
    ManualRecovery,
}

impl UpdateState {
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateState::Idle => "idle",
            UpdateState::Checking => "checking",
            UpdateState::Available => "available",
            UpdateState::Downloading => "downloading",
            UpdateState::Staged => "staged",
            UpdateState::Waiting => "waiting",
            UpdateState::Installing => "installing",
            UpdateState::Restarting => "restarting",
            UpdateState::Failed => "failed",
            UpdateState::RollingBack => "rolling_back",
            UpdateState::ManualRecovery => "manual_recovery",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "idle" => UpdateState::Idle,
            "checking" => UpdateState::Checking,
            "available" => UpdateState::Available,
            "downloading" => UpdateState::Downloading,
            "staged" => UpdateState::Staged,
            "waiting" => UpdateState::Waiting,
            "installing" => UpdateState::Installing,
            "restarting" => UpdateState::Restarting,
            "failed" => UpdateState::Failed,
            "rolling_back" => UpdateState::RollingBack,
            "manual_recovery" => UpdateState::ManualRecovery,
            _ => return None,
        })
    }

    /// state 允许的 journal `detail` 驻留值（§5.2 映射表；诊断展示用）
    pub fn allows_detail(self, detail: &str) -> bool {
        let allowed: &[&str] = match self {
            UpdateState::Idle => &["idle", "committed", "cleaning"],
            UpdateState::Checking => &["checking"],
            UpdateState::Available => &["checked"],
            UpdateState::Downloading => &["downloading", "verifying"],
            UpdateState::Staged => &["staged"],
            UpdateState::Waiting => &["waiting_idle"],
            UpdateState::Installing => &[
                "draining",
                "stopped",
                "snapshotting",
                "snapshot_verified",
                "migrating",
                "switched",
            ],
            UpdateState::Restarting => &["candidate_starting", "candidate_ready", "activating"],
            UpdateState::Failed => &["failed"],
            UpdateState::RollingBack => &["rolling_back"],
            UpdateState::ManualRecovery => &["manual_recovery_required"],
        };
        allowed.contains(&detail)
    }
}

/// 四个动作端点（§4.2 矩阵列）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateAction {
    Check,
    Download,
    Install,
    Rollback,
}

/// 11 个统一业务错误码（§7 冻结；与 IPC 帧共享）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateErrorCode {
    UpdateNotManaged,
    UpdateBusy,
    UpdateNotAvailable,
    UpdateNotReady,
    SignatureInvalid,
    ArtifactInvalid,
    InsufficientSpace,
    SchemaIncompatible,
    LauncherUnreachable,
    RollbackUnavailable,
    ManualRecoveryRequired,
}

impl UpdateErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateErrorCode::UpdateNotManaged => "update_not_managed",
            UpdateErrorCode::UpdateBusy => "update_busy",
            UpdateErrorCode::UpdateNotAvailable => "update_not_available",
            UpdateErrorCode::UpdateNotReady => "update_not_ready",
            UpdateErrorCode::SignatureInvalid => "signature_invalid",
            UpdateErrorCode::ArtifactInvalid => "artifact_invalid",
            UpdateErrorCode::InsufficientSpace => "insufficient_space",
            UpdateErrorCode::SchemaIncompatible => "schema_incompatible",
            UpdateErrorCode::LauncherUnreachable => "launcher_unreachable",
            UpdateErrorCode::RollbackUnavailable => "rollback_unavailable",
            UpdateErrorCode::ManualRecoveryRequired => "manual_recovery_required",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "update_not_managed" => UpdateErrorCode::UpdateNotManaged,
            "update_busy" => UpdateErrorCode::UpdateBusy,
            "update_not_available" => UpdateErrorCode::UpdateNotAvailable,
            "update_not_ready" => UpdateErrorCode::UpdateNotReady,
            "signature_invalid" => UpdateErrorCode::SignatureInvalid,
            "artifact_invalid" => UpdateErrorCode::ArtifactInvalid,
            "insufficient_space" => UpdateErrorCode::InsufficientSpace,
            "schema_incompatible" => UpdateErrorCode::SchemaIncompatible,
            "launcher_unreachable" => UpdateErrorCode::LauncherUnreachable,
            "rollback_unavailable" => UpdateErrorCode::RollbackUnavailable,
            "manual_recovery_required" => UpdateErrorCode::ManualRecoveryRequired,
            _ => return None,
        })
    }

    /// HTTP 状态码映射（§7 冻结列）
    pub fn http_status(self) -> u16 {
        match self {
            UpdateErrorCode::SignatureInvalid
            | UpdateErrorCode::ArtifactInvalid
            | UpdateErrorCode::SchemaIncompatible => 422,
            UpdateErrorCode::InsufficientSpace => 507,
            UpdateErrorCode::LauncherUnreachable => 502,
            _ => 409,
        }
    }

    /// 从 IPC 错误帧 `code` 解析；协议级错误码 / 未知码统一视为通道损伤
    /// （launcher_unreachable，ipc-v1 §7 降级语义）
    pub fn from_ipc_frame_code(value: &str) -> Self {
        Self::parse(value).unwrap_or(UpdateErrorCode::LauncherUnreachable)
    }
}

/// §4.3 install 门禁枚举（blocking 数组元素，冻结全集；launcher_unreachable /
/// insufficient_space 由 launcher 侧判定，server 本地门禁不产出）
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum InstallBlocking {
    StagingNotReady,
    ActiveRun,
    UpdateTransaction,
    CronFreezeWindow,
    LauncherUnreachable,
    InsufficientSpace,
}

impl InstallBlocking {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallBlocking::StagingNotReady => "staging_not_ready",
            InstallBlocking::ActiveRun => "active_run",
            InstallBlocking::UpdateTransaction => "update_transaction",
            InstallBlocking::CronFreezeWindow => "cron_freeze_window",
            InstallBlocking::LauncherUnreachable => "launcher_unreachable",
            InstallBlocking::InsufficientSpace => "insufficient_space",
        }
    }
}

/// 动作受理结论（矩阵纯函数输出）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// 202 受理：body 中返回的目标 state（§4.2 括号内取值）
    Accepted(UpdateState),
    /// 同步拒绝：业务错误码
    Rejected(UpdateErrorCode),
    /// install：进入 §4.3 门禁判定（判定依赖 workload/controller，不在纯矩阵内）
    InstallGate,
}

/// §4.2 状态×动作受理矩阵（冻结）。
///
/// `manual_install_priority`：`waiting` 态手动 install 优先于维护窗口等待，
/// 但仍需过 §4.3 门禁——两种取值都落入 [`Admission::InstallGate`]，此处参数
/// 仅为矩阵完备性显式化（auto 协调器复用同函数时传 false）。
pub fn admit(state: UpdateState, action: UpdateAction) -> Admission {
    use UpdateAction::*;
    use UpdateErrorCode as E;
    use UpdateState as S;
    match (state, action) {
        // manual_recovery：一切动作被拒（§5.1 唯一无自动迁出终态）
        (S::ManualRecovery, _) => Admission::Rejected(E::ManualRecoveryRequired),
        // installing / restarting / rolling_back：事务进行中，全部 update_busy
        (S::Installing | S::Restarting | S::RollingBack, _) => Admission::Rejected(E::UpdateBusy),

        // check 在除三态/manual_recovery 外均可受理（幂等重启检查）
        (_, Check) => Admission::Accepted(S::Checking),

        (S::Idle, Download) | (S::Idle, Install) => Admission::Rejected(E::UpdateNotAvailable),
        (S::Idle, Rollback) => Admission::Rejected(E::RollbackUnavailable),

        (S::Checking, Download) | (S::Checking, Install) | (S::Checking, Rollback) => {
            Admission::Rejected(E::UpdateBusy)
        }

        (S::Available, Download) => Admission::Accepted(S::Downloading),
        (S::Available, Install) => Admission::Rejected(E::UpdateNotReady),
        (S::Available, Rollback) => Admission::Rejected(E::RollbackUnavailable),

        (S::Downloading, Download) => Admission::Accepted(S::Downloading),
        (S::Downloading, Install) | (S::Downloading, Rollback) => {
            Admission::Rejected(E::UpdateBusy)
        }

        (S::Staged | S::Waiting, Download) => Admission::Accepted(S::Staged),
        (S::Staged | S::Waiting, Install) => Admission::InstallGate,
        (S::Staged | S::Waiting, Rollback) => Admission::Accepted(S::RollingBack),

        (S::Failed, Download) => Admission::Accepted(S::Downloading),
        (S::Failed, Install) => Admission::InstallGate,
        (S::Failed, Rollback) => Admission::Accepted(S::RollingBack),
    }
}

/// 单元测试
#[cfg(test)]
mod tests {
    use super::*;

    fn states() -> [UpdateState; 11] {
        [
            UpdateState::Idle,
            UpdateState::Checking,
            UpdateState::Available,
            UpdateState::Downloading,
            UpdateState::Staged,
            UpdateState::Waiting,
            UpdateState::Installing,
            UpdateState::Restarting,
            UpdateState::Failed,
            UpdateState::RollingBack,
            UpdateState::ManualRecovery,
        ]
    }

    #[test]
    fn state_roundtrip_and_detail_mapping() {
        for s in states() {
            assert_eq!(UpdateState::parse(s.as_str()), Some(s));
        }
        assert_eq!(UpdateState::parse("nope"), None);
        // 抽查 §5.2 映射表
        assert!(UpdateState::Idle.allows_detail("committed"));
        assert!(UpdateState::Idle.allows_detail("cleaning"));
        assert!(!UpdateState::Idle.allows_detail("staged"));
        assert!(UpdateState::Available.allows_detail("checked"));
        assert!(!UpdateState::Available.allows_detail("downloading"));
        assert!(UpdateState::Installing.allows_detail("snapshot_verified"));
        assert!(UpdateState::Restarting.allows_detail("activating"));
        assert!(UpdateState::ManualRecovery.allows_detail("manual_recovery_required"));
    }

    #[test]
    fn error_code_http_status_mapping_is_frozen() {
        assert_eq!(UpdateErrorCode::UpdateNotManaged.http_status(), 409);
        assert_eq!(UpdateErrorCode::UpdateBusy.http_status(), 409);
        assert_eq!(UpdateErrorCode::UpdateNotAvailable.http_status(), 409);
        assert_eq!(UpdateErrorCode::UpdateNotReady.http_status(), 409);
        assert_eq!(UpdateErrorCode::SignatureInvalid.http_status(), 422);
        assert_eq!(UpdateErrorCode::ArtifactInvalid.http_status(), 422);
        assert_eq!(UpdateErrorCode::SchemaIncompatible.http_status(), 422);
        assert_eq!(UpdateErrorCode::InsufficientSpace.http_status(), 507);
        assert_eq!(UpdateErrorCode::LauncherUnreachable.http_status(), 502);
        assert_eq!(UpdateErrorCode::RollbackUnavailable.http_status(), 409);
        assert_eq!(UpdateErrorCode::ManualRecoveryRequired.http_status(), 409);
    }

    #[test]
    fn action_matrix_matches_contract_4_2() {
        use Admission::{Accepted, InstallGate, Rejected};
        let a = |s, op| admit(s, op);
        // idle
        assert_eq!(
            a(UpdateState::Idle, UpdateAction::Check),
            Accepted(UpdateState::Checking)
        );
        assert_eq!(
            a(UpdateState::Idle, UpdateAction::Download),
            Rejected(UpdateErrorCode::UpdateNotAvailable)
        );
        assert_eq!(
            a(UpdateState::Idle, UpdateAction::Install),
            Rejected(UpdateErrorCode::UpdateNotAvailable)
        );
        assert_eq!(
            a(UpdateState::Idle, UpdateAction::Rollback),
            Rejected(UpdateErrorCode::RollbackUnavailable)
        );
        // checking
        assert_eq!(
            a(UpdateState::Checking, UpdateAction::Check),
            Accepted(UpdateState::Checking)
        );
        for op in [
            UpdateAction::Download,
            UpdateAction::Install,
            UpdateAction::Rollback,
        ] {
            assert_eq!(
                a(UpdateState::Checking, op),
                Rejected(UpdateErrorCode::UpdateBusy)
            );
        }
        // available
        assert_eq!(
            a(UpdateState::Available, UpdateAction::Check),
            Accepted(UpdateState::Checking)
        );
        assert_eq!(
            a(UpdateState::Available, UpdateAction::Download),
            Accepted(UpdateState::Downloading)
        );
        assert_eq!(
            a(UpdateState::Available, UpdateAction::Install),
            Rejected(UpdateErrorCode::UpdateNotReady)
        );
        assert_eq!(
            a(UpdateState::Available, UpdateAction::Rollback),
            Rejected(UpdateErrorCode::RollbackUnavailable)
        );
        // downloading
        assert_eq!(
            a(UpdateState::Downloading, UpdateAction::Check),
            Accepted(UpdateState::Checking)
        );
        assert_eq!(
            a(UpdateState::Downloading, UpdateAction::Download),
            Accepted(UpdateState::Downloading)
        );
        assert_eq!(
            a(UpdateState::Downloading, UpdateAction::Install),
            Rejected(UpdateErrorCode::UpdateBusy)
        );
        assert_eq!(
            a(UpdateState::Downloading, UpdateAction::Rollback),
            Rejected(UpdateErrorCode::UpdateBusy)
        );
        // staged / waiting
        for s in [UpdateState::Staged, UpdateState::Waiting] {
            assert_eq!(a(s, UpdateAction::Check), Accepted(UpdateState::Checking));
            assert_eq!(a(s, UpdateAction::Download), Accepted(UpdateState::Staged));
            assert_eq!(a(s, UpdateAction::Install), InstallGate);
            assert_eq!(
                a(s, UpdateAction::Rollback),
                Accepted(UpdateState::RollingBack)
            );
        }
        // installing / restarting / rolling_back：全 busy
        for s in [
            UpdateState::Installing,
            UpdateState::Restarting,
            UpdateState::RollingBack,
        ] {
            for op in [
                UpdateAction::Check,
                UpdateAction::Download,
                UpdateAction::Install,
                UpdateAction::Rollback,
            ] {
                assert_eq!(
                    a(s, op),
                    Rejected(UpdateErrorCode::UpdateBusy),
                    "{s:?} {op:?}"
                );
            }
        }
        // failed
        assert_eq!(
            a(UpdateState::Failed, UpdateAction::Check),
            Accepted(UpdateState::Checking)
        );
        assert_eq!(
            a(UpdateState::Failed, UpdateAction::Download),
            Accepted(UpdateState::Downloading)
        );
        assert_eq!(a(UpdateState::Failed, UpdateAction::Install), InstallGate);
        assert_eq!(
            a(UpdateState::Failed, UpdateAction::Rollback),
            Accepted(UpdateState::RollingBack)
        );
        // manual_recovery：全部拒绝
        for op in [
            UpdateAction::Check,
            UpdateAction::Download,
            UpdateAction::Install,
            UpdateAction::Rollback,
        ] {
            assert_eq!(
                a(UpdateState::ManualRecovery, op),
                Rejected(UpdateErrorCode::ManualRecoveryRequired)
            );
        }
    }
}
