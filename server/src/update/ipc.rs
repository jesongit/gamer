//! Launcher IPC 客户端帧协议（SYS-003 / release/contracts/ipc-v1.md，冻结）。
//!
//! 分层：
//! - **协议纯函数**（[`codec`]）：u32 LE 长度前缀 + UTF-8 JSON 载荷；请求/响应
//!   帧的构造与解析、单帧 1 MiB 上限强制——单测直接驱动，不碰任何 IO；
//! - **传输抽象**（[`FrameTransport`]）：一请求一帧响应。生产实现
//!   [`PipeTransport`]（Windows named pipe 客户端，连接超时 5s、交换超时 30s，
//!   ERROR_PIPE_BUSY 有界重试）；测试用内存 mock / 进程内真 pipe（#[cfg(windows)]）；
//! - **客户端会话**（[`LauncherClient`]）：6 操作枚举 + 受理解析 + 错误帧映射
//!   （业务码 1:1 映射 11 码；协议级码/未知码 → `launcher_unreachable`）。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::model::{UpdateErrorCode, UpdateState};

/// 单帧上限 1 MiB（ipc-v1 §2 冻结）：长度前缀超限立即断开（不读载荷）
pub const MAX_FRAME_BYTES: u32 = 1_048_576;
/// 连接建立超时（建议值 5s）
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 单帧交换超时（建议值 30s；长操作受理时限相同）
pub const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);

/// 内部操作枚举（ipc-v1 §4 冻结 6 个；launcher 永不接受任意字符串）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Status,
    Check,
    Download,
    PrepareInstall,
    Rollback,
    /// 依赖修复编排：server 侧透传（trait 方法保留；修复编排的消费方为
    /// launcher，server 暂无自动调用路径——SYS-003 契约面完整性）
    #[allow(dead_code)]
    RepairDependency(DependencyKind),
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

    /// 除 repair_dependency 外 payload 恒为 `{}`
    pub fn payload(self) -> Value {
        match self {
            Operation::RepairDependency(kind) => json!({ "dependency": kind.as_str() }),
            _ => json!({}),
        }
    }
}

/// repair_dependency 的内部枚举 payload（scrcpy 不可修，随应用版本整体更换）。
/// 取值集为契约冻结；server 侧当前仅透传给 launcher。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    Adb,
    Ffmpeg,
}

impl DependencyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DependencyKind::Adb => "adb",
            DependencyKind::Ffmpeg => "ffmpeg",
        }
    }
}

/// 传输层错误（帧级：连接/超时/超限/JSON 损坏）——统一映射 launcher_unreachable
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// pipe 打不开（launcher 不在场）或中途断开
    Unavailable(String),
    /// 连接/交换超时
    Timeout(&'static str),
    /// 长度前缀超限（ipc-v1 §2：立即断开，不读载荷）
    FrameTooLarge(u32),
    /// 载荷不是合法 UTF-8 JSON / 形态破损
    Malformed(String),
}

impl FrameError {
    pub fn message(&self) -> String {
        match self {
            FrameError::Unavailable(m) => format!("launcher IPC channel unavailable: {m}"),
            FrameError::Timeout(what) => format!("launcher IPC {what} timed out"),
            FrameError::FrameTooLarge(n) => {
                format!("frame length prefix {n} exceeds the 1 MiB limit; connection dropped")
            }
            FrameError::Malformed(m) => format!("launcher IPC frame malformed: {m}"),
        }
    }
}

// ---------------- 协议纯函数（codec） ----------------

/// 构造一帧完整请求字节：u32 LE 前缀 + JSON 载荷。返回 (request_id, bytes)。
pub fn encode_request(token: &str, request_id: &str, operation: Operation) -> Vec<u8> {
    let payload = json!({
        "protocol_version": 1,
        "request_id": request_id,
        "auth": token,
        "operation": operation.as_str(),
        "payload": operation.payload(),
    });
    encode_frame(&payload)
}

