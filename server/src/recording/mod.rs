//! Core 录制服务与统一输入观察（视频工作台 V1）。
//!
//! 计划：`docs/plans/gamer_video_workbench_development_plan.md` §5/§7.2；
//! 实施合同：`docs/plans/gamer_video_workbench_contracts.md`。
//!
//! 职责边界：
//! - 录制器接在 scrcpy 原始编码帧分发之后、WebRTC 节流/重放之前，按需订阅
//!   （经 `DeviceManager` 的帧广播 `Sender::subscribe()`，与 viewer 同源、
//!   各自独立背压）；默认 H.264 → finalize 时封装 MP4（不重编码，PTS 原样
//!   保留，见 [`mp4`] 模块头注），分段边界记录进会话时间轴；
//! - 录制数据落 `data/media/<media-id>/recording/`（会话元数据 + 事件 JSONL）；
//!   每个分段是一条独立媒体素材（metadata.json: importing → ready）；
//! - InputObserver 只观察**已被接受**的输入（手动/Keymap/Runner/插件），
//!   观察点 = `ScrcpySession::inject_touch / inject_keycode / inject_text`
//!   （全部来源的设备注入唯一收敛点）；原始 down/move/up 按 pointer 压缩为
//!   语义 tap/swipe（同一下按 operation_id 关联，不重复生成语义动作），
//!   key down/up 配对为一次按键；不解释 YAML、不回放、不记录浏览器全局键盘；
//!   敏感文本默认脱敏（只留长度，不留明文）。
//!
//! 状态机：`recording → finalizing → completed`；异常 `interrupted/failed`；
//! 用户取消 `cancelled`。停止/取消幂等。同一设备同时至多一个活动会话。
//! 浏览器断开不影响录制（服务端权威）。
//!
//! 实时安全：录制消费任务独占订阅广播（tokio broadcast 慢消费者被 Lagged
//! 丢弃并计数，绝不反压采集/推流路径）；输入观察在无活动会话时是一次
//! `OnceLock::get` + 空 map 查找的直通开销。

pub mod mp4;

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};

use crate::config::Config;
use crate::device::DeviceManager;
use crate::media::{MediaId, MediaMetadata, MediaSource, MediaState};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RecordingId(pub String);

/// 录制会话状态（wire = snake_case）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingState {
    Recording,
    Finalizing,
    Completed,
    /// 会话边界（断连/编码参数变化）后已安全收尾的部分。
    Interrupted,
    Failed,
    Cancelled,
}

/// 分段原因：编码参数变化/断连/磁盘压力导致的会话时间轴分段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentReason {
    Normal,
    Disconnect,
    CodecChange,
    DiskPressure,
}

/// 一段视频片段（media 库中的一条素材 + 会话时间轴区间）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentMeta {
    pub media_id: MediaId,
    /// 会话时间轴起点/时长（微秒，单调时钟域）。
    pub start_us: u64,
    pub duration_us: u64,
    /// 段首帧原始媒体 PTS（微秒）。**事件时间轴 ↔ 媒体 PTS 的整数映射基准**：
    /// `media_pts_us = timeline_us - start_us + base_pts_us`（会话单调钟与
    /// 媒体时钟同刻度对齐，整数微秒、不混用浏览器时间）。
    #[serde(default)]
    pub base_pts_us: u64,
    pub reason: SegmentReason,
}

impl SegmentMeta {
    /// 会话时间轴时刻（微秒，事件 `timeline_us` 同域）→ 段内媒体 PTS。
    /// 时刻早于段起点时截断为段首帧 PTS（saturating）。
    /// 当前消费方：草稿生成/时间轴 UI 读 session.json 后换算（下一阶段接线）；
    /// 本阶段由录制测试锁定语义。
    #[allow(dead_code)]
    pub fn media_pts_for_timeline(&self, timeline_us: u64) -> u64 {
        self.base_pts_us + timeline_us.saturating_sub(self.start_us)
    }
}

/// 录制会话元数据（`recording/session.json` 形态即 wire 形态）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingSessionMeta {
    pub id: RecordingId,
    pub device_id: String,
    pub state: RecordingState,
    /// RFC3339。
    pub started_at: String,
    pub ended_at: Option<String>,
    /// 会话时间轴（一段或多段；不同会话的时间戳绝不拼接）。
    pub segments: Vec<SegmentMeta>,
    pub event_count: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingStartReq {
    pub device_id: String,
}

/// 操作事件（wire 形态；语义与字段口径见实施合同 §4）。
/// 只记录**已被接受**的操作；原始事件流与语义事件同流存储，
/// 以 `operation_id` 关联去重（原始 down/move/up 不重复生成语义动作）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputEventRecord {
    pub schema_version: u32,
    pub event_id: String,
    pub operation_id: String,
    pub session_id: String,
    /// manual | keymap | runner | plugin
    pub source: String,
    /// tap | swipe | key | text | wait
    pub kind: String,
    /// 会话时间轴（微秒，服务端单调时钟域；录像起录时建立与媒体 PTS 的映射）。
    pub timeline_us: u64,
    pub time_domain: String,
    pub coordinate_space: String,
    pub display_size: DisplaySize,
    /// kind 相关负载；text 默认脱敏（长度/摘要，不留明文）。
    pub payload: serde_json::Value,
    /// accepted | rejected
    pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplaySize {
    pub width: u32,
    pub height: u32,
}

/// 结构化失败：api 层按 kind 映射 HTTP 状态码（404/409/400/500）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Busy,
    DeviceOffline,
    NotFound,
    Invalid,
    /// 预留：无结构化归类的服务内错误（api 层映射 500）。
    #[allow(dead_code)]
    Internal,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct RecordingFailure {
    pub kind: FailureKind,
    pub message: String,
}

impl RecordingFailure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

fn failure<T>(kind: FailureKind, message: impl Into<String>) -> anyhow::Result<T> {
    Err(anyhow::Error::from(RecordingFailure::new(kind, message)))
}

// ---------------------------------------------------------------------------
// 输入来源（合同 §2.1：manual | keymap | runner | plugin）
// ---------------------------------------------------------------------------

tokio::task_local! {
    /// 当前任务内"进入设备发送路径"的输入来源。设备注入原语（scrcpy 层）
    /// 观察时读取；未 scope 的调用（REST/投屏 DataChannel 的浏览器直发）
    /// 默认 manual。
    static INPUT_SOURCE: &'static str;
}

/// 在 `source` 语义下执行一段输入调用（manual/keymap/runner/plugin）。
/// 能力层适配器（runner/plugin/keymap 来源）用该 API 标注来源；嵌套 scope
/// 以最内层为准。
pub async fn with_input_source<R>(
    source: &'static str,
    fut: impl std::future::Future<Output = R>,
) -> R {
    INPUT_SOURCE.scope(source, fut).await
}

fn current_source() -> &'static str {
    INPUT_SOURCE.try_with(|s| *s).unwrap_or("manual")
}

// ---------------------------------------------------------------------------
// 会话内部状态
// ---------------------------------------------------------------------------

