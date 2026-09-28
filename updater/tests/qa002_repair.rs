//! QA-002：repair 修复编排测试（LCH-007）。
//! - 离线（远端 URL 不可达、无网络参与）从 seeds 恢复缺失/损坏依赖文件；
//! - 修复失败保持上一份 runtime 不被破坏；
//! - 换装成功后损坏旧目录进 quarantine；
//! - 并发 repair 只有一个执行者（复用单实例锁，锁被持有时拒绝动作）；
//! - 端到端：自造无签名 manifest→ doctor 报缺 →
//!   repair 离线恢复 → doctor 通过（与 CLI 实跑同一条代码路径）。

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{build_zip, cleanup, sha256_hex, unique_root, ZipEntrySpec};
use gamer_updater::inventory::{CheckOptions, ComponentSpec, ComponentStatus};
use gamer_updater::layout::InstallLayout;
use gamer_updater::repair::{
    repair_with_lock, verify_app_dir, AppInstallSpec, AppOutcome, ComponentOutcome, RepairGate,
    RepairOptions,
};
use gamer_updater::state::atomic::LoadOutcome;
use gamer_updater::state::lock::InstanceLock;
use gamer_updater::state::StateStore;

const A_EXE: &[u8] = b"adb-exe-content-v1";
const B_DLL: &[u8] = b"adb-dll-content-v1";

/// 组件夹具：zip（seeds 用）+ ComponentSpec（远端 URL 指向不可达地址，保证离线）。
fn component_fixture(id: &str, version: &str) -> (ComponentSpec, PathBuf) {
    let root = unique_root("comp-fixture");
    let name = format!("{id}-{version}.zip");
    let zip_path = root.join(&name);
    build_zip(
        &zip_path,
        &[
            ZipEntrySpec::file("adb.exe", A_EXE),
            ZipEntrySpec::file("AdbWinApi.dll", B_DLL),
        ],
    );
    let zip_bytes = fs::read(&zip_path).unwrap();
    let spec = ComponentSpec {
        id: id.to_string(),
        version: version.to_string(),
        files: vec![
            gamer_updater::inventory::FileSpec {
                path: "adb.exe".to_string(),
                size: A_EXE.len() as u64,
                sha256: sha256_hex(A_EXE),
            },
            gamer_updater::inventory::FileSpec {
                path: "AdbWinApi.dll".to_string(),
                size: B_DLL.len() as u64,
                sha256: sha256_hex(B_DLL),
            },
        ],
        artifact_name: name,
        artifact_sha256: sha256_hex(&zip_bytes),
        artifact_size: zip_bytes.len() as u64,
        // 不可达域名（NXDOMAIN，快速失败）：seed 命中时绝不触网，miss 时离线失败
        artifact_url: "https://qa002-unreachable.invalid/comp.zip".to_string(),
    };
    (spec, zip_path)
}

fn setup(tag: &str) -> InstallLayout {
    let root = unique_root(tag);
    InstallLayout { root }
}

fn put_seed(layout: &InstallLayout, zip_path: &Path, name: &str) {
    fs::create_dir_all(layout.seeds_dir()).unwrap();
    fs::copy(zip_path, layout.seeds_dir().join(name)).unwrap();
}

const APP_EXE: &[u8] = b"placeholder-gamer-server-exe-v1";
const APP_JAR: &[u8] = b"placeholder-scrcpy-server-jar-v1";

/// app 组件夹具：versions/<v>/ 形态的 zip（entrypoint + scrcpy jar + web-dist）
/// + AppInstallSpec（远端 URL 不可达，保证离线）。
fn app_fixture(version: &str) -> (AppInstallSpec, PathBuf) {
    let root = unique_root("app-fixture");
    let name = format!("gamer-app-{version}-windows-x64.zip");
    let zip_path = root.join(&name);
    build_zip(
        &zip_path,
        &[
            ZipEntrySpec::file("gamer-server.exe", APP_EXE),
            ZipEntrySpec::dir("assets"),
            ZipEntrySpec::file("assets/scrcpy-server.jar", APP_JAR),
            ZipEntrySpec::dir("web-dist"),
            ZipEntrySpec::file("web-dist/index.html", b"<html></html>"),
        ],
    );
    let zip_bytes = fs::read(&zip_path).unwrap();
    let spec = AppInstallSpec {
        version: version.to_string(),
        entrypoint: "gamer-server.exe".to_string(),
        artifact_name: name,
        artifact_sha256: sha256_hex(&zip_bytes),
        artifact_size: zip_bytes.len() as u64,
        artifact_url: "https://qa002-unreachable.invalid/app.zip".to_string(),
        scrcpy_path: "assets/scrcpy-server.jar".to_string(),
        scrcpy_sha256: sha256_hex(APP_JAR),
    };
    (spec, zip_path)
}