/// JSON 载荷 → 帧字节（u32 LE 前缀 + 载荷）
pub fn encode_frame(payload: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(payload).expect("JSON serialization of IPC frame");
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// 从缓冲解析单帧（纯函数：调用方负责按长度前缀切分后传入载荷字节）
pub fn decode_payload(body: &[u8]) -> Result<Value, FrameError> {
    let value: Value =
        serde_json::from_slice(body).map_err(|e| FrameError::Malformed(e.to_string()))?;
    Ok(value)
}

/// 校验长度前缀：超限立即拒绝（ipc-v1 §2 双向适用）
pub fn check_frame_limit(prefix: u32) -> Result<(), FrameError> {
    if prefix > MAX_FRAME_BYTES {
        return Err(FrameError::FrameTooLarge(prefix));
    }
    Ok(())
}

/// Decode one complete IPC response frame.
///
/// `FrameTransport` is deliberately a frame-level abstraction: both the
/// production named-pipe transport and test transports return the four-byte
/// little-endian length prefix followed by exactly one JSON payload. Keeping
/// framing validation here prevents a transport implementation from silently
/// accepting a truncated or concatenated response.
pub fn decode_frame(frame: &[u8]) -> Result<&[u8], FrameError> {
    if frame.len() < 4 {
        return Err(FrameError::Malformed(
            "frame is shorter than its length prefix".into(),
        ));
    }
    let len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
    check_frame_limit(len)?;
    let expected = 4usize.saturating_add(len as usize);
    if frame.len() != expected {
        return Err(FrameError::Malformed(format!(
            "frame length mismatch: prefix declares {len} bytes, received {}",
            frame.len().saturating_sub(4)
        )));
    }
    Ok(&frame[4..])
}

/// 响应帧解析（纯函数）：
/// - `ok:true` → [`IpcOk`]（result）
/// - `ok:false` → [`IpcErr`]（code + message；request_id 无法定位时为空串）
pub fn parse_response(
    value: Value,
    expected_request_id: &str,
) -> Result<Result<Value, IpcErr>, FrameError> {
    #[derive(Deserialize)]
    struct Raw {
        protocol_version: Option<i64>,
        #[serde(default)]
        request_id: String,
        ok: bool,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        code: Option<String>,
        #[serde(default)]
        message: Option<String>,
    }
    if !value.is_object() {
        return Err(FrameError::Malformed("frame is not a JSON object".into()));
    }
    let raw: Raw =
        serde_json::from_value(value).map_err(|e| FrameError::Malformed(e.to_string()))?;
    if raw.protocol_version != Some(1) {
        return Err(FrameError::Malformed("unsupported protocol_version".into()));
    }
    if raw.request_id != expected_request_id {
        return Err(FrameError::Malformed(format!(
            "request_id mismatch: expected {expected_request_id}, got {}",
            raw.request_id
        )));
    }
    if raw.ok {
        return Ok(Ok(raw.result.unwrap_or(Value::Null)));
    }
    let code = raw
        .code
        .ok_or_else(|| FrameError::Malformed("error frame missing code".into()))?;
    Ok(Err(IpcErr {
        request_id: raw.request_id,
        code,
        message: raw.message.unwrap_or_default(),
    }))
}

/// 错误响应帧（ipc-v1 §3.3）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcErr {
    pub request_id: String,
    pub code: String,
    pub message: String,
}

/// 新 request_id（UUID v4，≤64 字符；建议值）
pub fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

// ---------------- 受理与状态结果模型 ----------------

/// 长操作受理 result（ipc-v1 §4.2 冻结）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedResult {
    pub operation: String,
    pub update_id: Option<String>,
    pub state: Option<UpdateState>,
}

