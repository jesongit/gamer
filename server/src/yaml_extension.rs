//! YAML v3 extension boundary.
//!
//! The interpreter here is intentionally small. It owns control flow and
//! lowering policy, while every device/frame/vision/runtime operation goes
//! through an existing Core capability. The WASM implementation uses the same
//! `CapabilityInvoker` contract through the generic WIT `capability.invoke`
//! escape hatch.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_recursion::async_recursion;
use async_trait::async_trait;

use crate::capabilities::{
    AppId, CapabilityRegistry, DeviceHandle, DeviceId, FramePoint, FrameSize, KeyAction, KeyCode,
    KeyInput, LogLevel, LogRecord, MatchManyRequest, MatchOptions, MatchOutcome, ResourceId,
    RuntimeService, SwipeGesture, TemplateQuery, TextInput, TouchPoint,
};
use crate::core::AppContext;
use crate::extensions::{HostApi, Permission};

use crate::yaml_vnext::{Condition, Expr, Program, SmallStep, Value};

pub(crate) const YAML_EXTENSION_ID: &str = "gamer.yaml";
/// Reference manifest for the installable YAML guest. The server never embeds
/// its WASM bytes; package installation supplies `plugin.wasm` independently.
pub(crate) const YAML_EXTENSION_MANIFEST_TOML: &str = r#"manifest_version = 1
id = "gamer.yaml"
version = "3.0.0"
name = "Gamer YAML vNext"
description = "Surface YAML v3 lowering and execution guest"
entry = "plugin.wasm"
permissions = ["device.read", "device.app", "input.tap", "input.swipe", "input.key", "input.text", "vision.match", "vision.color", "resource.read", "runtime.sleep", "log.write"]

[host_api]
device = "^1.0"
vision = "^1.0"
input = "^1.0"
resource = "^1.0"
runtime = "^1.0"
log = "^1.0"

[[ui.contributions]]
panel_id = "automation"
title = "自动化"
icon = "⚙️"
order = 25
location = "console.right"
runtime = "iframe"
requires_device = true
preferred_width = 440
entry = "ui/automation.html"

[[ui.contributions]]
panel_id = "functions"
title = "函数"
icon = "ƒ"
order = 30
location = "console.right"
runtime = "iframe"
requires_device = false
preferred_width = 440
entry = "ui/functions.html"
"#;

#[derive(Debug)]
pub(crate) enum CompatibleYamlError {
    V2(Vec<crate::script_v2::ScriptError>),
    V3(Vec<crate::yaml_vnext::Diagnostic>),
}

impl CompatibleYamlError {
    pub(crate) fn into_json(self) -> serde_json::Value {
        match self {
            Self::V2(diagnostics) => serde_json::to_value(diagnostics).unwrap_or_default(),
            Self::V3(diagnostics) => serde_json::to_value(diagnostics).unwrap_or_default(),
        }
    }
}

#[derive(Debug)]
pub(crate) enum CompatibleYamlSource {
    V2,
    V3(Program),
}

/// Compatibility adapter used by save/import/run boundaries. v2 remains
/// owned by the existing strict loader; v3 is parsed only by yaml_vnext.
pub(crate) fn validate_compatible_script(
    scripts: &crate::scripts::ScriptStore,
    package: &str,
    resource: &str,
    source: &str,
) -> Result<CompatibleYamlSource, CompatibleYamlError> {
    if crate::yaml_vnext::is_v3_source(source) {
        crate::yaml_vnext::load(source)
            .map(CompatibleYamlSource::V3)
            .map_err(CompatibleYamlError::V3)
    } else {
        scripts
            .parse_script_content(package, resource, source)
            .map(|_| CompatibleYamlSource::V2)
            .map_err(CompatibleYamlError::V2)
    }
}

const DEFAULT_SCREEN_WIDTH: u32 = 1000;
const DEFAULT_SCREEN_HEIGHT: u32 = 1000;
const MAX_STEP_BUDGET: u64 = 100_000;

/// Request passed to the real YAML Component runtime. The program is already
/// lowered by the extension front-end; the guest only interprets the small
/// wire AST and calls capability.invoke.
#[derive(Clone)]
pub(crate) struct YamlWasmRunRequest {
    pub(crate) wasm: Vec<u8>,
    pub(crate) program: Program,
    pub(crate) args: BTreeMap<String, Value>,
    pub(crate) resolver: Option<Arc<dyn YamlProgramResolver>>,
    pub(crate) host: HostApi,
    pub(crate) context: AppContext,
    pub(crate) stop: Arc<AtomicBool>,
}

#[derive(Debug)]
pub(crate) struct YamlWasmRunResult {
    pub(crate) value: Value,
}

#[async_trait]
pub(crate) trait YamlWasmRuntime: Send + Sync {
    async fn run(&self, request: YamlWasmRunRequest) -> Result<YamlWasmRunResult>;

    fn is_available(&self) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NoYamlWasmRuntime;

#[async_trait]
impl YamlWasmRuntime for NoYamlWasmRuntime {
    async fn run(&self, _request: YamlWasmRunRequest) -> Result<YamlWasmRunResult> {
        bail!("未启用 wasm-runtime feature")
    }

    fn is_available(&self) -> bool {
        false
    }
}

/// Dynamic capability boundary used by the small AST. This is deliberately a
/// generic operation; the trait has no YAML types and can be reused by another
/// extension that speaks the same JSON-safe Value protocol.
#[async_trait]
pub(crate) trait CapabilityInvoker: Send + Sync {
    async fn invoke(&self, capability: &str, args: Value) -> Result<Value>;