/// tap/swipe 判定阈值：DOWN→UP 漂移（切比雪夫距离，device-display 像素）。
const TAP_MAX_DRIFT: u32 = 8;
/// 单事件 JSONL 文件行数上限（超过滚动 events-NNNN.jsonl）。
const EVENTS_FILE_MAX_LINES: u64 = 20_000;
/// 单会话字节配额（触顶按磁盘压力安全收尾；u32 mdat 上限由 muxer 另行保护）。
const MAX_SESSION_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// 磁盘剩余空间下限（低于即按磁盘压力收尾；非 Windows 平台跳过探测）。
const MIN_FREE_BYTES: u64 = 1024 * 1024 * 1024;
/// 同指针活动手势上限（防泄漏；正常交互远低于该值）。
const MAX_ACTIVE_GESTURES: usize = 64;

/// scrcpy 触控动作（与 `device::scrcpy` 常量同值；本地声明保持录制层不反向
/// 依赖设备层）。
const TOUCH_DOWN: u8 = 0;
const TOUCH_UP: u8 = 1;
/// scrcpy keycode 动作：0=down 1=up。
const KEY_DOWN: u8 = 0;
const KEY_UP: u8 = 1;

struct TouchGesture {
    x0: u32,
    y0: u32,
    last_x: u32,
    last_y: u32,
    down_us: u64,
    operation_id: String,
}

struct KeyPending {
    down_us: u64,
    operation_id: String,
}

/// 一段视频的写出器：独立媒体素材目录 + MP4 muxer。
struct SegmentWriter {
    media_id: MediaId,
    name: String,
    dir: PathBuf,
    /// 会话时间轴起点（单调时钟域）。
    start_us: u64,
    /// 段首帧原始 PTS（文件内 PTS 归一化为 0 起）。
    base_pts: u64,
    last_raw_pts: u64,
    frames: u64,
    writer: mp4::H264Mp4Writer,
}

/// 事件 JSONL 追加器（events-NNNN.jsonl，滚动写入）。
struct EventLog {
    dir: PathBuf,
    file: Option<std::fs::File>,
    file_seq: u32,
    lines: u64,
}

impl EventLog {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            file: None,
            file_seq: 0,
            lines: 0,
        }
    }

    fn append(&mut self, record: &InputEventRecord) {
        if self.file.is_none() || self.lines >= EVENTS_FILE_MAX_LINES {
            self.file_seq += 1;
            let path = self.dir.join(format!("events-{:04}.jsonl", self.file_seq));
            match std::fs::File::options()
                .create(true)
                .append(true)
                .open(&path)
            {
                Ok(f) => {
                    self.file = Some(f);
                    self.lines = 0;
                }
                Err(e) => {
                    warn!(path = %path.display(), err = %e, "事件文件打开失败，事件丢弃");
                    return;
                }
            }
        }
        match serde_json::to_string(record) {
            Ok(mut line) => {
                line.push('\n');
                if let Some(f) = self.file.as_mut() {
                    // 事件写入失败不致命：计数已进 meta，读侧按实际文件为准
                    if f.write_all(line.as_bytes()).is_ok() {
                        self.lines += 1;
                    }
                }
            }
            Err(e) => warn!(err = %e, "事件序列化失败"),
        }
    }

    fn close(&mut self) {
        self.file = None;
    }
}

struct SessionState {
    meta: RecordingSessionMeta,
    /// 会话单调时钟起点（事件 timeline_us 与分段起点的时域）。
    origin: Instant,
    width: u32,
    height: u32,
    /// 素材展示名基名（分段按 -partN 追加）。
    asset_base_name: String,
    current: Option<SegmentWriter>,
    /// 待开段使用的参数集（SPS/PPS Annex-B）。
    pending_config: Option<Vec<u8>>,
    events: EventLog,
    dropped_frames: u64,
    written_bytes: u64,
    segment_seq: u32,
    op_counter: u64,
    evt_counter: u64,
    touch: HashMap<u64, TouchGesture>,
    keys: HashMap<u32, KeyPending>,
    frames_since_space_check: u32,
}

impl SessionState {
    fn elapsed_us(&self) -> u64 {
        self.origin.elapsed().as_micros() as u64
    }

    fn next_op(&mut self) -> String {
        self.op_counter += 1;
        format!("op-{:09}", self.op_counter)
    }

    fn emit(
        &mut self,
        session_id: &str,
        kind: &str,
        payload: serde_json::Value,
        timeline_us: u64,
        operation_id: String,
    ) {
        self.evt_counter += 1;
        let record = InputEventRecord {
            schema_version: 1,
            event_id: format!("evt-{:09}", self.evt_counter),
            operation_id,
            session_id: session_id.to_string(),
            source: current_source().to_string(),
            kind: kind.to_string(),
            timeline_us,
            time_domain: "recording".to_string(),
            coordinate_space: "device-display".to_string(),
            display_size: DisplaySize {
                width: self.width,
                height: self.height,
            },
            payload,
            status: "accepted".to_string(),
        };
        self.meta.event_count += 1;
        self.events.append(&record);
    }
}

/// 一个活动（或已收尾仍在内存里的）录制会话。
pub(crate) struct SessionShared {
    inner: Arc<Inner>,
    id: RecordingId,
    device_id: String,
    /// 主素材目录 `data/media/<primary-media-id>/`（session.json + events 所在）。
    dir: PathBuf,
    stop_notify: tokio::sync::Notify,
    stopping: AtomicBool,
    state: Mutex<SessionState>,
}

impl SessionShared {
    fn meta_snapshot(&self) -> RecordingSessionMeta {
        self.state.lock().meta.clone()
    }

    // ---------- 帧路径（录制消费任务专用；不反压实时链路） ----------

    fn feed_frame(&self, frame: crate::device::scrcpy::VideoFrame) {
        let mut st = self.state.lock();
        if st.meta.state != RecordingState::Recording {
            return;
        }
        if frame.is_config {
            // 参数集：相同则忽略；变化 = 编码参数变化 → 收口当前段
            if st.pending_config.as_deref() == Some(frame.data.as_ref()) {
                return;
            }
            if st.current.is_some() {
                self.close_segment(&mut st, SegmentReason::CodecChange);
                self.persist(&st);
            }
            st.pending_config = Some(frame.data.to_vec());
            return;
        }
        if st.current.is_none() {
            // 起录等待可解码起点：IDR + 有效参数集（pending 或样本内联）
            if !frame.is_keyframe {
                return;
            }
            let has_config = st.pending_config.is_some() || mp4::au_contains_param_set(&frame.data);
            if !has_config {
                return;
            }
            if let Err(e) = self.open_segment(&mut st, frame.pts_us) {
                self.finalize_with(
                    &mut st,
                    RecordingState::Failed,
                    SegmentReason::Normal,
                    Some(format!("录制开段失败: {e:#}")),
                );
                return;
            }
        }
        // PTS 回退检测：编码器重启会重置媒体时钟 → 按编码变化分段，绝不把
        // 不同时钟的时间戳拼进同一段。
        let pts_reset = {
            let seg = st.current.as_ref().expect("current segment open above");
            frame.pts_us + 500_000 < seg.last_raw_pts
        };
        if pts_reset {
            self.close_segment(&mut st, SegmentReason::CodecChange);
            self.persist(&st);
            if !frame.is_keyframe {
                return; // 等下一个关键帧开新段
            }
            if let Err(e) = self.open_segment(&mut st, frame.pts_us) {
                self.finalize_with(
                    &mut st,
                    RecordingState::Failed,
                    SegmentReason::Normal,
                    Some(format!("录制开段失败: {e:#}")),
                );
                return;
            }
        }
        let write_result = {
            let seg = st.current.as_mut().expect("segment open");
            let rel_pts = frame.pts_us.saturating_sub(seg.base_pts);
            seg.writer
                .write_annexb_sample(&frame.data, rel_pts, frame.is_keyframe)
        };
        match write_result {
            Ok(n) => {
                st.written_bytes += n as u64;
                let seg = st.current.as_mut().expect("segment open");
                seg.frames += 1;
                seg.last_raw_pts = frame.pts_us;
            }
            Err(e) => {
                self.finalize_with(
                    &mut st,
                    RecordingState::Failed,
                    SegmentReason::Normal,
                    Some(format!("录制写入失败: {e}")),
                );
                return;
            }
        }
        // 磁盘压力 / 容量配额采样（每 512 帧一次，零稳态开销）
        st.frames_since_space_check += 1;
        if st.frames_since_space_check >= 512 {
            st.frames_since_space_check = 0;
            let pressured = st.written_bytes >= MAX_SESSION_BYTES
                || disk_free_bytes(&self.dir).is_some_and(|free| free < MIN_FREE_BYTES);
            if pressured {
                self.close_segment(&mut st, SegmentReason::DiskPressure);
                self.finalize_with(
                    &mut st,
                    RecordingState::Interrupted,
                    SegmentReason::DiskPressure,
                    Some("磁盘空间不足或达到会话容量上限，录制已安全收尾".to_string()),
                );
            }
        }
    }

