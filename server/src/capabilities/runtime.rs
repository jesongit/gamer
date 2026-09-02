use std::time::Duration;

use async_trait::async_trait;

use super::CapabilityResult;

/// Scheduler/engine-independent timing and cancellation boundary.
#[async_trait]
pub trait RuntimeService: Send + Sync {
    async fn sleep(&self, duration: Duration) -> CapabilityResult<()>;

    fn cancelled(&self) -> bool;
}
