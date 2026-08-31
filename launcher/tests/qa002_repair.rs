//! QA-002：repair 修复编排测试（LCH-007）。
//! - 离线（远端 URL 不可达、无网络参与）从 seeds 恢复缺失/损坏依赖文件；
//! - 修复失败保持上一份 runtime 不被破坏；
//! - 换装成功后损坏旧目录进 quarantine；
//! - 并发 repair 只有一个执行者（复用单实例锁，锁被持有时拒绝动作）；
//! - 端到端：自造签名 manifest（测试专用 Ed25519 key）→ doctor 报缺 →
//!   repair 离线恢复 → doctor 通过（与 CLI 实跑同一条代码路径）。

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use clap::Parser as _;
use common::{build_zip, cleanup, sha256_hex, unique_root, ZipEntrySpec};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use gamer_launcher::cli::Cli;
use gamer_launcher::commands;
use gamer_launcher::inventory::{CheckOptions, ComponentSpec, ComponentStatus};
use gamer_launcher::layout::InstallLayout;
use gamer_launcher::repair::{
    repair_with_lock, verify_app_dir, AppInstallSpec, AppOutcome, ComponentOutcome, RepairGate,
    RepairOptions,
};
use gamer_launcher::state::atomic::LoadOutcome;
use gamer_launcher::state::lock::InstanceLock;
use gamer_launcher::state::{CurrentState, StateStore};

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
            gamer_launcher::inventory::FileSpec {
                path: "adb.exe".to_string(),
                size: A_EXE.len() as u64,
                sha256: sha256_hex(A_EXE),
            },
            gamer_launcher::inventory::FileSpec {
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
    gamer_launcher::inventory::check_installed(
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
            assert_eq!(
                c.schema_version,
                gamer_launcher::state::STATE_SCHEMA_VERSION
            );
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

// -- 端到端（签名 manifest + CLI 分发） ----------------------------------------

const DEMO_KEY_SEED: [u8; 32] = *b"qa002-demo-key-seed-0123456789ab";
const DEMO_KEY_ID: &str = "qa002-demo-key-1";

/// 构造 Ed25519 SPKI PEM（fixture 专用测试 key；launcher 的 PEM 解析器可直接消费）。
fn demo_key_pem(verifying: &VerifyingKey) -> String {
    let raw = verifying.as_bytes();
    let mut der = vec![
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    der.extend_from_slice(raw);
    let b64 = B64.encode(&der);
    let mut pem = String::from("-----BEGIN PUBLIC KEY-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).unwrap());
        pem.push('\n');
    }
    pem.push_str("-----END PUBLIC KEY-----\n");
    pem
}

/// 生成一份已签名的 release manifest（内容指向 component/app fixture 的产物）。
fn signed_manifest(layout: &InstallLayout, spec: &ComponentSpec, app: &AppInstallSpec) {
    let app_version = app.version.as_str();
    let component_json = format!(
        r#"{{
            "id": "adb",
            "version": "{}",
            "artifact": {{
                "name": "{}",
                "url": "{}",
                "size": {},
                "sha256": "{}"
            }},
            "required_files": [
                {{ "path": "adb.exe", "size": {}, "sha256": "{}" }},
                {{ "path": "AdbWinApi.dll", "size": {}, "sha256": "{}" }}
            ]
        }}"#,
        spec.version,
        spec.artifact_name,
        spec.artifact_url,
        spec.artifact_size,
        spec.artifact_sha256,
        spec.files[0].size,
        spec.files[0].sha256,
        spec.files[1].size,
        spec.files[1].sha256,
    );
    let manifest = format!(
        r#"{{
  "schema_version": 1,
  "product": "gamebot",
  "release": {{
    "version": "{app_version}",
    "channel": "stable",
    "published_at": "2026-08-31T00:00:00Z",
    "minimum_launcher_version": "0.1.0",
    "minimum_upgrade_version": "0.1.0",
    "data_schema": 1,
    "rollback_floor": 1,
    "release_notes_url": "https://example.invalid/releases"
  }},
  "platforms": {{
    "windows-x86_64": {{
      "app": {{
        "artifact": {{
          "name": "{app_artifact_name}",
          "url": "{app_artifact_url}",
          "size": {app_artifact_size},
          "sha256": "{app_artifact_sha}"
        }},
        "entrypoint": "{app_entrypoint}"
      }},
      "components": [{component_json}],
      "resources": {{
        "scrcpy_server": {{
          "version": "3.3.3",
          "path": "{app_scrcpy_path}",
          "sha256": "{app_scrcpy_sha}",
          "binding": "application"
        }}
      }}
    }}
  }}
}}"#,
        app_version = app_version,
        component_json = component_json,
        app_artifact_name = app.artifact_name,
        app_artifact_url = app.artifact_url,
        app_artifact_size = app.artifact_size,
        app_artifact_sha = app.artifact_sha256,
        app_entrypoint = app.entrypoint,
        app_scrcpy_path = app.scrcpy_path,
        app_scrcpy_sha = app.scrcpy_sha256,
    );

    let signing = SigningKey::from_bytes(&DEMO_KEY_SEED);
    let raw = manifest.as_bytes();
    let signature = signing.sign(raw);

    fs::create_dir_all(layout.manifests_dir()).unwrap();
    fs::write(
        layout.manifests_dir().join(format!("{app_version}.json")),
        raw,
    )
    .unwrap();
    let sig_text = format!(
        "gamebot-manifest-sig-1 {DEMO_KEY_ID}\n{}\n",
        B64.encode(signature.to_bytes())
    );
    fs::write(
        layout.manifests_dir().join(format!("{app_version}.sig")),
        sig_text,
    )
    .unwrap();
    fs::create_dir_all(layout.root.join("keys")).unwrap();
    fs::write(
        layout.root.join("keys").join(format!("{DEMO_KEY_ID}.pem")),
        demo_key_pem(&VerifyingKey::from(&signing)),
    )
    .unwrap();
}

