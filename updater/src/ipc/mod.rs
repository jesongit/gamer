pub mod dispatch;
pub mod frames;
pub use dispatch::Dispatcher;
pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
pub const DEDUP_WINDOW: std::time::Duration = std::time::Duration::from_secs(600);
