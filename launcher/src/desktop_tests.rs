use super::*;
use std::time::Instant;

#[test]
fn opening_tray_during_setup_reveals_window_without_starting_another_job() {
    for stage in [
        Stage::Install,
        Stage::Update,
        Stage::Working,
        Stage::Paused,
        Stage::Plugins,
        Stage::Error,
        Stage::Stopped,
    ] {
        let (jobs, receiver) = mpsc::channel();
        let (_, updates) = mpsc::channel();
        let mut desktop = Desktop {
            layout: InstallLayout {
                root: PathBuf::new(),
            },
            jobs,
            updates,
            control: TransferControl::default(),
            snapshot: Snapshot {
                stage,
                ..Default::default()
            },
            tray: None,
            exit: false,
            cancelling: false,
            quiet_start: true,
        };
        desktop.action("open", &egui::Context::default());
        assert!(
            !desktop.quiet_start,
            "{stage:?} must be reachable from the tray"
        );
        assert_eq!(desktop.snapshot.stage, stage);
        assert!(receiver.try_recv().is_err());
        assert!(!desktop.cancelling);
    }
}

#[test]
fn cancelling_busy_install_preserves_progress_and_queues_exit_once() {
    let (jobs, receiver) = mpsc::channel();
    let (_, updates) = mpsc::channel();
    let mut desktop = Desktop {
        layout: InstallLayout {
            root: PathBuf::new(),
        },
        jobs,
        updates,
        control: TransferControl::default(),
        snapshot: Snapshot {
            stage: Stage::Working,
            ..Default::default()
        },
        tray: None,
        exit: false,
        cancelling: false,
        quiet_start: true,
    };
    desktop.action("exit", &egui::Context::default());
    desktop.action("exit", &egui::Context::default());
    assert!(desktop.control.is_paused());
    assert!(desktop.cancelling);
    assert!(matches!(receiver.try_recv(), Ok(Job::Exit)));
    assert!(receiver.try_recv().is_err());
    assert!(
        !desktop.exit,
        "GUI must await worker safe-exit acknowledgement"
    );
}

