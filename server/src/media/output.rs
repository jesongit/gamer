//! Platform-neutral, process-owned media output. No platform credentials or room semantics.
use super::stream_mux;
use crate::{
    core::ActivityKind,
    device::{scrcpy::AudioFrame, DeviceManager},
};
use anyhow::{bail, ensure, Context, Result};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    process::Stdio,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::{io::AsyncWriteExt, sync::watch, task::JoinHandle};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputRequest {
    pub device_id: String,
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
        ensure!(!self.device_id.trim().is_empty(), "请选择设备");
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
}
pub struct OutputHandle {
    pub status: Arc<Mutex<OutputStatus>>,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}
impl OutputHandle {
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
impl Drop for OutputHandle {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}
#[derive(Clone)]
struct SourceFiles {
    root: PathBuf,
    token: String,
}
async fn source_file(
    State(files): State<SourceFiles>,
    Path((token, name)): Path<(String, String)>,
) -> impl IntoResponse {
    let segment = name
        .strip_suffix(".ts")
        .is_some_and(|n| !n.is_empty() && n.len() <= 20 && n.bytes().all(|b| b.is_ascii_digit()));
    if token != files.token || !(name == "index.m3u8" || segment) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::read(files.root.join(&name)).await {
        Ok(data) => (
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
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
pub async fn start(devices: Arc<DeviceManager>, req: OutputRequest) -> Result<OutputHandle> {
    req.validate()?;
    ensure!(devices.snapshot(&req.device_id).is_some(), "设备不存在");
    let lease = devices.acquire_activity(&req.device_id, ActivityKind::Extension);
    // Connection has a bounded startup; later retries run inside the owned worker.
    tokio::time::timeout(
        Duration::from_secs(15),
        devices.connect_device(&req.device_id),
    )
    .await
    .context("设备连接超时")??;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = devices.cfg.data_dir.join("media-output").join(&id);
    tokio::fs::create_dir_all(&root).await?;
    let (stop, mut stopped) = watch::channel(false);
    let (source_url, server) = if req.mode == "local" {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let url = format!(
            "http://127.0.0.1:{}/{token}/index.m3u8",
            listener.local_addr()?.port()
        );
        let app = Router::new()
            .route("/:token/:name", get(source_file))
            .with_state(SourceFiles {
                root: root.clone(),
                token,
            });
        let mut cancel = stop.subscribe();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = cancel.changed().await;
                })
                .await;
        });
        (Some(url), Some(server))
    } else {
        (None, None)
    };
    let status = Arc::new(Mutex::new(OutputStatus {
        id,
        device_id: req.device_id.clone(),
        state: "preparing".into(),
        mode: req.mode.clone(),
        source_url,
        started_at: chrono::Utc::now().to_rfc3339(),
        audio: req.audio,
        fps: req.fps,
        bitrate_kbps: req.bitrate_kbps,
        ..Default::default()
    }));
    let report = status.clone();
    let finished = stop.clone();
    let task = tokio::spawn(async move {
        let _lease = lease;
        for attempt in 0..=5 {
            if *stopped.borrow() {
                break;
            }
            let result = pump(&devices, &req, &root, &report, &mut stopped).await;
            if *stopped.borrow() {
                break;
            }
            match result {
                Ok(()) => break,
                Err(error) => {
                    let mut s = report.lock();
                    s.error = Some(error.to_string());
                    s.state = if attempt == 5 {
                        "failed"
                    } else {
                        "reconnecting"
                    }
                    .into();
                    s.reconnects = attempt;
                }
            }
            if attempt == 5 {
                break;
            }
            tokio::select! { _ = stopped.changed() => break, _ = tokio::time::sleep(Duration::from_secs(1 << attempt.min(3))) => {} }
        }
        if *stopped.borrow() {
            report.lock().state = "stopped".into();
        }
        let _ = finished.send(true);
        if let Some(mut server) = server {
            if tokio::time::timeout(Duration::from_secs(2), &mut server)
                .await
                .is_err()
            {
                server.abort();
                let _ = server.await;
            }
        }
        let _ = tokio::fs::remove_dir_all(&root).await;
    });
    Ok(OutputHandle { status, stop, task })
}

async fn write_packet(stdin: &mut tokio::process::ChildStdin, data: &[u8]) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(3), stdin.write_all(data))
        .await
        .context("媒体输出阻塞，正在恢复")?
        .context("媒体输出进程已结束")
}

