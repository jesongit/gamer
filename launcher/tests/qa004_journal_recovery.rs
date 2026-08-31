//! QA-004 / LCH-010：每个 launcher journal 持久化边的启动恢复矩阵。
//!
//! 测试只把状态和动作结果写入临时安装根，再用新的 StateStore 读取，模拟
//! launcher 重启；不杀进程、不伪造断电，也不依赖真实 server/候选进程。

mod common;

use std::fs;

use common::{cleanup, sha256_hex, unique_root};
use gamer_launcher::layout::InstallLayout;
use gamer_launcher::state::{
    CurrentState, JournalError, SnapshotInfo, StateStore, UpdateJournal, UpdateState,
};
use gamer_launcher::upgrade::recovery::{recover_on_startup, RecoveryOutcome};
use gamer_launcher::upgrade::snapshot;

const OLD_VERSION: &str = "0.1.0";
const NEW_VERSION: &str = "0.2.0";

fn new_layout(tag: &str) -> InstallLayout {
    InstallLayout {
        root: unique_root(tag),
    }
}

fn journal(state: UpdateState) -> UpdateJournal {
    UpdateJournal {
        update_id: Some("upd-qa004".to_string()),
        state,
        from_version: Some(OLD_VERSION.to_string()),
        to_version: Some(NEW_VERSION.to_string()),
        last_step: Some(state.as_str().to_string()),
        ..UpdateJournal::default()
    }
}

fn write_old_current(layout: &InstallLayout) {
    StateStore::new(&layout.root)
        .write_current(&CurrentState::new(OLD_VERSION, None))
        .expect("写入旧版本 current");
}

fn write_live_data(layout: &InstallLayout, value: &[u8]) {
    fs::create_dir_all(layout.data_dir()).expect("创建 data");
    fs::create_dir_all(layout.config_file().parent().expect("config 父目录")).expect("创建 config");
    fs::write(layout.data_dir().join("state.bin"), value).expect("写入业务数据");
    fs::write(layout.config_file(), b"port = 8443\n").expect("写入配置");
}

fn write_candidate_manifest_and_staging(layout: &InstallLayout, update_id: &str) {
    let jar = b"qa004 scrcpy-server fixture";
    let manifest = serde_json::json!({
        "schema_version": 1,
        "product": "gamer",
        "release": {
            "version": NEW_VERSION,
            "channel": "stable",
            "published_at": "2026-08-31T00:00:00Z",
            "minimum_launcher_version": "0.1.0",
            "minimum_upgrade_version": "0.1.0",
            "data_schema": 1,
            "rollback_floor": 1,
            "release_notes_url": "https://example.com/releases/0.2.0"
        },
        "platforms": {
            "windows-x86_64": {
                "app": {
                    "artifact": {
                        "name": "app.zip",
                        "url": "https://example.com/app.zip",
                        "size": 1,
                        "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
                    },
                    "entrypoint": "gamer-server.exe"
                },
                "components": [],
                "resources": {
                    "scrcpy_server": {
                        "version": "3.3.3",
                        "path": "assets/scrcpy-server.jar",
                        "sha256": sha256_hex(jar),
                        "binding": "app"
                    }
                }
            }
        }
    });
    fs::create_dir_all(layout.manifests_dir()).expect("创建 manifests");
    fs::write(
        layout.manifests_dir().join(format!("{NEW_VERSION}.json")),
        serde_json::to_vec_pretty(&manifest).expect("序列化 manifest"),
    )
    .expect("写入 manifest");

    let app = layout.staging_dir().join(update_id).join("app");
    fs::create_dir_all(app.join("assets")).expect("创建候选 staging");
    fs::write(app.join("gamer-server.exe"), b"fake candidate").expect("写入口");
    fs::write(app.join("assets/scrcpy-server.jar"), jar).expect("写 scrcpy jar");
}

fn persist_then_restart(
    layout: &InstallLayout,
    journal: &UpdateJournal,
) -> (
    gamer_launcher::upgrade::recovery::RecoveryReport,
    UpdateJournal,
) {
    StateStore::new(&layout.root)
        .write_journal(journal)
        .expect("持久化崩溃前 journal");

    // 新 StateStore 表示 launcher 重启后的全新读取边界。
    let restarted_store = StateStore::new(&layout.root);
    let report = recover_on_startup(layout, &restarted_store).expect("启动恢复不应 IO 失败");
    let persisted = StateStore::new(&layout.root)
        .load_journal()
        .expect("读取恢复后的 journal")
        .journal;
    (report, persisted)
}