    fn note_dropped(&self, n: u64) {
        let mut st = self.state.lock();
        st.dropped_frames += n;
        if st.dropped_frames % 300 < n {
            warn!(
                recording = %self.id.0,
                dropped = st.dropped_frames,
                "录制消费者落后于实时帧流，丢弃部分帧（解码在下个 IDR 自愈，最长 2s）"
            );
        }
    }

    // ---------- 分段与会话收口 ----------

    fn open_segment(&self, st: &mut SessionState, first_pts: u64) -> anyhow::Result<()> {
        let media = MediaId(uuid::Uuid::new_v4().simple().to_string());
        let dir = self.inner.data_root.join(&media.0);
        std::fs::create_dir_all(&dir)?;
        let mut writer =
            mp4::H264Mp4Writer::create(&dir.join("original.mp4"), st.width, st.height)?;
        if let Some(cfg) = st.pending_config.take() {
            writer.set_config(&cfg);
        }
        st.segment_seq += 1;
        let name = segment_asset_name(&st.asset_base_name, st.segment_seq);
        write_media_metadata(
            &dir,
            &media,
            &name,
            st.width,
            st.height,
            MediaState::Importing,
            "",
            0,
            None,
        );
        st.current = Some(SegmentWriter {
            media_id: media,
            name,
            dir,
            start_us: st.elapsed_us(),
            base_pts: first_pts,
            last_raw_pts: first_pts,
            frames: 0,
            writer,
        });
        self.persist(st);
        Ok(())
    }

    fn close_segment(&self, st: &mut SessionState, reason: SegmentReason) {
        let Some(seg) = st.current.take() else {
            return;
        };
        let (media_id, media_name, media_dir) =
            (seg.media_id.clone(), seg.name.clone(), seg.dir.clone());
        match seg.writer.finish() {
            Ok(summary) => {
                write_media_metadata(
                    &media_dir,
                    &media_id,
                    &media_name,
                    st.width,
                    st.height,
                    MediaState::Ready,
                    &summary.sha256,
                    summary.size_bytes,
                    Some(summary.duration_us),
                );
                st.meta.segments.push(SegmentMeta {
                    media_id: media_id.clone(),
                    start_us: seg.start_us,
                    // 单调时钟域时长（事件时间轴同域；素材自身时长在其 metadata）
                    duration_us: st.elapsed_us().saturating_sub(seg.start_us),
                    // 段首帧原始媒体 PTS：事件 timeline_us ↔ 媒体 PTS 的整数映射基准
                    base_pts_us: seg.base_pts,
                    reason,
                });
                debug_segment_closed(&self.id.0, &media_id.0, &summary, seg.frames);
            }
            Err(e) => {
                // 段废弃：素材停在 importing（不出现在媒体列表），已收口的
                // 前序段完好。
                warn!(
                    recording = %self.id.0,
                    media = %media_id.0,
                    frames = seg.frames,
                    err = %e,
                    "分段收口失败，该段素材不可用"
                );
            }
        }
    }

    /// 会话内分段（编码参数变化）：收口当前段，会话继续。
    fn segment_boundary(&self, st: &mut SessionState, reason: SegmentReason) {
        self.close_segment(st, reason);
        self.persist(st);
    }

    /// 终态转换（幂等）：终态直返当前 meta。
    fn finalize_with(
        &self,
        st: &mut SessionState,
        target: RecordingState,
        close_reason: SegmentReason,
        error: Option<String>,
    ) -> RecordingSessionMeta {
        if !matches!(
            st.meta.state,
            RecordingState::Recording | RecordingState::Finalizing
        ) {
            return st.meta.clone();
        }
        st.meta.state = RecordingState::Finalizing;
        self.close_segment(st, close_reason);
        st.meta.state = target;
        st.meta.ended_at = Some(now_rfc3339());
        st.meta.error = error;
        st.events.close();
        self.persist(st);
        self.inner.release_device(&self.device_id, &self.id.0);
        self.stopping.store(true, Ordering::Release);
        self.stop_notify.notify_waiters();
        st.meta.clone()
    }

    fn finalize_stop(&self) -> RecordingSessionMeta {
        let mut st = self.state.lock();
        self.finalize_with(
            &mut st,
            RecordingState::Completed,
            SegmentReason::Normal,
            None,
        )
    }

    fn finalize_cancel(&self) -> RecordingSessionMeta {
        let mut st = self.state.lock();
        self.finalize_with(
            &mut st,
            RecordingState::Cancelled,
            SegmentReason::Normal,
            None,
        )
    }

    fn end_interrupted(&self, reason: SegmentReason, error: Option<String>) {
        let mut st = self.state.lock();
        self.finalize_with(&mut st, RecordingState::Interrupted, reason, error);
    }

    // ---------- 输入观察（合同 §2.1） ----------

    /// 原始触控注入观察：按 pointer 关联 down/move/up，UP 时压缩为一条语义
    /// tap/swipe（operation_id = DOWN 时分配），不重复生成语义动作。
    pub(crate) fn on_touch(&self, action: u8, pointer_id: u64, x: u32, y: u32) {
        let mut st = self.state.lock();
        if st.meta.state != RecordingState::Recording {
            return;
        }
        let now_us = st.elapsed_us();
        match action {
            TOUCH_DOWN => {
                if st.touch.len() >= MAX_ACTIVE_GESTURES {
                    return;
                }
                let operation_id = st.next_op();
                st.touch.insert(
                    pointer_id,
                    TouchGesture {
                        x0: x,
                        y0: y,
                        last_x: x,
                        last_y: y,
                        down_us: now_us,
                        operation_id,
                    },
                );
            }
            TOUCH_UP => {
                if let Some(g) = st.touch.remove(&pointer_id) {
                    let drift = g.x0.abs_diff(g.last_x).max(g.y0.abs_diff(g.last_y));
                    let (kind, payload) = if drift <= TAP_MAX_DRIFT {
                        ("tap", json!({ "x": g.x0, "y": g.y0 }))
                    } else {
                        (
                            "swipe",
                            json!({ "x": g.x0, "y": g.y0, "x2": g.last_x, "y2": g.last_y }),
                        )
                    };
                    st.emit(&self.id.0, kind, payload, g.down_us, g.operation_id);
                }
            }
            // MOVE 及其他动作只更新轨迹
            _ => {
                if let Some(g) = st.touch.get_mut(&pointer_id) {
                    g.last_x = x;
                    g.last_y = y;
                }
            }
        }
    }

