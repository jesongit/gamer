//! LCH-010：launcher 启动时扫描未完成 journal，按计划 §6.6 失败分支恢复。
//!
//! 恢复只整理磁盘与 journal，不拉起进程（旧版本由随后的 start 流程启动）。
//! 每个持久边的恢复策略（QA-004 断电矩阵的实现依据）：
//!
//! | journal 状态 | 恢复策略 | 终态 |
//! |---|---|---|
//! | idle/无 journal | 不动 | — |
//! | checking / checked | staging 完整→staged；否则 aborted→idle | 旧版健康 |
//! | downloading / verifying | 同上（半截 staging 复验必败） | 旧版健康 |
//! | staged / waiting_idle | staging 完整→staged（可续）；否则→idle | 旧版健康 |
//! | draining | 卡住默认取消→staged/idle（不硬杀） | 旧版健康 |
//! | stopped | 未快照、数据未改→idle（旧版本由 start 重启） | 旧版健康 |
//! | snapshotting | 快照完整→回 idle/failed；半截→丢弃继续→idle（数据未改） | 旧版健康 |
//! | snapshot_verified / migrating | 未切换：数据未改→idle/failed | 旧版健康 |
//! | switched..activating | 停孤儿候选（PID+exe 精确）→恢复快照→切回 previous | 旧版健康 |
//! | committed / cleaning | 校验 current.json→to，清 staging→idle | 新版健康 |
//! | manual_recovery_required | 不动（停止自动重试） | 人工 |
//!
//! 回滚路径任一步失败 → manual_recovery_required（journal 落盘）。

use std::fs;

use serde_json::Value;

use crate::layout::InstallLayout;
use crate::manifest::model::Manifest;
use crate::repair::{verify_app_dir, AppInstallSpec};
use crate::state::atomic::LoadOutcome;
use crate::state::{StateStore, UpdateJournal, UpdateState};

use super::engine::{Engine, UpgradeOptions};
use super::snapshot;

/// 恢复终态（QA-004 断言口径）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// 无未完成事务。
    NothingToDo,
    /// 回到 idle + failed 记录（旧版健康，事务作废）。
    Aborted { reason: String },
    /// 候选 staging 完整，驻留 staged（旧版健康，可续装）。
    StagedResumable { reason: String },
    /// 走了回滚路径（停候选/恢复快照/切回），旧版健康。
    RolledBack { reason: String },
    /// committed/cleaning 事后收尾完成，新版健康。
    CommittedFinished { version: String },
    /// 回滚也失败：manual_recovery_required，停止自动重试。
    ManualRequired { reason: String },
}

#[derive(Debug, Clone)]
pub struct RecoveryReport {
    pub from: Option<UpdateState>,
    pub outcome: RecoveryOutcome,
}

impl RecoveryReport {
    pub fn is_manual(&self) -> bool {
        matches!(self.outcome, RecoveryOutcome::ManualRequired { .. })
    }
}

