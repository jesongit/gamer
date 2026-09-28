//! Apply the already verified, release-pinned official bundle while business writes are paused.
#[cfg(windows)]
pub async fn apply(service: &crate::extensions::ExtensionService) -> anyhow::Result<()> {
    use crate::extensions::{ExtensionId, ExtensionInstallContext};
    let layout = gamer_updater::portable::layout().ok_or_else(|| anyhow::anyhow!("非便携安装"))?;
    let version = gamer_updater::distribution::current(&layout)
        .ok_or_else(|| anyhow::anyhow!("无当前版本"))?;
    let (_, manifest) = gamer_updater::distribution::cached(&layout, Some(&version))
        .ok_or_else(|| anyhow::anyhow!("发行清单缺失"))?;
    let choices =
        gamer_updater::official_plugins::choices(&layout, &manifest).map_err(anyhow::Error::msg)?;
    let installed = service.list()?;
    let mut updated = std::collections::HashSet::new();
    for choice in choices {
        let Some(old) = installed.iter().find(|p| p.id().as_str() == choice.id) else {
            continue;
        };
        if semver::Version::parse(&choice.version)? <= *old.active_version().semver() {
            continue;
        }
        anyhow::ensure!(
            choice.permissions.iter().all(|p| old
                .manifest()
                .permissions()
                .names()
                .contains(&p.as_str())),
            "插件 {} 新增权限，请先在插件页确认更新",
            choice.name
        );
        let bytes = std::fs::read(&choice.path)?;
        let context = ExtensionInstallContext {
            official: true,
            permission_confirmed: true,
            expected_sha256: Some(choice.hash),
        };
        service.update_with_context(&bytes, &context).await?;
        updated.insert(choice.id);
        // Enable intent is preserved by the extension service; restore it only after the bundle is installed.
    }
    service.reconcile_startup().await;
    for plugin in service.list()? {
        if updated.contains(plugin.id().as_str())
            && matches!(plugin.state(), crate::extensions::ExtensionState::Enabled)
        {
            service
                .enable_and_start(&ExtensionId::parse(plugin.id().as_str())?, None, None)
                .await?;
        }
    }
    Ok(())
}
