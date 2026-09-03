use async_trait::async_trait;

use super::super::{
    CapabilityError, CapabilityResult, RunHandle, RunRequest, RunService, RunStatus,
};

/// Phase 2's RunRequest does not yet carry runner/source/payload semantics, so
/// native RunManager submission is intentionally not guessed here. Keeping the
/// adapter explicit makes an incomplete migration observable to tests and to a
/// future WASM host.
pub(crate) struct RunAdapter;

#[async_trait]
impl RunService for RunAdapter {
    async fn submit(&self, _request: RunRequest) -> CapabilityResult<RunHandle> {
        Err(CapabilityError::Unavailable(
            "run adapter requires the finalized Phase 2 RunRequest mapping",
        ))
    }

    async fn cancel(&self, _run: RunHandle) -> CapabilityResult<()> {
        Err(CapabilityError::Unavailable(
            "run adapter requires the finalized Phase 2 RunRequest mapping",
        ))
    }

    async fn status(&self, _run: RunHandle) -> CapabilityResult<RunStatus> {
        Err(CapabilityError::Unavailable(
            "run adapter requires the finalized Phase 2 RunRequest mapping",
        ))
    }
}
