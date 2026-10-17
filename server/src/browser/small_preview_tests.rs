use super::*;

#[tokio::test]
#[ignore = "requires installed browser; static small viewport must never be enlarged"]
async fn static_small_viewport_bootstrap_keeps_its_original_size() {
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
        id: "browser-preview-small".into(),
        name: "isolated small".into(),
        url,
        profile_id: "preview-small".into(),
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
        tokio::time::sleep(Duration::from_millis(600)).await;
        for mode in ["grid4", "grid9", "single"] {
            let mut frames = session.start_preview_with_mode(mode).await.unwrap();
            assert!(
                frames.has_changed().unwrap(),
                "first static frame must remain unseen in {mode}"
            );
            tokio::time::timeout(Duration::from_secs(1), frames.changed())
                .await
                .expect("static first frame must be consumable without another paint")
                .unwrap();
            let frame = preview_tests::next_frame(&mut frames).await;
            let (_, size, _) = session.preview_image(&frame).await.unwrap();
            assert_eq!(
                size,
                (320, 240),
                "small previews must not be enlarged in {mode}"
            );
            assert_eq!(session.size(), (320, 240));
        }
        // A static recording already has a latest image when a viewer joins.
        // Subscribing must not silently mark that sole image as seen either.
        session.start_recording().await.unwrap();
        let mut joined = session.start_preview_with_mode("single").await.unwrap();
        assert!(joined.has_changed().unwrap());
        tokio::time::timeout(Duration::from_secs(1), joined.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(joined.borrow_and_update().is_some());
        session.stop_recording().await;
        session.stop_preview().await;
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    session.close().await;
    server.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
