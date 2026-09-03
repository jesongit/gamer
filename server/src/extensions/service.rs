//! Extension lifecycle service. It owns transitions; the store only owns bytes
//! and durable metadata, and the runtime only owns an optional instance.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::capabilities::CapabilityRegistry;

use super::archive::inspect_archive;
use super::error::{ExtensionError, ExtensionResult};
use super::host_api::{HostApi, HostApiCatalog};
use super::manifest::ExtensionManifest;
use super::model::{ExtensionId, ExtensionRecord, ExtensionState, ExtensionVersion};
use super::signature::{RegistryProof, SignatureInfo, SignatureStatus, SignatureVerifier};
use super::store::{ExtensionStore, InstalledExtension};
use super::ui::{RegisteredUiContribution, UiContributionRegistry};
use super::wasm::{WasmInstanceHandle, WasmRuntime, WasmStartRequest};
use super::{
    InputEvent, InputResult, KeymapWasmInstanceHandle, KeymapWasmRuntime, KeymapWasmStartRequest,
    NoKeymapWasmRuntime, ScreenSize, KEYMAP_EXTENSION_ID,
};
use crate::yaml_extension::{
    NoYamlWasmRuntime, YamlProgramResolver, YamlWasmRunRequest, YamlWasmRuntime,
};

#[derive(Clone, Debug)]
pub(crate) struct ExtensionSnapshot {
    manifest: ExtensionManifest,
    active_version: ExtensionVersion,
    installed_versions: Vec<ExtensionVersion>,
    state: ExtensionState,
    last_error: Option<String>,
    signature: SignatureInfo,
}

/// Management-only result for the pre-install inspection step. Keeping this
/// separate from lifecycle snapshots lets REST validate an archive and show a
/// permission diff before any bytes are staged.
#[derive(Clone, Debug)]
pub(crate) struct ExtensionInspection {
    manifest: ExtensionManifest,
    archive_sha256: String,
    signature: SignatureInfo,
    permission_diff: PermissionDiff,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct PermissionDiff {
    pub(crate) added: Vec<String>,
    pub(crate) removed: Vec<String>,
    pub(crate) unchanged: Vec<String>,
}

/// Trust and confirmation metadata supplied by the management boundary. The
/// browser may request an inspection without confirmation; install/update
/// re-checks both fields while holding the lifecycle lock.
#[derive(Clone, Debug, Default)]
pub(crate) struct ExtensionInstallContext {
    pub(crate) official: bool,
    pub(crate) registry_proof: Option<RegistryProof>,
    pub(crate) permission_confirmed: bool,
}

impl ExtensionInspection {
    pub(crate) fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    pub(crate) fn archive_sha256(&self) -> &str {
        &self.archive_sha256
    }

    pub(crate) fn signature(&self) -> &SignatureInfo {
        &self.signature
    }

    pub(crate) fn permission_diff(&self) -> &PermissionDiff {
        &self.permission_diff
    }
}

impl ExtensionSnapshot {
    pub(crate) fn id(&self) -> &ExtensionId {
        self.manifest.id()
    }

    pub(crate) fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    pub(crate) fn active_version(&self) -> &ExtensionVersion {
        &self.active_version
    }

    pub(crate) fn installed_versions(&self) -> &[ExtensionVersion] {
        &self.installed_versions
    }

    pub(crate) fn state(&self) -> ExtensionState {
        self.state
    }

    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub(crate) fn signature(&self) -> &SignatureInfo {
        &self.signature
    }
}

pub(crate) struct ExtensionService {
    store: ExtensionStore,
    runtime: Arc<dyn WasmRuntime>,
    keymap_runtime: Arc<dyn KeymapWasmRuntime>,
    yaml_runtime: Arc<dyn YamlWasmRuntime>,
    capabilities: CapabilityRegistry,
    host_api: HostApiCatalog,
    signature: SignatureVerifier,
    operation_lock: Mutex<()>,
    running: std::sync::Mutex<HashMap<ExtensionId, WasmInstanceHandle>>,
    keymap_running: std::sync::Mutex<HashMap<ExtensionId, KeymapWasmInstanceHandle>>,
    ui: UiContributionRegistry,
}

impl ExtensionService {
    pub(crate) fn new(
        store: ExtensionStore,
        runtime: Arc<dyn WasmRuntime>,
        capabilities: CapabilityRegistry,
    ) -> Self {
        Self::with_keymap_runtime_and_signature(
            store,
            runtime,
            Arc::new(NoKeymapWasmRuntime),
            capabilities,
            SignatureVerifier::default(),
        )
    }

