//! Resolve logical I/O targets without changing the Android device transport.
use crate::{
    core::{ActivityKind, ActivityLease, AndroidPackageName, AppContext, AppPackageId, DeviceId},
    device::DeviceManager,
};

pub fn is_browser(id: &str) -> bool {
    id.starts_with("browser-")
}

/// 兼容性所需的运行目标身份；不包含配置包或浏览器账号信息。
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct TargetIdentity {
    pub kind: String,
    pub value: Option<String>,
    pub source: String,
    pub note: String,
}

pub(crate) async fn identity(devices: &DeviceManager, id: &str) -> anyhow::Result<TargetIdentity> {
    if !is_browser(id) {
        let (device, _, _) = devices
            .snapshot(id)
            .ok_or_else(|| anyhow::anyhow!("目标不存在: {id}"))?;
        return Ok(TargetIdentity {
            kind: "android".into(),
            value: device.pkg.filter(|p| !p.trim().is_empty()),
            source: "configured".into(),
            note: "按设备配置的应用包名判断".into(),
        });
    }
    let target = devices.browsers.get(id)?;
    let Ok(session) = devices.browsers.session(id) else {
        return Ok(TargetIdentity {
            kind: "web".into(),
            value: Some(target.url),
            source: "configured".into(),
            note: "未连接：按配置网址估计，尚未验证实际页面".into(),
        });
    };
    // 已建立过会话但目标丢失时不能把启动网址冒充当前页面。
    let stamp = session.stamp();
    let result = async {
        let pages = session.pages().await?;
        session.validate(&stamp)?;
        let current = devices.browsers.session(id)?;
        current.validate(&stamp)?;
        pages
            .iter()
            .find(|p| p["id"].as_str() == Some(&session.target_id))
            .and_then(|p| p["url"].as_str())
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("绑定的标签页已关闭或无法读取"))
    }
    .await;
    Ok(match result {
        Ok(url) => TargetIdentity {
            kind: "web".into(),
            value: Some(url),
            source: "bound".into(),
            note: "按当前绑定标签页的网址判断".into(),
        },
        Err(_) => TargetIdentity {
            kind: "web".into(),
            value: None,
            source: "unavailable".into(),
            note: "目标已断开、变化或无法读取，请连接后重试".into(),
        },
    })
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
