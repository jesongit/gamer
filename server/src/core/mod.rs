pub mod activity;
pub mod events;
pub(crate) mod fs;
pub mod models;
pub(crate) mod secrets;

#[allow(unused_imports)]
pub use activity::{ActivityKind, ActivityLease, DeviceActivity, DeviceLease, NoopLease};
#[allow(unused_imports)]
pub use events::{EventSink, NullEventSink, RuntimeEvent, RuntimeEventKind};
#[allow(
    unused_imports,
    reason = "core facade is consumed incrementally by future adapters"
)]
pub use models::{
    AndroidPackageId, AndroidPackageName, AppContext, AppPackageId, ContentPackageId, DeviceId,
    ModelError, ResourceId, RunContext, RunId, RunPayload, RunRequest,
};
pub mod input_ownership;
pub mod side_effect;
