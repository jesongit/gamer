use std::sync::Arc;

use async_trait::async_trait;

use crate::device::DeviceManager;

use super::super::{
    AppId, CapabilityError, CapabilityResult, DeviceHandle, DeviceId, DeviceService,
};

/// Device/app lifecycle adapter over the existing device registry and scrcpy
/// session. The session remains an implementation detail of this type.
pub(crate) struct DeviceAdapter {
    pub(crate) devices: Arc<DeviceManager>,
}

impl DeviceAdapter {
    pub(crate) fn new(devices: Arc<DeviceManager>) -> Self {
        Self { devices }
    }

    pub(crate) fn session(
        &self,
        device: &DeviceHandle,
    ) -> CapabilityResult<Arc<crate::device::scrcpy::ScrcpySession>> {
        self.devices
            .session(device.id().as_str())
            .ok_or_else(|| CapabilityError::Unavailable("device session"))
    }

    fn check_app_name(app: &AppId) -> CapabilityResult<()> {
        let name = app.as_str();
        let package = name
            .strip_prefix('+')
            .or_else(|| name.strip_prefix('?'))
            .unwrap_or(name);
        if package.is_empty() || !crate::device::adb::is_safe_pkg(package) {
            return Err(CapabilityError::InvalidRequest(format!(
                "invalid Android application name: {name}"
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl DeviceService for DeviceAdapter {
    async fn resolve(&self, id: &DeviceId) -> CapabilityResult<DeviceHandle> {
        if self.devices.snapshot(id.as_str()).is_some() {
            Ok(DeviceHandle::new(id.clone()))
        } else {
            Err(CapabilityError::NotFound(format!("device {}", id.as_str())))
        }
    }

    async fn start_app(&self, device: &DeviceHandle, app: &AppId) -> CapabilityResult<()> {
        Self::check_app_name(app)?;
        self.session(device)?
            .start_app(app.as_str())
            .await
            .map_err(|error| CapabilityError::Failed(error.to_string()))
    }

    async fn stop_app(&self, device: &DeviceHandle, app: &AppId) -> CapabilityResult<()> {
        Self::check_app_name(app)?;
        let package = app.as_str();
        if package.starts_with('+') || package.starts_with('?') {
            return Err(CapabilityError::InvalidRequest(
                "stop_app expects a package name without a launch prefix".into(),
            ));
        }
        let (_, _, serial) = self
            .devices
            .snapshot(device.id().as_str())
            .ok_or_else(|| CapabilityError::NotFound(format!("device {}", device.id().as_str())))?;
        let serial = serial.ok_or_else(|| CapabilityError::Unavailable("adb serial"))?;
        self.devices
            .adb
            .shell(
                &serial,
                &format!("am force-stop {package}"),
                std::time::Duration::from_secs(8),
            )
            .await
            .map(|_| ())
            .map_err(|error| CapabilityError::Failed(error.to_string()))
    }
}
