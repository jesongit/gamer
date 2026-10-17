//! Browser recording reuses the preview producer, but owns its subscription.
//! JPEGs retain elapsed timestamps; a VFR encode on stop preserves static holds.
use super::*;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::io::Read;

pub(super) struct BrowserRecording {
    ffmpeg: String,
    frames: Vec<u64>,
}

// Several grids can stop at once. One bounded encoder protects the live preview.
static ENCODER: Mutex<()> = Mutex::new(());
const ENCODE_TIMEOUT: Duration = Duration::from_secs(300);
const STREAM_RESTART_DELAY: Duration = Duration::from_millis(500);

struct EncoderChild(std::process::Child);
impl Drop for EncoderChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[derive(Debug)]
struct RecordingSizeChanged {
    expected: (u32, u32),
    actual: (u32, u32),
}
impl std::fmt::Display for RecordingSizeChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "画面尺寸从 {}×{} 变为 {}×{}，录制已结束，请重新开始录制",
            self.expected.0, self.expected.1, self.actual.0, self.actual.1
        )
    }
}
impl std::error::Error for RecordingSizeChanged {}

fn validate_recording_boundary(
    session: &crate::browser::CdpSession,
    identity: &crate::browser::FrameStamp,
    dimensions: (u32, u32),
) -> anyhow::Result<()> {
    anyhow::ensure!(session.is_alive(), "浏览器目标已断开");
    let current = session.stamp();
    // A recording belongs to a bound page and connection, rather than one
    // document revision. Navigation still retires every old input/frame stamp.
    anyhow::ensure!(
        current.target == identity.target && current.epoch == identity.epoch,
        "浏览器目标绑定已变化，录制已结束"
    );
    let actual = session.size();
    if actual != dimensions {
        return Err(RecordingSizeChanged {
            expected: dimensions,
            actual,
        }
        .into());
    }
    Ok(())
}
impl BrowserRecording {
    pub(super) fn new(ffmpeg: String) -> Self {
        Self {
            ffmpeg,
            frames: Vec::new(),
        }
    }
}

pub(super) fn spawn(shared: Arc<SessionShared>, session: Arc<crate::browser::CdpSession>) {
    tokio::spawn(async move {
        let mut subscribed = false;
        let result = collect(&shared, &session, &mut subscribed).await;
        if subscribed {
            session.stop_recording().await;
        }
        if let Err(error) = result {
            let reason = if error.downcast_ref::<RecordingSizeChanged>().is_some() {
                SegmentReason::CodecChange
            } else {
                SegmentReason::Disconnect
            };
            let _ = tokio::task::spawn_blocking(move || {
                shared.end_interrupted(reason, Some(error.to_string()));
            })
            .await;
        }
    });
}

