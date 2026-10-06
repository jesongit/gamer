use std::time::Duration;

use async_trait::async_trait;

use super::CapabilityResult;

/// Scheduler/engine-independent timing and cancellation boundary.
#[async_trait]
pub trait RuntimeService: Send + Sync {
    /// Monotonic runtime time; replay overrides this together with sleep.
    fn now_ms(&self) -> u64 {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_millis() as u64
    }

    async fn sleep(&self, duration: Duration) -> CapabilityResult<()>;

    fn cancelled(&self) -> bool;
}