fn cli(args: &[&str]) -> Cli {
    Cli::parse_from(args)
}

#[test]
fn end_to_end_signed_manifest_doctor_reports_missing_then_repair_then_doctor_passes() {
    let layout = setup("e2e");
    let (spec, zip_path) = component_fixture("adb", "1.0.0");
    let (app, app_zip) = app_fixture("0.2.0");
    put_seed(&layout, &zip_path, &spec.artifact_name);
    put_seed(&layout, &app_zip, &app.artifact_name);
    install_broken(&layout, &spec, Broken::MissingDll);
    // 已安装态（版本指针已写）：doctor 对组件缺失仍应 FAIL（首装态 WARN 语义不适用于此）
    StateStore::new(&layout.root)
        .write_current(&CurrentState::new("0.2.0", None))
        .unwrap();
    signed_manifest(&layout, &spec, &app);

    let root_s = layout.root.to_string_lossy().into_owned();

    // doctor --deep：应报缺并返回非 0（组件缺 DLL + app 版本目录缺失）
    let code = commands::dispatch(
        &cli(&[
            "gamer-launcher",
            "--install-root",
            &root_s,
            "doctor",
            "--deep",
        ]),
        &layout,
    );
    assert_eq!(code, 1, "缺 DLL 时 doctor --deep 应失败");

    // repair（不指定 --manifest，走 manifests/ 缓存 + <root>/keys 信任库）：
    // 离线恢复 adb + 安装 app + 写版本指针，一步到位
    let code = commands::dispatch(
        &cli(&["gamer-launcher", "--install-root", &root_s, "repair"]),
        &layout,
    );
    assert_eq!(code, 0, "离线 repair 应成功");

    // doctor --deep：应全过
    let code = commands::dispatch(
        &cli(&[
            "gamer-launcher",
            "--install-root",
            &root_s,
            "doctor",
            "--deep",
        ]),
        &layout,
    );
    assert_eq!(code, 0, "修复后 doctor --deep 应通过");
    assert!(check_ok(&layout, &spec));
    assert!(verify_app_dir(&app.install_dir(&layout), &app).is_ok());
    match StateStore::new(&layout.root).load_current().unwrap() {
        LoadOutcome::Present(c) => assert_eq!(c.current, "0.2.0"),
        other => panic!("repair 后版本指针应存在，实际 {other:?}"),
    }

    // 再坏一个文件 + probe 开启（非 adb/ffmpeg 探针为 Unsupported，不影响判定）
    let dll = spec.install_dir(&layout).join("AdbWinApi.dll");
    fs::write(&dll, b"corrupted").unwrap();
    let code = commands::dispatch(
        &cli(&[
            "gamer-launcher",
            "--install-root",
            &root_s,
            "repair",
            "--probe",
        ]),
        &layout,
    );
    assert_eq!(code, 0, "--probe 修复应成功");
    cleanup(&layout.root);
    cleanup(zip_path.parent().unwrap());
    cleanup(app_zip.parent().unwrap());
}

