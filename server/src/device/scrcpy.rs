//! scrcpy 客户端协议实现（对齐官方 v3.3.3）
//!
//! 服务端（scrcpy-server，官方开源 jar）在设备上通过 app_process 运行，
//! 我们扮演客户端角色：adb reverse 建隧道 → accept video/control socket →
//! 解析 H.264 视频流 + 注入控制消息。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::store::{Device, ScreenMode};

use super::adb::Adb;

/// 官方锁定版本（server 要求与客户端版本严格一致）
pub const SCRCPY_VERSION: &str = "3.3.3";
const DEVICE_NAME_LEN: usize = 64;
const VIDEO_META_LEN: usize = 12;

// 视频包标志位
const PACKET_FLAG_CONFIG: u64 = 1 << 63;
const PACKET_FLAG_KEY_FRAME: u64 = 1 << 62;

/// 触控动作（对应 Android MotionEvent）
pub const ACTION_DOWN: u8 = 0;
pub const ACTION_UP: u8 = 1;
pub const ACTION_MOVE: u8 = 2;

/// 一帧视频（H.264 Annex-B / 原始编码器输出）
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub data: Vec<u8>,
    pub pts_us: u64,
    pub is_config: bool,
    pub is_keyframe: bool,
    /// 视频编码器输出是否为 Annex-B 格式（含 start code）
    pub annex_b: bool,
}

/// 一帧音频（OPUS 编码，48kHz 立体声，每帧 ~20ms）
#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub data: Vec<u8>,
    pub pts_us: u64,
    /// OPUS 参数集帧（OpusHead，scrcpy 包装成 AOPUSHDR）——WebRTC 用 SDP fmtp
    /// 即可解码，无需转发该帧，但保留标记便于日志/调试
    pub is_config: bool,
}

/// 视频流元信息
#[derive(Debug, Clone)]
pub struct VideoMeta {
    pub codec_id: u32,
    pub width: u32,
    pub height: u32,
    pub device_name: String,
}

pub const CODEC_H264: u32 = 0x68323634; // "h264"

/// scrcpy 会话：一条已建立的设备连接
pub struct ScrcpySession {
    pub device: Device,
    pub meta: Mutex<Option<VideoMeta>>,
    /// tokio Mutex：控制 socket 写入可能跨 await
    control: tokio::sync::Mutex<Option<TcpStream>>,
    /// 设备分辨率（虚拟屏模式下 = 虚拟屏分辨率）
    pub width: Mutex<u32>,
    pub height: Mutex<u32>,
    pub connected: Arc<std::sync::atomic::AtomicBool>,
    /// 帧计数（用于 RTP 时间戳）
    pub frame_seq: Mutex<u64>,
    /// 最近一帧视频的到达时间（unix 微秒；0 = 尚无帧），
    /// 供视频静默看门狗检测断流并自动重连
    pub last_frame_at: std::sync::atomic::AtomicU64,
}

impl ScrcpySession {
    /// 距最近一帧的毫秒数（尚无帧返回 0，视为新鲜，给足首帧窗口）
    pub fn video_idle_ms(&self) -> u64 {
        let last = self.last_frame_at.load(std::sync::atomic::Ordering::SeqCst);
        if last == 0 {
            return 0;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0);
        now.saturating_sub(last) / 1000
    }

    /// 当前视频分辨率（虚拟屏模式下 = 虚拟屏分辨率）
    pub fn video_size(&self) -> (u32, u32) {
        (*self.width.lock(), *self.height.lock())
    }
}

/// connect() 的返回值：会话 + 视频帧接收端 + 音频帧接收端
pub struct SessionHandle {
    pub session: Arc<ScrcpySession>,
    pub video_rx: mpsc::Receiver<VideoFrame>,
    pub audio_rx: mpsc::Receiver<AudioFrame>,
}

