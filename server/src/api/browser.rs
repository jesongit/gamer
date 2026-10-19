//! Authenticated browser target management and a bounded CDP screencast preview.
use super::{ApiError, AppState};
use crate::browser::{BrowserTarget, CdpSession, FrameStamp};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::Arc, time::Duration};

pub(super) async fn list(State(st): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    let targets = st
        .db
        .browser_targets()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(
        targets
            .into_iter()
            .map(|t| {
                let live = st
                    .devices
                    .browsers
                    .session(&t.id)
                    .is_ok_and(|s| s.is_alive());
                let mut value = serde_json::to_value(&t).unwrap();
                value["kind"] = json!("browser");
                value["capabilities"] = json!(crate::targets::TargetCapabilities::browser());
                value["status"] = json!(if live { "online" } else { "offline" });
                value["input_control"] = json!(st.devices.controls.status(&t.id));
                value
            })
            .collect(),
    ))
}
pub(super) async fn save(
    State(st): State<AppState>,
    Json(target): Json<BrowserTarget>,
) -> Result<Json<Value>, ApiError> {
    target
        .validate()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let id = target.id.clone();
    st.devices
        .browsers
        .save(target, &st.runs)
        .await
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    Ok(Json(json!({"id":id})))
}
fn guard_run(st: &AppState, id: &str) -> Result<(), ApiError> {
    if st.runs.active_for_device(id).is_some() {
        return Err(ApiError::conflict("目标正在执行任务，请先取消或等待结束"));
    }
    Ok(())
}
pub(super) async fn remove(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    guard_run(&st, &id)?;
    st.devices
        .browsers
        .remove(&id, &st.runs)
        .await
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    Ok(Json(json!({"ok":true})))
}
pub(super) async fn connect(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    // A viewer may join a running target, but may never replace its dead session.
    if st.runs.active_for_device(&id).is_some() {
        let s = st
            .devices
            .browsers
            .session(&id)
            .map_err(|e| ApiError::conflict(e.to_string()))?;
        if !s.is_alive() {
            return Err(ApiError::conflict(
                "运行中的浏览器已断开，请结束运行后重新连接",
            ));
        }
    }
    let s = st
        .devices
        .browsers
        .prepare(&id)
        .await
        .map_err(|e| ApiError::bad_gateway(e.to_string()))?;
    Ok(Json(json!({"stamp":s.stamp(),"target_id":s.target_id})))
}
pub(super) async fn disconnect(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    guard_run(&st, &id)?;
    st.devices
        .browsers
        .close(&id, &st.runs)
        .await
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
pub(super) struct PreviewQuery {
    #[serde(default = "default_preview_mode")]
    mode: String,
}
fn default_preview_mode() -> String {
    "single".into()
}
pub(super) async fn preview(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<PreviewQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    CdpSession::validate_preview_mode(&query.mode)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let s = st
        .devices
        .browsers
        .session(&id)
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    // A mode switch can close and reopen the same target while the old viewer
    // is still releasing its producer/input lease. Wait briefly for that normal
    // teardown; an active viewer remains exclusive and is never taken over.
    let lease = tokio::time::timeout(Duration::from_secs(3), s.viewer.clone().lock_owned())
        .await
        .map_err(|_| ApiError::conflict("该浏览器已有投屏页面，请先断开旧页面"))?;
    if !s.is_alive() {
        return Err(ApiError::conflict("浏览器已断开，请重新连接"));
    }
    let sid = super::auth::AuthState::extract_sid(&headers);
    Ok(ws
        .max_message_size(32 * 1024)
        .on_upgrade(move |socket| async move {
            let _lease = lease;
            serve(socket, s, st, query.mode, sid).await;
        })
        .into_response())
}
pub(super) async fn pages(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let s = st
        .devices
        .browsers
        .session(&id)
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    let pages = s
        .pages()
        .await
        .map_err(|e| ApiError::bad_gateway(e.to_string()))?
        .into_iter()
        .map(|p| json!({"id":p["id"],"title":p["title"],"url":p["url"]}))
        .collect::<Vec<_>>();
    Ok(Json(json!({"bound":s.target_id,"pages":pages})))
}
pub(super) async fn bind(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(value): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    guard_run(&st, &id)?;
    let page = value["target_id"]
        .as_str()
        .ok_or_else(|| ApiError::bad_request("缺少标签页 ID"))?;
    st.devices
        .browsers
        .rebind(&id, page, &st.runs)
        .await
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    Ok(Json(json!({"ok":true})))
}

const DISPLAY_ACK_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_DISPLAY_RECOVERIES: u8 = 4;
const MAX_PENDING_INPUTS: usize = 64;

enum PreviewUpdate {
    Frame(String, (u32, u32), FrameStamp),
    Restart(tokio::sync::watch::Receiver<Option<Arc<crate::browser::ScreencastFrame>>>),
    Mode,
}

#[derive(Clone, Copy, PartialEq)]
enum PreviewWorkKind {
    Frame,
    Restart,
    Mode,
}

struct PreviewInputWork {
    admitted: Arc<std::sync::atomic::AtomicBool>,
    operation: BoxFuture<'static, anyhow::Result<crate::core::control::ManualLease>>,
}

/// One image may be awaiting display. A missing load/ACK retires that image and
/// retries the current one; it never builds a queue of obsolete JPEGs. Only a
/// successful ACK for the current serial restores the recovery budget.
#[derive(Default)]
struct PreviewDelivery {
    awaiting: Option<(u64, tokio::time::Instant)>,
    last_frame_started: Option<tokio::time::Instant>,
    recoveries: u8,
    unresponsive: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum DisplayReadiness {
    Ready,
    Waiting,
    Retry,
    Unresponsive,
}

impl PreviewDelivery {
    fn frame_started(&mut self, now: tokio::time::Instant) {
        self.last_frame_started = Some(now);
    }

    fn next_frame_at(&self, now: tokio::time::Instant, interval: Duration) -> tokio::time::Instant {
        self.last_frame_started
            .map_or(now, |started| (started + interval).max(now))
    }

    fn sent(&mut self, id: u64, now: tokio::time::Instant) {
        self.awaiting = Some((id, now));
    }

    fn acknowledge(&mut self, id: Option<u64>, displayed: bool) -> bool {
        if !self
            .awaiting
            .is_some_and(|(pending, _)| Some(pending) == id)
        {
            return false;
        }
        self.awaiting = None;
        if displayed {
            self.recoveries = 0;
        } else {
            self.recover();
        }
        true
    }

    fn recover(&mut self) {
        if self.recoveries >= MAX_DISPLAY_RECOVERIES {
            self.unresponsive = true;
        } else {
            self.recoveries += 1;
        }
    }

    fn readiness(&mut self, now: tokio::time::Instant) -> DisplayReadiness {
        if self.unresponsive {
            return DisplayReadiness::Unresponsive;
        }
        let Some((_, sent_at)) = self.awaiting else {
            return DisplayReadiness::Ready;
        };
        if now.saturating_duration_since(sent_at) < DISPLAY_ACK_TIMEOUT {
            return DisplayReadiness::Waiting;
        }
        self.awaiting = None;
        self.recover();
        if self.unresponsive {
            DisplayReadiness::Unresponsive
        } else {
            DisplayReadiness::Retry
        }
    }
}

async fn serve(
    mut socket: WebSocket,
    s: Arc<CdpSession>,
    st: AppState,
    mode: String,
    sid: Option<String>,
) {
    let mut account_changes = st.auth.subscribe_account_changes();
    if sid
        .as_deref()
        .is_some_and(|sid| st.auth.validate(sid).is_none())
    {
        return;
    }
    let mut events = st.devices.browsers.events.subscribe();
    let mut frames = match s.start_preview_with_mode(&mode).await {
        Ok(frames) => frames,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    json!({"type":"error","error":e.to_string()}).to_string(),
                ))
                .await;
            return;
        }
    };
    let mut revision = s.stamp();
    let mut restart_at = None;
    let mut serial = 0_u64;
    let mut delivery = PreviewDelivery::default();
    let mut frame_ready_after_ack = false;
    let mut tick = tokio::time::interval(s.preview_interval());
    let mut cleanup_lease = st.devices.controls.manual_lease(&s.id);
    let mut inputs = VecDeque::new();
    let mut input: Option<PreviewInputWork> = None;
    let mut work: Option<(
        PreviewWorkKind,
        BoxFuture<'static, anyhow::Result<PreviewUpdate>>,
    )> = None;
    let mut next_mode: Option<String> = None;
    let mut stop_after_reply = false;
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if input.is_none() {
            if let Some((value, ticket)) = inputs.pop_front() {
                let admitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
                input = Some(PreviewInputWork {
                    admitted: admitted.clone(),
                    operation: Box::pin(preview_input(
                        st.clone(),
                        s.clone(),
                        value,
                        ticket,
                        admitted,
                    )),
                });
            }
        }
        if next_mode.is_some()
            && work
                .as_ref()
                .is_none_or(|(kind, _)| *kind == PreviewWorkKind::Frame)
        {
            drop(work.take());
            let mode = next_mode.take().unwrap();
            let session = s.clone();
            work = Some((
                PreviewWorkKind::Mode,
                Box::pin(async move {
                    session.configure_preview(&mode).await?;
                    Ok(PreviewUpdate::Mode)
                }),
            ));
        }
        let frame_due = frame_ready_after_ack;
        if sid.is_some() && account_changes.has_changed().unwrap_or(true) {
            break;
        }
        let reply = tokio::select! {
            _ = account_changes.changed(), if sid.is_some() => { break; }
            // Input remains ordered and gated, but a continuous stream of
            // pointer/log events must not starve the preview timer or its ACKs.
            message=socket.recv()=> {
                let text = match message {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    Some(Err(error)) => {
                        tracing::warn!(target_id = %s.id, %error, "browser preview socket receive failed");
                        break;
                    },
                    _ => break,
                };
                let result=async {
                    let value:Value=serde_json::from_str(&text)?;
                    if value["type"]=="frame_ack" {
                        let displayed = value["displayed"].as_bool().unwrap_or(true);
                        if delivery.acknowledge(value["id"].as_u64(), displayed) {
                            if !displayed { frames.mark_changed(); }
                            // A decode/ACK arriving just after a pacing tick must
                            // not lose another entire period. Keep the frame-rate
                            // limit, but wake immediately when it is already due.
                            let now = tokio::time::Instant::now();
                            let next = delivery.next_frame_at(now, s.preview_interval());
                            frame_ready_after_ack = next <= now;
                            if !frame_ready_after_ack { tick.reset_at(next); }
                        }
                        return Ok::<_,anyhow::Error>(());
                    }
                    if value["type"]=="preview" {
                        let mode=value["mode"].as_str().ok_or_else(||anyhow::anyhow!("缺少预览模式"))?;
                        CdpSession::validate_preview_mode(mode)?;
                        next_mode = Some(mode.to_string());
                        return Ok(());
                    }
                    if inputs.len() >= MAX_PENDING_INPUTS {
                        // Never silently lose a button/key UP when overloaded.
                        // End this viewer, drain its admitted operation and
                        // release held input through the same cleanup barrier.
                        stop_after_reply = true;
                        anyhow::bail!("输入积压，正在释放输入，请重新连接");
                    }
                    let ticket = st.devices.controls.manual_lease(&s.id);
                    // Freeze trusted admission when receiving, not after a
                    // queued event crosses an owner/pause/resume transition.
                    inputs.push_back((value, ticket));
                    Ok(())
                }.await;
                match result {Ok(())=>continue,Err(e)=>{
                    // A static page may not produce another screencast event.
                    // Re-send its latest valid image to restore the display guard.
                    frames.mark_changed();
                    json!({"type":"error","source":"input","error":e.to_string()})
                }}
            },
            result=async { input.as_mut().unwrap().operation.as_mut().await }, if input.is_some()=> {
                input = None;
                match result {
                    Ok(ticket) => { cleanup_lease = ticket; continue; },
                    Err(error) => {
                        frames.mark_changed();
                        json!({"type":"error","source":"input","error":error.to_string()})
                    },
                }
            },
            result=async { work.as_mut().unwrap().1.as_mut().await }, if work.is_some()=> {
                let kind = work.take().unwrap().0;
                match result {
                    Ok(PreviewUpdate::Frame(jpeg, preview_size, stamp)) => {
                        if s.validate(&stamp).is_err() { frames.mark_changed(); continue; }
                        revision = stamp.clone();
                        serial += 1;
                        let (width,height)=s.size();
                        json!({"type":"frame","id":serial,"jpeg":jpeg,"stamp":stamp,"width":width,"height":height,"preview_width":preview_size.0,"preview_height":preview_size.1})
                    },
                    Ok(PreviewUpdate::Restart(receiver)) => {
                        frames = receiver;
                        restart_at = None;
                        json!({"type":"invalidated"})
                    },
                    Ok(PreviewUpdate::Mode) => {
                        frame_ready_after_ack = false;
                        tick = tokio::time::interval(s.preview_interval());
                        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        frames.mark_changed();
                        continue;
                    },
                    Err(error) if kind == PreviewWorkKind::Restart && s.is_alive() => {
                        tracing::debug!(target_id = %s.id, %error, "browser preview restart deferred");
                        restart_at = Some(tokio::time::Instant::now() + Duration::from_millis(500));
                        json!({"type":"invalidated"})
                    },
                    Err(error) => json!({"type":"error","error":error.to_string()}),
                }
            },
            event=events.recv()=> {
                let Ok(event)=event else {continue};
                if event.device_id.as_str()!=s.id {continue}
                let mut value=serde_json::to_value(event.kind).unwrap_or(Value::Null);
                if let Some(obj)=value.as_object_mut() {obj.insert("type".into(),json!("se"));if let Some(trace)=event.trace {obj.insert("trace".into(),trace);}}
                value
            },
            _=async { if !frame_due { tick.tick().await; } }=> {
                frame_ready_after_ack = false;
                if !s.is_alive() {
                    tracing::info!(target_id = %s.id, "browser preview target disconnected");
                    break;
                }
                if s.stamp() != revision || restart_at.is_some() {
                    if work.as_ref().is_some_and(|(kind, _)| *kind != PreviewWorkKind::Frame) { continue; }
                    if restart_at.is_some_and(|at| tokio::time::Instant::now() < at) {
                        continue;
                    }
                    revision = s.stamp();
                    delivery = PreviewDelivery::default();
                    let session = s.clone();
                    work = Some((PreviewWorkKind::Restart, Box::pin(async move {
                        session.start_preview().await.map(PreviewUpdate::Restart)
                    })));
                    continue;
                } else {
                    if work.is_some() { continue; }
                    match delivery.readiness(tokio::time::Instant::now()) {
                        DisplayReadiness::Waiting => continue,
                        DisplayReadiness::Unresponsive => {
                            tracing::warn!(target_id = %s.id, "browser preview viewer stopped acknowledging frames");
                            break;
                        },
                        DisplayReadiness::Retry => {
                            // A load event can be lost while the browser replaces
                            // its image. Invalidate that pending decode and re-send
                            // the latest image even if the page itself is static.
                            frames.mark_changed();
                            if !matches!(tokio::time::timeout(
                                Duration::from_secs(3),
                                socket.send(Message::Text(json!({"type":"invalidated"}).to_string()))
                            ).await, Ok(Ok(()))) {
                                tracing::warn!(target_id = %s.id, "browser preview recovery message could not be sent within 3s");
                                break;
                            }
                            continue;
                        },
                        DisplayReadiness::Ready => {},
                    }
                    if !frames.has_changed().unwrap_or(false) {continue;}
                    let frame = frames.borrow_and_update().clone();
                    let Some(frame) = frame else {continue};
                    delivery.frame_started(tokio::time::Instant::now());
                    let session = s.clone();
                    work = Some((PreviewWorkKind::Frame, Box::pin(async move {
                        let (jpeg, size, stamp) = session.preview_image(&frame).await?;
                        Ok(PreviewUpdate::Frame(jpeg, size, stamp))
                    })));
                    continue;
                }
            },
        };
        let displayed_serial = (reply["type"] == "frame").then_some(serial);
        if !matches!(
            tokio::time::timeout(
                Duration::from_secs(3),
                socket.send(Message::Text(reply.to_string()))
            )
            .await,
            Ok(Ok(()))
        ) {
            tracing::warn!(target_id = %s.id, reply_type = %reply["type"], "browser preview message could not be sent within 3s");
            break;
        }
        if let Some(id) = displayed_serial {
            // Transmission has its own timeout. Start the decode/ACK deadline
            // only after sending, so a slow link does not consume that grace.
            delivery.sent(id, tokio::time::Instant::now());
        }
        if stop_after_reply {
            break;
        }
        if !s.is_alive() {
            break;
        }
    }
    // Discard queued events on close, but drain the already admitted operation
    // before releasing the viewer and its original manual cleanup ticket.
    drop(work);
    inputs.clear();
    if let Some(job) = input {
        if job.admitted.load(std::sync::atomic::Ordering::SeqCst) {
            if let Ok(ticket) = job.operation.await {
                cleanup_lease = ticket;
            }
        }
    }
    s.stop_preview().await;
    // Do not release keys owned by an automation that started after this viewer.
    let _ = st
        .devices
        .controls
        .cleanup_manual(&cleanup_lease, async {
            s.release_manual().await;
            Ok(())
        })
        .await;
}

