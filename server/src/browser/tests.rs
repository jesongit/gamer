use super::*;
use serde_json::{json, Value};
use std::time::Duration;

#[test]
fn target_validation_rejects_path_traversal_and_non_web_urls() {
    let mut t = target("http://localhost:1234", "a");
    assert!(t.validate().is_ok());
    t.profile_id = "../a".into();
    assert!(t.validate().is_err());
    t.profile_id = "a".into();
    t.url = "file:///C:/secret".into();
    assert!(t.validate().is_err());
    t.url = "https://user:password@example.com".into();
    assert!(t.validate().is_err());
}
fn target(url: &str, profile: &str) -> BrowserTarget {
    BrowserTarget {
        id: format!("browser-{profile}"),
        name: profile.into(),
        url: url.into(),
        profile_id: profile.into(),
        width: 640,
        height: 480,
    }
}

// Explicit opt-in: requires an installed Chrome/Edge, uses only a local test page
// and temporary profile directories. No real cloud account or game is opened.
#[tokio::test]
#[ignore = "requires installed browser; local CDP integration"]
async fn browser_roundtrip_isolation_navigation_and_profile_persistence() {
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let app=Router::new().route("/",get(||async {Html(r#"<!doctype html><style>body{margin:0;background:#123456}input{position:absolute;left:20px;top:20px;width:240px;height:40px}</style><input id='entry'><script>window.events=[];addEventListener('keydown',e=>events.push([e.key,e.ctrlKey,e.shiftKey]));addEventListener('pointerdown',e=>events.push(['pointer',e.button,e.clientX,e.clientY]));addEventListener('pointerup',e=>events.push(['up',e.clientX,e.clientY]));</script>"#)}));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let cfg = crate::config::Config {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };
    let executable = browser_executable(&cfg).expect("installed browser");
    let a = target(&url, "a");
    let b = target(&url, "b");
    let first = CdpSession::launch(&a, &cfg, executable.clone())
        .await
        .unwrap();
    let (png, stamp) = capture(&first).await;
    let frame = image::load_from_memory(&png).unwrap().to_rgb8();
    assert_eq!(frame.dimensions(), (640, 480));
    assert_eq!(frame.get_pixel(500, 400).0, [0x12, 0x34, 0x56]);
    first
        .input(&json!({"type":"tap","x":80,"y":40}), Some(&stamp))
        .await
        .unwrap();
    first
        .input(&json!({"type":"key","key":"a"}), None)
        .await
        .unwrap();
    first
        .input(&json!({"type":"text","text":"中文"}), None)
        .await
        .unwrap();
    assert_eq!(first.evaluate_for_test("entry.value").await, json!("a中文"));
    first
        .input(&json!({"type":"key","key":"ctrl","action":"down"}), None)
        .await
        .unwrap();
    first
        .input(&json!({"type":"key","key":"w","action":"down"}), None)
        .await
        .unwrap();
    first.release_inputs().await;
    assert_eq!(
        first
            .evaluate_for_test("events.filter(e=>e[0]==='w').at(-1)[1]")
            .await,
        json!(true)
    );
    first
        .evaluate_for_test(
            "localStorage.setItem('account','a'); document.cookie='account=a;max-age=3600'; true",
        )
        .await;
    assert_eq!(
        first.evaluate_for_test("document.cookie").await,
        json!("account=a")
    );
    let db = Arc::new(crate::store::Store::open(&cfg).unwrap());
    db.save_browser_target(b.clone()).unwrap();
    let devices = Arc::new(crate::device::DeviceManager::new(db, cfg.clone()));
    // The same prepare/acquire path used by scheduled and manual runners, without DeviceManager::start or ADB.
    crate::targets::prepare(&devices, &b.id).await.unwrap();
    let lease = crate::targets::acquire(&devices, &b.id).unwrap();
    let second = devices.browsers.session(&b.id).unwrap();
    use crate::capabilities::adapters::{
        DeviceAdapter, FrameAdapter, FrameStore, InputAdapter, TouchAdapter,
    };
    use crate::capabilities::{DeviceService, FrameService, InputService, TouchPoint};
    let device = Arc::new(DeviceAdapter::new(devices.clone()));
    let handle = device
        .resolve(&crate::capabilities::DeviceId::new(b.id.clone()))
        .await
        .unwrap();
    let frames = Arc::new(FrameStore::new());
    let frame_adapter = FrameAdapter::new(devices.clone(), frames.clone());
    let frame_handle = frame_adapter.capture(&handle).await.unwrap();
    let provenance = frame_adapter.stamp(frame_handle).await.unwrap().unwrap();
    let mut template = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::imageops::crop_imm(&frame, 15, 18, 180, 44).to_image())
        .write_to(&mut template, image::ImageFormat::Png)
        .unwrap();
    let matched = crate::matcher::match_decoded_frame(
        &frames.get(frame_handle).unwrap(),
        &crate::matcher::DecodedMatchRequest {
            template_png: template.into_inner(),
            threshold: Some(0.9),
            region: Some([0, 0, 320, 100]),
            color: false,
        },
    )
    .unwrap()
    .expect("browser pixels match existing NCC");
    let input = InputAdapter::new(device.clone(), Arc::new(TouchAdapter::new(device)));
    input
        .tap_from_frame(
            &handle,
            TouchPoint::new(
                matched.x + matched.width / 2,
                matched.y + matched.height / 2,
                1.0,
            ),
            Some(&provenance),
        )
        .await
        .unwrap();
    assert_eq!(
        second.evaluate_for_test("document.activeElement.id").await,
        json!("entry")
    );
    input
        .swipe(
            &handle,
            crate::capabilities::SwipeGesture::new(
                TouchPoint::new(40, 40, 1.0),
                TouchPoint::new(120, 40, 1.0),
                Duration::from_millis(100),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        second
            .evaluate_for_test("events.filter(e=>e[0]==='up').at(-1)")
            .await,
        json!(["up", 120, 40])
    );
    drop(lease);
    capture(&second).await;
    assert_eq!(
        second
            .evaluate_for_test("localStorage.getItem('account')")
            .await,
        Value::Null
    );
    assert!(second
        .input(&json!({"type":"tap","x":80,"y":40}), Some(&stamp))
        .await
        .is_err());
    // A popup must never change the bound target.
    first
        .evaluate_for_test("window.open('about:blank'); true")
        .await;
    assert_eq!(first.evaluate_for_test("entry.value").await, json!("a中文"));
    first.evaluate_for_test("location.hash='next'; true").await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(first
        .input(&json!({"type":"tap","x":80,"y":40}), Some(&stamp))
        .await
        .is_err());
    capture(&first).await;
    let popup = first
        .pages()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p["id"] != first.target_id)
        .expect("popup page");
    let rebound = first.rebind(popup["id"].as_str().unwrap()).await.unwrap();
    assert!(!first.is_alive());
    assert!(first
        .input(&json!({"type":"key","key":"a"}), None)
        .await
        .is_err());
    let (_, new_stamp) = capture(&rebound).await;
    assert_ne!(new_stamp.epoch, stamp.epoch);
    rebound.close().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let reopened = CdpSession::launch(&a, &cfg, executable).await.unwrap();
    capture(&reopened).await;
    assert_eq!(
        reopened
            .evaluate_for_test("localStorage.getItem('account')")
            .await,
        json!("a")
    );
    assert_eq!(
        reopened
            .evaluate_for_test("document.cookie.includes('account=a')")
            .await,
        json!(true)
    );
    assert!(reopened
        .input(&json!({"type":"tap","x":80,"y":40}), Some(&stamp))
        .await
        .is_err());
    reopened.close().await;
    second.close().await;
    server.abort();
}
async fn capture(session: &CdpSession) -> (Vec<u8>, FrameStamp) {
    for _ in 0..20 {
        if let Ok(frame) = session.capture().await {
            return frame;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("browser frame never became ready")
}

#[tokio::test]
#[ignore = "requires installed browser; local CDP integration"]
async fn screencast_continuous_frames_input_capture_and_restart() {
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new().route("/", get(|| async { Html(r#"<!doctype html><style>body{margin:0;background:#123456}input{position:absolute;left:20px;top:20px;width:240px;height:40px}#box{position:absolute;top:200px;width:200px;height:200px;background:red}</style><input id="entry"><div id="box"></div><script>function animate(t){box.style.left=(t/4%1500)+'px';window.animation=requestAnimationFrame(animate)}animate(0)</script>"#) }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let cfg = crate::config::Config {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };
    let mut t = target(&url, "stream");
    t.width = 1920;
    t.height = 1080;
    let session = CdpSession::launch(&t, &cfg, browser_executable(&cfg).unwrap())
        .await
        .unwrap();
    // Let navigation settle without using captureScreenshot to bootstrap preview.
    tokio::time::sleep(Duration::from_millis(800)).await;
    let mut frames = session.start_preview().await.unwrap();
    let mut stamp = None;
    let started = tokio::time::Instant::now();
    let mut count = 0;
    while started.elapsed() < Duration::from_secs(2) {
        tokio::time::timeout(Duration::from_secs(5), frames.changed())
            .await
            .unwrap()
            .unwrap();
        let frame = frames.borrow_and_update().clone();
        if let Some(frame) = frame {
            stamp = Some(session.preview_stamp(&frame).await.unwrap());
            count += 1;
        }
    }
    eprintln!(
        "1080p CDP frames in {:.2}s: {count}",
        started.elapsed().as_secs_f64()
    );
    assert!(
        count >= 10,
        "continuous animation should exceed old 2fps polling"
    );
    let stamp = stamp.unwrap();
    assert_eq!(session.size(), (1920, 1080));
    session
        .manual_input(&json!({"type":"tap","x":80,"y":40}), &stamp)
        .await
        .unwrap();
    session
        .manual_input(&json!({"type":"text","text":"流畅123"}), &stamp)
        .await
        .unwrap();
    assert_eq!(
        session.evaluate_for_test("entry.value").await,
        json!("流畅123")
    );
    let (png, capture_stamp) = session.capture().await.unwrap();
    assert_eq!(image::load_from_memory(&png).unwrap().width(), 1920);
    assert_eq!(capture_stamp, stamp);
    // A stalled consumer retains just the newest frame; input remains responsive.
    let previous_time = frames.borrow_and_update().as_ref().unwrap().metadata["timestamp"]
        .as_f64()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    let latest_time = frames.borrow_and_update().as_ref().unwrap().metadata["timestamp"]
        .as_f64()
        .unwrap();
    assert!(latest_time > previous_time + 0.1);
    session
        .evaluate_for_test("cancelAnimationFrame(animation); location.hash='next'; true")
        .await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(session
        .manual_input(&json!({"type":"tap","x":80,"y":40}), &stamp)
        .await
        .is_err());
    // Re-starting requests a frame even when the page stopped changing.
    frames = session.start_preview().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), frames.changed())
        .await
        .unwrap()
        .unwrap();
    let frame = frames.borrow_and_update().clone().unwrap();
    let fresh = session.preview_stamp(&frame).await.unwrap();
    assert_ne!(fresh.revision, stamp.revision);
    session.stop_preview().await;
    session.close().await;
    server.abort();
}
