//! WebRTC 服务端 peer：
//! - 把 scrcpy 的 H.264 帧打包成 RTP 通过 video track 推给浏览器（不转码，零画质损失）
//! - DataChannel "control" 接收浏览器的触控/按键/文本等控制消息，转发给 scrcpy 控制 socket
//!
//! 模块布局（OPTIMIZATION_PLAN §12.4）：本文件承载 pusher/RTP 推送与帧队列；
//! viewer 生命周期在 `viewer`，编码器诊断探针在 `probe`，RTP 线格式在 `protocol`，
//! ICE 候选外部宣告（容器/NAT 部署）在 `rtc_net`。

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use parking_lot::Mutex;
use tokio::sync::{broadcast, Notify};
use tracing::{debug, info, warn};

use crate::device::scrcpy::{AudioFrame, ScrcpySession, VideoFrame};

mod protocol;

mod probe;
mod rtc_net;
mod viewer;

pub use viewer::{
    remove_and_teardown_viewer, teardown_viewer, ViewerDisconnectReason, ViewerHandle, ViewerMap,
    ViewerSession,
};
// 兼容导出：crate 内当前仅 viewer 模块内部调用，保留 webrtc::take_viewer 公开路径
#[allow(unused_imports)]
pub use viewer::take_viewer;

use probe::{probe_encoder_blockiness, should_probe_encoder};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PusherDrainDecision {
    DrainAll,
    DrainToKeyframe {
        drop_prefix: usize,
        request_idr: bool,
    },
    Keep,
}

fn decide_pusher_drain(
    queue_len: usize,
    backlog_limit: usize,
    overflowed: bool,
    waiting_key: bool,
    keyframe_index: Option<usize>,
) -> PusherDrainDecision {
    if overflowed {
        return PusherDrainDecision::DrainAll;
    }
    if waiting_key {
        return match keyframe_index {
            Some(ki) => PusherDrainDecision::DrainToKeyframe {
                drop_prefix: ki,
                request_idr: false,
            },
            None => PusherDrainDecision::DrainAll,
        };
    }
    if queue_len > backlog_limit {
        return match keyframe_index {
            Some(0) => PusherDrainDecision::Keep,
            Some(ki) => PusherDrainDecision::DrainToKeyframe {
                drop_prefix: ki,
                request_idr: false,
            },
            None => PusherDrainDecision::DrainAll,
        };
    }
    PusherDrainDecision::Keep
}

use webrtc::interceptor::Attributes;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::rtp::codecs::h264::H264Payloader;
use webrtc::rtp::codecs::opus::OpusPayloader;
use webrtc::rtp::header::Header;
use webrtc::rtp::packet::Packet;
use webrtc::rtp::packetizer::Payloader;
use webrtc::rtp_transceiver::rtp_sender::RTCRtpSender;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PeerConnectionEffect {
    Connected,
    TemporarilyDisconnected,
    Terminal,
    Ignore,
}

