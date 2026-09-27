//! Resolve logical I/O targets without changing the Android device transport.
use crate::{
    core::{ActivityKind, ActivityLease, AndroidPackageName, AppContext, AppPackageId, DeviceId},
    device::DeviceManager,
};

pub fn is_browser(id: &str) -> bool {
    id.starts_with("browser-")
}

/// Target support, independent of plugin permissions and connection readiness.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct TargetCapabilities {
    pub frame: bool,
    pub keyboard: bool,
    pub pointer: bool,
    pub multitouch: bool,
    pub android_app: bool,
    pub recording: bool,
    pub media_output: bool,
}
impl TargetCapabilities {
    pub fn android() -> Self {
        Self {
            frame: true,
            keyboard: true,
            pointer: true,
            multitouch: true,
            android_app: true,
            recording: true,
            media_output: true,
        }
    }
    pub fn browser() -> Self {
        Self {
            multitouch: false,
            android_app: false,
            recording: true,
            media_output: false,
            ..Self::android()
        }
    }
}
pub fn capabilities(devices: &DeviceManager, id: &str) -> anyhow::Result<TargetCapabilities> {
    if is_browser(id) {
        devices.browsers.get(id)?;
        Ok(TargetCapabilities::browser())
    } else {
        anyhow::ensure!(devices.snapshot(id).is_some(), "目标不存在: {id}");
        Ok(TargetCapabilities::android())
    }
}

pub fn check_available(devices: &DeviceManager, id: &str) -> anyhow::Result<()> {
    capabilities(devices, id)?;
    if let Some((_, status, error)) = devices.snapshot(id) {
        anyhow::ensure!(
            status != crate::device::DeviceStatus::Offline || error.is_none(),
            "目标离线：{}",
            error.unwrap_or_default()
        );
    }
    Ok(())
}
pub fn app_context(
    devices: &DeviceManager,
    id: &str,
    content_package: Option<AppPackageId>,
) -> anyhow::Result<AppContext> {
    let device_id = DeviceId::new(id)?;
    if is_browser(id) {
        devices.browsers.get(id)?;
        return Ok(AppContext {
            device_id,
            android_package: None,
            content_package,
        });
    }
    let (device, _, _) = devices
        .snapshot(id)
        .ok_or_else(|| anyhow::anyhow!("设备不存在: {id}"))?;
    let pkg = device
        .pkg
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("设备 {id} 未配置 Android 应用包名（pkg）"))?;
    Ok(AppContext::new(
        device_id,
        AndroidPackageName::new(pkg)?,
        content_package,
    ))
}
pub async fn prepare(devices: &DeviceManager, id: &str) -> anyhow::Result<()> {
    if is_browser(id) {
        let session = devices.browsers.prepare(id).await?;
        // The prior run may still be releasing held keys after cancellation.
        tokio::time::timeout(std::time::Duration::from_secs(4), async {
            while session.runs.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await?;
        session.release_inputs().await;
        session.capture().await?;
    } else if devices.session(id).is_none() {
        devices.connect_device(id).await?;
    }
    Ok(())
}
pub fn acquire(devices: &DeviceManager, id: &str) -> anyhow::Result<Box<dyn ActivityLease>> {
    if is_browser(id) {
        let s = devices.browsers.session(id)?;
        anyhow::ensure!(s.is_alive(), "目标已断开");
        s.runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        return Ok(Box::new(crate::browser::BrowserRunLease(s)));
    }
    Ok(Box::new(devices.acquire_activity(id, ActivityKind::Run)))
}
