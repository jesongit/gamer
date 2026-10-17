use super::*;

#[tokio::test]
#[ignore = "requires installed browser; isolated input rejection and held-state recovery"]
async fn rejected_pointer_bounds_release_inputs_without_reconnecting() {
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new().route("/", get(|| async {
        Html(r#"<!doctype html><style>body{margin:0;background:#234}</style><script>window.events=[];for(const type of ['mousedown','mouseup','keydown','keyup'])addEventListener(type,e=>events.push([type,e.key||e.button,e.ctrlKey]));</script>"#)
    }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let cfg = crate::config::Config {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let target = BrowserTarget {
        id: "browser-input-bounds-test".into(),
        name: "isolated input bounds".into(),
        url,
        profile_id: "input-bounds-test".into(),
        width: 320,
        height: 240,
    };
    let session = CdpSession::launch(
        &target,
        &cfg,
        crate::browser::browser_executable(&cfg).unwrap(),
    )
    .await
    .unwrap();
    let check = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let (_, stamp) = session.capture().await.unwrap();
        session
            .manual_input(
                &json!({"type":"key","key":"Control","action":"down"}),
                &stamp,
            )
            .await
            .unwrap();
        session
            .manual_input(
                &json!({"type":"pointer","action":"down","x":20,"y":30}),
                &stamp,
            )
            .await
            .unwrap();
        let error = session
            .manual_input(
                &json!({"type":"pointer","action":"move","x":320,"y":30}),
                &stamp,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("坐标超出画面"));
        assert!(session.is_alive());
        assert_eq!(
            session.stamp(),
            stamp,
            "an invalid point must not invalidate the target"
        );
        assert_eq!(*session.buttons.lock().await, 0);
        assert!(session.keys.lock().await.is_empty());
        assert_eq!(
            session.evaluate_for_test("events.map(e=>e[0])").await,
            json!(["keydown", "mousedown", "keyup", "mouseup"])
        );
        session
            .manual_input(&json!({"type":"tap","x":319,"y":239}), &stamp)
            .await
            .unwrap();
        assert_eq!(
            session
                .evaluate_for_test("events.filter(e=>e[0]==='mousedown').at(-1)[2]")
                .await,
            json!(false)
        );
        assert_eq!(
            session
                .evaluate_for_test("events.filter(e=>e[0]==='mouseup').length")
                .await,
            json!(2)
        );
        // The keymap contact also releases its identity on rejection rather
        // than leaving the next drag permanently blocked.
        session
            .mapped_contact(1, "down", 20, 30, Some(&stamp))
            .await
            .unwrap();
        assert!(session
            .mapped_contact(1, "move", 20, 240, Some(&stamp))
            .await
            .is_err());
        assert!(session.held_contact.lock().await.is_none());
        session
            .mapped_contact(2, "down", 30, 40, Some(&stamp))
            .await
            .unwrap();
        session
            .mapped_contact(2, "up", 30, 40, Some(&stamp))
            .await
            .unwrap();
        // Recovery does not weaken the original frame identity barrier.
        session
            .evaluate_for_test("location.hash='changed';true")
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(session
            .manual_input(&json!({"type":"tap","x":30,"y":40}), &stamp)
            .await
            .is_err());
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    session.close().await;
    server.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires installed browser; static viewport resize audit"]
async fn static_viewport_resize_does_not_relabel_cached_pixels() {
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new().route(
        "/",
        get(|| async {
            Html("<!doctype html><style>body{margin:0;background:#234}</style>static")
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let cfg = crate::config::Config {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let target = BrowserTarget {
        id: "browser-static-resize-audit".into(),
        name: "isolated static resize".into(),
        url,
        profile_id: "static-resize-audit".into(),
        width: 320,
        height: 240,
    };
    let session = CdpSession::launch(
        &target,
        &cfg,
        crate::browser::browser_executable(&cfg).unwrap(),
    )
    .await
    .unwrap();
    let check = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        for (recording, width, height) in [(false, 640, 480), (true, 800, 600)] {
            if recording {
                session.start_recording().await.unwrap();
            }
            let mut frames = session.start_preview().await.unwrap();
            let initial = super::preview_tests::next_frame(&mut frames).await;
            let initial_stamp = session.preview_image(&initial).await.unwrap().2;
            session
                .transport
                .call(
                    "Emulation.setDeviceMetricsOverride",
                    json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
                )
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            let resized = super::preview_tests::next_frame(&mut frames).await;
            let shown = session.preview_image(&resized).await;
            match shown {
                Ok((_, size, stamp)) => {
                    assert_eq!(size, (width, height));
                    assert_eq!(
                        stamp.revision, resized.revision,
                        "do not relabel cached frame pixels with a newer revision"
                    );
                    assert_ne!(stamp.revision, initial_stamp.revision);
                    // A static page may be re-borrowed after a rejected input. It
                    // must remain valid at the revision delivered to the viewer.
                    assert!(session.preview_image(&resized).await.is_ok());
                }
                Err(_) => {
                    // Invalidating a geometry transition is also valid, provided
                    // a restarted producer delivers newly captured pixels.
                    frames = session.start_preview().await.unwrap();
                    let fresh = super::preview_tests::next_frame(&mut frames).await;
                    let (_, size, stamp) = session.preview_image(&fresh).await.unwrap();
                    assert_eq!(size, (width, height));
                    assert_eq!(stamp.revision, fresh.revision);
                    assert!(session.preview_image(&fresh).await.is_ok());
                }
            }
            assert!(session
                .manual_input(&json!({"type":"tap","x":30,"y":40}), &initial_stamp)
                .await
                .is_err());
            if recording {
                session.stop_recording().await;
            }
        }
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    session.close().await;
    server.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