async fn collect(
    shared: &Arc<SessionShared>,
    session: &crate::browser::CdpSession,
    subscribed: &mut bool,
) -> anyhow::Result<()> {
    if shared.stopping.load(Ordering::Acquire) {
        return Ok(());
    }
    let mut frames = session.start_recording().await?;
    *subscribed = true;
    let identity = session.stamp();
    let dimensions = {
        let st = shared.state.lock();
        (st.width, st.height)
    };
    let mut revision = identity.clone();
    let mut restart_pending = false;
    let mut next_restart = tokio::time::Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(33));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last = None;
    loop {
        tokio::select! {
            _ = shared.stop_notify.notified() => return Ok(()),
            _ = tick.tick() => {}
        }
        if shared.stopping.load(Ordering::Acquire) {
            return Ok(());
        }
        validate_recording_boundary(session, &identity, dimensions)?;
        if session.stamp() != revision {
            revision = session.stamp();
            if !restart_pending {
                next_restart = tokio::time::Instant::now();
            }
            restart_pending = true;
        }
        if restart_pending {
            if tokio::time::Instant::now() < next_restart {
                continue;
            }
            let stopped = shared.stop_notify.notified();
            tokio::pin!(stopped);
            stopped.as_mut().enable();
            if shared.stopping.load(Ordering::Acquire) {
                return Ok(());
            }
            let refreshed = tokio::select! {
                _ = stopped => return Ok(()),
                result = session.refresh_recording() => result,
            };
            match refreshed {
                Ok(receiver) => {
                    frames = receiver;
                    restart_pending = false;
                }
                Err(error) => {
                    validate_recording_boundary(session, &identity, dimensions)?;
                    // CDP can temporarily refuse screencast commands while a
                    // new document loads, even without another revision event.
                    tracing::debug!(%error, target = %session.id, "页面切换期间录制流暂未就绪，延迟重试");
                    next_restart = tokio::time::Instant::now() + STREAM_RESTART_DELAY;
                    continue;
                }
            }
            last = None;
        }
        let frame = frames.borrow_and_update().clone();
        let Some(frame) = frame else { continue };
        let timestamp = frame.metadata["timestamp"].as_f64();
        if timestamp == last {
            continue;
        }
        let stamp = match session.preview_stamp(&frame).await {
            Ok(stamp) => stamp,
            Err(error) => {
                validate_recording_boundary(session, &identity, dimensions)?;
                if frame.revision != session.stamp().revision
                    || session.validate_stream_frame(&frame).is_err()
                {
                    // Another consumer can restart the producer while this
                    // frame is being decoded. Never adopt its retired pixels.
                    continue;
                }
                return Err(error);
            }
        };
        validate_recording_boundary(session, &identity, dimensions)?;
        if session.validate(&stamp).is_err() {
            continue;
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(&frame.data)?;
        let writer = shared.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut st = writer.state.lock();
            if st.meta.state != RecordingState::Recording {
                return Ok(());
            }
            anyhow::ensure!(
                st.written_bytes + bytes.len() as u64 <= MAX_SESSION_BYTES,
                "录制临时文件达到 4 GiB 上限"
            );
            let now = st.elapsed_us();
            let recording = st.browser.as_mut().unwrap();
            if recording.frames.len().is_multiple_of(90) {
                anyhow::ensure!(
                    disk_free_bytes(&writer.dir).is_none_or(|n| n >= MIN_FREE_BYTES),
                    "磁盘空间不足，录制已结束"
                );
            }
            let index = recording.frames.len();
            std::fs::write(
                writer
                    .dir
                    .join("recording")
                    .join(format!("frame-{index:09}.jpg")),
                &bytes,
            )?;
            recording.frames.push(now);
            st.written_bytes += bytes.len() as u64;
            Ok(())
        })
        .await??;
        last = timestamp;
    }
}