    fn cancelled(&self) -> bool {
        false
    }
}

/// YAML extension-only lookup for the small AST `call` node. This does not
/// enter `CapabilityRegistry`, so YAML source/resource semantics stay out of
/// Core capabilities.
pub(crate) trait YamlProgramResolver: Send + Sync {
    fn resolve(&self, target: &str, args: &BTreeMap<String, Value>) -> Result<Program>;
}

/// Native host adapter used by tests and by the no-WASM compatibility path.
/// It is an adapter, not a Core capability: the registry remains the only
/// source of device/input/frame/vision functionality.
pub(crate) struct NativeYamlHost {
    host: HostApi,
    registry: CapabilityRegistry,
    context: AppContext,
    device: DeviceHandle,
    runtime: Arc<dyn RuntimeService>,
    screen: FrameSize,
}

impl NativeYamlHost {
    pub(crate) async fn new(
        host: HostApi,
        context: AppContext,
        stop: Arc<AtomicBool>,
    ) -> Result<Self> {
        let registry = host.registry().clone();
        let device_service = registry
            .device()
            .ok_or_else(|| anyhow!("device capability 未注册"))?;
        let device = device_service
            .resolve(&DeviceId::new(context.device_id.as_str()))
            .await
            .map_err(anyhow::Error::new)?;
        Ok(Self {
            host,
            registry,
            context,
            device,
            runtime: Arc::new(crate::capabilities::adapters::RuntimeAdapter::new(stop)),
            screen: FrameSize::new(DEFAULT_SCREEN_WIDTH, DEFAULT_SCREEN_HEIGHT),
        })
    }

    pub(crate) fn with_screen(mut self, screen: FrameSize) -> Self {
        self.screen = screen;
        self
    }

    pub(crate) async fn invoke_json(
        host: HostApi,
        context: AppContext,
        stop: Arc<AtomicBool>,
        capability: &str,
        args_json: &str,
    ) -> Result<Value> {
        let args = serde_json::from_str::<serde_json::Value>(args_json)
            .map_err(|error| anyhow!("invoke args 不是合法 JSON: {error}"))?;
        let args = Value::from_json(args).map_err(|error| anyhow!("invoke args 无效: {error}"))?;
        let host = Self::new(host, context, stop).await?;
        host.invoke(capability, args).await
    }

    fn authorize(&self, capability: &str) -> Result<()> {
        let permission = match capability {
            "device.resolve" => Permission::DeviceRead,
            "app.start" | "app.stop" | "device.start_app" | "device.stop_app" => {
                Permission::DeviceApp
            }
            "input.tap" => Permission::InputTap,
            "input.swipe" => Permission::InputSwipe,
            "input.key" => Permission::InputKey,
            "input.text" => Permission::InputText,
            "vision.match" | "vision.match_template" | "vision.match_many" | "frame.capture" => {
                Permission::VisionMatch
            }
            "vision.sample_color" => Permission::VisionColor,
            "runtime.sleep" => Permission::RuntimeSleep,
            "log.write" => Permission::LogWrite,
            other => bail!("未知 capability: {other}"),
        };
        self.host.authorize(permission).map_err(anyhow::Error::new)
    }

    fn args_map(args: Value) -> Result<BTreeMap<String, Value>> {
        match args {
            Value::Map(args) => Ok(args),
            Value::Null => Ok(BTreeMap::new()),
            _ => bail!("capability 参数必须是 record/map"),
        }
    }

    fn arg<'a>(args: &'a BTreeMap<String, Value>, name: &str) -> Result<&'a Value> {
        args.get(name)
            .ok_or_else(|| anyhow!("缺少 capability 参数 {name}"))
    }

    fn package(&self, args: &BTreeMap<String, Value>) -> Result<String> {
        match args.get("package").or_else(|| args.get("app")) {
            Some(Value::String(value)) if !value.trim().is_empty() => Ok(value.clone()),
            Some(Value::Null) | None => Ok(self.context.android_package.as_str().to_string()),
            Some(value) => bail!("package 必须是字符串，得到 {value:?}"),
        }
    }

    fn point(&self, value: &Value) -> Result<TouchPoint> {
        let Value::Coordinate([x, y]) = value else {
            bail!("point 必须是 [0..1, 0..1] 坐标")
        };
        if !(0.0..=1.0).contains(x) || !(0.0..=1.0).contains(y) {
            bail!("point 坐标超出 0..1")
        }
        Ok(TouchPoint::new(
            (x * self.screen.width as f64).round() as u32,
            (y * self.screen.height as f64).round() as u32,
            1.0,
        ))
    }

    fn frame_point(&self, value: &Value) -> Result<FramePoint> {
        let point = self.point(value)?;
        Ok(FramePoint::new(point.x(), point.y()))
    }

    fn resource_name(value: &Value) -> Result<String> {
        match value {
            Value::String(value) | Value::Color(value) if !value.trim().is_empty() => {
                Ok(value.trim_start_matches("tmpl/").to_string())
            }
            _ => bail!("template 必须是非空字符串"),
        }
    }

    async fn template(&self, value: &Value) -> Result<crate::capabilities::ResourceHandle> {
        self.host
            .authorize(Permission::ResourceRead)
            .map_err(anyhow::Error::new)?;
        let name = Self::resource_name(value)?;
        let package = self
            .context
            .content_package
            .as_ref()
            .ok_or_else(|| anyhow!("当前上下文没有 content package"))?;
        let resource = self
            .registry
            .resource()
            .ok_or_else(|| anyhow!("resource capability 未注册"))?
            .resolve(&ResourceId::new(
                package.as_str().to_string(),
                format!("tmpl/{name}"),
            ))
            .await
            .map_err(anyhow::Error::new)?;
        Ok(resource)
    }

    async fn capture(&self) -> Result<crate::capabilities::FrameHandle> {
        self.registry
            .frame()
            .ok_or_else(|| anyhow!("frame capability 未注册"))?
            .capture(&self.device)
            .await
            .map_err(anyhow::Error::new)
    }

    fn match_value(outcome: MatchOutcome) -> Value {
        match outcome {
            MatchOutcome::Found(found) => {
                let center = [
                    (found.x + found.width / 2) as f64 / DEFAULT_SCREEN_WIDTH as f64,
                    (found.y + found.height / 2) as f64 / DEFAULT_SCREEN_HEIGHT as f64,
                ];
                Value::Map(BTreeMap::from([
                    ("found".to_string(), Value::Bool(true)),
                    ("x".to_string(), Value::Int(found.x as i64)),
                    ("y".to_string(), Value::Int(found.y as i64)),
                    ("width".to_string(), Value::Int(found.width as i64)),
                    ("height".to_string(), Value::Int(found.height as i64)),
                    ("score".to_string(), Value::Float(found.score as f64)),
                    ("center".to_string(), Value::Coordinate(center)),
                ]))
            }
            MatchOutcome::NotFound => {
                Value::Map(BTreeMap::from([("found".to_string(), Value::Bool(false))]))
            }
        }
    }

    fn color_value(red: u8, green: u8, blue: u8) -> Value {
        Value::Map(BTreeMap::from([
            ("red".to_string(), Value::Int(red as i64)),
            ("green".to_string(), Value::Int(green as i64)),
            ("blue".to_string(), Value::Int(blue as i64)),
            (
                "hex".to_string(),
                Value::Color(format!("{red:02x}{green:02x}{blue:02x}")),
            ),
        ]))
    }
}

