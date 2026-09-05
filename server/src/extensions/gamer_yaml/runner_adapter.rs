//! Native YAML runner adapter for the generic RunManager boundary.
//!
//! This module is deliberately the only place that translates a
//! [`RunTarget`] and typed YAML arguments into `core::RunRequest` payload
//! data, and the single v3 execution entry: scripts must be `version: 3`
//! (`yaml_vnext::load`), functions resolve via `yaml_vnext::load_function`;
//! anything else fails with an unsupported-version error — no fallback.

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
use crate::extensions::gamer_yaml::run_target::{RunSpec, RunTarget, TypedValue};
use crate::extensions::gamer_yaml::yaml_extension::YamlProgramResolver;
use crate::extensions::gamer_yaml::yaml_vnext::{self, Program, Value};
use crate::run_manager::{RunExecutor, RunSource, StartRequest};
use crate::store::Db;

/// `TypedValue` intentionally only implements the public scalar JSON shape;
/// the generic request needs a lossless private wire encoding so the adapter
/// can reconstruct parameter types without teaching the YAML model
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

/// Build a generic request while preserving the YAML target/args format
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

/// Production executor: YAML decoding and v3 execution stay at the execution
/// boundary; RunManager only sees generic core values.
pub struct EngineExecutor {
    devices: Arc<DeviceManager>,
    db: Db,
    /// Filled after RunManager construction because the native capability
    /// registry itself contains a RunService backed by that manager.
    yaml_vnext: Arc<std::sync::RwLock<Option<Arc<YamlVnextAdapter>>>>,
}

/// 非 `version: 3` 源的统一运行错误（无 fallback；版本门禁与
/// `yaml_vnext` 保存期诊断同码 `yaml.v3.version`）。
fn unsupported_version(resource: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "yaml.v3.version: 不支持的 YAML 版本（{resource}）——当前只支持 version: 3 脚本"
    )
}

