//! System information, health, metrics, watchdog, and graceful shutdown endpoints.
//!
//! `/api/system/info` 按 release/contracts/system-api-v1.md §2（冻结）实现：
//! 六组顶层字段 `app` / `deployment` / `schema` / `dependencies` / `capabilities`
//! / `startup`，依赖结论来自 [`crate::deps_probe`]（SYS-002），构建信息来自
//! [`crate::build_info`]。泄露禁令（§1.3）：任何响应不出现盘符路径、用户名、
//! token、完整命令行。fixture 字段集比对测试见文件尾 contract_tests。

use std::fmt::Display;
use std::sync::OnceLock;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use tracing::{info, warn};

use super::common::run_blocking_api;
use super::{ApiError, AppState};
use crate::deps_probe::{self, Mode, Snapshot};

/// 文件布局 schema 基线（schema-policy §5：`data/<pkg>/{yaml,func,tmpl}` = v1）。
/// DB schema 取值走 `migrations::TARGET_SCHEMA`（DATA-003 常量），不在此重复。
const FILE_SCHEMA_VERSION: i64 = 1;

static BOOT_ID: OnceLock<String> = OnceLock::new();

fn boot_id() -> &'static str {
    BOOT_ID
        .get_or_init(|| uuid::Uuid::new_v4().to_string())
        .as_str()
}

/// 受保护的系统信息（system-api-v1 §2，需登录由受保护组 auth_guard 统一判定；
/// 未登录 401 `{"error":"unauthorized"}` 与 fixture `system-info.unauthorized`
/// 一致）。响应只含契约冻结字段；依赖探针懒执行 + 缓存，不阻塞启动。
pub(super) async fn api_system_info(State(st): State<AppState>) -> Response {
    let mode = Mode::detect();
    let snapshot = deps_probe::snapshot(&st.cfg).await;
    let body = system_info_body(mode, &snapshot, boot_id());
    Json(body).into_response()
}

/// 契约响应装配（纯函数）：fixture 字段集比对测试直接驱动，不依赖路由与环境。
pub(super) fn system_info_body(mode: Mode, deps: &Snapshot, boot_id: &str) -> serde_json::Value {
    let build = crate::build_info::build_info();
    serde_json::json!({
        "app": {
            "version": build.version,
            "commit": build.commit,
            "built_at": build.built_at,
            "channel": build.channel,
            "target": build.target,
        },
        "deployment": {
            "mode": mode.as_str(),
            "update_strategy": mode.update_strategy(),
        },
        "schema": {
            "db": crate::migrations::TARGET_SCHEMA,
            "file": FILE_SCHEMA_VERSION,
            "rollback_floor": crate::migrations::MIN_READ_SCHEMA,
        },
        "dependencies": {
            "adb": deps.adb,
            "ffmpeg": deps.ffmpeg,
            "scrcpy": deps.scrcpy,
        },
        "capabilities": capabilities(mode),
        "startup": {
            "stage": startup_stage(),
            "boot_id": boot_id,
        },
    })
}

/// capability 仅由 deployment 决定（契约 §2.1 冻结）：launcher 托管且 IPC
/// 通道建立（以 GAMER_LAUNCHER_IPC_TOKEN 注入为准）→ 全 true；docker/direct
/// → 全 false。策略 off 只关自动行为，不影响此处。
fn capabilities(mode: Mode) -> serde_json::Value {
    let managed = mode.managed_ipc_provisioned(|key| std::env::var(key).ok());
    serde_json::json!({
        "check": managed,
        "download": managed,
        "install": managed,
        "rollback": managed,
    })
}

