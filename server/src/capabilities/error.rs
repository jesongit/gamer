use thiserror::Error;

/// Errors shared by all capability adapters.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CapabilityError {
    #[error("capability is unavailable: {0}")]
    Unavailable(&'static str),
    #[error("invalid capability request: {0}")]
    InvalidRequest(String),
    #[error("capability resource was not found: {0}")]
    NotFound(String),
    #[error("capability operation was cancelled")]
    Cancelled,
    #[error("capability operation failed: {0}")]
    Failed(String),
}

pub type CapabilityResult<T> = Result<T, CapabilityError>;
