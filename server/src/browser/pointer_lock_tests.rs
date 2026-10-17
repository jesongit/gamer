use super::*;

#[cfg(windows)]
fn desktop_clip() -> [i32; 4] {
    #[link(name = "user32")]
    extern "system" {
        fn GetClipCursor(rect: *mut [i32; 4]) -> i32;
        fn SetThreadDpiAwarenessContext(context: isize) -> isize;
    }
    let mut rect = [0; 4];
    unsafe {
        let previous = SetThreadDpiAwarenessContext(-4);
        assert_ne!(GetClipCursor(&mut rect), 0);
        SetThreadDpiAwarenessContext(previous);
    }
    rect
}

#[tokio::test]
#[ignore = "requires installed browser; isolated pointer lock and Windows desktop cursor"]
async fn headless_pointer_lock_does_not_confine_desktop_cursor() {
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new().route(
        "/",
        get(|| async {
            Html(
                r#"<!doctype html><body>isolated pointer lock fixture<script>
        if (location.search.includes('child')) {
          document.body.requestPointerLock().then(
            () => ({locked:!!document.pointerLockElement}),
            e => ({locked:false,error:e.name})
          ).then(result => {
            window.lockResult=result;
            if(window.opener)window.opener.postMessage({kind:'pointer-result',result},'*');
            if(parent!==window)parent.postMessage({kind:'pointer-result',result},'*');
          });
        }
        </script></body>"#,
            )
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let cfg = crate::config::Config {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let target = BrowserTarget {
        id: "browser-pointer-lock-test".into(),
        name: "isolated pointer lock".into(),
        url,
        profile_id: "pointer-lock-test".into(),
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
        tokio::time::sleep(Duration::from_millis(400)).await;
        #[cfg(windows)]
        let before = desktop_clip();
        session
            .transport
            .call(
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled":true}),
            )
            .await
            .unwrap();
        // The cloud-game recovery path brings the bound page to the front.
        // Exercise that native focus transition before the game requests lock.
        session
            .transport
            .call("Page.bringToFront", json!({}))
            .await
            .unwrap();
        let permission = session.transport.call("Runtime.evaluate", json!({"expression":"navigator.permissions.query({name:'pointer-lock'}).then(p=>p.state)","awaitPromise":true,"returnByValue":true})).await.unwrap();
        assert_eq!(permission["result"]["value"], "denied");
        let result = session.transport.call("Runtime.evaluate", json!({"expression":"document.body.requestPointerLock().then(()=>({locked:!!document.pointerLockElement}),e=>({locked:false,error:e.name}))", "userGesture":true,"awaitPromise":true,"returnByValue":true})).await.unwrap();
        assert_eq!(result["result"]["value"]["locked"], false);
        assert_eq!(result["result"]["value"]["error"], "NotAllowedError");
        #[cfg(windows)]
        {
            eprintln!("pointer lock denied: result={result}, desktop_before={before:?}, desktop_after={:?}", desktop_clip());
            assert_eq!(desktop_clip(), before);
        }
        session.release_inputs().await;
        assert_eq!(
            session
                .evaluate_for_test("!!document.pointerLockElement")
                .await,
            false
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        #[cfg(windows)]
        assert_eq!(
            desktop_clip(),
            before,
            "fixture must release any native cursor lock"
        );
        // Cross-origin iframe and popup scripts run at document startup, before
        // any explicit rebind. The browser guard must cover both automatically.
        let cross_origin = target.url.replace("127.0.0.1", "localhost");
        session.transport.call("Runtime.evaluate", json!({"expression":format!(
            "window.pointerResults=[];window.addEventListener('message',e=>{{if(e.data?.kind==='pointer-result')pointerResults.push(e.data.result)}});const frame=document.createElement('iframe');frame.src={};document.body.append(frame);window.fixturePopup=window.open({});",
            serde_json::to_string(&format!("{cross_origin}?child-iframe")).unwrap(),
            serde_json::to_string(&format!("{}?child-popup", target.url)).unwrap()
        ),"userGesture":true})).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let results = session.evaluate_for_test("window.pointerResults").await;
                if results.as_array().is_some_and(|results| results.len() >= 2) {
                    for result in results.as_array().unwrap() {
                        assert_eq!(result["locked"], false);
                        assert_eq!(result["error"], "NotAllowedError");
                    }
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("popup and cross-origin iframe must run with the preload guard");
        assert!(session.pointer_guard.is_alive());
        #[cfg(windows)]
        assert_eq!(
            desktop_clip(),
            before,
            "iframe/popup startup must not clip the desktop cursor"
        );
        assert_eq!(session.evaluate_for_test("location.href").await, target.url);
        session
            .transport
            .call(
                "Page.navigate",
                json!({"url":format!("{}?navigation", target.url)}),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let rebound = session.rebind(&session.target_id).await.unwrap();
        let permission = rebound.transport.call("Runtime.evaluate", json!({"expression":"navigator.permissions.query({name:'pointer-lock'}).then(p=>p.state)","awaitPromise":true,"returnByValue":true})).await.unwrap();
        assert_eq!(
            permission["result"]["value"], "denied",
            "navigation/rebinding must preserve the desktop boundary"
        );
        assert_eq!(
            rebound
                .evaluate_for_test(
                    "Element.prototype.requestPointerLock.gamerPointerLockBlocked===true"
                )
                .await,
            true
        );
        rebound.close().await;
    };
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(check)).await;
    session.close().await;
    server.abort();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
