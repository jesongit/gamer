use std::sync::Arc;

use super::{
    DeviceService, FrameService, InputService, LogService, ResourceService, RunService,
    RuntimeService, TouchService, VisionService,
};

/// Optional service registry used while adapters are introduced incrementally.
/// Missing services are represented by `None`; no concrete backend is implied.
#[derive(Clone, Default)]
pub struct CapabilityRegistry {
    device: Option<Arc<dyn DeviceService>>,
    input: Option<Arc<dyn InputService>>,
    touch: Option<Arc<dyn TouchService>>,
    frame: Option<Arc<dyn FrameService>>,
    vision: Option<Arc<dyn VisionService>>,
    resource: Option<Arc<dyn ResourceService>>,
    runtime: Option<Arc<dyn RuntimeService>>,
    run: Option<Arc<dyn RunService>>,
    log: Option<Arc<dyn LogService>>,
}

impl CapabilityRegistry {
    pub fn builder() -> CapabilityRegistryBuilder {
        CapabilityRegistryBuilder::default()
    }

    pub fn device(&self) -> Option<&dyn DeviceService> {
        self.device.as_deref()
    }

    pub fn input(&self) -> Option<&dyn InputService> {
        self.input.as_deref()
    }

    pub fn touch(&self) -> Option<&dyn TouchService> {
        self.touch.as_deref()
    }

    pub fn frame(&self) -> Option<&dyn FrameService> {
        self.frame.as_deref()
    }

    pub fn vision(&self) -> Option<&dyn VisionService> {
        self.vision.as_deref()
    }

    pub fn resource(&self) -> Option<&dyn ResourceService> {
        self.resource.as_deref()
    }

    pub fn runtime(&self) -> Option<&dyn RuntimeService> {
        self.runtime.as_deref()
    }

    pub fn run(&self) -> Option<&dyn RunService> {
        self.run.as_deref()
    }

    pub fn log(&self) -> Option<&dyn LogService> {
        self.log.as_deref()
    }
}

/// Builder for a partially populated capability registry.
#[derive(Default)]
pub struct CapabilityRegistryBuilder {
    registry: CapabilityRegistry,
}

macro_rules! service_builder {
    ($with:ident, $with_erased:ident, $field:ident, $trait_name:ident) => {
        pub fn $with<S>(mut self, service: S) -> Self
        where
            S: $trait_name + 'static,
        {
            self.registry.$field = Some(Arc::new(service));
            self
        }

        pub fn $with_erased(mut self, service: Arc<dyn $trait_name>) -> Self {
            self.registry.$field = Some(service);
            self
        }
    };
}

impl CapabilityRegistryBuilder {
    service_builder!(with_device, with_device_service, device, DeviceService);
    service_builder!(with_input, with_input_service, input, InputService);
    service_builder!(with_touch, with_touch_service, touch, TouchService);
    service_builder!(with_frame, with_frame_service, frame, FrameService);
    service_builder!(with_vision, with_vision_service, vision, VisionService);
    service_builder!(
        with_resource,
        with_resource_service,
        resource,
        ResourceService
    );
    service_builder!(with_runtime, with_runtime_service, runtime, RuntimeService);
    service_builder!(with_run, with_run_service, run, RunService);
    service_builder!(with_log, with_log_service, log, LogService);

    pub fn build(self) -> CapabilityRegistry {
        self.registry
    }
}
