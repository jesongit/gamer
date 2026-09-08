//! Extension lifecycle service. It owns transitions; the store only owns bytes
//! and durable metadata, and the runtime only owns an optional instance.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::capabilities::CapabilityRegistry;

use super::archive::inspect_archive;
use super::error::{ExtensionError, ExtensionResult};
use super::host_api::{HostApi, HostApiCatalog};
use super::manifest::ExtensionManifest;
use super::model::{ExtensionId, ExtensionRecord, ExtensionState, ExtensionVersion};
use super::store::{ExtensionStore, InstalledExtension};
use super::ui::{RegisteredUiContribution, UiContributionRegistry};
use super::wasm::{WasmInstanceHandle, WasmRuntime, WasmStartRequest};
use super::{
    InputEvent, InputResult, KeymapTraceContext, KeymapWasmInstanceHandle, KeymapWasmRuntime,
    KeymapWasmStartRequest, NoKeymapWasmRuntime, ScreenSize,
};

/// Extension lifecycle → Timer runner registry seam（ADR-13 / P11.2）。The
/// service only knows *when* a lifecycle transition happened; the
/// composition-root implementation owns the registry (the Scheduler) and each
/// extension boundary declares its own runner construction and execution
/// model through this trait. Optional so minimal assemblies (tests, gate
/// router) can run without a scheduler.
#[async_trait]
pub(crate) trait TimerRunnerRegistrar: Send + Sync {
    /// The extension entered `Running`: register every runner it owns
    /// (owner = extension id) and resume tasks that were suspended because
    /// those runners were missing.
    async fn extension_started(&self, extension_id: &str) -> anyhow::Result<()>;

    /// The extension left `Running` (stop / disable / uninstall): unregister
    /// every runner it owns; Active tasks bound to them enter
    /// `DependencyMissing` (tasks are kept, never deleted).
    async fn extension_stopped(&self, extension_id: &str) -> anyhow::Result<()>;