#[async_trait]
impl CapabilityInvoker for NativeYamlHost {
    async fn invoke(&self, capability: &str, args: Value) -> Result<Value> {
        self.authorize(capability)?;
        let args = Self::args_map(args)?;
        match capability {
            "device.resolve" => Ok(Value::Handle {
                kind: "device".to_string(),
                id: 1,
            }),
            "app.start" | "device.start_app" => {
                self.registry
                    .device()
                    .ok_or_else(|| anyhow!("device capability 未注册"))?
                    .start_app(
                        &self.device,
                        &AppId::new(format!("+{}", self.package(&args)?)),
                    )
                    .await
                    .map_err(anyhow::Error::new)?;
                Ok(Value::Null)
            }
            "app.stop" | "device.stop_app" => {
                self.registry
                    .device()
                    .ok_or_else(|| anyhow!("device capability 未注册"))?
                    .stop_app(&self.device, &AppId::new(self.package(&args)?))
                    .await
                    .map_err(anyhow::Error::new)?;
                Ok(Value::Null)
            }
            "input.tap" => {
                let point =
                    self.point(Self::arg(&args, "point").or_else(|_| Self::arg(&args, "at"))?)?;
                self.registry
                    .input()
                    .ok_or_else(|| anyhow!("input capability 未注册"))?
                    .tap(&self.device, point)
                    .await
                    .map_err(anyhow::Error::new)?;
                Ok(Value::Null)
            }
            "input.swipe" => {
                let from = self.point(Self::arg(&args, "from")?)?;
                let to = self.point(Self::arg(&args, "to")?)?;
                let duration = Self::arg(&args, "duration")?
                    .duration_ms()
                    .ok_or_else(|| anyhow!("duration 必须是时间值"))?;
                self.registry
                    .input()
                    .ok_or_else(|| anyhow!("input capability 未注册"))?
                    .swipe(
                        &self.device,
                        SwipeGesture::new(from, to, Duration::from_millis(duration)),
                    )
                    .await
                    .map_err(anyhow::Error::new)?;
                Ok(Value::Null)
            }
            "input.key" => {
                let key = Self::arg(&args, "key")?;
                let code = key_code(key)?;
                let action = match args
                    .get("action")
                    .and_then(Value::as_string)
                    .unwrap_or("press")
                {
                    "down" => KeyAction::Down,
                    "up" => KeyAction::Up,
                    "press" => KeyAction::Press,
                    other => bail!("未知 key action: {other}"),
                };
                self.registry
                    .input()
                    .ok_or_else(|| anyhow!("input capability 未注册"))?
                    .key(&self.device, KeyInput::new(KeyCode::new(code), action))
                    .await
                    .map_err(anyhow::Error::new)?;
                Ok(Value::Null)
            }
            "input.text" => {
                let value = Self::arg(&args, "value")?
                    .as_string()
                    .ok_or_else(|| anyhow!("text value 必须是字符串"))?;
                self.registry
                    .input()
                    .ok_or_else(|| anyhow!("input capability 未注册"))?
                    .text(&self.device, TextInput::new(value))
                    .await
                    .map_err(anyhow::Error::new)?;
                Ok(Value::Null)
            }
            "runtime.sleep" => {
                let duration = Self::arg(&args, "duration")?
                    .duration_ms()
                    .ok_or_else(|| anyhow!("duration 必须是时间值"))?;
                self.runtime
                    .sleep(Duration::from_millis(duration.min(3_600_000)))
                    .await
                    .map_err(anyhow::Error::new)?;
                Ok(Value::Null)
            }
            "frame.capture" => Ok(Value::Handle {
                kind: "frame".to_string(),
                id: 1,
            }),
            "vision.match" | "vision.match_template" => {
                let frame = self.capture().await?;
                let template = self.template(Self::arg(&args, "template")?).await?;
                let outcome = self
                    .registry
                    .vision()
                    .ok_or_else(|| anyhow!("vision capability 未注册"))?
                    .match_template(frame, TemplateQuery::new(template, MatchOptions::default()))
                    .await
                    .map_err(anyhow::Error::new)?;
                Ok(Self::match_value(outcome))
            }
            "vision.match_many" => {
                let templates = match Self::arg(&args, "templates")? {
                    Value::List(values) => values,
                    _ => bail!("templates 必须是列表"),
                };
                let frame = self.capture().await?;
                let mut request = MatchManyRequest::new(frame);
                for template in templates {
                    let resource = self.template(template).await?;
                    request = request
                        .with_template(TemplateQuery::new(resource, MatchOptions::default()));
                }
                let results = self
                    .registry
                    .vision()
                    .ok_or_else(|| anyhow!("vision capability 未注册"))?
                    .match_many(&request)
                    .await
                    .map_err(anyhow::Error::new)?;
                let matches = results
                    .into_iter()
                    .map(|result| Self::match_value(result.outcome))
                    .collect::<Vec<_>>();
                let found = matches.iter().any(Value::truthy);
                Ok(Value::Map(BTreeMap::from([
                    ("found".to_string(), Value::Bool(found)),
                    ("matches".to_string(), Value::List(matches)),
                ])))
            }
            "vision.sample_color" => {
                let frame = self.capture().await?;
                let point = self
                    .frame_point(Self::arg(&args, "point").or_else(|_| Self::arg(&args, "at"))?)?;
                let color = self
                    .registry
                    .vision()
                    .ok_or_else(|| anyhow!("vision capability 未注册"))?
                    .sample_color(frame, point)
                    .await
                    .map_err(anyhow::Error::new)?;
                Ok(Self::color_value(color.red, color.green, color.blue))
            }
            "log.write" => {
                let level = match args
                    .get("level")
                    .and_then(Value::as_string)
                    .unwrap_or("info")
                {
                    "trace" => LogLevel::Trace,
                    "debug" => LogLevel::Debug,
                    "info" => LogLevel::Info,
                    "warn" | "warning" => LogLevel::Warn,
                    "error" => LogLevel::Error,
                    other => bail!("未知 log level: {other}"),
                };
                let message = Self::arg(&args, "message")?
                    .as_string()
                    .ok_or_else(|| anyhow!("log message 必须是字符串"))?;
                self.registry
                    .log()
                    .ok_or_else(|| anyhow!("log capability 未注册"))?
                    .write(LogRecord::new(level, message))
                    .map_err(anyhow::Error::new)?;
                Ok(Value::Null)
            }
            other => bail!("未知 capability: {other}"),
        }
    }

