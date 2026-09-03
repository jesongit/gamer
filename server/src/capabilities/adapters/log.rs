use crate::store::Db;

use super::super::{CapabilityError, CapabilityResult, LogLevel, LogRecord, LogService};

pub(crate) struct LogAdapter {
    db: Db,
}

impl LogAdapter {
    pub(crate) fn new(db: Db) -> Self {
        Self { db }
    }
}

impl LogService for LogAdapter {
    fn write(&self, record: LogRecord) -> CapabilityResult<()> {
        let device = record
            .device()
            .map(|device| device.id().as_str())
            .unwrap_or_default();
        let level = match record.level() {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        };
        self.db
            .add_log(device, "capability", level, record.message())
            .map_err(|error| CapabilityError::Failed(error.to_string()))
    }
}