    /// 原始按键注入观察：down/up 按 keycode 配对为一次 key 事件；无 down 的
    /// up（外部直发）单独成事件。
    pub(crate) fn on_key(&self, action: u8, code: u32) {
        let mut st = self.state.lock();
        if st.meta.state != RecordingState::Recording {
            return;
        }
        let now_us = st.elapsed_us();
        match action {
            KEY_DOWN => {
                if st.keys.len() >= MAX_ACTIVE_GESTURES || st.keys.contains_key(&code) {
                    return; // 自动重复的 down 不重开手势（保持首个 down 时刻）
                }
                let operation_id = st.next_op();
                st.keys.insert(
                    code,
                    KeyPending {
                        down_us: now_us,
                        operation_id,
                    },
                );
            }
            KEY_UP => match st.keys.remove(&code) {
                Some(p) => st.emit(
                    &self.id.0,
                    "key",
                    json!({ "code": code }),
                    p.down_us,
                    p.operation_id,
                ),
                None => {
                    let op = st.next_op();
                    st.emit(&self.id.0, "key", json!({ "code": code }), now_us, op);
                }
            },
            _ => {}
        }
    }

    /// 文本注入观察：默认脱敏，只留字符数，不留明文。
    pub(crate) fn on_text(&self, text: &str) {
        let mut st = self.state.lock();
        if st.meta.state != RecordingState::Recording {
            return;
        }
        let length = text.chars().count();
        let op = st.next_op();
        let now_us = st.elapsed_us();
        st.emit(&self.id.0, "text", json!({ "length": length }), now_us, op);
    }

    // ---------- 持久化 / 读取 ----------

    fn persist(&self, st: &SessionState) {
        let path = self.dir.join("recording").join("session.json");
        match serde_json::to_string_pretty(&st.meta) {
            Ok(s) => {
                if let Err(e) = crate::core::fs::atomic_write(&path, s.as_bytes()) {
                    warn!(recording = %self.id.0, err = %e, "session.json 写入失败");
                }
            }
            Err(e) => warn!(recording = %self.id.0, err = %e, "session.json 序列化失败"),
        }
    }

    fn read_events(&self) -> Vec<InputEventRecord> {
        read_events_dir(&self.dir)
    }
}

fn read_events_dir(media_dir: &Path) -> Vec<InputEventRecord> {
    let recording_dir = media_dir.join("recording");
    let mut files: Vec<PathBuf> = match std::fs::read_dir(&recording_dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                name.starts_with("events-") && name.ends_with(".jsonl")
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            // 容忍正在写入的最后一行（半行 JSON 解析失败即跳过）
            if let Ok(record) = serde_json::from_str::<InputEventRecord>(line) {
                out.push(record);
            }
        }
    }
    out.sort_by_key(|e| e.timeline_us); // 稳定排序：同刻保持文件顺序
    out
}

// ---------------------------------------------------------------------------
// 服务
// ---------------------------------------------------------------------------

/// Core 录制服务。进程级单例（[`service`]），零组合根接线。
pub struct RecordingService {
    inner: Arc<Inner>,
}

struct Inner {
    data_root: PathBuf,
    sessions: Mutex<HashMap<String, Arc<SessionShared>>>,
    active_by_device: Mutex<HashMap<String, String>>,
}

impl Inner {
    fn get(&self, id: &RecordingId) -> Option<Arc<SessionShared>> {
        self.sessions.lock().get(&id.0).cloned()
    }

    fn lookup_active(&self, device_id: &str) -> Option<Arc<SessionShared>> {
        let rec = self.active_by_device.lock().get(device_id).cloned()?;
        self.sessions.lock().get(&rec).cloned()
    }

    fn release_device(&self, device_id: &str, recording_id: &str) {
        let mut map = self.active_by_device.lock();
        if map.get(device_id).is_some_and(|v| v == recording_id) {
            map.remove(device_id);
        }
    }
}

