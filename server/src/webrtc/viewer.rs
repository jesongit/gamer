//! viewer 生命周期：注册表（ViewerHandle/ViewerMap）、接管（force/conflict）、
//! taken_over 通知与统一 teardown，以及浏览器 WebRTC 会话（ViewerSession）的
//! 创建与 SDP 协商。
//!
//! 推流循环（pusher）、RTP 打包与帧队列仍在本模块父级（`webrtc` mod.rs /
//! `protocol`）——viewer 经既有 handle/queue 边界与推流交互，不引入新抽象。

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use parking_lot::Mutex;
use tokio::sync::Notify;
use tracing::{debug, info};

use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::rtp_transceiver::rtp_sender::RTCRtpSender;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;

use crate::config::Config;
use crate::device::scrcpy::{AudioFrame, ScrcpySession, VideoFrame};

use super::{handle_control_msg, peer_connection_effect, protocol, PeerConnectionEffect};

/// viewer 断开原因：用于统一 takeover / device disconnect / shutdown / peer closed
/// 的 teardown 行为与日志口径。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewerDisconnectReason {
    TakenOver,
    DeviceDisconnected,
    Shutdown,
    PeerClosed,
}

impl ViewerDisconnectReason {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TakenOver => "taken_over",
            Self::DeviceDisconnected => "device_disconnected",
            Self::Shutdown => "shutdown",
            Self::PeerClosed => "peer_closed",
        }
    }

    /// 返回需要经信令 WS 发送的控制通知；其它断开原因不应伪装成接管。
    fn notification_type(self) -> Option<&'static str> {
        match self {
            Self::TakenOver => Some("taken_over"),
            Self::DeviceDisconnected | Self::Shutdown | Self::PeerClosed => None,
        }
    }
}

/// 每设备活跃 viewer 注册表条目：
/// - running/peer 用于"新连接踢旧连接"（停旧 pusher + 关旧 peer）
/// - control_dc 供服务端反向给浏览器推消息（脚本 tap/swipe/匹配命中可视化事件）
#[derive(Clone)]
pub struct ViewerHandle {
    pub running: Arc<std::sync::atomic::AtomicBool>,
    pub peer: std::sync::Weak<webrtc::peer_connection::RTCPeerConnection>,
    pub control_dc: Arc<Mutex<Option<Arc<webrtc::data_channel::RTCDataChannel>>>>,
    /// viewer 实例标识（OBS-002 关联字段）：注册/接管踢除/断开日志用它对齐
    /// 同一个 viewer 会话的迁移轨迹（uuid，仅日志关联用）
    pub viewer_id: String,
    /// pusher 最近一次向浏览器发送（实时帧或静止补帧）的 unix 毫秒：
    /// api 静默看门狗据此区分"设备 0 帧（静态屏常态，viewer 仍被补帧投喂）"
    /// 与"真断流（pusher 停止供帧）"，避免静态屏被 35s 周期兜底重连踢 viewer
    pub last_serve: Arc<std::sync::atomic::AtomicI64>,
    /// 反向通知通道（该 viewer 的信令 ws 发送端）：被新页面 force 顶替时推
    /// {"type":"taken_over"}，旧页面收到后不再自动重连（防互顶死循环）。
    /// None = 尚未注册或已被取用
    pub notify: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>>>,
}

/// device_id → 活跃 viewer（main.rs 创建，AppState / Scheduler / ws.rs 共享）
pub type ViewerMap = Arc<std::sync::Mutex<std::collections::HashMap<String, ViewerHandle>>>;

/// 从 viewer 注册表移除指定设备的活跃 viewer。
pub fn take_viewer(viewers: &ViewerMap, device_id: &str) -> Option<ViewerHandle> {
    viewers.lock().unwrap().remove(device_id)
}

