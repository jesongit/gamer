//! System information, health, metrics, watchdog, and graceful shutdown endpoints.
//!
//! `api_system_info` is intentionally not wired here: `api/mod.rs` is an
//! integration-owned hotspot. The temporary dead-code allowance keeps this
//! branch clippy-clean until that route is connected.
#![allow(dead_code)]

use std::fmt::Display;
use std::path::Path;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Local;
use tokio::process::Command;
use tracing::{info, warn};

use super::common::run_blocking_api;
use super::{ApiError, AppState};

// 与自动更新计划 §6.4/§6.7 的当前基线保持一致。这里的 schema_version 是
// system/info 响应契约版本；schema.database/files 是当前数据契约的版本，
// 不另建版本接口，也不把更新能力误当成已实现。
const SYSTEM_INFO_SCHEMA_VERSION: u64 = 1;
const DATABASE_SCHEMA_VERSION: u64 = 1;
const FILE_SCHEMA_VERSION: u64 = 1;
const ROLLBACK_FLOOR: u64 = 1;
const TOOL_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const ADB_PROBE_ARGS: &[&str] = &["version"];
const FFMPEG_PROBE_ARGS: &[&str] = &["-version"];

static BOOT_ID: OnceLock<String> = OnceLock::new();

#[derive(Debug)]
struct ToolProbeResult {
    status: &'static str,
    version: Option<String>,
    source: &'static str,
}

/// 受保护的系统信息：版本/构建信息、部署能力、schema、时区和依赖状态。
///
/// 该响应只包含白名单字段。配置中的路径、认证材料和外部命令输出不会进入
/// JSON；外部工具探针也有严格超时，避免 Settings 页面拖住服务端请求。
pub(super) async fn api_system_info(State(st): State<AppState>) -> Response {
    let mode = deployment_mode();
    let data_source = st.cfg.data_dir.clone();
    let scrcpy_source = st.cfg.scrcpy_server.clone();
    let db = st.db.clone();

    let (data_status, scrcpy_status, database_status) = tokio::join!(
        run_blocking_api(move || Ok(path_status(&data_source, true))),
        run_blocking_api(move || Ok(path_status(&scrcpy_source, false))),
        run_blocking_api(move || Ok(database_status(&db))),
    );
    let data_status = data_status.unwrap_or("error");
    let scrcpy_status = scrcpy_status.unwrap_or("error");
    let database_status = database_status.unwrap_or("error");

    let adb_path = st.cfg.adb_path.clone();
    let ffmpeg_path = st.cfg.ffmpeg_path.clone();
    let (adb, ffmpeg) = tokio::join!(
        probe_tool(
            adb_path,
            ADB_PROBE_ARGS,
            dependency_source("adb", &st.cfg.adb_path, mode),
        ),
        probe_tool(
            ffmpeg_path,
            FFMPEG_PROBE_ARGS,
            dependency_source("ffmpeg", &st.cfg.ffmpeg_path, mode),
        ),
    );

    let data_ok = data_status == "ready";
    let scrcpy_ok = scrcpy_status == "ready";
    let database_ok = database_status == "ready";
    let adb_ok = adb.status == "ready";
    let ffmpeg_ok = ffmpeg.status == "ready";
    let ready = data_ok && scrcpy_ok && database_ok && adb_ok && ffmpeg_ok;

    let dependencies = serde_json::json!({
        "adb": {
            "status": adb.status,
            "version": adb.version,
            "source": adb.source,
        },
        "ffmpeg": {
            "status": ffmpeg.status,
            "version": ffmpeg.version,
            "source": ffmpeg.source,
        },
        "scrcpy": {
            "status": scrcpy_status,
            "version": if scrcpy_ok {
                Some(crate::device::scrcpy::SCRCPY_VERSION)
            } else {
                None
            },
            "source": dependency_source(
                "scrcpy",
                &st.cfg.scrcpy_server.to_string_lossy(),
                mode,
            ),
        },
        "data": { "status": data_status },
        "database": { "status": database_status },
    });

    let body = serde_json::json!({
        "schema_version": SYSTEM_INFO_SCHEMA_VERSION,
        "app": build_info(),
        "deployment": {
            "mode": mode.as_str(),
            "update_strategy": update_strategy(mode),
        },
        "readiness": {
            "ready": ready,
            "status": if ready { "ready" } else { "not_ready" },
            "checks": {
                "data_dir": { "ok": data_ok },
                "sqlite": { "ok": database_ok },
                "scrcpy_server": { "ok": scrcpy_ok },
                "adb": { "ok": adb_ok },
                "ffmpeg": { "ok": ffmpeg_ok },
            },
        },
        "dependencies": dependencies,
        "schema": {
            "database": { "version": DATABASE_SCHEMA_VERSION, "status": database_status },
            "files": { "version": FILE_SCHEMA_VERSION, "status": data_status },
            "rollback_floor": ROLLBACK_FLOOR,
        },
        // 当前基线没有 UpdateController；所有操作能力显式为 false，避免 UI
        // 在 direct/Docker/尚未接通 launcher 时伪造可更新入口。
        "capabilities": {
            "check": false,
            "download": false,
            "install": false,
            "rollback": false,
        },
        "timezone": timezone_info(),
        "startup": {
            "stage": "ready",
            "boot_id": boot_id(),
        },
    });
    Json(body).into_response()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeploymentMode {
    Direct,
    Docker,
    Launcher,
}

impl DeploymentMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Docker => "docker",
            Self::Launcher => "launcher",
        }
    }
}