impl ScrcpySession {
    /// 建立 scrcpy 会话：push server → reverse 隧道 → 启动 → accept 两个 socket
    pub async fn connect(adb: &Adb, cfg: &Config, device: &Device) -> anyhow::Result<SessionHandle> {
        let adb = adb.clone();
        let serial = if device.addr.is_empty() { "usb".to_string() } else { device.addr.clone() };
        info!(device = %device.name, serial = %serial, "connecting scrcpy session");

        // 1. 确保 adb transport 可用。只有网络地址（IP:port）才用 adb connect；
        //    USB serial / mDNS 名传给 adb connect 会被当主机名解析 →
        //    "cannot resolve host" 假错误，掩盖真实的 offline/未授权/拔出状态。
        //    USB/mDNS 掉到 offline 时先 adb reconnect offline 恢复一次（免拔线）
        if !adb.is_connected(&serial).await {
            if device.addr.contains(':') {
                adb.connect(&device.addr).await?;
            } else {
                let _ = adb.run(&["reconnect", "offline"], Duration::from_secs(5)).await;
                tokio::time::sleep(Duration::from_millis(2500)).await;
            }
            if !adb.is_connected(&serial).await {
                anyhow::bail!(
                    "设备不在线：adb devices 中无 {}（offline/未授权/已拔出？USB 请重新插拔或重开 USB 调试，无线请确认无线调试已开启）",
                    serial
                );
            }
        }

        // 1.5 清理旧 reverse 隧道（残留隧道可能干扰新连接）
        let _ = adb.run(&["-s", &serial, "reverse", "--remove-all"], Duration::from_secs(10)).await;

        // 2. 生成 scid 与 socket 名
        let scid = rand::random::<u32>() & 0x7fffffff;
        let socket_name = format!("scrcpy_{:08x}", scid);

        // 3. 推送 scrcpy-server
        let server_path = cfg.scrcpy_server.to_string_lossy().to_string();
        adb.push(&serial, &server_path, "/data/local/tmp/scrcpy-server.jar").await?;

        // 4. 本地监听 + adb reverse 隧道
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let local_port = listener.local_addr()?.port();
        adb.reverse(&serial, &socket_name, local_port).await?;

        // 5. 构造 server 启动参数
        let mut args = vec![
            SCRCPY_VERSION.to_string(),
            format!("scid={:08x}", scid),
            "log_level=debug".into(),
            "video=true".into(),
            // 音频：audio_source=output → 虚拟屏音频路由到 remote_submix（Android 13+），
            // 不进真机扬声器（audio_dup=false 时不复制到设备输出）——这是
            // "虚拟屏声音不在真机播放"的正确机制（静音会误伤真机其他用途）。
            // 服务端接收音频流后当前丢弃；如需浏览器出声，接 WebRTC 音频 track 转发。
            "audio=true".into(),
            "audio_source=output".into(),
            "audio_dup=false".into(),
            "audio_codec=opus".into(),
            "audio_bit_rate=128000".into(),
            "control=true".into(),
            format!("video_bit_rate={}", cfg.bitrate_mbps * 1_000_000),
            "send_codec_meta=true".into(),
            "send_device_meta=true".into(),
            "send_frame_meta=true".into(),
            "cleanup=true".into(),
            // 关键帧间隔 2s：WebRTC 新 viewer 连接后需要定期 IDR 才能快速出画面
            // （静态画面下编码器默认长时间不产 IDR，浏览器将无法开始解码）；
            // 同时也限制每秒关键帧 burst 对浏览器 jitter buffer 目标延迟的扰动
            // （1s 时实测 perF 缓慢爬升，见 AGENTS.md 已知坑；2s 后扰动减半，
            // 断链跳帧恢复 ≤2s 可接受）
            // repeat-previous-headers=1：每个关键帧前重复 SPS/PPS，
            // 浏览器即使错过连接瞬间的参数集也能随时开始解码
            "video_codec_options=i-frame-interval=2,repeat-previous-headers=1".into(),
        ];
        // 与官方客户端一致：0 值参数不传（max_fps=0 会导致 server 端除零）
        if cfg.max_size > 0 {
            args.push(format!("max_size={}", cfg.max_size));
        }
        // 帧率：不再向设备传 max_fps！实测 Redmi 25079RPDCC (MTK c2.mtk.avc.encoder)：
        //   传 max_fps=15/30/60 时，编码器配合 repeat-previous-frame-after 输出大量
        //   空 keep-alive 缓冲（size==0），scrcpy 跳过空缓冲 → 服务端只收到 ~1fps；
        //   不传该 key 时编码器按自身默认节奏输出真实帧（实测 ~15fps，15 倍提升）。
        //   帧率上限改由服务端 pusher 控制（pusher 最小帧间隔硬限 + 静止补帧 idle_repeat）。
        // 注：全局配置 cfg.fps 与设备 fps 仍用于 pusher 的补帧/节流节奏（见 webrtc.rs）。
        // 屏幕模式：虚拟屏 or 镜像主屏
        match &device.screen_mode {
            ScreenMode::Virtual => {
                let (w, h) = parse_vd_res(device.vd_res.as_deref().unwrap_or("1920x1080"));
                let dpi = device.vd_dpi.unwrap_or(0);
                let vd = if dpi > 0 {
                    format!("{}x{}/{}", w, h, dpi)
                } else {
                    format!("{}x{}", w, h)
                };
                args.push(format!("new_display={}", vd));
                // server 持唤醒锁防止系统超时睡眠：主屏真睡眠会让副屏应用被冻结
                // （见 set_display_power 注释/AGENTS.md 已知坑），软关屏的前提是
                // 逻辑显示始终 ON——原生 scrcpy --turn-screen-off 同样隐含 stay-awake
                args.push("stay_awake=true".into());
                info!(device = %device.name, vd = %vd, "using virtual display");
            }
            ScreenMode::Mirror => {
                args.push("display_id=0".into());
            }
        }

        let shell_cmd = format!(
            "CLASSPATH=/data/local/tmp/scrcpy-server.jar app_process / com.genymobile.scrcpy.Server {}",
            args.join(" ")
        );
        debug!(device = %device.name, "starting scrcpy-server: {}", shell_cmd);

        // 6. 后台启动 server（不等待，socket 连接后即视为成功）；输出打到日志便于排障
        let device_name = device.name.clone();
        adb.shell_logged(&serial, &shell_cmd, &format!("scrcpy-{}", device_name));

        // 7. accept 顺序必须与 server 端一致：video → audio → control
        let (video, _) = accept_with_timeout(&listener, Duration::from_secs(15))
            .await
            .ok_or_else(|| anyhow::anyhow!("accept video socket timeout"))?;
        info!(device = %device.name, "video socket accepted");
        let (audio, _) = accept_with_timeout(&listener, Duration::from_secs(15))
            .await
            .ok_or_else(|| anyhow::anyhow!("accept audio socket timeout"))?;
        info!(device = %device.name, "audio socket accepted");
        let (control, _) = accept_with_timeout(&listener, Duration::from_secs(15))
            .await
            .ok_or_else(|| anyhow::anyhow!("accept control socket timeout"))?;
        info!(device = %device.name, "control socket accepted");

        // 8. 读设备名 + 视频元信息
        // 注意：不能 into_split() 丢弃写半——tokio 的 OwnedWriteHalf drop 时会发送 FIN，
        // 导致 scrcpy server 关闭连接。video socket 是单向的（server→client），整个 TcpStream 保留即可。
        let mut video_reader = video;
        let mut device_name_buf = vec![0u8; DEVICE_NAME_LEN];
        video_reader.read_exact(&mut device_name_buf).await?;
        let device_name = String::from_utf8_lossy(&device_name_buf)
            .trim_end_matches('\0')
            .trim()
            .to_string();
        let mut meta_buf = [0u8; VIDEO_META_LEN];
        video_reader.read_exact(&mut meta_buf).await?;
        let codec_id = u32::from_be_bytes(meta_buf[0..4].try_into()?);
        let width = u32::from_be_bytes(meta_buf[4..8].try_into()?);
        let height = u32::from_be_bytes(meta_buf[8..12].try_into()?);
        info!(device = %device.name, codec_id, width, height, "video meta received");
        let meta = VideoMeta { codec_id, width, height, device_name };

        let (video_tx, video_rx) = mpsc::channel::<VideoFrame>(64);
        let session = Arc::new(Self {
            device: device.clone(),
            meta: Mutex::new(Some(meta)),
            control: tokio::sync::Mutex::new(Some(control)),
            width: Mutex::new(width),
            height: Mutex::new(height),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            frame_seq: Mutex::new(0),
            last_frame_at: std::sync::atomic::AtomicU64::new(0),
        });

        // 9. 视频读取循环（后台任务）
        let s2 = session.clone();
        tokio::spawn(async move {
            let mut reader = video_reader;
            let mut buf = Vec::with_capacity(256 * 1024);
            let mut frame_count: u64 = 0;
            // 诊断：前 200ms 不消费 video_tx（模拟背压前状态）
            loop {
                // 读 12 字节帧头
                let mut header = [0u8; 12];
                match reader.read_exact(&mut header).await {
                    Ok(_) => {}
                    Err(e) => {
                        warn!(
                            device = %s2.device.name,
                            frames = frame_count,
                            kind = ?e.kind(),
                            raw = e.raw_os_error(),
                            err = %e,
                            "video socket closed"
                        );
                        break;
                    }
                }
                let pts_and_flags = u64::from_be_bytes(header[0..8].try_into().unwrap());
                let size = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;
                if size > 10 * 1024 * 1024 {
                    warn!(device = %s2.device.name, size, "oversized video packet, aborting");
                    break;
                }
                buf.resize(size, 0);
                match reader.read_exact(&mut buf).await {
                    Ok(_) => {}
                    Err(e) => {
                        warn!(device = %s2.device.name, frames = frame_count, err = %e, "video socket closed mid-frame");
                        break;
                    }
                }
                frame_count += 1;
                s2.last_frame_at.store(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_micros() as u64)
                        .unwrap_or(0),
                    std::sync::atomic::Ordering::SeqCst,
                );
                let frame = VideoFrame {
                    data: buf.clone(),
                    pts_us: pts_and_flags & !(PACKET_FLAG_CONFIG | PACKET_FLAG_KEY_FRAME),
                    is_config: pts_and_flags & PACKET_FLAG_CONFIG != 0,
                    is_keyframe: pts_and_flags & PACKET_FLAG_KEY_FRAME != 0,
                    annex_b: true,
                };
                // 关键帧日志（验证 i-frame-interval 生效）与周期采样
                if frame_count <= 3 || frame_count % 300 == 0 || frame.is_keyframe {
                    debug!(device = %s2.device.name, frame_count, size, config = frame.is_config, key = frame.is_keyframe, "frame");
                }
                // 诊断：SPS/PPS 配置帧打印前 40 字节（含 profile/level，验证与协商 fmtp 匹配）
                if frame.is_config {
                    let hex: String = frame.data.iter().take(40).map(|b| format!("{:02x}", b)).collect();
                    info!(device = %s2.device.name, "config frame {} bytes: {}", frame.data.len(), hex);
                }
                if video_tx.send(frame).await.is_err() {
                    debug!(device = %s2.device.name, "video channel consumer closed");
                    break; // 消费者已关闭
                }
            }
            s2.connected.store(false, std::sync::atomic::Ordering::SeqCst);
        });

