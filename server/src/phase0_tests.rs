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
