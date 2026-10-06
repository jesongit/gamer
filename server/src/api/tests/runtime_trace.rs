use super::*;
use crate::run_manager::{RunRecord, RunSource, RunState};

fn trace_run(id: &str, device: &str, finished: bool) -> RunRecord {
    RunRecord {
        run_id: id.into(),
        device_id: device.into(),
        runner_id: "gamer-yaml".into(),
        entrypoint: "p/main.yaml".into(),
        script_id: "p/main.yaml".into(),
        source: RunSource::Manual,
        task_id: None,
        scheduled_at: None,
        state: if finished {
            RunState::Success
        } else {
            RunState::Running
        },
        started_at: chrono::Utc::now(),
        finished_at: finished.then(chrono::Utc::now),
        error: None,
    }
}
fn add_trace(t: &TestApp, run: &RunRecord, color: u8) -> String {
    t.devices.db.save_run_record(run).unwrap();
    let image = t.devices.db.trace.submit(crate::runtime_trace::CapturedEvidence {
        frame:Arc::new(crate::matcher::DecodedFrame::from_rgb(image::RgbImage::from_pixel(20,10,image::Rgb([color,2,3])))),
        captured_at:"2026-10-05T10:00:00.000Z".into(),source:None,
    },serde_json::json!({"run_id":run.run_id,"kind":"consumed","trace_enabled":true,"template":"p/templates/test.png","found":false}),false,None).unwrap();
    t.devices.db.trace.flush();
    image
}
#[tokio::test]
async fn trace_endpoints_require_authentication_including_images_and_retention() {
    let t = build_app("trace401", test_credential("admin123"), Default::default());
    for (method, path) in [
        ("GET", "/api/runs/missing/trace"),
        (
            "GET",
            "/api/runs/missing/trace/images/00000000-0000-0000-0000-000000000000",
        ),
        ("POST", "/api/runs/missing/trace/retain"),
    ] {
        let response = send(&t.app, req(method, path, None, &[], None)).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
    }
}
#[tokio::test]
async fn trace_image_access_is_private_run_scoped_and_missing_is_distinct() {
    let t = build_app(
        "trace_private",
        test_credential("admin123"),
        Default::default(),
    );
    let a = add_trace(&t, &trace_run("run-a", "device-a", false), 10);
    let b = add_trace(&t, &trace_run("run-b", "device-b", false), 20);
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let response = get_json(&t, &sid, "/api/runs/run-a/trace?limit=999999").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let page = json_body(response).await;
    assert_eq!(page["images"].as_array().unwrap().len(), 1);
    assert_eq!(page["images"][0]["image_id"], a);
    assert_eq!(page["images"][0]["metadata"]["found"], false);
    let response = get_json(&t, &sid, &format!("/api/runs/run-a/trace/images/{a}")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        image::load_from_memory(&bytes)
            .unwrap()
            .to_rgb8()
            .get_pixel(0, 0)
            .0,
        [10, 2, 3]
    );
    assert_eq!(
        get_json(&t, &sid, &format!("/api/runs/run-a/trace/images/{b}"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get_json(&t, &sid, "/api/runs/missing/trace").await.status(),
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn trace_expiry_keeps_base_events_and_returns_gone_for_old_image() {
    let t = build_app(
        "trace_expired",
        test_credential("admin123"),
        Default::default(),
    );
    let mut run = trace_run("run-a", "device-a", false);
    let image = add_trace(&t, &run, 10);
    t.devices
        .db
        .append_run_event(
            "run-a".into(),
            serde_json::json!({"ev":"detail","name":"log","data":{"message":"base log remains"}}),
        )
        .await
        .unwrap();
    run.finished_at = Some(chrono::Utc::now() - chrono::Duration::hours(25));
    run.state = RunState::Success;
    t.devices.db.save_run_record(&run).unwrap();
    t.devices.db.trace.flush();
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let page = json_body(get_json(&t, &sid, "/api/runs/run-a/trace").await).await;
    assert_eq!(page["status"], "expired");
    assert_eq!(page["images"], serde_json::json!([]));
    assert_eq!(
        get_json(&t, &sid, &format!("/api/runs/run-a/trace/images/{image}"))
            .await
            .status(),
        StatusCode::GONE
    );
    let events = json_body(get_json(&t, &sid, "/api/runs/run-a/events").await).await;
    assert_eq!(events["events"][0]["data"]["message"], "base log remains");
}
#[tokio::test]
async fn trace_retain_requires_completed_run_and_separate_quota() {
    let dir = tempfile::tempdir().unwrap();
    let t = build_app_with_config(
        Config {
            data_dir: dir.path().into(),
            trace: crate::runtime_trace::TraceConfig {
                retained_max_runs: 1,
                ..Default::default()
            },
            ..Default::default()
        },
        test_credential("admin123"),
        Default::default(),
    );
    let mut a = trace_run("run-a", "device-a", false);
    add_trace(&t, &a, 10);
    add_trace(&t, &trace_run("run-b", "device-b", true), 20);
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let headers = json_headers(sid.clone());
    let active = send(
        &t.app,
        req("POST", "/api/runs/run-a/trace/retain", None, &headers, None),
    )
    .await;
    assert_eq!(active.status(), StatusCode::CONFLICT);
    a.finished_at = Some(chrono::Utc::now());
    a.state = RunState::Success;
    t.devices.db.save_run_record(&a).unwrap();
    let retained = send(
        &t.app,
        req("POST", "/api/runs/run-a/trace/retain", None, &headers, None),
    )
    .await;
    assert_eq!(retained.status(), StatusCode::OK);
    assert_eq!(json_body(retained).await["status"], "retained");
    let quota = send(
        &t.app,
        req("POST", "/api/runs/run-b/trace/retain", None, &headers, None),
    )
    .await;
    assert_eq!(quota.status(), StatusCode::CONFLICT);
    assert!(json_body(quota).await["error"]
        .as_str()
        .unwrap()
        .contains("quota"));
    assert_eq!(
        json_body(get_json(&t, &sid, "/api/runs/run-a/trace").await).await["status"],
        "retained"
    );
    assert_eq!(
        json_body(get_json(&t, &sid, "/api/runs/run-b/trace").await).await["status"],
        "available"
    );
}