fn install_broken(layout: &InstallLayout, spec: &ComponentSpec, mode: Broken) {
    let dir = spec.install_dir(layout);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("adb.exe"), A_EXE).unwrap();
    match mode {
        Broken::MissingDll => { /* AdbWinApi.dll 不写，模拟缺 DLL */ }
        Broken::CorruptDll => fs::write(dir.join("AdbWinApi.dll"), b"garbage-bytes").unwrap(),
    }
}

#[derive(Clone, Copy)]
enum Broken {
    MissingDll,
    CorruptDll,
}

fn check_ok(layout: &InstallLayout, spec: &ComponentSpec) -> bool {
    gamer_updater::inventory::check_installed(
        layout,
        spec,
        CheckOptions {
            deep: true,
            probe: false,
        },
    )
    .status
        == ComponentStatus::Ok
}

/// 回归（M1 首轮 E-3 第 2 步）：全新安装根（runtime/ 不存在）repair 必须自建
/// `runtime/<id>/` 父目录后 rename 到位——曾因 fs::rename 不建父目录报 os error 3。
#[test]
fn fresh_install_root_repair_creates_runtime_parent_dirs() {
    let layout = setup("repair-fresh-root");
    let (spec, zip_path) = component_fixture("adb", "1.0.0");
    put_seed(&layout, &zip_path, &spec.artifact_name);
    assert!(
        !layout.runtime_dir().exists(),
        "前置：全新安装根没有 runtime/"
    );

    let report = repair_with_lock(
        &layout,
        std::slice::from_ref(&spec),
        None,
        &RepairOptions::default(),
    )
    .expect("repair 应取到锁");
    assert_eq!(report.failed_count(), 0);
    assert_eq!(
        report.components[0].outcome,
        ComponentOutcome::Repaired {
            source: "seed".to_string()
        }
    );
    assert!(check_ok(&layout, &spec), "首装 repair 应直接成功");
    cleanup(&layout.root);
    cleanup(zip_path.parent().unwrap());
}

#[test]
fn repair_installs_app_from_seed_and_writes_current_pointer() {
    // 阻断缺陷 #2 回归：repair/首装必须安装 app 组件并写 state/current.json
    let layout = setup("repair-app");
    let (app, app_zip) = app_fixture("0.2.0");
    put_seed(&layout, &app_zip, &app.artifact_name);
    assert!(!layout.versions_dir().exists());

    let report =
        repair_with_lock(&layout, &[], Some(&app), &RepairOptions::default()).expect("应取到锁");
    assert_eq!(report.failed_count(), 0);
    match &report.app.as_ref().expect("app 结果应存在").outcome {
        AppOutcome::Installed { source } => assert_eq!(source, "seed"),
        other => panic!("应从 seed 新装成功，实际 {other:?}"),
    }
    let dir = app.install_dir(&layout);
    assert!(dir.join("gamer-server.exe").is_file(), "entrypoint 应存在");
    assert!(
        verify_app_dir(&dir, &app).is_ok(),
        "entrypoint + jar hash 应通过"
    );
    match StateStore::new(&layout.root).load_current().unwrap() {
        LoadOutcome::Present(c) => {
            assert_eq!(c.current, "0.2.0", "版本指针应指向刚安装的版本");
            assert_eq!(c.schema_version, gamer_updater::state::STATE_SCHEMA_VERSION);
        }
        other => panic!("state/current.json 应存在，实际 {other:?}"),
    }
    cleanup(&layout.root);
    cleanup(app_zip.parent().unwrap());
}

#[test]
fn repair_app_second_run_reports_healthy_without_overwrite() {
    // 契约 §2：版本目录安装成功后不可变——第二次 repair 报 Healthy，不原地覆盖
    let layout = setup("repair-app-healthy");
    let (app, app_zip) = app_fixture("0.2.0");
    put_seed(&layout, &app_zip, &app.artifact_name);
    repair_with_lock(&layout, &[], Some(&app), &RepairOptions::default()).unwrap();

    let marker = app.install_dir(&layout).join("web-dist").join("index.html");
    let before = fs::read(&marker).unwrap();
    let report = repair_with_lock(&layout, &[], Some(&app), &RepairOptions::default()).unwrap();
    assert_eq!(
        report.app.as_ref().unwrap().outcome,
        AppOutcome::Healthy,
        "已装且完好应报 Healthy"
    );
    assert_eq!(fs::read(&marker).unwrap(), before, "版本目录内容不得被动");
    cleanup(&layout.root);
    cleanup(app_zip.parent().unwrap());
}