    fn cancelled(&self) -> bool {
        self.runtime.cancelled()
    }
}

fn key_code(value: &Value) -> Result<u32> {
    let value = value
        .as_string()
        .ok_or_else(|| anyhow!("key 必须是按键名字符串或数字字符串"))?;
    if let Ok(code) = value.parse::<u32>() {
        return Ok(code);
    }
    Ok(match value.to_ascii_uppercase().as_str() {
        "HOME" => 3,
        "BACK" => 4,
        "MENU" => 82,
        "APP_SWITCH" | "RECENTS" => 187,
        "VOL_UP" | "VOLUME_UP" => 24,
        "VOL_DOWN" | "VOLUME_DOWN" => 25,
        "ESC" | "ESCAPE" => 111,
        "ENTER" | "RETURN" => 66,
        "SPACE" => 62,
        "TAB" => 61,
        "BACKSPACE" | "DEL" => 67,
        other => bail!("不支持的 Android key: {other}"),
    })
}

/// Optional provider for `call`; a resolver can load a target from an app
/// package without changing the AST or the host capability contract.
#[async_trait]
pub(crate) trait ProgramResolver: Send + Sync {
    async fn resolve(&self, target: &str) -> Result<Program>;
}

#[derive(Debug)]
pub(crate) struct ExecutionResult {
    pub value: Value,
    pub logs: Vec<(String, String)>,
}

enum Flow {
    Continue,
    Break,
    Return(Value),
    Throw(String),
}

pub(crate) struct Interpreter {
    invoker: Arc<dyn CapabilityInvoker>,
    resolver: Option<Arc<dyn ProgramResolver>>,
    values: BTreeMap<String, Value>,
    logs: Vec<(String, String)>,
    steps: u64,
}

impl Interpreter {
    pub(crate) fn new(invoker: Arc<dyn CapabilityInvoker>) -> Self {
        Self {
            invoker,
            resolver: None,
            values: BTreeMap::new(),
            logs: Vec::new(),
            steps: 0,
        }
    }

