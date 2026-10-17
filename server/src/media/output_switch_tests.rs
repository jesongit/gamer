use super::*;

#[tokio::test]
#[ignore = "requires installed Chrome and FFmpeg; isolated reload and actual HLS pixels, no game or platform"]
async fn browser_reloads_preserve_encoder_and_publish_fresh_hls_pixels() {
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().route("/", get(|| async {
        Html("<!doctype html><style>html,body{margin:0;width:100%;height:100%}</style><script>window.loads=Number(sessionStorage.loads||0)+1;sessionStorage.loads=loads;document.documentElement.style.background=loads%2?'rgb(220,30,30)':'rgb(30,30,220)';</script>")
    }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let cfg = crate::config::Config {
        data_dir: directory.path().into(),
        adb_path: "unavailable-test-adb".into(),
        ..Default::default()
    };
    let db = Arc::new(crate::store::Store::open(&cfg).unwrap());
    let id = "browser-output-reload";
    db.save_browser_target(crate::browser::BrowserTarget {
        id: id.into(),
        name: "reload test".into(),
        profile_id: "output-reload".into(),
        url: address,
        width: 640,
        height: 360,
    })
    .unwrap();
    let devices = Arc::new(DeviceManager::new(db, cfg));
    let output = start(
        devices.clone(),
        OutputRequest {
            device_id: id.into(),
            video_folder: None,
            mode: "local".into(),
            push_url: String::new(),
            audio: false,
            fps: 10,
            bitrate_kbps: 500,
        },
        Some(tests::FIXED_LOCAL_URL.into()),
    )
    .await
    .unwrap();
    let session = devices.browsers.session(id).unwrap();
    let lease = devices.controls.claim(id, "reload-test").await.unwrap();
    let check = async {
        tokio::time::timeout(Duration::from_secs(15), async {
            while output.status.lock().state != "streaming" {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        let before = output.status.lock().clone();
        assert_eq!(before.source_url.as_deref(), Some(tests::FIXED_LOCAL_URL));
        for cycle in 0..4 {
            session.capture_for_prepare().await.unwrap();
            let old_stamp = session.stamp();
            devices
                .controls
                .execute(&lease, session.reload(Some(&old_stamp)))
                .await
                .unwrap();
            // A fresh document takes over the same CDP page. During the loading
            // gap the encoder must keep writing its last accepted video frame.
            for _ in 0..60 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let state = output.status.lock().clone();
                assert_eq!(state.id, before.id);
                assert_eq!(state.source_url, before.source_url);
                assert_eq!(
                    state.reconnects, 0,
                    "reload {cycle} restarted the encoder: {:?}",
                    state.error
                );
                assert!(
                    state.error.is_none(),
                    "reload {cycle} broke the output: {:?}",
                    state.error
                );
                assert_ne!(state.state, "reconnecting");
            }
            session.capture_for_prepare().await.unwrap();
            assert!(
                devices
                    .controls
                    .execute(
                        &lease,
                        session.input(
                            &serde_json::json!({"type":"tap","x":20,"y":20}),
                            Some(&old_stamp)
                        )
                    )
                    .await
                    .is_err(),
                "old input frame must remain fenced"
            );
            let count = session
                .evaluate_for_test("window.loads")
                .await
                .as_u64()
                .unwrap();
            let playlist =
                String::from_utf8(tests::local_bytes(&output, "index.m3u8").await).unwrap();
            let segment = playlist
                .lines()
                .rev()
                .find(|line| line.ends_with(".ts"))
                .unwrap();
            let segment_path = directory.path().join(format!("reload-{cycle}.ts"));
            std::fs::write(&segment_path, tests::local_bytes(&output, segment).await).unwrap();
            let mut decoder = tokio::process::Command::new("ffmpeg");
            decoder
                .args(["-v", "error", "-i"])
                .arg(&segment_path)
                .args([
                    "-frames:v",
                    "1",
                    "-pix_fmt",
                    "rgb24",
                    "-f",
                    "rawvideo",
                    "pipe:1",
                ]);
            #[cfg(windows)]
            decoder.creation_flags(0x08000000);
            let decoded = tokio::time::timeout(Duration::from_secs(10), decoder.output())
                .await
                .unwrap()
                .unwrap();
            assert!(
                decoded.status.success(),
                "{}",
                String::from_utf8_lossy(&decoded.stderr)
            );
            let offset = ((before.height / 2 * before.width + before.width / 2) * 3) as usize;
            let expected: [u8; 3] = if count % 2 == 1 {
                [220, 30, 30]
            } else {
                [30, 30, 220]
            };
            assert!(decoded.stdout.len() >= offset + 3);
            for channel in 0..3 {
                assert!(
                    decoded.stdout[offset + channel].abs_diff(expected[channel]) < 20,
                    "reload {cycle} did not reach HLS: pixel={:?}, expected={expected:?}",
                    &decoded.stdout[offset..offset + 3]
                );
            }
            assert!(output.status.lock().video_frames > before.video_frames);
        }
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    devices
        .controls
        .release(&lease, async {
            session.release_inputs().await;
            Ok(())
        })
        .await
        .unwrap();
    output.stop().await;
    devices.browsers.shutdown().await;
    server.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires installed Chrome and FFmpeg; isolated browser outputs, no live platform"]
async fn browser_outputs_switch_without_changing_url_or_claiming_input_control() {
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().route("/red", get(|| async {
        Html("<!doctype html><style>body{margin:0;background:red;height:1500px}</style><button style='position:fixed;left:10px;top:10px;width:100px;height:80px' onclick='window.hits=(window.hits||0)+1'>test</button>")
    })).route("/blue", get(|| async { Html("<!doctype html><style>body{margin:0;background:blue}</style>blue") }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let cfg = crate::config::Config {
        data_dir: directory.path().into(),
        adb_path: "unavailable-test-adb".into(),
        ..Default::default()
    };
    let db = Arc::new(crate::store::Store::open(&cfg).unwrap());
    for (id, path, width, height) in [
        ("browser-output-a", "red", 641, 481),
        ("browser-output-b", "blue", 480, 270),
    ] {
        db.save_browser_target(crate::browser::BrowserTarget {
            id: id.into(),
            name: id.into(),
            profile_id: id.into(),
            url: format!("{address}/{path}"),
            width,
            height,
        })
        .unwrap();
    }
    let devices = Arc::new(DeviceManager::new(db, cfg));
    let output = start(
        devices.clone(),
        OutputRequest {
            device_id: "browser-output-a".into(),
            video_folder: None,
            mode: "local".into(),
            push_url: String::new(),
            audio: true,
            fps: 10,
            bitrate_kbps: 500,
        },
        Some(tests::FIXED_LOCAL_URL.into()),
    )
    .await
    .unwrap();
    let check = async {
        tokio::time::timeout(Duration::from_secs(15), async {
            while output.status.lock().state != "streaming" {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        let before = output.status.lock().clone();
        let a = devices.browsers.session("browser-output-a").unwrap();
        assert!(!a.can_idle());
        assert_eq!(a.runs.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(devices.controls.status("browser-output-a").manual_allowed);
        let (_, stamp) = a.capture().await.unwrap();
        a.manual_input(&serde_json::json!({"type":"tap","x":50,"y":50}), &stamp)
            .await
            .unwrap();
        assert_eq!(
            a.evaluate_for_test("window.hits").await,
            serde_json::json!(1)
        );
        let _preview = a.start_preview_with_mode("grid9").await.unwrap();
        let _recording = a.start_recording().await.unwrap();
        a.stop_preview().await;
        let switched = output.switch_device("browser-output-b").await.unwrap();
        assert_eq!(switched.id, before.id);
        assert_eq!(switched.source_url, before.source_url);
        assert_eq!(
            (
                switched.width,
                switched.height,
                switched.fps,
                switched.bitrate_kbps
            ),
            (644, 482, 10, 500)
        );
        assert_eq!(switched.device_id, "browser-output-b");
        assert!(a.is_recording_active());
        a.stop_recording().await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while !a.can_idle() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let stable = output.switch_device("browser-output-b").await.unwrap();
        assert_eq!(stable.source_url, before.source_url);
        assert!(output.switch_device("browser-not-found").await.is_err());
        assert_eq!(output.status.lock().device_id, "browser-output-b");
        let old_count = output.status.lock().video_frames;
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            output.status.lock().video_frames > old_count,
            "static browser output must continue encoding"
        );
        let playlist = tests::local_bytes(&output, "index.m3u8").await;
        assert!(String::from_utf8(playlist).unwrap().contains("#EXTINF:"));
        assert_eq!(
            devices.controls.status("browser-output-b").phase,
            crate::core::control::ControlPhase::Idle
        );
        devices
            .browsers
            .session("browser-output-b")
            .unwrap()
            .close()
            .await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while output.status.lock().state != "reconnecting" {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let recovered = output.switch_device("browser-output-a").await.unwrap();
        assert_eq!(recovered.device_id, "browser-output-a");
        assert_eq!(recovered.source_url, output.status.lock().source_url);
        assert_eq!(recovered.id, before.id);
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    output.stop().await;
    assert!(devices
        .browsers
        .session("browser-output-a")
        .unwrap()
        .can_idle());
    devices.browsers.shutdown().await;
    server.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn different_source_sizes_fit_without_stretching_or_changing_colors() {
    let frame = source::Frame {
        width: 4,
        height: 4,
        pixels: [0, 0, 255].repeat(16).into(),
        playback: None,
    };
    let fitted = source::fit(&frame, 8, 4).unwrap();
    assert_eq!(fitted.len(), 8 * 4 * 3);
    assert_eq!(&fitted[..3], &[0, 0, 0]);
    assert_eq!(&fitted[2 * 3..3 * 3], &[0, 0, 255]);
    assert_eq!(&fitted[7 * 3..8 * 3], &[0, 0, 0]);
}

#[tokio::test]
#[ignore = "requires local FFmpeg; continuous encoder/source switch and cancellation, no device or public broadcast"]
async fn live_source_switch_keeps_encoder_settings_and_playlist_continuity() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    let request = OutputRequest {
        device_id: "a".into(),
        video_folder: None,
        mode: "local".into(),
        push_url: String::new(),
        audio: false,
        fps: 10,
        bitrate_kbps: 500,
    };
    let status = Arc::new(Mutex::new(OutputStatus {
        id: "existing-output".into(),
        device_id: "a".into(),
        mode: "local".into(),
        source_url: Some("http://127.0.0.1/unchanged/index.m3u8".into()),
        fps: 10,
        bitrate_kbps: 500,
        width: 160,
        height: 120,
        ..Default::default()
    }));
    let (stop, mut stopped) = watch::channel(false);
    let (switches, mut receiver) = mpsc::channel(4);
    let initial = source::Source::fixture("a", 160, 120, [0, 0, 255]);
    let target_root = root.clone();
    let report = status.clone();
    let worker = tokio::spawn(async move {
        pump(
            "ffmpeg",
            &request,
            &target_root,
            &report,
            &mut stopped,
            &mut receiver,
            initial,
            None,
        )
        .await
    });
    let check = async {
        tokio::time::timeout(Duration::from_secs(12), async {
            while status.lock().state != "streaming" || status.lock().video_frames < 25 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        let before = status.lock().clone();
        let (reply, result) = oneshot::channel();
        switches
            .send(SwitchCommand {
                source: source::Source::fixture("b", 320, 90, [255, 0, 0]),
                reply,
            })
            .await
            .unwrap();
        let after = tokio::time::timeout(Duration::from_secs(5), result)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(after.device_id, "b");
        assert_eq!(after.id, before.id);
        assert_eq!(after.source_url, before.source_url);
        assert_eq!(
            (after.width, after.height, after.fps, after.bitrate_kbps),
            (160, 120, 10, 500)
        );
        let (reply, cancelled) = oneshot::channel();
        drop(cancelled);
        switches
            .send(SwitchCommand {
                source: source::Source::fixture("cancelled", 80, 80, [0, 255, 0]),
                reply,
            })
            .await
            .unwrap();
        let unavailable = source::Source::fixture("unavailable", 160, 120, [0, 255, 0]);
        unavailable.fail_for_test();
        let (reply, result) = oneshot::channel();
        switches
            .send(SwitchCommand {
                source: unavailable,
                reply,
            })
            .await
            .unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(5), result)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        tokio::time::timeout(Duration::from_secs(10), async {
            while status.lock().video_frames < after.video_frames + 30 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(status.lock().device_id, "b");
        let playlist = std::fs::read_to_string(root.join("index.m3u8")).unwrap();
        let segment = playlist
            .lines()
            .rev()
            .find(|line| line.ends_with(".ts"))
            .unwrap();
        let mut decode = std::process::Command::new("ffmpeg");
        decode
            .args(["-v", "error", "-i"])
            .arg(root.join(segment))
            .args([
                "-frames:v",
                "1",
                "-pix_fmt",
                "rgb24",
                "-f",
                "rawvideo",
                "pipe:1",
            ]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            decode.creation_flags(0x08000000);
        }
        let output = decode.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout.len(), 160 * 120 * 3);
        let center = &output.stdout[(60 * 160 + 80) * 3..(60 * 160 + 80) * 3 + 3];
        assert!(
            center[2] > 200 && center[0] < 40 && center[1] < 40,
            "new source must be blue: {center:?}"
        );
        assert!(
            output.stdout[5 * 160 * 3..5 * 160 * 3 + 3]
                .iter()
                .all(|value| *value < 15),
            "letterboxing must stay black"
        );
    };
    let outcome = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    let _ = stop.send(true);
    let result = tokio::time::timeout(Duration::from_secs(12), worker)
        .await
        .unwrap()
        .unwrap();
    result.unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