#[test]
fn repair_app_failure_preserves_existing_dir() {
    // 无 seed/cache 且远端不可达：app 安装失败，既有（损坏）版本目录保持原样
    let layout = setup("repair-app-fail");
    let (app, app_zip) = app_fixture("0.2.0");
    let dir = app.install_dir(&layout);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("gamer-server.exe"), b"broken-exe").unwrap();

    let report = repair_with_lock(&layout, &[], Some(&app), &RepairOptions::default()).unwrap();
    match &report.app.as_ref().unwrap().outcome {
        AppOutcome::Failed { reason } => assert!(reason.contains("获取应用产物失败")),
        other => panic!("应失败，实际 {other:?}"),
    }
    assert_eq!(
        fs::read(dir.join("gamer-server.exe")).unwrap(),
        b"broken-exe",
        "既有版本目录不得被动"
    );
    assert!(
        !matches!(
            StateStore::new(&layout.root).load_current().unwrap(),
            LoadOutcome::Present(_)
        ),
        "安装失败不得写版本指针"
    );
    cleanup(&layout.root);
    cleanup(app_zip.parent().unwrap());
}

#[test]
fn repair_restores_changed_web_resources_and_missing_executable() {
    let layout = setup("repair-all-app-files");
    let (app, zip) = app_fixture("0.2.0");
    put_seed(&layout, &zip, &app.artifact_name);
    repair_with_lock(&layout, &[], Some(&app), &Default::default()).unwrap();
    let html = app.install_dir(&layout).join("web-dist/index.html");
    fs::write(&html, b"<html>broken!</html>").unwrap();
    let report = repair_with_lock(&layout, &[], Some(&app), &Default::default()).unwrap();
    assert_eq!(report.failed_count(), 0);
    assert!(matches!(
        report.app.unwrap().outcome,
        AppOutcome::Installed { .. }
    ));
    assert_eq!(fs::read(&html).unwrap(), b"<html></html>");
    fs::remove_file(app.install_dir(&layout).join("gamer-server.exe")).unwrap();
    let report = repair_with_lock(&layout, &[], Some(&app), &Default::default()).unwrap();
    assert_eq!(report.failed_count(), 0);
    assert_eq!(
        fs::read(app.install_dir(&layout).join("gamer-server.exe")).unwrap(),
        APP_EXE
    );
    cleanup(&layout.root);
    cleanup(zip.parent().unwrap());
}

#[test]
fn offline_repair_restores_missing_dll_from_seed() {
    let layout = setup("repair-missing");
    let (spec, zip_path) = component_fixture("adb", "1.0.0");
    put_seed(&layout, &zip_path, &spec.artifact_name);
    install_broken(&layout, &spec, Broken::MissingDll);
    assert!(!check_ok(&layout, &spec), "修复前应检出不完整");

    let report = repair_with_lock(
        &layout,
        std::slice::from_ref(&spec),
        None,
        &RepairOptions::default(),
    )
    .expect("repair 应取到锁");
    assert_eq!(report.failed_count(), 0);
    assert_eq!(
        report.components[0].outcome,
        ComponentOutcome::Repaired {
            source: "seed".to_string()
        },
        "离线修复应来自 seed"
    );
    assert!(check_ok(&layout, &spec), "修复后深检应通过");
    assert_eq!(
        fs::read(spec.install_dir(&layout).join("AdbWinApi.dll")).unwrap(),
        B_DLL,
        "恢复出的 DLL 字节应与声明 hash 一致"
    );
    // staging 清理干净
    assert!(!layout
        .staging_dir()
        .join(format!("repair-{}-{}", spec.id, spec.version))
        .exists());
    cleanup(&layout.root);
    cleanup(zip_path.parent().unwrap());
}

#[test]
fn offline_repair_restores_corrupted_file_from_seed() {
    let layout = setup("repair-corrupt");
    let (spec, zip_path) = component_fixture("adb", "1.0.0");
    put_seed(&layout, &zip_path, &spec.artifact_name);
    install_broken(&layout, &spec, Broken::CorruptDll);

    let report = repair_with_lock(
        &layout,
        std::slice::from_ref(&spec),
        None,
        &RepairOptions::default(),
    )
    .unwrap();
    assert_eq!(report.failed_count(), 0);
    assert!(check_ok(&layout, &spec));
    cleanup(&layout.root);
    cleanup(zip_path.parent().unwrap());
}

