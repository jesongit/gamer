use super::*;

#[test]
fn explicit_reload_permission_survives_ack_until_main_commit() {
    let state = Arc::new(parking_lot::Mutex::new(ReloadDialogs::default()));
    let now = Instant::now();
    state.lock().arm(7, now);
    let request = ReloadRequest {
        state: state.clone(),
        operation: 7,
        acknowledged: true,
    };
    drop(request);
    assert_eq!(
        state
            .lock()
            .dialog("beforeunload", now + Duration::from_millis(8)),
        Some(7)
    );
    assert_eq!(
        state
            .lock()
            .dialog("beforeunload", now + Duration::from_millis(9)),
        None,
        "at most one automatic acceptance per reload"
    );
    state.lock().clear(); // main-frame commit
    assert_eq!(
        state
            .lock()
            .dialog("beforeunload", now + Duration::from_millis(10)),
        None
    );
}

#[test]
fn retired_queued_reload_cannot_rearm_after_session_closure() {
    let now = Instant::now();
    let mut state = ReloadDialogs::default();
    state.arm(1, now);
    state.cancel_through(4);
    assert!(
        !state.arm(3, now),
        "a request queued before retirement must not restore permission"
    );
    assert_eq!(state.dialog("beforeunload", now), None);
    assert!(
        state.arm(5, now),
        "a later valid explicit action has its own permission"
    );
    state.finish(3);
    assert_eq!(state.dialog("beforeunload", now), Some(5));
}

#[test]
fn ordinary_dialogs_and_outside_navigation_never_gain_permission() {
    let now = Instant::now();
    let mut state = ReloadDialogs::default();
    assert_eq!(state.dialog("beforeunload", now), None);
    for kind in ["alert", "confirm", "prompt", "unknown"] {
        state.arm(1, now);
        assert_eq!(state.dialog(kind, now), None);
        assert_eq!(
            state.dialog("beforeunload", now),
            None,
            "an unrelated modal interrupts the scope"
        );
    }
}

#[test]
fn deadline_bounds_late_dialogs_and_clears_permission() {
    let now = Instant::now();
    let mut state = ReloadDialogs::default();
    state.arm(3, now);
    state.expire(now + RELOAD_DIALOG_TIMEOUT);
    assert_eq!(state.deadline(), None);
    assert_eq!(
        state.dialog("beforeunload", now + RELOAD_DIALOG_TIMEOUT),
        None
    );
    state.arm(4, now);
    assert_eq!(
        state.dialog(
            "beforeunload",
            now + RELOAD_DIALOG_TIMEOUT + Duration::from_millis(1)
        ),
        None
    );
}

#[test]
fn cancelled_failed_or_old_requests_cannot_leave_or_clear_new_permission() {
    let state = Arc::new(parking_lot::Mutex::new(ReloadDialogs::default()));
    let now = Instant::now();
    state.lock().arm(1, now);
    drop(ReloadRequest {
        state: state.clone(),
        operation: 1,
        acknowledged: false,
    });
    assert_eq!(state.lock().deadline(), None);
    state.lock().arm(2, now);
    drop(ReloadRequest {
        state: state.clone(),
        operation: 1,
        acknowledged: false,
    });
    state.lock().finish(1); // a late error reply belongs to the older operation
    assert_eq!(state.lock().dialog("beforeunload", now), Some(2));
    state.lock().finish(2);
    assert_eq!(state.lock().deadline(), None);
}

async fn receive(socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_slice(&message.into_data()).unwrap()
}
async fn send(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    value: Value,
) {
    socket.send(Message::Text(value.to_string())).await.unwrap();
}

