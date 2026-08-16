//! 帧缓存：用 ffmpeg 软解 scrcpy 的 H.264 流，缓存最新完整帧（PNG），
//! 供模板匹配与截图接口使用（延迟 <50ms，远快于 adb screencap 的 300~800ms）。
//!
//! 实现：ffmpeg 子进程 stdin 喂 H.264（SPS/PPS + 关键帧），stdout 输出 PNG 流；
//! stdout 由独立线程持续消费并解析（避免阻塞视频帧消费循环）。

use std::io::{BufRead, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex as StdMutex};
use std::time::Duration;

use parking_lot::Mutex;
use parking_lot::RwLock;
use tracing::{debug, warn};

use crate::store::Device;

use super::scrcpy::VideoFrame;

const PNG_SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
const IEND: [u8; 4] = [0x49, 0x45, 0x4E, 0x44]; // "IEND" chunk

/// GOP 缓存上限（帧数与字节数，超限丢弃整个 GOP 等下一个 IDR 重建）
const GOP_MAX_FRAMES: usize = 400;
const GOP_MAX_BYTES: usize = 8 * 1024 * 1024;

/// 待写入 ffmpeg stdin 的一帧（单槽：新帧替换旧帧，天然背压，不阻塞调用方）
struct PendingFeed {
    data: Vec<u8>,
    /// 优先级：关键帧 3 > 配置帧 2 > 普通帧 1。
    /// 高优先级帧可以顶掉低优先级帧；同优先级保留更新的一帧。
    prio: u8,
}

pub struct FrameCache {
    device: Device,
    ffmpeg: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    /// ffmpeg 可执行文件路径（重启时复用）
    ffmpeg_path: Mutex<String>,
    /// ffmpeg stdout 的未解析缓冲（由消费线程持有）
    pending: Arc<Mutex<Vec<u8>>>,
    /// 最新完整帧 PNG
    latest: Arc<RwLock<Option<Vec<u8>>>>,
    /// 上次喂给 ffmpeg 的 SPS/PPS 配置帧
    config_buf: Mutex<Vec<u8>>,
    /// 最近完整 GOP（自最近 IDR 起），供 WebRTC 新 viewer 初始重放快速出画面
    gop: Arc<Mutex<Vec<VideoFrame>>>,
    /// 帧尺寸
    pub width: Arc<RwLock<u32>>,
    pub height: Arc<RwLock<u32>>,
    /// 消费线程退出标志（重启时置位）
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
    /// ffmpeg stdin 写入队列（单槽）+ 唤醒（专用写入线程持有）
    feed_slot: Arc<(StdMutex<Option<PendingFeed>>, Condvar)>,
}

impl FrameCache {
    pub fn start(device: Device, ffmpeg_path: &str) -> anyhow::Result<Arc<Self>> {
        let cache = Arc::new(Self {
            device,
            ffmpeg: Mutex::new(None),
            stdin: Mutex::new(None),
            ffmpeg_path: Mutex::new(ffmpeg_path.to_string()),
            pending: Arc::new(Mutex::new(Vec::new())),
            latest: Arc::new(RwLock::new(None)),
            config_buf: Mutex::new(Vec::new()),
            gop: Arc::new(Mutex::new(Vec::new())),
            width: Arc::new(RwLock::new(0)),
            height: Arc::new(RwLock::new(0)),
            stop_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            feed_slot: Arc::new((StdMutex::new(None), Condvar::new())),
        });
        cache.spawn_ffmpeg()?;
        cache.spawn_writer();
        Ok(cache)
    }

