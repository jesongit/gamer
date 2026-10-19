//! Optional, on-demand OCR. The host owns the process and all path configuration.
pub(crate) mod external;
mod parser;
pub(crate) mod settings;
#[cfg(test)]
mod tests;

use anyhow::{ensure, Context, Result};

use base64::Engine as _;
use image::{imageops::FilterType, ImageFormat, ImageReader};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::Cursor,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU32, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{oneshot, watch, Semaphore},
    time::Instant,
};

const MAX_PNG: usize = 8 * 1024 * 1024;
const MAX_OUTPUT: usize = 2 * 1024 * 1024;
const MAX_STDERR: usize = 32 * 1024;

pub struct OcrService {
    state: Arc<State>,
}
struct State {
    data_root: PathBuf,
    settings_path: PathBuf,
    settings_gate: Mutex<()>,
    workers: Arc<Semaphore>,
    generation: watch::Sender<u64>,
    pending: AtomicUsize,
    active_pid: AtomicU32,
    closing: AtomicUsize,
    resolved: Mutex<Option<(Option<String>, Instant, settings::Engine)>>,
    #[cfg(test)]
    child_fixture: Mutex<Option<fn() -> Command>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    png_base64: String,
    #[serde(default = "default_language")]
    language: String,
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default)]
    whitelist: Option<String>,
    #[serde(default = "default_scale")]
    scale: f64,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
}
fn default_language() -> String {
    "chi_sim+eng".into()
}
fn default_mode() -> String {
    "block".into()
}
fn default_scale() -> f64 {
    2.0
}
fn default_timeout() -> u64 {
    10_000
}

struct Prepared {
    png: Vec<u8>,
    width: u32,
    height: u32,
    scaled_width: u32,
    scaled_height: u32,
}
impl Request {
    fn validate(&self, engine: &settings::Engine) -> Result<u8> {
        ensure!(
            (1..=15_000).contains(&self.timeout_ms),
            "OCR timeout_ms 必须为 1..15000 毫秒"
        );
        ensure!(
            self.scale.is_finite() && (1.0..=3.0).contains(&self.scale),
            "OCR scale 必须为 1..3"
        );
        let psm = match self.mode.as_str() {
            "line" => 7,
            "block" => 6,
            "sparse" => 11,
            _ => anyhow::bail!("OCR mode 必须为 line、block 或 sparse"),
        };
        ensure!(self.language.len() <= 256, "OCR language 过长");
        let languages = self.language.split('+').collect::<Vec<_>>();
        ensure!(
            !languages.is_empty() && languages.len() <= 4,
            "OCR language 无效"
        );
        for language in languages {
            ensure!(settings::safe_language(language), "OCR language 名称无效");
            ensure!(
                engine.languages.iter().any(|name| name == language),
                "OCR 语言包未安装: {language}"
            );
        }
        if let Some(whitelist) = &self.whitelist {
            ensure!(
                whitelist.chars().count() <= 128 && !whitelist.chars().any(char::is_control),
                "OCR whitelist 最多 128 字符且不能包含控制字符"
            );
        }
        ensure!(
            self.png_base64.len() <= MAX_PNG.div_ceil(3) * 4,
            "OCR PNG 超过 8 MiB"
        );
        Ok(psm)
    }
    fn prepare(&self) -> Result<Prepared> {
        let input = base64::engine::general_purpose::STANDARD
            .decode(&self.png_base64)
            .context("OCR PNG base64 无效")?;
        ensure!(
            !input.is_empty() && input.len() <= MAX_PNG,
            "OCR PNG 大小无效"
        );
        ensure!(
            input.starts_with(b"\x89PNG\r\n\x1a\n"),
            "OCR 仅接受 PNG 图像"
        );
        let (width, height) = ImageReader::with_format(Cursor::new(&input), ImageFormat::Png)
            .into_dimensions()
            .context("OCR PNG 头无效")?;
        ensure!(
            width > 0
                && height > 0
                && width <= 4096
                && height <= 4096
                && width as u64 * height as u64 <= 4_000_000,
            "OCR 原图最多 4096 边长及 400 万像素，请先裁剪识别区域"
        );
        let scaled_width = (width as f64 * self.scale).round() as u32;
        let scaled_height = (height as f64 * self.scale).round() as u32;
        ensure!(
            scaled_width as u64 * scaled_height as u64 <= 16_000_000,
            "OCR 放大后超过 1600 万像素，请减小识别区域或 scale"
        );
        let mut reader = ImageReader::with_format(Cursor::new(input), ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(32 * 1024 * 1024);
        reader.limits(limits);
        let decoded = reader.decode().context("OCR PNG 解码失败")?.into_luma8();
        let image = if width == scaled_width && height == scaled_height {
            decoded
        } else {
            image::imageops::resize(&decoded, scaled_width, scaled_height, FilterType::Triangle)
        };
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(image)
            .write_to(&mut png, ImageFormat::Png)
            .context("OCR PNG 编码失败")?;
        Ok(Prepared {
            png: png.into_inner(),
            width,
            height,
            scaled_width,
            scaled_height,
        })
    }
}

impl OcrService {
    pub fn new(data_root: &Path) -> Result<Self> {
        let data_root = settings::anchor_data_root(
            &std::env::current_dir().context("无法读取 OCR 工作目录")?,
            data_root,
        )?;
        let settings_path = data_root.join("state").join("ocr").join("settings.dat");
        // OCR is optional. Private configuration is read only when an actual
        // call needs it, so an unavailable DPAPI identity or damaged file does
        // not prevent the Core from starting. Calls retain strict validation.
        settings::migrate(&data_root, &settings_path)?;
        let (generation, _) = watch::channel(0);
        Ok(Self {
            state: Arc::new(State {
                data_root,
                settings_path,
                settings_gate: Mutex::new(()),
                workers: Arc::new(Semaphore::new(1)),
                generation,
                pending: AtomicUsize::new(0),
                active_pid: AtomicU32::new(0),
                closing: AtomicUsize::new(0),
                resolved: Mutex::new(None),
                #[cfg(test)]
                child_fixture: Mutex::new(None),
            }),
        })
    }