async fn pump(
    devices: &Arc<DeviceManager>,
    req: &OutputRequest,
    root: &std::path::Path,
    status: &Arc<Mutex<OutputStatus>>,
    stop: &mut watch::Receiver<bool>,
) -> Result<()> {
    tokio::select! {
        _=stop.changed()=>return Ok(()),
        result=tokio::time::timeout(Duration::from_secs(12),devices.connect_device(&req.device_id))=>result.context("等待设备超时")??,
    }
    let session = devices.session(&req.device_id).context("设备会话不可用")?;
    let video = devices
        .frames_tx(&req.device_id)
        .context("视频广播不可用")?
        .subscribe();
    let mut audio = devices
        .audio_frames_tx(&req.device_id)
        .context("音频广播不可用")?
        .subscribe();
    let cache = devices
        .frame_cache(&req.device_id)
        .context("视频缓存不可用")?;
    let initial = tokio::select! {
      _=stop.changed()=>return Ok(()),
      result=tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(frames) = cache.initial_frames() {
                if frames.iter().any(|f| f.is_keyframe) {
                    break frames;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })=>result.context("等待设备关键帧超时")?,
    };
    let (width, height) = session.video_size();
    let header = stream_mux::raw_header(width, height, req.audio)?;
    if *stop.borrow() {
        return Ok(());
    }
    let latest = Arc::new(Mutex::new(None));
    let (decode_stop, decode_rx) = watch::channel(false);
    let decoder = super::live_decoder::run(
        &devices.cfg.ffmpeg_path,
        width,
        height,
        initial,
        video,
        latest.clone(),
        decode_rx,
    );
    tokio::pin!(decoder);
    // Drive the decoder while waiting for its first complete frame.
    let prepared = tokio::select! {
        result=&mut decoder=>return result.and_then(|_|anyhow::bail!("视频解码已结束")),
        _=stop.changed()=>false,
        result=tokio::time::timeout(Duration::from_secs(10),async {
            while latest.lock().is_none() {tokio::time::sleep(Duration::from_millis(10)).await;}
        })=>result.is_ok(),
    };
    if !prepared {
        let _ = decode_stop.send(true);
        let _ = decoder.await;
        ensure!(*stop.borrow(), "等待解码画面超时");
        return Ok(());
    }
    // Discard audio accumulated during decoder startup; fresh audio and repeated
    // decoded video share one monotonic output clock, including static screens.
    while audio.try_recv().is_ok() {}
    let _ = tokio::fs::remove_file(root.join("index.m3u8")).await;
    let mut cmd = ffmpeg_command(&devices.cfg.ffmpeg_path, req, root);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(_) => {
            let _ = decode_stop.send(true);
            let _ = decoder.await;
            bail!("无法启动 FFmpeg，请检查服务端 ffmpeg_path");
        }
    };
    let mut stdin = child.stdin.take().context("FFmpeg 输入不可用")?;
    let encode = async {
        write_packet(&mut stdin, &header).await?;
        let origin = tokio::time::Instant::now();
        let mut video_clock =
            tokio::time::interval(Duration::from_micros(1_000_000 / req.fps as u64));
        let mut audio_clock = tokio::time::interval(Duration::from_millis(20));
        video_clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        audio_clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        let mut pending_audio = std::collections::VecDeque::new();
        loop {
            tokio::select! {
                _=stop.changed()=>return Ok(()),
                _=video_clock.tick()=>{
                    let pixels=latest.lock().clone().context("缺少视频画面")?;
                    write_packet(&mut stdin,&stream_mux::raw_packet(1,origin.elapsed().as_micros() as u64,true,&pixels)?).await?;
                    status.lock().video_frames+=1;
                },
                frame=audio.recv(),if req.audio=>match frame {
                    Ok(AudioFrame {data,is_config:false,..})=>{pending_audio.push_back(data);while pending_audio.len()>5 {pending_audio.pop_front();}},
                    Ok(_)=>{},
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>{pending_audio.clear();},
                    Err(_)=>bail!("音频流中断，正在重建输出"),
                },
                _=audio_clock.tick(),if req.audio=>{
                    let data=pending_audio.pop_front();
                    // Standard Opus 20 ms DTX silence keeps muxing live when a
                    // device/app cannot capture audio. Never captures a microphone.
                    write_packet(&mut stdin,&stream_mux::raw_packet(2,origin.elapsed().as_micros() as u64,true,data.as_deref().unwrap_or(&[0xf8,0xff,0xfe]))?).await?;
                    if data.is_some(){status.lock().audio_packets+=1;}
                },
                _=timer.tick()=>{
                    ensure!(session.connected.load(Ordering::Acquire),"设备已断开");
                    ensure!(session.video_size()==(width,height),"设备分辨率变化，正在重建输出");
                    if let Some(exit)=child.try_wait()? {bail!("FFmpeg 输出失败（退出状态 {exit}），请检查编码支持和目标地址");}
                    if req.mode!="local"||root.join("index.m3u8").exists(){let mut s=status.lock();s.state="streaming".into();s.error=None;}
                }
            }
        }
    };
    let (decoder_finished, result) = tokio::select! {result=&mut decoder=>(true,result.and_then(|_|anyhow::bail!("视频解码已结束"))), result=encode=>(false,result)};
    let _ = decode_stop.send(true);
    if !decoder_finished {
        let _ = decoder.await;
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
