//! Update status and HTTP error values. No named-pipe transport.
use super::model::{UpdateErrorCode, UpdateState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub enum DependencyKind {
    Adb,
    Ffmpeg,
}
#[derive(Debug)]
pub struct FrameError(String);
impl FrameError {
    #[allow(non_snake_case)]
    fn Malformed(message: String) -> Self {
        Self(message)
    }
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateStatus {
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

pub(crate) fn parse_status_update_block(result: &Value) -> Result<UpdateStatus, FrameError> {
    // Older launchers may omit the update block while no transaction exists.
    // Treat that shape as the documented idle/default state; a null block has
    // the same meaning.
    let Some(block) = result.get("update") else {
        return Ok(UpdateStatus::default());
    };
    if block.is_null() {
        return Ok(UpdateStatus::default());
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
    Ok(UpdateStatus {
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
    pub fn from_frame(err: FrameError) -> Self {
        Self::new(UpdateErrorCode::UpdaterUnavailable, err.0)
    }
}
