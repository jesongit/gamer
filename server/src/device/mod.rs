//! 设备管理：设备注册表 + scrcpy 会话生命周期 + 帧广播
pub mod adb;
pub mod frames;
pub mod scrcpy;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tokio::sync::broadcast;
use tokio::sync::Mutex as TokioMutex;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::store::{Db, Device, ScreenMode};

use self::adb::Adb;
use self::frames::FrameCache;
use self::scrcpy::{AudioFrame, ScrcpySession, SessionHandle, VideoFrame};

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    Offline,
    Connecting,
    Online,
}

/// 单个设备的运行时状态
pub struct DeviceRuntime {
    pub device: Device,
    pub status: DeviceStatus,
    pub session: Option<Arc<ScrcpySession>>,
    pub frames: Option<broadcast::Sender<VideoFrame>>,
    pub audio_frames: Option<broadcast::Sender<AudioFrame>>,
    pub frame_cache: Option<Arc<FrameCache>>,
    /// 防并发连接
    pub connecting: Arc<TokioMutex<bool>>,
    pub error: Option<String>,
}

/// 设备管理器
pub struct DeviceManager {
    pub db: Db,
    pub cfg: Config,
    pub adb: Adb,
    pub devices: RwLock<HashMap<String, DeviceRuntime>>,
}

impl DeviceManager {
    pub fn new(db: Db, cfg: Config) -> Self {
        let adb = Adb::new(&cfg);
        Self { db, cfg, adb, devices: RwLock::new(HashMap::new()) }
    }

    pub async fn start(&self) -> anyhow::Result<()> {
        let list = self.db.list_devices()?;
        for d in list {
            self.devices.write().insert(
                d.id.clone(),
                DeviceRuntime {
                    device: d,
                    status: DeviceStatus::Offline,
                    session: None,
                    frames: None,
                    audio_frames: None,
                    frame_cache: None,
                    connecting: Arc::new(TokioMutex::new(false)),
                    error: None,
                },
            );
        }
        info!("device manager started, {} devices registered", self.devices.read().len());
        Ok(())
    }

    /// 娉ㄥ唽/鏇存柊璁惧
    pub async fn upsert_device(&self, device: &Device) -> anyhow::Result<()> {
        self.db.upsert_device(device)?;
        self.devices
            .write()
            .entry(device.id.clone())
            .or_insert_with(|| DeviceRuntime {
                device: device.clone(),
                status: DeviceStatus::Offline,
                session: None,
                frames: None,
                audio_frames: None,
                frame_cache: None,
                connecting: Arc::new(TokioMutex::new(false)),
                error: None,
            })
            .device = device.clone();
        Ok(())
    }

    pub async fn delete_device(&self, id: &str) -> anyhow::Result<()> {
        self.disconnect_device(id).await;
        self.db.delete_device(id)?;
        self.devices.write().remove(id);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<parking_lot::RwLockReadGuard<'_, DeviceRuntime>> {
        let map = self.devices.read();
        // 直接返回 guard 需要特殊处理，这里简化
        drop(map);
        let _ = id;
        None
    }

    /// 获取设备运行时（克隆信息）
    pub fn snapshot(&self, id: &str) -> Option<(Device, DeviceStatus, Option<String>)> {
        let map = self.devices.read();
        let rt = map.get(id)?;
        Some((rt.device.clone(), rt.status, rt.error.clone()))
    }

    pub fn list_snapshot(&self) -> Vec<(Device, DeviceStatus, Option<String>)> {
        let map = self.devices.read();
        map.values()
            .map(|rt| (rt.device.clone(), rt.status, rt.error.clone()))
            .collect()
    }