    pub(crate) fn with_keymap_runtime(
        store: ExtensionStore,
        runtime: Arc<dyn WasmRuntime>,
        keymap_runtime: Arc<dyn KeymapWasmRuntime>,
        capabilities: CapabilityRegistry,
    ) -> Self {
        Self::with_keymap_runtime_and_signature(
            store,
            runtime,
            keymap_runtime,
            capabilities,
            SignatureVerifier::default(),
        )
    }

    fn with_keymap_runtime_and_signature(
        store: ExtensionStore,
        runtime: Arc<dyn WasmRuntime>,
        keymap_runtime: Arc<dyn KeymapWasmRuntime>,
        capabilities: CapabilityRegistry,
        signature: SignatureVerifier,
    ) -> Self {
        Self::with_keymap_runtime_and_signature_and_yaml(
            store,
            runtime,
            keymap_runtime,
            Arc::new(NoYamlWasmRuntime),
            capabilities,
            signature,
        )
    }

    fn with_keymap_runtime_and_signature_and_yaml(
        store: ExtensionStore,
        runtime: Arc<dyn WasmRuntime>,
        keymap_runtime: Arc<dyn KeymapWasmRuntime>,
        yaml_runtime: Arc<dyn YamlWasmRuntime>,
        capabilities: CapabilityRegistry,
        signature: SignatureVerifier,
    ) -> Self {
        Self {
            store,
            runtime,
            keymap_runtime,
            yaml_runtime,
            capabilities,
            host_api: HostApiCatalog::default(),
            signature,
            operation_lock: Mutex::new(()),
            running: std::sync::Mutex::new(HashMap::new()),
            keymap_running: std::sync::Mutex::new(HashMap::new()),
            ui: UiContributionRegistry::default(),
        }
    }

    pub(crate) fn with_default_runtime(
        store: ExtensionStore,
        capabilities: CapabilityRegistry,
    ) -> Self {
        Self::new(store, Arc::new(super::wasm::NoWasmRuntime), capabilities)
    }

    pub(crate) fn for_data_root(
        data_root: impl AsRef<Path>,
        capabilities: CapabilityRegistry,
    ) -> Self {
        #[cfg(feature = "wasm-runtime")]
        let runtime: Arc<dyn WasmRuntime> = Arc::new(super::wasm::LazyWasmtimeRuntime::new());
        #[cfg(not(feature = "wasm-runtime"))]
        let runtime: Arc<dyn WasmRuntime> = Arc::new(super::wasm::NoWasmRuntime);
        #[cfg(feature = "wasm-runtime")]
        let keymap_runtime: Arc<dyn KeymapWasmRuntime> =
            Arc::new(super::keymap::LazyKeymapWasmRuntime::new());
        #[cfg(not(feature = "wasm-runtime"))]
        let keymap_runtime: Arc<dyn KeymapWasmRuntime> = Arc::new(NoKeymapWasmRuntime);
        #[cfg(feature = "wasm-runtime")]
        let yaml_runtime: Arc<dyn YamlWasmRuntime> =
            Arc::new(super::wasm::LazyYamlWasmtimeRuntime::new());
        #[cfg(not(feature = "wasm-runtime"))]
        let yaml_runtime: Arc<dyn YamlWasmRuntime> = Arc::new(NoYamlWasmRuntime);
        let signature = SignatureVerifier::from_data_root(data_root.as_ref());
        Self::with_keymap_runtime_and_signature_and_yaml(
            ExtensionStore::new(data_root),
            runtime,
            keymap_runtime,
            yaml_runtime,
            capabilities,
            signature,
        )
    }

    pub(crate) fn runtime_available(&self) -> bool {
        self.runtime.is_available()
            || self.keymap_runtime.is_available()
            || self.yaml_runtime.is_available()
    }

