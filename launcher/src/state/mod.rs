//! LCH-002：`state/` 原子 IO（临时文件 + 同目录 rename，契约 §5.1）与状态结构。

pub mod atomic;
pub mod lock;

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::state::atomic::LoadOutcome;

pub const STATE_DIR: &str = "state";
pub const CURRENT_FILE: &str = "current.json";
pub const JOURNAL_FILE: &str = "update-journal.json";
pub const LOCK_FILE: &str = "launcher.lock";

/// `state/current.json` / `state/update-journal.json` 的结构版本；字段集按本批冻结。
pub const STATE_SCHEMA_VERSION: u32 = 1;

/// 当前版本指针（唯一切换入口）。字段集为本批提案，后续以 LCH-002 后续 fixture 冻结。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentState {
    pub schema_version: u32,
    /// 当前应用版本（versions/<semver>/）
    pub current: String,
    /// 上一版本（回滚点），首装为 null
    #[serde(default)]
    pub previous: Option<String>,
    #[serde(default)]
    pub updated_at_unix_ms: Option<u64>,
}

impl CurrentState {
    pub fn new(current: impl Into<String>, previous: Option<String>) -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            current: current.into(),
            previous,
            updated_at_unix_ms: Some(atomic::now_unix_millis()),
        }
    }
}

/// 升级状态机状态（计划 §6.6），序列化为 snake_case。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateState {
    Idle,
    Checking,
    Downloading,
    Verifying,
    Staged,
    WaitingIdle,
    Draining,
    Stopped,
    Snapshotting,
    SnapshotVerified,
    Migrating,
    Switched,
    CandidateStarting,
    CandidateReady,
    Activating,
    Committed,
    Cleaning,
    ManualRecoveryRequired,
}

impl UpdateState {
    pub fn as_str(&self) -> &'static str {
        match self {
            UpdateState::Idle => "idle",
            UpdateState::Checking => "checking",
            UpdateState::Downloading => "downloading",
            UpdateState::Verifying => "verifying",
            UpdateState::Staged => "staged",
            UpdateState::WaitingIdle => "waiting_idle",
            UpdateState::Draining => "draining",
            UpdateState::Stopped => "stopped",
            UpdateState::Snapshotting => "snapshotting",
            UpdateState::SnapshotVerified => "snapshot_verified",
            UpdateState::Migrating => "migrating",
            UpdateState::Switched => "switched",
            UpdateState::CandidateStarting => "candidate_starting",
            UpdateState::CandidateReady => "candidate_ready",
            UpdateState::Activating => "activating",
            UpdateState::Committed => "committed",
            UpdateState::Cleaning => "cleaning",
            UpdateState::ManualRecoveryRequired => "manual_recovery_required",
        }
    }
}

/// 被监管子进程的精确信息（计划 §6.6：准确 child PID/创建时间/exe）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChildInfo {
    pub pid: u32,
    #[serde(default)]
    pub created_at_unix_ms: Option<u64>,
    pub exe: String,
}

/// 升级前数据/配置快照的定位信息（快照本体与逐文件 hash 属 LCH-011）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub id: String,
    pub path: String,
    pub file_count: u64,
    pub total_bytes: u64,
}

/// 错误摘要（journal 只记码与摘要，不落敏感值）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct JournalError {
    pub code: String,
    pub message: String,
}

/// `state/update-journal.json`：升级状态机持久 journal（计划 §6.6）。
/// 本批只冻结类型与原子 IO；状态机编排在 LCH-010。
/// 用法约定：每个动作先原子记录意图（改 state），再执行动作，完成后推进状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateJournal {
    pub schema_version: u32,
    #[serde(default)]
    pub update_id: Option<String>,
    pub state: UpdateState,
    #[serde(default)]
    pub from_version: Option<String>,
    #[serde(default)]
    pub to_version: Option<String>,
    #[serde(default)]
    pub child: Option<ChildInfo>,
    #[serde(default)]
    pub current_version: Option<String>,
    #[serde(default)]
    pub previous_version: Option<String>,
    #[serde(default)]
    pub snapshot: Option<SnapshotInfo>,
    #[serde(default)]
    pub data_schema_before: Option<u32>,
    #[serde(default)]
    pub data_schema_after: Option<u32>,
    /// 最后完成的步骤名
    #[serde(default)]
    pub last_step: Option<String>,
    #[serde(default)]
    pub error: Option<JournalError>,
    #[serde(default)]
    pub updated_at_unix_ms: Option<u64>,
}

impl Default for UpdateJournal {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            update_id: None,
            state: UpdateState::Idle,
            from_version: None,
            to_version: None,
            child: None,
            current_version: None,
            previous_version: None,
            snapshot: None,
            data_schema_before: None,
            data_schema_after: None,
            last_step: None,
            error: None,
            updated_at_unix_ms: None,
        }
    }
}

/// journal 载入结果；`reset_from` 非空表示原文件损坏/版本不识别，已备份并回空状态。
#[derive(Debug, Clone)]
pub struct JournalLoad {
    pub journal: UpdateJournal,
    pub reset_from: Option<PathBuf>,
}

/// 安装根 state/ 目录的读写入口。所有写入均为原子替换，半截 JSON 可恢复。
pub struct StateStore {
    root: PathBuf,
}

impl StateStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn state_dir(&self) -> PathBuf {
        self.root.join(STATE_DIR)
    }

    pub fn current_path(&self) -> PathBuf {
        self.state_dir().join(CURRENT_FILE)
    }

    pub fn journal_path(&self) -> PathBuf {
        self.state_dir().join(JOURNAL_FILE)
    }

    pub fn lock_path(&self) -> PathBuf {
        self.state_dir().join(LOCK_FILE)
    }

    /// 读取 current.json；Missing=未安装，Corrupted=已把损坏文件备份到 .corrupt-<ts>。
    /// 调用方（status）对 Corrupted 报错而非崩溃。
    pub fn load_current(&self) -> io::Result<LoadOutcome<CurrentState>> {
        atomic::load_json_recover(&self.current_path())
    }

    pub fn write_current(&self, state: &CurrentState) -> io::Result<()> {
        atomic::write_json_atomic(&self.current_path(), state)
    }

    /// 读取 journal；损坏或 schema 版本不识别 → 备份后返回默认空状态（fail closed 不再续用）。
    pub fn load_journal(&self) -> io::Result<JournalLoad> {
        match atomic::load_json_recover::<UpdateJournal>(&self.journal_path())? {
            LoadOutcome::Present(j) if j.schema_version == STATE_SCHEMA_VERSION => {
                Ok(JournalLoad {
                    journal: j,
                    reset_from: None,
                })
            }
            LoadOutcome::Present(_) => {
                let backup = atomic::backup_to_corrupt(&self.journal_path())?;
                Ok(JournalLoad {
                    journal: UpdateJournal::default(),
                    reset_from: Some(backup),
                })
            }
            LoadOutcome::Missing => Ok(JournalLoad {
                journal: UpdateJournal::default(),
                reset_from: None,
            }),
            LoadOutcome::Corrupted { backup_path } => Ok(JournalLoad {
                journal: UpdateJournal::default(),
                reset_from: Some(backup_path),
            }),
        }
    }

    pub fn write_journal(&self, journal: &UpdateJournal) -> io::Result<()> {
        atomic::write_json_atomic(&self.journal_path(), journal)
    }
}