fn deployment_mode() -> DeploymentMode {
    match std::env::var("GAMER_DEPLOYMENT_MODE")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("docker") => DeploymentMode::Docker,
        Some("launcher") => DeploymentMode::Launcher,
        Some("direct") => DeploymentMode::Direct,
        _ if std::env::var_os("GAMER_LAUNCHER_PIPE").is_some() => DeploymentMode::Launcher,
        _ if Path::new("/.dockerenv").is_file() => DeploymentMode::Docker,
        _ => DeploymentMode::Direct,
    }
}

fn update_strategy(mode: DeploymentMode) -> &'static str {
    match mode {
        DeploymentMode::Direct => "unsupported",
        DeploymentMode::Docker => "external",
        DeploymentMode::Launcher => "managed",
    }
}

fn build_info() -> serde_json::Value {
    let git_commit = first_metadata(
        &["GAMER_BUILD_COMMIT", "GAMER_GIT_COMMIT"],
        &[
            option_env!("GIT_COMMIT"),
            option_env!("BUILD_GIT_COMMIT"),
            option_env!("VERGEN_GIT_SHA"),
            option_env!("GITHUB_SHA"),
        ],
        valid_commit,
    )
    .unwrap_or_else(|| "unknown".into());
    let built_at = first_metadata(
        &["GAMER_BUILD_AT"],
        &[
            option_env!("BUILD_TIMESTAMP"),
            option_env!("VERGEN_BUILD_TIMESTAMP"),
        ],
        valid_timestamp,
    )
    .unwrap_or_else(|| "unknown".into());
    let channel = first_metadata(
        &["GAMER_CHANNEL"],
        &[option_env!("BUILD_CHANNEL")],
        |value| matches!(value, "stable" | "beta" | "dev" | "unknown"),
    )
    .unwrap_or_else(|| "dev".into());
    let target = first_metadata(
        &["GAMER_BUILD_TARGET"],
        &[option_env!("BUILD_TARGET"), option_env!("TARGET")],
        valid_token,
    )
    .unwrap_or_else(runtime_target);

    serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "git_commit": git_commit,
        "built_at": built_at,
        "channel": channel,
        "target": target,
    })
}

fn first_metadata(
    runtime_keys: &[&str],
    compiled_values: &[Option<&'static str>],
    valid: impl Fn(&str) -> bool,
) -> Option<String> {
    runtime_keys
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .chain(
            compiled_values
                .iter()
                .flatten()
                .map(|value| value.to_string()),
        )
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty() && valid(value))
}

fn valid_commit(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn valid_timestamp(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ".:+-".contains(ch))
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "._-".contains(ch))
}

fn runtime_target() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

fn boot_id() -> &'static str {
    BOOT_ID
        .get_or_init(|| uuid::Uuid::new_v4().to_string())
        .as_str()
}

fn timezone_info() -> serde_json::Value {
    let (name, source) = match std::env::var("TZ").ok().and_then(sanitize_timezone) {
        Some(value) => (value, "TZ"),
        None => {
            let local_name = Local::now().format("%Z").to_string();
            if local_name.is_empty() || local_name == "?" {
                ("system".into(), "system")
            } else {
                (local_name, "system")
            }
        }
    };
    serde_json::json!({
        "name": name,
        "offset": Local::now().format("%:z").to_string(),
        "source": source,
    })
}

