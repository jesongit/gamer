use super::*;
use crate::core::ActivityLease as _;
use serde_json::json;

#[test]
fn idle_grace_begins_after_the_last_observed_consumer_and_resets() {
    let start = Instant::now();
    let grace = Duration::from_secs(30);
    let mut timer = IdleTimer::default();
    assert!(!timer.ready(start, grace, true));
    assert!(!timer.ready(start + Duration::from_secs(10), grace, false));
    assert!(!timer.ready(start + Duration::from_secs(39), grace, false));
    assert!(timer.ready(start + Duration::from_secs(40), grace, false));
    assert!(!timer.ready(start + Duration::from_secs(41), grace, true));
    assert!(!timer.ready(start + Duration::from_secs(80), grace, false));
    assert!(!timer.ready(start + Duration::from_secs(109), grace, false));
    assert!(timer.ready(start + Duration::from_secs(110), grace, false));
    assert!(!timer.ready(start + Duration::from_secs(200), Duration::ZERO, false));
    assert!(!timer.ready(start + Duration::from_secs(201), grace, false));
}

#[tokio::test]
async fn maintenance_cannot_seal_a_running_or_paused_ai_owner_or_change_tickets() {
    let controls = crate::core::control::ControlRegistry::default();
    let id = "browser-idle-control";
    let manual = controls.manual_lease(id);
    let before = controls.status(id);
    assert_eq!(controls.when_idle(id, || 7), Some(7));
    assert_eq!(controls.manual_lease(id), manual);
    assert_eq!(controls.status(id).generation, before.generation);
    let lease = controls.claim(id, "ai").await.unwrap();
    assert!(controls
        .when_idle(id, || panic!("owned target was sealed"))
        .is_none());
    let paused = controls.pause(&lease, async { Ok(()) }).await.unwrap();
    let paused_ticket = controls.manual_lease(id);
    assert!(controls.status(id).manual_allowed);
    assert!(controls
        .when_idle(id, || panic!("paused owner was sealed"))
        .is_none());
    assert_eq!(controls.manual_lease(id), paused_ticket);
    controls.release(&paused, async { Ok(()) }).await.unwrap();
    assert_eq!(controls.when_idle(id, || true), Some(true));
}

/// Separate from production cloud accounts: all Chrome state lives in tempdir.
#[tokio::test]
#[ignore = "requires installed browser; local profile/idle protection integration"]
async fn browser_idle_recovery_preserves_login_and_all_consumer_boundaries() {
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/", get(|| async {
            Html("<!doctype html><title>idle test</title><body>local browser idle fixture</body>")
        }))).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        data_dir: dir.path().into(),
        browser_idle_secs: 2,
        ..Default::default()
    };
    let db = Arc::new(crate::store::Store::open(&cfg).unwrap());
    let target = BrowserTarget {
        id: "browser-idle-fixture".into(),
        name: "idle fixture".into(),
        url,
        profile_id: "idle-fixture".into(),
        width: 640,
        height: 480,
    };
    db.save_browser_target(target.clone()).unwrap();
    let controls = Arc::new(crate::core::control::ControlRegistry::default());
    let manager = BrowserManager::with_controls(db.clone(), cfg, controls.clone());
    let session = manager.prepare(&target.id).await.unwrap();
    // prepare may return while the initial navigation is still publishing its
    // revision. Use the real capture readiness path before writing the profile.
    let (_, original_stamp) = session.capture().await.unwrap();
    session.evaluate_for_test("localStorage.setItem('login','persisted'); document.cookie='login=persisted;max-age=3600';true").await;
    let start = Instant::now();

    // An upgrade owns the viewer before the first frame subscription starts.
    let viewer = session.viewer.clone().try_lock_owned().unwrap();
    manager.reclaim_idle(start, |_| false).await;
    manager
        .reclaim_idle(start + Duration::from_secs(20), |_| false)
        .await;
    assert!(session.is_alive());
    drop(viewer);

    // Recording remains a consumer after its preview disappears.
    let _recording = session.start_recording().await.unwrap();
    manager
        .reclaim_idle(start + Duration::from_secs(21), |_| false)
        .await;
    manager
        .reclaim_idle(start + Duration::from_secs(40), |_| false)
        .await;
    assert!(session.is_alive());
    session.stop_recording().await;

    // Core capture/extension activity is passed through without pretending to
    // be a script run, and an AI paused owner retains its original transport.
    manager
        .reclaim_idle(start + Duration::from_secs(41), |_| true)
        .await;
    let owner = controls.claim(&target.id, "idle-test-ai").await.unwrap();
    let owner = controls.pause(&owner, async { Ok(()) }).await.unwrap();
    manager
        .reclaim_idle(start + Duration::from_secs(60), |_| false)
        .await;
    assert_eq!(session.stamp(), original_stamp);
    controls.release(&owner, async { Ok(()) }).await.unwrap();

    // An unrelated target's slow launch cannot block this run's lease.
    let other_gate = manager.lifecycle_gate("browser-other-fixture");
    let other_launch = other_gate.lock().await;
    let mut lease = manager.acquire(&target.id).unwrap();
    drop(other_launch);
    let this_gate = manager.lifecycle_gate(&target.id);
    let same_target_launch = this_gate.lock().await;
    assert!(manager.acquire(&target.id).is_err());
    drop(same_target_launch);
    manager
        .reclaim_idle(start + Duration::from_secs(61), |_| false)
        .await;
    manager
        .reclaim_idle(start + Duration::from_secs(80), |_| false)
        .await;
    assert!(session.is_alive());
    lease.release().await;
    drop(lease);

    // No consumer may inherit the pre-consumer idle interval.
    manager
        .reclaim_idle(start + Duration::from_secs(81), |_| false)
        .await;
    manager
        .reclaim_idle(start + Duration::from_secs(82), |_| false)
        .await;
    assert!(session.is_alive());
    manager
        .reclaim_idle(start + Duration::from_secs(83), |_| false)
        .await;
    assert!(!session.is_alive());
    assert!(manager.session(&target.id).is_err());
    assert!(manager.acquire(&target.id).is_err());
    assert!(db
        .browser_targets()
        .unwrap()
        .iter()
        .any(|saved| saved.id == target.id));
    assert!(dir.path().join("browser-profiles/idle-fixture").is_dir());

    let recovered = manager.prepare(&target.id).await.unwrap();
    let (_, recovered_stamp) = recovered.capture().await.unwrap();
    assert!(recovered.is_alive());
    assert_ne!(recovered_stamp.epoch, original_stamp.epoch);
    assert!(recovered.validate(&original_stamp).is_err());
    assert_eq!(
        recovered
            .evaluate_for_test("localStorage.getItem('login')")
            .await,
        json!("persisted")
    );
    assert_eq!(
        recovered
            .evaluate_for_test("document.cookie.includes('login=persisted')")
            .await,
        json!(true)
    );
    let mut recovered_lease = manager.acquire(&target.id).unwrap();
    recovered_lease.release().await;
    manager.shutdown().await;
    server.abort();
}
