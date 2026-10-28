use crate::media::{MediaId, MediaMetadata, MediaRef, MediaSource, MediaState};
use crate::recording::{
    DisplaySize, InputEventRecord, RecordingId, RecordingSessionMeta, RecordingState, SegmentMeta,
    SegmentReason,
};

fn media_principal(f: &Fixture, devices: &[&str], media: &[&str]) -> crate::mcp::ExternalPrincipal {
    let token = f
        .service
        .credentials()
        .create(CreateTokenRequest {
            label: "media fixture".into(),
            scopes: ["media.read".into()].into(),
            expires_in_seconds: 3600,
            allowed: AllowedObjects {
                devices: devices.iter().map(|s| s.to_string()).collect(),
                media: media.iter().map(|s| s.to_string()).collect(),
                packages: ["default".into()].into(),
                plugins: ["fixture-plugin".into()].into(),
                ..Default::default()
            },
        })
        .unwrap();
    f.service.credentials().authenticate(&token.token).unwrap()
}
fn seed_media(f: &Fixture, id: &str, name: &str) {
    let dir = f.core.media.data_root().join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("original.mp4"), b"synthetic bytes").unwrap();
    crate::media::write_metadata_for_test(
        &dir,
        &MediaMetadata {
            id: MediaId(id.into()),
            name: name.into(),
            sha256: "a".repeat(64),
            size: 15,
            duration_us: Some(2_000_000),
            container: "mp4".into(),
            codec: "h264".into(),
            width: 1920,
            height: 1080,
            rotation: 0,
            source: MediaSource::Recording,
            state: MediaState::Ready,
            created_at: "2026-10-28T00:00:00Z".into(),
            refs: vec![
                MediaRef {
                    package_id: "default".into(),
                    plugin_id: "fixture-plugin".into(),
                    kind: "project".into(),
                },
                MediaRef {
                    package_id: "other".into(),
                    plugin_id: "secret-plugin".into(),
                    kind: "secret reference".into(),
                },
            ],
        },
    )
    .unwrap();
}
fn seed_recording(
    f: &Fixture,
    id: &str,
    device: &str,
    media: &[&str],
    events: &[InputEventRecord],
) -> std::path::PathBuf {
    let dir = f.core.media.data_root().join(media[0]).join("recording");
    std::fs::create_dir_all(&dir).unwrap();
    let record = RecordingSessionMeta {
        id: RecordingId(id.into()),
        device_id: device.into(),
        state: RecordingState::Completed,
        started_at: "2026-10-28T00:00:00Z".into(),
        ended_at: Some("2026-10-28T00:00:02Z".into()),
        segments: media
            .iter()
            .enumerate()
            .map(|(i, id)| SegmentMeta {
                media_id: MediaId((*id).into()),
                start_us: i as u64 * 2_000_000,
                duration_us: 2_000_000,
                base_pts_us: 0,
                reason: SegmentReason::Normal,
            })
            .collect(),
        event_count: events.len() as u64,
        evidence_issues: vec![],
        error: None,
    };
    std::fs::write(
        dir.join("session.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let text = events
        .iter()
        .map(|e| serde_json::to_string(e).unwrap() + "\n")
        .collect::<String>();
    std::fs::write(dir.join("events-0001.jsonl"), text).unwrap();
    dir
}
fn recorded_event(id: &str, time: u64, kind: &str) -> InputEventRecord {
    InputEventRecord {
        schema_version: 1,
        event_id: id.into(),
        operation_id: format!("op-{id}"),
        session_id: "session-fixture".into(),
        source: "manual".into(),
        kind: kind.into(),
        timeline_us: time,
        time_domain: "recording_session_us".into(),
        coordinate_space: "device-display".into(),
        display_size: DisplaySize {
            width: 1920,
            height: 1080,
        },
        payload: json!({"key":"q","modifiers":["alt"]}),
        status: "accepted".into(),
    }
}
async fn media_call(
    f: &Fixture,
    p: &crate::mcp::ExternalPrincipal,
    name: &str,
    args: Value,
) -> crate::mcp::ToolResult {
    match f.service.call_tool(p.clone(), name, args).await {
        Ok(result) => result,
        Err(error) => crate::mcp::ToolResult::error(&error),
    }
}
fn data(result: crate::mcp::ToolResult) -> Value {
    assert!(!result.is_error, "{result:?}");
    result.structured_content.unwrap()
}

#[tokio::test]
async fn mcp_media_name_search_paginates_only_authorized_items_and_redacts_foreign_refs() {
    let f = fixture().await;
    seed_media(&f, "clip-a", "退出云游戏.mp4");
    seed_media(&f, "clip-b", "退出云游戏-alt.mp4");
    seed_media(&f, "secret", "退出云游戏-private.mp4");
    let p = media_principal(&f, &["fixture-device"], &["clip-a", "clip-b"]);
    let first = data(
        media_call(
            &f,
            &p,
            "gamer.core.media_list",
            json!({"name":"退出云游戏","limit":1}),
        )
        .await,
    );
    assert_eq!(first["total"], 2);
    assert_eq!(first["next_offset"], 1);
    assert_eq!(first["items"][0]["id"], "clip-a");
    assert_eq!(first["items"][0]["refs"].as_array().unwrap().len(), 1);
    assert!(!first.to_string().contains("private"));
    assert!(!first.to_string().contains("secret-plugin"));
    let next = data(
        media_call(
            &f,
            &p,
            "gamer.core.media_list",
            json!({"name":"退出云游戏","limit":1,"offset":1}),
        )
        .await,
    );
    assert_eq!(next["items"][0]["id"], "clip-b");
    assert_eq!(next["complete"], true);
    assert!(
        media_call(&f, &p, "gamer.core.media_get", json!({"media_id":"secret"}))
            .await
            .is_error
    );
    let empty = data(media_call(&f, &p, "gamer.core.media_list", json!({"name":"private"})).await);
    assert_eq!(empty["total"], 0);
    let no_scope = principal(&f, &["observe"]);
    assert!(
        media_call(&f, &no_scope, "gamer.core.media_list", json!({}))
            .await
            .is_error
    );
}

#[tokio::test]
async fn mcp_recording_discovery_joins_renamed_media_and_requires_device_and_every_segment() {
    let f = fixture().await;
    for (id, name) in [
        ("clip-a", "退出云游戏.mp4"),
        ("clip-b", "second.mp4"),
        ("secret", "hidden.mp4"),
        ("foreign", "foreign.mp4"),
    ] {
        seed_media(&f, id, name);
    }
    seed_recording(
        &f,
        "recording-a",
        "fixture-device",
        &["clip-a", "clip-b"],
        &[],
    );
    seed_recording(
        &f,
        "recording-secret",
        "fixture-device",
        &["secret", "clip-b"],
        &[],
    );
    seed_recording(&f, "recording-foreign", "foreign-device", &["foreign"], &[]);
    let p = media_principal(&f, &["fixture-device"], &["clip-a", "clip-b", "foreign"]);
    let found = data(
        media_call(
            &f,
            &p,
            "gamer.core.recording_list",
            json!({"name":"退出云游戏","state":"completed"}),
        )
        .await,
    );
    assert_eq!(found["total"], 1);
    assert_eq!(found["items"][0]["id"], "recording-a");
    assert_eq!(found["items"][0]["segments"][1]["media_id"], "clip-b");
    f.core
        .media
        .rename(&MediaId("clip-a".into()), "Exit Cloud.mp4")
        .unwrap();
    let found = data(
        media_call(
            &f,
            &p,
            "gamer.core.recording_list",
            json!({"name":"exit CLOUD"}),
        )
        .await,
    );
    assert_eq!(found["total"], 1);
    for id in ["recording-secret", "recording-foreign"] {
        for tool in ["gamer.core.recording_get", "gamer.core.recording_events"] {
            assert!(
                media_call(&f, &p, tool, json!({"recording_id":id}))
                    .await
                    .is_error
            );
        }
    }
    let partial = media_principal(&f, &["fixture-device"], &["clip-a"]);
    assert_eq!(
        data(media_call(&f, &partial, "gamer.core.recording_list", json!({})).await)["total"],
        0
    );
    seed_media(&f, "empty", "empty.mp4");
    let dir = seed_recording(&f, "recording-empty", "fixture-device", &["empty"], &[]);
    let path = dir.join("session.json");
    let mut meta: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    meta["segments"] = json!([]);
    std::fs::write(path, serde_json::to_vec(&meta).unwrap()).unwrap();
    for tool in ["gamer.core.recording_get", "gamer.core.recording_events"] {
        assert!(
            media_call(&f, &p, tool, json!({"recording_id":"recording-empty"}))
                .await
                .is_error
        );
    }
    let all_media = media_principal(&f, &["fixture-device"], &["*"]);
    data(
        media_call(
            &f,
            &all_media,
            "gamer.core.recording_get",
            json!({"recording_id":"recording-empty"}),
        )
        .await,
    );
}

#[tokio::test]
async fn mcp_recording_events_preserve_alt_q_and_page_in_original_session_time() {
    let f = fixture().await;
    seed_media(&f, "clip-a", "退出云游戏.mp4");
    seed_recording(
        &f,
        "recording-a",
        "fixture-device",
        &["clip-a"],
        &[
            recorded_event("later", 2000, "tap"),
            recorded_event("shortcut", 1000, "key"),
            recorded_event("confirm", 3000, "tap"),
        ],
    );
    let p = media_principal(&f, &["fixture-device"], &["clip-a"]);
    let keys = data(
        media_call(
            &f,
            &p,
            "gamer.core.recording_events",
            json!({"recording_id":"recording-a","kind":"key"}),
        )
        .await,
    );
    assert_eq!(keys["items"][0]["timeline_us"], 1000);
    assert_eq!(
        keys["items"][0]["payload"],
        json!({"key":"q","modifiers":["alt"]})
    );
    let first = data(
        media_call(
            &f,
            &p,
            "gamer.core.recording_events",
            json!({"recording_id":"recording-a","start_us":1500,"end_us":3000,"limit":1}),
        )
        .await,
    );
    assert_eq!(first["total"], 2);
    assert_eq!(first["items"][0]["event_id"], "later");
    assert_eq!(first["next_offset"], 1);
    let next=data(media_call(&f,&p,"gamer.core.recording_events",json!({"recording_id":"recording-a","start_us":1500,"end_us":3000,"offset":1,"limit":1})).await);
    assert_eq!(next["items"][0]["event_id"], "confirm");
    assert_eq!(next["next_offset"], Value::Null);
    for args in [
        json!({"recording_id":"recording-a","limit":101}),
        json!({"recording_id":"recording-a","start_us":2,"end_us":1}),
    ] {
        assert!(
            media_call(&f, &p, "gamer.core.recording_events", args)
                .await
                .is_error
        );
    }
    assert_eq!(f.accepted.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn mcp_recording_discovery_never_repairs_stale_metadata_or_loads_corrupt_event_streams() {
    let f = fixture().await;
    seed_media(&f, "clip-a", "退出云游戏.mp4");
    let dir = seed_recording(
        &f,
        "recording-a",
        "fixture-device",
        &["clip-a"],
        &[recorded_event("shortcut", 1000, "key")],
    );
    let path = dir.join("session.json");
    let mut meta: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    meta["state"] = "recording".into();
    std::fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
    std::fs::write(
        dir.join("events-0001.jsonl"),
        b"not json; fixture private path",
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();
    let p = media_principal(&f, &["fixture-device"], &["clip-a"]);
    let found = data(media_call(&f, &p, "gamer.core.recording_list", json!({"name":"退出"})).await);
    assert_eq!(found["total"], 1);
    assert_eq!(found["items"][0]["state"], "interrupted");
    data(
        media_call(
            &f,
            &p,
            "gamer.core.recording_get",
            json!({"recording_id":"recording-a"}),
        )
        .await,
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let failed = media_call(
        &f,
        &p,
        "gamer.core.recording_events",
        json!({"recording_id":"recording-a"}),
    )
    .await;
    assert!(failed.is_error);
    let error = format!("{failed:?}");
    assert!(!error.contains(&dir.to_string_lossy().to_string()));
    assert!(!error.contains("fixture private path"));
}

#[tokio::test]
async fn mcp_recording_event_budget_missing_evidence_and_root_isolation_are_enforced() {
    let f = fixture().await;
    let other = fixture().await;
    assert_ne!(
        f.core.recordings.data_root(),
        other.core.recordings.data_root()
    );
    seed_media(&f, "clip-a", "退出云游戏.mp4");
    let dir = seed_recording(
        &f,
        "recording-a",
        "fixture-device",
        &["clip-a"],
        &[recorded_event("shortcut", 1000, "key")],
    );
    assert!(f
        .core
        .recordings
        .events_snapshot_bounded(&RecordingId("recording-a".into()), 10, 4)
        .unwrap_err()
        .to_string()
        .contains("too_large"));
    let p = media_principal(&other, &["fixture-device"], &["*"]);
    assert_eq!(
        data(media_call(&other, &p, "gamer.core.recording_list", json!({})).await)["total"],
        0
    );
    let p = media_principal(&f, &["fixture-device"], &["clip-a"]);
    std::fs::write(dir.join("events-0001.jsonl"), b"").unwrap();
    assert!(
        media_call(
            &f,
            &p,
            "gamer.core.recording_events",
            json!({"recording_id":"recording-a"})
        )
        .await
        .is_error
    );
    let path = dir.join("session.json");
    let mut meta: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    meta["event_count"] = 100001.into();
    std::fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let too_large = media_call(
        &f,
        &p,
        "gamer.core.recording_events",
        json!({"recording_id":"recording-a"}),
    )
    .await;
    assert!(too_large.is_error);
    assert!(format!("{too_large:?}").contains("recording_events_too_large"));
}

#[tokio::test]
async fn mcp_media_frame_uses_real_pts_preserves_geometry_and_rejects_ambiguous_selectors() {
    use base64::Engine as _;
    let f = fixture().await;
    std::fs::create_dir_all(f.core.media.data_root()).unwrap();
    let path = f.core.media.data_root().join("fixture-source.mp4");
    let output = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=red:s=64x64:r=2:d=1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-y",
        ])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let meta = f
        .core
        .media
        .import_bytes("exit.mp4", &std::fs::read(path).unwrap())
        .unwrap();
    let p = media_principal(&f, &[], &[&meta.id.0]);
    let info = data(
        media_call(
            &f,
            &p,
            "gamer.core.media_frames",
            json!({"media_id":meta.id.0}),
        )
        .await,
    );
    assert_eq!(info["frame_count"], 2);
    let first = media_call(
        &f,
        &p,
        "gamer.core.media_frame",
        json!({"media_id":meta.id.0,"index":0}),
    )
    .await;
    assert!(!first.is_error, "{first:?}");
    let by_pts = media_call(
        &f,
        &p,
        "gamer.core.media_frame",
        json!({"media_id":meta.id.0,"pts_us":0}),
    )
    .await;
    assert_eq!(first, by_pts);
    let png = first.content.iter().find(|v| v["type"] == "image").unwrap()["data"]
        .as_str()
        .unwrap();
    let image = image::load_from_memory(
        &base64::engine::general_purpose::STANDARD
            .decode(png)
            .unwrap(),
    )
    .unwrap();
    assert_eq!((image.width(), image.height()), (64, 64));
    for args in [
        json!({"media_id":meta.id.0}),
        json!({"media_id":meta.id.0,"pts_us":0,"index":0}),
    ] {
        assert!(
            media_call(&f, &p, "gamer.core.media_frame", args)
                .await
                .is_error
        );
    }
}