/// doctor 首装态语义：全新解压根（无 state/、无 current.json）→ WARN「未安装，
/// 先运行 repair」且退出码 0（不把「尚未安装」当故障）；repair 完成后 → 全 PASS。
#[test]
fn doctor_reports_fresh_root_as_never_installed_warn_not_fail() {
    let layout = setup("doctor-fresh");
    let (spec, zip_path) = component_fixture("adb", "1.0.0");
    let (app, app_zip) = app_fixture("0.2.0");
    put_seed(&layout, &zip_path, &spec.artifact_name);
    put_seed(&layout, &app_zip, &app.artifact_name);
    signed_manifest(&layout, &spec, &app);
    let root_s = layout.root.to_string_lossy().into_owned();

    // 首装态：退出码 0，输出含未安装 WARN（含 state/ 缺失 WARN）与 repair 指引
    let cli_args = cli(&["gamer-launcher", "--install-root", &root_s, "doctor"]);
    let (lines, code) = commands::doctor_inventory_report(&layout, &cli_args, false, false, None);
    assert_eq!(
        code, 0,
        "从未安装的根 doctor 不应 FAIL（退出码 1），实际输出:\n{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("[WARN]") && l.contains("未安装")),
        "应输出「未安装」WARN:\n{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("repair")),
        "WARN 应提示先运行 repair:\n{lines:?}"
    );

    // 已安装后组件缺失仍 FAIL（语义保持）：先造出缺 DLL 的 adb + 指针
    install_broken(&layout, &spec, Broken::MissingDll);
    StateStore::new(&layout.root)
        .write_current(&CurrentState::new("0.2.0", None))
        .unwrap();
    let cli_args = cli(&["gamer-launcher", "--install-root", &root_s, "doctor"]);
    let (lines, code) = commands::doctor_inventory_report(&layout, &cli_args, false, false, None);
    assert_eq!(code, 1, "已安装后组件缺失应 FAIL");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("[FAIL]") && l.contains("AdbWinApi.dll")),
        "应定位到缺失文件:\n{lines:?}"
    );

    // repair 一步到位 → doctor 全 PASS
    let code = commands::dispatch(
        &cli(&["gamer-launcher", "--install-root", &root_s, "repair"]),
        &layout,
    );
    assert_eq!(code, 0);
    let cli_args = cli(&["gamer-launcher", "--install-root", &root_s, "doctor"]);
    let (lines, code) = commands::doctor_inventory_report(&layout, &cli_args, false, false, None);
    assert_eq!(code, 0, "repair 后 doctor 应全 PASS:\n{lines:?}");
    assert!(
        lines.iter().any(|l| l.starts_with("[PASS] app 0.2.0")),
        "已安装后应含 app 版本目录 quick 检查 PASS:\n{lines:?}"
    );
    cleanup(&layout.root);
    cleanup(zip_path.parent().unwrap());
    cleanup(app_zip.parent().unwrap());
}