        // 9.5 音频读取循环（后台任务）：解析 OPUS 帧 → audio channel 转发给 viewer。
        // 流格式与视频相同：4B codec meta（send_codec_meta=true）→ [12B 帧头 + 数据]*
        let (audio_tx, audio_rx) = mpsc::channel::<AudioFrame>(256);
        let s_audio = session.clone();
        tokio::spawn(async move {
            let mut reader = audio;
            let mut codec_buf = [0u8; 4];
            if reader.read_exact(&mut codec_buf).await.is_err() {
                debug!(device = %s_audio.device.name, "audio socket closed before codec meta");
                return;
            }
            let codec_id = u32::from_be_bytes(codec_buf);
            info!(device = %s_audio.device.name, codec_id, "audio meta received");
            let mut frames: u64 = 0;
            let mut bytes: u64 = 0;
            let mut header = [0u8; 12];
            loop {
                if reader.read_exact(&mut header).await.is_err() {
                    break;
                }
                let pts_and_flags = u64::from_be_bytes(header[0..8].try_into().unwrap());
                let size = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;
                if size > 10 * 1024 * 1024 {
                    warn!(device = %s_audio.device.name, size, "oversized audio packet, aborting");
                    break;
                }
                let mut buf = vec![0u8; size];
                if reader.read_exact(&mut buf).await.is_err() {
                    break;
                }
                frames += 1;
                bytes += size as u64;
                let is_config = pts_and_flags & PACKET_FLAG_CONFIG != 0;
                let pts = pts_and_flags & !(PACKET_FLAG_CONFIG | PACKET_FLAG_KEY_FRAME);
                let frame = AudioFrame { data: buf, pts_us: pts, is_config };
                if audio_tx.send(frame).await.is_err() {
                    break; // 消费者已关闭
                }
                if frames % 1000 == 1 || frames <= 3 {
                    info!(device = %s_audio.device.name, frames, bytes, config = is_config, pts = pts, "audio frame");
                }
            }
            info!(device = %s_audio.device.name, frames, bytes, "audio stream ended");
        });