/// 将 webrtc-rs 的连接状态压缩成 pusher/ws 真正关心的生命周期事件。
/// Disconnected 是可恢复的 ICE 抖动，Failed/Closed 才是终态；New/Connecting
/// 不覆盖当前状态，避免异步回调的迟到状态把已建立连接误标为未连接。
fn peer_connection_effect(state: RTCPeerConnectionState) -> PeerConnectionEffect {
    match state {
        RTCPeerConnectionState::Connected => PeerConnectionEffect::Connected,
        RTCPeerConnectionState::Disconnected => PeerConnectionEffect::TemporarilyDisconnected,
        RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => {
            PeerConnectionEffect::Terminal
        }
        _ => PeerConnectionEffect::Ignore,
    }
}
impl ViewerSession {
    /// 视频推流循环：从环形帧缓冲取帧，H.264 payload 化 → RTP 写 track
    ///
    /// 延迟控制（核心）：帧缓冲是"丢最旧保最新"的环形队列（见 make_frame_queue），
    /// 且这里按**帧数积压**剪裁——队列深度超过 ~1s 的帧数时，丢弃队首到最近关键帧
    /// 之间的旧帧，从关键帧重新开始（用帧数而非 PTS 时间差：设备编码器重启/虚拟屏
    /// 重建时 PTS 会整体跳变，时间差不可靠）。配合设备端 i-frame-interval=1s，
    /// 画面内容滞后被钳制在 ~1s 以内：写路径慢于设备出帧时，旧帧被跳过而不是排队
    /// 积压（旧 mpsc 实现满队列丢新帧，pusher 永远消费几秒前的旧帧 → 画面滞后 5s+
    /// → "操作延迟很久"的根因）。
    // 参数为推流循环依赖的全部资源与协商参数；pusher 状态机拆分
    //（OPTIMIZATION_PLAN 阶段 6）时再收敛为结构体
    #[allow(clippy::too_many_arguments)]
    fn spawn_pusher(
        &self,
        rtp_sender: Arc<RTCRtpSender>,
        frame_q: Arc<Mutex<VecDeque<VideoFrame>>>,
        frame_notify: Arc<Notify>,
        overflowed: Arc<std::sync::atomic::AtomicBool>,
        payload_type: u8,
        ssrc: u32,
        initial_frames: Option<Vec<VideoFrame>>,
        mut conn_rx: tokio::sync::mpsc::Receiver<()>,
        peer_connected: Arc<std::sync::atomic::AtomicBool>,
        idle_repeat_ms: u64,
        min_frame_interval_ms: u64,
        ffmpeg_path: String,
        probe_encoder: bool,
    ) {
        let track = self.track.clone();
        let running = self.running.clone();
        let last_serve = self.last_serve.clone();
        let ts_base = self.ts_base.clone();
        let last_ts = self.last_ts.clone();
        let config_nalu = self.config_nalu.clone();
        let session = self.session.clone();
        tokio::spawn(async move {
            // 等待 peer connected（DTLS/SRTP 就绪）再开始推流：
            // answer 已在 ws.rs 发给浏览器，浏览器开始 ICE/DTLS 握手；
            // SRTP session 建立前 webrtc-rs 的 write_rtp 静默返回 Ok(0) 丢包（黑屏根因）。
            // 最多等 10s，超时也继续（连接可能失败，pusher 会因 write 错误退出）。
            let _ = tokio::time::timeout(Duration::from_secs(10), conn_rx.recv()).await;
            // SRTP session 建立比 peer connected 略晚，再补一个小延迟
            tokio::time::sleep(Duration::from_millis(300)).await;
            if !running.load(std::sync::atomic::Ordering::SeqCst) {
                info!("pusher not started (connection closed before SRTP ready)");
                return;
            }
            info!("SRTP ready, starting pusher");
            let mut payloader: Box<dyn Payloader + Send + Sync> =
                Box::new(H264Payloader::default());
            let mut seq: u16 = rand::random();
            let mut last_rtp_ts = 0u32;
            let mut frame_no = 0u64;
            let mut last_pts: Option<u64> = None;
            // 最近成功发送的一帧（静止补帧用）
            let mut last_sent: Option<(VideoFrame, u32)> = None;
            // 真实时间锚点（(墙钟, 对应 RTP ts)）：媒体时钟的绝对基准，首帧建立。
            // 设备 PTS 快漂（实测 ~26%）时靠它把 ts 钳制在墙钟附近（见发送循环）
            let mut ts_anchor: Option<(std::time::Instant, u32)> = None;
            // 发送节奏（帧级 pacer）：固定 ~16.7ms（60fps 兜底上限）节奏发送。
            // 虚拟屏固定 60fps 编码（fps 配置不生效，见 scrcpy.rs），设备帧率仍有
            // 波动（静止低帧率 ↔ 运动 60fps、USB 批量到达）→ 无 pacer 时帧到达
            // 呈块状，浏览器 jitter buffer 目标延迟被顶到 ~300ms（实测 perF
            // 180~466ms 波动、渲染帧间隔 59% <10ms，见 AGENTS.md 已知坑）。
            // pacer：帧到早 → 等发送时刻；积压（生产 >60fps / 处理慢）→ 立即发
            // 并重置节奏追赶（backlog 跳帧兜底）。浏览器到达均匀 → target 收敛。
            let pacer_interval = Duration::from_millis(16);
            let mut next_tx_at = std::time::Instant::now();
            // 断链恢复中：丢 P 帧直到新的关键帧出现（overflow 断链 / 初始重放无 IDR 时置位）
            let mut waiting_key = false;

            // 初始 GOP 重放：config 帧喂给 payloader（缓存 SPS/PPS，后续 IDR 自动拼 STAP-A）；
            // GOP 帧按原帧节奏发送（ts 基于 GOP 首帧 PTS，与后续实时流同一时间轴）。
            // 浏览器收到第一个 IDR 即开始渲染，之后无缝追到实时。
            // 重放节流：GOP 可能很大（MTK 关键帧间隔 ~25s ≈ 750 帧），clamp(2,10)ms
            // 保证重放总时长 ≤ ~6s（旧 clamp(16,40) 会把大 GOP 放 25s+，连接后长时间
            // 停留在旧画面）。浏览器从重放首帧（IDR）起就有画面，短暂快进追平可接受。
            let mut replayed_had_key = false;
            if let Some(frames) = initial_frames {
                let total_bytes: usize = frames.iter().map(|f| f.data.len()).sum();
                // 重放可能整体 0 字节（SRTP session 实际未就绪时 webrtc-rs 的
                // write_rtp 静默返回 Ok(0)，实证：connected +300ms 后重放 109 帧
                // 仍全部 0 字节）。此时浏览器一帧都收不到，若直接进实时流，
                // waiting_key 未置位 → P 帧裸推 → 花屏。检测 written==0 →
                // 短暂等待重试；仍失败则请求编码器重置（RESET_VIDEO → 新
                // config+IDR，~200ms 到达）并置 waiting_key（丢 P 帧等 IDR）。
                let mut replay_ok = false;
                let mut replay_sent = 0usize;
                for attempt in 0..3 {
                    let mut base: Option<u64> = None;
                    let mut sent = 0usize;
                    let mut written_total = 0usize;
                    let mut ok = true;
                    for f in &frames {
                        if f.is_config {
                            let _ = payloader.payload(1200, &f.data);
                            // SPS/PPS 独立单 NALU 包直接发送（见 send_config_nalus 注释：
                            // H264Payloader 的 STAP-A 在 IDR slice 超限时会静默丢弃参数集）。
                            if !send_config_nalus(&track, &f.data, payload_type, ssrc, &mut seq, 0)
                                .await
                            {
                                ok = false;
                                break;
                            }
                            continue;
                        }
                        if f.is_keyframe {
                            replayed_had_key = true;
                        }
                        if base.is_none() {
                            base = Some(f.pts_us);
                        }
                        // 按原帧间隔节流发送，避免瞬时大流量打爆浏览器 jitter buffer / UDP 丢包；
                        // 下限 16ms（≈60fps）保证重放不超过源节奏的 2 倍——旧实现 clamp(5,40)
                        // 会把 1~2s 的 GOP 在 ~0.5s 内快放完，表现为连接后"画面突然加速"。
                        if let Some(lp) = last_pts {
                            let gap = f.pts_us.saturating_sub(lp);
                            let sleep_ms = (gap / 1000).clamp(2, 10);
                            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                        }
                        last_pts = Some(f.pts_us);
                        let ts = ((f.pts_us.saturating_sub(base.unwrap()) * 90) / 1000) as u32;
                        let (cont, w) =
                            push_rtp(&track, &mut payloader, f, payload_type, ssrc, &mut seq, ts)
                                .await;
                        written_total += w;
                        last_serve.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
                        if !cont {
                            ok = false;
                            break;
                        }
                        last_rtp_ts = ts;
                        last_sent = Some((f.clone(), ts));
                        sent += 1;
                    }
                    if let Some(b) = base {
                        *ts_base.lock() = Some(b);
                    }
                    if !ok {
                        // 硬失败（连接已断），pusher 即将退出，不再重试
                        break;
                    }
                    if written_total > 0 || frames.is_empty() {
                        replay_ok = true;
                        replay_sent = sent;
                        break;
                    }
                    // 全部 0 字节：SRTP 未就绪，等一会重试
                    info!(
                        "initial GOP replay wrote 0 bytes (SRTP not ready?), retrying {}/2",
                        attempt + 1
                    );
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                if replay_ok {
                    info!(
                        "pusher replayed initial GOP: {} frames, {} bytes",
                        replay_sent, total_bytes
                    );
                } else {
                    // 重放彻底失败（3 次全 0 字节）：请求编码器立即出 IDR，
                    // 等 IDR 期间丢 P 帧——浏览器保持黑屏而非花屏
                    waiting_key = true;
                    info!("initial GOP replay failed (0 bytes after retries), requesting reset_video, dropping P frames until IDR");
                    let _ = session.reset_video().await;
                }
                if !replayed_had_key {
                    // 初始帧里没有 IDR（reset_video 兜底超时路径，只剩 SPS/PPS）：
                    // 必须等实时流第一个 IDR 再推 P 帧——否则浏览器解码器用参数集
                    // 初始化后收到无参考的 P 帧，错误传播成花屏，直到 ~25s 后自然 IDR
                    // （MTK 忽略 i-frame-interval）才恢复。等 IDR 期间浏览器保持无画面
                    // （黑屏/定格），IDR 一到即干净出画。
                    waiting_key = true;
                    info!("no keyframe in initial frames, dropping P frames until first IDR");
                }
            }

            // 延迟控制（核心）：帧缓冲是"丢最旧保最新"的环形队列（见 make_frame_queue），
            // 这里按**帧数积压**剪裁——队列深度超过 ~1s 的帧数时，丢弃队首到最近关键帧
            // 之间的旧帧，从关键帧重新开始发送（用帧数而非 PTS 时间差：设备编码器重启/
            // 虚拟屏重建时 PTS 会整体跳变，时间差不可靠）。
            //   - 写路径跟得上设备帧率 → 队列始终很短（1~5 帧），零额外延迟，连续播放；
            //   - 写路径慢（CPU 竞争 / 网络抖动）→ 旧帧被跳过而不是排队，画面滞后
            //     被钳制在 ~1s 内且不会越积越多。旧 mpsc 实现满队列丢"新帧"保"旧帧"，
            //     pusher 永远消费几秒前的旧帧（画面滞后 5s+ → "操作延迟很久"的根因）。
            // 参考链保护（重要）：正常运转时队列里几乎全是 P 帧——关键帧已在上轮发出，
            // 这些 P 帧直接延续"已发送的帧链"，完全可解码，必须照常发送。绝不能因
            // "队内无关键帧"就整队丢弃（那会把流塌缩成每秒一个关键帧，画面每秒跳一下
            // ——曾因该误判导致推流塌缩，表现"更卡"）。真正断链只有一种情况：
            // 环形缓冲溢出（forwarder 丢过最旧帧），此时用 overflowed 标志通知，
            // pusher 清空队列等待下一个 IDR 重建（i-frame-interval=1 → ≤1s）。
            // ≈1s 的帧数。设备实际按 60fps 出帧（虚拟屏忽略 fps 配置，见
            // scrcpy.rs），只按配置 fps 换算会把阈值压到远小于 1s（fps=15 →
            // 15 帧 ≈ 250ms@60fps）：消费稍慢即触发跳帧清队。下限 60 帧
            //（60fps 下 1s）恢复本意，配置 fps 更高时取配置值
            //（min_frame_interval_ms=0 时按无配置处理，同样取下限 60）
            let backlog_limit = 1000u64
                .checked_div(min_frame_interval_ms)
                .map(|fps| fps.max(60) as usize)
                .unwrap_or(60);
            // 参数集切换窗口标志：config 帧（新 SPS/PPS）已到、新 IDR 未到。
            // 窗口内禁止静止补帧（见取帧/补帧处注释），IDR 发送时复位。
            let mut pending_config = false;
            let mut drops_broken = 0u64;
            let mut drops_to_key = 0u64;
            // 断链清队后主动请求 IDR（锁内置位，锁外 await——MutexGuard 不能跨
            // await 存活），限频 2s 防编码器重启风暴
            let mut need_idr = false;
            let mut last_idr_req = std::time::Instant::now();
            // 补帧压制限时跟踪（见 to_send.is_empty 分支）：压制窗口（参数集切换/
            // 断链等 IDR）超过 3s 仍无 IDR 时恢复补帧，防止浏览器断供被前端静默
            // 检测杀掉（MTK 静态屏 reset 后长时间不出 IDR 是常态）
            let mut suppress_started: Option<std::time::Instant> = None;
            let mut was_suppressed = false;
            // 诊断探针：每 300 帧输出平均单帧 RTP 发送耗时（不含节流 sleep）与队列深度
            let mut send_time_us = 0u64;
            let mut send_samples = 0u64;
            let mut last_q_len = 0usize;

            while running.load(std::sync::atomic::Ordering::SeqCst) {
                // 等待新帧（notify）或静止补帧超时（idle_repeat_ms）
                tokio::select! {
                    _ = frame_notify.notified() => {}
                    _ = tokio::time::sleep(Duration::from_millis(idle_repeat_ms)) => {}
                }
                if !running.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }

                // 锁内只做队列剪裁/取出（不 await）：取出"从最近关键帧起的连续帧链"
                let mut to_send: Vec<VideoFrame> = Vec::new();
                {
                    let mut q = frame_q.lock();
                    if !q.is_empty() {
                        // 参数集（SPS/PPS）帧：先于一切剪裁提取——只更新 config_nalu
                        // 并置 pending_config（新 IDR 到达前禁止静止补帧），不进入发送
                        // 列表（参数集只在 IDR 时经 H264Payloader 合成 STAP-A 下发）。
                        // 绝不能被 backlog 跳帧 / waiting_key 的 drain 丢掉：config 丢了，
                        // IDR 前重发的就是旧参数集，浏览器用旧 SPS 初始化新分辨率 IDR
                        // → 花屏。若 viewer 连接早于帧缓存首帧（会话刚建立的空窗期），
                        // 此处也是浏览器拿到 SPS/PPS 的唯一机会——错过则永久黑屏。
                        let mut i = 0;
                        while i < q.len() {
                            if q[i].is_config {
                                if let Some(cf) = q.remove(i) {
                                    *config_nalu.lock() = Some(Bytes::from(cf.data));
                                    pending_config = true;
                                }
                            } else {
                                i += 1;
                            }
                        }
                        let keyframe_index = q.iter().position(|f| f.is_keyframe);
                        let mut decision =
                            if overflowed.swap(false, std::sync::atomic::Ordering::SeqCst) {
                                PusherDrainDecision::DrainAll
                            } else {
                                decide_pusher_drain(
                                    q.len(),
                                    backlog_limit,
                                    false,
                                    waiting_key,
                                    keyframe_index,
                                )
                            };
                        if matches!(decision, PusherDrainDecision::DrainAll)
                            && keyframe_index.is_some()
                            && !waiting_key
                            && q.len() > backlog_limit
                        {
                            decision = PusherDrainDecision::DrainToKeyframe {
                                drop_prefix: keyframe_index.unwrap_or(0),
                                request_idr: false,
                            };
                        }
                        match decision {
                            PusherDrainDecision::DrainAll => {
                                if waiting_key
                                    || keyframe_index.is_none()
                                    || q.len() > backlog_limit
                                {
                                    drops_broken += 1;
                                    if drops_broken % 20 == 1 {
                                        info!("pusher backlog without keyframe in queue, dropped {} frames (reference chain broken), drops={}", q.len(), drops_broken);
                                    }
                                }
                                // OBS-003：积压跳帧/断链清队的整批丢帧计数
                                crate::metrics::global().record_rtp_drops(q.len() as u64);
                                q.clear();
                                waiting_key = true;
                                need_idr = true;
                            }
                            PusherDrainDecision::DrainToKeyframe {
                                drop_prefix,
                                request_idr,
                            } => {
                                if drop_prefix > 0 {
                                    drops_to_key += 1;
                                    if drops_to_key % 20 == 1 {
                                        info!("pusher skipped {} stale frames to keyframe (queue {}), skips={}", drop_prefix, q.len(), drops_to_key);
                                    }
                                    crate::metrics::global().record_rtp_drops(drop_prefix as u64);
                                    q.drain(..drop_prefix);
                                }
                                waiting_key = false;
                                if request_idr {
                                    need_idr = true;
                                }
                            }
                            PusherDrainDecision::Keep => {}
                        }
                        if !waiting_key || !q.is_empty() {
                            to_send = q.drain(..).collect();
                            last_q_len = to_send.len(); // 本轮取出的帧数（≈ 队列积压深度）
                        }
                    }
                }

                // 断链清队后主动向编码器要 IDR（RESET_VIDEO，~200ms 出新 config+IDR），
                // 而不是干等自然 IDR（i-frame-interval=2s → 平均冻结 ~1s、最高 2s，
                // 游戏高码率下发送饱和时每几秒一次 → "投屏延迟高"的直接体感来源）。
                // 与前端 PLI 触发的 reset_video 同一链路；waiting_key 会丢到 IDR 前的
                // P 帧，浏览器定格 ≤200ms 后干净恢复
                if need_idr {
                    need_idr = false;
                    if last_idr_req.elapsed() >= Duration::from_secs(2) {
                        last_idr_req = std::time::Instant::now();
                        info!("chain broken without keyframe, requesting IDR via reset_video");
                        let _ = session.reset_video().await;
                    }
                }

                // 压制窗口跟踪：进入压制（pending_config/waiting_key 置位）记起点
                {
                    let supp = pending_config || waiting_key;
                    if supp && !was_suppressed {
                        suppress_started = Some(std::time::Instant::now());
                    } else if !supp {
                        suppress_started = None;
                    }
                    was_suppressed = supp;
                }

                if to_send.is_empty() {
                    // 参数集切换窗口（config 已到、新 IDR 未到，典型触发：点击投屏画面
                    // 后游戏切分辨率/编码器重启）：禁止重发旧帧。H264Payloader 对关键帧/
                    // 非关键帧一视同仁——只要缓存了 SPS/PPS，下一个 NALU 就会被合成
                    // STAP-A 下发；此时重发旧分辨率帧 = "新参数集 + 旧帧"，浏览器用新
                    // 参数初始化解码器后再解旧帧 → 解码器失步 → 画面慢慢浮现黑白/彩色
                    // 块点、卡顿（见 AGENTS.md 已知坑）。跳过补帧 → 画面定格在最后一
                    // 帧，新 IDR 一到即干净恢复（通常 ≤1s，远低于前端 ~4s 静默检测）。
                    // waiting_key（断链/无 IDR 起点）同理：等新 IDR 重建后再补帧。
                    // **限时 3s**：MTK 静态屏对 reset_video 响应极慢（实测要多次 reset、
                    // 最长 6s+ 才吐 config+IDR，甚至不吐），无限期压制 = 浏览器断供
                    // → 前端 4s 静默检测杀连接 → 重连 → PLI → 再 reset → 死循环
                    // （"连上一会儿就断"）。超时后恢复补帧是安全的：last_sent 属于
                    // 已成功发送的旧参考链，payloader 缓存的仍是旧参数集（新参数集
                    // 只在 IDR 时喂入），旧帧 + 旧参数自洽可解码
                    if (pending_config || waiting_key)
                        && suppress_started.is_some_and(|t| t.elapsed() < Duration::from_secs(3))
                    {
                        continue;
                    }
                    // 静止补帧：idle_repeat_ms 内无新帧 → 重发最后一帧。
                    // **注意**：Chrome 会静默丢弃重复 P 帧（相同 frame_num 的 slice
                    // 被视为冗余副本，不解码、currentTime 不推进）——补帧的作用是
                    // 维持**链路活性**（字节持续到达 → 前端静默检测/码率/RTCP 正常），
                    // 不是维持解码帧率；静态屏画面定格是正确渲染。正因如此前端静默
                    // 检测是双条件（currentTime 冻结 && 零新增字节），且补帧保持
                    // P 帧形态（不换 IDR 重复）：唤醒后新 P 帧直接续参考链，无花屏。
                    // 重复帧内容相同且参考链完整（该帧此前已成功发送），解码器可正常渲染；
                    // RTP 时间戳单调推进，浏览器帧率统计不掉。
                    // ts 与实时帧同源（墙钟锚点 + 单调保底）：旧实现固定步进
                    // (ts0 + idle_repeat*90) 与墙钟有速率差，与实时帧交替时 ts 跳变，
                    // 浏览器 jitter buffer 目标延迟被扰动（见 AGENTS.md 已知坑）。
                    if peer_connected.load(std::sync::atomic::Ordering::SeqCst) {
                        if let Some((last, _)) = &last_sent {
                            let ts = {
                                let real_ts = match ts_anchor {
                                    Some((aw, at)) => at.wrapping_add(
                                        ((aw.elapsed().as_micros() as u64) * 90 / 1000) as u32,
                                    ),
                                    None => {
                                        ts_anchor = Some((std::time::Instant::now(), last_rtp_ts));
                                        last_rtp_ts
                                    }
                                };
                                if real_ts <= last_rtp_ts && frame_no > 1 {
                                    ts_anchor = Some((std::time::Instant::now(), last_rtp_ts));
                                    last_rtp_ts + 3000
                                } else {
                                    real_ts
                                }
                            };
                            let (cont, _w) = push_rtp(
                                &track,
                                &mut payloader,
                                last,
                                payload_type,
                                ssrc,
                                &mut seq,
                                ts,
                            )
                            .await;
                            if !cont {
                                break;
                            }
                            last_serve.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
                            *last_ts.lock() = ts;
                            last_rtp_ts = ts;
                        }
                    }
                    continue;
                }

                // 一次取到多帧说明消费慢于输入：统一走 pacer 限速追赶
                // 注意：to_send 在 for 循环中被移动，paced 必须在循环外计算
                for frame in to_send {
                    frame_no += 1;
                    // 诊断：实时循环心跳（验证帧缓冲是否收到设备帧）
                    if frame_no % 300 == 1 {
                        debug!(
                            "pusher recv: no={} key={} cfg={} pts={} peer={}",
                            frame_no,
                            frame.is_keyframe,
                            frame.is_config,
                            frame.pts_us,
                            peer_connected.load(std::sync::atomic::Ordering::SeqCst)
                        );
                    }

                    // 帧级 pacer：等待本帧发送时刻。
                    // - 单帧正常节奏：帧以 ~60fps 到达、next_tx_at 已到 → 零等待，
                    //   到达节奏 = 设备节奏（均匀）；旧 min_interval(33ms) 节流与
                    //   设备 60fps 生产不匹配（虚拟屏 fps 配置不生效），单帧 30fps
                    //   与批量全速交替 → 块状到达。
                    // - 积压（消费慢于输入）：next_tx_at 落后 → 立即发并重置节奏
                    //   （见下方重置），追赶快于生产 2 倍以上，不会永久滞后；
                    //   旧"批量全速连发"（~1ms/帧）是块状到达的直接来源。
                    // 定时器精度由 main.rs 的 timeBeginPeriod(1) 保证（见该处注释）。
                    if let Some(remain) =
                        next_tx_at.checked_duration_since(std::time::Instant::now())
                    {
                        tokio::time::sleep(remain).await;
                    }

                    // RTP 时间戳：90kHz，**媒体时钟 == 墙钟**（真实时间锚点 + 墙钟流逝）。
                    // 设备 PTS（MTK 虚拟屏编码器）实测与墙钟严重不同步：快时可领先 ~26%，
                    // 且偶发停滞/回退/30fps 节奏（33ms 间隔）。若 ts 跟随 PTS，浏览器
                    // jitter buffer 认为帧"早到/迟到"，目标延迟被拉高到数秒 → 缓冲
                    // 积压满后整段丢弃 → 只有关键帧可解码 → 画面掉到 1fps 并频繁冻结。
                    // 首帧以当前已发送时间轴锚定（与初始 GOP 重放无缝衔接），此后 ts
                    // 只按墙钟推进：帧以 ~60fps 到达、ts 以同节奏前进 → 浏览器 1.0x
                    // 平滑播放，jitter buffer 目标收敛到最小值（实测 ~150ms）。
                    // 单调保底（real_ts ≤ last_rtp_ts，正常不会触发）：重新锚定到当前
                    // ts，避免 +3000 自持循环把媒体时钟推到 2 倍墙钟速率。
                    let ts = {
                        let real_ts = match ts_anchor {
                            Some((aw, at)) => at.wrapping_add(
                                ((aw.elapsed().as_micros() as u64) * 90 / 1000) as u32,
                            ),
                            None => {
                                ts_anchor = Some((std::time::Instant::now(), last_rtp_ts));
                                last_rtp_ts
                            }
                        };
                        let ts = if real_ts <= last_rtp_ts && frame_no > 1 {
                            ts_anchor = Some((std::time::Instant::now(), last_rtp_ts));
                            last_rtp_ts + 3000
                        } else {
                            real_ts
                        };
                        last_rtp_ts = ts;
                        ts
                    };
                    *last_ts.lock() = ts;

                    // ICE 抖动（Disconnected）期间跳过发送，等待恢复；连接恢复后继续推
                    if !peer_connected.load(std::sync::atomic::Ordering::SeqCst) {
                        debug!("peer disconnected, skipping frame {}", frame_no);
                        // 任何被跳过的帧都切断参考链：P 帧的后继帧引用它，关键帧
                        // 丢失则后继 P 帧失去修复点。恢复后必须丢到下一个 IDR，
                        // 否则浏览器用断裂参考链解码——花屏且无 PLI/NACK 信号
                        // （协议层无丢包，解码器不知情），只能等自然 IDR
                        waiting_key = true;
                        continue;
                    }

                    // 关键帧（IDR）：参数集切换窗口结束（复位 pending_config，恢复静止
                    // 补帧许可），并把最近一次 SPS/PPS（取帧阶段维护的 config_nalu）发
                    // 出去——**独立单 NALU 包**（send_config_nalus）保证参数集必定到达：
                    // H264Payloader 的 STAP-A 打包在总长 > mtu（1200B）时**静默丢弃整包**
                    // （含 SPS/PPS），而 IDR slice 通常几十 KB（首 NALU 即超限），只有
                    // IDR 帧恰好以小 SEI 开头时参数集才侥幸发出——"切分辨率/编码器重启
                    // 后偶发花屏直到下个自然 IDR（~25s）"的直接机制。参数集包与 IDR 同
                    // ts、marker=false → 浏览器 jitter buffer 视为同一帧，FFmpeg 先解析
                    // 参数集再解 IDR slice，干净出画。同时仍喂 payloader（STAP-A 冗余，
                    // 无害）。config 帧本身不进发送列表，静止补帧重发的旧帧因此永远不
                    // 会带新参数集前缀。
                    if frame.is_keyframe {
                        pending_config = false;
                        // 先 clone 出锁内容再 await（MutexGuard 非 Send，不能跨 await 存活）
                        let cfg = config_nalu.lock().clone();
                        if let Some(cfg) = cfg {
                            if !send_config_nalus(&track, &cfg, payload_type, ssrc, &mut seq, ts)
                                .await
                            {
                                break;
                            }
                            let _ = payloader.payload(1200, &cfg);
                        }
                    }

                    let t_send = std::time::Instant::now();
                    // 编码器输出质量探针（转场块效应定位）：抽样帧（关键帧全查 + P 帧
                    // 1/30）用 ffmpeg 解码原始 H.264 → 宏块网格块效应检测。报 >1.25 说明
                    // **编码器输出帧本身有块效应**（浏览器/传输无辜）；不报说明编码器
                    // 干净、块效应在浏览器解码路径（jitter buffer/丢帧）。
                    // 默认关闭（config probe_encoder）：60fps 下 ~2.5 进程/秒的阻塞
                    // ffmpeg 抢 tokio worker，实测把单帧 RTP 发送耗时推高 3~4 倍
                    // （send_avg 6→20ms），发送饱和 → 积压断链 → 周期性冻结
                    if should_probe_encoder(probe_encoder, frame_no, frame.is_keyframe) {
                        let cfg = config_nalu.lock().clone();
                        let fdata = frame.data.clone();
                        let ff = ffmpeg_path.clone();
                        let fn_ = frame_no;
                        let fk = frame.is_keyframe;
                        let fs = frame.data.len();
                        tokio::spawn(async move {
                            probe_encoder_blockiness(&ff, cfg, &fdata, fn_, fk, fs);
                        });
                    }
                    let (cont, _w) = push_rtp(
                        &track,
                        &mut payloader,
                        &frame,
                        payload_type,
                        ssrc,
                        &mut seq,
                        ts,
                    )
                    .await;
                    if !cont {
                        break;
                    }
                    last_serve.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
                    // pacer：推进发送时刻；积压（落后 >20ms ≈ 超过 1 个 pacer 周期，
                    // 如关键帧 11ms 平滑耗时）→ 重置节奏立即发下一帧，避免积压被
                    // 缓慢摊开（backlog 跳帧仍兜底内容滞后）
                    next_tx_at += pacer_interval;
                    if next_tx_at < std::time::Instant::now() - Duration::from_millis(20) {
                        next_tx_at = std::time::Instant::now();
                    }
                    send_time_us += t_send.elapsed().as_micros() as u64;
                    send_samples += 1;
                    // 诊断：实时推帧日志（每 300 帧，含平均 RTP 发送耗时与队列深度）
                    if frame_no % 300 == 1 {
                        let avg = send_time_us.checked_div(send_samples).unwrap_or(0) / 1000;
                        info!("pusher live: frame_no={} key={} ts={} peer={} size={} send_avg={}ms q={}", frame_no, frame.is_keyframe, ts, peer_connected.load(std::sync::atomic::Ordering::SeqCst), frame.data.len(), avg, last_q_len);
                        send_time_us = 0;
                        send_samples = 0;
                    }
                    last_sent = Some((frame, ts));
                }
            }
            let _ = rtp_sender.stop().await;
            info!("pusher stopped");
        });
    }