fn sanitize_timezone(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 64
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains("..")
        || value.chars().any(|ch| ch.is_control() || ch == '\\')
        || !value
            .chars()
            .all(|ch| ch.is_alphanumeric() || "/_+-:".contains(ch))
    {
        return None;
    }
    Some(value.to_string())
}

fn dependency_source(component: &str, path: &str, mode: DeploymentMode) -> &'static str {
    if mode == DeploymentMode::Docker {
        // Dockerfile 内的 adb/ffmpeg 与 scrcpy jar 都随镜像发布；不把容器内路径
        // 暴露给客户端，但来源仍可明确标为 bundled。
        return "bundled";
    }
    if mode == DeploymentMode::Launcher {
        // launcher 模式的路径由其 managed runtime 注入；当前 server 尚未接入
        // 依赖修复 IPC，因此只报告约定来源，不宣称修复/更新能力可用。
        return "bundled";
    }
    let path = Path::new(path.trim());
    if component == "scrcpy"
        && path.is_relative()
        && path
            .file_name()
            .is_some_and(|name| name == "scrcpy-server.jar")
        && path
            .parent()
            .is_some_and(|parent| parent.ends_with("assets"))
    {
        "bundled"
    } else if path.components().count() == 1 {
        "system"
    } else {
        "custom"
    }
}

fn path_status(path: &Path, directory: bool) -> &'static str {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() == directory => "ready",
        Ok(_) => "invalid",
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "missing",
        Err(_) => "error",
    }
}

fn database_status(db: &crate::store::Db) -> &'static str {
    if db.health_check().is_ok() {
        "ready"
    } else {
        "error"
    }
}

async fn probe_tool(
    path: String,
    args: &'static [&'static str],
    source: &'static str,
) -> ToolProbeResult {
    let mut command = Command::new(path.trim());
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ToolProbeResult {
                status: "missing",
                version: None,
                source,
            }
        }
        Err(_) => {
            return ToolProbeResult {
                status: "error",
                version: None,
                source,
            }
        }
    };

    match tokio::time::timeout(TOOL_PROBE_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => ToolProbeResult {
            status: "ready",
            version: first_version_token(&String::from_utf8_lossy(&output.stdout)),
            source,
        },
        Ok(Ok(_)) => ToolProbeResult {
            status: "error",
            version: None,
            source,
        },
        Ok(Err(_)) => ToolProbeResult {
            status: "error",
            version: None,
            source,
        },
        Err(_) => ToolProbeResult {
            status: "timeout",
            version: None,
            source,
        },
    }
}

fn first_version_token(output: &str) -> Option<String> {
    let mut after_version = false;
    for token in output.split_whitespace() {
        if after_version && valid_version_token(token) {
            return Some(token.to_string());
        }
        after_version = token.eq_ignore_ascii_case("version");
    }
    None
}

fn valid_version_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.contains('.')
        && value.chars().any(|ch| ch.is_ascii_digit())
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ".-_+".contains(ch))
}

fn append_metric(body: &mut String, help: &str, kind: &str, metric: &str, value: impl Display) {
    body.push_str("# HELP ");
    body.push_str(metric.split('{').next().unwrap_or(metric));
    body.push(' ');
    body.push_str(help);
    body.push('\n');
    body.push_str("# TYPE ");
    body.push_str(metric.split('{').next().unwrap_or(metric));
    body.push(' ');
    body.push_str(kind);
    body.push('\n');
    body.push_str(metric);
    body.push(' ');
    body.push_str(&value.to_string());
    body.push('\n');
}

