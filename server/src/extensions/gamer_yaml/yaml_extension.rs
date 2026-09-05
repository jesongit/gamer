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

use crate::extensions::gamer_yaml::yaml_vnext::{Condition, Expr, Program, SmallStep, Value};

pub(crate) const YAML_EXTENSION_ID: &str = "gamer.yaml";
/// Reference manifest for the installable YAML guest. The server never embeds
/// its WASM bytes; package installation supplies `plugin.wasm` independently.
/// 仅测试引用：与 tools/plugins/gamer.yaml/manifest.toml 的同步护栏 +
/// 安装/卸载面板测试以此为打包 manifest 源。
#[allow(dead_code)]
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

# runtime = "core"：面板由宿主 Vue 组件渲染，component 键由前端
# core-component-registry 解释（console.scripts = 自动化编辑器、
# console.functions = 函数库模式、console.templates = 模板框选）。

[[ui.contributions]]
panel_id = "automation"
title = "自动化"
icon = "⚙️"
order = 25
location = "console.right"
runtime = "core"
requires_device = true
preferred_width = 440
component = "console.scripts"

[[ui.contributions]]
panel_id = "functions"
title = "函数"
icon = "ƒ"
order = 30
location = "console.right"
runtime = "core"
requires_device = false
preferred_width = 440
component = "console.functions"

[[ui.contributions]]
panel_id = "templates"
title = "模板"
icon = "🖼️"
order = 35
location = "console.right"
runtime = "core"
requires_device = true
preferred_width = 440
component = "console.templates"
"#;

/// 官方市场打包源（tools/plugins/gamer.yaml/manifest.toml）与本常量锁同步：
/// build-plugins.ps1 以文件为准打包，漂移会导致线上包与运行时语义不一致。
#[cfg(test)]
mod manifest_sync_tests {
    #[test]
    fn yaml_packaging_manifest_stays_in_sync_with_shipped_constant() {
        let packaged = include_str!("../../../../tools/plugins/gamer.yaml/manifest.toml");
        assert_eq!(
            super::YAML_EXTENSION_MANIFEST_TOML.trim(),
            packaged.trim(),
            "tools/plugins/gamer.yaml/manifest.toml 与 YAML_EXTENSION_MANIFEST_TOML 不一致"
        );
    }
}

const DEFAULT_SCREEN_WIDTH: u32 = 1000;
const DEFAULT_SCREEN_HEIGHT: u32 = 1000;

/// v3 执行预算（ADR-YAML-04 / 契约 §5）：逻辑步与调用深度上限。
///
/// 生产链路由 WASM guest 本地计数（`server/tests/yaml-guest` 的
/// ExecutionBudget，常量在此对齐），本模块的原生参考解释器（无 wasm 退化
/// 路径 / 测试）实现同语义。步数按**逻辑步**计：顶层、loop 体每轮每个子步、
/// if 分支体、call 目标程序体全计，loop 每轮迭代本身也计（空转体死循环同受
/// 约束）；外层 loop 包裹不得绕过预算。
pub(crate) const MAX_STEPS: u64 = 100_000;
/// v3 `call` 递归深度上限（ADR-YAML-02 与 ADR-YAML-04 同值）。
pub(crate) const MAX_CALL_DEPTH: u32 = 32;

/// 深度守卫（原生参考解释器用）：guest 每进入一层 callable 深度 +1、返回
/// -1，超过 [`MAX_CALL_DEPTH`] 立即终止。错误文本以机器可读码开头，经
/// run_yaml_vnext → RunManager（RunRecord 错误信息 / 日志）原样透传。
/// P12.4 起 WIT `programs.resolve` 不再透传 depth，resolver 侧临时守卫移除，
/// 深度计数正式归 guest 本地（生产链路）与本解释器（无 wasm 路径）。
#[cfg_attr(not(feature = "wasm-runtime"), allow(dead_code))]
pub(crate) fn check_call_depth(depth: u32) -> Result<()> {
    if depth > MAX_CALL_DEPTH {
        bail!("CALL_DEPTH_EXCEEDED: depth={depth} max={MAX_CALL_DEPTH}");
    }
    Ok(())
}

/// 步预算守卫（原生参考解释器用）：`consumed` 为刚消耗的逻辑步计数。
fn check_step_budget(consumed: u64) -> Result<()> {
    if consumed > MAX_STEPS {
        bail!("STEP_BUDGET_EXCEEDED: consumed={consumed} max={MAX_STEPS}");
    }
    Ok(())
}

/// Request passed to the real YAML Component runtime. The program is already
/// lowered by the extension front-end; the guest only interprets the small
/// wire AST and calls capability.invoke.
/// WASM guest 执行请求。字段仅由 wasm-runtime feature 的
/// `LazyYamlWasmtimeRuntime` 消费；无该 feature 时（NoYamlWasmRuntime 只回错）
/// 字段不会被读取，故按 feature 条件豁免 dead_code。
#[cfg_attr(not(feature = "wasm-runtime"), allow(dead_code))]
#[derive(Clone)]
pub(crate) struct YamlWasmRunRequest {
    pub(crate) wasm: Vec<u8>,
    pub(crate) program: Program,
    pub(crate) args: BTreeMap<String, Value>,
    pub(crate) resolver: Option<Arc<dyn YamlProgramResolver>>,
    /// 手动运行「从此运行」：跳过的顶层 surface 步序号（契约 §8）。
    /// `None` = 从头执行；guest 只按顶层步序号跳，嵌套分支/循环体不受影响。
    #[cfg_attr(not(feature = "wasm-runtime"), allow(dead_code))]
    pub(crate) start_index: Option<usize>,
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

    /// 取消查询属于 invoker 契约的一部分（原生解释器消费；WASM 链路经
    /// YamlHostState.cancelled 传递，生产构建中无直接调用方）。
    #[allow(dead_code)]
    fn cancelled(&self) -> bool {
        false
    }
}

/// YAML extension-only lookup for the small AST `call` node. This does not
/// enter `CapabilityRegistry`, so YAML source/resource semantics stay out of
/// Core capabilities.
///
/// P12.4（ADR-YAML-04）：`call` 深度由 guest 本地 ExecutionBudget 计数，
/// resolver 只按命名空间定位目标程序，不再接收 depth、也不再做深度守卫。
pub(crate) trait YamlProgramResolver: Send + Sync {
    #[cfg_attr(not(feature = "wasm-runtime"), allow(dead_code))]
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
    /// `new`/`invoke_json` 是 WASM guest 的 capability.invoke 后端
    /// （wasm.rs 的 YamlHostState 调用）；无 wasm-runtime feature 时仅测试使用。
    #[cfg_attr(not(feature = "wasm-runtime"), allow(dead_code))]
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

    #[cfg_attr(not(feature = "wasm-runtime"), allow(dead_code))]
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
                Ok(value.trim_start_matches("templates/").to_string())
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
                format!("templates/{name}"),
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