    /// 音频推流循环：OPUS 帧（~20ms/帧）→ 每帧一个 RTP 包（48kHz 时间戳，960 ticks/帧）
    fn spawn_audio_pusher(
        &self,
        audio_track: Arc<TrackLocalStaticRTP>,
        mut audio_rx: tokio::sync::mpsc::Receiver<AudioFrame>,
        payload_type: u8,
        ssrc: u32,
        peer_connected: Arc<std::sync::atomic::AtomicBool>,
        audio_on: Arc<std::sync::atomic::AtomicBool>,
    ) {
        let running = self.running.clone();
        tokio::spawn(async move {
            let mut payloader: Box<dyn Payloader + Send + Sync> = Box::new(OpusPayloader);
            let mut seq: u16 = rand::random();
            let mut last_ts: Option<u32> = None;
            let mut sent: u64 = 0;
            // 音频真实时间锚点：与视频同理，设备音频 PTS 也可能快漂（虚拟屏
            // remote_submix 时钟），若 ts 跟着漂，浏览器音频 jitter buffer 目标
            // 被拉高 → A/V 同步把整个画面拖慢数秒。钳制在 [real, real+40ms]。
            let mut audio_anchor: Option<(std::time::Instant, u32)> = None;
            while running.load(std::sync::atomic::Ordering::SeqCst) {
                let Some(frame) = audio_rx.recv().await else {
                    break;
                };
                // OPUS 参数集帧（OpusHead）无需发送：WebRTC 用 SDP fmtp 描述参数
                if frame.is_config {
                    continue;
                }
                // viewer 未请求音频：丢弃但保持排空（audio channel 满了会背压
                // 阻塞 scrcpy 音频读取任务）
                if !audio_on.load(std::sync::atomic::Ordering::SeqCst) {
                    continue;
                }
                if !peer_connected.load(std::sync::atomic::Ordering::SeqCst) {
                    continue;
                }
                // 48kHz 时间戳：pts_us → ticks（×48/1000）；锚定真实时间；单调保底 +20ms
                let ts = {
                    let src_ts = ((frame.pts_us.saturating_mul(48)) / 1000) as u32;
                    let real_ts = match audio_anchor {
                        Some((aw, at)) => {
                            at.wrapping_add(((aw.elapsed().as_micros() as u64) * 48 / 1000) as u32)
                        }
                        None => {
                            audio_anchor = Some((std::time::Instant::now(), src_ts));
                            src_ts
                        }
                    };
                    let ts = src_ts.max(real_ts).min(real_ts.saturating_add(2 * 960)); // 领先 ≤40ms
                    match last_ts {
                        Some(lt) if ts <= lt => lt + 960,
                        _ => ts,
                    }
                };
                last_ts = Some(ts);
                let payloads = match payloader.payload(1200, &Bytes::from(frame.data.clone())) {
                    Ok(p) => p,
                    Err(e) => {
                        debug!("audio payload error: {}", e);
                        continue;
                    }
                };
                let frame_size = frame.data.len();
                let n = payloads.len();
                for (i, payload) in payloads.into_iter().enumerate() {
                    let pkt = Packet {
                        header: Header {
                            version: 2,
                            padding: false,
                            extension: false,
                            marker: i == n - 1,
                            payload_type,
                            sequence_number: seq,
                            timestamp: ts,
                            ssrc,
                            ..Default::default()
                        },
                        payload,
                    };
                    seq = seq.wrapping_add(1);
                    match tokio::time::timeout(
                        Duration::from_millis(3000),
                        audio_track.write_rtp_with_extensions_attributes(
                            &pkt,
                            &[],
                            &Attributes::new(),
                        ),
                    )
                    .await
                    {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => {
                            debug!("audio write_rtp error: {}", e);
                            break;
                        }
                        Err(_) => {
                            warn!("audio write_rtp timed out, stopping audio pusher");
                            break;
                        }
                    }
                }
                sent += 1;
                if sent % 1000 == 1 || sent <= 3 {
                    info!("audio pusher: sent={} ts={} size={}", sent, ts, frame_size);
                }
            }
            info!("audio pusher stopped");
        });
    }
}

