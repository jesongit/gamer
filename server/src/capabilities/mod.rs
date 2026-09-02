//! Gamer Core 的稳定能力边界。
//!
//! 这是 Phase 3 的接口骨架，不承载现有设备、scrcpy、matcher 或 YAML
//! Engine 的实现。当前消费者通过 crate-visible 的 service trait 接入；未来
//! 既有实现和 WASM Host 都应在这里与具体后端之间增加 adapter。

#![allow(
    dead_code,
    reason = "Phase 3 capability contracts are adopted incrementally by adapters"
)]
#![allow(
    unused_imports,
    reason = "The crate-visible re-export surface is consumed by later adapters"
)]

mod device;
mod error;
mod frame;
mod input;
mod log;
mod registry;
mod resource;
mod run;
mod runtime;
mod touch;
mod vision;

pub(crate) use device::{AppId, DeviceHandle, DeviceId, DeviceService};
pub(crate) use error::{CapabilityError, CapabilityResult};
pub(crate) use frame::{FrameHandle, FrameService, FrameSize};
pub(crate) use input::{InputService, KeyAction, KeyCode, KeyInput, SwipeGesture, TextInput};
pub(crate) use log::{LogLevel, LogRecord, LogService};
pub(crate) use registry::{CapabilityRegistry, CapabilityRegistryBuilder};
pub(crate) use resource::{ResourceHandle, ResourceId, ResourceLease, ResourceService};
pub(crate) use run::{RunHandle, RunRequest, RunService, RunStatus};
pub(crate) use runtime::RuntimeService;
pub(crate) use touch::{TouchHandle, TouchPoint, TouchService};
pub(crate) use vision::{
    ColorSample, FramePoint, MatchBox, MatchManyRequest, MatchManyResult, MatchOptions,
    MatchOutcome, SearchRegion, TemplateQuery, VisionService,
};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use super::*;

    #[derive(Default)]
    struct MockDevice;

    #[async_trait]
    impl DeviceService for MockDevice {
        async fn resolve(&self, id: &DeviceId) -> CapabilityResult<DeviceHandle> {
            Ok(DeviceHandle::new(id.clone()))
        }

        async fn start_app(&self, _device: &DeviceHandle, _app: &AppId) -> CapabilityResult<()> {
            Ok(())
        }

        async fn stop_app(&self, _device: &DeviceHandle, _app: &AppId) -> CapabilityResult<()> {
            Ok(())
        }
    }

    #[test]
    fn registry_keeps_services_behind_trait_object_boundaries() {
        let registry = CapabilityRegistry::builder()
            .with_device(MockDevice)
            .build();

        assert!(registry.device().is_some());
        assert!(registry.input().is_none());
        assert!(registry.vision().is_none());

        let cloned = registry.clone();
        assert!(cloned.device().is_some());

        // The builder also accepts an already-erased service, which is useful for
        // adapters assembled by a higher-level runtime.
        let erased: Arc<dyn DeviceService> = Arc::new(MockDevice);
        let registry = CapabilityRegistry::builder()
            .with_device_service(erased)
            .build();
        assert!(registry.device().is_some());
    }

    #[tokio::test]
    async fn device_service_returns_a_small_handle() {
        let service = MockDevice;
        let id = DeviceId::new("phone-1");
        let handle = service.resolve(&id).await.unwrap();

        assert_eq!(handle.id(), &id);
        service
            .start_app(&handle, &AppId::new("com.example.game"))
            .await
            .unwrap();
    }

    #[test]
    fn match_many_has_one_frame_and_ordered_template_queries() {
        let frame = FrameHandle::new();
        let first = ResourceHandle::new();
        let second = ResourceHandle::new();
        let request = MatchManyRequest::new(frame)
            .with_template(TemplateQuery::new(first, MatchOptions::default()))
            .with_template(TemplateQuery::new(second, MatchOptions::default()));

        assert_eq!(request.frame(), frame);
        assert_eq!(request.templates().len(), 2);
        assert_eq!(request.templates()[0].template(), first);
        assert_eq!(request.templates()[1].template(), second);
    }

    #[test]
    fn handles_are_opaque_and_not_backend_pointer_ids() {
        let touch = TouchHandle::new();
        let frame = FrameHandle::new();
        let resource = ResourceHandle::new();
        let run = RunHandle::new();

        assert_ne!(touch, TouchHandle::new());
        assert_ne!(frame, FrameHandle::new());
        assert_ne!(resource, ResourceHandle::new());
        assert_ne!(run, RunHandle::new());
    }
}
