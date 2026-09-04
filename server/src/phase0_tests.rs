//! Phase 0 跨前后端夹具护栏。
//!
//! 这些测试只读取仓库根 `tests/fixtures`，并调用正式的 loader / matcher /
//! store API。默认测试不启动 adb、scrcpy、ffmpeg 或 WebRTC peer；对应的
//! 外部边界见 `tests/README.md`，避免离线单测被误报成设备集成通过。

use std::fs;
use std::path::{Path, PathBuf};

use image::GenericImageView;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::keymaps::{parse_keymap_content, serialize_keymap};
use crate::matcher::{match_template, template_region_from_name, MatchRequest};
use crate::scheduler;
use crate::script_v2::{parse_script_file, serialize_script, InMemoryResources};
use crate::store::{Store, Task};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures")
}

fn read_fixture(relative: &str) -> Vec<u8> {
    let path = fixtures_dir().join(relative);
    fs::read(&path).unwrap_or_else(|error| panic!("读取 fixture {} 失败: {error}", path.display()))
}

fn read_text(relative: &str) -> String {
    String::from_utf8(read_fixture(relative))
        .unwrap_or_else(|error| panic!("fixture {relative} 不是 UTF-8: {error}"))
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn manifest_files() -> Vec<(String, String, String)> {
    let manifest: Value = serde_json::from_slice(&read_fixture("manifest.json"))
        .expect("Phase 0 manifest 必须是合法 JSON");
    assert_eq!(manifest["schema_version"], 1);
    let files = manifest["files"]
        .as_object()
        .expect("manifest.files 必须是对象");
    let mut entries = Vec::new();
    for (category, values) in files {
        for value in values
            .as_array()
            .unwrap_or_else(|| panic!("manifest.files.{category} 必须是数组"))
        {
            let relative = value["path"]
                .as_str()
                .unwrap_or_else(|| panic!("manifest.files.{category} 缺少 path"));
            let expected_sha = value["sha256"]
                .as_str()
                .unwrap_or_else(|| panic!("manifest.files.{category}.{relative} 缺少 sha256"));
            entries.push((
                category.clone(),
                relative.to_string(),
                expected_sha.to_string(),
            ));
        }
    }
    entries
}

#[test]
fn phase0_fixture_manifest_is_complete_and_hash_pinned() {
    let manifest: Value = serde_json::from_slice(&read_fixture("manifest.json")).unwrap();
    let boundaries = manifest["external_boundaries"]
        .as_object()
        .expect("外部依赖边界必须显式记录");
    for name in ["android", "adb", "scrcpy", "ffmpeg", "webrtc_peer"] {
        assert!(boundaries.contains_key(name), "缺少 {name} 外部依赖边界");
    }

    let entries = manifest_files();
    assert_eq!(entries.len(), 8, "Phase 0 最小夹具数量发生漂移");
    for (category, relative, expected_sha) in entries {
        let path = Path::new(&relative);
        assert!(!path.is_absolute(), "{category}/{relative} 不能是绝对路径");
        assert!(
            !relative.split('/').any(|part| part == ".."),
            "{relative} 越界"
        );
        assert!(
            relative.starts_with(&format!("{category}/")),
            "{relative} 必须位于 {category}/ 分区"
        );
        let bytes = read_fixture(&relative);
        assert_eq!(sha256(&bytes), expected_sha, "{relative} SHA-256 漂移");
    }
}

#[test]
fn phase0_script_fixture_uses_strict_loader_and_roundtrips() {
    let source = read_text("scripts/phase0_smoke.yaml");
    let mut resources = InMemoryResources::new();
    resources.add_template("primary#361_365_639_479.png");
    let script = parse_script_file(&source, "phase0_smoke.yaml", &resources)
        .unwrap_or_else(|errors| panic!("脚本 fixture 未通过严格 loader: {errors:?}"));
    assert_eq!(script.steps.len(), 9, "脚本步骤覆盖面发生漂移");

    let serialized = serialize_script(&script);
    let reparsed = parse_script_file(&serialized, "phase0_smoke.yaml", &resources)
        .unwrap_or_else(|errors| panic!("规范化脚本无法再次装载: {errors:?}"));
    assert_eq!(script, reparsed, "脚本 parse/serialize/parse 不保持 AST");
}

#[test]
fn phase0_keymap_fixture_covers_persisted_actions_and_roundtrips() {
    let source = read_text("keymaps/phase0_combat.yaml");
    let keymap = parse_keymap_content(&source, "phase0_combat.yaml")
        .unwrap_or_else(|errors| panic!("keymap fixture 未通过严格 loader: {errors:?}"));
    assert_eq!(keymap.bindings.len(), 4);
    let serialized = serialize_keymap(&keymap).expect("keymap 规范序列化失败");
    let reparsed = parse_keymap_content(&serialized, "phase0_combat.yaml")
        .unwrap_or_else(|errors| panic!("规范化 keymap 无法再次装载: {errors:?}"));
    assert_eq!(keymap, reparsed, "keymap parse/serialize/parse 不保持模型");
}

#[test]
fn phase0_matcher_covers_hit_miss_and_match_many() {
    let success = read_fixture("screenshots/match-success.png");
    let failure = read_fixture("screenshots/match-failure.png");
    let success_image = image::load_from_memory(&success).expect("成功截图不是合法图片");
    assert_eq!(success_image.dimensions(), (1080, 1920));

    let cases = [
        (
            "primary#361_365_639_479.png",
            [390, 700, 300, 220],
            [390, 701, 300, 219],
            (300, 220),
        ),
        (
            "status#130_219_185_240.png",
            [140, 420, 60, 40],
            [140, 420, 59, 40],
            (60, 40),
        ),
        (
            "corner#dr.png",
            [540, 960, 540, 960],
            [540, 960, 540, 960],
            (40, 40),
        ),
    ];
    for (name, region, suffix_region, dimensions) in cases {
        let template = read_fixture(&format!("templates/{name}"));
        let template_image = image::load_from_memory(&template).expect("模板不是合法图片");
        assert_eq!(template_image.dimensions(), dimensions, "{name} 尺寸漂移");
        assert_eq!(
            template_region_from_name(name, 1080, 1920),
            Some(suffix_region),
            "{name} 区域后缀解析漂移"
        );
        let result = match_template(&MatchRequest {
            screen_png: success.clone(),
            template_png: template,
            threshold: Some(0.8),
            region: Some(region),
            color: false,
        })
        .unwrap_or_else(|error| panic!("{name} NCC 失败: {error}"));
        assert!(result.is_some(), "{name} 应在成功截图命中");
    }

    let primary = read_fixture("templates/primary#361_365_639_479.png");
    let miss = match_template(&MatchRequest {
        screen_png: failure,
        template_png: primary,
        threshold: Some(0.8),
        region: Some([390, 700, 300, 220]),
        color: false,
    })
    .expect("未命中匹配不应报解码错误");
    assert!(miss.is_none(), "黑色失败截图不应命中主模板");
}

#[test]
fn phase0_scheduler_and_task_fixture_are_compatible() {
    let task: Value = serde_json::from_slice(&read_fixture("tasks/phase0_daily.json"))
        .expect("任务 fixture 必须是合法 JSON");
    assert_eq!(task["enabled"], true);
    assert_eq!(task["args"], serde_json::json!({}));
    assert!(scheduler::validate_cron(task["cron"].as_str().unwrap()));
    assert!(!scheduler::validate_cron("not a cron"));
    assert_eq!(
        scheduler::normalize_cron("*/15 * * * *"),
        "0 */15 * * * * *"
    );
}

#[test]
fn phase0_task_and_logs_survive_store_reopen() {
    let directory = tempfile::tempdir().expect("创建临时数据目录失败");
    let config = Config {
        data_dir: directory.path().to_path_buf(),
        ..Config::default()
    };
    let task_fixture: Value =
        serde_json::from_slice(&read_fixture("tasks/phase0_daily.json")).unwrap();
    let task = Task {
        id: task_fixture["id"].as_str().unwrap().to_string(),
        name: task_fixture["name"].as_str().unwrap().to_string(),
        cron: task_fixture["cron"].as_str().unwrap().to_string(),
        script_id: task_fixture["script_id"].as_str().unwrap().to_string(),
        device_id: task_fixture["device_id"].as_str().unwrap().to_string(),
        enabled: task_fixture["enabled"].as_bool().unwrap(),
        last_result: None,
        last_run_at: None,
        created_at: "2026-09-03T00:00:00Z".to_string(),
        args_json: "{}".to_string(),
        param_signature: "psig1|".to_string(),
    };
    {
        let store = Store::open(&config).expect("打开 Phase 0 临时数据库失败");
        store.upsert_task(&task).expect("写入任务失败");
        store
            .add_log(
                "fixture-device",
                "phase0_smoke.yaml",
                "info",
                "phase0 smoke",
            )
            .expect("写入日志失败");
    }
    {
        let reopened = Store::open(&config).expect("重启后重新打开数据库失败");
        let tasks = reopened.list_tasks().expect("读取任务失败");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, task.id);
        assert_eq!(tasks[0].args_json, "{}");
        let logs = reopened.list_logs(None, None, 10).expect("读取日志失败");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].msg, "phase0 smoke");
    }
}