/// 当前 unix 毫秒（ViewerHandle.last_serve 用）
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 把一帧 H.264 打成 RTP 包写入 track。
/// 返回 (是否继续推流, 实际写入字节数)：写失败 = 连接已断（false）；
/// 写入 0 字节 = SRTP 未就绪时 webrtc-rs 静默返回 Ok(0)（连接初期窗口，重放逻辑
/// 据此检测并重试，见 spawn_pusher）。
async fn push_rtp(
    track: &Arc<TrackLocalStaticRTP>,
    payloader: &mut Box<dyn Payloader + Send + Sync>,
    frame: &VideoFrame,
    payload_type: u8,
    ssrc: u32,
    seq: &mut u16,
    ts: u32,
) -> (bool, usize) {
    let payloads = match payloader.payload(1200, &frame.data) {
        Ok(p) => p,
        Err(e) => {
            debug!("payload error: {}", e);
            record_rtp_outcome(crate::metrics::global(), 0);
            return (true, 0);
        }
    };
    // 诊断探针（关键帧全查 + P 帧抽样 1/30，开销可忽略）：把本帧 RTP payloads 重组为
    // Annex-B 并与原始帧逐 NALU 比对。一致 ⇒ 服务端打包无损，花屏在浏览器解码侧；
    // 不一致 ⇒ 打包路径损毁数据（花屏根因），日志精确到第几个 NALU。
    // 注：关键帧 ~25s 才一个（MTK 编码器忽略 i-frame-interval=2），若坏帧在 P 帧，
    // 只查关键帧会漏——P 帧按 seq 抽样。
    let mut probe = frame.is_keyframe;
    if !probe {
        probe = (*seq).is_multiple_of(30);
    }
    if probe {
        verify_rtp_rebuild(frame, &payloads);
    }
    let n = payloads.len();
    // 关键帧发送平滑（pacer 简化版）：设备 i-frame-interval=1s，每秒产一个
    // ~200KB 关键帧（170+ 个 RTP 包）。全部瞬时写入会形成 burst（~3ms 发完），
    // 浏览器 jitter buffer 目标延迟被周期性拉高（见 AGENTS.md 已知坑）。
    // 中间一次 8ms sleep 分批发：总耗时 ~11ms，**必须小于帧级 pacer 间隔
    // (16ms)**——旧实现每 8 包 sleep 1ms（170 包 ~21ms）超过 pacer 周期，
    // 每秒净积压 ~1 帧，延迟缓慢爬升。8ms 摊开 burst 因子 ~4 倍，足够平滑。
    let smooth = n > 48; // >~57KB 视为关键帧（P 帧通常 ≤20 包）
    let mut written = 0usize;
    for (i, payload) in payloads.into_iter().enumerate() {
        if smooth && i == n / 2 {
            tokio::time::sleep(Duration::from_millis(8)).await;
        }
        let pkt = Packet {
            header: Header {
                version: 2,
                padding: false,
                extension: false,
                marker: i == n - 1, // 帧尾标记
                payload_type,
                sequence_number: *seq,
                timestamp: ts,
                ssrc,
                ..Default::default()
            },
            payload,
        };
        // 写 RTP 加 3s 超时兜底：若底层传输卡死（异常情况下 webrtc-rs 的
        // write 可能一直不返回），超时后放弃该连接，避免 pusher 永久挂起
        match tokio::time::timeout(
            Duration::from_millis(3000),
            track.write_rtp_with_extensions_attributes(&pkt, &[], &Attributes::new()),
        )
        .await
        {
            Ok(Ok(m)) => written += m,
            Ok(Err(e)) => {
                debug!("write_rtp error: {}", e);
                record_rtp_outcome(crate::metrics::global(), written);
                return (false, written);
            }
            Err(_) => {
                warn!("write_rtp timed out (3s), connection stalled, stopping pusher");
                record_rtp_outcome(crate::metrics::global(), written);
                return (false, written);
            }
        }
        *seq = seq.wrapping_add(1);
    }
    record_rtp_outcome(crate::metrics::global(), written);
    if written == 0 && n > 0 {
        // SRTP 未就绪时 webrtc-rs 静默返回 Ok(0) 丢弃整包——连接初期窗口
        debug!(
            "rtp write returned 0 bytes (SRTP not ready?), frame {} bytes, {} packets",
            frame.data.len(),
            n
        );
    }
    (true, written)
}

