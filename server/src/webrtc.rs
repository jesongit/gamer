//! WebRTC 服务端 peer：
//! - 把 scrcpy 的 H.264 帧打包成 RTP 通过 video track 推给浏览器（不转码，零画质损失）
//! - DataChannel "control" 接收浏览器的触控/按键/文本等控制消息，转发给 scrcpy 控制 socket

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use parking_lot::Mutex;
use tokio::sync::{broadcast, Notify};
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::device::scrcpy::{AudioFrame, ScrcpySession, VideoFrame};

use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::interceptor::Attributes;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp::codecs::h264::H264Payloader;
use webrtc::rtp::codecs::opus::OpusPayloader;
use webrtc::rtp::header::Header;
use webrtc::rtp::packet::Packet;
use webrtc::rtp::packetizer::Payloader;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::rtp_transceiver::rtp_sender::RTCRtpSender;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;

/// 一个浏览器的 WebRTC 会话
pub struct ViewerSession {
    pub peer: Arc<webrtc::peer_connection::RTCPeerConnection>,
    pub track: Arc<TrackLocalStaticRTP>,
    pub running: Arc<std::sync::atomic::AtomicBool>,
    /// RTP 时间戳基准（90kHz）
    pub ts_base: Arc<Mutex<Option<u64>>>,
    pub last_ts: Arc<Mutex<u32>>,
    /// 最近一次 SPS/PPS 配置帧数据：每个关键帧前重发，保证浏览器随时可初始化解码器
    pub config_nalu: Arc<Mutex<Option<Bytes>>>,
    /// 本地 answer SDP（协商完成后保存，避免 async 上下文 block_on）
    pub answer: RTCSessionDescription,
    /// 协商后的 H264 payload type
    pub payload_type: u8,
    /// peer Failed/Closed 通知：ws.rs 收到后立即退出 ws 循环、释放 viewer
    /// （浏览器 TCP 断开时 axum socket.next() 可能不返回，导致 viewer 泄漏——
    /// 泄漏的 mDNS 实例会让后续连接 ICE 协商失败 → 黑屏）
    pub peer_closed_rx: tokio::sync::watch::Receiver<bool>,
}

impl ViewerSession {
    /// 创建 PeerConnection + video track + data channel，
    /// 处理浏览器 offer 并返回本端 answer
    /// `initial_frames`：SPS/PPS 配置帧 + 最近完整 GOP（来自帧缓存）。
    /// pusher 启动时先重放这些帧：浏览器无需等待下一个 IDR 即可开始解码
    /// （静态画面下编码器可能长时间不产 IDR，否则浏览器将一直黑屏）
    pub async fn create(
        cfg: &Config,
        session: Arc<ScrcpySession>,
        frame_q: Arc<Mutex<VecDeque<VideoFrame>>>,
        frame_notify: Arc<Notify>,
        overflowed: Arc<std::sync::atomic::AtomicBool>,
        audio_rx: tokio::sync::mpsc::Receiver<AudioFrame>,
        offer: RTCSessionDescription,
        initial_frames: Option<Vec<VideoFrame>>,
    ) -> anyhow::Result<Self> {
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)?;
        let api = APIBuilder::new().with_media_engine(m).with_interceptor_registry(registry).build();

        let ice = if let Ok(s) = std::env::var("ICE_SERVERS") {
            s.split(';')
                .map(|u| RTCIceServer { urls: vec![u.to_string()], ..Default::default() })
                .collect()
        } else {
            vec![RTCIceServer { urls: vec!["stun:stun.l.google.com:19302".into()], ..Default::default() }]
        };
        let config = RTCConfiguration { ice_servers: ice, ..Default::default() };

        let peer = Arc::new(api.new_peer_connection(config).await?);