/// 统一 viewer teardown：先标记 running=false，再按 reason 决定是否发送 taken_over，
/// 最后关闭 peer，避免各条断开路径出现不同的收尾行为。
pub async fn teardown_viewer(handle: ViewerHandle, reason: ViewerDisconnectReason) {
    handle
        .running
        .store(false, std::sync::atomic::Ordering::SeqCst);
    if let Some(notification_type) = reason.notification_type() {
        if let Some(tx) = handle.notify.lock().take() {
            let _ = tx.send(serde_json::json!({"type": notification_type}).to_string());
        }
    }
    if let Some(peer) = handle.peer.upgrade() {
        let _ = peer.close().await;
    }
}

/// 从注册表移除并 teardown viewer；返回是否确实清掉了一个活跃 viewer。
pub async fn remove_and_teardown_viewer(
    viewers: &ViewerMap,
    device_id: &str,
    reason: ViewerDisconnectReason,
) -> bool {
    match take_viewer(viewers, device_id) {
        Some(handle) => {
            teardown_viewer(handle, reason).await;
            true
        }
        None => false,
    }
}

/// 一个浏览器的 WebRTC 会话
pub struct ViewerSession {
    /// viewer 实例标识（OBS-002 关联字段，注册/接管/断开日志共用）
    pub viewer_id: String,
    pub peer: Arc<webrtc::peer_connection::RTCPeerConnection>,
    pub track: Arc<TrackLocalStaticRTP>,
    pub running: Arc<std::sync::atomic::AtomicBool>,
    /// RTP 时间戳基准（90kHz）
    pub ts_base: Arc<Mutex<Option<u64>>>,
    pub last_ts: Arc<Mutex<u32>>,
    /// 最近一次 SPS/PPS 配置帧数据：每个关键帧前重发，保证浏览器随时可初始化解码器
    pub config_nalu: Arc<Mutex<Option<Bytes>>>,
    /// pusher 最近一次发送（实时帧/补帧）的 unix 毫秒（与 ViewerHandle.last_serve 同源）
    pub last_serve: Arc<std::sync::atomic::AtomicI64>,
    /// 本地 answer SDP（协商完成后保存，避免 async 上下文 block_on）
    pub answer: RTCSessionDescription,
    /// peer Failed/Closed 通知：ws.rs 收到后立即退出 ws 循环、释放 viewer
    /// （浏览器 TCP 断开时 axum socket.next() 可能不返回，导致 viewer 泄漏——
    /// 泄漏的 mDNS 实例会让后续 ICE 协商失败 → 黑屏）
    pub peer_closed_rx: tokio::sync::watch::Receiver<bool>,
    /// 浏览器创建的 control DataChannel（on_data_channel 时捕获）：
    /// 服务端→浏览器方向推送脚本运行可视化事件（引擎 emit 查注册表发送）
    pub control_dc: Arc<Mutex<Option<Arc<webrtc::data_channel::RTCDataChannel>>>>,
    /// scrcpy 会话引用：pusher 初始重放全部 0 字节（SRTP 未就绪）时
    /// 请求编码器重置（RESET_VIDEO → 新 SPS/PPS + IDR），让浏览器快速恢复
    pub session: Arc<ScrcpySession>,
}