struct Harness {
    jobs: mpsc::Sender<Job>,
    updates: mpsc::Receiver<Event>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Harness {
    fn wait(&self, expected: Stage) -> Snapshot {
        let until = Instant::now() + Duration::from_secs(180);
        loop {
            let event = self
                .updates
                .recv_timeout(until.saturating_duration_since(Instant::now()))
                .expect("worker did not reach expected state");
            if let Event::State(state) = event {
                if !state.busy {
                    eprintln!("desktop: {:?}: {}", state.stage, state.message);
                    assert_eq!(state.stage, expected, "{}", state.message);
                    return state;
                }
            }
        }
    }
    fn job(&self, job: Job, expected: Stage) -> Snapshot {
        self.jobs.send(job).unwrap();
        self.wait(expected)
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.jobs.send(Job::Exit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn write_candidate(layout: &InstallLayout, mut value: serde_json::Value) {
    value["release"]["version"] = "0.2.1".into();
    let raw = serde_json::to_vec_pretty(&value).unwrap();
    fs::write(layout.manifests_dir().join("0.2.1.json"), &raw).unwrap();
}

#[test]
#[ignore = "real release processes; requires a fresh isolated full-package extraction"]
fn real_desktop_install_repair_restart_update_and_rollback() {
    let root = std::env::var_os("GAMER_DESKTOP_SMOKE_ROOT").expect("isolated root required");
    let layout = InstallLayout::resolve(Some(root.into()));
    assert!(
        !layout.state_dir().join("current.json").exists(),
        "fresh install required"
    );
    let port = crate::supervisor::read_configured_port(&layout.config_file());
    assert_ne!(port, 8443, "do not test against developer data");
    assert!(
        std::env::var("GAMER_LAUNCHER_RELEASE_MANIFEST").is_ok(),
        "explicit offline discovery source required"
    );
    let control = TransferControl::default();
    let worker_control = control.clone();
    let worker_layout = layout.clone();
    let (jobs, receiver) = mpsc::channel();
    let (events, updates) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let _lock = InstanceLock::acquire(&worker_layout.state_dir()).unwrap();
        Worker::run(
            worker_layout,
            worker_control,
            receiver,
            events,
            egui::Context::default(),
        );
    });
    let harness = Harness {
        jobs,
        updates,
        thread: Some(thread),
    };
    harness.job(Job::Inspect(true), Stage::Install);
    control.pause();
    harness.job(Job::Install, Stage::Paused);
    assert!(dist::current(&layout).is_none());
    control.resume();
    let state = harness.job(Job::Install, Stage::Plugins);
    assert_eq!(state.plugins.len(), 3);
    harness.job(
        Job::Plugins(vec!["gamer-yaml".into(), "gamer-video".into()]),
        Stage::Running,
    );
    let token =
        crate::installation::load_or_create_admin_token(&StateStore::new(&layout.root)).unwrap();
    let response = ureq::get(&format!("http://127.0.0.1:{port}/api/extensions"))
        .set("X-Admin-Token", &token)
        .call()
        .unwrap();
    let installed: serde_json::Value = serde_json::from_reader(response.into_reader()).unwrap();
    assert_eq!(installed["extensions"].as_array().unwrap().len(), 2);
    assert!(installed["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["state"] == "running"));
    harness.job(Job::Stop, Stage::Stopped);

    let page = layout.versions_dir().join("0.2.0/web-dist/index.html");
    let original = fs::read(&page).unwrap();
    fs::write(&page, b"damaged frontend").unwrap();
    fs::write(
        layout.data_dir().join("qa-preserve.txt"),
        b"personal data must survive",
    )
    .unwrap();
    harness.job(Job::Inspect(true), Stage::Running);
    assert_eq!(
        fs::read(&page).unwrap(),
        original,
        "auto start must repair damaged web files"
    );
    harness.job(Job::Stop, Stage::Stopped);
    let receipts: Vec<_> = fs::read_dir(layout.state_dir().join("verified"))
        .unwrap()
        .map(|e| {
            let path = e.unwrap().path();
            let time = fs::metadata(&path).unwrap().modified().unwrap();
            (path, time)
        })
        .collect();
    harness.job(Job::Inspect(true), Stage::Running);
    for (path, time) in receipts {
        assert_eq!(
            fs::metadata(path).unwrap().modified().unwrap(),
            time,
            "unchanged files must reuse verification receipts"
        );
    }
    harness.job(Job::Stop, Stage::Stopped);
    harness.job(Job::Repair, Stage::Ready);
    assert_eq!(
        fs::read(layout.data_dir().join("qa-preserve.txt")).unwrap(),
        b"personal data must survive"
    );

    let base: serde_json::Value =
        serde_json::from_slice(&fs::read(layout.manifests_dir().join("0.2.0.json")).unwrap())
            .unwrap();
    write_candidate(&layout, base);
    harness.job(Job::Inspect(true), Stage::Update);
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "cached update must block automatic startup even offline"
    );
    // The signed candidate deliberately claims 0.2.1 while containing the real 0.2.0 server.
    // This must fail the candidate identity gate and restore the existing installation.
    harness.job(Job::Start, Stage::Running); // explicit "start existing version"
    harness.job(Job::Inspect(false), Stage::Update);
    harness.job(Job::Install, Stage::Error);
    assert_eq!(dist::current(&layout).as_deref(), Some("0.2.0"));
    assert_eq!(
        fs::read(layout.data_dir().join("qa-preserve.txt")).unwrap(),
        b"personal data must survive"
    );
    crate::supervisor::wait_for_ready(port, &Default::default())
        .expect("old server must be restored");
    harness.job(Job::Stop, Stage::Stopped);
    harness.job(Job::Start, Stage::Running);
    drop(harness);
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
}

#[test]
#[ignore = "requires an installed real v0.1.1 full release and signed v0.2.0 seeds"]
fn real_desktop_upgrade_preserves_data_and_offers_first_plugin_selection() {
    let root =
        std::env::var_os("GAMER_DESKTOP_UPGRADE_ROOT").expect("isolated old release required");
    let layout = InstallLayout::resolve(Some(root.into()));
    assert_eq!(dist::current(&layout).as_deref(), Some("0.1.1"));
    let port = crate::supervisor::read_configured_port(&layout.config_file());
    assert_ne!(port, 8443);
    let worker_layout = layout.clone();
    let (jobs, receiver) = mpsc::channel();
    let (events, updates) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let _lock = InstanceLock::acquire(&worker_layout.state_dir()).unwrap();
        Worker::run(
            worker_layout,
            TransferControl::default(),
            receiver,
            events,
            egui::Context::default(),
        );
    });
    let harness = Harness {
        jobs,
        updates,
        thread: Some(thread),
    };
    harness.job(Job::Inspect(true), Stage::Update);
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
    harness.job(Job::Start, Stage::Running);
    fs::write(
        layout.data_dir().join("preserve-old.txt"),
        b"old personal content",
    )
    .unwrap();
    harness.job(Job::Inspect(false), Stage::Update);
    harness.job(Job::Install, Stage::Plugins);
    assert_eq!(dist::current(&layout).as_deref(), Some("0.2.0"));
    assert_eq!(
        fs::read(layout.data_dir().join("preserve-old.txt")).unwrap(),
        b"old personal content"
    );
    harness.job(Job::Plugins(vec![]), Stage::Running);
    assert!(layout.state_dir().join("plugins-choice.json").is_file());
    drop(harness); // Exit while running must gracefully stop the owned server.
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
}
