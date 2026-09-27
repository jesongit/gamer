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

pub struct CdpSession {
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
    buttons: Mutex<u32>,
    pointer: Mutex<(f64, f64)>,
    input_revision: std::sync::atomic::AtomicU64,
    size: parking_lot::RwLock<(u32, u32)>,
    viewport: parking_lot::RwLock<(f64, f64)>,
}
impl CdpSession {
    pub async fn launch(
        target: &BrowserTarget,
        cfg: &crate::config::Config,
        executable: PathBuf,
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
                "--remote-debugging-port=0",
                "--remote-debugging-address=127.0.0.1",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-mode",
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
        let transport = Transport::connect(socket).await?;
        transport.call("Page.enable", json!({})).await?;
        transport.call("Runtime.enable", json!({})).await?;
        transport.call("Emulation.setDeviceMetricsOverride", json!({"width":target.width,"height":target.height,"deviceScaleFactor":1,"mobile":false})).await?;
        let session = Arc::new(Self {
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
            buttons: Mutex::new(0),
            pointer: Mutex::new((0.0, 0.0)),
            input_revision: std::sync::atomic::AtomicU64::new(0),
            size: parking_lot::RwLock::new((target.width, target.height)),
            viewport: parking_lot::RwLock::new((target.width as f64, target.height as f64)),
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
    pub(super) async fn evaluate_for_test(&self, expression: &str) -> Value {
        self.transport
            .call(
                "Runtime.evaluate",
                json!({"expression":expression,"returnByValue":true}),
            )
            .await
            .unwrap()["result"]["value"]
            .clone()
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
        let (width, height) = self.size();
        transport
            .call(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
            )
            .await?;
        self.closed.store(true, Ordering::SeqCst);
        self.transport.revision.fetch_add(1, Ordering::SeqCst);
        drop(_gate);
        self.release_inputs().await;
        Ok(Arc::new(Self {
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
            size: parking_lot::RwLock::new((width, height)),
            viewport: parking_lot::RwLock::new((width as f64, height as f64)),
        }))
    }
    pub(super) fn invalidate(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.transport.revision.fetch_add(1, Ordering::SeqCst);
    }
    pub fn is_alive(&self) -> bool {
        !self.closed.load(Ordering::SeqCst) && self.transport.alive.load(Ordering::SeqCst)
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
    pub async fn start_preview(
        &self,
    ) -> anyhow::Result<tokio::sync::watch::Receiver<Option<Arc<ScreencastFrame>>>> {
        ensure!(self.is_alive(), "浏览器目标已断开");
        // Restart also requests a fresh image of a static page after navigation.
        self.transport
            .call("Page.stopScreencast", json!({}))
            .await?;
        self.transport.frames.send_replace(None);
        let frames = self.transport.frames.subscribe();
        self.transport
            .call(
                "Page.startScreencast",
                json!({"format":"jpeg","quality":80,"everyNthFrame":1}),
            )
            .await?;
        Ok(frames)
    }
    pub async fn stop_preview(&self) {
        let _ = self.transport.call("Page.stopScreencast", json!({})).await;
        self.transport.frames.send_replace(None);
    }
    pub async fn preview_stamp(&self, frame: &ScreencastFrame) -> anyhow::Result<FrameStamp> {
        let _gate = self.gate.lock().await;
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
        // No preview downscaling: manual and automation coordinates share the
        // same full-resolution viewport. Never silently reinterpret a crop.
        ensure!(
            size == (w.round() as u32, h.round() as u32),
            "预览尺寸与页面不一致，请重新连接"
        );
        let changed = *self.viewport.read() != (w, h) || self.size() != size;
        self.validate(&stamp)?;
        *self.viewport.write() = (w, h);
        *self.size.write() = size;
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
            return Ok(FrameStamp {
                revision: stamp.revision + 1,
                ..stamp
            });
        }
        Ok(stamp)
    }
    pub async fn capture(&self) -> anyhow::Result<(Vec<u8>, FrameStamp)> {
        let _gate = self.gate.lock().await;
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
        let metrics = self
            .transport
            .call("Page.getLayoutMetrics", json!({}))
            .await?;
        let viewport = &metrics["cssVisualViewport"];
        let w = viewport["clientWidth"]
            .as_f64()
            .unwrap_or(self.size().0 as f64);
        let h = viewport["clientHeight"]
            .as_f64()
            .unwrap_or(self.size().1 as f64);
        ensure!(w > 0.0 && h > 0.0, "页面尺寸无效");
        let value = self
            .transport
            .call(
                "Page.captureScreenshot",
                json!({"format":"png","captureBeyondViewport":false,"clip":{"x":viewport["pageX"].as_f64().unwrap_or(0.0),"y":viewport["pageY"].as_f64().unwrap_or(0.0),"width":w,"height":h,"scale":1}}),
            )
            .await?;
        self.validate(&stamp)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value["data"].as_str().ok_or_else(|| anyhow!("截图为空"))?)?;
        ensure!(bytes.len() <= 24 * 1024 * 1024, "截图超过大小上限");
        let size = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()?
            .into_dimensions()?;
        self.validate(&stamp)?;
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
                .map_err(|_| anyhow!("页面已变化，请重新取帧"))?;
            stamp.revision += 1;
        }
        self.validate(&stamp)?;
        Ok((bytes, stamp))
    }
    pub async fn input(&self, value: &Value, expected: Option<&FrameStamp>) -> anyhow::Result<()> {
        let _gate = self.gate.lock().await;
        let result = self.input_locked(value, expected).await;
        if result.is_err() {
            self.release_locked().await;
        }
        result
    }
    pub async fn manual_input(&self, value: &Value, expected: &FrameStamp) -> anyhow::Result<()> {
        let _gate = self.gate.lock().await;
        ensure!(self.runs.load(Ordering::SeqCst) == 0, "目标正在执行任务");
        let result = self.input_locked(value, Some(expected)).await;
        if result.is_err() {
            self.release_locked().await;
        }
        result
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
                let metrics = self
                    .transport
                    .call("Page.getLayoutMetrics", json!({}))
                    .await?;
                let current = &metrics["cssVisualViewport"];
                if current["clientWidth"].as_f64() != Some(vw)
                    || current["clientHeight"].as_f64() != Some(vh)
                {
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
        Ok(())
    }
    pub async fn release_manual(&self) {
        let _gate = self.gate.lock().await;
        if self.runs.load(Ordering::SeqCst) == 0 {
            self.release_locked().await;
        }
    }
    pub async fn release_inputs(&self) {
        let _gate = self.gate.lock().await;
        self.release_locked().await;
    }
    async fn release_locked(&self) {
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
            if bits & bit != 0 {
                if self
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
    }
    pub async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.transport.revision.fetch_add(1, Ordering::SeqCst);
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
}

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
        "SHIFT" => ("Shift", "ShiftLeft", 16),
        "CTRL" | "CONTROL" => ("Control", "ControlLeft", 17),
        "ALT" => ("Alt", "AltLeft", 18),
        "META" => ("Meta", "MetaLeft", 91),
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
    fn keys_are_not_android_codes() {
        assert_eq!(key_info("Enter").unwrap().2, 13);
        assert_eq!(key_info("w").unwrap().1, "KeyW");
        assert_eq!(key_info("Home").unwrap().2, 36);
        assert!(key_info("66").is_err());
    }
}
