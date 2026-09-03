//! Phase 6 extension boundary.
//!
//! This module is deliberately not wired into REST, SQLite, the scheduler,
//! YAML, keymaps, FrameCache, or WebRTC. It provides installable immutable
//! package storage, explicit lifecycle transitions, a versioned Host API
//! facade for eight host domains, and an opt-in/lazy WASM adapter seam.

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
mod manifest;
mod model;
mod permissions;
mod service;
mod store;
mod wasm;
mod wit;

pub(crate) use error::{ExtensionError, ExtensionResult, PermissionError};
pub(crate) use host_api::{HostApi, HostApiCatalog, HostApiDomain, HOST_API_VERSION};
pub(crate) use manifest::{
    parse_manifest, ExtensionManifest, HostApiRequirements, MANIFEST_FILE_NAME, MANIFEST_VERSION,
};
pub(crate) use model::{
    ExtensionId, ExtensionPath, ExtensionRecord, ExtensionState, ExtensionVersion,
};
pub(crate) use permissions::{Permission, PermissionSet};
pub(crate) use service::{ExtensionService, ExtensionSnapshot};
pub(crate) use store::{ExtensionStore, InstalledExtension};
pub(crate) use wasm::{NoWasmRuntime, WasmInstanceHandle, WasmRuntime, WasmStartRequest};

#[cfg(feature = "wasm-runtime")]
pub(crate) use wasm::LazyWasmtimeRuntime;

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

    #[test]
    fn wit_contract_contains_every_versioned_domain() {
        assert_eq!(wit::WIT_PACKAGE_VERSION, HOST_API_VERSION);
        for domain in HostApiDomain::ALL {
            assert!(wit::WIT_PACKAGE.contains(&format!("interface {}", domain.as_str())));
        }
        assert!(wit::WIT_PACKAGE.contains("world extension-host"));
    }

    #[cfg(feature = "wasm-runtime")]
    #[test]
    fn optional_wasmtime_adapter_is_lazy_and_does_not_execute_wasm() {
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
}