    pub(crate) fn with_resolver(mut self, resolver: Arc<dyn ProgramResolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    pub(crate) fn with_values(mut self, values: BTreeMap<String, Value>) -> Self {
        self.values = values;
        self
    }

    pub(crate) async fn run(mut self, program: &Program) -> Result<ExecutionResult> {
        match self.run_steps(&program.steps).await? {
            Flow::Continue | Flow::Return(_) => Ok(ExecutionResult {
                value: match self.run_steps_value() {
                    Some(value) => value,
                    None => Value::Null,
                },
                logs: self.logs,
            }),
            Flow::Break => bail!("yaml.v3.runtime.break_outside_loop"),
            Flow::Throw(message) => bail!("{message}"),
        }
    }

    fn run_steps_value(&self) -> Option<Value> {
        self.values.get("__yaml_return").cloned()
    }

    #[async_recursion]
    async fn run_steps(&mut self, steps: &[SmallStep]) -> Result<Flow> {
        for step in steps {
            if self.invoker.cancelled() {
                bail!("运行已取消")
            }
            self.steps += 1;
            if self.steps > MAX_STEP_BUDGET {
                bail!("yaml.v3.runtime.step_budget_exceeded")
            }
            let flow = self.run_step(step).await?;
            if !matches!(flow, Flow::Continue) {
                return Ok(flow);
            }
        }
        Ok(Flow::Continue)
    }

    #[async_recursion]
    async fn run_step(&mut self, step: &SmallStep) -> Result<Flow> {
        match step {
            SmallStep::Invoke {
                capability,
                args,
                save,
            } => {
                let evaluated_args = self.eval_map(args)?;
                let value = self
                    .invoker
                    .invoke(capability, Value::Map(evaluated_args.clone()))
                    .await?;
                if let Some(save) = save {
                    self.values.insert(save.clone(), value);
                }
                if capability == "log.write" {
                    // The actual persistence is handled by the invoker. This
                    // local copy is only the generic RunExecutor result.
                    let level = evaluated_args
                        .get("level")
                        .and_then(Value::as_string)
                        .unwrap_or("info")
                        .to_string();
                    if let Some(message) = evaluated_args.get("message").and_then(Value::as_string)
                    {
                        self.logs.push((level, message.to_string()));
                    }
                }
                Ok(Flow::Continue)
            }
            SmallStep::If {
                cond,
                then_steps,
                else_steps,
            } => {
                if self.eval_condition(cond)? {
                    self.run_steps(then_steps).await
                } else {
                    self.run_steps(else_steps).await
                }
            }
            SmallStep::Loop { times, body } => {
                let count = times.as_ref().map(|value| self.eval(value)).transpose()?;
                let limit = count
                    .as_ref()
                    .and_then(Value::duration_ms)
                    .map(|ms| (ms / 100).max(1));
                let limit = limit.or_else(|| {
                    count.as_ref().and_then(|value| match value {
                        Value::Int(value) if *value >= 0 => Some(*value as u64),
                        _ => None,
                    })
                });
                let mut iteration = 0u64;
                loop {
                    if let Some(limit) = limit {
                        if iteration >= limit {
                            break;
                        }
                    }
                    iteration += 1;
                    match self.run_steps(body).await? {
                        Flow::Continue => {}
                        Flow::Break => break,
                        flow => return Ok(flow),
                    }
                }
                Ok(Flow::Continue)
            }
            SmallStep::Break => Ok(Flow::Break),
            SmallStep::Call { target, args, save } => {
                let resolver = self
                    .resolver
                    .clone()
                    .ok_or_else(|| anyhow!("call resolver 未配置"))?;
                let program = resolver.resolve(target).await?;
                let mut child =
                    Interpreter::new(self.invoker.clone()).with_values(self.eval_map(args)?);
                if let Some(resolver) = self.resolver.clone() {
                    child = child.with_resolver(resolver);
                }
                match child.run_steps(&program.steps).await? {
                    Flow::Return(value) => {
                        if let Some(save) = save {
                            self.values.insert(save.clone(), value);
                        }
                        Ok(Flow::Continue)
                    }
                    Flow::Continue => {
                        if let Some(save) = save {
                            self.values.insert(save.clone(), Value::Null);
                        }
                        Ok(Flow::Continue)
                    }
                    Flow::Break => bail!("call 返回了 loop break"),
                    Flow::Throw(message) => Ok(Flow::Throw(message)),
                }
            }
            SmallStep::Return { value } => {
                let value = self.eval(value)?;
                self.values
                    .insert("__yaml_return".to_string(), value.clone());
                Ok(Flow::Return(value))
            }
            SmallStep::Throw { message } => Ok(Flow::Throw(
                self.eval(message)?
                    .as_string()
                    .unwrap_or("脚本 throw")
                    .to_string(),
            )),
            SmallStep::Set { name, value } => {
                self.values.insert(name.clone(), self.eval(value)?);
                Ok(Flow::Continue)
            }
        }
    }

    fn eval_map(&self, values: &BTreeMap<String, Expr>) -> Result<BTreeMap<String, Value>> {
        values
            .iter()
            .map(|(key, value)| Ok((key.clone(), self.eval(value)?)))
            .collect()
    }

    fn eval(&self, expr: &Expr) -> Result<Value> {
        match expr {
            Expr::Literal(value) => Ok(value.clone()),
            Expr::Ref(name) => {
                lookup_path(&self.values, name).ok_or_else(|| anyhow!("未定义变量 ${name}"))
            }
            Expr::List(values) => Ok(Value::List(
                values
                    .iter()
                    .map(|value| self.eval(value))
                    .collect::<Result<_, _>>()?,
            )),
            Expr::Map(values) => Ok(Value::Map(self.eval_map(values)?)),
        }
    }

    fn eval_condition(&self, condition: &Condition) -> Result<bool> {
        Ok(match condition {
            Condition::Truthy { value } => self.eval(value)?.truthy(),
            Condition::Equals { left, right } => {
                values_equal(&self.eval(left)?, &self.eval(right)?)
            }
            Condition::Not { value } => !self.eval_condition(value)?,
        })
    }
}

fn lookup_path(values: &BTreeMap<String, Value>, path: &str) -> Option<Value> {
    let mut segments = path.split('.');
    let mut current = values.get(segments.next()?)?.clone();
    for segment in segments {
        let (name, indices) = parse_segment(segment);
        if !name.is_empty() {
            current = match current {
                Value::Map(map) => map.get(name).cloned()?,
                _ => return None,
            };
        }
        for index in indices {
            current = match current {
                Value::List(list) => list.get(index).cloned()?,
                _ => return None,
            };
        }
    }
    Some(current)
}

fn parse_segment(segment: &str) -> (&str, Vec<usize>) {
    let name = segment.split('[').next().unwrap_or(segment);
    let mut indices = Vec::new();
    let mut rest = segment.strip_prefix(name).unwrap_or_default();
    while let Some(value) = rest.strip_prefix('[') {
        let Some(end) = value.find(']') else { break };
        if let Ok(index) = value[..end].parse() {
            indices.push(index);
        }
        rest = &value[end + 1..];
    }
    (name, indices)
}

fn values_equal(left: &Value, right: &Value) -> bool {
    if left == right {
        return true;
    }
    match (left, right) {
        (Value::Color(left), Value::Map(right)) | (Value::Map(right), Value::Color(left)) => right
            .get("hex")
            .and_then(Value::as_string)
            .is_some_and(|value| value.eq_ignore_ascii_case(left.trim_start_matches('#'))),
        (Value::Int(left), Value::Float(right)) | (Value::Float(right), Value::Int(left)) => {
            (*left as f64) == *right
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{DeviceService, InputService};
    use crate::yaml_vnext::load;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;

    #[derive(Default)]
    struct Trace {
        calls: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl DeviceService for Trace {
        async fn resolve(
            &self,
            id: &DeviceId,
        ) -> crate::capabilities::CapabilityResult<DeviceHandle> {
            Ok(DeviceHandle::new(id.clone()))
        }
        async fn start_app(
            &self,
            _: &DeviceHandle,
            app: &AppId,
        ) -> crate::capabilities::CapabilityResult<()> {
            self.calls.lock().unwrap().push(format!("start:{app:?}"));
            Ok(())
        }
        async fn stop_app(
            &self,
            _: &DeviceHandle,
            app: &AppId,
        ) -> crate::capabilities::CapabilityResult<()> {
            self.calls.lock().unwrap().push(format!("stop:{app:?}"));
            Ok(())
        }
    }

    #[async_trait]
    impl InputService for Trace {
        async fn tap(
            &self,
            _: &DeviceHandle,
            point: TouchPoint,
        ) -> crate::capabilities::CapabilityResult<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("tap:{}:{}", point.x(), point.y()));
            Ok(())
        }
        async fn swipe(
            &self,
            _: &DeviceHandle,
            _: SwipeGesture,
        ) -> crate::capabilities::CapabilityResult<()> {
            self.calls.lock().unwrap().push("swipe".into());
            Ok(())
        }
        async fn key(
            &self,
            _: &DeviceHandle,
            _: KeyInput,
        ) -> crate::capabilities::CapabilityResult<()> {
            self.calls.lock().unwrap().push("key".into());
            Ok(())
        }
        async fn text(
            &self,
            _: &DeviceHandle,
            input: TextInput,
        ) -> crate::capabilities::CapabilityResult<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("text:{}", input.as_str()));
            Ok(())
        }
    }

