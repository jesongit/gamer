//! IPC 帧格式与请求/响应类型（ipc-v1.md §2/§3 冻结）。
//!
//! 帧 = u32 little-endian 长度前缀（= JSON 载荷精确字节数）+ UTF-8 JSON。
//! 收到超过上限的长度前缀：立即断开、不回错误帧、不读取载荷。

use std::collections::HashSet;
use std::io::Read;

use serde::Deserialize;
use serde_json::Value;

use super::MAX_FRAME_BYTES;

/// 长度前缀字节数。
pub const PREFIX_LEN: usize = 4;

#[derive(Debug)]
pub enum FrameError {
    /// 长度前缀超上限（接收方立即断开，不回帧）。
    Oversize(usize),
    UnexpectedEof,
    Io(std::io::Error),
}

/// 编码一帧（u32 LE 前缀 + 载荷）。
pub fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + PREFIX_LEN);
    let len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Encode a frame while enforcing the configured wire-size limit.
pub fn encode_frame_limited(payload: &[u8], max: usize) -> Result<Vec<u8>, FrameError> {
    if payload.len() > max {
        return Err(FrameError::Oversize(payload.len()));
    }
    Ok(encode_frame(payload))
}

/// 同步读一帧（测试/工具用；服务端走 tokio 版）。
pub fn read_frame_sync<R: Read>(reader: &mut R, max: usize) -> Result<Vec<u8>, FrameError> {
    let mut prefix = [0u8; PREFIX_LEN];
    read_exact_or_eof(reader, &mut prefix)?;
    let len = u32::from_le_bytes(prefix) as usize;
    if len > max {
        return Err(FrameError::Oversize(len));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).map_err(FrameError::Io)?;
    Ok(payload)
}

fn read_exact_or_eof<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<(), FrameError> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => return Err(FrameError::UnexpectedEof),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(FrameError::Io(e)),
        }
    }
    Ok(())
}

/// 请求帧（ipc-v1 §3.1）。payload 逐操作严格校验（多余字段拒绝 → invalid_payload）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestFrame {
    pub request_id: String,
    pub auth: String,
    pub operation: Operation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Status,
    Check,
    Download,
    PrepareInstall,
    Rollback,
    RepairDependency(Dependency),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dependency {
    Adb,
    Ffmpeg,
}

impl Dependency {
    pub fn as_str(self) -> &'static str {
        match self {
            Dependency::Adb => "adb",
            Dependency::Ffmpeg => "ffmpeg",
        }
    }
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Operation::Status => "status",
            Operation::Check => "check",
            Operation::Download => "download",
            Operation::PrepareInstall => "prepare_install",
            Operation::Rollback => "rollback",
            Operation::RepairDependency(_) => "repair_dependency",
        }
    }
}

/// 操作枚举解析（ipc-v1 §4 冻结 6 个）；未知 → unknown_operation。
pub fn parse_operation(name: &str) -> Option<Operation> {
    match name {
        "status" => Some(Operation::Status),
        "check" => Some(Operation::Check),
        "download" => Some(Operation::Download),
        "prepare_install" => Some(Operation::PrepareInstall),
        "rollback" => Some(Operation::Rollback),
        "repair_dependency" => Some(Operation::RepairDependency(Dependency::Adb)), // 需 payload 再定
        _ => None,
    }
}

fn is_empty_object(value: &Value) -> bool {
    value.as_object().is_some_and(|m| m.is_empty())
}

