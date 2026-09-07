//! Core 媒体库（视频工作台 V1）。
//!
//! 计划：`docs/plans/gamer_video_workbench_development_plan.md` §6/§7；
//! 实施合同（REST 形态/字段/限额/所有权）：`docs/plans/gamer_video_workbench_contracts.md`。
//!
//! Core 只管机制：全局媒体库 `data/media/<media-id>/` 的导入、探测、元数据、
//! 精确帧提取与文件生命周期。原文件不可变；元数据 source of truth =
//! 每个 media 目录下的 `metadata.json`（V1 不落 SQLite，不改 schema 版本）。
//! 模板/YAML/项目等业务语义归插件，本模块不解释。
//!
//! 探测/抽帧执行体来自 `Config.ffmpeg_path`（ffprobe 同目录派生）；工具缺失
//! 或探测失败统一映射为结构化 [`MediaError`]（REST 层 → 415 `media_unsupported`）。
//! 所有外部进程调用都带超时并同步执行——调用方（REST/host facade）负责放入
//! blocking 池（`api::common::run_blocking_api`）。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

/// 结构化媒体错误分类（REST 层映射：404/409/415/400）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaErrorKind {
    NotFound,
    Referenced,
    Unsupported,
    Invalid,
}

/// 结构化媒体错误：kind 供 REST 层映射合同机器码
/// （`media_not_found` / `media_referenced` / `media_unsupported`），
/// message 供日志与 400 文案。经 `anyhow::Error` 透传（签名即合同：
/// 服务方法保持 `anyhow::Result`），调用方 `downcast_ref` 取回。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MediaError {
    kind: MediaErrorKind,
    message: String,
}

