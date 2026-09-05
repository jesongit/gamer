use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::super::{
    CapabilityError, CapabilityResult, RunHandle, RunRequest, RunService, RunStatus,
};
use crate::core::{AndroidPackageName, AppContext, AppPackageId, DeviceId, RunPayload};
use crate::run_manager::{CancelOutcome, RunManager, RunSource, StartError, StartRequest};

use super::ResourceAdapter;

/// run.submit 能力提交的手动脚本运行固定路由到的自动化 runner id（当前由
/// gamer.yaml 扩展经 ADR-13 注册缝提供）。这是 Core 侧的路由语义字符串——
/// Core 不解读其内容、不依赖该扩展的任何符号；id 与扩展市场包 id 一致，
/// 由 P11.9 守卫测试白名单约束。
pub(crate) const AUTOMATION_RUNNER_ID: &str = "gamer.yaml";

/// Native bridge from the small capability request to the generic RunManager.
///
/// The WIT contract deliberately passes an opaque resource handle instead of a
/// host path. The resource adapter resolves that handle back to its logical
/// id, and this adapter translates the `scripts/<script>.yaml` convention into a
/// generic automation-runner request. RunManager still owns mutual exclusion,
/// cancellation, terminal state, and history.
pub(crate) struct RunAdapter {
    manager: Arc<RunManager>,
    resources: Arc<ResourceAdapter>,
    handles: Mutex<HashMap<RunHandle, String>>,
}

impl RunAdapter {
    pub(crate) fn new(manager: Arc<RunManager>, resources: Arc<ResourceAdapter>) -> Self {
        Self {
            manager,
            resources,
            handles: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn run_id(&self, handle: RunHandle) -> CapabilityResult<String> {
        self.handles
            .lock()
            .map_err(|_| CapabilityError::Failed("run handle state poisoned".into()))?
            .get(&handle)
            .cloned()
            .ok_or_else(|| CapabilityError::NotFound("run handle".into()))
    }

    fn request_for(&self, request: RunRequest) -> CapabilityResult<StartRequest> {
        let resource = self.resources.id(request.entry())?;
        let script = resource.name().strip_prefix("scripts/").ok_or_else(|| {
            CapabilityError::InvalidRequest(
                "run.submit entry 必须是 scripts/<script>.yaml 逻辑资源".into(),
            )
        })?;
        if script.is_empty() || !script.to_ascii_lowercase().ends_with(".yaml") {
            return Err(CapabilityError::InvalidRequest(
                "run.submit entry 必须指向 .yaml 脚本".into(),
            ));
        }
        let package = AppPackageId::new(resource.namespace())
            .map_err(|error| CapabilityError::InvalidRequest(error.to_string()))?;
        let android = AndroidPackageName::new(resource.namespace())
            .map_err(|error| CapabilityError::InvalidRequest(error.to_string()))?;
        let app = AppContext::new(
            DeviceId::new(request.device().id().as_str())
                .map_err(|error| CapabilityError::InvalidRequest(error.to_string()))?,
            android,
            Some(package),
        );
        // 通用 runner 分发约定（P11.6）：runner_id = 自动化 runner 注册 id，
        // entrypoint = `<内容分区>/<脚本>`，payload 为 runner 私有不透明值
        // （缺省对象 = 默认目标从头跑）。目标/payload 语义由注册该 runner 的
        // 扩展解码，Core 只构造 generic RunRequest。
        let inner = crate::core::RunRequest::for_app(
            app,
            AUTOMATION_RUNNER_ID,
            format!("{}/{}", resource.namespace(), script),
            RunPayload::new(serde_json::json!({})),
        )
        .map_err(|error| CapabilityError::InvalidRequest(error.to_string()))?;
        Ok(StartRequest {
            request: inner,
            source: RunSource::Manual,
            task_id: None,
            scheduled_at: None,
            realtime_logs: false,
        })
    }
}

#[async_trait]
impl RunService for RunAdapter {
    async fn submit(&self, request: RunRequest) -> CapabilityResult<RunHandle> {
        let start = self.request_for(request)?;
        let record = self
            .manager
            .submit(start, None)
            .map_err(|error| match error {
                StartError::Conflict(record) => CapabilityError::Failed(format!(
                    "device busy: {} ({})",
                    record.run_id, record.script_id
                )),
                StartError::ShuttingDown => {
                    CapabilityError::Unavailable("run manager is shutting down")
                }
            })?;
        let handle = RunHandle::new();
        self.handles
            .lock()
            .map_err(|_| CapabilityError::Failed("run handle state poisoned".into()))?
            .insert(handle, record.run_id);
        Ok(handle)
    }

    async fn cancel(&self, run: RunHandle) -> CapabilityResult<()> {
        let run_id = self.run_id(run)?;
        match self.manager.cancel(&run_id) {
            CancelOutcome::Accepted => Ok(()),
            CancelOutcome::NotFound => Err(CapabilityError::NotFound("run".into())),
            CancelOutcome::AlreadyFinished(state) => Err(CapabilityError::Failed(format!(
                "run already finished: {state:?}"
            ))),
        }
    }

    async fn status(&self, run: RunHandle) -> CapabilityResult<RunStatus> {
        let run_id = self.run_id(run)?;
        let record = self
            .manager
            .get_run(&run_id)
            .ok_or_else(|| CapabilityError::NotFound("run".into()))?;
        Ok(match record.state {
            crate::run_manager::RunState::Starting => RunStatus::Queued,
            crate::run_manager::RunState::Running | crate::run_manager::RunState::Stopping => {
                RunStatus::Running
            }
            crate::run_manager::RunState::Success => RunStatus::Succeeded,
            crate::run_manager::RunState::Failed => RunStatus::Failed,
            crate::run_manager::RunState::Cancelled => RunStatus::Cancelled,
        })
    }
}
