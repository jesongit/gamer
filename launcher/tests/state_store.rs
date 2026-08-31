//! QA-001 / LCH-002：单实例锁、原子写与损坏恢复测试（含中文/空格路径）。

use std::fs;
use std::path::{Path, PathBuf};

use gamer_launcher::state::atomic::LoadOutcome;
use gamer_launcher::state::lock::{InstanceLock, LockError};
use gamer_launcher::state::{
    ChildInfo, CurrentState, JournalError, SnapshotInfo, StateStore, UpdateJournal, UpdateState,
    STATE_SCHEMA_VERSION,
};

fn unique_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gamer-launcher-state-tests-{tag}-{}-{}",
        std::process::id(),
        gamer_launcher::state::atomic::now_unix_millis()
    ));
    fs::create_dir_all(&dir).expect("创建临时安装根");
    dir
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

fn sample_current() -> CurrentState {
    CurrentState::new("0.2.0", Some("0.1.0".to_string()))
}

fn sample_journal() -> UpdateJournal {
    UpdateJournal {
        schema_version: STATE_SCHEMA_VERSION,
        update_id: Some("upd-20260831-0001".to_string()),
        state: UpdateState::SnapshotVerified,
        from_version: Some("0.1.0".to_string()),
        to_version: Some("0.2.0".to_string()),
        child: Some(ChildInfo {
            pid: 4242,
            created_at_unix_ms: Some(1_788_000_000_000),
            exe: "C:\\GameBot\\versions\\0.2.0\\gamer-server.exe".to_string(),
        }),
        current_version: Some("0.1.0".to_string()),
        previous_version: None,
        snapshot: Some(SnapshotInfo {
            id: "upd-20260831-0001".to_string(),
            path: "C:\\GameBot\\backups\\upd-20260831-0001".to_string(),
            file_count: 3,
            total_bytes: 128,
        }),
        data_schema_before: Some(1),
        data_schema_after: Some(2),
        last_step: Some("snapshot_verified".to_string()),
        error: Some(JournalError {
            code: "probe_failed".to_string(),
            message: "ready 探针超时".to_string(),
        }),
        updated_at_unix_ms: Some(1_788_000_001_000),
    }
}

#[test]
fn current_roundtrip() {
    let root = unique_root("roundtrip");
    let store = StateStore::new(&root);
    assert!(matches!(store.load_current(), Ok(LoadOutcome::Missing)));

    let sample = sample_current();
    store.write_current(&sample).expect("写入 current");
    match store.load_current().expect("读取 current") {
        LoadOutcome::Present(state) => assert_eq!(state, sample),
        other => panic!("应为 Present，实际 {other:?}"),
    }
    cleanup(&root);
}

#[test]
fn chinese_and_space_paths_roundtrip() {
    // 计划 §11.5：安装路径包含空格、中文
    let root = unique_root("测试 目录 中 文");
    let store = StateStore::new(&root);
    store
        .write_current(&sample_current())
        .expect("写入 current");
    store
        .write_journal(&sample_journal())
        .expect("写入 journal");
    match store.load_current().expect("读取 current") {
        LoadOutcome::Present(state) => assert_eq!(state.current, "0.2.0"),
        other => panic!("应为 Present，实际 {other:?}"),
    }
    let journal = store.load_journal().expect("读取 journal").journal;
    assert_eq!(journal, sample_journal());

    // 锁在中文/空格路径下同样可用
    let lock = InstanceLock::acquire(&store.state_dir()).expect("取锁");
    drop(lock);
    cleanup(&root);
}

#[test]
fn journal_roundtrip_and_defaults() {
    let root = unique_root("journal");
    let store = StateStore::new(&root);
    // 无文件 → 空状态，无重置
    let loaded = store.load_journal().expect("读取空 journal");
    assert_eq!(loaded.journal.state, UpdateState::Idle);
    assert!(loaded.reset_from.is_none());

    store
        .write_journal(&sample_journal())
        .expect("写入 journal");
    let loaded = store.load_journal().expect("读取 journal");
    assert_eq!(loaded.journal, sample_journal());
    assert!(loaded.reset_from.is_none());
    cleanup(&root);
}

#[test]
fn corrupt_current_backed_up_not_crash() {
    let root = unique_root("corrupt-current");
    let store = StateStore::new(&root);
    fs::create_dir_all(store.state_dir()).unwrap();
    fs::write(store.current_path(), "{ this is not json }}").unwrap();

    match store.load_current().expect("损坏文件也应返回 Ok") {
        LoadOutcome::Corrupted { backup_path } => {
            assert!(backup_path.exists(), "备份文件应存在");
            assert!(backup_path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("current.json.corrupt-")));
            assert!(!store.current_path().exists(), "原损坏文件应已移走");
        }
        other => panic!("应为 Corrupted，实际 {other:?}"),
    }
    // 备份后可正常写入新状态
    store.write_current(&sample_current()).unwrap();
    assert!(matches!(store.load_current(), Ok(LoadOutcome::Present(_))));
    cleanup(&root);
}

