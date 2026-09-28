//! Portable application bootstrap and short-lived update worker. No resident launcher.
use crate::{
    layout::InstallLayout,
    state::{atomic::LoadOutcome, lock::InstanceLock, StateStore},
    upgrade::engine::{Engine, UpgradeOptions, UpgradeOutcome},
};
use std::os::windows::process::CommandExt;
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

pub const ROOT_ENV: &str = "GAMER_INSTALL_ROOT";
pub const ENTRY: &str = "Gamer.exe";

pub fn layout() -> Option<InstallLayout> {
    std::env::var_os(ROOT_ENV).map(|p| InstallLayout::resolve(Some(p.into())))
}

pub fn engine_options(layout: &InstallLayout) -> io::Result<UpgradeOptions> {
    let token = crate::installation::load_or_create_admin_token(&StateStore::new(&layout.root))?;
    Ok(UpgradeOptions {
        portable: true,
        admin_token: Some(token.clone()),
        activation_token: Some(token),
        ..Default::default()
    })
}

/// None means this is a source/container run. Packaged startup owns the server lock.
pub fn prepare() -> Result<Option<InstanceLock>, String> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--update-worker") {
        let root = args.get(2).ok_or("missing update root")?;
        let operation = args
            .get(3)
            .and_then(|v| v.to_str())
            .ok_or("missing operation")?;
        let layout = InstallLayout::resolve(Some(root.into()));
        let result = worker(&layout, operation);
        if let Err(error) = &result {
            let _ = fs::create_dir_all(layout.logs_dir());
            let _ = fs::write(layout.logs_dir().join("update-worker-error.txt"), error);
            if operation == "recover" {
                report_startup_error(error);
            }
        }
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    // Maintenance subcommands inspect/migrate must not open a browser or acquire a server lock.
    if args.len() > 1 {
        return Ok(None);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let detected = layout().or_else(|| {
        let root = exe.parent()?;
        root.join("state/current.json")
            .is_file()
            .then(|| InstallLayout::resolve(Some(root.into())))
    });
    let Some(layout) = detected else {
        return Ok(None);
    };
    let child = std::env::var_os("GAMER_PORTABLE_CHILD").is_some();
    if !child {
        // A worker may be between processes: never start another server during replacement.
        match InstanceLock::acquire(&layout.state_dir().join("update")) {
            Ok(guard) => drop(guard),
            Err(crate::state::lock::LockError::Held { .. }) => {
                open_browser(crate::supervisor::read_configured_port(
                    &layout.config_file(),
                ));
                std::process::exit(0);
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    let lock = match InstanceLock::acquire(&layout.state_dir().join("server")) {
        Ok(lock) => lock,
        Err(crate::state::lock::LockError::Held { .. }) => {
            if !child && orphan_candidate(&layout) {
                spawn_worker(&layout, "recover").map_err(|e| e.to_string())?;
            } else {
                open_browser(crate::supervisor::read_configured_port(
                    &layout.config_file(),
                ));
            }
            std::process::exit(0);
        }
        Err(e) => return Err(e.to_string()),
    };
    if !child {
        let store = StateStore::new(&layout.root);
        let journal = store.load_journal().map_err(|e| e.to_string())?.journal;
        use crate::state::UpdateState;
        if !matches!(journal.state, UpdateState::Idle | UpdateState::Staged) {
            spawn_worker(&layout, "recover").map_err(|e| e.to_string())?;
            drop(lock);
            std::process::exit(0);
        }
    }
    crate::bootstrap::ensure_config(&layout).map_err(|e| e.to_string())?;
    let version = crate::distribution::current(&layout).ok_or("missing current version")?;
    let (_, manifest) =
        crate::distribution::cached(&layout, Some(&version)).ok_or("missing release manifest")?;
    let plan = crate::distribution::plan(&layout, &manifest)?;
    let extras = crate::supervisor::LaunchExtras::default().with_admin_token(
        engine_options(&layout)
            .map_err(|e| e.to_string())?
            .admin_token,
    );
    let env = crate::supervisor::build_child_env_with_extras(&plan, &extras, |key| {
        std::env::var(key).ok()
    });
    for (key, value) in env {
        std::env::set_var(key, value);
    }
    std::env::set_var(ROOT_ENV, &layout.root);
    std::env::set_var("GAMER_DEPLOYMENT_MODE", "portable");
    crate::tray::spawn(crate::supervisor::read_configured_port(
        &layout.config_file(),
    ));
    if !child {
        let port = crate::supervisor::read_configured_port(&layout.config_file());
        std::thread::spawn(move || {
            if crate::supervisor::wait_for_ready(port, &Default::default()).is_ok() {
                open_browser(port);
            }
        });
    }
    Ok(Some(lock))
}

pub fn open_browser(port: u16) {
    // ShellExecute opens the registered browser without a shell command or console window.
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let url: Vec<u16> = format!("http://127.0.0.1:{port}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            url.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        );
    }
}

pub fn spawn_worker(layout: &InstallLayout, operation: &str) -> io::Result<std::process::Child> {
    let dir = layout.state_dir().join("workers");
    fs::create_dir_all(&dir)?;
    let exe = dir.join(format!(
        "update-{}-{}.exe",
        std::process::id(),
        crate::state::atomic::now_unix_millis()
    ));
    fs::copy(std::env::current_exe()?, &exe)?;
    Command::new(exe)
        .args(["--update-worker"])
        .arg(&layout.root)
        .arg(operation)
        .env(ROOT_ENV, &layout.root)
        .env("GAMER_WORKER_PARENT", std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()
}

fn worker(layout: &InstallLayout, operation: &str) -> Result<(), String> {
    let _guard =
        InstanceLock::acquire(&layout.state_dir().join("update")).map_err(|e| e.to_string())?;
    std::env::set_var(ROOT_ENV, &layout.root);
    if operation == "recover" {
        if orphan_candidate(layout) {
            let journal = StateStore::new(&layout.root)
                .load_journal()
                .map_err(|e| e.to_string())?
                .journal;
            if let Some(child) = journal.child {
                crate::winutil::terminate_pid_if_image(child.pid, Path::new(&child.exe));
            }
        }
        // The root executable must exit before it can be restored. Hold the update lock meanwhile.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            match InstanceLock::acquire(&layout.state_dir().join("server")) {
                Ok(lock) => {
                    drop(lock);
                    break;
                }
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100))
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        let report =
            crate::upgrade::recovery::recover_on_startup(layout, &StateStore::new(&layout.root))
                .map_err(|e| e.to_string())?;
        if report.is_manual() {
            return Err(format!("{report:?}"));
        }
        sync_entrypoint(layout)?;
        Command::new(layout.root.join(ENTRY))
            .env("GAMER_PORTABLE_CHILD", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    if operation != "install" {
        return Err("unknown update operation".into());
    }
    // Give the accepted HTTP response time to flush before requesting graceful shutdown.
    std::thread::sleep(Duration::from_millis(500));
    let engine = Engine::new(
        layout.clone(),
        engine_options(layout).map_err(|e| e.to_string())?,
    );
    let outcome = engine.install_staged();
    match outcome {
        UpgradeOutcome::Committed { .. } => Ok(()),
        UpgradeOutcome::FailedOldHealthy { .. } => sync_entrypoint(layout),
        other => Err(format!("{other:?}")),
    }
}

/// Replace the stable user entry atomically; the versioned executable remains available for recovery.
pub fn sync_entrypoint(layout: &InstallLayout) -> Result<(), String> {
    let current = match StateStore::new(&layout.root)
        .load_current()
        .map_err(|e| e.to_string())?
    {
        LoadOutcome::Present(current) => current,
        _ => return Err("missing current version".into()),
    };
    let source = crate::supervisor::resolve_entrypoint(layout, &current.current)?;
    if crate::digest::sha256_file_hex(&source)
        .ok()
        .zip(crate::digest::sha256_file_hex(&layout.root.join(ENTRY)).ok())
        .is_some_and(|(a, b)| a == b)
    {
        return Ok(());
    }
    let staged = layout.root.join("Gamer.exe.next");
    fs::copy(&source, &staged).map_err(|e| e.to_string())?;
    crate::state::atomic::rename_with_retry(&staged, &layout.root.join(ENTRY))
        .map_err(|e| e.to_string())
}

pub fn portable_root_from_app(app: &Path) -> Option<PathBuf> {
    let versions = app.parent()?;
    (versions.file_name()? == "versions")
        .then(|| versions.parent().map(Path::to_path_buf))
        .flatten()
}

fn orphan_candidate(layout: &InstallLayout) -> bool {
    use crate::state::UpdateState;
    StateStore::new(&layout.root).load_journal().is_ok_and(|j| {
        matches!(
            j.journal.state,
            UpdateState::Switched
                | UpdateState::CandidateStarting
                | UpdateState::CandidateReady
                | UpdateState::Activating
        )
    })
}

pub fn report_startup_error(message: &str) {
    let root = layout().map(|l| l.root).or_else(|| {
        std::env::current_exe()
            .ok()?
            .parent()
            .map(Path::to_path_buf)
    });
    if let Some(root) = root {
        let _ = fs::create_dir_all(root.join("logs"));
        let _ = fs::write(root.join("logs/startup-error.txt"), message);
    }
    let message: Vec<_> =
        format!("Gamer 无法启动：{message}\n详细信息已写入 logs/startup-error.txt")
            .encode_utf16()
            .chain(Some(0))
            .collect();
    let title: Vec<_> = "Gamer\0".encode_utf16().collect();
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            0x10,
        );
    }
}