        // H.264 视频轨
        // 注意：实测 42e01f（Constrained Baseline）协商可稳定出画面；
        // 声明 64001f（High）会让 answer 的 fmtp 与浏览器 offer 不匹配，
        // 浏览器 setRemoteDescription 失败 → 主动关闭连接（SCTP ErrChunk / DTLS alert）。
        // 设备虽编码 High profile，但浏览器按 SPS 实际值解码，fmtp 声明不影响解码。
        let codec = RTCRtpCodecCapability {
            mime_type: "video/H264".into(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f".into(),
            rtcp_feedback: vec![],
        };
        let track = Arc::new(TrackLocalStaticRTP::new(codec, "video".into(), "gamer".into()));
        let rtp_sender: Arc<RTCRtpSender> = peer.add_track(track.clone()).await?;

        // OPUS 音频轨：转发真机虚拟屏音频（浏览器默认静音，用户自行取消）。
        // WebRTC OPUS 协商参数与 scrcpy 编码一致（48kHz 立体声），无需额外参数集。
        let audio_codec = RTCRtpCodecCapability {
            mime_type: "audio/opus".into(),
            clock_rate: 48000,
            channels: 2,
            sdp_fmtp_line: "minptime=10;useinbandfec=1".into(),
            rtcp_feedback: vec![],
        };
        let audio_track = Arc::new(TrackLocalStaticRTP::new(audio_codec, "audio".into(), "gamer".into()));
        let audio_rtp_sender: Arc<RTCRtpSender> = peer.add_track(audio_track.clone()).await?;

        // 控制 DataChannel：必须由 offerer（浏览器）创建。
        // webrtc-rs 的 answer 只镜像 offer 中的 media section（generate_matched_sdp，
        // include_unmatched=false），answerer 端 create_data_channel 无法把
        // m=application 加进 answer，SCTP 永远不会协商。这里用 on_data_channel 接收。
        //
        // 控制消息必须**串行、保序**写入 scrcpy 控制 socket（DOWN/MOVE/UP 乱序会
        // 导致拖拽错乱）。旧实现每条消息 tokio::spawn 一个任务，拖动时每秒几十上百
        // 个任务并发抢锁，顺序无法保证且开销大。这里改为单消费者队列：
        // DataChannel 回调只入队，专用任务按到达顺序逐条处理。
        let (control_tx, mut control_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let s_worker = session.clone();
        tokio::spawn(async move {
            while let Some(data) = control_rx.recv().await {
                if let Err(e) = handle_control_msg(&s_worker, &data).await {
                    debug!("control msg error: {}", e);
                }
            }
        });

        let session_dc = session.clone();
        peer.on_data_channel(Box::new(move |dc: Arc<webrtc::data_channel::RTCDataChannel>| {
            info!("control data channel opened: {}", dc.label());
            let s = session_dc.clone();
            let tx = control_tx.clone();
            dc.on_message(Box::new(move |msg| {
                let data = msg.data.to_vec();
                let s2 = s.clone();
                // 只记录长度，不打印内容：拖动时每秒几十上百条消息，
                // 逐条格式化打印会让服务端日志成为性能瓶颈（全局日志锁串行化）
                debug!("control msg: {} bytes", data.len());
                if tx.send(data).is_err() {
                    debug!("control queue closed, dropping msg for {}", s2.device.name);
                }
                Box::pin(async {})
            }));
            Box::pin(async {})
        }));

        // ICE 状态回调：连接就绪信号（pusher 必须等 SRTP 就绪后才推流，
        // 否则 webrtc-rs 的 write_rtp 在 SRTP 未就绪时静默返回 Ok(0) 丢包——黑屏根因）
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let running2 = running.clone();
        // peer 是否处于 Connected：Disconnected（ICE 抖动）期间 pusher 跳过发送但不退出
        let peer_connected = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let peer_connected2 = peer_connected.clone();
        let (conn_tx, mut conn_rx) = tokio::sync::mpsc::channel::<()>(1);
        let conn_tx2 = conn_tx.clone();
        // peer 死亡通知（Failed/Closed）：ws.rs 据此退出 ws 循环，释放 viewer（mDNS 等）
        let (peer_closed_tx, peer_closed_rx) = tokio::sync::watch::channel(false);
        peer.on_peer_connection_state_change(Box::new(move |s| {
            debug!("peer state: {:?}", s);
            match s {
                RTCPeerConnectionState::Connected => {
                    peer_connected2.store(true, std::sync::atomic::Ordering::SeqCst);
                    let _ = conn_tx2.try_send(());
                }
                RTCPeerConnectionState::Disconnected => {
                    // ICE 短暂抖动（无线 adb / 浏览器标签页后台常见）：
                    // 仅标记未连接，pusher 跳过发送等待恢复，不杀 pusher
                    peer_connected2.store(false, std::sync::atomic::Ordering::SeqCst);
                }
                RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => {
                    peer_connected2.store(false, std::sync::atomic::Ordering::SeqCst);
                    running2.store(false, std::sync::atomic::Ordering::SeqCst);
                    let _ = peer_closed_tx.send(true);
                    info!("peer failed/closed, notified ws loop");
                }
                _ => {}
            }
            Box::pin(async {})
        }));

        // SDP 协商
        peer.set_remote_description(offer).await?;
        let answer = peer.create_answer(None).await?;
        let mut gather_complete = peer.gathering_complete_promise().await;
        peer.set_local_description(answer).await?;
        let _ = tokio::time::timeout(Duration::from_secs(5), gather_complete.recv()).await;

        // 保存本地 answer + 从 answer SDP 解析协商后的 payload type
        let answer_sdp = peer.local_description().await.ok_or_else(|| anyhow::anyhow!("no local description"))?;
        let payload_type = parse_h264_payload_type(&answer_sdp.sdp).unwrap_or(96);
        let ssrc = rtp_sender.get_parameters().await.encodings.first().map(|e| e.ssrc).unwrap_or(12345);
        let audio_payload_type = parse_opus_payload_type(&answer_sdp.sdp).unwrap_or(111);
        let audio_ssrc = audio_rtp_sender.get_parameters().await.encodings.first().map(|e| e.ssrc).unwrap_or(22345);
        debug!(
            "negotiated video: payload_type={} ssrc={}; audio: payload_type={} ssrc={}",
            payload_type, ssrc, audio_payload_type, audio_ssrc
        );
        // 诊断：打印协商 SDP 关键行（m 行 / direction / rtpmap / fmtp / ssrc / msid）
        for line in answer_sdp.sdp.lines() {
            let l = line.trim();
            if l.starts_with("m=") || l.starts_with("a=send") || l.starts_with("a=recv") || l.starts_with("a=rtpmap") || l.starts_with("a=fmtp") || l.starts_with("a=ssrc") || l.starts_with("a=msid") || l.starts_with("a=group") || l.starts_with("a=extmap") {
                debug!("answer sdp: {}", l);
            }
        }

        let vs = Self {
            peer,
            track,
            running,
            ts_base: Arc::new(Mutex::new(None)),
            last_ts: Arc::new(Mutex::new(0)),
            config_nalu: Arc::new(Mutex::new(initial_frames.as_ref().and_then(|f| {
                f.iter().find(|x| x.is_config).map(|x| Bytes::from(x.data.clone()))
            }))),
            answer: answer_sdp,
            payload_type,
            peer_closed_rx,
        };
        let fps = session.device.fps.or_else(|| (cfg.fps > 0).then_some(cfg.fps));
        // 静止补帧间隔：画面无新帧时按此节奏重发上一帧（也是该配置下的最小帧间隔）
        let idle_repeat_ms = fps.filter(|&f| f > 0).map(|f| (1000 / f).max(33) as u64).unwrap_or(33);
        // 硬性帧率上限：即使设备端实际输出 60fps，pusher 也按这里的最小间隔发送，
        // 避免“设置了 30fps 实际却跑到 60fps”（scrcpy 侧不再传 max_fps，见 scrcpy.rs）
        let min_frame_interval_ms = fps.filter(|&f| f > 0).map(|f| (1000 / f).max(1) as u64).unwrap_or(0);
        vs.spawn_pusher(rtp_sender, frame_q, frame_notify, overflowed, payload_type, ssrc, initial_frames, conn_rx, peer_connected.clone(), idle_repeat_ms, min_frame_interval_ms);
        vs.spawn_audio_pusher(audio_track, audio_rx, audio_payload_type, audio_ssrc, peer_connected);
        Ok(vs)
    }

    /// 视频推流循环：从环形帧缓冲取帧，H.264 payload 化 → RTP 写 track
    ///
    /// 延迟控制（核心）：帧缓冲是"丢最旧保最新"的环形队列（见 make_frame_queue），
    /// 且这里按**帧数积压**剪裁——队列深度超过 ~1s 的帧数时，丢弃队首到最近关键帧
    /// 之间的旧帧，从关键帧重新开始（用帧数而非 PTS 时间差：设备编码器重启/虚拟屏
    /// 重建时 PTS 会整体跳变，时间差不可靠）。配合设备端 i-frame-interval=1s，
    /// 画面内容滞后被钳制在 ~1s 以内：写路径慢于设备出帧时，旧帧被跳过而不是排队
    /// 积压（旧 mpsc 实现满队列丢新帧，pusher 永远消费几秒前的旧帧 → 画面滞后 5s+
    /// → "操作延迟很久"的根因）。
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
    ) {
        let track = self.track.clone();
        let running = self.running.clone();
        let ts_base = self.ts_base.clone();
        let last_ts = self.last_ts.clone();
        let config_nalu = self.config_nalu.clone();
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
            let mut payloader: Box<dyn Payloader + Send + Sync> = Box::new(H264Payloader::default());
            let mut seq: u16 = rand::random();
            let mut last_rtp_ts = 0u32;
            let mut frame_no = 0u64;
            let mut sent_packets = 0u64;
            let mut sent_bytes = 0u64;
            let mut last_pts: Option<u64> = None;
            // 最近成功发送的一帧（静止补帧用）
            let mut last_sent: Option<(VideoFrame, u32)> = None;
            // 上次成功发送的时刻（动态时间戳下限用：ts 增量 ≥ 真实发送耗时，
            // 防止帧大/处理慢时 RTP 时间戳超前于实际发送 → 浏览器 jitter buffer 欠账卡顿）
            let mut last_tx: Option<std::time::Instant> = None;
            // 真实时间锚点（(墙钟, 对应 RTP ts)）：媒体时钟的绝对基准，首帧建立。
            // 设备 PTS 快漂（实测 ~26%）时靠它把 ts 钳制在墙钟附近（见发送循环）
            let mut ts_anchor: Option<(std::time::Instant, u32)> = None;

            // 初始 GOP 重放：config 帧喂给 payloader（缓存 SPS/PPS，后续 IDR 自动拼 STAP-A）；
            // GOP 帧按原帧节奏发送（ts 基于 GOP 首帧 PTS，与后续实时流同一时间轴）。
            // 浏览器收到第一个 IDR 即开始渲染，之后无缝追到实时。
            if let Some(frames) = initial_frames {
                let mut base: Option<u64> = None;
                let mut sent = 0usize;
                let total_bytes: usize = frames.iter().map(|f| f.data.len()).sum();
                for f in frames {
                    if f.is_config {
                        let _ = payloader.payload(1200, &Bytes::from(f.data.clone()));
                        // 直接把 SPS/PPS 作为 RTP（STAP-A）发出去：浏览器可提前初始化
                        // H.264 解码器，之后首个 IDR 到达即可立即出画面；
                        // 仅依赖"关键帧前重发"时，错过重发窗口就永久黑屏
                        if !push_rtp(&track, &mut payloader, &f, payload_type, ssrc, &mut seq, 0).await {
                            break;
                        }
                        continue;
                    }
                    if base.is_none() {
                        base = Some(f.pts_us);
                    }
                    // 按原帧间隔节流发送，避免瞬时大流量打爆浏览器 jitter buffer / UDP 丢包；
                    // 下限 16ms（≈60fps）保证重放不超过源节奏的 2 倍——旧实现 clamp(5,40)
                    // 会把 1~2s 的 GOP 在 ~0.5s 内快放完，表现为连接后"画面突然加速"。
                    if let Some(lp) = last_pts {
                        let gap = f.pts_us.saturating_sub(lp);
                        let sleep_ms = (gap / 1000).clamp(16, 40);
                        tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                    }
                    last_pts = Some(f.pts_us);
                    let ts = ((f.pts_us.saturating_sub(base.unwrap()) * 90) / 1000) as u32;
                    if !push_rtp(&track, &mut payloader, &f, payload_type, ssrc, &mut seq, ts).await {
                        break;
                    }
                    last_rtp_ts = ts;
                    last_sent = Some((f, ts));
                    last_tx = Some(std::time::Instant::now());
                    sent += 1;
                }
                if let Some(b) = base {
                    *ts_base.lock() = Some(b);
                }
                info!("pusher replayed initial GOP: {} frames, {} bytes", sent, total_bytes);
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
            let backlog_limit = if min_frame_interval_ms > 0 {
                (1000 / min_frame_interval_ms) as usize // ≈1s 的帧数（fps 上限换算）
            } else {
                45
            };
            let mut waiting_key = false;
            let mut drops_broken = 0u64;
            let mut drops_wait = 0u64;
            let mut drops_to_key = 0u64;
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
                        // 环形缓冲溢出 → 参考链断裂：清空队列，等下一个 IDR 重建
                        if overflowed.swap(false, std::sync::atomic::Ordering::SeqCst) {
                            drops_broken += 1;
                            if drops_broken % 20 == 1 {
                                info!("pusher chain broken (ring overflow), dropped {} frames, drops={}", q.len(), drops_broken);
                            }
                            q.clear();
                            waiting_key = true;
                        }
                        if waiting_key {
                            // 断链恢复中：丢弃 P 帧直到新的关键帧出现
                            if let Some(ki) = q.iter().position(|f| f.is_keyframe) {
                                waiting_key = false;
                                q.drain(..ki); // 关键帧之前的 P 帧来自断链段，丢弃
                                to_send = q.drain(..).collect();
                            } else {
                                drops_wait += 1;
                                q.clear();
                            }
                        } else {
                            // 积压跳帧：队列深度超过 ~1s 的帧数 → 跳到最近关键帧
                            // （其前帧依赖更早参考链，丢弃）。正常运转时队列 1~5 帧，不触发。
                            if q.len() > backlog_limit {
                                let ki = q.iter().rposition(|f| f.is_keyframe).unwrap_or(0);
                                if ki > 0 {
                                    drops_to_key += 1;
                                    if drops_to_key % 20 == 1 {
                                        info!("pusher skipped {} stale frames to keyframe (queue {}), skips={}", ki, q.len(), drops_to_key);
                                    }
                                    q.drain(..ki);
                                }
                            }
                            to_send = q.drain(..).collect();
                            last_q_len = to_send.len(); // 本轮取出的帧数（≈ 队列积压深度）
                        }
                    }
                }

