//! Core 媒体库（视频工作台 V1）。
//!
//! 计划：`docs/plans/gamer_video_workbench_development_plan.md` §6/§7；
//! 实施合同（REST 形态/字段/限额/所有权）：`docs/plans/gamer_video_workbench_contracts.md`。
//!
//! Core 只管机制：全局媒体库 `data/media/<media-id>/` 的导入、探测、元数据、
//! 精确帧提取与文件生命周期。原文件不可变；元数据 source of truth =
//! 每个 media 目录下的 `metadata.json`（V1 不落 SQLite，不改 schema 版本）。
//! 模板/YAML/项目等业务语义归插件，本模块不解释。

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

use crate::config::Config;

/// 不可猜测的全局媒体 id（uuid simple）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MediaId(pub String);

/// 媒体来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaSource {
    /// 本机录制会话产出。
    Recording,
    /// 用户导入的外部素材。
    Import,
}

/// 媒体状态（导入原子提交：importing → ready；文件缺失 → missing）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaState {
    Importing,
    Ready,
    Missing,
}

/// 业务侧引用（Package 内插件项目对素材的逻辑引用；删除被引用素材须拒绝）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRef {
    pub package_id: String,
    pub plugin_id: String,
    /// 引用用途（如 `project`），Core 不解释。
    pub kind: String,
}

/// 媒体元数据（`metadata.json` 形态即 wire 形态）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaMetadata {
    pub id: MediaId,
    /// 展示名（原始文件名或录制会话名）。
    pub name: String,
    pub sha256: String,
    pub size: u64,
    /// 容器时长（微秒）；探测失败为 None。
    pub duration_us: Option<u64>,
    /// 容器（如 `mp4`）与视频 codec（如 `h264`），小写。
    pub container: String,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// 0/90/180/270（展示旋转元数据）。
    pub rotation: u16,
    pub source: MediaSource,
    pub state: MediaState,
    /// RFC3339。
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<MediaRef>,
}

/// 精确帧请求：`pts_us` 与 `index` 二选一（都给以 pts_us 优先），
/// 提取必须可重复（同一请求逐字节一致），不按固定帧率估算。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FrameRequest {
    pub pts_us: Option<u64>,
    pub index: Option<u32>,
    /// 最长边缩放上限（可选；保持纵横比）。
    pub max_width: Option<u32>,
}

/// Core 媒体库服务。进程级单例（[`service`]），组合根零接线：
/// 处理器用 `State<AppState>` 取 `cfg` 后惰性初始化。
pub struct MediaService {
    data_root: PathBuf,
}

impl MediaService {
    pub fn open(data_root: PathBuf) -> anyhow::Result<Self> {
        Ok(Self { data_root })
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    /// 导入字节：临时文件 → ffprobe 探测 → 校验 → 原子提交（导入失败不留下
    /// 可被正常引用的半成品）。同名不去重（各自独立 media id）。
    pub fn import_bytes(&self, _name: &str, _bytes: &[u8]) -> anyhow::Result<MediaMetadata> {
        todo!("Phase 1：导入 + ffprobe 探测 + 原子落盘（实施者：媒体服务）")
    }

    pub fn list(&self) -> anyhow::Result<Vec<MediaMetadata>> {
        todo!("Phase 1：按创建时间倒序列出全部 ready/missing 素材")
    }

    pub fn get(&self, _id: &MediaId) -> anyhow::Result<MediaMetadata> {
        todo!("Phase 1：读取单个素材元数据（不存在 → 结构化 not_found）")
    }

    /// 删除素材目录；仍被引用（refs 非空）→ 明确错误（409 语义）。
    pub fn delete(&self, _id: &MediaId) -> anyhow::Result<()> {
        todo!("Phase 1：引用保护 + 目录删除")
    }

    /// 原文件磁盘路径（仅供受控 Range 播放端点使用；不进任何 API 响应）。
    pub fn file_path(&self, _id: &MediaId) -> anyhow::Result<PathBuf> {
        todo!("Phase 1：original.<ext> 路径解析")
    }

    /// 精确帧提取为 PNG 字节（服务端解码为唯一精确帧来源；同一请求可重复）。
    pub fn extract_frame_png(&self, _id: &MediaId, _req: &FrameRequest) -> anyhow::Result<Vec<u8>> {
        todo!("Phase 1：FFmpeg 按 PTS/帧索引确定帧提取")
    }

    /// 声明/解除业务引用（写回 metadata.json；删除保护依据）。
    pub fn set_refs(&self, _id: &MediaId, _refs: &[MediaRef]) -> anyhow::Result<()> {
        todo!("Phase 5：引用登记（V1 先留接口）")
    }
}

/// 进程级媒体服务（惰性装配；不加 AppState 字段，避免组合根签名扩散）。
static MEDIA: OnceLock<Arc<MediaService>> = OnceLock::new();

pub fn service(cfg: &Config) -> Arc<MediaService> {
    MEDIA
        .get_or_init(|| {
            let root = cfg.data_dir.join("media");
            Arc::new(MediaService::open(root).expect("media service init"))
        })
        .clone()
}