    pub(crate) async fn dispatch(&self, action: &str, values: Value) -> Result<Value> {
        match action {
            "recognize" => self.recognize(values).await,
            "settings.read" => {
                let _gate = self.state.settings_gate.lock();
                Ok(serde_json::to_value(settings::Settings::load(
                    &self.state.settings_path,
                )?)?)
            }
            "settings.save" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Save {
                    engine_path: String,
                    tessdata_path: String,
                    #[serde(default)]
                    expected_version: Option<String>,
                }
                let save: Save = serde_json::from_value(values).context("OCR 配置无效")?;
                let _gate = self.state.settings_gate.lock();
                let mut settings = settings::Settings::load(&self.state.settings_path)?;
                if let Some(expected) = save.expected_version {
                    ensure!(
                        settings.version.as_deref() == Some(expected.as_str()),
                        "OCR 配置已更新，请读取配置后重试"
                    );
                }
                if !save.engine_path.trim().is_empty() {
                    settings::checked_path(save.engine_path.trim(), &self.state.data_root, false)?;
                }
                if !save.tessdata_path.trim().is_empty() {
                    settings::checked_path(save.tessdata_path.trim(), &self.state.data_root, true)?;
                }
                settings.engine_path = save.engine_path.trim().into();
                settings.tessdata_path = save.tessdata_path.trim().into();
                settings.write(&self.state.settings_path)?;
                Ok(serde_json::to_value(settings)?)
            }
            "status" => {
                let saved = {
                    let _gate = self.state.settings_gate.lock();
                    settings::Settings::load(&self.state.settings_path)?
                };
                let active_pid = self.state.active_pid.load(Ordering::Acquire);
                let pending = self.state.pending.load(Ordering::Acquire);
                Ok(match self.resolve_engine(&saved).await {
                    Ok(engine) => {
                        json!({"available":true,"engine_path":engine.executable,"tessdata_path":engine.tessdata,"languages":engine.languages,"pending":pending,"active_pid":(active_pid!=0).then_some(active_pid)})
                    }
                    Err(error) => {
                        json!({"available":false,"reason":error.to_string(),"pending":pending,"active_pid":(active_pid!=0).then_some(active_pid)})
                    }
                })
            }
            _ => anyhow::bail!("OCR 动作不支持"),
        }
    }

    pub(crate) async fn recognize(&self, values: Value) -> Result<Value> {
        self.recognize_cancellable(values, None).await
    }

