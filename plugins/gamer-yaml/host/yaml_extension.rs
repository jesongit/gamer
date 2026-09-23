//! gamer-yaml 扩展边界（ADR-11/14，V1）。
//!
//! YAML 执行权威在 `yaml-interp` crate（WASM guest 与 server 测试同源，计划
//! Phase 2）。本模块职责：
//!
//! - 原生函数宿主（[`NativeYamlHost`]）：解释器 `__fn` 通道的后端——按
//!   [`native_funcs`] 注册表做 Schema 校验、权限检查与 Core capability 组合
//!   （tap/swipe/find/... 首版函数，计划 Phase 3.5）；
//! - WASM runtime 契约（[`YamlWasmRuntime`] / `NoYamlWasmRuntime`）与每 run
//!   请求/结果形态；
//! - 官方插件 manifest 参考常量（与 tools/plugins 打包源锁同步）。
//!
//! 依赖方向：本模块 → Core（capabilities / device / matcher）单向；Core 不得
//! import 本目录符号（架构守卫测试锁定）。

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde_json::{json, Map as JsonMap, Value};

use crate::capabilities::{
    AppId, CapabilityRegistry, DeviceHandle, FramePoint, FrameSize, KeyAction, KeyCode, KeyInput,
    LogLevel, LogRecord, MatchOptions, MatchOutcome, ResourceId, RuntimeService, SearchRegion,
    SwipeGesture, TemplateQuery, TextInput, TouchPoint,
};
use crate::core::events::{EventSink, RuntimeEvent, RuntimeEventKind};
use crate::core::AppContext;
use crate::extensions::gamer_yaml::native_funcs::{native_function, ParamSchema};
use crate::extensions::gamer_yaml::syntax::{parse_duration_ms, point_components, ParamType};
use crate::extensions::{HostApi, Permission};

pub(crate) const YAML_EXTENSION_ID: &str = "gamer-yaml";
/// 运行结构事件私有通道（guest → sink，先于权限校验拦截）。
pub(crate) const EVENT_CAPABILITY: &str = "__event";
/// 原生函数派发私有通道（guest → [`NativeYamlHost`]）。
pub(crate) const FN_CAPABILITY: &str = "__fn";
/// Reference manifest for the installable YAML guest. The server never embeds
/// its WASM bytes; package installation supplies `plugin.wasm` independently.
/// 仅测试引用：与 plugins/gamer-yaml/manifest.toml 的同步护栏 +
/// 安装/卸载面板测试以此为打包 manifest 源。
#[allow(dead_code)]
pub(crate) const YAML_EXTENSION_MANIFEST_TOML: &str = include_str!("../manifest.toml");

/// 官方市场打包源（plugins/gamer-yaml/manifest.toml）与本常量锁同步：
/// build-plugins.ps1 以文件为准打包，漂移会导致线上包与运行时语义不一致。
#[cfg(test)]
mod manifest_sync_tests {
    #[test]
    fn yaml_packaging_manifest_stays_in_sync_with_shipped_constant() {
        let packaged = include_str!("../manifest.toml");
        assert_eq!(
            super::YAML_EXTENSION_MANIFEST_TOML.trim(),
            packaged.trim(),
            "plugins/gamer-yaml/manifest.toml 与 YAML_EXTENSION_MANIFEST_TOML 不一致"
        );
    }
}

const DEFAULT_SCREEN_WIDTH: u32 = 1000;
const DEFAULT_SCREEN_HEIGHT: u32 = 1000;

/// find/tap_template 轮询间隔下限（防止 0 间隔打爆设备）。
const MIN_POLL_INTERVAL_MS: u64 = 50;
/// 单次 sleep 上限（与 v3 一致）。
const MAX_SLEEP_MS: u64 = 3_600_000;

// ---------------------------------------------------------------------------
// WASM runtime 契约
// ---------------------------------------------------------------------------

/// 一次 YAML 运行的请求：`program` 是宿主 lowering 产出的解释器 wire JSON
/// （含冻结的 Package 函数表、绑定参数与可选 start_index）。
pub(crate) struct YamlWasmRunRequest {
    pub(crate) wasm: Vec<u8>,
    pub(crate) program: Value,
    pub(crate) host: HostApi,
    pub(crate) context: AppContext,
    pub(crate) stop: Arc<AtomicBool>,
    /// 运行可视化事件汇（`__event` 私有通道拦截 + 宿主侧 vision/input 补发）；
    /// `None` = 静默。
    pub(crate) sink: Option<Arc<dyn EventSink>>,
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

// ---------------------------------------------------------------------------
// 原生函数宿主
// ---------------------------------------------------------------------------

/// 原生函数执行宿主：`__fn` 通道后端。每个 run 创建一个实例（持有设备句柄
/// 与坐标系），解释器的函数调用经 wasm_host 转发到这里。
///
/// 权限：每个函数声明所需权限，派发前逐项 `HostApi::authorize`——函数调用
/// 不能绕过插件权限（计划 Phase 3.2）。
pub(crate) struct NativeYamlHost {
    settings: super::settings::Settings,
    host: HostApi,
    registry: CapabilityRegistry,
    context: AppContext,
    device: DeviceHandle,
    runtime: Arc<dyn RuntimeService>,
    /// 设备坐标系（相对坐标 ⇄ 像素）：capture 后以真实帧分辨率刷新。
    screen: RwLock<FrameSize>,
    sink: Option<Arc<dyn EventSink>>,
}

/// 按函数 Schema 校验/规整后的参数视图。
struct BoundArgs {
    values: JsonMap<String, Value>,
}

impl BoundArgs {
    fn point(&self, name: &str) -> Result<[f64; 2]> {
        point_components(
            self.values
                .get(name)
                .ok_or_else(|| anyhow!("缺少参数 {name}"))?,
        )
        .ok_or_else(|| anyhow!("参数 {name} 不是合法 point（0..1 相对坐标）"))
    }

    fn duration_ms(&self, name: &str) -> Result<u64> {
        let value = self
            .values
            .get(name)
            .ok_or_else(|| anyhow!("缺少参数 {name}"))?;
        let ms = match value {
            Value::Number(number) => number.as_f64(),
            Value::String(text) => parse_duration_ms(text),
            _ => None,
        };
        ms.map(|value| value.round().max(0.0) as u64)
            .ok_or_else(|| anyhow!("参数 {name} 不是合法 duration"))
    }

    fn string(&self, name: &str) -> Result<String> {
        self.values
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("参数 {name} 必须是字符串"))
    }

    fn opt_string(&self, name: &str) -> Result<Option<String>> {
        match self.values.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(other) => Err(anyhow!("参数 {name} 必须是字符串，得到 {other}")),
        }
    }

    fn number(&self, name: &str) -> Result<Option<f64>> {
        match self.values.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(number)) => number
                .as_f64()
                .map(Some)
                .ok_or_else(|| anyhow!("参数 {name} 非有限数字")),
            Some(other) => Err(anyhow!("参数 {name} 必须是数字，得到 {other}")),
        }
    }

    fn region(&self, name: &str) -> Result<Option<[f64; 4]>> {
        let Some(value) = self.values.get(name) else {
            return Ok(None);
        };
        if value.is_null() {
            return Ok(None);
        }
        let items = value
            .as_array()
            .ok_or_else(|| anyhow!("region 必须是 [x, y, w, h] 相对坐标数组"))?;
        if items.len() != 4 {
            bail!("region 必须是四元数组 [x, y, w, h]");
        }
        let mut out = [0f64; 4];
        for (index, item) in items.iter().enumerate() {
            let component = item
                .as_f64()
                .ok_or_else(|| anyhow!("region 分量必须是数字"))?;
            if !(0.0..=1.0).contains(&component) {
                bail!("region 分量必须在 0..1（相对坐标），得到 {component}");
            }
            out[index] = component;
        }
        Ok(Some(out))
    }
}

/// 数值比较：JSON 数字统一按 f64 比较（整型/浮点互通）。
fn numeric_pair(a: &Value, b: &Value) -> Option<(f64, f64)> {
    Some((a.as_f64()?, b.as_f64()?))
}

fn json_equals(a: &Value, b: &Value) -> bool {
    if a == b {
        return true;
    }
    match (a, b) {
        (Value::Number(_), Value::Number(_)) => a.as_f64() == b.as_f64(),
        _ => false,
    }
}

fn message_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

impl NativeYamlHost {
    /// WASM guest 的 `__fn` 后端入口（wasm_host 调用）；无 wasm-runtime
    /// feature 时仅测试使用。
    #[cfg_attr(not(feature = "wasm-runtime"), allow(dead_code))]
    pub(crate) async fn call_function_json(
        host: HostApi,
        context: AppContext,
        stop: Arc<AtomicBool>,
        sink: Option<Arc<dyn EventSink>>,
        name: &str,
        args_json: &str,
        settings: super::settings::Settings,
    ) -> Result<Value> {
        let args: Value = serde_json::from_str(args_json)
            .map_err(|error| anyhow!("函数 {name} 参数不是合法 JSON: {error}"))?;
        let mut host = Self::new(host, context, stop, sink).await?;
        host.settings = settings;
        // 录制输入来源标注：guest 实例线程内执行点（task-local 不跨线程），
        // 在此线程内把 YAML runner 注入的输入标为 "runner"。
        crate::capabilities::adapters::with_caller_input_source("runner", async {
            host.call_function(name, args).await
        })
        .await
    }

