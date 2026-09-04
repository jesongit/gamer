//! Phase 6 extension boundary.
//!
//! This module owns installable immutable package storage, lifecycle
//! transitions, the versioned Host API facade, and opt-in/lazy WASM execution.
//! REST injects the service through [`crate::api::AppState`]; it does not own
//! extension bytes or lifecycle state.

#![allow(
    dead_code,
    reason = "Phase 6 extension contracts are introduced before business adapters"
)]
#![allow(
    unused_imports,
    reason = "The extension facade is consumed incrementally by later adapters"
)]

mod archive;
mod error;
mod host_api;
mod keymap;
mod manifest;
mod model;
mod permissions;
mod service;
mod signature;
mod store;
mod ui;
mod wasm;
mod wit;

pub(crate) use error::{ExtensionError, ExtensionResult, PermissionError};
pub(crate) use host_api::{HostApi, HostApiCatalog, HostApiDomain, HOST_API_VERSION};
pub(crate) use keymap::{
    android_keycode, decode_input_event, emit_keymap_trace, keymap_trace_active, load_user_profile,
    now_epoch_us, real_wasm_host_status, AppPackageKeymapSource, CapabilityDeviceActionExecutor,
    DeviceAction, DeviceActionExecutor, InputEvent, InputResult, KeymapContributionRegistry,
    KeymapPanelContribution, KeymapTraceContext, KeymapTracePath, KeymapTraceRecord,
    NormalizedPoint, ScreenSize, INPUT_PROTOCOL_VERSION, KEYMAP_EXTENSION_ID,
    KEYMAP_EXTENSION_MANIFEST_TOML, KEYMAP_PANEL_ID, KEYMAP_WASM_ABI_VERSION,
};
#[cfg(all(test, feature = "wasm-runtime"))]
pub(crate) use keymap::{build_guest_fixture_component, package_guest_fixture_gplugin};
pub(crate) use keymap::{
    clear_keymap_trace_sink, install_keymap_trace_sink, KeymapWasmInstanceHandle,
    KeymapWasmRuntime, KeymapWasmStartRequest, NoKeymapWasmRuntime,
};

pub(crate) use manifest::{
    parse_manifest, ExtensionManifest, HostApiRequirements, UiContribution, UiRuntime,
    MANIFEST_FILE_NAME, MANIFEST_VERSION,
};
pub(crate) use model::{
    ExtensionId, ExtensionPath, ExtensionRecord, ExtensionState, ExtensionVersion,
};
pub(crate) use permissions::{Permission, PermissionSet};
pub(crate) use service::{
    ExtensionInspection, ExtensionInstallContext, ExtensionService, ExtensionSnapshot,
    PermissionDiff, TimerRunnerRegistrar,
};
pub(crate) use signature::{
    RegistryProof, SignatureInfo, SignatureStatus, SignatureVerifier, TrustStore,
};
pub(crate) use store::{ExtensionStore, InstalledExtension};
pub(crate) use ui::{RegisteredUiContribution, UiContributionRegistry};
pub(crate) use wasm::{NoWasmRuntime, WasmInstanceHandle, WasmRuntime, WasmStartRequest};

