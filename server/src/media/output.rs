//! Platform-neutral, process-owned media output. No platform credentials or room semantics.
use super::stream_mux;
mod source;
mod video_folder;
use crate::device::DeviceManager;
use anyhow::{bail, ensure, Context, Result};
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::AsyncWriteExt,
    sync::{mpsc, oneshot, watch, Mutex as AsyncMutex},
    task::JoinHandle,
};
pub use video_folder::VideoFolderInfo;

pub async fn inspect_video_folder(folder: &str) -> Result<VideoFolderInfo> {
    video_folder::inspect_video_folder(folder).await
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputRequest {
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub video_folder: Option<String>,
    #[serde(default = "local_mode")]
    pub mode: String,
    #[serde(default)]
    pub push_url: String,
    #[serde(default = "yes")]
    pub audio: bool,
    #[serde(default = "fps")]
    pub fps: u32,
    #[serde(default = "bitrate")]
    pub bitrate_kbps: u32,
}
fn local_mode() -> String {
    "local".into()
}
fn yes() -> bool {
    true
}
fn fps() -> u32 {
    30
}
fn bitrate() -> u32 {
    4000
}
impl OutputRequest {
    pub fn validate(&self) -> Result<()> {
        let device = !self.device_id.trim().is_empty();
        let folder = self
            .video_folder
            .as_deref()
            .is_some_and(|path| !path.trim().is_empty());
        ensure!(
            device ^ folder,
            "请选择设备或本地视频文件夹，且只能选择一种来源"
        );
        ensure!((10..=60).contains(&self.fps), "帧率范围为 10–60");
        ensure!(
            (500..=20000).contains(&self.bitrate_kbps),
            "码率范围为 500–20000 Kbps"
        );
        match self.mode.as_str() {
            "local" => ensure!(self.push_url.is_empty(), "本机素材模式不需要推流地址"),
            "rtmp" => {
                let url = reqwest::Url::parse(&self.push_url).context("推流地址格式错误")?;
                ensure!(
                    matches!(url.scheme(), "rtmp" | "rtmps")
                        && url.host_str().is_some()
                        && url.fragment().is_none()
                        && self.push_url.len() <= 4096,
                    "只支持 RTMP / RTMPS 地址"
                );
            }
            _ => bail!("未知输出模式"),
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Default)]
pub struct OutputStatus {
    pub id: String,
    pub device_id: String,
    pub source_kind: String,
    pub video_file: Option<String>,
    pub video_position_secs: f64,
    pub state: String,
    pub mode: String,
    pub source_url: Option<String>,
    pub error: Option<String>,
    pub reconnects: u32,
    pub video_frames: u64,
    pub audio_packets: u64,
    pub started_at: String,
    pub audio: bool,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub width: u32,
    pub height: u32,
}
struct SwitchCommand {
    source: source::Source,
    reply: oneshot::Sender<Result<OutputStatus>>,
}
pub struct OutputHandle {
    pub status: Arc<Mutex<OutputStatus>>,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
    devices: Arc<DeviceManager>,
    switches: mpsc::Sender<SwitchCommand>,
    switch_gate: AsyncMutex<()>,
    playlist: Option<Arc<video_folder::Playlist>>,
    hls_root: PathBuf,
}
impl OutputHandle {
    /// The trusted host owns the URL and HTTP boundary; Core only reads files
    /// produced by this active local output.
    pub async fn local_file(&self, name: &str) -> Response {
        let segment = name.strip_suffix(".ts").is_some_and(|number| {
            !number.is_empty()
                && number.len() <= 20
                && number.bytes().all(|byte| byte.is_ascii_digit())
        });
        if !(name == "index.m3u8" || segment) || self.status.lock().mode != "local" {
            return local_file_not_found();
        }
        if !self.local_available() {
            return local_file_unavailable();
        }
        match tokio::fs::read(self.hls_root.join(name)).await {
            Ok(data) if self.local_available() => (
                [
                    (
                        header::CONTENT_TYPE,
                        if segment {
                            "video/mp2t"
                        } else {
                            "application/vnd.apple.mpegurl"
                        },
                    ),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                data,
            )
                .into_response(),
            Ok(_) => local_file_unavailable(),
            Err(_) if name == "index.m3u8" => local_file_unavailable(),
            Err(_) => local_file_not_found(),
        }
    }
    fn local_available(&self) -> bool {
        let status = self.status.lock();
        status.mode == "local"
            && !["stopped", "failed"].contains(&status.state.as_str())
            && !self.finished()
            && !*self.stop.borrow()
    }
    pub async fn switch_device(&self, device_id: &str) -> Result<OutputStatus> {
        let _gate = self.switch_gate.lock().await;
        ensure!(
            !self.finished() && !*self.stop.borrow(),
            "live.output_not_running: 请先在直播助手开始输出"
        );
        let current = self.status.lock().clone();
        if current.source_kind == "device" && current.device_id == device_id {
            return Ok(current);
        }
        // Admission/preparation completes before touching the active producer.
        // A bad target leaves the old encoder, URL and source untouched.
        let source = tokio::time::timeout(
            Duration::from_secs(20),
            source::Source::start(self.devices.clone(), device_id),
        )
        .await
        .context("live.switch_prepare_timeout: 等待新设备画面超时")??;
        self.commit_source(source).await
    }
    /// Resume the initial folder at its last presented frame, keeping the encoder.
    pub async fn switch_video_folder(&self) -> Result<OutputStatus> {
        let _gate = self.switch_gate.lock().await;
        ensure!(
            !self.finished() && !*self.stop.borrow(),
            "live.output_not_running: 输出已结束"
        );
        let current = self.status.lock().clone();
        if current.source_kind == "video_folder" {
            return Ok(current);
        }
        let playlist = self
            .playlist
            .clone()
            .context("live.video_folder_missing: 本次输出没有配置视频文件夹")?;
        let source = tokio::time::timeout(
            Duration::from_secs(20),
            source::Source::start_video_folder(
                &self.devices.cfg.ffmpeg_path,
                playlist,
                current.fps,
                current.audio,
            ),
        )
        .await
        .context("live.switch_prepare_timeout: 等待视频文件画面超时")??;
        self.commit_source(source).await
    }
    async fn commit_source(&self, source: source::Source) -> Result<OutputStatus> {
        let (reply, result) = oneshot::channel();
        self.switches
            .send(SwitchCommand { source, reply })
            .await
            .map_err(|_| anyhow::anyhow!("live.output_stopped: 输出已结束"))?;
        tokio::time::timeout(Duration::from_secs(5), result)
            .await
            .context("live.switch_timeout: 输出切换超时")?
            .context("live.output_stopped: 输出已结束")?
    }
    pub async fn stop(mut self) {
        let _ = self.stop.send(true);
        // The worker owns and reaps its child before completing.
        if tokio::time::timeout(Duration::from_secs(30), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
            let _ = (&mut self.task).await;
        }
        self.status.lock().state = "stopped".into();
    }
    pub fn finished(&self) -> bool {
        self.task.is_finished()
    }
}
fn local_file_not_found() -> Response {
    (StatusCode::NOT_FOUND, [(header::CACHE_CONTROL, "no-store")]).into_response()
}
fn local_file_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::RETRY_AFTER, "1"),
        ],
    )
        .into_response()
}
impl Drop for OutputHandle {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}
pub async fn start(
    devices: Arc<DeviceManager>,
    req: OutputRequest,
    local_source_url: Option<String>,
) -> Result<OutputHandle> {
    let update_activity = crate::update::barrier::activity().map_err(anyhow::Error::msg)?;
    req.validate()?;
    let source_url = if req.mode == "local" {
        let url = local_source_url.context("本机素材模式需要宿主提供访问地址")?;
        ensure!(!url.trim().is_empty(), "本机素材模式需要宿主提供访问地址");
        Some(url)
    } else {
        None
    };
    let playlist = match req.video_folder.as_deref() {
        Some(folder) if !folder.trim().is_empty() => {
            Some(video_folder::Playlist::open(folder).await?)
        }
        _ => None,
    };
    let initial_source = tokio::time::timeout(
        Duration::from_secs(25),
        prepare_source(
            devices.clone(),
            &req,
            playlist.clone(),
            &req.device_id,
            playlist.is_some(),
        ),
    )
    .await
    .context("等待输出设备超时")??;
    let first = initial_source.frame().context("缺少输出画面")?;
    // BGR24 VFW scanlines use four-byte alignment; H.264 yuv420p needs
    // even dimensions. Pad the fixed canvas rather than stretching pixels.
    let (width, height) = ((first.width + 3) & !3, (first.height + 1) & !1);
    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = devices.cfg.data_dir.join("media-output").join(&id);
    tokio::fs::create_dir_all(&root).await?;
    let (stop, mut stopped) = watch::channel(false);
    let (switches, mut switch_rx) = mpsc::channel::<SwitchCommand>(4);
    let mut initial_status = OutputStatus {
        id,
        device_id: req.device_id.clone(),
        state: "preparing".into(),
        mode: req.mode.clone(),
        source_url,
        started_at: chrono::Utc::now().to_rfc3339(),
        audio: req.audio,
        fps: req.fps,
        bitrate_kbps: req.bitrate_kbps,
        width,
        height,
        ..Default::default()
    };
    initial_source.update_status(&mut initial_status);
    let status = Arc::new(Mutex::new(initial_status));
    let report = status.clone();
    let finished = stop.clone();
    let worker_devices = devices.clone();
    let worker_playlist = playlist.clone();
    let worker_root = root.clone();
    let task = tokio::spawn(async move {
        let _update_activity = update_activity;
        let devices = worker_devices;
        let root = worker_root;
        let mut prepared = Some((initial_source, None));
        for attempt in 0..=5 {
            if *stopped.borrow() {
                break;
            }
            let (current_device, folder_active) = {
                let state = report.lock();
                (state.device_id.clone(), state.source_kind == "video_folder")
            };
            let source = match prepared.take() {
                Some(source) => Ok(source),
                None => tokio::select! {
                    _ = stopped.changed() => break,
                    source = prepare_source(devices.clone(), &req, worker_playlist.clone(), &current_device, folder_active) => source.map(|source| (source, None)),
                    command = switch_rx.recv() => match command {
                        Some(command) if !command.reply.is_closed() => Ok((command.source, Some(command.reply))),
                        Some(command) => { command.source.stop().await; continue; },
                        None => break,
                    },
                },
            };
            let result = match source {
                Ok((source, reply)) => {
                    pump(
                        &devices.cfg.ffmpeg_path,
                        &req,
                        &root,
                        &report,
                        &mut stopped,
                        &mut switch_rx,
                        source,
                        reply,
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            if *stopped.borrow() {
                break;
            }
            match result {
                Ok(()) => break,
                Err(error) => {
                    tracing::warn!(%error, attempt, device_id = %current_device, "媒体输出重建编码进程");
                    let mut s = report.lock();
                    s.error = Some(error.to_string());
                    s.state = if attempt == 5 {
                        "failed"
                    } else {
                        "reconnecting"
                    }
                    .into();
                    s.reconnects = attempt + 1;
                }
            }
            if attempt == 5 {
                break;
            }
            tokio::select! {
                _ = stopped.changed() => break,
                _ = tokio::time::sleep(Duration::from_secs(1 << attempt.min(3))) => {},
                command = switch_rx.recv() => match command {
                    Some(command) if !command.reply.is_closed() => prepared = Some((command.source, Some(command.reply))),
                    Some(command) => command.source.stop().await,
                    None => break,
                },
            }
        }
        if *stopped.borrow() {
            report.lock().state = "stopped".into();
        }
        let _ = finished.send(true);
        let _ = tokio::fs::remove_dir_all(&root).await;
    });
    Ok(OutputHandle {
        status,
        stop,
        task,
        devices,
        switches,
        switch_gate: AsyncMutex::new(()),
        playlist,
        hls_root: root,
    })
}

async fn prepare_source(
    devices: Arc<DeviceManager>,
    request: &OutputRequest,
    playlist: Option<Arc<video_folder::Playlist>>,
    device_id: &str,
    folder: bool,
) -> Result<source::Source> {
    if folder {
        source::Source::start_video_folder(
            &devices.cfg.ffmpeg_path,
            playlist.context("视频文件夹未配置")?,
            request.fps,
            request.audio,
        )
        .await
    } else {
        source::Source::start(devices, device_id).await
    }
}

async fn write_packet(stdin: &mut tokio::process::ChildStdin, data: &[u8]) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(3), stdin.write_all(data))
        .await
        .context("媒体输出阻塞，正在恢复")?
        .context("媒体输出进程已结束")
}

// One output task owns both lifecycle channels and its frozen source context.
#[allow(clippy::too_many_arguments)]
async fn pump(
    ffmpeg: &str,
    req: &OutputRequest,
    root: &std::path::Path,
    status: &Arc<Mutex<OutputStatus>>,
    stop: &mut watch::Receiver<bool>,
    switches: &mut mpsc::Receiver<SwitchCommand>,
    mut source: source::Source,
    mut initial_reply: Option<oneshot::Sender<Result<OutputStatus>>>,
) -> Result<()> {
    let (width, height) = {
        let state = status.lock();
        (state.width, state.height)
    };
    let header = stream_mux::raw_header(width, height, req.audio)?;
    let mut cmd = ffmpeg_command(ffmpeg, req, root);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(_) => {
            source.stop().await;
            bail!("无法启动 FFmpeg，请检查服务端 ffmpeg_path");
        }
    };
    let mut stdin = child.stdin.take().context("FFmpeg 输入不可用")?;
    let mut reaping: Vec<JoinHandle<()>> = vec![];
    let encode = async {
        source.check()?;
        ensure!(
            !initial_reply
                .as_ref()
                .is_some_and(oneshot::Sender::is_closed),
            "live.switch_cancelled: 切换请求已取消"
        );
        write_packet(&mut stdin, &header).await?;
        source.activate();
        let origin = tokio::time::Instant::now();
        let mut video_clock =
            tokio::time::interval(Duration::from_micros(1_000_000 / req.fps as u64));
        let mut audio_clock = tokio::time::interval(Duration::from_millis(20));
        video_clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        audio_clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        let mut pending_audio = std::collections::VecDeque::<Vec<u8>>::new();
        let mut rendered: Option<(bytes::Bytes, u32, u32, bytes::Bytes)> = None;
        loop {
            reaping.retain(|task| !task.is_finished());
            tokio::select! {
                _ = stop.changed() => return Ok(()),
                command = switches.recv() => {
                    let Some(SwitchCommand { source: next, reply }) = command else { return Ok(()); };
                    if reply.is_closed() { reaping.push(tokio::spawn(next.stop())); continue; }
                    if let Err(error) = next.check() {
                        let _ = reply.send(Err(error)); reaping.push(tokio::spawn(next.stop())); continue;
                    }
                    let Some(frame) = next.frame() else { let _ = reply.send(Err(anyhow::anyhow!("新来源画面尚未就绪"))); reaping.push(tokio::spawn(next.stop())); continue; };
                    let prepared_frame = frame.clone();
                    let pixels = match tokio::task::spawn_blocking(move || source::fit(&prepared_frame, width, height)).await? {
                        Ok(pixels) => pixels,
                        Err(error) => { let _ = reply.send(Err(error)); reaping.push(tokio::spawn(next.stop())); continue; }
                    };
                    // A cancelled admission cannot commit after preparation.
                    if reply.is_closed() { reaping.push(tokio::spawn(next.stop())); continue; }
                    write_packet(&mut stdin, &stream_mux::raw_packet(1, origin.elapsed().as_micros() as u64, true, &pixels)?).await?;
                    source.deactivate();
                    next.activate();
                    next.presented(&frame);
                    let old = std::mem::replace(&mut source, next);
                    pending_audio.clear(); rendered = None;
                    let result = { let mut state = status.lock(); source.update_status(&mut state); state.video_frames += 1; state.error = None; state.clone() };
                    let _ = reply.send(Ok(result));
                    reaping.push(tokio::spawn(old.stop()));
                },
                _ = video_clock.tick() => {
                    ensure!(!initial_reply.as_ref().is_some_and(oneshot::Sender::is_closed), "live.switch_cancelled: 切换请求已取消");
                    let frame = source.frame().context("缺少视频画面")?;
                    let key = (frame.pixels.as_ptr() as usize, frame.width, frame.height);
                    if rendered.as_ref().is_none_or(|cache| (cache.0.as_ptr() as usize, cache.1, cache.2) != key) {
                        let original = frame.pixels.clone();
                        let prepared_frame = frame.clone();
                        let pixels = tokio::task::spawn_blocking(move || source::fit(&prepared_frame, width, height)).await??;
                        rendered = Some((original, key.1, key.2, pixels));
                    }
                    write_packet(&mut stdin, &stream_mux::raw_packet(1, origin.elapsed().as_micros() as u64, true, &rendered.as_ref().unwrap().3)?).await?;
                    source.presented(&frame);
                    { let mut state = status.lock(); source.update_status(&mut state); state.video_frames += 1; }
                    if let Some(reply) = initial_reply.take() {
                        let result = { let mut state = status.lock(); source.update_status(&mut state); state.error = None; state.clone() };
                        let _ = reply.send(Ok(result));
                    }
                },
                packet = source.audio_packet(), if req.audio => {
                    if let Some(packet) = packet? { pending_audio.push_back(packet); while pending_audio.len() > 5 { pending_audio.pop_front(); } }
                },
                _ = audio_clock.tick(), if req.audio => {
                    let packet = pending_audio.pop_front();
                    write_packet(&mut stdin, &stream_mux::raw_packet(2, origin.elapsed().as_micros() as u64, true, packet.as_deref().unwrap_or(&[0xf8,0xff,0xfe]))?).await?;
                    if packet.is_some() { status.lock().audio_packets += 1; }
                },
                _ = timer.tick() => {
                    source.check()?;
                    if let Some(exit) = child.try_wait()? { bail!("FFmpeg 输出失败（退出状态 {exit}），请检查编码支持和目标地址"); }
                    if req.mode != "local" || root.join("index.m3u8").exists() { let mut state = status.lock(); state.state = "streaming".into(); state.error = None; }
                }
            }
        }
    };
    let result = encode.await;
    source.stop().await;
    for task in reaping {
        let _ = task.await;
    }
    drop(stdin);
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}