#[tokio::test]
async fn late_dialog_after_ack_is_accepted_without_blocking_the_transport_reader() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (handled, observed) = oneshot::channel();
    let (ack_checked, ready_for_late_dialog) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let reload = receive(&mut socket).await;
        assert_eq!(reload["method"], "Page.reload");
        send(&mut socket, json!({"id":reload["id"],"result":{}})).await;
        ready_for_late_dialog.await.unwrap();
        // Child targets must never borrow the parent page's scope.
        send(&mut socket, json!({"sessionId":"child","method":"Page.javascriptDialogOpening","params":{"type":"beforeunload"}})).await;
        send(
            &mut socket,
            json!({"method":"Page.javascriptDialogOpening","params":{"type":"beforeunload"}}),
        )
        .await;
        let accept = receive(&mut socket).await;
        assert!(accept["id"].as_i64().unwrap() < -1);
        assert_eq!(accept["method"], "Page.handleJavaScriptDialog");
        assert_eq!(accept["params"], json!({"accept":true}));
        send(&mut socket, json!({"id":accept["id"],"result":{}})).await;
        send(&mut socket, json!({"method":"Page.frameNavigated","params":{"frame":{"id":"main","loaderId":"reloaded"}}})).await;
        handled.send(()).unwrap();
        let ready = receive(&mut socket).await;
        assert_eq!(ready["method"], "Runtime.evaluate");
        send(
            &mut socket,
            json!({"id":ready["id"],"result":{"result":{"value":"complete"}}}),
        )
        .await;
        // A later manual-navigation dialog has no permission after the commit.
        send(
            &mut socket,
            json!({"method":"Page.javascriptDialogOpening","params":{"type":"beforeunload"}}),
        )
        .await;
        let probe = receive(&mut socket).await;
        assert_eq!(probe["method"], "Runtime.evaluate");
        send(
            &mut socket,
            json!({"id":probe["id"],"result":{"result":{"value":true}}}),
        )
        .await;
    });
    let transport = Transport::connect(&url).await.unwrap();
    transport.reload_checked(1).await.unwrap();
    assert!(
        transport.reload_dialogs.lock().deadline().is_some(),
        "ACK cleanup must preserve the scope"
    );
    ack_checked.send(()).unwrap();
    observed.await.unwrap();
    let ready = transport
        .call(
            "Runtime.evaluate",
            json!({"expression":"document.readyState"}),
        )
        .await
        .unwrap();
    assert_eq!(ready["result"]["value"], "complete");
    assert!(transport.reload_dialogs.lock().deadline().is_none());
    transport
        .call("Runtime.evaluate", json!({"expression":"true"}))
        .await
        .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn failed_reload_reply_and_dropped_caller_clear_only_their_scope() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (sent, observed) = oneshot::channel();
    let (finish, release) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let first = receive(&mut socket).await;
        send(
            &mut socket,
            json!({"id":first["id"],"error":{"code":-1,"message":"synthetic reload rejected"}}),
        )
        .await;
        let _second = receive(&mut socket).await;
        sent.send(()).unwrap();
        release.await.unwrap();
    });
    let transport = Transport::connect(&url).await.unwrap();
    assert!(transport.reload_checked(1).await.is_err());
    assert!(transport.reload_dialogs.lock().deadline().is_none());
    let owned = transport.clone();
    let caller = tokio::spawn(async move { owned.reload_checked(1).await });
    observed.await.unwrap();
    assert!(transport.reload_dialogs.lock().deadline().is_some());
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(transport.reload_dialogs.lock().deadline().is_none());
    finish.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn child_commit_does_not_end_scope_but_new_input_does() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (sent, observed) = oneshot::channel();
    let (finish, release) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let reload = receive(&mut socket).await;
        send(&mut socket, json!({"id":reload["id"],"result":{}})).await;
        send(&mut socket, json!({"method":"Page.frameNavigated","params":{"frame":{"id":"iframe","parentId":"main"}}})).await;
        send(&mut socket, json!({"sessionId":"child","method":"Page.frameNavigated","params":{"frame":{"id":"child-main"}}})).await;
        let probe = receive(&mut socket).await;
        send(&mut socket, json!({"id":probe["id"],"result":{}})).await;
        sent.send(()).unwrap();
        let input = receive(&mut socket).await;
        assert_eq!(input["method"], "Input.dispatchMouseEvent");
        send(&mut socket, json!({"id":input["id"],"result":{}})).await;
        release.await.unwrap();
    });
    let transport = Transport::connect(&url).await.unwrap();
    transport.reload_checked(1).await.unwrap();
    transport
        .call("Runtime.evaluate", json!({"expression":"true"}))
        .await
        .unwrap();
    observed.await.unwrap();
    assert!(transport.reload_dialogs.lock().deadline().is_some());
    transport
        .call(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseMoved","x":1,"y":1}),
        )
        .await
        .unwrap();
    assert!(transport.reload_dialogs.lock().deadline().is_none());
    finish.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn disconnected_transport_clears_an_acknowledged_reload_scope() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (close, release) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let reload = receive(&mut socket).await;
        send(&mut socket, json!({"id":reload["id"],"result":{}})).await;
        release.await.unwrap();
        socket.close(None).await.unwrap();
    });
    let transport = Transport::connect(&url).await.unwrap();
    transport.reload_checked(1).await.unwrap();
    assert!(transport.reload_dialogs.lock().deadline().is_some());
    close.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while transport.alive.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(transport.reload_dialogs.lock().deadline().is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn explicit_session_retirement_clears_scope_while_socket_is_still_alive() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (finish, release) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let reload = receive(&mut socket).await;
        send(&mut socket, json!({"id":reload["id"],"result":{}})).await;
        release.await.unwrap();
    });
    let transport = Transport::connect(&url).await.unwrap();
    transport.reload_checked(1).await.unwrap();
    assert!(transport.reload_dialogs.lock().deadline().is_some());
    transport.cancel_reload_dialogs();
    assert!(transport.alive.load(Ordering::SeqCst));
    assert!(transport.reload_dialogs.lock().deadline().is_none());
    finish.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