impl ViewerSession {
    /// 创建 PeerConnection + video track + data channel，
    /// 处理浏览器 offer 并返回本端 answer
    /// `initial_frames`：SPS/PPS 配置帧 + 最近完整 GOP（来自帧缓存）。
    /// pusher 启动时先重放这些帧：浏览器无需等待下一个 IDR 即可开始解码
    /// （静态画面下编码器可能长时间不产 IDR，否则浏览器将一直黑屏）
    // 参数较多源于会话组装所需的全部资源句柄；拆分 viewer 生命周期
    //（OPTIMIZATION_PLAN 阶段 6 webrtc 模块化）时再收敛
    #[allow(clippy::too_many_arguments)]
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
        // 容器 / NAT 1-to-1 部署（config.toml rtc_external_ip / rtc_udp_port /
        // rtc_external_port，语义见 rtc_net）：三键全缺省返回 None，构建链与
        // 既有逐字节一致（Windows 直跑 / 既有部署零变化）
        let api_builder = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry);
        let api = match super::rtc_net::build_rtc_setting_engine(cfg).await? {
            Some(se) => api_builder.with_setting_engine(se).build(),
            None => api_builder.build(),
        };

        let ice = if let Ok(s) = std::env::var("ICE_SERVERS") {
            s.split(';')
                .map(|u| RTCIceServer {
                    urls: vec![u.to_string()],
                    ..Default::default()
                })
                .collect()
        } else {
            vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".into()],
                ..Default::default()
            }]
        };
        let config = RTCConfiguration {
            ice_servers: ice,
            ..Default::default()
        };

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
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                .into(),
            rtcp_feedback: vec![],
        };
        let track = Arc::new(TrackLocalStaticRTP::new(
            codec,
            "video".into(),
            "gamer".into(),
        ));
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
        let audio_track = Arc::new(TrackLocalStaticRTP::new(
            audio_codec,
            "audio".into(),
            "gamer".into(),
        ));
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
        // 音频按需发送（默认不发）：静音时也持续发音频 RTP 的话，部分浏览器
        // 内核即使音频轨 enabled=false 仍把它选为 A/V 同步主时钟（实测 ZCode
        // IAB webview；Chrome 无此问题），而虚拟屏 remote_submix 音频时钟有
        // ~1% 慢漂 → 视频 jitter buffer 目标延迟以 ~12ms/s 单调累积（200ms →
        // 看门狗 1.5s 阈值 → 重连清零 → 再累积，循环）。viewer 通过
        // {"type":"audio","on":bool} 显式开启后才开始转发，静音时零音频包，
        // 任何内核都无法拿音频做主时钟
        let audio_on = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s_worker = session.clone();
        let worker_audio_on = audio_on.clone();
        tokio::spawn(async move {
            while let Some(data) = control_rx.recv().await {
                if let Err(e) = handle_control_msg(&s_worker, &worker_audio_on, &data).await {
                    debug!("control msg error: {}", e);
                }
            }
        });

        let session_dc = session.clone();
        // control DataChannel 捕获：on_data_channel 回调触发时存入，
        // 供服务端反向推送（脚本事件）——浏览器只会创建 "control" 一个通道
        let control_dc: Arc<Mutex<Option<Arc<webrtc::data_channel::RTCDataChannel>>>> =
            Arc::new(Mutex::new(None));
        let dc_holder = control_dc.clone();
        peer.on_data_channel(Box::new(
            move |dc: Arc<webrtc::data_channel::RTCDataChannel>| {
                info!("control data channel opened: {}", dc.label());
                *dc_holder.lock() = Some(dc.clone());
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
            },
        ));

        // ICE 状态回调：连接就绪信号（pusher 必须等 SRTP 就绪后才推流，
        // 否则 webrtc-rs 的 write_rtp 在 SRTP 未就绪时静默返回 Ok(0) 丢包——黑屏根因）
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let running2 = running.clone();
        // peer 是否处于 Connected：Disconnected（ICE 抖动）期间 pusher 跳过发送但不退出
        let peer_connected = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let peer_connected2 = peer_connected.clone();
        let (conn_tx, conn_rx) = tokio::sync::mpsc::channel::<()>(1);
        let conn_tx2 = conn_tx.clone();
        // peer 死亡通知（Failed/Closed）：ws.rs 据此退出 ws 循环，释放 viewer（mDNS 等）
        let (peer_closed_tx, peer_closed_rx) = tokio::sync::watch::channel(false);
        peer.on_peer_connection_state_change(Box::new(move |s| {
            debug!("peer state: {:?}", s);
            match peer_connection_effect(s) {
                PeerConnectionEffect::Connected => {
                    peer_connected2.store(true, std::sync::atomic::Ordering::SeqCst);
                    let _ = conn_tx2.try_send(());
                }
                PeerConnectionEffect::TemporarilyDisconnected => {
                    // ICE 短暂抖动（无线 adb / 浏览器标签页后台常见）：
                    // 仅标记未连接，pusher 跳过发送等待恢复，不杀 pusher
                    peer_connected2.store(false, std::sync::atomic::Ordering::SeqCst);
                }
                PeerConnectionEffect::Terminal => {
                    peer_connected2.store(false, std::sync::atomic::Ordering::SeqCst);
                    running2.store(false, std::sync::atomic::Ordering::SeqCst);
                    let _ = peer_closed_tx.send(true);
                    info!("peer failed/closed, notified ws loop");
                }
                PeerConnectionEffect::Ignore => {}
            }
            Box::pin(async {})
        }));

        // SDP 协商
        // 运维观测：offer 携带的远端候选数（0 = 浏览器发了无候选 offer——前端
        // 未等收集完成就发送，连接只能靠浏览器对 answer 候选的 prflx 回路）
        let remote_candidates = offer
            .sdp
            .lines()
            .filter(|l| l.starts_with("a=candidate:"))
            .count();
        peer.set_remote_description(offer).await?;
        let answer = peer.create_answer(None).await?;
        let mut gather_complete = peer.gathering_complete_promise().await;
        peer.set_local_description(answer).await?;
        let _ = tokio::time::timeout(Duration::from_secs(5), gather_complete.recv()).await;

        // 保存本地 answer + 从 answer SDP 解析协商后的 payload type
        let answer_sdp = peer
            .local_description()
            .await
            .ok_or_else(|| anyhow::anyhow!("no local description"))?;
        // 运维观测：ICE 候选宣告（容器 / 端口映射部署排障第一现场）——
        // local=[] = muxed gather 失败（rtc_external_ip/rtc_udp_port 配置或网络问题）；
        // offer_remote=0 = 浏览器发了无候选 offer（前端未等收集完成），此时
        // 连接全靠浏览器对 answer 候选的 prflx 回路，健壮性差（见 PITFALLS）
        let local_candidates: Vec<&str> = answer_sdp
            .sdp
            .lines()
            .filter(|l| l.starts_with("a=candidate:"))
            .map(|l| l.trim_start_matches("a=candidate:"))
            .collect();
        info!(
            device = %session.device.name,
            "ICE candidates: local=[{}] offer_remote={} (offer_remote=0 relies on browser prflx)",
            local_candidates.join(" | "),
            remote_candidates,
        );
        let payload_type = protocol::payload_type_for(&answer_sdp.sdp, "H264/90000").unwrap_or(96);
        let ssrc = rtp_sender
            .get_parameters()
            .await
            .encodings
            .first()
            .map(|e| e.ssrc)
            .unwrap_or(12345);
        let audio_payload_type =
            protocol::payload_type_for(&answer_sdp.sdp, "opus/48000").unwrap_or(111);
        let audio_ssrc = audio_rtp_sender
            .get_parameters()
            .await
            .encodings
            .first()
            .map(|e| e.ssrc)
            .unwrap_or(22345);
        debug!(
            "negotiated video: payload_type={} ssrc={}; audio: payload_type={} ssrc={}",
            payload_type, ssrc, audio_payload_type, audio_ssrc
        );
        // 诊断：打印协商 SDP 关键行（m 行 / direction / rtpmap / fmtp / ssrc / msid）
        for line in answer_sdp.sdp.lines() {
            let l = line.trim();
            if l.starts_with("m=")
                || l.starts_with("a=send")
                || l.starts_with("a=recv")
                || l.starts_with("a=rtpmap")
                || l.starts_with("a=fmtp")
                || l.starts_with("a=ssrc")
                || l.starts_with("a=msid")
                || l.starts_with("a=group")
                || l.starts_with("a=extmap")
            {
                debug!("answer sdp: {}", l);
            }
        }

        let vs = Self {
            viewer_id: uuid::Uuid::new_v4().simple().to_string(),
            peer,
            track,
            running,
            ts_base: Arc::new(Mutex::new(None)),
            last_ts: Arc::new(Mutex::new(0)),
            config_nalu: Arc::new(Mutex::new(initial_frames.as_ref().and_then(|f| {
                f.iter()
                    .find(|x| x.is_config)
                    .map(|x| Bytes::from(x.data.clone()))
            }))),
            last_serve: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            answer: answer_sdp,
            peer_closed_rx,
            control_dc,
            session: session.clone(),
        };
        let fps = session
            .device
            .fps
            .or_else(|| (cfg.fps > 0).then_some(cfg.fps));
        // 静止补帧心跳（2026-08-23 从 33ms 降到 500ms）：30fps 重复帧让 Chrome
        // jitter buffer 的统计持续累积，目标延迟随运行时间膨胀（实测静止 23min
        // 后 676ms，滚动突发后飙到 4.9s——包在到、帧在解码计数、画面却逐位冻结
        // /残缺花屏，见 AGENTS.md 已知坑）。降到 2fps 心跳只为维持链路活性：
        // 前端静默看门狗需要 bytesReceived 增长、api 看门狗需要 last_serve 新鲜。
        // 唤醒无延迟代价：新帧到达经 frame_notify 立即唤醒，不睡满 500ms。
        let idle_repeat_ms = 500u64;
        // 硬性帧率上限：即使设备端实际输出 60fps，pusher 也按这里的最小间隔发送，
        // 避免“设置了 30fps 实际却跑到 60fps”（scrcpy 侧不再传 max_fps，见 scrcpy.rs）
        let min_frame_interval_ms = fps
            .filter(|&f| f > 0)
            .map(|f| (1000 / f).max(1) as u64)
            .unwrap_or(0);
        vs.spawn_pusher(
            rtp_sender,
            frame_q,
            frame_notify,
            overflowed,
            payload_type,
            ssrc,
            initial_frames,
            conn_rx,
            peer_connected.clone(),
            idle_repeat_ms,
            min_frame_interval_ms,
            cfg.ffmpeg_path.clone(),
            cfg.probe_encoder,
        );
        vs.spawn_audio_pusher(
            audio_track,
            audio_rx,
            audio_payload_type,
            audio_ssrc,
            peer_connected,
            audio_on,
        );
        Ok(vs)
    }
}