    /// Credential cancellation joins this invocation's owned worker without
    /// cancelling or waiting behind other callers' queued requests.
    pub(crate) async fn recognize_cancellable(
        &self,
        values: Value,
        external: Option<watch::Receiver<bool>>,
    ) -> Result<Value> {
        let (_uncancelled, fallback) = watch::channel(false);
        let mut external = external.unwrap_or(fallback);
        ensure!(!*external.borrow(), "OCR 调用已取消");
        ensure!(
            self.state.closing.load(Ordering::Acquire) == 0,
            "OCR 正在停止"
        );
        let mut cancelled = self.state.generation.subscribe();
        let epoch = *cancelled.borrow();
        let request: Request = serde_json::from_value(values).context("OCR 请求无效")?;
        let saved = {
            let _gate = self.state.settings_gate.lock();
            settings::Settings::load(&self.state.settings_path)?
        };
        let engine = self.resolve_engine(&saved).await?;
        let psm = request.validate(&engine)?;
        let deadline = Instant::now() + Duration::from_millis(request.timeout_ms);
        let _pending = Pending::new(self.state.clone())?;
        // Images are decoded only after the single worker is available. Concurrent
        // targets keep their small encoded input while waiting, not enlarged frames.
        let permit = tokio::select! {
            biased;
            _=external.changed()=>anyhow::bail!("OCR 调用已取消"),
            _=cancelled.changed()=>anyhow::bail!("OCR 已停止"),
            _=tokio::time::sleep_until(deadline)=>anyhow::bail!("OCR 排队超时"),
            permit=self.state.workers.clone().acquire_owned()=>permit.context("OCR 工作队列已关闭")?,
        };
        ensure!(*cancelled.borrow() == epoch, "OCR 已停止");
        ensure!(
            self.state.closing.load(Ordering::Acquire) == 0,
            "OCR 正在停止"
        );
        let state = self.state.clone();
        let (abort, aborted) = oneshot::channel();
        // An owned task retains the permit through kill + wait, even when its caller
        // is cancelled. stop() therefore cannot outlive a detached OCR process.
        let mut work = tokio::spawn(async move {
            let _permit = permit;
            let (request, prepared) = tokio::task::spawn_blocking(move || {
                let prepared = request.prepare()?;
                Ok::<_, anyhow::Error>((request, prepared))
            })
            .await
            .context("OCR 图像处理任务异常")??;
            ensure!(Instant::now() < deadline, "OCR 图像处理超时");
            ensure!(*cancelled.borrow() == epoch, "OCR 已停止");
            ensure!(state.closing.load(Ordering::Acquire) == 0, "OCR 正在停止");
            let mut command = Command::new(settings::engine_argument_path(&engine.executable));
            command
                .args(["stdin", "stdout", "--tessdata-dir"])
                .arg(settings::engine_argument_path(&engine.tessdata))
                .args([
                    "-l",
                    &request.language,
                    "--psm",
                    &psm.to_string(),
                    "--oem",
                    "1",
                ])
                .args(["-c", "user_defined_dpi=300", "-c", "tessedit_create_tsv=1"]);
            if let Some(whitelist) = request.whitelist.filter(|text| !text.is_empty()) {
                command
                    .arg("-c")
                    .arg(format!("tessedit_char_whitelist={whitelist}"));
            }
            // Equivalent to Tesseract's tsv config, without requiring a copied
            // tessdata/configs/tsv file in installations containing only models.
            command.env("OMP_THREAD_LIMIT", "1");
            #[cfg(test)]
            if let Some(factory) = *state.child_fixture.lock() {
                // Lifecycle fixtures exercise the actual admitted call/worker
                // path with a sleeping OS child, not OCR recognition quality.
                command = factory();
            }
            let bytes =
                run_process(command, prepared.png, deadline, cancelled, aborted, state).await?;
            Ok::<_, anyhow::Error>(serde_json::to_value(parser::parse_tsv(
                &bytes,
                prepared.width,
                prepared.height,
                prepared.scaled_width,
                prepared.scaled_height,
            )?)?)
        });
        let mut abort = Some(abort);
        let result = tokio::select! {
            biased;
            _=external.changed()=>{
                drop(abort.take());
                let _ = work.await.context("OCR 工作任务异常")?;
                Err(anyhow::anyhow!("OCR 调用已取消"))
            }
            result=&mut work=>result.context("OCR 工作任务异常")?,
        };
        drop(abort);
        result
    }
}

