use super::*;
use crate::media::stream_mux;
use axum::http::StatusCode;
use std::{process::Command, time::Duration};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn local_files_only_serve_playlists_and_numeric_segments_with_uncached_mime_types() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("index.m3u8"),
        b"#EXTM3U\n1758000000000000.ts",
    )
    .unwrap();
    std::fs::write(temp.path().join("1758000000000000.ts"), b"segment").unwrap();
    std::fs::write(temp.path().join("0.ts"), b"zero").unwrap();
    std::fs::write(
        temp.path().join("12345678901234567890.ts"),
        b"twenty digits",
    )
    .unwrap();
    std::fs::write(
        temp.path().join("123456789012345678901.ts"),
        b"too many digits",
    )
    .unwrap();
    let output = handle_fixture(temp.path());
    for (name, mime, body) in [
        (
            "index.m3u8",
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n1758000000000000.ts".as_slice(),
        ),
        ("1758000000000000.ts", "video/mp2t", b"segment".as_slice()),
        ("0.ts", "video/mp2t", b"zero".as_slice()),
        (
            "12345678901234567890.ts",
            "video/mp2t",
            b"twenty digits".as_slice(),
        ),
    ] {
        let response = output.local_file(name).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], mime);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
                .as_ref(),
            body
        );
    }
    for name in [
        "",
        "../config.toml",
        "config.toml",
        "../index.m3u8",
        "../0.ts",
        "a/0.ts",
        "a\\0.ts",
        "%2e%2e/0.ts",
        "1.ts?token=secret",
        ".ts",
        "-1.ts",
        "+1.ts",
        "1.0.ts",
        "123456789012345678901.ts",
        "INDEX.M3U8",
        "0001.TS",
        "1.ts.tmp",
        "99.ts",
    ] {
        assert_eq!(
            output.local_file(name).await.status(),
            StatusCode::NOT_FOUND,
            "{name}"
        );
    }
    output.stop().await;
}

pub(super) const FIXED_LOCAL_URL: &str = "http://127.0.0.1:8443/local-media/index.m3u8";

pub(super) fn fixture_devices(root: &std::path::Path) -> Arc<DeviceManager> {
    let cfg = crate::config::Config {
        data_dir: root.join("fixture-data"),
        adb_path: "unavailable-test-adb".into(),
        ..Default::default()
    };
    let store = Arc::new(crate::store::Store::open(&cfg).unwrap());
    Arc::new(DeviceManager::new(store, cfg))
}

fn handle_fixture(root: &std::path::Path) -> OutputHandle {
    let (stop, mut stopped) = watch::channel(false);
    let task = tokio::spawn(async move {
        let _ = stopped.changed().await;
    });
    let (switches, _) = mpsc::channel(4);
    OutputHandle {
        status: Arc::new(Mutex::new(OutputStatus {
            mode: "local".into(),
            state: "streaming".into(),
            source_url: Some(FIXED_LOCAL_URL.into()),
            ..Default::default()
        })),
        stop,
        task,
        devices: fixture_devices(root),
        switches,
        switch_gate: AsyncMutex::new(()),
        playlist: None,
        hls_root: root.into(),
    }
}