        Ok(SessionHandle { session, video_rx, audio_rx })
    }

    // ---------- 控制消息注入 ----------

    async fn send_control(&self, msg: &[u8]) -> anyhow::Result<()> {
        let mut guard = self.control.lock().await;
        if let Some(sock) = guard.as_mut() {
            sock.write_all(msg).await?;
            Ok(())
        } else {
            anyhow::bail!("control socket closed")
        }
    }

    /// 注入触控：action=DOWN/UP/MOVE，坐标基于视频分辨率（0..width, 0..height）
    pub async fn inject_touch(&self, action: u8, pointer_id: u64, x: f32, y: f32, pressure: f32) -> anyhow::Result<()> {
        let (w, h) = (*self.width.lock(), *self.height.lock());
        let x = (x.max(0.0).min(w as f32 - 1.0)) as u32;
        let y = (y.max(0.0).min(h as f32 - 1.0)) as u32;
        let mut buf = [0u8; 32];
        buf[0] = 2; // TYPE_INJECT_TOUCH_EVENT
        buf[1] = action;
        buf[2..10].copy_from_slice(&pointer_id.to_be_bytes());
        buf[10..14].copy_from_slice(&x.to_be_bytes());
        buf[14..18].copy_from_slice(&y.to_be_bytes());
        buf[18..20].copy_from_slice(&(w as u16).to_be_bytes());
        buf[20..22].copy_from_slice(&(h as u16).to_be_bytes());
        let p = (pressure.clamp(0.0, 1.0) * 65535.0) as u16;
        buf[22..24].copy_from_slice(&p.to_be_bytes());
        // action_button / buttons = 0
        self.send_control(&buf).await
    }

    /// 单击（DOWN+UP）
    pub async fn tap(&self, x: f32, y: f32) -> anyhow::Result<()> {
        self.inject_touch(ACTION_DOWN, 0, x, y, 1.0).await?;
        tokio::time::sleep(Duration::from_millis(60)).await;
        self.inject_touch(ACTION_UP, 0, x, y, 0.0).await
    }

    /// 滑动
    pub async fn swipe(&self, x1: f32, y1: f32, x2: f32, y2: f32, duration_ms: u64) -> anyhow::Result<()> {
        self.inject_touch(ACTION_DOWN, 0, x1, y1, 1.0).await?;
        let steps = 20u64;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let x = x1 + (x2 - x1) * t;
            let y = y1 + (y2 - y1) * t;
            self.inject_touch(ACTION_MOVE, 0, x, y, 1.0).await?;
            tokio::time::sleep(Duration::from_millis(duration_ms / steps)).await;
        }
        self.inject_touch(ACTION_UP, 0, x2, y2, 0.0).await
    }

    /// 按键注入（Android keycode）
    pub async fn inject_keycode(&self, action: u8, keycode: u32, repeat: u32, meta: u32) -> anyhow::Result<()> {
        let mut buf = [0u8; 14];
        buf[0] = 0; // TYPE_INJECT_KEYCODE
        buf[1] = action;
        buf[2..6].copy_from_slice(&keycode.to_be_bytes());
        buf[6..10].copy_from_slice(&repeat.to_be_bytes());
        buf[10..14].copy_from_slice(&meta.to_be_bytes());
        self.send_control(&buf).await
    }

    /// 按键（按下+释放），如 HOME=3, BACK=4, APP_SWITCH=187
    pub async fn press_key(&self, keycode: u32) -> anyhow::Result<()> {
        self.inject_keycode(0, keycode, 0, 0).await?;
        tokio::time::sleep(Duration::from_millis(40)).await;
        self.inject_keycode(1, keycode, 0, 0).await
    }

    /// 文本输入（UTF-8）
    pub async fn inject_text(&self, text: &str) -> anyhow::Result<()> {
        let bytes = text.as_bytes();
        let len = bytes.len().min(300);
        let mut buf = Vec::with_capacity(5 + len);
        buf.push(1); // TYPE_INJECT_TEXT
        buf.extend_from_slice(&(len as u32).to_be_bytes());
        buf.extend_from_slice(&bytes[..len]);
        self.send_control(&buf).await
    }

    /// 滚轮（scroll_x/y 为像素值，会被 /16 归一化）
    pub async fn inject_scroll(&self, x: f32, y: f32, scroll_x: f32, scroll_y: f32) -> anyhow::Result<()> {
        let (w, h) = (*self.width.lock(), *self.height.lock());
        let mut buf = [0u8; 21];
        buf[0] = 3; // TYPE_INJECT_SCROLL_EVENT
        buf[1..5].copy_from_slice(&(x as u32).to_be_bytes());
        buf[5..9].copy_from_slice(&(y as u32).to_be_bytes());
        buf[9..11].copy_from_slice(&(w as u16).to_be_bytes());
        buf[11..13].copy_from_slice(&(h as u16).to_be_bytes());
        let hnorm = (scroll_x / 16.0).clamp(-1.0, 1.0);
        let vnorm = (scroll_y / 16.0).clamp(-1.0, 1.0);
        let hs = (hnorm * 32767.0) as i16;
        let vs = (vnorm * 32767.0) as i16;
        buf[13..15].copy_from_slice(&(hs as u16).to_be_bytes());
        buf[15..17].copy_from_slice(&(vs as u16).to_be_bytes());
        self.send_control(&buf).await
    }

    /// 返回键 / 点亮屏幕
    pub async fn back_or_screen_on(&self, action: u8) -> anyhow::Result<()> {
        self.send_control(&[4, action]).await
    }

    /// 旋转设备
    pub async fn rotate_device(&self) -> anyhow::Result<()> {
        self.send_control(&[11]).await
    }

    /// 请求设备重置视频编码（scrcpy ControlMsg type 17 RESET_VIDEO，无 payload）。
    /// server 收到后对 MediaCodec 调 signalEndOfInputStream()，编码器会立即输出
    /// 新的 SPS/PPS（config 帧）+ IDR 关键帧。
    /// 用途：新 viewer 连接时若帧缓存还没有 GOP（会话刚建立 / MTK 等关键帧稀疏
    /// 的设备），请求设备尽快产出可解码初始帧，避免浏览器拿不到参数集而黑屏。
    pub async fn reset_video(&self) -> anyhow::Result<()> {
        self.send_control(&[17]).await
    }

    /// 主屏软关屏/亮屏（scrcpy ControlMsg type 10 SET_DISPLAY_POWER，payload 1 字节
    /// bool）。new-display 会话下 server 端固定作用于**物理主屏**（Controller.
    /// setDisplayPower 对虚拟屏会话 target displayId=0）：仅关面板电源，逻辑显示
    /// 状态保持 ON。这是关键防冻结手段——主屏真睡眠（power 键/超时）时，副屏
    /// 应用会在 ~10-30s 无交互后被系统整体冻结（do_freezer_trap：输入注入超时、
    /// 零渲染，ANR trace 可证），面板级软关屏则不会触发（同原生 scrcpy
    /// --turn-screen-off，见 AGENTS.md 已知坑）。server 收到 on=false 时自动注册
    /// 退出恢复电源（cleanup restoreDisplayPower），会话销毁无需手动还原。
    pub async fn set_display_power(&self, on: bool) -> anyhow::Result<()> {
        self.send_control(&[10, on as u8]).await
    }

    /// 设置剪贴板
    pub async fn set_clipboard(&self, text: &str, paste: bool) -> anyhow::Result<()> {
        let bytes = text.as_bytes();
        let len = bytes.len().min(131072);
        let mut buf = Vec::with_capacity(10 + len);
        buf.push(9); // TYPE_SET_CLIPBOARD
        buf.extend_from_slice(&0u64.to_be_bytes()); // sequence
        buf.push(paste as u8);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
        buf.extend_from_slice(&bytes[..len]);
        self.send_control(&buf).await
    }

    /// 启动应用（new-display 模式下自动启动到虚拟屏）
    /// name 支持：包名；"+" 前缀先 force-stop；"?" 前缀按应用名搜索
    pub async fn start_app(&self, name: &str) -> anyhow::Result<()> {
        let bytes = name.as_bytes();
        let len = bytes.len().min(255);
        let mut buf = Vec::with_capacity(1 + len);
        buf.push(16); // TYPE_START_APP
        buf.push(len as u8);
        buf.extend_from_slice(&bytes[..len]);
        self.send_control(&buf).await
    }
}

fn parse_vd_res(s: &str) -> (u32, u32) {
    let parts: Vec<&str> = s.split('x').collect();
    if parts.len() == 2 {
        let w = parts[0].parse().unwrap_or(1920);
        let h = parts[1].parse().unwrap_or(1080);
        (w, h)
    } else {
        (1920, 1080)
    }
}

async fn accept_with_timeout(listener: &TcpListener, timeout: Duration) -> Option<(TcpStream, SocketAddr)> {
    match tokio::time::timeout(timeout, listener.accept()).await {
        Ok(Ok((s, a))) => Some((s, a)),
        _ => None,
    }
}
