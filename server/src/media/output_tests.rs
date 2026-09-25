use super::*;
use crate::media::stream_mux;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::{process::Command, time::Duration};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn source_only_serves_token_and_segments_including_epoch_microseconds() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("1758000000000000.ts"), b"segment").unwrap();
    let files = SourceFiles {
        root: temp.path().into(),
        token: "secret".into(),
    };
    for (token, name, expected) in [
        ("secret", "1758000000000000.ts", 200),
        ("wrong", "1758000000000000.ts", 404),
        ("secret", "../config.toml", 404),
        ("secret", "config.toml", 404),
    ] {
        let response = source_file(State(files.clone()), Path((token.into(), name.into())))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::from_u16(expected).unwrap());
    }
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
        if n[0] & 31 == 7 || n[0] & 31 == 8 {
            if config.len() < 100 {
                config.extend_from_slice(&[0, 0, 0, 1]);
                config.extend_from_slice(n);
            }
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
    let frames = raw.chunks_exact(160 * 120 * 3).collect::<Vec<_>>();
    let mut packets = Vec::new();
    for (i, f) in frames.iter().enumerate() {
        let pts = 250_000 + i as u64 * 40_000 + if i >= 20 { 200_000 } else { 0 };
        packets.push((pts, stream_mux::raw_packet(1, pts, true, f).unwrap()));
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
