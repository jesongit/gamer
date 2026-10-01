use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::super::{CapabilityError, CapabilityResult, RuntimeService};

pub(crate) struct RuntimeAdapter {
    cancelled: Arc<AtomicBool>,
}

impl RuntimeAdapter {
    pub(crate) fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled }
    }
}

#[async_trait]
impl RuntimeService for RuntimeAdapter {
    async fn sleep(&self, duration: Duration) -> CapabilityResult<()> {
        if self.cancelled() {
            return Err(CapabilityError::Cancelled);
        }
        tokio::time::sleep(duration).await;
        if self.cancelled() {
            Err(CapabilityError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}