impl MediaError {
    pub(crate) fn new(kind: MediaErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn kind(&self) -> MediaErrorKind {
        self.kind
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn not_found(message: impl Into<String>) -> anyhow::Error {
        anyhow::Error::new(Self::new(MediaErrorKind::NotFound, message))
    }

    pub(crate) fn referenced(message: impl Into<String>) -> anyhow::Error {
        anyhow::Error::new(Self::new(MediaErrorKind::Referenced, message))
    }

    pub(crate) fn unsupported(message: impl Into<String>) -> anyhow::Error {
        anyhow::Error::new(Self::new(MediaErrorKind::Unsupported, message))
    }

    pub(crate) fn invalid(message: impl Into<String>) -> anyhow::Error {
        anyhow::Error::new(Self::new(MediaErrorKind::Invalid, message))
    }
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MediaError {}

const METADATA_FILE: &str = "metadata.json";
/// 导入暂存目录前缀（点前缀：list/删除永不可见，media id 语法也拒绝点开头）。
const STAGING_PREFIX: &str = ".import-";
const ORIGINAL_STEM: &str = "original";
/// 展示名与 id 的长度上限（防御性；正常文件名远小于此）。
const MAX_MEDIA_NAME_BYTES: usize = 200;
const MAX_MEDIA_ID_BYTES: usize = 64;
/// 帧提取 max_width 上限（解码内存防御；超过按非法参数拒绝）。
const MAX_FRAME_MAX_WIDTH: u32 = 8192;
/// ffprobe/ffmpeg 单次执行超时（探测大文件 seek 可能偏慢，给足余量）。
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);

/// Core 媒体库服务。进程级单例（[`service`]），组合根零接线：
/// 处理器用 `State<AppState>` 取 `cfg` 后惰性初始化。
pub struct MediaService {
    data_root: PathBuf,
    ffmpeg_path: String,
    /// metadata.json 读写的进程内串行化（写走 tmp+rename 原子替换）。
    io_lock: Mutex<()>,
}

impl MediaService {
    pub fn open(data_root: PathBuf, ffmpeg_path: String) -> anyhow::Result<Self> {
        Ok(Self {
            data_root,
            ffmpeg_path,
            io_lock: Mutex::new(()),
        })
    }

    /// 数据根目录（`data/media`）；供诊断/测试断言，V1 无进程内消费者。
    #[allow(dead_code)]
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    /// 导入字节：临时文件 → ffprobe 探测 → 校验 → 原子提交（导入失败不留下
    /// 可被正常引用的半成品）。同名不去重（各自独立 media id）。
    pub fn import_bytes(&self, name: &str, bytes: &[u8]) -> anyhow::Result<MediaMetadata> {
        let display_name = sanitize_import_name(name)?;
        if bytes.is_empty() {
            return Err(MediaError::invalid("导入内容为空"));
        }
        let id = MediaId(uuid::Uuid::new_v4().simple().to_string());
        // staging 目录先写齐 original + metadata.json，最后一步整体改名提交；
        // 任何一步失败 → 全量清理，不留下半成品。
        let staging = self.data_root.join(format!("{STAGING_PREFIX}{}", id.0));
        fs::create_dir_all(&staging).map_err(|e| {
            anyhow::Error::new(e).context(format!(
                "创建导入暂存目录失败: {}",
                staging.display()
            ))
        })?;
        match self.commit_import(&staging, &id, &display_name, bytes) {
            Ok(meta) => Ok(meta),
            Err(err) => {
                let _ = fs::remove_dir_all(&staging);
                Err(err)
            }
        }
    }

    fn commit_import(
        &self,
        staging: &Path,
        id: &MediaId,
        name: &str,
        bytes: &[u8],
    ) -> anyhow::Result<MediaMetadata> {
        let original = staging.join(original_file_name(name));
        fs::write(&original, bytes)
            .map_err(|e| anyhow::Error::new(e).context("写入导入文件失败"))?;
        let probe = self.probe(&original)?;
        let meta = MediaMetadata {
            id: id.clone(),
            name: name.to_string(),
            sha256: sha256_hex(bytes),
            size: bytes.len() as u64,
            duration_us: probe.duration_us,
            container: probe.container,
            codec: probe.codec,
            width: probe.width,
            height: probe.height,
            rotation: probe.rotation,
            source: MediaSource::Import,
            state: MediaState::Ready,
            created_at: now_rfc3339(),
            refs: Vec::new(),
        };
        let _io = self.io_lock.lock();
        write_metadata(staging, &meta)?;
        // 提交 = 目录原子改名（同卷 rename；目标存在 = id 碰撞，直接失败）
        fs::rename(staging, self.media_dir(id))
            .map_err(|e| anyhow::Error::new(e).context("提交媒体目录失败（id 冲突？）"))?;
        Ok(meta)
    }

    pub fn list(&self) -> anyhow::Result<Vec<MediaMetadata>> {
        let _io = self.io_lock.lock();
        let mut items = Vec::new();
        let entries = match fs::read_dir(&self.data_root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(items),
            Err(e) => {
                return Err(anyhow::Error::new(e).context("读取媒体目录失败"));
            }
        };
        for entry in entries.flatten() {
            let dir_name = entry.file_name();
            let Some(dir_name) = dir_name.to_str() else {
                continue;
            };
            // 暂存目录（点前缀）与非目录条目不进列表
            if dir_name.starts_with('.') || !entry.path().is_dir() {
                continue;
            }
            match read_metadata(&entry.path()) {
                Ok(meta) => items.push(meta),
                Err(e) => {
                    // 半成品/损坏目录不阻断列表（警告可观测），也不可被删除外引用
                    tracing::warn!(media_dir = %dir_name, error = %e, "跳过不可读媒体目录");
                }
            }
        }
        // 合同：创建时间倒序（同秒内按 id 倒序保证稳定顺序）
        items.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.0.cmp(&a.id.0))
        });
        Ok(items)
    }

    pub fn get(&self, id: &MediaId) -> anyhow::Result<MediaMetadata> {
        Self::validate_id(id)?;
        let _io = self.io_lock.lock();
        read_metadata(&self.media_dir(id))
    }

    /// 删除素材目录；仍被引用（refs 非空）→ 明确错误（409 语义）。
    pub fn delete(&self, id: &MediaId) -> anyhow::Result<()> {
        Self::validate_id(id)?;
        let _io = self.io_lock.lock();
        let dir = self.media_dir(id);
        let meta = read_metadata(&dir)?;
        if !meta.refs.is_empty() {
            return Err(MediaError::referenced(format!(
                "素材仍被 {} 项业务引用，拒绝删除",
                meta.refs.len()
            )));
        }
        fs::remove_dir_all(&dir).map_err(|e| {
            anyhow::Error::new(e).context(format!("删除媒体目录失败: {}", dir.display()))
        })?;
        Ok(())
    }

    /// 原文件磁盘路径（仅供受控 Range 播放端点使用；不进任何 API 响应）。
    pub fn file_path(&self, id: &MediaId) -> anyhow::Result<PathBuf> {
        Self::validate_id(id)?;
        let dir = self.media_dir(id);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(MediaError::not_found("媒体不存在"));
            }
            Err(e) => {
                return Err(anyhow::Error::new(e).context("读取媒体目录失败"));
            }
        };
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&format!("{ORIGINAL_STEM}.")))
            })
            .collect();
        candidates.sort();
        candidates.into_iter().next().ok_or_else(|| {
            MediaError::not_found(format!("{} 文件缺失", ORIGINAL_STEM))
        })
    }

    /// 精确帧提取为 PNG 字节（服务端解码为唯一精确帧来源；同一请求可重复）。
    pub fn extract_frame_png(&self, id: &MediaId, req: &FrameRequest) -> anyhow::Result<Vec<u8>> {
        Self::validate_frame_request(req)?;
        self.get(id)?;
        let path = self.file_path(id)?;
        if self.ffmpeg_path.trim().is_empty() {
            return Err(MediaError::unsupported(
                "ffmpeg 未配置（config.toml ffmpeg_path）",
            ));
        }
        self.ffmpeg_extract(&path, req)
    }

    /// 声明/解除业务引用（写回 metadata.json；删除保护依据）。
    pub fn set_refs(&self, id: &MediaId, refs: &[MediaRef]) -> anyhow::Result<()> {
        Self::validate_id(id)?;
        for r in refs {
            crate::resources::validate_scope_id("refs.package_id", &r.package_id)
                .map_err(|e| MediaError::invalid(format!("refs.package_id 非法: {e}")))?;
            crate::resources::validate_scope_id("refs.plugin_id", &r.plugin_id)
                .map_err(|e| MediaError::invalid(format!("refs.plugin_id 非法: {e}")))?;
            let kind = r.kind.trim();
            if kind.is_empty() || kind.len() > 64 || kind.chars().any(char::is_control) {
                return Err(MediaError::invalid(
                    "refs.kind 非法（1..=64 字节且不含控制字符）",
                ));
            }
        }
        let _io = self.io_lock.lock();
        let dir = self.media_dir(id);
        let mut meta = read_metadata(&dir)?;
        meta.refs = refs
            .iter()
            .map(|r| MediaRef {
                package_id: r.package_id.trim().to_string(),
                plugin_id: r.plugin_id.trim().to_string(),
                kind: r.kind.trim().to_string(),
            })
            .collect();
        write_metadata(&dir, &meta)
    }

    // ---------- 内部实现 ----------

    fn media_dir(&self, id: &MediaId) -> PathBuf {
        self.data_root.join(&id.0)
    }

    /// media id 语法校验（路径安全）：非空、长度受限、仅字母数字与 `-_`、
    /// 不以点开头（暂存目录前缀隔离）。
    fn validate_id(id: &MediaId) -> anyhow::Result<()> {
        let v = id.0.as_str();
        let ok = !v.is_empty()
            && v.len() <= MAX_MEDIA_ID_BYTES
            && !v.starts_with('.')
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
        if ok {
            Ok(())
        } else {
            Err(MediaError::invalid(format!("media id 非法: {v:?}")))
        }
    }

    fn validate_frame_request(req: &FrameRequest) -> anyhow::Result<()> {
        if req.pts_us.is_none() && req.index.is_none() {
            return Err(MediaError::invalid("pts_us 与 index 必须给其一"));
        }
        if let Some(max_width) = req.max_width {
            if max_width == 0 || max_width > MAX_FRAME_MAX_WIDTH {
                return Err(MediaError::invalid(format!(
                    "max_width 非法（1..={MAX_FRAME_MAX_WIDTH}）"
                )));
            }
        }
        Ok(())
    }

    /// ffprobe 可执行文件路径（ffmpeg 同目录派生，见 [`derive_ffprobe_path`]）。
    fn ffprobe_exec(&self) -> String {
        derive_ffprobe_path(&self.ffmpeg_path)
    }

    /// ffprobe 探测原文件（`-print_format json -show_format -show_streams`）。
    /// 工具不可用/输出不可解析/无视频轨 → 统一 Unsupported（合同 415 语义）。
    fn probe(&self, file: &Path) -> anyhow::Result<ProbeOutcome> {
        if self.ffmpeg_path.trim().is_empty() {
            return Err(MediaError::unsupported(
                "ffmpeg 未配置（config.toml ffmpeg_path）",
            ));
        }
        let ffprobe = self.ffprobe_exec();
        let mut cmd = Command::new(&ffprobe);
        cmd.args(["-v", "quiet", "-print_format", "json", "-show_format", "-show_streams"])
            .arg(file);
        let output = match run_with_timeout(&ffprobe, &mut cmd, TOOL_TIMEOUT) {
            Ok(output) => output,
            Err(err) => {
                return Err(MediaError::unsupported(format!(
                    "ffprobe 不可用（{ffprobe}）: {err:#}"
                )));
            }
        };
        if !output.status.success() {
            let tail = stderr_tail(&output.stderr);
            return Err(MediaError::unsupported(format!(
                "ffprobe 无法识别该文件: {tail}"
            )));
        }
        let parsed: ProbeJson = serde_json::from_slice(&output.stdout).map_err(|e| {
            MediaError::unsupported(format!("ffprobe 输出解析失败: {e}"))
        })?;
        let stream = parsed
            .streams
            .iter()
            .find(|s| is_video_stream(s))
            .ok_or_else(|| MediaError::unsupported("未找到视频轨（非视频或纯音频文件）"))?;
        let (Some(width), Some(height)) = (stream.width, stream.height) else {
            return Err(MediaError::unsupported("视频轨缺少分辨率信息"));
        };
        if width == 0 || height == 0 {
            return Err(MediaError::unsupported("视频轨分辨率为 0"));
        }
        let rotation = normalize_rotation_deg(
            stream
                .side_data_list
                .iter()
                .find_map(|d| d.rotation),
            stream.tags.as_ref().and_then(|t| t.rotate.as_deref()),
        );
        // 展示语义：metadata 宽高 = 旋转后（oriented）尺寸，与抽帧 PNG
        // （ffmpeg 默认 autorotate）像素空间一致；rotation 只作展示元数据。
        let (oriented_w, oriented_h) = if rotation == 90 || rotation == 270 {
            (height, width)
        } else {
            (width, height)
        };
        let duration_us = parsed
            .format
            .as_ref()
            .and_then(|f| f.duration.as_deref())
            .or(stream.duration.as_deref())
            .and_then(parse_duration_us);
        Ok(ProbeOutcome {
            container: derive_container(
                parsed.format.as_ref().and_then(|f| f.format_name.as_deref()),
            ),
            codec: stream
                .codec_name
                .as_deref()
                .map(str::to_ascii_lowercase)
                .unwrap_or_else(|| "unknown".into()),
            width: oriented_w,
            height: oriented_h,
            rotation,
            duration_us,
        })
    }

    /// ffmpeg 抽帧：pts 路径 = 输入端 `-ss`（默认 accurate seek：自目标前一个
    /// 关键帧解码、丢弃至目标时刻），输出首帧即 pts ≥ 目标的首个展示帧；
    /// index 路径 = `select=eq(n,idx)` 逐帧直通。同一输入同一参数 → 同一帧 →
    /// 同一 PNG 字节（ffmpeg 单帧 PNG 编码确定性）。
    fn ffmpeg_extract(&self, path: &Path, req: &FrameRequest) -> anyhow::Result<Vec<u8>> {
        let mut cmd = Command::new(&self.ffmpeg_path);
        cmd.args(["-nostdin", "-v", "error"]);
        if let Some(pts_us) = req.pts_us {
            let secs = format!("{}", pts_us as f64 / 1_000_000.0);
            cmd.args(["-ss", &secs]);
        }
        cmd.arg("-i").arg(path);
        let mut filters: Vec<String> = Vec::new();
        if req.pts_us.is_none() {
            if let Some(index) = req.index {
                filters.push(format!("select=eq(n\\,{index})"));
            }
        }
        if let Some(max_width) = req.max_width {
            // 等比缩放上限：宽 ≤ max，高自动（偶数对齐）；逗号经反斜杠转义
            filters.push(format!("scale=w=min(iw\\,{max_width}):h=-2"));
        }
        if !filters.is_empty() {
            cmd.arg("-vf").arg(filters.join(","));
        }
        // 不用 -vsync（ffmpeg 8/9 已移除；本仓现有 ffmpeg 调用先例同样不用）：
        // select 直通 + `-frames:v 1` 已保证恰好一个输出帧
        cmd.args(["-frames:v", "1", "-f", "image2pipe", "-vcodec", "png", "pipe:1"]);
        let output = run_with_timeout("ffmpeg", &mut cmd, TOOL_TIMEOUT)
            .map_err(|e| anyhow::anyhow!("ffmpeg 抽帧失败: {e:#}"))?;
        if !output.status.success() || output.stdout.is_empty() {
            // 帧越界（pts/index 超出媒体范围）与解码失败同为 400 语义：
            // 参数未落在媒体可表达范围
            return Err(MediaError::invalid(format!(
                "帧提取失败（pts_us={:?} index={:?}）: {}",
                req.pts_us,
                req.index,
                stderr_tail(&output.stderr)
            )));
        }
        Ok(output.stdout)
    }
}

