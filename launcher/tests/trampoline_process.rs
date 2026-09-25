//! Real Windows executable replacement. No server, ADB, GUI or network is started.
mod common;

use gamer_launcher::upgrade::trampoline::{self, LauncherUpdateRequest};
use std::os::windows::{fs::OpenOptionsExt, process::CommandExt};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

#[test]
#[allow(clippy::zombie_processes)] // Helper must outlive this parent; outer test waits for its PID.
fn trampoline_parent() {
    let Some(root) = std::env::var_os("GAMER_TRAMPOLINE_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let request = LauncherUpdateRequest::new(
        root.join("gamer-launcher.exe"),
        root.join("component/gamer-launcher.exe"),
    );
    let child = trampoline::schedule(&request).unwrap();
    fs::write(root.join("helper-pid"), child.id().to_string()).unwrap();
    // Returning exits this test process; the real helper waits for its death.
}

fn exercise(occupied: bool) {
    let root = common::unique_root(if occupied {
        "trampoline-locked"
    } else {
        "trampoline-success"
    });
    fs::create_dir(root.join("component")).unwrap();
    let binary = fs::read(env!("CARGO_BIN_EXE_gamer-launcher")).unwrap();
    fs::write(root.join("component/gamer-launcher.exe"), &binary).unwrap();
    let mut old = binary.clone();
    old.extend_from_slice(b"old-version-marker");
    let current = root.join("gamer-launcher.exe");
    fs::write(&current, &old).unwrap();
    let lock = occupied.then(|| {
        fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&current)
            .unwrap()
    });
    let mut parent = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "trampoline_parent", "--nocapture"])
        .env("GAMER_TRAMPOLINE_TEST_ROOT", &root)
        .env("GAMER_LOCAL_ONLY", "1")
        .env("ADB_MDNS", "0")
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while parent.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = parent.kill();
            panic!("test parent timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid: u32 = fs::read_to_string(root.join("helper-pid"))
        .unwrap()
        .parse()
        .unwrap();
    while gamer_launcher::winutil::process_image_path(pid).is_some() {
        assert!(Instant::now() < deadline, "helper timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        fs::read(&current).unwrap() == if occupied { old } else { binary.clone() },
        "entry bytes differ from expected version"
    );
    assert!(
        fs::read(root.join("component/gamer-launcher.exe")).unwrap() == binary,
        "immutable component was modified"
    );
    assert!(!fs::read_dir(&root).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".gamer-launcher-trampoline-")));
    let log = fs::read_to_string(root.join("logs/launcher.log")).unwrap();
    if occupied {
        assert!(log.contains("自更新失败"));
        assert!(root.join("state/launcher-update-error.txt").is_file());
    } else {
        assert!(log.contains("启动器替换完成"));
        assert!(!root.join("state/launcher-update-error.txt").exists());
    }
    drop(lock);
    common::cleanup(&root);
}

#[test]
fn real_helper_replaces_entry_after_parent_exit() {
    exercise(false);
}

#[test]
fn real_helper_preserves_locked_entry_and_records_failure() {
    exercise(true);
}
