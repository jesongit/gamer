use super::*;
use crate::browser::{BrowserManager, BrowserTarget, CdpSession};
use axum::{response::Html, routing::get, Router};
use futures_util::{FutureExt as _, SinkExt as _, StreamExt as _};

async fn fixture(
    root: &Path,
    url: &str,
    name: &str,
) -> (
    BrowserManager,
    Arc<CdpSession>,
    Arc<SessionShared>,
    RecordingService,
) {
    let cfg = crate::config::Config {
        data_dir: root.join("browser"),
        ffmpeg_path: std::env::var("GAMER_TEST_FFMPEG").expect("installed FFmpeg path"),
        ..Default::default()
    };
    let db = Arc::new(crate::store::Store::open(&cfg).unwrap());
    let target = BrowserTarget {
        id: format!("browser-{name}"),
        name: name.into(),
        url: format!("{url}/first"),
        profile_id: name.into(),
        width: 320,
        height: 240,
    };
    db.save_browser_target(target.clone()).unwrap();
    let manager = BrowserManager::with_controls(
        db,
        cfg.clone(),
        Arc::new(crate::core::control::ControlRegistry::default()),
    );
    let session = manager.prepare(&target.id).await.unwrap();
    session.capture_for_prepare().await.unwrap();
    let shared = crate::recording::tests::test_session(&root.join("media"), &target.id);
    {
        let mut st = shared.state.lock();
        (st.width, st.height) = session.size();
        st.browser = Some(BrowserRecording::new(cfg.ffmpeg_path));
    }
    let service = RecordingService {
        inner: shared.inner.clone(),
    };
    spawn(shared.clone(), session.clone());
    (manager, session, shared, service)
}

