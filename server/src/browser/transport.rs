//! Minimal CDP transport. Responses and target events are consumed independently.
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, mpsc, oneshot, watch, Mutex};
use tokio_tungstenite::tungstenite::Message;

struct Command {
    session_id: Option<String>,
    id: u64,
    method: String,
    params: Value,
    revision: Option<u64>,
    explicit_reload: bool,
}

const RELOAD_DIALOG_TIMEOUT: Duration = Duration::from_secs(20);

/// Permission belongs to one explicitly sent reload on this page socket. It
/// survives the early CDP acknowledgement, but never a navigation commit.
#[derive(Default)]
struct ReloadDialogs {
    active: Option<ReloadAttempt>,
    retired_through: u64,
}
struct ReloadAttempt {
    operation: u64,
    deadline: Instant,
    accepted: bool,
}
impl ReloadDialogs {
    fn arm(&mut self, operation: u64, now: Instant) -> bool {
        if operation <= self.retired_through {
            return false;
        }
        self.active = Some(ReloadAttempt {
            operation,
            deadline: now + RELOAD_DIALOG_TIMEOUT,
            accepted: false,
        });
        true
    }
    fn deadline(&self) -> Option<Instant> {
        self.active.as_ref().map(|scope| scope.deadline)
    }
    fn expire(&mut self, now: Instant) {
        if self
            .active
            .as_ref()
            .is_some_and(|scope| now >= scope.deadline)
        {
            self.clear();
        }
    }
    fn dialog(&mut self, kind: &str, now: Instant) -> Option<u64> {
        self.expire(now);
        if kind != "beforeunload" {
            // An unrelated modal interrupts this navigation permission. It
            // remains a normal manual dialog, never an automatic confirmation.
            self.clear();
            return None;
        }
        let scope = self.active.as_mut()?;
        if scope.accepted {
            return None;
        }
        scope.accepted = true;
        Some(scope.operation)
    }
    fn finish(&mut self, operation: u64) {
        if self
            .active
            .as_ref()
            .is_some_and(|scope| scope.operation == operation)
        {
            self.clear();
        }
        self.retired_through = self.retired_through.max(operation);
    }
    fn cancel_through(&mut self, operation: u64) {
        self.retired_through = self.retired_through.max(operation);
        self.clear();
    }
    fn clear(&mut self) {
        self.active = None;
    }
}

struct ReloadRequest {
    state: Arc<parking_lot::Mutex<ReloadDialogs>>,
    operation: u64,
    acknowledged: bool,
}
impl Drop for ReloadRequest {
    fn drop(&mut self) {
        if !self.acknowledged {
            self.state.lock().finish(self.operation);
        }
    }
}

#[derive(Clone)]
pub struct ScreencastFrame {
    pub data: String,
    pub metadata: Value,
    pub revision: u64,
    pub generation: u64,
}

type PendingReplies = HashMap<u64, oneshot::Sender<Result<Value, String>>>;

pub struct Transport {
    pub events: broadcast::Sender<Value>,
    tx: mpsc::Sender<Command>,
    pending: Arc<Mutex<PendingReplies>>,
    next: AtomicU64,
    pub revision: Arc<AtomicU64>,
    pub alive: Arc<AtomicBool>,
    pub frames: watch::Sender<Option<Arc<ScreencastFrame>>>,
    pub stream_generation: Arc<AtomicU64>,
    stream_active: Arc<AtomicBool>,
    stream_cutoff: Arc<parking_lot::RwLock<f64>>,
    reload_dialogs: Arc<parking_lot::Mutex<ReloadDialogs>>,
}

