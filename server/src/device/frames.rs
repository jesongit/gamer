//! 帧缓存（帧环 + 按需解码）：
//! 缓存 scrcpy 的 H.264 原始帧（SPS/PPS 配置帧 + 自最近关键帧起的完整 GOP），
//! 用途：
//!   1. 截图/模板匹配 —— 每次请求用**临时 ffmpeg** 把最新一帧解码成 PNG 返回。
//!      每次都是全新解码，天然实时：返回的图就是服务器当前收到的最新画面，
//!      不存在常驻解码管线的停滞/陈旧问题（旧设计：常驻 ffmpeg 软解 PNG 流，
//!      一旦管线静默冻结，截图/匹配会永远拿到旧画面，需要代数/新鲜度/健康检查
//!      一堆补丁兜底——按需解码把这些判断全部消除）。
//!   2. WebRTC 新 viewer 初始重放（SPS/PPS + 最近 GOP，见 initial_frames）。

use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, warn};

use super::scrcpy::VideoFrame;

const PNG_SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// 从 PNG 头（IHDR）取宽高：8 字节签名 + 4 长度 + 4 "IHDR" → width@16 / height@20（大端）
fn png_dims(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[..8] != &PNG_SIG {
        return None;
    }
    let w = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(png[20..24].try_into().ok()?);
    Some((w, h))
}

/// GOP 缓存上限（帧数与字节数，超限丢弃整个 GOP 等下一个 IDR 重建）。
/// 注意：MTK 编码器实测忽略 i-frame-interval=2，关键帧实际间隔 ~20-25s
/// （@30fps/20Mbps ≈ 750 帧 / 60MB）——上限必须覆盖一个完整 IDR 周期，
/// 否则新 viewer 重放拿不到含 IDR 的完整 GOP（旧值 400 帧/8MB 在 IDR 后
/// ~3s 就清空），连接只能靠 reset_video 兜底，兜底失败就裸推 P 帧 → 花屏
const GOP_MAX_FRAMES: usize = 800;
const GOP_MAX_BYTES: usize = 64 * 1024 * 1024;

/// 按需解码超时（spawn + 解码 + PNG 编码的预算）
const DECODE_TIMEOUT: Duration = Duration::from_secs(3);
/// 等待首个可解码帧（GOP 非空）的最长时间：会话刚建立时首个 IDR 通常 ≤1s 到达
const WAIT_FIRST_GOP: Duration = Duration::from_millis(1500);

pub struct FrameCache {
    ffmpeg_path: Mutex<String>,
    /// 最近 SPS/PPS 配置帧（解码任何 GOP 前必须先喂它）
    config_buf: Mutex<Vec<u8>>,
    /// 最近完整 GOP（自最近 IDR 起，含 IDR 与后续 P 帧；新 IDR 清空重建）
    gop: Mutex<Vec<VideoFrame>>,
    /// 帧尺寸（最近一次成功解码的 PNG 尺寸，供设备信息展示）
    width: RwLock<u32>,
    height: RwLock<u32>,
}

impl FrameCache {
    pub fn start(ffmpeg_path: &str) -> Arc<Self> {
        Arc::new(Self {
            ffmpeg_path: Mutex::new(ffmpeg_path.to_string()),
            config_buf: Mutex::new(Vec::new()),
            gop: Mutex::new(Vec::new()),
            width: RwLock::new(0),
            height: RwLock::new(0),
        })
    }

    /// 喂入一帧：仅更新 SPS/PPS 配置帧与 GOP 环（无任何解码工作）。
    /// 注意：本方法由视频消费任务调用，必须保持轻量（只做内存拷贝）。
    pub fn feed(&self, frame: &VideoFrame) {
        if frame.is_config {
            let mut cb = self.config_buf.lock();
            if cb.is_empty() || *cb != frame.data {
                // 参数集变化（分辨率/编码参数切换，如游戏切横竖屏、编码器重启）：
                // 旧 GOP 与新参数不匹配，清空等新 IDR 重建——否则 WebRTC 初始重放
                // 与按需解码会把"新 SPS/PPS + 旧 GOP"喂给解码器 → 花屏/解码失败
                // （repeat-previous-headers 会周期性重发相同参数集，字节相同则不清）
                if !cb.is_empty() {
                    self.gop.lock().clear();
                    debug!("SPS/PPS changed ({}B → {}B), GOP cleared", cb.len(), frame.data.len());
                }
                *cb = frame.data.clone();
            }
            return;
        }
        // GOP 缓存维护：IDR 清空重建；P 帧追加；超限丢弃整个 GOP（等待下一个 IDR）
        let mut gop = self.gop.lock();
        if frame.is_keyframe {
            gop.clear();
            gop.push(frame.clone());
        } else if !gop.is_empty() {
            gop.push(frame.clone());
            let total: usize = gop.iter().map(|f| f.data.len()).sum();
            if gop.len() > GOP_MAX_FRAMES || total > GOP_MAX_BYTES {
                gop.clear();
            }
        }
    }

    /// WebRTC 新 viewer 的初始推流帧：SPS/PPS 配置帧 + 最近完整 GOP（自最近 IDR 起）。
    /// pusher 先重放这些帧，浏览器无需等待下一个 IDR 即可开始解码（静态画面下可能长时间无 IDR）。
    /// 返回 None 表示缓存里还没有可用帧（会话刚建立时）。
    pub fn initial_frames(&self) -> Option<Vec<VideoFrame>> {
        let mut frames = Vec::new();
        {
            let c = self.config_buf.lock();
            if !c.is_empty() {
                frames.push(VideoFrame {
                    data: c.clone(),
                    pts_us: 0,
                    is_config: true,
                    is_keyframe: false,
                    annex_b: true,
                });
            }
        }
        {
            let gop = self.gop.lock();
            if !gop.is_empty() {
                frames.extend(gop.iter().cloned());
            }
        }
        if frames.is_empty() {
            None
        } else {
            Some(frames)
        }
    }

