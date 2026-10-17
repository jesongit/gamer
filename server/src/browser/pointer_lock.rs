//! Background browser documents must never own the Windows desktop cursor.
//! Permission overrides alone do not stop ordinary pointer lock after Chromium
//! foreground activation. Install a document guard before any site code runs,
//! including popup pages and out-of-process subframes, without changing binding.
use super::transport::Transport;
use anyhow::{anyhow, ensure};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

const GUARD: &str = r#"(() => {
  const descriptor = Object.getOwnPropertyDescriptor(Element.prototype, 'requestPointerLock');
  if (!descriptor?.value?.gamerPointerLockBlocked) {
    const denied = function () {
      queueMicrotask(() => document.dispatchEvent(new Event('pointerlockerror')));
      return Promise.reject(new DOMException('Pointer lock is disabled for background browser targets', 'NotAllowedError'));
    };
    Object.defineProperty(denied, 'gamerPointerLockBlocked', {value: true});
    Object.defineProperty(Element.prototype, 'requestPointerLock', {value: denied, writable: false, configurable: false});
  }
  document.exitPointerLock();
})()"#;

fn attach_params() -> Value {
    json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true,
        "filter":[{"type":"page","exclude":false},{"type":"iframe","exclude":false},{"exclude":true}]})
}

pub(super) struct PointerLockGuard {
    transport: Arc<Transport>,
    task: tokio::task::JoinHandle<()>,
}
impl PointerLockGuard {
    pub async fn install(browser_socket: &str, initial_target: &str) -> anyhow::Result<Arc<Self>> {
        let transport = Transport::connect(browser_socket).await?;
        transport
            .call(
                "Browser.setPermission",
                json!({"permission":{"name":"pointer-lock"},"setting":"denied"}),
            )
            .await?;
        let mut events = transport.events.subscribe();
        let (ready, waiting) = tokio::sync::oneshot::channel();
        let initial_target = initial_target.to_string();
        let connection = transport.clone();
        let task = tokio::spawn(async move {
            let mut ready = Some(ready);
            loop {
                let event = tokio::select! {
                    event = events.recv() => match event { Ok(event) => event, Err(_) => break },
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        if !connection.alive.load(std::sync::atomic::Ordering::SeqCst) { break; }
                        continue;
                    }
                };
                if event["method"] != "Target.attachedToTarget" {
                    continue;
                }
                let params = &event["params"];
                let Some(session) = params["sessionId"].as_str() else {
                    break;
                };
                let target = params["targetInfo"]["targetId"]
                    .as_str()
                    .unwrap_or_default();
                let result = install_document(
                    &connection,
                    session,
                    params["waitingForDebugger"].as_bool().unwrap_or(false),
                )
                .await;
                if target == initial_target {
                    if let Some(ready) = ready.take() {
                        let _ =
                            ready.send(result.as_ref().map(|_| ()).map_err(ToString::to_string));
                    }
                }
                if result.is_err() {
                    // Never resume an unprotected popup or iframe. Close this
                    // browser if isolation fails, rather than exposing the desktop.
                    break;
                }
            }
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                connection.call("Browser.close", json!({})),
            )
            .await;
        });
        let guard = Arc::new(Self { transport, task });
        guard
            .transport
            .call("Target.setAutoAttach", attach_params())
            .await?;
        tokio::time::timeout(Duration::from_secs(10), waiting)
            .await
            .map_err(|_| anyhow!("后台浏览器鼠标隔离初始化超时"))??
            .map_err(|error| anyhow!("后台浏览器鼠标隔离失败: {error}"))?;
        Ok(guard)
    }
    pub fn is_alive(&self) -> bool {
        self.transport
            .alive
            .load(std::sync::atomic::Ordering::SeqCst)
            && !self.task.is_finished()
    }
}
impl Drop for PointerLockGuard {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn install_document(
    transport: &Transport,
    session: &str,
    paused: bool,
) -> anyhow::Result<()> {
    transport
        .call_in_session(session, "Page.enable", json!({}))
        .await?;
    transport
        .call_in_session(
            session,
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source":GUARD,"runImmediately":true}),
        )
        .await?;
    transport
        .call_in_session(session, "Target.setAutoAttach", attach_params())
        .await?;
    transport
        .call_in_session(session, "Runtime.runIfWaitingForDebugger", json!({}))
        .await?;
    // The initial about:blank document already exists. New navigations use the
    // preload above; this covers current contexts before the host navigates.
    if !paused {
        let result = transport
            .call_in_session(session, "Runtime.evaluate", json!({"expression":GUARD}))
            .await?;
        ensure!(
            result.get("exceptionDetails").is_none(),
            "document pointer-lock guard failed"
        );
    }
    Ok(())
}