/// ffprobe 探测产物（oriented 尺寸口径，见 `probe`）。
struct ProbeOutcome {
    container: String,
    codec: String,
    width: u32,
    height: u32,
    rotation: u16,
    duration_us: Option<u64>,
}

/// 进程级媒体服务（惰性装配；不加 AppState 字段，避免组合根签名扩散）。
static MEDIA: OnceLock<Arc<MediaService>> = OnceLock::new();

pub fn service(cfg: &Config) -> Arc<MediaService> {
    MEDIA
        .get_or_init(|| {
            let root = cfg.data_dir.join("media");
            Arc::new(MediaService::open(root, cfg.ffmpeg_path.clone()).expect("media service init"))
        })
        .clone()
}

// ---------- 纯函数助手（单测覆盖） ----------

/// metadata.json 原子写：tmp + rename（同卷原子替换，读侧要么旧要么新）。
fn write_metadata(dir: &Path, meta: &MediaMetadata) -> anyhow::Result<()> {
    let path = dir.join(METADATA_FILE);
    let tmp = dir.join(format!("{METADATA_FILE}.tmp"));
    fs::write(&tmp, serde_json::to_vec_pretty(meta)?)
        .map_err(|e| anyhow::Error::new(e).context("写入 metadata.json 失败"))?;
    fs::rename(&tmp, &path)
        .map_err(|e| anyhow::Error::new(e).context("替换 metadata.json 失败"))?;
    Ok(())
}