    /// Execute a lowered YAML v3 program in the installed `gamer.yaml`
    /// Component guest. The lifecycle lock only protects the immutable package
    /// lookup; the guest itself runs after the lock is released so uninstall or
    /// update cannot be interleaved with reading its bytes.
    pub(crate) async fn run_yaml_vnext(
        &self,
        program: crate::yaml_vnext::Program,
        context: crate::core::AppContext,
        args: BTreeMap<String, crate::yaml_vnext::Value>,
        resolver: Option<Arc<dyn YamlProgramResolver>>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> ExtensionResult<crate::yaml_vnext::Value> {
        let id = ExtensionId::parse(crate::yaml_extension::YAML_EXTENSION_ID)
            .expect("built-in YAML extension id is valid");
        let (wasm, host) = {
            let _guard = self.operation_lock.lock().await;
            let states = self.store.read_state()?;
            let versions = self.versions_for(&id)?;
            let record = state_for_versions(&id, &versions, states.get(&id).cloned())?;
            if !matches!(
                record.state,
                ExtensionState::Enabled | ExtensionState::Running
            ) {
                return Err(invalid_transition(&id, "run", record.state));
            }
            let active = active_version(&versions, &record)?;
            let host = HostApi::for_manifest(
                self.capabilities.clone(),
                self.host_api.clone(),
                active.manifest(),
            )?;
            (active.read_wasm()?, host)
        };
        self.yaml_runtime
            .run(YamlWasmRunRequest {
                wasm,
                program,
                args,
                resolver,
                host,
                context,
                stop,
            })
            .await
            .map(|result| result.value)
            .map_err(|error| ExtensionError::Runtime(error.to_string()))
    }

    /// Dispatch an input envelope to the running keymap extension. A missing
    /// or stopped keymap is a normal pass-through so the legacy control path
    /// remains available.
    pub(crate) async fn dispatch_keymap_input(
        &self,
        device: crate::capabilities::DeviceHandle,
        screen: ScreenSize,
        event: InputEvent,
    ) -> ExtensionResult<InputResult> {
        let instance = self
            .keymap_running
            .lock()
            .expect("keymap running map poisoned")
            .get(&ExtensionId::parse(KEYMAP_EXTENSION_ID).expect("built-in keymap id"))
            .copied();
        let Some(instance) = instance else {
            return Ok(InputResult::pass());
        };
        self.keymap_runtime
            .dispatch(instance, device, screen, event)
            .await
    }

    pub(crate) fn store(&self) -> &ExtensionStore {
        &self.store
    }

    /// Validate an archive without staging it. The management UI uses this
    /// as the confirmation boundary for source, signature, and permissions.
    pub(crate) fn inspect(&self, archive: &[u8]) -> ExtensionResult<ExtensionInspection> {
        self.inspect_with_context(archive, &ExtensionInstallContext::default())
    }

    pub(crate) fn inspect_with_context(
        &self,
        archive: &[u8],
        context: &ExtensionInstallContext,
    ) -> ExtensionResult<ExtensionInspection> {
        let manifest = self.inspect_compatible(archive)?;
        if context.official && context.registry_proof.is_none() {
            return Err(ExtensionError::RegistryProofRequired);
        }
        let signature = self.signature.verify_archive(archive)?;
        if context.official && signature.status != SignatureStatus::Valid {
            return Err(ExtensionError::InvalidSignature(
                "官方插件必须带有可验证的 manifest.toml Ed25519 签名".into(),
            ));
        }
        if let Some(proof) = context.registry_proof.as_ref() {
            self.signature.verify_registry_proof(
                proof,
                manifest.id(),
                manifest.version(),
                archive,
            )?;
        }
        let archive_sha256 = format!("{:x}", Sha256::digest(archive));
        let permission_diff = self.permission_diff_for(&manifest)?;
        Ok(ExtensionInspection {
            manifest,
            archive_sha256,
            signature,
            permission_diff,
        })
    }

    pub(crate) fn ui_contributions(&self) -> ExtensionResult<Vec<RegisteredUiContribution>> {
        self.refresh_ui_registry()?;
        Ok(self.ui.list())
    }

    pub(crate) fn read_ui_file(
        &self,
        id: &ExtensionId,
        path: &super::model::ExtensionPath,
    ) -> ExtensionResult<(Vec<u8>, String)> {
        if !path.as_str().starts_with("ui/") {
            return Err(ExtensionError::InvalidPath(path.to_string()));
        }
        self.refresh_ui_registry()?;
        let visible = self
            .ui
            .list()
            .into_iter()
            .any(|contribution| contribution.plugin_id == *id);
        if !visible {
            return Err(ExtensionError::UiUnavailable { id: id.to_string() });
        }
        let versions = self.versions_for(id)?;
        let states = self.store.read_state()?;
        let record = state_for_versions(id, &versions, states.get(id).cloned())?;
        let active = active_version(&versions, &record)?;
        let bytes = active.read_file(path)?;
        Ok((bytes, path.as_str().to_string()))
    }

    pub(crate) fn list(&self) -> ExtensionResult<Vec<ExtensionSnapshot>> {
        self.refresh_ui_registry()?;
        let installed = self.store.list_installed()?;
        let states = self.store.read_state()?;
        let mut by_id: BTreeMap<ExtensionId, Vec<InstalledExtension>> = BTreeMap::new();
        for extension in installed {
            by_id
                .entry(extension.manifest().id().clone())
                .or_default()
                .push(extension);
        }
        let mut snapshots = Vec::with_capacity(by_id.len());
        for (id, versions) in by_id {
            let record = state_for_versions(&id, &versions, states.get(&id).cloned())?;
            snapshots.push(snapshot_from_versions(versions, record, &self.signature)?);
        }
        if let Some(id) = states
            .keys()
            .find(|id| !snapshots.iter().any(|snapshot| snapshot.id() == *id))
        {
            return Err(ExtensionError::InvalidState(format!(
                "插件 {} 的状态记录没有对应安装版本",
                id
            )));
        }
        Ok(snapshots)
    }

    /// Install a new immutable version. A second version of an existing ID is
    /// kept side-by-side and does not silently become active.
    pub(crate) async fn install(&self, archive: &[u8]) -> ExtensionResult<ExtensionSnapshot> {
        self.install_with_context(
            archive,
            &ExtensionInstallContext {
                permission_confirmed: true,
                ..Default::default()
            },
        )
        .await
    }

    pub(crate) async fn install_with_context(
        &self,
        archive: &[u8],
        context: &ExtensionInstallContext,
    ) -> ExtensionResult<ExtensionSnapshot> {
        let _guard = self.operation_lock.lock().await;
        let inspection = self.inspect_with_context(archive, context)?;
        ensure_permission_confirmation(&inspection, context)?;
        let manifest = inspection.manifest().clone();
        let installed = self.store.install_archive(archive)?;
        let mut states = self.store.read_state()?;
        states.entry(manifest.id().clone()).or_insert_with(|| {
            ExtensionRecord::new(manifest.id().clone(), manifest.version().clone())
        });
        self.store.write_state(&states)?;
        self.refresh_ui_registry()?;
        self.snapshot_for(&installed.manifest().id().clone())
    }

    /// Update means install a new immutable version and select it as active.
    /// A running extension must be stopped explicitly before it can update.
    pub(crate) async fn update(&self, archive: &[u8]) -> ExtensionResult<ExtensionSnapshot> {
        self.update_with_context(
            archive,
            &ExtensionInstallContext {
                permission_confirmed: true,
                ..Default::default()
            },
        )
        .await
    }

    pub(crate) async fn update_with_context(
        &self,
        archive: &[u8],
        context: &ExtensionInstallContext,
    ) -> ExtensionResult<ExtensionSnapshot> {
        let _guard = self.operation_lock.lock().await;
        let inspection = self.inspect_with_context(archive, context)?;
        ensure_permission_confirmation(&inspection, context)?;
        let manifest = inspection.manifest().clone();
        let mut states = self.store.read_state()?;
        let versions = self.versions_for(manifest.id())?;
        if versions.is_empty() {
            return Err(ExtensionError::NotInstalled {
                id: manifest.id().to_string(),
            });
        }
        let mut record =
            state_for_versions(manifest.id(), &versions, states.get(manifest.id()).cloned())?;
        if record.state.is_running() {
            return Err(invalid_transition(manifest.id(), "update", record.state));
        }
        self.store.install_archive(archive)?;
        record.active_version = Some(manifest.version().clone());
        record.state = match record.state {
            ExtensionState::Enabled | ExtensionState::Disabled => record.state,
            ExtensionState::Installed | ExtensionState::Failed | ExtensionState::Running => {
                ExtensionState::Installed
            }
        };
        record.last_error = None;
        states.insert(manifest.id().clone(), record);
        self.store.write_state(&states)?;
        self.refresh_ui_registry()?;
        self.snapshot_for(manifest.id())
    }

    pub(crate) async fn enable(&self, id: &ExtensionId) -> ExtensionResult<ExtensionSnapshot> {
        let _guard = self.operation_lock.lock().await;
        let mut states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let mut record = state_for_versions(id, &versions, states.get(id).cloned())?;
        match record.state {
            ExtensionState::Installed | ExtensionState::Disabled | ExtensionState::Failed => {
                record.state = ExtensionState::Enabled;
                record.last_error = None;
            }
            ExtensionState::Enabled => {}
            ExtensionState::Running => return Err(invalid_transition(id, "enable", record.state)),
        }
        states.insert(id.clone(), record);
        self.store.write_state(&states)?;
        self.refresh_ui_registry()?;
        self.snapshot_for(id)
    }

    pub(crate) async fn disable(&self, id: &ExtensionId) -> ExtensionResult<ExtensionSnapshot> {
        let _guard = self.operation_lock.lock().await;
        let mut states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let mut record = state_for_versions(id, &versions, states.get(id).cloned())?;
        match record.state {
            ExtensionState::Running => return Err(invalid_transition(id, "disable", record.state)),
            ExtensionState::Disabled => {}
            ExtensionState::Installed | ExtensionState::Enabled | ExtensionState::Failed => {
                record.state = ExtensionState::Disabled;
                record.last_error = None;
            }
        }
        states.insert(id.clone(), record);
        self.store.write_state(&states)?;
        self.refresh_ui_registry()?;
        self.snapshot_for(id)
    }

    pub(crate) async fn start(&self, id: &ExtensionId) -> ExtensionResult<ExtensionSnapshot> {
        self.start_with_context(id, None).await
    }

    pub(crate) async fn start_with_context(
        &self,
        id: &ExtensionId,
        app_context: Option<crate::core::AppContext>,
    ) -> ExtensionResult<ExtensionSnapshot> {
        let _guard = self.operation_lock.lock().await;
        let states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let record = state_for_versions(id, &versions, states.get(id).cloned())?;
        if !record.state.can_start() {
            return Err(invalid_transition(id, "start", record.state));
        }
        let active = active_version(&versions, &record)?;
        let host = HostApi::for_manifest(
            self.capabilities.clone(),
            self.host_api.clone(),
            active.manifest(),
        )?;
        let handle = if id.as_str() == KEYMAP_EXTENSION_ID {
            match self
                .keymap_runtime
                .start(KeymapWasmStartRequest {
                    id: id.clone(),
                    version: active.manifest().version().clone(),
                    wasm: active.read_wasm()?,
                    host,
                    app_context,
                })
                .await
            {
                Ok(handle) => StartHandle::Keymap(handle),
                Err(error) => return self.mark_start_failed(id, states, record, error).await,
            }
        } else {
            match self
                .runtime
                .start(WasmStartRequest {
                    id: id.clone(),
                    version: active.manifest().version().clone(),
                    wasm: active.read_wasm()?,
                    host,
                    app_context,
                })
                .await
            {
                Ok(handle) => StartHandle::Generic(handle),
                Err(error) => return self.mark_start_failed(id, states, record, error).await,
            }
        };

        let instance_insert = match handle {
            StartHandle::Generic(handle) => RunningHandle::Generic(handle),
            StartHandle::Keymap(handle) => RunningHandle::Keymap(handle),
        };

        let mut running_record = record;
        running_record.state = ExtensionState::Running;
        running_record.last_error = None;
        if let Err(error) = self.store.write_state(&{
            let mut next = states.clone();
            next.insert(id.clone(), running_record.clone());
            next
        }) {
            let _ = self.stop_running_handle(instance_insert).await;
            return Err(error);
        }
        match instance_insert {
            RunningHandle::Generic(handle) => {
                self.running
                    .lock()
                    .expect("extension running map poisoned")
                    .insert(id.clone(), handle);
            }
            RunningHandle::Keymap(handle) => {
                self.keymap_running
                    .lock()
                    .expect("keymap running map poisoned")
                    .insert(id.clone(), handle);
            }
        }
        self.refresh_ui_registry()?;
        self.snapshot_for(id)
    }

    pub(crate) async fn stop(&self, id: &ExtensionId) -> ExtensionResult<ExtensionSnapshot> {
        let _guard = self.operation_lock.lock().await;
        let mut states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let mut record = state_for_versions(id, &versions, states.get(id).cloned())?;
        if !record.state.is_running() {
            return Err(invalid_transition(id, "stop", record.state));
        }
        if id.as_str() == KEYMAP_EXTENSION_ID {
            let handle = self
                .keymap_running
                .lock()
                .expect("keymap running map poisoned")
                .get(id)
                .copied()
                .ok_or(ExtensionError::RuntimeUnavailable(
                    "当前进程没有该插件的运行实例",
                ))?;
            self.keymap_runtime.stop(handle).await?;
        } else {
            let handle = self
                .running
                .lock()
                .expect("extension running map poisoned")
                .get(id)
                .copied()
                .ok_or(ExtensionError::RuntimeUnavailable(
                    "当前进程没有该插件的运行实例",
                ))?;
            self.runtime.stop(handle).await?;
        }
        record.state = ExtensionState::Enabled;
        record.last_error = None;
        states.insert(id.clone(), record);
        self.store.write_state(&states)?;
        self.running
            .lock()
            .expect("extension running map poisoned")
            .remove(id);
        self.keymap_running
            .lock()
            .expect("keymap running map poisoned")
            .remove(id);
        self.refresh_ui_registry()?;
        self.snapshot_for(id)
    }

    pub(crate) async fn uninstall(
        &self,
        id: &ExtensionId,
        version: &ExtensionVersion,
    ) -> ExtensionResult<bool> {
        let _guard = self.operation_lock.lock().await;
        let mut states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let record = state_for_versions(id, &versions, states.get(id).cloned())?;
        if record.state.is_running() {
            return Err(invalid_transition(id, "uninstall", record.state));
        }
        if !versions
            .iter()
            .any(|candidate| candidate.manifest().version() == version)
        {
            return Err(ExtensionError::VersionNotInstalled {
                id: id.to_string(),
                version: version.to_string(),
            });
        }
        if !self.store.remove_version(id, version)? {
            return Ok(false);
        }
        let remaining = self.versions_for(id)?;
        if remaining.is_empty() {
            states.remove(id);
        } else {
            let mut next_record = record;
            if next_record.active_version.as_ref() == Some(version) {
                next_record.active_version = Some(
                    remaining
                        .iter()
                        .map(|extension| extension.manifest().version().clone())
                        .max()
                        .expect("remaining versions is non-empty"),
                );
            }
            states.insert(id.clone(), next_record);
        }
        self.store.write_state(&states)?;
        self.refresh_ui_registry()?;
        Ok(true)
    }

    fn inspect_compatible(&self, archive: &[u8]) -> ExtensionResult<ExtensionManifest> {
        let manifest = inspect_archive(archive)?;
        self.host_api.validate(&manifest)?;
        Ok(manifest)
    }

    fn snapshot_for(&self, id: &ExtensionId) -> ExtensionResult<ExtensionSnapshot> {
        let states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let record = state_for_versions(id, &versions, states.get(id).cloned())?;
        snapshot_from_versions(versions, record, &self.signature)
    }

    fn versions_for(&self, id: &ExtensionId) -> ExtensionResult<Vec<InstalledExtension>> {
        Ok(self
            .store
            .list_installed()?
            .into_iter()
            .filter(|extension| extension.manifest().id() == id)
            .collect())
    }

    fn refresh_ui_registry(&self) -> ExtensionResult<()> {
        let installed = self.store.list_installed()?;
        let states = self.store.read_state()?;
        let mut by_id: BTreeMap<ExtensionId, Vec<InstalledExtension>> = BTreeMap::new();
        for extension in installed {
            by_id
                .entry(extension.manifest().id().clone())
                .or_default()
                .push(extension);
        }
        self.ui.clear();
        for (id, versions) in by_id {
            let record = state_for_versions(&id, &versions, states.get(&id).cloned())?;
            // A disabled or merely installed package must not remain visible
            // to the dynamic panel registry. Stopping a running extension
            // transitions back to Enabled, so its declarative UI remains
            // available while its WASM entrypoint is not executing.
            if matches!(
                record.state,
                ExtensionState::Enabled | ExtensionState::Running
            ) {
                self.ui
                    .register(active_version(&versions, &record)?.manifest());
            }
        }
        Ok(())
    }

    async fn mark_start_failed(
        &self,
        id: &ExtensionId,
        states: BTreeMap<ExtensionId, ExtensionRecord>,
        record: ExtensionRecord,
        error: ExtensionError,
    ) -> ExtensionResult<ExtensionSnapshot> {
        if matches!(&error, ExtensionError::Runtime(_)) {
            let mut failed_record = record;
            failed_record.state = ExtensionState::Failed;
            failed_record.last_error = Some(error.to_string());
            let mut next = states;
            next.insert(id.clone(), failed_record);
            self.store.write_state(&next)?;
            self.refresh_ui_registry()?;
        }
        Err(error)
    }

    async fn stop_running_handle(&self, handle: RunningHandle) -> ExtensionResult<()> {
        match handle {
            RunningHandle::Generic(handle) => self.runtime.stop(handle).await,
            RunningHandle::Keymap(handle) => self.keymap_runtime.stop(handle).await,
        }
    }
}

#[derive(Clone, Copy)]
enum StartHandle {
    Generic(WasmInstanceHandle),
    Keymap(KeymapWasmInstanceHandle),
}

#[derive(Clone, Copy)]
enum RunningHandle {
    Generic(WasmInstanceHandle),
    Keymap(KeymapWasmInstanceHandle),
}

fn state_for_versions(
    id: &ExtensionId,
    versions: &[InstalledExtension],
    record: Option<ExtensionRecord>,
) -> ExtensionResult<ExtensionRecord> {
    if versions.is_empty() {
        return Err(ExtensionError::NotInstalled { id: id.to_string() });
    }
    let fallback = versions
        .iter()
        .map(|extension| extension.manifest().version().clone())
        .max()
        .expect("state lookup requires at least one version");
    let record = record.unwrap_or_else(|| ExtensionRecord::new(id.clone(), fallback));
    if record.active_version.is_none()
        || !versions
            .iter()
            .any(|extension| Some(extension.manifest().version()) == record.active_version.as_ref())
    {
        return Err(ExtensionError::InvalidState(format!(
            "插件 {} 的 active_version 未安装",
            id
        )));
    }
    Ok(record)
}

fn active_version<'a>(
    versions: &'a [InstalledExtension],
    record: &ExtensionRecord,
) -> ExtensionResult<&'a InstalledExtension> {
    let active = record
        .active_version
        .as_ref()
        .expect("validated active version");
    versions
        .iter()
        .find(|extension| extension.manifest().version() == active)
        .ok_or_else(|| ExtensionError::InvalidState("active_version 未安装".to_string()))
}