#[test]
fn repair_failure_preserves_previous_runtime() {
    // 无 seed/cache，远端不可达 → 修复失败；上一份（损坏）runtime 必须原样保留
    let layout = setup("repair-fail");
    let (spec, zip_path) = component_fixture("adb", "1.0.0");
    install_broken(&layout, &spec, Broken::CorruptDll);
    let dll = spec.install_dir(&layout).join("AdbWinApi.dll");
    let before = fs::read(&dll).unwrap();

    let report = repair_with_lock(
        &layout,
        std::slice::from_ref(&spec),
        None,
        &RepairOptions::default(),
    )
    .unwrap();
    assert_eq!(report.failed_count(), 1);
    match &report.components[0].outcome {
        ComponentOutcome::Failed { reason } => assert!(reason.contains("获取组件产物失败")),
        other => panic!("应失败，实际 {other:?}"),
    }
    assert_eq!(
        fs::read(&dll).unwrap(),
        before,
        "上一份 runtime 文件不得被动"
    );
    // staging 无残留
    if layout.staging_dir().is_dir() {
        let leftovers: Vec<String> = fs::read_dir(layout.staging_dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .collect();
        assert!(leftovers.is_empty(), "staging 应清理干净: {leftovers:?}");
    }
    cleanup(&layout.root);
    cleanup(zip_path.parent().unwrap());
}

#[test]
fn successful_repair_quarantines_damaged_dir() {
    let layout = setup("repair-quarantine");
    let (spec, zip_path) = component_fixture("adb", "1.0.0");
    put_seed(&layout, &zip_path, &spec.artifact_name);
    install_broken(&layout, &spec, Broken::CorruptDll);

    let report = repair_with_lock(
        &layout,
        std::slice::from_ref(&spec),
        None,
        &RepairOptions::default(),
    )
    .unwrap();
    assert_eq!(report.failed_count(), 0);
    // 损坏旧目录应被保留在 quarantine（契约：不静默删除）
    let q = layout.quarantine_dir();
    let entries: Vec<String> = fs::read_dir(&q)
        .expect("quarantine 应存在")
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect();
    assert!(
        entries.iter().any(|n| n.starts_with("adb-1.0.0-")),
        "损坏目录应进 quarantine: {entries:?}"
    );
    cleanup(&layout.root);
    cleanup(zip_path.parent().unwrap());
}

#[test]
fn healthy_component_reports_no_repair() {
    let layout = setup("repair-healthy");
    let (spec, zip_path) = component_fixture("adb", "1.0.0");
    // 直接用 zip 内容装出完好目录
    install_broken(&layout, &spec, Broken::CorruptDll);
    put_seed(&layout, &zip_path, &spec.artifact_name);
    repair_with_lock(
        &layout,
        std::slice::from_ref(&spec),
        None,
        &RepairOptions::default(),
    )
    .unwrap();
    // 第二次：已完好
    let report = repair_with_lock(
        &layout,
        std::slice::from_ref(&spec),
        None,
        &RepairOptions::default(),
    )
    .unwrap();
    assert_eq!(report.components[0].outcome, ComponentOutcome::Healthy);
    cleanup(&layout.root);
    cleanup(zip_path.parent().unwrap());
}

#[test]
fn concurrent_repair_single_executor_via_lock() {
    let layout = setup("repair-locked");
    let (spec, zip_path) = component_fixture("adb", "1.0.0");
    install_broken(&layout, &spec, Broken::CorruptDll);

    // 另一个 launcher 实例先持有单实例锁
    let _foreign_lock = InstanceLock::acquire(&layout.state_dir()).expect("外部实例应能取锁");
    let result = repair_with_lock(
        &layout,
        std::slice::from_ref(&spec),
        None,
        &RepairOptions::default(),
    );
    match result {
        Err(RepairGate::Locked { path }) => {
            assert_eq!(path, layout.state_dir().join("launcher.lock"));
        }
        other => panic!("锁被持有时 repair 必须拒绝执行，实际 {other:?}"),
    }
    // 未执行任何动作：runtime 原样、无 staging、无 quarantine
    assert_eq!(
        fs::read(spec.install_dir(&layout).join("AdbWinApi.dll")).unwrap(),
        b"garbage-bytes"
    );
    assert!(!layout.staging_dir().exists());
    assert!(!layout.quarantine_dir().exists());
    cleanup(&layout.root);
    cleanup(zip_path.parent().unwrap());
}

// -- 端到端（manifest + CLI 分发） ----------------------------------------
