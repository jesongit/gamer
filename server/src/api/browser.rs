//! Authenticated browser target management and a bounded screenshot preview.
use super::{ApiError, AppState};
use crate::browser::{BrowserTarget, CdpSession, FrameStamp};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine as _;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

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
                let mut value = serde_json::to_value(t).unwrap();
                value["kind"] = json!("browser");
                value["status"] = json!(if live { "online" } else { "offline" });
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
    guard_run(&st, &target.id)?;
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
pub(super) async fn preview(
    State(st): State<AppState>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let s = st
        .devices
        .browsers
        .session(&id)
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    let lease = s
        .viewer
        .clone()
        .try_lock_owned()
        .map_err(|_| ApiError::conflict("该浏览器已有投屏页面，请先断开旧页面"))?;
    Ok(ws
        .max_message_size(32 * 1024)
        .on_upgrade(move |socket| async move {
            let _lease = lease;
            serve(socket, s, st).await;
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
async fn serve(mut socket: WebSocket, s: Arc<CdpSession>, st: AppState) {
    let mut events = st.devices.browsers.events.subscribe();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let reply = tokio::select! {
            event=events.recv()=> {
                let Ok(event)=event else {continue};
                if event.device_id.as_str()!=s.id {continue}
                let mut value=serde_json::to_value(event.kind).unwrap_or(Value::Null);
                if let Some(obj)=value.as_object_mut() {obj.insert("type".into(),json!("se"));if let Some(trace)=event.trace {obj.insert("trace".into(),trace);}}
                value
            },
            _=tick.tick()=> match s.capture().await {
                Ok((bytes,stamp))=>json!({"type":"frame","png":base64::engine::general_purpose::STANDARD.encode(bytes),"stamp":stamp}),
                Err(e)=>json!({"type":"error","error":e.to_string()}),
            },
            message=socket.recv()=> {
                let Some(Ok(Message::Text(text)))=message else {break};
                let result=async {
                    let value:Value=serde_json::from_str(&text)?;
                    if value["type"]=="release" {anyhow::ensure!(st.runs.active_for_device(&s.id).is_none() && s.runs.load(std::sync::atomic::Ordering::SeqCst)==0,"任务执行期间禁止手动操作");s.release_manual().await;return Ok::<_,anyhow::Error>(())}
                    anyhow::ensure!(st.runs.active_for_device(&s.id).is_none(),"任务执行期间禁止手动操作");
                    let stamp:FrameStamp=serde_json::from_value(value["stamp"].clone())?;
                    s.manual_input(&value,&stamp).await
                }.await;
                match result {Ok(())=>continue,Err(e)=>json!({"type":"error","error":e.to_string()})}
            }
        };
        if !matches!(
            tokio::time::timeout(
                Duration::from_secs(3),
                socket.send(Message::Text(reply.to_string()))
            )
            .await,
            Ok(Ok(()))
        ) {
            break;
        }
        if !s.is_alive() {
            break;
        }
    }
    // Do not release keys owned by an automation that started after this viewer.
    if st.runs.active_for_device(&s.id).is_none() {
        s.release_manual().await;
    }
}
