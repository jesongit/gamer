//! QA-002：server supervisor 故障测试（LCH-008 + OPS-003）。
//! - env 注入与最小 PATH：假 exe（cmd.exe）实跑 `set` 验证 GAMER_*/GB_* 注入、
//!   PATH 收敛到 System32、父环境其余变量一律不带；
//! - OPS-003：持有子进程句柄等待退出，退出码精确回收（不按端口/进程名判定）；
//! - 就绪探测：真实 server 的 /health/ready 响应形态（200 ready / 503 not_ready，
//!   见 server/src/api/system.rs api_health_ready）用 std::net 夹具 HTTP 服务模拟，
//!   覆盖 503→200 翻转、永不就绪超时、连接拒绝；config.toml 端口解析；
//! - start 对空安装根 / 指针指向缺失版本给出清晰错误（退出码 1，不 panic）。

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser as _;
use common::{cleanup, http_server, unique_root, write_response};
use gamer_launcher::cli::Cli;
use gamer_launcher::commands;
use gamer_launcher::layout::InstallLayout;
use gamer_launcher::state::{CurrentState, StateStore};
use gamer_launcher::supervisor::{self, build_child_env, wait_for_ready, LaunchPlan, ReadyProbe};

fn setup(tag: &str) -> InstallLayout {
    let root = unique_root(tag);
    InstallLayout { root }
}

fn fake_plan(layout: &InstallLayout) -> LaunchPlan {
    let app_dir = layout.versions_dir().join("0.2.0");
    LaunchPlan {
        exe: PathBuf::from("cmd.exe"),
        cwd: app_dir.clone(),
        app_dir: app_dir.clone(),
        data_dir: layout.data_dir(),
        adb_path: Some(layout.component_dir("adb", "37.0.1").join("adb.exe")),
        ffmpeg_path: Some(layout.component_dir("ffmpeg", "N-1").join("ffmpeg.exe")),
        scrcpy_server: app_dir.join("assets").join("scrcpy-server.jar"),
        config_path: layout.config_file(),
        log_path: layout.logs_dir().join("gamer-server.log"),
    }
}

fn quick_probe() -> ReadyProbe {
    ReadyProbe {
        overall_timeout: Duration::from_millis(4_000),
        per_attempt_timeout: Duration::from_millis(1_000),
        interval: Duration::from_millis(100),
    }
}

#[test]
fn child_env_injects_contract_vars_with_minimal_path() {
    let layout = setup("sup-env");
    fs::create_dir_all(layout.versions_dir().join("0.2.0")).unwrap();
    let plan = fake_plan(&layout);

    // 纯函数视角：键集合 = 最小系统集 + 契约注入集，父环境其余一概不带
    let env = build_child_env(&plan);
    let allowed = [
        "PATH",
        "SystemRoot",
        "SystemDrive",
        "TEMP",
        "TMP",
        "GAMER_APP_DIR",
        "GAMER_DATA_DIR",
        "GAMER_ADB_PATH",
        "GAMER_FFMPEG_PATH",
        "GAMER_SCRCPY_SERVER",
        "GB_CONFIG",
        "GB_LOG",
    ];
    for key in env.keys() {
        assert!(
            allowed.contains(&key.as_str()),
            "意外注入子进程的环境变量: {key}"
        );
    }
    for key in [
        "GAMER_APP_DIR",
        "GAMER_DATA_DIR",
        "GAMER_ADB_PATH",
        "GAMER_FFMPEG_PATH",
        "GAMER_SCRCPY_SERVER",
        "GB_CONFIG",
        "GB_LOG",
    ] {
        assert!(env.contains_key(key), "缺少契约注入变量 {key}");
    }
    assert_eq!(
        env["PATH"].to_ascii_lowercase(),
        supervisor::minimal_path().to_ascii_lowercase(),
        "PATH 必须收敛到最小集（System32）"
    );
    cleanup(&layout.root);
}

