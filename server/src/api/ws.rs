//! WebSocket 信令：浏览器连接 /ws/device/:id → 交换 SDP → 建立 WebRTC

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use futures_util::StreamExt;
use serde_json::json;
use tracing::{debug, info, warn};

use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use super::AppState;
use crate::device::scrcpy::VideoFrame;
use crate::webrtc::{make_audio_queue, make_frame_queue, ViewerHandle, ViewerSession};

pub async fn ws_device(
    ws: WebSocketUpgrade,
    State(st): State<AppState>,
    Path(device_id): Path<String>,
) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, st, device_id))
}

async fn handle_ws(mut socket: WebSocket, st: AppState, device_id: String) {
    info!(device = %device_id, "ws signaling connected");

    // 设备必须在线
    let Some(session) = st.devices.session(&device_id) else {
        let _ = socket
            .send(Message::Text(
                json!({"type": "error", "error": "device offline"}).to_string(),
            ))
            .await;
        return;
    };
    let Some(frames_tx) = st.devices.frames_tx(&device_id) else {
        let _ = socket
            .send(Message::Text(
                json!({"type": "error", "error": "device not streaming"}).to_string(),
            ))
            .await;
        return;
    };
    let audio_frames_tx = st.devices.audio_frames_tx(&device_id);

    // 服务端 → 浏览器反向消息通道：viewer 被新页面 force 顶替时经此推送
    // taken_over 通知（select 中转发给 socket），旧页面收到后不再自动重连
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // 等待浏览器发来 offer
    let mut viewer: Option<ViewerSession> = None;
    let mut first_message = true;
    // peer 死亡通知 receiver：固定复用同一个 receiver（不可每次 clone——
    // watch::Receiver 新 clone 的 version 是当前值，会错过已发生的 Failed 通知）
    let mut peer_closed_rx: Option<tokio::sync::watch::Receiver<bool>> = None;

    loop {
        if peer_closed_rx.is_none() {
            peer_closed_rx = viewer.as_ref().map(|v| v.peer_closed_rx.clone());
        }
        // peer Failed/Closed 通知：立即退出 ws 循环、释放 viewer（mDNS 等资源）。
        // 浏览器 TCP 断开时 axum socket.next() 可能不返回，若不监听此通知，
        // viewer 会泄漏——泄漏的 mDNS 实例让后续连接 ICE 协商失败（黑屏）
        tokio::select! {
            msg = socket.next() => {
                let Some(Ok(msg)) = msg else { break };
                match msg {
                    Message::Text(text) => {
                        if first_message {
                            let v: serde_json::Value = match serde_json::from_str(&text) {
                                Ok(v) => v,
                                Err(_) => {
                                    let _ = socket.send(Message::Text(json!({"type": "error", "error": "bad json"}).to_string())).await;
                                    break;
                                }
                            };
                            if v.get("type").and_then(|x| x.as_str()) != Some("offer") {
                                let _ = socket.send(Message::Text(json!({"type": "error", "error": "expected offer"}).to_string())).await;
                                break;
                            }
                            // 多页面接管协商：已有活跃 viewer 且非 force → 不直接顶替，
                            // 回 conflict 让新页面弹窗确认；用户确认后带 force 重发
                            // offer 才真正接管（仅换浏览器↔服务端链路，设备会话不动）。
                            // conflict 不消耗 first_message：下一条消息仍按 offer 解析
                            let force = v.get("force").and_then(|x| x.as_bool()).unwrap_or(false);
                            if !force {
                                let occupied = {
                                    let map = st.viewers.lock().unwrap();
                                    map.get(&device_id)
                                        .map(|h| h.running.load(std::sync::atomic::Ordering::SeqCst))
                                        .unwrap_or(false)
                                };
                                if occupied {
                                    info!(device = %device_id, "ws offer rejected: another viewer active (conflict)");
                                    let _ = socket.send(Message::Text(json!({"type": "conflict"}).to_string())).await;
                                    continue;
                                }
                            }
                            first_message = false;
                            let offer: RTCSessionDescription = match serde_json::from_value(v["sdp"].clone()) {
                                Ok(s) => s,
                                Err(e) => {
                                    let _ = socket.send(Message::Text(json!({"type": "error", "error": format!("bad sdp: {}", e)}).to_string())).await;
                                    break;
                                }
                            };
                            let (frame_q, frame_notify, overflowed) = make_frame_queue(frames_tx.clone());
                            // 音频队列（无音频源时给空 channel：音频 pusher 立即退出）
                            let audio_rx = match &audio_frames_tx {
                                Some(tx) => make_audio_queue(tx.clone()),
                                None => tokio::sync::mpsc::channel(8).1,
                            };
                            // 初始推流帧（SPS/PPS + 最近 GOP）：pusher 先重放，浏览器立即出画面。
                            // 缓存不足（会话刚建立 / MTK 等关键帧稀疏设备，GOP 可能几十秒才更新一次）：
                            //  1. 请求设备重置视频编码（RESET_VIDEO → MediaCodec EOS → 新 SPS/PPS + IDR）；
                            //  2. 轮询等待缓存捕获，最多 ~3s。
                            // 否则新 viewer 拿不到参数集，错过会话开头的 config 帧就永久黑屏。
                            let mut initial_frames = st.devices.frame_cache(&device_id).and_then(|fc| fc.initial_frames());
                            let has_gop = |f: &Option<Vec<VideoFrame>>| f.as_ref().map(|v| v.iter().any(|x| x.is_keyframe)).unwrap_or(false);
                            if !has_gop(&initial_frames) {
                                let _ = session.reset_video().await;
                                for _ in 0..30 {
                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                    let f = st.devices.frame_cache(&device_id).and_then(|fc| fc.initial_frames());
                                    if has_gop(&f) {
                                        initial_frames = f;
                                        break;
                                    }
                                    if f.is_some() {
                                        initial_frames = f; // 至少拿到 SPS/PPS
                                    }
                                }
                                if initial_frames.is_some() {
                                    info!(device = %device_id, "waited for initial frames after reset_video");
                                }
                            }
                            match ViewerSession::create(&st.cfg, session.clone(), frame_q, frame_notify, overflowed, audio_rx, offer, initial_frames).await {
                                Ok(vs) => {
                                    // 单 viewer 限制：同一设备的新连接踢掉旧连接
                                    // （旧 pusher 停止 + 旧 peer 关闭），避免多连接多推流
                                    let handle = ViewerHandle {
                                        running: vs.running.clone(),
                                        peer: std::sync::Arc::downgrade(&vs.peer),
                                        control_dc: vs.control_dc.clone(),
                                        last_serve: vs.last_serve.clone(),
                                        notify: std::sync::Arc::new(parking_lot::Mutex::new(Some(notify_tx.clone()))),
                                    };
                                    let old_pair = {
                                        st.viewers
                                            .lock()
                                            .unwrap()
                                            .insert(device_id.clone(), handle)
                                    };
                                    if let Some(old) = old_pair {
                                        // 先通知旧页面被接管（经其信令 ws 下发，页面收到后不再
                                        // 自动重连），再关 peer；旧 ws 循环在 peer 关闭后有
                                        // 冲刷窗口保证通知先于 close 到达。仅断浏览器↔服务端
                                        // 链路，设备 scrcpy 会话保持不动（新 viewer 无缝接管）
                                        if let Some(tx) = old.notify.lock().take() {
                                            let _ = tx.send(json!({"type": "taken_over"}).to_string());
                                        }
                                        old.running.store(false, std::sync::atomic::Ordering::SeqCst);
                                        if let Some(p) = old.peer.upgrade() {
                                            let _ = p.close().await;
                                        }
                                        info!(device = %device_id, "kicked previous viewer (takeover)");
                                    }
                                    // 消费者出现：打断空闲低功耗计时（镜像模式若已关屏则唤醒）
                                    st.devices.notify_activity(&device_id);
                                    let answer = vs.local_description();
                                    let _ = socket
                                        .send(Message::Text(json!({"type": "answer", "sdp": answer}).to_string()))
                                        .await;
                                    viewer = Some(vs);
                                }
                                Err(e) => {
                                    warn!("webrtc create failed: {}", e);
                                    let _ = socket.send(Message::Text(json!({"type": "error", "error": format!("webrtc: {}", e)}).to_string())).await;
                                    break;
                                }
                            }
                        } else {
                            // 后续消息：控制走 DataChannel，这里仅处理心跳
                            debug!("ws msg: {}", text);
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(p) => {
                        let _ = socket.send(Message::Pong(p)).await;
                    }
                    _ => {}
                }
            }
            _ = async {
                if let Some(rx) = peer_closed_rx.as_mut() {
                    let _ = rx.changed().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                // peer 已死：退出前给 taken_over 通知一个送达窗口（被顶替时页面
                // 先收通知再收 peer close，据此跳过自动重连，防互顶）
                let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
                loop {
                    tokio::select! {
                        msg = notify_rx.recv() => {
                            match msg {
                                Some(m) => { if socket.send(Message::Text(m)).await.is_err() { break } }
                                None => break,
                            }
                        }
                        _ = tokio::time::sleep_until(deadline) => break,
                    }
                }
                debug!("peer closed signal, closing ws loop");
                break;
            }
            Some(text) = notify_rx.recv() => {
                if socket.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
        }
    }

    if let Some(v) = viewer {
        v.running.store(false, std::sync::atomic::Ordering::SeqCst);
        // 只注销自己（若期间已被新连接替换，则注册表里已不是自己）
        // 注意：std::sync::Mutex 不可重入！不能在持锁状态下再次 lock()
        // （旧实现 if-let 匹配的临时 guard 存活到块结束，块内再 lock 自死锁，
        //   worker 线程永久卡死 → 整个服务假死）。检查与 remove 必须分开加锁。
        let is_mine = {
            let map = st.viewers.lock().unwrap();
            map.get(&device_id)
                .map(|h| std::sync::Arc::ptr_eq(&h.running, &v.running))
                .unwrap_or(false)
        };
        if is_mine {
            st.viewers.lock().unwrap().remove(&device_id);
        }
        let _ = v.peer.close().await;
        info!(device = %device_id, "viewer closed");
    }
}