pub(crate) fn parse_accepted(
    result: &Value,
    operation: Operation,
) -> Result<AcceptedResult, IpcErr> {
    let ok = result.get("accepted").and_then(Value::as_bool);
    if ok != Some(true) {
        return Err(IpcErr {
            request_id: String::new(),
            code: "internal_error".into(),
            message: "acceptance frame missing accepted:true".into(),
        });
    }
    let op_ok = result
        .get("operation")
        .and_then(Value::as_str)
        .is_some_and(|s| s == operation.as_str());
    if !op_ok {
        return Err(IpcErr {
            request_id: String::new(),
            code: "internal_error".into(),
            message: "acceptance frame operation mismatch".into(),
        });
    }
    Ok(AcceptedResult {
        operation: operation.as_str().to_string(),
        update_id: result
            .get("update_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        state: result
            .get("state")
            .and_then(Value::as_str)
            .and_then(UpdateState::parse),
    })
}

/// `status` result 的 `update` 块（ipc-v1 §4.1；server 据此合成 GET
/// /api/system/update，11 态跨重启稳定）
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LauncherUpdateStatus {
    pub state: Option<UpdateState>,
    pub detail: Option<String>,
    pub update_id: Option<String>,
    pub candidate: Option<Candidate>,
    pub progress: Option<Progress>,
    pub last_error: Option<LastErrorCodeMessage>,
}

/// 候选版本（HTTP 契约 §3 形态；IPC status 只携带 version/channel/published_at）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub version: String,
    pub channel: String,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_notes_url: Option<String>,
}

impl Candidate {
    /// IPC status 的 candidate 块（无 size/release_notes）→ HTTP 形态（缺省
    /// 字段置 null，键集对前端保持稳定）
    pub fn to_http_json(&self) -> Value {
        json!({
            "version": self.version,
            "channel": self.channel,
            "published_at": self.published_at,
            "size_bytes": self.size_bytes,
            "release_notes_url": self.release_notes_url,
        })
    }
}

/// 下载进度（仅 downloading 态非空）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub bytes_done: i64,
    pub bytes_total: i64,
}

/// last_error（code 属 11 个业务错误码）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastErrorCodeMessage {
    pub code: String,
    pub message: String,
}

