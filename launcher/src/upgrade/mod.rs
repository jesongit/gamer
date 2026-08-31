//! LCH-010/011/012：升级状态机编排、离线快照与恢复、候选启动/提交/回滚。
//!
//! 状态机（计划 §6.6）：`idle → checking → downloading → verifying → staged →
//! waiting_idle → draining → stopped → snapshotting → snapshot_verified →
//! migrating → switched → candidate_starting → candidate_ready → activating →
//! committed → cleaning → idle`。每个持久边先原子写 journal 意图、再执行动作。
//! journal 复用 `state::UpdateJournal`（18 态枚举 + last_step 细粒度步骤）；
//! HTTP 展示态（system-api §5.1 的 11 态）由 [`display_state`] 派生。

pub mod engine;
pub mod httpc;
pub mod recovery;
pub mod snapshot;
pub mod trampoline;

use crate::state::{UpdateJournal, UpdateState};
use crate::winutil;

/// 业务错误码：与 HTTP API / IPC 共享的 11 个统一错误码（system-api-v1.md §7）。
pub mod codes {
    pub const UPDATE_NOT_MANAGED: &str = "update_not_managed";
    pub const UPDATE_BUSY: &str = "update_busy";
    pub const UPDATE_NOT_AVAILABLE: &str = "update_not_available";
    pub const UPDATE_NOT_READY: &str = "update_not_ready";
    pub const SIGNATURE_INVALID: &str = "signature_invalid";
    pub const ARTIFACT_INVALID: &str = "artifact_invalid";
    pub const INSUFFICIENT_SPACE: &str = "insufficient_space";
    pub const SCHEMA_INCOMPATIBLE: &str = "schema_incompatible";
    pub const LAUNCHER_UNREACHABLE: &str = "launcher_unreachable";
    pub const ROLLBACK_UNAVAILABLE: &str = "rollback_unavailable";
    pub const MANUAL_RECOVERY_REQUIRED: &str = "manual_recovery_required";
    pub const LAUNCHER_TOO_OLD: &str = "launcher-too-old";
}

/// 业务错误（journal.error / IPC 错误帧 / outcome 共用形态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusinessError {
    pub code: String,
    pub message: String,
}

impl BusinessError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// 当前 launcher 是否满足 release manifest 声明的最低版本。
///
/// manifest 在签名和结构校验阶段已经验证过版本格式；这里仍然对格式错误
/// fail closed，供升级引擎和 trampoline 入口共用同一门禁语义。
pub fn check_minimum_launcher_version(minimum: &str) -> Result<(), BusinessError> {
    let current = env!("CARGO_PKG_VERSION");
    match (
        crate::manifest::semver::parse(current),
        crate::manifest::semver::parse(minimum),
    ) {
        (Some(current), Some(minimum)) if !crate::manifest::semver::is_lt(&current, &minimum) => {
            Ok(())
        }
        (Some(_), Some(_)) => Err(BusinessError::new(
            codes::LAUNCHER_TOO_OLD,
            format!("launcher {current} 低于 manifest 要求的最低版本 {minimum}，请先升级 launcher"),
        )),
        _ => Err(BusinessError::new(
            codes::SCHEMA_INCOMPATIBLE,
            format!("最低 launcher 版本不是合法 SemVer: {minimum}"),
        )),
    }
}

impl std::fmt::Display for BusinessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// journal → 前端展示态（system-api §5.1 的 11 态；launcher 内部派生规则，
/// server 经 IPC status 拿到的即此值，1:1 透传）。
pub fn display_state(j: &UpdateJournal) -> &'static str {
    if j.state == UpdateState::ManualRecoveryRequired {
        return "manual_recovery";
    }
    if j.last_step.as_deref() == Some("rolling_back") {
        return "rolling_back";
    }
    match j.state {
        UpdateState::Idle => {
            if j.last_step.as_deref() == Some("failed") {
                "failed"
            } else {
                "idle"
            }
        }
        UpdateState::Checking => {
            if j.last_step.as_deref() == Some("checked") {
                "available"
            } else {
                "checking"
            }
        }
        UpdateState::Downloading | UpdateState::Verifying => "downloading",
        UpdateState::Staged => "staged",
        UpdateState::WaitingIdle => "waiting",
        UpdateState::Draining
        | UpdateState::Stopped
        | UpdateState::Snapshotting
        | UpdateState::SnapshotVerified
        | UpdateState::Migrating
        | UpdateState::Switched => "installing",
        UpdateState::CandidateStarting | UpdateState::CandidateReady | UpdateState::Activating => {
            "restarting"
        }
        UpdateState::Committed | UpdateState::Cleaning => "idle",
        UpdateState::ManualRecoveryRequired => "manual_recovery",
    }
}