    /// 连接设备（建立 scrcpy 会话 + 帧分发）
    pub async fn connect_device(&self, id: &str) -> anyhow::Result<()> {
        let (device, busy) = {
            let map = self.devices.read();
            let rt = map.get(id).ok_or_else(|| anyhow::anyhow!("device not found: {}", id))?;
            if rt.status == DeviceStatus::Online {
                return Ok(()); // 已连接
            }
            (rt.device.clone(), rt.connecting.clone())
        };
        let _guard = busy.lock().await;
        // 再次检查
        {
            let map = self.devices.read();
            if let Some(rt) = map.get(id) {
                if rt.status == DeviceStatus::Online {
                    return Ok(());
                }
            }
        }

        self.set_status(id, DeviceStatus::Connecting, None);

        info!(device = %device.name, "connecting...");
        let result = ScrcpySession::connect(&self.adb, &self.cfg, &device).await;
        let handle: SessionHandle = match result {
            Ok(h) => h,
            Err(e) => {
                error!(device = %device.name, "connect failed: {:#}", e);
                self.set_status(id, DeviceStatus::Offline, Some(e.to_string()));
                return Err(e);
            }
        };

        // 帧广播 + 帧缓存（ffmpeg 解码供模板匹配）
        let (tx, _) = broadcast::channel::<VideoFrame>(128);
        // 音频广播（OPUS 帧 → WebRTC 音频轨）
        let (audio_tx, _) = broadcast::channel::<AudioFrame>(128);
        let frame_cache = if self.cfg.decode_frames {
            match FrameCache::start(device.clone(), &self.cfg.ffmpeg_path) {
                Ok(fc) => Some(fc),
                Err(e) => {
                    warn!(device = %device.name, "frame cache unavailable: {}", e);
                    None
                }
            }
        } else {
            None
        };

        // 后台消费视频帧：广播 + 帧缓存
        let session = handle.session.clone();
        let s2 = session.clone();
        let device_name = device.name.clone();
        let dn1 = device_name.clone();
        let tx2 = tx.clone();
        let frame_cache2 = frame_cache.clone();
        tokio::spawn(async move {
            let mut rx = handle.video_rx;
            let cache = frame_cache2;
            while let Some(frame) = rx.recv().await {
                if let Some(fc) = &cache {
                    fc.feed(&frame);
                }
                // 诊断：广播 send 结果（接收者数 / 错误）。
                // 注意：无任何 viewer 时 tokio broadcast 返回 Err(SendError)（含整帧数据），
                // 不能打印 e（会刷巨型日志），只记录计数。
                match tx2.send(frame) {
                    Ok(0) => debug!(device = %dn1, "broadcast: no receivers"),
                    Ok(_) => {}
                    Err(_) => debug!(device = %dn1, "broadcast: no viewers, frame skipped"),
                }
                if !s2.connected.load(std::sync::atomic::Ordering::SeqCst) {
                    warn!(device = %dn1, "session disconnected");
                    break;
                }
            }
            drop(cache);
        });

        // 音频消费任务：scrcpy 音频帧 → 广播给各 viewer 的音频 pusher
        let session_a = session.clone();
        let audio_tx2 = audio_tx.clone();
        let dn_a = device_name.clone();
        tokio::spawn(async move {
            let mut rx = handle.audio_rx;
            while let Some(frame) = rx.recv().await {
                match audio_tx2.send(frame) {
                    Ok(0) => debug!(device = %dn_a, "audio broadcast: no receivers"),
                    Ok(_) => {}
                    Err(_) => debug!(device = %dn_a, "audio broadcast: no viewers, frame skipped"),
                }
                if !session_a.connected.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
            }
            info!(device = %dn_a, "audio broadcast task ended");
        });

        {
            let mut map = self.devices.write();
            if let Some(rt) = map.get_mut(id) {
                rt.session = Some(session.clone());
                rt.frames = Some(tx);
                rt.audio_frames = Some(audio_tx);
                rt.frame_cache = frame_cache;
                rt.status = DeviceStatus::Online;
                rt.error = None;
                if rt.device.pkg.is_some() {
                    // 连接成功后自动拉起游戏（异步，不阻塞）
                    let s3 = session.clone();
                    let pkg = rt.device.pkg.clone().unwrap();
                    let dn = device_name.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(1500)).await;
                        if let Err(e) = s3.start_app(&pkg).await {
                            warn!(device = %dn, "auto start app {} failed: {}", pkg, e);
                        } else {
                            info!(device = %dn, "auto started app {}", pkg);
                        }
                    });
                }
            }
        }
        info!(device = %device.name, "online");

        // 虚拟屏音频：scrcpy 以 audio_source=output 捕获虚拟屏音频（路由到
        // remote_submix），不进真机扬声器——不再需要静音媒体音量（静音会
        // 误伤真机其他用途，如用户自己听歌）。

        // 镜像会话期间保持屏幕唤醒：
        // 手机屏幕休眠后 Android 显示管线停止出帧，scrcpy H.264 流静默断流，
        // 浏览器画面定格在最后一帧（"连接后画面卡住"）。
        // 策略：连接时把 screen_off_timeout 调到最大 + 唤醒一次（解锁尽力而为）；
        // 每 30s 补一次唤醒兜底；会话结束（connected=false）时恢复原超时值。
        {
            let adb2 = self.adb.clone();
            let serial2 = device.addr.clone();
            let s3 = session.clone();
            let dn2 = device_name.clone();
            tokio::spawn(async move {
                let orig = adb2
                    .shell(&serial2, "settings get system screen_off_timeout", Duration::from_secs(8))
                    .await
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                if !orig.is_empty() && orig != "null" {
                    let _ = adb2
                        .shell(&serial2, "settings put system screen_off_timeout 2147483647", Duration::from_secs(8))
                        .await;
                }
                // KEYCODE_WAKEUP（224）：屏幕若已熄则唤醒（唤醒后锁屏也会出帧）
                let _ = adb2.shell(&serial2, "input keyevent 224", Duration::from_secs(8)).await;
                // 无 PIN 锁时直接进桌面；有 PIN 锁则停在锁屏（画面仍实时）
                let _ = adb2.shell(&serial2, "wm dismiss-keyguard", Duration::from_secs(8)).await;
                loop {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    if !s3.connected.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    let _ = adb2.shell(&serial2, "input keyevent 224", Duration::from_secs(8)).await;
                }
                if !orig.is_empty() && orig != "null" {
                    let _ = adb2
                        .shell(&serial2, &format!("settings put system screen_off_timeout {}", orig), Duration::from_secs(8))
                        .await;
                }
                info!(device = %dn2, orig_timeout = %orig, "screen keepalive stopped, screen_off_timeout restored");
            });
        }
        Ok(())
    }

    pub async fn disconnect_device(&self, id: &str) {
        let mut map = self.devices.write();
        let Some(rt) = map.get_mut(id) else { return };
        // 立即标记会话结束：屏幕保活任务 / 帧消费任务据此退出，及时恢复熄屏超时
        if let Some(s) = &rt.session {
            s.connected.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        // 停止帧缓存（退出专用线程、杀 ffmpeg），避免重连时线程/子进程泄漏
        if let Some(fc) = &rt.frame_cache {
            fc.stop();
        }
        rt.session = None;
        rt.frames = None;
        rt.audio_frames = None;
        rt.frame_cache = None;
        rt.status = DeviceStatus::Offline;
        drop(map);
        // 注意：scrcpy server 端 cleanup=true，socket 关闭即清理
    }

    /// 在线设备及其会话（供视频静默看门狗检测断流）
    pub fn online_sessions(&self) -> Vec<(String, Arc<ScrcpySession>)> {
        let map = self.devices.read();
        map.iter()
            .filter(|(_, rt)| rt.status == DeviceStatus::Online)
            .filter_map(|(id, rt)| rt.session.clone().map(|s| (id.clone(), s)))
            .collect()
    }

    fn set_status(&self, id: &str, status: DeviceStatus, error: Option<String>) {
        let mut map = self.devices.write();
        if let Some(rt) = map.get_mut(id) {
            rt.status = status;
            rt.error = error;
        }
    }

    /// 鑾峰彇浼氳瘽寮曠敤
    pub fn session(&self, id: &str) -> Option<Arc<ScrcpySession>> {
        let map = self.devices.read();
        map.get(id)?.session.clone()
    }

    /// 鑾峰彇甯у箍鎾彂閫佺锛圵ebRTC 璁㈤槄鐢級
    pub fn frames_tx(&self, id: &str) -> Option<broadcast::Sender<VideoFrame>> {
        let map = self.devices.read();
        map.get(id)?.frames.clone()
    }

    /// 获取音频广播发送端（WebRTC 音频轨订阅用）
    pub fn audio_frames_tx(&self, id: &str) -> Option<broadcast::Sender<AudioFrame>> {
        let map = self.devices.read();
        map.get(id)?.audio_frames.clone()
    }

    /// 鑾峰彇甯х紦瀛橈紙妯℃澘鍖归厤鎴浘婧愶級
    pub fn frame_cache(&self, id: &str) -> Option<Arc<FrameCache>> {
        let map = self.devices.read();
        map.get(id)?.frame_cache.clone()
    }

    /// 鎴浘锛圥NG锛夛細浼樺厛甯х紦瀛橈紝fallback adb screencap
    pub async fn screenshot(&self, id: &str) -> anyhow::Result<Vec<u8>> {
        let (device, cache) = {
            let map = self.devices.read();
            let rt = map.get(id).ok_or_else(|| anyhow::anyhow!("device not found"))?;
            (rt.device.clone(), rt.frame_cache.clone())
        };
        if let Some(fc) = cache {
            if let Some(png) = fc.latest_png() {
                return Ok(png);
            }
        }
        let serial = if device.addr.is_empty() { "usb".to_string() } else { device.addr.clone() };
        // 虚拟屏模式：screencap 默认截物理屏，需指定虚拟屏 display id
        if device.screen_mode == ScreenMode::Virtual {
            if let Some(did) = self.virtual_display_id(&serial, device.vd_res.as_deref()).await {
                return self.adb.screencap_display(&serial, did).await;
            }
        }
        self.adb.screencap(&serial).await
    }

    /// 解析虚拟屏 display id（dumpsys display 中 type=VIRTUAL 且分辨率匹配 scrcpy 虚拟屏）
    async fn virtual_display_id(&self, serial: &str, vd_res: Option<&str>) -> Option<i64> {
        let out = self.adb.shell(serial, "dumpsys display", Duration::from_secs(10)).await.ok()?;
        let (vw, vh) = parse_vd_size(vd_res.unwrap_or("1920x1080"))?;
        // mViewports=[DisplayViewport{...}, DisplayViewport{...}] 单行包含多个，需分段解析
        for seg in out.split("DisplayViewport{") {
            if !seg.contains("type=VIRTUAL") {
                continue;
            }
            let did = seg.split("displayId=").nth(1)?.split(',').next()?.trim().parse::<i64>().ok()?;
            if let Some(rect) = extract_rect(seg) {
                if rect == (vw, vh) || rect == (vh, vw) {
                    return Some(did);
                }
            }
        }
        None
    }
}

/// 解析 "1920x1080" → (1920, 1080)
fn parse_vd_size(s: &str) -> Option<(i64, i64)> {
    let mut it = s.split('x');
    let w = it.next()?.trim().parse::<i64>().ok()?;
    let h = it.next()?.trim().parse::<i64>().ok()?;
    Some((w, h))
}

/// 从 DisplayViewport 行提取 logicalFrame 尺寸
/// 格式: logicalFrame=Rect(0, 0 - 1920, 1080)
fn extract_rect(line: &str) -> Option<(i64, i64)> {
    let part = line.split("logicalFrame=Rect(").nth(1)?;
    let inner = part.split(')').next()?;
    let right = inner.split('-').nth(1)?.trim();
    let mut it = right.split(',');
    let x2 = it.next()?.trim().parse::<i64>().ok()?;
    let y2 = it.next()?.trim().parse::<i64>().ok()?;
    Some((x2, y2))
}