fn snapshot_from_versions(
    versions: Vec<InstalledExtension>,
    record: ExtensionRecord,
    signature: &SignatureVerifier,
) -> ExtensionResult<ExtensionSnapshot> {
    let active = active_version(&versions, &record)?;
    let signature_info = signature.verify_installed(
        active.root(),
        &active.root().join(super::manifest::MANIFEST_FILE_NAME),
    );
    Ok(ExtensionSnapshot {
        manifest: active.manifest().clone(),
        active_version: active.manifest().version().clone(),
        installed_versions: versions
            .iter()
            .map(|extension| extension.manifest().version().clone())
            .collect(),
        state: record.state,
        last_error: record.last_error,
        signature: signature_info,
    })
}

impl ExtensionService {
    fn permission_diff_for(&self, manifest: &ExtensionManifest) -> ExtensionResult<PermissionDiff> {
        let current = self
            .list()?
            .into_iter()
            .find(|snapshot| snapshot.id() == manifest.id())
            .map(|snapshot| snapshot.manifest().permissions().names())
            .unwrap_or_default();
        let requested = manifest.permissions().names();
        Ok(PermissionDiff {
            added: requested
                .iter()
                .filter(|permission| !current.contains(permission))
                .map(|permission| (*permission).to_string())
                .collect(),
            removed: current
                .iter()
                .filter(|permission| !requested.contains(permission))
                .map(|permission| (*permission).to_string())
                .collect(),
            unchanged: requested
                .iter()
                .filter(|permission| current.contains(permission))
                .map(|permission| (*permission).to_string())
                .collect(),
        })
    }
}

fn ensure_permission_confirmation(
    inspection: &ExtensionInspection,
    context: &ExtensionInstallContext,
) -> ExtensionResult<()> {
    if inspection.permission_diff().added.is_empty() || context.permission_confirmed {
        return Ok(());
    }
    Err(ExtensionError::PermissionConfirmationRequired(format!(
        "新增权限: {}",
        inspection.permission_diff().added.join(", ")
    )))
}

fn invalid_transition(
    id: &ExtensionId,
    operation: &'static str,
    state: ExtensionState,
) -> ExtensionError {
    ExtensionError::InvalidTransition {
        id: id.to_string(),
        operation,
        state,
    }
}