/// 解析请求帧：JSON 值 → 严格校验（协议版本 / auth 由 server 层比对，这里只
/// 解析结构与 payload）。失败返回协议级错误码 + request_id（可定位时）。
pub fn parse_request(
    payload: &[u8],
    expected_token: &str,
) -> Result<RequestFrame, (ProtocolError, String)> {
    let value: Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(_) => return Err((ProtocolError::InvalidPayload, String::new())),
    };
    let Some(object) = value.as_object() else {
        return Err((ProtocolError::InvalidPayload, String::new()));
    };
    // Keep the wire contract closed: accepting a future/attacker-controlled
    // top-level field would make request semantics depend on version drift.
    const FIELDS: [&str; 5] = [
        "protocol_version",
        "request_id",
        "auth",
        "operation",
        "payload",
    ];
    if object.len() != FIELDS.len() || object.keys().any(|key| !FIELDS.contains(&key.as_str())) {
        let request_id = object
            .get("request_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.chars().count() <= 64)
            .unwrap_or_default();
        return Err((ProtocolError::InvalidPayload, request_id.to_string()));
    }
    let request_id = value
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let request_id = if request_id.is_empty() || request_id.chars().count() > 64 {
        String::new()
    } else {
        request_id
    };
    // 协议版本
    let version = value.get("protocol_version").and_then(Value::as_u64);
    if version != Some(u64::from(super::PROTOCOL_VERSION)) {
        return Err((ProtocolError::UnsupportedProtocolVersion, request_id));
    }
    // 令牌（逐帧校验；不匹配 → unauthorized + 断开，request_id 置空）
    let auth = value
        .get("auth")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if expected_token.is_empty() || auth.is_empty() || auth != expected_token {
        return Err((ProtocolError::Unauthorized, String::new()));
    }
    let op_name = value
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(base) = parse_operation(op_name) else {
        return Err((ProtocolError::UnknownOperation, request_id));
    };
    let Some(payload_value) = value.get("payload").cloned() else {
        return Err((ProtocolError::InvalidPayload, request_id));
    };
    let operation = match base {
        Operation::RepairDependency(Dependency::Adb) => {
            let parsed: Result<DependencyPayload, _> =
                serde_json::from_value(payload_value.clone());
            match parsed.ok().and_then(|p| parse_dependency(&p.dependency)) {
                Some(dep) => Operation::RepairDependency(dep),
                None => return Err((ProtocolError::InvalidPayload, request_id)),
            }
        }
        other => {
            if !is_empty_object(&payload_value) {
                return Err((ProtocolError::InvalidPayload, request_id));
            }
            other
        }
    };
    // request_id 非空（幂等键）
    if request_id.is_empty() {
        return Err((ProtocolError::InvalidPayload, String::new()));
    }
    Ok(RequestFrame {
        request_id,
        auth: auth.to_string(),
        operation,
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DependencyPayload {
    dependency: String,
}

fn parse_dependency(name: &str) -> Option<Dependency> {
    match name {
        "adb" => Some(Dependency::Adb),
        "ffmpeg" => Some(Dependency::Ffmpeg),
        _ => None,
    }
}

/// 协议级错误码（ipc-v1 §6.2，仅 IPC）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    UnsupportedProtocolVersion,
    UnknownOperation,
    InvalidPayload,
    Unauthorized,
}

impl ProtocolError {
    pub fn code(self) -> &'static str {
        match self {
            ProtocolError::UnsupportedProtocolVersion => "unsupported_protocol_version",
            ProtocolError::UnknownOperation => "unknown_operation",
            ProtocolError::InvalidPayload => "invalid_payload",
            ProtocolError::Unauthorized => "unauthorized",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            ProtocolError::UnsupportedProtocolVersion => {
                "不支持的 IPC 协议版本，本 launcher 仅支持 protocol_version=1"
            }
            ProtocolError::UnknownOperation => {
                "未知操作，接受的操作枚举为 status/check/download/prepare_install/rollback/repair_dependency"
            }
            ProtocolError::InvalidPayload => "payload 不符合该操作的契约，已拒绝",
            ProtocolError::Unauthorized => "IPC 会话令牌校验失败，连接已拒绝",
        }
    }

    /// 是否在回错误帧后立即断开（ipc-v1 §6.2 处置列）。
    pub fn must_disconnect(self) -> bool {
        matches!(
            self,
            ProtocolError::UnsupportedProtocolVersion | ProtocolError::Unauthorized
        )
    }
}

/// 成功响应帧（ipc-v1 §3.2）。
pub fn success_frame(request_id: &str, result: Value) -> Value {
    serde_json::json!({
        "protocol_version": super::PROTOCOL_VERSION,
        "request_id": request_id,
        "ok": true,
        "result": result,
    })
}

/// 错误响应帧（ipc-v1 §3.3 冻结形态：恰五字段）。
pub fn error_frame(request_id: &str, code: &str, message: &str) -> Value {
    serde_json::json!({
        "protocol_version": super::PROTOCOL_VERSION,
        "request_id": request_id,
        "ok": false,
        "code": code,
        "message": message,
    })
}

/// 单帧上限的便捷重导出。
pub fn frame_limit() -> usize {
    MAX_FRAME_BYTES
}

/// Track request ids already used on one byte-mode connection. A reconnect
/// may reuse the id for cross-connection idempotency.
pub fn check_request_id_unique(
    seen: &mut HashSet<String>,
    request_id: &str,
) -> Result<(), ProtocolError> {
    if !seen.insert(request_id.to_string()) {
        return Err(ProtocolError::InvalidPayload);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frame_json(value: &Value) -> Vec<u8> {
        value.to_string().into_bytes()
    }

    fn valid_request(operation: &str, payload: Value) -> Value {
        serde_json::json!({
            "protocol_version": 1,
            "request_id": "0b7f8c1e-5b6a-4a57-9a2e-2f1c3d4b5a6f",
            "auth": "tok",
            "operation": operation,
            "payload": payload,
        })
    }

    #[test]
    fn round_trip_frame_encoding() {
        let payload = br#"{"a":1}"#;
        let frame = encode_frame(payload);
        assert_eq!(&frame[..4], &7u32.to_le_bytes());
        let decoded = read_frame_sync(&mut Cursor::new(frame), MAX_FRAME_BYTES).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn oversize_prefix_is_rejected_without_reading_payload() {
        let mut frame = Vec::new();
        frame.extend_from_slice(&(MAX_FRAME_BYTES as u32 + 1).to_le_bytes());
        frame.extend_from_slice(b"should-not-be-read");
        match read_frame_sync(&mut Cursor::new(frame), MAX_FRAME_BYTES) {
            Err(FrameError::Oversize(n)) => assert_eq!(n, MAX_FRAME_BYTES + 1),
            other => panic!("应报 Oversize，实际 {other:?}"),
        }
    }

    #[test]
    fn encode_frame_limited_rejects_oversize_payload() {
        let payload = vec![b'x'; MAX_FRAME_BYTES + 1];
        match encode_frame_limited(&payload, MAX_FRAME_BYTES) {
            Err(FrameError::Oversize(size)) => assert_eq!(size, MAX_FRAME_BYTES + 1),
            other => panic!("出站超大帧应在编码前拒绝，实际 {other:?}"),
        }
    }

    #[test]
    fn truncated_frame_is_unexpected_eof() {
        let mut frame = encode_frame(b"12345");
        frame.pop();
        match read_frame_sync(&mut Cursor::new(frame), MAX_FRAME_BYTES) {
            Err(FrameError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof),
            other => panic!("应报 Io(UnexpectedEof)，实际 {other:?}"),
        }
    }

    #[test]
    fn parses_all_fixtures_operation_frames() {
        // 夹具（release/contracts/fixtures/ipc/）的请求帧形态必须全部可解析
        for (name, payload) in [
            ("status", serde_json::json!({})),
            ("check", serde_json::json!({})),
            ("download", serde_json::json!({})),
            ("prepare_install", serde_json::json!({})),
            ("rollback", serde_json::json!({})),
        ] {
            let parsed = parse_request(frame_json(&valid_request(name, payload)).as_slice(), "tok")
                .unwrap_or_else(|e| panic!("{name} 应可解析: {e:?}"));
            assert_eq!(parsed.request_id, "0b7f8c1e-5b6a-4a57-9a2e-2f1c3d4b5a6f");
            assert_eq!(parsed.operation.as_str(), name);
        }
        let repair = parse_request(
            frame_json(&valid_request(
                "repair_dependency",
                serde_json::json!({"dependency": "adb"}),
            ))
            .as_slice(),
            "tok",
        )
        .unwrap();
        assert_eq!(
            repair.operation,
            Operation::RepairDependency(Dependency::Adb)
        );
    }

    #[test]
    fn protocol_errors_are_classified() {
        let mut bad = valid_request("status", serde_json::json!({}));
        bad["protocol_version"] = serde_json::json!(2);
        let (err, rid) = parse_request(frame_json(&bad).as_slice(), "tok").unwrap_err();
        assert_eq!(err, ProtocolError::UnsupportedProtocolVersion);
        assert!(err.must_disconnect());
        assert_eq!(rid, "0b7f8c1e-5b6a-4a57-9a2e-2f1c3d4b5a6f");

        let (err, rid) = parse_request(
            frame_json(&valid_request("evil", serde_json::json!({}))).as_slice(),
            "tok",
        )
        .unwrap_err();
        assert_eq!(err, ProtocolError::UnknownOperation);
        assert!(!err.must_disconnect());
        assert!(!rid.is_empty());

        let mut extra = valid_request("status", serde_json::json!({"channel": "stable"}));
        extra["payload"] = serde_json::json!({"channel": "stable"});
        let (err, _) = parse_request(frame_json(&extra).as_slice(), "tok").unwrap_err();
        assert_eq!(err, ProtocolError::InvalidPayload);
        assert!(!err.must_disconnect());

        let (err, rid) = parse_request(
            frame_json(&valid_request("status", serde_json::json!({}))).as_slice(),
            "other",
        )
        .unwrap_err();
        assert_eq!(err, ProtocolError::Unauthorized);
        assert!(err.must_disconnect());
        assert_eq!(rid, "", "令牌不匹配时 request_id 置空");
    }

    #[test]
    fn repair_dependency_rejects_non_enum_values() {
        let (err, _) = parse_request(
            frame_json(&valid_request(
                "repair_dependency",
                serde_json::json!({"dependency": "scrcpy"}),
            ))
            .as_slice(),
            "tok",
        )
        .unwrap_err();
        assert_eq!(err, ProtocolError::InvalidPayload);
        // 多余字段
        let (err, _) = parse_request(
            frame_json(&valid_request(
                "repair_dependency",
                serde_json::json!({"dependency": "adb", "cmd": "rm -rf"}),
            ))
            .as_slice(),
            "tok",
        )
        .unwrap_err();
        assert_eq!(err, ProtocolError::InvalidPayload);
    }

    #[test]
    fn frames_shapes_are_frozen() {
        let ok = success_frame("rid", serde_json::json!({"accepted": true}));
        assert_eq!(ok["protocol_version"], 1);
        assert_eq!(ok["request_id"], "rid");
        assert_eq!(ok["ok"], true);
        assert_eq!(ok.as_object().unwrap().len(), 4);
        let err = error_frame("", "unauthorized", "x");
        assert_eq!(err.as_object().unwrap().len(), 5, "错误帧恰五字段");
        assert_eq!(err["ok"], false);
    }
}
