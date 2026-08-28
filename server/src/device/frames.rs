//! 帧缓存（帧环 + 按需解码）：
//! 缓存 scrcpy 的 H.264 原始帧（SPS/PPS 配置帧 + 自最近关键帧起的完整 GOP），
//! 用途：
//!   1. 截图/模板匹配 —— 每次请求用**临时 ffmpeg** 把最新一帧解码成 PNG 返回。
//!      每次都是全新解码，天然实时：返回的图就是服务器当前收到的最新画面，
//!      不存在常驻解码管线的停滞/陈旧问题（旧设计：常驻 ffmpeg 软解 PNG 流，
//!      一旦管线静默冻结，截图/匹配会永远拿到旧画面，需要代数/新鲜度/健康检查
//!      一堆补丁兜底——按需解码把这些判断全部消除）。
//!   2. WebRTC 新 viewer 初始重放（SPS/PPS + 最近 GOP，见 initial_frames）。

use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use parking_lot::{Mutex, RwLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use super::scrcpy::VideoFrame;

const PNG_SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// 从 PNG 头（IHDR）取宽高：8 字节签名 + 4 长度 + 4 "IHDR" → width@16 / height@20（大端）
fn png_dims(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || png[..8] != PNG_SIG {
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
/// 单设备截图解码并发上限：保留同帧合并，但不同帧请求在设备内串行化。
const MAX_CONCURRENT_DECODES_PER_CACHE: usize = 1;

type SharedResult<T, E> = Shared<BoxFuture<'static, Result<Arc<T>, Arc<E>>>>;

struct InFlightEntry<T, E> {
    result: SharedResult<T, E>,
    waiters: AtomicUsize,
}

struct InFlightWaiter<'a, K, T, E>
where
    K: Eq + Hash + Clone + Send + 'static,
    T: Send + Sync + 'static,
    E: Send + Sync + 'static,
{
    owner: &'a InFlight<K, T, E>,
    key: K,
    entry: Arc<InFlightEntry<T, E>>,
    completed: bool,
}

impl<K, T, E> Drop for InFlightWaiter<'_, K, T, E>
where
    K: Eq + Hash + Clone + Send + 'static,
    T: Send + Sync + 'static,
    E: Send + Sync + 'static,
{
    fn drop(&mut self) {
        let last_waiter = self.entry.waiters.fetch_sub(1, Ordering::AcqRel) == 1;
        if self.completed || last_waiter {
            // A completed waiter removes the map entry even when other waiters still consume
            // its result, so a later request cannot re-use a completed old PNG. A cancelled
            // ffmpeg operation is removed when it was the last waiter, allowing its future
            // (and the child process's kill_on_drop) to be dropped.
            self.owner.remove_if_same(&self.key, &self.entry);
        }
    }
}

/// 合并同一个 key 的并发异步请求。
///
/// 首个调用创建并驱动 future，后续调用共享同一个 future 及其结果。结果（包括
/// 错误）会广播给所有等待者；任一等待者拿到结果后都会尝试按条目身份清理，避免
/// 旧请求的收尾误删已经开始的新请求。
pub struct InFlight<K, T, E> {
    entries: Arc<Mutex<HashMap<K, Arc<InFlightEntry<T, E>>>>>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct FrameKey {
    /// Identifies the config + GOP lifetime that produced this frame.
    snapshot_generation: u64,
    /// Explicitly separates a parameter-set replacement from an ordinary GOP update.
    config_generation: u64,
    /// Separates ordinary P-frame arrivals within the same GOP. Only requests that captured
    /// the exact same latest frame may share a decode.
    frame_sequence: u64,
}

#[derive(Clone)]
struct FrameSnapshot {
    key: FrameKey,
    config: Vec<u8>,
    gop: Vec<VideoFrame>,
    /// Sequence of the most recently received frame when this snapshot was taken.
    frame_sequence: u64,
    latest_frame_at: Instant,
}

#[derive(Debug)]
struct DecodedFrame {
    png: Vec<u8>,
    frame_sequence: u64,
}

struct FrameState {
    config_buf: Vec<u8>,
    gop: Vec<VideoFrame>,
    /// Counts every received video frame, including repeated config frames.
    frame_sequence: u64,
    /// Advances when a decodable GOP is replaced or invalidated, not for ordinary P frames.
    snapshot_generation: u64,
    config_generation: u64,
    latest_frame_at: Option<Instant>,
}

impl FrameState {
    fn new() -> Self {
        Self {
            config_buf: Vec::new(),
            gop: Vec::new(),
            frame_sequence: 0,
            snapshot_generation: 0,
            config_generation: 0,
            latest_frame_at: None,
        }
    }

    fn next_counter(counter: &mut u64) {
        *counter = counter
            .checked_add(1)
            .expect("frame sequence/generation exhausted");
    }

    fn observe_frame(&mut self) -> u64 {
        Self::next_counter(&mut self.frame_sequence);
        self.latest_frame_at = Some(Instant::now());
        self.frame_sequence
    }

    fn snapshot_changed(&mut self, sequence: u64) {
        Self::next_counter(&mut self.snapshot_generation);
        debug_assert!(sequence <= self.frame_sequence);
    }

    fn key(&self) -> FrameKey {
        FrameKey {
            snapshot_generation: self.snapshot_generation,
            config_generation: self.config_generation,
            frame_sequence: self.frame_sequence,
        }
    }

    fn snapshot(&self) -> Option<FrameSnapshot> {
        if self.gop.is_empty() {
            return None;
        }
        Some(FrameSnapshot {
            key: self.key(),
            config: self.config_buf.clone(),
            gop: self.gop.clone(),
            frame_sequence: self.frame_sequence,
            latest_frame_at: self.latest_frame_at?,
        })
    }
}

impl<K, T, E> Default for InFlight<K, T, E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, T, E> InFlight<K, T, E> {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl<K, T, E> InFlight<K, T, E>
where
    K: Eq + Hash + Clone + Send + 'static,
    T: Send + Sync + 'static,
    E: Send + Sync + 'static,
{
    /// 执行或加入 key 对应的请求；同一 key 的并发调用只执行一次。
    pub async fn run<F, Fut>(&self, key: K, operation: F) -> Result<Arc<T>, Arc<E>>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        let entry = {
            let mut entries = self.entries.lock();
            if let Some(entry) = entries.get(&key) {
                Arc::clone(entry)
            } else {
                let result = async move { operation().await.map(Arc::new).map_err(Arc::new) }
                    .boxed()
                    .shared();
                let entry = Arc::new(InFlightEntry {
                    result,
                    waiters: AtomicUsize::new(0),
                });
                entries.insert(key.clone(), Arc::clone(&entry));
                entry
            }
        };

        entry.waiters.fetch_add(1, Ordering::AcqRel);
        let mut waiter = InFlightWaiter {
            owner: self,
            key,
            entry,
            completed: false,
        };
        let result = waiter.entry.result.clone().await;
        waiter.completed = true;
        drop(waiter);
        result
    }

    fn remove_if_same(&self, key: &K, expected: &Arc<InFlightEntry<T, E>>) {
        let mut entries = self.entries.lock();
        if entries
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, expected))
        {
            entries.remove(key);
        }
    }
}

pub struct FrameCache {
    ffmpeg_path: Mutex<String>,
    /// 配置帧、GOP、序号和到达时间必须在同一把锁下观察，避免撕裂快照。
    state: Mutex<FrameState>,
    /// 同一个精确帧快照只允许一个真实 ffmpeg 解码，结果/错误由所有 waiter 共享。
    decode_in_flight: InFlight<FrameKey, DecodedFrame, String>,
    /// 单设备有界闸门：避免同一设备上出现多个 ffmpeg / PNG 解码同时占用 Tokio 资源。
    decode_budget: Arc<Semaphore>,
    /// 帧尺寸（最近一次成功解码的 PNG 尺寸，供设备信息展示）
    width: RwLock<u32>,
    height: RwLock<u32>,
}

impl FrameCache {
    pub fn start(ffmpeg_path: &str) -> Arc<Self> {
        Arc::new(Self {
            ffmpeg_path: Mutex::new(ffmpeg_path.to_string()),
            state: Mutex::new(FrameState::new()),
            decode_in_flight: InFlight::new(),
            decode_budget: Arc::new(Semaphore::new(MAX_CONCURRENT_DECODES_PER_CACHE)),
            width: RwLock::new(0),
            height: RwLock::new(0),
        })
    }

    /// 喂入一帧：仅更新 SPS/PPS 配置帧与 GOP 环（无任何解码工作）。
    /// 注意：本方法由视频消费任务调用，必须保持轻量（只做内存拷贝）。
    pub fn feed(&self, frame: &VideoFrame) {
        let mut state = self.state.lock();
        let sequence = state.observe_frame();
        if frame.is_config {
            if state.config_buf.is_empty() || state.config_buf != frame.data {
                // 参数集变化（分辨率/编码参数切换，如游戏切横竖屏、编码器重启）：
                // 旧 GOP 与新参数不匹配，清空等新 IDR 重建——否则 WebRTC 初始重放
                // 与按需解码会把"新 SPS/PPS + 旧 GOP"喂给解码器 → 花屏/解码失败
                // （repeat-previous-headers 会周期性重发相同参数集，字节相同则不清）
                if !state.config_buf.is_empty() {
                    state.gop.clear();
                    debug!(
                        "SPS/PPS changed ({}B → {}B), GOP cleared",
                        state.config_buf.len(),
                        frame.data.len()
                    );
                }
                FrameState::next_counter(&mut state.config_generation);
                state.config_buf = frame.data.clone();
                state.snapshot_changed(sequence);
            }
            return;
        }
        // GOP 缓存维护：IDR 清空重建；P 帧追加；超限丢弃整个 GOP（等待下一个 IDR）
        if frame.is_keyframe {
            state.gop.clear();
            state.gop.push(frame.clone());
            state.snapshot_changed(sequence);
        } else if !state.gop.is_empty() {
            state.gop.push(frame.clone());
            let total: usize = state.gop.iter().map(|f| f.data.len()).sum();
            if state.gop.len() > GOP_MAX_FRAMES || total > GOP_MAX_BYTES {
                state.gop.clear();
                // A discarded GOP is no longer a decodable snapshot, so invalidate any
                // in-flight decode that captured it. Ordinary P frames keep the same GOP
                // generation: they must not make a live decode stale on every video tick.
                state.snapshot_changed(sequence);
            }
        }
    }

    /// WebRTC 新 viewer 的初始推流帧：SPS/PPS 配置帧 + 最近完整 GOP（自最近 IDR 起）。
    /// pusher 先重放这些帧，浏览器无需等待下一个 IDR 即可开始解码（静态画面下可能长时间无 IDR）。
    /// 返回 None 表示缓存里还没有可用帧（会话刚建立时）。
    pub fn initial_frames(&self) -> Option<Vec<VideoFrame>> {
        let mut frames = Vec::new();
        let state = self.state.lock();
        if !state.config_buf.is_empty() {
            frames.push(VideoFrame {
                data: state.config_buf.clone(),
                pts_us: 0,
                is_config: true,
                is_keyframe: false,
                annex_b: true,
            });
        }
        if !state.gop.is_empty() {
            frames.extend(state.gop.iter().cloned());
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

    /// 返回收到的最新帧序号和到达时间；二者来自同一快照，调用方不会观察到撕裂状态。
    pub fn latest_frame_info(&self) -> (u64, Option<Instant>) {
        let state = self.state.lock();
        (state.frame_sequence, state.latest_frame_at)
    }

    fn snapshot(&self) -> Option<FrameSnapshot> {
        self.state.lock().snapshot()
    }

    fn is_snapshot_current(&self, key: FrameKey, frame_sequence: u64) -> bool {
        let state = self.state.lock();
        !state.gop.is_empty() && state.key() == key && state.frame_sequence == frame_sequence
    }

    /// 仅在失败仍对应当前快照时清空 GOP。旧快照的异步 decode 收尾绝不能清掉新 GOP。
    fn clear_gop_if_same(&self, key: FrameKey) -> bool {
        let mut state = self.state.lock();
        if !state.gop.is_empty() && state.key() == key {
            state.gop.clear();
            FrameState::next_counter(&mut state.snapshot_generation);
            true
        } else {
            false
        }
    }

    /// 按需解码最新帧为 PNG（供截图/模板匹配）：
    /// 每次调用都从当前精确帧快照全新解码。相同帧序号的并发请求共享同一个真实
    /// ffmpeg future；解码完成后重新核对 key，避免把 config/GOP 已替换的旧 PNG 当成当前帧。
    /// 无帧（会话刚建立/刚清空）时等首个 IDR ≤1.5s；解码失败时仅当失败仍对应当前
    /// 快照才清空 GOP，随后等待新 IDR 重试一次。
    /// Ok(None) = 等待超时仍无关键帧（会话刚建立，稍后再试）。
    pub async fn decode_latest_png(&self) -> anyhow::Result<Option<Vec<u8>>> {
        let ffmpeg = self.ffmpeg_path.lock().clone();
        let decode_budget = Arc::clone(&self.decode_budget);
        let mut decode_error = None;
        let mut retried_after_error = false;
        let mut refreshed_after_stale = false;

        loop {
            let Some(snapshot) = self.await_gop().await else {
                return match decode_error {
                    Some(error) => {
                        Err(anyhow::anyhow!("按需解码失败且等待新关键帧超时: {}", error))
                    }
                    None => Ok(None),
                };
            };
            let key = snapshot.key;
            let snapshot_latest_frame_at = snapshot.latest_frame_at;
            let ffmpeg_for_decode = ffmpeg.clone();
            let decode_budget = Arc::clone(&decode_budget);
            let decoded = self
                .request_snapshot(snapshot, move |snapshot| async move {
                    Self::decode_once_with_budget(
                        Arc::clone(&decode_budget),
                        &ffmpeg_for_decode,
                        &snapshot.config,
                        &snapshot.gop,
                    )
                    .await
                })
                .await;

            match decoded {
                Ok(decoded) => {
                    if self.is_snapshot_current(key, decoded.frame_sequence) {
                        self.record_png_dims(&decoded.png);
                        return Ok(Some(decoded.png.clone()));
                    }

                    // 不能返回被替换的 config/GOP。只刷新一次，避免视频持续到达时无限追帧；
                    // 第二次仍被更新则丢弃本次结果，让上层按“无可用帧”路径稍后重试。
                    if !refreshed_after_stale {
                        refreshed_after_stale = true;
                        debug!(
                            "frame snapshot superseded during decode: seq={}, age={:?}",
                            decoded.frame_sequence,
                            snapshot_latest_frame_at.elapsed()
                        );
                        continue;
                    }
                    warn!("按需解码完成时帧快照已更新，丢弃过期 PNG");
                    return Ok(None);
                }
                Err(error) => {
                    let error = error.as_ref().clone();
                    warn!("按需解码失败: {}；清空当前 GOP 等新关键帧重试", error);
                    let _cleared_current = self.clear_gop_if_same(key);
                    if !retried_after_error {
                        retried_after_error = true;
                        decode_error = Some(error);
                        continue;
                    }
                    return Err(anyhow::anyhow!("按需解码失败（重试后仍失败）: {}", error));
                }
            }
        }
    }

    /// 执行或加入精确帧快照的解码请求。该边界由生产截图路径直接调用，并允许测试
    /// 注入解码器，验证并发合并、错误广播和不同 key 的独立进度。
    async fn request_snapshot<F, Fut>(
        &self,
        snapshot: FrameSnapshot,
        decode: F,
    ) -> Result<Arc<DecodedFrame>, Arc<String>>
    where
        F: FnOnce(FrameSnapshot) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<Vec<u8>>> + Send + 'static,
    {
        let key = snapshot.key;
        let frame_sequence = snapshot.frame_sequence;
        self.decode_in_flight
            .run(key, move || async move {
                decode(snapshot)
                    .await
                    .map(|png| DecodedFrame {
                        png,
                        frame_sequence,
                    })
                    .map_err(|error| error.to_string())
            })
            .await
    }

    async fn decode_once_with_budget(
        budget: Arc<Semaphore>,
        ffmpeg: &str,
        config: &[u8],
        gop: &[VideoFrame],
    ) -> anyhow::Result<Vec<u8>> {
        let permit = budget
            .acquire_owned()
            .await
            .map_err(|_| anyhow::anyhow!("截图解码闸门已关闭"))?;
        let decoded = Self::decode_once(ffmpeg, config, gop).await;
        drop(permit);
        decoded
    }

    #[cfg(test)]
    async fn run_with_decode_budget<F, Fut, T>(&self, work: F) -> anyhow::Result<T>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let permit = self
            .decode_budget
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow::anyhow!("截图解码闸门已关闭"))?;
        let result = work().await;
        drop(permit);
        Ok(result)
    }

    /// 等待 GOP 非空（首个 IDR 未到或解码失败后刚清空重建），超时返回 None
    async fn await_gop(&self) -> Option<FrameSnapshot> {
        let deadline = tokio::time::Instant::now() + WAIT_FIRST_GOP;
        loop {
            if let Some(snapshot) = self.snapshot() {
                return Some(snapshot);
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
    async fn decode_once(
        ffmpeg: &str,
        config: &[u8],
        gop: &[VideoFrame],
    ) -> anyhow::Result<Vec<u8>> {
        tokio::time::timeout(DECODE_TIMEOUT, Self::decode_inner(ffmpeg, config, gop))
            .await
            .map_err(|_| anyhow::anyhow!("ffmpeg 解码超时（3s 未产出 PNG）"))?
    }

    async fn decode_inner(
        ffmpeg: &str,
        config: &[u8],
        gop: &[VideoFrame],
    ) -> anyhow::Result<Vec<u8>> {
        let n = gop.len().saturating_sub(1);
        let mut input =
            Vec::with_capacity(config.len() + gop.iter().map(|f| f.data.len()).sum::<usize>());
        input.extend_from_slice(config);
        for f in gop {
            input.extend_from_slice(&f.data);
        }
        let filter = format!("select=gte(n\\,{})", n);
        let mut child = tokio::process::Command::new(ffmpeg)
            .args([
                "-loglevel",
                "error",
                "-f",
                "h264",
                "-i",
                "pipe:0",
                "-vf",
                filter.as_str(),
                "-frames:v",
                "1",
                "-f",
                "image2pipe",
                "-vcodec",
                "png",
                "-compression_level",
                "1",
                "pipe:1",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow::anyhow!("启动 ffmpeg 失败: {}", e))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("ffmpeg 无 stdin"))?;
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
                if tail.is_empty() {
                    "无错误输出"
                } else {
                    &tail
                }
            );
        }
        debug!(
            "frame decoded on demand: {} bytes ({} frames)",
            out.len(),
            gop.len()
        );
        Ok(out)
    }

    fn record_png_dims(&self, png: &[u8]) {
        if let Some((w, h)) = png_dims(png) {
            *self.width.write() = w;
            *self.height.write() = h;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::{FrameCache, FrameKey, InFlight};
    use crate::device::scrcpy::VideoFrame;
    use tokio::sync::Notify;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestError(&'static str);

    fn video_frame(data: u8, is_config: bool, is_keyframe: bool) -> VideoFrame {
        VideoFrame {
            data: vec![data],
            pts_us: 0,
            is_config,
            is_keyframe,
            annex_b: true,
        }
    }

    #[test]
    fn frame_sequence_and_snapshot_generation_are_monotonic() {
        let cache = FrameCache::start("ffmpeg");
        assert_eq!(cache.latest_frame_info().0, 0);

        cache.feed(&video_frame(1, true, false));
        let (sequence_after_config, latest_at) = cache.latest_frame_info();
        assert_eq!(sequence_after_config, 1);
        assert!(latest_at.is_some());

        // Repeated SPS/PPS is a new arrival but not a new decode snapshot.
        cache.feed(&video_frame(1, true, false));
        assert_eq!(cache.latest_frame_info().0, 2);

        cache.feed(&video_frame(2, false, true));
        let first = cache.snapshot().expect("keyframe creates a snapshot");
        assert!(cache.is_snapshot_current(first.key, first.frame_sequence));
        cache.feed(&video_frame(3, false, false));
        let second = cache.snapshot().expect("P frame extends the snapshot");

        assert_eq!(cache.latest_frame_info().0, 4);
        assert_eq!(
            second.key.snapshot_generation,
            first.key.snapshot_generation
        );
        assert_eq!(second.key.config_generation, first.key.config_generation);
        assert_ne!(second.key, first.key);
        assert!(second.frame_sequence > first.frame_sequence);
        assert!(second.latest_frame_at >= first.latest_frame_at);
        assert!(!cache.is_snapshot_current(first.key, first.frame_sequence));
        assert!(cache.is_snapshot_current(second.key, second.frame_sequence));
    }

    #[test]
    fn old_snapshot_failure_cannot_clear_replaced_gop() {
        let cache = FrameCache::start("ffmpeg");
        cache.feed(&video_frame(1, true, false));
        cache.feed(&video_frame(2, false, true));
        let old = cache.snapshot().expect("old snapshot");

        cache.feed(&video_frame(9, true, false));
        cache.feed(&video_frame(10, false, true));
        let current = cache.snapshot().expect("replacement snapshot");

        assert_ne!(old.key, current.key);
        assert!(!cache.is_snapshot_current(old.key, old.frame_sequence));
        assert!(!cache.clear_gop_if_same(old.key));

        let preserved = cache.snapshot().expect("new GOP remains available");
        assert_eq!(preserved.key, current.key);
        assert_eq!(preserved.gop[0].data, vec![10]);
    }

    #[test]
    fn clearing_current_snapshot_invalidates_its_key() {
        let cache = FrameCache::start("ffmpeg");
        cache.feed(&video_frame(1, true, false));
        cache.feed(&video_frame(2, false, true));
        let current = cache.snapshot().expect("current snapshot");

        assert!(cache.clear_gop_if_same(current.key));
        assert!(cache.snapshot().is_none());
    }

    struct DropProbe(std::sync::Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    async fn wait_for_waiters(cache: &FrameCache, key: FrameKey, expected: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let waiters = cache
                    .decode_in_flight
                    .entries
                    .lock()
                    .get(&key)
                    .map(|entry| entry.waiters.load(Ordering::Acquire))
                    .unwrap_or(0);
                if waiters >= expected {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("waiters should join the in-flight request");
    }

    #[tokio::test]
    async fn request_snapshot_coalesces_concurrent_same_frame() {
        let cache = FrameCache::start("ffmpeg");
        cache.feed(&video_frame(1, true, false));
        cache.feed(&video_frame(2, false, true));
        let snapshot = cache.snapshot().expect("snapshot");
        let key = snapshot.key;
        let executions = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());

        let first_cache = Arc::clone(&cache);
        let first_snapshot = snapshot.clone();
        let first_executions = Arc::clone(&executions);
        let first_started = Arc::clone(&started);
        let first_release = Arc::clone(&release);
        let first = tokio::spawn(async move {
            first_cache
                .request_snapshot(first_snapshot, move |_| async move {
                    first_executions.fetch_add(1, Ordering::SeqCst);
                    first_started.notify_one();
                    first_release.notified().await;
                    Ok(vec![7])
                })
                .await
        });

        started.notified().await;

        let second_cache = Arc::clone(&cache);
        let second_executions = Arc::clone(&executions);
        let second = tokio::spawn(async move {
            second_cache
                .request_snapshot(snapshot, move |_| async move {
                    second_executions.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![99])
                })
                .await
        });

        wait_for_waiters(&cache, key, 2).await;
        release.notify_one();

        assert_eq!(first.await.unwrap().unwrap().png, vec![7]);
        assert_eq!(second.await.unwrap().unwrap().png, vec![7]);
        assert_eq!(executions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn request_snapshot_propagates_failure_and_recovers() {
        let cache = FrameCache::start("ffmpeg");
        cache.feed(&video_frame(1, true, false));
        cache.feed(&video_frame(2, false, true));
        let snapshot = cache.snapshot().expect("snapshot");
        let key = snapshot.key;
        let executions = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());

        let first_cache = Arc::clone(&cache);
        let first_snapshot = snapshot.clone();
        let first_executions = Arc::clone(&executions);
        let first_started = Arc::clone(&started);
        let first_release = Arc::clone(&release);
        let first = tokio::spawn(async move {
            first_cache
                .request_snapshot(first_snapshot, move |_| async move {
                    first_executions.fetch_add(1, Ordering::SeqCst);
                    first_started.notify_one();
                    first_release.notified().await;
                    Err(anyhow::anyhow!("temporary failure"))
                })
                .await
        });

        started.notified().await;

        let second_cache = Arc::clone(&cache);
        let second_snapshot = snapshot.clone();
        let second = tokio::spawn(async move {
            second_cache
                .request_snapshot(second_snapshot, |_| async move {
                    Err(anyhow::anyhow!("should not execute"))
                })
                .await
        });

        wait_for_waiters(&cache, key, 2).await;
        release.notify_one();

        assert_eq!(
            first.await.unwrap().unwrap_err().as_str(),
            "temporary failure"
        );
        assert_eq!(
            second.await.unwrap().unwrap_err().as_str(),
            "temporary failure"
        );
        assert_eq!(executions.load(Ordering::SeqCst), 1);

        let retry_executions = Arc::clone(&executions);
        let retry = cache
            .request_snapshot(snapshot, move |_| async move {
                retry_executions.fetch_add(1, Ordering::SeqCst);
                Ok(vec![9])
            })
            .await
            .unwrap();
        assert_eq!(retry.png, vec![9]);
        assert_eq!(executions.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn request_snapshot_different_keys_do_not_block_each_other() {
        let cache = FrameCache::start("ffmpeg");
        cache.feed(&video_frame(1, true, false));
        cache.feed(&video_frame(2, false, true));
        let first_snapshot = cache.snapshot().expect("first snapshot");
        let first_key = first_snapshot.key;
        let first_started = Arc::new(Notify::new());
        let first_release = Arc::new(Notify::new());

        let first_cache = Arc::clone(&cache);
        let started = Arc::clone(&first_started);
        let release = Arc::clone(&first_release);
        let first = tokio::spawn(async move {
            first_cache
                .request_snapshot(first_snapshot, move |_| async move {
                    started.notify_one();
                    release.notified().await;
                    Ok(vec![1])
                })
                .await
        });

        first_started.notified().await;
        cache.feed(&video_frame(3, false, false));
        let second_snapshot = cache.snapshot().expect("second snapshot");
        assert_ne!(first_key, second_snapshot.key);

        let second = tokio::time::timeout(
            Duration::from_secs(1),
            cache.request_snapshot(second_snapshot, |_| async move { Ok(vec![2]) }),
        )
        .await
        .expect("a different frame key must not wait for the first decode")
        .unwrap();
        assert_eq!(second.png, vec![2]);

        first_release.notify_one();
        assert_eq!(first.await.unwrap().unwrap().png, vec![1]);
    }

    #[tokio::test]
    async fn request_snapshot_is_scoped_to_each_frame_cache() {
        let first_cache = FrameCache::start("ffmpeg");
        let second_cache = FrameCache::start("ffmpeg");
        for cache in [&first_cache, &second_cache] {
            cache.feed(&video_frame(1, true, false));
            cache.feed(&video_frame(2, false, true));
        }
        let first_snapshot = first_cache.snapshot().expect("first snapshot");
        let second_snapshot = second_cache.snapshot().expect("second snapshot");
        assert_eq!(first_snapshot.key, second_snapshot.key);

        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let task_cache = Arc::clone(&first_cache);
        let task_started = Arc::clone(&started);
        let task_release = Arc::clone(&release);
        let first = tokio::spawn(async move {
            task_cache
                .request_snapshot(first_snapshot, move |_| async move {
                    task_started.notify_one();
                    task_release.notified().await;
                    Ok(vec![1])
                })
                .await
        });

        started.notified().await;
        let second = tokio::time::timeout(
            Duration::from_secs(1),
            second_cache.request_snapshot(second_snapshot, |_| async move { Ok(vec![2]) }),
        )
        .await
        .expect("another device cache must have an independent in-flight map")
        .unwrap();
        assert_eq!(second.png, vec![2]);

        release.notify_one();
        assert_eq!(first.await.unwrap().unwrap().png, vec![1]);
    }

    #[tokio::test]
    async fn request_snapshot_bounds_per_cache_decode_concurrency() {
        let cache = FrameCache::start("ffmpeg");
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let first_cache = Arc::clone(&cache);
        let first_started = Arc::clone(&started);
        let first_release = Arc::clone(&release);
        let first_active = Arc::clone(&active);
        let first_peak = Arc::clone(&peak);
        let first = tokio::spawn(async move {
            first_cache
                .run_with_decode_budget(move || async move {
                    let now = first_active.fetch_add(1, Ordering::SeqCst) + 1;
                    first_peak.fetch_max(now, Ordering::SeqCst);
                    first_started.notify_one();
                    first_release.notified().await;
                    first_active.fetch_sub(1, Ordering::SeqCst);
                    1usize
                })
                .await
        });

        started.notified().await;

        let second_cache = Arc::clone(&cache);
        let second_active = Arc::clone(&active);
        let second_peak = Arc::clone(&peak);
        let second = tokio::spawn(async move {
            second_cache
                .run_with_decode_budget(move || async move {
                    let now = second_active.fetch_add(1, Ordering::SeqCst) + 1;
                    second_peak.fetch_max(now, Ordering::SeqCst);
                    second_active.fetch_sub(1, Ordering::SeqCst);
                    2usize
                })
                .await
        });

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(peak.load(Ordering::SeqCst), 1);
        release.notify_one();

        assert_eq!(first.await.unwrap().unwrap(), 1);
        assert_eq!(second.await.unwrap().unwrap(), 2);
        assert_eq!(peak.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn request_snapshot_releases_budget_after_failure() {
        let cache = FrameCache::start("ffmpeg");
        cache.feed(&video_frame(1, true, false));
        cache.feed(&video_frame(2, false, true));
        let snapshot = cache.snapshot().expect("snapshot");
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let executed = Arc::new(AtomicUsize::new(0));

        let retry_snapshot = snapshot.clone();
        let retry_executed = Arc::clone(&executed);
        let first_cache = Arc::clone(&cache);
        let first_started = Arc::clone(&started);
        let first_release = Arc::clone(&release);
        let first_executed = Arc::clone(&executed);
        let first = tokio::spawn(async move {
            first_cache
                .request_snapshot(snapshot.clone(), move |_| async move {
                    first_executed.fetch_add(1, Ordering::SeqCst);
                    first_started.notify_one();
                    first_release.notified().await;
                    Err(anyhow::anyhow!("boom"))
                })
                .await
        });

        started.notified().await;
        release.notify_one();
        assert_eq!(first.await.unwrap().unwrap_err().as_str(), "boom");
        tokio::task::yield_now().await;

        let retry = cache
            .request_snapshot(retry_snapshot, move |_| async move {
                retry_executed.fetch_add(1, Ordering::SeqCst);
                Ok(vec![9])
            })
            .await
            .expect("retry should recover");

        assert_eq!(retry.png, vec![9]);
        assert_eq!(executed.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn in_flight_cancellation_cleans_last_waiter_and_drops_future() {
        let requests = std::sync::Arc::new(InFlight::<u64, usize, TestError>::new());
        let executions = std::sync::Arc::new(AtomicUsize::new(0));
        let future_drops = std::sync::Arc::new(AtomicUsize::new(0));
        let started = std::sync::Arc::new(Notify::new());

        let first_requests = std::sync::Arc::clone(&requests);
        let first_executions = std::sync::Arc::clone(&executions);
        let first_future_drops = std::sync::Arc::clone(&future_drops);
        let first_started = std::sync::Arc::clone(&started);
        let first = tokio::spawn(async move {
            first_requests
                .run(42, move || async move {
                    first_executions.fetch_add(1, Ordering::SeqCst);
                    let _probe = DropProbe(first_future_drops);
                    first_started.notify_one();
                    std::future::pending::<Result<usize, TestError>>().await
                })
                .await
        });

        started.notified().await;
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        tokio::task::yield_now().await;
        assert_eq!(future_drops.load(Ordering::SeqCst), 1);

        let retry_executions = std::sync::Arc::clone(&executions);
        let retry = requests
            .run(42, move || async move {
                retry_executions.fetch_add(1, Ordering::SeqCst);
                Ok(9)
            })
            .await
            .unwrap();
        assert_eq!(*retry, 9);
        assert_eq!(executions.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn in_flight_shares_one_execution_for_concurrent_callers() {
        let requests = std::sync::Arc::new(InFlight::<u64, usize, TestError>::new());
        let executions = std::sync::Arc::new(AtomicUsize::new(0));
        let started = std::sync::Arc::new(Notify::new());
        let release = std::sync::Arc::new(Notify::new());

        let first_requests = std::sync::Arc::clone(&requests);
        let first_executions = std::sync::Arc::clone(&executions);
        let first_started = std::sync::Arc::clone(&started);
        let first_release = std::sync::Arc::clone(&release);
        let first = tokio::spawn(async move {
            first_requests
                .run(42, move || async move {
                    first_executions.fetch_add(1, Ordering::SeqCst);
                    first_started.notify_one();
                    first_release.notified().await;
                    Ok(7)
                })
                .await
        });

        started.notified().await;

        let second_requests = std::sync::Arc::clone(&requests);
        let second = tokio::spawn(async move { second_requests.run(42, || async { Ok(7) }).await });
        tokio::task::yield_now().await;
        release.notify_one();

        assert_eq!(*first.await.unwrap().unwrap(), 7);
        assert_eq!(*second.await.unwrap().unwrap(), 7);
        assert_eq!(executions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn in_flight_propagates_error_and_allows_retry() {
        let requests = std::sync::Arc::new(InFlight::<u64, usize, TestError>::new());
        let executions = std::sync::Arc::new(AtomicUsize::new(0));
        let started = std::sync::Arc::new(Notify::new());
        let release = std::sync::Arc::new(Notify::new());

        let first_requests = std::sync::Arc::clone(&requests);
        let first_executions = std::sync::Arc::clone(&executions);
        let first_started = std::sync::Arc::clone(&started);
        let first_release = std::sync::Arc::clone(&release);
        let first = tokio::spawn(async move {
            first_requests
                .run(42, move || async move {
                    first_executions.fetch_add(1, Ordering::SeqCst);
                    first_started.notify_one();
                    first_release.notified().await;
                    Err(TestError("temporary failure"))
                })
                .await
        });

        started.notified().await;

        let second_requests = std::sync::Arc::clone(&requests);
        let second = tokio::spawn(async move {
            second_requests
                .run(42, || async { Err(TestError("should not execute")) })
                .await
        });
        tokio::task::yield_now().await;
        release.notify_one();

        assert_eq!(
            first.await.unwrap().unwrap_err().as_ref(),
            &TestError("temporary failure")
        );
        assert_eq!(
            second.await.unwrap().unwrap_err().as_ref(),
            &TestError("temporary failure")
        );
        assert_eq!(executions.load(Ordering::SeqCst), 1);

        let retry_executions = std::sync::Arc::clone(&executions);
        let retry = requests
            .run(42, move || async move {
                retry_executions.fetch_add(1, Ordering::SeqCst);
                Ok(9)
            })
            .await
            .unwrap();
        assert_eq!(*retry, 9);
        assert_eq!(executions.load(Ordering::SeqCst), 2);
    }
}