fn ffmpeg_command(
    ffmpeg: &str,
    req: &OutputRequest,
    root: &std::path::Path,
) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-y",
        "-fflags",
        "+genpts",
        "-analyzeduration",
        "1000000",
        "-probesize",
        "1000000",
        "-f",
        "matroska",
        "-i",
        "pipe:0",
        "-map",
        "0:v:0",
    ]);
    if req.audio {
        cmd.args(["-map", "0:a:0", "-c:a", "aac", "-b:a", "128k"]);
    }
    cmd.args([
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-tune",
        "zerolatency",
        "-pix_fmt",
        "yuv420p",
        "-bf",
        "0",
        "-r",
    ])
    .arg(req.fps.to_string())
    .arg("-g")
    .arg(req.fps.to_string())
    .arg("-keyint_min")
    .arg(req.fps.to_string())
    .args(["-sc_threshold", "0", "-b:v"])
    .arg(format!("{}k", req.bitrate_kbps))
    .args(["-max_interleave_delta", "1000000"]);
    if req.mode == "local" {
        // Epoch start avoids stale segment cache collisions during reconnect.
        cmd.args([
            "-f",
            "hls",
            "-hls_time",
            "1",
            "-hls_list_size",
            "6",
            "-hls_flags",
            "delete_segments+independent_segments+temp_file",
            "-hls_start_number_source",
            "epoch_us",
            "-hls_segment_filename",
        ])
        .arg(root.join("%d.ts"))
        .arg(root.join("index.m3u8"));
    } else {
        cmd.args(["-rw_timeout", "5000000", "-f", "flv"])
            .arg(&req.push_url);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    cmd
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "output_switch_tests.rs"]
mod switch_tests;

#[cfg(test)]
#[path = "output_video_folder_tests.rs"]
mod video_folder_tests;
