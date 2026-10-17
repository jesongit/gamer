//! Real Chromium regression tests for read-only refresh recovery. Every page
//! and browser profile belongs to this test; no running Gamer host is contacted.
use super::*;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

async fn fixture(
    profile: &str,
) -> (
    Arc<CdpSession>,
    tempfile::TempDir,
    tokio::task::JoinHandle<()>,
) {
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/", get(|| async {
            Html("<!doctype html><style>body{margin:0;background:#234}</style><script>window.inputs=0;addEventListener('mousedown',()=>inputs++);</script>")
        }))).await.unwrap()
    });
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let target = BrowserTarget {
        id: format!("browser-{profile}"),
        name: profile.into(),
        url,
        profile_id: profile.into(),
        width: 640,
        height: 480,
    };
    let session = CdpSession::launch(&target, &cfg, browser_executable(&cfg).unwrap())
        .await
        .unwrap();
    session.capture_for_prepare().await.unwrap();
    (session, dir, server)
}

#[tokio::test]
#[ignore = "requires installed browser; isolated same-target refresh capture recovery"]
async fn capture_recaptures_retired_image_without_accepting_stale_input() {
    let (session, _dir, server) = fixture("capture-refresh-recovery").await;
    let check = async {
        let before = session.stamp();
        let attempts = session.capture_attempts_for_test();
        // The hook represents a navigation event received after Chromium has
        // returned screenshot pixels and before the strict stamp check. It
        // changes only revision, never validation or screenshot transport.
        session.invalidate_next_captures_for_test(1);
        let (png, current) = session.capture().await.unwrap();
        assert!(!png.is_empty());
        assert_eq!(current.target, before.target);
        assert_eq!(current.epoch, before.epoch);
        assert!(current.revision > before.revision);
        assert!(session.capture_attempts_for_test() >= attempts + 2);
        assert_eq!(current, session.stamp());
        assert!(session
            .manual_input(&json!({"type":"tap","x":20,"y":30}), &before)
            .await
            .is_err());
        assert_eq!(session.evaluate_for_test("inputs").await, json!(0));
        session
            .manual_input(&json!({"type":"tap","x":20,"y":30}), &current)
            .await
            .unwrap();
        assert_eq!(session.evaluate_for_test("inputs").await, json!(1));
        session.close().await;
        let started = Instant::now();
        let error = session.capture().await.unwrap_err();
        assert!(error.to_string().contains("已断开"), "{error:#}");
        assert!(started.elapsed() < Duration::from_secs(1));
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    session.close().await;
    server.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires installed browser; isolated continuous-refresh capture budget"]
async fn continuously_retired_captures_stop_at_one_total_deadline() {
    let (session, _dir, server) = fixture("capture-refresh-budget").await;
    let check = async {
        let attempts = session.capture_attempts_for_test();
        session.invalidate_next_captures_for_test(usize::MAX);
        let started = Instant::now();
        let error = session
            .capture_with_timeout_for_test(Duration::from_millis(650))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("超时"), "{error:#}");
        assert!(started.elapsed() >= Duration::from_millis(550));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(session.capture_attempts_for_test() >= attempts + 2);
        assert!(
            session.is_alive(),
            "read-only retry exhaustion must not reconnect or close the target"
        );
        session.invalidate_next_captures_for_test(0);
        let (_, stamp) = session.capture().await.unwrap();
        assert_eq!(stamp, session.stamp());
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    session.close().await;
    server.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires installed browser; isolated unrelated capture error does not retry"]
async fn unrelated_capture_error_is_returned_without_refresh_retry() {
    let (session, _dir, server) = fixture("capture-error-no-retry").await;
    let check = async {
        let before = session.stamp();
        let attempts = session.capture_attempts_for_test();
        session.evaluate_for_test("Object.defineProperty(window,'visualViewport',{configurable:true,value:{scale:2}});true").await;
        let started = Instant::now();
        let error = session.capture().await.unwrap_err();
        assert!(error.to_string().contains("坐标映射无效"), "{error:#}");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(session.capture_attempts_for_test(), attempts + 1);
        assert_eq!(session.stamp(), before);
        session
            .evaluate_for_test("delete window.visualViewport;true")
            .await;
        session.capture().await.unwrap();
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    session.close().await;
    server.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
