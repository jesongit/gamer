use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use super::*;

/// A deterministic instance-free registrar keeps these tests focused on the
/// lifecycle transaction rather than on Wasmtime component construction.
#[derive(Clone, Default)]
struct RecordingRegistrar {
    started: Arc<AtomicUsize>,
    stopped: Arc<AtomicUsize>,
    fail_next_start: Arc<AtomicBool>,
}

#[async_trait]
impl TimerRunnerRegistrar for RecordingRegistrar {
    async fn extension_started(&self, _extension_id: &str) -> anyhow::Result<()> {
        self.started.fetch_add(1, Ordering::SeqCst);
        if self.fail_next_start.swap(false, Ordering::SeqCst) {
            anyhow::bail!("injected runner registration failure")
        }
        Ok(())
    }

    async fn extension_stopped(&self, _extension_id: &str) -> anyhow::Result<()> {
        self.stopped.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn executes_without_instance(&self, _extension_id: &str) -> bool {
        true
    }
}

fn service(data: &std::path::Path, registrar: RecordingRegistrar) -> ExtensionService {
    ExtensionService::for_data_root(data, crate::capabilities::CapabilityRegistry::default())
        .with_runner_registrar(Arc::new(registrar))
}

#[tokio::test]
async fn running_update_stops_once_and_restores_running() {
    let temp = tempfile::tempdir().unwrap();
    let registrar = RecordingRegistrar::default();
    let service = service(temp.path(), registrar.clone());
    let id = ExtensionId::parse("com.example.m01.update").unwrap();

    service
        .install(&wasm_archive(id.as_str(), "1.0.0"))
        .await
        .unwrap();
    service.enable(&id).await.unwrap();
    service.start(&id).await.unwrap();

    let updated = service
        .update(&wasm_archive(id.as_str(), "2.0.0"))
        .await
        .unwrap();

    assert_eq!(updated.active_version().as_str(), "2.0.0");
    assert_eq!(updated.state(), ExtensionState::Running);
    assert_eq!(registrar.started.load(Ordering::SeqCst), 2);
    assert_eq!(registrar.stopped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn disabled_update_remains_disabled_without_lifecycle_churn() {
    let temp = tempfile::tempdir().unwrap();
    let registrar = RecordingRegistrar::default();
    let service = service(temp.path(), registrar.clone());
    let id = ExtensionId::parse("com.example.m01.disabled").unwrap();

    service
        .install(&wasm_archive(id.as_str(), "1.0.0"))
        .await
        .unwrap();
    service.disable(&id).await.unwrap();

    let updated = service
        .update(&wasm_archive(id.as_str(), "2.0.0"))
        .await
        .unwrap();

    assert_eq!(updated.active_version().as_str(), "2.0.0");
    assert_eq!(updated.state(), ExtensionState::Disabled);
    assert_eq!(registrar.started.load(Ordering::SeqCst), 0);
    // disable performs its existing idempotent runner cleanup once; update
    // must not add another stop/unregister cycle for a Disabled extension.
    assert_eq!(registrar.stopped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failed_update_restores_old_running_version() {
    let temp = tempfile::tempdir().unwrap();
    let registrar = RecordingRegistrar::default();
    let service = service(temp.path(), registrar.clone());
    let id = ExtensionId::parse("com.example.m01.failed").unwrap();

    service
        .install(&wasm_archive(id.as_str(), "1.0.0"))
        .await
        .unwrap();
    service.enable(&id).await.unwrap();
    service.start(&id).await.unwrap();

    // Make the incoming version fail after the stop boundary without changing
    // the old immutable version. The store sees this as an existing path even
    // though it is not an installed version directory.
    let collision = temp
        .path()
        .join("extensions")
        .join(id.as_str())
        .join("2.0.0");
    std::fs::create_dir_all(collision.parent().unwrap()).unwrap();
    std::fs::write(&collision, b"occupied").unwrap();

    let error = service
        .update(&wasm_archive(id.as_str(), "2.0.0"))
        .await
        .unwrap_err();
    assert!(matches!(error, ExtensionError::AlreadyInstalled { .. }));

    let snapshot = service.snapshot_for(&id).unwrap();
    assert_eq!(snapshot.active_version().as_str(), "1.0.0");
    assert_eq!(snapshot.state(), ExtensionState::Running);
    assert_eq!(registrar.started.load(Ordering::SeqCst), 2);
    assert_eq!(registrar.stopped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failed_new_start_rolls_back_and_reports_recovery_failure() {
    let temp = tempfile::tempdir().unwrap();
    let registrar = RecordingRegistrar::default();
    let service = service(temp.path(), registrar.clone());
    let id = ExtensionId::parse("com.example.m01.rollback").unwrap();

    service
        .install(&wasm_archive(id.as_str(), "1.0.0"))
        .await
        .unwrap();
    service.enable(&id).await.unwrap();
    service.start(&id).await.unwrap();
    registrar.fail_next_start.store(true, Ordering::SeqCst);

    let error = service
        .update(&wasm_archive(id.as_str(), "2.0.0"))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("新版本恢复启动失败"),
        "must distinguish update rollback from a successful update: {error}"
    );

    let snapshot = service.snapshot_for(&id).unwrap();
    assert_eq!(snapshot.active_version().as_str(), "1.0.0");
    assert_eq!(snapshot.state(), ExtensionState::Running);
    assert_eq!(snapshot.installed_versions().len(), 2);
    assert_eq!(registrar.started.load(Ordering::SeqCst), 3);
    assert_eq!(registrar.stopped.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn running_active_uninstall_restores_remaining_version() {
    let temp = tempfile::tempdir().unwrap();
    let registrar = RecordingRegistrar::default();
    let service = service(temp.path(), registrar.clone());
    let id = ExtensionId::parse("com.example.m01.uninstall").unwrap();

    service
        .install(&wasm_archive(id.as_str(), "1.0.0"))
        .await
        .unwrap();
    service
        .update(&wasm_archive(id.as_str(), "2.0.0"))
        .await
        .unwrap();
    service.enable(&id).await.unwrap();
    service.start(&id).await.unwrap();

    assert!(service
        .uninstall(&id, &ExtensionVersion::parse("2.0.0").unwrap())
        .await
        .unwrap());

    let snapshot = service.snapshot_for(&id).unwrap();
    assert_eq!(snapshot.active_version().as_str(), "1.0.0");
    assert_eq!(snapshot.state(), ExtensionState::Running);
    assert_eq!(snapshot.installed_versions().len(), 1);
    assert_eq!(registrar.started.load(Ordering::SeqCst), 2);
    assert_eq!(registrar.stopped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn repeated_same_version_update_does_not_stop_running_plugin() {
    let temp = tempfile::tempdir().unwrap();
    let registrar = RecordingRegistrar::default();
    let service = service(temp.path(), registrar.clone());
    let id = ExtensionId::parse("com.example.m01.idempotent").unwrap();

    service
        .install(&wasm_archive(id.as_str(), "1.0.0"))
        .await
        .unwrap();
    service.enable(&id).await.unwrap();
    service.start(&id).await.unwrap();

    let error = service
        .update(&wasm_archive(id.as_str(), "1.0.0"))
        .await
        .unwrap_err();
    assert!(matches!(error, ExtensionError::AlreadyInstalled { .. }));
    assert_eq!(snapshot_state(&service, &id), ExtensionState::Running);
    assert_eq!(registrar.started.load(Ordering::SeqCst), 1);
    assert_eq!(registrar.stopped.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn running_last_version_uninstall_completes_without_spurious_restore_failure() {
    let temp = tempfile::tempdir().unwrap();
    let registrar = RecordingRegistrar::default();
    let service = service(temp.path(), registrar.clone());
    let id = ExtensionId::parse("com.example.m01.last-uninstall").unwrap();

    service
        .install(&wasm_archive(id.as_str(), "1.0.0"))
        .await
        .unwrap();
    service.enable(&id).await.unwrap();
    service.start(&id).await.unwrap();

    assert!(service
        .uninstall(&id, &ExtensionVersion::parse("1.0.0").unwrap())
        .await
        .unwrap());

    assert!(matches!(
        service.snapshot_for(&id),
        Err(ExtensionError::NotInstalled { .. })
    ));
    assert_eq!(registrar.started.load(Ordering::SeqCst), 1);
    assert_eq!(registrar.stopped.load(Ordering::SeqCst), 1);
}