#[test]
fn real_child_process_receives_injected_env_and_minimal_path() {
    let layout = setup("sup-env-real");
    fs::create_dir_all(layout.versions_dir().join("0.2.0")).unwrap();
    let plan = fake_plan(&layout);

    // 假 exe 实跑：cmd.exe /C set 打印子进程全部环境变量
    let child = supervisor::spawn_child(&plan, &["/C", "set"], Stdio::piped(), Stdio::null())
        .expect("spawn cmd.exe 应成功");
    let out = child.wait_with_output().expect("等待 cmd /c set");
    let text = String::from_utf8_lossy(&out.stdout);

    let expect = |needle: &str| {
        assert!(
            text.contains(needle),
            "子进程环境缺少 {needle}；实际输出:\n{text}"
        );
    };
    expect(&format!("GAMER_APP_DIR={}", plan.app_dir.display()));
    expect(&format!("GAMER_DATA_DIR={}", plan.data_dir.display()));
    expect(&format!(
        "GAMER_ADB_PATH={}",
        plan.adb_path.as_ref().unwrap().display()
    ));
    expect(&format!(
        "GAMER_FFMPEG_PATH={}",
        plan.ffmpeg_path.as_ref().unwrap().display()
    ));
    expect(&format!(
        "GAMER_SCRCPY_SERVER={}",
        plan.scrcpy_server.display()
    ));
    expect(&format!("GB_CONFIG={}", plan.config_path.display()));
    expect(&format!("GB_LOG={}", plan.log_path.display()));

    // PATH 实际值 = System32 最小集（不继承父进程 PATH）
    let path_line = text
        .lines()
        .find(|l| l.len() >= 5 && l[..5].eq_ignore_ascii_case("PATH="))
        .expect("子进程应有 PATH");
    assert_eq!(
        path_line[5..].to_ascii_lowercase(),
        supervisor::minimal_path().to_ascii_lowercase()
    );
    // 父进程环境不应泄漏：构建/CI 常见的 CARGO_* 与本测试目录路径不得出现
    assert!(
        !text.contains("CARGO"),
        "父环境 CARGO_* 不应带入子进程:\n{text}"
    );
    cleanup(&layout.root);
}

#[test]
fn supervisor_holds_child_handle_and_recovers_exit_code() {
    // OPS-003：按句柄 wait（不轮询端口/进程名）；退出码精确回收
    let layout = setup("sup-wait");
    fs::create_dir_all(layout.versions_dir().join("0.2.0")).unwrap();
    let plan = fake_plan(&layout);
    let mut child =
        supervisor::spawn_child(&plan, &["/C", "exit", "7"], Stdio::null(), Stdio::null())
            .expect("spawn 应成功");
    let pid = child.id();
    assert_ne!(pid, 0);
    let status = child.wait().expect("wait 应成功");
    assert_eq!(status.code(), Some(7), "退出码应精确回收");
    cleanup(&layout.root);
}

