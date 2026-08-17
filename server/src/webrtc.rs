//! WebRTC 服务端 peer：
//! - 把 scrcpy 的 H.264 帧打包成 RTP 通过 video track 推给浏览器（不转码，零画质损失）
//! - DataChannel "control" 接收浏览器的触控/按键/文本等控制消息，转发给 scrcpy 控制 socket

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use parking_lot::Mutex;
use tokio::sync::broadcast;
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
        frame_rx: tokio::sync::mpsc::Receiver<VideoFrame>,
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
        let session_dc = session.clone();
        peer.on_data_channel(Box::new(move |dc: Arc<webrtc::data_channel::RTCDataChannel>| {
            info!("control data channel opened: {}", dc.label());
            let s = session_dc.clone();
            dc.on_message(Box::new(move |msg| {
                let data = msg.data.to_vec();
                let s2 = s.clone();
                // 打印消息内容：验证坐标映射（浏览器点击 → 设备坐标）
                info!("control msg: {} bytes: {}", data.len(), String::from_utf8_lossy(&data));
                tokio::spawn(async move {
                    if let Err(e) = handle_control_msg(&s2, &data).await {
                        debug!("control msg error: {}", e);
                    }
                });
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
        // 静止补帧间隔：画面无新帧时按此节奏重发上一帧，维持浏览器端帧率稳定
        // （max_fps 是上限不是下限——内容不动时设备不产新帧，不补帧则 fps 显示会掉到 1 以下；
        //   补帧只重发相同内容，新帧到达立即恢复实时。补帧节奏封顶 30fps 防浪费带宽）
        let fps = session.device.fps.or_else(|| (cfg.fps > 0).then_some(cfg.fps));
        let idle_repeat_ms = fps.filter(|&f| f > 0).map(|f| (1000 / f).max(33) as u64).unwrap_or(33);
        vs.spawn_pusher(rtp_sender, frame_rx, payload_type, ssrc, initial_frames, conn_rx, peer_connected.clone(), idle_repeat_ms);
        vs.spawn_audio_pusher(audio_track, audio_rx, audio_payload_type, audio_ssrc, peer_connected);
        Ok(vs)
    }

    /// 视频推流循环：从帧队列取帧，H.264 payload 化 → RTP 写 track
    fn spawn_pusher(
        &self,
        rtp_sender: Arc<RTCRtpSender>,
        mut frame_rx: tokio::sync::mpsc::Receiver<VideoFrame>,
        payload_type: u8,
        ssrc: u32,
        initial_frames: Option<Vec<VideoFrame>>,
        mut conn_rx: tokio::sync::mpsc::Receiver<()>,
        peer_connected: Arc<std::sync::atomic::AtomicBool>,
        idle_repeat_ms: u64,
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

            // 初始 GOP 重放：config 帧喂给 payloader（缓存 SPS/PPS，后续 IDR 自动拼 STAP-A）；
            // GOP 帧按原帧节奏发送（ts 基于 GOP 首帧 PTS，与后续实时流同一时间轴）。
            // 浏览器收到第一个 IDR 即开始渲染，之后无缝追到实时。
            if let Some(frames) = initial_frames {
                let mut base: Option<u64> = None;
                let mut sent = 0usize;
                let total_bytes: usize = frames.iter().map(|f| f.data.len()).sum();
                for f in frames {
                    if f.is_config {
                        let _ = payloader.payload(1200, &Bytes::from(f.data));
                        continue;
                    }
                    if base.is_none() {
                        base = Some(f.pts_us);
                    }
                    // 按原帧间隔节流发送，避免瞬时大流量打爆浏览器 jitter buffer / UDP 丢包
                    if let Some(lp) = last_pts {
                        let gap = f.pts_us.saturating_sub(lp);
                        let sleep_ms = (gap / 1000).clamp(5, 40);
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

            // 断链保护 + 积压批处理（修复"塌缩成只发关键帧"问题）：
            // H.264 的 P 帧依赖前序参考帧。上游（编码器）可能以 30~60fps 投帧，
            // 而 RTP 写入较慢，队列必然积压。旧实现的"追最新帧 + 等关键帧"策略
            // 在积压时把中间 P 帧全丢、只发关键帧 → 浏览器每 1~5 秒才见一帧新画面
            // （实测 pusher 收到的帧 frame_no 全部 key=true）——"30fps 却卡成 1fps"
            // 的真正根因。新策略：积压帧按序收成一批（上限 BATCH_MAX_FRAMES 防
            // 延迟无限累积），批内从最后一个关键帧起发（其前的帧依赖更早参考链，
            // 丢弃），整批按序发送：
            //   - 链路完整 → 连续播放，无跳变、无塌缩；
            //   - 链路已断（批内无关键帧且此前等待中）→ 整批丢弃，等下一个 IDR
            //     重建（i-frame-interval=1 → ≤1s）。
            // 代价：积压时最多落后一批（≈0.75~1.5s），远好于画面定格。
            const BATCH_MAX_FRAMES: usize = 45;
            let mut waiting_key = false;
            let mut batch_drops = 0u64;

            while running.load(std::sync::atomic::Ordering::SeqCst) {
                let recv = tokio::time::timeout(Duration::from_millis(idle_repeat_ms), frame_rx.recv()).await;
                let mut batch: Vec<VideoFrame> = match recv {
                    Ok(Some(f)) => vec![f],
                    Ok(None) => break, // 帧队列关闭
                    Err(_) => {
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
                };
                // 收集积压帧（按序，不丢中间帧）
                while let Ok(f2) = frame_rx.try_recv() {
                    batch.push(f2);
                    if batch.len() >= BATCH_MAX_FRAMES {
                        break;
                    }
                }
                // 批内有关键帧 → 从最后一个关键帧开始发（其前帧依赖更早参考链，丢弃）
                if let Some(ki) = batch.iter().rposition(|f| f.is_keyframe) {
                    if ki > 0 {
                        debug!("pusher batch: dropping {} stale frames to keyframe", ki);
                        batch.drain(..ki);
                    }
                    waiting_key = false;
                } else if waiting_key {
                    // 链路已断且批内无关键帧：丢弃整批，等下个 IDR 重建
                    batch_drops += 1;
                    if batch_drops % 50 == 1 {
                        info!("pusher batch dropped (waiting keyframe), drops={}", batch_drops);
                    }
                    continue;
                }
                // 本批是否有积压（>1 帧说明消费慢于输入）：有积压时跳过节流 sleep 全力追赶
                let have_backlog = batch.len() > 1;
                for frame in batch {
                    frame_no += 1;
                    // 诊断：实时循环心跳（验证 frame_rx 是否收到设备帧）
                    if frame_no % 5 == 0 || frame_no <= 3 {
                        debug!("pusher recv: no={} key={} cfg={} pts={} peer={}", frame_no, frame.is_keyframe, frame.is_config, frame.pts_us, peer_connected.load(std::sync::atomic::Ordering::SeqCst));
                    }

                    // 实时节流：仅当本批无积压时按源帧间隔 sleep（平滑节奏）；
                    // 有积压（帧大/处理慢导致队列堆积）时立即发送追赶——
                    // 否则 pusher 永远慢于输入 → 时间戳超前 → 浏览器 jitter buffer 欠账卡顿。
                    if !have_backlog {
                        if let Some(lp) = last_pts {
                            let gap = frame.pts_us.saturating_sub(lp);
                            // 帧间隔 clamp 到 5~40ms（对应 25~60fps 上限节奏）
                            let sleep_ms = (gap / 1000).clamp(5, 40);
                            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                        }
                    }
                    last_pts = Some(frame.pts_us);

                    // RTP 时间戳：90kHz，基于 PTS 差值累积，保证单调。
                    // 动态下限：ts 增量 ≥ 真实发送耗时（距上次成功发送），
                    // 帧大/处理慢时时间戳跟随实际节奏，浏览器不会因"帧还没到、时间已到"而卡顿。
                    let pts = frame.pts_us;
                    let ts = {
                        let mut base = ts_base.lock();
                        if base.is_none() {
                            *base = Some(pts);
                        }
                        let delta = pts.saturating_sub(base.unwrap());
                        let src_ts = ((delta * 90) / 1000) as u32;
                        let floor_ts = last_tx
                            .map(|t| last_rtp_ts.wrapping_add((t.elapsed().as_micros() as u32) * 90 / 1000))
                            .unwrap_or(0);
                        let ts = src_ts.max(floor_ts);
                        let ts = if ts <= last_rtp_ts && frame_no > 1 { last_rtp_ts + 3000 } else { ts };
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

                    if !push_rtp(&track, &mut payloader, &frame, payload_type, ssrc, &mut seq, ts).await {
                        break;
                    }
                    sent_packets += 1;
                    sent_bytes += frame.data.len() as u64;
                    // 诊断：实时推帧日志（调试用，每 25 帧）
                    if frame_no % 25 == 0 {
                        info!("pusher live: frame_no={} key={} ts={} peer={} size={}", frame_no, frame.is_keyframe, ts, peer_connected.load(std::sync::atomic::Ordering::SeqCst), frame.data.len());
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
            while running.load(std::sync::atomic::Ordering::SeqCst) {
                let Some(frame) = audio_rx.recv().await else { break };
                // OPUS 参数集帧（OpusHead）无需发送：WebRTC 用 SDP fmtp 描述参数
                if frame.is_config {
                    continue;
                }
                if !peer_connected.load(std::sync::atomic::Ordering::SeqCst) {
                    continue;
                }
                // 48kHz 时间戳：pts_us → ticks（×48/1000）；单调保底 +20ms
                let ts = ((frame.pts_us.saturating_mul(48)) / 1000) as u32;
                let ts = match last_ts {
                    Some(lt) if ts <= lt => lt + 960,
                    _ => ts,
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

/// 订阅设备帧广播 → 转 mpsc 队列（每个 viewer 独立）
///
/// 注意：broadcast::RecvError::Lagged 表示订阅者消费太慢被丢帧（正常现象），
/// 必须 continue 继续消费，绝不能 break——否则实时帧流会永久断开（黑屏）。
/// mpsc 队列满时丢新帧；pusher 端会 drain 追最新帧（见 spawn_pusher）。
pub fn make_frame_queue(frames: broadcast::Sender<VideoFrame>) -> tokio::sync::mpsc::Receiver<VideoFrame> {    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let mut sub = frames.subscribe();
    let mut dropped = 0u64;
    let mut fwd = 0u64;
    tokio::spawn(async move {
        loop {
            // viewer 注销后 rx 被 drop → tx.closed() 完成 → 转发任务退出，
            // 避免泄漏：否则任务会永远 recv broadcast + try_send 失败空转
            // （浏览器刷新/断线频繁时泄漏任务累积，长时间运行拖垮服务）
            tokio::select! {
                _ = tx.closed() => break,
                f = sub.recv() => {
                    match f {
                        Ok(f) => {
                            fwd += 1;
                            // 诊断：转发心跳（每帧）
                            debug!("fq fwd: {} key={} cfg={} pts={}", fwd, f.is_keyframe, f.is_config, f.pts_us);
                            if tx.try_send(f).is_err() {
                                dropped += 1;
                                if dropped % 1000 == 1 {
                                    debug!("frame queue full, dropped={}", dropped);
                                }
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