fn assert_aborted(
    layout: &InstallLayout,
    state: UpdateState,
    report: &gamer_launcher::upgrade::recovery::RecoveryReport,
    persisted: &UpdateJournal,
) {
    assert_eq!(report.from, Some(state), "应报告原始 journal 状态");
    assert!(matches!(report.outcome, RecoveryOutcome::Aborted { .. }));
    assert_eq!(persisted.state, UpdateState::Idle);
    assert_eq!(persisted.last_step.as_deref(), Some("failed"));
    assert_eq!(
        persisted.error.as_ref().map(|e| e.code.as_str()),
        Some("artifact_invalid")
    );
    assert_eq!(
        StateStore::new(&layout.root)
            .load_current()
            .expect("读取 current")
            .present_current(),
        Some(OLD_VERSION.to_string()),
    );
}

fn current_version(layout: &InstallLayout) -> Option<String> {
    match StateStore::new(&layout.root)
        .load_current()
        .expect("读取 current")
    {
        gamer_launcher::state::atomic::LoadOutcome::Present(current) => Some(current.current),
        _ => None,
    }
}

trait CurrentOutcomeExt {
    fn present_current(self) -> Option<String>;
}

impl CurrentOutcomeExt for gamer_launcher::state::atomic::LoadOutcome<CurrentState> {
    fn present_current(self) -> Option<String> {
        match self {
            gamer_launcher::state::atomic::LoadOutcome::Present(current) => Some(current.current),
            _ => None,
        }
    }
}

fn snapshot_info(layout: &InstallLayout, update_id: &str) -> SnapshotInfo {
    let report = snapshot::create(layout, update_id, None).expect("创建快照");
    SnapshotInfo {
        id: report.id,
        path: report.path,
        file_count: report.file_count,
        total_bytes: report.total_bytes,
    }
}

fn switched_journal(state: UpdateState, snapshot: SnapshotInfo) -> UpdateJournal {
    UpdateJournal {
        snapshot: Some(snapshot),
        child: None,
        current_version: Some(NEW_VERSION.to_string()),
        ..journal(state)
    }
}

#[test]
fn upgrade_recovery_pre_install_states_resume_only_complete_staging() {
    let states = [
        UpdateState::Checking,
        UpdateState::Downloading,
        UpdateState::Verifying,
        UpdateState::Staged,
        UpdateState::WaitingIdle,
        UpdateState::Draining,
    ];

    for state in states {
        let layout = new_layout(&format!("pre-incomplete-{}", state.as_str()));
        write_old_current(&layout);
        let update_id = "upd-qa004";
        let incomplete = layout.staging_dir().join(update_id).join("app");
        fs::create_dir_all(&incomplete).expect("创建半截 staging");
        fs::write(incomplete.join("gamer-server.exe"), b"missing jar").expect("写半截入口");

        let (report, persisted) = persist_then_restart(&layout, &journal(state));
        assert_aborted(&layout, state, &report, &persisted);
        assert!(!layout.staging_dir().join(update_id).exists());
        cleanup(&layout.root);
    }

    for state in states {
        let layout = new_layout(&format!("pre-complete-{}", state.as_str()));
        write_old_current(&layout);
        write_candidate_manifest_and_staging(&layout, "upd-qa004");

        let (report, persisted) = persist_then_restart(&layout, &journal(state));
        assert_eq!(report.from, Some(state));
        assert!(matches!(
            report.outcome,
            RecoveryOutcome::StagedResumable { .. }
        ));
        assert_eq!(persisted.state, UpdateState::Staged);
        assert_eq!(persisted.last_step.as_deref(), Some("staged"));
        if state == UpdateState::Draining {
            assert_eq!(
                persisted.error.as_ref().map(|e| e.code.as_str()),
                Some("update_busy")
            );
        } else {
            assert!(persisted.error.is_none());
        }
        assert!(layout.staging_dir().join("upd-qa004/app").is_dir());
        assert_eq!(current_version(&layout).as_deref(), Some(OLD_VERSION));
        cleanup(&layout.root);
    }
}