/// 启动恢复入口（start / IPC serve 前调用；QA-004 矩阵逐状态驱动它）。
pub fn recover_on_startup(
    layout: &InstallLayout,
    store: &StateStore,
) -> std::io::Result<RecoveryReport> {
    let journal_load = store.load_journal()?;
    let journal = journal_load.journal;
    let from = if journal_load.reset_from.is_some() {
        None
    } else {
        Some(journal.state)
    };
    let engine = Engine::new(layout.clone(), UpgradeOptions::default());
    let report = match journal.state {
        UpdateState::Idle => RecoveryReport {
            from,
            outcome: RecoveryOutcome::NothingToDo,
        },
        // 这是唯一不允许启动恢复自动迁出的终态；调用方必须看到
        // ManualRequired 并拒绝拉起 server/继续 upgrade。
        UpdateState::ManualRecoveryRequired => RecoveryReport {
            from,
            outcome: RecoveryOutcome::ManualRequired {
                reason: journal
                    .error
                    .as_ref()
                    .map(|e| e.message.clone())
                    .unwrap_or_else(|| "journal 要求人工恢复".to_string()),
            },
        },
        UpdateState::Checking
        | UpdateState::Downloading
        | UpdateState::Verifying
        | UpdateState::Staged
        | UpdateState::WaitingIdle
        | UpdateState::Draining => recover_pre_install(&engine, store, &journal),
        UpdateState::Stopped => {
            // 未快照、数据未改：旧版本由 start 重启
            finish_idle(
                store,
                &journal,
                "停机后未及快照即中断，已回退（旧版本将随后启动）",
            )
        }
        UpdateState::Snapshotting => recover_snapshotting(&engine, store, &journal),
        UpdateState::SnapshotVerified | UpdateState::Migrating => {
            // 未切换（current.json 仍指旧版本）：数据未改，无需恢复
            if pointer_matches(store, journal.from_version.as_deref()) {
                finish_idle(store, &journal, "快照后/迁移中中断，数据未切换，已回退")
            } else {
                recover_switched(&engine, store, &journal, "切换后中断")
            }
        }
        UpdateState::Switched
        | UpdateState::CandidateStarting
        | UpdateState::CandidateReady
        | UpdateState::Activating => recover_switched(&engine, store, &journal, "候选阶段中断"),
        UpdateState::Committed | UpdateState::Cleaning => {
            recover_committed(store, layout, &journal)
        }
    };
    Ok(report)
}

fn finish_idle(store: &StateStore, journal: &UpdateJournal, reason: &str) -> RecoveryReport {
    let mut j = journal.clone();
    j.state = UpdateState::Idle;
    j.last_step = Some("failed".to_string());
    j.error = Some(crate::state::JournalError {
        code: "artifact_invalid".to_string(),
        message: reason.to_string(),
    });
    let _ = store.write_journal(&j);
    RecoveryReport {
        from: Some(journal.state),
        outcome: RecoveryOutcome::Aborted {
            reason: reason.to_string(),
        },
    }
}

fn finish_manual(store: &StateStore, journal: &UpdateJournal, reason: &str) -> RecoveryReport {
    let mut j = journal.clone();
    j.state = UpdateState::ManualRecoveryRequired;
    j.last_step = Some("manual_recovery_required".to_string());
    j.error = Some(crate::state::JournalError {
        code: "rollback_unavailable".to_string(),
        message: reason.to_string(),
    });
    let _ = store.write_journal(&j);
    RecoveryReport {
        from: Some(journal.state),
        outcome: RecoveryOutcome::ManualRequired {
            reason: reason.to_string(),
        },
    }
}

/// staging/<update-id>/app 是否完整（entrypoint + scrcpy jar 校验）。
fn staging_intact(layout: &InstallLayout, journal: &UpdateJournal) -> bool {
    let (Some(update_id), Some(to_version)) = (&journal.update_id, &journal.to_version) else {
        return false;
    };
    let app_dir = layout.staging_dir().join(update_id).join("app");
    if !app_dir.is_dir() {
        return false;
    }
    let manifest_path = layout.manifests_dir().join(format!("{to_version}.json"));
    let Ok(raw) = fs::read(&manifest_path) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&raw) else {
        return false;
    };
    let Ok(manifest) = Manifest::parse(&value) else {
        return false;
    };
    let Some(platform) = manifest.platforms.get("windows-x86_64") else {
        return false;
    };
    let Ok(app) = AppInstallSpec::from_model(platform, to_version) else {
        return false;
    };
    verify_app_dir(&app_dir, &app).is_ok()
}

