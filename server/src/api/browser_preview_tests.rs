use super::*;

#[test]
fn display_ack_wakes_an_overdue_frame_without_losing_another_pacing_period() {
    let start = tokio::time::Instant::now();
    let interval = Duration::from_nanos(1_000_000_000 / 30);
    let mut delivery = PreviewDelivery::default();
    delivery.frame_started(start);
    delivery.sent(1, start + Duration::from_millis(14));
    assert!(delivery.acknowledge(Some(1), true));
    let ack_at = start + Duration::from_millis(35);
    assert_eq!(delivery.next_frame_at(ack_at, interval), ack_at);

    // A fast decoder still cannot exceed the configured frame rate.
    delivery.frame_started(ack_at);
    delivery.sent(2, ack_at + Duration::from_millis(1));
    assert!(delivery.acknowledge(Some(2), true));
    assert_eq!(
        delivery.next_frame_at(ack_at + Duration::from_millis(2), interval),
        ack_at + interval,
    );
    delivery.sent(3, ack_at + interval);
    assert!(!delivery.acknowledge(Some(2), true));
    assert_eq!(
        delivery.readiness(ack_at + interval),
        DisplayReadiness::Waiting
    );
}

#[test]
fn missing_display_ack_retires_only_the_current_frame() {
    let now = tokio::time::Instant::now();
    let mut delivery = PreviewDelivery::default();
    delivery.sent(1, now);
    assert_eq!(delivery.readiness(now), DisplayReadiness::Waiting);
    assert_eq!(
        delivery.readiness(now + DISPLAY_ACK_TIMEOUT),
        DisplayReadiness::Retry
    );
    assert_eq!(delivery.readiness(now), DisplayReadiness::Ready);
    delivery.sent(2, now);
    assert!(!delivery.acknowledge(Some(1), true));
    assert_eq!(delivery.readiness(now), DisplayReadiness::Waiting);
    assert!(delivery.acknowledge(Some(2), true));
    assert_eq!(delivery.readiness(now), DisplayReadiness::Ready);
}

#[test]
fn non_displaying_viewer_has_a_bounded_recovery_budget() {
    let now = tokio::time::Instant::now();
    let mut delivery = PreviewDelivery::default();
    for id in 1..=u64::from(MAX_DISPLAY_RECOVERIES) {
        delivery.sent(id, now);
        assert_eq!(
            delivery.readiness(now + DISPLAY_ACK_TIMEOUT),
            DisplayReadiness::Retry
        );
    }
    delivery.sent(100, now);
    assert_eq!(
        delivery.readiness(now + DISPLAY_ACK_TIMEOUT),
        DisplayReadiness::Unresponsive
    );
}

#[test]
fn failed_decode_retries_but_a_late_ack_cannot_reset_its_budget() {
    let now = tokio::time::Instant::now();
    let mut delivery = PreviewDelivery::default();
    delivery.sent(1, now);
    assert!(delivery.acknowledge(Some(1), false));
    assert_eq!(delivery.recoveries, 1);
    delivery.sent(2, now);
    assert!(!delivery.acknowledge(Some(1), true));
    assert_eq!(delivery.recoveries, 1);
    assert!(!delivery.acknowledge(None, false));
    assert_eq!(delivery.readiness(now), DisplayReadiness::Waiting);
    assert!(delivery.acknowledge(Some(2), true));
    assert_eq!(delivery.recoveries, 0);
}

#[test]
fn repeated_decoder_errors_cannot_spin_an_unbounded_preview() {
    let now = tokio::time::Instant::now();
    let mut delivery = PreviewDelivery::default();
    for id in 1..=u64::from(MAX_DISPLAY_RECOVERIES) + 1 {
        delivery.sent(id, now);
        assert!(delivery.acknowledge(Some(id), false));
    }
    assert_eq!(delivery.readiness(now), DisplayReadiness::Unresponsive);
}