struct Pending(Arc<State>);
impl Pending {
    fn new(state: Arc<State>) -> Result<Self> {
        let before = state.pending.fetch_add(1, Ordering::AcqRel);
        if before >= 16 {
            state.pending.fetch_sub(1, Ordering::AcqRel);
            anyhow::bail!("OCR 队列已满，请稍后重试");
        }
        Ok(Self(state))
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.0.pending.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Active(Arc<State>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.active_pid.store(0, Ordering::Release);
    }
}

struct Cancellation(Arc<State>);
impl Drop for Cancellation {
    fn drop(&mut self) {
        self.0.closing.fetch_sub(1, Ordering::AcqRel);
    }
}

async fn bounded_read<R: AsyncRead + Unpin>(reader: R, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    ensure!(bytes.len() <= limit, "OCR 输出超过限制");
    Ok(bytes)
}

async fn run_process(
    mut command: Command,
    input: Vec<u8>,
    deadline: Instant,
    mut cancelled: watch::Receiver<u64>,
    mut aborted: oneshot::Receiver<()>,
    state: Arc<State>,
) -> Result<Vec<u8>> {
    ensure!(Instant::now() < deadline, "OCR 识别超时");
    ensure!(!cancelled.has_changed().unwrap_or(true), "OCR 已停止");
    ensure!(
        matches!(aborted.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
        "OCR 调用已取消"
    );
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn().context("无法启动 Tesseract")?;
    state
        .active_pid
        .store(child.id().unwrap_or(0), Ordering::Release);
    let _active = Active(state);
    let mut stdin = child.stdin.take().context("OCR stdin 不可用")?;
    let stdout = child.stdout.take().context("OCR stdout 不可用")?;
    let stderr = child.stderr.take().context("OCR stderr 不可用")?;
    let writer = tokio::spawn(async move {
        stdin.write_all(&input).await?;
        stdin.shutdown().await?;
        Ok::<_, std::io::Error>(())
    });
    let output = tokio::spawn(bounded_read(stdout, MAX_OUTPUT));
    let diagnostic = tokio::spawn(bounded_read(stderr, MAX_STDERR));
    let status = tokio::select! {
        biased;
        _=&mut aborted=>Err(anyhow::anyhow!("OCR 调用已取消")),
        _=cancelled.changed()=>Err(anyhow::anyhow!("OCR 已停止")),
        _=tokio::time::sleep_until(deadline)=>Err(anyhow::anyhow!("OCR 识别超时")),
        status=child.wait()=>status.context("等待 OCR 进程失败"),
    };
    if let Err(error) = status {
        // No deadline here: lifecycle completion means the OS process has exited.
        let _ = child.start_kill();
        let waited = child.wait().await;
        writer.abort();
        let _ = writer.await;
        let _ = output.await;
        let _ = diagnostic.await;
        waited.context("停止 OCR 进程失败")?;
        return Err(error);
    }
    let status = status?;
    let written = writer.await.context("OCR 输入任务异常")?;
    let bytes = output.await.context("OCR 输出任务异常")??;
    let stderr = diagnostic.await.context("OCR 诊断任务异常")??;
    ensure!(
        status.success(),
        "Tesseract 识别失败: {}",
        String::from_utf8_lossy(&stderr).trim()
    );
    written.context("OCR 输入失败")?;
    Ok(bytes)
}

impl OcrService {
    async fn resolve_engine(&self, saved: &settings::Settings) -> Result<settings::Engine> {
        #[cfg(test)]
        if self.state.child_fixture.lock().is_some() {
            return settings::resolve(saved, &self.state.data_root);
        }
        if let Some((_, _, engine)) =
            self.state
                .resolved
                .lock()
                .as_ref()
                .filter(|(version, at, engine)| {
                    *version == saved.version
                        && at.elapsed() < Duration::from_secs(60)
                        && engine.executable.is_file()
                })
        {
            return Ok(engine.clone());
        }
        let cfg = crate::config::Config {
            data_dir: self.state.data_root.clone(),
            ..Default::default()
        };
        let dependency = crate::runtime_deps::inspect(&cfg, "tesseract").await?;
        ensure!(dependency.status == "ready", "{}", dependency.detail);
        let mut bound = saved.clone();
        bound.engine_path = dependency
            .path
            .context("OCR 引擎不存在")?
            .to_string_lossy()
            .into_owned();
        let engine = settings::resolve(&bound, &self.state.data_root)?;
        *self.state.resolved.lock() = Some((saved.version.clone(), Instant::now(), engine.clone()));
        Ok(engine)
    }

    pub(crate) fn cancel_inflight(&self) -> Option<Box<dyn Send>> {
        self.state.closing.fetch_add(1, Ordering::AcqRel);
        self.state
            .generation
            .send_modify(|value| *value = value.wrapping_add(1));
        Some(Box::new(Cancellation(self.state.clone())))
    }
    pub(crate) async fn stop(&self) {
        let _cancellation = self.cancel_inflight();
        let _ = self.state.workers.clone().acquire_owned().await;
    }
}