pub(crate) fn parse_status_update_block(
    result: &Value,
) -> Result<LauncherUpdateStatus, FrameError> {
    // Older launchers may omit the update block while no transaction exists.
    // Treat that shape as the documented idle/default state; a null block has
    // the same meaning.
    let Some(block) = result.get("update") else {
        return Ok(LauncherUpdateStatus::default());
    };
    if block.is_null() {
        return Ok(LauncherUpdateStatus::default());
    }
    let state = block
        .get("state")
        .and_then(Value::as_str)
        .and_then(UpdateState::parse);
    let candidate = match block.get("candidate") {
        None | Some(Value::Null) => None,
        Some(c) => Some(
            serde_json::from_value::<Candidate>(c.clone())
                .map_err(|e| FrameError::Malformed(format!("candidate block: {e}")))?,
        ),
    };
    let progress = match block.get("progress") {
        None | Some(Value::Null) => None,
        Some(p) => Some(
            serde_json::from_value::<Progress>(p.clone())
                .map_err(|e| FrameError::Malformed(format!("progress block: {e}")))?,
        ),
    };
    let last_error = match block.get("last_error") {
        None | Some(Value::Null) => None,
        Some(e) => Some(
            serde_json::from_value::<LastErrorCodeMessage>(e.clone())
                .map_err(|e2| FrameError::Malformed(format!("last_error block: {e2}")))?,
        ),
    };
    Ok(LauncherUpdateStatus {
        state,
        detail: block
            .get("detail")
            .and_then(Value::as_str)
            .map(str::to_string),
        update_id: block
            .get("update_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        candidate,
        progress,
        last_error,
    })
}

/// 业务/协议错误帧 → 统一业务错误码（协议级码与未知码 → launcher_unreachable）
pub fn map_ipc_err(err: IpcErr) -> UpdateErrorCode {
    UpdateErrorCode::from_ipc_frame_code(&err.code)
}

// ---------------- 传输抽象与客户端 ----------------

/// 一请求一帧响应的字节传输（ipc-v1 §1.3：byte-mode；同步请求-响应）
pub trait FrameTransport: Send + Sync {
    /// 发送一帧请求字节并收齐一帧响应字节（含长度前缀）。实现负责超时与
    /// 连接管理；错误统一映射 [`FrameError`]。
    fn exchange(
        &self,
        request: Vec<u8>,
        request_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, FrameError>> + Send + '_>>;
}

/// Launcher 客户端会话：泛型传输，单测注入内存 mock / 真 pipe。
pub struct LauncherClient<T: FrameTransport> {
    transport: T,
    token: String,
}

impl<T: FrameTransport> LauncherClient<T> {
    pub fn new(transport: T, token: String) -> Self {
        Self { transport, token }
    }

    /// 单次交换：构造请求帧 → 传输 → 解析响应帧。传输层/协议层错误统一
    /// 折算 [`FrameError`]（调用方映射 launcher_unreachable）。
    pub async fn call(&self, operation: Operation) -> Result<Result<Value, IpcErr>, FrameError> {
        let request_id = new_request_id();
        let frame = encode_request(&self.token, &request_id, operation);
        let raw = self.transport.exchange(frame, &request_id).await?;
        let value = decode_payload(decode_frame(&raw)?)?;
        parse_response(value, &request_id)
    }

    /// `status`（同步只读操作）
    pub async fn status(&self) -> Result<LauncherUpdateStatus, FrameError> {
        match self.call(Operation::Status).await? {
            Ok(result) => parse_status_update_block(&result),
            Err(err) => Err(FrameError::Malformed(format!(
                "status returned business error {}: {}",
                err.code, err.message
            ))),
        }
    }

    /// 长操作受理（check/download/prepare_install/rollback）：受理即回。
    /// 业务错误帧（`ok:false`，如 `update_busy`）**不是**通道损伤——1:1 映射
    /// 11 个业务错误码回传调用方（ipc-v1 §6.1），绝不折算 launcher_unreachable。
    pub async fn accept(&self, operation: Operation) -> Result<AcceptedResult, AcceptFailure> {
        match self.call(operation).await {
            Err(frame) => Err(AcceptFailure::Transport(frame)),
            Ok(Err(err)) => Err(AcceptFailure::Business(err)),
            Ok(Ok(result)) => parse_accepted(&result, operation).map_err(AcceptFailure::Business),
        }
    }
}

/// 统一业务错误（HTTP API / 协调器共用；details 为契约 §7 冻结键集的 JSON）
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateError {
    pub code: UpdateErrorCode,
    pub message: String,
    pub details: Option<Value>,
}

impl UpdateError {
    pub fn new(code: UpdateErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// 传输层错误 → launcher_unreachable（502，可重试）
    pub fn from_frame(err: FrameError) -> Self {
        let code = match &err {
            // 超限/畸形帧属 IPC 损伤，同为 launcher_unreachable（契约 §7：
            // "launcher named pipe 连接失败/超时/令牌不匹配/IPC 损坏"）
            FrameError::Unavailable(_)
            | FrameError::Timeout(_)
            | FrameError::FrameTooLarge(_)
            | FrameError::Malformed(_) => UpdateErrorCode::LauncherUnreachable,
        };
        Self::new(code, err.message())
    }
}

/// 长操作受理失败（ipc-v1 §6.1/§6.2 两类）：
/// - [`AcceptFailure::Transport`]：连接/帧损伤 → launcher_unreachable（502）；
/// - [`AcceptFailure::Business`]：launcher 业务错误帧 → 1:1 映射 11 码
///   （协议级码/未知码同样归一 launcher_unreachable，见 [`map_ipc_err`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptFailure {
    Transport(FrameError),
    Business(IpcErr),
}

impl AcceptFailure {
    pub fn into_update_error(self, operation: Operation) -> UpdateError {
        match self {
            AcceptFailure::Transport(e) => UpdateError::from_frame(e),
            AcceptFailure::Business(e) => {
                let code = map_ipc_err(e);
                UpdateError::new(
                    code,
                    format!("launcher rejected {} request", operation.as_str()),
                )
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! 内存 mock 传输（单测专用）：按注册的响应帧序列应答，并记录收到的请求帧。

    use std::sync::Mutex;

    use super::*;

    pub struct MockTransport {
        pub handler: Box<dyn Fn(Value) -> Result<Value, FrameError> + Send + Sync>,
        pub seen_requests: Mutex<Vec<Value>>,
    }

    impl MockTransport {
        pub fn new(
            handler: impl Fn(Value) -> Result<Value, FrameError> + Send + Sync + 'static,
        ) -> Self {
            Self {
                handler: Box::new(handler),
                seen_requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl FrameTransport for MockTransport {
        fn exchange(
            &self,
            request: Vec<u8>,
            _request_id: &str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Vec<u8>, FrameError>> + Send + '_>,
        > {
            let this = self;
            Box::pin(async move {
                let payload = decode_payload(decode_frame(&request)?)?;
                this.seen_requests.lock().unwrap().push(payload.clone());
                let response = (this.handler)(payload)?;
                Ok(encode_frame(&response))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frame_roundtrip_with_little_endian_prefix() {
        let payload = json!({"hello": "世界"});
        let frame = encode_frame(&payload);
        let body_len = frame.len() - 4;
        let prefix = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
        assert_eq!(prefix as usize, body_len);
        let value = decode_payload(&frame[4..]).unwrap();
        assert_eq!(value, payload);
    }

    #[test]
    fn frame_limit_is_enforced_before_payload_read() {
        assert!(check_frame_limit(MAX_FRAME_BYTES).is_ok());
        let err = check_frame_limit(MAX_FRAME_BYTES + 1).unwrap_err();
        assert!(matches!(err, FrameError::FrameTooLarge(n) if n == MAX_FRAME_BYTES + 1));
    }

    #[test]
    fn complete_frame_decoder_rejects_truncation_and_concatenation() {
        let frame = encode_frame(&json!({"ok": true}));
        assert_eq!(decode_frame(&frame).unwrap(), &frame[4..]);
        assert!(matches!(
            decode_frame(&frame[..3]),
            Err(FrameError::Malformed(message)) if message.contains("length prefix")
        ));
        assert!(matches!(
            decode_frame(&frame[..frame.len() - 1]),
            Err(FrameError::Malformed(message)) if message.contains("length mismatch")
        ));
        let mut concatenated = frame.clone();
        concatenated.extend_from_slice(&frame);
        assert!(matches!(
            decode_frame(&concatenated),
            Err(FrameError::Malformed(message)) if message.contains("length mismatch")
        ));
    }

    #[test]
    fn request_frame_carries_frozen_fields() {
        let frame = encode_request("tok", "rid-1", Operation::Status);
        let value = decode_payload(&frame[4..]).unwrap();
        assert_eq!(value["protocol_version"], 1);
        assert_eq!(value["request_id"], "rid-1");
        assert_eq!(value["auth"], "tok");
        assert_eq!(value["operation"], "status");
        assert_eq!(value["payload"], json!({}));
        // repair_dependency 的枚举 payload
        let frame = encode_request(
            "tok",
            "rid-2",
            Operation::RepairDependency(DependencyKind::Ffmpeg),
        );
        let value = decode_payload(&frame[4..]).unwrap();
        assert_eq!(value["operation"], "repair_dependency");
        assert_eq!(value["payload"], json!({"dependency": "ffmpeg"}));
    }

    #[test]
    fn response_parsing_accepts_success_and_error_frames() {
        let ok = json!({
            "protocol_version": 1,
            "request_id": "rid-1",
            "ok": true,
            "result": {"accepted": true, "operation": "download"},
        });
        let result = parse_response(ok, "rid-1").unwrap().unwrap();
        assert_eq!(result["accepted"], true);

        let err = json!({
            "protocol_version": 1,
            "request_id": "rid-1",
            "ok": false,
            "code": "signature_invalid",
            "message": "验签失败",
        });
        let err = parse_response(err, "rid-1").unwrap().unwrap_err();
        assert_eq!(err.code, "signature_invalid");
        assert_eq!(map_ipc_err(err), UpdateErrorCode::SignatureInvalid);
    }

    #[test]
    fn response_parsing_rejects_mismatched_frames() {
        let bad_version = json!({"protocol_version": 2, "request_id": "r", "ok": true});
        assert!(parse_response(bad_version, "r").is_err());
        let bad_id = json!({"protocol_version": 1, "request_id": "other", "ok": true});
        assert!(parse_response(bad_id, "r").is_err());
        let not_object = json!([1, 2, 3]);
        assert!(parse_response(not_object, "r").is_err());
        let missing_code = json!({"protocol_version": 1, "request_id": "r", "ok": false});
        assert!(parse_response(missing_code, "r").is_err());
    }

    #[test]
    fn protocol_error_codes_map_to_launcher_unreachable() {
        for code in [
            "unsupported_protocol_version",
            "unknown_operation",
            "invalid_payload",
            "unauthorized",
            "internal_error",
            "something-unknown",
        ] {
            let err = IpcErr {
                request_id: "r".into(),
                code: code.into(),
                message: "x".into(),
            };
            assert_eq!(
                map_ipc_err(err),
                UpdateErrorCode::LauncherUnreachable,
                "{code}"
            );
        }
    }

    #[test]
    fn accepted_result_parsing_follows_ipc_contract() {
        let ok = json!({
            "accepted": true,
            "operation": "download",
            "update_id": "upd-20260831-9f3ab2c1",
            "state": "downloading",
        });
        let accepted = parse_accepted(&ok, Operation::Download).unwrap();
        assert_eq!(accepted.update_id.as_deref(), Some("upd-20260831-9f3ab2c1"));
        assert_eq!(accepted.state, Some(UpdateState::Downloading));

        let missing_accepted = json!({"operation": "download"});
        assert!(parse_accepted(&missing_accepted, Operation::Download).is_err());
        let wrong_op = json!({"accepted": true, "operation": "rollback"});
        assert!(parse_accepted(&wrong_op, Operation::Download).is_err());
    }

    #[test]
    fn status_update_block_parses_all_sections() {
        let result = json!({
            "launcher_version": "0.1.0",
            "installation_id": "a1b2c3d4e5f6a7b8",
            "protocol_version": 1,
            "versions": {"current": "0.2.0", "previous": "0.1.0"},
            "schema": {"db": 1, "file": 1, "rollback_floor": 1},
            "update": {
                "state": "downloading",
                "detail": "downloading",
                "update_id": "upd-20260831-9f3ab2c1",
                "candidate": {"version": "0.3.0", "channel": "stable", "published_at": "2026-09-15T00:00:00Z"},
                "progress": {"bytes_done": 402650112, "bytes_total": 893451200},
                "last_error": null,
            },
            "dependencies": {},
        });
        let block = parse_status_update_block(&result).unwrap();
        assert_eq!(block.state, Some(UpdateState::Downloading));
        assert_eq!(block.detail.as_deref(), Some("downloading"));
        assert_eq!(block.update_id.as_deref(), Some("upd-20260831-9f3ab2c1"));
        assert_eq!(block.candidate.as_ref().unwrap().version, "0.3.0");
        assert_eq!(block.progress.as_ref().unwrap().bytes_done, 402650112);
        assert_eq!(block.last_error, None);

        // 无 update 块 / null 块 → 空状态
        assert_eq!(
            parse_status_update_block(&json!({})).unwrap(),
            LauncherUpdateStatus::default()
        );
        assert_eq!(
            parse_status_update_block(&json!({"update": null})).unwrap(),
            LauncherUpdateStatus::default()
        );

        // candidate/progress/last_error 全量的失败形态
        let failed = json!({"update": {
            "state": "failed",
            "detail": "failed",
            "update_id": "upd-x",
            "candidate": null,
            "progress": null,
            "last_error": {"code": "artifact_invalid", "message": "产物校验失败"},
        }});
        let block = parse_status_update_block(&failed).unwrap();
        assert_eq!(block.state, Some(UpdateState::Failed));
        assert_eq!(block.candidate, None);
        assert_eq!(block.progress, None);
        assert_eq!(block.last_error.unwrap().code, "artifact_invalid");
    }

    #[tokio::test]
    async fn client_end_to_end_with_mock_transport() {
        let transport = test_support::MockTransport::new(|req| {
            assert_eq!(req["operation"], "check");
            assert_eq!(req["protocol_version"], 1);
            Ok(json!({
                "protocol_version": 1,
                "request_id": req["request_id"],
                "ok": true,
                "result": {
                    "accepted": true,
                    "operation": "check",
                    "update_id": "upd-1",
                    "state": "checking",
                },
            }))
        });
        let client = LauncherClient::new(transport, "secret".into());
        let accepted = client.accept(Operation::Check).await.unwrap();
        assert_eq!(accepted.update_id.as_deref(), Some("upd-1"));
        assert_eq!(accepted.state, Some(UpdateState::Checking));
    }

    #[tokio::test]
    async fn client_maps_transport_errors_to_launcher_unreachable() {
        let transport = test_support::MockTransport::new(|_req| {
            Err(FrameError::Unavailable("no pipe instance".into()))
        });
        let client = LauncherClient::new(transport, "secret".into());
        let err = client.status().await.unwrap_err();
        assert!(matches!(err, FrameError::Unavailable(_)));
        let mapped = UpdateError::from_frame(err);
        assert_eq!(mapped.code, UpdateErrorCode::LauncherUnreachable);
    }

    #[tokio::test]
    async fn accept_maps_business_error_frames_one_to_one() {
        // launcher 拒绝（update_busy 业务错误帧）绝不折算 launcher_unreachable，
        // 而是 1:1 映射同一业务错误码（ipc-v1 §6.1）
        let transport = test_support::MockTransport::new(|req| {
            Ok(json!({
                "protocol_version": 1,
                "request_id": req["request_id"],
                "ok": false,
                "code": "update_busy",
                "message": "已有升级事务进行中",
            }))
        });
        let client = LauncherClient::new(transport, "secret".into());
        let failure = client.accept(Operation::PrepareInstall).await.unwrap_err();
        let err = failure.into_update_error(Operation::PrepareInstall);
        assert_eq!(err.code, UpdateErrorCode::UpdateBusy);
        assert_eq!(err.code.http_status(), 409);
    }

    #[tokio::test]
    async fn accept_maps_transport_failure_to_launcher_unreachable() {
        let transport = test_support::MockTransport::new(|_req| {
            Err(FrameError::Unavailable("launcher gone".into()))
        });
        let client = LauncherClient::new(transport, "secret".into());
        let failure = client.accept(Operation::Download).await.unwrap_err();
        let err = failure.into_update_error(Operation::Download);
        assert_eq!(err.code, UpdateErrorCode::LauncherUnreachable);
        assert_eq!(err.code.http_status(), 502);
    }

    #[test]
    fn request_ids_are_uuid_v4_unique_and_bounded() {
        let a = new_request_id();
        let b = new_request_id();
        assert_ne!(a, b);
        assert!(a.len() <= 64);
        assert_eq!(a.matches('-').count(), 4);
    }
}