/// RTP 单帧发送结果采集（OBS-003，旁路）：实际写入 >0 字节记发送，
/// 0 字节（SRTP 未就绪静默丢弃 / payload 失败）记整帧丢弃。
/// 独立函数便于对采集语义做无网络单测。
fn record_rtp_outcome(metrics: &crate::metrics::Metrics, written_bytes: usize) {
    if written_bytes > 0 {
        metrics.record_rtp_sent_frame();
    } else {
        metrics.record_rtp_drop();
    }
}

/// 把 SPS/PPS 配置帧作为**独立单 NALU RTP 包**发送（RFC 6184 允许 type 7/8 单包）。
///
/// 为什么必须独立发送：H264Payloader 的 STAP-A 打包只在 `stap_a_nalu.len() <= mtu`
/// 时才 push，超限时**静默丢弃整个 STAP-A（含 SPS/PPS）并清空缓存**（rtp-0.13.0
/// codecs/h264/mod.rs）。而 IDR slice 通常几十 KB（MTK 单 slice，日志实测 85~92KB），
/// 首个 NALU 就远超 1200B——除非 IDR 帧恰好以小 SEI 开头，SPS/PPS 永远到不了浏览器。
/// 后果：切分辨率/编码器重启后浏览器用旧参数集解码新流 → 解码失败 → 花屏，直到
/// 下一个"侥幸带小 SEI 前缀"的 IDR（MTK 忽略 i-frame-interval，间隔 ~25s）——
/// "点击后偶发花屏、卡顿、非必现"的直接机制。
///
/// 参数集包与后续 IDR 帧**同 ts、marker=false**：浏览器 jitter buffer 把 [SPS][PPS]
/// [IDR 包们] 视为同一帧，FFmpeg 先解析参数集再解 IDR slice，干净初始化。
async fn send_config_nalus(
    track: &Arc<TrackLocalStaticRTP>,
    cfg: &Bytes,
    payload_type: u8,
    ssrc: u32,
    seq: &mut u16,
    ts: u32,
) -> bool {
    // Annex-B start code 切分 NALU
    let d = cfg.as_ref();
    let nals = protocol::annexb_nalus(d);
    if nals.is_empty() {
        return true;
    }
    let mut sent_any = false;
    for nal in &nals {
        let t = nal[0] & 0x1F;
        if t != 7 && t != 8 {
            continue; // 只发 SPS/PPS
        }
        if nal.len() > 1200 {
            continue; // 理论不会发生（SPS/PPS 通常 ~几十字节）
        }
        let pkt = Packet {
            header: Header {
                version: 2,
                padding: false,
                extension: false,
                marker: false, // 与后续 IDR 同帧组（同 ts）
                payload_type,
                sequence_number: *seq,
                timestamp: ts,
                ssrc,
                ..Default::default()
            },
            payload: Bytes::copy_from_slice(nal),
        };
        match tokio::time::timeout(
            Duration::from_millis(3000),
            track.write_rtp_with_extensions_attributes(&pkt, &[], &Attributes::new()),
        )
        .await
        {
            Ok(Ok(_)) => sent_any = true,
            Ok(Err(e)) => {
                debug!("write_rtp error sending SPS/PPS: {}", e);
                return false;
            }
            Err(_) => {
                warn!("write_rtp timed out sending SPS/PPS, connection stalled");
                return false;
            }
        }
        *seq = seq.wrapping_add(1);
    }
    if sent_any {
        info!(
            "config SPS/PPS sent as single NALUs: {} nalu(s), {} bytes, ts={}",
            nals.len(),
            d.len(),
            ts
        );
    }
    true
}

