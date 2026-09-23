use crate::core::{EventSink, RuntimeEvent, RuntimeEventKind};
use crate::{run_manager::RunManager, store::Db};
use futures_util::future::BoxFuture;
use std::sync::Arc;

/// Persistence precedes optional live delivery, so disconnecting a viewer loses no history.
pub struct JournalEventSink {
    pub db: Db,
    pub runs: Arc<RunManager>,
    pub viewer: Arc<dyn EventSink>,
}

impl EventSink for JournalEventSink {
    fn emit(&self, event: RuntimeEvent) -> BoxFuture<'_, anyhow::Result<()>> {
        Box::pin(async move {
            if !matches!(
                event.kind,
                RuntimeEventKind::Hit { .. } | RuntimeEventKind::Miss { .. }
            ) {
                let run_id = event
                    .trace
                    .as_ref()
                    .and_then(|t| t["run_id"].as_str())
                    .map(str::to_owned)
                    .or_else(|| {
                        self.runs
                            .active_for_device(event.device_id.as_str())
                            .map(|r| r.run_id)
                    });
                if let Some(run_id) = run_id {
                    let mut payload = serde_json::to_value(&event.kind)?;
                    payload["trace"] = event
                        .trace
                        .clone()
                        .unwrap_or_else(|| serde_json::json!({"run_id":run_id}));
                    if let Err(error) = self.db.append_run_event(run_id, payload).await {
                        tracing::error!(%error, "cannot persist run event");
                    }
                }
            }
            self.viewer.emit(event).await
        })
    }
}
