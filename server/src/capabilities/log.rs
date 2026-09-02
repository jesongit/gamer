use super::{CapabilityResult, DeviceHandle, RunHandle};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Small structured log event. Large payloads and backend logger types stay out
/// of the capability boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogRecord {
    level: LogLevel,
    message: String,
    device: Option<DeviceHandle>,
    run: Option<RunHandle>,
}

impl LogRecord {
    pub fn new(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            level,
            message: message.into(),
            device: None,
            run: None,
        }
    }

    pub fn with_device(mut self, device: DeviceHandle) -> Self {
        self.device = Some(device);
        self
    }

    pub fn with_run(mut self, run: RunHandle) -> Self {
        self.run = Some(run);
        self
    }

    pub fn level(&self) -> LogLevel {
        self.level
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn device(&self) -> Option<&DeviceHandle> {
        self.device.as_ref()
    }

    pub fn run(&self) -> Option<RunHandle> {
        self.run
    }
}

/// Structured logging boundary.
pub trait LogService: Send + Sync {
    fn write(&self, record: LogRecord) -> CapabilityResult<()>;
}