/// Called under the existing recording state lock on a blocking worker.
pub(super) fn finish(
    shared: &SessionShared,
    recording: BrowserRecording,
    width: u32,
    height: u32,
    name: &str,
    end_us: u64,
    reason: SegmentReason,
) -> anyhow::Result<Option<SegmentMeta>> {
    let Some(&start_us) = recording.frames.first() else {
        return Ok(None);
    };
    let end_us = end_us.max(start_us + 1);
    let deadline = Instant::now() + ENCODE_TIMEOUT;
    let _encoder = loop {
        anyhow::ensure!(
            !shared.encoding_cancelled.load(Ordering::Acquire),
            "服务停止，录像原始帧保留"
        );
        if let Some(guard) = ENCODER.try_lock_for(Duration::from_millis(100)) {
            break guard;
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "等待视频保存超时，原始帧保留在录制目录"
        );
    };
    let mut manifest = String::from("ffconcat version 1.0\n");
    for (i, &time) in recording.frames.iter().enumerate() {
        let next = recording.frames.get(i + 1).copied().unwrap_or(end_us);
        manifest.push_str(&format!(
            "file frame-{i:09}.jpg\noption framerate 1000000\nduration {:.6}\n",
            next.saturating_sub(time).max(1) as f64 / 1_000_000.0
        ));
    }
    manifest.push_str(&format!(
        "file frame-{:09}.jpg\noption framerate 1000000\n",
        recording.frames.len() - 1
    ));
    let dir = shared.dir.join("recording");
    std::fs::write(dir.join("frames.ffconcat"), manifest)?;
    let stderr = std::fs::File::create(dir.join("encoder.log"))?;
    let mut command = std::process::Command::new(&recording.ffmpeg);
    command
        .current_dir(&dir)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-safe",
            "0",
            "-threads",
            "4",
            "-f",
            "concat",
            "-i",
            "frames.ffconcat",
            "-an",
            "-c:v",
            "libx264",
            "-bf",
            "0",
            "-x264-params",
            "fps=30/1",
            "-preset",
            "veryfast",
            "-threads",
            "4",
            "-filter_threads",
            "1",
            "-crf",
            "20",
            "-vf",
            "scale=in_range=pc:out_range=tv:out_color_matrix=bt709,pad=ceil(iw/2)*2:ceil(ih/2)*2",
            "-pix_fmt",
            "yuv420p",
            "-color_range",
            "tv",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-fps_mode",
            "vfr",
            "-enc_time_base",
            "1:1000000",
            "-video_track_timescale",
            "1000000",
            "-movflags",
            "+faststart",
            "../original.mp4",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(stderr);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Hidden window + lower CPU scheduling priority keeps live input responsive.
        command.creation_flags(0x08000000 | 0x00004000);
    }
    let mut child = EncoderChild(command.spawn()?);
    loop {
        anyhow::ensure!(
            !shared.encoding_cancelled.load(Ordering::Acquire),
            "服务停止，录像原始帧保留"
        );
        if let Some(status) = child.0.try_wait()? {
            anyhow::ensure!(
                status.success(),
                "FFmpeg 编码失败，详情见录制目录 encoder.log"
            );
            break;
        }
        if Instant::now() >= deadline {
            anyhow::bail!("FFmpeg 编码超时，原始帧保留在录制目录");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let path = shared.dir.join("original.mp4");
    let mut file = std::fs::File::open(&path)?;
    let size = file.metadata()?.len();
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        anyhow::ensure!(
            !shared.encoding_cancelled.load(Ordering::Acquire),
            "服务停止，录像原始帧保留"
        );
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    let sha = format!("{:x}", hasher.finalize());
    let media = MediaId(
        shared
            .dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    );
    write_media_metadata(
        &shared.dir,
        &media,
        name,
        width.div_ceil(2) * 2,
        height.div_ceil(2) * 2,
        MediaState::Ready,
        &sha,
        size,
        Some(end_us - start_us),
    );
    let segment = SegmentMeta {
        media_id: media,
        start_us,
        duration_us: end_us - start_us,
        base_pts_us: 0,
        reason,
    };
    for i in 0..recording.frames.len() {
        let _ = std::fs::remove_file(dir.join(format!("frame-{i:09}.jpg")));
    }
    let _ = std::fs::remove_file(dir.join("frames.ffconcat"));
    Ok(Some(segment))
}

#[cfg(test)]
#[path = "browser_navigation_tests.rs"]
mod navigation_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn failing_browser(shared: &Arc<SessionShared>) {
        shared.state.lock().browser = Some(BrowserRecording {
            ffmpeg: shared
                .dir
                .join("no-such-ffmpeg.exe")
                .to_string_lossy()
                .into_owned(),
            frames: vec![0],
        });
        std::fs::write(
            shared.dir.join("recording/frame-000000000.jpg"),
            b"retained evidence",
        )
        .unwrap();
    }

    async fn terminal(service: &RecordingService, id: &RecordingId) -> RecordingSessionMeta {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let meta = service.status(id).unwrap();
                if meta.state != RecordingState::Finalizing {
                    return meta;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("encoding worker must finish")
    }

    #[tokio::test]
    async fn accepted_stop_freezes_time_keeps_exclusivity_and_reports_failure() {
        let root = tempfile::tempdir().unwrap();
        let shared = crate::recording::tests::test_session(root.path(), "browser-stop");
        failing_browser(&shared);
        let service = RecordingService {
            inner: shared.inner.clone(),
        };
        {
            let _busy_encoder = ENCODER.lock();
            let meta = service.request_finish(&shared.id, false).unwrap();
            assert_eq!(meta.state, RecordingState::Finalizing);
            let stopped_us = shared.state.lock().elapsed_us();
            std::thread::sleep(Duration::from_millis(40));
            assert_eq!(shared.state.lock().elapsed_us(), stopped_us);
            assert_eq!(service.active_count(), 1);
            assert_eq!(
                service.active_for_device("browser-stop").unwrap().state,
                RecordingState::Finalizing
            );
            assert_eq!(service.request_finish(&shared.id, true).unwrap(), meta);
            assert!(service
                .delete_history(&shared.id)
                .unwrap_err()
                .to_string()
                .contains("recording_active"));
            shared.end_interrupted(SegmentReason::Disconnect, Some("late disconnect".into()));
            assert_eq!(
                shared.meta_snapshot(),
                meta,
                "disconnect cannot steal an accepted stop"
            );
        }
        let meta = terminal(&service, &shared.id).await;
        assert_eq!(meta.state, RecordingState::Failed);
        assert!(meta.error.unwrap().contains("浏览器录像封装失败"));
        assert_eq!(service.active_count(), 0);
        assert!(shared.dir.join("recording/frame-000000000.jpg").exists());
    }

    #[tokio::test]
    async fn shutdown_cancels_pending_encoder_and_preserves_frame_evidence() {
        let root = tempfile::tempdir().unwrap();
        let shared = crate::recording::tests::test_session(root.path(), "browser-drain");
        failing_browser(&shared);
        let service = RecordingService {
            inner: shared.inner.clone(),
        };
        // An owned worker queued at shutdown must not launch another encoder.
        shared.encoding_cancelled.store(true, Ordering::Release);
        let meta = service.request_finish(&shared.id, false).unwrap();
        assert_eq!(meta.state, RecordingState::Finalizing);
        service.shutdown().await;
        assert_eq!(service.active_count(), 0);
        assert_eq!(
            service.status(&shared.id).unwrap().state,
            RecordingState::Interrupted
        );
        assert!(service.inner.shutting_down.load(Ordering::Acquire));
        assert!(shared.dir.join("recording/frame-000000000.jpg").exists());
    }

    #[test]
    #[ignore = "requires GAMER_TEST_FFMPEG; local VFR encode regression"]
    fn irregular_frames_and_static_tail_preserve_playback_duration() {
        let root = tempfile::tempdir().unwrap();
        let shared = crate::recording::tests::test_session(root.path(), "browser-timing");
        let ffmpeg = std::env::var("GAMER_TEST_FFMPEG").expect("installed FFmpeg path");
        {
            let mut st = shared.state.lock();
            st.origin = Instant::now() - Duration::from_secs(4);
            st.width = 64;
            st.height = 48;
            st.browser = Some(BrowserRecording {
                ffmpeg: ffmpeg.clone(),
                frames: vec![100_000, 265_397, 1_848_048],
            });
        }
        for i in 0..3 {
            image::RgbImage::from_pixel(64, 48, image::Rgb([i * 80, 30, 90]))
                .save(
                    shared
                        .dir
                        .join("recording")
                        .join(format!("frame-{i:09}.jpg")),
                )
                .unwrap();
        }
        let meta = shared.finalize_stop();
        assert_eq!(meta.state, RecordingState::Completed, "{:?}", meta.error);
        assert_eq!(shared.finalize_stop(), meta);
        let file_bytes = std::fs::read(shared.dir.join("original.mp4")).unwrap();
        let metadata: MediaMetadata =
            serde_json::from_slice(&std::fs::read(shared.dir.join("metadata.json")).unwrap())
                .unwrap();
        assert_eq!(
            metadata.sha256,
            format!("{:x}", Sha256::digest(&file_bytes))
        );
        let segment = &meta.segments[0];
        let ffprobe = Path::new(&ffmpeg).with_file_name(if cfg!(windows) {
            "ffprobe.exe"
        } else {
            "ffprobe"
        });
        let mut command = std::process::Command::new(ffprobe);
        command
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration:frame=best_effort_timestamp_time:stream=color_range,color_space",
                "-of",
                "json",
            ])
            .arg(shared.dir.join("original.mp4"));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let output = command.output().unwrap();
        assert!(output.status.success());
        let probe: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(probe["streams"][0]["color_range"], "tv");
        assert_eq!(probe["streams"][0]["color_space"], "bt709");
        let duration: f64 = probe["format"]["duration"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((duration - segment.duration_us as f64 / 1e6).abs() < 0.005);
        let frames = probe["frames"].as_array().unwrap();
        let times: Vec<f64> = frames
            .iter()
            .map(|f| {
                f["best_effort_timestamp_time"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap()
            })
            .collect();
        assert!((times[1] - 0.165397).abs() < 0.00001);
        assert!(times.last().unwrap() - times[times.len() - 2] > 2.0);
        assert!(!shared.dir.join("recording/frame-000000000.jpg").exists());
    }
}