/// journal → 细粒度 detail（system-api §5.2 的冻结映射来源）。
pub fn display_detail(j: &UpdateJournal) -> String {
    j.last_step
        .clone()
        .unwrap_or_else(|| j.state.as_str().to_string())
}

/// 新升级事务 id：`upd-<unix-ms>-<4hex>`（fixture 形态近似；全局唯一即可）。
pub fn new_update_id() -> String {
    let suffix = winutil::random_bytes(2)
        .map(|b| crate::digest::to_hex(&b))
        .unwrap_or_else(|_| "0000".to_string());
    format!("upd-{}-{suffix}", crate::state::atomic::now_unix_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journal(state: UpdateState, last_step: Option<&str>) -> UpdateJournal {
        UpdateJournal {
            state,
            last_step: last_step.map(str::to_string),
            ..UpdateJournal::default()
        }
    }

    #[test]
    fn display_mapping_matches_frozen_11_state_table() {
        // idle 系
        assert_eq!(display_state(&journal(UpdateState::Idle, None)), "idle");
        assert_eq!(
            display_state(&journal(UpdateState::Idle, Some("failed"))),
            "failed"
        );
        assert_eq!(
            display_state(&journal(UpdateState::Committed, Some("committed"))),
            "idle"
        );
        assert_eq!(
            display_state(&journal(UpdateState::Cleaning, Some("cleaning"))),
            "idle"
        );
        // checking / available
        assert_eq!(
            display_state(&journal(UpdateState::Checking, Some("checking"))),
            "checking"
        );
        assert_eq!(
            display_state(&journal(UpdateState::Checking, Some("checked"))),
            "available"
        );
        // downloading（含 verifying detail）
        assert_eq!(
            display_state(&journal(UpdateState::Downloading, Some("downloading"))),
            "downloading"
        );
        assert_eq!(
            display_state(&journal(UpdateState::Verifying, Some("verifying"))),
            "downloading"
        );
        assert_eq!(
            display_state(&journal(UpdateState::Staged, Some("staged"))),
            "staged"
        );
        assert_eq!(
            display_state(&journal(UpdateState::WaitingIdle, Some("waiting_idle"))),
            "waiting"
        );
        // installing
        for st in [
            UpdateState::Draining,
            UpdateState::Stopped,
            UpdateState::Snapshotting,
            UpdateState::SnapshotVerified,
            UpdateState::Migrating,
            UpdateState::Switched,
        ] {
            assert_eq!(display_state(&journal(st, None)), "installing", "{st:?}");
        }
        // restarting
        for st in [
            UpdateState::CandidateStarting,
            UpdateState::CandidateReady,
            UpdateState::Activating,
        ] {
            assert_eq!(display_state(&journal(st, None)), "restarting", "{st:?}");
        }
        // rolling_back / manual
        assert_eq!(
            display_state(&journal(
                UpdateState::SnapshotVerified,
                Some("rolling_back")
            )),
            "rolling_back"
        );
        assert_eq!(
            display_state(&journal(UpdateState::ManualRecoveryRequired, None)),
            "manual_recovery"
        );
    }

    #[test]
    fn detail_falls_back_to_state_name() {
        assert_eq!(display_detail(&journal(UpdateState::Idle, None)), "idle");
        assert_eq!(
            display_detail(&journal(UpdateState::Staged, Some("staged"))),
            "staged"
        );
    }

    #[test]
    fn minimum_launcher_version_gate_rejects_newer_requirement() {
        let err = check_minimum_launcher_version("999.0.0").expect_err("当前 launcher 应过低");
        assert_eq!(err.code, codes::LAUNCHER_TOO_OLD);
        assert!(err.message.contains("999.0.0"));
    }

    #[test]
    fn minimum_launcher_version_gate_accepts_current_requirement() {
        check_minimum_launcher_version(env!("CARGO_PKG_VERSION"))
            .expect("当前版本应满足同版本最低门禁");
    }
}