#[test]
fn readiness_probe_flips_503_to_200_like_real_server() {
    // server/src/api/system.rs api_health_ready：未就绪 503 + ready:false，就绪 200 + ready:true
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let addr = http_server(Arc::new(move |req, stream| {
        let path = String::from_utf8_lossy(req);
        h.fetch_add(1, Ordering::SeqCst);
        if path.contains("/health/ready") {
            let n = h.load(Ordering::SeqCst);
            if n <= 2 {
                write_response(
                    stream,
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: 26\r\nConnection: close\r\n\r\n{\"ready\":false,\"checks\":{}}",
                );
            } else {
                write_response(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 25\r\nConnection: close\r\n\r\n{\"ready\":true,\"checks\":{}}",
                );
            }
        } else {
            write_response(
                stream,
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    }));
    let port = addr.port();
    wait_for_ready(port, &quick_probe()).expect("503→200 翻转后应就绪");
    assert!(hits.load(Ordering::SeqCst) >= 3);
}

#[test]
fn readiness_probe_times_out_when_never_ready() {
    let addr = http_server(Arc::new(|_req, stream| {
        write_response(
            stream,
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 26\r\nConnection: close\r\n\r\n{\"ready\":false,\"checks\":{}}",
        );
    }));
    let probe = ReadyProbe {
        overall_timeout: Duration::from_millis(600),
        per_attempt_timeout: Duration::from_millis(300),
        interval: Duration::from_millis(80),
    };
    let err = wait_for_ready(addr.port(), &probe).expect_err("永不就绪应超时");
    assert!(err.contains("超时"), "应报有界超时: {err}");
}

#[test]
fn readiness_probe_fails_on_connection_refused() {
    // 未监听端口：连接拒绝
    let probe = ReadyProbe {
        overall_timeout: Duration::from_millis(600),
        per_attempt_timeout: Duration::from_millis(300),
        interval: Duration::from_millis(80),
    };
    let err = wait_for_ready(1, &probe).expect_err("无监听应失败");
    assert!(
        err.contains("连接") || err.contains("超时"),
        "应报连接失败: {err}"
    );
}

#[test]
fn configured_port_from_config_toml_with_default_fallback() {
    let layout = setup("sup-port");
    // 无配置文件 → 默认 8443
    assert_eq!(
        supervisor::read_configured_port(&layout.config_file()),
        8443
    );
    // 有配置 → 读取顶层 port
    fs::create_dir_all(layout.root.join("config")).unwrap();
    fs::write(
        layout.config_file(),
        "port = 9443\ndata_dir = \"data\"\nadb_path = \"adb\"\nffmpeg_path = \"ffmpeg\"\nscrcpy_server = \"a.jar\"\n",
    )
    .unwrap();
    assert_eq!(
        supervisor::read_configured_port(&layout.config_file()),
        9443
    );
    cleanup(&layout.root);
}

#[test]
fn entrypoint_resolution_prefers_manifest_then_falls_back() {
    let layout = setup("sup-entry");
    let version = "0.2.0";
    let app_dir = layout.versions_dir().join(version);
    fs::create_dir_all(&app_dir).unwrap();

    // 什么都不放：清晰报错
    let err = supervisor::resolve_entrypoint(&layout, version).unwrap_err();
    assert!(
        err.contains("入口程序不存在") || err.contains("版本目录"),
        "应清晰报错: {err}"
    );

    // 默认 gamer-server.exe 存在 → 直接用
    fs::write(app_dir.join("gamer-server.exe"), b"fake").unwrap();
    assert_eq!(
        supervisor::resolve_entrypoint(&layout, version).unwrap(),
        app_dir.join("gamer-server.exe")
    );

    // 缓存 manifest 指定别的 entrypoint 且文件存在 → 优先 manifest
    fs::create_dir_all(layout.manifests_dir()).unwrap();
    fs::write(
        layout.manifests_dir().join("0.2.0.json"),
        r#"{"platforms":{"windows-x86_64":{"app":{"entrypoint":"bin/server.exe"}}}}"#,
    )
    .unwrap();
    fs::create_dir_all(app_dir.join("bin")).unwrap();
    fs::write(app_dir.join("bin").join("server.exe"), b"fake2").unwrap();
    assert_eq!(
        supervisor::resolve_entrypoint(&layout, version).unwrap(),
        app_dir.join("bin").join("server.exe")
    );
    cleanup(&layout.root);
}

#[test]
fn latest_component_exe_picks_highest_semver_dir() {
    let layout = setup("sup-runtime");
    for v in ["1.0.0", "2.10.0", "2.9.0"] {
        let dir = layout.component_dir("adb", v);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("adb.exe"), b"x").unwrap();
    }
    let picked = supervisor::latest_component_exe(&layout, "adb", "adb.exe").unwrap();
    assert_eq!(
        picked,
        layout.component_dir("adb", "2.10.0").join("adb.exe")
    );
    // 缺失组件 → None
    assert!(supervisor::latest_component_exe(&layout, "ffmpeg", "ffmpeg.exe").is_none());
    cleanup(&layout.root);
}

#[test]
fn start_reports_clear_errors_without_install_or_version_dir() {
    // 空安装根：未安装 → 清晰错误退出码 1
    let layout = setup("start-empty");
    let root_s = layout.root.to_string_lossy().into_owned();
    let cli = Cli::parse_from(["gamer-launcher", "--install-root", &root_s, "start"]);
    assert_eq!(commands::dispatch(&cli, &layout), 1, "未安装应退出码 1");

    // current.json 指向不存在的版本目录 → 清晰错误退出码 1
    StateStore::new(&layout.root)
        .write_current(&CurrentState::new("9.9.9", None))
        .unwrap();
    let cli = Cli::parse_from(["gamer-launcher", "--install-root", &root_s, "start"]);
    assert_eq!(
        commands::dispatch(&cli, &layout),
        1,
        "版本目录缺失应退出码 1"
    );
    cleanup(&layout.root);
}

#[test]
fn start_full_chain_with_fake_server_probe_ready_then_waits_exit() {
    // 完整 start 链路（假 server）：entrypoint 为 .bat（cmd 自动接管），常驻约 1s；
    // config.toml 把端口指向夹具 HTTP 服务（200 ready）→ start 应：spawn → 探测就绪
    // → 持句柄等待 → 回收退出码 0。夹具在 System32 PATH 之外仅依赖 cmd/ping，
    // 同时验证最小 PATH 下子进程可正常拉起。
    let layout = setup("start-fake");
    let version = "0.2.0";
    let app_dir = layout.versions_dir().join(version);
    fs::create_dir_all(&app_dir).unwrap();
    StateStore::new(&layout.root)
        .write_current(&CurrentState::new(version, None))
        .unwrap();

    // 夹具就绪端点（真 server 形态：200 + {"ready":true,...}）
    let addr = http_server(Arc::new(|_req, stream| {
        write_response(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 25\r\nConnection: close\r\n\r\n{\"ready\":true,\"checks\":{}}",
        );
    }));

    // config.toml：端口指向夹具
    fs::create_dir_all(layout.root.join("config")).unwrap();
    fs::write(layout.config_file(), format!("port = {}\n", addr.port())).unwrap();

    // 缓存 manifest 指定 entrypoint = fake-server.bat（假 exe）
    fs::create_dir_all(layout.manifests_dir()).unwrap();
    fs::write(
        layout.manifests_dir().join("0.2.0.json"),
        r#"{"platforms":{"windows-x86_64":{"app":{"entrypoint":"fake-server.bat"}}}}"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("fake-server.bat"),
        "@echo off\r\nping -n 2 127.0.0.1 >nul\r\nexit /b 0\r\n",
    )
    .unwrap();

    let root_s = layout.root.to_string_lossy().into_owned();
    let cli = Cli::parse_from(["gamer-launcher", "--install-root", &root_s, "start"]);
    let code = commands::dispatch(&cli, &layout);
    assert_eq!(code, 0, "假 server 正常退出（0）时 start 应返回 0");
    cleanup(&layout.root);
}

#[test]
fn start_reports_nonzero_when_fake_server_fails() {
    // 假 server 立即失败（exit /b 3）：start 应把退出码透传为非 0
    let layout = setup("start-fail");
    let version = "0.2.0";
    let app_dir = layout.versions_dir().join(version);
    fs::create_dir_all(&app_dir).unwrap();
    StateStore::new(&layout.root)
        .write_current(&CurrentState::new(version, None))
        .unwrap();
    let addr = http_server(Arc::new(|_req, stream| {
        write_response(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: 25\r\nConnection: close\r\n\r\n{\"ready\":true,\"checks\":{}}",
        );
    }));
    fs::create_dir_all(layout.root.join("config")).unwrap();
    fs::write(layout.config_file(), format!("port = {}\n", addr.port())).unwrap();
    fs::create_dir_all(layout.manifests_dir()).unwrap();
    fs::write(
        layout.manifests_dir().join("0.2.0.json"),
        r#"{"platforms":{"windows-x86_64":{"app":{"entrypoint":"failing-server.bat"}}}}"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("failing-server.bat"),
        "@echo off\r\nexit /b 3\r\n",
    )
    .unwrap();

    let root_s = layout.root.to_string_lossy().into_owned();
    let cli = Cli::parse_from(["gamer-launcher", "--install-root", &root_s, "start"]);
    assert_eq!(commands::dispatch(&cli, &layout), 3, "子进程退出码应透传");
    cleanup(&layout.root);
}
