use async_trait::async_trait;
use uuid::Uuid;

use super::CapabilityResult;

/// Device-independent touch coordinate and pressure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchPoint {
    x: u32,
    y: u32,
    pressure: f32,
}

impl TouchPoint {
    pub fn new(x: u32, y: u32, pressure: f32) -> Self {
        Self { x, y, pressure }
    }

    pub fn x(self) -> u32 {
        self.x
    }

    pub fn y(self) -> u32 {
        self.y
    }

    pub fn pressure(self) -> f32 {
        self.pressure
    }
}

/// Opaque touch contact. An adapter maps this to its own transport state;
/// scrcpy pointer IDs never cross this boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TouchHandle(Uuid);

impl TouchHandle {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Touch lifecycle boundary. The handle carries the contact identity, not a
/// transport pointer ID, so callers can keep it without knowing backend wire
/// details.
#[async_trait]
pub trait TouchService: Send + Sync {
    async fn begin(
        &self,
        device: &super::DeviceHandle,
        point: TouchPoint,
    ) -> CapabilityResult<TouchHandle>;

    async fn move_touch(&self, touch: &TouchHandle, point: TouchPoint) -> CapabilityResult<()>;

    async fn end(&self, touch: &TouchHandle) -> CapabilityResult<()>;
}
