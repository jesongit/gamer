//! Runtime event boundary shared by runners and output adapters.

use futures_util::future::BoxFuture;
use serde::Serialize;

use super::DeviceId;

/// Wire-neutral runtime event payload.  The device scope stays outside the
/// browser-facing event shape and is consumed by the selected sink adapter.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum RuntimeEventKind {
    Tap {
        x: u32,
        y: u32,
    },
    Swipe {
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
    },
    Hit {
        tpl: String,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        score: f32,
    },
    Miss {
        tpl: String,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    },
}

#[derive(Clone, Debug)]
pub struct RuntimeEvent {
    pub device_id: DeviceId,
    pub kind: RuntimeEventKind,
}

impl RuntimeEvent {
    pub fn new(device_id: DeviceId, kind: RuntimeEventKind) -> Self {
        Self { device_id, kind }
    }
}

/// Thin output seam.  Adapters choose whether to send to WebRTC, logs, a
/// websocket, or nowhere; the runner does not know the destination registry.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: RuntimeEvent) -> BoxFuture<'_, anyhow::Result<()>>;
}

/// 测试 / 无事件订阅装配用的空事件汇（engine 测试 Rig 兜底）。
#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct NullEventSink;

impl EventSink for NullEventSink {
    fn emit(&self, _event: RuntimeEvent) -> BoxFuture<'_, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;

    #[test]
    fn runtime_event_keeps_device_scope_out_of_wire_kind() {
        let event = RuntimeEvent::new(
            DeviceId::new("d1").unwrap(),
            RuntimeEventKind::Tap { x: 1, y: 2 },
        );
        assert_eq!(
            serde_json::to_value(&event.kind).unwrap(),
            serde_json::json!({
                "ev": "tap",
                "x": 1,
                "y": 2,
            })
        );
        NullEventSink.emit(event).now_or_never().unwrap().unwrap();
    }
}
