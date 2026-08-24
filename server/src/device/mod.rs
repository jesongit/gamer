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
use crate::webrtc::ViewerMap;

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

/// 单设备空闲低功耗状态（idle_power_loop 维护）
struct IdleState {
    /// 空闲起始时刻；None = 当前有消费者（viewer/脚本）
    idle_since: Option<std::time::Instant>,
    /// 镜像模式已主动关屏（消费者回来时需唤醒）
    slept: bool,
    /// 镜像模式上次 WAKEUP 补醒时刻（30s 节流兜底）
    last_wake: std::time::Instant,
}

// ---------- 虚拟屏会话的设备侧状态改写（freezer 禁用 + 媒体静音） ----------

/// 崩溃自愈标记（data/pending_restore.json，键 = adb serial）：改写设备设置
/// **之前**落盘、恢复之后删除；进程被硬杀时设备侧不会自己还原，由服务启动
/// 时读该文件兜底恢复。
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct PendingRestore {
    /// cached_apps_freezer_enabled 原值；"null" = 原本未设置（恢复用 settings delete）
    freezer: Option<String>,
    /// STREAM_MUSIC 原音量索引
    media_volume: Option<u32>,
}

fn pending_restore_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("pending_restore.json")
}

fn load_pending(data_dir: &std::path::Path) -> HashMap<String, PendingRestore> {
    std::fs::read_to_string(pending_restore_path(data_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_pending(data_dir: &std::path::Path, map: &HashMap<String, PendingRestore>) {
    let p = pending_restore_path(data_dir);
    if map.is_empty() {
        let _ = std::fs::remove_file(p);
        return;
    }
    match serde_json::to_string_pretty(map) {
        Ok(s) => {
            if let Err(e) = std::fs::write(&p, s) {
                warn!("pending_restore.json 写入失败: {}", e);
            }
        }
        Err(e) => warn!("pending_restore.json 序列化失败: {}", e),
    }
}

/// 读 STREAM_MUSIC 音量索引。注意：HyperOS 上 `media` 命令不存在，
/// `cmd media_session volume --set/--adj` 报成功但实际不生效（实测），
/// 必须走 `cmd audio set-volume`。
async fn get_media_volume(adb: &Adb, addr: &str) -> Option<u32> {
    let out = adb
        .shell(addr, "cmd audio get-stream-volume 3", Duration::from_secs(8))
        .await
        .ok()?;
    // 输出形如 "AudioManager.getStreamVolume(3) -> 54"
    let num = out
        .split("-> ")
        .nth(1)?
        .trim()
        .split(|c: char| !c.is_ascii_digit())
        .next()?;
    num.parse().ok()
}

/// 虚拟屏会话建立时的设备侧改写：
/// ① cached_apps_freezer_enabled=0——Android 15 主屏真睡眠时 cached apps
///   freezer 会把虚拟屏应用整体冻结（do_freezer_trap，scrcpy #5604），禁用后
///   主屏可自然睡眠/唤醒，用户随时能按电源键用手机（取代旧的软关屏 +
///   stay_awake + 周期唤醒自愈，那套机制用户永远点不亮主屏）；
/// ② STREAM_MUSIC 静音——audio_source=output 的 remote submix 重定向在
///   HyperOS 上静默失效（扬声器照常外放，见 AGENTS 已知坑），音量置 0 是唯一
///   可靠的"手机不响"手段（副作用：浏览器音频大概率随媒体音量一起变静）。
/// 改写前先落盘标记，disconnect_device 统一恢复。
async fn apply_virtual_overrides(adb: &Adb, data_dir: &std::path::Path, addr: &str) {
    if addr.is_empty() {
        return;
    }
    let mut map = load_pending(data_dir);
    map.entry(addr.to_string()).or_default();

    if map[addr].freezer.is_none() {
        match adb
            .shell(addr, "settings get global cached_apps_freezer_enabled", Duration::from_secs(8))
            .await
        {
            Ok(cur) => {
                let cur = cur.trim().to_string();
                if cur != "0" {
                    map.get_mut(addr).unwrap().freezer = Some(cur);
                    save_pending(data_dir, &map);
                    if let Err(e) = adb
                        .shell(addr, "settings put global cached_apps_freezer_enabled 0", Duration::from_secs(8))
                        .await
                    {
                        warn!(addr = %addr, err = %e, "禁用 freezer 失败（主屏睡眠时虚拟屏应用可能被冻结）");
                    }
                }
            }
            Err(e) => warn!(addr = %addr, err = %e, "读取 freezer 设置失败"),
        }
    }

    if map[addr].media_volume.is_none() {
        match get_media_volume(adb, addr).await {
            Some(v) if v > 0 => {
                map.get_mut(addr).unwrap().media_volume = Some(v);
                save_pending(data_dir, &map);
                if let Err(e) = adb
                    .shell(addr, "cmd audio set-volume 3 0", Duration::from_secs(8))
                    .await
                {
                    warn!(addr = %addr, err = %e, "媒体音量静音失败");
                }
            }
            Some(_) => {}
            None => warn!(addr = %addr, "读取媒体音量失败（跳过静音）"),
        }
    }
    info!(addr = %addr, "虚拟屏会话设备侧改写完成（freezer 已禁用 + 媒体已静音）");
}

/// 恢复设备侧改写（disconnect_device 统一调用；无标记则零操作）。
/// 恢复失败（设备不在线等）保留标记，下次启动自愈/会话结束时再试。
async fn restore_virtual_overrides(adb: &Adb, data_dir: &std::path::Path, addr: &str) {
    if addr.is_empty() {
        return;
    }
    let mut map = load_pending(data_dir);
    let Some(mut entry) = map.remove(addr) else { return };
    let mut ok = true;
    if let Some(v) = entry.freezer.take() {
        let cmd = if v == "null" {
            "settings delete global cached_apps_freezer_enabled".to_string()
        } else {
            format!("settings put global cached_apps_freezer_enabled {}", v)
        };
        if let Err(e) = adb.shell(addr, &cmd, Duration::from_secs(8)).await {
            warn!(addr = %addr, err = %e, "恢复 freezer 设置失败");
            entry.freezer = Some(v);
            ok = false;
        }
    }
    if let Some(vol) = entry.media_volume.take() {
        if let Err(e) = adb
            .shell(addr, &format!("cmd audio set-volume 3 {}", vol), Duration::from_secs(8))
            .await
        {
            warn!(addr = %addr, err = %e, "恢复媒体音量失败");
            entry.media_volume = Some(vol);
            ok = false;
        }
    }
    if !ok {
        map.insert(addr.to_string(), entry);
    } else {
        info!(addr = %addr, "虚拟屏会话设备侧改写已恢复（freezer + 媒体音量）");
    }
    save_pending(data_dir, &map);
}

/// 启动自愈：进程曾被硬杀时设备侧改写无人恢复，读标记逐设备还原
async fn restore_all_pending(adb: &Adb, data_dir: &std::path::Path) {
    let map = load_pending(data_dir);
    if map.is_empty() {
        return;
    }
    warn!("发现 {} 条设备侧改写残留（上次进程异常退出？），正在恢复", map.len());
    for addr in map.keys().cloned().collect::<Vec<_>>() {
        restore_virtual_overrides(adb, data_dir, &addr).await;
    }
}

/// 设备管理器
pub struct DeviceManager {
    pub db: Db,
    pub cfg: Config,
    pub adb: Adb,
    pub devices: RwLock<HashMap<String, DeviceRuntime>>,
    /// 每设备活跃 viewer（空闲低功耗守卫：有 viewer = 有人在投屏，不进低功耗）
    pub viewers: ViewerMap,
    /// 每设备脚本运行计数（空闲低功耗守卫；归零时移除条目）
    run_counts: std::sync::Mutex<HashMap<String, u32>>,
    /// 空闲低功耗状态（idle_power_loop / notify_activity 共享）
    idle: std::sync::Mutex<HashMap<String, IdleState>>,
}

impl DeviceManager {
    pub fn new(db: Db, cfg: Config, viewers: ViewerMap) -> Self {
        let adb = Adb::new(&cfg);
        Self {
            db,
            cfg,
            adb,
            viewers,
            run_counts: std::sync::Mutex::new(HashMap::new()),
            idle: std::sync::Mutex::new(HashMap::new()),
            devices: RwLock::new(HashMap::new()),
        }
    }

    pub async fn start(self: &Arc<Self>) -> anyhow::Result<()> {
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

        // 崩溃自愈：进程曾被硬杀时，虚拟屏会话的设备侧改写（freezer/媒体音量）
        // 无人恢复——读 pending_restore.json 标记逐设备还原（设备不在线则留待下次）
        let dm0 = self.clone();
        tokio::spawn(async move {
            restore_all_pending(&dm0.adb, &dm0.cfg.data_dir).await;
        });

        // 启动自举 + adb 保活：低功耗空闲模式的基础——adb 链路常连
        // （WiFi/emu 设备周期补 adb connect，幂等），scrcpy 会话只在
        // 脚本运行/投屏时按需建立，空闲时设备侧不编码、正常熄屏
        let dm = self.clone();
        tokio::spawn(async move {
            match dm.scan_and_sync().await {
                Ok(n) => info!("startup scan synced, {} new device(s)", n),
                Err(e) => warn!("startup scan failed: {}", e),
            }
            dm.connect_wireless_adb().await;
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            loop {
                tick.tick().await;
                dm.connect_wireless_adb().await;
            }
        });

        // 空闲低功耗循环：会话存活的唯一管理者——周期检查"无 viewer 且无
        // 脚本运行"持续时长，超 idle_power_secs 后虚拟屏拆会话/镜像关屏
        let dm = self.clone();
        tokio::spawn(dm.idle_power_loop());
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
        self.disconnect_device(id, true).await;
        self.db.delete_device(id)?;
        self.devices.write().remove(id);
        self.idle.lock().unwrap().remove(id);
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

    /// 全量快照（含设备 id；idle_power_loop 遍历用）
    fn list_snapshot_full(&self) -> Vec<(String, Device, DeviceStatus)> {
        let map = self.devices.read();
        map.iter()
            .map(|(id, rt)| (id.clone(), rt.device.clone(), rt.status))
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

        // adb 健康探测（2s）：server 被隧道 teardown 楔死时（AGENTS 已知坑）先重置，
        // 避免连接卡在 is_connected/reverse/push 上 70s+ 超时后失败；正常时 ~50ms
        if !self.adb.probe().await {
            warn!(device = %device.name, "adb probe 超时，疑似 server 楔死，重置 adb server");
            self.adb.reset_server().await;
            // 给全新 server 一点 USB 重新枚举时间
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }

        // 解析 adb transport：设备连接方式变化后（USB ↔ 无线调试 mDNS/IP:port），
        // 配置里的 serial 与 `adb devices` 显示名会失配（resolve_serial 按
        // 精确/子串/model 匹配），否则 push/reverse/-s 全部找不到设备。
        let mut device = device;
        {
            let resolved = self.adb.resolve_serial(&device.addr, &device.name).await;
            if !resolved.is_empty() && resolved != device.addr {
                info!(device = %device.name, from = %device.addr, to = %resolved, "adb transport resolved");
                device.addr = resolved.clone();
                // 写回运行时设备：后续截图/屏幕保活等 adb 操作直接使用解析后的 transport
                if let Some(rt) = self.devices.write().get_mut(id) {
                    rt.device.addr = resolved;
                }
            }
        }

        info!(device = %device.name, "connecting...");
        let result = ScrcpySession::connect(&self.adb, &self.cfg, &device).await;
        let handle: SessionHandle = match result {
            Ok(h) => h,
            Err(e) => {
                let msg = format!("{:#}", e);
                // adb 楔死自愈：连接失败根因是 adb 超时（push/reverse/shell）→ 重置
                // server 后重试一次（楔死期重试必失败；重置后全新 server 通常秒连，
                // 设备侧也坏了则明确报"设备不在线"，不再无限 70s 挂起）
                if msg.contains("adb timeout") {
                    warn!(device = %device.name, "connect adb timeout（{}），重置 adb server 后重试一次", msg);
                    self.adb.reset_server().await;
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    match ScrcpySession::connect(&self.adb, &self.cfg, &device).await {
                        Ok(h) => h,
                        Err(e2) => {
                            error!(device = %device.name, "connect failed after adb reset: {:#}", e2);
                            self.set_status(id, DeviceStatus::Offline, Some(e2.to_string()));
                            return Err(e2);
                        }
                    }
                } else {
                    error!(device = %device.name, "connect failed: {:#}", e);
                    self.set_status(id, DeviceStatus::Offline, Some(e.to_string()));
                    return Err(e);
                }
            }
        };

        // 帧广播 + 帧缓存（ffmpeg 解码供模板匹配）
        let (tx, _) = broadcast::channel::<VideoFrame>(128);
        // 音频广播（OPUS 帧 → WebRTC 音频轨）
        let (audio_tx, _) = broadcast::channel::<AudioFrame>(128);
        // 帧缓存（帧环 + 按需解码，供截图/模板匹配与 WebRTC 初始重放）
        let frame_cache = if self.cfg.decode_frames {
            Some(FrameCache::start(&self.cfg.ffmpeg_path))
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
            let mut bc_no_viewer = 0u64;
            while let Some(frame) = rx.recv().await {
                if let Some(fc) = &cache {
                    fc.feed(&frame);
                }
                // 诊断：广播 send 结果（接收者数 / 错误），降频避免每帧刷日志。
                // 注意：无任何 viewer 时 tokio broadcast 返回 Err(SendError)（含整帧数据），
                // 不能打印 e（会刷巨型日志），只记录计数。
                match tx2.send(frame) {
                    Ok(0) => {
                        bc_no_viewer += 1;
                        if bc_no_viewer % 300 == 1 {
                            debug!(device = %dn1, "broadcast: no receivers");
                        }
                    }
                    Ok(_) => {}
                    Err(_) => {
                        bc_no_viewer += 1;
                        if bc_no_viewer % 300 == 1 {
                            debug!(device = %dn1, "broadcast: no viewers, frame skipped");
                        }
                    }
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
                // 注：不再自动拉起设备配置的应用——启动应用由脚本 str_app /
                // Console 启动按钮显式触发；否则空闲断开后 cron 自动重连
                // 又会把游戏拉起来，与低功耗模式冲突
            }
        }
        info!(device = %device.name, "online");

        // 主屏保活仅限**镜像会话**：镜像内容来自物理屏管线，屏幕休眠后显示管线
        // 停止出帧，流静默定格（"连接后画面卡住"）。
        // 策略：连接时把 screen_off_timeout 调到最大 + 唤醒一次（解锁尽力而为），
        // 会话结束（connected=false）时恢复原超时值。周期补醒（30s）与空闲关屏
        // （keyevent 223）由 idle_power_loop 按"有无消费者"统一管理。
        // 虚拟屏会话跳过：编码不依赖物理屏管线，熄屏照常出帧，主屏保持熄屏省电。
        // （曾因误判"熄屏杀 USB adb"短暂改为全模式保活——真实根因是 Windows
        // USB 选择性暂停 + 接触不良的 USB 口，已排除，见 AGENTS 已知坑）
        if device.screen_mode == ScreenMode::Mirror {
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
                // 只等会话结束以恢复熄屏超时；补醒/关屏已移交 idle_power_loop
                loop {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    if !s3.connected.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                }
                if !orig.is_empty() && orig != "null" {
                    let _ = adb2
                        .shell(&serial2, &format!("settings put system screen_off_timeout {}", orig), Duration::from_secs(8))
                        .await;
                }
                info!(device = %dn2, orig_timeout = %orig, "screen keepalive stopped, screen_off_timeout restored");
            });
        } else {
            // 虚拟屏模式：不动主屏电源、不传 stay_awake——防冻结改为禁用
            // cached apps freezer（Android 15 主屏真睡眠时副屏应用会被 freezer
            // 整体冻结，do_freezer_trap，scrcpy #5604；实测软关屏 + stay_awake
            // 的旧方案代价是主屏永远无法点亮）。禁用后主屏自然睡眠/唤醒，
            // 用户随时可按电源键正常使用手机。同时媒体音量置 0：remote submix
            // 重定向在 HyperOS 上静默失效（扬声器照常外放，见 AGENTS 已知坑），
            // 静音是唯一可靠的"手机不响"手段（浏览器音频大概率随之变静）。
            // 两者改写前落盘 pending_restore.json，disconnect 统一恢复。
            let addr = device.addr.clone();
            apply_virtual_overrides(&self.adb, &self.cfg.data_dir, &addr).await;
            info!(device = %device.name, "virtual display: main screen natural (freezer disabled + media muted)");
        }
        Ok(())
    }

    /// 拆 scrcpy 会话（编码停止/虚拟屏销毁；adb 链路保留，下次消费者触发自动重连）。
    /// 运行守卫：脚本运行中拒绝拆除（虚拟屏销毁会杀掉屏上游戏、脚本上下文全丢），
    /// 仅 force=true（删除设备/看门狗确认死链路/显式管理动作）可绕过
    pub async fn disconnect_device(&self, id: &str, force: bool) {
        if !force && self.has_running_scripts(id) {
            warn!(device = %id, "script running, skip disconnect (use force to override)");
            return;
        }
        let (addr, mode) = {
            let map = self.devices.read();
            let Some(rt) = map.get(id) else { return };
            (rt.device.addr.clone(), rt.device.screen_mode.clone())
        };
        {
            let mut map = self.devices.write();
            let Some(rt) = map.get_mut(id) else { return };
            // 立即标记会话结束：屏幕保活任务 / 帧消费任务据此退出，及时恢复熄屏超时
            if let Some(s) = &rt.session {
                s.connected.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            // 停止帧缓存（释放引用）；按需解码无常驻线程/子进程，无需额外清理
            rt.frame_cache = None;
            rt.session = None;
            rt.frames = None;
            rt.audio_frames = None;
            rt.frame_cache = None;
            rt.status = DeviceStatus::Offline;
        }
        // 注意：scrcpy server 端 cleanup=true，socket 关闭即清理
        // 恢复虚拟屏会话的设备侧改写（freezer/媒体音量；无标记则零操作）。
        // 上面的写锁 guard 已随块结束释放（guard 不能跨 await 存活，Send 约束）
        if mode == ScreenMode::Virtual {
            restore_virtual_overrides(&self.adb, &self.cfg.data_dir, &addr).await;
        }
    }

    /// 优雅停机（POST /api/shutdown → gamer.ps1 stop/rebuild 先调再兜底硬杀）：
    /// force 拆所有 scrcpy 会话并清 reverse 隧道——control socket 随会话 drop 关闭，
    /// 设备端 server（cleanup=true）退出、adb shell 子进程正常收尾；硬杀进程则这些
    /// 全变孤儿，曾有孤儿 teardown 期 adb 短暂楔死后续连接的坑（见 AGENTS.md 已知坑）
    pub async fn shutdown_all(&self) {
        let snap = self.list_snapshot_full();
        for (id, d, status) in &snap {
            if *status != DeviceStatus::Online {
                continue;
            }
            info!(device = %d.name, "shutdown: disconnecting scrcpy session");
            self.disconnect_device(id, true).await;
        }
        // 留退场时间：control socket 关闭 → 设备端 server cleanup 需要一点传播时间
        tokio::time::sleep(Duration::from_millis(1500)).await;
        // 清残留 reverse 隧道（下次 connect 的 --remove-all 也会清，这里主动清更干净）
        for (_, d, _) in &snap {
            if d.addr.is_empty() {
                continue;
            }
            let _ = self
                .adb
                .run(&["-s", &d.addr, "reverse", "--remove-all"], Duration::from_secs(5))
                .await;
        }
    }

    /// 脚本运行开始：设备运行计数 +1（空闲低功耗守卫）+ 消费者出现
    /// （打断空闲计时，镜像模式唤醒已关的屏）
    pub fn run_begin(&self, id: &str) {
        let mut counts = self.run_counts.lock().unwrap();
        *counts.entry(id.to_string()).or_insert(0) += 1;
        drop(counts);
        self.notify_activity(id);
    }

    /// 脚本运行结束：计数 -1，归零移除条目（防 map 无限增长）
    pub fn run_end(&self, id: &str) {
        let mut counts = self.run_counts.lock().unwrap();
        if let Some(v) = counts.get_mut(id) {
            *v = v.saturating_sub(1);
            if *v == 0 {
                counts.remove(id);
            }
        }
    }

    /// 该设备是否有正在运行的脚本（视频静默看门狗/空闲断开的消费者判断）
    pub fn has_running_scripts(&self, id: &str) -> bool {
        self.run_counts.lock().unwrap().contains_key(id)
    }

    /// 消费者出现（viewer 注册 / 脚本开始）：打断空闲计时；镜像模式若已
    /// 空闲关屏则立即唤醒（224 + dismiss-keyguard，幂等），避免 viewer 连上
    /// 黑屏等下一轮周期检查
    pub fn notify_activity(&self, id: &str) {
        let wake_needed = {
            let mut idle = self.idle.lock().unwrap();
            match idle.get_mut(id) {
                Some(st) => {
                    st.idle_since = None;
                    std::mem::replace(&mut st.slept, false)
                }
                None => false,
            }
        };
        if wake_needed {
            self.wake_screen(id);
        }
    }

    /// 镜像模式唤醒物理屏（spawn 执行，不阻塞调用方；幂等）
    fn wake_screen(&self, id: &str) {
        let Some((device, _, _)) = self.snapshot(id) else { return };
        if device.screen_mode != ScreenMode::Mirror || device.addr.is_empty() {
            return;
        }
        let adb = self.adb.clone();
        let serial = device.addr.clone();
        let dn = device.name.clone();
        tokio::spawn(async move {
            let _ = adb.shell(&serial, "input keyevent 224", Duration::from_secs(8)).await;
            let _ = adb.shell(&serial, "wm dismiss-keyguard", Duration::from_secs(8)).await;
            info!(device = %dn, "idle screen woke up (viewer/script active)");
        });
    }

    /// 空闲低功耗循环（start 时 spawn，10s 周期）：会话存活的唯一管理者。
    /// 无 viewer 且无脚本运行持续 cfg.idle_power_secs 秒 → 虚拟屏拆 scrcpy
    /// 会话（adb 保留，下次消费者自动重连 ~2-4s）；镜像模式关物理屏
    /// （keyevent 223，会话保留，消费者回来 notify_activity 唤醒）。
    /// 有消费者时镜像模式每 30s 补一次 WAKEUP 兜底（用户按电源键等）
    async fn idle_power_loop(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        loop {
            tick.tick().await;
            if self.cfg.idle_power_secs == 0 {
                self.idle.lock().unwrap().clear();
                continue;
            }
            for (id, device, status) in self.list_snapshot_full() {
                if status != DeviceStatus::Online {
                    self.idle.lock().unwrap().remove(&id);
                    continue;
                }
                let active = self.viewers.lock().unwrap().contains_key(&id) || self.has_running_scripts(&id);
                if active {
                    // 锁内改状态、锁外做异步动作（guard 不能跨 await 存活）
                    let (slept, wake_expired) = {
                        let mut idle = self.idle.lock().unwrap();
                        let st = idle.entry(id.clone()).or_insert_with(|| IdleState {
                            idle_since: None,
                            slept: false,
                            last_wake: std::time::Instant::now(),
                        });
                        st.idle_since = None;
                        let slept = std::mem::replace(&mut st.slept, false);
                        let wake_expired = st.last_wake.elapsed() >= Duration::from_secs(30);
                        if wake_expired {
                            st.last_wake = std::time::Instant::now();
                        }
                        (slept, wake_expired)
                    };
                    // 镜像模式：保活补醒（30s 节流）或唤醒已关的屏
                    if device.screen_mode == ScreenMode::Mirror && (slept || wake_expired) {
                        self.wake_screen(&id);
                    }
                    continue;
                }
                let (slept, since) = {
                    let mut idle = self.idle.lock().unwrap();
                    let st = idle.entry(id.clone()).or_insert_with(|| IdleState {
                        idle_since: Some(std::time::Instant::now()),
                        slept: false,
                        last_wake: std::time::Instant::now(),
                    });
                    (st.slept, *st.idle_since.get_or_insert_with(std::time::Instant::now))
                };
                // 已关屏 = 镜像低功耗态已就位，等消费者回来（notify_activity 唤醒）
                if slept || since.elapsed() < Duration::from_secs(self.cfg.idle_power_secs) {
                    continue;
                }
                // 空闲超时：按屏幕模式进低功耗
                if device.screen_mode == ScreenMode::Mirror {
                    if let Some(st) = self.idle.lock().unwrap().get_mut(&id) {
                        st.slept = true;
                    }
                    let serial = device.addr.clone();
                    let dn = device.name.clone();
                    let adb = self.adb.clone();
                    info!(device = %dn, idle_secs = self.cfg.idle_power_secs, "idle: turn off mirror screen (session kept)");
                    tokio::spawn(async move {
                        let _ = adb.shell(&serial, "input keyevent 223", Duration::from_secs(8)).await;
                    });
                } else {
                    info!(device = %device.name, idle_secs = self.cfg.idle_power_secs, "idle: disconnect scrcpy session (low-power, adb kept)");
                    self.disconnect_device(&id, false).await;
                }
            }
        }
    }

    /// 对 wifi/emu 设备执行 adb connect（幂等，已连接无副作用）。
    /// 低功耗空闲的连接保障：adb 链路断了设备无法被脚本触达，这里周期补连
    pub async fn connect_wireless_adb(&self) {
        for (d, _, _) in self.list_snapshot() {
            if (d.kind == "wifi" || d.kind == "emu") && !d.addr.is_empty() {
                if let Err(e) = self.adb.connect(&d.addr).await {
                    debug!(device = %d.name, addr = %d.addr, "adb connect failed: {}", e);
                }
            }
        }
    }

    /// 扫描 adb 设备并同步入库（`adb devices -l` 解析 + 去重 + addr/kind 更新），
    /// 返回新增设备数。启动自举与 REST 扫描共用（原 api_scan_devices 逻辑）
    pub async fn scan_and_sync(&self) -> anyhow::Result<usize> {
        let out = self.adb.run(&["devices", "-l"], Duration::from_secs(10)).await?;
        let mut existing = self.db.list_devices()?;
        let mut added = 0usize;
        for line in out.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            // 格式: <serial> device product:... model:... device:... transport_id:N
            if parts.len() < 2 || parts[1] != "device" {
                continue; // offline / unauthorized 不注册
            }
            let serial = parts[0].to_string();
            if serial.is_empty() || serial == "localhost" {
                continue;
            }
            let model = parts
                .iter()
                .find_map(|p| p.strip_prefix("model:"))
                .map(|m| m.replace('_', " "));
            let kind = infer_device_kind(&serial);
            // 去重 + 地址同步：精确/子串/model 匹配（USB↔无线切换、无线 IP 变化后
            // serial 会变，见 adb.rs resolve_serial）；匹配到的旧设备更新 addr/kind，
            // 避免同一台设备重复入库
            let matched = existing.iter_mut().find(|d| {
                if !d.addr.is_empty() {
                    d.addr == serial
                        || (!serial.is_empty() && d.addr.contains(&serial))
                        || (!d.addr.is_empty() && serial.contains(&d.addr))
                        || (model.is_some() && model.as_deref() == Some(d.name.as_str()))
                } else {
                    kind == "usb" && d.kind == "usb"
                }
            });
            if let Some(old) = matched {
                if old.addr != serial || old.kind != kind {
                    old.addr = serial.clone();
                    old.kind = kind.to_string();
                    self.upsert_device(old).await?;
                }
                continue;
            }
            let name = model.clone().unwrap_or_else(|| short_serial(&serial));
            let device = Device {
                id: uuid::Uuid::new_v4().simple().to_string(),
                name,
                kind: kind.to_string(),
                addr: serial,
                screen_mode: ScreenMode::Mirror,
                vd_res: None,
                vd_dpi: None,
                pkg: None,
                fps: None,
                created_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            };
            self.upsert_device(&device).await?;
            existing.push(device);
            added += 1;
        }
        Ok(added)
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

    /// 截图（PNG）：帧缓存按需解码（每次全新解码最新帧，天然实时），
    /// 不可用/失败时回退 adb 虚拟屏截图，再失败直接报错（不静默回退物理屏）。
    pub async fn screenshot(&self, id: &str) -> anyhow::Result<Vec<u8>> {
        let (device, cache) = {
            let map = self.devices.read();
            let rt = map.get(id).ok_or_else(|| anyhow::anyhow!("device not found"))?;
            (rt.device.clone(), rt.frame_cache.clone())
        };
        if let Some(fc) = cache {
            match fc.decode_latest_png().await {
                Ok(Some(png)) => {
                    debug!("screenshot decoded on demand: {} bytes", png.len());
                    return Ok(png);
                }
                Ok(None) => debug!("frame cache: no decodable frames yet (waiting first IDR)"),
                Err(e) => warn!("frame cache decode failed: {}", e),
            }
        }
        let serial = if device.addr.is_empty() { "usb".to_string() } else { device.addr.clone() };
        // 虚拟屏模式：优先截 scrcpy 虚拟屏；部分设备 adb screencap -d 不支持该虚拟屏，
        // 会返回非图片错误文本，此时**不再回退物理屏**——物理屏与虚拟屏内容/分辨率不同，
        // 静默回退会让模板匹配拿到错误的画面（如主屏竖屏数据）。
        if device.screen_mode == ScreenMode::Virtual {
            if let Some(did) = self.virtual_display_id(&serial, device.vd_res.as_deref()).await {
                match self.adb.screencap_display(&serial, did).await {
                    Ok(png) if image::load_from_memory(&png).is_ok() => {
                        debug!("screenshot from adb virtual display: {} bytes", png.len());
                        return Ok(png);
                    }
                    Ok(png) => warn!(
                        "virtual display screencap returned invalid image ({} bytes)",
                        png.len()
                    ),
                    Err(e) => warn!("virtual display screencap failed: {}", e),
                }
            }
            anyhow::bail!("虚拟屏截图失败：帧缓存解码无帧/失败且 adb 虚拟屏 screencap 失败");
        }
        let png = self.adb.screencap(&serial).await?;
        debug!("screenshot from adb screencap: {} bytes", png.len());
        Ok(png)
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

/// 从 adb serial 推断接入方式：emulator-* → 模拟器；ip:port / adb-*（mDNS）→ 无线；其余 → USB
fn infer_device_kind(serial: &str) -> &'static str {
    if serial.starts_with("emulator-") {
        "emu"
    } else if serial.contains(':') || serial.starts_with("adb-") {
        "wifi"
    } else {
        "usb"
    }
}

/// 缩短过长的 serial（如 mDNS 形式）用于默认设备名
fn short_serial(serial: &str) -> String {
    if serial.len() > 24 {
        let head: String = serial.chars().take(20).collect();
        format!("{}…", head)
    } else {
        serial.to_string()
    }
}