/// 结构化 readiness 探针：检查服务本地运行所需的持久化目录、SQLite、
/// scrcpy-server 资源和外部工具。探针本身匿名可访问，响应只返回布尔状态，
/// 不把本机路径、命令行或底层错误泄露给客户端。
pub(super) async fn api_health_ready(State(st): State<AppState>) -> Response {
    let data_dir = st.cfg.data_dir.clone();
    let scrcpy_server = st.cfg.scrcpy_server.clone();
    let db = st.db.clone();
    let cfg = st.cfg.clone();
    let (data_dir_ok, scrcpy_ok, db_ok, tools) = tokio::join!(
        run_blocking_api(move || Ok(data_dir.is_dir())),
        run_blocking_api(move || Ok(scrcpy_server.is_file())),
        run_blocking_api(move || Ok(db.health_check().is_ok())),
        run_blocking_api(move || Ok(cfg.probe_external_tools())),
    );
    let data_dir_ok = data_dir_ok.unwrap_or(false);
    let scrcpy_ok = scrcpy_ok.unwrap_or(false);
    let db_ok = db_ok.unwrap_or(false);
    let tool_probes = tools.unwrap_or_default();
    let adb_ok = tool_probes
        .iter()
        .find(|p| p.name == "adb")
        .map(|p| p.status.is_ok())
        .unwrap_or(false);
    let ffmpeg_ok = tool_probes
        .iter()
        .find(|p| p.name == "ffmpeg")
        .map(|p| p.status.is_ok())
        .unwrap_or(false);
    let ready = data_dir_ok && scrcpy_ok && db_ok && adb_ok && ffmpeg_ok;
    let body = serde_json::json!({
        "ready": ready,
        "checks": {
            "data_dir": { "ok": data_dir_ok },
            "sqlite": { "ok": db_ok },
            "scrcpy_server": { "ok": scrcpy_ok },
            "adb": { "ok": adb_ok },
            "ffmpeg": { "ok": ffmpeg_ok },
        }
    });
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body)).into_response()
}