fn read_metadata(dir: &Path) -> anyhow::Result<MediaMetadata> {
    let path = dir.join(METADATA_FILE);
    let bytes = fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => MediaError::not_found("媒体不存在"),
        _ => anyhow::Error::new(e).context("读取 metadata.json 失败"),
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("metadata.json 损坏（{}）: {e}", dir.display()))
}

/// 导入文件名规整：取最后一段（容忍客户端塞入完整路径）、拒绝空/`.`/`..`/
/// 控制字符与超长。展示名不做字符白名单（各 OS 文件名习惯差异大），
/// 落盘文件名只用其扩展名（`original.<ext>`，扩展名再过滤为字母数字）。
fn sanitize_import_name(raw: &str) -> anyhow::Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(MediaError::invalid("name 必填（导入文件名）"));
    }
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    if name.is_empty() || name == "." || name == ".." {
        return Err(MediaError::invalid("name 非法（不能是路径段 `.`/`..`）"));
    }
    if name.len() > MAX_MEDIA_NAME_BYTES {
        return Err(MediaError::invalid(format!(
            "name 超过 {MAX_MEDIA_NAME_BYTES} 字节"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(MediaError::invalid("name 含非法控制字符"));
    }
    Ok(name.to_string())
}

/// 落盘原文件名：`original.<ext>`（扩展名取自导入名、仅字母数字、≤16 字节；
/// 无扩展名 → `original.bin`）。
fn original_file_name(name: &str) -> String {
    let ext: String = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(16)
        .collect();
    if ext.is_empty() {
        format!("{ORIGINAL_STEM}.bin")
    } else {
        format!("{ORIGINAL_STEM}.{ext}")
    }
}

/// ffprobe 与 ffmpeg 同目录派生：`ffmpeg` → `ffprobe`、`ffmpeg.exe` →
/// `ffprobe.exe`（含目录成分时）；裸命令名走 PATH 查找；名称不含 ffmpeg
/// 前缀时退回同目录 `ffprobe`。
fn derive_ffprobe_path(ffmpeg: &str) -> String {
    let path = Path::new(ffmpeg);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if let Some(suffix) = name.to_ascii_lowercase().strip_prefix("ffmpeg") {
        let rest = &name[name.len() - suffix.len()..];
        return path
            .with_file_name(format!("ffprobe{rest}"))
            .to_string_lossy()
            .into_owned();
    }
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            parent.join("ffprobe").to_string_lossy().into_owned()
        }
        _ => "ffprobe".to_string(),
    }
}