impl RecordingService {
    pub fn open(data_root: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            inner: Arc::new(Inner {
                data_root,
                sessions: Mutex::new(HashMap::new()),
                active_by_device: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// 素材根目录（骨架合同保留访问器；当前无内部读取方）。
    #[allow(dead_code)]
    pub fn data_root(&self) -> &Path {
        &self.inner.data_root
    }

    /// 启动设备录制会话（设备独占；已有活动会话 → 明确错误 409 语义）。
    /// 起录等待可解码 IDR + 有效参数集；成功即登记 media 素材（importing）。
    pub fn start(
        &self,
        devices: &Arc<DeviceManager>,
        req: &RecordingStartReq,
    ) -> anyhow::Result<RecordingSessionMeta> {
        let device_id = req.device_id.trim();
        if device_id.is_empty() {
            return failure(FailureKind::Invalid, "device_id 不能为空");
        }
        if devices.snapshot(device_id).is_none() {
            return failure(FailureKind::NotFound, "device_not_found");
        }
        let Some(session) = devices.session(device_id) else {
            return failure(
                FailureKind::DeviceOffline,
                "设备未连接，无法开始录制（请先连接设备）",
            );
        };
        let Some(frames_tx) = devices.frames_tx(device_id) else {
            return failure(
                FailureKind::DeviceOffline,
                "设备视频帧分发不可用，无法开始录制（请先连接设备）",
            );
        };
        if self.inner.lookup_active(device_id).is_some() {
            return failure(FailureKind::Busy, "该设备已有进行中的录制会话");
        }

        let id = RecordingId(uuid::Uuid::new_v4().simple().to_string());
        let primary_media = MediaId(uuid::Uuid::new_v4().simple().to_string());
        let dir = self.inner.data_root.join(&primary_media.0);
        std::fs::create_dir_all(dir.join("recording"))?;
        let (width, height) = session.video_size();
        let asset_base_name = format!("recording-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
        let meta = RecordingSessionMeta {
            id: id.clone(),
            device_id: device_id.to_string(),
            state: RecordingState::Recording,
            started_at: now_rfc3339(),
            ended_at: None,
            segments: Vec::new(),
            event_count: 0,
            error: None,
        };
        let shared = Arc::new(SessionShared {
            inner: self.inner.clone(),
            id: id.clone(),
            device_id: device_id.to_string(),
            dir: dir.clone(),
            stop_notify: tokio::sync::Notify::new(),
            stopping: AtomicBool::new(false),
            state: Mutex::new(SessionState {
                meta,
                origin: Instant::now(),
                width,
                height,
                asset_base_name,
                current: None,
                pending_config: None,
                events: EventLog::new(dir.join("recording")),
                dropped_frames: 0,
                written_bytes: 0,
                segment_seq: 0,
                op_counter: 0,
                evt_counter: 0,
                touch: HashMap::new(),
                keys: HashMap::new(),
                frames_since_space_check: 0,
            }),
        });
        // 登记：media 素材（importing）+ session.json（先建会话再写盘，失败路径
        // 由下方 finalize 收敛为 failed）
        {
            let st = shared.state.lock();
            write_media_metadata(
                &dir,
                &primary_media,
                &st.asset_base_name,
                width,
                height,
                MediaState::Importing,
                "",
                0,
                None,
            );
            shared.persist(&st);
        }
        self.inner
            .sessions
            .lock()
            .insert(id.0.clone(), shared.clone());
        self.inner
            .active_by_device
            .lock()
            .insert(device_id.to_string(), id.0.clone());

        // 帧订阅消费任务：广播慢消费者被 Lagged 丢弃（有界队列语义），
        // 任何失败只影响录制会话，绝不反压采集/推流路径。
        let consumer = shared.clone();
        tokio::spawn(async move {
            let mut rx = frames_tx.subscribe();
            loop {
                tokio::select! {
                    _ = consumer.stop_notify.notified() => break,
                    received = rx.recv() => match received {
                        Ok(frame) => consumer.feed_frame(frame),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            consumer.note_dropped(n);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            consumer.end_interrupted(
                                SegmentReason::Disconnect,
                                Some("设备视频流结束，录制已安全收尾".to_string()),
                            );
                            break;
                        }
                    }
                }
                if consumer.stopping.load(Ordering::Acquire) {
                    break;
                }
            }
        });

        // 起录立即请求设备输出 IDR（reset_video；best effort，不阻塞返回）
        let reset = Some(session);
        tokio::spawn(async move {
            if let Some(session) = reset {
                let _ = tokio::time::timeout(Duration::from_secs(1), session.reset_video()).await;
            }
        });

        info!(recording = %id.0, device = %device_id, "recording session started");
        Ok(shared.meta_snapshot())
    }

    /// 停止并 finalize（封装 MP4、媒体库转 ready）。幂等：重复 stop 返回终态。
    pub fn stop(&self, id: &RecordingId) -> anyhow::Result<RecordingSessionMeta> {
        let shared = self
            .inner
            .get(id)
            .ok_or_else(|| RecordingFailure::new(FailureKind::NotFound, "recording_not_found"))?;
        Ok(shared.finalize_stop())
    }

    /// 取消：已落盘部分保留为 interrupted 素材（可查看），状态 cancelled。
    pub fn cancel(&self, id: &RecordingId) -> anyhow::Result<RecordingSessionMeta> {
        let shared = self
            .inner
            .get(id)
            .ok_or_else(|| RecordingFailure::new(FailureKind::NotFound, "recording_not_found"))?;
        Ok(shared.finalize_cancel())
    }

    pub fn status(&self, id: &RecordingId) -> anyhow::Result<RecordingSessionMeta> {
        if let Some(shared) = self.inner.get(id) {
            return Ok(shared.meta_snapshot());
        }
        self.load_from_disk(id)
            .map(|(meta, _)| meta)
            .ok_or_else(|| {
                anyhow::Error::from(RecordingFailure::new(
                    FailureKind::NotFound,
                    "recording_not_found",
                ))
            })
    }

    /// 该设备当前活动会话（无则 None；前端轮询/录制按钮态）。
    pub fn active_for_device(&self, device_id: &str) -> Option<RecordingSessionMeta> {
        self.inner
            .lookup_active(device_id)
            .map(|shared| shared.meta_snapshot())
    }

    /// 读取会话的操作事件（时间轴升序；来自 recording/events-*.jsonl）。
    pub fn events(&self, id: &RecordingId) -> anyhow::Result<Vec<InputEventRecord>> {
        if let Some(shared) = self.inner.get(id) {
            return Ok(shared.read_events());
        }
        self.load_from_disk(id)
            .map(|(_, dir)| read_events_dir(&dir))
            .ok_or_else(|| {
                anyhow::Error::from(RecordingFailure::new(
                    FailureKind::NotFound,
                    "recording_not_found",
                ))
            })
    }

    /// 设备会话确死回调（看门狗/断连）：当前会话安全收尾为分段/中断，
    /// 不阻塞实时链路。由 device 层在会话边界调用。
    pub fn on_device_session_boundary(&self, device_id: &str, reason: SegmentReason) {
        let Some(shared) = self.inner.lookup_active(device_id) else {
            return;
        };
        match reason {
            SegmentReason::Disconnect => shared.end_interrupted(SegmentReason::Disconnect, None),
            // 编码/磁盘边界：只收口当前段（会话继续）；磁盘压力下的整场收尾
            // 由录制链路自身的容量探测触发。
            SegmentReason::CodecChange | SegmentReason::DiskPressure | SegmentReason::Normal => {
                let mut st = shared.state.lock();
                if st.meta.state == RecordingState::Recording {
                    shared.segment_boundary(&mut st, reason);
                }
            }
        }
    }

    /// 磁盘恢复：按 recording id 扫描 `data/media/*/recording/session.json`。
    /// 崩溃/重启遗留的活动态会话标记为 interrupted（帧订阅随进程消失，无法
    /// 续录；已收口段完好，当前段素材停留在 importing 不可见）。
    fn load_from_disk(&self, id: &RecordingId) -> Option<(RecordingSessionMeta, PathBuf)> {
        let entries = std::fs::read_dir(&self.inner.data_root).ok()?;
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let session_path = dir.join("recording").join("session.json");
            let Ok(text) = std::fs::read_to_string(&session_path) else {
                continue;
            };
            let Ok(mut meta) = serde_json::from_str::<RecordingSessionMeta>(&text) else {
                continue;
            };
            if meta.id != *id {
                continue;
            }
            if matches!(
                meta.state,
                RecordingState::Recording | RecordingState::Finalizing
            ) {
                meta.state = RecordingState::Interrupted;
                meta.ended_at = Some(now_rfc3339());
                meta.error = Some("服务重启导致录制中断（已收口部分保留）".to_string());
                if let Ok(s) = serde_json::to_string_pretty(&meta) {
                    let _ = crate::core::fs::atomic_write(&session_path, s.as_bytes());
                }
            }
            return Some((meta, dir));
        }
        None
    }
}

/// 进程级录制服务（惰性装配；不加 AppState 字段）。
static RECORDING: OnceLock<Arc<RecordingService>> = OnceLock::new();

/// 输入观察注册表（进程级；由 [`service`] 首次装配时播种）。
/// scrcpy 注入原语经 [`observe_touch`] / [`observe_key`] / [`observe_text`]
/// 直通查询，无活动会话时零开销。
static OBSERVER: OnceLock<Arc<Inner>> = OnceLock::new();

pub fn service(cfg: &Config) -> Arc<RecordingService> {
    let svc = RECORDING
        .get_or_init(|| {
            let root = cfg.data_dir.join("media");
            Arc::new(RecordingService::open(root).expect("recording service init"))
        })
        .clone();
    let _ = OBSERVER.set(svc.inner.clone());
    svc
}

// ---------------------------------------------------------------------------
// scrcpy 注入原语观察入口（device 层调用；无活动会话 = 直通）
// ---------------------------------------------------------------------------

/// 原始触控注入已被接受（send 成功）后调用。
pub(crate) fn observe_touch(device_id: &str, action: u8, pointer_id: u64, x: u32, y: u32) {
    let Some(inner) = OBSERVER.get() else {
        return;
    };
    if let Some(shared) = inner.lookup_active(device_id) {
        shared.on_touch(action, pointer_id, x, y);
    }
}

/// 原始按键注入已被接受（send 成功）后调用（action: 0=down 1=up）。
pub(crate) fn observe_key(device_id: &str, action: u8, code: u32) {
    let Some(inner) = OBSERVER.get() else {
        return;
    };
    if let Some(shared) = inner.lookup_active(device_id) {
        shared.on_key(action, code);
    }
}

/// 文本注入已被接受（send 成功）后调用（记录长度，不留明文）。
pub(crate) fn observe_text(device_id: &str, text: &str) {
    let Some(inner) = OBSERVER.get() else {
        return;
    };
    if let Some(shared) = inner.lookup_active(device_id) {
        shared.on_text(text);
    }
}

/// 设备视频读循环退出（socket 断开）：活动录制立即安全收尾为 interrupted，
/// 不等看门狗。幂等（随后 device 层的 disconnect 边界命中终态直返）。
pub(crate) fn on_video_stream_ended(device_id: &str) {
    let Some(inner) = OBSERVER.get() else {
        return;
    };
    if let Some(shared) = inner.lookup_active(device_id) {
        info!(recording = %shared.id.0, device = %device_id, "video stream ended, finalizing recording (interrupted)");
        shared.end_interrupted(SegmentReason::Disconnect, None);
    }
}

// ---------------------------------------------------------------------------
// 磁盘与命名助手
// ---------------------------------------------------------------------------

fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339()
}

fn segment_asset_name(base: &str, seq: u32) -> String {
    if seq <= 1 {
        base.to_string()
    } else {
        format!("{base}-part{seq}")
    }
}

/// 写 `metadata.json`（与 `crate::media::MediaMetadata` 同形；录制产出由本模块
/// 登记，A 侧 MediaService 按同一格式消费）。
#[allow(clippy::too_many_arguments)]
fn write_media_metadata(
    dir: &Path,
    media: &MediaId,
    name: &str,
    width: u32,
    height: u32,
    state: MediaState,
    sha256: &str,
    size: u64,
    duration_us: Option<u64>,
) {
    let meta = MediaMetadata {
        id: media.clone(),
        name: name.to_string(),
        sha256: sha256.to_string(),
        size,
        duration_us,
        container: "mp4".to_string(),
        codec: "h264".to_string(),
        width,
        height,
        rotation: 0,
        source: MediaSource::Recording,
        state,
        created_at: now_rfc3339(),
        refs: Vec::new(),
    };
    match serde_json::to_string_pretty(&meta) {
        Ok(s) => {
            if let Err(e) = crate::core::fs::atomic_write(&dir.join("metadata.json"), s.as_bytes())
            {
                warn!(media = %media.0, err = %e, "metadata.json 写入失败");
            }
        }
        Err(e) => warn!(media = %media.0, err = %e, "metadata.json 序列化失败"),
    }
}

#[allow(unused_variables)]
fn disk_free_bytes(path: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free: u64 = 0;
        let mut total: u64 = 0;
        let mut total_free: u64 = 0;
        let ok =
            unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) };
        (ok != 0).then_some(free)
    }
    #[cfg(not(windows))]
    {
        None // 非 Windows 平台 V1 不做剩余空间探测（仅容量配额）
    }
}