#[tokio::test]
#[ignore = "requires installed browser; isolated rename and slow refresh on one viewer"]
async fn browser_rename_and_slow_reload_keep_the_same_preview_connection() {
    use axum::{body::Body, http::Response, routing::get, Router};
    use futures_util::{SinkExt, StreamExt};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_tungstenite::tungstenite::Message as ClientMessage;

    let loads = Arc::new(AtomicUsize::new(0));
    let finish_loading = Arc::new(tokio::sync::Semaphore::new(0));
    let route_loads = loads.clone();
    let route_finish = finish_loading.clone();
    let dom = Router::new().route("/", get(move || {
        let number = route_loads.fetch_add(1, Ordering::SeqCst);
        let finish = route_finish.clone();
        async move {
            let chunks = futures_util::stream::unfold(0, move |part| {
                let finish = finish.clone();
                async move {
                    let chunk = match part {
                        0 => format!("<!doctype html><style>body{{margin:0;background:#{}}}</style><body>", if number == 0 { "234" } else { "456" }),
                        1 => {
                            if number > 0 { finish.acquire().await.unwrap().forget(); }
                            format!("<b id='loaded'>{number}</b></body>")
                        }
                        _ => return None,
                    };
                    Some((Ok::<_, std::convert::Infallible>(chunk), part + 1))
                }
            });
            Response::builder().header("Content-Type", "text/html").body(Body::from_stream(chunks)).unwrap()
        }
    }));
    let dom_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", dom_listener.local_addr().unwrap());
    let dom_task = tokio::spawn(async move { axum::serve(dom_listener, dom).await.unwrap() });
    let temp = tempfile::tempdir().unwrap();
    let st = preview_test_state(crate::config::Config {
        data_dir: temp.path().into(),
        ..Default::default()
    });
    let target = BrowserTarget {
        id: "browser-refresh-fixture".into(),
        name: "before".into(),
        url,
        profile_id: "refresh-fixture".into(),
        width: 640,
        height: 480,
    };
    st.db.save_browser_target(target.clone()).unwrap();
    let session = st.devices.browsers.prepare(&target.id).await.unwrap();
    session.capture_for_prepare().await.unwrap();
    let bound = session.target_id.clone();
    let ws_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!("ws://{}/preview", ws_listener.local_addr().unwrap());
    let session_route = session.clone();
    let state_route = st.clone();
    let app = Router::new().route(
        "/preview",
        get(move |ws: WebSocketUpgrade| {
            let session = session_route.clone();
            let state = state_route.clone();
            async move {
                let lease = session.viewer.clone().lock_owned().await;
                ws.on_upgrade(move |socket| async move {
                    let _lease = lease;
                    serve(socket, session, state, "grid9".into(), None).await;
                })
            }
        }),
    );
    let ws_task = tokio::spawn(async move { axum::serve(ws_listener, app).await.unwrap() });
    let check = async {
        let (mut socket, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
        let first = next_preview_frame(&mut socket).await;
        socket
            .send(ClientMessage::Text(
                json!({"type":"frame_ack","id":first["id"]}).to_string(),
            ))
            .await
            .unwrap();
        let before = session.stamp();
        let run_lease = st.devices.browsers.acquire(&target.id).unwrap();
        let mut renamed = target.clone();
        renamed.name = "after".into();
        // Exercise the actual save handler as well as the manager while a
        // viewer and browser activity lease are present.
        let _ = save(State(st.clone()), Json(renamed.clone()))
            .await
            .unwrap();
        assert_eq!(st.devices.browsers.get(&target.id).unwrap().name, "after");
        assert!(Arc::ptr_eq(
            &session,
            &st.devices.browsers.session(&target.id).unwrap()
        ));
        assert_eq!(session.stamp(), before);
        renamed.width += 1;
        assert!(save(State(st.clone()), Json(renamed)).await.is_err());
        drop(run_lease);

        session.reload(None).await.unwrap();
        // Keep the new document in readyState=loading longer than capture's
        // old 20 second deadline. The viewer must remain responsive throughout.
        let until = tokio::time::Instant::now() + Duration::from_secs(22);
        while tokio::time::Instant::now() < until {
            match tokio::time::timeout(Duration::from_millis(500), socket.next()).await {
                Ok(Some(Ok(ClientMessage::Text(text)))) => {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    assert_ne!(value["type"], "error", "{value}");
                    if value["type"] == "frame" {
                        socket
                            .send(ClientMessage::Text(
                                json!({"type":"frame_ack","id":value["id"]}).to_string(),
                            ))
                            .await
                            .unwrap();
                    }
                }
                Err(_) => {}
                other => panic!("refresh closed the original viewer: {other:?}"),
            }
        }
        finish_loading.add_permits(1);
        let final_frame = next_preview_frame(&mut socket).await;
        assert_eq!(final_frame["stamp"]["epoch"], first["stamp"]["epoch"]);
        assert!(final_frame["stamp"]["revision"].as_u64().unwrap() > before.revision);
        assert_eq!(session.target_id, bound);
        assert!(session.is_alive());
        assert!(session.validate(&before).is_err());
        socket.close(None).await.unwrap();
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    finish_loading.add_permits(1);
    st.devices.browsers.shutdown().await;
    ws_task.abort();
    dom_task.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires installed browser; isolated transient preview restart failure"]
async fn browser_preview_retries_a_failed_navigation_restart_on_the_same_connection() {
    use axum::{response::Html, routing::get, Router};
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as ClientMessage;

    let dom_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", dom_listener.local_addr().unwrap());
    let dom = Router::new().route("/", get(|| async {
        Html(r#"<!doctype html><style>body{margin:0;background:#234}button{position:absolute;left:20px;top:20px;width:160px;height:80px}</style><button onclick="window.hits=(window.hits||0)+1;this.textContent=window.hits">hit</button>"#)
    }));
    let dom_task = tokio::spawn(async move { axum::serve(dom_listener, dom).await.unwrap() });
    let temp = tempfile::tempdir().unwrap();
    let st = preview_test_state(crate::config::Config {
        data_dir: temp.path().into(),
        ..Default::default()
    });
    let target = BrowserTarget {
        id: "browser-restart-failure-fixture".into(),
        name: "isolated restart failure".into(),
        url,
        profile_id: "restart-failure-fixture".into(),
        width: 640,
        height: 480,
    };
    st.db.save_browser_target(target.clone()).unwrap();
    let session = st.devices.browsers.prepare(&target.id).await.unwrap();
    session.capture_for_prepare().await.unwrap();
    let bound = session.target_id.clone();
    let ws_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!("ws://{}/preview", ws_listener.local_addr().unwrap());
    let session_route = session.clone();
    let state_route = st.clone();
    let app = Router::new().route(
        "/preview",
        get(move |ws: WebSocketUpgrade| {
            let session = session_route.clone();
            let state = state_route.clone();
            async move {
                let lease = session.viewer.clone().lock_owned().await;
                ws.on_upgrade(move |socket| async move {
                    let _lease = lease;
                    serve(socket, session, state, "grid9".into(), None).await;
                })
            }
        }),
    );
    let ws_task = tokio::spawn(async move { axum::serve(ws_listener, app).await.unwrap() });
    let check = async {
        let (mut socket, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
        let first = next_preview_frame(&mut socket).await;
        let old_stamp: FrameStamp = serde_json::from_value(first["stamp"].clone()).unwrap();
        socket
            .send(ClientMessage::Text(
                json!({"type":"frame_ack","id":first["id"]}).to_string(),
            ))
            .await
            .unwrap();

        // A static grid skips its first native compositor frame and bootstraps
        // through Runtime.evaluate. Keep that read temporarily invalid without
        // navigating away or killing Chrome: the actual start_preview call must
        // fail once, rather than this test merely exercising another reload.
        assert_eq!(
            session
                .evaluate_for_test(
                    r#"(() => {
            const width = window.innerWidth;
            window.previewFaultReads = 0;
            window.allowPreviewRecovery = false;
            Object.defineProperty(window, 'innerWidth', {configurable:true, get() {
                window.previewFaultReads++;
                return window.allowPreviewRecovery ? width : NaN;
            }});
            history.pushState({}, '', '?loggedOut=1');
            return true;
        })()"#
                )
                .await,
            json!(true)
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if session
                    .evaluate_for_test("window.previewFaultReads > 0")
                    .await
                    == json!(true)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("the restart must encounter the injected viewport capture failure");
        assert!(session.is_alive());
        assert!(session.validate(&old_stamp).is_err());

        // The connection still handles messages during deferred recovery. It
        // must reject old pixels even though the explicitly bound page survives.
        socket
            .send(ClientMessage::Text(
                json!({"type":"tap","x":80,"y":50,"stamp":old_stamp}).to_string(),
            ))
            .await
            .unwrap();
        loop {
            let message = next_preview_message(&mut socket).await;
            if message["source"] == "input" {
                assert!(message["error"]
                    .as_str()
                    .unwrap()
                    .contains("画面或目标已变化"));
                break;
            }
            assert_eq!(message["type"], "invalidated", "{message}");
        }
        assert_eq!(
            session.evaluate_for_test("window.hits || 0").await,
            json!(0)
        );
        assert_eq!(
            session
                .evaluate_for_test("window.allowPreviewRecovery = true")
                .await,
            json!(true)
        );

        let recovered = next_preview_frame(&mut socket).await;
        assert_eq!(recovered["stamp"]["epoch"], first["stamp"]["epoch"]);
        assert!(recovered["stamp"]["revision"].as_u64().unwrap() > old_stamp.revision);
        assert!(
            session
                .evaluate_for_test("window.previewFaultReads")
                .await
                .as_u64()
                .unwrap()
                >= 2,
            "a failed restart must retry its actual viewport capture"
        );
        assert_eq!(session.target_id, bound);
        socket
            .send(ClientMessage::Text(
                json!({"type":"frame_ack","id":recovered["id"]}).to_string(),
            ))
            .await
            .unwrap();
        socket
            .send(ClientMessage::Text(
                json!({"type":"tap","x":80,"y":50,"stamp":recovered["stamp"]}).to_string(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if session.evaluate_for_test("window.hits || 0").await == json!(1) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("freshly displayed pixels must authorize input on the same socket");

        session.close().await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match socket.next().await {
                    None | Some(Err(_)) | Some(Ok(ClientMessage::Close(_))) => break,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await
        .expect("a real target disconnect must still end the preview connection");
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    st.devices.browsers.shutdown().await;
    ws_task.abort();
    dom_task.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

// This starts an isolated local animated DOM and a temporary Chrome profile.
// It exercises the production preview loop, not a cloud game or real account.
#[tokio::test]
#[ignore = "requires installed browser; local WebSocket preview/ACK/control integration"]
async fn dynamic_preview_recovers_missing_ack_and_survives_input_errors_and_message_flood() {
    use axum::{response::Html, routing::get, Router};
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as ClientMessage;

    let dom_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_url = format!("http://{}/", dom_listener.local_addr().unwrap());
    let dom = Router::new().route("/", get(|| async {
        Html(r#"<!doctype html><style>body{margin:0;color:white;font:48px sans-serif}#box{position:absolute;width:80px;height:80px;background:#fd0}button{position:absolute;z-index:5;left:540px;top:400px;width:90px;height:70px}</style><b id="count"></b><i id="box"></i><button onclick="window.freeze=true">freeze</button><script>let n=0;function animate(){if(window.freeze)return;n++;count.textContent=n;box.style.left=(n*5%540)+'px';box.style.top=(n*3%380)+'px';document.body.style.background=n%60<30?'#174':'#247';requestAnimationFrame(animate)}animate()</script>"#)
    }));
    let dom_task = tokio::spawn(async move { axum::serve(dom_listener, dom).await.unwrap() });
    let temp = tempfile::tempdir().unwrap();
    let cfg = crate::config::Config {
        data_dir: temp.path().into(),
        ..Default::default()
    };
    let st = preview_test_state(cfg);
    let target = BrowserTarget {
        id: "browser-preview-recovery-test".into(),
        name: "isolated preview".into(),
        url: target_url,
        profile_id: "preview-recovery-test".into(),
        width: 640,
        height: 480,
    };
    st.db.save_browser_target(target.clone()).unwrap();
    let session = st.devices.browsers.prepare(&target.id).await.unwrap();
    let ws_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!("ws://{}/preview", ws_listener.local_addr().unwrap());
    let session_route = session.clone();
    let state_route = st.clone();
    let app = Router::new().route(
        "/preview",
        get(move |ws: WebSocketUpgrade| {
            let session = session_route.clone();
            let state = state_route.clone();
            async move {
                let lease = session.viewer.clone().lock_owned().await;
                ws.on_upgrade(move |socket| async move {
                    let _lease = lease;
                    serve(socket, session, state, "single".into(), None).await;
                })
            }
        }),
    );
    let ws_task = tokio::spawn(async move { axum::serve(ws_listener, app).await.unwrap() });
    let check = async {
        let (mut socket, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
        let first = next_preview_frame(&mut socket).await;
        socket
            .send(ClientMessage::Text(
                json!({"type":"tap","x":640,"y":0,"stamp":first["stamp"]}).to_string(),
            ))
            .await
            .unwrap();
        let rejected = next_preview_message(&mut socket).await;
        assert_eq!(rejected["source"], "input");
        assert!(rejected["error"].as_str().unwrap().contains("坐标超出画面"));
        // Deliberately omit the first ACK: a reset and newer image must arrive
        // on the same connection instead of requiring a viewer reconnect.
        let second = next_preview_frame(&mut socket).await;
        assert_ne!(second["id"], first["id"]);
        assert_ne!(second["jpeg"], first["jpeg"]);
        socket
            .send(ClientMessage::Text(
                json!({"type":"frame_ack","id":first["id"]}).to_string(),
            ))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(150), socket.next())
                .await
                .is_err(),
            "a retired ACK must not release the current frame"
        );
        socket
            .send(ClientMessage::Text(
                json!({"type":"frame_ack","id":second["id"],"displayed":false}).to_string(),
            ))
            .await
            .unwrap();
        let third = next_preview_frame(&mut socket).await;
        assert_ne!(third["id"], second["id"]);
        socket
            .send(ClientMessage::Text(
                json!({"type":"frame_ack","id":third["id"]}).to_string(),
            ))
            .await
            .unwrap();

        let (writer, mut reader) = socket.split();
        let writer = Arc::new(tokio::sync::Mutex::new(writer));
        let flood_writer = writer.clone();
        let flood = tokio::spawn(async move {
            let until = tokio::time::Instant::now() + Duration::from_secs(2);
            while tokio::time::Instant::now() < until {
                {
                    let mut writer = flood_writer.lock().await;
                    for _ in 0..20 {
                        writer
                            .send(ClientMessage::Text(
                                "{\"type\":\"frame_ack\",\"id\":0}".into(),
                            ))
                            .await
                            .unwrap();
                    }
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        let until = tokio::time::Instant::now() + Duration::from_secs(2);
        let mut delivered = 0;
        while tokio::time::Instant::now() < until {
            let next = tokio::time::timeout(Duration::from_millis(700), reader.next())
                .await
                .expect("control traffic must not starve dynamic previews")
                .unwrap()
                .unwrap();
            let ClientMessage::Text(text) = next else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] == "frame" {
                delivered += 1;
                writer
                    .lock()
                    .await
                    .send(ClientMessage::Text(
                        json!({"type":"frame_ack","id":value["id"]}).to_string(),
                    ))
                    .await
                    .unwrap();
            }
        }
        flood.await.unwrap();
        eprintln!("local dynamic preview delivered {delivered} frames during 2 seconds of continuous stale ACK traffic after recovery");
        assert!(
            delivered >= 10,
            "received only {delivered} frames under sustained traffic"
        );

        // Decode/paint acknowledgements often miss one 33 ms tick. They must
        // wake an overdue frame instead of quantizing a 45 ms display into
        // 66 ms delivery (15 FPS), without admitting another in-flight image.
        let until = tokio::time::Instant::now() + Duration::from_secs(2);
        let mut delayed_ack_frames = 0;
        while tokio::time::Instant::now() < until {
            let message = tokio::time::timeout(Duration::from_millis(700), reader.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let ClientMessage::Text(text) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] != "frame" {
                continue;
            }
            delayed_ack_frames += 1;
            tokio::time::sleep(Duration::from_millis(45)).await;
            writer
                .lock()
                .await
                .send(ClientMessage::Text(
                    json!({"type":"frame_ack","id":value["id"]}).to_string(),
                ))
                .await
                .unwrap();
        }
        eprintln!("local dynamic preview delivered {delayed_ack_frames} frames with 45 ms display acknowledgements in 2 seconds");
        assert!(
            delayed_ack_frames >= 36,
            "display pacing dropped to a second timer period: {delayed_ack_frames} frames"
        );

        session
            .manual_input(&json!({"type":"tap","x":560,"y":430}), &session.stamp())
            .await
            .unwrap();
        while let Ok(Some(Ok(ClientMessage::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(150), reader.next()).await
        {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] == "frame" {
                writer
                    .lock()
                    .await
                    .send(ClientMessage::Text(
                        json!({"type":"frame_ack","id":value["id"]}).to_string(),
                    ))
                    .await
                    .unwrap();
            }
        }
        writer
            .lock()
            .await
            .send(ClientMessage::Text(
                "{\"type\":\"preview\",\"mode\":\"grid4\"}".into(),
            ))
            .await
            .unwrap();
        let static_first = next_preview_frame(&mut reader).await;
        let static_retry = next_preview_frame(&mut reader).await;
        assert_ne!(static_first["id"], static_retry["id"]);
        assert_eq!(
            static_first["jpeg"], static_retry["jpeg"],
            "a static page must retry its valid retained image without restarting Chrome"
        );
        writer
            .lock()
            .await
            .send(ClientMessage::Text(
                json!({"type":"frame_ack","id":static_retry["id"]}).to_string(),
            ))
            .await
            .unwrap();
        writer.lock().await.close().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while session.is_preview_active() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            session.is_alive(),
            "preview recovery must keep the browser process/account alive"
        );
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    st.devices.browsers.shutdown().await;
    ws_task.abort();
    dom_task.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires installed browser; isolated popup and bound-page preview recovery"]
async fn single_preview_recovers_when_a_popup_hides_its_bound_page() {
    use axum::{response::Html, routing::get, Router};
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as ClientMessage;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let dom = Router::new()
        .route("/", get(|| async {
            Html(r#"<!doctype html><style>body{margin:0;background:#234}button{position:absolute;top:20px;width:160px;height:80px}#popup{left:20px}#hit{left:240px}</style><button id="popup" onclick="window.open('/popup')">popup</button><button id="hit" onclick="window.hits=(window.hits||0)+1;this.textContent=window.hits">hit</button>"#)
        }))
        .route("/popup", get(|| async { Html("<!doctype html><b>popup page</b>") }));
    let dom_task = tokio::spawn(async move { axum::serve(listener, dom).await.unwrap() });
    let temp = tempfile::tempdir().unwrap();
    let st = preview_test_state(crate::config::Config {
        data_dir: temp.path().into(),
        ..Default::default()
    });
    let target = BrowserTarget {
        id: "browser-popup-recovery-test".into(),
        name: "isolated popup recovery".into(),
        url,
        profile_id: "popup-recovery-test".into(),
        width: 640,
        height: 480,
    };
    st.db.save_browser_target(target.clone()).unwrap();
    let session = st.devices.browsers.prepare(&target.id).await.unwrap();
    let bound = session.target_id.clone();
    let ws_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!("ws://{}/preview", ws_listener.local_addr().unwrap());
    let session_route = session.clone();
    let state_route = st.clone();
    let app = Router::new().route(
        "/preview",
        get(move |ws: WebSocketUpgrade| {
            let session = session_route.clone();
            let state = state_route.clone();
            async move {
                let lease = session.viewer.clone().lock_owned().await;
                ws.on_upgrade(move |socket| async move {
                    let _lease = lease;
                    serve(socket, session, state, "single".into(), None).await;
                })
            }
        }),
    );
    let ws_task = tokio::spawn(async move { axum::serve(ws_listener, app).await.unwrap() });
    let check = async {
        let (mut socket, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
        let first = next_preview_frame(&mut socket).await;
        let old_stamp: FrameStamp = serde_json::from_value(first["stamp"].clone()).unwrap();
        socket
            .send(ClientMessage::Text(
                json!({"type":"frame_ack","id":first["id"]}).to_string(),
            ))
            .await
            .unwrap();
        session
            .manual_input(&json!({"type":"tap","x":80,"y":50}), &old_stamp)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let pages = session.pages().await.unwrap();
                if pages.len() == 2
                    && session.evaluate_for_test("document.visibilityState").await
                        == json!("visible")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("a popup must not leave the bound page hidden and frozen");
        assert_eq!(
            session.target_id, bound,
            "recovery must keep the explicit binding"
        );
        assert!(
            session.validate(&old_stamp).is_err(),
            "hidden-page pixels must not authorize input"
        );
        let recovered = loop {
            let frame = next_preview_frame(&mut socket).await;
            socket
                .send(ClientMessage::Text(
                    json!({"type":"frame_ack","id":frame["id"]}).to_string(),
                ))
                .await
                .unwrap();
            if frame["stamp"]["revision"].as_u64().unwrap() > old_stamp.revision {
                break frame;
            }
        };
        let stamp = serde_json::from_value(recovered["stamp"].clone()).unwrap();
        session
            .manual_input(&json!({"type":"tap","x":300,"y":50}), &stamp)
            .await
            .unwrap();
        assert_eq!(session.evaluate_for_test("window.hits").await, json!(1));
        assert_eq!(
            session.pages().await.unwrap().len(),
            2,
            "the popup remains available for explicit binding"
        );
        socket.close(None).await.unwrap();
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    st.devices.browsers.shutdown().await;
    ws_task.abort();
    dom_task.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

async fn next_preview_message<S>(socket: &mut S) -> Value
where
    S: futures_util::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    use futures_util::StreamExt;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            if let tokio_tungstenite::tungstenite::Message::Text(text) = message {
                return serde_json::from_str(&text).unwrap();
            }
        }
    })
    .await
    .expect("preview must respond within its bounded ACK recovery window")
}

#[tokio::test]
#[ignore = "requires installed browser; input queue/preview/control fencing integration"]
async fn browser_preview_keeps_frames_during_input_wait_and_rejects_queued_old_tickets() {
    use axum::{response::Html, routing::get, Router};
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as ClientMessage;

    let dom_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", dom_listener.local_addr().unwrap());
    let dom_task = tokio::spawn(async move {
        axum::serve(dom_listener, Router::new().route("/", get(|| async {
            Html("<!doctype html><body style='margin:0;height:100vh;background:#245;color:white;font:40px sans-serif' onclick='window.clicks.push(event.clientX)'><p id='clock'></p><script>window.clicks=[];function tick(t){clock.textContent=t;requestAnimationFrame(tick)}requestAnimationFrame(tick)</script>")
        }))).await.unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let st = preview_test_state(crate::config::Config {
        data_dir: temp.path().into(),
        ..Default::default()
    });
    let target = BrowserTarget {
        id: "browser-input-pacing".into(),
        name: "input pacing fixture".into(),
        url,
        profile_id: "input-pacing".into(),
        width: 640,
        height: 480,
    };
    st.db.save_browser_target(target.clone()).unwrap();
    let session = st.devices.browsers.prepare(&target.id).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!("ws://{}/preview", listener.local_addr().unwrap());
    let session_route = session.clone();
    let state_route = st.clone();
    let ws_task = tokio::spawn(async move {
        let app = Router::new().route(
            "/preview",
            get(move |ws: WebSocketUpgrade| {
                let session = session_route.clone();
                let state = state_route.clone();
                async move {
                    let lease = session.viewer.clone().lock_owned().await;
                    ws.on_upgrade(move |socket| async move {
                        let _lease = lease;
                        serve(socket, session, state, "single".into(), None).await;
                    })
                }
            }),
        );
        axum::serve(listener, app).await.unwrap();
    });
    let check = async {
        let (mut socket, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
        let first = next_preview_frame(&mut socket).await;
        socket
            .send(ClientMessage::Text(
                json!({"type":"frame_ack","id":first["id"]}).to_string(),
            ))
            .await
            .unwrap();
        let permit = st.devices.controls.admit_manual(&session.id).await.unwrap();
        for x in [10, 20] {
            socket
                .send(ClientMessage::Text(
                    json!({"type":"tap","x":x,"y":200,"stamp":first["stamp"]}).to_string(),
                ))
                .await
                .unwrap();
        }
        let until = tokio::time::Instant::now() + Duration::from_millis(800);
        let mut frames = 0;
        while tokio::time::Instant::now() < until {
            let value = next_preview_frame(&mut socket).await;
            frames += 1;
            socket
                .send(ClientMessage::Text(
                    json!({"type":"frame_ack","id":value["id"]}).to_string(),
                ))
                .await
                .unwrap();
        }
        assert!(
            frames >= 10,
            "input admission blocked the preview: {frames}"
        );
        assert_eq!(session.evaluate_for_test("window.clicks").await, json!([]));
        drop(permit);
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if session.evaluate_for_test("window.clicks.length").await == json!(2) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            session.evaluate_for_test("window.clicks").await,
            json!([10, 20])
        );

        let permit = st.devices.controls.admit_manual(&session.id).await.unwrap();
        let stamp = session.stamp();
        for x in [30, 40] {
            socket
                .send(ClientMessage::Text(
                    json!({"type":"tap","x":x,"y":200,"stamp":stamp}).to_string(),
                ))
                .await
                .unwrap();
        }
        // A delivered/ACKed frame proves the loop has consumed the input
        // messages without waiting on the still-held manual admission gate.
        for _ in 0..4 {
            let value = next_preview_frame(&mut socket).await;
            socket
                .send(ClientMessage::Text(
                    json!({"type":"frame_ack","id":value["id"]}).to_string(),
                ))
                .await
                .unwrap();
        }
        let controls = st.devices.controls.clone();
        let id = session.id.clone();
        let claim =
            tokio::spawn(async move { controls.claim(&id, "input-fence-test").await.unwrap() });
        tokio::time::timeout(Duration::from_secs(1), async {
            while st.devices.controls.status(&session.id).owner.is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(permit);
        let owner = claim.await.unwrap();
        let mut rejections = 0;
        while rejections < 2 {
            let value = next_preview_message(&mut socket).await;
            if value["source"] == "input" {
                rejections += 1;
            }
            if value["type"] == "frame" {
                socket
                    .send(ClientMessage::Text(
                        json!({"type":"frame_ack","id":value["id"]}).to_string(),
                    ))
                    .await
                    .unwrap();
            }
        }
        assert_eq!(
            session.evaluate_for_test("window.clicks").await,
            json!([10, 20])
        );
        st.devices
            .controls
            .release(&owner, async { Ok(()) })
            .await
            .unwrap();

        // Overflow cannot silently drop an UP and leave held inputs behind;
        // no queued or not-yet-admitted operation may replay after close.
        let permit = st.devices.controls.admit_manual(&session.id).await.unwrap();
        let stamp = session.stamp();
        for _ in 0..MAX_PENDING_INPUTS + 2 {
            socket
                .send(ClientMessage::Text(
                    json!({"type":"tap","x":50,"y":200,"stamp":stamp}).to_string(),
                ))
                .await
                .unwrap();
        }
        let mut overloaded = false;
        tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(Ok(message)) = socket.next().await {
                if let ClientMessage::Text(text) = message {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["error"]
                        .as_str()
                        .is_some_and(|error| error.contains("输入积压"))
                    {
                        overloaded = true;
                        break;
                    }
                }
            }
        })
        .await
        .unwrap();
        assert!(overloaded);
        drop(permit);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(lease) = session.viewer.try_lock() {
                    drop(lease);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            session.evaluate_for_test("window.clicks").await,
            json!([10, 20])
        );
        assert!(session.is_alive());
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    st.devices.browsers.shutdown().await;
    ws_task.abort();
    dom_task.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

async fn next_preview_frame<S>(socket: &mut S) -> Value
where
    S: futures_util::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    loop {
        let message = next_preview_message(socket).await;
        if message["type"] == "frame" {
            return message;
        }
        assert_ne!(
            message["type"], "error",
            "unexpected preview error: {message}"
        );
    }
}

fn preview_test_state(cfg: crate::config::Config) -> AppState {
    struct NoRuns;
    impl crate::run_manager::RunExecutor for NoRuns {
        fn prepare<'a>(
            &'a self,
            _: &'a crate::core::RunContext,
            _: &'a crate::core::RunRequest,
        ) -> futures_util::future::BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async { unreachable!("preview fixture starts no runs") })
        }
        fn execute<'a>(
            &'a self,
            _: &'a crate::core::RunContext,
            _: &'a crate::core::RunRequest,
            _: bool,
            _: Arc<std::sync::atomic::AtomicBool>,
        ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
            Box::pin(async { unreachable!("preview fixture starts no runs") })
        }
        fn acquire(
            &self,
            _: &crate::core::RunContext,
        ) -> anyhow::Result<Box<dyn crate::core::ActivityLease>> {
            unreachable!("preview fixture starts no runs")
        }
    }
    let db = Arc::new(crate::store::Store::open(&cfg).unwrap());
    let packages = Arc::new(crate::resources::PackageStore::open(&cfg).unwrap());
    let devices = Arc::new(crate::device::DeviceManager::new(db.clone(), cfg.clone()));
    let runs = Arc::new(crate::run_manager::RunManager::new(Arc::new(NoRuns)));
    let capabilities = crate::capabilities::adapters::build_registry(
        devices.clone(),
        packages.clone(),
        db.clone(),
        runs.clone(),
    );
    let extensions = Arc::new(crate::extensions::ExtensionService::for_data_root(
        cfg.data_dir.clone(),
        capabilities.clone(),
    ));
    let policy =
        crate::update::policy::PolicyStore::load_blocking(&cfg.data_dir, Default::default());
    let update = Arc::new(crate::update::service::UpdateService::new(
        Arc::new(crate::update::controller::UnsupportedController),
        policy,
        Arc::new(crate::update::service::UpdateTxn::default()),
        Arc::new(crate::update::workload::Workload::default),
        db.clone(),
    ));
    let scheduler = Arc::new(crate::scheduler::Scheduler::new(db.clone()));
    let (mcp, external_control) = crate::mcp_core::assemble(
        &cfg,
        db.clone(),
        devices.clone(),
        packages.clone(),
        runs.clone(),
        scheduler.clone(),
        extensions.clone(),
        capabilities,
    )
    .unwrap();
    AppState {
        mcp,
        external_control,
        metrics: db.metrics(),
        scheduler,
        db,
        devices,
        runs,
        packages,
        viewers: Arc::new(std::sync::Mutex::new(Default::default())),
        shutdown: Arc::new(crate::shutdown::ShutdownCoordinator::new(Arc::new(|| {
            Box::pin(async {})
        }))),
        auth: Arc::new(super::super::auth::AuthState::new(
            super::super::auth::Credential::Unavailable,
            Default::default(),
            false,
            None,
        )),
        update,
        extensions,
        plugin_market: Arc::new(crate::extensions::market::PluginMarket::new(
            cfg.web_dist_dir(),
        )),
        package_market: Arc::new(crate::package_market::PackageMarket::new(
            cfg.data_dir.clone(),
        )),
        cfg,
    }
}