async fn server() -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route("/first", get(|| async {
            Html("<!doctype html><style>body{margin:0;background:#204080}</style><script>window.clicks=0;addEventListener('mousedown',()=>clicks++);</script>")
        }))
        .route("/second", get(|| async {
            Html("<!doctype html><style>body{margin:0;background:#80a020}</style><script>window.clicks=0;addEventListener('mousedown',()=>clicks++);</script>")
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, task)
}

async fn recorded_color(shared: &SessionShared, after: usize, color: [u8; 3]) -> usize {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let count = {
                let st = shared.state.lock();
                assert_eq!(
                    st.meta.state,
                    RecordingState::Recording,
                    "{:?}",
                    st.meta.error
                );
                st.browser.as_ref().unwrap().frames.len()
            };
            if count > after {
                let jpeg = shared
                    .dir
                    .join("recording")
                    .join(format!("frame-{:09}.jpg", count - 1));
                let image = image::open(jpeg).unwrap().to_rgb8();
                let pixel = image.get_pixel(30, 30).0;
                if pixel
                    .into_iter()
                    .zip(color)
                    .all(|(a, b)| a.abs_diff(b) <= 10)
                {
                    return count;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("recording must collect fresh pixels after a page change")
}

async fn terminal(service: &RecordingService, id: &RecordingId) -> RecordingSessionMeta {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let meta = service.status(id).unwrap();
            if !matches!(
                meta.state,
                RecordingState::Recording | RecordingState::Finalizing
            ) {
                return meta;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("recording must save its collected frames")
}

#[tokio::test]
#[ignore = "requires installed browser and GAMER_TEST_FFMPEG; isolated recording across navigation and reload"]
async fn recording_keeps_one_session_across_navigation_and_reload_with_strict_input_stamps() {
    let (url, server) = server().await;
    let root = tempfile::tempdir().unwrap();
    let (manager, session, shared, service) =
        fixture(root.path(), &url, "recording-navigation").await;
    let check = async {
        let first_count = recorded_color(&shared, 0, [32, 64, 128]).await;
        let (_, before) = session.capture().await.unwrap();
        let bound = session.target_id.clone();
        assert!(
            !session.is_preview_active(),
            "recording must own its stream without a viewer"
        );
        session
            .evaluate_for_test("location.href='/second'; true")
            .await;
        let second_count = recorded_color(&shared, first_count, [128, 160, 32]).await;
        assert_eq!(session.target_id, bound);
        assert_eq!(session.stamp().epoch, before.epoch);
        assert_ne!(session.stamp().revision, before.revision);
        assert!(session
            .manual_input(&json!({"type":"tap","x":30,"y":30}), &before)
            .await
            .is_err());
        assert_eq!(session.evaluate_for_test("window.clicks").await, json!(0));
        let (_, second) = session.capture().await.unwrap();
        session
            .manual_input(&json!({"type":"tap","x":30,"y":30}), &second)
            .await
            .unwrap();
        assert_eq!(session.evaluate_for_test("window.clicks").await, json!(1));

        // A same-document navigation also retires input, while the next image
        // remains part of the same recording timeline.
        session
            .evaluate_for_test(
                "location.hash='next'; document.body.style.background='#a03060'; true",
            )
            .await;
        let hash_count = recorded_color(&shared, second_count, [160, 48, 96]).await;
        assert!(session
            .manual_input(&json!({"type":"tap","x":30,"y":30}), &second)
            .await
            .is_err());
        let (_, hash) = session.capture().await.unwrap();
        session.reload(None).await.unwrap();
        recorded_color(&shared, hash_count, [128, 160, 32]).await;
        assert_eq!(session.target_id, bound);
        assert_eq!(session.stamp().epoch, before.epoch);
        assert!(session
            .manual_input(&json!({"type":"tap","x":30,"y":30}), &hash)
            .await
            .is_err());
        assert!(!session.is_preview_active());
        assert_eq!(service.active_count(), 1);
        assert_eq!(
            service.request_finish(&shared.id, false).unwrap().state,
            RecordingState::Finalizing
        );
        let meta = terminal(&service, &shared.id).await;
        assert_eq!(meta.state, RecordingState::Completed, "{:?}", meta.error);
        assert_eq!(meta.segments.len(), 1);
        assert_eq!(meta.segments[0].reason, SegmentReason::Normal);
        assert!(shared.dir.join("original.mp4").is_file());
        assert_eq!(service.active_count(), 0);
        assert!(session.is_alive());
    };
    let result = std::panic::AssertUnwindSafe(check).catch_unwind().await;
    service.shutdown().await;
    manager.shutdown().await;
    server.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[derive(Clone, Copy, Debug)]
enum Boundary {
    Disconnect,
    Rebind,
    Resize,
}

async fn resize(session: &CdpSession, service: &RecordingService, id: &RecordingId) {
    use tokio_tungstenite::tungstenite::Message;
    let page = session
        .pages()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p["id"] == session.target_id)
        .unwrap();
    let (mut socket, _) =
        tokio_tungstenite::connect_async(page["webSocketDebuggerUrl"].as_str().unwrap())
            .await
            .unwrap();
    socket.send(Message::Text(json!({"id":1,"method":"Emulation.setDeviceMetricsOverride","params":{"width":400,"height":300,"deviceScaleFactor":1,"mobile":false}}).to_string())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let Some(Ok(Message::Text(text))) = socket.next().await else {
                continue;
            };
            let reply: serde_json::Value = serde_json::from_str(&text).unwrap();
            if reply["id"] == 1 {
                assert!(reply.get("error").is_none(), "{reply}");
                break;
            }
        }
    })
    .await
    .unwrap();
    // Emulation belongs to this CDP client. Keep it attached until the
    // recorder observes the new surface and saves the original-size portion.
    terminal(service, id).await;
    socket.close(None).await.unwrap();
}

#[tokio::test]
#[ignore = "requires installed browser and GAMER_TEST_FFMPEG; isolated disconnect, page rebind and size boundaries"]
async fn recording_saves_partial_video_on_real_connection_and_size_boundaries() {
    let (url, server) = server().await;
    for boundary in [Boundary::Disconnect, Boundary::Rebind, Boundary::Resize] {
        let root = tempfile::tempdir().unwrap();
        let (manager, session, shared, service) =
            fixture(root.path(), &url, "recording-boundary").await;
        let mut rebound = None;
        let check = async {
            recorded_color(&shared, 0, [32, 64, 128]).await;
            match boundary {
                Boundary::Disconnect => session.close().await,
                Boundary::Rebind => {
                    session
                        .evaluate_for_test("addEventListener('mousedown', () => window.open('/second'), {once:true}); true")
                        .await;
                    let (_, stamp) = session.capture().await.unwrap();
                    session
                        .manual_input(&json!({"type":"tap","x":30,"y":30}), &stamp)
                        .await
                        .unwrap();
                    let page = tokio::time::timeout(Duration::from_secs(5), async {
                        loop {
                            if let Some(page) =
                                session.pages().await.unwrap().into_iter().find(|p| {
                                    p["url"]
                                        .as_str()
                                        .is_some_and(|url| url.ends_with("/second"))
                                })
                            {
                                return page;
                            }
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    })
                    .await
                    .unwrap();
                    let replacement = session.rebind(page["id"].as_str().unwrap()).await.unwrap();
                    assert_ne!(replacement.stamp().epoch, session.stamp().epoch);
                    rebound = Some(replacement);
                }
                Boundary::Resize => resize(&session, &service, &shared.id).await,
            }
            let meta = terminal(&service, &shared.id).await;
            assert_eq!(
                meta.state,
                RecordingState::Interrupted,
                "{boundary:?}: {:?}",
                meta.error
            );
            assert_eq!(meta.segments.len(), 1);
            let reason = if matches!(boundary, Boundary::Resize) {
                SegmentReason::CodecChange
            } else {
                SegmentReason::Disconnect
            };
            assert_eq!(meta.segments[0].reason, reason, "{boundary:?}");
            if matches!(boundary, Boundary::Resize) {
                let error = meta.error.unwrap();
                assert!(
                    error.contains("320×240") && error.contains("400×300"),
                    "{error}"
                );
            }
            let media: MediaMetadata =
                serde_json::from_slice(&std::fs::read(shared.dir.join("metadata.json")).unwrap())
                    .unwrap();
            assert_eq!(
                (media.width, media.height),
                (320, 240),
                "changed dimensions cannot be mixed into the saved video"
            );
            assert!(shared.dir.join("original.mp4").is_file());
            assert_eq!(service.active_count(), 0);
        };
        let result = std::panic::AssertUnwindSafe(check).catch_unwind().await;
        service.shutdown().await;
        if let Some(rebound) = rebound {
            rebound.close().await;
        }
        manager.shutdown().await;
        if let Err(panic) = result {
            server.abort();
            std::panic::resume_unwind(panic);
        }
    }
    server.abort();
}
