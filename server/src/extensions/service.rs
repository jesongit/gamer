//! Extension lifecycle service. It owns transitions; the store only owns bytes
//! and durable metadata, and the runtime only owns an optional instance.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::capabilities::CapabilityRegistry;

use super::archive::inspect_archive;
use super::error::{ExtensionError, ExtensionResult};
use super::host_api::{HostApi, HostApiCatalog};
use super::manifest::ExtensionManifest;
use super::model::{ExtensionId, ExtensionRecord, ExtensionState, ExtensionVersion};
use super::native_public_actions;
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

/// Opaque capability for a plugin-originated cross-extension call.
///
/// The caller id and Package Context are deliberately private.  A call is
/// admitted only while the token still belongs to the current-process live
/// instance/runner that issued it; restarting or stopping the extension
/// invalidates every previously issued context.
#[derive(Clone, Debug)]
pub(crate) struct PluginCallContext {
    caller: ExtensionId,
    token: Uuid,
    app_context: Option<crate::core::AppContext>,
}

struct ProcessRunning {
    token: Uuid,
    app_context: Option<crate::core::AppContext>,
}

/// Capability proving that the service has already completed its lifecycle,
/// state, and permission checks. The native dispatcher receives this token so
/// its low-level function cannot be used as a security boundary by another
/// crate module.
pub(crate) struct NativeDispatchPermit {
    _private: (),
}

impl NativeDispatchPermit {
    fn new() -> Self {
        Self { _private: () }
    }
}