#[cfg(feature = "wasm-runtime")]
pub(crate) use keymap::LazyKeymapWasmRuntime;
#[cfg(feature = "wasm-runtime")]
pub(crate) use wasm::LazyWasmtimeRuntime;
#[cfg(feature = "wasm-runtime")]
pub(crate) use wasm::LazyYamlWasmtimeRuntime;

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;

    use crate::capabilities::CapabilityRegistry;

    use super::*;

    const VALID_WASM: &[u8] = b"\0asm\x01\0\0\0";

    fn manifest(id: &str, version: &str, permissions: &[&str], host_api: &str) -> Vec<u8> {
        let permissions = permissions
            .iter()
            .map(|permission| format!("\"{permission}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "manifest_version = 1\nid = \"{id}\"\nversion = \"{version}\"\nname = \"Test extension\"\nentry = \"plugin.wasm\"\npermissions = [{permissions}]\n[host_api]\ndevice = \"{host_api}\"\n"
        )
        .into_bytes()
    }

    fn archive(manifest: &[u8], wasm: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            writer.start_file(MANIFEST_FILE_NAME, options).unwrap();
            writer.write_all(manifest).unwrap();
            writer.start_file("plugin.wasm", options).unwrap();
            writer.write_all(wasm).unwrap();
            writer.finish().unwrap();
        }
        bytes
    }

    fn archive_with_extra_path(manifest: &[u8], name: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            writer.start_file(MANIFEST_FILE_NAME, options).unwrap();
            writer.write_all(manifest).unwrap();
            writer.start_file("plugin.wasm", options).unwrap();
            writer.write_all(VALID_WASM).unwrap();
            writer.start_file(name, options).unwrap();
            writer.write_all(b"untrusted").unwrap();
            writer.finish().unwrap();
        }
        bytes
    }

    fn archive_with_ui(manifest: &[u8], wasm: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            writer.start_file(MANIFEST_FILE_NAME, options).unwrap();
            writer.write_all(manifest).unwrap();
            writer.start_file("plugin.wasm", options).unwrap();
            writer.write_all(wasm).unwrap();
            writer.start_file("ui/index.html", options).unwrap();
            writer.write_all(b"<!doctype html>").unwrap();
            writer.finish().unwrap();
        }
        bytes
    }

    #[derive(Default)]
    struct CountingRuntime {
        starts: Mutex<usize>,
        stops: Mutex<usize>,
    }

    #[async_trait]
    impl WasmRuntime for CountingRuntime {
        async fn start(&self, request: WasmStartRequest) -> ExtensionResult<WasmInstanceHandle> {
            assert_eq!(request.id.as_str(), "com.example.extension");
            assert_eq!(request.version.as_str(), "1.0.0");
            assert!(request.host.api_version(HostApiDomain::Device).is_some());
            *self.starts.lock().unwrap() += 1;
            Ok(WasmInstanceHandle::new())
        }

        async fn stop(&self, _instance: WasmInstanceHandle) -> ExtensionResult<()> {
            *self.stops.lock().unwrap() += 1;
            Ok(())
        }

        fn is_available(&self) -> bool {
            true
        }
    }

    struct FailingRuntime;

    #[async_trait]
    impl WasmRuntime for FailingRuntime {
        async fn start(&self, _request: WasmStartRequest) -> ExtensionResult<WasmInstanceHandle> {
            Err(ExtensionError::Runtime("runner failed during start".into()))
        }

        async fn stop(&self, _instance: WasmInstanceHandle) -> ExtensionResult<()> {
            Ok(())
        }

        fn is_available(&self) -> bool {
            true
        }
    }

    /// 记录扩展生命周期回调的 registrar 桩（ADR-13 测试用）。
    #[derive(Default, Clone)]
    struct RecordingRegistrar {
        started: Arc<Mutex<Vec<String>>>,
        stopped: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl TimerRunnerRegistrar for RecordingRegistrar {
        async fn extension_started(&self, extension_id: &str) -> anyhow::Result<()> {
            self.started.lock().unwrap().push(extension_id.to_string());
            Ok(())
        }

        async fn extension_stopped(&self, extension_id: &str) -> anyhow::Result<()> {
            self.stopped.lock().unwrap().push(extension_id.to_string());
            Ok(())
        }
    }

    struct FailingRegistrar;

    #[async_trait]
    impl TimerRunnerRegistrar for FailingRegistrar {
        async fn extension_started(&self, _extension_id: &str) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("registry exploded"))
        }

        async fn extension_stopped(&self, _extension_id: &str) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("registry exploded"))
        }
    }

    #[test]
    fn manifest_is_closed_typed_and_host_requirements_are_semver_checked() {
        let parsed = parse_manifest(&manifest(
            "com.example.extension",
            "1.0.0",
            &["device.read", "vision.match"],
            "^1.0",
        ))
        .unwrap();
        assert_eq!(parsed.id().as_str(), "com.example.extension");
        assert_eq!(parsed.version().as_str(), "1.0.0");
        assert!(parsed
            .host_api()
            .get(HostApiDomain::Device)
            .unwrap()
            .matches(&semver::Version::new(1, 4, 0)));
        assert!(matches!(
            parse_manifest(&manifest(
                "com.example.extension",
                "1.0.0",
                &["filesystem.read"],
                "^1.0"
            )),
            Err(ExtensionError::Permission(PermissionError::Forbidden(_)))
        ));
        let unknown = b"manifest_version = 1\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"Test extension\"\nentry = \"plugin.wasm\"\nextra = true\n";
        assert!(matches!(
            parse_manifest(unknown),
            Err(ExtensionError::InvalidManifest(_))
        ));
    }

    #[test]
    fn default_permission_policy_rejects_dangerous_names() {
        for permission in [
            "filesystem.read",
            "network.connect",
            "shell.execute",
            "device.shell",
            "process.spawn",
        ] {
            assert!(matches!(
                Permission::parse(permission),
                Err(PermissionError::Forbidden(_))
            ));
        }
        assert!(matches!(
            Permission::parse("device.write"),
            Err(PermissionError::Unknown(_))
        ));
        let set = PermissionSet::parse(["device.read"]).unwrap();
        assert!(set.allows(Permission::DeviceRead));
        assert!(!set.allows(Permission::DeviceApp));
    }

    #[test]
    fn host_facade_requires_permission_and_reports_registered_domains() {
        let parsed = parse_manifest(&manifest(
            "com.example.extension",
            "1.0.0",
            &["device.read"],
            "^1.0",
        ))
        .unwrap();
        let host = HostApi::for_manifest(
            CapabilityRegistry::default(),
            HostApiCatalog::default(),
            &parsed,
        )
        .unwrap();
        host.authorize(Permission::DeviceRead).unwrap();
        assert!(matches!(
            host.authorize(Permission::DeviceApp),
            Err(ExtensionError::Permission(PermissionError::NotGranted(_)))
        ));
        assert!(!host.domain_available(HostApiDomain::Device));
        assert_eq!(host.api_version(HostApiDomain::Device).unwrap().major, 1);
    }

    #[test]
    fn archive_rejects_traversal_and_non_wasm_entry_before_staging() {
        let valid_manifest = manifest("com.example.extension", "1.0.0", &[], "^1.0");
        assert!(matches!(
            archive::inspect_archive(&archive_with_extra_path(&valid_manifest, "../escape")),
            Err(ExtensionError::InvalidPath(_)) | Err(ExtensionError::InvalidArchive(_))
        ));
        let non_wasm = archive(&valid_manifest, b"not a wasm module");
        assert!(matches!(
            archive::inspect_archive(&non_wasm),
            Err(ExtensionError::InvalidArchive(_))
        ));
    }

    #[tokio::test]
    async fn install_enable_start_stop_disable_and_update_are_explicit_transitions() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path());
        let runtime = Arc::new(CountingRuntime::default());
        let service = ExtensionService::new(store, runtime.clone(), CapabilityRegistry::default());
        assert!(service.list().unwrap().is_empty());

        let first = archive(
            &manifest("com.example.extension", "1.0.0", &["device.read"], "^1.0"),
            VALID_WASM,
        );
        let installed = service.install(&first).await.unwrap();
        assert_eq!(installed.state(), ExtensionState::Installed);
        assert_eq!(installed.active_version().as_str(), "1.0.0");
        let state = std::fs::read_to_string(temp.path().join("extensions/state.json")).unwrap();
        assert!(state.contains("\"active_version\": \"1.0.0\""));
        assert!(state.contains("\"state\": \"installed\""));
        assert!(matches!(
            service.start(installed.id()).await,
            Err(ExtensionError::InvalidTransition { .. })
        ));

        let enabled = service.enable(installed.id()).await.unwrap();
        assert_eq!(enabled.state(), ExtensionState::Enabled);
        let running = service.start(installed.id()).await.unwrap();
        assert_eq!(running.state(), ExtensionState::Running);
        assert_eq!(*runtime.starts.lock().unwrap(), 1);
        let stopped = service.stop(installed.id()).await.unwrap();
        assert_eq!(stopped.state(), ExtensionState::Enabled);
        assert_eq!(*runtime.stops.lock().unwrap(), 1);
        let disabled = service.disable(installed.id()).await.unwrap();
        assert_eq!(disabled.state(), ExtensionState::Disabled);

        let second = archive(
            &manifest("com.example.extension", "1.1.0", &["device.read"], "^1.0"),
            VALID_WASM,
        );
        let updated = service.update(&second).await.unwrap();
        assert_eq!(updated.active_version().as_str(), "1.1.0");
        assert_eq!(updated.state(), ExtensionState::Disabled);
        assert_eq!(updated.installed_versions().len(), 2);
        assert!(temp
            .path()
            .join("extensions/com.example.extension/1.0.0/plugin.wasm")
            .is_file());
        assert!(temp
            .path()
            .join("extensions/com.example.extension/1.1.0/plugin.wasm")
            .is_file());
    }

    #[tokio::test]
    async fn default_runtime_never_initializes_or_claims_running() {
        let temp = TempDir::new().unwrap();
        let service = ExtensionService::with_default_runtime(
            ExtensionStore::new(temp.path()),
            CapabilityRegistry::default(),
        );
        assert!(!service.runtime_available());
        let installed = service
            .install(&archive(
                &manifest("com.example.extension", "1.0.0", &[], "^1.0"),
                VALID_WASM,
            ))
            .await
            .unwrap();
        assert!(!service.runtime_available());
        let enabled = service.enable(installed.id()).await.unwrap();
        assert!(matches!(
            service.start(enabled.id()).await,
            Err(ExtensionError::RuntimeUnavailable(_))
        ));
        assert_eq!(service.list().unwrap()[0].state(), ExtensionState::Enabled);
    }

    #[tokio::test]
    async fn install_rejects_new_permissions_and_official_source_without_proof() {
        let temp = TempDir::new().unwrap();
        let service = ExtensionService::new(
            ExtensionStore::new(temp.path()),
            Arc::new(CountingRuntime::default()),
            CapabilityRegistry::default(),
        );
        let archive = archive(
            &manifest("com.example.extension", "1.0.0", &["device.read"], "^1.0"),
            VALID_WASM,
        );
        assert!(matches!(
            service
                .install_with_context(&archive, &ExtensionInstallContext::default())
                .await,
            Err(ExtensionError::PermissionConfirmationRequired(_))
        ));
        assert!(service.list().unwrap().is_empty());
        assert!(matches!(
            service.inspect_with_context(
                &archive,
                &ExtensionInstallContext {
                    official: true,
                    ..Default::default()
                }
            ),
            Err(ExtensionError::RegistryProofRequired)
        ));
    }

    #[tokio::test]
    async fn runner_failure_is_persisted_as_failed_without_claiming_running() {
        let temp = TempDir::new().unwrap();
        let service = ExtensionService::new(
            ExtensionStore::new(temp.path()),
            Arc::new(FailingRuntime),
            CapabilityRegistry::default(),
        );
        let installed = service
            .install(&archive(
                &manifest("com.example.extension", "1.0.0", &[], "^1.0"),
                VALID_WASM,
            ))
            .await
            .unwrap();
        service.enable(installed.id()).await.unwrap();
        assert!(matches!(
            service.start(installed.id()).await,
            Err(ExtensionError::Runtime(_))
        ));
        let snapshot = service.list().unwrap().pop().unwrap();
        assert_eq!(snapshot.state(), ExtensionState::Failed);
        assert!(snapshot.last_error().unwrap().contains("runner failed"));
    }

    #[tokio::test]
    async fn running_extension_cannot_be_updated_or_uninstalled() {
        let temp = TempDir::new().unwrap();
        let service = ExtensionService::new(
            ExtensionStore::new(temp.path()),
            Arc::new(CountingRuntime::default()),
            CapabilityRegistry::default(),
        );
        let first = archive(
            &manifest("com.example.extension", "1.0.0", &[], "^1.0"),
            VALID_WASM,
        );
        let installed = service.install(&first).await.unwrap();
        let enabled = service.enable(installed.id()).await.unwrap();
        service.start(enabled.id()).await.unwrap();
        let second = archive(
            &manifest("com.example.extension", "1.1.0", &[], "^1.0"),
            VALID_WASM,
        );
        assert!(matches!(
            service.update(&second).await,
            Err(ExtensionError::InvalidTransition { .. })
        ));
        assert!(matches!(
            service
                .uninstall(installed.id(), &ExtensionVersion::parse("1.0.0").unwrap())
                .await,
            Err(ExtensionError::InvalidTransition { .. })
        ));
    }

    #[tokio::test]
    async fn ui_contributions_appear_after_install_and_are_removed_after_uninstall() {
        let temp = TempDir::new().unwrap();
        let service = ExtensionService::with_default_runtime(
            ExtensionStore::new(temp.path()),
            CapabilityRegistry::default(),
        );
        let manifest = format!(
            "{}[ui]\n[[ui.contributions]]\npanel_id = \"hello\"\ntitle = \"Hello\"\nruntime = \"iframe\"\nentry = \"ui/index.html\"\n",
            String::from_utf8(manifest("com.example.extension", "1.0.0", &[], "^1.0")).unwrap()
        );
        let installed = service
            .install(&archive_with_ui(manifest.as_bytes(), VALID_WASM))
            .await
            .unwrap();
        assert!(service.ui_contributions().unwrap().is_empty());
        service.enable(installed.id()).await.unwrap();
        let contributions = service.ui_contributions().unwrap();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].panel_id, "hello");
        assert_eq!(contributions[0].version.as_str(), "1.0.0");

        let path = ExtensionPath::parse("ui/index.html").unwrap();
        assert_eq!(
            service.read_ui_file(installed.id(), &path).unwrap().0,
            b"<!doctype html>"
        );
        service.disable(installed.id()).await.unwrap();
        assert!(service.ui_contributions().unwrap().is_empty());
        assert!(service
            .uninstall(installed.id(), installed.active_version())
            .await
            .unwrap());
        assert!(service.ui_contributions().unwrap().is_empty());
    }

    /// ADR-13 / P11.2：runner 注册缝跟随扩展生命周期——start 注册（owner 正确）、
    /// enable 不注册、disable 运行中 = 自动 stop + 注销 + UI 贡献消失、再 start
    /// 可重复注册。
    #[tokio::test]
    async fn lifecycle_hooks_register_runners_on_start_and_disable_running_stops_them() {
        let temp = TempDir::new().unwrap();
        let runtime = Arc::new(CountingRuntime::default());
        let registrar = Arc::new(RecordingRegistrar::default());
        let service = ExtensionService::new(
            ExtensionStore::new(temp.path()),
            runtime.clone(),
            CapabilityRegistry::default(),
        )
        .with_runner_registrar(registrar.clone());
        let installed = service
            .install(&archive(
                &manifest("com.example.extension", "1.0.0", &[], "^1.0"),
                VALID_WASM,
            ))
            .await
            .unwrap();
        let id = installed.id().clone();

        // enable 不触碰 runner：Running 才是 runner 生命周期边界
        service.enable(&id).await.unwrap();
        assert!(registrar.started.lock().unwrap().is_empty());
        assert!(registrar.stopped.lock().unwrap().is_empty());

        service.start(&id).await.unwrap();
        assert_eq!(*runtime.starts.lock().unwrap(), 1);
        assert_eq!(
            registrar.started.lock().unwrap().as_slice(),
            ["com.example.extension"]
        );

        // disable 运行中：先自动 stop（实例结束 + runner 注销）再 Disabled
        let disabled = service.disable(&id).await.unwrap();
        assert_eq!(disabled.state(), ExtensionState::Disabled);
        assert_eq!(*runtime.stops.lock().unwrap(), 1);
        assert_eq!(
            registrar.stopped.lock().unwrap().as_slice(),
            ["com.example.extension"]
        );
        assert!(
            service.ui_contributions().unwrap().is_empty(),
            "disable 后 UI 贡献消失"
        );

        // 再 enable+start → 再注册（同 owner 重复注册 = 原地替换）
        service.enable(&id).await.unwrap();
        service.start(&id).await.unwrap();
        assert_eq!(*runtime.starts.lock().unwrap(), 2);
        assert_eq!(registrar.started.lock().unwrap().len(), 2);
    }

    /// registrar 回调失败不得影响生命周期本身：start 仍进入 Running（缺
    /// runner 的后果由任务侧 DependencyMissing 语义兜底），stop 仍回到 Enabled。
    #[tokio::test]
    async fn failing_registrar_does_not_break_lifecycle_transitions() {
        let temp = TempDir::new().unwrap();
        let service = ExtensionService::new(
            ExtensionStore::new(temp.path()),
            Arc::new(CountingRuntime::default()),
            CapabilityRegistry::default(),
        )
        .with_runner_registrar(Arc::new(FailingRegistrar));
        let installed = service
            .install(&archive(
                &manifest("com.example.extension", "1.0.0", &[], "^1.0"),
                VALID_WASM,
            ))
            .await
            .unwrap();
        service.enable(installed.id()).await.unwrap();
        let running = service.start(installed.id()).await.unwrap();
        assert_eq!(running.state(), ExtensionState::Running);
        let stopped = service.stop(installed.id()).await.unwrap();
        assert_eq!(stopped.state(), ExtensionState::Enabled);
    }

    #[test]
    fn wit_contract_contains_every_versioned_domain() {
        assert_eq!(wit::WIT_PACKAGE_VERSION, HOST_API_VERSION);
        for domain in HostApiDomain::ALL {
            let wit_name = if domain == HostApiDomain::Resource {
                "resources"
            } else {
                domain.as_str()
            };
            assert!(wit::WIT_PACKAGE.contains(&format!("interface {wit_name}")));
        }
        assert!(wit::WIT_PACKAGE.contains("world extension-host"));
    }

    #[cfg(feature = "wasm-runtime")]
    #[test]
    fn optional_wasmtime_adapter_is_lazy_and_does_not_initialize_without_a_start() {
        let runtime = LazyWasmtimeRuntime::new();
        assert!(!runtime.is_initialized());
        assert!(runtime.is_available());
        let temp = TempDir::new().unwrap();
        let lazy = Arc::new(LazyWasmtimeRuntime::new());
        let service = ExtensionService::new(
            ExtensionStore::new(temp.path()),
            lazy.clone(),
            CapabilityRegistry::default(),
        );
        assert!(service.list().unwrap().is_empty());
        assert!(!lazy.is_initialized());
    }

    #[cfg(feature = "wasm-runtime")]
    #[tokio::test]
    async fn wasmtime_component_runtime_rejects_a_core_module_as_a_component() {
        let runtime = LazyWasmtimeRuntime::new();
        let manifest =
            parse_manifest(&manifest("com.example.extension", "1.0.0", &[], "^1.0")).unwrap();
        let host = HostApi::for_manifest(
            CapabilityRegistry::default(),
            HostApiCatalog::default(),
            &manifest,
        )
        .unwrap();
        let error = runtime
            .start(WasmStartRequest {
                id: manifest.id().clone(),
                version: manifest.version().clone(),
                wasm: VALID_WASM.to_vec(),
                host,
                app_context: None,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(error, ExtensionError::Runtime(message) if message.contains("组件编译失败"))
        );
        assert!(runtime.is_initialized());
    }
}
