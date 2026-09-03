//! WebRTC adapter for the core runtime event sink.

use futures_util::future::BoxFuture;

use crate::core::{EventSink, RuntimeEvent};

use super::viewer::ViewerMap;

/// Adapts runtime events to the active viewer's control DataChannel.
///
/// The runner only sees `EventSink`; the registry lookup and legacy `{"type":"se"}`
/// wire envelope remain WebRTC concerns.
pub struct ViewerEventSink {
    viewers: ViewerMap,
}

impl ViewerEventSink {
    pub fn new(viewers: ViewerMap) -> Self {
        Self { viewers }
    }
}

impl EventSink for ViewerEventSink {
    fn emit(&self, event: RuntimeEvent) -> BoxFuture<'_, anyhow::Result<()>> {
        let dc = {
            let map = self.viewers.lock().unwrap();
            map.get(event.device_id.as_str())
                .and_then(|handle| handle.control_dc.lock().clone())
        };
        let Some(dc) = dc else {
            return Box::pin(async { Ok(()) });
        };
        Box::pin(async move {
            let mut payload = serde_json::to_value(event.kind)?;
            if let Some(object) = payload.as_object_mut() {
                object.insert("type".into(), serde_json::json!("se"));
            }
            dc.send_text(payload.to_string()).await?;
            Ok(())
        })
    }
}