/// ffprobe JSON 形态（仅取需要字段；未知字段忽略，缺字段全部 Option 兜底）。
#[derive(Debug, Deserialize)]
struct ProbeJson {
    #[serde(default)]
    format: Option<ProbeFormat>,
    #[serde(default)]
    streams: Vec<ProbeStream>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    #[serde(default, rename = "format_name")]
    format_name: Option<String>,
    #[serde(default)]
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    #[serde(default, rename = "codec_type")]
    codec_type: Option<String>,
    #[serde(default, rename = "codec_name")]
    codec_name: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    disposition: ProbeDisposition,
    #[serde(default, rename = "side_data_list")]
    side_data_list: Vec<ProbeSideData>,
    #[serde(default)]
    tags: Option<ProbeTags>,
}

#[derive(Debug, Default, Deserialize)]
struct ProbeDisposition {
    #[serde(default, rename = "attached_pic")]
    attached_pic: i64,
}

#[derive(Debug, Deserialize)]
struct ProbeSideData {
    #[serde(default, rename = "side_data_type")]
    #[allow(dead_code)]
    side_data_type: Option<String>,
    #[serde(default)]
    rotation: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ProbeTags {
    #[serde(default)]
    rotate: Option<String>,
}

/// 视频轨判定：codec_type=video 且非封面图（attached_pic）。
fn is_video_stream(stream: &ProbeStream) -> bool {
    stream.codec_type.as_deref() == Some("video") && stream.disposition.attached_pic == 0
}

/// 旋转角归一：side_data_list（Display Matrix）优先、legacy tags.rotate 兜底；
/// 折返到 0/90/180/270（负角与任意角都吸附到 90 的倍数）。
fn normalize_rotation_deg(side_data: Option<i64>, tag: Option<&str>) -> u16 {
    let raw = side_data.or_else(|| tag.and_then(|t| t.trim().parse::<i64>().ok()));
    let deg = raw.unwrap_or(0);
    let deg = ((deg % 360) + 360) % 360;
    ((deg + 45) / 90 * 90) as u16 % 360
}

/// 容器名：`format_name` 常是逗号串（如 `mov,mp4,m4a,...`）；含 `mp4` 优先出
/// `mp4`，否则取首段小写；无 format_name 时空串（wire 允许，元数据尽力而为）。
fn derive_container(format_name: Option<&str>) -> String {
    let raw = format_name.unwrap_or("").to_ascii_lowercase();
    if raw.is_empty() {
        return String::new();
    }
    if raw.split(',').any(|t| t.trim() == "mp4") {
        return "mp4".into();
    }
    raw.split(',').next().unwrap_or("").trim().to_string()
}

/// duration（秒，字符串）→ 微秒（非负、四舍五入；解析失败 = None）。
fn parse_duration_us(seconds: &str) -> Option<u64> {
    let seconds: f64 = seconds.trim().parse().ok()?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    Some((seconds * 1_000_000.0).round() as u64)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// stderr 尾部（日志/错误信息用；截到最后 ≤300 字节）。
fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr).trim().to_string();
    if text.len() > 300 {
        format!("...{}", &text[text.len() - 300..])
    } else {
        text
    }
}

