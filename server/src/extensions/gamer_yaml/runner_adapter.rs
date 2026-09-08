//! Native YAML runner adapter for the generic RunManager boundary.
//!
//! This module is deliberately the only place that translates a
//! [`RunTarget`] and raw parameter overrides into `core::RunRequest` payload
//! data, and the single V1 execution entry: it composes the run-scoped
//! function registry (native plugin functions + current Package functions),
//! validates call targets, binds entry args, lowers to the interpreter wire
//! program and executes it through the WASM guest. Parse/binding failures
//! surface as structured diagnostics — no fallback.

use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Weak};

use futures_util::future::BoxFuture;
use serde_json::{json, Map as JsonMap, Value};

use crate::core::{
    ActivityKind, ActivityLease, AndroidPackageName, AppContext, AppPackageId, DeviceId,
    RunContext, RunPayload, RunRequest,
};
use crate::device::DeviceManager;
use crate::extensions::gamer_yaml::error::{ScriptError, FUNCTION_CONFLICT, FUNCTION_NOT_FOUND};
use crate::extensions::gamer_yaml::native_funcs::native_names;
use crate::extensions::gamer_yaml::run_target::{RunSpec, RunTarget};
use crate::extensions::gamer_yaml::syntax::{
    build_function_program, build_program, parse_function_library, parse_script, FunctionDef,
    FunctionLibrary, Script,
};
use crate::extensions::gamer_yaml::task_params::bind_entry_args;
use crate::extensions::gamer_yaml::{resources, run_yaml_program, YAML_EXTENSION_ID};
use crate::run_manager::{RunExecutor, RunSource, StartRequest};
use crate::store::Db;