    pub(crate) async fn new(
        host: HostApi,
        context: AppContext,
        stop: Arc<AtomicBool>,
        sink: Option<Arc<dyn EventSink>>,
    ) -> Result<Self> {
        let registry = host.registry().clone();
        let device_service = registry
            .device()
            .ok_or_else(|| anyhow!("device capability 未注册"))?;
        let device = device_service
            .resolve(&crate::capabilities::DeviceId::new(
                context.device_id.as_str(),
            ))
            .await
            .map_err(anyhow::Error::new)?;
        Ok(Self {
            settings: super::settings::Settings::default(),
            host,
            registry,
            context,
            device,
            runtime: Arc::new(crate::capabilities::adapters::RuntimeAdapter::new(stop)),
            screen: RwLock::new(FrameSize::new(DEFAULT_SCREEN_WIDTH, DEFAULT_SCREEN_HEIGHT)),
            sink,
        })
    }

    /// 函数派发：查注册表 → 权限 → Schema 绑定 → handler。
    pub(crate) async fn call_function(&self, name: &str, args: Value) -> Result<Value> {
        let Some(func) = native_function(name) else {
            bail!("未知函数: {name}");
        };
        for permission in func.permissions {
            self.host
                .authorize(*permission)
                .map_err(anyhow::Error::new)?;
        }
        let bound = self.bind_args(&func.params, args)?;
        self.emit_event(RuntimeEventKind::Detail {
            name: "effective_args".into(),
            data: json!({"function":name,"args":bound.values}),
        })
        .await;
        match name {
            "tap" => self.tap(&bound).await,
            "swipe" => self.swipe(&bound).await,
            "key" => self.key(&bound).await,
            "input_text" => self.input_text(&bound).await,
            "launch" => self.launch(&bound).await,
            "stop_app" => self.stop_app(&bound).await,
            "sleep" => self.sleep(&bound).await,
            "log" => self.log(&bound).await,
            "find" => self.find(&bound).await,
            "find_any" => self.find_any(&bound).await,
            "wait_find" => self.wait_find(&bound).await,
            "tap_template" => self.tap_template(&bound).await,
            "wait_disappear" => self.wait_disappear(&bound).await,
            "eq" => self.compare(&bound, CompareOp::Eq),
            "ne" => self.compare(&bound, CompareOp::Ne),
            "gt" => self.compare(&bound, CompareOp::Gt),
            "ge" => self.compare(&bound, CompareOp::Ge),
            "lt" => self.compare(&bound, CompareOp::Lt),
            "le" => self.compare(&bound, CompareOp::Le),
            other => bail!("函数 {other} 未实现"),
        }
    }

    /// Schema 绑定：未知参数 / 缺必填 / 类型不符均结构化报错。
    /// 非对象实参 = 位置简写（计划 §1.3，如 `- tap: [0.5, 0.8]`、
    /// `- log: 未进入主页`），绑定到第一个参数。
    fn bind_args(&self, schema: &[ParamSchema], args: Value) -> Result<BoundArgs> {
        let args = match args {
            Value::Null => JsonMap::new(),
            Value::Object(map) => map,
            other => {
                let Some(param) = schema.first() else {
                    bail!("函数参数必须是命名参数对象，得到 {other}");
                };
                JsonMap::from_iter([(param.name.to_string(), other)])
            }
        };
        let mut values = JsonMap::new();
        for param in schema {
            match args.get(param.name) {
                None => {
                    if let Some(default) = &param.default {
                        values.insert(param.name.to_string(), default.clone());
                    } else if param.required {
                        bail!("缺少必填参数 {}", param.name);
                    }
                }
                Some(value) => {
                    check_schema_type(param, value)?;
                    if let Some(item_type) = &param.item_type {
                        for item in value
                            .as_array()
                            .ok_or_else(|| anyhow!("{} 必须是列表", param.name))?
                        {
                            check_schema_type(
                                &ParamSchema {
                                    name: param.name,
                                    ty: *item_type,
                                    required: true,
                                    default: None,
                                    desc: param.desc,
                                    item_type: None,
                                },
                                item,
                            )?;
                        }
                    }
                    values.insert(param.name.to_string(), value.clone());
                }
            }
        }
        for name in args.keys() {
            if !schema.iter().any(|param| param.name == name) {
                bail!("未知参数 {name}");
            }
        }
        Ok(BoundArgs { values })
    }

    // -- handlers ----------------------------------------------------------

    async fn tap(&self, args: &BoundArgs) -> Result<Value> {
        let point = self.touch_point(args.point("position")?)?;
        self.click_point(point).await?;
        Ok(Value::Null)
    }

    /// All automation clicks pass here once; manual input keeps its existing behavior.
    async fn click_point(&self, point: TouchPoint) -> Result<()> {
        self.click_delay("before", self.settings.before_click_ms)
            .await?;
        self.registry
            .input()
            .ok_or_else(|| anyhow!("input capability 未注册"))?
            .tap(&self.device, point)
            .await
            .map_err(anyhow::Error::new)?;
        self.emit_event(RuntimeEventKind::Tap {
            x: point.x(),
            y: point.y(),
        })
        .await;
        self.click_delay("after", self.settings.after_click_ms)
            .await
    }

    async fn click_delay(&self, phase: &str, duration_ms: u64) -> Result<()> {
        if self.runtime.cancelled() {
            bail!("CANCELLED: 运行已取消");
        }
        if duration_ms > 0 {
            self.emit_event(RuntimeEventKind::Detail {
                name: "click_delay".into(),
                data: json!({"phase":phase,"duration_ms":duration_ms}),
            })
            .await;
        }
        let mut remaining = duration_ms;
        while remaining > 0 {
            let slice = remaining.min(50);
            self.runtime
                .sleep(Duration::from_millis(slice))
                .await
                .map_err(anyhow::Error::new)?;
            remaining -= slice;
        }
        if self.runtime.cancelled() {
            bail!("CANCELLED: 运行已取消");
        }
        Ok(())
    }

    async fn swipe(&self, args: &BoundArgs) -> Result<Value> {
        let from = self.touch_point(args.point("from")?)?;
        let to = self.touch_point(args.point("to")?)?;
        let duration = args.duration_ms("duration")?;
        self.registry
            .input()
            .ok_or_else(|| anyhow!("input capability 未注册"))?
            .swipe(
                &self.device,
                SwipeGesture::new(from, to, Duration::from_millis(duration)),
            )
            .await
            .map_err(anyhow::Error::new)?;
        self.emit_event(RuntimeEventKind::Swipe {
            x1: from.x(),
            y1: from.y(),
            x2: to.x(),
            y2: to.y(),
        })
        .await;
        Ok(Value::Null)
    }

