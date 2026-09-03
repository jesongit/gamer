//! Native YAML runner adapter for the generic RunManager boundary.
//!
//! This module is deliberately the only place that translates a legacy
//! `RunTarget` and typed YAML arguments into `core::RunRequest` payload data.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde_json::json;

use crate::core::{
    ActivityKind, ActivityLease, AndroidPackageName, AppContext, AppPackageId, DeviceId,
    RunContext, RunPayload, RunRequest,
};
use crate::device::DeviceManager;
use crate::run_manager::{RunExecutor, RunSource, StartRequest};
use crate::script_v2::TypedValue;
use crate::store::Db;

use super::exec::{RunSpec, RunTarget, Runner};

/// `TypedValue` intentionally only implements the public scalar JSON shape;
/// the generic request needs a lossless private wire encoding so the adapter
/// can reconstruct parameter types without teaching the legacy YAML model
/// about RunManager serialization.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum WireTypedValue {
    Tmpl(String),
    Coord([f64; 2]),
    Color(String),
    Time(String),
    Key(String),
    Text(String),
    Bool(bool),
}

impl From<&TypedValue> for WireTypedValue {
    fn from(value: &TypedValue) -> Self {
        match value {
            TypedValue::Tmpl(value) => Self::Tmpl(value.clone()),
            TypedValue::Coord(value) => Self::Coord(*value),
            TypedValue::Color(value) => Self::Color(value.clone()),
            TypedValue::Time(value) => Self::Time(value.clone()),
            TypedValue::Key(value) => Self::Key(value.clone()),
            TypedValue::Text(value) => Self::Text(value.clone()),
            TypedValue::Bool(value) => Self::Bool(*value),
        }
    }
}

impl From<WireTypedValue> for TypedValue {
    fn from(value: WireTypedValue) -> Self {
        match value {
            WireTypedValue::Tmpl(value) => Self::Tmpl(value),
            WireTypedValue::Coord(value) => Self::Coord(value),
            WireTypedValue::Color(value) => Self::Color(value),
            WireTypedValue::Time(value) => Self::Time(value),
            WireTypedValue::Key(value) => Self::Key(value),
            WireTypedValue::Text(value) => Self::Text(value),
            WireTypedValue::Bool(value) => Self::Bool(value),
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct WireArg {
    name: String,
    value: WireTypedValue,
}

/// Build a generic request while preserving the old YAML target/args format
/// inside the `gamer.yaml` payload.
pub fn yaml_start_request(
    app: AppContext,
    target: RunTarget,
    source: RunSource,
    task_id: Option<String>,
    scheduled_at: Option<i64>,
    args: Vec<(String, TypedValue)>,
    realtime_logs: bool,
) -> anyhow::Result<StartRequest> {
    let entrypoint = target.label();
    let request = RunRequest::for_app(
        app,
        "gamer.yaml",
        entrypoint,
        RunPayload::new(json!({
            "target": target,
            "args": args
                .iter()
                .map(|(name, value)| WireArg {
                    name: name.clone(),
                    value: WireTypedValue::from(value),
                })
                .collect::<Vec<_>>(),
        })),
    )?;
    Ok(StartRequest {
        request,
        source,
        task_id,
        scheduled_at,
        realtime_logs,
    })
}

/// Construct the current YAML app scope.  A configured Android package is
/// preferred; falling back to the content package keeps old device rows and
/// tests runnable during the migration.
pub fn yaml_app_context(
    device_id: impl Into<String>,
    android_package: Option<String>,
    content_package: impl Into<String>,
) -> anyhow::Result<AppContext> {
    let content_package = content_package.into();
    let android_package = android_package.unwrap_or_else(|| content_package.clone());
    Ok(AppContext::new(
        DeviceId::new(device_id)?,
        AndroidPackageName::new(android_package)?,
        Some(AppPackageId::new(content_package)?),
    ))
}

/// Production executor: YAML decoding and engine execution stay at the
/// execution boundary; RunManager only sees generic core values.
pub struct EngineExecutor {
    runner: Arc<Runner>,
    devices: Arc<DeviceManager>,
    db: Db,
}

impl EngineExecutor {
    pub fn new(runner: Arc<Runner>, devices: Arc<DeviceManager>, db: Db) -> Self {
        Self {
            runner,
            devices,
            db,
        }
    }

    fn decode(request: &RunRequest, context: &RunContext) -> anyhow::Result<RunSpec> {
        anyhow::ensure!(
            request.runner_id == "gamer.yaml",
            "不支持的 runner: {}",
            request.runner_id
        );
        let payload = request
            .payload
            .as_value()
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("gamer.yaml payload 必须是对象"))?;
        let target = serde_json::from_value(
            payload
                .get("target")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("gamer.yaml payload 缺少 target"))?,
        )?;
        let args = payload
            .get("args")
            .cloned()
            .map(serde_json::from_value::<Vec<WireArg>>)
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .map(|arg| (arg.name, arg.value.into()))
            .collect();
        Ok(RunSpec {
            context: context.clone(),
            target,
            args,
        })
    }
}

impl RunExecutor for EngineExecutor {
    fn prepare<'a>(
        &'a self,
        context: &'a RunContext,
        _request: &'a RunRequest,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let device_id = context.device_id().as_str();
            if self.devices.session(device_id).is_none() {
                self.devices.connect_device(device_id).await?;
            }
            Ok(())
        })
    }

    fn execute<'a>(
        &'a self,
        context: &'a RunContext,
        request: &'a RunRequest,
        realtime_logs: bool,
        stop: Arc<AtomicBool>,
    ) -> BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
        Box::pin(async move {
            let log_cb: Option<Arc<dyn Fn(String, String) + Send + Sync>> =
                if realtime_logs && request.payload.as_value().is_object() {
                    let db = self.db.clone();
                    let device_id = context.device_id().to_string();
                    let entrypoint = request.entrypoint.clone();
                    Some(Arc::new(move |level, message| {
                        let db = db.clone();
                        let device_id = device_id.clone();
                        let entrypoint = entrypoint.clone();
                        tokio::spawn(async move {
                            if let Err(error) = db
                                .add_log_async(&device_id, &entrypoint, &level, &message)
                                .await
                            {
                                tracing::warn!(%error, "runtime log write failed");
                            }
                        });
                    }))
                } else {
                    None
                };
            let spec = Self::decode(request, context)?;
            self.runner.run(&spec, stop, log_cb).await
        })
    }

    fn acquire(&self, context: &RunContext) -> anyhow::Result<Box<dyn ActivityLease>> {
        Ok(Box::new(self.devices.acquire_activity(
            context.device_id().as_str(),
            ActivityKind::Run,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_adapter_keeps_runner_and_target_outside_manager() {
        let app = yaml_app_context("d1", Some("com.example.game".into()), "content").unwrap();
        let request = yaml_start_request(
            app,
            RunTarget::Script {
                script_id: "content/daily.yaml".into(),
                start_index: 2,
            },
            RunSource::Manual,
            None,
            None,
            vec![],
            true,
        )
        .unwrap();
        assert_eq!(request.request.runner_id, "gamer.yaml");
        assert_eq!(request.request.entrypoint, "content/daily.yaml");
        assert_eq!(
            request.request.payload.as_value()["target"]["start_index"],
            2
        );
    }
}