/// 同步执行外部工具 + 超时击杀：stdout/stderr 由读线程排空（避免管道写满
/// 死锁），主循环 `try_wait` 轮询至退出或超时（超时 → kill + 收割）。
/// 仅限 blocking 池内调用（REST handler 经 `run_blocking_api`）。
fn run_with_timeout(
    program: &str,
    cmd: &mut Command,
    timeout: Duration,
) -> anyhow::Result<Output> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("启动 {program} 失败: {e}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("{program} 无 stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("{program} 无 stderr"))?;
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("{program} 执行超时（>{timeout:?}）");
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let stdout_buf = stdout_reader
        .join()
        .map_err(|_| anyhow::anyhow!("{program} stdout 读线程异常退出"))?;
    let stderr_buf = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("{program} stderr 读线程异常退出"))?;
    Ok(Output {
        status,
        stdout: stdout_buf,
        stderr: stderr_buf,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_service(root: &Path) -> MediaService {
        MediaService::open(root.to_path_buf(), "gamer-no-such-ffmpeg".into()).unwrap()
    }

    fn meta_fixture(id: &str, created_at: &str, refs: Vec<MediaRef>) -> MediaMetadata {
        MediaMetadata {
            id: MediaId(id.into()),
            name: "clip.mp4".into(),
            sha256: "ab".into(),
            size: 1,
            duration_us: Some(1_000_000),
            container: "mp4".into(),
            codec: "h264".into(),
            width: 320,
            height: 240,
            rotation: 0,
            source: MediaSource::Import,
            state: MediaState::Ready,
            created_at: created_at.into(),
            refs,
        }
    }

    // ---------- metadata.json 读写 ----------

    #[test]
    fn metadata_json_roundtrip_and_missing_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("abc")).unwrap();
        let meta = meta_fixture("abc", "2026-09-07T00:00:00.000000Z", vec![]);
        write_metadata(&root.join("abc"), &meta).unwrap();
        let loaded = read_metadata(&root.join("abc")).unwrap();
        assert_eq!(loaded, meta);
        // tmp 文件不残留
        assert!(!root.join("abc").join("metadata.json.tmp").exists());

        match read_metadata(&root.join("ghost")) {
            Err(e) => {
                let me = e.downcast_ref::<MediaError>().unwrap();
                assert_eq!(me.kind(), MediaErrorKind::NotFound);
            }
            Ok(_) => panic!("missing media must fail"),
        }
    }

    // ---------- 导入失败清理 + 415 语义 ----------

    /// ffmpeg 缺失（探测失败）→ 结构化 Unsupported（合同 415），且暂存目录
    /// 全量清理：不留下可被引用的半成品。
    #[test]
    fn import_failure_cleans_staging_and_maps_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let svc = test_service(dir.path());
        let err = svc.import_bytes("clip.mp4", b"not-a-video").unwrap_err();
        let me = err.downcast_ref::<MediaError>().unwrap();
        assert_eq!(me.kind(), MediaErrorKind::Unsupported);
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(leftovers.is_empty(), "staging leftover: {leftovers:?}");
        // 空字节同样拒绝（Invalid），且同样不落盘
        let err = svc.import_bytes("clip.mp4", b"").unwrap_err();
        assert_eq!(
            err.downcast_ref::<MediaError>().unwrap().kind(),
            MediaErrorKind::Invalid
        );
        assert!(fs::read_dir(dir.path()).unwrap().flatten().count() == 0);
    }

    // ---------- 列表 / 引用保护删除 ----------

    fn seed_media_dir(root: &Path, id: &str, created_at: &str, refs: Vec<MediaRef>) {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("original.mp4"), b"x").unwrap();
        write_metadata(&dir, &meta_fixture(id, created_at, refs)).unwrap();
    }

    #[test]
    fn list_orders_desc_and_skips_staging_and_corrupt_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_media_dir(root, "aaa1", "2026-09-07T01:00:00.000000Z", vec![]);
        seed_media_dir(root, "aaa2", "2026-09-07T02:00:00.000000Z", vec![]);
        // 暂存目录与损坏目录（无 metadata.json）都不可见
        fs::create_dir_all(root.join(".import-zzz")).unwrap();
        fs::create_dir_all(root.join("broken")).unwrap();
        fs::write(root.join("loose-file"), b"x").unwrap();

        let svc = test_service(root);
        let items = svc.list().unwrap();
        let ids: Vec<_> = items.iter().map(|m| m.id.0.as_str()).collect();
        assert_eq!(ids, ["aaa2", "aaa1"], "创建时间倒序");
    }

    #[test]
    fn delete_is_protected_by_refs_then_set_refs_releases() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let id = MediaId("aaa1".into());
        seed_media_dir(
            root,
            "aaa1",
            "2026-09-07T01:00:00.000000Z",
            vec![MediaRef {
                package_id: "com.test.game".into(),
                plugin_id: "gamer.yaml".into(),
                kind: "project".into(),
            }],
        );
        let svc = test_service(root);
        // 引用保护：非空 refs 拒绝删除（409 语义）
        let err = svc.delete(&id).unwrap_err();
        assert_eq!(
            err.downcast_ref::<MediaError>().unwrap().kind(),
            MediaErrorKind::Referenced
        );
        assert!(root.join("aaa1").exists(), "被引用素材必须保留");

        // 解除引用 → 删除成功且不可再 get
        svc.set_refs(&id, &[]).unwrap();
        assert_eq!(svc.get(&id).unwrap().refs.len(), 0);
        svc.delete(&id).unwrap();
        assert!(!root.join("aaa1").exists());
        assert_eq!(
            svc.get(&id)
                .unwrap_err()
                .downcast_ref::<MediaError>()
                .unwrap()
                .kind(),
            MediaErrorKind::NotFound
        );
    }

    #[test]
    fn set_refs_validates_scope_ids_and_kind() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_media_dir(root, "aaa1", "2026-09-07T01:00:00.000000Z", vec![]);
        let svc = test_service(root);
        let id = MediaId("aaa1".into());
        for bad in [
            MediaRef {
                package_id: "Bad Package".into(),
                plugin_id: "gamer.yaml".into(),
                kind: "project".into(),
            },
            MediaRef {
                package_id: "pkg".into(),
                plugin_id: "../evil".into(),
                kind: "project".into(),
            },
            MediaRef {
                package_id: "pkg".into(),
                plugin_id: "gamer.yaml".into(),
                kind: String::new(),
            },
        ] {
            let err = svc.set_refs(&id, &[bad]).unwrap_err();
            assert_eq!(
                err.downcast_ref::<MediaError>().unwrap().kind(),
                MediaErrorKind::Invalid
            );
        }
        // 不存在的素材 set_refs → NotFound
        let ghost = MediaId("zzz9".into());
        assert_eq!(
            svc.set_refs(&ghost, &[])
                .unwrap_err()
                .downcast_ref::<MediaError>()
                .unwrap()
                .kind(),
            MediaErrorKind::NotFound
        );
    }

    // ---------- file_path / id 校验 ----------

    #[test]
    fn file_path_resolves_original_and_missing_media() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_media_dir(root, "aaa1", "2026-09-07T01:00:00.000000Z", vec![]);
        let svc = test_service(root);
        let path = svc.file_path(&MediaId("aaa1".into())).unwrap();
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("original.mp4")
        );
        // 绝不泄漏绝对路径之外的目录成分（文件名限定 original.*）
        assert!(path.starts_with(root));
        let err = svc
            .file_path(&MediaId("ghost".into()))
            .unwrap_err();
        assert_eq!(
            err.downcast_ref::<MediaError>().unwrap().kind(),
            MediaErrorKind::NotFound
        );
    }

    #[test]
    fn validate_id_rejects_traversal_and_empty() {
        for bad in ["", "../x", "a/b", "a\\b", ".hidden", "a b", &"x".repeat(65)] {
            assert!(
                MediaService::validate_id(&MediaId(bad.into())).is_err(),
                "{bad:?} must be rejected"
            );
        }
        assert!(MediaService::validate_id(&MediaId("0123456789abcdef".into())).is_ok());
        assert!(MediaService::validate_id(&MediaId("clip-01_x".into())).is_ok());
    }

    // ---------- FrameRequest 校验 ----------

    #[test]
    fn frame_request_requires_selector_and_bounds_max_width() {
        assert!(MediaService::validate_frame_request(&FrameRequest::default()).is_err());
        assert!(MediaService::validate_frame_request(&FrameRequest {
            pts_us: Some(0),
            index: Some(3),
            max_width: Some(640),
        })
        .is_ok());
        for bad_max in [Some(0), Some(MAX_FRAME_MAX_WIDTH + 1)] {
            assert!(MediaService::validate_frame_request(&FrameRequest {
                pts_us: Some(0),
                index: None,
                max_width: bad_max,
            })
            .is_err());
        }
    }

    // ---------- 命名 / ffprobe 派生 / 旋转归一 / 容器与时长 ----------

    #[test]
    fn import_name_sanitizes_paths_and_controls() {
        assert_eq!(sanitize_import_name("clip.mp4").unwrap(), "clip.mp4");
        assert_eq!(
            sanitize_import_name("C:\\Users\\me\\游戏 录像.mp4").unwrap(),
            "游戏 录像.mp4"
        );
        assert_eq!(sanitize_import_name("a/b/clip.mp4").unwrap(), "clip.mp4");
        for bad in ["", "   ", "..", ".", "dir/", "a\tb"] {
            assert!(sanitize_import_name(bad).is_err(), "{bad:?} must fail");
        }
    }

    #[test]
    fn original_file_name_keeps_alnum_extension_only() {
        assert_eq!(original_file_name("clip.mp4"), "original.mp4");
        assert_eq!(original_file_name("clip.MOV"), "original.MOV");
        assert_eq!(original_file_name("no-ext"), "original.bin");
        assert_eq!(original_file_name("evil.../../x"), "original.bin");
        assert_eq!(original_file_name("a.verylongext"), "original.verylongext");
    }

    #[test]
    fn ffprobe_path_derives_from_ffmpeg() {
        assert_eq!(derive_ffprobe_path("ffmpeg"), "ffprobe");
        // Path 组件比较（Windows 分隔符差异不影响断言）
        assert_eq!(
            PathBuf::from(derive_ffprobe_path("C:\\tools\\ffmpeg.exe")),
            Path::new("C:\\tools").join("ffprobe.exe")
        );
        assert_eq!(
            PathBuf::from(derive_ffprobe_path("/usr/local/bin/ffmpeg")),
            Path::new("/usr/local/bin").join("ffprobe")
        );
        // 非 ffmpeg 前缀 → 同目录 ffprobe
        assert_eq!(
            PathBuf::from(derive_ffprobe_path("C:\\tools\\avconv.exe")),
            Path::new("C:\\tools").join("ffprobe")
        );
    }

    #[test]
    fn rotation_normalizes_side_data_and_tags() {
        assert_eq!(normalize_rotation_deg(None, None), 0);
        assert_eq!(normalize_rotation_deg(Some(-90), None), 270);
        assert_eq!(normalize_rotation_deg(Some(180), None), 180);
        assert_eq!(normalize_rotation_deg(None, Some("90")), 90);
        assert_eq!(normalize_rotation_deg(Some(45), None), 90);
        assert_eq!(normalize_rotation_deg(Some(360 + 270), None), 270);
        // 非法 tag 被忽略（= 0）
        assert_eq!(normalize_rotation_deg(None, Some("abc")), 0);
    }

    #[test]
    fn container_prefers_mp4_and_first_token() {
        assert_eq!(derive_container(Some("mov,mp4,m4a,3gp,3g2,mj2")), "mp4");
        assert_eq!(derive_container(Some("matroska,webm")), "matroska");
        assert_eq!(derive_container(Some("webm")), "webm");
        assert_eq!(derive_container(Some("avi")), "avi");
        assert_eq!(derive_container(None), "");
    }

    #[test]
    fn duration_us_parses_positive_seconds_only() {
        assert_eq!(parse_duration_us("1.5"), Some(1_500_000));
        assert_eq!(parse_duration_us("0.04"), Some(40_000));
        assert_eq!(parse_duration_us("0"), None);
        assert_eq!(parse_duration_us("-1"), None);
        assert_eq!(parse_duration_us("abc"), None);
        assert_eq!(parse_duration_us("N/A"), None);
    }

    // ---------- 真 ffmpeg/ffprobe 集成（本机装了 ffmpeg 才有意义） ----------

    /// 需要 ffmpeg + ffprobe：合成 1s/10fps/320x240 的 H.264 MP4 → 导入探测 →
    /// pts/index 抽帧 → 可重复性与 max_width 缩放。CI 无 ffmpeg 时 `#[ignore]`。
    #[test]
    #[ignore = "需要真实 ffmpeg/ffprobe（本机验证一次抽帧与探测）"]
    fn import_probe_and_frame_extraction_with_real_ffmpeg() {
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("clip.mp4");
        let gen = std::process::Command::new("ffmpeg")
            .args([
                "-y", "-v", "error", "-f", "lavfi",
                "-i", "testsrc=duration=1:size=320x240:rate=10",
                "-c:v", "libx264", "-pix_fmt", "yuv420p",
            ])
            .arg(&clip)
            .output()
            .expect("生成测试视频失败（本机需 ffmpeg）");
        assert!(gen.status.success(), "{}", String::from_utf8_lossy(&gen.stderr));
        let bytes = fs::read(&clip).unwrap();

        let root = tempfile::tempdir().unwrap();
        let svc = MediaService::open(root.path().to_path_buf(), "ffmpeg".into()).unwrap();
        let meta = svc.import_bytes("clip.mp4", &bytes).unwrap();
        assert_eq!(meta.state, MediaState::Ready);
        assert_eq!(meta.width, 320);
        assert_eq!(meta.height, 240);
        assert_eq!(meta.rotation, 0);
        assert_eq!(meta.container, "mp4");
        assert_eq!(meta.codec, "h264");
        assert!(meta.duration_us.unwrap() > 900_000 && meta.duration_us.unwrap() <= 1_100_000);
        assert_eq!(meta.sha256.len(), 64);

        // pts 首帧 + 可重复（同一请求逐字节一致）
        let first = svc
            .extract_frame_png(&meta.id, &FrameRequest { pts_us: Some(0), index: None, max_width: None })
            .unwrap();
        assert_eq!(&first[..8], b"\x89PNG\r\n\x1a\n");
        let again = svc
            .extract_frame_png(&meta.id, &FrameRequest { pts_us: Some(0), index: None, max_width: None })
            .unwrap();
        assert_eq!(first, again, "同一请求必须逐字节可重复");

        // 帧索引路径
        let by_index = svc
            .extract_frame_png(&meta.id, &FrameRequest { pts_us: None, index: Some(5), max_width: None })
            .unwrap();
        assert_eq!(&by_index[..8], b"\x89PNG\r\n\x1a\n");

        // max_width 等比缩放
        let scaled = svc
            .extract_frame_png(&meta.id, &FrameRequest { pts_us: Some(0), index: None, max_width: Some(100) })
            .unwrap();
        let image = image::load_from_memory(&scaled).unwrap();
        assert!(image.width() <= 100 && image.height() < 240, "scaled {}x{}", image.width(), image.height());

        // 帧越界 → Invalid（400 语义）
        assert_eq!(
            svc.extract_frame_png(&meta.id, &FrameRequest { pts_us: Some(60_000_000), index: None, max_width: None })
                .unwrap_err()
                .downcast_ref::<MediaError>()
                .unwrap()
                .kind(),
            MediaErrorKind::Invalid
        );

        // 删除后原目录消失
        svc.delete(&meta.id).unwrap();
        assert!(!root.path().join(&meta.id.0).exists());
    }

    /// 非 media 字节导入 → 415 语义（需要 ffmpeg/ffprobe 真执行才能到达
    /// 「无法识别」分支；无 ffmpeg 时同样落 Unsupported，但那由上一条
    /// 纯逻辑测试覆盖，此条验证真探测路径）。
    #[test]
    #[ignore = "需要真实 ffmpeg/ffprobe"]
    fn probe_rejects_non_media_bytes_as_unsupported() {
        let root = tempfile::tempdir().unwrap();
        let svc = MediaService::open(root.path().to_path_buf(), "ffmpeg".into()).unwrap();
        let err = svc.import_bytes("readme.txt", b"hello world, not a video").unwrap_err();
        assert_eq!(
            err.downcast_ref::<MediaError>().unwrap().kind(),
            MediaErrorKind::Unsupported
        );
    }
}