#[test]
fn upgrade_recovery_stopped_and_snapshotting_restore_stable_old_state() {
    let layout = new_layout("stopped");
    write_old_current(&layout);
    write_live_data(&layout, b"old");
    let (report, persisted) = persist_then_restart(&layout, &journal(UpdateState::Stopped));
    assert_aborted(&layout, UpdateState::Stopped, &report, &persisted);
    assert_eq!(
        fs::read(layout.data_dir().join("state.bin")).unwrap(),
        b"old"
    );
    cleanup(&layout.root);

    let layout = new_layout("snapshotting-partial");
    write_old_current(&layout);
    write_live_data(&layout, b"old");
    fs::create_dir_all(snapshot::backup_dir(&layout, "upd-qa004")).expect("创建半截快照");
    fs::write(
        snapshot::backup_dir(&layout, "upd-qa004").join(snapshot::SNAPSHOT_MANIFEST),
        b"{\"partial\":true}",
    )
    .expect("写入半截快照清单");
    let (report, persisted) = persist_then_restart(&layout, &journal(UpdateState::Snapshotting));
    assert_aborted(&layout, UpdateState::Snapshotting, &report, &persisted);
    assert!(!snapshot::backup_dir(&layout, "upd-qa004").exists());
    assert_eq!(
        fs::read(layout.data_dir().join("state.bin")).unwrap(),
        b"old"
    );
    cleanup(&layout.root);

    let layout = new_layout("snapshotting-complete");
    write_old_current(&layout);
    write_live_data(&layout, b"old");
    let snapshot = snapshot_info(&layout, "upd-qa004");
    let (report, persisted) = persist_then_restart(
        &layout,
        &UpdateJournal {
            snapshot: Some(snapshot),
            ..journal(UpdateState::Snapshotting)
        },
    );
    assert_eq!(report.from, Some(UpdateState::Snapshotting));
    assert!(matches!(report.outcome, RecoveryOutcome::RolledBack { .. }));
    assert_eq!(persisted.state, UpdateState::Idle);
    assert_eq!(persisted.last_step.as_deref(), Some("failed"));
    assert_eq!(current_version(&layout).as_deref(), Some(OLD_VERSION));
    cleanup(&layout.root);
}

#[test]
fn upgrade_recovery_snapshot_verified_and_migrating_use_pointer_guard() {
    for state in [UpdateState::SnapshotVerified, UpdateState::Migrating] {
        let layout = new_layout(&format!("not-switched-{}", state.as_str()));
        write_old_current(&layout);
        write_live_data(&layout, b"old");
        let snapshot = snapshot_info(&layout, "upd-qa004");
        let (report, persisted) = persist_then_restart(
            &layout,
            &UpdateJournal {
                snapshot: Some(snapshot),
                ..journal(state)
            },
        );
        assert_aborted(&layout, state, &report, &persisted);
        assert_eq!(
            fs::read(layout.data_dir().join("state.bin")).unwrap(),
            b"old"
        );
        cleanup(&layout.root);
    }

    // 即使 journal 仍停在 migrating，只要 current 已切到候选，就按保守路径完整回滚。
    let layout = new_layout("migrating-after-pointer");
    write_old_current(&layout);
    write_live_data(&layout, b"old");
    let snapshot = snapshot_info(&layout, "upd-qa004");
    fs::write(layout.data_dir().join("state.bin"), b"migrated").unwrap();
    fs::write(layout.data_dir().join("candidate-only.bin"), b"candidate").unwrap();
    StateStore::new(&layout.root)
        .write_current(&CurrentState::new(
            NEW_VERSION,
            Some(OLD_VERSION.to_string()),
        ))
        .unwrap();
    let (report, persisted) = persist_then_restart(
        &layout,
        &UpdateJournal {
            snapshot: Some(snapshot),
            current_version: Some(NEW_VERSION.to_string()),
            ..journal(UpdateState::Migrating)
        },
    );
    assert_eq!(report.from, Some(UpdateState::Migrating));
    assert!(matches!(report.outcome, RecoveryOutcome::RolledBack { .. }));
    assert_eq!(persisted.state, UpdateState::Idle);
    assert_eq!(current_version(&layout).as_deref(), Some(OLD_VERSION));
    assert_eq!(
        fs::read(layout.data_dir().join("state.bin")).unwrap(),
        b"old"
    );
    assert!(!layout.data_dir().join("candidate-only.bin").exists());
    cleanup(&layout.root);
}