    struct FakeInvoker;
    #[async_trait]
    impl CapabilityInvoker for FakeInvoker {
        async fn invoke(&self, capability: &str, args: Value) -> Result<Value> {
            if capability == "vision.match" {
                return Ok(Value::Map(BTreeMap::from([
                    ("found".into(), Value::Bool(true)),
                    ("center".into(), Value::Coordinate([0.5, 0.5])),
                ])));
            }
            Ok(args)
        }
    }

    fn all_permissions(registry: CapabilityRegistry) -> HostApi {
        let manifest = crate::extensions::parse_manifest(br#"manifest_version = 1
id = "gamer.yaml"
version = "3.0.0"
name = "YAML vNext"
entry = "plugin.wasm"
permissions = ["device.read", "device.app", "input.tap", "input.swipe", "input.key", "input.text", "vision.match", "vision.color", "resource.read", "runtime.sleep", "log.write"]
"#).unwrap();
        HostApi::for_manifest(
            registry,
            crate::extensions::HostApiCatalog::default(),
            &manifest,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn interpreter_executes_control_flow_and_general_return_values() {
        let program = load("version: 3\nsteps:\n  - set: {ready: true}\n  - if:\n      cond: $ready\n      then:\n        - call:\n            target: missing\n            save: answer\n      else: []\n").unwrap();
        // The call is intentionally not entered in this test; a missing
        // resolver is a useful guard that proves the AST does not silently
        // execute arbitrary host code.
        let error = Interpreter::new(Arc::new(FakeInvoker))
            .run(&program)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("resolver"));
    }

    #[tokio::test]
    async fn native_host_routes_primitive_actions_to_capability_registry() {
        let trace = Arc::new(Trace::default());
        let registry = CapabilityRegistry::builder()
            .with_device_service(trace.clone() as Arc<dyn DeviceService>)
            .with_input_service(trace.clone() as Arc<dyn InputService>)
            .build();
        let host = NativeYamlHost::new(
            all_permissions(registry),
            AppContext::from_legacy_package("d1", "com.test.game").unwrap(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
        host.invoke(
            "input.tap",
            Value::Map(BTreeMap::from([(
                "point".into(),
                Value::Coordinate([0.5, 0.25]),
            )])),
        )
        .await
        .unwrap();
        host.invoke(
            "input.text",
            Value::Map(BTreeMap::from([(
                "value".into(),
                Value::String("hello".into()),
            )])),
        )
        .await
        .unwrap();
        assert_eq!(
            trace.calls.lock().unwrap().as_slice(),
            ["tap:500:250", "text:hello"]
        );
    }

    #[test]
    fn color_record_compares_with_color_literal() {
        let left = NativeYamlHost::color_value(255, 0, 0);
        assert!(values_equal(&left, &Value::Color("ff0000".into())));
    }

    #[test]
    fn path_lookup_supports_match_many_indexed_results() {
        let values = BTreeMap::from([(
            "result".into(),
            Value::Map(BTreeMap::from([(
                "matches".into(),
                Value::List(vec![
                    Value::Map(BTreeMap::from([("found".into(), Value::Bool(false))])),
                    Value::Map(BTreeMap::from([("found".into(), Value::Bool(true))])),
                ]),
            )])),
        )]);
        assert_eq!(
            lookup_path(&values, "result.matches[1].found"),
            Some(Value::Bool(true))
        );
    }

    #[test]
    fn compatibility_adapter_keeps_v2_and_v3_on_separate_loaders() {
        let temp = TempDir::new().unwrap();
        let mut config = crate::config::Config::default();
        config.data_dir = temp.path().to_path_buf();
        let scripts = crate::scripts::ScriptStore::open(&config).unwrap();

        assert!(matches!(
            validate_compatible_script(&scripts, "com.test", "old.yaml", "steps: []\n"),
            Ok(CompatibleYamlSource::V2)
        ));
        assert!(matches!(
            validate_compatible_script(&scripts, "com.test", "new.yaml", "version: 3\nsteps: []\n"),
            Ok(CompatibleYamlSource::V3(_))
        ));
    }

    #[tokio::test]
    async fn yaml_manifest_panels_are_removed_after_uninstall() {
        let temp = TempDir::new().unwrap();
        let service = crate::extensions::ExtensionService::with_default_runtime(
            crate::extensions::ExtensionStore::new(temp.path()),
            CapabilityRegistry::default(),
        );
        let mut archive = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut archive));
            let options = SimpleFileOptions::default();
            writer.start_file("manifest.toml", options).unwrap();
            writer
                .write_all(YAML_EXTENSION_MANIFEST_TOML.as_bytes())
                .unwrap();
            writer.start_file("plugin.wasm", options).unwrap();
            writer.write_all(b"\0asm\x01\0\0\0").unwrap();
            for entry in ["ui/automation.html", "ui/functions.html"] {
                writer.start_file(entry, options).unwrap();
                writer.write_all(b"<!doctype html>").unwrap();
            }
            writer.finish().unwrap();
        }
        let installed = service.install(&archive).await.unwrap();
        service.enable(installed.id()).await.unwrap();
        let panels = service.ui_contributions().unwrap();
        assert_eq!(panels.len(), 2);
        assert!(panels.iter().any(|panel| panel.panel_id == "automation"));
        assert!(panels.iter().any(|panel| panel.panel_id == "functions"));
        service.disable(installed.id()).await.unwrap();
        assert!(service
            .uninstall(installed.id(), installed.active_version())
            .await
            .unwrap());
        assert!(service.ui_contributions().unwrap().is_empty());
    }
}

#[cfg(all(test, feature = "wasm-runtime"))]
mod wasm_tests {
    use super::*;
    use crate::capabilities::{CapabilityRegistry, CapabilityResult};
    use crate::extensions::{HostApiCatalog, LazyYamlWasmtimeRuntime};
    use crate::yaml_vnext::load;
    use async_trait::async_trait;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::sync::{Arc, Mutex, OnceLock};