impl Transport {
    pub async fn connect(url: &str) -> anyhow::Result<Arc<Self>> {
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(url),
        )
        .await??;
        let (mut writer, mut reader) = socket.split();
        let (tx, mut rx) = mpsc::channel::<Command>(32);
        let pending = Arc::new(Mutex::new(HashMap::<
            u64,
            oneshot::Sender<Result<Value, String>>,
        >::new()));
        let revision = Arc::new(AtomicU64::new(1));
        let alive = Arc::new(AtomicBool::new(true));
        let (frames, _) = watch::channel(None);
        let next = AtomicU64::new(1);
        let stream_generation = Arc::new(AtomicU64::new(1));
        let stream_active = Arc::new(AtomicBool::new(false));
        let stream_cutoff = Arc::new(parking_lot::RwLock::new(0.0));
        let (events, _) = broadcast::channel(128);
        let reload_dialogs = Arc::new(parking_lot::Mutex::new(ReloadDialogs::default()));
        let transport = Arc::new(Self {
            events: events.clone(),
            tx,
            pending: pending.clone(),
            next,
            revision: revision.clone(),
            alive: alive.clone(),
            frames: frames.clone(),
            stream_generation: stream_generation.clone(),
            stream_active: stream_active.clone(),
            stream_cutoff: stream_cutoff.clone(),
            reload_dialogs: reload_dialogs.clone(),
        });
        tokio::spawn(async move {
            let mut main_frame: Option<String> = None;
            let mut invalidated_at = 0.0;
            loop {
                let reload_deadline = reload_dialogs.lock().deadline();
                tokio::select! {
                    _ = async {
                        match reload_deadline {
                            Some(deadline) => tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await,
                            None => std::future::pending::<()>().await,
                        }
                    } => { reload_dialogs.lock().expire(Instant::now()); },
                    message = rx.recv() => match message {
                        Some(command) => {
                            // A cancelled/timed-out caller must not leave a queued input behind.
                            if pending.lock().await.get(&command.id).is_none_or(|reply| reply.is_closed()) {
                                pending.lock().await.remove(&command.id);
                                continue;
                            }
                            if command.revision.is_some_and(|r|r!=revision.load(Ordering::SeqCst)) {
                                if let Some(reply)=pending.lock().await.remove(&command.id) {let _=reply.send(Err("画面已变化，操作已丢弃".into()));}
                                continue;
                            }
                            if command.explicit_reload {
                                if !reload_dialogs.lock().arm(command.id, Instant::now()) {
                                    if let Some(reply) = pending.lock().await.remove(&command.id) {
                                        let _ = reply.send(Err("刷新请求已失效，请重试".into()));
                                    }
                                    continue;
                                }
                                // The caller may have been dropped just before
                                // arming; never leave permission from that race.
                                if pending.lock().await.get(&command.id).is_none_or(|reply| reply.is_closed()) {
                                    reload_dialogs.lock().finish(command.id);
                                    pending.lock().await.remove(&command.id);
                                    continue;
                                }
                            } else if command.method == "Page.navigate" || command.method.starts_with("Input.") {
                                // New navigation/input intent cannot borrow a
                                // previous, not-yet-committed reload's permission.
                                reload_dialogs.lock().cancel_through(command.id);
                            }
                            let mut message = json!({"id":command.id,"method":command.method,"params":command.params});
                            if let Some(session_id) = command.session_id { message["sessionId"] = json!(session_id); }
                            let message = Message::Text(message.to_string());
                            if writer.send(message).await.is_err() { break; }
                        },
                        None => break,
                    },
                    message = reader.next() => {
                        let Some(Ok(message)) = message else { break };
                        if message.is_close() { break; }
                        if let Message::Ping(bytes) = message { if writer.send(Message::Pong(bytes)).await.is_err() {break;} continue; }
                        let Ok(v) = serde_json::from_slice::<Value>(&message.into_data()) else {continue};
                        if let Some(id) = v["id"].as_i64().filter(|id| *id < -1) {
                            if v.get("error").is_some() {
                                reload_dialogs.lock().finish(id.unsigned_abs() - 1);
                                tracing::debug!("本次刷新离开提示处理失败，已清理刷新作用域");
                            }
                            continue;
                        }
                        if let Some(id) = v["id"].as_u64() {
                            if let Some(reply) = pending.lock().await.remove(&id) {
                                let result = if v.get("error").is_some() {Err(v["error"].to_string())} else {Ok(v["result"].clone())};
                                let _ = reply.send(result);
                            }
                        } else {
                            if events.receiver_count() > 0 { let _ = events.send(v.clone()); }
                            // Flattened child sessions have their own document
                            // lifetime; they must not invalidate or close this socket.
                            if v.get("sessionId").is_some() { continue; }
                            let method = v["method"].as_str().unwrap_or_default();
                            if method == "Page.javascriptDialogOpening" {
                                let operation = reload_dialogs.lock().dialog(v["params"]["type"].as_str().unwrap_or_default(), Instant::now());
                                if let Some(operation) = operation {
                                    // Send directly in this I/O loop. Awaiting
                                    // Transport::call here would wait on this
                                    // same reader and deadlock the connection.
                                    let id = -i64::try_from(operation.saturating_add(1)).unwrap_or(i64::MAX);
                                    let response = json!({"id":id,"method":"Page.handleJavaScriptDialog","params":{"accept":true}});
                                    if writer.send(Message::Text(response.to_string())).await.is_err() { break; }
                                }
                            }
                            if method == "Page.javascriptDialogClosed" && v["params"]["result"].as_bool() == Some(false) {
                                reload_dialogs.lock().clear();
                            }
                            if method == "Page.screencastFrame" {
                                let params = &v["params"];
                                // Acknowledge even discarded frames. Only the latest image is
                                // retained; a slow viewer must not block CDP responses/input.
                                // Negative IDs are reserved for these fire-and-forget ACKs.
                                if writer.send(Message::Text(json!({"id":-1,"method":"Page.screencastFrameAck","params":{"sessionId":params["sessionId"]}}).to_string())).await.is_err() {break;}
                                let generation = stream_generation.load(Ordering::SeqCst);
                                if stream_active.load(Ordering::SeqCst) && params["metadata"]["timestamp"].as_f64().is_some_and(|t| t >= invalidated_at && t >= *stream_cutoff.read()) {
                                    if let Some(data) = params["data"].as_str().filter(|data|data.len() <= 32*1024*1024) {
                                        frames.send_replace(Some(Arc::new(ScreencastFrame {
                                            data: data.into(), metadata: params["metadata"].clone(),
                                            revision: revision.load(Ordering::SeqCst),
                                            generation,
                                        })));
                                    }
                                }
                                continue;
                            }
                            let top_navigation=method=="Page.frameNavigated" && v["params"]["frame"].get("parentId").is_none();
                            if top_navigation {
                                main_frame=v["params"]["frame"]["id"].as_str().map(str::to_string);
                                reload_dialogs.lock().clear();
                            }
                            let main_event=main_frame.as_deref().is_none_or(|id|v["params"]["frameId"].as_str()==Some(id));
                            // A popup can hide the explicitly bound page without navigating it.
                            // Chrome then stops its screencast: retire those pixels so the
                            // viewer restores this same page instead of waiting indefinitely.
                            let hidden=method=="Page.screencastVisibilityChanged" && v["params"]["visible"].as_bool()==Some(false);
                            // A slow refresh may have no usable static bootstrap
                            // until parsing finishes. Re-arm that same producer
                            // when the main document becomes interactive.
                            if top_navigation || method=="Page.frameResized" || hidden || method=="Page.domContentEventFired"
                                || (main_event && matches!(method,"Page.navigatedWithinDocument"|"Page.frameStartedLoading")) {
                                revision.fetch_add(1, Ordering::SeqCst);
                                invalidated_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
                                frames.send_replace(None);
                            }
                            if matches!(method, "Inspector.detached" | "Inspector.targetCrashed") {break;}
                        }
                    }
                }
            }
            reload_dialogs.lock().cancel_through(u64::MAX);
            alive.store(false, Ordering::SeqCst);
            frames.send_replace(None);
            for (_, reply) in pending.lock().await.drain() {
                let _ = reply.send(Err("browser target disconnected".into()));
            }
        });
        Ok(transport)
    }

    pub async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.call_checked(method, params, None).await
    }
    /// Clear a stopped stream immediately, including frames already being copied
    /// by a consumer. Timestamp fencing prevents a late old-size CDP event from
    /// being stamped as belonging to a newly started stream.
    pub fn reset_stream(&self, active: bool) {
        *self.stream_cutoff.write() = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        self.stream_generation.fetch_add(1, Ordering::SeqCst);
        self.stream_active.store(active, Ordering::SeqCst);
        self.frames.send_replace(None);
    }
    pub async fn call_checked(
        &self,
        method: &str,
        params: Value,
        revision: Option<u64>,
    ) -> anyhow::Result<Value> {
        self.request(method, params, revision, None, false).await
    }
    pub async fn reload_checked(&self, revision: u64) -> anyhow::Result<Value> {
        self.request("Page.reload", json!({}), Some(revision), None, true)
            .await
    }
    /// Retiring a bound session must revoke its navigation permission before
    /// asynchronous input release or socket/browser shutdown begins.
    pub fn cancel_reload_dialogs(&self) {
        let barrier = self.next.fetch_add(1, Ordering::SeqCst);
        self.reload_dialogs.lock().cancel_through(barrier);
    }
    pub async fn call_in_session(
        &self,
        session_id: &str,
        method: &str,
        params: Value,
    ) -> anyhow::Result<Value> {
        self.request(method, params, None, Some(session_id.into()), false)
            .await
    }
    async fn request(
        &self,
        method: &str,
        params: Value,
        revision: Option<u64>,
        session_id: Option<String>,
        explicit_reload: bool,
    ) -> anyhow::Result<Value> {
        anyhow::ensure!(
            self.alive.load(Ordering::SeqCst),
            "browser target disconnected"
        );
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut reload = explicit_reload.then(|| ReloadRequest {
            state: self.reload_dialogs.clone(),
            operation: id,
            acknowledged: false,
        });
        let (reply, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, reply);
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            self.tx
                .send(Command {
                    session_id,
                    id,
                    method: method.into(),
                    params,
                    revision,
                    explicit_reload,
                })
                .await?;
            rx.await?.map_err(anyhow::Error::msg)
        })
        .await;
        self.pending.lock().await.remove(&id);
        let result = result.map_err(|_| anyhow::anyhow!("CDP operation timed out: {method}"))?;
        if result.is_ok() {
            if let Some(reload) = &mut reload {
                reload.acknowledged = true;
            }
        }
        result
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.reload_dialogs.lock().cancel_through(u64::MAX);
    }
}

#[cfg(test)]
#[path = "reload_dialog_tests.rs"]
mod reload_dialog_tests;