    pub fn dims(&self) -> (u32, u32) {
        (*self.width.read(), *self.height.read())
    }

    fn snapshot(&self) -> (Vec<u8>, Vec<VideoFrame>) {
        (self.config_buf.lock().clone(), self.gop.lock().clone())
    }

    /// 按需解码最新帧为 PNG（供截图/模板匹配）：
    /// 每次调用都从"当前缓存的最近 GOP"全新解码，返回的就是服务器此刻收到的最新画面，
    /// 天然实时、无陈旧。无帧（会话刚建立/刚清空）时等首个 IDR ≤1.5s；
    /// 解码失败（如分辨率切换窗口 config 与 GOP 不匹配）→ 清空 GOP 等新 IDR 重试一次。
    /// Ok(None) = 等待超时仍无关键帧（会话刚建立，稍后再试）。
    pub async fn decode_latest_png(&self) -> anyhow::Result<Option<Vec<u8>>> {
        let ffmpeg = self.ffmpeg_path.lock().clone();
        let Some((config, gop)) = self.await_gop().await else {
            return Ok(None);
        };
        match self.decode_once(&ffmpeg, &config, &gop).await {
            Ok(png) => Ok(Some(png)),
            Err(e) => {
                warn!("按需解码失败: {}；清空 GOP 等下一个关键帧重试", e);
                self.gop.lock().clear();
                match self.await_gop().await {
                    Some((config2, gop2)) => self
                        .decode_once(&ffmpeg, &config2, &gop2)
                        .await
                        .map(Some)
                        .map_err(|e2| anyhow::anyhow!("按需解码失败（重试后仍失败）: {}", e2)),
                    None => Err(anyhow::anyhow!("按需解码失败且等待新关键帧超时: {}", e)),
                }
            }
        }
    }

    /// 等待 GOP 非空（首个 IDR 未到或解码失败后刚清空重建），超时返回 None
    async fn await_gop(&self) -> Option<(Vec<u8>, Vec<VideoFrame>)> {
        let deadline = tokio::time::Instant::now() + WAIT_FIRST_GOP;
        loop {
            let (c, g) = self.snapshot();
            if !g.is_empty() {
                return Some((c, g));
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// 用临时 ffmpeg 解码 config+GOP，输出 GOP 最后一帧的 PNG。
    /// select=gte(n\,N)（N = GOP 帧数-1）：demuxer 会把配置帧也算进帧索引，
    /// gte 容忍 ±1~2 帧偏移，最多取到倒数第 3 帧（~100ms 旧），仍是实时画面。
    async fn decode_once(&self, ffmpeg: &str, config: &[u8], gop: &[VideoFrame]) -> anyhow::Result<Vec<u8>> {
        tokio::time::timeout(DECODE_TIMEOUT, self.decode_inner(ffmpeg, config, gop))
            .await
            .map_err(|_| anyhow::anyhow!("ffmpeg 解码超时（3s 未产出 PNG）"))?
    }

    async fn decode_inner(&self, ffmpeg: &str, config: &[u8], gop: &[VideoFrame]) -> anyhow::Result<Vec<u8>> {
        let n = gop.len().saturating_sub(1);
        let mut input = Vec::with_capacity(config.len() + gop.iter().map(|f| f.data.len()).sum::<usize>());
        input.extend_from_slice(config);
        for f in gop {
            input.extend_from_slice(&f.data);
        }
        let filter = format!("select=gte(n\\,{})", n);
        let mut child = tokio::process::Command::new(ffmpeg)
            .args([
                "-loglevel", "error",
                "-f", "h264",
                "-i", "pipe:0",
                "-vf", filter.as_str(),
                "-frames:v", "1",
                "-f", "image2pipe",
                "-vcodec", "png",
                "-compression_level", "1",
                "pipe:1",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow::anyhow!("启动 ffmpeg 失败: {}", e))?;
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("ffmpeg 无 stdin"))?;
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        // 写输入与读输出并发：若先写完再读，ffmpeg 输出管道满时会堵住解码（死锁）
        let write = async move {
            let _ = stdin.write_all(&input).await;
            drop(stdin); // 关闭 stdin → ffmpeg 读到 EOF
        };
        let read = async {
            if let (Some(mut so), Some(mut se)) = (child.stdout.take(), child.stderr.take()) {
                let _ = tokio::join!(so.read_to_end(&mut out), se.read_to_end(&mut err));
            }
        };
        tokio::join!(write, read);
        let status = child
            .wait()
            .await
            .map_err(|e| anyhow::anyhow!("等待 ffmpeg 失败: {}", e))?;
        if !status.success() || out.is_empty() {
            let tail = String::from_utf8_lossy(&err).trim().to_string();
            let tail = if tail.len() > 300 {
                format!("...{}", &tail[tail.len() - 300..])
            } else {
                tail
            };
            anyhow::bail!(
                "ffmpeg 解码失败（exit={}，输入 {} 帧）: {}",
                status,
                gop.len(),
                if tail.is_empty() { "无错误输出" } else { &tail }
            );
        }
        if let Some((w, h)) = png_dims(&out) {
            *self.width.write() = w;
            *self.height.write() = h;
        }
        debug!("frame decoded on demand: {} bytes ({} frames)", out.len(), gop.len());
        Ok(out)
    }
}
