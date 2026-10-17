use super::*;

fn session(token_id: &str) -> Session {
    Session {
        record: Mutex::new(SessionRecord {
            session_id: "session-one".into(),
            run_id: "run-one".into(),
            token_id: token_id.into(),
            token_label: "label-is-not-identity".into(),
            device_id: "target-one".into(),
            android_package: None,
            content_package: Some("same-package".into()),
            state: SessionPhase::Running,
            generation: 7,
            ttl_seconds: 300,
            remaining_seconds: 300,
            reason: None,
            manual_allowed: false,
        }),
        lease: Mutex::new(None),
        transition: AsyncMutex::new(()),
        operation: AsyncMutex::new(()),
        ending: AtomicBool::new(false),
        wake: Notify::new(),
        deadline: Mutex::new(Instant::now() + Duration::from_secs(300)),
        observation: Mutex::new(None),
        binding: Mutex::new(None),
        receipts: Mutex::new(BTreeMap::new()),
        package_activity: Mutex::new(None),
    }
}

#[test]
fn control_identity_does_not_follow_a_shared_package_or_label() {
    let session = session("token-one");
    assert!(session.belongs_to("token-one").is_ok());
    assert!(session.belongs_to("token-two").is_err());
    assert!(session.belongs_to("label-is-not-identity").is_err());
    assert!(session.belongs_to("same-package").is_err());
}

#[test]
fn paused_expired_and_old_generation_cannot_admit_input() {
    let session = session("token-one");
    assert!(session.running(7).is_ok());
    assert!(session.running(6).is_err());
    session.phase(SessionPhase::Paused, None);
    assert!(session.running(7).is_err());
    session.phase(SessionPhase::Running, None);
    *session.deadline.lock() = Instant::now() - Duration::from_secs(1);
    assert!(session.running(7).is_err());
    session.stop("expired");
    session.renew();
    assert_eq!(session.record.lock().state, SessionPhase::Stopping);
    assert!(
        session.running(7).is_err(),
        "Renewing the clock cannot revive a stopping session"
    );
}

#[test]
fn screenshot_coordinates_map_back_to_original_pixels_and_reject_edges() {
    let observation = capture::Observation {
        metadata: CaptureMetadata {
            frame_id: "observation".into(),
            device_id: "target-one".into(),
            session_id: Some("session-one".into()),
            generation: 7,
            captured_at: String::new(),
            width: 1000,
            height: 500,
            original_width: 2000,
            original_height: 1000,
            source: None,
            image_id: None,
        },
    };
    let point = observation.point(250.5, 100.75).unwrap();
    assert_eq!((point.x(), point.y()), (501, 201));
    assert!(observation.point(1000.0, 0.0).is_err());
    assert!(observation.point(0.0, 500.0).is_err());
    assert!(observation.point(f64::NAN, 0.0).is_err());
    assert!(observation.point(-1.0, 0.0).is_err());
}

#[test]
fn input_dtos_do_not_accept_an_owner_or_unbounded_held_input() {
    assert!(serde_json::from_value::<InputAction>(
        json!({"action":"tap","x":1,"y":2,"owner":"token-one"})
    )
    .is_err());
    assert!(serde_json::from_value::<InputAction>(json!({"action":"key_down","key":"A"})).is_err());
    assert!(InputAction::Press {
        x: 1.0,
        y: 2.0,
        duration_ms: 30_000
    }
    .validate()
    .is_err());
    assert!(InputAction::Key {
        key: "A".into(),
        duration_ms: 30_000
    }
    .validate()
    .is_err());
    assert!(InputAction::Swipe {
        x1: 0.0,
        y1: 0.0,
        x2: 1.0,
        y2: 1.0,
        duration_ms: 500
    }
    .validate()
    .is_ok());
}

#[tokio::test]
async fn input_barrier_failure_keeps_manual_closed_and_same_lease_retryable() {
    let controls = crate::core::control::ControlRegistry::default();
    let lease = controls
        .claim("synthetic-target", "synthetic-owner")
        .await
        .unwrap();
    let paused = controls.pause(&lease, async { Ok(()) }).await.unwrap();
    assert!(controls.status("synthetic-target").manual_allowed);
    let failed = controls
        .release(&paused, async {
            anyhow::bail!("synthetic cleanup failure")
        })
        .await;
    assert!(failed.is_err());
    assert!(!controls.status("synthetic-target").manual_allowed);
    assert_eq!(
        controls.status("synthetic-target").generation,
        paused.generation
    );
    controls.release(&paused, async { Ok(()) }).await.unwrap();
    assert!(controls.status("synthetic-target").manual_allowed);
}

#[tokio::test]
async fn ttl_pause_then_user_resume_keeps_run_identity_and_changes_generation() {
    let controls = crate::core::control::ControlRegistry::default();
    let session = session("token-one");
    let initial = controls
        .claim("synthetic-target", "session-one")
        .await
        .unwrap();
    session.record.lock().generation = initial.generation;
    *session.lease.lock() = Some(initial.clone());
    *session.deadline.lock() = Instant::now() - Duration::from_secs(1);
    assert!(session.running(initial.generation).is_err());
    let paused = controls.pause(&initial, async { Ok(()) }).await.unwrap();
    session.apply_paused(paused.clone(), "control_ttl_expired");
    assert_eq!(session.record.lock().state, SessionPhase::Paused);
    assert_eq!(session.record.lock().run_id, "run-one");
    assert!(!session.ending.load(Ordering::Acquire));
    assert!(controls.status("synthetic-target").manual_allowed);
    let resumed = controls.resume(&paused, async { Ok(()) }).await.unwrap();
    session.apply_resumed(resumed.clone());
    assert_eq!(session.record.lock().run_id, "run-one");
    assert!(resumed.generation > paused.generation && paused.generation > initial.generation);
    assert!(session.running(resumed.generation).is_ok());
    assert!(session.running(initial.generation).is_err());
    assert!(!controls.status("synthetic-target").manual_allowed);
}
