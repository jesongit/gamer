//! Resolve logical I/O targets without changing the Android device transport.
use crate::{
    core::{ActivityKind, ActivityLease, AndroidPackageName, AppContext, AppPackageId, DeviceId},
    device::DeviceManager,
};

pub fn is_browser(id: &str) -> bool {
    id.starts_with("browser-")
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