impl EngineExecutor {
    pub fn new(devices: Arc<DeviceManager>, db: Db) -> Self {
        Self {
            devices,
            db,
            yaml_vnext: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    pub fn attach_yaml_vnext(
        &self,
        scripts: Arc<crate::resources::ResourceStore>,
        extensions: Arc<crate::extensions::ExtensionService>,
        sink: Option<Arc<dyn crate::core::events::EventSink>>,
    ) {
        *self
            .yaml_vnext
            .write()
            .expect("YAML vNext adapter lock poisoned") = Some(Arc::new(YamlVnextAdapter {
            scripts,
            extensions: Arc::downgrade(&extensions),
            sink,
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
        _realtime_logs: bool,
        stop: Arc<AtomicBool>,
    ) -> BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
        Box::pin(async move {
            let spec = Self::decode(request, context)?;
            let adapter = self
                .yaml_vnext
                .read()
                .expect("YAML vNext adapter lock poisoned")
                .clone()
                .ok_or_else(|| anyhow::anyhow!("YAML v3 运行适配器未装配"))?;
            adapter.execute(&spec, stop).await
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
    scripts: Arc<crate::resources::ResourceStore>,
    extensions: Weak<crate::extensions::ExtensionService>,
    /// v3 运行可视化事件汇（P12.6）：viewer 的 DataChannel 旁路；None = 静默。
    sink: Option<Arc<dyn crate::core::events::EventSink>>,
}

/// 包内可调用资源（`scripts/` / `functions/` 分区）解析器：resolve 仅被
/// wasm-runtime 的 YAML guest programs 通道调用，无该 feature 时字段不被读取。
/// target 命名空间解析与穿越校验收口在 [`yaml_vnext::split_call_target`]。
#[cfg_attr(not(feature = "wasm-runtime"), allow(dead_code))]
struct ScriptProgramResolver {
    scripts: Arc<crate::resources::ResourceStore>,
    package: String,
}

impl ScriptProgramResolver {
    /// 分区内相对 id → 资源 id：`.yaml` 后缀与 `<pkg>/` 前缀都可省略
    /// （`daily/login` → `<pkg>/daily/login.yaml`）。
    fn resource_id(&self, id: &str) -> String {
        let id = id.trim();
        let lower = id.to_ascii_lowercase();
        let with_ext = if lower.ends_with(".yaml") || lower.ends_with(".yml") {
            id.to_string()
        } else {
            format!("{id}.yaml")
        };
        if with_ext.starts_with(&format!("{}/", self.package)) {
            with_ext
        } else {
            format!("{}/{}", self.package, with_ext)
        }
    }

    fn resolve_script(&self, id: &str) -> anyhow::Result<Program> {
        let target = self.resource_id(id);
        let script = self
            .scripts
            .get_text(crate::resources::ResourceKind::Scripts, &target)?
            .ok_or_else(|| anyhow::anyhow!("找不到 v3 call 目标: {target}"))?;
        if !yaml_vnext::is_v3_source(&script.content) {
            return Err(unsupported_version(&target));
        }
        yaml_vnext::load(&script.content)
            .map_err(|diagnostics| v3_diagnostics_error("v3 call 目标无效", &diagnostics))
    }

    /// `function:<文件短路径>/<函数名>`：functions/ 分区定位文件 → v3 函数库
    /// 解析 → 取目标函数 `{params, steps}` 组装 Program（ADR-YAML-02）。
    fn resolve_function(&self, file: &str, function: &str) -> anyhow::Result<Program> {
        let target = self.resource_id(file);
        let entry = self
            .scripts
            .get_text(crate::resources::ResourceKind::Functions, &target)?
            .ok_or_else(|| anyhow::anyhow!("找不到 v3 call 函数文件: {target}"))?;
        yaml_vnext::load_function(&entry.content, function)
            .map_err(|diagnostics| v3_diagnostics_error("v3 函数无效", &diagnostics))
    }
}

fn v3_diagnostics_error(prefix: &str, diagnostics: &[yaml_vnext::Diagnostic]) -> anyhow::Error {
    anyhow::anyhow!(
        "{}: {}",
        prefix,
        diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("；")
    )
}

impl YamlProgramResolver for ScriptProgramResolver {
    fn resolve(&self, target: &str, _args: &BTreeMap<String, Value>) -> anyhow::Result<Program> {
        // P12.4（ADR-YAML-04）：调用深度由 guest 本地 ExecutionBudget 计数，
        // resolver 只按命名空间定位目标程序，不再做深度守卫。
        let parsed = yaml_vnext::split_call_target(target)
            .map_err(|diagnostics| v3_diagnostics_error("v3 call 目标无效", &diagnostics))?;
        match parsed {
            yaml_vnext::CallTarget::Script(id) => self.resolve_script(&id),
            yaml_vnext::CallTarget::Function { file, function } => {
                self.resolve_function(&file, &function)
            }
        }
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
    /// v3 唯一执行路径：脚本非 `version: 3`（含 v2 存量源）→ 统一版本错误，
    /// 无任何 fallback。
    async fn execute(
        &self,
        spec: &RunSpec,
        stop: Arc<AtomicBool>,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let (program, package, start_index) = match &spec.target {
            RunTarget::Script {
                script_id,
                start_index,
            } => {
                let scripts = self.scripts.clone();
                let probe_id = script_id.clone();
                let script = tokio::task::spawn_blocking(move || {
                    scripts.get_text(crate::resources::ResourceKind::Scripts, &probe_id)
                })
                .await
                .map_err(|error| anyhow::anyhow!("读取 v3 脚本失败: {error}"))??;
                let Some(script) = script else {
                    anyhow::bail!("脚本不存在: {}", script_id);
                };
                if !yaml_vnext::is_v3_source(&script.content) {
                    return Err(unsupported_version(&script_id));
                }
                let program = yaml_vnext::load(&script.content)
                    .map_err(|diagnostics| v3_diagnostics_error("v3 脚本无效", &diagnostics))?;
                (program, script.package, Some(*start_index))
            }
            RunTarget::Function {
                pkg,
                file,
                function,
                start_index,
            } => {
                let target = format!("{pkg}/{file}.yaml");
                let scripts = self.scripts.clone();
                let probe_target = target.clone();
                let entry = tokio::task::spawn_blocking(move || {
                    scripts.get_text(crate::resources::ResourceKind::Functions, &probe_target)
                })
                .await
                .map_err(|error| anyhow::anyhow!("读取 v3 函数库失败: {error}"))??;
                let Some(entry) = entry else {
                    anyhow::bail!("函数文件不存在: {target}");
                };
                // 函数库是 bare-map（无 version 键，v3-ness 由步语法承载）：
                // 直接按 v3 严格解析，结构不符即报解析诊断。
                let name = match function {
                    Some(name) => name.clone(),
                    None => {
                        // 缺省 = 文件内第一个函数
                        let library = yaml_vnext::parse_function_library(&entry.content)
                            .map_err(|diagnostics| {
                                v3_diagnostics_error("v3 函数库无效", &diagnostics)
                            })?;
                        library
                            .first()
                            .map(|decl| decl.name.clone())
                            .ok_or_else(|| anyhow::anyhow!("函数文件 {target} 未定义任何函数"))?
                    }
                };
                let program = yaml_vnext::load_function(&entry.content, &name)
                    .map_err(|diagnostics| v3_diagnostics_error("v3 函数无效", &diagnostics))?;
                (program, pkg.clone(), Some(*start_index))
            }
        };
        let resolver = Arc::new(ScriptProgramResolver {
            scripts: self.scripts.clone(),
            package,
        });
        let extensions = self
            .extensions
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("YAML 扩展服务已关闭"))?;
        // 「从此运行」start_index 经 YamlWasmRunRequest 注入 program JSON，
        // 由 guest 按顶层 surface 步序号跳步（契约 §8），不再预切片。
        // P12.6：运行事件汇随请求下发（viewer DataChannel 旁路）。
        crate::extensions::gamer_yaml::run_yaml_vnext(
            &extensions,
            program,
            spec.context.app.clone(),
            yaml_args(&spec.args),
            Some(resolver),
            stop,
            start_index,
            self.sink.clone(),
        )
        .await
        .map(|_| Vec::new())
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
        assert_eq!(request.request.payload.as_value()["target"]["start_index"], 2);
    }

    /// ScriptProgramResolver 的 script:/function: 命名空间解析
    /// （真实 ResourceStore + 分区目录）。P12.4 起深度守卫归 guest 本地，
    /// resolver 不再接收/校验 depth。
    #[test]
    fn script_program_resolver_supports_namespaced_targets() {
        let data = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            data_dir: data.path().to_path_buf(),
            ..Default::default()
        };
        let store = Arc::new(crate::resources::ResourceStore::open(&cfg).unwrap());
        store
            .save_text(
                crate::resources::ResourceKind::Scripts,
                None,
                "com.test.app",
                "sub/inner.yaml",
                "version: 3\nsteps:\n  - log: inner\n",
            )
            .unwrap();
        store
            .save_text(
                crate::resources::ResourceKind::Functions,
                None,
                "com.test.app",
                "lib.yaml",
                "fn1:\n  params:\n    - name: n\n      type: number\n      default: 1\n  steps:\n    - return: $n\n",
            )
            .unwrap();
        let resolver = ScriptProgramResolver {
            scripts: store,
            package: "com.test.app".into(),
        };

        // script: 分区内相对 id（.yaml 可省略，可含子目录）
        let program = resolver.resolve("script:sub/inner", &BTreeMap::new()).unwrap();
        assert_eq!(program.steps.len(), 1);
        // function: 文件短路径/函数名
        let program = resolver.resolve("function:lib/fn1", &BTreeMap::new()).unwrap();
        assert_eq!(program.params.len(), 1);
        assert_eq!(program.steps.len(), 1);

        // 裸 target 拒绝（yaml.v3.call.namespace）
        let error = resolver.resolve("helper", &BTreeMap::new()).unwrap_err();
        assert!(
            error.to_string().contains("yaml.v3.call.namespace"),
            "裸 target 必须报命名空间诊断: {error}"
        );
        // 穿越拒绝
        let error = resolver
            .resolve("function:../evil/fn", &BTreeMap::new())
            .unwrap_err();
        assert!(error.to_string().contains("yaml.v3.call.target"));
        // 不存在的目标
        let error = resolver
            .resolve("script:nope/missing", &BTreeMap::new())
            .unwrap_err();
        assert!(error.to_string().contains("找不到 v3 call 目标"));
        let error = resolver
            .resolve("function:lib/missing", &BTreeMap::new())
            .unwrap_err();
        assert!(error.to_string().contains("yaml.v3.function.not_found"));
    }

    /// 非 `version: 3` 源：脚本与函数两条路径都必须落在统一版本错误
    /// （v3-only 门禁，无 fallback）。
    #[test]
    fn resolver_rejects_non_v3_sources_with_version_error() {
        let data = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            data_dir: data.path().to_path_buf(),
            ..Default::default()
        };
        let store = Arc::new(crate::resources::ResourceStore::open(&cfg).unwrap());
        store
            .save_text(
                crate::resources::ResourceKind::Scripts,
                None,
                "com.test.app",
                "legacy.yaml",
                "steps:\n  - log: v2 形态\n",
            )
            .unwrap();
        let resolver = ScriptProgramResolver {
            scripts: store,
            package: "com.test.app".into(),
        };
        let error = resolver
            .resolve("script:legacy", &BTreeMap::new())
            .unwrap_err();
        assert!(
            error.to_string().contains("yaml.v3.version"),
            "非 v3 脚本必须报版本错误: {error}"
        );
    }

    /// 分区内相对 id → 资源 id 的归一规则（不触碰磁盘）。
    #[test]
    fn resolver_resource_id_normalization() {
        let data = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            data_dir: data.path().to_path_buf(),
            ..Default::default()
        };
        let resolver = ScriptProgramResolver {
            scripts: Arc::new(crate::resources::ResourceStore::open(&cfg).unwrap()),
            package: "com.test.app".into(),
        };
        assert_eq!(
            resolver.resource_id("daily/login"),
            "com.test.app/daily/login.yaml"
        );
        assert_eq!(resolver.resource_id("daily.yaml"), "com.test.app/daily.yaml");
        assert_eq!(
            resolver.resource_id("com.test.app/daily.yaml"),
            "com.test.app/daily.yaml"
        );
    }
}