/// 暴露低基数 Prometheus 文本指标。读数据库和外部探测均移到 blocking 池，
/// 避免指标抓取把 rusqlite/命令执行带到 Tokio 核心线程；业务指标采集失败时
/// 仍返回合法响应，并用 `gamer_db_ready` 标记异常。
pub(super) async fn api_metrics(State(st): State<AppState>) -> Response {
    let db = st.db.clone();
    let db_snapshot = run_blocking_api(move || {
        db.metrics_snapshot()
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await;
    let db_ready = db_snapshot.is_ok();
    let db_metrics = db_snapshot.unwrap_or_default();
    let configured_devices = st.devices.list_snapshot().len();
    let active_sessions = st.devices.online_sessions().len();
    let active_viewers = st.viewers.lock().map(|v| v.len()).unwrap_or_default();
    let active_runs = st.runs.active_count();
    let runtime_metrics = st.metrics.snapshot();

    let mut body = String::new();
    append_metric(
        &mut body,
        "Number of configured devices.",
        "gauge",
        "gamer_configured_devices",
        configured_devices,
    );
    append_metric(
        &mut body,
        "Current online scrcpy sessions.",
        "gauge",
        "gamer_sessions_active",
        active_sessions,
    );
    append_metric(
        &mut body,
        "Current registered WebRTC viewers.",
        "gauge",
        "gamer_viewers_active",
        active_viewers,
    );
    append_metric(
        &mut body,
        "Current non-terminal runs.",
        "gauge",
        "gamer_runs_active",
        active_runs,
    );
    append_metric(
        &mut body,
        "Whether the database metrics query succeeded.",
        "gauge",
        "gamer_db_ready",
        u8::from(db_ready),
    );
    append_metric(
        &mut body,
        "Rows in the devices table.",
        "gauge",
        "gamer_db_devices_total",
        db_metrics.devices,
    );
    append_metric(
        &mut body,
        "Rows in the tasks table.",
        "gauge",
        "gamer_db_tasks_total",
        db_metrics.tasks,
    );
    append_metric(
        &mut body,
        "Rows in the logs table.",
        "gauge",
        "gamer_db_logs_total",
        db_metrics.logs,
    );
    append_metric(
        &mut body,
        "Rows in the scheduled_runs table.",
        "gauge",
        "gamer_scheduled_runs_total",
        db_metrics.scheduled_runs,
    );
    append_metric(
        &mut body,
        "Database worker queue depth.",
        "gauge",
        "gamer_db_queue_depth",
        runtime_metrics.db_queue_depth,
    );
    append_metric(
        &mut body,
        "Completed database log batches.",
        "counter",
        "gamer_db_log_batches_total",
        runtime_metrics.db_batches_total,
    );
    append_metric(
        &mut body,
        "Rows committed in database log batches.",
        "counter",
        "gamer_db_log_batch_rows_total",
        runtime_metrics.db_batch_rows_total,
    );
    append_metric(
        &mut body,
        "Total database log batch duration in milliseconds.",
        "counter",
        "gamer_db_log_batch_duration_ms_total",
        runtime_metrics.db_batch_duration_ms_total,
    );
    append_metric(
        &mut body,
        "Database log batch failures.",
        "counter",
        "gamer_db_log_flush_errors_total",
        runtime_metrics.db_flush_errors_total,
    );
    append_metric(
        &mut body,
        "Debug logs dropped because the database queue was full.",
        "counter",
        "gamer_db_debug_logs_dropped_total",
        runtime_metrics.db_logs_dropped_debug_total,
    );
    append_metric(
        &mut body,
        "Successful scrcpy connection attempts.",
        "counter",
        "gamer_scrcpy_connect_success_total",
        runtime_metrics.scrcpy_connect_success_total,
    );
    append_metric(
        &mut body,
        "Failed scrcpy connection attempts.",
        "counter",
        "gamer_scrcpy_connect_failure_total",
        runtime_metrics.scrcpy_connect_failure_total,
    );
    for (reason, value) in [
        ("manual", runtime_metrics.scrcpy_reconnect_manual_total),
        (
            "watchdog_dead",
            runtime_metrics.scrcpy_reconnect_watchdog_dead_total,
        ),
        (
            "watchdog_silent",
            runtime_metrics.scrcpy_reconnect_watchdog_silent_total,
        ),
    ] {
        append_metric(
            &mut body,
            "Scrcpy reconnect attempts by bounded reason.",
            "counter",
            &format!("gamer_scrcpy_reconnect_total{{reason=\"{reason}\"}}"),
            value,
        );
    }
    append_metric(
        &mut body,
        "Video frames received from devices.",
        "counter",
        "gamer_video_input_frames_total",
        runtime_metrics.video_input_frames_total,
    );
    append_metric(
        &mut body,
        "Approximate input video frames per second.",
        "gauge",
        "gamer_video_input_fps",
        runtime_metrics.video_input_fps_milli as f64 / 1000.0,
    );
    append_metric(
        &mut body,
        "Video frames sent through RTP.",
        "counter",
        "gamer_rtp_sent_frames_total",
        runtime_metrics.rtp_sent_frames_total,
    );
    append_metric(
        &mut body,
        "Approximate RTP video frames per second.",
        "gauge",
        "gamer_rtp_sent_fps",
        runtime_metrics.rtp_sent_fps_milli as f64 / 1000.0,
    );
    append_metric(
        &mut body,
        "Current RTP queue depth.",
        "gauge",
        "gamer_rtp_queue_depth",
        runtime_metrics.rtp_queue_depth,
    );
    append_metric(
        &mut body,
        "Video frames dropped before RTP send.",
        "counter",
        "gamer_rtp_dropped_frames_total",
        runtime_metrics.rtp_dropped_frames_total,
    );
    append_metric(
        &mut body,
        "Frames currently retained in the latest GOP.",
        "gauge",
        "gamer_gop_frames",
        runtime_metrics.gop_frames,
    );
    append_metric(
        &mut body,
        "Bytes currently retained in the latest GOP.",
        "gauge",
        "gamer_gop_bytes",
        runtime_metrics.gop_bytes,
    );
    append_metric(
        &mut body,
        "On-demand ffmpeg frame decodes.",
        "counter",
        "gamer_ffmpeg_decode_total",
        runtime_metrics.ffmpeg_decode_total,
    );
    for (result, value) in [
        ("success", runtime_metrics.ffmpeg_decode_success_total),
        ("timeout", runtime_metrics.ffmpeg_decode_timeout_total),
        ("failure", runtime_metrics.ffmpeg_decode_failure_total),
    ] {
        append_metric(
            &mut body,
            "On-demand ffmpeg decodes by bounded result.",
            "counter",
            &format!("gamer_ffmpeg_decode_result_total{{result=\"{result}\"}}"),
            value,
        );
    }
    append_metric(
        &mut body,
        "Total on-demand ffmpeg decode duration in milliseconds.",
        "counter",
        "gamer_ffmpeg_decode_duration_ms_total",
        runtime_metrics.ffmpeg_decode_duration_ms_total,
    );
    for (stage, count, duration) in [
        (
            "spawn",
            runtime_metrics.ffmpeg_stage_spawn_total,
            runtime_metrics.ffmpeg_stage_spawn_ms_total,
        ),
        (
            "write",
            runtime_metrics.ffmpeg_stage_write_total,
            runtime_metrics.ffmpeg_stage_write_ms_total,
        ),
        (
            "decode",
            runtime_metrics.ffmpeg_stage_decode_total,
            runtime_metrics.ffmpeg_stage_decode_ms_total,
        ),
        (
            "png",
            runtime_metrics.ffmpeg_stage_png_total,
            runtime_metrics.ffmpeg_stage_png_ms_total,
        ),
    ] {
        append_metric(
            &mut body,
            "On-demand ffmpeg decode pipeline stage executions.",
            "counter",
            &format!("gamer_ffmpeg_stage_total{{stage=\"{stage}\"}}"),
            count,
        );
        append_metric(
            &mut body,
            "Total on-demand ffmpeg decode pipeline stage duration in milliseconds.",
            "counter",
            &format!("gamer_ffmpeg_stage_ms_total{{stage=\"{stage}\"}}"),
            duration,
        );
    }
    append_metric(
        &mut body,
        "NCC template match operations.",
        "counter",
        "gamer_ncc_matches_total",
        runtime_metrics.ncc_matches_total,
    );
    for (result, value) in [
        ("hit", runtime_metrics.ncc_hits_total),
        ("miss", runtime_metrics.ncc_misses_total),
    ] {
        append_metric(
            &mut body,
            "NCC matches by bounded result.",
            "counter",
            &format!("gamer_ncc_result_total{{result=\"{result}\"}}"),
            value,
        );
    }
    for (scope, value) in [
        ("region", runtime_metrics.ncc_region_total),
        ("fullscreen", runtime_metrics.ncc_fullscreen_total),
    ] {
        append_metric(
            &mut body,
            "NCC matches by bounded search scope.",
            "counter",
            &format!("gamer_ncc_scope_total{{scope=\"{scope}\"}}"),
            value,
        );
    }
    let ncc_hit_ratio = if runtime_metrics.ncc_matches_total == 0 {
        0.0
    } else {
        runtime_metrics.ncc_hits_total as f64 / runtime_metrics.ncc_matches_total as f64
    };
    append_metric(
        &mut body,
        "NCC hit ratio.",
        "gauge",
        "gamer_ncc_hit_ratio",
        ncc_hit_ratio,
    );
    append_metric(
        &mut body,
        "Total NCC match duration in milliseconds.",
        "counter",
        "gamer_ncc_duration_ms_total",
        runtime_metrics.ncc_duration_ms_total,
    );
    append_metric(
        &mut body,
        "Scheduler trigger submissions.",
        "counter",
        "gamer_scheduler_triggers_total",
        runtime_metrics.scheduler_triggers_total,
    );
    append_metric(
        &mut body,
        "Total scheduler trigger submission latency in milliseconds.",
        "counter",
        "gamer_scheduler_trigger_latency_ms_total",
        runtime_metrics.scheduler_trigger_latency_ms_total,
    );
    for (event, value) in [
        ("conflict", runtime_metrics.scheduler_conflicts_total),
        ("skipped", runtime_metrics.scheduler_skipped_total),
        ("failed", runtime_metrics.scheduler_failures_total),
    ] {
        append_metric(
            &mut body,
            "Scheduler outcomes by bounded result.",
            "counter",
            &format!("gamer_scheduler_events_total{{event=\"{event}\"}}"),
            value,
        );
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )
        .body(Body::from(body))
        .expect("metrics response builder with static headers")
        .into_response()
}

/// 视频静默看门狗：设备在线但视频流超过阈值无新帧时的处置。
///
/// 判死以 `session.connected`（video socket 读取循环退出即 false）为准，
/// **视频静默 ≠ 链路死亡**：虚拟屏无应用/静态画面时编码器 0 帧是正常态。
/// - 会话确死（connected=false）：拆会话；有脚本或 viewer 在跑则立即重连
///   （脚本引擎逐步重新取 session，可接续；无消费者则等下次触发连接）
/// - 脚本运行中 + 会话活着 + 静默：不处置（静态屏正常态；判死看控制
///   socket——引擎 tap 失败会报错终止脚本，不需要看门狗拆会话帮倒忙）
/// - 无 viewer 无脚本 + 会话活着 + 静默：交给 idle_power_loop 空闲低功耗
/// - viewer 在看且未被补帧投喂 + 会话活着 + 静默：reset_video 探测，
///   15s 仍静默才拆开重连（pusher 卡死等边缘兜底，踢 viewer）
const VIDEO_IDLE_RECONNECT_MS: u64 = 20_000;
/// reset_video 探测后等待新帧的宽限期，超过则认定编码器/链路已死，升级为重连
const VIDEO_NUDGE_GRACE: Duration = Duration::from_secs(15);

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(super) fn spawn_watchdog(st: AppState) {
    tokio::spawn(async move {
        // 已发过 reset_video 探测的设备 → 探测时刻
        let mut nudged: std::collections::HashMap<String, std::time::Instant> =
            std::collections::HashMap::new();
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            for (id, session) in st.devices.online_sessions() {
                let idle = session.video_idle_ms();
                if idle < VIDEO_IDLE_RECONNECT_MS {
                    nudged.remove(&id);
                    continue;
                }
                let running = st.devices.has_running_scripts(&id);
                // 会话确死（video socket 已关）：唯一允许脚本运行中强拆重连的
                // 路径——控制 socket 同链路已死，不重连脚本会永远卡死
                if !session.connected.load(std::sync::atomic::Ordering::SeqCst) {
                    st.metrics
                        .scrcpy_reconnect(crate::metrics::ReconnectReason::WatchdogDead);
                    warn!(device = %id, idle_ms = idle, "session dead (video socket closed), tearing down");
                    nudged.remove(&id);
                    // 踢 viewer：旧 pusher 挂在旧帧通道上，重连建新通道后不会
                    // 自动迁移；踢掉让前端 onclose 立即重连到新会话
                    let kicked = {
                        let mut map = st.viewers.lock().unwrap();
                        map.remove(&id)
                    };
                    if let Some(h) = kicked {
                        h.running.store(false, std::sync::atomic::Ordering::SeqCst);
                        if let Some(p) = h.peer.upgrade() {
                            let _ = p.close().await;
                        }
                    }
                    st.devices.disconnect_device(&id, true).await;
                    if running {
                        if let Err(e) = st.devices.connect_device(&id).await {
                            warn!(device = %id, err = %e, "auto-reconnect failed");
                        }
                    }
                    continue;
                }
                // 脚本运行中 + 静默 = 静态屏正常态（虚拟屏编码器 0 帧），
                // 不处置；会话真死由上面的 connected 分支兜底
                if running {
                    nudged.remove(&id);
                    continue;
                }
                // 无消费者：空闲低功耗统一交给 idle_power_loop
                let has_viewer = st.viewers.lock().unwrap().contains_key(&id);
                if !has_viewer {
                    nudged.remove(&id);
                    continue;
                }
                // viewer 正在被投喂（设备 0 帧是静态屏常态，pusher 静止补帧还活着）
                // → 会话对 viewer 是健康的：不 nudge（reset 反而打断补帧，MTK 静态
                // 屏 reset 后长时间不出 IDR → 浏览器断供被前端杀连接），也不走
                // 35s 兜底重连（静态屏挂机会话会被无限循环重连踢 viewer）。
                // 真断流时 pusher 退出、last_serve 过期，仍走 nudge → 15s → 重连兜底
                let served_ago_ms = {
                    let map = st.viewers.lock().unwrap();
                    match map.get(&id) {
                        Some(h) => {
                            let t = h.last_serve.load(std::sync::atomic::Ordering::Relaxed);
                            if t == 0 {
                                i64::MAX
                            } else {
                                now_unix_ms() - t
                            }
                        }
                        None => i64::MAX,
                    }
                };
                if served_ago_ms.max(0) < 10_000 {
                    nudged.remove(&id);
                    continue;
                }
                match nudged.get(&id).copied() {
                    None => {
                        // 第一轮：reset_video 请求 config+IDR——编码器活着会立即出帧，
                        // idle 归零回到健康分支，避免黑屏空转被误判为断流
                        if let Some(s) = st.devices.session(&id) {
                            let _ = s.reset_video().await;
                        }
                        nudged.insert(id.clone(), std::time::Instant::now());
                        continue;
                    }
                    Some(t) if t.elapsed() < VIDEO_NUDGE_GRACE => continue,
                    Some(_) => {
                        nudged.remove(&id);
                    }
                }
                warn!(device = %id, idle_ms = idle, "video stream silent after keyframe nudge, auto-reconnecting scrcpy session");
                st.metrics
                    .scrcpy_reconnect(crate::metrics::ReconnectReason::WatchdogSilent);
                // 踢旧 viewer：pusher 停止 + peer 关闭 → ws.rs 退出清理
                let kicked = {
                    let mut map = st.viewers.lock().unwrap();
                    map.remove(&id)
                };
                if let Some(h) = kicked {
                    h.running.store(false, std::sync::atomic::Ordering::SeqCst);
                    if let Some(p) = h.peer.upgrade() {
                        let _ = p.close().await;
                    }
                }
                st.devices.disconnect_device(&id, false).await;
                if let Err(e) = st.devices.connect_device(&id).await {
                    warn!(device = %id, err = %e, "auto-reconnect failed");
                }
            }
        }
    });
}

/// 优雅停机（gamer.ps1 stop/rebuild 先调此端点，超时才兜底硬杀）。
/// OPS-001：drain 序列收口进 [`crate::shutdown::ShutdownCoordinator`]——
/// ① RunManager drain（拒绝新 run → 等待活动任务，宽限 10s）；② 踢全部 viewer；
/// ③ 拆所有 scrcpy 会话/清 reverse 隧道（防孤儿 adb 楔死后续连接）——与
/// Ctrl+C / SIGTERM 信号触发共用同一路径；并发/重复请求由协调器一次性语义
/// 吸收（只执行一次，其余等待完成后返回）。
pub(super) async fn api_shutdown(State(st): State<AppState>) -> Response {
    info!("graceful shutdown requested (POST /api/shutdown)");
    st.shutdown.request().await;
    Json(serde_json::json!({"ok": true})).into_response()
}

/// 手动维护动作（DATA-004）：SQLite VACUUM，返回 vacuum 前后的数据库文件
/// 字节数。VACUUM 耗时且需独占锁——在 store 的 DB worker 线程串行执行，
/// handler 经 blocking 池等待结果，不占用 Tokio 核心线程。
pub(super) async fn api_maintenance_vacuum(State(st): State<AppState>) -> Response {
    let db = st.db.clone();
    info!("manual maintenance: sqlite vacuum requested");
    match run_blocking_api(move || db.vacuum().map_err(|e| ApiError::internal(e.to_string()))).await
    {
        Ok(report) => {
            info!(
                before_bytes = report.before_bytes,
                after_bytes = report.after_bytes,
                "manual maintenance: sqlite vacuum done"
            );
            Json(report).into_response()
        }
        Err(err) => err.into_response(),
    }
}

#[cfg(test)]
mod system_info_tests {
    use super::*;

    #[test]
    fn build_info_has_contract_fields_without_secrets_or_paths() {
        let body = build_info();
        for field in ["version", "git_commit", "built_at", "channel", "target"] {
            assert!(body.get(field).is_some(), "missing app field {field}");
        }
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
        let serialized = body.to_string();
        for forbidden in ["password", "token", "C:\\Users", "/home/"] {
            assert!(!serialized.contains(forbidden), "leaked {forbidden}");
        }
    }

    #[test]
    fn deployment_strategy_is_conservative_for_each_mode() {
        assert_eq!(update_strategy(DeploymentMode::Direct), "unsupported");
        assert_eq!(update_strategy(DeploymentMode::Docker), "external");
        assert_eq!(update_strategy(DeploymentMode::Launcher), "managed");
    }

    #[test]
    fn dependency_source_does_not_echo_configured_paths() {
        assert_eq!(
            dependency_source("adb", "adb", DeploymentMode::Direct),
            "system"
        );
        assert_eq!(
            dependency_source(
                "scrcpy",
                "./assets/scrcpy-server.jar",
                DeploymentMode::Direct
            ),
            "bundled"
        );
        assert_eq!(
            dependency_source(
                "ffmpeg",
                "D:/private/tools/ffmpeg.exe",
                DeploymentMode::Direct
            ),
            "custom"
        );
        assert_eq!(
            dependency_source("adb", "/usr/bin/adb", DeploymentMode::Docker),
            "bundled"
        );
    }

    #[test]
    fn timezone_sanitizer_rejects_path_like_values() {
        assert_eq!(
            sanitize_timezone("Asia/Shanghai".into()).as_deref(),
            Some("Asia/Shanghai")
        );
        assert!(sanitize_timezone("/etc/localtime".into()).is_none());
        assert!(sanitize_timezone("..\\secret".into()).is_none());
    }

    #[test]
    fn tool_version_parser_only_returns_version_token() {
        assert_eq!(
            first_version_token("Android Debug Bridge version 1.0.41\nVersion 35.0.2"),
            Some("1.0.41".into())
        );
        assert_eq!(
            first_version_token("ffmpeg version 7.1.1 Copyright (c)"),
            Some("7.1.1".into())
        );
        assert_eq!(first_version_token("not a version response"), None);
        assert!(!valid_version_token("C:/private/tool.exe"));
    }
}