fn recover_pre_install(
    engine: &Engine,
    store: &StateStore,
    journal: &UpdateJournal,
) -> RecoveryReport {
    let draining = journal.state == UpdateState::Draining;
    if staging_intact(&engine.layout, journal) {
        let mut j = journal.clone();
        j.state = UpdateState::Staged;
        j.last_step = Some("staged".to_string());
        j.child = None;
        if draining {
            j.error = Some(crate::state::JournalError {
                code: "update_busy".to_string(),
                message: "draining 中断，按契约默认取消升级（可续装或回滚）".to_string(),
            });
        } else {
            j.error = None;
        }
        let _ = store.write_journal(&j);
        RecoveryReport {
            from: Some(journal.state),
            outcome: RecoveryOutcome::StagedResumable {
                reason: "候选 staging 完整，驻留 staged".to_string(),
            },
        }
    } else {
        // 半截 staging / 无候选：事务作废，回 idle（旧版未动）
        if let Some(update_id) = &journal.update_id {
            let _ = fs::remove_dir_all(engine.layout.staging_dir().join(update_id));
        }
        finish_idle(
            store,
            journal,
            "启动恢复：事务未完成且 staging 不完整，已回退（旧版本未受影响）",
        )
    }
}

fn recover_snapshotting(
    engine: &Engine,
    store: &StateStore,
    journal: &UpdateJournal,
) -> RecoveryReport {
    let Some(update_id) = journal.update_id.clone() else {
        return finish_idle(store, journal, "快照阶段中断且无事务 id，已回退");
    };
    // 快照完整 → 走标准回滚收尾（数据未切换，等价于回 idle/failed）
    if snapshot::verify(&engine.layout, &update_id, None, false).is_ok() {
        match engine.rollback_procedure(journal, None, false) {
            Ok(()) => RecoveryReport {
                from: Some(journal.state),
                outcome: RecoveryOutcome::RolledBack {
                    reason: "快照阶段中断，快照完整已登记，回滚收尾完成".to_string(),
                },
            },
            Err(err) => finish_manual(store, journal, &format!("快照阶段回滚失败: {err}")),
        }
    } else {
        // 半截快照：数据未改（快照只读 data/），丢弃半截目录回 idle
        let _ = fs::remove_dir_all(snapshot::backup_dir(&engine.layout, &update_id));
        finish_idle(
            store,
            journal,
            "快照阶段中断且快照不完整，数据未改动，已回退",
        )
    }
}

fn recover_switched(
    engine: &Engine,
    store: &StateStore,
    journal: &UpdateJournal,
    context: &str,
) -> RecoveryReport {
    if journal.snapshot.is_none() {
        return finish_manual(
            store,
            journal,
            &format!("{context}且无快照信息，无法自动恢复"),
        );
    }
    match engine.rollback_procedure(journal, None, false) {
        Ok(()) => RecoveryReport {
            from: Some(journal.state),
            outcome: RecoveryOutcome::RolledBack {
                reason: format!("{context}，已停候选/恢复快照/切回旧版本"),
            },
        },
        Err(err) => finish_manual(store, journal, &format!("{context}，回滚失败: {err}")),
    }
}

fn recover_committed(
    store: &StateStore,
    layout: &InstallLayout,
    journal: &UpdateJournal,
) -> RecoveryReport {
    let Some(to) = journal.to_version.clone() else {
        return finish_manual(store, journal, "committed 后缺 to_version，状态不可判定");
    };
    if !pointer_matches(store, Some(to.as_str())) {
        return finish_manual(
            store,
            journal,
            "committed 但 current.json 未指向新版本，状态不可判定",
        );
    }
    if let Some(update_id) = &journal.update_id {
        let _ = fs::remove_dir_all(layout.staging_dir().join(update_id));
    }
    let mut j = journal.clone();
    j.state = UpdateState::Idle;
    j.last_step = Some("idle".to_string());
    j.error = None;
    let _ = store.write_journal(&j);
    RecoveryReport {
        from: Some(journal.state),
        outcome: RecoveryOutcome::CommittedFinished { version: to },
    }
}

fn pointer_matches(store: &StateStore, version: Option<&str>) -> bool {
    let Some(expected) = version else {
        return false;
    };
    matches!(
        store.load_current(),
        Ok(LoadOutcome::Present(c)) if c.current == expected
    )
}