/// 手工验收材料化（默认跳过；`cargo test -- --ignored` 显式执行）：
/// 在 GAMER_LAUNCHER_DEMO_ROOT（缺省 <crate>/target/demo-install）物化完整演示
/// 安装根——签名 manifest + keys/ + seeds/（release/vendor 真实产物重打包）+
/// 已损坏的 runtime/adb（删除 AdbWinApi.dll）。随后可用真实 CLI 跑：
/// doctor --deep（报缺）→ repair --probe（离线恢复 + 真实探针）→ doctor --deep（通过）。
#[test]
#[ignore = "手工验收材料化：cargo test --test qa002_repair materialize_demo -- --ignored"]
fn materialize_demo_install_root() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = crate_dir.parent().unwrap();
    let vendor = repo_root.join("release").join("vendor");
    let adb_dir = vendor.join("adb").join("37.0.1");
    let ffmpeg_dir = vendor.join("ffmpeg").join("N-126335-gb32f8d1c23-20260830");
    assert!(
        adb_dir.is_dir(),
        "演示材料化依赖 release/vendor 的 adb 产物（{adb_dir:?}）"
    );
    assert!(
        ffmpeg_dir.is_dir(),
        "演示材料化依赖 release/vendor 的 ffmpeg 产物（{ffmpeg_dir:?}）"
    );

    let root = std::env::var("GAMER_LAUNCHER_DEMO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate_dir.join("target").join("demo-install"));
    let _ = fs::remove_dir_all(&root);
    let layout = InstallLayout { root: root.clone() };

    // versions/0.2.0 + state/current.json（占位 server：供后续 start 链路观察）
    let app_dir = layout.versions_dir().join("0.2.0");
    fs::create_dir_all(app_dir.join("assets")).unwrap();
    fs::write(
        app_dir.join("gamer-server.exe"),
        b"placeholder-not-runnable",
    )
    .unwrap();
    gamer_launcher::state::StateStore::new(&layout.root)
        .write_current(&gamer_launcher::state::CurrentState::new("0.2.0", None))
        .unwrap();

    // runtime/ffmpeg（完好）+ runtime/adb（删除 AdbWinApi.dll 制造故障）
    let adb_files = ["adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll"];
    let mut adb_contents = Vec::new();
    for f in adb_files {
        let bytes = fs::read(adb_dir.join(f)).unwrap();
        adb_contents.push(bytes.clone());
        fs::create_dir_all(layout.component_dir("adb", "37.0.1")).unwrap();
        fs::write(layout.component_dir("adb", "37.0.1").join(f), bytes).unwrap();
    }
    fs::remove_file(layout.component_dir("adb", "37.0.1").join("AdbWinApi.dll")).unwrap();

    let ffmpeg_bytes = fs::read(ffmpeg_dir.join("ffmpeg.exe")).unwrap();
    fs::create_dir_all(layout.component_dir("ffmpeg", "N-126335-gb32f8d1c23-20260830")).unwrap();
    fs::write(
        layout
            .component_dir("ffmpeg", "N-126335-gb32f8d1c23-20260830")
            .join("ffmpeg.exe"),
        &ffmpeg_bytes,
    )
    .unwrap();

    // seeds：adb 三件 zip + ffmpeg zip（manifest hash 与 zip 内容一致）
    let mut adb_entries = Vec::new();
    for (f, bytes) in adb_files.iter().zip(&adb_contents) {
        adb_entries.push(ZipEntrySpec::file(f, bytes));
    }
    fs::create_dir_all(layout.seeds_dir()).unwrap();
    build_zip(
        &layout.seeds_dir().join("gamer-adb-37.0.1-windows-x64.zip"),
        &adb_entries,
    );
    build_zip(
        &layout
            .seeds_dir()
            .join("gamer-ffmpeg-N-126335-windows-x64.zip"),
        &[ZipEntrySpec::file("ffmpeg.exe", &ffmpeg_bytes)],
    );

    // manifest（两组件，hash/size 全部实算）+ 测试签名 + keys/
    let adb_zip = fs::read(layout.seeds_dir().join("gamer-adb-37.0.1-windows-x64.zip")).unwrap();
    let ffmpeg_zip = fs::read(
        layout
            .seeds_dir()
            .join("gamer-ffmpeg-N-126335-windows-x64.zip"),
    )
    .unwrap();
    let hash = |b: &[u8]| sha256_hex(b);
    let manifest = format!(
        r#"{{
  "schema_version": 1,
  "product": "gamebot",
  "release": {{
    "version": "0.2.0",
    "channel": "stable",
    "published_at": "2026-08-31T00:00:00Z",
    "minimum_launcher_version": "0.1.0",
    "minimum_upgrade_version": "0.1.0",
    "data_schema": 1,
    "rollback_floor": 1,
    "release_notes_url": "https://example.invalid/releases"
  }},
  "platforms": {{
    "windows-x86_64": {{
      "app": {{
        "artifact": {{
          "name": "gamer-app-0.2.0-windows-x64.zip",
          "url": "https://demo-unreachable.invalid/app.zip",
          "size": 1,
          "sha256": "{placeholder}"
        }},
        "entrypoint": "gamer-server.exe"
      }},
      "components": [
        {{
          "id": "adb",
          "version": "37.0.1",
          "artifact": {{
            "name": "gamer-adb-37.0.1-windows-x64.zip",
            "url": "https://demo-unreachable.invalid/adb.zip",
            "size": {adb_zip_size},
            "sha256": "{adb_zip_hash}"
          }},
          "required_files": [
            {{ "path": "adb.exe", "size": {adb_exe_size}, "sha256": "{adb_exe_hash}" }},
            {{ "path": "AdbWinApi.dll", "size": {adb_dll_size}, "sha256": "{adb_dll_hash}" }},
            {{ "path": "AdbWinUsbApi.dll", "size": {adb_usb_size}, "sha256": "{adb_usb_hash}" }}
          ]
        }},
        {{
          "id": "ffmpeg",
          "version": "N-126335-gb32f8d1c23-20260830",
          "artifact": {{
            "name": "gamer-ffmpeg-N-126335-windows-x64.zip",
            "url": "https://demo-unreachable.invalid/ffmpeg.zip",
            "size": {ffmpeg_zip_size},
            "sha256": "{ffmpeg_zip_hash}"
          }},
          "required_files": [
            {{ "path": "ffmpeg.exe", "size": {ffmpeg_size}, "sha256": "{ffmpeg_hash}" }}
          ]
        }}
      ],
      "resources": {{
        "scrcpy_server": {{
          "version": "3.3.3",
          "path": "assets/scrcpy-server.jar",
          "sha256": "{placeholder}",
          "binding": "application"
        }}
      }}
    }}
  }}
}}"#,
        placeholder = hash(&[0]),
        adb_zip_size = adb_zip.len(),
        adb_zip_hash = hash(&adb_zip),
        adb_exe_size = adb_contents[0].len(),
        adb_exe_hash = hash(&adb_contents[0]),
        adb_dll_size = adb_contents[1].len(),
        adb_dll_hash = hash(&adb_contents[1]),
        adb_usb_size = adb_contents[2].len(),
        adb_usb_hash = hash(&adb_contents[2]),
        ffmpeg_zip_size = ffmpeg_zip.len(),
        ffmpeg_zip_hash = hash(&ffmpeg_zip),
        ffmpeg_size = ffmpeg_bytes.len(),
        ffmpeg_hash = hash(&ffmpeg_bytes),
    );

    let signing = SigningKey::from_bytes(&DEMO_KEY_SEED);
    fs::create_dir_all(layout.manifests_dir()).unwrap();
    fs::write(
        layout.manifests_dir().join("0.2.0.json"),
        manifest.as_bytes(),
    )
    .unwrap();
    let signature = signing.sign(manifest.as_bytes());
    fs::write(
        layout.manifests_dir().join("0.2.0.sig"),
        format!(
            "gamebot-manifest-sig-1 {DEMO_KEY_ID}\n{}\n",
            B64.encode(signature.to_bytes())
        ),
    )
    .unwrap();
    fs::create_dir_all(layout.root.join("keys")).unwrap();
    fs::write(
        layout.root.join("keys").join(format!("{DEMO_KEY_ID}.pem")),
        demo_key_pem(&VerifyingKey::from(&signing)),
    )
    .unwrap();

    println!("演示安装根已物化: {}", layout.root.display());
    println!("验证步骤（真实 CLI）：");
    println!("  1. gamer-launcher --install-root <demo> doctor --deep   # 应报 AdbWinApi.dll 缺失，退出码 1");
    println!("  2. gamer-launcher --install-root <demo> repair --probe  # 应从 seed 离线恢复 adb，ffmpeg 探针匹配，退出码 0");
    println!("  3. gamer-launcher --install-root <demo> doctor --deep   # 应全过，退出码 0");
}