/// 单条依赖的实时状态（快照/插件中心提示用，简化计划 Phase 3）：
/// `satisfied` = 已安装 + 版本兼容 + 目标 Running（可被调用/可作为启动前提）。
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct DependencyStatus {
    pub(crate) id: String,
    pub(crate) version_req: String,
    pub(crate) required: bool,
    pub(crate) installed: bool,
    pub(crate) version: Option<String>,
    pub(crate) state: Option<ExtensionState>,
    pub(crate) satisfied: bool,
    /// 可选依赖缺失时给 UI 的降级提示（必需依赖缺失走启动错误）。
    pub(crate) note: Option<String>,
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
    /// Per-extension call gates. A call holds a read lease while its actual
    /// native/WASM operation runs; stop/disable/uninstall take the write lease
    /// before changing lifecycle state. This drains in-flight calls without
    /// holding the global lifecycle lock across long-running work.
    call_gates: std::sync::Mutex<HashMap<ExtensionId, Arc<RwLock<()>>>>,
    /// Startup reconciliation is a process-level recovery pass. Once it has
    /// run, a later call must not reinterpret the now-live Running records as
    /// stale and create duplicate instances or runner registrations.
    startup_reconciled: std::sync::atomic::AtomicBool,
    /// Current-process live instance/runner ownership. A durable `Running`
    /// record is otherwise stale after restart; the opaque token also binds
    /// plugin-to-plugin calls to the exact start handshake that issued them.
    process_running: std::sync::Mutex<HashMap<ExtensionId, ProcessRunning>>,
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
            call_gates: std::sync::Mutex::new(HashMap::new()),
            startup_reconciled: std::sync::atomic::AtomicBool::new(false),
            process_running: std::sync::Mutex::new(HashMap::new()),
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

    fn call_gate(&self, id: &ExtensionId) -> Arc<RwLock<()>> {
        self.call_gates
            .lock()
            .expect("extension call gates poisoned")
            .entry(id.clone())
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone()
    }

    fn is_process_running(&self, id: &ExtensionId) -> bool {
        self.process_running
            .lock()
            .expect("process running set poisoned")
            .contains_key(id)
    }

    fn mark_process_running(&self, id: &ExtensionId, app_context: Option<crate::core::AppContext>) {
        self.process_running
            .lock()
            .expect("process running set poisoned")
            .insert(
                id.clone(),
                ProcessRunning {
                    token: Uuid::new_v4(),
                    app_context,
                },
            );
    }

    fn clear_process_running(&self, id: &ExtensionId) {
        self.process_running
            .lock()
            .expect("process running set poisoned")
            .remove(id);
    }

    /// Verify the process-owned execution surface, not just the durable
    /// state.  Instance-free extensions use a successfully registered runner
    /// (or a host builtin) as their live surface; resident extensions must
    /// still have their actual runtime handle.
    fn has_live_instance_or_runner(&self, id: &ExtensionId) -> bool {
        if !self.is_process_running(id) {
            return false;
        }
        if self.instance_free(id) {
            return true;
        }
        if super::keymap::is_keymap_extension(id) {
            self.keymap_running
                .lock()
                .expect("keymap running map poisoned")
                .contains_key(id)
        } else {
            self.running
                .lock()
                .expect("extension running map poisoned")
                .contains_key(id)
        }
    }

    /// A durable `Running` record is not enough to admit work: after a
    /// process restart it describes the previous process instance. The
    /// current-process ownership marker is published only after runtime,
    /// runner, and UI startup has completed.
    fn require_current_process_running(
        &self,
        id: &ExtensionId,
        state: ExtensionState,
        operation: &'static str,
    ) -> ExtensionResult<()> {
        if state != ExtensionState::Running {
            return Err(invalid_transition(id, operation, state));
        }
        if !self.has_live_instance_or_runner(id) {
            return Err(ExtensionError::RuntimeUnavailable(
                "当前进程没有该插件的运行实例",
            ));
        }
        Ok(())
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
        let gate = self.call_gate(id);
        let _call_lease = gate.read().await;
        self.load_guest_for_run(id).await
    }

    async fn load_guest_for_run(&self, id: &ExtensionId) -> ExtensionResult<(Vec<u8>, HostApi)> {
        let _guard = self.operation_lock.lock().await;
        let states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let record = state_for_versions(id, &versions, states.get(id).cloned())?;
        self.require_current_process_running(id, record.state, "run")?;
        let active = active_version(&versions, &record)?;
        let host = HostApi::for_manifest(
            self.capabilities.clone(),
            self.host_api.clone(),
            active.manifest(),
        )?;
        Ok((active.read_wasm()?, host))
    }

    /// Run a per-call guest while holding the same read lease used by native
    /// and declarative calls. The lease spans the future, so stop/disable/
    /// uninstall cannot make a new lifecycle state visible while an admitted
    /// guest operation is still using its Package/capability context.
    pub(crate) async fn with_guest_for_run<F, Fut, T>(
        &self,
        id: &ExtensionId,
        operation: F,
    ) -> ExtensionResult<T>
    where
        F: FnOnce(Vec<u8>, HostApi) -> Fut,
        Fut: std::future::Future<Output = ExtensionResult<T>>,
    {
        let gate = self.call_gate(id);
        let _call_lease = gate.read().await;
        let (wasm, host) = self.load_guest_for_run(id).await?;
        operation(wasm, host).await
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
        let id = super::keymap::keymap_extension_id();
        let gate = self.call_gate(&id);
        let _call_lease = gate.read().await;
        let instance = self
            .keymap_running
            .lock()
            .expect("keymap running map poisoned")
            .get(&id)
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
    ///
    /// This method is the authenticated user-management surface. Plugin
    /// callers must use `call_extension_from_plugin`, which has a separate
    /// trusted caller and Package Context contract.
    pub(crate) async fn call_extension(
        &self,
        id: &ExtensionId,
        action: &str,
        values: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        if action.trim().is_empty() {
            return Err(ExtensionError::CallRejected("action 不能为空".into()));
        }
        let gate = self.call_gate(id);
        let _call_lease = gate.read().await;
        let snapshot = self.snapshot_for(id)?;
        self.require_current_process_running(id, snapshot.state(), "call")?;

        // Native actions must only be recognized here, after the lifecycle
        // gate and state check. The side-effecting dispatcher is intentionally
        // called only after authorization has completed.
        if super::is_public_native_action(id, action) {
            let host = HostApi::for_manifest(
                self.capabilities.clone(),
                self.host_api.clone(),
                snapshot.manifest(),
            )?;
            let permissions = super::native_action_required_permissions(id, action)
                .ok_or_else(|| ExtensionError::CallRejected("native action 权限契约缺失".into()))?;
            for permission in permissions {
                host.authorize(*permission)?;
            }
            let permit = NativeDispatchPermit::new();
            return super::native_call_action(&permit, id, action, &values, self.store.data_root())
                .ok_or_else(|| {
                    ExtensionError::CallRejected("native action 分发实现缺失".into())
                })?;
        }

        let (handle, runtime) = {
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

    /// Issue an opaque cross-extension call context from the current live
    /// instance/runner. The returned value carries the AppContext captured by
    /// the start handshake; callers cannot choose a caller id or Package
    /// Context in the call request.
    pub(crate) async fn plugin_call_context(
        &self,
        id: &ExtensionId,
    ) -> ExtensionResult<PluginCallContext> {
        let gate = self.call_gate(id);
        let _call_lease = gate.read().await;
        let snapshot = self.snapshot_for(id)?;
        self.require_current_process_running(id, snapshot.state(), "call")?;
        let (token, app_context) = {
            let process_running = self
                .process_running
                .lock()
                .expect("process running set poisoned");
            let process = process_running
                .get(id)
                .ok_or(ExtensionError::RuntimeUnavailable(
                    "当前进程没有该插件的运行实例",
                ))?;
            (process.token, process.app_context.clone())
        };
        Ok(PluginCallContext {
            caller: id.clone(),
            token,
            app_context,
        })
    }

    /// Controlled in-process cross-extension call seam. The caller identity
    /// comes from the service-issued token for a live instance/runner and is
    /// never read from `values`. Only native actions with an explicit catalog
    /// caller are exposed here until a general plugin-to-plugin contract is
    /// needed. Package-scoped actions must use the AppContext captured by the
    /// caller's start handshake.
    #[allow(
        dead_code,
        reason = "the host-owned seam is the extension-to-extension integration point; current production callers are added by extension boundaries"
    )]
    pub(crate) async fn call_extension_from_plugin(
        &self,
        context: &PluginCallContext,
        target: &ExtensionId,
        action: &str,
        values: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        let caller = &context.caller;
        let caller_gate = self.call_gate(caller);
        let _caller_lease = caller_gate.read().await;
        let caller_app_context = {
            let process_running = self
                .process_running
                .lock()
                .expect("process running set poisoned");
            process_running
                .get(caller)
                .filter(|process| process.token == context.token)
                .map(|process| process.app_context.clone())
                .ok_or(ExtensionError::RuntimeUnavailable("插件调用上下文已失效"))?
        };
        let caller_snapshot = self.snapshot_for(caller)?;
        self.require_current_process_running(caller, caller_snapshot.state(), "call")?;
        let caller_host = HostApi::for_manifest(
            self.capabilities.clone(),
            self.host_api.clone(),
            caller_snapshot.manifest(),
        )?;
        let expected = super::native_action_expected_caller(target, action).ok_or_else(|| {
            ExtensionError::CallRejected(format!(
                "插件调用未公开：目标 {} 没有带 caller 契约的 native action {action}",
                target
            ))
        })?;
        if expected != caller.as_str() {
            return Err(ExtensionError::CallRejected(format!(
                "插件调用方 {} 不匹配动作 {action} 要求的调用方 {expected}",
                caller
            )));
        }
        let caller_permissions = super::native_action_caller_permissions(target, action)
            .ok_or_else(|| ExtensionError::CallRejected("native action 权限契约缺失".into()))?;
        for permission in caller_permissions {
            caller_host.authorize(*permission)?;
        }
        if super::native_action_requires_package_context(target, action) {
            let expected_package = caller_app_context
                .as_ref()
                .and_then(|context| context.content_package.as_ref())
                .map(crate::core::AppPackageId::as_str);
            let requested_package = values.get("package_id").and_then(serde_json::Value::as_str);
            if expected_package.is_none() || requested_package != expected_package {
                return Err(ExtensionError::CallRejected(
                    "插件调用的 Package Context 缺失或与动作入参不一致".into(),
                ));
            }
        }
        self.call_extension(target, action, values).await
    }

    pub(crate) fn store(&self) -> &ExtensionStore {
        &self.store
    }

    // -----------------------------------------------------------------------
    // 插件依赖（简化计划 Phase 3）：最小必需/可选依赖。不做自动下载、自动
    // 启用或复杂依赖图——必需依赖是启动门禁，可选依赖只产生降级提示。
    // -----------------------------------------------------------------------

    /// 单条依赖的实时状态：已安装？版本兼容？目标 Running？
    fn dependency_status(&self, dep: &super::manifest::ExtensionDependency) -> DependencyStatus {
        let installed = self.list().ok().and_then(|all| {
            all.into_iter()
                .find(|snapshot| snapshot.id().as_str() == dep.id().as_str())
        });
        let (version, state) = match &installed {
            Some(snapshot) => (
                Some(snapshot.active_version().to_string()),
                Some(snapshot.state()),
            ),
            None => (None, None),
        };
        let version_ok = installed.as_ref().is_some_and(|snapshot| {
            dep.version_req()
                .matches(snapshot.active_version().semver())
        });
        let running = state.is_some_and(ExtensionState::is_running)
            && self.has_live_instance_or_runner(dep.id());
        let satisfied = version_ok && running;
        let note = (!satisfied).then(|| {
            if installed.is_none() {
                "依赖未安装".to_string()
            } else if !version_ok {
                "依赖版本不兼容".to_string()
            } else if !running {
                "依赖未在当前进程运行".to_string()
            } else {
                "依赖未启用".to_string()
            }
        });
        DependencyStatus {
            id: dep.id().to_string(),
            version_req: dep.version_req().to_string(),
            required: dep.required(),
            installed: installed.is_some(),
            version,
            state,
            satisfied,
            note,
        }
    }

    /// 依赖状态报告（快照附带；插件中心/面板据此做缺依赖提示与可选能力降级）。
    pub(crate) fn dependency_report(&self, manifest: &ExtensionManifest) -> Vec<DependencyStatus> {
        manifest
            .dependencies()
            .iter()
            .map(|dep| self.dependency_status(dep))
            .collect()
    }

    /// 启动门禁：必需依赖必须已安装、版本兼容且处于 Running；可选依赖缺失
    /// 不阻止启动（基础功能照常，能力由调用方按 `dependency_report` 降级）。
    /// 不自动下载、不自动启用其他插件。
    fn check_required_dependencies(
        &self,
        id: &ExtensionId,
        manifest: &ExtensionManifest,
    ) -> ExtensionResult<()> {
        for dep in manifest.dependencies().iter().filter(|dep| dep.required()) {
            let status = self.dependency_status(dep);
            if status.satisfied {
                continue;
            }
            let reason = if !status.installed {
                format!("必需依赖 {} 未安装（请在插件中心安装并启用）", dep.id())
            } else if status.version.as_deref().is_none_or(|text| {
                ExtensionVersion::parse(text)
                    .map(|version| !dep.version_req().matches(version.semver()))
                    .unwrap_or(true)
            }) {
                format!(
                    "必需依赖 {} 版本不兼容：已装 {}，要求 {}",
                    dep.id(),
                    status.version.as_deref().unwrap_or("?"),
                    dep.version_req()
                )
            } else {
                format!("必需依赖 {} 未启用（请在插件中心启用）", dep.id())
            };
            return Err(ExtensionError::DependencyUnsatisfied {
                id: id.to_string(),
                reason,
            });
        }
        Ok(())
    }

    /// 必需依赖循环检测：从本插件出发沿「必需依赖」边走已安装插件图，回到
    /// 自身即拒绝启动（否则双方互等，永远无法启动且报因难寻）。可选依赖
    /// 不参与（它从不阻塞启动）。
    fn check_dependency_cycles(
        &self,
        id: &ExtensionId,
        manifest: &ExtensionManifest,
    ) -> ExtensionResult<()> {
        let mut manifests = BTreeMap::<ExtensionId, ExtensionManifest>::new();
        for snapshot in self.list()? {
            manifests.insert(snapshot.id().clone(), snapshot.manifest().clone());
        }
        // `manifest` is the active manifest currently being started. Keeping
        // it authoritative also makes the check correct when a caller is
        // validating an incoming active version before it is persisted.
        manifests.insert(id.clone(), manifest.clone());

        fn visit(
            current: &ExtensionId,
            manifests: &BTreeMap<ExtensionId, ExtensionManifest>,
            visiting: &mut BTreeSet<ExtensionId>,
            visited: &mut BTreeSet<ExtensionId>,
            path: &mut Vec<ExtensionId>,
        ) -> Option<Vec<ExtensionId>> {
            if let Some(start) = path.iter().position(|item| item == current) {
                let mut cycle = path[start..].to_vec();
                cycle.push(current.clone());
                return Some(cycle);
            }
            if visited.contains(current) || !visiting.insert(current.clone()) {
                return None;
            }
            path.push(current.clone());
            let result = manifests.get(current).and_then(|manifest| {
                manifest
                    .dependencies()
                    .iter()
                    .filter(|dependency| dependency.required())
                    .find_map(|dependency| {
                        visit(&dependency.id().clone(), manifests, visiting, visited, path)
                    })
            });
            path.pop();
            visiting.remove(current);
            visited.insert(current.clone());
            result
        }

        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();
        if let Some(cycle) = visit(id, &manifests, &mut visiting, &mut visited, &mut Vec::new()) {
            let reason = cycle
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("→");
            return Err(ExtensionError::DependencyUnsatisfied {
                id: id.to_string(),
                reason: format!("必需依赖形成循环（{reason}），请调整 [[dependencies]] 声明"),
            });
        }
        Ok(())
    }

    /// Provider lifecycle/version transition guard. None means the provider
    /// becomes unavailable (stop/disable/active uninstall); Some means the
    /// proposed active version must satisfy every running required dependent.
    /// Optional dependencies are deliberately ignored and no cascade is done.
    fn ensure_provider_transition(
        &self,
        id: &ExtensionId,
        next_active_version: Option<&ExtensionVersion>,
    ) -> ExtensionResult<()> {
        let snapshots = self.list()?;
        let mut unavailable_dependents = Vec::new();
        for snapshot in &snapshots {
            if snapshot.id() == id
                || !snapshot.state().is_running()
                || !self.has_live_instance_or_runner(snapshot.id())
            {
                continue;
            }
            for dependency in snapshot.manifest().dependencies() {
                if !dependency.required() || dependency.id() != id {
                    continue;
                }
                let Some(version) = next_active_version else {
                    unavailable_dependents.push(snapshot.id().to_string());
                    continue;
                };
                if !dependency.version_req().matches(version.semver()) {
                    return Err(ExtensionError::IncompatibleDependencyVersion {
                        id: id.to_string(),
                        dependent: snapshot.id().to_string(),
                        requirement: dependency.version_req().to_string(),
                        version: version.to_string(),
                    });
                }
            }
        }
        if !unavailable_dependents.is_empty() {
            return Err(ExtensionError::DependencyOfRunningExtension {
                id: id.to_string(),
                dependent: unavailable_dependents.join(", "),
            });
        }
        Ok(())
    }

    /// An active-version change is itself a dependency transition. Check the
    /// incoming target version against the requirement declared by every
    /// currently running dependent, even when the target extension is not
    /// currently running (for example, after a stale/crash recovery window).
    fn ensure_version_satisfies_running_dependents(
        &self,
        id: &ExtensionId,
        version: &ExtensionVersion,
    ) -> ExtensionResult<()> {
        self.ensure_provider_transition(id, Some(version))
    }

    /// 能力发现（简化计划 Phase 4）：目标插件对外公开的动作集合 = declarative
    /// UI 按钮集合 ∪ 原生公开动作清单（gamer.yaml actions）。调用方（其他
    /// 插件/前端）据此决定功能入口是否可用——动作存在 ≠ 可调用，调用时仍须
    /// 目标 Running + 权限/上下文门禁（`call_extension` 统一执行）。
    pub(crate) fn capability_actions(
        &self,
        manifest: &ExtensionManifest,
    ) -> Vec<serde_json::Value> {
        let mut actions: Vec<serde_json::Value> = native_public_actions(manifest.id());
        for action in declarative_actions(manifest) {
            // native 目录与 declarative 集合概念互斥（native 动作不经按钮集合）；
            // 去重防御，保持清单语义单源。
            if actions
                .iter()
                .any(|entry| entry["action"] == action.as_str())
            {
                continue;
            }
            actions.push(serde_json::json!({
                "action": action,
                "surface": "declarative",
                "summary": serde_json::Value::Null,
            }));
        }
        actions
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
        let manifest = self.inspect_compatible(archive)?;
        let call_gate = self.call_gate(manifest.id());
        let _call_gate = call_gate.write().await;
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
        self.ensure_version_satisfies_running_dependents(manifest.id(), manifest.version())?;
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
        let call_gate = self.call_gate(id);
        let _call_gate = call_gate.write().await;
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
        self.ensure_version_satisfies_running_dependents(id, version)?;
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
        let call_gate = self.call_gate(id);
        let _call_gate = call_gate.write().await;
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
        let call_gate = self.call_gate(id);
        {
            let _call_gate = call_gate.write().await;
            let _guard = self.operation_lock.lock().await;
            let mut states = self.store.read_state()?;
            let versions = self.versions_for(id)?;
            let mut record = state_for_versions(id, &versions, states.get(id).cloned())?;
            match record.state {
                ExtensionState::Running => {
                    if self.is_process_running(id) {
                        // 已在本进程运行：启用意图已满足，直接返回现状（幂等）。
                        return self.snapshot_for(id);
                    }
                    // Durable Running can be left by a previous process. Repair
                    // it before the regular start path instead of treating a
                    // stale record as a live instance.
                    record.state = ExtensionState::Enabled;
                    record.last_error = None;
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
    /// rejecting disable-while-running. 简化计划 Phase 3：目标被运行中插件的
    /// 必需依赖引用时拒绝（提示先停用依赖方，不做自动级联停用）。
    pub(crate) async fn disable(&self, id: &ExtensionId) -> ExtensionResult<ExtensionSnapshot> {
        let call_gate = self.call_gate(id);
        let _call_gate = call_gate.write().await;
        let _guard = self.operation_lock.lock().await;
        self.ensure_provider_transition(id, None)?;
        let mut states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let mut record = state_for_versions(id, &versions, states.get(id).cloned())?;
        match record.state {
            ExtensionState::Running => {
                self.stop_running_instance(id).await?;
                record.state = ExtensionState::Disabled;
            }
            ExtensionState::Disabled => {}
            ExtensionState::Installed | ExtensionState::Enabled | ExtensionState::Failed => {
                record.state = ExtensionState::Disabled;
            }
        }
        // Also clean an owner that is already non-running. This is an
        // idempotent repair for a partially completed previous transition.
        let unregister_error = self.unregister_extension_runners(id).await.err();
        record.last_error = unregister_error.as_ref().map(ToString::to_string);
        states.insert(id.clone(), record);
        self.store.write_state(&states)?;
        self.refresh_ui_registry()?;
        if let Some(error) = unregister_error {
            return Err(error);
        }
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
        let call_gate = self.call_gate(id);
        let _call_gate = call_gate.write().await;
        self.start_with_context_locked(id, app_context, keymap_profile)
            .await
    }

    /// Start implementation for callers that already hold the per-extension
    /// write gate (startup reconciliation). Keeping the gate across the stale
    /// record check and the complete start transaction prevents a manual
    /// enable and reconcile pass from both creating an instance.
    async fn start_with_context_locked(
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
        // 依赖门禁（简化计划 Phase 3）：必需依赖缺失/版本不兼容/未启用或形成
        // 循环 → 结构化拒绝启动。依赖缺失 ≠ 插件损坏：状态保留 Enabled 并记录
        // last_error（区别于运行时错误的 Failed），可从插件中心处置后重试。
        // 可选依赖不检查（缺失不阻止启动）。
        let dependency_error = self
            .check_dependency_cycles(id, active.manifest())
            .err()
            .or_else(|| {
                self.check_required_dependencies(id, active.manifest())
                    .err()
            });
        if let Some(error) = dependency_error {
            return self.mark_start_failed(id, states, record, error).await;
        }
        let host = match HostApi::for_manifest(
            self.capabilities.clone(),
            self.host_api.clone(),
            active.manifest(),
        ) {
            Ok(host) => host,
            Err(error) => return self.mark_start_failed(id, states, record, error).await,
        };
        let process_app_context = app_context.clone();
        let wasm = if self.instance_free(id) {
            None
        } else {
            match active.read_wasm() {
                Ok(wasm) => Some(wasm),
                Err(error) => return self.mark_start_failed(id, states, record, error).await,
            }
        };
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
                    wasm: wasm.expect("non-instance-free keymap must have wasm"),
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
                    wasm: wasm.expect("non-instance-free extension must have wasm"),
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

        if let Some(handle) = instance_insert {
            match handle {
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
        }

        // Runner registration is part of the start transaction. A failure is
        // surfaced and rolled back before the durable state can claim
        // Running; this prevents a visible extension with no executable
        // runner.
        if let Err(error) = self.register_extension_runners(id).await {
            let _ = self.unregister_extension_runners(id).await;
            self.rollback_started_instance(id, instance_insert).await;
            return self.mark_start_failed(id, states, record, error).await;
        }

        // UI registration itself is infallible, but do it before publishing
        // Running so every successful state transition has a complete UI
        // contribution. The full derived registry is refreshed immediately
        // after the durable write below and is rolled back on failure.
        self.ui.register(active.manifest());
        // Publish process ownership before the durable write. This closes the
        // tiny window where a concurrent startup reconcile could observe the
        // newly persisted Running record but not yet know this process owns it.
        self.mark_process_running(id, process_app_context);
        let mut running_record = record.clone();
        running_record.state = ExtensionState::Running;
        running_record.last_error = None;
        if let Err(error) = self.store.write_state(&{
            let mut next = states.clone();
            next.insert(id.clone(), running_record.clone());
            next
        }) {
            let _ = self.unregister_extension_runners(id).await;
            self.rollback_started_instance(id, instance_insert).await;
            let _ = self.refresh_ui_registry();
            return Err(error);
        }

        if let Err(error) = self.refresh_ui_registry() {
            let _ = self.unregister_extension_runners(id).await;
            self.rollback_started_instance(id, instance_insert).await;
            let mut next = states.clone();
            let mut failed_record = record;
            failed_record.state = ExtensionState::Enabled;
            failed_record.last_error = Some(error.to_string());
            let _ = self.store.write_state({
                next.insert(id.clone(), failed_record);
                &next
            });
            let _ = self.refresh_ui_registry();
            return Err(error);
        }
        self.snapshot_for(id)
    }

    pub(crate) async fn stop(&self, id: &ExtensionId) -> ExtensionResult<ExtensionSnapshot> {
        let call_gate = self.call_gate(id);
        let _call_gate = call_gate.write().await;
        let _guard = self.operation_lock.lock().await;
        self.ensure_provider_transition(id, None)?;
        let mut states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let mut record = state_for_versions(id, &versions, states.get(id).cloned())?;
        if !record.state.is_running() {
            return Err(invalid_transition(id, "stop", record.state));
        }
        self.stop_running_instance(id).await?;
        record.state = ExtensionState::Enabled;
        let unregister_error = self.unregister_extension_runners(id).await.err();
        record.last_error = unregister_error.as_ref().map(ToString::to_string);
        states.insert(id.clone(), record);
        self.store.write_state(&states)?;
        self.refresh_ui_registry()?;
        if let Some(error) = unregister_error {
            return Err(error);
        }
        self.snapshot_for(id)
    }

    /// Deterministic recovery order for the stale Running records left by a
    /// previous process. Only persisted Running extensions participate in the
    /// ordering; a missing dependency is intentionally left for the normal
    /// start gate to diagnose. Cycles have no ready node and are emitted in
    /// stable ID order, after which each start reports the cycle/dependency
    /// error without spinning or recursively starting another extension.
    ///
    /// Snapshot failure is a hard, structured error. Falling back to an ID
    /// order would make a provider/consumer restore nondeterministic and could
    /// incorrectly publish a dependent as Running.
    fn startup_restore_order(&self, running: &[ExtensionId]) -> ExtensionResult<Vec<ExtensionId>> {
        let candidates: BTreeSet<ExtensionId> = running.iter().cloned().collect();
        let mut required_dependencies: HashMap<ExtensionId, Vec<ExtensionId>> = HashMap::new();
        let snapshots = self.list().map_err(|error| {
            ExtensionError::InvalidState(format!("启动恢复排序读取扩展快照失败: {error}"))
        })?;
        for snapshot in snapshots {
            if candidates.contains(snapshot.id()) {
                required_dependencies.insert(
                    snapshot.id().clone(),
                    snapshot
                        .manifest()
                        .dependencies()
                        .iter()
                        .filter(|dependency| dependency.required())
                        .map(|dependency| dependency.id().clone())
                        .collect(),
                );
            }
        }

        let mut remaining = candidates;
        let mut order = Vec::with_capacity(remaining.len());
        while !remaining.is_empty() {
            let next = remaining
                .iter()
                .find(|id| {
                    !required_dependencies.get(*id).is_some_and(|dependencies| {
                        dependencies
                            .iter()
                            .any(|dependency| remaining.contains(dependency))
                    })
                })
                .cloned();
            let Some(next) = next else {
                // Required-dependency cycle: preserve deterministic order and
                // let each regular start gate produce the structured error.
                order.extend(remaining);
                break;
            };
            remaining.remove(&next);
            order.push(next);
        }
        Ok(order)
    }

    /// P11.7 启动对账：进程重启后，持久化状态遗留 `Running` 的扩展既没有活
    /// 实例也不在 Runner 注册表——其名下任务会一直 `DependencyMissing`，直到
    /// 用户手动 start。对每条 Running 记录执行与 `start` 等价的恢复（实例
    /// 启动 / runner 注册 / UI 贡献刷新）；恢复失败降级为 `Enabled` 并把失败
    /// 原因记入 last_error，绝不阻塞服务启动。组合根在 Scheduler 启动前调用，
    /// 使恢复出的 runner 立即可派发任务。幂等：无 Running 记录时为 no-op。
    pub(crate) async fn reconcile_startup(&self) {
        if self
            .startup_reconciled
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let running: Vec<ExtensionId> = match self.store.read_state() {
            Ok(states) => states
                .into_iter()
                .filter(|(_, record)| record.state == ExtensionState::Running)
                .map(|(id, _)| id)
                .collect(),
            Err(error) => {
                self.startup_reconciled
                    .store(false, std::sync::atomic::Ordering::Release);
                tracing::warn!(%error, "extension startup reconcile skipped: state unreadable");
                return;
            }
        };
        let running = running
            .into_iter()
            .filter(|id| !self.is_process_running(id))
            .collect::<Vec<_>>();
        let restore_order = match self.startup_restore_order(&running) {
            Ok(order) => order,
            Err(error) => {
                tracing::error!(
                    error = %error,
                    candidates = ?running,
                    "extension startup reconcile aborted before restore"
                );
                // Do not try to recover any candidate after a snapshot error.
                // Keep the one-shot guard clear so a later explicit retry can
                // diagnose the repaired store instead of silently doing no-op.
                self.startup_reconciled
                    .store(false, std::sync::atomic::Ordering::Release);
                return;
            }
        };
        for id in restore_order {
            let call_gate = self.call_gate(&id);
            let _call_gate = call_gate.write().await;
            let is_stale = match self.store.read_state() {
                Ok(states) => {
                    states
                        .get(&id)
                        .is_some_and(|record| record.state == ExtensionState::Running)
                        && !self.is_process_running(&id)
                }
                Err(error) => {
                    tracing::warn!(
                        extension = %id,
                        %error,
                        "extension startup reconcile: cannot recheck stale state"
                    );
                    false
                }
            };
            if !is_stale {
                continue;
            }
            // 重启即实例全灭：先把记录降为 Enabled（Running 只描述活实例），
            // 再走与手动 start 完全相同的恢复路径。
            let clear_result = {
                let _guard = self.operation_lock.lock().await;
                self.force_state_locked(&id, ExtensionState::Enabled, None)
            };
            if let Err(error) = clear_result {
                tracing::warn!(
                    extension = %id,
                    %error,
                    "extension startup reconcile: cannot clear stale Running record"
                );
                continue;
            }
            match self.start_with_context_locked(&id, None, None).await {
                Ok(_) => {
                    tracing::info!(extension = %id, "extension restored to Running at startup");
                }
                Err(error) => {
                    // start 失败可能已把状态写成 Failed——统一降级 Enabled 并
                    // 记录原因，服务启动不受影响。
                    let _ = {
                        let _guard = self.operation_lock.lock().await;
                        self.force_state_locked(
                            &id,
                            ExtensionState::Enabled,
                            Some(error.to_string()),
                        )
                    };
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
        let call_gate = self.call_gate(id);
        let _call_gate = call_gate.write().await;
        let _guard = self.operation_lock.lock().await;
        let mut states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let record = state_for_versions(id, &versions, states.get(id).cloned())?;
        let is_active = record.active_version.as_ref() == Some(version);
        if !versions
            .iter()
            .any(|candidate| candidate.manifest().version() == version)
        {
            return Err(ExtensionError::VersionNotInstalled {
                id: id.to_string(),
                version: version.to_string(),
            });
        }
        if is_active && record.state.is_running() {
            // Report a dependency violation before the local lifecycle error
            // so the caller gets the concrete running dependents it must stop.
            // The replacement version is considered below; this avoids
            // rejecting a compatible active-version replacement merely because
            // a required dependent is currently Running.
            let next_active = versions
                .iter()
                .filter(|candidate| candidate.manifest().version() != version)
                .map(|candidate| candidate.manifest().version())
                .max();
            self.ensure_provider_transition(id, next_active)?;
            return Err(invalid_transition(id, "uninstall", record.state));
        }
        // Removing an inactive immutable version cannot affect callers using
        // the active version. Removing the active version is a provider
        // transition: either no provider remains or the replacement active
        // version must satisfy every running required dependent.
        let next_active = if is_active {
            versions
                .iter()
                .filter(|candidate| candidate.manifest().version() != version)
                .map(|candidate| candidate.manifest().version())
                .max()
        } else {
            None
        };
        if is_active {
            self.ensure_provider_transition(id, next_active)?;
        }
        // ADR-13 卸载清场：Running 已被上方守卫拒绝，正常路径 owner 名下本就
        // 无 runner（stop/disable 已注销）；此处兜底摘除，幂等 no-op。
        if is_active {
            self.unregister_extension_runners(id).await?;
            self.clear_process_running(id);
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
        let mut failed_record = record;
        failed_record.state = if matches!(&error, ExtensionError::Runtime(_)) {
            ExtensionState::Failed
        } else {
            ExtensionState::Enabled
        };
        failed_record.last_error = Some(error.to_string());
        let mut next = states;
        next.insert(id.clone(), failed_record);
        self.store.write_state(&next)?;
        if let Err(refresh_error) = self.refresh_ui_registry() {
            tracing::warn!(
                extension = %id,
                %refresh_error,
                "extension start failure recorded but UI registry refresh failed"
            );
        }
        Err(error)
    }

    /// Complete the management-side auto-start fallback without allowing an
    /// older failure to overwrite a later lifecycle transition. `start` has
    /// already persisted the exact diagnostic while holding the gate; this
    /// method only changes `Failed` to `Enabled` when that diagnostic is
    /// still present and no live instance/runner has appeared since then.
    pub(crate) async fn degrade_start_failure(
        &self,
        id: &ExtensionId,
        error: &ExtensionError,
    ) -> ExtensionResult<ExtensionSnapshot> {
        let error_text = error.to_string();
        let gate = self.call_gate(id);
        let _call_gate = gate.write().await;
        let _guard = self.operation_lock.lock().await;
        let mut states = self.store.read_state()?;
        let versions = self.versions_for(id)?;
        let mut record = state_for_versions(id, &versions, states.get(id).cloned())?;
        if record.last_error.as_deref() != Some(error_text.as_str())
            || self.has_live_instance_or_runner(id)
            || record.state == ExtensionState::Running
        {
            // The failure is stale (or was never persisted by this start
            // attempt). Preserve the newer state and only return its snapshot.
            return snapshot_from_versions(versions, record);
        }
        record.state = ExtensionState::Enabled;
        record.last_error = Some(error_text);
        states.insert(id.clone(), record);
        self.store.write_state(&states)?;
        self.refresh_ui_registry()?;
        self.snapshot_for(id)
    }

    async fn rollback_started_instance(&self, id: &ExtensionId, handle: Option<RunningHandle>) {
        self.clear_process_running(id);
        self.running
            .lock()
            .expect("extension running map poisoned")
            .remove(id);
        self.keymap_running
            .lock()
            .expect("keymap running map poisoned")
            .remove(id);
        if let Some(handle) = handle {
            if let Err(error) = self.stop_running_handle(handle).await {
                tracing::warn!(extension = %id, %error, "extension start rollback stop failed");
            }
        }
    }

    /// 受控直写生命周期状态（仅启动对账与测试夹具使用）。即使调用方来自
    /// 一个失败的异步路径，也必须取得同一 per-extension write gate 和全局
    /// operation lock；它不能越过正在进行的 call/disable/start。
    pub(crate) async fn force_state(
        &self,
        id: &ExtensionId,
        state: ExtensionState,
        last_error: Option<String>,
    ) -> ExtensionResult<()> {
        let gate = self.call_gate(id);
        let _call_gate = gate.write().await;
        let _guard = self.operation_lock.lock().await;
        self.force_state_locked(id, state, last_error)
    }

    fn force_state_locked(
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
        if state != ExtensionState::Running {
            self.clear_process_running(id);
        }
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
            self.clear_process_running(id);
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
        self.clear_process_running(id);
        Ok(())
    }

    /// ADR-13 start hook: registration is part of the start transaction. The
    /// durable Running state is published only after this succeeds, so a
    /// failed registrar cannot expose an extension without its Runner/UI
    /// surface.
    async fn register_extension_runners(&self, id: &ExtensionId) -> ExtensionResult<()> {
        if let Some(registrar) = &self.runner_registrar {
            registrar
                .extension_started(id.as_str())
                .await
                .map_err(|error| {
                    ExtensionError::Runtime(format!("扩展 Runner/UI 注册失败: {error:#}"))
                })?;
        }
        Ok(())
    }

    /// ADR-13 stop/disable/uninstall hook. The instance is already gone before
    /// this is called; a failure is persisted as a diagnostic and returned to
    /// the caller, while the non-running lifecycle state remains durable.
    async fn unregister_extension_runners(&self, id: &ExtensionId) -> ExtensionResult<()> {
        if let Some(registrar) = &self.runner_registrar {
            registrar
                .extension_stopped(id.as_str())
                .await
                .map_err(|error| {
                    ExtensionError::Runtime(format!("扩展 Runner 注销失败: {error:#}"))
                })?;
        }
        Ok(())
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

    // ---------- 简化计划 Phase 3：最小插件依赖 ----------

    /// 假 registrar：任意扩展都按「无实例执行模型」处理——`start` 不读 guest
    /// 字节，依赖门禁/守卫可在无 WASM 运行时的环境下验证。
    struct InstanceFreeRegistrar;

    #[async_trait]
    impl TimerRunnerRegistrar for InstanceFreeRegistrar {
        async fn extension_started(&self, _extension_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn extension_stopped(&self, _extension_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn executes_without_instance(&self, _extension_id: &str) -> bool {
            true
        }
    }

    fn dependency_service(dir: &std::path::Path) -> ExtensionService {
        ExtensionService::for_data_root(dir, crate::capabilities::CapabilityRegistry::default())
            .with_runner_registrar(Arc::new(InstanceFreeRegistrar))
    }

    /// 带 `[[dependencies]]` 的 wasm 安装包（deps = 原始 TOML 片段）。
    fn wasm_archive_with_deps(id: &str, version: &str, deps: &str) -> Vec<u8> {
        let manifest = format!(
            "manifest_version = 2\nid = \"{id}\"\nversion = \"{version}\"\nname = \"D\"\nentry = \"plugin.wasm\"\n{deps}"
        )
        .into_bytes();
        zip_of(&[
            ("manifest.toml", manifest),
            ("plugin.wasm", VALID_WASM.to_vec()),
        ])
    }

    fn builtin_video_archive() -> Vec<u8> {
        zip_of(&[(
            "manifest.toml",
            builtin_manifest_bytes(
                super::super::video::VIDEO_EXTENSION_ID,
                "1.0.0",
                "gamer.video",
            ),
        )])
    }

    /// 必需依赖缺失 → 启动被拒（安装自动启动降级 Enabled + last_error）；
    /// 依赖安装并启用 → 启动通过；停用被运行中依赖方必需依赖的插件 → 拒绝。
    #[tokio::test]
    async fn required_dependency_gates_start_and_disable_guard() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let consumer = ExtensionId::parse("com.example.consumer").unwrap();
        let provider = ExtensionId::parse(super::super::video::VIDEO_EXTENSION_ID).unwrap();

        // 安装消费方（必需依赖 gamer.video 未安装）：enable 落 Enabled，start
        // 被依赖门禁拒绝 → Failed + last_error（保留启用意图，可重试）。
        service
            .install(&wasm_archive_with_deps(
                "com.example.consumer",
                "1.0.0",
                "# 必需依赖\n[[dependencies]]\nid = \"gamer.video\"\nversion = \"^1.0.0\"\n",
            ))
            .await
            .unwrap();
        service.enable(&consumer).await.unwrap();
        let error = service.start(&consumer).await.unwrap_err();
        assert!(error.to_string().contains("必需依赖"), "{error}");
        // 依赖缺失 ≠ 插件损坏：状态保留 Enabled（启用意图），错误经响应与
        // 快照可见；区别于运行时错误的 Failed 语义。
        assert_eq!(snapshot_state(&service, &consumer), ExtensionState::Enabled);
        let snapshot = service.snapshot_for(&consumer).unwrap();
        let error = snapshot
            .last_error()
            .expect("必须记录依赖缺失原因")
            .to_string();
        assert!(error.contains("必需依赖"), "{error}");
        assert!(error.contains("gamer.video"), "{error}");

        // 依赖状态报告：未安装/未满足/带降级提示。
        let manifest = service.snapshot_for(&consumer).unwrap().manifest().clone();
        let report = service.dependency_report(&manifest);
        assert_eq!(report.len(), 1);
        assert_eq!(report[0].id, "gamer.video");
        assert!(report[0].required, "缺省 required 从紧 = true");
        assert!(!report[0].installed);
        assert!(!report[0].satisfied);
        assert_eq!(report[0].note.as_deref(), Some("依赖未安装"));

        // 安装 provider（builtin）：安装后落 Installed，显式 enable → start 即
        // Running（无实例执行模型）→ 消费方启动通过。
        service.install(&builtin_video_archive()).await.unwrap();
        service.enable(&provider).await.unwrap();
        service.start(&provider).await.unwrap();
        assert_eq!(snapshot_state(&service, &provider), ExtensionState::Running);
        let started = service.start(&consumer).await.unwrap();
        assert_eq!(started.state(), ExtensionState::Running, "依赖满足后可启动");

        // 停用被运行中消费方必需依赖的 provider → 拒绝（提示先停用依赖方）。
        let error = service.stop(&provider).await.unwrap_err();
        assert!(
            error.to_string().contains("com.example.consumer"),
            "stop 也必须走相同依赖守卫: {error}"
        );
        let error = service.disable(&provider).await.unwrap_err();
        assert!(
            error.to_string().contains("com.example.consumer"),
            "{error}"
        );
        // 卸载同样被拒。
        let error = service
            .uninstall(&provider, &ExtensionVersion::parse("1.0.0").unwrap())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("com.example.consumer"),
            "{error}"
        );

        // 停用消费方后 → provider 可停用。
        service.disable(&consumer).await.unwrap();
        service.disable(&provider).await.unwrap();
        assert_eq!(
            snapshot_state(&service, &provider),
            ExtensionState::Disabled
        );
    }

    /// The dependency requirement belongs to the running dependent, while
    /// the version being checked belongs to the target provider. This catches
    /// the old implementation that accidentally compared A's version with
    /// A→B's requirement instead of comparing B's active version.
    #[tokio::test]
    async fn required_dependency_guard_uses_target_active_version() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let provider = ExtensionId::parse("com.example.provider").unwrap();
        let consumer = ExtensionId::parse("com.example.consumer.versioned").unwrap();

        service
            .install(&wasm_archive_with_deps(provider.as_str(), "2.0.0", ""))
            .await
            .unwrap();
        service
            .install(&wasm_archive_with_deps(
                consumer.as_str(),
                "1.0.0",
                "[[dependencies]]\nid = \"com.example.provider\"\nversion = \">=2.0.0\"\n",
            ))
            .await
            .unwrap();
        service.enable(&provider).await.unwrap();
        service.start(&provider).await.unwrap();
        service.enable(&consumer).await.unwrap();
        service.start(&consumer).await.unwrap();

        let error = service.disable(&provider).await.unwrap_err();
        assert!(matches!(
            error,
            ExtensionError::DependencyOfRunningExtension { ref dependent, .. }
                if dependent == consumer.as_str()
        ));
    }

    /// Removing an inactive immutable version is safe even while the active
    /// version is running; it must not unregister the active extension's
    /// runners or trip the dependency guard.
    #[tokio::test]
    async fn uninstall_inactive_version_does_not_disrupt_running_active_version() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let id = ExtensionId::parse("com.example.multi-version").unwrap();
        service
            .install(&wasm_archive_with_deps(id.as_str(), "1.0.0", ""))
            .await
            .unwrap();
        service
            .update(&wasm_archive_with_deps(id.as_str(), "2.0.0", ""))
            .await
            .unwrap();
        service.enable(&id).await.unwrap();
        service.start(&id).await.unwrap();

        assert!(service
            .uninstall(&id, &ExtensionVersion::parse("1.0.0").unwrap())
            .await
            .unwrap());
        let snapshot = service.snapshot_for(&id).unwrap();
        assert_eq!(snapshot.state(), ExtensionState::Running);
        assert_eq!(snapshot.active_version().as_str(), "2.0.0");
        assert_eq!(snapshot.installed_versions().len(), 1);
    }

    /// Uninstalling the active version evaluates the version that will become
    /// active. A compatible replacement is allowed for a stale Running
    /// dependent.
    #[tokio::test]
    async fn uninstall_active_version_checks_compatible_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let provider = ExtensionId::parse("com.example.provider.uninstall").unwrap();
        let consumer = ExtensionId::parse("com.example.consumer.uninstall").unwrap();
        service
            .install(&wasm_archive_with_deps(provider.as_str(), "1.0.0", ""))
            .await
            .unwrap();
        service
            .install(&wasm_archive_with_deps(provider.as_str(), "2.0.0", ""))
            .await
            .unwrap();
        service
            .install(&wasm_archive_with_deps(
                consumer.as_str(),
                "1.0.0",
                "[[dependencies]]\nid = \"com.example.provider.uninstall\"\nversion = \"^1.0\"\n",
            ))
            .await
            .unwrap();
        service.enable(&provider).await.unwrap();
        service.start(&provider).await.unwrap();
        service.enable(&consumer).await.unwrap();
        service.start(&consumer).await.unwrap();

        // The provider is stale/non-running, so the test isolates the version
        // transition guard from the ordinary stop-before-uninstall guard.
        service
            .force_state(&provider, ExtensionState::Enabled, None)
            .await
            .unwrap();
        let mut states = service.store().read_state().unwrap();
        states.get_mut(&provider).unwrap().active_version =
            Some(ExtensionVersion::parse("2.0.0").unwrap());
        service.store().write_state(&states).unwrap();
        let error = service
            .uninstall(&provider, &ExtensionVersion::parse("2.0.0").unwrap())
            .await
            .unwrap();
        assert!(error);
        assert_eq!(
            service
                .snapshot_for(&provider)
                .unwrap()
                .active_version()
                .as_str(),
            "1.0.0"
        );
    }

    /// An active-version uninstall is refused when the replacement would break
    /// a currently Running required dependent, without deleting that version.
    #[tokio::test]
    async fn uninstall_active_version_rejects_incompatible_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let provider = ExtensionId::parse("com.example.provider.uninstall.bad").unwrap();
        let consumer = ExtensionId::parse("com.example.consumer.uninstall.bad").unwrap();
        service
            .install(&wasm_archive_with_deps(provider.as_str(), "1.0.0", ""))
            .await
            .unwrap();
        service
            .update(&wasm_archive_with_deps(provider.as_str(), "2.0.0", ""))
            .await
            .unwrap();
        service
            .install(&wasm_archive_with_deps(
                consumer.as_str(),
                "1.0.0",
                "[[dependencies]]\nid = \"com.example.provider.uninstall.bad\"\nversion = \"^2.0\"\n",
            ))
            .await
            .unwrap();
        service.enable(&provider).await.unwrap();
        service.start(&provider).await.unwrap();
        service.enable(&consumer).await.unwrap();
        service.start(&consumer).await.unwrap();
        service
            .force_state(&provider, ExtensionState::Enabled, None)
            .await
            .unwrap();

        let error = service
            .uninstall(&provider, &ExtensionVersion::parse("2.0.0").unwrap())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ExtensionError::IncompatibleDependencyVersion { ref dependent, .. }
                if dependent == consumer.as_str()
        ));
        assert!(service
            .snapshot_for(&provider)
            .unwrap()
            .installed_versions()
            .iter()
            .any(|version| version.as_str() == "2.0.0"));
    }

    /// Updating or activating a target version is also a dependency change.
    /// A stale-but-running dependent must not be silently broken by selecting
    /// an incompatible provider version.
    #[tokio::test]
    async fn incompatible_provider_version_is_rejected_for_running_dependent() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let provider = ExtensionId::parse("com.example.provider.switch").unwrap();
        let consumer = ExtensionId::parse("com.example.consumer.switch").unwrap();

        service
            .install(&wasm_archive_with_deps(provider.as_str(), "2.0.0", ""))
            .await
            .unwrap();
        service
            .install(&wasm_archive_with_deps(
                consumer.as_str(),
                "1.0.0",
                "[[dependencies]]\nid = \"com.example.provider.switch\"\nversion = \"^2.0\"\n",
            ))
            .await
            .unwrap();
        service.enable(&provider).await.unwrap();
        service.start(&provider).await.unwrap();
        service.enable(&consumer).await.unwrap();
        service.start(&consumer).await.unwrap();

        // Reproduce a stale lifecycle window: the provider is no longer
        // marked Running, but the dependent remains Running.
        service
            .force_state(&provider, ExtensionState::Enabled, None)
            .await
            .unwrap();
        let error = service
            .update(&wasm_archive_with_deps(provider.as_str(), "3.0.0", ""))
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ExtensionError::IncompatibleDependencyVersion { ref dependent, .. }
                if dependent == consumer.as_str()
        ));
        assert_eq!(
            service
                .snapshot_for(&provider)
                .unwrap()
                .active_version()
                .as_str(),
            "2.0.0",
            "被拒版本切换不得改变 active_version"
        );
    }

    #[derive(Clone, Default)]
    struct RestoreOrderRegistrar {
        started: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl TimerRunnerRegistrar for RestoreOrderRegistrar {
        async fn extension_started(&self, extension_id: &str) -> anyhow::Result<()> {
            self.started
                .lock()
                .expect("restore order recorder poisoned")
                .push(extension_id.to_string());
            Ok(())
        }

        async fn extension_stopped(&self, _extension_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn executes_without_instance(&self, _extension_id: &str) -> bool {
            true
        }
    }

    #[derive(Clone)]
    struct OptionalFailureRegistrar {
        failed_id: String,
    }

    #[async_trait]
    impl TimerRunnerRegistrar for OptionalFailureRegistrar {
        async fn extension_started(&self, extension_id: &str) -> anyhow::Result<()> {
            if extension_id == self.failed_id {
                Err(anyhow::anyhow!("optional provider restore failed"))
            } else {
                Ok(())
            }
        }

        async fn extension_stopped(&self, _extension_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn executes_without_instance(&self, _extension_id: &str) -> bool {
            true
        }
    }

    /// Startup recovery must not depend on the durable map's lexical order:
    /// the consumer ID sorts before the provider ID in this fixture, while the
    /// provider still has to be restored first.
    #[tokio::test]
    async fn reconcile_startup_restores_required_provider_before_consumer() {
        let temp = tempfile::tempdir().unwrap();
        let provider = ExtensionId::parse("com.example.restore.provider").unwrap();
        let consumer = ExtensionId::parse("com.example.restore.consumer").unwrap();

        let service = dependency_service(temp.path());
        service
            .install(&wasm_archive_with_deps(provider.as_str(), "1.0.0", ""))
            .await
            .unwrap();
        service
            .install(&wasm_archive_with_deps(
                consumer.as_str(),
                "1.0.0",
                "[[dependencies]]\nid = \"com.example.restore.provider\"\n",
            ))
            .await
            .unwrap();
        service.enable(&provider).await.unwrap();
        service.start(&provider).await.unwrap();
        service.enable(&consumer).await.unwrap();
        service.start(&consumer).await.unwrap();
        // Simulate a process crash after both extensions were Running.
        mark_running(&service, &consumer);
        mark_running(&service, &provider);
        drop(service);

        let recorder = RestoreOrderRegistrar::default();
        let restarted = ExtensionService::for_data_root(
            temp.path(),
            crate::capabilities::CapabilityRegistry::default(),
        )
        .with_runner_registrar(Arc::new(recorder.clone()));
        restarted.reconcile_startup().await;

        let started = recorder
            .started
            .lock()
            .expect("restore order recorder poisoned")
            .clone();
        assert_eq!(
            started,
            vec![provider.as_str().to_string(), consumer.as_str().to_string()]
        );
        assert_eq!(
            snapshot_state(&restarted, &provider),
            ExtensionState::Running
        );
        assert_eq!(
            snapshot_state(&restarted, &consumer),
            ExtensionState::Running
        );

        // A second reconciliation in the same process is a no-op: it must
        // not restart the live instances or register duplicate runners.
        restarted.reconcile_startup().await;
        let started_again = recorder
            .started
            .lock()
            .expect("restore order recorder poisoned")
            .clone();
        assert_eq!(started_again.len(), 2);
    }

    /// A Running record owned by this process is not stale merely because the
    /// durable state was read again. Reconcile must not start/register it a
    /// second time.
    #[tokio::test]
    async fn reconcile_startup_does_not_restart_current_process_running_extension() {
        let temp = tempfile::tempdir().unwrap();
        let recorder = RestoreOrderRegistrar::default();
        let service =
            dependency_service(temp.path()).with_runner_registrar(Arc::new(recorder.clone()));
        let id = ExtensionId::parse("com.example.current-process").unwrap();
        service
            .install(&wasm_archive_with_deps(id.as_str(), "1.0.0", ""))
            .await
            .unwrap();
        service.enable(&id).await.unwrap();
        service.start(&id).await.unwrap();

        service.reconcile_startup().await;
        service.reconcile_startup().await;

        assert_eq!(
            recorder.started.lock().unwrap().as_slice(),
            [id.as_str()],
            "当前进程实例只能完成一次 start/runner 注册"
        );
        assert_eq!(snapshot_state(&service, &id), ExtensionState::Running);
    }

    /// Optional dependency recovery is best effort: a failed optional
    /// provider restore is diagnosed on that provider, while the consumer is
    /// still restored and remains Running.
    #[tokio::test]
    async fn optional_dependency_restore_failure_does_not_block_consumer() {
        let temp = tempfile::tempdir().unwrap();
        let provider = ExtensionId::parse("com.example.optional.provider").unwrap();
        let consumer = ExtensionId::parse("com.example.optional.consumer").unwrap();
        let service = dependency_service(temp.path());
        service
            .install(&wasm_archive_with_deps(provider.as_str(), "1.0.0", ""))
            .await
            .unwrap();
        service
            .install(&wasm_archive_with_deps(
                consumer.as_str(),
                "1.0.0",
                "[[dependencies]]\nid = \"com.example.optional.provider\"\nrequired = false\n",
            ))
            .await
            .unwrap();
        service.enable(&provider).await.unwrap();
        service.start(&provider).await.unwrap();
        service.enable(&consumer).await.unwrap();
        service.start(&consumer).await.unwrap();
        mark_running(&service, &provider);
        mark_running(&service, &consumer);
        drop(service);

        let restarted = ExtensionService::for_data_root(
            temp.path(),
            crate::capabilities::CapabilityRegistry::default(),
        )
        .with_runner_registrar(Arc::new(OptionalFailureRegistrar {
            failed_id: provider.to_string(),
        }));
        restarted.reconcile_startup().await;

        let provider_snapshot = restarted.snapshot_for(&provider).unwrap();
        assert_eq!(provider_snapshot.state(), ExtensionState::Enabled);
        assert!(provider_snapshot
            .last_error()
            .is_some_and(|error| error.contains("optional provider restore failed")));
        assert_eq!(
            snapshot_state(&restarted, &consumer),
            ExtensionState::Running
        );
    }

    /// A native action is side-effect free when the target is not Running.
    /// The test uses a real PackageStore so a regression that dispatches first
    /// would leave a visible draft file behind.
    #[tokio::test]
    async fn native_action_checks_lifecycle_before_writing_package_data() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let package_store = PackageStore::open(&Config {
            data_dir: temp.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap();
        package_store
            .create_package(crate::resources::PackageInput {
                id: "native-call".into(),
                ..Default::default()
            })
            .unwrap();

        let id = ExtensionId::parse(YAML_EXTENSION_ID).unwrap();
        service.install(&gamer_yaml_archive()).await.unwrap();
        service.enable(&id).await.unwrap();
        service.start(&id).await.unwrap();
        let running_values = serde_json::json!({
            "package_id": "native-call",
            "name": "running",
            "yaml": "run:\n  - log: running\n"
        });
        service
            .call_extension(
                &id,
                super::super::gamer_yaml::actions::AUTOMATION_SAVE_DRAFT,
                running_values,
            )
            .await
            .unwrap();
        service.disable(&id).await.unwrap();

        let blocked_values = serde_json::json!({
            "package_id": "native-call",
            "name": "blocked",
            "yaml": "run:\n  - log: blocked\n"
        });
        let error = service
            .call_extension(
                &id,
                super::super::gamer_yaml::actions::AUTOMATION_SAVE_DRAFT,
                blocked_values,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, ExtensionError::InvalidTransition { .. }));
        assert!(!temp
            .path()
            .join("packages/native-call/plugins/gamer.yaml/automations/blocked.yaml")
            .exists());
        assert!(temp
            .path()
            .join("packages/native-call/plugins/gamer.yaml/automations/running.yaml")
            .exists());

        let blocked_template = serde_json::json!({
            "package_id": "native-call",
            "name": "blocked-template",
            "png_base64": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
            "region": [0.1, 0.2, 0.3, 0.4],
            "frame": { "media_id": "m-1", "frame_index": 1, "pts_us": 1000 },
            "calibration": { "version": 1, "reference_size": [1280, 720], "rotation": 0 }
        });
        let error = service
            .call_extension(
                &id,
                super::super::gamer_yaml::actions::TEMPLATE_CREATE_FROM_FRAME,
                blocked_template,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, ExtensionError::InvalidTransition { .. }));
        assert!(!temp
            .path()
            .join("packages/native-call/plugins/gamer.yaml/templates/blocked-template#100_200_300_400.png")
            .exists());
    }

    /// A persisted Running record from a previous process is not a live
    /// execution context. Native actions must reject it before reaching their
    /// PackageStore handler, even if the request payload itself is valid.
    #[tokio::test]
    async fn stale_running_record_cannot_call_before_reconcile() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let package_store = PackageStore::open(&Config {
            data_dir: temp.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap();
        package_store
            .create_package(crate::resources::PackageInput {
                id: "stale-call".into(),
                ..Default::default()
            })
            .unwrap();

        let id = ExtensionId::parse(YAML_EXTENSION_ID).unwrap();
        service.install(&gamer_yaml_archive()).await.unwrap();
        service
            .force_state(&id, ExtensionState::Running, None)
            .await
            .unwrap();

        let error = service
            .call_extension(
                &id,
                super::super::gamer_yaml::actions::AUTOMATION_SAVE_DRAFT,
                serde_json::json!({
                    "package_id": "stale-call",
                    "name": "must-not-write",
                    "yaml": "run:\n  - log: stale\n"
                }),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error, ExtensionError::RuntimeUnavailable(_)),
            "旧进程 Running 记录不得被当作当前实例: {error}"
        );
        assert!(!temp
            .path()
            .join("packages/stale-call/plugins/gamer.yaml/automations/must-not-write.yaml")
            .exists());
    }

    /// Native permission metadata is enforced before dispatch: a valid save
    /// request from a Running target without resource.write cannot create a
    /// package file.
    #[tokio::test]
    async fn native_action_permission_gate_precedes_handler() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let package_store = PackageStore::open(&Config {
            data_dir: temp.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap();
        package_store
            .create_package(crate::resources::PackageInput {
                id: "permission-call".into(),
                ..Default::default()
            })
            .unwrap();
        let id = ExtensionId::parse(YAML_EXTENSION_ID).unwrap();
        service
            .install(&wasm_archive_with_deps(YAML_EXTENSION_ID, "1.0.0", ""))
            .await
            .unwrap();
        service.enable(&id).await.unwrap();
        service.start(&id).await.unwrap();

        let error = service
            .call_extension(
                &id,
                super::super::gamer_yaml::actions::AUTOMATION_SAVE_DRAFT,
                serde_json::json!({
                    "package_id": "permission-call",
                    "name": "must-not-write",
                    "yaml": "run:\n  - log: blocked\n"
                }),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, ExtensionError::Permission(_)), "{error}");
        assert!(!temp
            .path()
            .join("packages/permission-call/plugins/gamer.yaml/automations/must-not-write.yaml")
            .exists());
    }

    /// The plugin-originated seam derives caller identity from the running
    /// extension and pins package-scoped writes to the supplied content
    /// context. A caller that merely puts another plugin ID in request values
    /// cannot pass this check.
    #[tokio::test]
    async fn plugin_call_uses_trusted_caller_and_package_context() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let video = ExtensionId::parse(super::super::video::VIDEO_EXTENSION_ID).unwrap();
        let yaml = ExtensionId::parse(YAML_EXTENSION_ID).unwrap();
        let attacker = ExtensionId::parse("com.example.attacker").unwrap();
        let package = crate::core::AppPackageId::new("plugin-context").unwrap();

        let packages = PackageStore::open(&Config {
            data_dir: temp.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap();
        packages
            .create_package(crate::resources::PackageInput {
                id: package.as_str().to_string(),
                ..Default::default()
            })
            .unwrap();
        packages
            .create_package(crate::resources::PackageInput {
                id: "other-context".into(),
                ..Default::default()
            })
            .unwrap();

        service.install(&builtin_video_archive()).await.unwrap();
        service.install(&gamer_yaml_archive()).await.unwrap();
        service
            .install(&wasm_archive_with_deps(attacker.as_str(), "1.0.0", ""))
            .await
            .unwrap();
        service
            .enable_and_start(
                &video,
                Some(AppContext::for_test("device-1", "plugin-context").unwrap()),
                None,
            )
            .await
            .unwrap();
        service.enable(&yaml).await.unwrap();
        service.start(&yaml).await.unwrap();
        service.enable(&attacker).await.unwrap();
        service.start(&attacker).await.unwrap();

        let action = super::super::gamer_yaml::actions::AUTOMATION_SAVE_DRAFT;
        let values = serde_json::json!({
            "package_id": package.as_str(),
            "name": "from-video",
            "yaml": "run:\n  - log: from video\n"
        });
        let video_context = service.plugin_call_context(&video).await.unwrap();
        service
            .call_extension_from_plugin(&video_context, &yaml, action, values)
            .await
            .unwrap();
        assert!(temp
            .path()
            .join("packages/plugin-context/plugins/gamer.yaml/automations/from-video.yaml")
            .exists());

        let spoofed = serde_json::json!({
            "package_id": package.as_str(),
            "caller": video.as_str(),
            "name": "spoofed",
            "yaml": "run:\n  - log: spoofed\n"
        });
        let attacker_context = service.plugin_call_context(&attacker).await.unwrap();
        let error = service
            .call_extension_from_plugin(&attacker_context, &yaml, action, spoofed)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("不匹配动作"), "{error}");
        assert!(!temp
            .path()
            .join("packages/plugin-context/plugins/gamer.yaml/automations/spoofed.yaml")
            .exists());

        let wrong_package = serde_json::json!({
            "package_id": "other-context",
            "name": "wrong-package",
            "yaml": "run:\n  - log: wrong package\n"
        });
        let error = service
            .call_extension_from_plugin(&video_context, &yaml, action, wrong_package)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Package Context"), "{error}");
        assert!(!temp
            .path()
            .join("packages/other-context/plugins/gamer.yaml/automations/wrong-package.yaml")
            .exists());

        service.disable(&video).await.unwrap();
        let stale_error = service
            .call_extension_from_plugin(
                &video_context,
                &yaml,
                action,
                serde_json::json!({
                    "package_id": package.as_str(),
                    "name": "stale-context",
                    "yaml": "run:\n  - log: stale\n"
                }),
            )
            .await
            .unwrap_err();
        assert!(matches!(stale_error, ExtensionError::RuntimeUnavailable(_)));
        assert!(!temp
            .path()
            .join("packages/plugin-context/plugins/gamer.yaml/automations/stale-context.yaml")
            .exists());
    }

    /// The same per-extension gate is used by calls and lifecycle mutations:
    /// a stop waits for an already admitted call instead of racing its state
    /// transition with the side effect.
    #[tokio::test]
    async fn lifecycle_write_gate_waits_for_admitted_call_lease() {
        let temp = tempfile::tempdir().unwrap();
        let service = Arc::new(dependency_service(temp.path()));
        let id = ExtensionId::parse("com.example.gated").unwrap();
        service
            .install(&wasm_archive_with_deps(id.as_str(), "1.0.0", ""))
            .await
            .unwrap();
        service.enable(&id).await.unwrap();
        service.start(&id).await.unwrap();

        let gate = service.call_gate(&id);
        let lease = gate.read().await;
        let stop_service = service.clone();
        let stop_id = id.clone();
        let stop = tokio::spawn(async move { stop_service.disable(&stop_id).await });
        tokio::task::yield_now().await;
        assert!(!stop.is_finished(), "生命周期写闸门不应越过活跃调用租约");
        drop(lease);
        assert_eq!(
            stop.await.unwrap().unwrap().state(),
            ExtensionState::Disabled
        );
    }

    /// The old failure writer cannot pass a lifecycle transition without the
    /// same write gate as disable/start.
    #[tokio::test]
    async fn force_state_waits_for_lifecycle_gate() {
        let temp = tempfile::tempdir().unwrap();
        let service = Arc::new(dependency_service(temp.path()));
        let id = ExtensionId::parse("com.example.force-gated").unwrap();
        service
            .install(&wasm_archive_with_deps(id.as_str(), "1.0.0", ""))
            .await
            .unwrap();
        service.enable(&id).await.unwrap();
        service.start(&id).await.unwrap();

        let gate = service.call_gate(&id);
        let lease = gate.read().await;
        let force_service = service.clone();
        let force_id = id.clone();
        let force = tokio::spawn(async move {
            force_service
                .force_state(&force_id, ExtensionState::Disabled, None)
                .await
        });
        tokio::task::yield_now().await;
        assert!(!force.is_finished(), "force_state 不得越过活跃生命周期租约");
        drop(lease);
        force.await.unwrap().unwrap();
        assert_eq!(snapshot_state(&service, &id), ExtensionState::Disabled);
    }

    /// 可选依赖缺失不阻止启动；依赖恢复后报告转为满足。
    #[tokio::test]
    async fn optional_dependency_does_not_block_start() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let id = ExtensionId::parse("com.example.optconsumer").unwrap();

        service
            .install(&wasm_archive_with_deps(
                "com.example.optconsumer",
                "1.0.0",
                "[[dependencies]]\nid = \"gamer.yaml\"\nversion = \"*\"\nrequired = false\n",
            ))
            .await
            .unwrap();
        service.enable(&id).await.unwrap();
        let snapshot = service.start(&id).await.unwrap();
        assert_eq!(
            snapshot.state(),
            ExtensionState::Running,
            "可选依赖缺失不阻止启动"
        );
        assert!(snapshot.last_error().is_none());

        let manifest = snapshot.manifest().clone();
        let report = service.dependency_report(&manifest);
        assert!(!report[0].required);
        assert!(!report[0].satisfied);
        assert_eq!(report[0].note.as_deref(), Some("依赖未安装"));
    }

    /// A persisted Running provider without a current-process runner/instance
    /// is stale and must not satisfy a required dependency.
    #[tokio::test]
    async fn stale_persisted_running_dependency_is_not_satisfied() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let provider = ExtensionId::parse("com.example.stale.provider").unwrap();
        let consumer = ExtensionId::parse("com.example.stale.consumer").unwrap();
        service
            .install(&wasm_archive_with_deps(provider.as_str(), "1.0.0", ""))
            .await
            .unwrap();
        service
            .install(&wasm_archive_with_deps(
                consumer.as_str(),
                "1.0.0",
                "[[dependencies]]\nid = \"com.example.stale.provider\"\nversion = \">=1.0\"\n",
            ))
            .await
            .unwrap();
        mark_running(&service, &provider);

        let manifest = service.snapshot_for(&consumer).unwrap().manifest().clone();
        let report = service.dependency_report(&manifest);
        assert_eq!(report[0].state, Some(ExtensionState::Running));
        assert!(!report[0].satisfied);
        assert_eq!(report[0].note.as_deref(), Some("依赖未在当前进程运行"));

        service.enable(&consumer).await.unwrap();
        let error = service.start(&consumer).await.unwrap_err();
        assert!(matches!(
            error,
            ExtensionError::DependencyUnsatisfied { .. }
        ));
    }

    /// Dependency matching is against the provider's version: B=3.0.0
    /// satisfies a dependent requirement of >=2.0.0.
    #[tokio::test]
    async fn provider_three_satisfies_required_dependency_at_two() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let provider = ExtensionId::parse("com.example.version.provider").unwrap();
        let consumer = ExtensionId::parse("com.example.version.consumer").unwrap();
        service
            .install(&wasm_archive_with_deps(provider.as_str(), "3.0.0", ""))
            .await
            .unwrap();
        service
            .install(&wasm_archive_with_deps(
                consumer.as_str(),
                "1.0.0",
                "[[dependencies]]\nid = \"com.example.version.provider\"\nversion = \">=2.0.0\"\n",
            ))
            .await
            .unwrap();
        service.enable(&provider).await.unwrap();
        service.start(&provider).await.unwrap();
        service.enable(&consumer).await.unwrap();
        service.start(&consumer).await.unwrap();

        let manifest = service.snapshot_for(&consumer).unwrap().manifest().clone();
        let report = service.dependency_report(&manifest);
        assert!(report[0].satisfied);
        assert_eq!(report[0].version.as_deref(), Some("3.0.0"));
    }

    /// Optional dependencies never protect a provider from stop/uninstall;
    /// the dependent remains live and reports the optional capability as
    /// unavailable after the provider is removed.
    #[tokio::test]
    async fn optional_provider_stop_and_uninstall_do_not_block() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let provider = ExtensionId::parse("com.example.optional.provider.lifecycle").unwrap();
        let consumer = ExtensionId::parse("com.example.optional.consumer.lifecycle").unwrap();
        service
            .install(&wasm_archive_with_deps(provider.as_str(), "1.0.0", ""))
            .await
            .unwrap();
        service
            .install(&wasm_archive_with_deps(
                consumer.as_str(),
                "1.0.0",
                "[[dependencies]]\nid = \"com.example.optional.provider.lifecycle\"\nrequired = false\n",
            ))
            .await
            .unwrap();
        service.enable(&provider).await.unwrap();
        service.start(&provider).await.unwrap();
        service.enable(&consumer).await.unwrap();
        service.start(&consumer).await.unwrap();

        service.stop(&provider).await.unwrap();
        assert!(service
            .uninstall(&provider, &ExtensionVersion::parse("1.0.0").unwrap())
            .await
            .unwrap());
        assert_eq!(snapshot_state(&service, &consumer), ExtensionState::Running);
        let manifest = service.snapshot_for(&consumer).unwrap().manifest().clone();
        assert!(!service.dependency_report(&manifest)[0].satisfied);
    }

    /// Required-provider restore failure is explicit: the provider degrades
    /// with its diagnostic and its dependent is not restored as Running.
    #[tokio::test]
    async fn required_provider_restore_failure_degrades_dependent() {
        let temp = tempfile::tempdir().unwrap();
        let provider = ExtensionId::parse("com.example.restore.required.provider").unwrap();
        let consumer = ExtensionId::parse("com.example.restore.required.consumer").unwrap();
        let initial = dependency_service(temp.path());
        initial
            .install(&wasm_archive_with_deps(provider.as_str(), "1.0.0", ""))
            .await
            .unwrap();
        initial
            .install(&wasm_archive_with_deps(
                consumer.as_str(),
                "1.0.0",
                "[[dependencies]]\nid = \"com.example.restore.required.provider\"\n",
            ))
            .await
            .unwrap();
        mark_running(&initial, &provider);
        mark_running(&initial, &consumer);
        drop(initial);

        let restarted = ExtensionService::for_data_root(
            temp.path(),
            crate::capabilities::CapabilityRegistry::default(),
        )
        .with_runner_registrar(Arc::new(OptionalFailureRegistrar {
            failed_id: provider.to_string(),
        }));
        restarted.reconcile_startup().await;

        let provider_snapshot = restarted.snapshot_for(&provider).unwrap();
        assert_eq!(provider_snapshot.state(), ExtensionState::Enabled);
        assert!(provider_snapshot
            .last_error()
            .is_some_and(|error| error.contains("optional provider restore failed")));
        let consumer_snapshot = restarted.snapshot_for(&consumer).unwrap();
        assert_eq!(consumer_snapshot.state(), ExtensionState::Enabled);
        assert!(consumer_snapshot
            .last_error()
            .is_some_and(|error| error.contains("必需依赖")));
    }

    /// A required-dependency cycle during startup is diagnosed for every
    /// candidate; no cycle member is accidentally restored by ID order.
    #[tokio::test]
    async fn reconcile_startup_rejects_required_dependency_cycle() {
        let temp = tempfile::tempdir().unwrap();
        let a = ExtensionId::parse("com.example.restore.cycle.a").unwrap();
        let b = ExtensionId::parse("com.example.restore.cycle.b").unwrap();
        let initial = dependency_service(temp.path());
        initial
            .install(&wasm_archive_with_deps(
                a.as_str(),
                "1.0.0",
                "[[dependencies]]\nid = \"com.example.restore.cycle.b\"\n",
            ))
            .await
            .unwrap();
        initial
            .install(&wasm_archive_with_deps(
                b.as_str(),
                "1.0.0",
                "[[dependencies]]\nid = \"com.example.restore.cycle.a\"\n",
            ))
            .await
            .unwrap();
        mark_running(&initial, &a);
        mark_running(&initial, &b);
        drop(initial);

        let recorder = RestoreOrderRegistrar::default();
        let restarted = ExtensionService::for_data_root(
            temp.path(),
            crate::capabilities::CapabilityRegistry::default(),
        )
        .with_runner_registrar(Arc::new(recorder.clone()));
        restarted.reconcile_startup().await;

        assert!(recorder.started.lock().unwrap().is_empty());
        for id in [&a, &b] {
            let snapshot = restarted.snapshot_for(id).unwrap();
            assert_eq!(snapshot.state(), ExtensionState::Enabled);
            assert!(snapshot
                .last_error()
                .is_some_and(|error| error.contains("循环")));
        }
    }

    /// 必需依赖循环（A→B→A）→ 启动拒绝并给出清晰错误（不互等死锁）。
    #[tokio::test]
    async fn required_dependency_cycle_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let service = dependency_service(temp.path());
        let a = ExtensionId::parse("com.example.cyclea").unwrap();

        service
            .install(&wasm_archive_with_deps(
                "com.example.cyclea",
                "1.0.0",
                "[[dependencies]]\nid = \"com.example.cycleb\"\n",
            ))
            .await
            .unwrap();
        service
            .install(&wasm_archive_with_deps(
                "com.example.cycleb",
                "1.0.0",
                "[[dependencies]]\nid = \"com.example.cyclea\"\n",
            ))
            .await
            .unwrap();
        service.enable(&a).await.unwrap();

        let error = service.start(&a).await.unwrap_err();
        assert!(error.to_string().contains("循环"), "{error}");
        assert_eq!(snapshot_state(&service, &a), ExtensionState::Enabled);
    }

    /// A snapshot error aborts the whole restore pass instead of silently
    /// falling back to lexical IDs. No candidate may be started from an
    /// incomplete dependency view.
    #[tokio::test]
    async fn reconcile_startup_aborts_when_restore_snapshot_fails() {
        let temp = tempfile::tempdir().unwrap();
        let id = ExtensionId::parse("com.example.restore.snapshot-error").unwrap();
        let initial = dependency_service(temp.path());
        initial
            .install(&wasm_archive_with_deps(id.as_str(), "1.0.0", ""))
            .await
            .unwrap();
        mark_running(&initial, &id);
        let mut states = initial.store().read_state().unwrap();
        states.get_mut(&id).unwrap().active_version =
            Some(ExtensionVersion::parse("9.9.9").unwrap());
        initial.store().write_state(&states).unwrap();
        drop(initial);

        let recorder = RestoreOrderRegistrar::default();
        let restarted = ExtensionService::for_data_root(
            temp.path(),
            crate::capabilities::CapabilityRegistry::default(),
        )
        .with_runner_registrar(Arc::new(recorder.clone()));
        restarted.reconcile_startup().await;

        assert!(recorder.started.lock().unwrap().is_empty());
        let state = restarted.store().read_state().unwrap();
        assert_eq!(state.get(&id).unwrap().state, ExtensionState::Running);
    }

    /// manifest 依赖声明解析：缺省 required = true、version 缺省 `*`、
    /// 重复 id / 自引用 / 非法版本 → InvalidManifest。
    #[test]
    fn manifest_dependency_declaration_parsing() {
        let ok = crate::extensions::parse_manifest(
            br#"manifest_version = 2
id = "com.example.deps"
version = "1.0.0"
name = "D"
entry = "plugin.wasm"
[[dependencies]]
id = "gamer.video"
[[dependencies]]
id = "gamer.yaml"
version = "^3.0"
required = false
"#,
        )
        .unwrap();
        let deps = ok.dependencies();
        assert_eq!(deps.len(), 2);
        assert!(deps[0].required(), "缺省 required = true");
        assert_eq!(deps[0].version_req().to_string(), "*");
        assert!(!deps[1].required());
        assert_eq!(deps[1].version_req().to_string(), "^3.0");

        // 自引用
        let err = crate::extensions::parse_manifest(
            br#"manifest_version = 2
id = "com.example.deps"
version = "1.0.0"
name = "D"
entry = "plugin.wasm"
[[dependencies]]
id = "com.example.deps"
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("自身"), "{err}");

        // 重复 id
        let err = crate::extensions::parse_manifest(
            br#"manifest_version = 2
id = "com.example.deps"
version = "1.0.0"
name = "D"
entry = "plugin.wasm"
[[dependencies]]
id = "gamer.video"
[[dependencies]]
id = "gamer.video"
required = false
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("重复"), "{err}");

        // 非法版本要求
        let err = crate::extensions::parse_manifest(
            br#"manifest_version = 2
id = "com.example.deps"
version = "1.0.0"
name = "D"
entry = "plugin.wasm"
[[dependencies]]
id = "gamer.video"
version = "not-a-req"
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("version 无效"), "{err}");
    }
}