/// 外部设备边界：默认不运行。启用后必须真实执行 `adb get-state`，没有设备或
/// 没有 adb 会失败；该测试不把本地结构测试升级为 scrcpy/WebRTC 集成通过。
#[test]
#[ignore = "需要真实 Android 设备；设置 GAMER_PHASE0_ANDROID=1 后显式运行"]
fn phase0_android_smoke_requires_explicit_opt_in() {
    assert_eq!(
        std::env::var("GAMER_PHASE0_ANDROID").as_deref(),
        Ok("1"),
        "请设置 GAMER_PHASE0_ANDROID=1；默认 CI 不接触真实设备"
    );
    let adb = std::env::var("GAMER_PHASE0_ADB").unwrap_or_else(|_| "adb".to_string());
    let output = std::process::Command::new(&adb)
        .args(["get-state"])
        .output()
        .unwrap_or_else(|error| panic!("无法执行 {adb} get-state: {error}"));
    assert!(output.status.success(), "adb get-state 失败：{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "device",
        "adb 没有处于 device 状态的真实 Android 设备"
    );
}

// ===================== 真机基线补测（opt-in） =====================
//
// baseline.json 中 `requires_hardware` 的两个指标在此补测（沿用上面的
// `GAMER_PHASE0_ANDROID=1` 显式 opt-in 门禁；默认 `cargo test` 全部跳过）：
//
// - `scrcpy_connect_p95_ms`：走生产连接入口 `ScrcpySession::connect`
//   （DeviceManager::connect_device 内部同款：is_connected → reverse →
//   push server → accept 三 socket → 视频元信息），以收到首帧视频数据为
//   终点计时，迭代 5 次取 P50/P95；静止屏 3s 无帧时按生产 pusher 同款
//   RESET_VIDEO 兜底要首帧并记录 fallback 次数。
// - `webrtc_stability`：进程内 webrtc-rs 回环——本端一对 peer connection
//   经内存信令互连（ICE host 候选本机直连），接收端统计真实 scrcpy H.264
//   流经 DTLS/SRTP 链路持续 45s 的 RTP 包/帧到达与停顿；静止期间与生产
//   pusher 一致按 500ms 补帧保持链路活性。浏览器端解码渲染未覆盖。
//
// 运行（真机在线时）：
//   GAMER_PHASE0_ANDROID=1 cargo test --release -Z unstable-options -- \
//     --ignored --nocapture phase0_android
//   （stable 工具链：`cargo test --release -- --ignored --nocapture` 后按
//   名字过滤两行 phase0_android_ 前缀测试）
mod android_bench {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use bytes::Bytes;
    use webrtc::api::interceptor_registry::register_default_interceptors;
    use webrtc::api::media_engine::MediaEngine;
    use webrtc::api::APIBuilder;
    use webrtc::interceptor::registry::Registry;
    use webrtc::interceptor::Attributes;
    use webrtc::peer_connection::configuration::RTCConfiguration;
    use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
    use webrtc::rtp::codecs::h264::H264Payloader;
    use webrtc::rtp::header::Header;
    use webrtc::rtp::packet::Packet;
    use webrtc::rtp::packetizer::Payloader;
    use webrtc::rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTPCodecType};
    use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
    use webrtc::rtp_transceiver::RTCRtpTransceiverInit;
    use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
    use webrtc::track::track_remote::TrackRemote;

    use crate::config::Config;
    use crate::device::adb::Adb;
    use crate::device::scrcpy::{ScrcpySession, SessionHandle, VideoFrame};
    use crate::store::{Device, ScreenMode};

    const CONNECT_ITERS: usize = 5;
    const CONNECT_SETTLE: Duration = Duration::from_millis(2500);
    /// 静止屏等首帧的主动等待窗口；超过后按生产同款 RESET_VIDEO 要帧
    const FIRST_FRAME_WINDOW: Duration = Duration::from_secs(3);
    /// RESET_VIDEO 后等待首帧的总窗口（MTK 静态屏出帧慢是已知常态）
    const RESET_FRAME_WINDOW: Duration = Duration::from_secs(15);
    /// WebRTC 回环统计窗口与静止补帧节奏（与生产 pusher idle_repeat 同值）
    const STABILITY_SECS: u64 = 45;
    const IDLE_REPEAT_MS: u64 = 500;

    fn android_gate() {
        assert_eq!(
            std::env::var("GAMER_PHASE0_ANDROID").as_deref(),
            Ok("1"),
            "请设置 GAMER_PHASE0_ANDROID=1；默认 CI 不接触真实设备"
        );
    }

    /// 两个真机基准共享唯一设备：cargo test 默认并行跑测试，这里串行化，
    /// 避免一个测试的 `reverse --remove-all` 拆掉另一个正在握手的隧道
    /// （互踩会产生短暂的孤儿 scrcpy server 与失败迭代）。
    static DEVICE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_device() -> std::sync::MutexGuard<'static, ()> {
        DEVICE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn bench_config() -> Config {
        Config {
            adb_path: std::env::var("GAMER_PHASE0_ADB").unwrap_or_else(|_| "adb".to_string()),
            scrcpy_server: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/scrcpy-server.jar"),
            // 基准不启 ffmpeg 帧缓存（DeviceManager 才会起）；纯连接链路计时
            decode_frames: false,
            ..Config::default()
        }
    }

    async fn require_device(adb: &Adb) -> String {
        let serial = match std::env::var("GAMER_PHASE0_ADB_SERIAL") {
            Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => adb
                .list_devices()
                .await
                .expect("adb devices 失败")
                .into_iter()
                .next()
                .unwrap_or_else(|| panic!("没有处于 device 状态的 Android 设备")),
        };
        assert!(adb.is_connected(&serial).await, "设备 {serial} 不在线");
        serial
    }

    /// 镜像主屏基准设备（scan_and_sync 入库的默认形态）
    fn bench_device(serial: &str) -> Device {
        Device {
            id: "phase0-bench".to_string(),
            name: "phase0-bench".to_string(),
            kind: "usb".to_string(),
            addr: serial.to_string(),
            screen_mode: ScreenMode::Mirror,
            vd_res: None,
            vd_dpi: None,
            pkg: None,
            fps: None,
            created_at: "phase0-bench".to_string(),
        }
    }

    /// 镜像会话生产路径会唤醒物理屏（DeviceManager Mirror 分支同款 224 +
    /// dismiss-keyguard），保证显示管线出帧；基准计时不含唤醒。
    async fn wake_screen(adb: &Adb, serial: &str) {
        let _ = adb
            .shell(serial, "input keyevent 224", Duration::from_secs(8))
            .await;
        let _ = adb
            .shell(serial, "wm dismiss-keyguard", Duration::from_secs(8))
            .await;
    }

    async fn sleep_screen(adb: &Adb, serial: &str) {
        let _ = adb
            .shell(serial, "input keyevent 223", Duration::from_secs(8))
            .await;
    }

    /// 设备侧 scrcpy app_process 残留进程数（ps -A 按进程名行过滤）
    async fn scrcpy_residue(adb: &Adb, serial: &str) -> usize {
        let out = adb
            .shell(serial, "ps -A", Duration::from_secs(10))
            .await
            .unwrap_or_default();
        out.lines().filter(|l| l.contains("app_process")).count()
    }

    /// 残留收敛检查：socket 关闭 → 设备端 server（cleanup=true）退出有数秒
    /// 传播延迟；在基线之上时轮询等待，超时仍高于基线才视为真残留
    async fn wait_residue_settles(adb: &Adb, serial: &str, baseline: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut count = scrcpy_residue(adb, serial).await;
        while count > baseline && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_secs(2)).await;
            count = scrcpy_residue(adb, serial).await;
        }
        count
    }

    /// 连接成功的一次会话：保留所有权以便统一 teardown（两个 rx 通道必须
    /// 全部释放，scrcpy 任一 socket 关闭即设备端 server cleanup 退出）
    struct ConnectOutcome {
        session: std::sync::Arc<ScrcpySession>,
        video_rx: tokio::sync::mpsc::Receiver<VideoFrame>,
        audio_task: tokio::task::JoinHandle<()>,
        first_frame: VideoFrame,
        connect_ms: f64,
        total_ms: f64,
        used_reset: bool,
        width: u32,
        height: u32,
    }

    /// 生产连接入口 → 首帧视频数据。返回 (连接耗时, 含首帧总耗时)。
    async fn connect_to_first_frame(
        adb: &Adb,
        cfg: &Config,
        device: &Device,
    ) -> anyhow::Result<ConnectOutcome> {
        let t0 = Instant::now();
        let SessionHandle {
            session,
            video_rx,
            mut audio_rx,
        } = ScrcpySession::connect(adb, cfg, device).await?;
        let connect_ms = t0.elapsed().as_secs_f64() * 1000.0;
        // audio_rx 必须保活（scrcpy 任一 socket 断开即整机退出）；收包任务在
        // teardown 时 abort 释放
        let audio_task = tokio::spawn(async move {
            while audio_rx.recv().await.is_some() {}
        });
        let mut video_rx = video_rx;
        let (first_frame, used_reset) = match tokio::time::timeout(
            FIRST_FRAME_WINDOW,
            video_rx.recv(),
        )
        .await {
            Ok(Some(frame)) => (frame, false),
            _ => {
                // 静止屏/熄屏编码器可能不主动出帧：与生产 pusher 同款 RESET_VIDEO 要帧
                let _ = session.reset_video().await;
                let frame = tokio::time::timeout(RESET_FRAME_WINDOW, video_rx.recv())
                    .await
                    .ok()
                    .flatten()
                    .ok_or_else(|| {
                        anyhow::anyhow!("连接已建立但 15s 内未收到首帧视频数据（含 reset_video 兜底）")
                    })?;
                (frame, true)
            }
        };
        let total_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let (width, height) = session.video_size();
        Ok(ConnectOutcome {
            session,
            video_rx,
            audio_task,
            first_frame,
            connect_ms,
            total_ms,
            used_reset,
            width,
            height,
        })
    }

    /// 干净断开：关掉两个帧接收端 → 设备端 server（cleanup=true）随 socket
    /// 关闭退出 → 等待 connected 翻转，不 adb 强杀设备侧进程。
    async fn teardown(outcome: ConnectOutcome) {
        let ConnectOutcome {
            session,
            mut video_rx,
            audio_task,
            ..
        } = outcome;
        video_rx.close();
        audio_task.abort();
        let deadline = Instant::now() + Duration::from_secs(6);
        while session.connected.load(Ordering::SeqCst) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        drop(video_rx);
        drop(session);
    }

    /// nearest-rank 百分位（输入必须已升序）
    fn nearest_rank(sorted: &[f64], p: f64) -> f64 {
        let n = sorted.len();
        let idx = (((p / 100.0) * n as f64).ceil() as usize).clamp(1, n);
        sorted[idx - 1]
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "需要真实 Android 设备；设置 GAMER_PHASE0_ANDROID=1 后显式运行"]
    async fn phase0_android_scrcpy_connect_first_frame_latency_p50_p95() {
        android_gate();
        let _device = lock_device();
        let cfg = bench_config();
        let adb = Adb::new(&cfg);
        let serial = require_device(&adb).await;
        println!("PHASE0 scrcpy_connect device={serial} adb={}", cfg.adb_path);
        wake_screen(&adb, &serial).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let residue_before = scrcpy_residue(&adb, &serial).await;
        let device = bench_device(&serial);

        let mut totals: Vec<f64> = Vec::new();
        let mut connects: Vec<f64> = Vec::new();
        let mut resets = 0usize;
        let mut failures: Vec<String> = Vec::new();
        for i in 0..CONNECT_ITERS {
            match connect_to_first_frame(&adb, &cfg, &device).await {
                Ok(outcome) => {
                    println!(
                        "PHASE0 scrcpy_connect iter={}/{} connect_ms={:.1} total_ms={:.1} reset_fallback={} first_bytes={} first_is_config={} res={}x{}",
                        i + 1,
                        CONNECT_ITERS,
                        outcome.connect_ms,
                        outcome.total_ms,
                        outcome.used_reset,
                        outcome.first_frame.data.len(),
                        outcome.first_frame.is_config,
                        outcome.width,
                        outcome.height,
                    );
                    if outcome.used_reset {
                        resets += 1;
                    }
                    totals.push(outcome.total_ms);
                    connects.push(outcome.connect_ms);
                    teardown(outcome).await;
                }
                Err(e) => failures.push(format!("iter{}: {e:#}", i + 1)),
            }
            // 连接/断开抖动 settle
            tokio::time::sleep(CONNECT_SETTLE).await;
        }
        // 设备端 server cleanup 传播
        let residue_after = wait_residue_settles(&adb, &serial, residue_before).await;
        let _ = adb
            .run(
                &["-s", &serial, "reverse", "--remove-all"],
                Duration::from_secs(5),
            )
            .await;
        sleep_screen(&adb, &serial).await;

        let mut sorted = totals.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p50 = nearest_rank(&sorted, 50.0);
        let p95 = nearest_rank(&sorted, 95.0);
        let mut c_sorted = connects.clone();
        c_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let c_p50 = nearest_rank(&c_sorted, 50.0);
        let c_p95 = nearest_rank(&c_sorted, 95.0);
        println!(
            "PHASE0 scrcpy_connect_only samples={} p50_ms={c_p50:.1} p95_ms={c_p95:.1}",
            c_sorted.len()
        );
        for f in &failures {
            eprintln!("PHASE0 scrcpy_connect failure: {f}");
        }
        println!(
            "PERF metric=scrcpy_connect samples={} p50_ms={p50:.1} p95_ms={p95:.1} max_ms={:.1} failures={} reset_fallbacks={resets} residue_before={residue_before} residue_after={residue_after}",
            sorted.len(),
            sorted.last().copied().unwrap_or(0.0),
            failures.len(),
        );
        println!("RESULT scrcpy_connect_p95_ms={p95:.3}");
        assert!(
            !totals.is_empty(),
            "scrcpy 连接基准 {CONNECT_ITERS} 次迭代全部失败：{failures:?}"
        );
        assert!(
            residue_after <= residue_before,
            "设备侧出现 scrcpy app_process 残留进程（before={residue_before} after={residue_after}）"
        );
    }

    // ---------- webrtc_stability：进程内 webrtc-rs 回环 ----------

    fn annexb_nalus(data: &[u8]) -> Vec<&[u8]> {
        let mut starts: Vec<(usize, usize)> = Vec::new(); // (start code 起点, payload 起点)
        let mut i = 0;
        while i + 2 < data.len() {
            if data[i] == 0 && data[i + 1] == 0 {
                if data[i + 2] == 1 {
                    starts.push((i, i + 3));
                    i += 3;
                    continue;
                }
                if data[i + 2] == 0 && i + 3 < data.len() && data[i + 3] == 1 {
                    starts.push((i, i + 4));
                    i += 4;
                    continue;
                }
            }
            i += 1;
        }
        starts
            .iter()
            .enumerate()
            .map(|(k, &(_, payload_start))| {
                let end = starts.get(k + 1).map(|&(s, _)| s).unwrap_or(data.len());
                &data[payload_start..end]
            })
            .collect()
    }

    fn payload_type_from_sdp(sdp: &str) -> u8 {
        for line in sdp.lines() {
            if let Some(rest) = line.trim().strip_prefix("a=rtpmap:") {
                let mut parts = rest.splitn(2, ' ');
                if let (Some(pt), Some(name)) = (parts.next(), parts.next()) {
                    if name.starts_with("H264/") {
                        return pt.parse().unwrap_or(96);
                    }
                }
            }
        }
        96
    }

    async fn build_api() -> webrtc::api::API {
        let mut m = MediaEngine::default();
        m.register_default_codecs().expect("register default codecs");
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)
            .expect("register default interceptors");
        APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build()
    }

    #[derive(Debug, Clone, serde::Serialize)]
    struct StabilityStats {
        seconds: u64,
        frames_received: u64,
        packets_received: u64,
        bytes_received: u64,
        stalls: u64,
        max_gap_ms: u64,
        read_errors: u64,
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "需要真实 Android 设备；设置 GAMER_PHASE0_ANDROID=1 后显式运行"]
    async fn phase0_android_webrtc_loopback_stability_45s() {
        android_gate();
        let _device = lock_device();
        let cfg = bench_config();
        let adb = Adb::new(&cfg);
        let serial = require_device(&adb).await;
        println!("PHASE0 webrtc_stability device={serial} window={STABILITY_SECS}s");
        let residue_before = scrcpy_residue(&adb, &serial).await;
        wake_screen(&adb, &serial).await;
        let device = bench_device(&serial);

        // 1) 真实 scrcpy 会话（生产连接入口）
        let outcome = connect_to_first_frame(&adb, &cfg, &device)
            .await
            .expect("scrcpy 会话建立失败");
        let ConnectOutcome {
            session,
            mut video_rx,
            audio_task,
            ..
        } = outcome;

        // 2) 进程内回环：接收端 offerer(recvonly) ↔ 推送端 answerer(真实 track)，
        //    内存信令 + host 候选本机直连，ICE/DTLS/SRTP 全真实链路
        let recv_pc = std::sync::Arc::new(
            build_api()
                .await
                .new_peer_connection(RTCConfiguration::default())
                .await
                .expect("接收端 peer connection"),
        );
        let send_pc = std::sync::Arc::new(
            build_api()
                .await
                .new_peer_connection(RTCConfiguration::default())
                .await
                .expect("推送端 peer connection"),
        );

        // 推送端 H264 track（与 ViewerSession 同款 42e01f 声明）；track 必须
        // 先于 set_remote_description 添加（transceiver 与 offer m-line 按序匹配）
        let codec = RTCRtpCodecCapability {
            mime_type: "video/H264".into(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line:
                "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                    .into(),
            rtcp_feedback: vec![],
        };
        let track = std::sync::Arc::new(TrackLocalStaticRTP::new(
            codec,
            "video".into(),
            "gamer".into(),
        ));
        send_pc.add_track(track.clone()).await.expect("add track");

        // 推送端连接状态：Connected 才推流（与生产 pusher 同语义）
        let (conn_tx, mut conn_rx) = tokio::sync::mpsc::channel::<()>(1);
        let running = std::sync::Arc::new(AtomicBool::new(true));
        let running2 = running.clone();
        send_pc.on_peer_connection_state_change(Box::new(move |s| {
            match s {
                webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Connected => {
                    let _ = conn_tx.try_send(());
                }
                webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Failed
                | webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Closed => {
                    running2.store(false, Ordering::SeqCst);
                }
                _ => {}
            }
            Box::pin(async {})
        }));

        // 接收端：recvonly transceiver + on_track 捕获远端轨
        let recv_transceiver = recv_pc
            .add_transceiver_from_kind(
                RTPCodecType::Video,
                Some(RTCRtpTransceiverInit {
                    direction: RTCRtpTransceiverDirection::Recvonly,
                    send_encodings: vec![],
                }),
            )
            .await
            .expect("add recvonly transceiver");
        let (remote_tx, remote_rx) = tokio::sync::oneshot::channel::<std::sync::Arc<TrackRemote>>();
        let remote_slot: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<std::sync::Arc<TrackRemote>>>> =
            std::sync::Mutex::new(Some(remote_tx));
        recv_pc.on_track(Box::new(move |remote, _receiver, _transceiver| {
            if let Some(tx) = remote_slot.lock().unwrap().take() {
                let _ = tx.send(remote);
            }
            Box::pin(async {})
        }));

        // 3) 内存信令：receiver offer → sender answer → receiver set_remote
        let offer = {
            let o = recv_pc.create_offer(None).await.expect("create offer");
            let mut gather = recv_pc.gathering_complete_promise().await;
            recv_pc
                .set_local_description(o)
                .await
                .expect("set local offer");
            let _ = tokio::time::timeout(Duration::from_secs(5), gather.recv()).await;
            recv_pc
                .local_description()
                .await
                .expect("local offer description")
        };
        let payload_type = payload_type_from_sdp(&offer.sdp);
        send_pc
            .set_remote_description(offer.clone())
            .await
            .expect("sender set remote offer");
        let answer: RTCSessionDescription = {
            let a = send_pc.create_answer(None).await.expect("create answer");
            let mut gather = send_pc.gathering_complete_promise().await;
            send_pc
                .set_local_description(a)
                .await
                .expect("set local answer");
            let _ = tokio::time::timeout(Duration::from_secs(5), gather.recv()).await;
            send_pc
                .local_description()
                .await
                .expect("local answer description")
        };
        recv_pc
            .set_remote_description(answer)
            .await
            .expect("receiver set remote answer");

        // 4) 等 ICE/DTLS/SRTP 就绪（生产 pusher：connected + 300ms）
        tokio::time::timeout(Duration::from_secs(15), conn_rx.recv())
            .await
            .expect("推送端 15s 未进入 Connected")
            .expect("conn channel closed");
        tokio::time::sleep(Duration::from_millis(300)).await;

        // 5) 推送端：真实 scrcpy 帧 + 静止补帧（与生产 pusher 相同语义）。
        //    接收端 on_track 由首个到达的 SRTP 包触发，必须先起推流。
        //    先等一帧种子（静止屏 3s 无帧 → RESET_VIDEO），随后窗口内保证
        //    ≤500ms 一个 RTP 包，>1s 间隔即记停顿。
        let mut config_nalu: Option<Bytes> = None;
        let mut last_frame: Option<VideoFrame> = None;
        for _ in 0..4 {
            match tokio::time::timeout(FIRST_FRAME_WINDOW, video_rx.recv()).await {
                Ok(Some(f)) => {
                    if f.is_config {
                        config_nalu = Some(f.data);
                    } else {
                        if f.is_keyframe {
                            last_frame = Some(f);
                            break;
                        }
                        last_frame.get_or_insert(f);
                    }
                }
                _ => {
                    let _ = session.reset_video().await;
                }
            }
        }
        if last_frame.is_none() {
            panic!("WebRTC 回环等待种子帧失败（含 reset_video 兜底）");
        }

        let pushing = std::sync::Arc::new(AtomicBool::new(true));
        let pushing_tx = pushing.clone();
        let wall_start = Instant::now();
        let push_task = tokio::spawn(async move {
            let mut payloader = H264Payloader::default();
            let mut seq: u16 = rand::random();
            let ssrc: u32 = 0x5048_4153;
            let deadline = wall_start + Duration::from_secs(STABILITY_SECS);
            let mut sent_frames = 0u64;
            let mut zero_writes = 0u64;
            let mut send_errors = 0u64;
            while running.load(Ordering::SeqCst) && Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_millis(IDLE_REPEAT_MS), video_rx.recv())
                    .await
                {
                    Ok(Some(f)) => {
                        if f.is_config {
                            config_nalu = Some(f.data);
                            continue;
                        }
                        last_frame = Some(f);
                    }
                    Ok(None) => break, // scrcpy 会话结束
                    Err(_) => {}       // 超时 → 静止补帧
                }
                let frame = last_frame.as_ref().expect("seeded");
                let ts = ((wall_start.elapsed().as_micros() as u64 * 90 / 1000) as u32) | 1;
                // 关键帧前独立单 NALU 发参数集（生产 send_config_nalus 同语义）
                if frame.is_keyframe {
                    if let Some(cfg_nalu) = &config_nalu {
                        for nal in annexb_nalus(cfg_nalu) {
                            let t = nal[0] & 0x1F;
                            if t != 7 && t != 8 {
                                continue;
                            }
                            if nal.len() > 1200 {
                                continue;
                            }
                            let pkt = Packet {
                                header: Header {
                                    version: 2,
                                    padding: false,
                                    extension: false,
                                    marker: false,
                                    payload_type,
                                    sequence_number: seq,
                                    timestamp: ts,
                                    ssrc,
                                    ..Default::default()
                                },
                                payload: Bytes::copy_from_slice(nal),
                            };
                            let _ = tokio::time::timeout(
                                Duration::from_secs(3),
                                track.write_rtp_with_extensions_attributes(
                                    &pkt,
                                    &[],
                                    &Attributes::new(),
                                ),
                            )
                            .await;
                            seq = seq.wrapping_add(1);
                        }
                    }
                }
                let payloads = payloader.payload(1200, &frame.data).unwrap_or_default();
                let n = payloads.len();
                for (i, payload) in payloads.into_iter().enumerate() {
                    let pkt = Packet {
                        header: Header {
                            version: 2,
                            padding: false,
                            extension: false,
                            marker: i == n - 1,
                            payload_type,
                            sequence_number: seq,
                            timestamp: ts,
                            ssrc,
                            ..Default::default()
                        },
                        payload,
                    };
                    match tokio::time::timeout(
                        Duration::from_secs(3),
                        track.write_rtp_with_extensions_attributes(
                            &pkt,
                            &[],
                            &Attributes::new(),
                        ),
                    )
                    .await
                    {
                        Ok(Ok(m)) => {
                            if m == 0 {
                                zero_writes += 1;
                            }
                        }
                        _ => send_errors += 1,
                    }
                    seq = seq.wrapping_add(1);
                }
                sent_frames += 1;
            }
            pushing_tx.store(false, Ordering::SeqCst);
            (sent_frames, zero_writes, send_errors)
        });

        // 6) 接收端 track：on_track（首包触发）；超时则经
        //    transceiver.receiver().tracks() 兜底
        let remote = match tokio::time::timeout(Duration::from_secs(10), remote_rx).await {
            Ok(Ok(t)) => t,
            _ => recv_transceiver
                .receiver()
                .await
                .tracks()
                .await
                .into_iter()
                .next()
                .expect("接收端 track 未建立（on_track 超时且 tracks() 为空）"),
        };
        println!("PHASE0 webrtc_stability srtp_ready payload_type={payload_type}");

        // 7) 接收统计任务：以首包到达为窗口起点，统计 45s 内 RTP 包/帧到达与停顿
        let (stats_tx, stats_rx) = tokio::sync::oneshot::channel::<StabilityStats>();
        let pushing_rx = pushing.clone();
        let receiver_task = tokio::spawn(async move {
            let mut packets = 0u64;
            let mut frames = 0u64;
            let mut bytes = 0u64;
            let mut stalls = 0u64;
            let mut max_gap_ms = 0u64;
            let mut read_errors = 0u64;
            let mut buf = vec![0u8; 4096];
            let mut window_start: Option<Instant> = None;
            let mut last: Option<Instant> = None;
            let window = Duration::from_secs(STABILITY_SECS);
            loop {
                if let Some(start) = window_start {
                    if start.elapsed() >= window {
                        break;
                    }
                }
                match tokio::time::timeout(
                    Duration::from_millis(1500),
                    remote.read(&mut buf),
                )
                .await
                {
                    Ok(Ok((pkt, _))) => {
                        let now = Instant::now();
                        let start = *window_start.get_or_insert(now);
                        if start.elapsed() >= window {
                            break;
                        }
                        if let Some(l) = last {
                            let gap = now.duration_since(l);
                            if gap > Duration::from_secs(1) {
                                stalls += 1;
                            }
                            max_gap_ms = max_gap_ms.max(gap.as_millis() as u64);
                        }
                        last = Some(now);
                        packets += 1;
                        bytes += pkt.payload.len() as u64;
                        if pkt.header.marker {
                            frames += 1;
                        }
                    }
                    Ok(Err(e)) => {
                        read_errors += 1;
                        eprintln!("PHASE0 webrtc_stability read error: {e}");
                        break;
                    }
                    Err(_) => {
                        // 读超时：链路完全无包。推送已结束 → 尾部静默不计数；
                        // 推送仍在跑（静止补帧保证 ≤500ms 一包）→ 记停顿
                        if !pushing_rx.load(Ordering::SeqCst) {
                            break;
                        }
                        if let Some(l) = last {
                            let gap = Instant::now().duration_since(l);
                            stalls += 1;
                            max_gap_ms = max_gap_ms.max(gap.as_millis() as u64);
                            last = Some(Instant::now());
                        }
                    }
                }
            }
            let _ = stats_tx.send(StabilityStats {
                seconds: STABILITY_SECS,
                frames_received: frames,
                packets_received: packets,
                bytes_received: bytes,
                stalls,
                max_gap_ms,
                read_errors,
            });
        });

        // 8) 推流窗口结束：收推送端计数，再收集统计并清理
        let (sent_frames, zero_writes, send_errors) =
            tokio::time::timeout(Duration::from_secs(70), push_task)
                .await
                .expect("推送任务超时未结束")
                .expect("推送任务 panic");
        println!(
            "PHASE0 webrtc_stability sender sent_frames={sent_frames} zero_writes={zero_writes} send_errors={send_errors}"
        );

        // 9) 收集统计并清理
        let stats = tokio::time::timeout(Duration::from_secs(10), stats_rx)
            .await
            .expect("接收统计任务未结束")
            .expect("stats channel dropped");
        let _ = receiver_task.await;
        let _ = send_pc.close().await;
        let _ = recv_pc.close().await;
        audio_task.abort();
        let deadline = Instant::now() + Duration::from_secs(6);
        while session.connected.load(Ordering::SeqCst) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        drop(session);
        let residue_after = wait_residue_settles(&adb, &serial, residue_before).await;
        let _ = adb
            .run(
                &["-s", &serial, "reverse", "--remove-all"],
                Duration::from_secs(5),
            )
            .await;
        sleep_screen(&adb, &serial).await;

        let stats_json = serde_json::to_string(&stats).unwrap();
        println!(
            "PERF metric=webrtc_stability {} sent_frames={sent_frames} send_errors={send_errors} residue_before={residue_before} residue_after={residue_after}",
            stats_json.trim_matches(|c| c == '{' || c == '}').replace('"', ""),
        );
        println!("RESULT webrtc_stability={stats_json}");
        assert!(
            stats.packets_received > 0 && stats.frames_received > 0,
            "WebRTC 回环未收到任何 RTP 数据：{stats:?}"
        );
        assert_eq!(
            stats.read_errors, 0,
            "WebRTC 回环链路中途断开（SRTP read error）：{stats:?}"
        );
        assert!(
            residue_after <= residue_before,
            "设备侧出现 scrcpy app_process 残留进程（before={residue_before} after={residue_after}）"
        );
    }
}
