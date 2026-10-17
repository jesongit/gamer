//! Bounded video producers. Switching a producer does not restart the encoder.
use crate::{
    core::ActivityKind,
    device::{scrcpy::AudioFrame, DeviceManager},
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use bytes::Bytes;
use parking_lot::Mutex;
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::{
    sync::{broadcast, watch},
    task::JoinHandle,
};

#[derive(Clone)]
pub(super) struct Frame {
    pub pixels: Bytes,
    pub width: u32,
    pub height: u32,
    pub playback: Option<(usize, f64)>,
}

pub(super) struct Source {
    pub device_id: String,
    pub(super) latest: Arc<Mutex<Option<Frame>>>,
    pub(super) error: Arc<Mutex<Option<String>>>,
    pub(super) audio: Option<broadcast::Receiver<AudioFrame>>,
    pub(super) stop: watch::Sender<bool>,
    pub(super) task: Option<JoinHandle<()>>,
    pub(super) playlist: Option<super::video_folder::Producer>,
}
impl Source {
    pub async fn start(devices: Arc<DeviceManager>, id: &str) -> Result<Self> {
        ensure!(
            crate::targets::capabilities(&devices, id)?.media_output,
            "target_capability_unsupported: 当前目标不支持音视频输出"
        );
        let latest = Arc::new(Mutex::new(None));
        let error = Arc::new(Mutex::new(None));
        let (stop, mut stopped) = watch::channel(false);
        let (task, audio) = if crate::targets::is_browser(id) {
            devices.browsers.prepare(id).await?;
            let (session, mut frames) = devices.browsers.output_stream(id).await?;
            let lease = BrowserOutputLease {
                session: session.clone(),
                released: false,
                cleaned: false,
            };
            let latest = latest.clone();
            let error = error.clone();
            let task = tokio::spawn(async move {
                let mut cancelling = stopped.clone();
                let result = async {
                    loop {
                        ensure!(session.is_alive(), "浏览器输出目标已断开");
                        let frame = frames.borrow_and_update().clone();
                        if let Some(frame) = frame {
                            if session.preview_stamp(&frame).await.is_ok() {
                                let decoded = tokio::task::spawn_blocking(move || {
                                    decode_browser(&frame.data)
                                })
                                .await??;
                                *latest.lock() = Some(decoded);
                            } else {
                                frames = session.refresh_media_output().await?;
                                continue;
                            }
                        } else {
                            frames = session.refresh_media_output().await?;
                            continue;
                        }
                        tokio::select! {
                            _ = stopped.changed() => return Ok::<_, anyhow::Error>(()),
                            changed = frames.changed() => changed.context("浏览器画面流已结束")?,
                        }
                    }
                };
                let result = tokio::select! { result = result => result, _ = cancelling.changed() => Ok(()) };
                if let Err(failure) = result {
                    *error.lock() = Some(failure.to_string());
                }
                lease.release().await;
            });
            (task, None)
        } else {
            let lease = devices.acquire_activity(id, ActivityKind::Extension);
            tokio::time::timeout(Duration::from_secs(15), devices.connect_device(id))
                .await
                .context("设备连接超时")??;
            let session = devices.session(id).context("设备会话不可用")?;
            let video = devices.frames_tx(id).context("视频广播不可用")?.subscribe();
            let audio = devices
                .audio_frames_tx(id)
                .context("音频广播不可用")?
                .subscribe();
            let cache = devices.frame_cache(id).context("视频缓存不可用")?;
            let initial = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Some(frames) = cache.initial_frames() {
                        if frames.iter().any(|frame| frame.is_keyframe) {
                            break frames;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .context("等待设备关键帧超时")?;
            let (width, height) = session.video_size();
            let pixels = Arc::new(Mutex::new(None));
            let output = latest.clone();
            let errors = error.clone();
            let ffmpeg = devices.cfg.ffmpeg_path.clone();
            let task = tokio::spawn(async move {
                let (decoder_stop, decoder_rx) = watch::channel(false);
                let decoder = super::super::live_decoder::run(
                    &ffmpeg,
                    width,
                    height,
                    initial,
                    video,
                    pixels.clone(),
                    decoder_rx,
                );
                tokio::pin!(decoder);
                let mut tick = tokio::time::interval(Duration::from_millis(10));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let (decoder_finished, result) = loop {
                    tokio::select! {
                        result = &mut decoder => break (true, result.and_then(|_| Err(anyhow!("视频解码已结束")))),
                        _ = stopped.changed() => break (false, Ok(())),
                        _ = tick.tick() => {
                            if !session.connected.load(Ordering::Acquire) || session.video_size() != (width, height) {
                                break (false, Err(anyhow!("设备连接或画面尺寸变化，正在重建输出")));
                            }
                            if let Some(pixels) = pixels.lock().clone() {
                                *output.lock() = Some(Frame { pixels, width, height, playback: None });
                            }
                        }
                    }
                };
                if let Err(failure) = result {
                    *errors.lock() = Some(failure.to_string());
                }
                let _ = decoder_stop.send(true);
                if !decoder_finished {
                    let _ = decoder.await;
                }
                lease.release();
            });
            (task, Some(audio))
        };
        let mut source = Self {
            device_id: id.into(),
            latest,
            error,
            audio,
            stop,
            task: Some(task),
            playlist: None,
        };
        let ready = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                source.check()?;
                if source.frame().is_some() {
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("等待输出画面超时")
        .and_then(|r| r);
        if let Err(error) = ready {
            source.stop().await;
            return Err(error);
        }
        if let Some(audio) = source.audio.as_mut() {
            while audio.try_recv().is_ok() {}
        }
        Ok(source)
    }
    pub fn frame(&self) -> Option<Frame> {
        self.latest.lock().clone()
    }
    pub async fn start_video_folder(
        ffmpeg: &str,
        playlist: Arc<super::video_folder::Playlist>,
        fps: u32,
        audio: bool,
    ) -> Result<Self> {
        super::video_folder::start(ffmpeg, playlist, fps, audio).await
    }
    pub fn activate(&self) {
        if let Some(playlist) = &self.playlist {
            playlist.activate();
        }
    }
    pub fn deactivate(&self) {
        if let Some(playlist) = &self.playlist {
            playlist.deactivate();
        }
    }
    pub fn presented(&self, frame: &Frame) {
        if let Some(playlist) = &self.playlist {
            playlist.presented(frame);
        }
    }
    pub fn update_status(&self, status: &mut super::OutputStatus) {
        status.device_id = self.device_id.clone();
        if let Some(playlist) = &self.playlist {
            status.source_kind = "video_folder".into();
            let (file, position) = playlist.position();
            status.video_file = Some(file);
            status.video_position_secs = position;
        } else {
            status.source_kind = "device".into();
            status.video_file = None;
            status.video_position_secs = 0.0;
        }
    }
    pub fn check(&self) -> Result<()> {
        if let Some(error) = self.error.lock().as_ref() {
            bail!("{error}");
        }
        ensure!(
            !self.task.as_ref().is_some_and(JoinHandle::is_finished),
            "输出画面采集已结束"
        );
        Ok(())
    }
    pub async fn audio_packet(&mut self) -> Result<Option<Vec<u8>>> {
        let Some(audio) = self.audio.as_mut() else {
            return std::future::pending().await;
        };
        match audio.recv().await {
            Ok(AudioFrame {
                data,
                is_config: false,
                ..
            }) => Ok(Some(data.to_vec())),
            Ok(_) => Ok(None),
            Err(broadcast::error::RecvError::Lagged(_)) => Ok(None),
            Err(_) => bail!("音频流中断，正在重建输出"),
        }
    }
    pub async fn stop(mut self) {
        self.deactivate();
        let _ = self.stop.send(true);
        if let Some(mut task) = self.task.take() {
            if tokio::time::timeout(Duration::from_secs(8), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
    }
}
struct BrowserOutputLease {
    session: Arc<crate::browser::CdpSession>,
    released: bool,
    cleaned: bool,
}
impl BrowserOutputLease {
    fn release_count(&mut self) {
        if !self.released {
            self.session.release_media_output_count();
            self.released = true;
        }
    }
    async fn release(mut self) {
        self.release_count();
        self.session.reconcile_media_output().await;
        self.cleaned = true;
    }
}
impl Drop for BrowserOutputLease {
    fn drop(&mut self) {
        self.release_count();
        if !self.cleaned {
            let session = self.session.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = tokio::time::timeout(
                        Duration::from_secs(30),
                        session.reconcile_media_output(),
                    )
                    .await;
                });
            }
        }
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        self.deactivate();
        let _ = self.stop.send(true);
    }
}

fn decode_browser(data: &str) -> Result<Frame> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
    let image = image::load_from_memory(&bytes)?.into_rgb8();
    let (width, height) = image.dimensions();
    let mut pixels = image.into_raw();
    ensure!(
        !pixels.is_empty() && pixels.len() <= 64 * 1024 * 1024,
        "浏览器输出画面过大"
    );
    for pixel in pixels.as_chunks_mut::<3>().0 {
        pixel.swap(0, 2);
    }
    Ok(Frame {
        pixels: pixels.into(),
        width,
        height,
        playback: None,
    })
}

pub(super) fn fit(frame: &Frame, width: u32, height: u32) -> Result<Bytes> {
    if (frame.width, frame.height) == (width, height) {
        return Ok(frame.pixels.clone());
    }
    // Resizing three BGR channels works identically to RGB interpolation.
    let image = image::RgbImage::from_raw(frame.width, frame.height, frame.pixels.to_vec())
        .context("输出画面字节与尺寸不符")?;
    let ratio = (width as f64 / frame.width as f64).min(height as f64 / frame.height as f64);
    let scaled_width = ((frame.width as f64 * ratio).round() as u32).clamp(1, width);
    let scaled_height = ((frame.height as f64 * ratio).round() as u32).clamp(1, height);
    let scaled = image::imageops::resize(
        &image,
        scaled_width,
        scaled_height,
        image::imageops::FilterType::Triangle,
    );
    let mut canvas = image::RgbImage::new(width, height);
    image::imageops::replace(
        &mut canvas,
        &scaled,
        ((width - scaled.width()) / 2) as i64,
        ((height - scaled.height()) / 2) as i64,
    );
    Ok(canvas.into_raw().into())
}

#[cfg(test)]
impl Source {
    pub fn fixture(id: &str, width: u32, height: u32, bgr: [u8; 3]) -> Self {
        let pixels: Vec<u8> = (0..width * height).flat_map(|_| bgr).collect();
        let latest = Arc::new(Mutex::new(Some(Frame {
            pixels: pixels.into(),
            width,
            height,
            playback: None,
        })));
        let (stop, mut stopped) = watch::channel(false);
        let task = tokio::spawn(async move {
            let _ = stopped.changed().await;
        });
        Self {
            device_id: id.into(),
            latest,
            error: Arc::new(Mutex::new(None)),
            audio: None,
            stop,
            task: Some(task),
            playlist: None,
        }
    }
    pub fn fail_for_test(&self) {
        *self.error.lock() = Some("fixture unavailable".into());
    }
}
