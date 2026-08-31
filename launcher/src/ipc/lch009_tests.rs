//! LCH-009 executable contract tests.
//!
//! The byte-mode connection tests use `tokio::io::duplex`, so the framing and
//! lifecycle contract runs on non-Windows CI as well as on Windows. The
//! production listener still uses the Windows named-pipe implementation.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{duplex, AsyncWriteExt};

use crate::ipc::frames::{
    check_request_id_unique, encode_frame, encode_frame_limited, parse_request, success_frame,
    FrameError, ProtocolError,
};
use crate::ipc::server::{handle_connection, read_frame_async, IpcServerConfig};
use crate::ipc::{Dispatcher, FRAME_TIMEOUT, MAX_FRAME_BYTES, PROTOCOL_VERSION};
use crate::layout::InstallLayout;
use crate::upgrade::engine::{ManifestSource, UpgradeOptions};

const TOKEN: &str = "lch009-test-token";

fn request_value(
    request_id: &str,
    protocol_version: u32,
    operation: &str,
    payload: Value,
) -> Value {
    json!({
        "protocol_version": protocol_version,
        "request_id": request_id,
        "auth": TOKEN,
        "operation": operation,
        "payload": payload,
    })
}

fn request_bytes(
    request_id: &str,
    protocol_version: u32,
    operation: &str,
    payload: Value,
) -> Vec<u8> {
    request_value(request_id, protocol_version, operation, payload)
        .to_string()
        .into_bytes()
}

fn test_dispatcher(tag: &str, run_inline: bool) -> (Arc<Dispatcher>, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "gamer-launcher-lch009-{tag}-{}-{}",
        std::process::id(),
        crate::state::atomic::now_unix_millis()
    ));
    fs::create_dir_all(&root).expect("创建 LCH-009 临时安装根");
    let dispatcher = Dispatcher::new(
        InstallLayout { root: root.clone() },
        "lch009-test-installation".to_string(),
        ManifestSource::None,
        root.join("keys"),
        UpgradeOptions::default(),
        run_inline,
    );
    (dispatcher, root)
}

fn mock_config(frame_limit: usize, frame_timeout: Duration) -> IpcServerConfig {
    IpcServerConfig {
        pipe_name: "mock://lch009".to_string(),
        token: TOKEN.to_string(),
        frame_limit,
        frame_timeout,
    }
}

async fn assert_mock_connection_closes_after_input(
    input: Vec<u8>,
    cfg: IpcServerConfig,
    dispatcher: Arc<Dispatcher>,
) {
    let (mut client, server) = duplex(64 * 1024);
    let server_task = tokio::spawn(handle_connection(server, dispatcher, cfg));
    client.write_all(&input).await.expect("mock 客户端写入输入");
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        read_frame_async(&mut client, MAX_FRAME_BYTES),
    )
    .await
    .expect("服务端应在测试超时前关闭 mock 连接");
    assert!(
        matches!(result, Err(FrameError::UnexpectedEof)),
        "连接关闭前不应收到响应帧，实际 {result:?}"
    );
    server_task.await.expect("mock IPC 服务任务不应 panic");
}

#[test]
fn lch009_protocol_version_and_request_id_are_strict() {
    let valid_id = "r".repeat(64);
    let raw = request_bytes(&valid_id, PROTOCOL_VERSION, "status", json!({}));
    let parsed = parse_request(&raw, TOKEN).expect("64 字符 request_id 应可接受");
    assert_eq!(parsed.request_id, valid_id);

    let mut bad_version = request_value("version-rid", PROTOCOL_VERSION + 1, "status", json!({}));
    let (error, request_id) = parse_request(bad_version.to_string().as_bytes(), TOKEN).unwrap_err();
    assert_eq!(error, ProtocolError::UnsupportedProtocolVersion);
    assert!(error.must_disconnect());
    assert_eq!(request_id, "version-rid");

    bad_version["protocol_version"] = json!(PROTOCOL_VERSION);
    bad_version["request_id"] = json!("r".repeat(65));
    let (error, request_id) = parse_request(bad_version.to_string().as_bytes(), TOKEN).unwrap_err();
    assert_eq!(error, ProtocolError::InvalidPayload);
    assert_eq!(request_id, "");

    let mut seen = HashSet::new();
    check_request_id_unique(&mut seen, "same-rid").expect("首次 request_id 应可接受");
    assert_eq!(
        check_request_id_unique(&mut seen, "same-rid"),
        Err(ProtocolError::InvalidPayload),
        "同一连接内重复 request_id 必须拒绝"
    );
}