async fn preview_input(
    st: AppState,
    session: Arc<CdpSession>,
    value: Value,
    ticket: crate::core::control::ManualLease,
    admitted: Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<crate::core::control::ManualLease> {
    st.devices
        .controls
        .manual_with(&ticket, async {
            admitted.store(true, std::sync::atomic::Ordering::SeqCst);
            anyhow::ensure!(
                st.runs.active_for_device(&session.id).is_none()
                    || st.devices.controls.status(&session.id).owner.is_some(),
                "任务执行期间禁止手动操作"
            );
            if value["type"] == "release" {
                session.release_manual().await;
                return Ok(());
            }
            let stamp: FrameStamp = serde_json::from_value(value["stamp"].clone())?;
            if value["type"] == "input_event" {
                mapped_input(&st, &session, &value, &stamp).await
            } else {
                session.manual_input(&value, &stamp).await
            }
        })
        .await?;
    Ok(ticket)
}

async fn mapped_input(
    st: &AppState,
    session: &CdpSession,
    value: &Value,
    stamp: &FrameStamp,
) -> anyhow::Result<()> {
    use crate::capabilities::{DeviceHandle, DeviceId};
    use crate::extensions::{InputEvent, ScreenSize};
    session.validate(stamp)?;
    anyhow::ensure!(
        session.runs.load(std::sync::atomic::Ordering::SeqCst) == 0
            || st.devices.controls.status(&session.id).owner.is_some(),
        "目标正在执行任务"
    );
    let event: InputEvent = serde_json::from_value(value["event"].clone())?;
    let (width, height) = session.size();
    let result = st
        .extensions
        .dispatch_keymap_input(
            DeviceHandle::new(DeviceId::new(session.id.clone())).with_manual_frame(stamp.clone()),
            ScreenSize::new(width, height),
            event.clone(),
            None,
        )
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    if result.consume {
        return Ok(());
    }
    let button = |b| match b {
        1 => "middle",
        2 => "right",
        _ => "left",
    };
    let input = match event {
        InputEvent::KeyDown { code, .. } => json!({"type":"key","key":code,"action":"down"}),
        InputEvent::KeyUp { code, .. } => json!({"type":"key","key":code,"action":"up"}),
        InputEvent::MouseDown { button: b, x, y } => {
            json!({"type":"pointer","action":"down","button":button(b),"x":x,"y":y})
        }
        InputEvent::MouseUp { button: b, x, y } => {
            json!({"type":"pointer","action":"up","button":button(b),"x":x,"y":y})
        }
        InputEvent::MouseMove { x, y, .. } => json!({"type":"pointer","action":"move","x":x,"y":y}),
        InputEvent::Wheel {
            x,
            y,
            delta_x,
            delta_y,
        } => json!({"type":"scroll","x":x,"y":y,"delta_x":delta_x,"delta_y":delta_y}),
        _ => return Ok(()),
    };
    session.manual_input(&input, stamp).await
}

#[cfg(test)]
#[path = "browser_preview_tests.rs"]
mod preview_tests;