                if to_send.is_empty() {
                    // 静止补帧：idle_repeat_ms 内无新帧 → 重发最后一帧。
                    // 重复帧内容相同且参考链完整（该帧此前已成功发送），解码器可正常渲染；
                    // RTP 时间戳单调推进，浏览器帧率统计不掉。
                    if peer_connected.load(std::sync::atomic::Ordering::SeqCst) {
                        if let Some((last, ts0)) = &last_sent {
                            let ts = ts0.wrapping_add((idle_repeat_ms as u32) * 90);
                            if push_rtp(&track, &mut payloader, last, payload_type, ssrc, &mut seq, ts).await {
                                *last_ts.lock() = ts;
                                last_rtp_ts = ts;
                                last_tx = Some(std::time::Instant::now());
                            } else {
                                break;
                            }
                        }
                    }
                    continue;
                }

                // 一次取到多帧说明消费慢于输入：全力追赶（帧率上限仅对单帧批次生效）
                // 注意：to_send 在 for 循环中被移动，paced 必须在循环外计算
                let paced = to_send.len() == 1;
                for frame in to_send {
                    frame_no += 1;
                    // 诊断：实时循环心跳（验证帧缓冲是否收到设备帧）
                    if frame_no % 300 == 1 {
                        debug!("pusher recv: no={} key={} cfg={} pts={} peer={}", frame_no, frame.is_keyframe, frame.is_config, frame.pts_us, peer_connected.load(std::sync::atomic::Ordering::SeqCst));
                    }

                    // 节流：仅对"单帧批次"（正常节奏）应用最小帧间隔（帧率上限——
                    // "设置 30fps"就是硬上限，设备端 60fps 输入会被限到 ~30fps）。
                    // 关键：**批量取出 = 消费慢于输入（积压），批次内绝不再逐帧 sleep**。
                    // 旧逻辑对批次内每一帧都 sleep(min_interval - elapsed)：积压 45 帧要
                    // ~700ms 才能发完，期间生产端又补进 ~45 帧 → 队列永远清不空，内容
                    // 永久滞后 ~0.7s 且随积压增长（"操作延迟大"的结构性根因，日志特征
                    // q=30~62 持续不降 + 周期性 skip-to-keyframe 跳帧）。积压时应全速
                    // 追赶：RTP 时间戳已携带媒体节奏，浏览器 jitter buffer 会平滑播出，
                    // 提前到达的帧只占 ~百毫秒缓冲，不会引入可感知延迟。
                    // 单帧批次（正常节奏）时距上次发送已 ≥ min_interval，sleep 实际为 0，
                    // 节流不会产生额外延迟；真正的上限约束靠 backlog 跳帧兜底。
                    // 定时器精度由 main.rs 的 timeBeginPeriod(1) 保证（见该处注释）。
                    let mut sleep_ms = 0u64;
                    if paced && min_frame_interval_ms > 0 {
                        if let Some(t) = last_tx {
                            let elapsed_ms = t.elapsed().as_millis() as u64;
                            if elapsed_ms < min_frame_interval_ms {
                                sleep_ms = sleep_ms.max(min_frame_interval_ms - elapsed_ms);
                            }
                        }
                    }
                    if sleep_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
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
                            Some((aw, at)) => at.wrapping_add(((aw.elapsed().as_micros() as u64) * 90 / 1000) as u32),
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

                    // config 帧（SPS/PPS）：缓存进 config_nalu（供后续关键帧前重发）
                    // + 交给 payloader 缓存随下个关键帧打 STAP-A。
                    // 必须更新 config_nalu：若 viewer 连接早于帧缓存首帧（配置切换重连、
                    // 会话刚建立的空窗期），initial_frames 为 None，此处是浏览器拿到
                    // SPS/PPS 的唯一机会——否则错过会话开头的 config 帧就永久黑屏。
                    if frame.is_config {
                        *config_nalu.lock() = Some(Bytes::from(frame.data.clone()));
                        let _ = payloader.payload(1200, &Bytes::from(frame.data));
                        continue;
                    }

                    // ICE 抖动（Disconnected）期间跳过发送，等待恢复；连接恢复后继续推
                    if !peer_connected.load(std::sync::atomic::Ordering::SeqCst) {
                        debug!("peer disconnected, skipping frame {}", frame_no);
                        continue;
                    }

                    // 关键帧前重发 SPS/PPS：保证浏览器随时能拿到参数集初始化解码器。
                    // scrcpy 只在会话开始时发一次 config，若浏览器错过初始 STAP-A（SRTP 时序/丢包），
                    // 后续 IDR 不带参数集且服务端无 PLI 响应 → 永久黑屏。
                    if frame.is_keyframe {
                        if let Some(cfg) = config_nalu.lock().clone() {
                            let _ = payloader.payload(1200, &cfg);
                        }
                    }

                    let t_send = std::time::Instant::now();
                    if !push_rtp(&track, &mut payloader, &frame, payload_type, ssrc, &mut seq, ts).await {
                        break;
                    }
                    send_time_us += t_send.elapsed().as_micros() as u64;
                    send_samples += 1;
                    sent_packets += 1;
                    sent_bytes += frame.data.len() as u64;
                    // 诊断：实时推帧日志（每 300 帧，含平均 RTP 发送耗时与队列深度）
                    if frame_no % 300 == 1 {
                        let avg = if send_samples > 0 { send_time_us / send_samples / 1000 } else { 0 };
                        info!("pusher live: frame_no={} key={} ts={} peer={} size={} send_avg={}ms q={}", frame_no, frame.is_keyframe, ts, peer_connected.load(std::sync::atomic::Ordering::SeqCst), frame.data.len(), avg, last_q_len);
                        send_time_us = 0;
                        send_samples = 0;
                    }
                    last_sent = Some((frame, ts));
                    last_tx = Some(std::time::Instant::now());
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
    ) {
        let running = self.running.clone();
        tokio::spawn(async move {
            let mut payloader: Box<dyn Payloader + Send + Sync> = Box::new(OpusPayloader::default());
            let mut seq: u16 = rand::random();
            let mut last_ts: Option<u32> = None;
            let mut sent: u64 = 0;
            // 音频真实时间锚点：与视频同理，设备音频 PTS 也可能快漂（虚拟屏
            // remote_submix 时钟），若 ts 跟着漂，浏览器音频 jitter buffer 目标
            // 被拉高 → A/V 同步把整个画面拖慢数秒。钳制在 [real, real+40ms]。
            let mut audio_anchor: Option<(std::time::Instant, u32)> = None;
            while running.load(std::sync::atomic::Ordering::SeqCst) {
                let Some(frame) = audio_rx.recv().await else { break };
                // OPUS 参数集帧（OpusHead）无需发送：WebRTC 用 SDP fmtp 描述参数
                if frame.is_config {
                    continue;
                }
                if !peer_connected.load(std::sync::atomic::Ordering::SeqCst) {
                    continue;
                }
                // 48kHz 时间戳：pts_us → ticks（×48/1000）；锚定真实时间；单调保底 +20ms
                let ts = {
                    let src_ts = ((frame.pts_us.saturating_mul(48)) / 1000) as u32;
                    let real_ts = match audio_anchor {
                        Some((aw, at)) => at.wrapping_add(((aw.elapsed().as_micros() as u64) * 48 / 1000) as u32),
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
                        ..Default::default()
                    };
                    seq = seq.wrapping_add(1);
                    match tokio::time::timeout(
                        Duration::from_millis(3000),
                        audio_track.write_rtp_with_extensions_attributes(&pkt, &[], &Attributes::new()),
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

    /// 返回本地 answer SDP（协商时已保存，直接返回）
    pub fn local_description(&self) -> RTCSessionDescription {
        self.answer.clone()
    }
}

/// 把一帧 H.264 打成 RTP 包写入 track；返回是否继续推流（写失败 = 连接已断）
async fn push_rtp(
    track: &Arc<TrackLocalStaticRTP>,
    payloader: &mut Box<dyn Payloader + Send + Sync>,
    frame: &VideoFrame,
    payload_type: u8,
    ssrc: u32,
    seq: &mut u16,
    ts: u32,
) -> bool {
    let payloads = match payloader.payload(1200, &Bytes::from(frame.data.clone())) {
        Ok(p) => p,
        Err(e) => {
            debug!("payload error: {}", e);
            return true;
        }
    };
    let n = payloads.len();
    let mut written = 0usize;
    for (i, payload) in payloads.into_iter().enumerate() {
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
            ..Default::default()
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
                return false;
            }
            Err(_) => {
                warn!("write_rtp timed out (3s), connection stalled, stopping pusher");
                return false;
            }
        }
        *seq = seq.wrapping_add(1);
    }
    if written == 0 {
        // SRTP 未就绪时 webrtc-rs 静默返回 Ok(0) 丢弃整包——必须等待连接就绪后再推
        warn!("rtp write returned 0 bytes (SRTP not ready?), frame {} bytes, {} packets", frame.data.len(), n);
    }
    true
}

/// DataChannel 控制消息协议（JSON）
/// { "type": "tap"|"swipe"|"key"|"press"|"text"|"scroll"|"clipboard"|"start_app"|"rotate"|"back", ... }
async fn handle_control_msg(session: &ScrcpySession, data: &[u8]) -> anyhow::Result<()> {
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
            let action = msg["action"].as_str().unwrap_or("move");
            let x = msg["x"].as_f64().unwrap_or(0.0) as f32;
            let y = msg["y"].as_f64().unwrap_or(0.0) as f32;
            let (act, pressure) = match action {
                "down" => (crate::device::scrcpy::ACTION_DOWN, 1.0f32),
                "up" => (crate::device::scrcpy::ACTION_UP, 0.0f32),
                _ => (crate::device::scrcpy::ACTION_MOVE, 1.0f32),
            };
            session.inject_touch(act, 0, x, y, pressure).await?;
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
            let action = msg["action"].as_u64().unwrap_or(0) as u8;
            let code = msg["keycode"].as_u64().unwrap_or(0) as u32;
            let repeat = msg["repeat"].as_u64().unwrap_or(0) as u32;
            let meta = msg["meta"].as_u64().unwrap_or(0) as u32;
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
    let queue: Arc<Mutex<VecDeque<VideoFrame>>> = Arc::new(Mutex::new(VecDeque::with_capacity(QUEUE_CAP)));
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
                                if dropped % 1000 == 1 {
                                    debug!("frame queue ring full, dropped oldest, dropped={}", dropped);
                                }
                            }
                            q.push_back(f);
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
pub fn make_audio_queue(audio: broadcast::Sender<AudioFrame>) -> tokio::sync::mpsc::Receiver<AudioFrame> {
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