#[test]
fn lch009_duplicate_request_id_replays_cached_long_operation_after_reconnect() {
    let (dispatcher, root) = test_dispatcher("idempotency", true);
    let raw = request_bytes(
        "retry-rid",
        PROTOCOL_VERSION,
        "repair_dependency",
        json!({"dependency": "adb"}),
    );

    // `handle` represents separate connections: the connection-local seen set
    // is new, while the dispatcher-wide cache must replay the first reply.
    let first = dispatcher.handle(&raw, TOKEN);
    let replay = dispatcher.handle(&raw, TOKEN);
    assert_eq!(first.frame, replay.frame, "重连重发必须返回原受理帧");
    assert!(!first.disconnect && !replay.disconnect);
    assert_eq!(first.frame["request_id"], "retry-rid");
    assert_eq!(first.frame["result"]["accepted"], true);

    drop(dispatcher);
    fs::remove_dir_all(root).expect("清理幂等测试临时安装根");
}

#[test]
fn lch009_duplicate_request_id_is_rejected_within_one_connection() {
    let (dispatcher, root) = test_dispatcher("connection-dedup", true);
    let raw = request_bytes(
        "same-connection-rid",
        PROTOCOL_VERSION,
        "repair_dependency",
        json!({"dependency": "adb"}),
    );
    let mut seen = HashSet::new();
    let first = dispatcher.handle_with_seen(&raw, TOKEN, &mut seen);
    let duplicate = dispatcher.handle_with_seen(&raw, TOKEN, &mut seen);

    assert_eq!(first.frame["ok"], true);
    assert_eq!(duplicate.frame["ok"], false);
    assert_eq!(duplicate.frame["code"], "invalid_payload");
    assert!(!duplicate.disconnect);

    drop(dispatcher);
    fs::remove_dir_all(root).expect("清理连接内重复测试临时安装根");
}

#[test]
fn lch009_frame_limit_accepts_exact_payload_and_rejects_one_byte_over() {
    let exact = vec![b'x'; 32];
    let encoded = encode_frame_limited(&exact, exact.len()).expect("恰好达到上限应可编码");
    assert_eq!(
        u32::from_le_bytes(encoded[..4].try_into().unwrap()),
        exact.len() as u32
    );

    assert!(matches!(
        encode_frame_limited(&vec![b'x'; exact.len() + 1], exact.len()),
        Err(FrameError::Oversize(size)) if size == exact.len() + 1
    ));
}