    #[derive(Default)]
    struct Trace {
        text: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl crate::capabilities::DeviceService for Trace {
        async fn resolve(&self, id: &DeviceId) -> CapabilityResult<DeviceHandle> {
            Ok(DeviceHandle::new(id.clone()))
        }

        async fn start_app(&self, _: &DeviceHandle, _: &AppId) -> CapabilityResult<()> {
            Ok(())
        }

        async fn stop_app(&self, _: &DeviceHandle, _: &AppId) -> CapabilityResult<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl crate::capabilities::InputService for Trace {
        async fn tap(&self, _: &DeviceHandle, _: TouchPoint) -> CapabilityResult<()> {
            Ok(())
        }

        async fn swipe(&self, _: &DeviceHandle, _: SwipeGesture) -> CapabilityResult<()> {
            Ok(())
        }

        async fn key(&self, _: &DeviceHandle, _: KeyInput) -> CapabilityResult<()> {
            Ok(())
        }

        async fn text(&self, _: &DeviceHandle, value: TextInput) -> CapabilityResult<()> {
            self.text.lock().unwrap().push(value.as_str().to_string());
            Ok(())
        }
    }

    struct FixtureResolver;

    impl YamlProgramResolver for FixtureResolver {
        fn resolve(&self, target: &str, _args: &BTreeMap<String, Value>) -> Result<Program> {
            if target != "helper" {
                bail!("unknown fixture target: {target}");
            }
            load("version: 3\nsteps:\n  - return: from-call\n")
                .map_err(|diagnostics| anyhow!("fixture resolver: {diagnostics:?}"))
        }
    }

    fn fixture_guest_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("yaml-guest")
    }

    fn fixture_target_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("yaml-guest")
    }

    fn fixture_module_path() -> PathBuf {
        fixture_target_dir()
            .join("wasm32-unknown-unknown")
            .join("release")
            .join("gamer_yaml_fixture.wasm")
    }

    fn run_fixture_cargo(args: &[String]) -> Output {
        let (subcommand, rest) = args
            .split_first()
            .expect("yaml guest cargo 子进程缺少 subcommand");
        let guest_dir = fixture_guest_dir();
        let target_dir = fixture_target_dir();
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut command = Command::new(cargo);
        command
            .current_dir(&guest_dir)
            .arg(subcommand)
            .arg("--manifest-path")
            .arg(guest_dir.join("Cargo.toml"))
            // Do not inherit the server's target directory. In particular,
            // CARGO_TARGET_DIR is often set by CI and must not redirect the
            // nested wasm build into the host test's files.
            .arg("--target-dir")
            .arg(&target_dir);
        for arg in rest {
            command.arg(arg);
        }
        command.output().unwrap_or_else(|error| {
            panic!(
                "无法启动 yaml guest cargo 子进程: {error}; guest_dir={}; target_dir={}",
                guest_dir.display(),
                target_dir.display()
            )
        })
    }

