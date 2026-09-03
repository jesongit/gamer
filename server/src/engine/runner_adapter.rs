//! Native YAML runner adapter for the generic RunManager boundary.
//!
//! This module is deliberately the only place that translates a legacy
//! `RunTarget` and typed YAML arguments into `core::RunRequest` payload data.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Weak};

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
use crate::yaml_extension::YamlProgramResolver;
use crate::yaml_vnext::{Program, Value};

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
    /// Filled after RunManager construction because the native capability
    /// registry itself contains a RunService backed by that manager.
    yaml_vnext: Arc<std::sync::RwLock<Option<Arc<YamlVnextAdapter>>>>,
}

impl EngineExecutor {
    pub fn new(runner: Arc<Runner>, devices: Arc<DeviceManager>, db: Db) -> Self {
        Self {
            runner,
            devices,
            db,
            yaml_vnext: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    pub fn attach_yaml_vnext(
        &self,
        scripts: Arc<crate::scripts::ScriptStore>,
        extensions: Arc<crate::extensions::ExtensionService>,
    ) {
        *self
            .yaml_vnext
            .write()
            .expect("YAML vNext adapter lock poisoned") = Some(Arc::new(YamlVnextAdapter {
            scripts,
            extensions: Arc::downgrade(&extensions),
        }));
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
            let adapter = self
                .yaml_vnext
                .read()
                .expect("YAML vNext adapter lock poisoned")
                .clone();
            if let Some(adapter) = adapter {
                if let Some(logs) = adapter.execute(&spec, stop.clone()).await? {
                    return Ok(logs);
                }
            }
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

struct YamlVnextAdapter {
    scripts: Arc<crate::scripts::ScriptStore>,
    extensions: Weak<crate::extensions::ExtensionService>,
}

/// 包内脚本（`scripts/`）解析器：resolve 仅被 wasm-runtime 的 YAML guest
/// programs 通道调用，无该 feature 时字段不被读取。
#[cfg_attr(not(feature = "wasm-runtime"), allow(dead_code))]
struct ScriptProgramResolver {
    scripts: Arc<crate::scripts::ScriptStore>,
    package: String,
}

impl YamlProgramResolver for ScriptProgramResolver {
    fn resolve(&self, target: &str, _args: &BTreeMap<String, Value>) -> anyhow::Result<Program> {
        let target = target.trim();
        let target = if target.contains('/') {
            target.to_string()
        } else if target.to_ascii_lowercase().ends_with(".yaml")
            || target.to_ascii_lowercase().ends_with(".yml")
        {
            format!("{}/{}", self.package, target)
        } else {
            format!("{}/{}.yaml", self.package, target)
        };
        let script = self
            .scripts
            .get(&target)?
            .ok_or_else(|| anyhow::anyhow!("找不到 v3 call 目标: {target}"))?;
        if !crate::yaml_vnext::is_v3_source(&script.content) {
            anyhow::bail!("call 目标不是 v3 脚本: {target}");
        }
        crate::yaml_vnext::load(&script.content).map_err(|diagnostics| {
            anyhow::anyhow!(
                "v3 call 目标无效: {}",
                diagnostics
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("；")
            )
        })
    }
}

fn yaml_args(args: &[(String, TypedValue)]) -> BTreeMap<String, Value> {
    args.iter()
        .map(|(name, value)| {
            let value = match value {
                TypedValue::Tmpl(value) | TypedValue::Text(value) | TypedValue::Key(value) => {
                    Value::String(value.clone())
                }
                TypedValue::Coord(value) => Value::Coordinate(*value),
                TypedValue::Color(value) => Value::Color(value.clone()),
                TypedValue::Time(value) => Value::String(value.clone()),
                TypedValue::Bool(value) => Value::Bool(*value),
            };
            (name.clone(), value)
        })
        .collect()
}

impl YamlVnextAdapter {
    async fn execute(
        &self,
        spec: &RunSpec,
        stop: Arc<AtomicBool>,
    ) -> anyhow::Result<Option<Vec<(String, String)>>> {
        let RunTarget::Script {
            script_id,
            start_index,
        } = &spec.target
        else {
            return Ok(None);
        };
        let scripts = self.scripts.clone();
        let script_id = script_id.clone();
        let script = tokio::task::spawn_blocking(move || scripts.get(&script_id))
            .await
            .map_err(|error| anyhow::anyhow!("读取 v3 脚本失败: {error}"))??;
        let Some(script) = script else {
            return Ok(None);
        };
        if !crate::yaml_vnext::is_v3_source(&script.content) {
            return Ok(None);
        }
        let mut program = crate::yaml_vnext::load(&script.content).map_err(|diagnostics| {
            anyhow::anyhow!(diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("；"))
        })?;
        if *start_index > program.steps.len() {
            anyhow::bail!(
                "start_index {} 超过 v3 脚本步数 {}",
                start_index,
                program.steps.len()
            );
        }
        if *start_index != 0 {
            program.steps = program.steps.into_iter().skip(*start_index).collect();
        }
        let resolver = Arc::new(ScriptProgramResolver {
            scripts: self.scripts.clone(),
            package: script.package.clone(),
        });
        let extensions = self
            .extensions
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("YAML 扩展服务已关闭"))?;
        extensions
            .run_yaml_vnext(
                program,
                spec.context.app.clone(),
                yaml_args(&spec.args),
                Some(resolver),
                stop,
            )
            .await
            .map(|_| Some(Vec::new()))
            .map_err(|error| anyhow::anyhow!(error.to_string()))
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
