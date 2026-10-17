use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

/// Created by an authenticated management handler after it checks the current
/// token's control scope and target/Package ranges. Never an MCP start tool.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartRequest {
    pub token_id: String,
    #[serde(default)]
    pub token_label: String,
    pub device_id: String,
    pub content_package: Option<String>,
    #[serde(default = "default_ttl")]
    pub ttl_seconds: u64,
}

pub(crate) fn default_ttl() -> u64 {
    300
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionBinding {
    pub session_id: String,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionPhase {
    Starting,
    Running,
    Pausing,
    Paused,
    Resuming,
    Stopping,
    Finished,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SessionRecord {
    pub session_id: String,
    pub run_id: String,
    pub token_id: String,
    pub token_label: String,
    pub device_id: String,
    pub android_package: Option<String>,
    pub content_package: Option<String>,
    pub state: SessionPhase,
    pub generation: u64,
    pub ttl_seconds: u64,
    pub remaining_seconds: u64,
    pub reason: Option<String>,
    pub manual_allowed: bool,
}

/// Wire coordinates refer to the dimensions returned with the last screenshot.
/// Held input is bounded; no persistent key-down/touch-down tool is exposed.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum InputAction {
    Tap {
        x: f64,
        y: f64,
    },
    Hover {
        x: f64,
        y: f64,
    },
    Press {
        x: f64,
        y: f64,
        duration_ms: u64,
    },
    Swipe {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        duration_ms: u64,
    },
    Key {
        key: String,
        #[serde(default)]
        duration_ms: u64,
    },
    Text {
        text: String,
    },
    AppLaunch,
    AppStop,
}

impl InputAction {
    pub(super) fn validate(&self) -> Result<()> {
        match self {
            Self::Press { duration_ms, .. } => ensure!(
                (60..=3000).contains(duration_ms),
                "invalid_args: press duration must be 60..3000 ms"
            ),
            Self::Swipe { duration_ms, .. } => ensure!(
                (100..=3000).contains(duration_ms),
                "invalid_args: swipe duration must be 100..3000 ms"
            ),
            Self::Key { key, duration_ms } => {
                ensure!(
                    !key.is_empty() && key.len() <= 80 && !key.chars().any(char::is_control),
                    "invalid_args: invalid key name"
                );
                ensure!(
                    *duration_ms <= 3000,
                    "invalid_args: key duration exceeds 3000 ms"
                );
            }
            Self::Text { text } => ensure!(
                text.chars().count() <= 2000,
                "invalid_args: text exceeds 2000 characters"
            ),
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CaptureMetadata {
    pub frame_id: String,
    pub device_id: String,
    pub session_id: Option<String>,
    pub generation: u64,
    pub captured_at: String,
    pub width: u32,
    pub height: u32,
    pub original_width: u32,
    pub original_height: u32,
    pub source: Option<crate::capabilities::FrameStamp>,
    pub image_id: Option<String>,
}

/// The MCP adapter encodes these real PNG bytes as an image content block.
pub(crate) struct Capture {
    pub png: Vec<u8>,
    pub metadata: CaptureMetadata,
}