#[test]
fn truncated_and_empty_files_are_corrupted() {
    let root = unique_root("truncated");
    let store = StateStore::new(&root);
    fs::create_dir_all(store.state_dir()).unwrap();

    // 半截 JSON（写一半崩溃的形态）
    fs::write(store.current_path(), "{\"schema_version\": 1, \"curr").unwrap();
    assert!(matches!(
        store.load_current(),
        Ok(LoadOutcome::Corrupted { .. })
    ));

    // 空文件
    fs::write(store.current_path(), "").unwrap();
    assert!(matches!(
        store.load_current(),
        Ok(LoadOutcome::Corrupted { .. })
    ));

    // journal 同样处理：损坏 → 备份并回空状态
    fs::write(store.journal_path(), "garbage\u{0}\u{1}").unwrap();
    let loaded = store.load_journal().expect("journal 损坏恢复");
    assert_eq!(loaded.journal.state, UpdateState::Idle);
    assert!(loaded.reset_from.is_some_and(|p| p.exists()));
    cleanup(&root);
}

#[test]
fn journal_unknown_schema_version_is_reset() {
    let root = unique_root("journal-schema");
    let store = StateStore::new(&root);
    fs::create_dir_all(store.state_dir()).unwrap();
    fs::write(
        store.journal_path(),
        r#"{"schema_version": 99, "state": "idle"}"#,
    )
    .unwrap();
    let loaded = store.load_journal().expect("未知 journal 版本按损坏处理");
    assert_eq!(loaded.journal, UpdateJournal::default());
    assert!(loaded.reset_from.is_some());
    cleanup(&root);
}

#[test]
fn atomic_write_leaves_no_temp_files() {
    let root = unique_root("no-temp");
    let store = StateStore::new(&root);
    for i in 0..5 {
        store
            .write_current(&CurrentState::new(format!("0.1.{i}"), None))
            .unwrap();
    }
    let leftovers: Vec<String> = fs::read_dir(store.state_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.starts_with('.') || n.contains(".tmp-") || n.contains(".corrupt-"))
        .collect();
    assert!(leftovers.is_empty(), "不应残留临时/备份文件: {leftovers:?}");
    let mut names: Vec<String> = fs::read_dir(store.state_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect();
    names.sort();
    assert_eq!(names, vec!["current.json"]);
    cleanup(&root);
}

#[test]
fn repeated_writes_stay_consistent() {
    let root = unique_root("consistent");
    let store = StateStore::new(&root);
    for i in 0..20 {
        let version = format!("0.{i}.0");
        store
            .write_current(&CurrentState::new(version.clone(), None))
            .expect("原子写");
        match store.load_current().expect("读取") {
            LoadOutcome::Present(state) => assert_eq!(state.current, version),
            other => panic!("第 {i} 次写入后应为 Present，实际 {other:?}"),
        }
    }
    cleanup(&root);
}

#[test]
fn lock_excludes_second_holder_then_releases() {
    let root = unique_root("lock-exclusive");
    let store = StateStore::new(&root);
    let first = InstanceLock::acquire(&store.state_dir()).expect("第一个实例应取到锁");

    match InstanceLock::acquire(&store.state_dir()) {
        Err(LockError::Held { path }) => {
            assert_eq!(path, store.lock_path());
        }
        Err(LockError::Io(e)) => panic!("应为 Held，实际 Io: {e}"),
        Ok(_) => panic!("第二个实例不应取到锁"),
    }

    drop(first);
    // 释放后可重新获取
    let _second = InstanceLock::acquire(&store.state_dir()).expect("释放后应可重新取锁");
    cleanup(&root);
}

#[test]
fn lock_reclaims_stale_lock_file() {
    // 崩溃遗留的锁文件（无持有者）应可被接管
    let root = unique_root("lock-stale");
    let store = StateStore::new(&root);
    fs::create_dir_all(store.state_dir()).unwrap();
    fs::write(store.lock_path(), "pid=999999\n").unwrap();
    let lock = InstanceLock::acquire(&store.state_dir()).expect("遗留锁文件应被接管");
    assert!(InstanceLock::is_locked(&store.lock_path()));
    drop(lock);
    assert!(!InstanceLock::is_locked(&store.lock_path()));
    cleanup(&root);
}

#[test]
fn lock_file_absence_reports_unlocked() {
    let root = unique_root("lock-absent");
    let store = StateStore::new(&root);
    fs::create_dir_all(store.state_dir()).unwrap();
    assert!(!InstanceLock::is_locked(&store.lock_path()));
    cleanup(&root);
}