#[test]
fn upgrade_recovery_switched_candidate_ready_and_activating_roll_back() {
    for state in [
        UpdateState::Switched,
        UpdateState::CandidateStarting,
        UpdateState::CandidateReady,
        UpdateState::Activating,
    ] {
        let layout = new_layout(&format!("candidate-{}", state.as_str()));
        write_old_current(&layout);
        write_live_data(&layout, b"old");
        let snapshot = snapshot_info(&layout, "upd-qa004");
        fs::write(layout.data_dir().join("state.bin"), b"migrated").unwrap();
        fs::write(layout.data_dir().join("candidate-only.bin"), b"candidate").unwrap();
        StateStore::new(&layout.root)
            .write_current(&CurrentState::new(
                NEW_VERSION,
                Some(OLD_VERSION.to_string()),
            ))
            .unwrap();

        let (report, persisted) = persist_then_restart(&layout, &switched_journal(state, snapshot));
        assert_eq!(report.from, Some(state));
        assert!(matches!(report.outcome, RecoveryOutcome::RolledBack { .. }));
        assert_eq!(persisted.state, UpdateState::Idle);
        assert_eq!(persisted.last_step.as_deref(), Some("failed"));
        assert_eq!(current_version(&layout).as_deref(), Some(OLD_VERSION));
        assert_eq!(
            fs::read(layout.data_dir().join("state.bin")).unwrap(),
            b"old"
        );
        assert!(!layout.data_dir().join("candidate-only.bin").exists());
        cleanup(&layout.root);
    }
}

#[test]
fn upgrade_recovery_committed_and_cleaning_finish_only_with_new_pointer() {
    for state in [UpdateState::Committed, UpdateState::Cleaning] {
        let layout = new_layout(&format!("commit-{}", state.as_str()));
        write_old_current(&layout);
        write_candidate_manifest_and_staging(&layout, "upd-qa004");
        StateStore::new(&layout.root)
            .write_current(&CurrentState::new(
                NEW_VERSION,
                Some(OLD_VERSION.to_string()),
            ))
            .unwrap();
        let (report, persisted) = persist_then_restart(&layout, &state_with_new_pointer(state));
        assert_eq!(report.from, Some(state));
        assert_eq!(
            report.outcome,
            RecoveryOutcome::CommittedFinished {
                version: NEW_VERSION.to_string()
            }
        );
        assert_eq!(persisted.state, UpdateState::Idle);
        assert_eq!(persisted.last_step.as_deref(), Some("idle"));
        assert!(persisted.error.is_none());
        assert!(!layout.staging_dir().join("upd-qa004").exists());
        assert_eq!(current_version(&layout).as_deref(), Some(NEW_VERSION));
        cleanup(&layout.root);
    }

    let layout = new_layout("committed-pointer-mismatch");
    write_old_current(&layout);
    write_candidate_manifest_and_staging(&layout, "upd-qa004");
    let (report, persisted) = persist_then_restart(&layout, &journal(UpdateState::Committed));
    assert_eq!(report.from, Some(UpdateState::Committed));
    assert!(matches!(
        report.outcome,
        RecoveryOutcome::ManualRequired { .. }
    ));
    assert_eq!(persisted.state, UpdateState::ManualRecoveryRequired);
    assert_eq!(
        persisted.last_step.as_deref(),
        Some("manual_recovery_required")
    );
    assert_eq!(
        persisted.error.as_ref().map(|e| e.code.as_str()),
        Some("rollback_unavailable")
    );
    assert!(layout.staging_dir().join("upd-qa004").exists());
    assert_eq!(current_version(&layout).as_deref(), Some(OLD_VERSION));
    cleanup(&layout.root);
}

#[test]
fn upgrade_recovery_manual_required_is_sticky_and_never_auto_moves() {
    let layout = new_layout("manual-sticky");
    write_old_current(&layout);
    let mut journal = journal(UpdateState::ManualRecoveryRequired);
    journal.last_step = Some("manual_recovery_required".to_string());
    journal.error = Some(JournalError {
        code: "rollback_unavailable".to_string(),
        message: "保留证据等待人工处理".to_string(),
    });
    let (report, persisted) = persist_then_restart(&layout, &journal);
    assert_eq!(report.from, Some(UpdateState::ManualRecoveryRequired));
    assert!(report.is_manual());
    assert_eq!(persisted, journal);
    cleanup(&layout.root);
}

fn state_with_new_pointer(state: UpdateState) -> UpdateJournal {
    UpdateJournal {
        current_version: Some(NEW_VERSION.to_string()),
        ..journal(state)
    }
}