pub(super) async fn local_bytes(output: &OutputHandle, name: &str) -> Vec<u8> {
    let response = output.local_file(name).await;
    assert_eq!(response.status(), StatusCode::OK, "{name}");
    axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn local_files_are_retryable_while_preparing_or_stopped_and_never_serve_rtmp() {
    let temp = tempfile::tempdir().unwrap();
    let output = handle_fixture(temp.path());
    let preparing = output.local_file("index.m3u8").await;
    assert_eq!(preparing.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(preparing.headers()[header::RETRY_AFTER], "1");
    assert_eq!(preparing.headers()[header::CACHE_CONTROL], "no-store");
    std::fs::write(temp.path().join("index.m3u8"), b"#EXTM3U").unwrap();
    assert_eq!(
        output.local_file("index.m3u8").await.status(),
        StatusCode::OK
    );
    output.status.lock().mode = "rtmp".into();
    assert_eq!(
        output.local_file("index.m3u8").await.status(),
        StatusCode::NOT_FOUND
    );
    output.status.lock().mode = "local".into();
    output.status.lock().state = "stopped".into();
    assert_eq!(
        output.local_file("index.m3u8").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        output.local_file("../index.m3u8").await.status(),
        StatusCode::NOT_FOUND
    );
    output.status.lock().state = "streaming".into();
    output.stop.send(true).unwrap();
    assert_eq!(
        output.local_file("index.m3u8").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    output.stop().await;

    let output = handle_fixture(temp.path());
    output.task.abort();
    while !output.finished() {
        tokio::task::yield_now().await;
    }
    assert!(temp.path().join("index.m3u8").exists());
    assert_eq!(
        output.local_file("index.m3u8").await.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "finished workers must not expose retained files"
    );
    output.stop().await;
}

#[tokio::test]
async fn local_output_requires_host_url_before_preparing_a_source() {
    let temp = tempfile::tempdir().unwrap();
    let devices = fixture_devices(temp.path());
    let req: OutputRequest =
        serde_json::from_value(serde_json::json!({"device_id":"fixture"})).unwrap();
    for supplied in [None, Some("".into()), Some("  ".into())] {
        let error = start(devices.clone(), req.clone(), supplied)
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("宿主提供访问地址"), "{error}");
    }
    assert!(!devices.cfg.data_dir.join("media-output").exists());
}

fn ffmpeg(args: &[&str], path: &std::path::Path) {
    let mut c = Command::new("ffmpeg");
    c.args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    let r = c
        .output()
        .expect("FFmpeg must be installed for this explicit integration test");
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
}

/// Demux Ogg lacing for fixture Opus packets (production consumes scrcpy packets).
fn opus_packets(data: &[u8]) -> Vec<Vec<u8>> {
    let mut offset = 0;
    let mut pending = Vec::new();
    let mut out = Vec::new();
    while offset < data.len() {
        assert_eq!(&data[offset..offset + 4], b"OggS");
        let n = data[offset + 26] as usize;
        let sizes = &data[offset + 27..offset + 27 + n];
        let mut pos = offset + 27 + n;
        for size in sizes {
            pending.extend_from_slice(&data[pos..pos + *size as usize]);
            pos += *size as usize;
            if *size < 255 {
                out.push(std::mem::take(&mut pending));
            }
        }
        offset = pos;
    }
    out.into_iter()
        .filter(|p| !p.starts_with(b"OpusHead") && !p.starts_with(b"OpusTags"))
        .collect()
}

#[tokio::test]
#[ignore = "requires local FFmpeg/ffprobe; synthetic media only, no device or broadcast"]
async fn ffmpeg_hls_preserves_timestamped_video_and_game_audio() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let h264 = root.join("source.h264");
    let opus = root.join("source.ogg");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x120:rate=25",
            "-t",
            "4",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-x264-params",
            "aud=1:keyint=1:bframes=0",
            "-f",
            "h264",
        ],
        &h264,
    );
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "4.5",
            "-ac",
            "2",
            "-c:a",
            "libopus",
            "-frame_duration",
            "20",
        ],
        &opus,
    );
    let data = std::fs::read(&h264).unwrap();
    let nals = nals(&data);
    let mut config = Vec::new();
    let mut frames = Vec::new();
    let mut frame = Vec::new();
    for n in nals {
        if n[0] & 31 == 9 && !frame.is_empty() {
            frames.push(std::mem::take(&mut frame));
        }
        if (n[0] & 31 == 7 || n[0] & 31 == 8) && config.len() < 100 {
            config.extend_from_slice(&[0, 0, 0, 1]);
            config.extend_from_slice(n);
        }
        frame.extend_from_slice(&[0, 0, 0, 1]);
        frame.extend_from_slice(n);
    }
    frames.push(frame);
    assert_eq!(frames.len(), 100);
    let rgb = root.join("source.bgr");
    ffmpeg(
        &[
            "-i",
            h264.to_str().unwrap(),
            "-pix_fmt",
            "bgr24",
            "-f",
            "rawvideo",
        ],
        &rgb,
    );
    let raw = std::fs::read(rgb).unwrap();
    let frames = raw.as_chunks::<{ 160 * 120 * 3 }>().0;
    let mut packets = Vec::new();
    for (i, f) in frames.iter().enumerate() {
        let pts = 250_000 + i as u64 * 40_000 + if i >= 20 { 200_000 } else { 0 };
        packets.push((
            pts,
            stream_mux::raw_packet(1, pts, true, f.as_slice()).unwrap(),
        ));
    }
    for (i, p) in opus_packets(&std::fs::read(opus).unwrap())
        .iter()
        .enumerate()
    {
        let pts = i as u64 * 20_000;
        packets.push((pts, stream_mux::raw_packet(2, pts, true, p).unwrap()));
    }
    packets.sort_by_key(|p| p.0);
    let mut mkv = stream_mux::raw_header(160, 120, true).unwrap();
    for (_, p) in packets {
        mkv.extend(p);
    }
    let fixture = root.join("timed.mkv");
    std::fs::write(&fixture, &mkv).unwrap();
    let roundtrip = root.join("roundtrip.bgr");
    ffmpeg(
        &[
            "-i",
            fixture.to_str().unwrap(),
            "-frames:v",
            "1",
            "-pix_fmt",
            "bgr24",
            "-f",
            "rawvideo",
        ],
        &roundtrip,
    );
    assert_eq!(
        std::fs::read(roundtrip).unwrap(),
        frames[0],
        "raw bridge must preserve orientation and colors"
    );
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v",
            "-show_entries",
            "frame=pts_time",
            "-of",
            "json",
        ])
        .arg(&fixture)
        .output()
        .unwrap();
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let info: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(info["frames"][0]["pts_time"], "0.250000");
    assert_eq!(info["frames"][20]["pts_time"], "1.250000");
    let req: OutputRequest =
        serde_json::from_value(serde_json::json!({"device_id":"fixture"})).unwrap();
    let mut cmd = ffmpeg_command("ffmpeg", &req, root);
    cmd.stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(&mkv).await.unwrap();
    drop(input);
    let result = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let manifest = std::fs::read_to_string(root.join("index.m3u8")).unwrap();
    assert!(manifest.contains("#EXTINF:"));
    for name in manifest
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        assert!(root.join(name).is_file());
    }
    let probe = Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(root.join("index.m3u8"))
        .output()
        .unwrap();
    assert!(probe.status.success());
    let info: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    let streams = info["streams"].as_array().unwrap();
    assert!(streams.iter().any(|s| s["codec_name"] == "h264"));
    assert!(streams.iter().any(|s| s["codec_name"] == "aac"));
    // Same encoder command also sends a real local RTMP session. No public
    // platform or credentials are involved.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("rtmp://127.0.0.1:{port}/live/fixture");
    let received = root.join("received.flv");
    let mut receiver = tokio::process::Command::new("ffmpeg");
    receiver
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-listen",
            "1",
            "-i",
            &url,
            "-c",
            "copy",
            "-f",
            "flv",
        ])
        .arg(&received)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    receiver.creation_flags(0x08000000);
    let receiver = receiver.spawn().unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let req: OutputRequest = serde_json::from_value(
        serde_json::json!({"device_id":"fixture","mode":"rtmp","push_url":url}),
    )
    .unwrap();
    let mut sender = ffmpeg_command("ffmpeg", &req, root);
    sender.stderr(std::process::Stdio::piped());
    let mut sender = sender.spawn().unwrap();
    let mut input = sender.stdin.take().unwrap();
    input.write_all(&mkv).await.unwrap();
    drop(input);
    let result = tokio::time::timeout(Duration::from_secs(15), sender.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let _ = tokio::time::timeout(Duration::from_secs(10), receiver.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    let probe = Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(received)
        .output()
        .unwrap();
    assert!(probe.status.success());
    let info: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    let streams = info["streams"].as_array().unwrap();
    assert!(streams.iter().any(|s| s["codec_name"] == "h264"));
    assert!(streams.iter().any(|s| s["codec_name"] == "aac"));
}

pub fn nals(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        let len = if data[i..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if data[i..].starts_with(&[0, 0, 1]) {
            3
        } else {
            i += 1;
            continue;
        };
        starts.push((i, i + len));
        i += len;
    }
    starts
        .iter()
        .enumerate()
        .filter_map(|(i, (_, start))| {
            let end = starts.get(i + 1).map_or(data.len(), |s| s.0);
            (*start < end).then_some(&data[*start..end])
        })
        .collect()
}

#[tokio::test]
#[ignore = "requires FFmpeg; checks single static frame without any device"]
async fn static_decoder_yields_without_next_device_frame_and_stops() {
    use crate::device::scrcpy::VideoFrame;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("one.h264");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x120:rate=25",
            "-frames:v",
            "1",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-f",
            "h264",
        ],
        &path,
    );
    let data = std::fs::read(path).unwrap();
    let mut config = Vec::new();
    for n in nals(&data) {
        if n[0] & 31 == 7 || n[0] & 31 == 8 {
            config.extend_from_slice(&[0, 0, 0, 1]);
            config.extend_from_slice(n);
        }
    }
    let initial = vec![
        VideoFrame {
            data: config.into(),
            pts_us: 0,
            is_config: true,
            is_keyframe: false,
            annex_b: true,
        },
        VideoFrame {
            data: data.into(),
            pts_us: 100,
            is_config: false,
            is_keyframe: true,
            annex_b: true,
        },
    ];
    let (_tx, rx) = tokio::sync::broadcast::channel(8);
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let latest = std::sync::Arc::new(parking_lot::Mutex::new(None));
    let decoder =
        crate::media::live_decoder::run("ffmpeg", 160, 120, initial, rx, latest.clone(), stopped);
    tokio::pin!(decoder);
    tokio::select! { result=&mut decoder=>panic!("decoder exited early: {result:?}"),result=tokio::time::timeout(Duration::from_secs(5),async {while latest.lock().is_none(){tokio::time::sleep(Duration::from_millis(10)).await;}})=>result.expect("static frame must decode without another source frame") }
    assert_eq!(latest.lock().as_ref().unwrap().len(), 160 * 120 * 3);
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(3), decoder)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[ignore = "requires FFmpeg; verifies live static HLS before stdin closes"]
async fn static_hls_publishes_before_input_eof() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let req: OutputRequest =
        serde_json::from_value(serde_json::json!({"device_id":"fixture","fps":25})).unwrap();
    let mut cmd = ffmpeg_command("ffmpeg", &req, root);
    let mut child = cmd.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(&stream_mux::raw_header(160, 120, true).unwrap())
        .await
        .unwrap();
    let pixels = vec![60; 160 * 120 * 3];
    let mut clock = tokio::time::interval(Duration::from_millis(20));
    for i in 0..200u64 {
        clock.tick().await;
        if i % 2 == 0 {
            input
                .write_all(&stream_mux::raw_packet(1, i * 20_000, true, &pixels).unwrap())
                .await
                .unwrap();
        }
        input
            .write_all(&stream_mux::raw_packet(2, i * 20_000, true, &[0xf8, 0xff, 0xfe]).unwrap())
            .await
            .unwrap();
    }
    assert!(child.try_wait().unwrap().is_none());
    let playlist = std::fs::read_to_string(root.join("index.m3u8"))
        .expect("live HLS must publish while the device screen remains static");
    assert!(playlist.contains("#EXTINF:"));
    assert!(!playlist.contains("#EXT-X-ENDLIST"));
    drop(input);
    child.kill().await.unwrap();
    let _ = child.wait().await;
}