    async fn key(&self, args: &BoundArgs) -> Result<Value> {
        let key = args.string("key")?;
        let code = key_code(&Value::String(key))?;
        let action = match args.opt_string("action")?.unwrap_or_else(|| "press".into()) {
            value if value == "down" => KeyAction::Down,
            value if value == "up" => KeyAction::Up,
            value if value == "press" => KeyAction::Press,
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

    async fn input_text(&self, args: &BoundArgs) -> Result<Value> {
        let text = args.string("text")?;
        self.registry
            .input()
            .ok_or_else(|| anyhow!("input capability 未注册"))?
            .text(&self.device, TextInput::new(&text))
            .await
            .map_err(anyhow::Error::new)?;
        Ok(Value::Null)
    }

    async fn launch(&self, args: &BoundArgs) -> Result<Value> {
        let package = self.package(args)?;
        self.registry
            .device()
            .ok_or_else(|| anyhow!("device capability 未注册"))?
            .start_app(&self.device, &AppId::new(format!("+{package}")))
            .await
            .map_err(anyhow::Error::new)?;
        Ok(Value::Null)
    }

    async fn stop_app(&self, args: &BoundArgs) -> Result<Value> {
        let package = self.package(args)?;
        self.registry
            .device()
            .ok_or_else(|| anyhow!("device capability 未注册"))?
            .stop_app(&self.device, &AppId::new(package))
            .await
            .map_err(anyhow::Error::new)?;
        Ok(Value::Null)
    }

    async fn sleep(&self, args: &BoundArgs) -> Result<Value> {
        let duration = args.duration_ms("duration")?.min(MAX_SLEEP_MS);
        self.runtime
            .sleep(Duration::from_millis(duration))
            .await
            .map_err(anyhow::Error::new)?;
        Ok(Value::Null)
    }

    async fn log(&self, args: &BoundArgs) -> Result<Value> {
        let level = match args.opt_string("level")?.unwrap_or_else(|| "info".into()) {
            value if value == "trace" => LogLevel::Trace,
            value if value == "debug" => LogLevel::Debug,
            value if value == "info" => LogLevel::Info,
            value if value == "warn" || value == "warning" => LogLevel::Warn,
            value if value == "error" => LogLevel::Error,
            other => bail!("未知 log level: {other}"),
        };
        let message = args
            .values
            .get("message")
            .map(message_to_text)
            .ok_or_else(|| anyhow!("缺少必填参数 message"))?;
        self.registry
            .log()
            .ok_or_else(|| anyhow!("log capability 未注册"))?
            .write(LogRecord::new(level, &message).with_device(self.device.clone()))
            .map_err(anyhow::Error::new)?;
        self.emit_event(RuntimeEventKind::Detail { name: "log".into(), data: serde_json::json!({"level": args.opt_string("level")?.unwrap_or_else(|| "info".into()), "message": message}) }).await;
        Ok(Value::Null)
    }

    /// find：单次模板匹配（简化计划 Phase 4.2：不再承担轮询语义）；未找到
    /// 返回 null。等待轮询请用 wait_find。每次尝试发 vision/hit/miss 事件
    /// （与 wait_find 同口径）。
    async fn find(&self, args: &BoundArgs) -> Result<Value> {
        let template_name = args.string("template")?;
        let (outcome, effective_px) = self.match_once(args).await?;
        let region =
            Self::relative_region_echo(effective_px, self.screen().width, self.screen().height);
        self.emit_vision_outcome(&template_name, outcome, effective_px)
            .await;
        Ok(match outcome {
            MatchOutcome::Found(_) => Self::match_value(outcome, region, self.screen()),
            MatchOutcome::NotFound => Value::Null,
        })
    }

    async fn wait_find(&self, args: &BoundArgs) -> Result<Value> {
        let click = args.values.get("click").and_then(Value::as_bool) == Some(true);
        if click
            || args
                .values
                .get("obstacles")
                .and_then(Value::as_array)
                .is_some_and(|v| !v.is_empty())
        {
            // 仅等待不需要输入权限；自动点击在开始轮询前校验。
            self.host
                .authorize(Permission::InputTap)
                .map_err(anyhow::Error::new)?;
        }
        let timeout = args.duration_ms("timeout")?;
        let Some(matched) = self.poll_match(args, timeout, "wait_find").await? else {
            return Ok(Value::Null);
        };
        if click {
            self.tap_match(&matched).await?;
        }
        Ok(matched)
    }

    async fn tap_template(&self, args: &BoundArgs) -> Result<Value> {
        let timeout = args.duration_ms("timeout")?;
        let Some(matched) = self.poll_match(args, timeout, "tap_template").await? else {
            return Ok(Value::Null);
        };
        self.tap_match(&matched).await?;
        Ok(matched)
    }

    async fn tap_match(&self, matched: &Value) -> Result<()> {
        let Some(center) = matched.get("center").and_then(point_components) else {
            bail!("模板匹配结果缺少 center");
        };
        self.click_point(self.touch_point(center)?).await
    }

    async fn wait_disappear(&self, args: &BoundArgs) -> Result<Value> {
        let timeout = args.duration_ms("timeout")?;
        let interval = args.duration_ms("interval")?.max(MIN_POLL_INTERVAL_MS);
        let started = Instant::now();
        loop {
            let (outcome, _) = self.match_once(args).await?;
            let found = matches!(outcome, MatchOutcome::Found(_));
            if !found {
                return Ok(Value::Bool(true));
            }
            if started.elapsed().as_millis() as u64 >= timeout {
                return Ok(Value::Bool(false));
            }
            self.runtime
                .sleep(Duration::from_millis(interval.min(MAX_SLEEP_MS)))
                .await
                .map_err(anyhow::Error::new)?;
        }
    }

    fn compare(&self, args: &BoundArgs, op: CompareOp) -> Result<Value> {
        let a = args
            .values
            .get("a")
            .cloned()
            .ok_or_else(|| anyhow!("缺少必填参数 a"))?;
        let b = args
            .values
            .get("b")
            .cloned()
            .ok_or_else(|| anyhow!("缺少必填参数 b"))?;
        let result = match op {
            CompareOp::Eq => json_equals(&a, &b),
            CompareOp::Ne => !json_equals(&a, &b),
            CompareOp::Gt => numeric_pair(&a, &b).is_some_and(|(a, b)| a > b),
            CompareOp::Ge => numeric_pair(&a, &b).is_some_and(|(a, b)| a >= b),
            CompareOp::Lt => numeric_pair(&a, &b).is_some_and(|(a, b)| a < b),
            CompareOp::Le => numeric_pair(&a, &b).is_some_and(|(a, b)| a <= b),
        };
        Ok(Value::Bool(result))
    }

    // -- 视觉轮询 -----------------------------------------------------------

    /// 单次截图匹配（不发事件）；返回结果与本次生效的像素搜索区域。
    async fn match_once(&self, args: &BoundArgs) -> Result<(MatchOutcome, Option<[u32; 4]>)> {
        let frame = self.capture().await?;
        self.match_on_frame(args, frame).await
    }

    async fn match_on_frame(
        &self,
        args: &BoundArgs,
        frame: crate::capabilities::FrameHandle,
    ) -> Result<(MatchOutcome, Option<[u32; 4]>)> {
        let template_name = args.string("template")?;
        let template = self.template(&template_name).await?;
        let threshold = args.number("threshold")?;
        let explicit_px = args
            .region("region")?
            .map(|region| self.pixel_region(region));
        let template_file = self.template_file_name(&template).await;
        let effective_px = crate::matcher::effective_search_region(
            explicit_px,
            template_file.as_deref(),
            self.screen().width,
            self.screen().height,
        );
        let options = MatchOptions {
            threshold: threshold.map(|value| value as f32),
            region: effective_px
                .map(|[x, y, width, height]| SearchRegion::new(x, y, width, height)),
            color_check: false,
        };
        let outcome = self
            .registry
            .vision()
            .ok_or_else(|| anyhow!("vision capability 未注册"))?
            .match_template(frame, TemplateQuery::new(template, options))
            .await
            .map_err(anyhow::Error::new)?;
        let _ = template_name;
        Ok((outcome, effective_px))
    }

    /// 顺序匹配共用同一帧，首个命中即返回；不产生输入操作。
    async fn find_any(&self, args: &BoundArgs) -> Result<Value> {
        let templates = args
            .values
            .get("templates")
            .and_then(Value::as_array)
            .expect("Schema validated list");
        if templates.is_empty() || templates.len() > 64 {
            bail!("templates 必须包含 1..64 项");
        }
        if !args
            .number("threshold")?
            .is_some_and(|v| (0.0..=1.0).contains(&v))
        {
            bail!("threshold 必须为 0..1 数字");
        }
        let frame = self.capture().await?;
        for (index, template) in templates.iter().enumerate() {
            if self.runtime.cancelled() {
                bail!("CANCELLED");
            }
            let mut values = args.values.clone();
            values.insert("template".into(), template.clone());
            let (outcome, effective_px) = self.match_on_frame(&BoundArgs { values }, frame).await?;
            self.emit_vision_outcome(
                template.as_str().expect("Schema validated template"),
                outcome,
                effective_px,
            )
            .await;
            if matches!(outcome, MatchOutcome::Found(_)) {
                let region = Self::relative_region_echo(
                    effective_px,
                    self.screen().width,
                    self.screen().height,
                );
                let mut result = Self::match_value(outcome, region, self.screen());
                result["index"] = json!(index);
                result["template"] = template.clone();
                return Ok(result);
            }
        }
        Ok(Value::Null)
    }

    /// wait_find/tap_template 共用轮询及 match_on_frame 匹配语义。
    async fn poll_match(
        &self,
        args: &BoundArgs,
        timeout_ms: u64,
        _fn_name: &str,
    ) -> Result<Option<Value>> {
        let interval = args.duration_ms("interval")?.max(MIN_POLL_INTERVAL_MS);
        let template_name = args.string("template")?;
        let started = Instant::now();
        let obstacles = args
            .values
            .get("obstacles")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        loop {
            if self.runtime.cancelled() {
                bail!("CANCELLED");
            }
            let frame = self.capture().await?;
            let mut cleared = false;
            for obstacle in &obstacles {
                let mut values = args.values.clone();
                values.insert("template".into(), obstacle.clone());
                values.remove("region"); // 障碍按自己的模板区域搜索，不继承目标区域。
                let obstacle_args = BoundArgs { values };
                let (outcome, effective_px) = self.match_on_frame(&obstacle_args, frame).await?;
                self.emit_vision_outcome(
                    obstacle.as_str().expect("Schema 已校验模板列表"),
                    outcome,
                    effective_px,
                )
                .await;
                if matches!(outcome, MatchOutcome::Found(_)) {
                    let region = Self::relative_region_echo(
                        effective_px,
                        self.screen().width,
                        self.screen().height,
                    );
                    self.tap_match(&Self::match_value(outcome, region, self.screen()))
                        .await?;
                    cleared = true;
                    break;
                }
                if timeout_ms > 0 && started.elapsed().as_millis() as u64 >= timeout_ms {
                    return Ok(None);
                }
                if self.runtime.cancelled() {
                    bail!("CANCELLED");
                }
            }
            let (outcome, effective_px) = if cleared {
                (MatchOutcome::NotFound, None)
            } else {
                self.match_on_frame(args, frame).await?
            };
            let region =
                Self::relative_region_echo(effective_px, self.screen().width, self.screen().height);
            if !cleared {
                self.emit_vision_outcome(&template_name, outcome, effective_px)
                    .await;
            }
            if let MatchOutcome::Found(_) = outcome {
                return Ok(Some(Self::match_value(outcome, region, self.screen())));
            }
            if started.elapsed().as_millis() as u64 >= timeout_ms {
                return Ok(None);
            }
            self.runtime
                .sleep(Duration::from_millis(interval.min(MAX_SLEEP_MS).min(
                    timeout_ms.saturating_sub(started.elapsed().as_millis() as u64),
                )))
                .await
                .map_err(anyhow::Error::new)?;
            if started.elapsed().as_millis() as u64 >= timeout_ms {
                return Ok(None);
            }
        }
    }

    // -- 共用辅助 -----------------------------------------------------------

    fn package(&self, args: &BoundArgs) -> Result<String> {
        match args.opt_string("package")? {
            Some(value) if !value.trim().is_empty() => Ok(value),
            _ => Ok(self.context.android_package.as_str().to_string()),
        }
    }

    fn touch_point(&self, relative: [f64; 2]) -> Result<TouchPoint> {
        let [x, y] = relative;
        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
            bail!("point 坐标超出 0..1");
        }
        Ok(TouchPoint::new(
            (x * self.screen().width as f64).round() as u32,
            (y * self.screen().height as f64).round() as u32,
            1.0,
        ))
    }

    fn pixel_region(&self, relative: [f64; 4]) -> [u32; 4] {
        let [x, y, width, height] = relative;
        let screen = self.screen();
        [
            (x * screen.width as f64).round() as u32,
            (y * screen.height as f64).round() as u32,
            (width * screen.width as f64).round() as u32,
            (height * screen.height as f64).round() as u32,
        ]
    }

    async fn template(&self, name: &str) -> Result<crate::capabilities::ResourceHandle> {
        self.host
            .authorize(Permission::ResourceRead)
            .map_err(anyhow::Error::new)?;
        let package = self
            .context
            .content_package
            .as_ref()
            .ok_or_else(|| anyhow!("当前上下文没有 content package"))?;
        let resource = self
            .registry
            .resource()
            .ok_or_else(|| anyhow!("resource capability 未注册"))?
            .resolve(
                &ResourceId::new(
                    package.as_str().to_string(),
                    YAML_EXTENSION_ID,
                    format!("templates/{name}"),
                )
                .map_err(anyhow::Error::new)?,
            )
            .await
            .map_err(anyhow::Error::new)?;
        Ok(resource)
    }

    async fn capture(&self) -> Result<crate::capabilities::FrameHandle> {
        let frame = self
            .registry
            .frame()
            .ok_or_else(|| anyhow!("frame capability 未注册"))?
            .capture(&self.device)
            .await
            .map_err(anyhow::Error::new)?;
        self.refresh_screen(&frame).await;
        Ok(frame)
    }

    /// 以最近一次截图的真实分辨率刷新坐标系（失败保持上次值）。
    async fn refresh_screen(&self, frame: &crate::capabilities::FrameHandle) {
        let Some(frame_service) = self.registry.frame() else {
            return;
        };
        if let Ok(size) = frame_service.size(*frame).await {
            if size.width > 0 && size.height > 0 {
                *self.screen.write().unwrap() = size;
            }
        }
    }

    fn screen(&self) -> FrameSize {
        *self.screen.read().unwrap()
    }

    /// 模板 handle → 解析后的实际文件名（`#` 后缀区域推断用；失败 = None）。
    async fn template_file_name(
        &self,
        handle: &crate::capabilities::ResourceHandle,
    ) -> Option<String> {
        self.registry
            .resource()?
            .resolved_file_name(*handle)
            .await
            .ok()
    }

    fn match_value(outcome: MatchOutcome, region: Value, screen: FrameSize) -> Value {
        match outcome {
            MatchOutcome::Found(found) => {
                let center = [
                    (found.x + found.width / 2) as f64 / screen.width as f64,
                    (found.y + found.height / 2) as f64 / screen.height as f64,
                ];
                json!({
                    "x": found.x,
                    "y": found.y,
                    "width": found.width,
                    "height": found.height,
                    "score": found.score,
                    "center": { "x": center[0], "y": center[1] },
                    "region": region,
                })
            }
            MatchOutcome::NotFound => Value::Null,
        }
    }

    /// 本次实际搜索区域的回显值（相对坐标 map）。
    fn relative_region_echo(px: Option<[u32; 4]>, w: u32, h: u32) -> Value {
        let (x, y, width, height) = match px {
            Some([x, y, width, height]) => (x, y, width, height),
            None => (0, 0, w, h),
        };
        json!({
            "x": x as f64 / w as f64,
            "y": y as f64 / h as f64,
            "width": width as f64 / w as f64,
            "height": height as f64 / h as f64,
        })
    }

    /// 尽力而为的事件旁路：发射失败只记 debug，不影响能力执行结果。
    async fn emit_event(&self, kind: RuntimeEventKind) {
        let Some(sink) = &self.sink else {
            return;
        };
        if let Err(error) = sink
            .emit(RuntimeEvent::new(self.context.device_id.clone(), kind))
            .await
        {
            tracing::debug!(%error, "yaml runtime event emit failed");
        }
    }

    /// vision 结果可视化旁路：`vision` 结构事件 + `hit`/`miss` 投屏标记。
    async fn emit_vision_outcome(
        &self,
        template: &str,
        outcome: MatchOutcome,
        region_px: Option<[u32; 4]>,
    ) {
        let region_px = region_px.unwrap_or([0, 0, self.screen().width, self.screen().height]);
        match outcome {
            MatchOutcome::Found(found) => {
                let center = [
                    (found.x + found.width / 2) as f64 / self.screen().width as f64,
                    (found.y + found.height / 2) as f64 / self.screen().height as f64,
                ];
                self.emit_event(RuntimeEventKind::Vision {
                    template: template.to_string(),
                    found: true,
                    score: Some(found.score),
                    center: Some(center),
                })
                .await;
                self.emit_event(RuntimeEventKind::Hit {
                    tpl: template.to_string(),
                    x: found.x,
                    y: found.y,
                    w: found.width,
                    h: found.height,
                    score: found.score,
                })
                .await;
            }
            MatchOutcome::NotFound => {
                self.emit_event(RuntimeEventKind::Vision {
                    template: template.to_string(),
                    found: false,
                    score: None,
                    center: None,
                })
                .await;
                self.emit_event(RuntimeEventKind::Miss {
                    tpl: template.to_string(),
                    x: region_px[0],
                    y: region_px[1],
                    w: region_px[2],
                    h: region_px[3],
                })
                .await;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum CompareOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

/// Schema 类型检查（绑定；值为规整前原文）。
fn check_schema_type(param: &ParamSchema, value: &Value) -> Result<()> {
    let ok = |condition: bool| {
        if condition {
            Ok(())
        } else {
            Err(anyhow!(
                "参数 {} 与类型 {} 不符，得到 {value}",
                param.name,
                param.ty.canonical()
            ))
        }
    };
    match param.ty {
        ParamType::Any => Ok(()),
        ParamType::Boolean => ok(value.is_boolean()),
        ParamType::Integer => ok(value.is_i64() || value.is_u64()),
        ParamType::Number => ok(value.is_number()),
        ParamType::String | ParamType::Template | ParamType::Key => {
            ok(value.as_str().is_some_and(|text| !text.trim().is_empty()))
        }
        ParamType::List => ok(value.is_array()),
        ParamType::Object => ok(value.is_object()),
        ParamType::Duration => ok(match value {
            Value::Number(number) => number.as_f64().is_some_and(|ms| ms >= 0.0),
            Value::String(text) => parse_duration_ms(text).is_some(),
            _ => false,
        }),
        ParamType::Point => ok(point_components(value).is_some()),
    }
}

fn key_code(value: &Value) -> Result<u32> {
    let text = value
        .as_str()
        .ok_or_else(|| anyhow!("key 必须是按键名字符串或数字字符串"))?;
    if let Ok(code) = text.parse::<u32>() {
        return Ok(code);
    }
    Ok(match text.to_ascii_uppercase().as_str() {
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::capabilities::{
        CapabilityResult, ColorSample, FrameHandle, FrameService, LogRecord, LogService,
        MatchManyRequest, MatchManyResult, ResourceHandle, ResourceId, ResourceLease,
        ResourceService, VisionService,
    };
    use crate::extensions::HostApiCatalog;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    /// device+input 记录桩。
    #[derive(Default)]
    pub(crate) struct Trace {
        pub(crate) text: Mutex<Vec<String>>,
        pub(crate) taps: Mutex<Vec<[u32; 2]>>,
        pub(crate) apps: Mutex<Vec<(String, String)>>,
        pub(crate) swipes: Mutex<Vec<SwipeGesture>>,
        pub(crate) keys: Mutex<Vec<KeyInput>>,
    }

    #[async_trait]
    impl crate::capabilities::DeviceService for Trace {
        async fn resolve(
            &self,
            id: &crate::capabilities::DeviceId,
        ) -> CapabilityResult<DeviceHandle> {
            Ok(DeviceHandle::new(id.clone()))
        }

        async fn start_app(&self, _: &DeviceHandle, app: &AppId) -> CapabilityResult<()> {
            self.apps
                .lock()
                .unwrap()
                .push(("launch".into(), app.as_str().into()));
            Ok(())
        }

        async fn stop_app(&self, _: &DeviceHandle, app: &AppId) -> CapabilityResult<()> {
            self.apps
                .lock()
                .unwrap()
                .push(("stop_app".into(), app.as_str().into()));
            Ok(())
        }
    }

    #[async_trait]
    impl crate::capabilities::InputService for Trace {
        async fn tap(&self, _: &DeviceHandle, point: TouchPoint) -> CapabilityResult<()> {
            self.taps.lock().unwrap().push([point.x(), point.y()]);
            Ok(())
        }

        async fn swipe(&self, _: &DeviceHandle, gesture: SwipeGesture) -> CapabilityResult<()> {
            self.swipes.lock().unwrap().push(gesture);
            Ok(())
        }

        async fn key(&self, _: &DeviceHandle, key: KeyInput) -> CapabilityResult<()> {
            self.keys.lock().unwrap().push(key);
            Ok(())
        }

        async fn text(&self, _: &DeviceHandle, value: TextInput) -> CapabilityResult<()> {
            self.text.lock().unwrap().push(value.as_str().to_string());
            Ok(())
        }
    }

    pub(crate) struct LogTrace {
        logs: Mutex<Vec<(String, String)>>,
        devices: Mutex<Vec<String>>,
    }

    impl LogTrace {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self {
                logs: Mutex::new(Vec::new()),
                devices: Mutex::new(Vec::new()),
            })
        }

        pub(crate) fn messages(&self) -> Vec<String> {
            self.logs
                .lock()
                .unwrap()
                .iter()
                .map(|(_, message)| message.clone())
                .collect()
        }
    }

    impl LogService for LogTrace {
        fn write(&self, record: LogRecord) -> CapabilityResult<()> {
            self.devices.lock().unwrap().push(
                record
                    .device()
                    .map(|d| d.id().as_str().to_owned())
                    .unwrap_or_default(),
            );
            self.logs.lock().unwrap().push((
                format!("{:?}", record.level()),
                record.message().to_string(),
            ));
            Ok(())
        }
    }

    /// frame+vision+resource 桩：按队列逐次返回匹配结果（缺省 NotFound）。
    pub(crate) struct VisionStub {
        pub(crate) size: FrameSize,
        pub(crate) outcomes: Mutex<VecDeque<MatchOutcome>>,
        pub(crate) match_calls: AtomicU64,
        frames: Mutex<Vec<FrameHandle>>,
        regions: Mutex<Vec<Option<SearchRegion>>>,
    }

    impl VisionStub {
        pub(crate) fn new(size: FrameSize) -> Arc<Self> {
            Arc::new(Self {
                size,
                outcomes: Mutex::new(VecDeque::new()),
                match_calls: AtomicU64::new(0),
                frames: Mutex::new(Vec::new()),
                regions: Mutex::new(Vec::new()),
            })
        }

        pub(crate) fn push_outcome(&self, outcome: MatchOutcome) {
            self.outcomes.lock().unwrap().push_back(outcome);
        }
    }

    #[async_trait]
    impl FrameService for VisionStub {
        async fn latest(&self, _device: &DeviceHandle) -> CapabilityResult<Option<FrameHandle>> {
            Ok(Some(FrameHandle::new()))
        }

        async fn capture(&self, _device: &DeviceHandle) -> CapabilityResult<FrameHandle> {
            Ok(FrameHandle::new())
        }

        async fn size(&self, _frame: FrameHandle) -> CapabilityResult<FrameSize> {
            Ok(self.size)
        }
    }

    pub(super) fn stub_outcome() -> MatchOutcome {
        MatchOutcome::Found(crate::capabilities::MatchBox {
            x: 10,
            y: 20,
            width: 200,
            height: 100,
            score: 0.93,
        })
    }

    #[async_trait]
    impl VisionService for VisionStub {
        async fn match_template(
            &self,
            _frame: FrameHandle,
            _template: TemplateQuery,
        ) -> CapabilityResult<MatchOutcome> {
            self.match_calls.fetch_add(1, Ordering::Relaxed);
            self.frames.lock().unwrap().push(_frame);
            self.regions
                .lock()
                .unwrap()
                .push(_template.options().region);
            Ok(self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(MatchOutcome::NotFound))
        }

        async fn match_many(
            &self,
            request: &MatchManyRequest,
        ) -> CapabilityResult<Vec<MatchManyResult>> {
            Ok(request
                .templates()
                .iter()
                .map(|query| MatchManyResult {
                    template: query.template(),
                    outcome: MatchOutcome::NotFound,
                })
                .collect())
        }

        async fn sample_color(
            &self,
            _frame: FrameHandle,
            _point: FramePoint,
        ) -> CapabilityResult<ColorSample> {
            Ok(ColorSample {
                red: 1,
                green: 2,
                blue: 3,
            })
        }
    }

    #[async_trait]
    impl ResourceService for VisionStub {
        async fn resolve(&self, _id: &ResourceId) -> CapabilityResult<ResourceHandle> {
            Ok(ResourceHandle::new())
        }

        async fn open(&self, resource: ResourceHandle) -> CapabilityResult<ResourceLease> {
            Ok(ResourceLease::new(resource, Some(0)))
        }

        async fn resolved_file_name(&self, _handle: ResourceHandle) -> CapabilityResult<String> {
            Ok("template.png".to_string())
        }
    }

    pub(crate) struct EventCollect {
        events: Mutex<Vec<Value>>,
    }

    impl EventCollect {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(Vec::new()),
            })
        }

        pub(crate) fn of(&self, ev: &str) -> Vec<Value> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event["ev"] == ev)
                .cloned()
                .collect()
        }
    }