    fn spawn_ffmpeg(&self) -> anyhow::Result<()> {
        let path = self.ffmpeg_path.lock().clone();
        // 注意：ffmpeg 9.0 移除了 -vsync（改用 -fps_mode），且 -fps_mode 在旧版不存在。
        // 去掉帧同步参数：image2pipe 按输入帧直接输出 PNG，天然一一对应。
        let mut child = std::process::Command::new(&path)
            .args([
                "-loglevel", "error",
                "-f", "h264",
                "-i", "pipe:0",
                "-f", "image2pipe",
                "-vcodec", "png",
                // 输出限 5fps：模板匹配/截图只需"最新帧"，无需每帧 PNG 编解码。
                // 3008x1880 全帧率 PNG 编解码会让单核跑满（CPU 100% 持续 → 拖垮推流）。
                // ffmpeg 仍解码全部输入帧（保持解码器连续），只是 PNG 输出抽样。
                "-r", "5",
                "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("no stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow::anyhow!("no stderr"))?;
        // stderr 消费（防管道满阻塞 ffmpeg），打日志
        std::thread::spawn(move || {
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(l) => warn!("[ffmpeg] {}", l),
                    Err(_) => break,
                }
            }
        });
        *self.ffmpeg.lock() = Some(child);
        *self.stdin.lock() = Some(stdin);
        self.spawn_stdout_consumer(stdout);
        Ok(())
    }

    /// 独立线程消费 ffmpeg stdout，解析 PNG 更新 latest
    fn spawn_stdout_consumer(&self, stdout: ChildStdout) {
        let pending = self.pending.clone();
        let latest = self.latest.clone();
        let width = self.width.clone();
        let height = self.height.clone();
        let stop = self.stop_flag.clone();
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut buf = [0u8; 16384];
            loop {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        pending.lock().extend_from_slice(&buf[..n]);
                        // 提取完整 PNG（IEND 结尾）
                        loop {
                            let mut p = pending.lock();
                            if p.len() < 8 {
                                break;
                            }
                            if &p[..8] != &PNG_SIG {
                                p.drain(..1);
                                continue;
                            }
                            let mut end = None;
                            let mut i = 8;
                            // 完整 PNG 需 IEND 标志（4B）+ CRC（4B）都在缓冲内。
                            // 注意：i 是 IEND 类型字节的位置，块结束 = i + 4(类型) + 4(CRC) = i + 8。
                            // 旧实现 i + 12 会多吞下一张 PNG 的前 4 字节（89 50 4E 47），
                            // 导致缓存 PNG 尾部带垃圾 + 每两张丢弃一张（帧率减半）。
                            while i + 8 <= p.len() {
                                if &p[i..i + 4] == &IEND {
                                    end = Some(i + 8);
                                    break;
                                }
                                i += 1;
                            }
                            match end {
                                Some(e) => {
                                    let png: Vec<u8> = p.drain(..e).collect();
                                    drop(p);
                                    if let Ok(img) = image::load_from_memory(&png) {
                                        *width.write() = img.width();
                                        *height.write() = img.height();
                                        *latest.write() = Some(png);
                                        debug!("frame cached {}x{}", img.width(), img.height());
                                    }
                                }
                                None => break,
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            debug!("ffmpeg stdout consumer exited");
        });
    }

    /// 喂入一帧：所有帧（SPS/PPS 配置帧 + IDR + P 帧）都写入 ffmpeg 保持解码器连续。
    ///
    /// 注意：只做内存入队（单槽替换），**绝不在此处做阻塞管道写**——
    /// 本方法由 tokio worker 线程调用，若直接 write_all 到 ffmpeg stdin，
    /// 管道写满时（ffmpeg 软解 PNG 远慢于输入帧率）会卡死整个 worker 线程，
    /// 同一 worker 上的帧转发/WebRTC pusher 全部冻结 → 浏览器画面卡住。
    /// 实际写管道由专用线程完成（见 spawn_writer）。
    pub fn feed(&self, frame: &VideoFrame) {
        if frame.is_config {
            // SPS/PPS 配置帧：缓存（供关键帧合并）并入队（优先级 2，槽空时才保留优先）
            *self.config_buf.lock() = frame.data.clone();
            self.enqueue(frame.data.clone(), 2);
            return;
        }
        // GOP 缓存维护：IDR 清空重建；P 帧追加；超限丢弃整个 GOP（等待下一个 IDR）
        {
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
        // 关键帧：合并缓存的 SPS/PPS（幂等，重复无害）
        let data_vec: Vec<u8> = if frame.is_keyframe {
            let cfg = self.config_buf.lock();
            if !cfg.is_empty() {
                let mut v = Vec::with_capacity(cfg.len() + frame.data.len());
                v.extend_from_slice(&cfg);
                v.extend_from_slice(&frame.data);
                v
            } else {
                frame.data.clone()
            }
        } else {
            frame.data.clone()
        };
        let prio = if frame.is_keyframe { 3 } else { 1 };
        self.enqueue(data_vec, prio);
    }

    /// 入队一帧待写（单槽替换：槽内已有更高/同级优先级帧时按规则替换，内存有界）
    fn enqueue(&self, data: Vec<u8>, prio: u8) {
        let mut slot = self.feed_slot.0.lock().unwrap();
        let replace = match slot.as_ref() {
            None => true,
            Some(p) => prio >= p.prio,
        };
        if replace {
            *slot = Some(PendingFeed { data, prio });
        }
        self.feed_slot.1.notify_one();
    }

    /// 专用写入线程：从单槽取帧 → 阻塞写 ffmpeg stdin（阻塞只发生在本线程，
    /// 绝不占用 tokio worker）。写失败（管道断）→ 重启 ffmpeg 后继续。
    /// stop() 后退出。
    fn spawn_writer(self: &Arc<Self>) {
        let this = self.clone();
        std::thread::spawn(move || loop {
            if this.stop_flag.load(Ordering::SeqCst) {
                break;
            }
            let data = {
                let mut slot = this.feed_slot.0.lock().unwrap();
                loop {
                    if let Some(p) = slot.take() {
                        break p.data;
                    }
                    if this.stop_flag.load(Ordering::SeqCst) {
                        return; // 已停止（锁随 guard 释放）
                    }
                    slot = this.feed_slot.1.wait(slot).unwrap();
                }
            };
            if !this.write_stdin(&data) {
                if this.stop_flag.load(Ordering::SeqCst) {
                    break;
                }
                this.restart();
            }
        });
    }

    /// 停止帧缓存：退出写线程、杀掉 ffmpeg、释放管道。
    /// 会话断开/重连时调用，防止专用线程与 ffmpeg 子进程泄漏。
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        {
            let mut slot = self.feed_slot.0.lock().unwrap();
            slot.take();
        }
        self.feed_slot.1.notify_all();
        let _ = self.ffmpeg.lock().take().map(|mut c| c.kill());
        *self.stdin.lock() = None;
    }

    /// 阻塞写 ffmpeg stdin（仅由专用写入线程调用；返回是否成功，
    /// 失败由调用方决定是否重启 ffmpeg）
    fn write_stdin(&self, data: &[u8]) -> bool {
        let mut w = self.stdin.lock();
        match w.as_mut() {
            Some(stdin) => stdin.write_all(data).is_ok(),
            None => false,
        }
    }

    pub fn latest_png(&self) -> Option<Vec<u8>> {
        self.latest.read().clone()
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

    fn restart(&self) {
        warn!("ffmpeg pipe broken, restarting");
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = self.ffmpeg.lock().take().map(|mut c| c.kill());
        *self.stdin.lock() = None;
        std::thread::sleep(Duration::from_millis(200));
        self.stop_flag.store(false, Ordering::SeqCst);
        if let Err(e) = self.spawn_ffmpeg() {
            warn!("ffmpeg restart failed: {}", e);
        }
    }
}