    fn assert_fixture_command(output: Output, stage: &str) {
        if output.status.success() {
            return;
        }
        panic!(
            "yaml guest {stage} 失败: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn fixture_module() -> Vec<u8> {
        static MODULE: OnceLock<Vec<u8>> = OnceLock::new();
        MODULE
            .get_or_init(|| {
                let output = run_fixture_cargo(&[
                    "build".into(),
                    "--locked".into(),
                    "--quiet".into(),
                    "--release".into(),
                    "--lib".into(),
                    "--target".into(),
                    "wasm32-unknown-unknown".into(),
                ]);
                assert_fixture_command(output, "guest wasm 构建");
                let path = fixture_module_path();
                fs::read(&path).unwrap_or_else(|error| {
                    panic!("yaml guest wasm 不存在: {}: {error}", path.display())
                })
            })
            .clone()
    }

    fn componentize_fixture(output_path: &Path) -> Vec<u8> {
        let module_path = fixture_module_path();
        let output = run_fixture_cargo(&[
            "run".into(),
            "--locked".into(),
            "--quiet".into(),
            "--release".into(),
            "--bin".into(),
            "componentize".into(),
            "--".into(),
            module_path.to_string_lossy().into_owned(),
            output_path.to_string_lossy().into_owned(),
        ]);
        assert_fixture_command(output, "WIT Component 封装");
        fs::read(output_path).unwrap_or_else(|error| {
            panic!(
                "yaml guest Component 输出不存在: {}: {error}",
                output_path.display()
            )
        })
    }

    fn fixture_component() -> Vec<u8> {
        static COMPONENT: OnceLock<Vec<u8>> = OnceLock::new();
        COMPONENT
            .get_or_init(|| {
                fixture_module();
                let temp = tempfile::tempdir().expect("无法创建 YAML Component 临时目录");
                componentize_fixture(&temp.path().join("yaml-guest.component.wasm"))
            })
            .clone()
    }

    #[test]
    fn yaml_guest_fixture_builds_wasm_module() {
        let module = fixture_module();
        assert!(module.len() >= 8, "guest wasm 太短");
        assert_eq!(&module[..4], b"\0asm");
        assert_eq!(&module[4..8], [1, 0, 0, 0]);
    }

    #[test]
    fn yaml_guest_fixture_componentizes_with_checked_in_wit() {
        let component = fixture_component();
        assert!(component.len() >= 8, "YAML Component 太短");
        assert_eq!(&component[..4], b"\0asm");
        assert_eq!(&component[4..8], [13, 0, 1, 0]);
    }

    #[test]
    fn yaml_componentizer_releases_output_file_before_returning() {
        fixture_module();
        let temp = tempfile::tempdir().expect("无法创建 YAML Component 生命周期临时目录");
        let output = temp.path().join("yaml-guest.component.wasm");
        componentize_fixture(&output);
        let moved = temp.path().join("yaml-guest.component.moved.wasm");
        fs::rename(&output, &moved).expect("Componentizer 返回后输出文件仍被占用");
        fs::remove_file(&moved).expect("无法删除已关闭的 Component 输出文件");
    }

    fn host_with_permissions(trace: Arc<Trace>, permissions: &[&str]) -> HostApi {
        let permissions = permissions
            .iter()
            .map(|permission| format!("\"{permission}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let manifest = crate::extensions::parse_manifest(
            format!(
                r#"manifest_version = 1
id = "gamer.yaml"
version = "3.0.0"
name = "YAML vNext"
entry = "plugin.wasm"
permissions = [{permissions}]
[host_api]
device = "^1.0"
input = "^1.0"
runtime = "^1.0"
"#
            )
            .as_bytes(),
        )
        .unwrap();
        HostApi::for_manifest(
            CapabilityRegistry::builder()
                .with_device_service(trace.clone() as Arc<dyn crate::capabilities::DeviceService>)
                .with_input_service(trace as Arc<dyn crate::capabilities::InputService>)
                .build(),
            HostApiCatalog::default(),
            &manifest,
        )
        .unwrap()
    }

    fn host(trace: Arc<Trace>) -> HostApi {
        host_with_permissions(trace, &["device.read", "input.text"])
    }

    #[tokio::test(flavor = "current_thread")]
    async fn real_yaml_component_invokes_wit_and_native_capability() {
        let trace = Arc::new(Trace::default());
        let runtime = LazyYamlWasmtimeRuntime::new();
        let program = load(
            "version: 3\nsteps:\n  - call:\n      target: helper\n      save: answer\n  - text: from-real-wasm\n  - return: $answer\n",
        )
        .unwrap();
        let result = runtime
            .run(YamlWasmRunRequest {
                wasm: fixture_component(),
                program,
                args: BTreeMap::new(),
                resolver: Some(Arc::new(FixtureResolver)),
                host: host(trace.clone()),
                context: AppContext::from_legacy_package("device-1", "com.example.game").unwrap(),
                stop: Arc::new(AtomicBool::new(false)),
            })
            .await
            .unwrap();
        assert_eq!(result.value, Value::String("from-call".into()));
        assert_eq!(trace.text.lock().unwrap().as_slice(), ["from-real-wasm"]);
        assert!(runtime.is_available());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_component_sync_wit_call_is_safe_on_multithread_tokio() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let program = load(
            "version: 3\nsteps:\n  - invoke:\n      capability: device.resolve\n      with:\n        id: device-1\n  - return: from-multithread\n",
        )
        .unwrap();
        let result = runtime
            .run(YamlWasmRunRequest {
                wasm: fixture_component(),
                program,
                args: BTreeMap::new(),
                resolver: None,
                host: host_with_permissions(Arc::new(Trace::default()), &["device.read"]),
                context: AppContext::from_legacy_package("device-1", "com.example.game").unwrap(),
                stop: Arc::new(AtomicBool::new(false)),
            })
            .await
            .unwrap();
        assert_eq!(result.value, Value::String("from-multithread".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn yaml_component_preserves_permission_and_cancellation_kinds() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let denied_program = load("version: 3\nsteps:\n  - text: denied\n").unwrap();
        let denied = runtime
            .run(YamlWasmRunRequest {
                wasm: fixture_component(),
                program: denied_program,
                args: BTreeMap::new(),
                resolver: None,
                host: host_with_permissions(Arc::new(Trace::default()), &["device.read"]),
                context: AppContext::from_legacy_package("device-1", "com.example.game").unwrap(),
                stop: Arc::new(AtomicBool::new(false)),
            })
            .await
            .unwrap_err();
        assert!(
            denied.to_string().contains("kind=denied"),
            "permission denial lost its WIT kind: {denied:#}"
        );

        let cancelled_program = load(
        "version: 3\nsteps:\n  - invoke:\n      capability: runtime.sleep\n      with:\n        duration: 1000\n",
        )
        .unwrap();
        let cancelled = runtime
            .run(YamlWasmRunRequest {
                wasm: fixture_component(),
                program: cancelled_program,
                args: BTreeMap::new(),
                resolver: None,
                host: host_with_permissions(
                    Arc::new(Trace::default()),
                    &["device.read", "runtime.sleep"],
                ),
                context: AppContext::from_legacy_package("device-1", "com.example.game").unwrap(),
                stop: Arc::new(AtomicBool::new(true)),
            })
            .await
            .unwrap_err();
        assert!(
            cancelled.to_string().contains("kind=cancelled"),
            "cancellation lost its WIT kind: {cancelled:#}"
        );
    }
}