#[ignore = "requires installed Chrome; isolated activated beforeunload reload, no real account or device"]
async fn activated_beforeunload_reload_commits_a_new_bound_document_and_retires_old_frames() {
    use super::super::{browser_executable, BrowserTarget, CdpSession};
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new().route("/", get(|| async {
        Html("<!doctype html><style>body{margin:0;background:#234;color:white}button{position:absolute;left:20px;top:20px;width:230px;height:60px}</style><button onclick=\"window.armed=true;addEventListener('beforeunload',e=>{e.preventDefault();e.returnValue=''});\">Arm</button><script>window.loads=Number(sessionStorage.loads||0)+1;sessionStorage.loads=loads;window.scratch='load-'+loads;</script>")
    }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let cfg = crate::config::Config {
        data_dir: directory.path().into(),
        ..Default::default()
    };
    let target = BrowserTarget {
        id: "browser-reload-dialog-qa".into(),
        name: "synthetic reload".into(),
        profile_id: "reload-dialog-qa".into(),
        url: url.clone(),
        width: 640,
        height: 480,
    };
    let controls = Arc::new(crate::core::control::ControlRegistry::default());
    let session = CdpSession::launch_with_controls(
        &target,
        &cfg,
        browser_executable(&cfg).unwrap(),
        controls.clone(),
    )
    .await
    .unwrap();
    let lease = controls
        .claim(&target.id, "reload-dialog-test")
        .await
        .unwrap();
    let check = async {
        session.capture_for_prepare().await.unwrap();
        let initial = session.stamp();
        let bound_page = session.target_id.clone();
        controls
            .execute(
                &lease,
                session.input(&json!({"type":"tap","x":80,"y":50}), Some(&initial)),
            )
            .await
            .unwrap();
        assert_eq!(
            session
                .evaluate_for_test(
                    "({armed:!!window.armed,activation:navigator.userActivation.hasBeenActive})"
                )
                .await,
            json!({"armed":true,"activation":true})
        );
        let before = session.stamp();
        assert!(
            session.reload(Some(&before)).await.is_err(),
            "a reload still requires its control owner"
        );
        controls
            .execute(&lease, session.reload(Some(&before)))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(session.evaluate_for_test("loads").await, json!(2));
        assert_eq!(session.evaluate_for_test("scratch").await, json!("load-2"));
        assert_eq!(session.target_id, bound_page);
        assert_eq!(session.evaluate_for_test("location.href").await, json!(url));
        session.capture_for_prepare().await.unwrap();
        assert_ne!(session.stamp(), before);
        assert!(controls
            .execute(
                &lease,
                session.input(&json!({"type":"tap","x":80,"y":50}), Some(&before))
            )
            .await
            .is_err());
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    controls
        .release(&lease, async {
            session.release_inputs().await;
            Ok(())
        })
        .await
        .unwrap();
    session.close().await;
    server.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
