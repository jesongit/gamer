//! Core 录制服务与统一输入观察（视频工作台 V1）。
//!
//! 计划：`docs/plans/gamer_video_workbench_development_plan.md` §5/§7.2；
//! 实施合同：`docs/plans/gamer_video_workbench_contracts.md`。
//!
//! 职责边界：
//! - 录制器接在 scrcpy 原始编码帧分发之后、WebRTC 节流/重放之前，按需订阅；
//!   默认 H.264 → finalize 时 remux MP4（不重编码），分段边界记录进会话时间轴；
//! - 录制数据落 `data/media/<media-id>/recording/`（会话元数据 + 事件 JSONL）；
//! - InputObserver 只观察**已被接受**的输入（手动/Keymap/Runner/插件），
//!   在统一输入分发收敛点产生结构化事件；不解释 YAML、不回放、不记录
//!   浏览器全局键盘；敏感文本默认脱敏（只留长度/摘要）。
//!
//! 状态机：`recording → finalizing → completed`；异常 `interrupted/failed`；
//! 用户取消 `cancelled`。停止/取消幂等。同一设备同时至多一个活动会话。
//! 浏览器断开不影响录制（服务端权威）。

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::device::DeviceManager;
use crate::media::MediaId;

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
    pub reason: SegmentReason,
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

/// Core 录制服务。进程级单例（[`service`]），零组合根接线。
pub struct RecordingService {
    data_root: PathBuf,
}

impl RecordingService {
    pub fn open(data_root: PathBuf) -> anyhow::Result<Self> {
        Ok(Self { data_root })
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    /// 启动设备录制会话（设备独占；已有活动会话 → 明确错误 409 语义）。
    /// 起录等待可解码 IDR + 有效参数集；成功即登记 media 素材（importing）。
    pub fn start(
        &self,
        _devices: &Arc<DeviceManager>,
        _req: &RecordingStartReq,
    ) -> anyhow::Result<RecordingSessionMeta> {
        todo!("Phase 2：订阅 scrcpy 编码帧 + 起录（实施者：录制服务）")
    }

    /// 停止并 finalize（remux MP4、媒体库转 ready）。幂等：重复 stop 返回终态。
    pub fn stop(&self, _id: &RecordingId) -> anyhow::Result<RecordingSessionMeta> {
        todo!("Phase 2：安全收尾 + remux + 素材 ready")
    }

    /// 取消：已落盘部分保留为 interrupted 素材（可查看），状态 cancelled。
    pub fn cancel(&self, _id: &RecordingId) -> anyhow::Result<RecordingSessionMeta> {
        todo!("Phase 2：取消收尾")
    }

    pub fn status(&self, _id: &RecordingId) -> anyhow::Result<RecordingSessionMeta> {
        todo!("Phase 2：会话状态查询")
    }

    /// 该设备当前活动会话（无则 None；前端轮询/录制按钮态）。
    pub fn active_for_device(&self, _device_id: &str) -> Option<RecordingSessionMeta> {
        todo!("Phase 2：设备独占判定")
    }

    /// 读取会话的操作事件（时间轴升序；来自 recording/events-*.jsonl）。
    pub fn events(&self, _id: &RecordingId) -> anyhow::Result<Vec<InputEventRecord>> {
        todo!("Phase 2：事件读取")
    }

    /// 设备会话确死回调（看门狗/断连）：当前会话安全收尾为分段/中断，
    /// 不阻塞实时链路。由 device 层在会话边界调用。
    pub fn on_device_session_boundary(&self, _device_id: &str, _reason: SegmentReason) {
        todo!("Phase 2：断连/编码变化分段收尾")
    }
}

/// 进程级录制服务（惰性装配；不加 AppState 字段）。
static RECORDING: OnceLock<Arc<RecordingService>> = OnceLock::new();

pub fn service(cfg: &Config) -> Arc<RecordingService> {
    RECORDING
        .get_or_init(|| {
            let root = cfg.data_dir.join("media");
            Arc::new(RecordingService::open(root).expect("recording service init"))
        })
        .clone()
}
