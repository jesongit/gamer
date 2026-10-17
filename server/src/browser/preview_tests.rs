use super::*;

#[test]
fn thumbnail_mapping_rejects_crops_and_keeps_rounding() {
    assert!(valid_preview_dimensions((960, 540), (1920, 1080)));
    assert!(valid_preview_dimensions((480, 540), (960, 1080)));
    assert!(valid_preview_dimensions((600, 337), (1920, 1080)));
    assert!(!valid_preview_dimensions((640, 480), (1920, 1080)));
    assert!(!valid_preview_dimensions((1921, 1080), (1920, 1080)));
    assert!(!valid_preview_dimensions((0, 0), (1920, 1080)));
}

pub(super) async fn next_frame(
    frames: &mut tokio::sync::watch::Receiver<Option<Arc<ScreencastFrame>>>,
) -> Arc<ScreencastFrame> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(frame) = frames.borrow_and_update().clone() {
                return frame;
            }
            frames.changed().await.unwrap();
        }
    })
    .await
    .expect("a static page must also yield a fresh initial preview")
}

#[tokio::test]
#[ignore = "requires installed browser; stable preview versus input/coordinate mutation barrier"]
async fn stable_preview_does_not_wait_for_input_but_coordinate_changes_still_do() {
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/",
                get(|| async {
                    Html("<!doctype html><body style='background:#234'>preview barrier fixture")
                }),
            ),
        )
        .await
        .unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let cfg = crate::config::Config {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let target = BrowserTarget {
        id: "browser-preview-input-barrier".into(),
        name: "preview barrier".into(),
        url,
        profile_id: "preview-input-barrier".into(),
        width: 1920,
        height: 1080,
    };
    let session = CdpSession::launch(
        &target,
        &cfg,
        crate::browser::browser_executable(&cfg).unwrap(),
    )
    .await
    .unwrap();
    let check = async {
        session.capture_for_prepare().await.unwrap();
        let mut frames = session.start_preview_with_mode("grid4").await.unwrap();
        let frame = next_frame(&mut frames).await;
        let (_, _, stamp) = session.preview_image(&frame).await.unwrap();
        let input_gate = session.gate.lock().await;
        let stable =
            tokio::time::timeout(Duration::from_millis(150), session.preview_image(&frame))
                .await
                .expect("stable preview waited for the input gate")
                .unwrap();
        assert_eq!(stable.2, stamp);
        let mut metadata = frame.metadata.clone();
        metadata["deviceWidth"] = json!(1600);
        metadata["deviceHeight"] = json!(900);
        let changed = ScreencastFrame {
            data: frame.data.clone(),
            metadata,
            revision: frame.revision,
            generation: frame.generation,
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(100), session.preview_stamp(&changed))
                .await
                .is_err(),
            "coordinate mutation must still wait for the input barrier"
        );
        assert_eq!(session.stamp(), stamp);
        assert_eq!(session.size(), (1920, 1080));
        drop(input_gate);
        assert!(session.preview_stamp(&changed).await.is_err());
        assert!(session.validate(&stamp).is_err());
        assert!(
            session.preview_stamp(&changed).await.is_err(),
            "old frame was relabelled with a new revision"
        );
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    session.close().await;
    server.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires installed browser; full viewport with both scrollbars, input and stale-frame guards"]
async fn scrollbars_keep_preview_capture_and_input_in_one_coordinate_space() {
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new().route("/", get(|| async {
        Html(r#"<!doctype html><style>html{overflow:scroll}::-webkit-scrollbar{width:15px;height:15px}body{margin:0;width:2400px;height:2200px;background:#234}#hit{position:fixed;left:1450px;top:750px;width:220px;height:180px;background:#ec4}</style><button id="hit" onclick="window.hits=(window.hits||0)+1">target</button>"#)
    }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let cfg = crate::config::Config {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let target = BrowserTarget {
        id: "browser-scrollbar-test".into(),
        name: "isolated scrollbars".into(),
        url,
        profile_id: "scrollbar-test".into(),
        width: 1920,
        height: 1080,
    };
    let session = CdpSession::launch(
        &target,
        &cfg,
        crate::browser::browser_executable(&cfg).unwrap(),
    )
    .await
    .unwrap();
    let check = async {
        session.capture_for_prepare().await.unwrap();
        assert_eq!(session.evaluate_for_test("[innerWidth,innerHeight,document.documentElement.clientWidth,document.documentElement.clientHeight]").await,
            json!([1920,1080,1905,1065]));
        let mut frames = session.start_preview_with_mode("grid4").await.unwrap();
        let frame = next_frame(&mut frames).await;
        let (_, size, stamp) = session.preview_image(&frame).await.unwrap();
        assert_eq!(size, (960, 540));
        assert_eq!(session.size(), (1920, 1080));
        for _ in 0..3 {
            session
                .manual_input(&json!({"type":"tap","x":1550,"y":850}), &stamp)
                .await
                .unwrap();
            let (png, capture_stamp) = session.capture().await.unwrap();
            let image = image::load_from_memory(&png).unwrap();
            assert_eq!((image.width(), image.height()), (1920, 1080));
            assert_eq!(
                capture_stamp, stamp,
                "taking a screenshot must not change a stable preview's mapping"
            );
            assert_eq!(session.preview_stamp(&frame).await.unwrap(), stamp);
        }
        assert_eq!(session.evaluate_for_test("window.hits").await, json!(3));
        session
            .manual_input(
                &json!({"type":"scroll","x":100,"y":100,"delta_x":40,"delta_y":300}),
                &stamp,
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let (_, scrolled_stamp) = session.capture().await.unwrap();
        session
            .manual_input(&json!({"type":"tap","x":1550,"y":850}), &scrolled_stamp)
            .await
            .unwrap();
        assert_eq!(session.evaluate_for_test("window.hits").await, json!(4));

        // A real viewport change still rejects the old frame before dispatch.
        session
            .transport
            .call(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":1600,"height":900,"deviceScaleFactor":1,"mobile":false}),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(session
            .manual_input(&json!({"type":"tap","x":1550,"y":850}), &scrolled_stamp)
            .await
            .is_err());
        assert_eq!(session.evaluate_for_test("window.hits").await, json!(4));
        let (_, fresh_stamp) = session.capture().await.unwrap();
        assert_ne!(fresh_stamp, scrolled_stamp);
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    session.close().await;
    server.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires installed browser; isolated thumbnail, input, screenshot and recording integration"]
async fn grid_preview_preserves_coordinate_capture_and_recording_surfaces() {
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new().route("/", get(|| async {
        Html(r#"<!doctype html><style>body{margin:0;background:#234}#hit{position:absolute;left:1450px;top:750px;width:220px;height:180px;background:#ec4}</style><button id="hit" onclick="window.hits=(window.hits||0)+1">target</button>"#)
    }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let cfg = crate::config::Config {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let target = BrowserTarget {
        id: "browser-preview-memory-test".into(),
        name: "isolated".into(),
        url,
        profile_id: "preview-memory-test".into(),
        width: 1920,
        height: 1080,
    };
    let session = CdpSession::launch(
        &target,
        &cfg,
        crate::browser::browser_executable(&cfg).unwrap(),
    )
    .await
    .unwrap();
    let check = async {
        tokio::time::sleep(Duration::from_millis(600)).await;
        let mut frames = session.start_preview_with_mode("grid4").await.unwrap();
        let grid = next_frame(&mut frames).await;
        let (jpeg, size, stamp) = session.preview_image(&grid).await.unwrap();
        assert_eq!(size, (960, 540));
        assert_eq!(session.size(), (1920, 1080));
        assert!(jpeg.len() < 100_000);
        session
            .manual_input(&json!({"type":"tap","x":1550,"y":850}), &stamp)
            .await
            .unwrap();
        assert_eq!(session.evaluate_for_test("window.hits").await, json!(1));
        let (png, capture_stamp) = session.capture().await.unwrap();
        let capture = image::load_from_memory(&png).unwrap();
        assert_eq!((capture.width(), capture.height()), (1920, 1080));
        assert_eq!(capture_stamp, stamp);

        // The page is static. Switching resolution must not leave it black or
        // recycle the old thumbnail, and must not change its input dimensions.
        session.configure_preview("grid9").await.unwrap();
        assert!(session.preview_stamp(&grid).await.is_err());
        let nine = next_frame(&mut frames).await;
        let (_, size, nine_stamp) = session.preview_image(&nine).await.unwrap();
        assert_eq!(size, (640, 360));
        assert_eq!(nine_stamp, stamp);

        let mut recording = session.start_recording().await.unwrap();
        assert!(session.preview_stamp(&nine).await.is_err());
        let original = next_frame(&mut recording).await;
        let original_bytes = base64::engine::general_purpose::STANDARD
            .decode(&original.data)
            .unwrap();
        let image = image::load_from_memory(&original_bytes).unwrap();
        assert_eq!((image.width(), image.height()), (1920, 1080));
        let (_, preview_size, recording_stamp) = session.preview_image(&original).await.unwrap();
        assert_eq!(preview_size, (640, 360));
        assert_eq!(recording_stamp, stamp);
        session.configure_preview("grid4").await.unwrap();
        assert_eq!(session.preview_stamp(&original).await.unwrap(), stamp);
        session.stop_preview().await;
        assert!(session.is_recording_active());
        session
            .evaluate_for_test("document.body.style.background='green';true")
            .await;
        recording.changed().await.unwrap();
        let recorded = next_frame(&mut recording).await;
        assert_eq!(session.preview_stamp(&recorded).await.unwrap(), stamp);
        session.start_preview_with_mode("grid9").await.unwrap();
        session.stop_recording().await;
        let restored = next_frame(&mut frames).await;
        assert_eq!(
            session.preview_image(&restored).await.unwrap().1,
            (640, 360)
        );
        session.stop_preview().await;
        assert!(frames.borrow().is_none());
        assert!(!session.is_preview_active());
        assert!(!session.is_recording_active());
        assert!(session.preview_stamp(&restored).await.is_err());
    };
    // Always close the isolated profile, including if an assertion unwinds.
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    session.close().await;
    server.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