/// 诊断：把一帧的 RTP payloads 重组为 Annex-B NALU 序列，与原始帧逐 NALU 比对。
/// 忽略差异：STAP-A 注入的 SPS/PPS（type 7/8，IDR 前由 config_nalu 喂入）与
/// 被 payloader 丢弃的 AUD/FILLER（type 9/12）。不一致 → warn 定位（打包路径损毁数据）。
fn verify_rtp_rebuild(frame: &VideoFrame, payloads: &[Bytes]) {
    // 1. payloads → NALU 列表（STAP-A 拆包 / FU-A 拼接 / 单包直出）
    let mut nals: Vec<(u8, Bytes)> = Vec::new();
    for p in payloads {
        if p.is_empty() {
            continue;
        }
        let t = p[0] & 0x1F;
        match t {
            24 => {
                // STAP-A：拆出各 NALU
                let mut off = 1usize;
                while off + 2 <= p.len() {
                    let len = ((p[off] as usize) << 8) | p[off + 1] as usize;
                    off += 2;
                    if off + len > p.len() {
                        break;
                    }
                    let n = p.slice(off..off + len);
                    if !n.is_empty() {
                        nals.push((n[0] & 0x1F, n));
                    }
                    off += len;
                }
            }
            28 | 29 => {
                // FU-A/FU-B：按 S/E 位拼接
                let start = p[1] & 0x80 != 0;
                let typ = p[1] & 0x1F;
                let data = p.slice(2..);
                if start {
                    let nri = p[0] & 0x60;
                    let mut nal = Vec::with_capacity(data.len() + 1);
                    nal.push(nri | typ);
                    nal.extend_from_slice(&data);
                    nals.push((typ, Bytes::from(nal)));
                } else if let Some((_, last)) = nals.last_mut() {
                    let mut merged = last.to_vec();
                    merged.extend_from_slice(&data);
                    *last = Bytes::from(merged);
                }
            }
            1..=23 => {
                nals.push((t, p.clone()));
            }
            _ => {}
        }
    }
    // 2. 解析原始帧（Annex-B start code 切分）
    let mut orig: Vec<(u8, Vec<u8>)> = Vec::new();
    let d = &frame.data;
    let mut pos = 0usize;
    while pos < d.len() {
        let sc_len = if pos + 4 <= d.len() && d[pos..pos + 4] == [0, 0, 0, 1] {
            4
        } else if pos + 3 <= d.len() && d[pos..pos + 3] == [0, 0, 1] {
            3
        } else {
            0
        };
        if sc_len == 0 {
            pos += 1;
            continue;
        }
        let ns = pos + sc_len;
        let mut ne = d.len();
        let mut zc = 0usize;
        for (i, b) in d.iter().enumerate().skip(ns) {
            if *b == 0 {
                zc += 1;
                continue;
            }
            if *b == 1 && zc >= 2 {
                ne = i - zc;
                break;
            }
            zc = 0;
        }
        if ne > ns {
            let n = &d[ns..ne];
            if !n.is_empty() {
                let t = n[0] & 0x1F;
                if t != 9 && t != 12 {
                    orig.push((t, n.to_vec()));
                }
            }
        }
        pos = ne;
    }
    // 3. 比对（过滤 SPS/PPS 与 AUD/FILLER 后逐 NALU 数据一致）
    let a: Vec<&(u8, Vec<u8>)> = orig.iter().filter(|(t, _)| *t != 7 && *t != 8).collect();
    let b: Vec<&(u8, Bytes)> = nals.iter().filter(|(t, _)| *t != 7 && *t != 8).collect();
    if a.len() != b.len() {
        warn!(
            "RTP rebuild MISMATCH (frame {}B): orig {} NALU vs rebuilt {} NALU",
            frame.data.len(),
            a.len(),
            b.len()
        );
        return;
    }
    for i in 0..a.len() {
        if a[i].1.as_slice() != b[i].1.as_ref() {
            warn!(
                "RTP rebuild MISMATCH at NALU {}: type {} orig {}B vs rebuilt {}B",
                i,
                a[i].0,
                a[i].1.len(),
                b[i].1.len()
            );
            return;
        }
    }
}

/// DataChannel 控制消息协议（JSON）
/// { "type": "tap"|"swipe"|"key"|"touch"|"press"|"text"|"scroll"|"clipboard"|"start_app"|"rotate"|"back", ... }
/// viewer 级消息：{"type":"reset_video"}（请求 IDR）、{"type":"audio","on":bool}（音频转发开关）
pub(crate) enum ControlCommand {
    Data(Vec<u8>),
    ReleaseTouches {
        done: Option<tokio::sync::oneshot::Sender<()>>,
    },
}

/// 一个 viewer 的活动触点。状态只由该 viewer 的控制队列消费者访问，
/// teardown 通过同一队列追加 ReleaseTouches，确保触控事件仍按接收顺序写入设备。
#[derive(Default)]
pub(crate) struct TouchState {
    active: Mutex<BTreeMap<u64, (f32, f32)>>,
}

impl TouchState {
    fn contains(&self, pointer_id: u64) -> bool {
        self.active.lock().contains_key(&pointer_id)
    }

    fn insert(&self, pointer_id: u64, x: f32, y: f32) -> anyhow::Result<()> {
        let mut active = self.active.lock();
        if active.insert(pointer_id, (x, y)).is_some() {
            anyhow::bail!("touch pointer {} is already active", pointer_id);
        }
        Ok(())
    }

    fn update(&self, pointer_id: u64, x: f32, y: f32) -> anyhow::Result<()> {
        let mut active = self.active.lock();
        let Some(position) = active.get_mut(&pointer_id) else {
            anyhow::bail!("touch pointer {} is not active", pointer_id);
        };
        *position = (x, y);
        Ok(())
    }

    fn remove(&self, pointer_id: u64) -> anyhow::Result<()> {
        if self.active.lock().remove(&pointer_id).is_none() {
            anyhow::bail!("touch pointer {} is not active", pointer_id);
        }
        Ok(())
    }

    /// Return one still-active pointer after a release.  A few games stop
    /// treating the remaining finger as held after an ACTION_POINTER_UP until
    /// they receive a follow-up MOVE, so the control path uses this as a
    /// re-anchor point for the remaining touch set.
    fn first_active(&self) -> Option<(u64, f32, f32)> {
        self.active
            .lock()
            .iter()
            .next()
            .map(|(&pointer_id, &(x, y))| (pointer_id, x, y))
    }

    fn take_all(&self) -> Vec<(u64, f32, f32)> {
        std::mem::take(&mut *self.active.lock())
            .into_iter()
            .map(|(pointer_id, (x, y))| (pointer_id, x, y))
            .collect()
    }
}

fn touch_fields(msg: &serde_json::Value) -> anyhow::Result<(u8, u64, f32, f32)> {
    let action = match msg.get("action").and_then(serde_json::Value::as_str) {
        Some("down") => crate::device::scrcpy::ACTION_DOWN,
        Some("move") => crate::device::scrcpy::ACTION_MOVE,
        Some("up") => crate::device::scrcpy::ACTION_UP,
        _ => anyhow::bail!("invalid touch action"),
    };
    let pointer_id = msg
        .get("pointer_id")
        .and_then(serde_json::Value::as_u64)
        .filter(|pointer_id| *pointer_id <= 31)
        .ok_or_else(|| anyhow::anyhow!("invalid touch pointer_id"))?;

    fn coordinate(msg: &serde_json::Value, name: &str) -> anyhow::Result<f32> {
        let value = msg
            .get(name)
            .and_then(serde_json::Value::as_f64)
            .filter(|value| value.is_finite())
            .ok_or_else(|| anyhow::anyhow!("invalid touch {}", name))?;
        let value = value as f32;
        if value.is_finite() {
            Ok(value)
        } else {
            anyhow::bail!("invalid touch {}", name)
        }
    }

    Ok((
        action,
        pointer_id,
        coordinate(msg, "x")?,
        coordinate(msg, "y")?,
    ))
}