fn debug_segment_closed(recording: &str, media: &str, summary: &mp4::MuxSummary, frames: u64) {
    info!(
        recording,
        media,
        frames,
        samples = summary.samples,
        bytes = summary.size_bytes,
        duration_us = summary.duration_us,
        "recording segment finalized"
    );
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::scrcpy::VideoFrame;
    use bytes::Bytes;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gamer-rec-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 绕过 DeviceManager 构造一个活动会话（纯逻辑测试；帧任务不参与）。
    fn test_session(root: &Path, device_id: &str) -> Arc<SessionShared> {
        let inner = Arc::new(Inner {
            data_root: root.to_path_buf(),
            sessions: Mutex::new(HashMap::new()),
            active_by_device: Mutex::new(HashMap::new()),
        });
        let id = RecordingId(uuid::Uuid::new_v4().simple().to_string());
        let primary = MediaId(uuid::Uuid::new_v4().simple().to_string());
        let dir = inner.data_root.join(&primary.0);
        std::fs::create_dir_all(dir.join("recording")).unwrap();
        let shared = Arc::new(SessionShared {
            inner: inner.clone(),
            id: id.clone(),
            device_id: device_id.to_string(),
            dir: dir.clone(),
            stop_notify: tokio::sync::Notify::new(),
            stopping: AtomicBool::new(false),
            state: Mutex::new(SessionState {
                meta: RecordingSessionMeta {
                    id,
                    device_id: device_id.to_string(),
                    state: RecordingState::Recording,
                    started_at: now_rfc3339(),
                    ended_at: None,
                    segments: Vec::new(),
                    event_count: 0,
                    error: None,
                },
                origin: Instant::now(),
                width: 1920,
                height: 1080,
                asset_base_name: "recording-test".to_string(),
                current: None,
                pending_config: None,
                events: EventLog::new(dir.join("recording")),
                dropped_frames: 0,
                written_bytes: 0,
                segment_seq: 0,
                op_counter: 0,
                evt_counter: 0,
                touch: HashMap::new(),
                keys: HashMap::new(),
                frames_since_space_check: 0,
            }),
        });
        inner
            .sessions
            .lock()
            .insert(shared.id.0.clone(), shared.clone());
        inner
            .active_by_device
            .lock()
            .insert(device_id.to_string(), shared.id.0.clone());
        shared
    }

    fn annexb(nals: &[&[u8]]) -> Vec<u8> {
        let mut v = Vec::new();
        for n in nals {
            v.extend([0, 0, 0, 1]);
            v.extend(*n);
        }
        v
    }

    fn nal(t: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![t];
        v.extend(payload);
        v
    }

    fn frame(data: Vec<u8>, pts_us: u64, is_config: bool, is_keyframe: bool) -> VideoFrame {
        VideoFrame {
            data: Bytes::from(data),
            pts_us,
            is_config,
            is_keyframe,
            annex_b: true,
        }
    }

    fn config_frame() -> VideoFrame {
        frame(
            annexb(&[
                &nal(7, &[0x67, 0x64, 0x00, 0x1f, 0xac]),
                &nal(8, &[0x68, 0xeb, 0xec]),
            ]),
            0,
            true,
            false,
        )
    }

    fn idr_frame(pts: u64) -> VideoFrame {
        frame(annexb(&[&nal(5, &[0xAA, 1])]), pts, false, true)
    }

    fn p_frame(pts: u64) -> VideoFrame {
        frame(annexb(&[&nal(1, &[0xBB, 2])]), pts, false, false)
    }

    fn media_state(dir: &Path, media: &str) -> MediaState {
        let text = std::fs::read_to_string(dir.join(media).join("metadata.json")).unwrap();
        serde_json::from_str::<MediaMetadata>(&text).unwrap().state
    }

    #[test]
    fn records_segment_and_stop_is_idempotent() {
        let root = temp_root("stop");
        let shared = test_session(&root, "dev-1");

        shared.feed_frame(config_frame());
        shared.feed_frame(idr_frame(0));
        for i in 1..5 {
            shared.feed_frame(p_frame(i * 33_333));
        }
        let stopped = shared.finalize_stop();
        assert_eq!(stopped.state, RecordingState::Completed);
        assert_eq!(stopped.segments.len(), 1);
        assert_eq!(stopped.segments[0].reason, SegmentReason::Normal);
        assert!(stopped.ended_at.is_some());

        // 素材转 ready，MP4 落盘
        let media = &stopped.segments[0].media_id.0;
        assert_eq!(media_state(&root, media), MediaState::Ready);
        assert!(root.join(media).join("original.mp4").is_file());

        // 幂等：重复 stop / cancel 返回同一终态，不产生新素材
        let again = shared.finalize_stop();
        assert_eq!(again, stopped);
        let cancelled = shared.finalize_cancel();
        assert_eq!(cancelled, stopped);
        assert_eq!(
            read_events_dir(&shared.dir).len(),
            0,
            "无输入时会话事件为空"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cancel_keeps_partial_segment_and_marks_terminal() {
        let root = temp_root("cancel");
        let shared = test_session(&root, "dev-1");
        shared.feed_frame(config_frame());
        shared.feed_frame(idr_frame(0));
        shared.feed_frame(p_frame(33_333));
        let meta = shared.finalize_cancel();
        assert_eq!(meta.state, RecordingState::Cancelled);
        assert_eq!(meta.segments.len(), 1, "已落盘部分保留");
        let media = &meta.segments[0].media_id.0;
        assert_eq!(
            media_state(&root, media),
            MediaState::Ready,
            "取消素材可查看"
        );
        // 幂等
        assert_eq!(shared.finalize_cancel(), meta);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn session_boundary_disconnect_finalizes_interrupted_and_stops_feeding() {
        let root = temp_root("boundary");
        let shared = test_session(&root, "dev-1");
        shared.feed_frame(config_frame());
        shared.feed_frame(idr_frame(0));
        shared.feed_frame(p_frame(33_333));
        shared.end_interrupted(SegmentReason::Disconnect, None);
        let meta = shared.meta_snapshot();
        assert_eq!(meta.state, RecordingState::Interrupted);
        assert_eq!(meta.segments.len(), 1);
        assert_eq!(meta.segments[0].reason, SegmentReason::Disconnect);

        // 断连后的帧不再进段
        shared.feed_frame(p_frame(66_666));
        assert_eq!(shared.meta_snapshot().segments.len(), 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn codec_change_splits_segments_with_reasons() {
        let root = temp_root("codec");
        let shared = test_session(&root, "dev-1");
        // 段 1：参数集 A + IDR + P
        shared.feed_frame(config_frame());
        shared.feed_frame(idr_frame(1_000));
        shared.feed_frame(p_frame(33_333));
        // 段 2：参数集 B（编码参数变化）+ 新 IDR
        let config_b = frame(
            annexb(&[
                &nal(7, &[0x67, 0x64, 0x00, 0x20, 0xac]),
                &nal(8, &[0x68, 0xeb, 0xec]),
            ]),
            0,
            true,
            false,
        );
        shared.feed_frame(config_b);
        shared.feed_frame(idr_frame(5_000));
        shared.feed_frame(p_frame(38_333));
        let meta = shared.finalize_stop();
        assert_eq!(meta.state, RecordingState::Completed);
        assert_eq!(meta.segments.len(), 2, "编码参数变化必须分段");
        assert_eq!(meta.segments[0].reason, SegmentReason::CodecChange);
        assert_eq!(meta.segments[1].reason, SegmentReason::Normal);
        assert_ne!(meta.segments[0].media_id, meta.segments[1].media_id);
        // 段首帧原始 PTS 进入 meta（事件↔媒体 PTS 映射基准）
        assert_eq!(meta.segments[0].base_pts_us, 1_000);
        assert_eq!(meta.segments[1].base_pts_us, 5_000);
        for seg in &meta.segments {
            assert_eq!(media_state(&root, &seg.media_id.0), MediaState::Ready);
        }
        // 两段时间轴不重叠
        assert!(meta.segments[1].start_us >= meta.segments[0].start_us);
        std::fs::remove_dir_all(root).ok();
    }

    /// 事件时间轴（服务端单调钟）↔ 媒体 PTS 的整数映射：`base_pts_us` 记录段首
    /// 帧原始 PTS，`media_pts_for_timeline` 把事件落回视频帧时刻，全程整数微秒、
    /// 不混用浏览器时间。
    #[test]
    fn event_timeline_maps_to_media_pts_via_segment_base_pts() {
        let root = temp_root("ptsmap");
        let shared = test_session(&root, "dev-1");
        shared.feed_frame(config_frame());
        // 段首帧 PTS = 1_000_000（≠ 0，验证映射带基准平移）
        shared.feed_frame(idr_frame(1_000_000));
        shared.feed_frame(p_frame(1_033_333));
        // 一次 tap（timeline_us = DOWN 时刻，会话单调钟域）
        shared.on_touch(TOUCH_DOWN, 7, 500, 400);
        shared.on_touch(TOUCH_UP, 7, 502, 401);
        let meta = shared.finalize_stop();

        assert_eq!(meta.segments.len(), 1);
        let seg = &meta.segments[0];
        assert_eq!(seg.base_pts_us, 1_000_000, "段首帧原始 PTS 进 meta");
        let events = shared.read_events();
        assert_eq!(events.len(), 1);
        let evt = &events[0];
        assert!(
            evt.timeline_us >= seg.start_us && evt.timeline_us <= seg.start_us + seg.duration_us,
            "事件时刻必须落在其分段区间内"
        );
        // 整数映射：事件媒体 PTS ≥ 段首帧 PTS，且 ≤ 段末帧 PTS（+段时长容差）
        let media_pts = seg.media_pts_for_timeline(evt.timeline_us);
        assert!(
            media_pts >= seg.base_pts_us && media_pts <= seg.base_pts_us + seg.duration_us + 1,
            "映射出的媒体 PTS {media_pts} 应落在段帧区间 [{}, {}]",
            seg.base_pts_us,
            seg.base_pts_us + seg.duration_us
        );
        // 早于段起点的事件时刻截断为段首帧 PTS
        assert_eq!(
            seg.media_pts_for_timeline(seg.start_us - 10),
            seg.base_pts_us
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn frames_before_idr_are_not_written() {
        let root = temp_root("waitidr");
        let shared = test_session(&root, "dev-1");
        shared.feed_frame(p_frame(0)); // 无 IDR：丢弃
        shared.feed_frame(config_frame());
        shared.feed_frame(p_frame(10_000)); // 仍无 IDR：丢弃
        shared.feed_frame(idr_frame(20_000)); // 起录
        shared.feed_frame(p_frame(53_333));
        let meta = shared.finalize_stop();
        assert_eq!(meta.segments.len(), 1);
        std::fs::remove_dir_all(root).ok();
    }

    /// 合同 §2.1：同一 operation_id 的原始流不重复产生语义动作；
    /// down/move/up 压缩为一条 tap 或 swipe。
    #[test]
    fn raw_touch_stream_dedupes_into_single_semantic_event() {
        let root = temp_root("touch");
        let shared = test_session(&root, "dev-1");

        // tap：DOWN + 微漂移 MOVE + UP → 恰好一条 tap
        shared.on_touch(TOUCH_DOWN, 7, 820, 460);
        shared.on_touch(2, 7, 821, 461); // MOVE
        shared.on_touch(TOUCH_UP, 7, 822, 462);
        // swipe：大位移 DOWN→UP → 恰好一条 swipe
        shared.on_touch(TOUCH_DOWN, 9, 100, 100);
        for i in 1..=10 {
            shared.on_touch(2, 9, 100 + i * 20, 100 + i * 10);
        }
        shared.on_touch(TOUCH_UP, 9, 300, 200);

        let events = shared.read_events();
        assert_eq!(events.len(), 2, "原始流压缩为两条语义事件：{events:?}");
        assert_eq!(events[0].kind, "tap");
        assert_eq!(events[0].payload, json!({"x": 820, "y": 460}));
        assert_eq!(events[0].source, "manual");
        assert_eq!(events[0].status, "accepted");
        assert_eq!(events[0].schema_version, 1);
        assert_eq!(events[0].session_id, shared.id.0);
        assert_eq!(
            events[0].display_size,
            DisplaySize {
                width: 1920,
                height: 1080
            }
        );
        assert_eq!(events[1].kind, "swipe");
        assert_eq!(
            events[1].payload,
            json!({"x": 100, "y": 100, "x2": 300, "y2": 200})
        );
        // operation_id 唯一且事件 id 唯一
        assert_ne!(events[0].operation_id, events[1].operation_id);
        assert_ne!(events[0].event_id, events[1].event_id);
        // 时间轴升序且 tap 的 timeline = DOWN 时刻（≤ UP 时刻）
        assert!(events[0].timeline_us <= events[1].timeline_us);
        std::fs::remove_dir_all(root).ok();
    }

    /// 合同 §2.1：text 只留长度，不留明文；key down/up 配对为一次事件。
    #[test]
    fn text_is_redacted_and_key_pairs_to_single_event() {
        let root = temp_root("redact");
        let shared = test_session(&root, "dev-1");

        shared.on_key(KEY_DOWN, 42);
        shared.on_key(KEY_DOWN, 42); // 自动重复 down 不重开
        shared.on_key(KEY_UP, 42);
        shared.on_text("超级密码 hunter2_SECRET");
        // 无 down 的 up：仍记录一次
        shared.on_key(KEY_UP, 100);

        let events = shared.read_events();
        assert_eq!(events.len(), 3, "{events:?}");
        assert_eq!(events[0].kind, "key");
        assert_eq!(events[0].payload, json!({"code": 42}));
        assert_eq!(events[1].kind, "text");
        assert_eq!(
            events[1].payload,
            json!({"length": 19}), // 4 汉字 + 空格 + 14 ASCII = 19 字符
            "只记字符数"
        );
        let serialized = serde_json::to_string(&events[1]).unwrap();
        assert!(
            !serialized.contains("hunter2")
                && !serialized.contains("SECRET")
                && !serialized.contains("超级密码"),
            "事件流不得包含明文: {serialized}"
        );
        assert_eq!(events[2].kind, "key");
        assert_eq!(events[2].payload, json!({"code": 100}));
        assert!(events.iter().all(|e| e.meta_ok()));
        std::fs::remove_dir_all(root).ok();
    }

    /// 无活动会话时观察点直通：不 panic、无落盘副作用。
    #[test]
    fn observer_passthrough_without_active_session() {
        let root = temp_root("passthrough");
        let inner = Arc::new(Inner {
            data_root: root.clone(),
            sessions: Mutex::new(HashMap::new()),
            active_by_device: Mutex::new(HashMap::new()),
        });
        // device 未登记任何会话：lookup 返回 None
        assert!(inner.lookup_active("dev-none").is_none());
        assert!(disk_free_bytes(&root).is_none_or(|f| f > 0));
        std::fs::remove_dir_all(root).ok();
    }

    /// 终态会话从内存摘除设备独占；断连边界后 active_for_device 为空。
    #[test]
    fn device_slot_released_on_terminal_state() {
        let root = temp_root("slot");
        let shared = test_session(&root, "dev-1");
        assert!(shared.inner.lookup_active("dev-1").is_some());
        shared.finalize_stop();
        assert!(
            shared.inner.lookup_active("dev-1").is_none(),
            "终态后设备独占必须释放"
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// 磁盘恢复：遗留 recording 态的 session.json → status 恢复为 interrupted。
    #[test]
    fn stale_active_session_from_disk_recovers_as_interrupted() {
        let root = temp_root("recover");
        let svc = RecordingService::open(root.clone()).unwrap();
        let media = MediaId(uuid::Uuid::new_v4().simple().to_string());
        let dir = root.join(&media.0);
        std::fs::create_dir_all(dir.join("recording")).unwrap();
        let id = RecordingId(uuid::Uuid::new_v4().simple().to_string());
        let stale = RecordingSessionMeta {
            id: id.clone(),
            device_id: "dev-9".to_string(),
            state: RecordingState::Recording,
            started_at: now_rfc3339(),
            ended_at: None,
            segments: Vec::new(),
            event_count: 3,
            error: None,
        };
        std::fs::write(
            dir.join("recording").join("session.json"),
            serde_json::to_string_pretty(&stale).unwrap(),
        )
        .unwrap();

        let recovered = svc.status(&id).unwrap();
        assert_eq!(recovered.state, RecordingState::Interrupted);
        assert!(recovered.ended_at.is_some());
        assert!(recovered.error.as_deref().unwrap().contains("重启"));
        // 幂等：再次读取终态不再改写
        let again = svc.status(&id).unwrap();
        assert_eq!(again, recovered);
        // 未知 id → not_found
        let err = svc
            .status(&RecordingId("nonexistent".to_string()))
            .unwrap_err();
        let fail = err.downcast_ref::<RecordingFailure>().unwrap();
        assert_eq!(fail.kind, FailureKind::NotFound);
        assert_eq!(fail.message, "recording_not_found");
        std::fs::remove_dir_all(root).ok();
    }

    /// wait 事件（词表内的建议间隔占位）可构造且能被事件读取链路往返。
    #[test]
    fn wait_event_roundtrip_through_jsonl() {
        let root = temp_root("wait");
        let shared = test_session(&root, "dev-1");
        {
            let mut st = shared.state.lock();
            let op = st.next_op();
            let now = st.elapsed_us();
            st.emit(
                &shared.id.0,
                "wait",
                json!({ "duration_us": 1_500_000 }),
                now,
                op,
            );
        }
        let events = shared.read_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "wait");
        assert_eq!(events[0].payload, json!({"duration_us": 1_500_000}));
        assert_eq!(events[0].time_domain, "recording");
        assert_eq!(events[0].coordinate_space, "device-display");
        std::fs::remove_dir_all(root).ok();
    }

    impl InputEventRecord {
        /// wire 形态自检：必填域口径（合同 §2.1）。
        fn meta_ok(&self) -> bool {
            self.schema_version == 1
                && self.time_domain == "recording"
                && self.coordinate_space == "device-display"
                && matches!(self.status.as_str(), "accepted" | "rejected")
                && !self.event_id.is_empty()
                && !self.operation_id.is_empty()
                && !self.session_id.is_empty()
        }
    }
}