    /// 该扩展是否采用「按调用执行、无常驻实例」模型：`start` 只表示「作为
    /// runner 提供方在线」，不启动 extension-host 常驻实例；执行由按调用
    /// 运行时在每次运行时惰性实例化（gamer.yaml 的 run_yaml_program）。默认
    /// false = 常驻实例模型（start 启动实例并持有句柄）。执行模型由拥有该
    /// 扩展 runner 构造的边界自行声明，本服务不按扩展 id 特判。
    fn executes_without_instance(&self, _extension_id: &str) -> bool {
        false
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ExtensionSnapshot {
    manifest: ExtensionManifest,
    active_version: ExtensionVersion,
    installed_versions: Vec<ExtensionVersion>,
    state: ExtensionState,
    last_error: Option<String>,
}

/// Management-only result for the pre-install inspection step. Keeping this
/// separate from lifecycle snapshots lets REST validate an archive and show a
/// permission diff before any bytes are staged.
#[derive(Clone, Debug)]
pub(crate) struct ExtensionInspection {
    manifest: ExtensionManifest,
    archive_sha256: String,
    permission_diff: PermissionDiff,
    /// 执行形态变化（Phase 8 §11.2）：同 id 已装版本与 incoming 的
    /// `[execution].kind` 不同时给出（wasm→builtin 官方迁移放行但必须提示；
    /// builtin→wasm 降级直接拒绝，见 `execution_policy_check`）。
    execution_change: Option<ExecutionChange>,
}

/// 执行形态变化（管理面提示载荷；`from`/`to` 随 inspect 响应透传前端）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ExecutionChange {
    pub(crate) from: super::manifest::ExecutionKind,
    pub(crate) to: super::manifest::ExecutionKind,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct PermissionDiff {
    pub(crate) added: Vec<String>,
    pub(crate) removed: Vec<String>,
    pub(crate) unchanged: Vec<String>,
}

/// Install-source metadata supplied by the management boundary. The browser
/// may request an inspection without confirmation; install/update re-checks
/// confirmation (and the optional expected-sha256 integrity pin) while holding
/// the lifecycle lock. 签名/Registry proof 已随 Phase 1 免签名策略整体退役。
#[derive(Clone, Debug, Default)]
pub(crate) struct ExtensionInstallContext {
    pub(crate) official: bool,
    pub(crate) permission_confirmed: bool,
    /// 可选完整性钉：请求声明的归档 SHA-256（如市场条目），不匹配拒绝安装。
    pub(crate) expected_sha256: Option<String>,
}

impl ExtensionInspection {
    pub(crate) fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    pub(crate) fn archive_sha256(&self) -> &str {
        &self.archive_sha256
    }

    pub(crate) fn permission_diff(&self) -> &PermissionDiff {
        &self.permission_diff
    }

    pub(crate) fn execution_change(&self) -> Option<&ExecutionChange> {
        self.execution_change.as_ref()
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
}

pub(crate) struct ExtensionService {
    store: ExtensionStore,
    runtime: Arc<dyn WasmRuntime>,
    keymap_runtime: Arc<dyn KeymapWasmRuntime>,
    capabilities: CapabilityRegistry,
    host_api: HostApiCatalog,
    operation_lock: Mutex<()>,
    running: std::sync::Mutex<HashMap<ExtensionId, WasmInstanceHandle>>,
    keymap_running: std::sync::Mutex<HashMap<ExtensionId, KeymapWasmInstanceHandle>>,
    ui: UiContributionRegistry,
    runner_registrar: Option<Arc<dyn TimerRunnerRegistrar>>,
}

impl ExtensionService {
    pub(crate) fn new(
        store: ExtensionStore,
        runtime: Arc<dyn WasmRuntime>,
        capabilities: CapabilityRegistry,
    ) -> Self {
        Self::with_keymap_runtime(store, runtime, Arc::new(NoKeymapWasmRuntime), capabilities)
    }

    pub(crate) fn with_keymap_runtime(
        store: ExtensionStore,
        runtime: Arc<dyn WasmRuntime>,
        keymap_runtime: Arc<dyn KeymapWasmRuntime>,
        capabilities: CapabilityRegistry,
    ) -> Self {
        Self {
            store,
            runtime,
            keymap_runtime,
            capabilities,
            host_api: HostApiCatalog::default(),
            operation_lock: Mutex::new(()),
            running: std::sync::Mutex::new(HashMap::new()),
            keymap_running: std::sync::Mutex::new(HashMap::new()),
            ui: UiContributionRegistry::default(),
            runner_registrar: None,
        }
    }

    /// Attach the ADR-13 runner registration seam (composition root only).
    /// Without it, lifecycle transitions simply do not touch any runner
    /// registry — assemblies without a scheduler stay bare-core.
    pub(crate) fn with_runner_registrar(
        mut self,
        registrar: Arc<dyn TimerRunnerRegistrar>,
    ) -> Self {
        self.runner_registrar = Some(registrar);
        self
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
        Self::with_keymap_runtime(
            ExtensionStore::new(data_root),
            runtime,
            keymap_runtime,
            capabilities,
        )
    }

    pub(crate) fn runtime_available(&self) -> bool {
        self.runtime.is_available() || self.keymap_runtime.is_available()
    }

    /// 是否按调用执行（无常驻实例）。组合根注册的 registrar 是唯一权威——
    /// 它拥有各扩展 runner 的构造方式，因此也拥有该扩展执行模型的声明；
    /// 未挂 registrar 的最小装配一律按常驻实例模型处理。builtin（宿主预置）
    /// 扩展（`gamer.video`，无 guest/无 Runner）的「按调用执行」由静态注册表
    /// （`builtin::is_builtin_extension`）声明：start 只表示进入 Running 以
    /// 点亮 UI 贡献与 call 通路。
    fn instance_free(&self, id: &ExtensionId) -> bool {
        super::builtin::is_builtin_extension(id)
            || self
                .runner_registrar
                .as_ref()
                .is_some_and(|registrar| registrar.executes_without_instance(id.as_str()))
    }

    /// Resolve the active-version guest bytes and host API for an extension
    /// that executes per-call instead of holding a resident instance. Generic
    /// mechanism: callers (extension boundaries such as gamer_yaml) supply
    /// their own runtime; this only performs the locked state/manifest lookup.
    /// The lifecycle lock only protects the immutable package lookup; the
    /// guest itself runs after the lock is released so uninstall or update
    /// cannot be interleaved with reading its bytes.
    pub(crate) async fn guest_for_run(
        &self,
        id: &ExtensionId,
    ) -> ExtensionResult<(Vec<u8>, HostApi)> {
        let _guard = self.operation_lock.lock().await;
        let states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let record = state_for_versions(id, &versions, states.get(id).cloned())?;
        if !matches!(
            record.state,
            ExtensionState::Enabled | ExtensionState::Running
        ) {
            return Err(invalid_transition(id, "run", record.state));
        }
        let active = active_version(&versions, &record)?;
        let host = HostApi::for_manifest(
            self.capabilities.clone(),
            self.host_api.clone(),
            active.manifest(),
        )?;
        Ok((active.read_wasm()?, host))
    }

    /// Dispatch an input envelope to the running keymap extension. A missing
    /// or stopped keymap is a normal pass-through so direct (unmapped) input
    /// remains available. `trace` is the optional Phase 6 E2E latency context
    /// (present only when the trace sink is installed or the env gate is on);
    /// it flows through to the runtime for stage stamping.
    pub(crate) async fn dispatch_keymap_input(
        &self,
        device: crate::capabilities::DeviceHandle,
        screen: ScreenSize,
        event: InputEvent,
        trace: Option<KeymapTraceContext>,
    ) -> ExtensionResult<InputResult> {
        let instance = self
            .keymap_running
            .lock()
            .expect("keymap running map poisoned")
            .get(&super::keymap::keymap_extension_id())
            .copied();
        let Some(instance) = instance else {
            return Ok(InputResult::pass());
        };
        self.keymap_runtime
            .dispatch(instance, device, screen, event, trace)
            .await
    }

    /// Declarative UI backend call (`plugin.call`). The extension must be
    /// Running, and the action must be one of the button actions declared in
    /// the active manifest's declarative UI schema — the manifest is the only
    /// allowed call surface, so a panel cannot invoke arbitrary guest logic.
    /// The lifecycle lock only guards the lookup; the guest call itself runs
    /// after it is released so uninstall/update cannot interleave with a live
    /// instance dispatch.
    pub(crate) async fn call_extension(
        &self,
        id: &ExtensionId,
        action: &str,
        values: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        if action.trim().is_empty() {
            return Err(ExtensionError::CallRejected("action 不能为空".into()));
        }
        // Native 扩展动作（视频工作台实施合同 §5）：动作由扩展边界原生实现时
        // （gamer.yaml `automation.create_draft`），不经 declarative 按钮集合与
        // 常驻实例；Running 状态要求与通用路径一致。生命周期锁只护状态查询，
        // 原生动作本体在锁外执行。
        if let Some(native) = super::native_call_action(id, action, &values, self.store.data_root())
        {
            let snapshot = self.snapshot_for(id)?;
            if snapshot.state() != ExtensionState::Running {
                return Err(invalid_transition(id, "call", snapshot.state()));
            }
            return native;
        }
        let (handle, runtime) = {
            let snapshot = self.snapshot_for(id)?;
            if snapshot.state() != ExtensionState::Running {
                return Err(invalid_transition(id, "call", snapshot.state()));
            }
            let allowed = declarative_actions(snapshot.manifest());
            if !allowed.iter().any(|candidate| candidate == action) {
                return Err(ExtensionError::CallRejected(format!(
                    "action 不在插件 declarative UI 声明的按钮集合内: {action}"
                )));
            }
            let handle = self
                .running
                .lock()
                .expect("extension running map poisoned")
                .get(id)
                .copied()
                .ok_or(ExtensionError::RuntimeUnavailable(
                    "当前进程没有该插件的运行实例",
                ))?;
            (handle, self.runtime.clone())
        };
        let result = runtime.call(handle, action, &values.to_string()).await?;
        serde_json::from_str::<serde_json::Value>(&result)
            .map_err(|error| ExtensionError::Runtime(format!("插件 call 返回值不是 JSON: {error}")))
    }

    pub(crate) fn store(&self) -> &ExtensionStore {
        &self.store
    }

    /// Validate an archive without staging it. The management UI uses this
    /// as the confirmation boundary for source, integrity, and permissions.
    /// Phase 1 免签名：官方与本地安装统一无签名，来源只作展示标注；
    /// 下载完整性由可选 `expected_sha256`（`x-expected-sha256` 头）钉住。
    pub(crate) fn inspect(&self, archive: &[u8]) -> ExtensionResult<ExtensionInspection> {
        self.inspect_with_context(archive, &ExtensionInstallContext::default())
    }

    pub(crate) fn inspect_with_context(
        &self,
        archive: &[u8],
        context: &ExtensionInstallContext,
    ) -> ExtensionResult<ExtensionInspection> {
        let manifest = self.inspect_compatible(archive)?;
        let archive_sha256 = format!("{:x}", Sha256::digest(archive));
        if let Some(expected) = context.expected_sha256.as_deref() {
            let expected = expected.trim().to_ascii_lowercase();
            if expected != archive_sha256 {
                return Err(ExtensionError::ArchiveSha256Mismatch {
                    expected,
                    actual: archive_sha256,
                });
            }
        }
        let installed = self.list()?;
        // Phase 8 §11.2：builtin id 归属/执行形态策略（install/update/inspect
        // 三入口共用同一判定），并暴露 wasm↔builtin 形态变化供确认弹窗提示。
        let execution_change = execution_policy_check(&manifest, &installed)?;
        let permission_diff = permission_diff_for(&manifest, &installed);
        Ok(ExtensionInspection {
            manifest,
            archive_sha256,
            permission_diff,
            execution_change,
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
            snapshots.push(snapshot_from_versions(versions, record)?);
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

    /// 切换 active_version 指针（版本回滚/前滚）。不复制、不删除任何版本
    /// 目录；Running 时拒绝（需先 stop），目标版本未安装报 VersionNotInstalled。
    /// 插件状态保持不变——Enabled 的插件下一次 start 即运行新版本。
    pub(crate) async fn activate_version(
        &self,
        id: &ExtensionId,
        version: &ExtensionVersion,
    ) -> ExtensionResult<ExtensionSnapshot> {
        let _guard = self.operation_lock.lock().await;
        let mut states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let mut record = state_for_versions(id, &versions, states.get(id).cloned())?;
        if record.state.is_running() {
            return Err(invalid_transition(id, "activate", record.state));
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
        record.active_version = Some(version.clone());
        record.last_error = None;
        states.insert(id.clone(), record);
        self.store.write_state(&states)?;
        self.refresh_ui_registry()?;
        self.snapshot_for(id)
    }

    /// 低层状态转移：Installed/Disabled/Failed → Enabled（不启动实例、不注册
    /// runner）。公开 REST 的「启用」走 [`Self::enable_and_start`]；本方法保留
    /// 给 reconcile 与测试作生命周期原语（计划 Phase 5：内部保留 start/stop
    /// 实现方法）。
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

    /// 用户「启用」（V1 计划 Phase 5）：用户操作只有 安装/启用/禁用/更新/卸载——
    /// enable 即表达「我要它工作」，落 Enabled 后**直接启动**（enable → start
    /// 一体化，幂等：Running 时直接返回现状）；启动失败降级 Failed + last_error，
    /// 保留启用意图可重试。`app_context` / `keymap_profile` 与原 start 数据
    /// 通道同形（keymap 专用 profile）。
    pub(crate) async fn enable_and_start(
        &self,
        id: &ExtensionId,
        app_context: Option<crate::core::AppContext>,
        keymap_profile: Option<String>,
    ) -> ExtensionResult<ExtensionSnapshot> {
        {
            let _guard = self.operation_lock.lock().await;
            let mut states = self.store.read_state()?;
            let versions = self.versions_for(id)?;
            let mut record = state_for_versions(id, &versions, states.get(id).cloned())?;
            match record.state {
                ExtensionState::Running => {
                    // 已在运行：启用意图已满足，直接返回现状（幂等）
                    return self.snapshot_for(id);
                }
                ExtensionState::Installed | ExtensionState::Disabled | ExtensionState::Failed => {
                    record.state = ExtensionState::Enabled;
                    record.last_error = None;
                }
                ExtensionState::Enabled => {}
            }
            states.insert(id.clone(), record);
            self.store.write_state(&states)?;
            self.refresh_ui_registry()?;
        }
        self.start_with_context(id, app_context, keymap_profile)
            .await
    }

    /// Disable an extension.  A Running instance is stopped first (ADR-13
    /// disable semantics: the WASM entrypoint ends and every runner the
    /// extension owns is unregistered) instead of the old behaviour of
    /// rejecting disable-while-running.
    pub(crate) async fn disable(&self, id: &ExtensionId) -> ExtensionResult<ExtensionSnapshot> {
        let _guard = self.operation_lock.lock().await;
        let mut states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let mut record = state_for_versions(id, &versions, states.get(id).cloned())?;
        match record.state {
            ExtensionState::Running => {
                self.stop_running_instance(id).await?;
                self.unregister_extension_runners(id).await;
                record.state = ExtensionState::Disabled;
                record.last_error = None;
            }
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
        self.start_with_context(id, None, None).await
    }

    /// `keymap_profile` carries the raw user-selected keymap YAML resolved by
    /// the REST layer; it is consumed only by the keymap Component world and
    /// ignored by every other extension kind.
    pub(crate) async fn start_with_context(
        &self,
        id: &ExtensionId,
        app_context: Option<crate::core::AppContext>,
        keymap_profile: Option<String>,
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
        let handle = if self.instance_free(id) {
            // 无实例执行模型：start 只表示「作为 timer runner 提供方在线」
            // （ADR-13 注册缝），不启动实例；执行由按调用运行时
            // （guest_for_run）在每次运行时惰性实例化。
            None
        } else if super::keymap::is_keymap_extension(id) {
            // keymap 独立 WIT world（keymap-host）：运行时归属 keymap 扩展
            // 边界（is_keymap_extension 判定在 keymap/mod.rs，Phase 4 §7.1
            // 收口——机制层不持有插件 id 字面量），start 持有常驻实例并把
            // profile 原文交给 guest。
            match self
                .keymap_runtime
                .start(KeymapWasmStartRequest {
                    id: id.clone(),
                    version: active.manifest().version().clone(),
                    wasm: active.read_wasm()?,
                    host,
                    app_context,
                    profile: keymap_profile,
                })
                .await
            {
                Ok(handle) => Some(StartHandle::Keymap(handle)),
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
                Ok(handle) => Some(StartHandle::Generic(handle)),
                Err(error) => return self.mark_start_failed(id, states, record, error).await,
            }
        };

        let instance_insert = match handle {
            Some(StartHandle::Generic(handle)) => Some(RunningHandle::Generic(handle)),
            Some(StartHandle::Keymap(handle)) => Some(RunningHandle::Keymap(handle)),
            None => None,
        };

        let mut running_record = record;
        running_record.state = ExtensionState::Running;
        running_record.last_error = None;
        if let Err(error) = self.store.write_state(&{
            let mut next = states.clone();
            next.insert(id.clone(), running_record.clone());
            next
        }) {
            if let Some(handle) = instance_insert {
                let _ = self.stop_running_handle(handle).await;
            }
            return Err(error);
        }
        match instance_insert {
            Some(RunningHandle::Generic(handle)) => {
                self.running
                    .lock()
                    .expect("extension running map poisoned")
                    .insert(id.clone(), handle);
            }
            Some(RunningHandle::Keymap(handle)) => {
                self.keymap_running
                    .lock()
                    .expect("keymap running map poisoned")
                    .insert(id.clone(), handle);
            }
            None => {}
        }
        // ADR-13：进入 Running 即注册该扩展拥有的 runner（owner=扩展 id）并
        // 恢复其缺依赖任务。注册失败不回滚 Running 状态（缺 runner 的后果
        // 由 dispatch 时的 DependencyMissing 语义兜底），只记录告警。
        self.register_extension_runners(id).await;
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
        self.stop_running_instance(id).await?;
        record.state = ExtensionState::Enabled;
        record.last_error = None;
        states.insert(id.clone(), record);
        self.store.write_state(&states)?;
        self.unregister_extension_runners(id).await;
        self.refresh_ui_registry()?;
        self.snapshot_for(id)
    }

    /// P11.7 启动对账：进程重启后，持久化状态遗留 `Running` 的扩展既没有活
    /// 实例也不在 Runner 注册表——其名下任务会一直 `DependencyMissing`，直到
    /// 用户手动 start。对每条 Running 记录执行与 `start` 等价的恢复（实例
    /// 启动 / runner 注册 / UI 贡献刷新）；恢复失败降级为 `Enabled` 并把失败
    /// 原因记入 last_error，绝不阻塞服务启动。组合根在 Scheduler 启动前调用，
    /// 使恢复出的 runner 立即可派发任务。幂等：无 Running 记录时为 no-op。
    pub(crate) async fn reconcile_startup(&self) {
        let running: Vec<ExtensionId> = match self.store.read_state() {
            Ok(states) => states
                .into_iter()
                .filter(|(_, record)| record.state == ExtensionState::Running)
                .map(|(id, _)| id)
                .collect(),
            Err(error) => {
                tracing::warn!(%error, "extension startup reconcile skipped: state unreadable");
                return;
            }
        };
        for id in running {
            // 重启即实例全灭：先把记录降为 Enabled（Running 只描述活实例），
            // 再走与手动 start 完全相同的恢复路径。
            if let Err(error) = self.force_state(&id, ExtensionState::Enabled, None) {
                tracing::warn!(
                    extension = %id,
                    %error,
                    "extension startup reconcile: cannot clear stale Running record"
                );
                continue;
            }
            match self.start(&id).await {
                Ok(_) => {
                    tracing::info!(extension = %id, "extension restored to Running at startup");
                }
                Err(error) => {
                    // start 失败可能已把状态写成 Failed——统一降级 Enabled 并
                    // 记录原因，服务启动不受影响。
                    let _ = self.force_state(&id, ExtensionState::Enabled, Some(error.to_string()));
                    tracing::warn!(
                        extension = %id,
                        %error,
                        "extension startup restore failed; degraded to Enabled"
                    );
                }
            }
        }
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
        // ADR-13 卸载清场：Running 已被上方守卫拒绝，正常路径 owner 名下本就
        // 无 runner（stop/disable 已注销）；此处兜底摘除，幂等 no-op。
        self.unregister_extension_runners(id).await;
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
        // builtin 执行类型必须在宿主注册表中已注册（下载包不能自行扩展注册
        // 表，也不能借 builtin 伪装绕过 WASM guest 校验——计划 §5.2）。
        if let Some(builtin_id) = manifest.execution().builtin_id() {
            if super::builtin::builtin_extension(builtin_id).is_none() {
                return Err(ExtensionError::HostFeatureUnavailable(
                    builtin_id.to_string(),
                ));
            }
        }
        Ok(manifest)
    }

    pub(crate) fn snapshot_for(&self, id: &ExtensionId) -> ExtensionResult<ExtensionSnapshot> {
        let states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let record = state_for_versions(id, &versions, states.get(id).cloned())?;
        snapshot_from_versions(versions, record)
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
            // UI 贡献只在 Running 可见（Phase 1 语义收紧）：Enabled 表示「已
            // 启用但没有活实例/runner」，面板与 ui 资产随 stop/disable 一并
            // 撤销，start 才重新发布。避免「看起来在跑、点进去没有能力」的
            // 半启用态。
            if record.state == ExtensionState::Running {
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

    /// 直写生命周期状态（启动对账专用）：调用点都在启动期或随后即走持锁的
    /// `start`，不经 operation_lock。记录不存在时报 NotInstalled。
    pub(crate) fn force_state(
        &self,
        id: &ExtensionId,
        state: ExtensionState,
        last_error: Option<String>,
    ) -> ExtensionResult<()> {
        let mut states = self.store.read_state()?;
        let Some(record) = states.get_mut(id) else {
            return Err(ExtensionError::NotInstalled { id: id.to_string() });
        };
        record.state = state;
        record.last_error = last_error;
        self.store.write_state(&states)
    }

    async fn stop_running_handle(&self, handle: RunningHandle) -> ExtensionResult<()> {
        match handle {
            RunningHandle::Generic(handle) => self.runtime.stop(handle).await,
            RunningHandle::Keymap(handle) => self.keymap_runtime.stop(handle).await,
        }
    }

    /// Stop the live instance (whichever world it runs in) and clear the
    /// running maps.  State persistence, runner hooks and UI refresh stay
    /// with the callers so `stop` (→ Enabled) and `disable` (→ Disabled)
    /// keep their distinct end states.  Instance-free extensions（按调用惰性
    /// 执行模型）simply have nothing to stop.
    async fn stop_running_instance(&self, id: &ExtensionId) -> ExtensionResult<()> {
        if self.instance_free(id) {
            return Ok(());
        }
        if super::keymap::is_keymap_extension(id) {
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
        self.running
            .lock()
            .expect("extension running map poisoned")
            .remove(id);
        self.keymap_running
            .lock()
            .expect("keymap running map poisoned")
            .remove(id);
        Ok(())
    }

    /// ADR-13 start hook: registration failures never fail the start itself —
    /// the extension is durably Running at this point, and a genuinely missing
    /// runner resurfaces as task-level `DependencyMissing` at dispatch time.
    async fn register_extension_runners(&self, id: &ExtensionId) {
        if let Some(registrar) = &self.runner_registrar {
            if let Err(error) = registrar.extension_started(id.as_str()).await {
                tracing::warn!(extension = %id, %error, "extension runner registration failed");
            }
        }
    }

    /// ADR-13 stop/disable/uninstall hook.  Failures are logged, never
    /// propagated: the instance is already gone at this point and the next
    /// lifecycle transition retries the idempotent unregister.
    async fn unregister_extension_runners(&self, id: &ExtensionId) {
        if let Some(registrar) = &self.runner_registrar {
            if let Err(error) = registrar.extension_stopped(id.as_str()).await {
                tracing::warn!(extension = %id, %error, "extension runner unregister failed");
            }
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
) -> ExtensionResult<ExtensionSnapshot> {
    let active = active_version(&versions, &record)?;
    Ok(ExtensionSnapshot {
        manifest: active.manifest().clone(),
        active_version: active.manifest().version().clone(),
        installed_versions: versions
            .iter()
            .map(|extension| extension.manifest().version().clone())
            .collect(),
        state: record.state,
        last_error: record.last_error,
    })
}

impl ExtensionService {
    pub(crate) fn permission_diff_for(
        &self,
        manifest: &ExtensionManifest,
    ) -> ExtensionResult<PermissionDiff> {
        Ok(permission_diff_for(manifest, &self.list()?))
    }
}

/// 权限增量对照（与已安装 active 版本的权限集比较）。
fn permission_diff_for(
    manifest: &ExtensionManifest,
    installed: &[ExtensionSnapshot],
) -> PermissionDiff {
    let current = installed
        .iter()
        .find(|snapshot| snapshot.id() == manifest.id())
        .map(|snapshot| snapshot.manifest().permissions().names())
        .unwrap_or_default();
    let requested = manifest.permissions().names();
    PermissionDiff {
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
    }
}

/// Phase 8 §11.2 执行形态与 builtin id 归属策略（install/update/inspect 三
/// 入口共用）：
///
/// 1. **wasm 包不得占用宿主内置扩展 id**（官方 builtin 归宿主所有——全新
///    安装是「借官方 id 分发未知实现」，已装 builtin 再装 wasm 是「降级为
///    未知实现」，都拒绝）；官方迁移路径 wasm→builtin 放行。
/// 2. **builtin 包的插件 id 必须与注册实现 id 一致**（实现即扩展，不允许
///    别名包借用宿主实现）。
/// 3. 同 id 形态变化（wasm→builtin 放行路径）在 inspection 中显式暴露
///    （`execution_change`），供确认弹窗明确提示。
fn execution_policy_check(
    manifest: &ExtensionManifest,
    installed: &[ExtensionSnapshot],
) -> ExtensionResult<Option<ExecutionChange>> {
    let incoming = manifest.execution().kind();
    if incoming == super::manifest::ExecutionKind::Builtin {
        let declared = manifest
            .execution()
            .builtin_id()
            .expect("manifest 校验保证 builtin 必带 builtin_id");
        if manifest.id().as_str() != declared {
            return Err(ExtensionError::InvalidManifest(format!(
                "builtin 扩展的插件 id（{}）必须与宿主注册实现 id（{declared}）一致",
                manifest.id()
            )));
        }
    }
    if incoming == super::manifest::ExecutionKind::Wasm
        && super::builtin::is_builtin_extension(manifest.id())
    {
        let message = match installed
            .iter()
            .find(|snapshot| snapshot.id() == manifest.id())
            .map(|snapshot| snapshot.manifest().execution().kind())
        {
            Some(super::manifest::ExecutionKind::Builtin) => format!(
                "宿主内置插件 {} 不得更新为 wasm 包重新实现（builtin 归宿主所有，host_feature_unavailable 风险）",
                manifest.id()
            ),
            _ => format!(
                "插件 id {} 是宿主内置扩展（builtin），本地 wasm 包不得占用该 id；请更换插件 id",
                manifest.id()
            ),
        };
        return Err(ExtensionError::InvalidManifest(message));
    }
    let current = installed
        .iter()
        .find(|snapshot| snapshot.id() == manifest.id())
        .map(|snapshot| snapshot.manifest().execution().kind());
    Ok(current
        .filter(|current| *current != incoming)
        .map(|from| ExecutionChange { from, to: incoming }))
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

/// Button actions exposed by a manifest's declarative UI contributions.
fn declarative_actions(manifest: &ExtensionManifest) -> Vec<String> {
    manifest
        .ui()
        .iter()
        .filter_map(|contribution| contribution.schema())
        .flat_map(|schema| schema.fields())
        .filter_map(|field| field.action())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    use zip::write::SimpleFileOptions;

    use crate::config::Config;
    use crate::core::AppContext;
    use crate::resources::PackageStore;
    use crate::run_manager::RunManager;
    use crate::scheduler::Scheduler;
    use crate::store::{Db, Store};
    use crate::timer_core::{Task, TaskSchedule, TaskState};

    use super::super::gamer_yaml::{YAML_EXTENSION_ID, YAML_EXTENSION_MANIFEST_TOML};

    struct UnreachableExecutor;

    impl crate::run_manager::RunExecutor for UnreachableExecutor {
        fn prepare<'a>(
            &'a self,
            _: &'a crate::core::RunContext,
            _: &'a crate::core::RunRequest,
        ) -> futures_util::future::BoxFuture<'a, anyhow::Result<()>> {
            unreachable!("启动对账测试不应触发真实运行")
        }

        fn execute<'a>(
            &'a self,
            _: &'a crate::core::RunContext,
            _: &'a crate::core::RunRequest,
            _: bool,
            _: Arc<std::sync::atomic::AtomicBool>,
        ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
            unreachable!("启动对账测试不应触发真实运行")
        }

        fn acquire(
            &self,
            _: &crate::core::RunContext,
        ) -> anyhow::Result<Box<dyn crate::core::ActivityLease>> {
            unreachable!("启动对账测试不应触发真实运行")
        }
    }

    fn zip_archive(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut archive = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut archive));
            let options = SimpleFileOptions::default();
            for (name, bytes) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        archive
    }

    /// gamer.yaml 安装包：无实例执行模型下 `start` 不读 guest 字节，占位
    /// wasm 即可通过安装；真实 v3 运行验收在 gamer_yaml 的端到端测试。
    fn gamer_yaml_archive() -> Vec<u8> {
        zip_archive(&[
            (
                "manifest.toml",
                YAML_EXTENSION_MANIFEST_TOML.as_bytes().to_vec(),
            ),
            ("plugin.wasm", b"\0asm\x01\0\0\0".to_vec()),
        ])
    }

    fn snapshot_state(service: &ExtensionService, id: &ExtensionId) -> ExtensionState {
        service
            .list()
            .unwrap()
            .into_iter()
            .find(|snapshot| snapshot.id() == id)
            .expect("extension snapshot")
            .state()
    }

    /// 复现崩溃窗口：不经生命周期直接把磁盘状态写成 Running（实例与 runner
    /// 均已不存在——重启后遗留记录的精确形态）。
    fn mark_running(service: &ExtensionService, id: &ExtensionId) {
        let mut states = service.store().read_state().unwrap();
        states.get_mut(id).unwrap().state = ExtensionState::Running;
        service.store().write_state(&states).unwrap();
    }

    /// P11.7 启动对账（ADR-13 验收）：标记 Running → 重建服务 → reconcile
    /// → runner 重注册、DependencyMissing 任务恢复 Active（唤醒游标经 cron
    /// 重算，可被 Scheduler 派发）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reconcile_startup_restores_running_extension_and_resumes_tasks() {
        let data = tempfile::tempdir().expect("无法创建启动对账测试临时目录");
        let cfg = Config {
            data_dir: data.path().to_path_buf(),
            ..Default::default()
        };
        let db: Db = Arc::new(Store::open(&cfg).unwrap());
        let scripts = Arc::new(PackageStore::open(&cfg).unwrap());

        // ——「上一次进程」：真实生命周期跑到 Running，然后 stop 留下
        //   DependencyMissing 任务，最后把磁盘状态写回 Running 模拟崩溃窗口。
        let scheduler1 = Arc::new(Scheduler::new(db.clone()));
        let registrar1 = Arc::new(super::super::gamer_yaml::YamlTimerRunnerRegistrar::new(
            scheduler1.clone(),
            db.clone(),
            Arc::new(RunManager::new(Arc::new(UnreachableExecutor))),
            scripts.clone(),
        ));
        let service1 = ExtensionService::for_data_root(
            data.path(),
            crate::capabilities::CapabilityRegistry::default(),
        )
        .with_runner_registrar(registrar1);
        service1.install(&gamer_yaml_archive()).await.unwrap();
        let id = ExtensionId::parse(YAML_EXTENSION_ID).unwrap();
        service1.enable(&id).await.unwrap();
        service1.start(&id).await.unwrap();

        let schedule = TaskSchedule::new("cron", serde_json::json!({"expression": "0 8 * * *"}))
            .expect("cron schedule");
        let mut task = Task::new(
            "task-reconcile",
            "Reconcile",
            AppContext::for_test("device-1", "com.example.game").unwrap(),
            YAML_EXTENSION_ID,
            "com.example.game/daily.yaml",
            serde_json::json!({"args": {}}),
            schedule,
        )
        .unwrap();
        task.next_wakeup = Some(chrono::Utc::now() + chrono::Duration::hours(1));
        db.upsert_timer_task_async(&task).await.unwrap();

        service1.stop(&id).await.unwrap();
        let suspended = db
            .get_timer_task_async("task-reconcile")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(suspended.state, TaskState::DependencyMissing);
        mark_running(&service1, &id);
        drop(service1);
        drop(scheduler1);

        // ——「重启后的进程」：裸 runner 注册表 + 遗留 Running 记录。
        let scheduler2 = Arc::new(Scheduler::new(db.clone()));
        assert!(scheduler2.runners().is_empty(), "重启后 runner 注册表为空");
        let registrar2 = Arc::new(super::super::gamer_yaml::YamlTimerRunnerRegistrar::new(
            scheduler2.clone(),
            db.clone(),
            Arc::new(RunManager::new(Arc::new(UnreachableExecutor))),
            scripts.clone(),
        ));
        let service2 = ExtensionService::for_data_root(
            data.path(),
            crate::capabilities::CapabilityRegistry::default(),
        )
        .with_runner_registrar(registrar2);
        assert_eq!(
            snapshot_state(&service2, &id),
            ExtensionState::Running,
            "重启后遗留 Running 记录"
        );

        service2.reconcile_startup().await;

        assert_eq!(
            snapshot_state(&service2, &id),
            ExtensionState::Running,
            "对账后恢复 Running"
        );
        let runners = scheduler2.runners();
        assert_eq!(runners.len(), 1, "对账后 runner 已重注册");
        assert_eq!(runners[0].runner_id, YAML_EXTENSION_ID);
        assert_eq!(runners[0].owner_extension_id, YAML_EXTENSION_ID);

        let resumed = db
            .get_timer_task_async("task-reconcile")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            resumed.state,
            TaskState::Active,
            "dependency_missing 已恢复"
        );
        assert!(resumed.suspend_reason.is_none());
        let next = resumed.next_wakeup.expect("恢复必须重算唤醒游标");
        assert!(
            next > chrono::Utc::now(),
            "唤醒游标为未来 cron 时刻，可派发"
        );
    }

    /// 启动对账降级路径：guest 启动失败（坏 wasm）→ 状态降级 Enabled 且
    /// 记录原因，服务继续可用。
    #[tokio::test]
    async fn reconcile_startup_degrades_ungastartable_extension_to_enabled() {
        let data = tempfile::tempdir().expect("无法创建降级路径测试临时目录");
        let service = ExtensionService::for_data_root(
            data.path(),
            crate::capabilities::CapabilityRegistry::default(),
        );

        let archive = zip_archive(&[
            (
                "manifest.toml",
                br#"manifest_version = 2
id = "com.example.broken"
version = "1.0.0"
name = "Broken guest"
entry = "plugin.wasm"
"#
                .to_vec(),
            ),
            // 合法 wasm 模块头、非 component：常驻实例模型下 start 必败。
            ("plugin.wasm", b"\0asm\x01\0\0\0".to_vec()),
        ]);
        service.install(&archive).await.unwrap();
        let id = ExtensionId::parse("com.example.broken").unwrap();
        service.enable(&id).await.unwrap();
        mark_running(&service, &id);

        service.reconcile_startup().await;

        let snapshot = service
            .list()
            .unwrap()
            .into_iter()
            .find(|snapshot| snapshot.id() == &id)
            .unwrap();
        assert_eq!(
            snapshot.state(),
            ExtensionState::Enabled,
            "恢复失败必须降级 Enabled（不是 Failed/Running）"
        );
        assert!(snapshot.last_error().is_some(), "降级时记录失败原因");
        // 服务继续可用：列表仍可读。
        assert_eq!(service.list().unwrap().len(), 1);
    }

    // ---------- Phase 8 §11.2 插件更新语义 ----------

    const VALID_WASM: &[u8] = b"\0asm\x01\0\0\0";

    fn wasm_manifest_bytes(id: &str, version: &str) -> Vec<u8> {
        format!(
            "manifest_version = 2\nid = \"{id}\"\nversion = \"{version}\"\nname = \"W\"\nentry = \"plugin.wasm\"\n"
        )
        .into_bytes()
    }

    fn builtin_manifest_bytes(id: &str, version: &str, builtin_id: &str) -> Vec<u8> {
        format!(
            "manifest_version = 2\nid = \"{id}\"\nversion = \"{version}\"\nname = \"B\"\n[execution]\nkind = \"builtin\"\nbuiltin_id = \"{builtin_id}\"\n"
        )
        .into_bytes()
    }

    /// wasm 安装包（manifest + 占位 guest 字节；安装路径要求 magic 校验）。
    fn wasm_archive(id: &str, version: &str) -> Vec<u8> {
        zip_of(&[
            ("manifest.toml", wasm_manifest_bytes(id, version)),
            ("plugin.wasm", VALID_WASM.to_vec()),
        ])
    }

    fn zip_of(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        zip_archive(
            &entries
                .iter()
                .map(|(name, bytes)| (*name, bytes.clone()))
                .collect::<Vec<_>>(),
        )
    }

    /// 本地 wasm 包不得占用宿主内置扩展 id（全新安装即拒绝——防借官方 id
    /// 分发未知实现；计划 §11.2「未知 ID 不得提升为官方内置」的 wasm 侧）。
    #[tokio::test]
    async fn wasm_package_cannot_usurp_builtin_id_on_fresh_install() {
        let temp = tempfile::tempdir().unwrap();
        let service = ExtensionService::for_data_root(
            temp.path(),
            crate::capabilities::CapabilityRegistry::default(),
        );
        let archive = wasm_archive(super::super::video::VIDEO_EXTENSION_ID, "1.0.0");
        let error = service.install(&archive).await.unwrap_err();
        assert!(
            matches!(error, ExtensionError::InvalidManifest(ref m) if m.contains("内置")),
            "usurp 必须以 InvalidManifest 拒绝，得到 {error:?}"
        );
        assert!(service.list().unwrap().is_empty(), "被拒安装不得留状态");
    }

    /// builtin 包的插件 id 必须与注册实现一致（不允许别名包借用宿主实现）。
    #[tokio::test]
    async fn builtin_package_id_must_match_registered_builtin_id() {
        let temp = tempfile::tempdir().unwrap();
        let service = ExtensionService::for_data_root(
            temp.path(),
            crate::capabilities::CapabilityRegistry::default(),
        );
        let archive = zip_of(&[(
            "manifest.toml",
            builtin_manifest_bytes("com.other.alias", "1.0.0", "gamer.video"),
        )]);
        let error = service.install(&archive).await.unwrap_err();
        assert!(
            matches!(error, ExtensionError::InvalidManifest(ref m) if m.contains("一致")),
            "别名 builtin 包必须拒绝，得到 {error:?}"
        );
    }

    /// 官方迁移路径 wasm→builtin 放行，且 inspect 显式暴露 execution_change
    /// （明确提示）；builtin→wasm「降级」一律拒绝。
    #[tokio::test]
    async fn execution_change_wasm_to_builtin_is_surfaced_and_downgrade_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let service = ExtensionService::for_data_root(
            temp.path(),
            crate::capabilities::CapabilityRegistry::default(),
        );
        let id = ExtensionId::parse(super::super::video::VIDEO_EXTENSION_ID).unwrap();

        // 种一个「旧形态」wasm 版本（绕过服务层策略直接落盘——模拟 Phase 2
        // 之前遗留安装），再走官方 builtin 更新。
        let legacy = wasm_archive(super::super::video::VIDEO_EXTENSION_ID, "0.9.0");
        service.store().install_archive(&legacy).unwrap();
        let mut states = service.store().read_state().unwrap();
        states.insert(
            id.clone(),
            ExtensionRecord::new(id.clone(), ExtensionVersion::parse("0.9.0").unwrap()),
        );
        service.store().write_state(&states).unwrap();

        let builtin_update = zip_of(&[(
            "manifest.toml",
            builtin_manifest_bytes("gamer.video", "1.0.0", "gamer.video"),
        )]);
        let inspected = service
            .inspect(&builtin_update)
            .unwrap_or_else(|e| panic!("wasm→builtin 迁移 inspect 必须放行: {e}"));
        let change = inspected.execution_change().expect("必须暴露形态变化");
        assert_eq!(
            format!("{:?}", change.from),
            "Wasm",
            "from 必须是旧形态 wasm"
        );
        assert_eq!(format!("{:?}", change.to), "Builtin");

        let updated = service.update(&builtin_update).await.unwrap();
        assert_eq!(updated.active_version().as_str(), "1.0.0");

        // builtin→wasm 降级：拒绝（builtin 归宿主所有，不得被 wasm 重实现）
        let downgrade = wasm_archive("gamer.video", "1.1.0");
        let error = service.update(&downgrade).await.unwrap_err();
        assert!(
            matches!(error, ExtensionError::InvalidManifest(ref m) if m.contains("内置")),
            "降级 wasm 必须拒绝，得到 {error:?}"
        );
        assert_eq!(
            service
                .snapshot_for(&id)
                .unwrap()
                .active_version()
                .as_str()
                .to_string(),
            "1.0.0",
            "被拒更新不得改动 active_version"
        );
    }

    /// 失败更新不破坏已安装可用版本：坏归档 → 拒绝，旧版本仍 active 可用。
    #[tokio::test]
    async fn failed_update_keeps_previous_version_active() {
        let temp = tempfile::tempdir().unwrap();
        let service = ExtensionService::for_data_root(
            temp.path(),
            crate::capabilities::CapabilityRegistry::default(),
        );
        let id = ExtensionId::parse("com.example.update").unwrap();
        service
            .install(&wasm_archive("com.example.update", "1.0.0"))
            .await
            .unwrap();
        service.enable(&id).await.unwrap();

        // 坏归档（非 zip 字节）
        assert!(service.update(b"not-a-zip").await.is_err());
        // 未安装 id 的归档 → update 侧 NotInstalled（新 id 不允许借 update 落地）
        let mismatched = wasm_archive("com.example.other", "2.0.0");
        assert!(service.update(&mismatched).await.is_err());

        let snapshot = service.snapshot_for(&id).unwrap();
        assert_eq!(snapshot.active_version().as_str(), "1.0.0");
        assert_eq!(snapshot.installed_versions().len(), 1, "不留失败半版本");
        assert_eq!(snapshot.state(), ExtensionState::Enabled);
    }

    /// 同版本重复 update → AlreadyInstalled（版本目录不可变语义），active
    /// 版本保持不变；回滚走 activate_version 指针切换。
    #[tokio::test]
    async fn same_version_update_conflicts_and_activate_switches_back() {
        let temp = tempfile::tempdir().unwrap();
        let service = ExtensionService::for_data_root(
            temp.path(),
            crate::capabilities::CapabilityRegistry::default(),
        );
        let id = ExtensionId::parse("com.example.rollback").unwrap();
        service
            .install(&wasm_archive("com.example.rollback", "1.0.0"))
            .await
            .unwrap();
        // 1.0.0 → 2.0.0：update 切 active
        service
            .update(&wasm_archive("com.example.rollback", "2.0.0"))
            .await
            .unwrap();
        service.enable(&id).await.unwrap();

        // 同版本 update → AlreadyInstalled（1.0.0 已在盘，目录不可变）
        let error = service
            .update(&wasm_archive("com.example.rollback", "1.0.0"))
            .await
            .unwrap_err();
        assert!(matches!(error, ExtensionError::AlreadyInstalled { .. }));
        assert_eq!(
            service.snapshot_for(&id).unwrap().active_version().as_str(),
            "2.0.0",
            "被拒 update 不切 active_version"
        );

        // 显式回滚：activate_version 切回 1.0.0（不复制不删除）
        let rolled = service
            .activate_version(&id, &ExtensionVersion::parse("1.0.0").unwrap())
            .await
            .unwrap();
        assert_eq!(rolled.active_version().as_str(), "1.0.0");
        assert_eq!(rolled.state(), ExtensionState::Enabled);
        assert_eq!(rolled.installed_versions().len(), 2, "两个版本都保留");
    }
}
