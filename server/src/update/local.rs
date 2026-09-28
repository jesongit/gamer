//! In-process update control; only file replacement runs in a temporary worker.
use super::{
    controller::{Acceptance, Capabilities, UpdateController},
    ipc::{DependencyKind, UpdateError, UpdateStatus},
    model::{UpdateErrorCode, UpdateState},
};
use async_trait::async_trait;
use gamer_updater::{
    ipc::{frames::Operation, Dispatcher},
    portable,
    upgrade::engine::ManifestSource,
};
use std::sync::Arc;

pub struct LocalController(Arc<Dispatcher>);

impl LocalController {
    pub fn from_installation() -> Option<Self> {
        let layout = portable::layout()?;
        let opts = portable::engine_options(&layout).ok()?;
        Some(Self(Dispatcher::new(
            layout,
            "portable".into(),
            ManifestSource::configured(),
            opts,
            false,
        )))
    }

    async fn submit(&self, op: Operation) -> Result<Acceptance, UpdateError> {
        let dispatcher = self.0.clone();
        let reply = tokio::task::spawn_blocking(move || {
            dispatcher.submit(&uuid::Uuid::new_v4().to_string(), op)
        })
        .await
        .map_err(|e| UpdateError::new(UpdateErrorCode::UpdaterUnavailable, e.to_string()))?
        .frame;
        if reply["ok"] != true {
            return Err(UpdateError::new(
                UpdateErrorCode::parse(reply["error"]["code"].as_str().unwrap_or(""))
                    .unwrap_or(UpdateErrorCode::UpdaterUnavailable),
                reply["error"]["message"].as_str().unwrap_or("更新操作失败"),
            ));
        }
        Ok(Acceptance {
            update_id: reply["result"]["update_id"].as_str().map(str::to_owned),
            state: reply["result"]["state"]
                .as_str()
                .and_then(UpdateState::parse),
        })
    }
}

#[async_trait]
impl UpdateController for LocalController {
    fn strategy(&self) -> &'static str {
        "managed"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::MANAGED
    }
    async fn status(&self) -> Result<UpdateStatus, UpdateError> {
        let dispatcher = self.0.clone();
        let status = tokio::task::spawn_blocking(move || dispatcher.status_result())
            .await
            .map_err(|e| UpdateError::new(UpdateErrorCode::UpdaterUnavailable, e.to_string()))?;
        super::ipc::parse_status_update_block(&status).map_err(UpdateError::from_frame)
    }
    async fn check(&self) -> Result<Acceptance, UpdateError> {
        self.submit(Operation::Check).await
    }
    async fn download(&self) -> Result<Acceptance, UpdateError> {
        self.submit(Operation::Download).await
    }
    async fn prepare_install(&self) -> Result<Acceptance, UpdateError> {
        self.submit(Operation::PrepareInstall).await
    }
    async fn rollback(&self) -> Result<Acceptance, UpdateError> {
        self.submit(Operation::Rollback).await
    }
    async fn repair_dependency(&self, _: DependencyKind) -> Result<Acceptance, UpdateError> {
        Err(UpdateError::new(
            UpdateErrorCode::UpdateNotManaged,
            "请重新下载完整包修复运行依赖",
        ))
    }
}