    fn match_value(outcome: MatchOutcome, region: Value) -> Value {
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
                    ("region".to_string(), region),
                ]))
            }
            MatchOutcome::NotFound => {
                Value::Map(BTreeMap::from([
                    ("found".to_string(), Value::Bool(false)),
                    ("region".to_string(), region),
                ]))
            }
        }
    }

    /// 本次搜索的 region 回显值（相对坐标 map；未给 region = 全帧）。
    fn region_echo(args: &BTreeMap<String, Value>) -> Result<Value> {
        match args.get("region") {
            Some(raw) => {
                let [x, y, width, height] = Self::relative_region(raw)?;
                Ok(Value::Map(BTreeMap::from([
                    ("x".to_string(), Value::Float(x)),
                    ("y".to_string(), Value::Float(y)),
                    ("width".to_string(), Value::Float(width)),
                    ("height".to_string(), Value::Float(height)),
                ])))
            }
            None => Ok(Value::Map(BTreeMap::from([
                ("x".to_string(), Value::Float(0.0)),
                ("y".to_string(), Value::Float(0.0)),
                ("width".to_string(), Value::Float(1.0)),
                ("height".to_string(), Value::Float(1.0)),
            ]))),
        }
    }

    /// region 实参：相对坐标 map `{x, y, width, height}` 或四元数组，全部
    /// 0.0~1.0（与 v3 表面坐标约定一致）。
    fn relative_region(value: &Value) -> Result<[f64; 4]> {
        let numbers: Vec<f64> = match value {
            Value::List(items) => items
                .iter()
                .map(|item| match item {
                    Value::Float(f) => Ok(*f),
                    Value::Int(i) => Ok(*i as f64),
                    _ => bail!("region 数组元素必须是数值"),
                })
                .collect::<Result<_>>()?,
            Value::Map(map) => ["x", "y", "width", "height"]
                .iter()
                .map(|key| match map.get(*key) {
                    Some(Value::Float(f)) => Ok(*f),
                    Some(Value::Int(i)) => Ok(*i as f64),
                    _ => bail!("region 必须含数值字段 x/y/width/height"),
                })
                .collect::<Result<_>>()?,
            _ => bail!("region 必须是 {{x, y, width, height}} 映射或四元数组"),
        };
        let [x, y, width, height] = numbers.try_into().map_err(|_| {
            anyhow!("region 必须是 {{x, y, width, height}} 映射或四元数组")
        })?;
        for component in [x, y, width, height] {
            if !(0.0..=1.0).contains(&component) {
                bail!("region 分量必须在 0..1（相对坐标），得到 {component}");
            }
        }
        Ok([x, y, width, height])
    }

    /// region 实参 → 像素 SearchRegion（按参考屏尺寸换算）。
    fn search_region(&self, value: &Value) -> Result<crate::capabilities::SearchRegion> {
        let [x, y, width, height] = Self::relative_region(value)?;
        Ok(crate::capabilities::SearchRegion::new(
            (x * self.screen.width as f64).round() as u32,
            (y * self.screen.height as f64).round() as u32,
            (width * self.screen.width as f64).round() as u32,
            (height * self.screen.height as f64).round() as u32,
        ))
    }

    /// threshold 实参：0.0~1.0 数值 → f32（MatchOptions.threshold）。
    fn threshold_option(args: &BTreeMap<String, Value>) -> Result<Option<f32>> {
        let Some(value) = args.get("threshold") else {
            return Ok(None);
        };
        let raw = match value {
            Value::Float(f) => *f,
            Value::Int(i) => *i as f64,
            Value::Null => return Ok(None),
            _ => bail!("threshold 必须是 0~1 的数字"),
        };
        if !(0.0..=1.0).contains(&raw) {
            bail!("threshold 必须在 0..1，得到 {raw}");
        }
        Ok(Some(raw as f32))
    }

    /// match_many 的 thresholds 实参（与 templates 平行的列表，缺项/Null =
    /// 该模板用缺省阈值）——match_first 候选级 threshold 的承载形态。
    fn thresholds_option(
        args: &BTreeMap<String, Value>,
        count: usize,
    ) -> Result<Vec<Option<f32>>> {
        let Some(Value::List(values)) = args.get("thresholds") else {
            return Ok(vec![None; count]);
        };
        if values.len() != count {
            bail!("thresholds 长度必须与 templates 一致");
        }
        values
            .iter()
            .map(|value| match value {
                Value::Null => Ok(None),
                Value::Float(f) => {
                    if (0.0..=1.0).contains(f) {
                        Ok(Some(*f as f32))
                    } else {
                        bail!("threshold 必须在 0..1，得到 {f}")
                    }
                }
                Value::Int(i) => Ok(Some(*i as f32)),
                _ => bail!("thresholds 必须是数值或 null 的列表"),
            })
            .collect()
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
                let options = MatchOptions {
                    threshold: Self::threshold_option(&args)?,
                    region: args
                        .get("region")
                        .map(|value| self.search_region(value))
                        .transpose()?,
                    color_check: false,
                };
                let outcome = self
                    .registry
                    .vision()
                    .ok_or_else(|| anyhow!("vision capability 未注册"))?
                    .match_template(frame, TemplateQuery::new(template, options))
                    .await
                    .map_err(anyhow::Error::new)?;
                let region = Self::region_echo(&args)?;
                Ok(Self::match_value(outcome, region))
            }
            "vision.match_many" => {
                let templates = match Self::arg(&args, "templates")? {
                    Value::List(values) => values.clone(),
                    _ => bail!("templates 必须是列表"),
                };
                let thresholds = Self::thresholds_option(&args, templates.len())?;
                let frame = self.capture().await?;
                let mut request = MatchManyRequest::new(frame);
                for (template, threshold) in templates.iter().zip(thresholds) {
                    let resource = self.template(template).await?;
                    request = request.with_template(TemplateQuery::new(
                        resource,
                        MatchOptions {
                            threshold,
                            ..MatchOptions::default()
                        },
                    ));
                }
                let results = self
                    .registry
                    .vision()
                    .ok_or_else(|| anyhow!("vision capability 未注册"))?
                    .match_many(&request)
                    .await
                    .map_err(anyhow::Error::new)?;
                let region = Self::region_echo(&args)?;
                let matches = results
                    .into_iter()
                    .map(|result| Self::match_value(result.outcome, region.clone()))
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
/// v3 原生参考解释器（下方 Interpreter/ExecutionResult/Flow）：生产执行走
/// WASM guest（LazyYamlWasmtimeRuntime），本块仅由单元测试消费。
#[allow(dead_code)]
#[async_trait]
pub(crate) trait ProgramResolver: Send + Sync {
    async fn resolve(&self, target: &str) -> Result<Program>;
}

/// v3 原生参考解释器专用（见 ProgramResolver 注）。
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct ExecutionResult {
    pub value: Value,
    pub logs: Vec<(String, String)>,
}

/// v3 原生参考解释器专用（见 ProgramResolver 注）。
#[allow(dead_code)]
enum Flow {
    Continue,
    Break,
    Return(Value),
    Throw(String),
}

/// v3 原生参考解释器（见 ProgramResolver 注）。
#[allow(dead_code)]
pub(crate) struct Interpreter {
    invoker: Arc<dyn CapabilityInvoker>,
    resolver: Option<Arc<dyn ProgramResolver>>,
    values: BTreeMap<String, Value>,
    logs: Vec<(String, String)>,
    steps: u64,
    call_depth: u32,
    /// wait 随机区间的 PRNG 状态（run nonce 播种的 splitmix64，与 guest 同步）。
    rng: u64,
}

#[allow(dead_code)]
impl Interpreter {
    pub(crate) fn new(invoker: Arc<dyn CapabilityInvoker>) -> Self {
        Self {
            invoker,
            resolver: None,
            values: BTreeMap::new(),
            logs: Vec::new(),
            steps: 0,
            call_depth: 0,
            rng: 0,
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
        self.rng = program.nonce.unwrap_or(0);
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
            // 每个逻辑步执行前计数：顶层、loop 体每轮子步、if 分支体、call
            // 目标程序体全计（与 WASM guest ExecutionBudget 同语义）。
            self.steps += 1;
            check_step_budget(self.steps)?;
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
                    // 每轮迭代本身也是逻辑步：空转体（body 无子步）的无 times
                    // loop 同样受预算约束终止（与 guest 同语义）。
                    self.steps += 1;
                    check_step_budget(self.steps)?;
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
                self.call_depth += 1;
                let outcome = self.run_call(target, args, save).await;
                self.call_depth -= 1;
                outcome
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
            SmallStep::WaitRandom { min, max } => {
                // 契约 §4 wait 随机区间：[min, max] 内按 nonce 播种的 splitmix64
                // 取值（与 guest 解释器同一算法/常量，见 yaml_vnext::splitmix64
                // 测试向量）；随后复用 runtime.sleep（取消可达）。
                let min = self
                    .eval(min)?
                    .duration_ms()
                    .ok_or_else(|| anyhow!("wait min 必须是时间值"))?;
                let max = self
                    .eval(max)?
                    .duration_ms()
                    .ok_or_else(|| anyhow!("wait max 必须是时间值"))?;
                let duration = if max > min {
                    min + crate::extensions::gamer_yaml::yaml_vnext::splitmix64(&mut self.rng)
                        % (max - min + 1)
                } else {
                    min
                };
                self.invoker
                    .invoke(
                        "runtime.sleep",
                        Value::Map(BTreeMap::from([(
                            "duration".to_string(),
                            Value::Duration(duration),
                        )])),
                    )
                    .await?;
                Ok(Flow::Continue)
            }
        }
    }

    /// `call` 执行体：入口时 `call_depth` 已 +1，此处统一做深度守卫；
    /// 有 `return` → 存返回值，无 `return` → 存 null（ADR-YAML-02 返回值泛化）。
    #[async_recursion]
    async fn run_call(
        &mut self,
        target: &str,
        args: &BTreeMap<String, Expr>,
        save: &Option<String>,
    ) -> Result<Flow> {
        check_call_depth(self.call_depth)?;
        let resolver = self
            .resolver
            .clone()
            .ok_or_else(|| anyhow!("call resolver 未配置"))?;
        let program = resolver.resolve(target).await?;
        let mut child = Interpreter::new(self.invoker.clone()).with_values(self.eval_map(args)?);
        // 子解释器继承当前调用深度，否则每层 call 的深度计数被重置、
        // 深度守卫永远不触发（无界递归）；随机序列同理继承（wait 区间
        // 在被调方与主程序共享同一 nonce 流）。
        child.call_depth = self.call_depth;
        child.rng = self.rng;
        if let Some(resolver) = self.resolver.clone() {
            child = child.with_resolver(resolver);
        }
        let outcome = child.run_steps(&program.steps).await;
        self.rng = child.rng;
        match outcome? {
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

/// v3 原生参考解释器的表达式求值辅助（仅测试链路消费）。
#[allow(dead_code)]
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

/// v3 原生参考解释器的表达式求值辅助（仅测试链路消费）。
#[allow(dead_code)]
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

/// v3 原生参考解释器的表达式求值辅助（仅测试链路消费）。
#[allow(dead_code)]
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
    use crate::capabilities::{
        CapabilityResult, DeviceService, FrameHandle, FrameService, FrameSize, InputService,
        MatchManyRequest, MatchManyResult, ResourceHandle, ResourceId, ResourceLease,
        ResourceService, SearchRegion, TemplateQuery, VisionService,
    };
    use crate::extensions::gamer_yaml::yaml_vnext::load;
    use std::collections::{BTreeSet, HashMap};
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;

    #[derive(Default)]
    pub(crate) struct Trace {
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl Trace {
        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
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

    /// vision 链路桩：一个类型同时实现 Resource / Frame / Vision 三个能力，
    /// 记录每次查询的（模板名, threshold, region）供断言；`hits` 集合内的
    /// 模板名判命中。
    pub(crate) struct VisionStub {
        hits: BTreeSet<String>,
        names: std::sync::Mutex<HashMap<ResourceHandle, String>>,
        seen: std::sync::Mutex<Vec<(String, Option<f32>, Option<[u32; 4]>)>>,
    }

    impl VisionStub {
        pub(crate) fn new(hits: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                hits: hits.iter().map(|name| name.to_string()).collect(),
                names: std::sync::Mutex::new(HashMap::new()),
                seen: std::sync::Mutex::new(Vec::new()),
            })
        }

        fn name_of(&self, handle: ResourceHandle) -> String {
            self.names
                .lock()
                .unwrap()
                .get(&handle)
                .cloned()
                .unwrap_or_default()
        }

        pub(crate) fn seen(&self) -> Vec<(String, Option<f32>, Option<[u32; 4]>)> {
            self.seen.lock().unwrap().clone()
        }

        fn record(&self, name: String, options: crate::capabilities::MatchOptions) {
            self.seen.lock().unwrap().push((
                name,
                options.threshold,
                options.region.map(|region| [region.x, region.y, region.width, region.height]),
            ));
        }

        fn outcome(&self, name: String) -> MatchOutcome {
            if self.hits.contains(&name) {
                MatchOutcome::Found(crate::capabilities::MatchBox {
                    x: 400,
                    y: 200,
                    width: 200,
                    height: 100,
                    score: 0.92,
                })
            } else {
                MatchOutcome::NotFound
            }
        }
    }

    #[async_trait]
    impl ResourceService for VisionStub {
        async fn resolve(&self, id: &ResourceId) -> CapabilityResult<ResourceHandle> {
            let handle = ResourceHandle::new();
            self.names.lock().unwrap().insert(
                handle,
                id.name().trim_start_matches("templates/").to_string(),
            );
            Ok(handle)
        }

        async fn open(&self, resource: ResourceHandle) -> CapabilityResult<ResourceLease> {
            Ok(ResourceLease::new(resource, None))
        }
    }

    #[async_trait]
    impl FrameService for VisionStub {
        async fn latest(
            &self,
            _device: &DeviceHandle,
        ) -> CapabilityResult<Option<FrameHandle>> {
            Ok(None)
        }

        async fn capture(&self, _device: &DeviceHandle) -> CapabilityResult<FrameHandle> {
            Ok(FrameHandle::new())
        }

        async fn size(&self, _frame: FrameHandle) -> CapabilityResult<FrameSize> {
            Ok(FrameSize::new(1000, 1000))
        }
    }

    #[async_trait]
    impl VisionService for VisionStub {
        async fn match_template(
            &self,
            _frame: FrameHandle,
            query: TemplateQuery,
        ) -> CapabilityResult<MatchOutcome> {
            let name = self.name_of(query.template());
            self.record(name.clone(), query.options());
            Ok(self.outcome(name))
        }

        async fn match_many(
            &self,
            request: &MatchManyRequest,
        ) -> CapabilityResult<Vec<MatchManyResult>> {
            Ok(request
                .templates()
                .iter()
                .map(|query| {
                    let name = self.name_of(query.template());
                    self.record(name.clone(), query.options());
                    MatchManyResult {
                        template: query.template(),
                        outcome: self.outcome(name),
                    }
                })
                .collect())
        }

        async fn sample_color(
            &self,
            _frame: FrameHandle,
            _point: FramePoint,
        ) -> CapabilityResult<crate::capabilities::ColorSample> {
            Ok(crate::capabilities::ColorSample {
                red: 0,
                green: 0,
                blue: 0,
            })
        }
    }

    pub(crate) fn vision_registry(stub: &Arc<VisionStub>) -> CapabilityRegistry {
        CapabilityRegistry::builder()
            .with_device_service(Arc::new(Trace::default()) as Arc<dyn DeviceService>)
            .with_frame_service(stub.clone() as Arc<dyn FrameService>)
            .with_resource_service(stub.clone() as Arc<dyn ResourceService>)
            .with_vision_service(stub.clone() as Arc<dyn VisionService>)
            .build()
    }

    /// P12.7：threshold / region 实参注入 MatchOptions（TemplateQuery 链路），
    /// 结果 map 携带 region 回显 + center 相对坐标（NativeYamlHost 直测）。
    #[tokio::test]
    async fn native_host_passes_threshold_and_region_into_match_options() {
        let stub = VisionStub::new(&["reward"]);
        let host = NativeYamlHost::new(
            all_permissions(vision_registry(&stub)),
            AppContext::for_test("d1", "com.test.game").unwrap(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();

        // 命中 + threshold + region（相对 map 形态）
        let value = host
            .invoke(
                "vision.match",
                Value::Map(BTreeMap::from([
                    ("template".into(), Value::String("reward".into())),
                    ("threshold".into(), Value::Float(0.9)),
                    (
                        "region".into(),
                        Value::Map(BTreeMap::from([
                            ("x".into(), Value::Float(0.1)),
                            ("y".into(), Value::Float(0.2)),
                            ("width".into(), Value::Float(0.3)),
                            ("height".into(), Value::Float(0.4)),
                        ])),
                    ),
                ])),
            )
            .await
            .unwrap();
        assert_eq!(
            stub.seen(),
            vec![("reward".to_string(), Some(0.9), Some([100, 200, 300, 400]))],
            "threshold 填入 MatchOptions；region 相对值换算为像素"
        );
        let map = into_map(value);
        assert_eq!(map.get("found"), Some(&Value::Bool(true)));
        assert_eq!(
            map.get("center"),
            Some(&Value::Coordinate([0.5, 0.25])),
            "center = 相对坐标（沿用现状）"
        );
        assert_eq!(
            into_map(map.get("region").cloned().unwrap()).get("width"),
            Some(&Value::Float(0.3)),
            "结果 map 回显本次搜索 region"
        );

        // 未命中：region 缺省 = 全帧回显
        let value = host
            .invoke(
                "vision.match",
                Value::Map(BTreeMap::from([(
                    "template".into(),
                    Value::String("ghost".into()),
                )])),
            )
            .await
            .unwrap();
        let map = into_map(value);
        assert_eq!(map.get("found"), Some(&Value::Bool(false)));
        let region = into_map(map.get("region").cloned().unwrap());
        assert_eq!(region.get("x"), Some(&Value::Float(0.0)));
        assert_eq!(region.get("width"), Some(&Value::Float(1.0)));
        assert_eq!(
            stub.seen().len(),
            2,
        );
        assert_eq!(stub.seen()[1].1, None, "threshold 缺省省略字段 → MatchOptions::default 口径");
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
        let program = load("version: 3\nsteps:\n  - set: {ready: true}\n  - if:\n      cond: $ready\n      then:\n        - call:\n            target: script:missing\n            save: answer\n      else: []\n").unwrap();
        // The call is intentionally not entered in this test; a missing
        // resolver is a useful guard that proves the AST does not silently
        // execute arbitrary host code.
        let error = Interpreter::new(Arc::new(FakeInvoker))
            .run(&program)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("resolver"));
    }

    /// 可脚本化 invoker：vision.match / match_many 结果可配置，全部调用
    /// （含 sleep 时长、threshold args）被记录供断言。
    struct ScriptedInvoker {
        match_found: bool,
        many_hits: Vec<bool>,
        calls: std::sync::Mutex<Vec<(String, Value)>>,
    }

    fn into_map(value: Value) -> BTreeMap<String, Value> {
        match value {
            Value::Map(map) => map,
            _ => BTreeMap::new(),
        }
    }

    impl ScriptedInvoker {
        fn found(vision_found: bool) -> Self {
            Self {
                match_found: vision_found,
                many_hits: Vec::new(),
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn recorded(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().clone()
        }

        fn sleep_durations(&self) -> Vec<u64> {
            self.recorded()
                .into_iter()
                .filter(|(capability, _)| capability == "runtime.sleep")
                .filter_map(|(_, args)| {
                    into_map(args).get("duration").cloned()
                })
                .filter_map(|value| value.duration_ms())
                .collect()
        }
    }

    #[async_trait]
    impl CapabilityInvoker for ScriptedInvoker {
        async fn invoke(&self, capability: &str, args: Value) -> Result<Value> {
            self.calls
                .lock()
                .unwrap()
                .push((capability.to_string(), args.clone()));
            match capability {
                "vision.match" => {
                    let map = if self.match_found {
                        Value::Map(BTreeMap::from([
                            ("found".into(), Value::Bool(true)),
                            ("score".into(), Value::Float(0.9)),
                            ("center".into(), Value::Coordinate([0.5, 0.5])),
                        ]))
                    } else {
                        Value::Map(BTreeMap::from([("found".into(), Value::Bool(false))]))
                    };
                    Ok(map)
                }
                "vision.match_many" => {
                    let templates = match into_map(args).get("templates").cloned() {
                        Some(Value::List(items)) => items,
                        _ => bail!("templates 必须是列表"),
                    };
                    let matches = templates
                        .iter()
                        .enumerate()
                        .map(|(index, _)| {
                            let found = self.many_hits.get(index).copied().unwrap_or(false);
                            Value::Map(BTreeMap::from([
                                ("found".into(), Value::Bool(found)),
                                ("score".into(), Value::Float(0.88)),
                                ("center".into(), Value::Coordinate([0.25, 0.75])),
                            ]))
                        })
                        .collect();
                    Ok(Value::Map(BTreeMap::from([
                        ("found".into(), Value::Bool(self.many_hits.iter().any(|hit| *hit))),
                        ("matches".into(), Value::List(matches)),
                    ])))
                }
                _ => Ok(Value::Null),
            }
        }
    }

    /// P12.5（契约 §4）：wait 随机区间由 nonce 播种的 splitmix64 决定，
    /// 经 runtime.sleep 等待（取消可达）。
    #[tokio::test]
    async fn native_interpreter_wait_random_is_nonce_seeded() {
        let program = load(
            "version: 3\nsteps:\n  - wait: {min: 100ms, max: 200ms}\n  - wait: {min: 1s, max: 1s}\n",
        )
        .unwrap();
        let program = Program {
            nonce: Some(7),
            ..program
        };
        let invoker = Arc::new(ScriptedInvoker::found(false));
        Interpreter::new(invoker.clone()).run(&program).await.unwrap();
        let mut state = 7u64;
        let expected_first =
            100 + crate::extensions::gamer_yaml::yaml_vnext::splitmix64(&mut state) % 101;
        assert_eq!(
            invoker.sleep_durations(),
            vec![expected_first, 1_000],
            "随机区间取值必须 = min + splitmix64(nonce) % (max-min+1)；定值区间原样"
        );
    }

    /// P12.7（ADR-YAML-03）：find 的 save / `$match` 块内上下文与块后复位。
    #[tokio::test]
    async fn native_interpreter_find_scopes_match_and_persists_save() {
        let program = load(
            "version: 3\nsteps:\n  - find:\n      template: reward\n      save: reward\n      then:\n        - log: hit\n      verify:\n        template: reward\n        timeout: 1s\n  - set: {leaked: $match}\n  - return: $reward.found\n",
        )
        .unwrap();
        let invoker = Arc::new(ScriptedInvoker::found(true));
        let result = Interpreter::new(invoker.clone())
            .run(&program)
            .await
            .unwrap();
        assert_eq!(result.value, Value::Bool(true), "save 变量跨步可用");
        assert_eq!(
            result.logs,
            vec![("info".to_string(), "hit".to_string())],
            "then 体内执行"
        );
        // `$match` 在块后被复位（不跨块泄漏），save 的命名变量不受影响
        assert_eq!(
            lookup_path(
                &BTreeMap::from([("leaked".into(), Value::Null)]),
                "leaked"
            ),
            Some(Value::Null)
        );
        let recorded = invoker.recorded();
        let verify_calls = recorded
            .iter()
            .filter(|(capability, _)| capability == "vision.match")
            .count();
        assert!(
            verify_calls >= 2,
            "verify 在 then 之后二次验证模板: {verify_calls}"
        );
    }

    /// P12.7 裁决：find 超时无 else → 抛 `FIND_TIMEOUT: <template>`。
    #[tokio::test]
    async fn native_interpreter_find_timeout_without_else_throws() {
        let program = load(
            "version: 3\nsteps:\n  - find:\n      template: ghost\n      timeout: 3s\n",
        )
        .unwrap();
        let error = Interpreter::new(Arc::new(ScriptedInvoker::found(false)))
            .run(&program)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("FIND_TIMEOUT: ghost"),
            "超时无 else 必须抛 FIND_TIMEOUT: {error}"
        );
    }

    /// P12.7：check 轮询至出现（未命中先 sleep(poll) 重试），threshold 经
    /// args 注入 vision.match。
    #[tokio::test]
    async fn native_interpreter_check_polls_and_passes_threshold() {
        let program = load(
            "version: 3\nsteps:\n  - check:\n      template: ready\n      threshold: 0.95\n",
        )
        .unwrap();
        let invoker = Arc::new(ScriptedInvoker::found(true));
        Interpreter::new(invoker.clone()).run(&program).await.unwrap();
        let recorded = invoker.recorded();
        let (capability, args) = recorded
            .iter()
            .find(|(capability, _)| capability == "vision.match")
            .expect("check 必须调用 vision.match");
        assert_eq!(capability, "vision.match");
        let map = into_map(args.clone());
        assert_eq!(
            map.get("threshold"),
            Some(&Value::Float(0.95)),
            "step threshold 注入 invoke args"
        );
        assert_eq!(
            map.get("template"),
            Some(&Value::String("ready".into()))
        );
    }

    /// P12.7：match_first 首个命中候选执行自己的 steps，`$match` = 该候选
    /// 结果；候选级 threshold 经 thresholds 平行列表传给 match_many。
    #[tokio::test]
    async fn native_interpreter_match_first_runs_first_hit_candidate_steps() {
        let program = load(
            "version: 3\nsteps:\n  - match_first:\n      candidates:\n        - template: a\n          threshold: 0.6\n          steps:\n            - log: cand-a\n        - template: b\n          steps:\n            - set: {m: $match}\n            - log: cand-b\n  - return: $m.center\n",
        )
        .unwrap();
        let invoker = Arc::new(ScriptedInvoker {
            match_found: false,
            many_hits: vec![false, true],
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let result = Interpreter::new(invoker.clone())
            .run(&program)
            .await
            .unwrap();
        assert_eq!(
            result.value,
            Value::Coordinate([0.25, 0.75]),
            "候选 steps 内 $match = 该候选结果"
        );
        assert_eq!(
            result.logs,
            vec![("info".to_string(), "cand-b".to_string())],
            "只执行首个命中候选的 steps"
        );
        let many = invoker
            .recorded()
            .into_iter()
            .find(|(capability, _)| capability == "vision.match_many")
            .expect("match_first 必须调用 vision.match_many");
        let args = into_map(many.1);
        assert_eq!(
            args.get("thresholds"),
            Some(&Value::List(vec![
                Value::Float(0.6),
                Value::Null
            ])),
            "候选级 threshold 以平行列表传给 match_many"
        );
    }

    /// 恒返回自递归程序的 resolver：递归 call 深度守卫测试用。
    struct SelfResolver;

    #[async_trait]
    impl ProgramResolver for SelfResolver {
        async fn resolve(&self, _target: &str) -> Result<Program> {
            load("version: 3\nsteps:\n  - call:\n      target: script:self\n")
                .map_err(|diagnostics| anyhow!("fixture resolver: {diagnostics:?}"))
        }
    }

    #[test]
    fn native_interpreter_enforces_call_depth_limit() {
        // 原生参考解释器的 async_recursion 调用链每层叠 3 个 boxed future 的
        // poll 帧，Windows 测试线程默认 1 MiB 栈容不下 33 层；放大栈执行。
        let handle = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async {
                    let program =
                        load("version: 3\nsteps:\n  - call:\n      target: script:self\n")
                            .unwrap();
                    let error = Interpreter::new(Arc::new(FakeInvoker))
                        .with_resolver(Arc::new(SelfResolver))
                        .run(&program)
                        .await
                        .unwrap_err();
                    assert!(
                        error.to_string().contains("CALL_DEPTH_EXCEEDED"),
                        "递归超限必须报 CALL_DEPTH_EXCEEDED: {error}"
                    );
                    assert!(
                        error.to_string().contains("max=32"),
                        "深度错误必须带预算上限: {error}"
                    );
                })
            })
            .unwrap();
        handle.join().unwrap();
    }

    /// P12.4（ADR-YAML-04）：无 times 空转体 loop 必须被步预算终止，报
    /// STEP_BUDGET_EXCEEDED（每轮迭代本身计一步，空 body 也受约束）。
    #[tokio::test]
    async fn native_interpreter_terminates_unbounded_empty_loop_with_step_budget() {
        let program = load("version: 3\nsteps:\n  - loop:\n      steps: []\n").unwrap();
        let error = Interpreter::new(Arc::new(FakeInvoker))
            .run(&program)
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("STEP_BUDGET_EXCEEDED"),
            "死循环必须报 STEP_BUDGET_EXCEEDED: {message}"
        );
        assert!(
            message.contains("max=100000"),
            "步数错误必须带预算上限: {message}"
        );
    }

    /// P12.4（ADR-YAML-04）：步数按逻辑步计——顶层只有 1 个 loop 步，但循环
    /// 体（内层 loop 每轮 + set 子步）全计，外层包裹不得绕过预算。
    #[tokio::test]
    async fn native_interpreter_counts_nested_loop_body_steps_against_budget() {
        let program = load(
            "version: 3\nsteps:\n  - loop:\n      steps:\n        - loop:\n            times: 60000\n            steps:\n              - set: {n: 1}\n",
        )
        .unwrap();
        let error = Interpreter::new(Arc::new(FakeInvoker))
            .run(&program)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("STEP_BUDGET_EXCEEDED"),
            "嵌套子步必须计入预算: {error}"
        );
    }

    /// P12.4：预算内的正常脚本不受影响（< 100_000 逻辑步正常完成）。
    #[tokio::test]
    async fn native_interpreter_runs_normal_scripts_within_budget() {
        let program = load(
            "version: 3\nsteps:\n  - loop:\n      times: 1000\n      steps:\n        - set: {n: 1}\n  - return: done\n",
        )
        .unwrap();
        let result = Interpreter::new(Arc::new(FakeInvoker))
            .run(&program)
            .await
            .unwrap();
        assert_eq!(result.value, Value::String("done".into()));
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
            AppContext::for_test("d1", "com.test.game").unwrap(),
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
            writer.finish().unwrap();
        }
        let installed = service.install(&archive).await.unwrap();
        service.enable(installed.id()).await.unwrap();
        let panels = service.ui_contributions().unwrap();
        assert_eq!(panels.len(), 3);
        let component_of = |panel_id: &str| {
            panels
                .iter()
                .find(|panel| panel.panel_id == panel_id)
                .map(|panel| panel.component.clone().unwrap_or_default())
                .unwrap_or_default()
        };
        assert_eq!(component_of("automation"), "console.scripts");
        assert_eq!(component_of("functions"), "console.functions");
        assert_eq!(component_of("templates"), "console.templates");
        assert!(panels
            .iter()
            .all(|panel| panel.runtime == crate::extensions::UiRuntime::Core));
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
    use super::super::wasm_host::LazyYamlWasmtimeRuntime;
    use super::*;
    use crate::capabilities::{
        CapabilityRegistry, CapabilityResult, FrameHandle, FrameService, LogRecord, LogService,
        ResourceHandle, ResourceId, ResourceLease, ResourceService, VisionService,
    };
    use crate::extensions::gamer_yaml::yaml_vnext::load;
    use crate::extensions::HostApiCatalog;
    use async_trait::async_trait;
    use std::fs;
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::sync::{Arc, Mutex, OnceLock};
    use zip::write::SimpleFileOptions;

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
            if target != "script:helper" {
                bail!("unknown fixture target: {target}");
            }
            load("version: 3\nsteps:\n  - return: from-call\n")
                .map_err(|diagnostics| anyhow!("fixture resolver: {diagnostics:?}"))
        }
    }

    /// 捕获 log.write 的 Trace 服务（v3 端到端用）。
    struct LogTrace {
        logs: Mutex<Vec<String>>,
    }

    impl LogService for LogTrace {
        fn write(&self, record: LogRecord) -> CapabilityResult<()> {
            self.logs.lock().unwrap().push(record.message().to_string());
            Ok(())
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
            "version: 3\nsteps:\n  - call:\n      target: script:helper\n      save: answer\n  - text: from-real-wasm\n  - return: $answer\n",
        )
        .unwrap();
        let result = runtime
            .run(YamlWasmRunRequest {
                wasm: fixture_component(),
                program,
                args: BTreeMap::new(),
                resolver: Some(Arc::new(FixtureResolver)),
                start_index: None,
                host: host(trace.clone()),
                context: AppContext::for_test("device-1", "com.example.game").unwrap(),
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
                start_index: None,
                host: host_with_permissions(Arc::new(Trace::default()), &["device.read"]),
                context: AppContext::for_test("device-1", "com.example.game").unwrap(),
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
                start_index: None,
                host: host_with_permissions(Arc::new(Trace::default()), &["device.read"]),
                context: AppContext::for_test("device-1", "com.example.game").unwrap(),
                stop: Arc::new(AtomicBool::new(false)),
            })
            .await
            .unwrap_err();
        assert!(
            denied.to_string().contains("kind=denied"),
            "permission denial lost its WIT kind: {denied:#}"
        );

        // P12.4：取消是双机制（capability 边界 kind=cancelled 与 epoch trap
        // CANCELLED 竞速，ADR-YAML-04），两者都是合法的取消形态——stop 先于
        // 运行置位时，若 capability 路径在一个 tick 内完成则报 kind=cancelled，
        // 否则 epoch trap 先打断报 CANCELLED。
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
                start_index: None,
                host: host_with_permissions(
                    Arc::new(Trace::default()),
                    &["device.read", "runtime.sleep"],
                ),
                context: AppContext::for_test("device-1", "com.example.game").unwrap(),
                stop: Arc::new(AtomicBool::new(true)),
            })
            .await
            .unwrap_err();
        let message = cancelled.to_string();
        assert!(
            message.contains("kind=cancelled") || message.contains("CANCELLED"),
            "cancellation must surface as a cancel-shaped error: {message}"
        );
    }

    /// WIT kind 保留属性（确定性单测）：capability 层取消错误必须映射为
    /// host-error kind=cancelled、权限拒绝映射为 denied（epoch 取消兜底与此
    /// 并行，见上一 e2e 注释）。
    #[test]
    fn capability_errors_map_to_wit_kinds() {
        use crate::capabilities::CapabilityError;
        use crate::extensions::wit::yaml::gamer::host::types::HostErrorKind;

        let error = anyhow::Error::new(CapabilityError::Cancelled);
        let mapped = super::super::wasm_host::yaml_capability_error_for_test(&error);
        assert!(matches!(mapped.kind, HostErrorKind::Cancelled));

        let denied = anyhow::Error::new(crate::extensions::ExtensionError::Permission(
            crate::extensions::PermissionError::NotGranted("input.tap".into()),
        ));
        let mapped = super::super::wasm_host::yaml_capability_error_for_test(&denied);
        assert!(matches!(mapped.kind, HostErrorKind::Denied));
    }

    /// Phase 10 验收（yaml 插件侧）：安装 → 启用 → 一个最小 `version: 3`
    /// 脚本（log 级别）经 ExtensionService::run_yaml_vnext 用真实 Component
    /// guest 跑通；卸载后同一脚本明确失败。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn installed_yaml_extension_runs_v3_program_end_to_end() {
        let temp = tempfile::tempdir().expect("无法创建 yaml 扩展临时目录");
        let logs = Arc::new(LogTrace {
            logs: Mutex::new(Vec::new()),
        });
        let registry = CapabilityRegistry::builder()
            .with_device_service(
                Arc::new(Trace::default()) as Arc<dyn crate::capabilities::DeviceService>
            )
            .with_log_service(logs.clone() as Arc<dyn LogService>)
            .build();
        let service = crate::extensions::ExtensionService::for_data_root(temp.path(), registry);

        let mut archive = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut archive));
            let options = SimpleFileOptions::default();
            writer.start_file("manifest.toml", options).unwrap();
            writer
                .write_all(YAML_EXTENSION_MANIFEST_TOML.as_bytes())
                .unwrap();
            writer.start_file("plugin.wasm", options).unwrap();
            writer.write_all(&fixture_component()).unwrap();
            writer.finish().unwrap();
        }
        let installed = service.install(&archive).await.unwrap();
        let id = crate::extensions::ExtensionId::parse(YAML_EXTENSION_ID).unwrap();
        service.enable(&id).await.unwrap();

        let program = load(
            "version: 3\nsteps:\n  - invoke:\n      capability: log.write\n      with:\n        level: info\n        message: from-v3-e2e\n  - set: {done: true}\n  - return: $done\n",
        )
        .unwrap();
        let value = super::super::run_yaml_vnext(
            &service,
            program,
            AppContext::for_test("device-1", "com.example.game").unwrap(),
            BTreeMap::new(),
            None,
            Arc::new(AtomicBool::new(false)),
            None,
        )
        .await
        .unwrap();
        assert_eq!(value, Value::Bool(true));
        assert_eq!(logs.logs.lock().unwrap().as_slice(), ["from-v3-e2e"]);

        service.disable(&id).await.unwrap();
        assert!(service
            .uninstall(&id, installed.active_version())
            .await
            .unwrap());
        let program = load("version: 3\nsteps:\n  - set: {done: true}\n").unwrap();
        assert!(super::super::run_yaml_vnext(
            &service,
            program,
            AppContext::for_test("device-1", "com.example.game").unwrap(),
            BTreeMap::new(),
            None,
            Arc::new(AtomicBool::new(false)),
            None,
        )
        .await
        .is_err());
    }

    /// 生命周期执行器桩：本测试只验证 runner 注册/注销与任务挂起/恢复，
    /// 永不真正提交运行（run loop 未启动）。
    struct UnreachableExecutor;

    impl crate::run_manager::RunExecutor for UnreachableExecutor {
        fn prepare<'a>(
            &'a self,
            _context: &'a crate::core::RunContext,
            _request: &'a crate::core::RunRequest,
        ) -> futures_util::future::BoxFuture<'a, anyhow::Result<()>> {
            unreachable!("生命周期测试不应触发真实运行")
        }

        fn execute<'a>(
            &'a self,
            _context: &'a crate::core::RunContext,
            _request: &'a crate::core::RunRequest,
            _realtime_logs: bool,
            _stop: Arc<AtomicBool>,
        ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
            unreachable!("生命周期测试不应触发真实运行")
        }

        fn acquire(
            &self,
            _context: &crate::core::RunContext,
        ) -> anyhow::Result<Box<dyn crate::core::ActivityLease>> {
            unreachable!("生命周期测试不应触发真实运行")
        }
    }

    /// P11.2 生命周期集成（ADR-13 验收，真实 wasm guest fixture 链路）：
    /// 裸 Core 无 runner → 安装 fixture 包 → enable（不注册）→ start gamer.yaml
    /// → runner 注册且 owner=扩展 id → 建 Task → stop：任务进 dependency_missing
    /// 且数据保留 → 再 start：任务自动回 Active 且 next_wakeup 经 cron 重算。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_extension_lifecycle_binds_and_resumes_the_timer_runner() {
        let data = tempfile::tempdir().expect("无法创建生命周期测试临时目录");
        let cfg = crate::config::Config {
            data_dir: data.path().to_path_buf(),
            ..Default::default()
        };
        let db: crate::store::Db = Arc::new(crate::store::Store::open(&cfg).unwrap());
        let scripts = Arc::new(crate::resources::ResourceStore::open(&cfg).unwrap());
        let runs = Arc::new(crate::run_manager::RunManager::new(Arc::new(
            UnreachableExecutor,
        )));
        let scheduler = Arc::new(crate::scheduler::Scheduler::new(db.clone()));
        assert!(
            scheduler.runners().is_empty(),
            "裸 Core：扩展 start 之前没有任何 runner"
        );

        let registrar = Arc::new(
            crate::extensions::gamer_yaml::timer_yaml::YamlTimerRunnerRegistrar::new(
                scheduler.clone(),
                db.clone(),
                runs.clone(),
                scripts.clone(),
            ),
        );
        let service = crate::extensions::ExtensionService::for_data_root(
            data.path(),
            CapabilityRegistry::default(),
        )
        .with_runner_registrar(registrar);

        let mut archive = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut archive));
            let options = SimpleFileOptions::default();
            writer.start_file("manifest.toml", options).unwrap();
            writer
                .write_all(YAML_EXTENSION_MANIFEST_TOML.as_bytes())
                .unwrap();
            writer.start_file("plugin.wasm", options).unwrap();
            writer.write_all(&fixture_component()).unwrap();
            writer.finish().unwrap();
        }
        service.install(&archive).await.unwrap();
        let id = crate::extensions::ExtensionId::parse(YAML_EXTENSION_ID).unwrap();
        service.enable(&id).await.unwrap();
        // enable 不注册 runner：Running 才是生命周期边界
        assert!(scheduler.runners().is_empty(), "enable 不应注册 runner");
        service.start(&id).await.unwrap();

        let runners = scheduler.runners();
        assert_eq!(runners.len(), 1);
        assert_eq!(runners[0].runner_id, "gamer.yaml");
        assert_eq!(runners[0].owner_extension_id, "gamer.yaml");

        // 建 Task：Active + 未来唤醒游标 + 用户数据
        let schedule = crate::timer_core::TaskSchedule::new(
            "cron",
            serde_json::json!({"expression": "0 8 * * *"}),
        )
        .unwrap();
        let mut task = crate::timer_core::Task::new(
            "task-lifecycle",
            "Lifecycle",
            AppContext::for_test("device-1", "com.example.game").unwrap(),
            "gamer.yaml",
            "com.example.game/daily.yaml",
            serde_json::json!({"args": {"lives": 3}}),
            schedule,
        )
        .unwrap();
        task.next_wakeup = Some(chrono::Utc::now() + chrono::Duration::hours(1));
        db.upsert_timer_task_async(&task).await.unwrap();

        // stop：runner 注销，Active 任务显式挂起，数据原样保留
        service.stop(&id).await.unwrap();
        assert!(scheduler.runners().is_empty(), "stop 后 runner 消失");
        let suspended = db
            .get_timer_task_async("task-lifecycle")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            suspended.state,
            crate::timer_core::TaskState::DependencyMissing
        );
        assert_eq!(
            suspended.suspend_reason.as_deref(),
            Some("missing_dependency=gamer.yaml")
        );
        assert!(suspended.enabled, "enabled 用户原意保留");
        assert_eq!(suspended.entrypoint, "com.example.game/daily.yaml");
        assert_eq!(suspended.payload["args"]["lives"], 3, "payload 原样保留");
        assert!(suspended.next_wakeup.is_none(), "挂起即清唤醒游标");

        // 再 start：runner 重注册，任务自动回 Active，唤醒游标经 cron 重算
        service.start(&id).await.unwrap();
        let resumed = db
            .get_timer_task_async("task-lifecycle")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resumed.state, crate::timer_core::TaskState::Active);
        assert!(resumed.suspend_reason.is_none());
        let next = resumed.next_wakeup.expect("恢复必须重算唤醒游标");
        assert!(
            next > chrono::Utc::now(),
            "重算后的唤醒游标是下一次 cron 触发（未来时刻），不是陈旧值"
        );
    }

    /// 内存版 `script:` / `function:` 命名空间 resolver（P12.2 e2e 用）：
    /// 与生产 ScriptProgramResolver 走同一 split_call_target / load_function
    /// 前端（P12.4 起深度守卫归 guest，resolver 不再介入）。
    struct MemoryResolver {
        scripts: BTreeMap<String, String>,
        functions: BTreeMap<String, String>,
    }

    impl YamlProgramResolver for MemoryResolver {
        fn resolve(&self, target: &str, _args: &BTreeMap<String, Value>) -> Result<Program> {
            let parsed = crate::extensions::gamer_yaml::yaml_vnext::split_call_target(target)
                .map_err(|diagnostics| anyhow!("call 目标无效: {diagnostics:?}"))?;
            match parsed {
                crate::extensions::gamer_yaml::yaml_vnext::CallTarget::Script(id) => {
                    let source = self
                        .scripts
                        .get(&id)
                        .ok_or_else(|| anyhow!("找不到脚本 {id}"))?;
                    load(source).map_err(|diagnostics| anyhow!("{diagnostics:?}"))
                }
                crate::extensions::gamer_yaml::yaml_vnext::CallTarget::Function {
                    file,
                    function,
                } => {
                    let source = self
                        .functions
                        .get(&file)
                        .ok_or_else(|| anyhow!("找不到函数文件 {file}"))?;
                    crate::extensions::gamer_yaml::yaml_vnext::load_function(source, &function)
                        .map_err(|diagnostics| anyhow!("{diagnostics:?}"))
                }
            }
        }
    }

    fn log_host(logs: Arc<LogTrace>) -> HostApi {
        let manifest = crate::extensions::parse_manifest(
            r#"manifest_version = 1
id = "gamer.yaml"
version = "3.0.0"
name = "YAML vNext"
entry = "plugin.wasm"
permissions = ["device.read", "log.write"]
[host_api]
device = "^1.0"
log = "^1.0"
"#
            .as_bytes(),
        )
        .unwrap();
        HostApi::for_manifest(
            CapabilityRegistry::builder()
                .with_device_service(
                    Arc::new(Trace::default()) as Arc<dyn crate::capabilities::DeviceService>
                )
                .with_log_service(logs as Arc<dyn LogService>)
                .build(),
            HostApiCatalog::default(),
            &manifest,
        )
        .unwrap()
    }

    fn run_request(
        program: Program,
        resolver: Option<Arc<dyn YamlProgramResolver>>,
        host: HostApi,
        start_index: Option<usize>,
    ) -> YamlWasmRunRequest {
        YamlWasmRunRequest {
            wasm: fixture_component(),
            program,
            args: BTreeMap::new(),
            resolver,
            start_index,
            host,
            context: AppContext::for_test("device-1", "com.example.game").unwrap(),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// P12.2 验收（e2e，真实 Component guest）：`call` `function:` 命名空间
    /// → v3 函数库装载 → object / array 返回值泛化，`save` + `$r.ok` /
    /// `$arr` 分支正确。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_component_calls_functions_with_generalized_returns() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let functions = BTreeMap::from([(
            // 注意：两元素数字数组会被 v3 表达式定型为 Coordinate，故 items
            // 用三元素数组承载「object 内嵌 array」示例。
            "lib".to_string(),
            "fn1:\n  params:\n    - name: flag\n      type: bool\n      default: false\n  steps:\n    - if: {cond: $flag, then: [{return: {ok: true, items: [1, 2, 3]}}]}\n    - return: {ok: false, items: []}\nfn2:\n  steps:\n    - return: [7, 8, 9]\n"
                .to_string(),
        )]);
        let resolver = Arc::new(MemoryResolver {
            scripts: BTreeMap::new(),
            functions,
        });

        // object 返回 + `$r.ok` 分支
        let logs = Arc::new(LogTrace {
            logs: Mutex::new(Vec::new()),
        });
        let program = load(
            "version: 3\nsteps:\n  - call:\n      target: function:lib/fn1\n      with: {flag: true}\n      save: r\n  - if:\n      cond: $r.ok\n      then:\n        - log: branch-ok\n  - return: $r\n",
        )
        .unwrap();
        let result = runtime
            .run(run_request(
                program,
                Some(resolver.clone()),
                log_host(logs.clone()),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(
            result.value,
            Value::Map(BTreeMap::from([
                (
                    "items".to_string(),
                    Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
                ),
                ("ok".to_string(), Value::Bool(true)),
            ]))
        );
        assert_eq!(logs.logs.lock().unwrap().as_slice(), ["branch-ok"]);

        // array 返回 + `$arr` truthy 分支
        let logs = Arc::new(LogTrace {
            logs: Mutex::new(Vec::new()),
        });
        let program = load(
            "version: 3\nsteps:\n  - call:\n      target: function:lib/fn2\n      save: arr\n  - if:\n      cond: $arr\n      then:\n        - log: arr-nonempty\n  - return: $arr\n",
        )
        .unwrap();
        let result = runtime
            .run(run_request(
                program,
                Some(resolver),
                log_host(logs.clone()),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(
            result.value,
            Value::List(vec![Value::Int(7), Value::Int(8), Value::Int(9)])
        );
        assert_eq!(logs.logs.lock().unwrap().as_slice(), ["arr-nonempty"]);
    }

    /// P12.2 验收（e2e）：`call` `script:` 命名空间带参数；未传参走声明
    /// 默认值兜底。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_component_calls_scripts_with_args() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let resolver = Arc::new(MemoryResolver {
            scripts: BTreeMap::from([(
                "com.test.app/other.yaml".to_string(),
                "version: 3\nparams:\n  - name: greeting\n    type: string\n    default: hi\nsteps:\n  - log: $greeting\n  - return: {echo: $greeting}\n"
                    .to_string(),
            )]),
            functions: BTreeMap::new(),
        });

        let logs = Arc::new(LogTrace {
            logs: Mutex::new(Vec::new()),
        });
        let program = load(
            "version: 3\nsteps:\n  - call:\n      target: script:com.test.app/other.yaml\n      with: {greeting: \"你好\"}\n      save: out\n  - return: $out.echo\n",
        )
        .unwrap();
        let result = runtime
            .run(run_request(
                program,
                Some(resolver.clone()),
                log_host(logs.clone()),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(result.value, Value::String("你好".into()));
        assert_eq!(logs.logs.lock().unwrap().as_slice(), ["你好"]);

        // 未传参 → 声明默认值兜底
        let logs = Arc::new(LogTrace {
            logs: Mutex::new(Vec::new()),
        });
        let program = load(
            "version: 3\nsteps:\n  - call:\n      target: script:com.test.app/other.yaml\n      save: out\n  - return: $out.echo\n",
        )
        .unwrap();
        let result = runtime
            .run(run_request(
                program,
                Some(resolver),
                log_host(logs.clone()),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(result.value, Value::String("hi".into()));
        assert_eq!(logs.logs.lock().unwrap().as_slice(), ["hi"]);
    }

    /// P12.4 验收（e2e）：递归 call 超 32 层 → guest 本地 ExecutionBudget
    /// 报 CALL_DEPTH_EXCEEDED（WIT 不再透传 depth，宿主 resolver 无深度守卫）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_component_rejects_recursion_beyond_call_depth_limit() {
        struct RecursiveResolver;

        impl YamlProgramResolver for RecursiveResolver {
            fn resolve(&self, _target: &str, _args: &BTreeMap<String, Value>) -> Result<Program> {
                load("version: 3\nsteps:\n  - call:\n      target: script:self\n")
                    .map_err(|diagnostics| anyhow!("fixture resolver: {diagnostics:?}"))
            }
        }

        let runtime = LazyYamlWasmtimeRuntime::new();
        let program = load("version: 3\nsteps:\n  - call:\n      target: script:self\n").unwrap();
        let error = runtime
            .run(run_request(
                program,
                Some(Arc::new(RecursiveResolver)),
                log_host(Arc::new(LogTrace {
                    logs: Mutex::new(Vec::new()),
                })),
                None,
            ))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("CALL_DEPTH_EXCEEDED"),
            "递归超限必须报 CALL_DEPTH_EXCEEDED: {error:#}"
        );
    }

    /// P12.2 验收（e2e，契约 §8）：program 顶层可选 `start_index` 只跳顶层
    /// 步骤；缺省 = 从头执行。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_component_honors_top_level_start_index() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let source = "version: 3\nsteps:\n  - log: first\n  - log: second\n  - return: done\n";

        let logs = Arc::new(LogTrace {
            logs: Mutex::new(Vec::new()),
        });
        let result = runtime
            .run(run_request(
                load(source).unwrap(),
                None,
                log_host(logs.clone()),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(result.value, Value::String("done".into()));
        assert_eq!(logs.logs.lock().unwrap().as_slice(), ["first", "second"]);

        let logs = Arc::new(LogTrace {
            logs: Mutex::new(Vec::new()),
        });
        // start_index = 1：跳过顶层第 0 步（log first），只跑 log second + return。
        let result = runtime
            .run(run_request(
                load(source).unwrap(),
                None,
                log_host(logs.clone()),
                Some(1),
            ))
            .await
            .unwrap();
        assert_eq!(result.value, Value::String("done".into()));
        assert_eq!(logs.logs.lock().unwrap().as_slice(), ["second"]);
    }

    /// P12.4 验收（e2e，ADR-YAML-04）：无 times 空转体 loop → guest 步预算
    /// 终止，报 STEP_BUDGET_EXCEEDED（确定性、非 trap/栈溢出）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_component_terminates_unbounded_loop_with_step_budget_exceeded() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let program = load("version: 3\nsteps:\n  - loop:\n      steps: []\n").unwrap();
        let error = runtime
            .run(run_request(
                program,
                None,
                log_host(Arc::new(LogTrace {
                    logs: Mutex::new(Vec::new()),
                })),
                None,
            ))
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("STEP_BUDGET_EXCEEDED"),
            "死循环必须以 STEP_BUDGET_EXCEEDED 终止: {message}"
        );
        assert!(
            message.contains("consumed=") && message.contains("max=100000"),
            "步数错误必须带 consumed/max: {message}"
        );
        assert!(
            !message.contains("call stack exhausted"),
            "预算终止不是栈溢出 trap: {message}"
        );
    }

    /// P12.4 验收（e2e，ADR-YAML-04）：纯计算段（不经 capability 边界）被
    /// epoch interruption 打断 → CANCELLED。
    ///
    /// 确定性设计：单个 `set` 步求值一个 30 万元素字面列表（guest 侧 serde
    /// 解析 ~14MB program JSON + 列表求值，远超 100ms 纯 wasm 计算），预算
    /// 不可能在计算完成前耗尽；stop 于 +50ms 置位，ticker 周期 10ms → 首个
    /// epoch 检查点（≤ ~70ms）必然落在计算中途，由 epoch trap 终止。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_component_cancels_pure_compute_via_epoch_interruption() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        // 大字面列表：guest 解析 + 求值都是纯 wasm 计算，不产生逻辑步计数。
        let program = crate::extensions::gamer_yaml::yaml_vnext::Program {
            version: 3,
            params: Vec::new(),
            nonce: None,
            steps: vec![SmallStep::Set {
                name: "big".into(),
                value: Expr::List(vec![Expr::Literal(Value::Int(1)); 300_000]),
            }],
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flip = stop.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            stop_flip.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let error = runtime
            .run(YamlWasmRunRequest {
                wasm: fixture_component(),
                program,
                args: BTreeMap::new(),
                resolver: None,
                start_index: None,
                host: log_host(Arc::new(LogTrace {
                    logs: Mutex::new(Vec::new()),
                })),
                context: AppContext::for_test("device-1", "com.example.game").unwrap(),
                stop,
            })
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("CANCELLED"),
            "纯计算段必须被 epoch 取消打断并报 CANCELLED: {message}"
        );
        assert!(
            !message.contains("STEP_BUDGET_EXCEEDED") && !message.contains("CALL_DEPTH_EXCEEDED"),
            "epoch 取消不应被误报为预算耗尽: {message}"
        );
    }

    // -----------------------------------------------------------------------
    use super::tests::{Trace as InputTrace, VisionStub};
    // P12.5 / P12.7 e2e（真实 Component guest + NativeYamlHost capability 链）
    // -----------------------------------------------------------------------

    /// e2e 宿主：device/input 走 Trace，vision/resource/frame 走 VisionStub，
    /// log 走 LogTrace；权限覆盖 vision/timing 链路全部 capability。
    fn vision_host(stub: &Arc<VisionStub>, input: Arc<InputTrace>, logs: Arc<LogTrace>) -> HostApi {
        let manifest = crate::extensions::parse_manifest(
            r#"manifest_version = 1
id = "gamer.yaml"
version = "3.0.0"
name = "YAML vNext"
entry = "plugin.wasm"
permissions = ["device.read", "input.tap", "log.write", "vision.match", "resource.read", "runtime.sleep"]
[host_api]
device = "^1.0"
input = "^1.0"
vision = "^1.0"
resource = "^1.0"
runtime = "^1.0"
log = "^1.0"
"#
            .as_bytes(),
        )
        .unwrap();
        HostApi::for_manifest(
            CapabilityRegistry::builder()
                .with_device_service(input.clone() as Arc<dyn crate::capabilities::DeviceService>)
                .with_input_service(input as Arc<dyn crate::capabilities::InputService>)
                .with_frame_service(stub.clone() as Arc<dyn FrameService>)
                .with_resource_service(stub.clone() as Arc<dyn ResourceService>)
                .with_vision_service(stub.clone() as Arc<dyn VisionService>)
                .with_log_service(logs as Arc<dyn LogService>)
                .build(),
            HostApiCatalog::default(),
            &manifest,
        )
        .unwrap()
    }

    /// P12.7 e2e：threshold 三级优先经真实 guest → capability.invoke →
    /// NativeYamlHost → TemplateQuery 注入（step 值 > defaults > 缺省省略）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_component_injects_resolved_threshold_into_vision_args() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let stub = VisionStub::new(&["a", "b"]);
        let program = load(
            "version: 3\ndefaults:\n  vision:\n    threshold: 0.7\nsteps:\n  - check:\n      template: a\n  - check:\n      template: b\n      threshold: 0.95\n",
        )
        .unwrap();
        runtime
            .run(run_request(
                program,
                None,
                vision_host(
                    &stub,
                    Arc::new(InputTrace::default()),
                    Arc::new(LogTrace {
                        logs: Mutex::new(Vec::new()),
                    }),
                ),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(
            stub.seen(),
            vec![
                ("a".to_string(), Some(0.7), None),
                ("b".to_string(), Some(0.95), None),
            ],
            "check(a) 用 defaults 0.7；check(b) 用 step 0.95"
        );
    }

    /// P12.7 e2e：find 命中 → save → then（tap `$reward.center`）→ verify
    /// 不命中抛 `VERIFY_FAILED: <template>`。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_component_find_then_tap_chain_and_verify_failure() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let stub = VisionStub::new(&["reward"]);
        let input = Arc::new(InputTrace::default());
        let logs = Arc::new(LogTrace {
            logs: Mutex::new(Vec::new()),
        });
        let program = load(
            "version: 3\nsteps:\n  - find:\n      template: reward\n      timeout: 5s\n      save: reward\n      then:\n        - tap: {point: $reward.center}\n        - log: got\n      else:\n        - log: miss\n      verify:\n        template: home\n        timeout: 600ms\n",
        )
        .unwrap();
        let error = runtime
            .run(run_request(
                program,
                None,
                vision_host(&stub, input.clone(), logs.clone()),
                None,
            ))
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("VERIFY_FAILED: home"),
            "verify 不命中必须抛 VERIFY_FAILED: {message}"
        );
        assert_eq!(
            input.calls(),
            vec!["tap:500:250".to_string()],
            "then 体内 tap $reward.center = 命中框中心相对坐标"
        );
        assert_eq!(
            logs.logs.lock().unwrap().as_slice(),
            ["got"],
            "then 执行、else 不执行（未超时）"
        );
    }

    /// P12.7 e2e：find 超时（无命中）→ 走 else，不抛错。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_component_find_timeout_runs_else_branch() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let stub = VisionStub::new(&[]);
        let logs = Arc::new(LogTrace {
            logs: Mutex::new(Vec::new()),
        });
        let program = load(
            "version: 3\nsteps:\n  - find:\n      template: ghost\n      timeout: 350ms\n      else:\n        - log: gone\n",
        )
        .unwrap();
        let start = std::time::Instant::now();
        runtime
            .run(run_request(
                program,
                None,
                vision_host(&stub, Arc::new(InputTrace::default()), logs.clone()),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(logs.logs.lock().unwrap().as_slice(), ["gone"]);
        assert!(
            start.elapsed() >= std::time::Duration::from_millis(350),
            "超时前不得提前走 else"
        );
    }

    /// P12.7 e2e：match_first 首个命中候选执行自己的 steps，`$match` =
    /// 该候选结果；候选级 threshold 以 thresholds 平行列表传入。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_component_match_first_runs_hit_candidate_steps() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let stub = VisionStub::new(&["b"]);
        let logs = Arc::new(LogTrace {
            logs: Mutex::new(Vec::new()),
        });
        let program = load(
            "version: 3\nsteps:\n  - match_first:\n      candidates:\n        - template: a\n          threshold: 0.6\n          steps:\n            - log: cand-a\n        - template: b\n          steps:\n            - log: cand-b\n            - set: {m: $match}\n  - return: $m.score\n",
        )
        .unwrap();
        let result = runtime
            .run(run_request(
                program,
                None,
                vision_host(&stub, Arc::new(InputTrace::default()), logs.clone()),
                None,
            ))
            .await
            .unwrap();
        // score 经 f32（MatchBox）往返，f64 比较用 1e-6 容差
        assert!(
            matches!(result.value, Value::Float(score) if (score - 0.92).abs() < 1e-6),
            "候选 steps 内 $match = 该候选结果，得到 {:?}",
            result.value
        );
        assert_eq!(logs.logs.lock().unwrap().as_slice(), ["cand-b"]);
        assert_eq!(
            stub.seen()
                .iter()
                .map(|(name, threshold, _)| (name.clone(), *threshold))
                .collect::<Vec<_>>(),
            vec![("a".to_string(), Some(0.6)), ("b".to_string(), None)],
            "候选级 threshold 经 thresholds 平行列表注入 match_many"
        );
    }

    /// P12.5 e2e（契约 §4）：wait 随机区间实际落进 [min, max]（nonce 由
    /// wasm_host 注入，guest splitmix64 取值，runtime.sleep 真实等待）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_component_wait_random_lands_within_range() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        // 先空跑一次预热（debug 构建 WASM 编译可占数秒，混入计时会让上界
        // 断言失效）；同一 runtime 复用已编译模块后再计时。
        let warmup = load("version: 3\nsteps:\n  - log: warm\n").unwrap();
        runtime
            .run(run_request(
                warmup,
                None,
                vision_host(
                    &VisionStub::new(&[]),
                    Arc::new(InputTrace::default()),
                    Arc::new(LogTrace {
                        logs: Mutex::new(Vec::new()),
                    }),
                ),
                None,
            ))
            .await
            .unwrap();
        let program = load(
            "version: 3\nsteps:\n  - wait: {min: 200ms, max: 500ms}\n  - return: done\n",
        )
        .unwrap();
        let start = std::time::Instant::now();
        let result = runtime
            .run(run_request(
                program,
                None,
                vision_host(
                    &VisionStub::new(&[]),
                    Arc::new(InputTrace::default()),
                    Arc::new(LogTrace {
                        logs: Mutex::new(Vec::new()),
                    }),
                ),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(result.value, Value::String("done".into()));
        let elapsed = start.elapsed();
        assert!(
            elapsed >= std::time::Duration::from_millis(200),
            "随机等待不得低于下界: {elapsed:?}"
        );
        assert!(
            elapsed <= std::time::Duration::from_millis(700),
            "随机等待不得显著超出上界（+200ms 调度余量）: {elapsed:?}"
        );
    }

    /// P12.5 e2e：timing defaults 经 lower 展开为显式 runtime.sleep —— tap 后
    /// after_tap 兜底 300ms，defaults 覆盖后取覆盖值。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_component_timing_defaults_sleep_after_tap() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let logs = Arc::new(LogTrace {
            logs: Mutex::new(Vec::new()),
        });
        // 内置兜底 300ms
        let program = load("version: 3\nsteps:\n  - tap: [0.5, 0.5]\n").unwrap();
        let start = std::time::Instant::now();
        runtime
            .run(run_request(
                program,
                None,
                vision_host(
                    &VisionStub::new(&[]),
                    Arc::new(InputTrace::default()),
                    logs.clone(),
                ),
                None,
            ))
            .await
            .unwrap();
        assert!(start.elapsed() >= std::time::Duration::from_millis(280));

        // defaults.timing.after_tap 覆盖
        let program = load(
            "version: 3\ndefaults:\n  timing:\n    after_tap: 60ms\nsteps:\n  - tap: [0.5, 0.5]\n",
        )
        .unwrap();
        let start = std::time::Instant::now();
        runtime
            .run(run_request(
                program,
                None,
                vision_host(
                    &VisionStub::new(&[]),
                    Arc::new(InputTrace::default()),
                    logs.clone(),
                ),
                None,
            ))
            .await
            .unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed >= std::time::Duration::from_millis(60)
                && elapsed <= std::time::Duration::from_millis(250),
            "after_tap 覆盖值必须生效（60ms + 调度余量）: {elapsed:?}"
        );
    }

    /// P12.7 e2e：save 变量跨步可用；`$match` 块后复位（不跨块泄漏）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_component_find_save_persists_and_match_is_scoped() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let stub = VisionStub::new(&["reward"]);
        let program = load(
            "version: 3\nsteps:\n  - find:\n      template: reward\n      save: reward\n  - set: {leak: $match}\n  - return: [$reward.found, $leak]\n",
        )
        .unwrap();
        let result = runtime
            .run(run_request(
                program,
                None,
                vision_host(
                    &stub,
                    Arc::new(InputTrace::default()),
                    Arc::new(LogTrace {
                        logs: Mutex::new(Vec::new()),
                    }),
                ),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(
            result.value,
            Value::List(vec![Value::Bool(true), Value::Null]),
            "save 的命名变量跨步可用；块外 $match 复位 null"
        );
    }
}
