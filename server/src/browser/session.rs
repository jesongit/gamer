use super::{
    transport::{ScreencastFrame, Transport},
    BrowserTarget,
};
use anyhow::{anyhow, ensure};
use base64::Engine as _;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};

pub(crate) use crate::capabilities::FrameStamp;

fn decrement_if_positive(counter: &std::sync::atomic::AtomicUsize) -> bool {
    let mut current = counter.load(Ordering::SeqCst);
    loop {
        let Some(next) = current.checked_sub(1) else {
            return false;
        };
        match counter.compare_exchange_weak(current, next, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(actual) => current = actual,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("画面或目标已变化，请重新取帧后操作")]
struct RetiredCapture {
    stamp: FrameStamp,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PreviewMode {
    #[default]
    Single,
    Grid4,
    Grid9,
}
impl PreviewMode {
    fn parse(mode: &str) -> anyhow::Result<Self> {
        match mode {
            "single" => Ok(Self::Single),
            "grid4" => Ok(Self::Grid4),
            "grid9" => Ok(Self::Grid9),
            _ => anyhow::bail!("未知预览模式"),
        }
    }
    fn limits(self) -> (u32, u32, u8, u32, u32) {
        match self {
            Self::Single => (1920, 1080, 80, 1, 30),
            Self::Grid4 => (960, 540, 60, 3, 10),
            Self::Grid9 => (640, 360, 55, 5, 6),
        }
    }
    fn interval(self) -> Duration {
        Duration::from_nanos(1_000_000_000 / self.limits().4 as u64)
    }
    fn screencast_params(self) -> Value {
        let (width, height, quality, every_nth, _) = self.limits();
        json!({"format":"jpeg","quality":quality,"maxWidth":width,"maxHeight":height,"everyNthFrame":every_nth})
    }
}

pub struct CdpSession {
    pointer_guard: Arc<super::pointer_lock::PointerLockGuard>,
    pub id: String,
    pub target_id: String,
    epoch: String,
    transport: Arc<Transport>,
    child: Arc<Mutex<Child>>,
    endpoint: String,
    gate: Mutex<()>,
    closed: AtomicBool,
    keys: Mutex<HashMap<String, String>>,
    pub viewer: Arc<Mutex<()>>,
    pub runs: std::sync::atomic::AtomicUsize,
    pub(super) controls: Arc<crate::core::control::ControlRegistry>,
    buttons: Mutex<u32>,
    pointer: Mutex<(f64, f64)>,
    input_revision: std::sync::atomic::AtomicU64,
    stream_gate: Mutex<()>,
    preview_active: AtomicBool,
    recording_active: AtomicBool,
    output_consumers: std::sync::atomic::AtomicUsize,
    preview_mode: parking_lot::RwLock<PreviewMode>,
    held_contact: Mutex<Option<(u64, FrameStamp)>>,
    size: parking_lot::RwLock<(u32, u32)>,
    viewport: parking_lot::RwLock<(f64, f64)>,
    #[cfg(test)]
    capture_revision_changes: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    capture_attempts: std::sync::atomic::AtomicUsize,
}
impl CdpSession {
    pub fn validate_preview_mode(mode: &str) -> anyhow::Result<()> {
        PreviewMode::parse(mode).map(|_| ())
    }
    #[cfg(test)]
    pub async fn launch(
        target: &BrowserTarget,
        cfg: &crate::config::Config,
        executable: PathBuf,
    ) -> anyhow::Result<Arc<Self>> {
        Self::launch_with_controls(
            target,
            cfg,
            executable,
            Arc::new(crate::core::control::ControlRegistry::default()),
        )
        .await
    }
    pub async fn launch_with_controls(
        target: &BrowserTarget,
        cfg: &crate::config::Config,
        executable: PathBuf,
        controls: Arc<crate::core::control::ControlRegistry>,
    ) -> anyhow::Result<Arc<Self>> {
        target.validate()?;
        let profile = cfg
            .data_dir
            .join("browser-profiles")
            .join(&target.profile_id);
        std::fs::create_dir_all(&profile)?;
        let profile = std::fs::canonicalize(profile)?;
        let port_file = profile.join("DevToolsActivePort");
        if port_file.is_file() {
            std::fs::remove_file(&port_file)?;
        }
        let mut command = Command::new(executable);
        command
            .args([
                "--headless=new",
                // CDP previews carry images only. Never play target audio on the host.
                "--mute-audio",
                "--remote-debugging-port=0",
                "--remote-debugging-address=127.0.0.1",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-mode",
                "--disable-extensions",
                "--disable-component-extensions-with-background-pages",
                "--disable-background-timer-throttling",
                "--disable-renderer-backgrounding",
            ])
            .arg(format!(
                "--user-data-dir={}",
                browser_profile_argument(&profile)
            ))
            .arg("about:blank")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn()?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()?;
        let info = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                ensure!(
                    child.try_wait()?.is_none(),
                    "浏览器启动后退出，请检查资料目录是否已被占用"
                );
                if let Ok(port) = std::fs::read_to_string(&port_file) {
                    if let Some(port) = port.lines().next().and_then(|s| s.parse::<u16>().ok()) {
                        if let Ok(response) = client
                            .get(format!("http://127.0.0.1:{port}/json/list"))
                            .send()
                            .await
                        {
                            if let Ok(pages) = response.json::<Vec<Value>>().await {
                                if let Some(page) = pages
                                    .into_iter()
                                    .find(|p| p["type"] == "page" && p["url"] == "about:blank")
                                {
                                    return Ok::<_, anyhow::Error>(page);
                                }
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| anyhow!("浏览器调试连接启动超时"))??;
        let socket = info["webSocketDebuggerUrl"]
            .as_str()
            .ok_or_else(|| anyhow!("浏览器未提供页面连接"))?;
        let socket_url = reqwest::Url::parse(socket)?;
        let endpoint = format!(
            "http://127.0.0.1:{}",
            socket_url.port().ok_or_else(|| anyhow!("缺少调试端口"))?
        );
        let browser = client
            .get(format!("{endpoint}/json/version"))
            .send()
            .await?
            .json::<Value>()
            .await?;
        let pointer_guard = super::pointer_lock::PointerLockGuard::install(
            browser["webSocketDebuggerUrl"]
                .as_str()
                .ok_or_else(|| anyhow!("缺少浏览器调试连接"))?,
            info["id"].as_str().unwrap_or_default(),
        )
        .await?;
        let transport = Transport::connect(socket).await?;
        transport.call("Page.enable", json!({})).await?;
        transport.call("Runtime.enable", json!({})).await?;
        Self::deny_desktop_pointer_lock(&transport).await?;
        transport.call("Emulation.setDeviceMetricsOverride", json!({"width":target.width,"height":target.height,"deviceScaleFactor":1,"mobile":false})).await?;
        let session = Arc::new(Self {
            pointer_guard,
            id: target.id.clone(),
            target_id: info["id"].as_str().unwrap_or_default().into(),
            epoch: uuid::Uuid::new_v4().to_string(),
            transport,
            child: Arc::new(Mutex::new(child)),
            endpoint: {
                let url = reqwest::Url::parse(socket)?;
                format!(
                    "http://127.0.0.1:{}",
                    url.port().ok_or_else(|| anyhow!("缺少调试端口"))?
                )
            },
            gate: Mutex::new(()),
            closed: AtomicBool::new(false),
            keys: Mutex::new(HashMap::new()),
            viewer: Arc::new(Mutex::new(())),
            runs: std::sync::atomic::AtomicUsize::new(0),
            controls,
            buttons: Mutex::new(0),
            pointer: Mutex::new((0.0, 0.0)),
            input_revision: std::sync::atomic::AtomicU64::new(0),
            stream_gate: Mutex::new(()),
            preview_active: AtomicBool::new(false),
            recording_active: AtomicBool::new(false),
            output_consumers: std::sync::atomic::AtomicUsize::new(0),
            preview_mode: parking_lot::RwLock::new(PreviewMode::Single),
            held_contact: Mutex::new(None),
            size: parking_lot::RwLock::new((target.width, target.height)),
            viewport: parking_lot::RwLock::new((target.width as f64, target.height as f64)),
            #[cfg(test)]
            capture_revision_changes: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            capture_attempts: std::sync::atomic::AtomicUsize::new(0),
        });
        let nav = session
            .transport
            .call("Page.navigate", json!({"url":target.url}))
            .await?;
        ensure!(
            nav.get("errorText").is_none(),
            "页面打开失败: {}",
            nav["errorText"]
        );
        Ok(session)
    }
    #[cfg(test)]
    pub(crate) async fn evaluate_for_test(&self, expression: &str) -> Value {
        self.transport
            .call(
                "Runtime.evaluate",
                json!({"expression":expression,"returnByValue":true}),
            )
            .await
            .unwrap()["result"]["value"]
            .clone()
    }
    async fn deny_desktop_pointer_lock(transport: &Transport) -> anyhow::Result<()> {
        // Remote pointer coordinates belong to this page. A hidden target must
        // never acquire the server's physical desktop cursor. Deny natively so
        // navigation, subframes and popup documents share the same boundary.
        transport
            .call(
                "Browser.setPermission",
                json!({
                    "permission":{"name":"pointer-lock"},"setting":"denied"
                }),
            )
            .await
            .map_err(|error| anyhow!("无法隔离后台网页的鼠标锁定，请更新浏览器: {error}"))?;
        // Also relinquish a lock held by an already loaded document when rebinding.
        transport
            .call(
                "Runtime.evaluate",
                json!({"expression":"document.exitPointerLock()"}),
            )
            .await?;
        Ok(())
    }
    pub async fn pages(&self) -> anyhow::Result<Vec<Value>> {
        ensure!(self.is_alive(), "浏览器已断开");
        let pages = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()?
            .get(format!("{}/json/list", self.endpoint))
            .send()
            .await?
            .json::<Vec<Value>>()
            .await?;
        Ok(pages.into_iter().filter(|p| p["type"] == "page").collect())
    }
    pub async fn rebind(&self, target_id: &str) -> anyhow::Result<Arc<Self>> {
        let _gate = self.gate.lock().await;
        ensure!(
            self.runs.load(Ordering::SeqCst) == 0,
            "任务执行期间不能切换标签页"
        );
        let page = self
            .pages()
            .await?
            .into_iter()
            .find(|p| p["id"] == target_id)
            .ok_or_else(|| anyhow!("标签页不存在"))?;
        let transport = Transport::connect(
            page["webSocketDebuggerUrl"]
                .as_str()
                .ok_or_else(|| anyhow!("页面未提供连接"))?,
        )
        .await?;
        transport.call("Page.enable", json!({})).await?;
        transport.call("Runtime.enable", json!({})).await?;
        Self::deny_desktop_pointer_lock(&transport).await?;
        let (width, height) = self.size();
        transport
            .call(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
            )
            .await?;
        self.closed.store(true, Ordering::SeqCst);
        self.transport.revision.fetch_add(1, Ordering::SeqCst);
        self.transport.cancel_reload_dialogs();
        drop(_gate);
        self.release_inputs().await;
        Ok(Arc::new(Self {
            pointer_guard: self.pointer_guard.clone(),
            id: self.id.clone(),
            target_id: target_id.into(),
            epoch: uuid::Uuid::new_v4().to_string(),
            transport,
            child: self.child.clone(),
            endpoint: self.endpoint.clone(),
            gate: Mutex::new(()),
            closed: AtomicBool::new(false),
            keys: Mutex::new(HashMap::new()),
            buttons: Mutex::new(0),
            pointer: Mutex::new((0.0, 0.0)),
            input_revision: std::sync::atomic::AtomicU64::new(0),
            viewer: Arc::new(Mutex::new(())),
            runs: std::sync::atomic::AtomicUsize::new(0),
            controls: self.controls.clone(),
            stream_gate: Mutex::new(()),
            preview_active: AtomicBool::new(false),
            recording_active: AtomicBool::new(false),
            output_consumers: std::sync::atomic::AtomicUsize::new(0),
            preview_mode: parking_lot::RwLock::new(*self.preview_mode.read()),
            held_contact: Mutex::new(None),
            size: parking_lot::RwLock::new((width, height)),
            viewport: parking_lot::RwLock::new((width as f64, height as f64)),
            #[cfg(test)]
            capture_revision_changes: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            capture_attempts: std::sync::atomic::AtomicUsize::new(0),
        }))
    }
    pub(super) fn invalidate(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.transport.revision.fetch_add(1, Ordering::SeqCst);
        self.transport.cancel_reload_dialogs();
    }
    pub fn is_alive(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
            && self.transport.alive.load(Ordering::SeqCst)
            && self.pointer_guard.is_alive()
    }
    pub fn stamp(&self) -> FrameStamp {
        FrameStamp {
            target: self.id.clone(),
            epoch: self.epoch.clone(),
            revision: self.transport.revision.load(Ordering::SeqCst),
        }
    }
    pub async fn coordinate_space(
        &self,
    ) -> anyhow::Result<(crate::capabilities::FrameSize, FrameStamp)> {
        let _gate = self.gate.lock().await;
        let stamp = self.stamp();
        self.validate(&stamp)?;
        let (width, height) = self.size();
        Ok((crate::capabilities::FrameSize::new(width, height), stamp))
    }
    pub fn size(&self) -> (u32, u32) {
        *self.size.read()
    }
    pub fn validate(&self, stamp: &FrameStamp) -> anyhow::Result<()> {
        ensure!(self.is_alive(), "浏览器目标已断开");
        ensure!(*stamp == self.stamp(), "画面或目标已变化，请重新取帧后操作");
        Ok(())
    }
    /// Preparation uses the same navigation-safe, read-only capture boundary.
    /// Inputs always retain strict validation against their original frame.
    pub(crate) async fn capture_for_prepare(&self) -> anyhow::Result<()> {
        self.capture_with_timeout(Duration::from_secs(10))
            .await
            .map(|_| ())
    }
    async fn ready(&self) -> anyhow::Result<()> {
        ensure!(self.is_alive(), "浏览器目标已断开");
        let state = self
            .transport
            .call(
                "Runtime.evaluate",
                json!({"expression":"document.readyState", "returnByValue":true}),
            )
            .await?;
        ensure!(
            matches!(
                state["result"]["value"].as_str(),
                Some("interactive" | "complete")
            ),
            "页面正在跳转，请等待后重新取帧"
        );
        Ok(())
    }
    /// The captured surface includes non-overlay scrollbars. LayoutMetrics'
    /// clientWidth/clientHeight exclude them and cannot describe that bitmap.
    /// Keep full CSS viewport dimensions separate from image pixel dimensions.
    async fn full_viewport(&self) -> anyhow::Result<(f64, f64, f64, f64)> {
        let value = self.transport.call("Runtime.evaluate", json!({
            "expression":"({width:window.innerWidth,height:window.innerHeight,pageX:window.scrollX,pageY:window.scrollY,scale:window.visualViewport?.scale??1})",
            "returnByValue":true,
        })).await?;
        let viewport = &value["result"]["value"];
        let w = viewport["width"]
            .as_f64()
            .ok_or_else(|| anyhow!("页面宽度无效"))?;
        let h = viewport["height"]
            .as_f64()
            .ok_or_else(|| anyhow!("页面高度无效"))?;
        let x = viewport["pageX"]
            .as_f64()
            .ok_or_else(|| anyhow!("页面横向偏移无效"))?;
        let y = viewport["pageY"]
            .as_f64()
            .ok_or_else(|| anyhow!("页面纵向偏移无效"))?;
        ensure!(
            w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 && x.is_finite() && y.is_finite(),
            "页面尺寸无效"
        );
        ensure!(
            viewport["scale"].as_f64() == Some(1.0),
            "预览坐标映射无效，请恢复页面缩放后重新连接"
        );
        Ok((w, h, x, y))
    }
    pub async fn start_preview(
        &self,
    ) -> anyhow::Result<tokio::sync::watch::Receiver<Option<Arc<ScreencastFrame>>>> {
        let _stream = self.stream_gate.lock().await;
        self.start_preview_locked().await
    }
    pub async fn start_preview_with_mode(
        &self,
        mode: &str,
    ) -> anyhow::Result<tokio::sync::watch::Receiver<Option<Arc<ScreencastFrame>>>> {
        let mode = PreviewMode::parse(mode)?;
        let _stream = self.stream_gate.lock().await;
        *self.preview_mode.write() = mode;
        self.start_preview_locked().await
    }
    async fn start_preview_locked(
        &self,
    ) -> anyhow::Result<tokio::sync::watch::Receiver<Option<Arc<ScreencastFrame>>>> {
        ensure!(self.is_alive(), "浏览器目标已断开");
        self.preview_active.store(true, Ordering::SeqCst);
        let result = if self.is_full_resolution_active() {
            let current = self
                .transport
                .frames
                .borrow()
                .as_ref()
                .is_some_and(|frame| {
                    frame.revision == self.transport.revision.load(Ordering::SeqCst)
                        && frame.generation
                            == self.transport.stream_generation.load(Ordering::SeqCst)
                });
            if current {
                let mut frames = self.transport.frames.subscribe();
                frames.mark_changed();
                Ok(frames)
            } else {
                // A static recorder may still cache pixels from before a
                // viewport change. Restart its original-resolution producer;
                // subscribing again cannot turn old pixels into a new frame.
                self.start_stream(true).await
            }
        } else {
            self.start_stream(false).await
        };
        if result.is_err() {
            self.preview_active.store(false, Ordering::SeqCst);
        }
        result
    }
    pub async fn configure_preview(&self, mode: &str) -> anyhow::Result<()> {
        let mode = PreviewMode::parse(mode)?;
        let _stream = self.stream_gate.lock().await;
        ensure!(self.is_alive(), "浏览器目标已断开");
        let changed = *self.preview_mode.read() != mode;
        *self.preview_mode.write() = mode;
        if changed && self.is_preview_active() && !self.is_full_resolution_active() {
            self.start_stream(false).await?;
        }
        Ok(())
    }
    pub fn preview_interval(&self) -> Duration {
        self.preview_mode.read().interval()
    }
    pub fn is_preview_active(&self) -> bool {
        self.preview_active.load(Ordering::SeqCst)
    }
    pub fn is_recording_active(&self) -> bool {
        self.recording_active.load(Ordering::SeqCst)
    }
    fn is_full_resolution_active(&self) -> bool {
        self.is_recording_active() || self.output_consumers.load(Ordering::SeqCst) > 0
    }
    pub(super) fn has_stream_consumers(&self) -> bool {
        self.is_preview_active() || self.is_full_resolution_active()
    }
    pub fn can_idle(&self) -> bool {
        self.runs.load(Ordering::SeqCst) == 0
            && !self.has_stream_consumers()
            && self.viewer.try_lock().is_ok()
            && self.stream_gate.try_lock().is_ok()
            && self.gate.try_lock().is_ok()
    }
    async fn start_stream(
        &self,
        recording: bool,
    ) -> anyhow::Result<tokio::sync::watch::Receiver<Option<Arc<ScreencastFrame>>>> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut retries = 0;
        loop {
            match self.start_stream_once(recording).await {
                Err(error)
                    if self.is_alive()
                        && inactive_page_during_navigation(&error)
                        && tokio::time::Instant::now() < deadline =>
                {
                    // Initial navigation can temporarily retire the active document.
                    // Retry this same binding, never another tab or a cached frame.
                    self.transport.reset_stream(false);
                    retries += 1;
                    tokio::time::sleep_until(
                        (tokio::time::Instant::now() + Duration::from_millis(100)).min(deadline),
                    )
                    .await;
                }
                result => {
                    if retries > 0 {
                        if result.is_ok() {
                            tracing::debug!(target_id = %self.id, retries, "browser stream attached after initial navigation");
                        } else {
                            tracing::warn!(target_id = %self.id, retries, "browser stream could not attach after navigation retry");
                        }
                    }
                    return result;
                }
            }
        }
    }
    async fn start_stream_once(
        &self,
        recording: bool,
    ) -> anyhow::Result<tokio::sync::watch::Receiver<Option<Arc<ScreencastFrame>>>> {
        ensure!(self.is_alive(), "浏览器目标已断开");
        // Restart requests a fresh image of static pages as well. Keep the
        // Emulation viewport untouched: preview resolution is not input size.
        // A website popup may have foregrounded a different tab. Restore only
        // this session's explicit binding; new tabs still require user selection.
        self.transport.call("Page.bringToFront", json!({})).await?;
        self.transport.reset_stream(false);
        self.transport
            .call("Page.stopScreencast", json!({}))
            .await?;
        self.transport.reset_stream(true);
        let mut frames = self.transport.frames.subscribe();
        let params = if recording {
            // Recording always owns the original viewport and original JPEG
            // quality. Its subscription can never inherit a grid thumbnail.
            json!({"format":"jpeg","quality":80,"everyNthFrame":1})
        } else {
            self.preview_mode.read().screencast_params()
        };
        if let Err(error) = self.transport.call("Page.startScreencast", params).await {
            self.transport.reset_stream(false);
            return Err(error);
        }
        // Chrome counts the first static compositor image toward everyNthFrame,
        // so a grid stream can discard its only image. Bootstrap once from the
        // precise screenshot path; subsequent updates still come from CDP.
        let fresh = tokio::time::timeout(Duration::from_millis(350), async {
            loop {
                if frames.borrow().is_some() {
                    return Ok::<_, anyhow::Error>(());
                }
                frames.changed().await?;
            }
        })
        .await;
        if !matches!(fresh, Ok(Ok(()))) {
            // A refresh can still be loading or change documents while the
            // bootstrap PNG is captured. The producer and bound CDP session
            // remain valid: await its next frame instead of closing the viewer.
            let before = self.stamp();
            if self.ready().await.is_err() && self.is_alive() {
                frames.mark_changed();
                return Ok(frames);
            }
            if let Err(error) = self.bootstrap_static_frame(recording).await {
                if self.is_alive() && before != self.stamp() {
                    frames.mark_changed();
                    return Ok(frames);
                }
                self.stop_stream().await;
                return Err(error);
            }
        }
        // The bootstrap wait calls changed(), which marks the first native
        // image as seen. A returned static stream must still deliver that image
        // to consumers that check has_changed() before borrowing it.
        frames.mark_changed();
        Ok(frames)
    }
    async fn bootstrap_static_frame(&self, recording: bool) -> anyhow::Result<()> {
        let generation = self.transport.stream_generation.load(Ordering::SeqCst);
        let metrics = self
            .transport
            .call("Page.getLayoutMetrics", json!({}))
            .await?;
        ensure!(
            metrics["cssVisualViewport"]["scale"].as_f64() == Some(1.0),
            "预览坐标映射无效，请恢复页面缩放后重新连接"
        );
        let (png, stamp) = self.capture().await?;
        let mode = *self.preview_mode.read();
        let (width, height) = self.size();
        let (max_width, max_height, quality, _, _) = mode.limits();
        let data = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
            let mut image = image::load_from_memory(&png)?;
            if !recording && (image.width() > max_width || image.height() > max_height) {
                image = image.thumbnail(max_width, max_height);
            }
            let mut jpeg = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut jpeg,
                if recording { 80 } else { quality },
            )
            .encode_image(&image)?;
            Ok(base64::engine::general_purpose::STANDARD.encode(jpeg))
        })
        .await??;
        self.validate(&stamp)?;
        ensure!(
            generation == self.transport.stream_generation.load(Ordering::SeqCst),
            "预览模式已变化，请等待新画面"
        );
        // Do not overwrite a newer native frame that arrived during encoding.
        let bootstrap = Arc::new(ScreencastFrame {
            data,
            metadata: json!({"deviceWidth":width,"deviceHeight":height,"pageScaleFactor":1.0,"offsetTop":0.0,"timestamp":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64()}),
            revision: stamp.revision,
            generation,
        });
        self.transport.frames.send_if_modified(move |latest| {
            if latest.is_some() {
                false
            } else {
                *latest = Some(bootstrap);
                true
            }
        });
        Ok(())
    }
    pub async fn start_recording(
        &self,
    ) -> anyhow::Result<tokio::sync::watch::Receiver<Option<Arc<ScreencastFrame>>>> {
        let _stream = self.stream_gate.lock().await;
        ensure!(self.is_alive(), "浏览器目标已断开");
        ensure!(!self.is_recording_active(), "目标正在录制");
        self.recording_active.store(true, Ordering::SeqCst);
        let result = self.start_stream(true).await;
        if result.is_err() {
            self.recording_active.store(false, Ordering::SeqCst);
        }
        result
    }
    pub async fn stop_recording(&self) {
        let _stream = self.stream_gate.lock().await;
        self.recording_active.store(false, Ordering::SeqCst);
        if self.is_full_resolution_active() {
            return;
        }
        if self.is_preview_active() {
            if self.start_stream(false).await.is_err() {
                self.stop_stream().await;
            }
        } else {
            self.stop_stream().await;
        }
    }
    pub(crate) async fn refresh_recording(
        &self,
    ) -> anyhow::Result<tokio::sync::watch::Receiver<Option<Arc<ScreencastFrame>>>> {
        let _stream = self.stream_gate.lock().await;
        ensure!(self.is_recording_active(), "浏览器录制已结束");
        self.full_stream_subscription().await
    }
    pub async fn stop_preview(&self) {
        let _stream = self.stream_gate.lock().await;
        self.preview_active.store(false, Ordering::SeqCst);
        if !self.is_full_resolution_active() {
            self.stop_stream().await;
        }
    }
    pub(crate) async fn start_media_output(
        &self,
    ) -> anyhow::Result<tokio::sync::watch::Receiver<Option<Arc<ScreencastFrame>>>> {
        let _stream = self.stream_gate.lock().await;
        ensure!(self.is_alive(), "浏览器目标已断开");
        let already_full = self.is_full_resolution_active();
        self.output_consumers.fetch_add(1, Ordering::SeqCst);
        let result = if already_full {
            self.full_stream_subscription().await
        } else {
            self.start_stream(true).await
        };
        if result.is_err() {
            self.output_consumers.fetch_sub(1, Ordering::SeqCst);
        }
        result
    }
    pub(crate) async fn refresh_media_output(
        &self,
    ) -> anyhow::Result<tokio::sync::watch::Receiver<Option<Arc<ScreencastFrame>>>> {
        let _stream = self.stream_gate.lock().await;
        ensure!(
            self.output_consumers.load(Ordering::SeqCst) > 0,
            "浏览器输出已结束"
        );
        loop {
            match self.full_stream_subscription().await {
                Err(error) if self.is_alive() && inactive_page_during_navigation(&error) => {
                    // A reload briefly retires Chrome's active document while
                    // this explicitly bound target and CDP socket stay alive.
                    // Keep the media lease and the encoder's last frame; retry
                    // this same producer without accepting any stale input.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                result => return result,
            }
        }
    }
    async fn full_stream_subscription(
        &self,
    ) -> anyhow::Result<tokio::sync::watch::Receiver<Option<Arc<ScreencastFrame>>>> {
        let current = self
            .transport
            .frames
            .borrow()
            .as_ref()
            .is_some_and(|frame| {
                frame.revision == self.transport.revision.load(Ordering::SeqCst)
                    && frame.generation == self.transport.stream_generation.load(Ordering::SeqCst)
            });
        if current {
            let mut frames = self.transport.frames.subscribe();
            frames.mark_changed();
            Ok(frames)
        } else {
            self.start_stream(true).await
        }
    }
    pub(crate) fn release_media_output_count(&self) {
        decrement_if_positive(&self.output_consumers);
    }
    pub(crate) async fn reconcile_media_output(&self) {
        let _stream = self.stream_gate.lock().await;
        if self.is_full_resolution_active() {
            return;
        }
        if self.is_preview_active() {
            if self.start_stream(false).await.is_err() {
                self.stop_stream().await;
            }
        } else {
            self.stop_stream().await;
        }
    }
    async fn stop_stream(&self) {
        self.transport.reset_stream(false);
        let _ = self.transport.call("Page.stopScreencast", json!({})).await;
    }
    pub async fn preview_stamp(&self, frame: &ScreencastFrame) -> anyhow::Result<FrameStamp> {
        self.validate_stream_frame(frame)?;
        let stamp = FrameStamp {
            revision: frame.revision,
            ..self.stamp()
        };
        self.validate(&stamp)?;
        let metadata = &frame.metadata;
        let w = metadata["deviceWidth"].as_f64().unwrap_or(0.0);
        let h = metadata["deviceHeight"].as_f64().unwrap_or(0.0);
        ensure!(
            w > 0.0
                && h > 0.0
                && metadata["pageScaleFactor"].as_f64() == Some(1.0)
                && metadata["offsetTop"].as_f64() == Some(0.0),
            "预览坐标映射无效，请恢复页面缩放后重新连接"
        );
        let bytes = base64::engine::general_purpose::STANDARD.decode(&frame.data)?;
        let size = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()?
            .into_dimensions()?;
        let coordinates = (w.round() as u32, h.round() as u32);
        // CDP maxWidth/maxHeight scale the whole image. Metadata still describes
        // the original viewport; never adopt thumbnail dimensions as coordinates.
        ensure!(
            valid_preview_dimensions(size, coordinates),
            "预览比例与页面不一致，请重新连接"
        );
        self.validate(&stamp)?;
        self.validate_stream_frame(frame)?;
        // Stable pixels are a read-only observation. Do not wait behind CDP
        // input round trips merely to present the same coordinate space.
        // Input still acquires the gate and revalidates its original stamp.
        if self.size() == coordinates {
            return Ok(stamp);
        }
        // A coordinate mutation still shares the input/capture barrier. Recheck
        // after waiting: no old frame may be relabelled with a new revision.
        let _gate = self.gate.lock().await;
        self.validate(&stamp)?;
        self.validate_stream_frame(frame)?;
        let changed = self.size() != coordinates;
        // Screencast metadata describes image dimensions, while viewport stores
        // CSS input dimensions established by the same full-surface capture.
        // A pixel-size change still retires this frame and bootstraps a new one.
        *self.size.write() = coordinates;
        if changed {
            self.transport
                .revision
                .compare_exchange(
                    stamp.revision,
                    stamp.revision + 1,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .map_err(|_| anyhow!("页面已变化，请等待新画面"))?;
            // The coordinate mapping has changed, so this cached frame keeps
            // its original revision and cannot be relabelled for input. The
            // viewer observes the revision change and restarts the producer
            // before presenting a newly captured image.
            anyhow::bail!("画面尺寸已变化，请等待新画面");
        }
        Ok(stamp)
    }
    pub(crate) fn validate_stream_frame(&self, frame: &ScreencastFrame) -> anyhow::Result<()> {
        ensure!(
            frame.generation == self.transport.stream_generation.load(Ordering::SeqCst),
            "预览模式已变化，请等待新画面"
        );
        Ok(())
    }
    pub async fn preview_image(
        &self,
        frame: &ScreencastFrame,
    ) -> anyhow::Result<(String, (u32, u32), FrameStamp)> {
        let stamp = self.preview_stamp(frame).await?;
        let mode = *self.preview_mode.read();
        let (max_width, max_height, quality, _, _) = mode.limits();
        let bytes = base64::engine::general_purpose::STANDARD.decode(&frame.data)?;
        let size = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()?
            .into_dimensions()?;
        let (data, size) = if size.0 > max_width || size.1 > max_height {
            // A recorder needs the unscaled producer. Only its viewer copy is
            // resized; the shared frame and recording bytes remain untouched.
            tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                let image = image::load_from_memory(&bytes)?.thumbnail(max_width, max_height);
                let size = (image.width(), image.height());
                let mut jpeg = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality)
                    .encode_image(&image)?;
                Ok((base64::engine::general_purpose::STANDARD.encode(jpeg), size))
            })
            .await??
        } else {
            (frame.data.clone(), size)
        };
        self.validate(&stamp)?;
        self.validate_stream_frame(frame)?;
        ensure!(
            *self.preview_mode.read() == mode,
            "预览模式已变化，请等待新画面"
        );
        Ok((data, size, stamp))
    }
    /// Retire an interrupted screenshot rather than returning bytes labelled
    /// with a newer revision. Only this read-only operation may retry.
    pub async fn capture(&self) -> anyhow::Result<(Vec<u8>, FrameStamp)> {
        self.capture_with_timeout(Duration::from_secs(20)).await
    }
    async fn capture_with_timeout(
        &self,
        timeout: Duration,
    ) -> anyhow::Result<(Vec<u8>, FrameStamp)> {
        tokio::time::timeout(timeout, async {
            loop {
                ensure!(self.is_alive(), "浏览器目标已断开");
                match self.capture_once().await {
                    Ok(frame) => return Ok(frame),
                    Err(error) if self.can_retry_retired_capture(&error) => {
                        tracing::debug!(%error, target = %self.id, "截图期间页面变化，丢弃过期截图并重新取帧");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    Err(error) => return Err(error),
                }
            }
        })
        .await
        .map_err(|_| anyhow!("等待稳定页面截图超时，请检查页面是否持续跳转"))?
    }
    fn can_retry_retired_capture(&self, error: &anyhow::Error) -> bool {
        let Some(retired) = error.downcast_ref::<RetiredCapture>() else {
            return false;
        };
        let current = self.stamp();
        self.is_alive()
            && current.target == retired.stamp.target
            && current.epoch == retired.stamp.epoch
            && current.revision != retired.stamp.revision
    }
    fn validate_capture_stamp(&self, stamp: &FrameStamp) -> anyhow::Result<()> {
        ensure!(self.is_alive(), "浏览器目标已断开");
        if *stamp != self.stamp() {
            return Err(RetiredCapture {
                stamp: stamp.clone(),
            }
            .into());
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn invalidate_next_captures_for_test(&self, count: usize) {
        self.capture_revision_changes.store(count, Ordering::SeqCst);
    }
    #[cfg(test)]
    pub(crate) fn capture_attempts_for_test(&self) -> usize {
        self.capture_attempts.load(Ordering::SeqCst)
    }
    #[cfg(test)]
    pub(crate) async fn capture_with_timeout_for_test(
        &self,
        timeout: Duration,
    ) -> anyhow::Result<(Vec<u8>, FrameStamp)> {
        self.capture_with_timeout(timeout).await
    }
    async fn capture_once(&self) -> anyhow::Result<(Vec<u8>, FrameStamp)> {
        let _gate = self.gate.lock().await;
        #[cfg(test)]
        self.capture_attempts.fetch_add(1, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if self.ready().await.is_ok() {
                    break;
                }
                ensure!(self.is_alive(), "浏览器目标已断开");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .map_err(|_| anyhow!("等待页面就绪超时"))??;
        let stamp = self.stamp();
        let (w, h, x, y) = self.full_viewport().await?;
        let value = self
            .transport
            .call(
                "Page.captureScreenshot",
                json!({"format":"png","captureBeyondViewport":false,"clip":{"x":x,"y":y,"width":w,"height":h,"scale":1}}),
            )
            .await?;
        #[cfg(test)]
        if decrement_if_positive(&self.capture_revision_changes) {
            self.transport.revision.fetch_add(1, Ordering::SeqCst);
        }
        self.validate_capture_stamp(&stamp)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value["data"].as_str().ok_or_else(|| anyhow!("截图为空"))?)?;
        ensure!(bytes.len() <= 24 * 1024 * 1024, "截图超过大小上限");
        let size = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()?
            .into_dimensions()?;
        self.validate_capture_stamp(&stamp)?;
        let changed = *self.viewport.read() != (w, h) || self.size() != size;
        *self.viewport.write() = (w, h);
        *self.size.write() = size;
        let mut stamp = stamp;
        if changed {
            self.transport
                .revision
                .compare_exchange(
                    stamp.revision,
                    stamp.revision + 1,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .map_err(|_| RetiredCapture {
                    stamp: stamp.clone(),
                })?;
            stamp.revision += 1;
        }
        self.validate_capture_stamp(&stamp)?;
        Ok((bytes, stamp))
    }
    /// Reload only this explicitly bound CDP page. Keep the control lease and
    /// retire old coordinates before navigation, even if the viewport is unchanged.
    pub async fn reload(&self, expected: Option<&FrameStamp>) -> anyhow::Result<()> {
        let _admission = self.controls.admit(&self.id).await?;
        let _gate = self.gate.lock().await;
        ensure!(self.is_alive(), "浏览器目标已断开");
        if let Some(expected) = expected {
            self.validate(expected)?;
        }
        self.release_locked().await;
        let revision = self.transport.revision.fetch_add(1, Ordering::SeqCst) + 1;
        self.transport.reload_checked(revision).await?;
        // Reload acknowledgement completes this action. Loading and visual
        // readiness belong to subsequent wait/observation steps. The transport
        // keeps this explicit reload's beforeunload permission until its main
        // navigation commits (or its bounded scope fails), including late dialogs.
        Ok(())
    }

    /// Read only the visible page's selection, without touching the server OS clipboard.
    pub async fn copy_text(&self, expected: &FrameStamp) -> anyhow::Result<String> {
        let _admission = self.controls.admit(&self.id).await?;
        let _gate = self.gate.lock().await;
        ensure!(
            self.runs.load(Ordering::SeqCst) == 0 || self.controls.status(&self.id).owner.is_some(),
            "目标正在执行任务"
        );
        self.ready().await?;
        self.validate(expected)?;
        let result = self.transport.call_checked("Runtime.evaluate", json!({
            "expression": r#"(() => {
                let doc = document, root = doc, active = doc.activeElement;
                for (let depth = 0; active && depth < 32; depth++) {
                    if (active.shadowRoot) { root = active.shadowRoot; active = root.activeElement; continue; }
                    if (active.tagName === 'IFRAME') {
                        try { if (active.contentDocument) { doc = active.contentDocument; root = doc; active = doc.activeElement; continue; } } catch {}
                        return '';
                    }
                    break;
                }
                if (active && ['INPUT', 'TEXTAREA'].includes(active.tagName)) {
                    if (active.type === 'password') return '';
                    if (typeof active.selectionStart === 'number') return active.value.slice(active.selectionStart, active.selectionEnd);
                }
                return String(root.getSelection?.() || doc.getSelection() || '');
            })()"#,
            "returnByValue": true,
        }), Some(expected.revision)).await?;
        self.validate(expected)?;
        let text = result["result"]["value"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        ensure!(text.len() <= 131072, "选中文本过长，请分段复制");
        Ok(text)
    }

    pub async fn input(&self, value: &Value, expected: Option<&FrameStamp>) -> anyhow::Result<()> {
        let _admission = self.controls.admit(&self.id).await?;
        let _gate = self.gate.lock().await;
        self.check_pointer_owner(value).await?;
        let result = self.input_locked(value, expected).await;
        if result.is_err() {
            self.release_locked().await;
        }
        result
    }
    pub async fn manual_input(&self, value: &Value, expected: &FrameStamp) -> anyhow::Result<()> {
        if !crate::core::control::ControlRegistry::manual_scoped(&self.id) {
            return self
                .controls
                .manual(&self.id, self.manual_input_admitted(value, expected))
                .await;
        }
        self.manual_input_admitted(value, expected).await
    }
    async fn manual_input_admitted(
        &self,
        value: &Value,
        expected: &FrameStamp,
    ) -> anyhow::Result<()> {
        let _admission = self.controls.admit(&self.id).await?;
        let _gate = self.gate.lock().await;
        ensure!(
            self.runs.load(Ordering::SeqCst) == 0 || self.controls.status(&self.id).owner.is_some(),
            "目标正在执行任务"
        );
        self.check_pointer_owner(value).await?;
        let result = self.input_locked(value, Some(expected)).await;
        if result.is_err() {
            self.release_locked().await;
        }
        result
    }
    async fn check_pointer_owner(&self, value: &Value) -> anyhow::Result<()> {
        if matches!(value["type"].as_str(), Some("tap" | "pointer" | "scroll")) {
            ensure!(
                self.held_contact.lock().await.is_none(),
                "请先松开映射中的持续指针"
            );
        }
        Ok(())
    }
    async fn input_locked(
        &self,
        value: &Value,
        expected: Option<&FrameStamp>,
    ) -> anyhow::Result<()> {
        self.ready().await?;
        let stamp = expected.cloned().unwrap_or_else(|| self.stamp());
        self.validate(&stamp)?;
        if self.input_revision.load(Ordering::SeqCst) != stamp.revision {
            ensure!(
                self.keys.lock().await.is_empty() && *self.buttons.lock().await == 0,
                "页面已变化，原有按住操作已终止"
            );
            self.input_revision.store(stamp.revision, Ordering::SeqCst);
        }
        let kind = value["type"].as_str().unwrap_or_default();
        match kind {
            "tap" | "pointer" | "scroll" => {
                let x = value["x"].as_f64().ok_or_else(|| anyhow!("缺少坐标 x"))?;
                let y = value["y"].as_f64().ok_or_else(|| anyhow!("缺少坐标 y"))?;
                let (width, height) = self.size();
                let (vw, vh) = *self.viewport.read();
                let (current_width, current_height, _, _) = self.full_viewport().await?;
                if current_width != vw || current_height != vh {
                    self.transport.revision.fetch_add(1, Ordering::SeqCst);
                    anyhow::bail!("坐标映射已变化，请重新取帧");
                }
                ensure!(
                    x.is_finite()
                        && y.is_finite()
                        && x >= 0.0
                        && y >= 0.0
                        && x < width as f64
                        && y < height as f64,
                    "坐标超出画面"
                );
                let button = value["button"].as_str().unwrap_or("left");
                let bit = match button {
                    "left" => 1,
                    "right" => 2,
                    "middle" => 4,
                    _ => anyhow::bail!("未知鼠标按钮"),
                };
                let mut buttons = self.buttons.lock().await;
                let event = match kind {
                    "tap" => "mousePressed",
                    "scroll" => "mouseWheel",
                    _ => match value["action"].as_str().unwrap_or("move") {
                        "down" => "mousePressed",
                        "up" => "mouseReleased",
                        "move" => "mouseMoved",
                        _ => anyhow::bail!("未知指针操作"),
                    },
                };
                if event == "mousePressed" {
                    *buttons |= bit;
                } else if event == "mouseReleased" {
                    *buttons &= !bit;
                }
                *self.pointer.lock().await = (x * vw / width as f64, y * vh / height as f64);
                let mut params = json!({"type":event,"x":x*vw/width as f64,"y":y*vh/height as f64,"button":if event=="mouseMoved" {"none"} else {button},"buttons":*buttons,"clickCount":1});
                if kind == "scroll" {
                    params["deltaX"] = json!(value["delta_x"].as_f64().unwrap_or(0.0));
                    params["deltaY"] = json!(value["delta_y"].as_f64().unwrap_or(0.0));
                }
                self.validate(&stamp)?;
                self.transport
                    .call_checked(
                        "Input.dispatchMouseEvent",
                        params.clone(),
                        Some(stamp.revision),
                    )
                    .await?;
                if kind == "tap" {
                    *buttons &= !bit;
                    params["type"] = json!("mouseReleased");
                    params["buttons"] = json!(*buttons);
                    self.transport
                        .call("Input.dispatchMouseEvent", params)
                        .await?;
                }
            }
            "key" => {
                let name = value["key"].as_str().ok_or_else(|| anyhow!("缺少按键"))?;
                let (key, code, vk) = key_info(name)?;
                let action = value["action"].as_str().unwrap_or("press");
                ensure!(matches!(action, "press" | "down" | "up"), "未知按键操作");
                let mut keys = self.keys.lock().await;
                let repeated = keys.contains_key(&code);
                if action != "up" {
                    keys.insert(code.clone(), key.clone());
                } else {
                    keys.remove(&code);
                }
                let modifiers = (if keys.contains_key("AltLeft") { 1 } else { 0 })
                    | (if keys.contains_key("ControlLeft") {
                        2
                    } else {
                        0
                    })
                    | (if keys.contains_key("MetaLeft") { 4 } else { 0 })
                    | (if keys.contains_key("ShiftLeft") { 8 } else { 0 });
                // Mapping envelopes use physical codes; preserve Shift text
                // semantics when the pass-through path supplies KeyA/Digit1.
                let key = if modifiers & 8 != 0 && name.len() > 1 && key.len() == 1 {
                    if key.as_bytes()[0].is_ascii_alphabetic() {
                        key.to_ascii_uppercase()
                    } else {
                        "1234567890-=[]\\;',./`"
                            .find(&key)
                            .and_then(|i| "!@#$%^&*()_+{}|:\"<>?~".chars().nth(i))
                            .map(|c| c.to_string())
                            .unwrap_or(key)
                    }
                } else {
                    key
                };
                let mut params = json!({"type":if action=="up" {"keyUp"} else {"keyDown"},"key":&key,"code":&code,"windowsVirtualKeyCode":vk,"modifiers":modifiers});
                if action != "up" && key.chars().count() == 1 && modifiers & 7 == 0 {
                    params["text"] = json!(&key);
                }
                params["autoRepeat"] = json!(repeated && action == "down");
                self.validate(&stamp)?;
                self.transport
                    .call_checked(
                        "Input.dispatchKeyEvent",
                        params.clone(),
                        Some(stamp.revision),
                    )
                    .await?;
                if action == "press" {
                    params["type"] = json!("keyUp");
                    self.transport
                        .call("Input.dispatchKeyEvent", params)
                        .await?;
                }
                if action != "down" {
                    keys.remove(&code);
                }
            }
            "text" => {
                let text = value["text"].as_str().ok_or_else(|| anyhow!("缺少文本"))?;
                ensure!(text.len() <= 16 * 1024, "文本过长");
                self.transport
                    .call_checked(
                        "Input.insertText",
                        json!({"text":text}),
                        Some(stamp.revision),
                    )
                    .await?;
            }
            _ => anyhow::bail!("浏览器不支持此操作: {kind}"),
        }
        crate::recording::observe_browser(&self.id, value);
        Ok(())
    }
    /// Single-contact mapping over the browser mouse. Concurrent contacts are rejected.
    pub async fn mapped_contact(
        &self,
        id: u64,
        action: &str,
        x: u32,
        y: u32,
        manual: Option<&FrameStamp>,
    ) -> anyhow::Result<()> {
        let _admission = self.controls.admit(&self.id).await?;
        let _gate = self.gate.lock().await;
        if manual.is_some() {
            ensure!(
                self.runs.load(Ordering::SeqCst) == 0
                    || self.controls.status(&self.id).owner.is_some(),
                "目标正在执行任务"
            );
        }
        let mut contact = self.held_contact.lock().await;
        let stamp = if action == "down" {
            ensure!(
                contact.is_none() && *self.buttons.lock().await == 0,
                "浏览器仅支持一个持续指针，请先松开已有操作"
            );
            manual.cloned().unwrap_or_else(|| self.stamp())
        } else {
            let (held, stamp) = contact.as_ref().ok_or_else(|| anyhow!("持续指针已释放"))?;
            ensure!(*held == id, "持续指针身份不匹配");
            stamp.clone()
        };
        let result = self
            .input_locked(
                &json!({"type":"pointer","action":action,"x":x,"y":y}),
                Some(&stamp),
            )
            .await;
        if result.is_err() || action == "up" {
            *contact = None;
        } else {
            *contact = Some((id, stamp));
        }
        drop(contact);
        if result.is_err() {
            self.release_locked().await;
        }
        result
    }
    pub async fn release_manual(&self) {
        let _gate = self.gate.lock().await;
        if self.runs.load(Ordering::SeqCst) == 0
            || self.controls.status(&self.id).phase == crate::core::control::ControlPhase::Paused
        {
            self.release_locked().await;
        }
    }
    pub async fn release_inputs(&self) {
        let _gate = self.gate.lock().await;
        self.release_locked().await;
    }
    async fn release_locked(&self) {
        let _ = self
            .transport
            .call(
                "Runtime.evaluate",
                json!({"expression":"document.exitPointerLock()"}),
            )
            .await;
        self.held_contact.lock().await.take();
        let keys: Vec<_> = self.keys.lock().await.drain().map(|(_, key)| key).collect();
        for name in keys {
            if let Ok((key, code, vk)) = key_info(&name) {
                if self
                    .transport
                    .call(
                        "Input.dispatchKeyEvent",
                        json!({"type":"keyUp","key":key,"code":code,"windowsVirtualKeyCode":vk}),
                    )
                    .await
                    .is_err()
                {
                    self.invalidate();
                }
            }
        }
        let bits = std::mem::take(&mut *self.buttons.lock().await);
        let (x, y) = *self.pointer.lock().await;
        for (bit, button) in [(1, "left"), (2, "right"), (4, "middle")] {
            if bits & bit != 0
                && self
                    .transport
                    .call(
                        "Input.dispatchMouseEvent",
                        json!({"type":"mouseReleased","button":button,"buttons":0,"x":x,"y":y}),
                    )
                    .await
                    .is_err()
            {
                self.invalidate();
            }
        }
    }
    pub async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.transport.revision.fetch_add(1, Ordering::SeqCst);
        self.transport.cancel_reload_dialogs();
        self.transport.reset_stream(false);
        let _ = tokio::time::timeout(Duration::from_secs(2), self.release_inputs()).await;
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            self.transport.call("Browser.close", json!({})),
        )
        .await;
        let mut child = self.child.lock().await;
        if tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .is_err()
        {
            let _ = child.kill().await;
        }
    }
    /// Caller holds the viewer mutex and BrowserManager admission gate. Do not
    /// wait for a capture: activity observed now must restart the idle timeout.
    pub(super) async fn try_idle_close(&self) -> bool {
        let Ok(_stream) = self.stream_gate.try_lock() else {
            return false;
        };
        let Ok(gate) = self.gate.try_lock() else {
            return false;
        };
        if !self.is_alive() || self.has_stream_consumers() || self.runs.load(Ordering::SeqCst) != 0
        {
            return false;
        }
        if self
            .controls
            .when_idle(&self.id, || self.closed.store(true, Ordering::SeqCst))
            .is_none()
        {
            return false;
        }
        drop(gate);
        self.close().await;
        true
    }
}

fn inactive_page_during_navigation(error: &anyhow::Error) -> bool {
    serde_json::from_str::<Value>(&error.to_string()).is_ok_and(|error| {
        error["code"].as_i64() == Some(-32000)
            && error["message"].as_str() == Some("Not attached to an active page")
    })
}

#[cfg(test)]
mod media_refresh_error_tests {
    use super::*;

    #[test]
    fn only_inactive_bound_document_is_a_retryable_media_refresh_error() {
        assert!(inactive_page_during_navigation(&anyhow::Error::msg(
            r#"{"code":-32000,"message":"Not attached to an active page"}"#
        )));
        for message in [
            r#"{"code":-32000,"message":"Target closed"}"#,
            r#"{"code":-32601,"message":"Not attached to an active page"}"#,
            "browser target disconnected",
            "预览坐标映射无效，请恢复页面缩放后重新连接",
            "CDP operation timed out: Page.startScreencast",
        ] {
            assert!(
                !inactive_page_during_navigation(&anyhow!(message.to_owned())),
                "{message}"
            );
        }
    }
}

fn valid_preview_dimensions(preview: (u32, u32), coordinates: (u32, u32)) -> bool {
    preview.0 > 0
        && preview.1 > 0
        && preview.0 <= coordinates.0
        && preview.1 <= coordinates.1
        && (preview.0 as i64 * coordinates.1 as i64 - preview.1 as i64 * coordinates.0 as i64).abs()
            <= coordinates.0.max(coordinates.1) as i64
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod preview_tests;

#[cfg(test)]
#[path = "small_preview_tests.rs"]
mod small_preview_tests;

#[cfg(test)]
#[path = "input_bounds_tests.rs"]
mod input_bounds_tests;

#[cfg(test)]
#[path = "pointer_lock_tests.rs"]
mod pointer_lock_tests;

fn browser_profile_argument(path: &std::path::Path) -> String {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{}", unc);
        }
        if let Some(disk) = value.strip_prefix(r"\\?\") {
            return disk.to_owned();
        }
    }
    value.into_owned()
}

fn key_info(name: &str) -> anyhow::Result<(String, String, u32)> {
    if let Some(letter) = name
        .strip_prefix("Key")
        .filter(|v| v.len() == 1 && v.is_ascii())
    {
        return key_info(&letter.to_ascii_lowercase());
    }
    if let Some(digit) = name.strip_prefix("Digit").filter(|v| v.len() == 1) {
        return key_info(digit);
    }
    if name.len() == 1 {
        let ch = name.chars().next().unwrap();
        if ch.is_ascii_alphabetic() {
            return Ok((
                name.into(),
                format!("Key{}", ch.to_ascii_uppercase()),
                ch.to_ascii_uppercase() as u32,
            ));
        }
        if ch.is_ascii_digit() {
            return Ok((name.into(), format!("Digit{ch}"), ch as u32));
        }
        let pair = match ch {
            ' ' => Some(("Space", 32)),
            ';' | ':' => Some(("Semicolon", 186)),
            '=' | '+' => Some(("Equal", 187)),
            ',' | '<' => Some(("Comma", 188)),
            '-' | '_' => Some(("Minus", 189)),
            '.' | '>' => Some(("Period", 190)),
            '/' | '?' => Some(("Slash", 191)),
            '`' | '~' => Some(("Backquote", 192)),
            '[' | '{' => Some(("BracketLeft", 219)),
            '\\' | '|' => Some(("Backslash", 220)),
            ']' | '}' => Some(("BracketRight", 221)),
            '\'' | '"' => Some(("Quote", 222)),
            _ => None,
        };
        if let Some((code, vk)) = pair {
            return Ok((name.into(), code.into(), vk));
        }
        if let Some(i) = "!@#$%^&*()".find(ch) {
            let digit = (i + 1) % 10;
            return Ok((name.into(), format!("Digit{digit}"), 48 + digit as u32));
        }
    }
    let upper = name.to_ascii_uppercase();
    if let Some(n) = upper
        .strip_prefix('F')
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|n| (1..=24).contains(n))
    {
        return Ok((upper.clone(), upper, 111 + n));
    }
    let (key, code, vk) = match upper.as_str() {
        "ENTER" | "RETURN" => ("Enter", "Enter", 13),
        "ESC" | "ESCAPE" => ("Escape", "Escape", 27),
        "SPACE" => (" ", "Space", 32),
        "TAB" => ("Tab", "Tab", 9),
        "BACKSPACE" => ("Backspace", "Backspace", 8),
        "DELETE" => ("Delete", "Delete", 46),
        "HOME" => ("Home", "Home", 36),
        "END" => ("End", "End", 35),
        "PAGEUP" => ("PageUp", "PageUp", 33),
        "PAGEDOWN" => ("PageDown", "PageDown", 34),
        "INSERT" => ("Insert", "Insert", 45),
        "ARROWUP" | "UP" => ("ArrowUp", "ArrowUp", 38),
        "ARROWDOWN" | "DOWN" => ("ArrowDown", "ArrowDown", 40),
        "ARROWLEFT" | "LEFT" => ("ArrowLeft", "ArrowLeft", 37),
        "ARROWRIGHT" | "RIGHT" => ("ArrowRight", "ArrowRight", 39),
        "SHIFT" | "SHIFTLEFT" | "SHIFTRIGHT" => ("Shift", "ShiftLeft", 16),
        "CTRL" | "CONTROL" | "CONTROLLEFT" | "CONTROLRIGHT" => ("Control", "ControlLeft", 17),
        "ALT" | "ALTLEFT" | "ALTRIGHT" => ("Alt", "AltLeft", 18),
        "META" | "METALEFT" | "METARIGHT" => ("Meta", "MetaLeft", 91),
        "NUMPADENTER" => ("Enter", "NumpadEnter", 13),
        "SEMICOLON" => (";", "Semicolon", 186),
        "EQUAL" => ("=", "Equal", 187),
        "COMMA" => (",", "Comma", 188),
        "MINUS" => ("-", "Minus", 189),
        "PERIOD" => (".", "Period", 190),
        "SLASH" => ("/", "Slash", 191),
        "BACKQUOTE" => ("`", "Backquote", 192),
        "BRACKETLEFT" => ("[", "BracketLeft", 219),
        "BACKSLASH" => ("\\", "Backslash", 220),
        "BRACKETRIGHT" => ("]", "BracketRight", 221),
        "QUOTE" => ("'", "Quote", 222),
        _ => anyhow::bail!("不支持的浏览器按键: {name}"),
    };
    Ok((key.into(), code.into(), vk))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn profile_argument_keeps_cookie_storage_compatible() {
        assert_eq!(
            browser_profile_argument(std::path::Path::new(r"\\?\C:\profiles\a")),
            r"C:\profiles\a"
        );
        assert_eq!(
            browser_profile_argument(std::path::Path::new(r"\\?\UNC\host\profiles\a")),
            r"\\host\profiles\a"
        );
    }
    #[test]
    fn counter_release_is_atomic_and_never_underflows() {
        use std::sync::atomic::AtomicUsize;

        let counter = AtomicUsize::new(1000);
        let released = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let counter = &counter;
                let released = &released;
                scope.spawn(move || {
                    while decrement_if_positive(counter) {
                        released.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
        });
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert_eq!(released.load(Ordering::SeqCst), 1000);
        assert!(!decrement_if_positive(&counter));
    }

    #[test]
    fn keys_are_not_android_codes() {
        assert_eq!(key_info("Enter").unwrap().2, 13);
        assert_eq!(key_info("w").unwrap().1, "KeyW");
        assert_eq!(key_info("Home").unwrap().2, 36);
        assert!(key_info("66").is_err());
    }
}
