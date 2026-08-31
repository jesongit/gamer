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
use gamer_launcher::repair::{repair_with_lock, ComponentOutcome, RepairGate, RepairOptions};
use gamer_launcher::state::lock::InstanceLock;

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
        &RepairOptions::default(),
    )
    .unwrap();
    // 第二次：已完好
    let report = repair_with_lock(
        &layout,
        std::slice::from_ref(&spec),
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

/// 生成一份已签名的 release manifest（内容指向 component_fixture 的产物）。
fn signed_manifest(layout: &InstallLayout, spec: &ComponentSpec, app_version: &str) {
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
          "name": "app-{app_version}.zip",
          "url": "https://qa002-unreachable.invalid/app.zip",
          "size": 1,
          "sha256": "{sha}"
        }},
        "entrypoint": "gamer-server.exe"
      }},
      "components": [{component_json}],
      "resources": {{
        "scrcpy_server": {{
          "version": "3.3.3",
          "path": "assets/scrcpy-server.jar",
          "sha256": "{sha}",
          "binding": "application"
        }}
      }}
    }}
  }}
}}"#,
        app_version = app_version,
        component_json = component_json,
        sha = "a".repeat(64),
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
    put_seed(&layout, &zip_path, &spec.artifact_name);
    install_broken(&layout, &spec, Broken::MissingDll);
    signed_manifest(&layout, &spec, "0.2.0");

    let root_s = layout.root.to_string_lossy().into_owned();

    // doctor --deep：应报缺并返回非 0
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

    // repair（不指定 --manifest，走 manifests/ 缓存 + <root>/keys 信任库）
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