pub(crate) async fn release_all_touches(
    session: &Arc<ScrcpySession>,
    touch_state: &TouchState,
) -> anyhow::Result<()> {
    let pointers = touch_state.take_all();
    let mut first_error = None;
    for (pointer_id, x, y) in pointers {
        if let Err(error) = session
            .inject_touch(crate::device::scrcpy::ACTION_UP, pointer_id, x, y, 0.0)
            .await
        {
            debug!(pointer_id, "failed to release touch pointer: {}", error);
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn key_control_fields(msg: &serde_json::Value) -> anyhow::Result<(u8, u32, u32, u32)> {
    let action = msg
        .get("action")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u8::try_from(value).ok())
        .filter(|value| *value <= 1)
        .ok_or_else(|| anyhow::anyhow!("invalid key action"))?;
    let keycode = msg
        .get("keycode")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| (1..=1000).contains(value))
        .ok_or_else(|| anyhow::anyhow!("invalid Android keycode"))?;
    let repeat = msg
        .get("repeat")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| anyhow::anyhow!("invalid key repeat"))?;
    let meta = msg
        .get("meta")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| anyhow::anyhow!("invalid key meta state"))?;
    Ok((action, keycode, repeat, meta))
}

async fn handle_control_msg(
    session: &Arc<ScrcpySession>,
    audio_on: &Arc<std::sync::atomic::AtomicBool>,
    touch_state: &TouchState,
    data: &[u8],
) -> anyhow::Result<()> {
    let msg: serde_json::Value = serde_json::from_slice(data)?;
    let t = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match t {
        "tap" => {
            let x = msg["x"].as_f64().unwrap_or(0.0) as f32;
            let y = msg["y"].as_f64().unwrap_or(0.0) as f32;
            session.tap(x, y).await?;
        }
        // 触控拖拽语义：DOWN/MOVE/UP 事件流（与 scrcpy INJECT_TOUCH_EVENT 一一对应）。
        // 不能用 tap 序列模拟拖动——每个 tap 都是独立 DOWN+UP，密集发送时乱序，
        // 设备收到几百次乱序"点击"，行为与实际操作对不上。
        "touch" => {
            let (act, pointer_id, x, y) = touch_fields(&msg)?;
            match act {
                crate::device::scrcpy::ACTION_DOWN if touch_state.contains(pointer_id) => {
                    anyhow::bail!("touch pointer {} is already active", pointer_id);
                }
                crate::device::scrcpy::ACTION_MOVE | crate::device::scrcpy::ACTION_UP
                    if !touch_state.contains(pointer_id) =>
                {
                    anyhow::bail!("touch pointer {} is not active", pointer_id);
                }
                _ => {}
            }
            let pressure = if act == crate::device::scrcpy::ACTION_DOWN
                || act == crate::device::scrcpy::ACTION_MOVE
            {
                1.0f32
            } else {
                0.0f32
            };
            session
                .inject_touch(act, pointer_id, x, y, pressure)
                .await?;
            match act {
                crate::device::scrcpy::ACTION_DOWN => touch_state.insert(pointer_id, x, y)?,
                crate::device::scrcpy::ACTION_MOVE => touch_state.update(pointer_id, x, y)?,
                crate::device::scrcpy::ACTION_UP => {
                    touch_state.remove(pointer_id)?;
                    // Keep the remaining virtual key alive for games which
                    // clear their directional state on POINTER_UP and only
                    // rebuild it when the active pointer moves again.
                    if let Some((remaining_id, remaining_x, remaining_y)) =
                        touch_state.first_active()
                    {
                        session
                            .inject_touch(
                                crate::device::scrcpy::ACTION_MOVE,
                                remaining_id,
                                remaining_x,
                                remaining_y,
                                1.0,
                            )
                            .await?;
                    }
                }
                _ => unreachable!(),
            }
        }
        "swipe" => {
            let x1 = msg["x1"].as_f64().unwrap_or(0.0) as f32;
            let y1 = msg["y1"].as_f64().unwrap_or(0.0) as f32;
            let x2 = msg["x2"].as_f64().unwrap_or(0.0) as f32;
            let y2 = msg["y2"].as_f64().unwrap_or(0.0) as f32;
            let dur = msg["duration"].as_u64().unwrap_or(300);
            session.swipe(x1, y1, x2, y2, dur).await?;
        }
        "key" => {
            let (action, code, repeat, meta) = key_control_fields(&msg)?;
            session.inject_keycode(action, code, repeat, meta).await?;
        }
        "press" => {
            let code = msg["keycode"].as_u64().unwrap_or(0) as u32;
            session.press_key(code).await?;
        }
        "text" => {
            let text = msg["text"].as_str().unwrap_or("");
            session.inject_text(text).await?;
        }
        "scroll" => {
            let x = msg["x"].as_f64().unwrap_or(0.0) as f32;
            let y = msg["y"].as_f64().unwrap_or(0.0) as f32;
            let sx = msg["scroll_x"].as_f64().unwrap_or(0.0) as f32;
            let sy = msg["scroll_y"].as_f64().unwrap_or(0.0) as f32;
            session.inject_scroll(x, y, sx, sy).await?;
        }
        "clipboard" => {
            let text = msg["text"].as_str().unwrap_or("");
            let paste = msg["paste"].as_bool().unwrap_or(false);
            session.set_clipboard(text, paste).await?;
        }
        "start_app" => {
            let name = msg["app"].as_str().unwrap_or("");
            session.start_app(name).await?;
        }
        "rotate" => {
            session.rotate_device().await?;
        }
        // 花屏自愈：浏览器解码器失步时自动发 RTCP PLI，但 webrtc-rs 不响应 PLI，
        // 只能等设备固定 IDR（i-frame-interval=2s）。前端检测到 pliCount 增量后
        // 经此消息请求设备立即重置编码器（输出新 config+IDR，~200ms 恢复）。
        "reset_video" => {
            info!(device = %session.device.name, "reset_video requested by viewer (decoder desync)");
            session.reset_video().await?;
        }
        "audio" => {
            // 音频转发开关（默认不发，见 spawn 处 audio_on 注释）
            let on = msg.get("on").and_then(|v| v.as_bool()).unwrap_or(false);
            audio_on.store(on, std::sync::atomic::Ordering::SeqCst);
            info!(device = %session.device.name, on, "audio forwarding toggled by viewer");
        }
        "back" => {
            session.back_or_screen_on(0).await?;
        }
        _ => warn!("unknown control msg type: {}", t),
    }
    Ok(())
}