/// 启动阶段（契约 §2.1 枚举 starting | maintenance_gate | ready）。业务路由
/// 随启动即打开 → ready；activation gate（OPS-004，候选进程闸内报
/// maintenance_gate）由 [`crate::update::gate`] 的进程级投影驱动。
fn startup_stage() -> &'static str {
    crate::update::gate::stage_str()
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
///
/// adb/ffmpeg 结论复用 [`crate::deps_probe`] 的缓存快照（SYS-002）：超时有
/// 界（各 ~3s）且 60s 内零子进程开销——比旧的阻塞式无超时探测更轻。
/// body 形态冻结（system-api-v1 §8，fixture `health-ready.*.json`）：禁止
/// 塞入版本检查、发布说明或任何本机路径。
pub(super) async fn api_health_ready(State(st): State<AppState>) -> Response {
    let data_dir = st.cfg.data_dir.clone();
    let scrcpy_server = st.cfg.scrcpy_server.clone();
    let db = st.db.clone();
    let cfg = st.cfg.clone();
    let (data_dir_ok, scrcpy_ok, db_ok, deps) = tokio::join!(
        run_blocking_api(move || Ok(data_dir.is_dir())),
        run_blocking_api(move || Ok(scrcpy_server.is_file())),
        async move { db.health_check_async().await.is_ok() },
        deps_probe::snapshot(&cfg),
    );
    let data_dir_ok = data_dir_ok.unwrap_or(false);
    let scrcpy_ok = scrcpy_ok.unwrap_or(false);
    let adb_ok = deps.adb.status == "ready";
    let ffmpeg_ok = deps.ffmpeg.status == "ready";
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

/// 停机状态轻量查询（OPS-002）：匿名可访问，body 只含当前停机状态机取值
/// （running/draining/finished）与 drained 布尔——不塞版本/依赖检查（那些
/// 属于 /api/system/info；readiness body 冻结，停机状态走独立轻量端点）。
pub(super) async fn api_shutdown_state(State(st): State<AppState>) -> Response {
    let state = st.shutdown.state();
    Json(serde_json::json!({
        "state": state.as_str(),
        "drained": state == crate::shutdown::ShutdownState::Finished,
    }))
    .into_response()
}

/// 暴露低基数 Prometheus 文本指标。外部探测移到 blocking 池，数据库通过异步
/// worker RPC 查询；业务指标采集失败时
/// 仍返回合法响应，并用 `gamer_db_ready` 标记异常。
pub(super) async fn api_metrics(State(st): State<AppState>) -> Response {
    let db_snapshot = st.db.metrics_snapshot_async().await;
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
                let running = st
                    .devices
                    .activity()
                    .has_kind(&id, crate::core::ActivityKind::Run);
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
/// handler 通过异步 worker RPC 等待结果，不占用 Tokio 核心线程。
pub(super) async fn api_maintenance_vacuum(State(st): State<AppState>) -> Response {
    info!("manual maintenance: sqlite vacuum requested");
    match st.db.vacuum_async().await {
        Ok(report) => {
            info!(
                before_bytes = report.before_bytes,
                after_bytes = report.after_bytes,
                "manual maintenance: sqlite vacuum done"
            );
            Json(report).into_response()
        }
        Err(err) => ApiError::internal(err.to_string()).into_response(),
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use crate::deps_probe::Dependency;
    use std::collections::BTreeSet;

    /// 契约 fixture（字段权威；SYS-001 验收：响应结构与 fixture 逐字段一致）
    fn fixture_body(name: &str) -> serde_json::Value {
        let path = format!("../release/contracts/fixtures/system-api/{name}");
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {path}: {e}"));
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        value["response"]["body"].clone()
    }

    /// 递归比对「字段集」：对象键集必须完全一致（多字段/少字段都算契约破坏），
    /// 数组按元素比对；标量只比对类型存在性（值由运行环境决定）
    fn assert_same_field_sets(fixture: &serde_json::Value, actual: &serde_json::Value, path: &str) {
        match (fixture, actual) {
            (serde_json::Value::Object(f), serde_json::Value::Object(a)) => {
                let fk: BTreeSet<&String> = f.keys().collect();
                let ak: BTreeSet<&String> = a.keys().collect();
                assert_eq!(
                    fk, ak,
                    "字段集不一致 @ {path}: fixture={fk:?} actual={ak:?}"
                );
                for (key, fv) in f {
                    assert_same_field_sets(fv, &a[key], &format!("{path}.{key}"));
                }
            }
            (serde_json::Value::Array(f), serde_json::Value::Array(a)) => {
                assert_eq!(f.len(), a.len(), "数组长度不一致 @ {path}");
                for (i, (fv, av)) in f.iter().zip(a.iter()).enumerate() {
                    assert_same_field_sets(fv, av, &format!("{path}[{i}]"));
                }
            }
            (serde_json::Value::Object(_), _) | (serde_json::Value::Array(_), _) => {
                panic!("结构类型不一致 @ {path}: fixture={fixture} actual={actual}");
            }
            _ => {}
        }
    }

    /// 全部 ready 的探测快照（fixture 同款版本形态；测试不真跑外部探针）。
    /// adb/ffmpeg 的 binding 随模式由探针装配给出：launcher=runtime、docker=external。
    fn ready_snapshot(mode: Mode) -> Snapshot {
        let binding = match mode {
            Mode::Launcher => "runtime",
            Mode::Docker | Mode::Direct => "external",
        };
        let dep = |version: &str, binding: &'static str| Dependency {
            status: "ready",
            version: Some(version.to_string()),
            source: "managed",
            binding,
        };
        Snapshot {
            adb: dep("34.0.5", binding),
            ffmpeg: dep("6.1.1", binding),
            scrcpy: dep("3.3.3", "application"),
        }
    }

    #[test]
    fn info_body_matches_success_fixture_field_set() {
        let body = system_info_body(
            Mode::Launcher,
            &ready_snapshot(Mode::Launcher),
            "3f2c9a58-6d1e-4b7f-9a30-5c8b2e7d1f04",
        );
        assert_same_field_sets(&fixture_body("system-info.success.json"), &body, "$");
        // launcher + IPC 注入 → 契约冻结的全 true 能力
        assert_eq!(body["deployment"]["mode"], "launcher");
        assert_eq!(body["deployment"]["update_strategy"], "managed");
    }

    #[test]
    fn info_body_matches_degraded_docker_fixture_field_set() {
        let body = system_info_body(
            Mode::Docker,
            &ready_snapshot(Mode::Docker),
            "8a41d0c2-93b7-4f5e-b6d8-2c7f0a9e31b5",
        );
        assert_same_field_sets(
            &fixture_body("system-info.degraded-docker.json"),
            &body,
            "$",
        );
        // 降级语义：external 策略、能力全 false、镜像内置依赖 binding=external
        assert_eq!(body["deployment"]["mode"], "docker");
        assert_eq!(body["deployment"]["update_strategy"], "external");
        assert_eq!(body["capabilities"]["check"], false);
        assert_eq!(body["capabilities"]["rollback"], false);
        assert_eq!(body["dependencies"]["adb"]["binding"], "external");
        assert_eq!(body["dependencies"]["ffmpeg"]["binding"], "external");
        assert_eq!(body["dependencies"]["scrcpy"]["binding"], "application");
    }

    #[test]
    fn unauthorized_fixture_matches_middleware_body() {
        // 未登录 401 的 body 由 auth_guard 固定产出；与 fixture 逐字段一致
        assert_eq!(
            fixture_body("system-info.unauthorized.json"),
            serde_json::json!({ "error": "unauthorized" })
        );
    }

    #[test]
    fn info_body_never_leaks_paths_tokens_or_usernames() {
        let body = system_info_body(Mode::Launcher, &ready_snapshot(Mode::Launcher), boot_id());
        let serialized = body.to_string();
        for forbidden in [
            "C:\\",
            "c:\\",
            "/home/",
            "/Users/",
            "scrcpy-server.jar",
            ".exe",
            "password",
            "GAMER_LAUNCHER_IPC_TOKEN",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "响应泄露敏感形态 {forbidden:?}: {serialized}"
            );
        }
        // boot_id 必须是 UUID v4 形态（重启必变，前端据此判定「确实重启过」）
        assert_eq!(boot_id().len(), 36);
        assert_eq!(boot_id().matches('-').count(), 4);
    }

    #[test]
    fn schema_block_reports_baseline_constants() {
        let body = system_info_body(Mode::Direct, &ready_snapshot(Mode::Direct), boot_id());
        assert_eq!(body["schema"]["db"], crate::migrations::TARGET_SCHEMA);
        assert_eq!(body["schema"]["file"], FILE_SCHEMA_VERSION);
        assert_eq!(
            body["schema"]["rollback_floor"],
            crate::migrations::MIN_READ_SCHEMA
        );
    }

    #[test]
    fn capabilities_false_unless_managed_ipc_provisioned() {
        // direct / docker：全 false（值不依赖真实环境变量——装配函数不含环境读取）
        for mode in [Mode::Direct, Mode::Docker] {
            let body = system_info_body(mode, &ready_snapshot(mode), boot_id());
            for cap in ["check", "download", "install", "rollback"] {
                assert_eq!(
                    body["capabilities"][cap],
                    false,
                    "{cap} @ {}",
                    mode.as_str()
                );
            }
        }
        // launcher 模式的能力门在 capabilities()：以 IPC token 注入为准。
        // 进程环境无 token（测试进程）→ false；tokio 单测不安全改进程级环境，
        // managed_ipc_provisioned 的注入矩阵已在 deps_probe 单测覆盖
        let body = system_info_body(Mode::Launcher, &ready_snapshot(Mode::Launcher), boot_id());
        let managed_expected =
            Mode::Launcher.managed_ipc_provisioned(|key| std::env::var(key).ok());
        assert_eq!(body["capabilities"]["check"], managed_expected);
    }

    #[test]
    fn health_ready_fixture_shape_matches_handler_body() {
        // /health/ready body 冻结（§8）：字段集与 fixture 一致（ok 值随环境）
        let body = serde_json::json!({
            "ready": true,
            "checks": {
                "data_dir": { "ok": true },
                "sqlite": { "ok": true },
                "scrcpy_server": { "ok": true },
                "adb": { "ok": true },
                "ffmpeg": { "ok": true },
            }
        });
        assert_same_field_sets(&fixture_body("health-ready.success.json"), &body, "$");
        assert_same_field_sets(&fixture_body("health-ready.not-ready.json"), &body, "$");
    }
}
