//! Minimal CDP transport. Responses and target events are consumed independently.
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::Message;

struct Command {
    id: u64,
    method: String,
    params: Value,
    revision: Option<u64>,
}

pub struct Transport {
    tx: mpsc::Sender<Command>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>,
    next: AtomicU64,
    pub revision: Arc<AtomicU64>,
    pub alive: Arc<AtomicBool>,
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
        let transport = Arc::new(Self {
            tx,
            pending: pending.clone(),
            next: AtomicU64::new(1),
            revision: revision.clone(),
            alive: alive.clone(),
        });
        tokio::spawn(async move {
            let mut main_frame: Option<String> = None;
            loop {
                tokio::select! {
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
                            let message=Message::Text(json!({"id":command.id,"method":command.method,"params":command.params}).to_string());
                            if writer.send(message).await.is_err() { break; }
                        },
                        None => break,
                    },
                    message = reader.next() => {
                        let Some(Ok(message)) = message else { break };
                        if message.is_close() { break; }
                        if let Message::Ping(bytes) = message { if writer.send(Message::Pong(bytes)).await.is_err() {break;} continue; }
                        let Ok(v) = serde_json::from_slice::<Value>(&message.into_data()) else {continue};
                        if let Some(id) = v["id"].as_u64() {
                            if let Some(reply) = pending.lock().await.remove(&id) {
                                let result = if v.get("error").is_some() {Err(v["error"].to_string())} else {Ok(v["result"].clone())};
                                let _ = reply.send(result);
                            }
                        } else {
                            let method = v["method"].as_str().unwrap_or_default();
                            let top_navigation=method=="Page.frameNavigated" && v["params"]["frame"].get("parentId").is_none();
                            if top_navigation {main_frame=v["params"]["frame"]["id"].as_str().map(str::to_string);}
                            let main_event=main_frame.as_deref().is_none_or(|id|v["params"]["frameId"].as_str()==Some(id));
                            if top_navigation || method=="Page.frameResized"
                                || (main_event && matches!(method,"Page.navigatedWithinDocument"|"Page.frameStartedLoading")) {
                                revision.fetch_add(1, Ordering::SeqCst);
                            }
                            if matches!(method, "Inspector.detached" | "Inspector.targetCrashed") {break;}
                        }
                    }
                }
            }
            alive.store(false, Ordering::SeqCst);
            for (_, reply) in pending.lock().await.drain() {
                let _ = reply.send(Err("browser target disconnected".into()));
            }
        });
        Ok(transport)
    }

    pub async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.call_checked(method, params, None).await
    }
    pub async fn call_checked(
        &self,
        method: &str,
        params: Value,
        revision: Option<u64>,
    ) -> anyhow::Result<Value> {
        anyhow::ensure!(
            self.alive.load(Ordering::SeqCst),
            "browser target disconnected"
        );
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (reply, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, reply);
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            self.tx
                .send(Command {
                    id,
                    method: method.into(),
                    params,
                    revision,
                })
                .await?;
            rx.await?.map_err(anyhow::Error::msg)
        })
        .await;
        self.pending.lock().await.remove(&id);
        result.map_err(|_| anyhow::anyhow!("CDP operation timed out: {method}"))?
    }
}