/// 订阅设备帧广播 → 有界环形缓冲（每个 viewer 独立）
///
/// 与旧 mpsc 队列的关键区别：**满队列时丢最旧、保最新**（pop_front + push_back），
/// pusher 永远消费最近到达的帧；配合 pusher 的积压跳帧（见 spawn_pusher），
/// 画面内容滞后被钳制在 ~1s（关键帧间隔）以内，而不是随积压无限增长——
/// 旧实现满队列丢"新帧"，pusher 永远在消费几秒前的旧帧（操作延迟大的根因之一）。
/// 注意：broadcast::RecvError::Lagged 表示订阅者消费太慢被丢帧（正常现象），
/// 必须 continue 继续消费，绝不能 break——否则实时帧流会永久断开（黑屏）。
/// 丢最旧帧会切断 H.264 参考链，必须置位 overflowed 标志通知 pusher 清空等待
/// 下一个 IDR（否则 pusher 会把无法解码的 P 帧发给浏览器）。
/// forwarder 通过 queue 的 Weak 引用检测 viewer 注销（唯一强引用释放）后退出，
/// 避免任务泄漏（浏览器刷新/断线频繁时泄漏任务累积，长时间运行拖垮服务）。
pub fn make_frame_queue(
    frames: broadcast::Sender<VideoFrame>,
) -> (
    Arc<Mutex<VecDeque<VideoFrame>>>,
    Arc<Notify>,
    Arc<std::sync::atomic::AtomicBool>,
) {
    const QUEUE_CAP: usize = 256;
    let queue: Arc<Mutex<VecDeque<VideoFrame>>> =
        Arc::new(Mutex::new(VecDeque::with_capacity(QUEUE_CAP)));
    let queue2 = queue.clone();
    let notify = Arc::new(Notify::new());
    let notify2 = notify.clone();
    let overflowed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let overflowed2 = overflowed.clone();
    let weak = Arc::downgrade(&queue);
    let mut sub = frames.subscribe();
    let mut dropped = 0u64;
    let mut fwd = 0u64;
    tokio::spawn(async move {
        loop {
            tokio::select! {
                // 无新帧时定期检查 viewer 是否已注销（weak 失效即退出）
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    if weak.upgrade().is_none() {
                        break;
                    }
                }
                f = sub.recv() => {
                    match f {
                        Ok(f) => {
                            fwd += 1;
                            // 诊断：转发心跳（降频，默认 info 级别不输出）
                            if fwd % 300 == 1 {
                                debug!("fq fwd: {} key={} cfg={} pts={}", fwd, f.is_keyframe, f.is_config, f.pts_us);
                            }
                            let mut q = queue2.lock();
                            if q.len() >= QUEUE_CAP {
                                q.pop_front(); // 满队列：丢最旧保最新（参考链断裂，通知 pusher）
                                overflowed2.store(true, std::sync::atomic::Ordering::SeqCst);
                                dropped += 1;
                                // OBS-003：环形缓冲溢出丢帧计数（旁路采集）
                                crate::metrics::global().record_rtp_drop();
                                if dropped % 1000 == 1 {
                                    debug!("frame queue ring full, dropped oldest, dropped={}", dropped);
                                }
                            }
                            q.push_back(f);
                            // OBS-003：生产侧队列深度 gauge。多个 viewer 各有独立
                            // 环形缓冲，gauge 记最近一次更新者的深度（简化取舍：
                            // 单设备单 viewer 是常态，多 viewer 时该值为最后活跃队列）
                            crate::metrics::global().set_rtp_queue_depth(q.len() as i64);
                            drop(q);
                            notify2.notify_one();
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });
    (queue, notify, overflowed)
}

/// 订阅设备音频广播 → 转 mpsc 队列（每个 viewer 独立；满队列丢新帧，音频实时性优先）
pub fn make_audio_queue(
    audio: broadcast::Sender<AudioFrame>,
) -> tokio::sync::mpsc::Receiver<AudioFrame> {
    let (tx, rx) = tokio::sync::mpsc::channel(128);
    let mut sub = audio.subscribe();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tx.closed() => break,
                f = sub.recv() => {
                    match f {
                        Ok(f) => {
                            if tx.try_send(f).is_err() {
                                // 队列满：丢新帧（音频实时性优先，防延迟累积）
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });
    rx
}

/// 从 answer SDP 解析 H264 的 payload type（a=rtpmap:102 H264/90000）
/// 返回第一个 H264 条目（实测 42e01f 协商为 102，浏览器可稳定解码）
#[allow(dead_code)]
fn parse_h264_payload_type(sdp: &str) -> Option<u8> {
    for line in sdp.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            // rest 形如 "102 H264/90000"（前缀已含冒号，直接取第一个空白分隔的 token）
            if rest.contains("H264/90000") {
                if let Some(pt) = rest.split_whitespace().next().and_then(|s| s.parse().ok()) {
                    return Some(pt);
                }
            }
        }
    }
    None
}

/// 从 answer SDP 解析 OPUS 的 payload type（a=rtpmap:111 opus/48000/2）
#[allow(dead_code)]
fn parse_opus_payload_type(sdp: &str) -> Option<u8> {
    for line in sdp.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            if rest.contains("opus/48000") {
                if let Some(pt) = rest.split_whitespace().next().and_then(|s| s.parse().ok()) {
                    return Some(pt);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrcpy_key_packet(action: u8, keycode: u32, repeat: u32, meta: u32) -> [u8; 14] {
        let mut packet = [0u8; 14];
        packet[0] = 0; // TYPE_INJECT_KEYCODE
        packet[1] = action;
        packet[2..6].copy_from_slice(&keycode.to_be_bytes());
        packet[6..10].copy_from_slice(&repeat.to_be_bytes());
        packet[10..14].copy_from_slice(&meta.to_be_bytes());
        packet
    }

    #[test]
    fn data_channel_key_fields_encode_scrcpy_control_packet() {
        for (json, expected) in [
            (
                r#"{"type":"key","action":0,"keycode":29,"repeat":0,"meta":0}"#,
                [0, 0, 0, 0, 0, 29, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
            (
                r#"{"type":"key","action":1,"keycode":111,"repeat":7,"meta":256}"#,
                [0, 1, 0, 0, 0, 111, 0, 0, 0, 7, 0, 0, 1, 0],
            ),
        ] {
            let msg: serde_json::Value = serde_json::from_str(json).unwrap();
            let fields = key_control_fields(&msg).unwrap();
            assert_eq!(
                scrcpy_key_packet(fields.0, fields.1, fields.2, fields.3),
                expected
            );
        }
    }

    #[test]
    fn data_channel_key_invalid_fields_are_rejected() {
        for json in [
            r#"{"type":"key","action":2,"keycode":0,"repeat":0,"meta":0}"#,
            r#"{"type":"key","action":-1,"keycode":4294967296,"repeat":0,"meta":0}"#,
            r#"{"type":"key","action":"down","keycode":"A","repeat":{},"meta":null}"#,
        ] {
            let msg: serde_json::Value = serde_json::from_str(json).unwrap();
            assert!(key_control_fields(&msg).is_err());
        }
    }

    #[test]
    fn data_channel_touch_fields_are_strict() {
        let valid: serde_json::Value = serde_json::from_str(
            r#"{"type":"touch","action":"down","pointer_id":31,"x":12.5,"y":34}"#,
        )
        .unwrap();
        assert_eq!(
            touch_fields(&valid).unwrap(),
            (crate::device::scrcpy::ACTION_DOWN, 31, 12.5, 34.0)
        );

        for json in [
            r#"{"type":"touch","action":"cancel","pointer_id":0,"x":1,"y":2}"#,
            r#"{"type":"touch","action":"down","x":1,"y":2}"#,
            r#"{"type":"touch","action":"down","pointer_id":32,"x":1,"y":2}"#,
            r#"{"type":"touch","action":"down","pointer_id":1.0,"x":1,"y":2}"#,
            r#"{"type":"touch","action":"down","pointer_id":0,"x":"1","y":2}"#,
            r#"{"type":"touch","action":"down","pointer_id":0,"x":1,"y":null}"#,
        ] {
            let msg: serde_json::Value = serde_json::from_str(json).unwrap();
            assert!(
                touch_fields(&msg).is_err(),
                "accepted invalid touch: {json}"
            );
        }
    }

    #[test]
    fn touch_state_tracks_unique_active_pointers_and_last_position() {
        let state = TouchState::default();
        assert!(!state.contains(2));
        state.insert(2, 10.0, 20.0).unwrap();
        assert!(state.contains(2));
        assert!(state.insert(2, 11.0, 21.0).is_err());
        state.update(2, 30.0, 40.0).unwrap();
        assert_eq!(state.take_all(), vec![(2, 30.0, 40.0)]);
        assert!(!state.contains(2));
        assert!(state.remove(2).is_err());
    }

    #[test]
    fn touch_state_exposes_a_remaining_pointer_for_release_reanchor() {
        let state = TouchState::default();
        state.insert(1, 10.0, 20.0).unwrap();
        state.insert(2, 30.0, 40.0).unwrap();

        state.remove(2).unwrap();

        assert_eq!(state.first_active(), Some((1, 10.0, 20.0)));
    }

    /// OBS-003：RTP 帧发送结果采集——写入 >0 字节记发送，0 字节记丢弃
    #[test]
    fn rtp_outcome_counts_sent_and_dropped_frames() {
        let metrics = crate::metrics::Metrics::default();
        record_rtp_outcome(&metrics, 1280);
        record_rtp_outcome(&metrics, 64);
        // SRTP 未就绪 0 字节静默丢弃 / payload 失败 → 丢帧
        record_rtp_outcome(&metrics, 0);
        let snap = metrics.snapshot();
        assert_eq!(snap.rtp_sent_frames_total, 2);
        assert_eq!(snap.rtp_dropped_frames_total, 1);
    }

    #[test]
    fn peer_connection_effect_keeps_ice_jitter_non_terminal() {
        assert_eq!(
            peer_connection_effect(RTCPeerConnectionState::Connected),
            PeerConnectionEffect::Connected
        );
        assert_eq!(
            peer_connection_effect(RTCPeerConnectionState::Disconnected),
            PeerConnectionEffect::TemporarilyDisconnected
        );
        assert_eq!(
            peer_connection_effect(RTCPeerConnectionState::Failed),
            PeerConnectionEffect::Terminal
        );
        assert_eq!(
            peer_connection_effect(RTCPeerConnectionState::Closed),
            PeerConnectionEffect::Terminal
        );
        assert_eq!(
            peer_connection_effect(RTCPeerConnectionState::New),
            PeerConnectionEffect::Ignore
        );
        assert_eq!(
            peer_connection_effect(RTCPeerConnectionState::Connecting),
            PeerConnectionEffect::Ignore
        );
    }

    #[test]
    fn pusher_drain_decision_preserves_reference_chain_boundaries() {
        assert_eq!(
            decide_pusher_drain(60, 60, false, false, None),
            PusherDrainDecision::Keep
        );
        assert_eq!(
            decide_pusher_drain(61, 60, false, false, Some(0)),
            PusherDrainDecision::Keep
        );
        assert_eq!(
            decide_pusher_drain(61, 60, false, false, Some(7)),
            PusherDrainDecision::DrainToKeyframe {
                drop_prefix: 7,
                request_idr: false,
            }
        );
        assert_eq!(
            decide_pusher_drain(61, 60, false, false, None),
            PusherDrainDecision::DrainAll
        );
        assert_eq!(
            decide_pusher_drain(3, 60, false, true, Some(2)),
            PusherDrainDecision::DrainToKeyframe {
                drop_prefix: 2,
                request_idr: false,
            }
        );
        assert_eq!(
            decide_pusher_drain(3, 60, false, true, None),
            PusherDrainDecision::DrainAll
        );
        assert_eq!(
            decide_pusher_drain(61, 60, true, false, Some(4)),
            PusherDrainDecision::DrainAll
        );
    }
}