impl ViewerSession {
    /// 返回本地 answer SDP（协商时已保存，直接返回）
    pub fn local_description(&self) -> RTCSessionDescription {
        self.answer.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnect_reasons_have_stable_names_and_notification_policy() {
        let cases = [
            (
                ViewerDisconnectReason::TakenOver,
                "taken_over",
                Some("taken_over"),
            ),
            (
                ViewerDisconnectReason::DeviceDisconnected,
                "device_disconnected",
                None,
            ),
            (ViewerDisconnectReason::Shutdown, "shutdown", None),
            (ViewerDisconnectReason::PeerClosed, "peer_closed", None),
        ];

        for (reason, name, notification_type) in cases {
            assert_eq!(reason.as_str(), name);
            assert_eq!(reason.notification_type(), notification_type);
        }
    }

    #[tokio::test]
    async fn remove_and_teardown_viewer_clears_map_and_stops_running() {
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let handle = ViewerHandle {
            running: running.clone(),
            peer: std::sync::Weak::new(),
            control_dc: Arc::new(Mutex::new(None)),
            viewer_id: "viewer-a".to_string(),
            last_serve: Arc::new(std::sync::atomic::AtomicI64::new(123)),
            notify: Arc::new(Mutex::new(Some(tx))),
        };
        let viewers: ViewerMap = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        viewers.lock().unwrap().insert("dev1".to_string(), handle);

        assert!(
            remove_and_teardown_viewer(
                &viewers,
                "dev1",
                ViewerDisconnectReason::DeviceDisconnected
            )
            .await
        );
        assert!(viewers.lock().unwrap().is_empty());
        assert!(!running.load(std::sync::atomic::Ordering::SeqCst));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn takeover_reason_emits_taken_over_notification() {
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let handle = ViewerHandle {
            running: running.clone(),
            peer: std::sync::Weak::new(),
            control_dc: Arc::new(Mutex::new(None)),
            viewer_id: "viewer-b".to_string(),
            last_serve: Arc::new(std::sync::atomic::AtomicI64::new(123)),
            notify: Arc::new(Mutex::new(Some(tx))),
        };

        teardown_viewer(handle, ViewerDisconnectReason::TakenOver).await;
        assert!(!running.load(std::sync::atomic::Ordering::SeqCst));
        let msg = rx.try_recv().expect("taken_over notification");
        assert!(msg.contains("\"taken_over\""));
    }
}