/// Build a generic request while preserving the YAML target/args format
/// inside the `gamer.yaml` payload. `args` = 稀疏原始覆盖（绑定在执行边界
/// 按当前 Schema 完成，计划 Phase 4.2）；手动运行严格拒绝未知键，任务路径
/// 宽松丢弃。
pub fn yaml_start_request(
    app: AppContext,
    target: RunTarget,
    source: RunSource,
    task_id: Option<String>,
    scheduled_at: Option<i64>,
    args: JsonMap<String, Value>,
    realtime_logs: bool,
) -> anyhow::Result<StartRequest> {
    let entrypoint = target.label();
    let strict_args = matches!(source, RunSource::Manual);
    let request = RunRequest::for_app(
        app,
        "gamer.yaml",
        entrypoint,
        RunPayload::new(json!({
            "target": target,
            "args": Value::Object(args),
            "strict_args": strict_args,
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

/// Production executor: YAML decoding and V1 execution stay at the execution
/// boundary; RunManager only sees generic core values.
pub struct EngineExecutor {
    devices: Arc<DeviceManager>,
    db: Db,
    /// Filled after RunManager construction because the native capability
    /// registry itself contains a RunService backed by that manager.
    yaml_vnext: Arc<std::sync::RwLock<Option<Arc<YamlRunAdapter>>>>,
}

impl EngineExecutor {
    pub fn new(devices: Arc<DeviceManager>, db: Db) -> Self {
        Self {
            devices,
            db,
            yaml_vnext: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    pub fn attach_yaml_runner(
        &self,
        scripts: Arc<crate::resources::PackageStore>,
        extensions: Arc<crate::extensions::ExtensionService>,
        sink: Option<Arc<dyn crate::core::events::EventSink>>,
    ) {
        *self
            .yaml_vnext
            .write()
            .expect("YAML vNext adapter lock poisoned") = Some(Arc::new(YamlRunAdapter {
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
            .map(|value| {
                value
                    .as_object()
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("gamer.yaml payload args 必须是对象"))
            })
            .transpose()?
            .unwrap_or_default();
        let strict_args = payload
            .get("strict_args")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(RunSpec {
            context: context.clone(),
            target,
            args,
            strict_args,
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
                .ok_or_else(|| anyhow::anyhow!("YAML V1 运行适配器未装配"))?;
            // 录制输入来源标注（合同 §2.1 / Phase 9 矩阵）：gamer.yaml runner
            // 经能力适配器注入的输入标记为 "runner"。guest 实例线程经
            // block_on_yaml 派生线程，task-local 不跨线程，由
            // `NativeYamlHost::call_function_json` 在线程内再标注。
            crate::capabilities::adapters::with_caller_input_source("runner", async {
                adapter.execute(&spec, stop).await
            })
            .await
        })
    }

    fn acquire(&self, context: &RunContext) -> anyhow::Result<Box<dyn ActivityLease>> {
        Ok(Box::new(self.devices.acquire_activity(
            context.device_id().as_str(),
            ActivityKind::Run,
        )))
    }
}

/// 一次 V1 运行的入口（blocking 池内读取并解析）。
enum Entry {
    Script {
        script_id: String,
        script: Script,
    },
    Function {
        target_id: String,
        name: String,
        def: FunctionDef,
    },
}

impl Entry {
    fn resource(&self) -> String {
        match self {
            Self::Script { script_id, .. } => script_id.clone(),
            Self::Function {
                target_id, name, ..
            } => format!("{target_id}#{name}"),
        }
    }
}

/// 组合运行期函数注册表：原生插件函数 + 当前 Package 全部函数文件
/// （计划 Phase 3.4：运行开始时冻结；同名冲突一律拒绝，不跨包查找）。
pub(crate) fn compose_function_library(
    store: &crate::resources::PackageStore,
    package: &str,
) -> anyhow::Result<FunctionLibrary> {
    let native = native_names();
    let files = store.list(package, YAML_EXTENSION_ID, "functions")?;
    let mut registry: FunctionLibrary = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for file in files {
        let Some(content) = file.content.as_deref() else {
            continue;
        };
        let library = parse_function_library(content).map_err(|diagnostics| {
            anyhow::anyhow!(
                "函数文件 {} 无效: {}",
                file.path,
                diagnostics_text(&diagnostics)
            )
        })?;
        for (name, def) in library {
            if native.contains(&name) {
                return Err(anyhow::anyhow!(
                    "{FUNCTION_CONFLICT}: Package 函数 {name:?} 与原生插件函数同名——请改名（{}）",
                    file.path
                ));
            }
            if !seen.insert(name.clone()) {
                return Err(anyhow::anyhow!(
                    "{FUNCTION_CONFLICT}: 函数 {name:?} 重复定义（见 {}）",
                    file.path
                ));
            }
            registry.push((name, def));
        }
    }
    Ok(registry)
}

fn diagnostics_text(diagnostics: &[crate::extensions::gamer_yaml::syntax::Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("；")
}

fn script_errors_text(errors: &[ScriptError]) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("；")
}

struct YamlRunAdapter {
    scripts: Arc<crate::resources::PackageStore>,
    extensions: Weak<crate::extensions::ExtensionService>,
    /// V1 运行可视化事件汇：viewer 的 DataChannel 旁路；None = 静默。
    sink: Option<Arc<dyn crate::core::events::EventSink>>,
}

impl YamlRunAdapter {
    /// V1 唯一执行路径：解析入口 → 组合冻结函数表 → 校验调用面 → 绑定参数 →
    /// 降线 → guest。旧 v3 源在解析层直接报 `yaml.version.removed`（无
    /// fallback），其余坏源报结构化诊断。
    async fn execute(
        &self,
        spec: &RunSpec,
        stop: Arc<AtomicBool>,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let scripts = self.scripts.clone();
        let target = spec.target.clone();
        let (entry, library) =
            tokio::task::spawn_blocking(move || -> anyhow::Result<(Entry, FunctionLibrary)> {
                let library = compose_function_library(&scripts, target.pkg())?;
                let entry = match &target {
                    RunTarget::Script { script_id, .. } => {
                        let content = resources::script_entry(&scripts, script_id)?
                            .ok_or_else(|| anyhow::anyhow!("脚本不存在: {script_id}"))?
                            .content;
                        let script = parse_script(&content).map_err(|diagnostics| {
                            anyhow::anyhow!("脚本无效: {}", diagnostics_text(&diagnostics))
                        })?;
                        Entry::Script {
                            script_id: script_id.clone(),
                            script,
                        }
                    }
                    RunTarget::Function {
                        pkg,
                        file,
                        function,
                        ..
                    } => {
                        let target_id = format!("{pkg}/{file}.yaml");
                        let content = resources::function_entry(&scripts, &target_id)?
                            .ok_or_else(|| anyhow::anyhow!("函数文件不存在: {target_id}"))?
                            .content;
                        let file_library =
                            parse_function_library(&content).map_err(|diagnostics| {
                                anyhow::anyhow!("函数文件无效: {}", diagnostics_text(&diagnostics))
                            })?;
                        let name = match function {
                            Some(name) => name.clone(),
                            None => file_library
                                .first()
                                .map(|(name, _)| name.clone())
                                .ok_or_else(|| {
                                    anyhow::anyhow!("函数文件 {target_id} 未定义任何函数")
                                })?,
                        };
                        let (_, def) = file_library
                            .iter()
                            .find(|(entry, _)| entry == &name)
                            .ok_or_else(|| {
                                anyhow::anyhow!("函数 {name:?} 不在文件 {target_id} 中")
                            })?;
                        Entry::Function {
                            target_id,
                            name,
                            def: def.clone(),
                        }
                    }
                };
                Ok((entry, library))
            })
            .await
            .map_err(|error| anyhow::anyhow!("读取 YAML 资源失败: {error}"))??;

        // 注册表 = 原生函数 ∪ Package 函数 ∪（函数目标入口自身）
        let mut registry = native_names();
        for (name, _) in &library {
            registry.insert(name.clone());
        }
        let entry_calls = match &entry {
            Entry::Function { name, def, .. } => {
                registry.insert(name.clone());
                def.called_functions()
            }
            Entry::Script { script, .. } => script.called_functions(),
        };
        for (owner, def) in &library {
            if let Some(name) = def
                .called_functions()
                .iter()
                .find(|name| !registry.contains(*name))
                .cloned()
            {
                return Err(anyhow::anyhow!(
                    "{FUNCTION_NOT_FOUND}: 函数 {owner} 调用的 {name:?} 不存在（可用：原生插件函数 + 当前 Package 函数）"
                ));
            }
        }
        if let Some(name) = entry_calls
            .iter()
            .find(|name| !registry.contains(*name))
            .cloned()
        {
            return Err(anyhow::anyhow!(
                "{FUNCTION_NOT_FOUND}: 函数 {name:?} 不存在（可用：原生插件函数 + 当前 Package 函数）"
            ));
        }

        let extensions = self
            .extensions
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("YAML 扩展服务已关闭"))?;

        let program = match &entry {
            Entry::Script { script_id, script } => {
                let bound =
                    bind_entry_args(script_id, &script.params, &spec.args, spec.strict_args)
                        .map_err(|diagnostics| {
                            anyhow::anyhow!("参数绑定失败: {}", script_errors_text(&diagnostics))
                        })?;
                let mut initial: JsonMap<String, Value> = script.vars.iter().cloned().collect();
                for (name, value) in bound.resolved {
                    initial.insert(name, value);
                }
                build_program(script, &library, initial, spec.target.start_index())
            }
            Entry::Function { name, def, .. } => {
                let bound =
                    bind_entry_args(&entry.resource(), &def.params, &spec.args, spec.strict_args)
                        .map_err(|diagnostics| {
                        anyhow::anyhow!("参数绑定失败: {}", script_errors_text(&diagnostics))
                    })?;
                let mut initial: JsonMap<String, Value> = def.vars.iter().cloned().collect();
                for (name, value) in bound.resolved {
                    initial.insert(name, value);
                }
                build_function_program(name, def, &library, initial, spec.target.start_index())
            }
        };
        run_yaml_program(
            &extensions,
            program,
            spec.context.app.clone(),
            stop,
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
        // 两个命名空间显式分离：Android 包名（launch/stop_app 缺省目标）与
        // Package id（资源解析域）各自给值，不存在互相兜底。
        let app = AppContext::new(
            DeviceId::new("d1").unwrap(),
            AndroidPackageName::new("com.example.game").unwrap(),
            Some(AppPackageId::new("official.hsr.daily").unwrap()),
        );
        let request = yaml_start_request(
            app,
            RunTarget::Script {
                script_id: "official.hsr.daily/daily.yaml".into(),
                start_index: 2,
            },
            RunSource::Manual,
            None,
            None,
            serde_json::from_value(json!({"retry": 2})).unwrap(),
            true,
        )
        .unwrap();
        assert_eq!(request.request.runner_id, "gamer.yaml");
        assert_eq!(request.request.entrypoint, "official.hsr.daily/daily.yaml");
        assert_eq!(
            request.request.payload.as_value()["target"]["start_index"],
            2
        );
        assert_eq!(request.request.payload.as_value()["args"]["retry"], 2);
    }

    #[test]
    fn compose_function_library_merges_files_and_rejects_conflicts() {
        let data = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            data_dir: data.path().to_path_buf(),
            ..Default::default()
        };
        let store = Arc::new(crate::resources::PackageStore::open(&cfg).unwrap());
        store
            .create_package(crate::resources::PackageInput {
                id: "com.test.app".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "functions/common.yaml",
                "functions:\n  greet:\n    run:\n      - log: hi\n",
                None,
                false,
            )
            .unwrap();
        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "functions/daily.yaml",
                "functions:\n  claim:\n    run:\n      - greet: {}\n",
                None,
                false,
            )
            .unwrap();
        let library = compose_function_library(&store, "com.test.app").unwrap();
        assert_eq!(library.len(), 2);
        let calls = library
            .iter()
            .find(|(name, _)| name == "claim")
            .map(|(_, def)| def.called_functions())
            .unwrap();
        assert!(calls.contains("greet"));

        // 与原生函数同名 → 冲突拒绝
        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "functions/bad.yaml",
                "functions:\n  tap:\n    run: []\n",
                None,
                false,
            )
            .unwrap();
        let error = compose_function_library(&store, "com.test.app").unwrap_err();
        assert!(
            error.to_string().contains(FUNCTION_CONFLICT),
            "同名必须报冲突: {error}"
        );

        // 跨文件同名 → 冲突拒绝
        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "functions/bad.yaml",
                "functions:\n  greet:\n    run: []\n",
                None,
                true,
            )
            .unwrap();
        let error = compose_function_library(&store, "com.test.app").unwrap_err();
        assert!(error.to_string().contains(FUNCTION_CONFLICT), "{error}");

        // 坏函数文件 → 带文件名的解析错误
        store
            .write_text(
                "com.test.app",
                YAML_EXTENSION_ID,
                "functions/bad.yaml",
                "functions:\n  if:\n    run: []\n",
                None,
                true,
            )
            .unwrap();
        let error = compose_function_library(&store, "com.test.app").unwrap_err();
        assert!(error.to_string().contains("bad.yaml"), "{error}");
    }

    /// Package 隔离：函数表只组合当前 Package 的函数（不跨包隐式查找）。
    #[test]
    fn package_function_isolation_between_packages() {
        let data = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            data_dir: data.path().to_path_buf(),
            ..Default::default()
        };
        let store = Arc::new(crate::resources::PackageStore::open(&cfg).unwrap());
        for package in ["com.a", "com.b"] {
            store
                .create_package(crate::resources::PackageInput {
                    id: package.into(),
                    ..Default::default()
                })
                .unwrap();
        }
        store
            .write_text(
                "com.a",
                YAML_EXTENSION_ID,
                "functions/only_a.yaml",
                "functions:\n  a_fn:\n    run: []\n",
                None,
                false,
            )
            .unwrap();
        let a = compose_function_library(&store, "com.a").unwrap();
        let b = compose_function_library(&store, "com.b").unwrap();
        assert_eq!(a.len(), 1);
        assert!(b.is_empty(), "b 包看不到 a 包函数");
    }
}