    /// Core EventSink 适配：RuntimeEvent → `{"ev":…}` JSON（与 wasm_host 的
    /// `__event` 通道出参同形，测试断言词表一致）。
    impl crate::core::events::EventSink for EventCollect {
        fn emit(
            &self,
            event: crate::core::events::RuntimeEvent,
        ) -> futures_util::future::BoxFuture<'_, anyhow::Result<()>> {
            let mut payload = serde_json::to_value(event.kind).unwrap_or(Value::Null);
            if let Some(trace) = event.trace {
                payload["trace"] = trace;
            }
            self.events.lock().unwrap().push(payload);
            Box::pin(std::future::ready(Ok(())))
        }
    }

    pub(super) fn vision_host(
        trace: Arc<Trace>,
        stub: &Arc<VisionStub>,
        logs: Arc<LogTrace>,
        permissions: &[&str],
    ) -> HostApi {
        let permissions = permissions
            .iter()
            .map(|permission| format!("\"{permission}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let manifest = crate::extensions::parse_manifest(
            format!(
                r#"manifest_version = 2
id = "gamer-yaml"
version = "3.0.0"
name = "自动化"
entry = "plugin.wasm"
permissions = [{permissions}]
[host_api]
device = "^1.0"
vision = "^1.0"
input = "^1.0"
resource = "^1.0"
runtime = "^1.0"
log = "^1.0"
"#
            )
            .as_bytes(),
        )
        .unwrap();
        HostApi::for_manifest(
            CapabilityRegistry::builder()
                .with_device_service(trace.clone() as Arc<dyn crate::capabilities::DeviceService>)
                .with_input_service(trace as Arc<dyn crate::capabilities::InputService>)
                .with_frame_service(stub.clone() as Arc<dyn FrameService>)
                .with_vision_service(stub.clone() as Arc<dyn VisionService>)
                .with_resource_service(stub.clone() as Arc<dyn ResourceService>)
                .with_log_service(logs as Arc<dyn LogService>)
                .build(),
            HostApiCatalog::default(),
            &manifest,
        )
        .unwrap()
    }

    fn test_context() -> AppContext {
        AppContext::for_test("device-1", "com.example.game").unwrap()
    }

    fn call(name: &str, args: Value, host: &HostApi) -> Result<Value> {
        let sink: Option<Arc<dyn EventSink>> = None;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let host_impl = NativeYamlHost::new(
                host.clone(),
                test_context(),
                Arc::new(AtomicBool::new(false)),
                sink,
            )
            .await
            .unwrap();
            host_impl.call_function(name, args).await
        })
    }

    #[test]
    fn comparisons_and_pure_functions_work() {
        let trace = Arc::new(Trace::default());
        let stub = VisionStub::new(FrameSize::new(1000, 1000));
        let host = vision_host(
            trace,
            &stub,
            LogTrace::new(),
            &["vision.match", "resource.read"],
        );
        assert_eq!(
            call("eq", json!({"a": 1, "b": 1.0}), &host).unwrap(),
            Value::Bool(true),
            "整型/浮点数字相等"
        );
        assert_eq!(
            call("gt", json!({"a": 3, "b": 2}), &host).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            call("ne", json!({"a": "x", "b": "y"}), &host).unwrap(),
            Value::Bool(true)
        );
        let error = call("gt", json!({"a": "x", "b": 2}), &host).unwrap_err();
        assert!(error.to_string().contains("number"), "{error}");
    }

    #[test]
    fn schema_binding_rejects_unknown_missing_and_mistyped_args() {
        let trace = Arc::new(Trace::default());
        let stub = VisionStub::new(FrameSize::new(1000, 1000));
        let host = vision_host(trace, &stub, LogTrace::new(), &["input.tap"]);
        let error = call("tap", json!({}), &host).unwrap_err();
        assert!(error.to_string().contains("position"), "{error}");
        let error = call("tap", json!({"position": [0.5, 0.5], "nope": 1}), &host).unwrap_err();
        assert!(error.to_string().contains("未知参数 nope"), "{error}");
        let error = call("tap", json!({"position": [0.5, 5.0]}), &host).unwrap_err();
        assert!(error.to_string().contains("point"), "{error}");
    }

    #[test]
    fn permission_denial_surfaces_denied_error() {
        let trace = Arc::new(Trace::default());
        let stub = VisionStub::new(FrameSize::new(1000, 1000));
        let host = vision_host(trace, &stub, LogTrace::new(), &["device.read"]);
        let error = call("tap", json!({"position": [0.5, 0.5]}), &host).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("denied") || text.contains("权限"), "{text}");
    }

    #[test]
    fn find_returns_match_then_null_and_tap_template_clicks_center() {
        let trace = Arc::new(Trace::default());
        let stub = VisionStub::new(FrameSize::new(1000, 1000));
        stub.push_outcome(stub_outcome());
        let host = vision_host(
            trace.clone(),
            &stub,
            LogTrace::new(),
            &["vision.match", "resource.read", "input.tap"],
        );
        let matched = call("find", json!({"template": "home"}), &host).unwrap();
        assert_eq!(matched["center"]["x"], 0.11, "中心 = (10+100)/1000");
        assert_eq!(matched["center"]["y"], 0.07, "中心 = (20+50)/1000");
        assert!(
            matched["score"].as_f64().unwrap() > 0.9,
            "score = {}（f32 经 wire 放大）",
            matched["score"]
        );

        let miss = call("find", json!({"template": "home"}), &host).unwrap();
        assert_eq!(miss, Value::Null);

        // find 是单次匹配：timeout/interval 已从 Schema 删除（等待轮询用 wait_find）
        let error = call("find", json!({"template": "home", "timeout": "1s"}), &host).unwrap_err();
        assert!(error.to_string().contains("未知参数 timeout"), "{error}");

        stub.push_outcome(stub_outcome());
        let matched = call(
            "tap_template",
            json!({"template": "home", "timeout": "0ms"}),
            &host,
        )
        .unwrap();
        assert_eq!(matched["center"]["x"], 0.11);
        let taps = trace.taps.lock().unwrap();
        assert_eq!(taps.len(), 1);
        assert_eq!(taps[0], [110, 70], "点击像素坐标 = center×屏");

        let disappeared = call(
            "wait_disappear",
            json!({"template": "home", "timeout": "100ms", "interval": "50ms"}),
            &host,
        )
        .unwrap();
        assert_eq!(disappeared, Value::Bool(true));
    }

    /// find 只执行一次；wait_find 才轮询，并在 timeout 到达后返回 null。
    #[test]
    fn wait_find_polls_until_match_and_times_out_without_match() {
        let trace = Arc::new(Trace::default());
        let stub = VisionStub::new(FrameSize::new(1000, 1000));
        stub.push_outcome(MatchOutcome::NotFound);
        stub.push_outcome(stub_outcome());
        let host = vision_host(
            trace,
            &stub,
            LogTrace::new(),
            &["vision.match", "resource.read"],
        );
        let matched = call(
            "wait_find",
            json!({"template": "home", "click": false, "timeout": "100ms", "interval": "1ms"}),
            &host,
        )
        .unwrap();
        assert_eq!(matched["center"]["x"], 0.11);
        assert_eq!(stub.match_calls.load(Ordering::Relaxed), 2);

        let trace = Arc::new(Trace::default());
        let stub = VisionStub::new(FrameSize::new(1000, 1000));
        let host = vision_host(
            trace,
            &stub,
            LogTrace::new(),
            &["vision.match", "resource.read"],
        );
        let result = call(
            "wait_find",
            json!({"template": "home", "click": false, "timeout": "0ms", "interval": "1ms"}),
            &host,
        )
        .unwrap();
        assert_eq!(result, Value::Null, "超时未命中必须返回 null");
        assert_eq!(
            stub.match_calls.load(Ordering::Relaxed),
            1,
            "timeout=0 仍只尝试一次"
        );
    }

    #[test]
    fn wait_find_click_defaults_true_and_false_only_waits() {
        for click in [None, Some(true), Some(false)] {
            let trace = Arc::new(Trace::default());
            let stub = VisionStub::new(FrameSize::new(1000, 1000));
            stub.push_outcome(stub_outcome());
            let permissions = if click == Some(false) {
                vec!["vision.match", "resource.read"]
            } else {
                vec!["vision.match", "resource.read", "input.tap"]
            };
            let host = vision_host(trace.clone(), &stub, LogTrace::new(), &permissions);
            let mut args = json!({"template": "home", "timeout": "0ms"});
            if let Some(click) = click {
                args["click"] = json!(click);
            }
            let matched = call("wait_find", args.clone(), &host).unwrap();
            assert_eq!(matched["center"], json!({"x": 0.11, "y": 0.07}));
            let expected_taps = if click == Some(false) {
                vec![]
            } else {
                vec![[110, 70]]
            };
            assert_eq!(*trace.taps.lock().unwrap(), expected_taps);
            assert_eq!(call("wait_find", args, &host).unwrap(), Value::Null);
            assert_eq!(*trace.taps.lock().unwrap(), expected_taps, "未命中不能点击");
        }
    }

    #[test]
    fn wait_find_click_requires_permission_and_boolean_argument() {
        let trace = Arc::new(Trace::default());
        let stub = VisionStub::new(FrameSize::new(1000, 1000));
        let host = vision_host(
            trace.clone(),
            &stub,
            LogTrace::new(),
            &["vision.match", "resource.read"],
        );
        let error = call("wait_find", json!({"template": "home"}), &host).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("denied") || text.contains("权限"), "{text}");
        let error = call(
            "wait_find",
            json!({"template": "home", "click": "false"}),
            &host,
        )
        .unwrap_err();
        assert!(error.to_string().contains("boolean"), "{error}");
        assert_eq!(stub.match_calls.load(Ordering::Relaxed), 0);
        assert!(trace.taps.lock().unwrap().is_empty());
    }

    #[test]
    fn find_any_stops_at_first_hit_shares_frame_and_never_taps() {
        let trace = Arc::new(Trace::default());
        let stub = VisionStub::new(FrameSize::new(1000, 1000));
        stub.push_outcome(MatchOutcome::NotFound);
        stub.push_outcome(stub_outcome());
        stub.push_outcome(stub_outcome());
        let host = vision_host(
            trace.clone(),
            &stub,
            LogTrace::new(),
            &["vision.match", "resource.read"],
        );
        let matched = call(
            "find_any",
            json!({"templates":["notice", "login", "home"]}),
            &host,
        )
        .unwrap();
        assert_eq!(matched["index"], 1);
        assert_eq!(matched["template"], "login");
        assert!(trace.taps.lock().unwrap().is_empty());
        let frames = stub.frames.lock().unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], frames[1]);
        drop(frames);
        for invalid in [json!([]), json!([""]), json!([1]), json!("home")] {
            assert!(call("find_any", json!({"templates":invalid}), &host).is_err());
        }
        assert_eq!(stub.match_calls.load(Ordering::Relaxed), 2);
        stub.outcomes.lock().unwrap().clear();
        assert_eq!(
            call("find_any", json!({"templates":["missing"]}), &host).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn wait_find_obstacles_click_first_then_refresh_and_share_next_frame() {
        let trace = Arc::new(Trace::default());
        let stub = VisionStub::new(FrameSize::new(1000, 1000));
        for outcome in [
            stub_outcome(),
            MatchOutcome::NotFound,
            MatchOutcome::NotFound,
            stub_outcome(),
        ] {
            stub.push_outcome(outcome);
        }
        let host = vision_host(
            trace.clone(),
            &stub,
            LogTrace::new(),
            &["vision.match", "resource.read", "input.tap"],
        );
        let matched = call(
            "wait_find",
            json!({"template":"home", "obstacles":["close", "confirm"],
            "click":false, "timeout":"2s", "interval":"50ms", "region":[0.5,0.5,0.5,0.5]}),
            &host,
        )
        .unwrap();
        assert_eq!(matched["center"], json!({"x":0.11,"y":0.07}));
        assert_eq!(
            *trace.taps.lock().unwrap(),
            vec![[110, 70]],
            "click=false 仍清障碍，不点击目标"
        );
        let frames = stub.frames.lock().unwrap();
        assert_eq!(frames.len(), 4);
        assert_ne!(frames[0], frames[1], "点击后必须取新帧");
        assert_eq!(frames[1], frames[2]);
        assert_eq!(frames[2], frames[3], "本轮障碍和目标共用同一帧");
        let regions = stub.regions.lock().unwrap();
        assert!(
            regions[..3].iter().all(Option::is_none),
            "障碍不继承目标区域"
        );
        assert!(regions[3].is_some());
    }

    #[test]
    fn wait_find_obstacle_time_counts_toward_timeout_and_validates_input() {
        let trace = Arc::new(Trace::default());
        let stub = VisionStub::new(FrameSize::new(1000, 1000));
        stub.push_outcome(stub_outcome());
        let host = vision_host(
            trace.clone(),
            &stub,
            LogTrace::new(),
            &["vision.match", "resource.read", "input.tap"],
        );
        let result = call("wait_find", json!({"template":"home", "obstacles":["close", "confirm"], "click":false,"timeout":"100ms"}), &host).unwrap();
        assert_eq!(result, Value::Null);
        assert_eq!(stub.match_calls.load(Ordering::Relaxed), 1);
        assert_eq!(trace.taps.lock().unwrap().len(), 1);
        for obstacles in [json!("close"), json!([1]), json!([""])] {
            assert!(call(
                "wait_find",
                json!({"template":"home","obstacles":obstacles}),
                &host
            )
            .is_err());
        }
        let no_input = vision_host(
            trace,
            &stub,
            LogTrace::new(),
            &["vision.match", "resource.read"],
        );
        assert!(call(
            "wait_find",
            json!({"template":"home","obstacles":["close"],"click":false}),
            &no_input
        )
        .is_err());
        assert_eq!(stub.match_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn log_function_writes_and_stringifies_non_text() {
        let trace = Arc::new(Trace::default());
        let stub = VisionStub::new(FrameSize::new(1000, 1000));
        let logs = LogTrace::new();
        let host = vision_host(trace, &stub, logs.clone(), &["log.write"]);
        call("log", json!({"message": "文本"}), &host).unwrap();
        call("log", json!({"message": {"k": 1}}), &host).unwrap();
        assert!(logs.devices.lock().unwrap().iter().all(|d| !d.is_empty()));
        assert_eq!(
            logs.messages(),
            vec!["文本".to_string(), "{\"k\":1}".to_string()]
        );
    }

    struct ClickClock {
        trace: Arc<Trace>,
        sleeps: Mutex<Vec<(usize, u64)>>,
        stop: AtomicBool,
        cancel_on_sleep: bool,
    }
    #[async_trait]
    impl RuntimeService for ClickClock {
        async fn sleep(&self, duration: Duration) -> crate::capabilities::CapabilityResult<()> {
            self.sleeps.lock().unwrap().push((
                self.trace.taps.lock().unwrap().len(),
                duration.as_millis() as u64,
            ));
            if self.cancel_on_sleep {
                self.stop.store(true, Ordering::SeqCst);
                return Err(crate::capabilities::CapabilityError::Cancelled);
            }
            Ok(())
        }
        fn cancelled(&self) -> bool {
            self.stop.load(Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn click_delays_wrap_every_automation_click_exactly_once() {
        for name in ["tap", "wait_find", "tap_template", "obstacle"] {
            let trace = Arc::new(Trace::default());
            let stub = VisionStub::new(FrameSize::new(1000, 1000));
            stub.push_outcome(stub_outcome());
            let events = EventCollect::new();
            let host = vision_host(
                trace.clone(),
                &stub,
                LogTrace::new(),
                &["input.tap", "vision.match", "resource.read"],
            );
            let mut native = NativeYamlHost::new(
                host,
                test_context(),
                Arc::new(AtomicBool::new(false)),
                Some(events.clone()),
            )
            .await
            .unwrap();
            native.settings.before_click_ms = 125;
            native.settings.after_click_ms = 225;
            let clock = Arc::new(ClickClock {
                trace: trace.clone(),
                sleeps: Mutex::new(vec![]),
                stop: AtomicBool::new(false),
                cancel_on_sleep: false,
            });
            native.runtime = clock.clone();
            let (function, args) = match name {
                "tap" => (name, json!([0.5, 0.5])),
                "obstacle" => (
                    "wait_find",
                    json!({"template":"home","obstacles":["close"],"click":false,"timeout":"0ms"}),
                ),
                _ => (name, json!({"template":"home","timeout":"0ms"})),
            };
            native.call_function(function, args).await.unwrap();
            assert_eq!(trace.taps.lock().unwrap().len(), 1, "{name}");
            let sleeps = clock.sleeps.lock().unwrap();
            assert_eq!(
                sleeps
                    .iter()
                    .filter(|(t, _)| *t == 0)
                    .map(|(_, ms)| ms)
                    .sum::<u64>(),
                125,
                "{name} before"
            );
            assert_eq!(
                sleeps
                    .iter()
                    .filter(|(t, _)| *t == 1)
                    .map(|(_, ms)| ms)
                    .sum::<u64>(),
                225,
                "{name} after"
            );
            let delays: Vec<_> = events
                .of("detail")
                .into_iter()
                .filter(|e| e["name"] == "click_delay")
                .collect();
            assert_eq!(delays.len(), 2);
            assert_eq!(
                delays[0]["data"],
                json!({"phase":"before","duration_ms":125})
            );
            assert_eq!(
                delays[1]["data"],
                json!({"phase":"after","duration_ms":225})
            );
        }
    }

    #[tokio::test]
    async fn click_delays_can_be_disabled_and_cancel_before_or_after_tap() {
        for (before, after, cancel, expected_taps) in
            [(0, 0, false, 1), (300, 300, true, 0), (0, 300, true, 1)]
        {
            let trace = Arc::new(Trace::default());
            let stub = VisionStub::new(FrameSize::new(1000, 1000));
            let host = vision_host(trace.clone(), &stub, LogTrace::new(), &["input.tap"]);
            let mut native =
                NativeYamlHost::new(host, test_context(), Arc::new(AtomicBool::new(false)), None)
                    .await
                    .unwrap();
            native.settings.before_click_ms = before;
            native.settings.after_click_ms = after;
            let clock = Arc::new(ClickClock {
                trace: trace.clone(),
                sleeps: Mutex::new(vec![]),
                stop: AtomicBool::new(false),
                cancel_on_sleep: cancel,
            });
            native.runtime = clock.clone();
            assert_eq!(
                native
                    .call_function("tap", json!([0.5, 0.5]))
                    .await
                    .is_err(),
                cancel
            );
            assert_eq!(trace.taps.lock().unwrap().len(), expected_taps);
            assert_eq!(
                clock.sleeps.lock().unwrap().len(),
                if cancel { 1 } else { 0 }
            );
        }
    }

    #[tokio::test]
    async fn matching_without_click_never_uses_click_delays() {
        let trace = Arc::new(Trace::default());
        let stub = VisionStub::new(FrameSize::new(1000, 1000));
        let host = vision_host(
            trace.clone(),
            &stub,
            LogTrace::new(),
            &["vision.match", "resource.read"],
        );
        let mut native =
            NativeYamlHost::new(host, test_context(), Arc::new(AtomicBool::new(false)), None)
                .await
                .unwrap();
        let clock = Arc::new(ClickClock {
            trace: trace.clone(),
            sleeps: Mutex::new(vec![]),
            stop: AtomicBool::new(false),
            cancel_on_sleep: false,
        });
        native.runtime = clock.clone();
        for (name, args) in [
            ("find", json!("home")),
            ("find_any", json!({"templates":["home"]})),
            (
                "wait_find",
                json!({"template":"home","click":false,"timeout":"0ms"}),
            ),
        ] {
            stub.push_outcome(stub_outcome());
            assert!(!native.call_function(name, args).await.unwrap().is_null());
        }
        assert!(clock.sleeps.lock().unwrap().is_empty());
        assert!(trace.taps.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn diagnostics_include_bound_defaults_and_preserve_explicit_timeout() {
        let stub = VisionStub::new(FrameSize::new(1000, 1000));
        let host = vision_host(
            Arc::new(Trace::default()),
            &stub,
            LogTrace::new(),
            &[
                "device.read",
                "vision.match",
                "resource.read",
                "input.tap",
                "runtime.sleep",
            ],
        );
        let events = EventCollect::new();
        let native = NativeYamlHost::new(
            host,
            test_context(),
            Arc::new(AtomicBool::new(false)),
            Some(events.clone()),
        )
        .await
        .unwrap();
        let _ = native
            .call_function("wait_find", json!({"template":"home","timeout":"0ms"}))
            .await;
        let details = events.of("detail");
        let args = &details
            .iter()
            .find(|e| e["name"] == "effective_args")
            .unwrap()["data"]["args"];
        assert_eq!(args["template"], "home");
        assert_eq!(args["timeout"], "0ms");
        assert_eq!(args["click"], true);
        assert!(args["threshold"].is_number());
        assert!(args.get("interval").is_some());
    }

    #[test]
    fn input_text_and_launch_flow_through_capabilities() {
        let trace = Arc::new(Trace::default());
        let stub = VisionStub::new(FrameSize::new(1000, 1000));
        let host = vision_host(
            trace.clone(),
            &stub,
            LogTrace::new(),
            &["input.text", "device.app", "device.read"],
        );
        call("input_text", json!({"text": "你好"}), &host).unwrap();
        assert_eq!(trace.text.lock().unwrap().as_slice(), ["你好"]);
        call("launch", json!({}), &host).unwrap();
    }
}

#[cfg(all(test, feature = "wasm-runtime"))]
mod wasm_tests {
    use super::super::wasm_host::LazyYamlWasmtimeRuntime;
    use super::tests;
    use super::*;
    use crate::extensions::gamer_yaml::syntax::{
        build_program, parse_function_library, parse_script,
    };
    use async_trait::async_trait;
    use std::fs;
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::sync::OnceLock;
    use zip::write::SimpleFileOptions;

    include!("acceptance_tests.rs");

    /// The YAML production registrar supplies the same execution-model
    /// declaration, but this focused guest test does not need a Scheduler.
    struct InstanceFreeRegistrar;

    #[async_trait]
    impl crate::extensions::TimerRunnerRegistrar for InstanceFreeRegistrar {
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

    fn guest_source_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../plugins/gamer-yaml/guest")
    }

    fn guest_target_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("yaml-guest")
    }

    fn guest_module_path() -> PathBuf {
        guest_target_dir()
            .join("wasm32-unknown-unknown")
            .join("release")
            .join("gamer_yaml_guest.wasm")
    }

    fn run_guest_cargo(args: &[String]) -> Output {
        let (subcommand, rest) = args
            .split_first()
            .expect("yaml guest cargo 子进程缺少 subcommand");
        let guest_dir = guest_source_dir();
        let target_dir = guest_target_dir();
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut command = Command::new(cargo);
        command
            .current_dir(&guest_dir)
            .arg(subcommand)
            .arg("--manifest-path")
            .arg(guest_dir.join("Cargo.toml"))
            // 不继承 server 的 target 目录（CI 常设 CARGO_TARGET_DIR）。
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

    fn assert_guest_command(output: Output, stage: &str) {
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

    fn guest_module() -> Vec<u8> {
        static MODULE: OnceLock<Vec<u8>> = OnceLock::new();
        MODULE
            .get_or_init(|| {
                let output = run_guest_cargo(&[
                    "build".into(),
                    "--locked".into(),
                    "--quiet".into(),
                    "--release".into(),
                    "--lib".into(),
                    "--target".into(),
                    "wasm32-unknown-unknown".into(),
                ]);
                assert_guest_command(output, "guest wasm 构建");
                let path = guest_module_path();
                fs::read(&path).unwrap_or_else(|error| {
                    panic!("yaml guest wasm 不存在: {}: {error}", path.display())
                })
            })
            .clone()
    }

    fn componentize_guest(output_path: &Path) -> Vec<u8> {
        let module_path = guest_module_path();
        let output = run_guest_cargo(&[
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
        assert_guest_command(output, "WIT Component 封装");
        fs::read(output_path).unwrap_or_else(|error| {
            panic!(
                "yaml guest Component 输出不存在: {}: {error}",
                output_path.display()
            )
        })
    }

    fn guest_component() -> Vec<u8> {
        static COMPONENT: OnceLock<Vec<u8>> = OnceLock::new();
        COMPONENT
            .get_or_init(|| {
                guest_module();
                let temp = tempfile::tempdir().expect("无法创建 YAML Component 临时目录");
                componentize_guest(&temp.path().join("yaml-guest.component.wasm"))
            })
            .clone()
    }

    #[test]
    fn yaml_guest_builds_wasm_module() {
        let module = guest_module();
        assert!(module.len() >= 8, "guest wasm 太短");
        assert_eq!(&module[..4], b"\0asm");
        assert_eq!(&module[4..8], [1, 0, 0, 0]);
    }

    #[test]
    fn yaml_guest_componentizes_with_checked_in_wit() {
        let component = guest_component();
        assert!(component.len() >= 8, "YAML Component 太短");
        assert_eq!(&component[..4], b"\0asm");
        assert_eq!(&component[4..8], [13, 0, 1, 0]);
    }

    #[test]
    fn yaml_componentizer_releases_output_file_before_returning() {
        guest_module();
        let temp = tempfile::tempdir().expect("无法创建 YAML Component 生命周期临时目录");
        let output = temp.path().join("yaml-guest.component.wasm");
        componentize_guest(&output);
        let moved = temp.path().join("yaml-guest.component.moved.wasm");
        fs::rename(&output, &moved).expect("Componentizer 返回后输出文件仍被占用");
        fs::remove_file(&moved).expect("无法删除已关闭的 Component 输出文件");
    }

    /// V1 源 → wire 程序（无 Package 函数）。
    fn wire(source: &str) -> Value {
        let script = parse_script(source).unwrap();
        let library = parse_function_library("functions: {}\n").unwrap();
        build_program(&script, &library, Default::default(), 0)
    }

    fn host_with_permissions(trace: Arc<tests::Trace>, permissions: &[&str]) -> HostApi {
        let permissions = permissions
            .iter()
            .map(|permission| format!("\"{permission}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let manifest = crate::extensions::parse_manifest(
            format!(
                r#"manifest_version = 2
id = "gamer-yaml"
version = "3.0.0"
name = "自动化"
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
            crate::extensions::HostApiCatalog::default(),
            &manifest,
        )
        .unwrap()
    }

    fn run_request(
        program: Value,
        host: HostApi,
        stop: Arc<AtomicBool>,
        sink: Option<Arc<dyn EventSink>>,
    ) -> YamlWasmRunRequest {
        YamlWasmRunRequest {
            wasm: guest_component(),
            program,
            host,
            context: AppContext::for_test("device-1", "com.example.game").unwrap(),
            stop,
            sink,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn real_yaml_component_runs_v1_program_with_native_functions() {
        let trace = Arc::new(tests::Trace::default());
        let runtime = LazyYamlWasmtimeRuntime::new();
        let mut program =
            wire("run:\n  - input_text: from-real-wasm\n  - tap: [0.5, 0.5]\n  - return: done\n");
        program["_native_settings"] =
            json!({"default_timeout_secs":10,"before_click_ms":17,"after_click_ms":29});
        let sink = tests::EventCollect::new();
        let result = runtime
            .run(run_request(
                program,
                host_with_permissions(trace.clone(), &["device.read", "input.text", "input.tap"]),
                Arc::new(AtomicBool::new(false)),
                Some(sink.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(result.value, Value::String("done".into()));
        assert_eq!(trace.text.lock().unwrap().as_slice(), ["from-real-wasm"]);
        let delays: Vec<_> = sink
            .of("detail")
            .into_iter()
            .filter(|e| e["name"] == "click_delay")
            .map(|e| e["data"]["duration_ms"].as_u64().unwrap())
            .collect();
        assert_eq!(
            delays,
            vec![17, 29],
            "每次原生调用必须使用本次运行的全局快照"
        );
        assert_eq!(trace.taps.lock().unwrap().len(), 1);
        assert!(runtime.is_available());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn yaml_component_runs_package_function_from_frozen_table() {
        let trace = Arc::new(tests::Trace::default());
        let runtime = LazyYamlWasmtimeRuntime::new();
        let script =
            parse_script("run:\n  - greet:\n      who: V1\n    as: out\n  - return: $out\n")
                .unwrap();
        let library = parse_function_library(
            "functions:\n  greet:\n    params:\n      who:\n        type: string\n        default: world\n    run:\n      - input_text: $who\n      - return: $who\n",
        )
        .unwrap();
        let program = build_program(&script, &library, Default::default(), 0);
        let result = runtime
            .run(run_request(
                program,
                host_with_permissions(trace.clone(), &["device.read", "input.text"]),
                Arc::new(AtomicBool::new(false)),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(result.value, Value::String("V1".into()));
        assert_eq!(trace.text.lock().unwrap().as_slice(), ["V1"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn yaml_component_preserves_permission_and_cancellation_kinds() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let denied = runtime
            .run(run_request(
                wire("run:\n  - input_text: denied\n"),
                host_with_permissions(Arc::new(tests::Trace::default()), &["device.read"]),
                Arc::new(AtomicBool::new(false)),
                None,
            ))
            .await
            .unwrap_err();
        assert!(
            denied.to_string().contains("kind=denied"),
            "permission denial lost its WIT kind: {denied:#}"
        );

        // stop 先于运行置位：sleep 函数的 runtime.sleep 报 kind=cancelled。
        let cancelled = runtime
            .run(run_request(
                wire("run:\n  - sleep: 1s\n"),
                host_with_permissions(
                    Arc::new(tests::Trace::default()),
                    &["device.read", "runtime.sleep"],
                ),
                Arc::new(AtomicBool::new(true)),
                None,
            ))
            .await
            .unwrap_err();
        let message = cancelled.to_string();
        assert!(
            message.contains("kind=cancelled") || message.contains("CANCELLED"),
            "cancellation must surface as a cancel-shaped error: {message}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn yaml_component_emits_run_events_and_budget_code() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let sink = tests::EventCollect::new();
        let trace = Arc::new(tests::Trace::default());
        let mut program =
            wire("run:\n  - input_text: one\n  - repeat: 2\n    do:\n      - input_text: tick\n");
        program["trace"] = json!({"run_id":"trace-qa", "entry":{"package_id":"qa","plugin_id":"gamer-yaml","path":"automations/main.yaml","version":"v1"}, "functions":{}});
        let _ = runtime
            .run(run_request(
                program,
                host_with_permissions(trace, &["device.read", "input.text"]),
                Arc::new(AtomicBool::new(false)),
                Some(sink.clone()),
            ))
            .await
            .unwrap();
        let paths: Vec<String> = sink
            .of("step_start")
            .iter()
            .filter_map(|event| event["path"].as_str().map(str::to_string))
            .collect();
        for event in sink.of("step_start") {
            assert_eq!(event["trace"]["run_id"], "trace-qa");
            assert_eq!(event["trace"]["source"]["version"], "v1");
            assert_eq!(event["trace"]["frame_id"], 0);
        }
        assert_eq!(
            paths,
            vec!["run[0]", "run[1]", "run[1].do[0]", "run[1].do[0]"]
        );
        let ends = sink.of("run_end");
        assert_eq!(
            ends.last().map(|event| event["ok"].clone()),
            Some(Value::Bool(true))
        );

        // 巨大次数空转 repeat → STEP_BUDGET_EXCEEDED + budget 事件
        let sink = tests::EventCollect::new();
        let trace = Arc::new(tests::Trace::default());
        let error = runtime
            .run(run_request(
                wire("run:\n  - repeat: 4294967295\n    do: []\n"),
                host_with_permissions(trace, &["device.read"]),
                Arc::new(AtomicBool::new(false)),
                Some(sink.clone()),
            ))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("STEP_BUDGET_EXCEEDED"),
            "{error:#}"
        );
        assert!(sink
            .of("budget")
            .iter()
            .any(|event| event["kind"] == "STEP_BUDGET_EXCEEDED"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn yaml_component_honors_top_level_start_index() {
        let runtime = LazyYamlWasmtimeRuntime::new();
        let trace = Arc::new(tests::Trace::default());
        let script =
            parse_script("run:\n  - input_text: first\n  - input_text: second\n  - return: done\n")
                .unwrap();
        let library = parse_function_library("functions: {}\n").unwrap();
        let program = build_program(&script, &library, Default::default(), 1);
        let result = runtime
            .run(run_request(
                program,
                host_with_permissions(trace.clone(), &["device.read", "input.text"]),
                Arc::new(AtomicBool::new(false)),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(result.value, Value::String("done".into()));
        assert_eq!(trace.text.lock().unwrap().as_slice(), ["second"]);
    }

    /// 生命周期 e2e：安装 → 启用 → V1 脚本经 run_yaml_program 全链跑通；卸载
    /// 后同脚本明确失败。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn installed_yaml_extension_runs_v1_program_end_to_end() {
        let temp = tempfile::tempdir().expect("无法创建 yaml 扩展临时目录");
        let logs = tests::LogTrace::new();
        let registry = CapabilityRegistry::builder()
            .with_device_service(
                Arc::new(tests::Trace::default()) as Arc<dyn crate::capabilities::DeviceService>
            )
            .with_log_service(logs.clone() as Arc<dyn crate::capabilities::LogService>)
            .build();
        let service = crate::extensions::ExtensionService::for_data_root(temp.path(), registry)
            .with_runner_registrar(Arc::new(InstanceFreeRegistrar));

        let mut archive = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut archive));
            let options = SimpleFileOptions::default();
            writer.start_file("manifest.toml", options).unwrap();
            writer
                .write_all(YAML_EXTENSION_MANIFEST_TOML.as_bytes())
                .unwrap();
            writer.start_file("plugin.wasm", options).unwrap();
            writer.write_all(&guest_component()).unwrap();
            writer.start_file("ui/plugin.js", options).unwrap();
            writer.write_all(b"export const sdkVersion = 1;").unwrap();
            writer.finish().unwrap();
        }
        let installed = service.install(&archive).await.unwrap();
        let id = crate::extensions::ExtensionId::parse(YAML_EXTENSION_ID).unwrap();
        service.enable(&id).await.unwrap();
        service.start(&id).await.unwrap();

        let value = super::super::run_yaml_program(
            &service,
            wire("run:\n  - log: from-v1-e2e\n  - return: true\n"),
            AppContext::for_test("device-1", "com.example.game").unwrap(),
            Arc::new(AtomicBool::new(false)),
            None,
        )
        .await
        .unwrap();
        assert_eq!(value, Value::Bool(true));
        assert_eq!(logs.messages(), vec!["from-v1-e2e".to_string()]);

        service.disable(&id).await.unwrap();
        assert!(service
            .uninstall(&id, installed.active_version())
            .await
            .unwrap());
        assert!(super::super::run_yaml_program(
            &service,
            wire("run: []\n"),
            AppContext::for_test("device-1", "com.example.game").unwrap(),
            Arc::new(AtomicBool::new(false)),
            None,
        )
        .await
        .is_err());
    }
}
