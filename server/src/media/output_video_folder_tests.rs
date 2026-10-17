use super::*;

#[tokio::test]
async fn folder_inventory_is_sorted_and_rejects_missing_empty_and_file_paths() {
    let directory = tempfile::tempdir().unwrap();
    assert!(inspect_video_folder(directory.path().to_str().unwrap())
        .await
        .is_err());
    assert!(
        inspect_video_folder(directory.path().join("missing").to_str().unwrap())
            .await
            .is_err()
    );
    std::fs::write(directory.path().join("02.MP4"), b"fixture").unwrap();
    std::fs::write(directory.path().join("01.webm"), b"fixture").unwrap();
    std::fs::write(directory.path().join("ignore.txt"), b"fixture").unwrap();
    std::fs::create_dir(directory.path().join("directory.mp4")).unwrap();
    let info = inspect_video_folder(directory.path().to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(info.files, ["01.webm", "02.MP4"]);
    assert!(
        inspect_video_folder(directory.path().join("02.MP4").to_str().unwrap())
            .await
            .is_err()
    );
}

#[test]
fn media_request_requires_exactly_one_source() {
    for (value, valid) in [
        (serde_json::json!({"video_folder":"C:/videos"}), true),
        (serde_json::json!({"device_id":"a"}), true),
        (serde_json::json!({}), false),
        (
            serde_json::json!({"device_id":"a","video_folder":"C:/videos"}),
            false,
        ),
        (serde_json::json!({"video_folder":" "}), false),
    ] {
        let request: OutputRequest = serde_json::from_value(value).unwrap();
        assert_eq!(request.validate().is_ok(), valid);
    }
}

fn fixture(path: &std::path::Path, color: &str, seconds: &str, audio: bool) {
    let mut command = std::process::Command::new("ffmpeg");
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg(format!("color=c={color}:s=160x120:r=10"));
    if audio {
        command.args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"]);
    }
    command.args([
        "-t",
        seconds,
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
    ]);
    if audio {
        command.args(["-c:a", "aac", "-ac", "2"]);
    }
    command.arg(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn wait_status(
    output: &OutputHandle,
    predicate: impl Fn(&OutputStatus) -> bool,
) -> OutputStatus {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let status = output.status.lock().clone();
            assert_ne!(status.state, "failed", "{:?}", status.error);
            if predicate(&status) {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap()
}

async fn switch_fixture(output: &OutputHandle, id: &str) -> OutputStatus {
    let (reply, result) = oneshot::channel();
    output
        .switches
        .send(SwitchCommand {
            source: source::Source::fixture(id, 320, 90, [255, 0, 0]),
            reply,
        })
        .await
        .unwrap();
    result.await.unwrap().unwrap()
}

#[tokio::test]
#[ignore = "requires installed FFmpeg/ffprobe; synthetic local media and device fixture only"]
async fn local_folder_stream_preserves_audio_resume_position_and_output_on_cancelled_switch() {
    let directory = tempfile::tempdir().unwrap();
    let videos = directory.path().join("视频素材 folder");
    std::fs::create_dir(&videos).unwrap();
    // Corrupt entries are skipped; remaining files retain filename order.
    std::fs::write(videos.join("00 corrupt.mp4"), b"not a video").unwrap();
    fixture(&videos.join("01 red.mp4"), "red", "4", true);
    fixture(&videos.join("02 blue.mp4"), "blue", "1", false);
    let cfg = crate::config::Config {
        data_dir: directory.path().join("data"),
        adb_path: "unavailable-test-adb".into(),
        ..Default::default()
    };
    let store = Arc::new(crate::store::Store::open(&cfg).unwrap());
    let devices = Arc::new(DeviceManager::new(store, cfg));
    let request: OutputRequest = serde_json::from_value(
        serde_json::json!({"video_folder":videos.to_str().unwrap(),"fps":10,"bitrate_kbps":500}),
    )
    .unwrap();
    let output = start(devices, request, Some(tests::FIXED_LOCAL_URL.into()))
        .await
        .unwrap();
    let check = async {
        let before = wait_status(&output, |status| {
            status.state == "streaming" && status.video_position_secs >= 0.8
        })
        .await;
        assert_eq!(before.source_kind, "video_folder");
        assert_eq!(before.video_file.as_deref(), Some("01 red.mp4"));
        assert!(
            before.audio_packets > 15,
            "video's own audio must reach output"
        );
        let output_root = directory.path().join("data/media-output").join(&before.id);
        let manifest = std::fs::read_to_string(output_root.join("index.m3u8")).unwrap();
        let segment = manifest.lines().find(|line| line.ends_with(".ts")).unwrap();
        let mut decode_audio = std::process::Command::new("ffmpeg");
        decode_audio
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(output_root.join(segment))
            .args([
                "-map", "0:a:0", "-t", "0.5", "-ac", "1", "-ar", "48000", "-f", "s16le", "pipe:1",
            ]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            decode_audio.creation_flags(0x08000000);
        }
        let decoded = decode_audio.output().unwrap();
        assert!(
            decoded.status.success(),
            "{}",
            String::from_utf8_lossy(&decoded.stderr)
        );
        let samples = decoded
            .stdout
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs() as u64)
            .collect::<Vec<_>>();
        assert!(!samples.is_empty());
        assert!(
            samples.iter().sum::<u64>() / samples.len() as u64 > 100,
            "synthetic tone must survive Opus to AAC output"
        );
        let switched = switch_fixture(&output, "device-fixture").await;
        assert_eq!(switched.source_kind, "device");
        let bookmark = output.playlist.as_ref().unwrap().saved_position();
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(output.playlist.as_ref().unwrap().saved_position(), bookmark);
        assert!(output.switch_device("missing-device").await.is_err());
        assert_eq!(output.status.lock().device_id, "device-fixture");
        let prepared = source::Source::start_video_folder(
            "ffmpeg",
            output.playlist.clone().unwrap(),
            10,
            true,
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            output.playlist.as_ref().unwrap().saved_position(),
            bookmark,
            "prewarm must not advance playback"
        );
        let (reply, cancelled) = oneshot::channel();
        drop(cancelled);
        output
            .switches
            .send(SwitchCommand {
                source: prepared,
                reply,
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(output.status.lock().device_id, "device-fixture");
        let resumed = output.switch_video_folder().await.unwrap();
        assert_eq!(resumed.source_kind, "video_folder");
        assert_eq!(resumed.video_file.as_deref(), Some("01 red.mp4"));
        assert!(
            (resumed.video_position_secs - bookmark).abs() < 0.2,
            "must resume near held frame"
        );
        assert_eq!(resumed.id, before.id);
        assert_eq!(resumed.source_url, before.source_url);
        assert_eq!(
            (
                resumed.width,
                resumed.height,
                resumed.fps,
                resumed.bitrate_kbps
            ),
            (160, 120, 10, 500)
        );
        assert_eq!(
            resumed.reconnects, before.reconnects,
            "switch must preserve output process"
        );
        wait_status(&output, |status| {
            status.video_file.as_deref() == Some("02 blue.mp4")
        })
        .await;
        wait_status(&output, |status| {
            status.video_file.as_deref() == Some("01 red.mp4") && status.video_position_secs < 1.0
        })
        .await;
        let playlist = tests::local_bytes(&output, "index.m3u8").await;
        assert!(String::from_utf8(playlist).unwrap().contains("#EXTINF:"));
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    let playlist = output.playlist.clone().unwrap();
    output.stop().await;
    assert!(playlist.inactive());
    assert!(
        std::fs::read_dir(directory.path().join("data/media-output"))
            .unwrap()
            .next()
            .is_none()
    );
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires installed FFmpeg/ffprobe; synthetic files and repeated starts only, no device or listener"]
async fn repeated_local_starts_keep_the_host_url_and_only_serve_current_output_files() {
    let directory = tempfile::tempdir().unwrap();
    let videos = directory.path().join("videos");
    std::fs::create_dir(&videos).unwrap();
    fixture(&videos.join("red.mp4"), "red", "3", false);
    let devices = tests::fixture_devices(directory.path());
    let request: OutputRequest = serde_json::from_value(serde_json::json!({
        "video_folder":videos.to_str().unwrap(),"audio":false,"fps":10,"bitrate_kbps":500
    }))
    .unwrap();
    let mut previous_id = None;
    let mut previous_segment: Option<String> = None;
    for _ in 0..2 {
        let output = start(
            devices.clone(),
            request.clone(),
            Some(tests::FIXED_LOCAL_URL.into()),
        )
        .await
        .unwrap();
        let check = async {
            let status = wait_status(&output, |state| state.state == "streaming").await;
            assert_eq!(status.source_url.as_deref(), Some(tests::FIXED_LOCAL_URL));
            assert_ne!(previous_id.as_deref(), Some(status.id.as_str()));
            let manifest =
                String::from_utf8(tests::local_bytes(&output, "index.m3u8").await).unwrap();
            assert!(manifest.contains("#EXTINF:"));
            let segment = manifest
                .lines()
                .find(|line| line.ends_with(".ts"))
                .unwrap()
                .to_string();
            let number = segment.strip_suffix(".ts").unwrap().parse::<u64>().unwrap();
            assert!(
                number > 1_000_000_000_000_000,
                "epoch_us numbering must survive repeated starts"
            );
            assert_ne!(previous_segment.as_deref(), Some(segment.as_str()));
            if let Some(old_segment) = &previous_segment {
                assert_eq!(
                    output.local_file(old_segment).await.status(),
                    axum::http::StatusCode::NOT_FOUND
                );
            }
            let body = tests::local_bytes(&output, &segment).await;
            assert!(!body.is_empty());
            assert_eq!(body, std::fs::read(output.hls_root.join(&segment)).unwrap());
            previous_id = Some(status.id);
            previous_segment = Some(segment);
        };
        let result =
            futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
        output.stop().await;
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}

#[tokio::test]
#[ignore = "requires installed FFmpeg/ffprobe; corrupt synthetic file only"]
async fn corrupt_playlist_fails_admission_without_creating_output() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("broken.mp4"), b"invalid").unwrap();
    let playlist = video_folder::Playlist::open(directory.path().to_str().unwrap())
        .await
        .unwrap();
    assert!(
        source::Source::start_video_folder("ffmpeg", playlist.clone(), 10, true)
            .await
            .is_err()
    );
    assert!(playlist.inactive());
}

#[tokio::test]
#[ignore = "requires installed FFmpeg/ffprobe; synthetic temporal-frame resume"]
async fn resumed_folder_decodes_the_last_presented_frame() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("clock.mp4");
    let mut generate = std::process::Command::new("ffmpeg");
    generate
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x120:rate=10",
            "-t",
            "3",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
        ])
        .arg(&path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        generate.creation_flags(0x08000000);
    }
    assert!(generate.output().unwrap().status.success());
    let playlist = video_folder::Playlist::open(directory.path().to_str().unwrap())
        .await
        .unwrap();
    let source = source::Source::start_video_folder("ffmpeg", playlist.clone(), 10, false)
        .await
        .unwrap();
    source.activate();
    tokio::time::sleep(Duration::from_millis(850)).await;
    let presented = source.frame().unwrap();
    source.presented(&presented);
    source.deactivate();
    let position = playlist.saved_position();
    assert!(position >= 0.7);
    // Decoding may have run ahead, but only the presented frame sets the bookmark.
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(playlist.saved_position(), position);
    source.stop().await;
    let resumed = source::Source::start_video_folder("ffmpeg", playlist.clone(), 10, false)
        .await
        .unwrap();
    assert_eq!(
        resumed.frame().unwrap().pixels,
        presented.pixels,
        "seek must restore the actual frame, not only the reported position"
    );
    resumed.stop().await;
}