#[tokio::test]
async fn lch009_mock_transport_round_trip_echoes_protocol_and_request_id() {
    let (dispatcher, root) = test_dispatcher("round-trip", true);
    let raw = request_bytes("round-trip-rid", PROTOCOL_VERSION, "status", json!({}));
    let (mut client, server) = duplex(64 * 1024);
    let server_task = tokio::spawn(handle_connection(
        server,
        dispatcher.clone(),
        mock_config(MAX_FRAME_BYTES, FRAME_TIMEOUT),
    ));

    client.write_all(&encode_frame(&raw)).await.unwrap();
    let response = read_frame_async(&mut client, MAX_FRAME_BYTES)
        .await
        .expect("mock IPC 应返回 status 响应");
    let response: Value = serde_json::from_slice(&response).expect("响应应为 JSON");
    assert_eq!(response["protocol_version"], PROTOCOL_VERSION);
    assert_eq!(response["request_id"], "round-trip-rid");
    assert_eq!(response["ok"], true);
    assert!(response["result"].is_object());

    client.shutdown().await.unwrap();
    server_task.await.unwrap();
    drop(dispatcher);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn lch009_mock_protocol_error_replies_then_disconnects() {
    let (dispatcher, root) = test_dispatcher("protocol-error", true);
    let raw = request_bytes("bad-version-rid", PROTOCOL_VERSION + 1, "status", json!({}));
    let (mut client, server) = duplex(64 * 1024);
    let server_task = tokio::spawn(handle_connection(
        server,
        dispatcher.clone(),
        mock_config(MAX_FRAME_BYTES, FRAME_TIMEOUT),
    ));

    client.write_all(&encode_frame(&raw)).await.unwrap();
    let response = read_frame_async(&mut client, MAX_FRAME_BYTES)
        .await
        .unwrap();
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["code"], "unsupported_protocol_version");
    assert_eq!(response["request_id"], "bad-version-rid");

    let closed = tokio::time::timeout(
        Duration::from_secs(2),
        read_frame_async(&mut client, MAX_FRAME_BYTES),
    )
    .await
    .unwrap();
    assert!(matches!(closed, Err(FrameError::UnexpectedEof)));
    server_task.await.unwrap();
    drop(dispatcher);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn lch009_mock_oversize_request_disconnects_without_response() {
    let (dispatcher, root) = test_dispatcher("oversize", true);
    let limit = 64;
    let mut input = ((limit + 1) as u32).to_le_bytes().to_vec();
    input.extend_from_slice(b"payload-is-not-read");
    assert_mock_connection_closes_after_input(input, mock_config(limit, FRAME_TIMEOUT), dispatcher)
        .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn lch009_mock_oversize_response_disconnects_without_response() {
    let (dispatcher, root) = test_dispatcher("response-oversize", true);
    let raw = request_bytes("response-limit-rid", PROTOCOL_VERSION, "status", json!({}));
    let expected = success_frame("response-limit-rid", dispatcher.status_result()).to_string();
    assert!(expected.len() > raw.len(), "status 响应应大于请求夹具");
    assert_mock_connection_closes_after_input(
        encode_frame(&raw),
        mock_config(raw.len(), FRAME_TIMEOUT),
        dispatcher,
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn lch009_mock_read_timeout_closes_without_response() {
    let (dispatcher, root) = test_dispatcher("timeout", true);
    let (mut client, server) = duplex(64 * 1024);
    let server_task = tokio::spawn(handle_connection(
        server,
        dispatcher,
        mock_config(MAX_FRAME_BYTES, Duration::from_millis(40)),
    ));

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        read_frame_async(&mut client, MAX_FRAME_BYTES),
    )
    .await
    .expect("读超时测试不应卡住");
    assert!(matches!(result, Err(FrameError::UnexpectedEof)));
    server_task.await.unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn lch009_mock_truncated_prefix_closes_without_response() {
    let (dispatcher, root) = test_dispatcher("truncated-prefix", true);
    let (mut client, server) = duplex(64 * 1024);
    let server_task = tokio::spawn(handle_connection(
        server,
        dispatcher,
        mock_config(MAX_FRAME_BYTES, FRAME_TIMEOUT),
    ));

    client.write_all(&[0x05]).await.unwrap();
    client.shutdown().await.unwrap();
    let result = read_frame_async(&mut client, MAX_FRAME_BYTES).await;
    assert!(matches!(result, Err(FrameError::UnexpectedEof)));
    server_task.await.unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn lch009_mock_truncated_payload_closes_without_response() {
    let (dispatcher, root) = test_dispatcher("truncated-payload", true);
    let (mut client, server) = duplex(64 * 1024);
    let server_task = tokio::spawn(handle_connection(
        server,
        dispatcher,
        mock_config(MAX_FRAME_BYTES, FRAME_TIMEOUT),
    ));

    let mut input = 5u32.to_le_bytes().to_vec();
    input.extend_from_slice(b"ab");
    client.write_all(&input).await.unwrap();
    client.shutdown().await.unwrap();
    let result = read_frame_async(&mut client, MAX_FRAME_BYTES).await;
    assert!(matches!(result, Err(FrameError::UnexpectedEof)));
    server_task.await.unwrap();
    fs::remove_dir_all(root).unwrap();
}
