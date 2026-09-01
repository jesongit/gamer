//! 严格 AST 执行引擎（2026-08 阶段 2 后半，取代 v1 serde_yaml 动态解析）。
//!
//! 执行目标 [`RunTarget`]（脚本 / 函数二选一）→ 分区源码快照（`snapshot.rs`）
//! → 严格解析（script_v2）→ 参数绑定（params::merge_args）→ AST 步骤执行。
//!
//! 语义要点（docs/SCRIPT_EDITOR_CONTRACT.md + plan §7/§12.2/§13.3）：
//! - `find`：主模板 + block 有序障碍轮询；命中恒点中心；verify 两击；then/else。
//! - `match`：每轮只截一帧按序匹配全部候选（不点击）；首个命中执行其子流程；
//!   无 timeout 单轮、有 timeout 按 config.interval 轮询；绑定后候选重复先报错。
//! - `color`：单点按序判色（容差 30/通道），命中即执行该色值分支并结束。
//! - 判断命中后延迟：find/match/color 命中路径在执行后续分支前统一插入
//!   judge_delay（config.toml judge_delay_ms，默认 200ms，0=关）；else/超时不延迟。
//! - `if`：仅布尔（无隐式转换）；`loop`：times 0/缺省=无限，`break` 跳出最近一层循环
//!   （10 万步 guard 兜底）。
//! - `call`：同分区 yaml/ 脚本，压入目标 config 与参数作用域（返回恢复）；
//!   `func`：`文件短路径/函数名`，继承调用点 config，返回布尔走 then/else。
//! - `throw` 跨调用链结束整个运行（失败终态）；`return` 仅退出当前函数（缺省 true）。
//! - 嵌套上限 32 层（call+func 合计）；取消经 stop 标志轮询退出。
//!
//! 设备访问与模板匹配经窄 trait 端口（`ports.rs`）注入；可视化事件
//! （tap/swipe/hit/miss）经 viewers 注册表 control DataChannel 反向推送。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_recursion::async_recursion;
use image::GenericImageView;
use serde::Serialize;

use crate::device::DeviceManager;
use crate::matcher;
use crate::script_v2::params::{self, merge_args};
use crate::script_v2::validate::{coerce_literal, split_func_path};
use crate::script_v2::{
    ArgAssign, Cell, ColorBranch, LogLevel, MatchCandidate, ParamDecl, ParamType, ScriptConfig,
    Step, TypedValue,
};
use crate::scripts::ScriptStore;
use crate::webrtc::ViewerMap;

use super::events::ScriptEvent;
use super::ports::{
    ComputePoolMatcher, DeviceControl, DeviceGateway, EngineSettings, ScreenshotSource,
    TemplateMatcher,
};
use super::snapshot::{ResourceCache, RunResources, RunSnapshot};

/// find 未显式指定 timeout 时的默认超时（30 分钟，必须 > 0 由装载层保证）。
const FIND_DEFAULT_TIMEOUT_MS: u64 = 1_800_000;
/// wait 分片睡眠的单片时长：停止请求最多延迟这么久生效。
const WAIT_STOP_SLICE_MS: u64 = 200;
/// find 截图持续失败的宽限期：超过则判定链路/会话异常，带因中止。
const FIND_SHOT_FAIL_GRACE_MS: u64 = 20_000;
/// color 截图重试次数（单次判定步骤，无轮询语义，小步重试即可）。
const COLOR_SHOT_RETRIES: u32 = 3;
/// call/func 合计嵌套上限（防无限递归，plan §13.3）。
const MAX_DEPTH: usize = 32;
/// 防死循环步数 guard（含嵌套子步骤；plan §13.3）。
const STEP_BUDGET: u64 = 100_000;
/// color 每通道容差（H.264 有损压缩帧间像素抖动，精确匹配实际不可用）。
const COLOR_TOLERANCE: i32 = 30;

// ---------------------------------------------------------------------------
// 运行目标与请求
// ---------------------------------------------------------------------------

/// 统一运行目标（CONTRACT §4.4）：手动运行 / 从步骤运行 / 函数测试 / 定时任务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunTarget {
    /// 可执行脚本（yaml/）。`start_index` = 顶层步骤序号（0=从头）。
    Script {
        script_id: String,
        start_index: usize,
    },
    /// 函数测试（func/）。`file` = 文件短路径；`function` = 函数名
    /// （None = 文件第一个函数，由入口/API 解析）；`start_index` = 函数体内
    /// 顶层步骤序号。函数不伪装成脚本 ID 进入选择器。
    Function {
        pkg: String,
        file: String,
        function: Option<String>,
        start_index: usize,
    },
}

impl RunTarget {
    /// 运行分区（应用包名）：模板/脚本/函数解析域，也是 str_app/cls_app 包名。
    pub fn pkg(&self) -> &str {
        match self {
            RunTarget::Script { script_id, .. } => script_id.split('/').next().unwrap_or_default(),
            RunTarget::Function { pkg, .. } => pkg,
        }
    }

    /// 展示标签（RunRecord.script_id；busy 弹窗 / 运行日志落库共用）。
    pub fn label(&self) -> String {
        match self {
            RunTarget::Script { script_id, .. } => script_id.clone(),
            RunTarget::Function {
                pkg,
                file,
                function,
                ..
            } => match function {
                Some(f) => format!("{pkg}/{file}.yaml#{f}"),
                None => format!("{pkg}/{file}.yaml"),
            },
        }
    }
}

impl Serialize for RunTarget {
    /// CONTRACT §4.4 JSON 形态。
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        match self {
            RunTarget::Script {
                script_id,
                start_index,
            } => {
                let mut s = serializer.serialize_struct("RunTarget", 3)?;
                s.serialize_field("type", "script")?;
                s.serialize_field("script_id", script_id)?;
                s.serialize_field("start_index", start_index)?;
                s.end()
            }
            RunTarget::Function {
                pkg,
                file,
                function,
                start_index,
            } => {
                let mut s = serializer.serialize_struct("RunTarget", 5)?;
                s.serialize_field("type", "function")?;
                s.serialize_field("pkg", pkg)?;
                s.serialize_field("file", file)?;
                s.serialize_field("function", function)?;
                s.serialize_field("start_index", start_index)?;
                s.end()
            }
        }
    }
}

/// 一次执行的完整规格（RunManager StartRequest → 引擎的直通车）。
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub device_id: String,
    pub target: RunTarget,
    /// 稀疏类型化参数覆盖（API 已按七类解析；引擎按快照声明绑定默认值）。
    pub args: Vec<(String, TypedValue)>,
}

/// YAML 脚本 runner。
///
/// 设备访问与模板匹配全部经窄 trait 端口注入（`super::ports`）：生产由
/// [`Runner::new`] 装配 adapter 转发 DeviceManager / matcher 真实实现，
/// 单元测试注入内存 fake 而不依赖 DeviceManager。
pub struct Runner {
    /// config.toml 中引擎消费字段的静态快照（interval / threshold / log_level）
    pub settings: EngineSettings,
    /// 截图源端口：生产 = DeviceGateway → DeviceManager::screenshot（帧缓存链路）
    pub shots: Arc<dyn ScreenshotSource>,
    /// 设备控制端口：生产 = DeviceGateway → scrcpy 会话控制 / adb
    pub ctl: Arc<dyn DeviceControl>,
    /// 模板匹配端口：生产 = ComputePoolMatcher → matcher::compute 计算池
    pub matcher: Arc<dyn TemplateMatcher>,
    /// Active viewer registry used for script visualization events.
    pub viewers: ViewerMap,
    /// 脚本存储：快照捕获、模板路径解析（分区寻址）
    pub scripts: Arc<ScriptStore>,
}

impl Runner {
    /// 生产装配：在构造点包一层端口 adapter，内部转发 DeviceManager / matcher
    /// 真实实现；Runner 执行路径只依赖窄 trait，生产行为零变化
    pub fn new(devices: Arc<DeviceManager>, viewers: ViewerMap, scripts: Arc<ScriptStore>) -> Self {
        let settings = EngineSettings::from_config(&devices.cfg);
        let gateway = Arc::new(DeviceGateway::new(devices));
        Self::with_ports(
            settings,
            gateway.clone(),
            gateway,
            Arc::new(ComputePoolMatcher),
            viewers,
            scripts,
        )
    }

    /// 端口注入装配（测试用）：截图源 / 设备控制 / 模板匹配各自注入
    pub(crate) fn with_ports(
        settings: EngineSettings,
        shots: Arc<dyn ScreenshotSource>,
        ctl: Arc<dyn DeviceControl>,
        matcher: Arc<dyn TemplateMatcher>,
        viewers: ViewerMap,
        scripts: Arc<ScriptStore>,
    ) -> Self {
        Self {
            settings,
            shots,
            ctl,
            matcher,
            viewers,
            scripts,
        }
    }

    /// 运行一个目标。取消经 stop 标志传递（轮询退出，正常返回日志）；
    /// 解析/校验失败返回结构化诊断；`throw` 返回 Err（失败终态）。
    pub async fn run(
        &self,
        spec: &RunSpec,
        stop: Arc<AtomicBool>,
        log_cb: Option<Arc<dyn Fn(String, String) + Send + Sync>>,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let pkg = spec.target.pkg().to_string();
        // 快照捕获（文件 IO 放 blocking 池）：本次运行实例的不可变源码集
        let scripts = self.scripts.clone();
        let capture_pkg = pkg.clone();
        let snapshot =
            tokio::task::spawn_blocking(move || RunSnapshot::capture(&scripts, &capture_pkg))
                .await
                .map_err(|e| anyhow::anyhow!("运行快照任务失败: {e}"))?
                .map_err(|e| anyhow::anyhow!("运行快照构建失败: {e:#}"))?;
        let resources = RunResources::new(&snapshot, &self.scripts, pkg.clone());
        let mut cache = ResourceCache::default();

        // 入口解析 + 参数绑定 + 运行配置（入口 Arc 在本作用域内保活，steps
        // 拷贝出起始切片——仅入口顶层一次，子树共享随 Arc 存活即可）
        let (steps, config, scope) = match &spec.target {
            RunTarget::Script {
                script_id,
                start_index,
            } => {
                let rel = relative_script_id(script_id, &pkg);
                let script = cache
                    .script(&resources, &rel)
                    .map_err(|errors| fail_diagnostics(&errors))?;
                let bound = bind_entry_args(&script.params, &spec.args, &rel)?;
                let config = RunConfig::from_settings(&self.settings)?
                    .with_script_override(script.config.as_ref());
                (
                    slice_from(&script.steps, *start_index).to_vec(),
                    config,
                    bound,
                )
            }
            RunTarget::Function {
                file,
                function,
                start_index,
                ..
            } => {
                let ff = cache
                    .function_file(&resources, file)
                    .map_err(|errors| fail_diagnostics(&errors))?;
                let name = match function {
                    Some(n) => n.clone(),
                    None => ff
                        .functions
                        .first()
                        .map(|f| f.name.clone())
                        .ok_or_else(|| anyhow::anyhow!("函数文件 {file} 未定义任何函数"))?,
                };
                let decl = ff.find(&name).ok_or_else(|| {
                    anyhow::anyhow!("函数 {file}/{name} 不存在（函数文件中无该函数名）")
                })?;
                let bound = bind_entry_args(&decl.params, &spec.args, &format!("{file}/{name}"))?;
                // 函数库无文件级 config：入口直接用 config.toml 默认
                let config = RunConfig::from_settings(&self.settings)?;
                (
                    slice_from(&decl.steps, *start_index).to_vec(),
                    config,
                    bound,
                )
            }
        };

        let mut ctx = Ctx {
            device_id: spec.device_id.clone(),
            pkg,
            stop,
            exit: Arc::new(AtomicBool::new(false)),
            config,
            log: Vec::new(),
            log_cb,
            scopes: vec![scope],
            frames: Vec::new(),
            return_value: None,
            break_loop: false,
            resources,
            cache,
            depth: 0,
            steps_run: 0,
            region_warned: HashSet::new(),
        };

        self.run_steps(&mut ctx, &steps).await?;

        if ctx.breaking() {
            anyhow::bail!("runtime.break.outside_loop：break 只能出现在 loop 子流程内");
        }
        if ctx.thrown() && !ctx.stopped() {
            // throw 携带原因跨调用链结束整个运行 → 失败终态（runtime.engine.throw）
            anyhow::bail!("脚本 throw 终止");
        }
        Ok(ctx.log)
    }

    /// 推送脚本可视化事件给该设备当前的 viewer（无 viewer / 通道未开 / 发送失败均静默忽略）
    async fn emit(&self, device_id: &str, ev: ScriptEvent) {
        let dc = {
            let map = self.viewers.lock().unwrap();
            map.get(device_id).and_then(|h| h.control_dc.lock().clone())
        };
        let Some(dc) = dc else {
            tracing::debug!(device = %device_id, "script event dropped: no viewer control_dc");
            return;
        };
        let mut v = match serde_json::to_value(&ev) {
            Ok(v) => v,
            Err(_) => return,
        };
        if let Some(o) = v.as_object_mut() {
            o.insert("type".into(), serde_json::json!("se"));
        }
        if let Err(e) = dc.send_text(v.to_string()).await {
            tracing::warn!(device = %device_id, "script event send failed: {}", e);
        }
    }
}

/// 完整脚本 id（`<pkg>/<rel>`）→ 分区内相对 id（`<rel>`）；已是相对形态原样返回。
fn relative_script_id(script_id: &str, pkg: &str) -> String {
    let s = script_id.trim();
    match s.strip_prefix(&format!("{pkg}/")) {
        Some(rel) if !rel.is_empty() => rel.to_string(),
        _ => s.to_string(),
    }
}

/// start_index 语义（沿用 v1「从某行运行」）：0 = 从头；0 < start < len 从该步
/// 执行；越界（>= len）回退从头（选中行快照过期时的兜底）。
fn slice_from(steps: &[Step], start: usize) -> &[Step] {
    if start > 0 && start < steps.len() {
        &steps[start..]
    } else {
        steps
    }
}

/// 入口参数绑定：稀疏覆盖（已类型化）→ 声明默认值打底 → 缺必填报错。
fn bind_entry_args(
    decls: &[ParamDecl],
    overrides: &[(String, TypedValue)],
    resource: &str,
) -> anyhow::Result<HashMap<String, TypedValue>> {
    let bound = merge_args(decls, overrides.iter().cloned(), resource)
        .map_err(|errors| fail_diagnostics(&errors))?;
    Ok(bound.into_iter().collect())
}

/// 结构化诊断 → 运行失败消息（逐条 Display 展开）。
fn fail_diagnostics(errors: &[crate::script_v2::ScriptError]) -> anyhow::Error {
    anyhow::anyhow!(
        "脚本解析/校验失败（{} 项）：{}",
        errors.len(),
        errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("；")
    )
}

// ---------------------------------------------------------------------------
// API 入口参数解析（手动运行 / 函数测试提交前校验）
// ---------------------------------------------------------------------------

/// 稀疏 JSON args 的解析与绑定结果：
/// - `overrides`：稀疏类型化覆盖（进 [`RunSpec`]，引擎运行开始时按快照重绑定）；
/// - `resolved`：声明默认值 → 覆盖 合并后的全量绑定视图（CONTRACT §4.3，
///   API 响应 `resolved_args`，展示本次运行实际生效的参数值）。
#[derive(Debug, Clone)]
pub struct BoundEntryArgs {
    pub overrides: Vec<(String, TypedValue)>,
    pub resolved: serde_json::Value,
    /// 目标当前声明的 psig1 参数签名（CONTRACT §4.5）：定时任务保存时与快照
    /// 一起持久化，调度/立即运行前与脚本当前声明复算值比对做过期门禁。
    pub param_signature: String,
}

/// API 入口参数解析：按分区当前磁盘状态捕获快照、严格解析入口文件，
/// 把稀疏 JSON args 按声明七类解析并合并默认值。
/// 解析/校验失败返回全部结构化诊断（API 映射 400 + 诊断列表）。
/// 注意：这只是提交前的即时解析；运行开始时引擎会以当时的快照重新绑定。
pub fn resolve_entry_args(
    scripts: &ScriptStore,
    target: &RunTarget,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<BoundEntryArgs, Vec<crate::script_v2::ScriptError>> {
    let (decls, label) = load_entry_param_decls(scripts, target)?;
    let overrides = params::parse_json_args(&decls, args, &label)?;
    let resolved = params::merge_args(&decls, overrides.iter().cloned(), &label)?;
    let mut map = serde_json::Map::new();
    for (name, value) in resolved {
        map.insert(name, serde_json::to_value(&value).unwrap_or_default());
    }
    Ok(BoundEntryArgs {
        overrides,
        resolved: serde_json::Value::Object(map),
        param_signature: crate::script_v2::param_signature(&decls),
    })
}

/// 载入运行目标的参数声明（分区磁盘快照 + 严格解析）：
/// - Script：yaml/ 脚本顶层 params；
/// - Function：func/ 文件中目标函数（缺省 = 第一个函数）的 params。
///
/// 同时返回诊断定位用的资源标签（脚本相对 id / `file/function`）；
/// 脚本/函数不存在或解析失败 → 全部结构化诊断（RESOURCE_*.not_found 等）。
pub fn load_entry_param_decls(
    scripts: &ScriptStore,
    target: &RunTarget,
) -> Result<(Vec<ParamDecl>, String), Vec<crate::script_v2::ScriptError>> {
    use crate::script_v2::error::codes;
    use crate::script_v2::ScriptError;

    let pkg = target.pkg().to_string();
    let snapshot = RunSnapshot::capture(scripts, &pkg).map_err(|e| {
        vec![ScriptError::new(
            codes::YAML_SYNTAX_ERROR,
            format!("运行快照构建失败: {e:#}"),
            pkg.clone(),
        )]
    })?;
    let resources = RunResources::new(&snapshot, scripts, pkg.clone());
    let mut cache = ResourceCache::default();
    match target {
        RunTarget::Script { script_id, .. } => {
            let rel = relative_script_id(script_id, &pkg);
            let script = cache.script(&resources, &rel)?;
            let decls = script.params.clone();
            Ok((decls, rel))
        }
        RunTarget::Function { file, function, .. } => {
            let ff = cache.function_file(&resources, file)?;
            let name = match function {
                Some(n) => n.clone(),
                None => ff
                    .functions
                    .first()
                    .map(|f| f.name.clone())
                    .ok_or_else(|| {
                        vec![ScriptError::new(
                            codes::RESOURCE_FUNC_NOT_FOUND,
                            format!("函数文件 {file} 未定义任何函数"),
                            file.clone(),
                        )]
                    })?,
            };
            let decl = ff.find(&name).ok_or_else(|| {
                vec![ScriptError::new(
                    codes::RESOURCE_FUNC_NOT_FOUND,
                    format!("函数 {file}/{name} 不存在（函数文件中无该函数名）"),
                    file.clone(),
                )]
            })?;
            Ok((decl.params.clone(), format!("{file}/{name}")))
        }
    }
}

// ---------------------------------------------------------------------------
// 运行配置
// ---------------------------------------------------------------------------

/// 单次运行生效的配置（config.toml 默认 → call 压栈时被目标脚本 config 覆盖）。
#[derive(Debug, Clone, Copy)]
struct RunConfig {
    interval: Duration,
    threshold: f32,
    log_level: LogLevel,
}

impl RunConfig {
    fn from_settings(settings: &EngineSettings) -> anyhow::Result<Self> {
        let interval = params::parse_time_duration(settings.interval.trim()).ok_or_else(|| {
            anyhow::anyhow!(
                "config.toml interval 非法：{:?}（须带单位且 > 0，如 500ms）",
                settings.interval
            )
        })?;
        let log_level = LogLevel::parse(settings.log_level.trim()).ok_or_else(|| {
            anyhow::anyhow!(
                "config.toml log_level 非法：{:?}（debug/info/warn/error）",
                settings.log_level
            )
        })?;
        Ok(Self {
            interval,
            threshold: settings.threshold,
            log_level,
        })
    }

    fn with_script_override(self, cfg: Option<&ScriptConfig>) -> Self {
        match cfg {
            None => self,
            Some(c) => Self {
                interval: c.interval,
                threshold: c.threshold as f32,
                log_level: c.log_level,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// 运行上下文
// ---------------------------------------------------------------------------

/// call/func 调用帧：进入时保存调用者的 config 与 return_value，返回时恢复。
#[derive(Debug, Clone, Copy)]
struct Frame {
    config: RunConfig,
    return_value: Option<bool>,
}

/// 一次运行的执行上下文（v1 Ctx 的 v2 重写：参数作用域栈替代 $N/^N 文本替换）。
struct Ctx<'a> {
    device_id: String,
    /// 运行分区（应用包名）：模板/子脚本解析域 + str_app/cls_app 包名。
    pkg: String,
    stop: Arc<AtomicBool>,
    /// throw 共享标志：跨 call/func 调用链结束整个运行。
    exit: Arc<AtomicBool>,
    config: RunConfig,
    log: Vec<(String, String)>,
    log_cb: Option<Arc<dyn Fn(String, String) + Send + Sync>>,
    /// 参数作用域栈：入口/每次 call/func 压入，查找最内层优先。
    scopes: Vec<HashMap<String, TypedValue>>,
    /// call/func 调用帧栈（config 覆盖压栈/恢复 + return_value 隔离）。
    frames: Vec<Frame>,
    /// 函数 return 值（Some 后嵌套步骤全部短路；函数边界取出）。
    return_value: Option<bool>,
    /// 当前最近一层 loop 的退出请求；由 loop 消费，不跨 loop 传播。
    break_loop: bool,
    resources: RunResources<'a>,
    cache: ResourceCache,
    /// call+func 合计嵌套深度。
    depth: usize,
    steps_run: u64,
    /// 已提醒过全屏回退的模板（每运行每模板一条）。
    region_warned: HashSet<String>,
}

impl<'a> Ctx<'a> {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn thrown(&self) -> bool {
        self.exit.load(Ordering::SeqCst)
    }

    fn returning(&self) -> bool {
        self.return_value.is_some()
    }

    fn breaking(&self) -> bool {
        self.break_loop
    }

    /// 嵌套步骤短路条件：停止 / throw / 函数 return。
    fn aborted(&self) -> bool {
        self.stopped() || self.thrown() || self.returning()
    }

    fn log(&mut self, level: &str, msg: String) {
        let Some(rank) = level_rank(level) else {
            return;
        };
        // 运行配置的 log_level 恒为合法四级（RunConfig::from_settings 已校验）
        let cur = level_rank(self.config.log_level.as_str()).unwrap_or(1);
        if rank < cur {
            return;
        }
        if let Some(cb) = &self.log_cb {
            cb(level.to_string(), msg.clone());
        }
        self.log.push((level.to_string(), msg));
    }

    /// $name 完整值引用：最内层作用域优先。
    fn lookup(&self, name: &str) -> Option<&TypedValue> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    fn push_scope(&mut self, vars: HashMap<String, TypedValue>) {
        self.scopes.push(vars);
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// call/func 进入帧：保存调用者 config 与 return_value（内层 return 不影响
    /// 外层），压入参数作用域；（call）目标 config 三键覆盖生效。
    fn push_frame(&mut self, config: Option<RunConfig>, scope: HashMap<String, TypedValue>) {
        self.frames.push(Frame {
            config: self.config,
            return_value: self.return_value.take(),
        });
        if let Some(c) = config {
            self.config = c;
        }
        self.push_scope(scope);
    }

    /// call/func 返回帧：弹出作用域，恢复调用者 config 与 return_value；
    /// 返回被调方的 return 值（None = 函数体走完未 return，由调用方按 true 处理）。
    fn leave_frame(&mut self) -> Option<bool> {
        self.pop_scope();
        // 先取走被调方的 return 值，再恢复调用者的帧状态
        let ret = self.return_value.take();
        if let Some(f) = self.frames.pop() {
            self.config = f.config;
            self.return_value = f.return_value;
        }
        ret
    }
}

fn level_rank(level: &str) -> Option<u8> {
    match level {
        "debug" => Some(0),
        "info" => Some(1),
        "warn" => Some(2),
        "error" => Some(3),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 步骤执行
// ---------------------------------------------------------------------------

impl Runner {
    /// 执行一个步骤列表（分支/子流程/函数体/被 call 脚本共用入口）。
    async fn run_steps(&self, ctx: &mut Ctx<'_>, steps: &[Step]) -> anyhow::Result<()> {
        for step in steps {
            if ctx.aborted() || ctx.breaking() {
                break;
            }
            self.exec_step(ctx, step).await?;
        }
        Ok(())
    }

    #[async_recursion]
    async fn exec_step(&self, ctx: &mut Ctx<'_>, step: &Step) -> anyhow::Result<()> {
        if ctx.aborted() {
            return Ok(());
        }
        // 10 万步防死循环 guard（含嵌套子步骤与循环体）
        ctx.steps_run += 1;
        if ctx.steps_run > STEP_BUDGET {
            anyhow::bail!("runtime.step.limit：已执行 {STEP_BUDGET} 步，疑似死循环，强制终止");
        }
        match step {
            Step::StrApp => self.exec_str_app(ctx).await?,
            Step::ClsApp => self.exec_cls_app(ctx).await?,
            Step::Tap { at } => {
                let [rx, ry] = self.coord_value(ctx, at, "tap.at")?;
                let (w, h) = self
                    .ctl
                    .video_size(&ctx.device_id)
                    .ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
                let x = (rx * w as f64).round().clamp(0.0, w as f64) as u32;
                let y = (ry * h as f64).round().clamp(0.0, h as f64) as u32;
                ctx.log(
                    "debug",
                    format!("点击坐标 ({:.3}, {:.3}) → 像素 ({x}, {y})", rx, ry),
                );
                self.emit(&ctx.device_id, ScriptEvent::Tap { x, y }).await;
                self.ctl.tap(&ctx.device_id, x as f32, y as f32).await?;
            }
            Step::Swipe { from, to, time } => {
                let [rx1, ry1] = self.coord_value(ctx, from, "swipe.from")?;
                let [rx2, ry2] = self.coord_value(ctx, to, "swipe.to")?;
                let duration = self.time_value(ctx, time, "swipe.time")?;
                let (w, h) = self
                    .ctl
                    .video_size(&ctx.device_id)
                    .ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
                let x1 = (rx1 * w as f64).round().clamp(0.0, w as f64) as u32;
                let y1 = (ry1 * h as f64).round().clamp(0.0, h as f64) as u32;
                let x2 = (rx2 * w as f64).round().clamp(0.0, w as f64) as u32;
                let y2 = (ry2 * h as f64).round().clamp(0.0, h as f64) as u32;
                ctx.log(
                    "debug",
                    format!(
                        "滑动 ({rx1:.3},{ry1:.3})→({rx2:.3},{ry2:.3}) {}ms",
                        duration
                    ),
                );
                self.emit(&ctx.device_id, ScriptEvent::Swipe { x1, y1, x2, y2 })
                    .await;
                self.ctl
                    .swipe(
                        &ctx.device_id,
                        x1 as f32,
                        y1 as f32,
                        x2 as f32,
                        y2 as f32,
                        duration,
                    )
                    .await?;
            }
            Step::Key { key } => {
                let key = self.text_value(ctx, key, "key", ParamType::Key)?;
                let code = key_code(&key)
                    .ok_or_else(|| anyhow::anyhow!("{}", params::invalid_key_reason(&key)))?;
                ctx.log("debug", format!("按键 {key}"));
                self.ctl.key(&ctx.device_id, code).await?;
            }
            Step::Text { value } => {
                let text = self.text_value(ctx, value, "text", ParamType::Text)?;
                ctx.log("debug", format!("输入文本 {text}"));
                self.ctl.text(&ctx.device_id, &text).await?;
            }
            Step::Log { message } => {
                let msg = self.text_value(ctx, message, "log", ParamType::Text)?;
                ctx.log("info", msg);
            }
            Step::Wait {
                duration,
                duration_max,
            } => {
                let min = self.time_value(ctx, duration, "wait.duration")?;
                let ms = match duration_max {
                    Some(max) => {
                        let max = self.time_value(ctx, max, "wait.duration_max")?;
                        if max > min {
                            min + rand::random::<u64>() % (max - min)
                        } else {
                            min
                        }
                    }
                    None => min,
                };
                ctx.log("debug", format!("等待 {ms}ms"));
                self.sleep_interruptible(ctx, Duration::from_millis(ms))
                    .await;
            }
            Step::Find {
                template,
                block,
                verify,
                timeout,
                then,
                r#else,
            } => {
                self.exec_find(ctx, template, block, *verify, timeout, then, r#else)
                    .await?;
            }
            Step::Match {
                candidates,
                r#else,
                timeout,
            } => {
                self.exec_match(ctx, candidates, r#else, timeout).await?;
            }
            Step::Check { template, r#throw } => {
                self.exec_check(ctx, template, r#throw).await?;
            }
            Step::Color { at, expect, r#else } => {
                self.exec_color(ctx, at, expect, r#else).await?;
            }
            Step::If { cond, then, r#else } => {
                let value = self.typed_value(ctx, cond, "if.cond")?;
                match value {
                    TypedValue::Bool(true) => self.run_steps(ctx, then).await?,
                    TypedValue::Bool(false) => self.run_steps(ctx, r#else).await?,
                    other => anyhow::bail!(
                        "if 条件需要布尔值，得到 {:?}（无隐式转换）",
                        other.param_type().as_str()
                    ),
                }
            }
            Step::Loop { times, steps } => {
                let mut n: u64 = 0;
                loop {
                    if *times > 0 && n >= *times {
                        break;
                    }
                    if ctx.aborted() {
                        break;
                    }
                    ctx.log("debug", format!("循环第 {} 次", n + 1));
                    self.run_steps(ctx, steps).await?;
                    if ctx.breaking() {
                        ctx.break_loop = false;
                        break;
                    }
                    n += 1;
                }
            }
            Step::Break => {
                ctx.break_loop = true;
            }
            Step::Call { target, args } => {
                self.exec_call(ctx, target, args).await?;
            }
            Step::Func {
                target,
                args,
                then,
                r#else,
            } => {
                let ret = self.exec_func(ctx, target, args).await?;
                self.run_steps(ctx, if ret { then } else { r#else }).await?;
            }
            Step::Throw { message } => {
                match message {
                    Some(m) => ctx.log("info", format!("因 {m} 结束运行脚本")),
                    None => ctx.log("info", "结束运行脚本".to_string()),
                }
                ctx.exit.store(true, Ordering::SeqCst);
            }
            Step::Return { value } => {
                let value = self.typed_value(ctx, value, "return.value")?;
                match value {
                    TypedValue::Bool(b) => {
                        ctx.log("debug", format!("函数 return {b}"));
                        ctx.return_value = Some(b);
                    }
                    other => {
                        anyhow::bail!("return 需要布尔值，得到 {:?}", other.param_type().as_str())
                    }
                }
            }
        }
        Ok(())
    }

    // ---- 基础动作 -----------------------------------------------------------

    /// str_app：冷启动应用（"+" 前缀 = 先 force-stop 再启动，scrcpy 定制控制
    /// 消息）。包名 = 运行分区。
    async fn exec_str_app(&self, ctx: &mut Ctx<'_>) -> anyhow::Result<()> {
        let pkg = validate_pkg(&ctx.pkg)?;
        if !self.ctl.has_session(&ctx.device_id) {
            anyhow::bail!("设备未连接");
        }
        ctx.log("info", format!("冷启动应用 {pkg}"));
        self.ctl
            .start_app(&ctx.device_id, &format!("+{pkg}"))
            .await?;
        Ok(())
    }

    /// cls_app：adb force-stop 关闭应用（不碰 scrcpy 会话，投屏不中断）。
    async fn exec_cls_app(&self, ctx: &mut Ctx<'_>) -> anyhow::Result<()> {
        let pkg = validate_pkg(&ctx.pkg)?;
        let serial = self
            .ctl
            .adb_serial(&ctx.device_id)
            .ok_or_else(|| anyhow::anyhow!("设备不存在或未解析出 adb serial"))?;
        ctx.log("info", format!("关闭应用 {pkg}"));
        self.ctl
            .shell(
                &serial,
                &format!("am force-stop {pkg}"),
                Duration::from_secs(8),
            )
            .await?;
        Ok(())
    }

    // ---- 取值 ---------------------------------------------------------------

    fn typed_value(&self, ctx: &Ctx<'_>, cell: &Cell, field: &str) -> anyhow::Result<TypedValue> {
        let value = match cell {
            Cell::Lit(v) => v.clone(),
            Cell::Ref(name) => ctx
                .lookup(name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("$name 引用 {name:?} 未绑定（{field}）"))?,
        };
        Ok(value)
    }

    fn coord_value(&self, ctx: &Ctx<'_>, cell: &Cell, field: &str) -> anyhow::Result<[f64; 2]> {
        match self.typed_value(ctx, cell, field)? {
            TypedValue::Coord(c) => Ok(c),
            other => anyhow::bail!(
                "{field} 需要 coord 类型，得到 {}",
                other.param_type().as_str()
            ),
        }
    }

    fn time_value(&self, ctx: &Ctx<'_>, cell: &Cell, field: &str) -> anyhow::Result<u64> {
        match self.typed_value(ctx, cell, field)? {
            TypedValue::Time(s) => params::parse_time_ms(&s)
                .map(|ms| ms as u64)
                .ok_or_else(|| anyhow::anyhow!("{field} 时间 {s:?} 非法")),
            other => anyhow::bail!(
                "{field} 需要 time 类型，得到 {}",
                other.param_type().as_str()
            ),
        }
    }

    /// key/text/log 等字符串值（Key/Text 字面量或引用）。
    fn text_value(
        &self,
        ctx: &Ctx<'_>,
        cell: &Cell,
        field: &str,
        expect: ParamType,
    ) -> anyhow::Result<String> {
        let value = self.typed_value(ctx, cell, field)?;
        let s = match &value {
            TypedValue::Key(s) | TypedValue::Text(s) => s.clone(),
            other => {
                anyhow::bail!(
                    "{field} 需要 {} 类型，得到 {}",
                    expect.as_str(),
                    other.param_type().as_str()
                )
            }
        };
        Ok(s)
    }

    fn tmpl_value(&self, ctx: &Ctx<'_>, cell: &Cell, field: &str) -> anyhow::Result<String> {
        match self.typed_value(ctx, cell, field)? {
            TypedValue::Tmpl(s) => Ok(s),
            other => anyhow::bail!(
                "{field} 需要 tmpl 类型，得到 {}",
                other.param_type().as_str()
            ),
        }
    }

    // ---- find ----------------------------------------------------------------

    /// find：超时时间内轮询等主模板出现并点击。每轮：主模板（新截图）命中 →
    /// 点击中心 → verify（true = 等 interval 重匹配，仍命中补一击，共两击）→
    /// then 结束；未命中 → block 依序匹配（命中即点击中心并结束本轮）→
    /// 全未命中等 interval 重开一轮；超时 → else。命中路径在 then 前插入
    /// judge_delay（config.toml judge_delay_ms，默认 200ms，0=关；then 空不等待）。
    #[allow(clippy::too_many_arguments)]
    async fn exec_find(
        &self,
        ctx: &mut Ctx<'_>,
        template: &Cell,
        block: &[Cell],
        verify: bool,
        timeout: &Option<Cell>,
        then: &[Step],
        r#else: &[Step],
    ) -> anyhow::Result<()> {
        let template = self.tmpl_value(ctx, template, "find.template")?;
        let mut blocks = Vec::with_capacity(block.len());
        for (i, b) in block.iter().enumerate() {
            blocks.push(self.tmpl_value(ctx, b, &format!("find.block[{i}]"))?);
        }
        let timeout_ms = match timeout {
            Some(cell) => self.time_value(ctx, cell, "find.timeout")?,
            None => FIND_DEFAULT_TIMEOUT_MS,
        };
        if blocks.is_empty() {
            ctx.log(
                "info",
                format!(
                    "等待模板 {template}，超时 {timeout_ms}ms，轮询 {}ms",
                    ctx.config.interval.as_millis()
                ),
            );
        } else {
            ctx.log(
                "info",
                format!(
                    "等待模板 {template}（障碍 {}），超时 {timeout_ms}ms，轮询 {}ms",
                    blocks.join("、"),
                    ctx.config.interval.as_millis()
                ),
            );
        }
        let start = Instant::now();
        // 截图瞬态失败（会话刚建立首帧未到 / 无线链路抖动）不整脚本夭折：
        // 轮询语义下跳过本轮重试，持续失败超过宽限期才判死带因退出
        let mut shot_fail_since: Option<Instant> = None;
        let mut shot_fail_warned_at: Option<Instant> = None;
        loop {
            if ctx.aborted() {
                break;
            }
            if start.elapsed().as_millis() as u64 > timeout_ms {
                ctx.log(
                    "warn",
                    format!("等待模板 {template} 超时（{timeout_ms}ms）"),
                );
                self.run_steps(ctx, r#else).await?;
                break;
            }
            let screen = match self.shot(ctx).await {
                Ok(s) => {
                    shot_fail_since = None;
                    s
                }
                Err(e) => {
                    let since = shot_fail_since.get_or_insert_with(Instant::now);
                    if since.elapsed().as_millis() as u64 > FIND_SHOT_FAIL_GRACE_MS {
                        anyhow::bail!(
                            "截图持续失败（已重试 {}s，疑似链路/会话异常）：{e:#}",
                            FIND_SHOT_FAIL_GRACE_MS / 1000
                        );
                    }
                    if shot_fail_warned_at.is_none_or(|t| t.elapsed().as_secs() >= 10) {
                        shot_fail_warned_at = Some(Instant::now());
                        ctx.log(
                            "warn",
                            format!(
                                "截图失败，{}ms 后重试：{e:#}",
                                ctx.config.interval.as_millis()
                            ),
                        );
                    }
                    self.sleep_interruptible(ctx, ctx.config.interval).await;
                    continue;
                }
            };
            if let Some(mm) = self.match_screen_one(ctx, &template, screen).await? {
                self.emit(
                    &ctx.device_id,
                    ScriptEvent::Hit {
                        tpl: template.clone(),
                        x: mm.x,
                        y: mm.y,
                        w: mm.width,
                        h: mm.height,
                        score: mm.score,
                    },
                )
                .await;
                ctx.log(
                    "info",
                    format!("模板 {template} 已找到 @ ({}, {})", mm.x, mm.y),
                );
                self.click_center(ctx, &mm).await?;
                if verify {
                    self.sleep_interruptible(ctx, ctx.config.interval).await;
                    let recheck = match self.shot(ctx).await {
                        Ok(scr) => self.match_screen_one(ctx, &template, scr).await,
                        Err(e) => {
                            ctx.log("debug", format!("verify 截图失败，跳过复查：{e:#}"));
                            Ok(None)
                        }
                    };
                    if let Some(m2) = recheck? {
                        self.emit(
                            &ctx.device_id,
                            ScriptEvent::Hit {
                                tpl: template.clone(),
                                x: m2.x,
                                y: m2.y,
                                w: m2.width,
                                h: m2.height,
                                score: m2.score,
                            },
                        )
                        .await;
                        ctx.log(
                            "info",
                            format!(
                                "verify：模板 {template} 仍存在，补点一次 @ ({}, {})",
                                m2.x, m2.y
                            ),
                        );
                        self.click_center(ctx, &m2).await?;
                    } else {
                        ctx.log(
                            "debug",
                            format!("verify：模板 {template} 已消失，点击已生效"),
                        );
                    }
                }
                if !then.is_empty() {
                    // 命中路径：点击/verify 后、then 前固定间隔（config.toml judge_delay_ms）
                    self.judge_delay(ctx).await;
                }
                self.run_steps(ctx, then).await?;
                break;
            }
            // 主模板未命中 → block 依序（命中即点击其中心并结束本轮）
            for b in &blocks {
                if ctx.stopped() {
                    break;
                }
                let found = match self.shot(ctx).await {
                    Ok(scr) => self.match_screen_one(ctx, b, scr).await.unwrap_or(None),
                    Err(_) => None, // 截图瞬态失败按本轮 block 未出现处理
                };
                if let Some(mm) = found {
                    self.emit(
                        &ctx.device_id,
                        ScriptEvent::Hit {
                            tpl: b.clone(),
                            x: mm.x,
                            y: mm.y,
                            w: mm.width,
                            h: mm.height,
                            score: mm.score,
                        },
                    )
                    .await;
                    ctx.log(
                        "info",
                        format!("障碍模板 {b} 出现，点击关闭 @ ({}, {})", mm.x, mm.y),
                    );
                    self.click_center(ctx, &mm).await?;
                    break;
                }
            }
            if ctx.aborted() {
                break;
            }
            self.sleep_interruptible(ctx, ctx.config.interval).await;
        }
        Ok(())
    }

    // ---- match ---------------------------------------------------------------

    /// match：每轮只截一帧，候选按书写顺序匹配；首个命中执行其分支并结束本步；
    /// 候选 `click: true` 命中后先点匹配框中心（与 find 同语义）再进分支。
    /// 未配 timeout 只执行一轮（全未命中立即进 else），
    /// 配了按 config.interval 轮询到超时才进 else。命中分支前插入 judge_delay
    /// （config.toml judge_delay_ms，默认 200ms，0=关；分支空不等待，else 不延迟）。
    /// 参数绑定后候选实际重复 → 截图前报错（$ref 值静态校验无法覆盖）。
    async fn exec_match(
        &self,
        ctx: &mut Ctx<'_>,
        candidates: &[MatchCandidate],
        r#else: &[Step],
        timeout: &Option<Cell>,
    ) -> anyhow::Result<()> {
        // 先解析全部候选模板名（不截图）并查重
        let mut names = Vec::with_capacity(candidates.len());
        for (i, cand) in candidates.iter().enumerate() {
            let name = self.tmpl_value(ctx, &cand.template, &format!("match.candidates[{i}]"))?;
            if names.contains(&name) {
                anyhow::bail!("match 候选模板 {name} 在参数绑定后重复（截图前拒绝）");
            }
            names.push(name);
        }
        let timeout_ms = match timeout {
            Some(cell) => Some(self.time_value(ctx, cell, "match.timeout")?),
            None => None,
        };
        let start = Instant::now();
        loop {
            if ctx.aborted() {
                break;
            }
            if let Some(t) = timeout_ms {
                if start.elapsed().as_millis() as u64 > t {
                    ctx.log("warn", format!("match 超时（{t}ms），执行 else"));
                    self.run_steps(ctx, r#else).await?;
                    break;
                }
            }
            // 本轮唯一一帧：全部候选复用同一张截图
            let screen = self.shot(ctx).await?;
            let mut matched = false;
            for (name, cand) in names.iter().zip(candidates.iter()) {
                match self.match_screen_one(ctx, name, screen.clone()).await? {
                    Some(mm) => {
                        self.emit(
                            &ctx.device_id,
                            ScriptEvent::Hit {
                                tpl: name.clone(),
                                x: mm.x,
                                y: mm.y,
                                w: mm.width,
                                h: mm.height,
                                score: mm.score,
                            },
                        )
                        .await;
                        ctx.log("info", format!("match 命中 {name} @ ({}, {})", mm.x, mm.y));
                        if cand.click {
                            // 候选级命中点击：点匹配框中心（与 find 同语义）
                            self.click_center(ctx, &mm).await?;
                        }
                        if !cand.steps.is_empty() {
                            // 命中候选：先经 judge_delay 再执行其分支（空分支不白等）
                            self.judge_delay(ctx).await;
                        }
                        self.run_steps(ctx, &cand.steps).await?;
                        matched = true;
                        break;
                    }
                    None => continue, // Miss 事件在 match_screen_one 内统一推送
                }
            }
            if matched || timeout_ms.is_none() {
                if !matched {
                    self.run_steps(ctx, r#else).await?;
                }
                break;
            }
            self.sleep_interruptible(ctx, ctx.config.interval).await;
        }
        Ok(())
    }

    // ---- check ---------------------------------------------------------------

    /// check：单帧匹配模板（不点击、不轮询、无分支），界面断言用。
    /// 命中 → Hit 可视化事件 + 日志后继续；未命中 → 以 throw 文案按 throw
    /// 步骤同语义结束运行（Miss 搜索区域事件由 match_screen_one 统一推送）。
    async fn exec_check(
        &self,
        ctx: &mut Ctx<'_>,
        template: &Cell,
        message: &str,
    ) -> anyhow::Result<()> {
        let name = self.tmpl_value(ctx, template, "check.template")?;
        ctx.log("info", format!("检查模板 {name}"));
        let screen = self.shot(ctx).await?;
        if let Some(mm) = self.match_screen_one(ctx, &name, screen).await? {
            self.emit(
                &ctx.device_id,
                ScriptEvent::Hit {
                    tpl: name.clone(),
                    x: mm.x,
                    y: mm.y,
                    w: mm.width,
                    h: mm.height,
                    score: mm.score,
                },
            )
            .await;
            ctx.log(
                "info",
                format!("检查通过：模板 {name} @ ({}, {})", mm.x, mm.y),
            );
        } else {
            ctx.log(
                "warn",
                format!("检查未通过：模板 {name} 未命中 —— {message}"),
            );
            ctx.exit.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    // ---- color ---------------------------------------------------------------

    /// color：单点坐标一次截图按序判色（容差 30/通道），命中即执行该色值
    /// 分支并结束本步；全未命中走 else。不轮询（重试套 loop）；分支
    /// `click: true` 命中后先点取样点再进分支。
    /// 命中分支前插入 judge_delay（config.toml judge_delay_ms，默认 200ms，
    /// 0=关；分支空不等待，else 不延迟）。
    async fn exec_color(
        &self,
        ctx: &mut Ctx<'_>,
        at: &Cell,
        expect: &[ColorBranch],
        r#else: &[Step],
    ) -> anyhow::Result<()> {
        let [rx, ry] = self.coord_value(ctx, at, "color.at")?;
        // 无轮询语义，截图瞬态失败小步重试几次，仍失败才带因中止
        let screen = {
            let mut last_err = None;
            let mut screen = None;
            for _ in 0..COLOR_SHOT_RETRIES {
                match self.shot(ctx).await {
                    Ok(s) => {
                        screen = Some(s);
                        break;
                    }
                    Err(e) => {
                        last_err = Some(e);
                        self.sleep_interruptible(ctx, ctx.config.interval).await;
                    }
                }
            }
            match screen {
                Some(s) => s,
                None => {
                    return Err(last_err.unwrap_or_else(|| anyhow::anyhow!("截图失败")));
                }
            }
        };
        let (w, h) = self.screen_size(ctx, &screen).await;
        if w == 0 || h == 0 {
            anyhow::bail!("无法获取屏幕尺寸");
        }
        let px = ((rx * w as f64).round() as i64).clamp(0, w as i64 - 1) as u32;
        let py = ((ry * h as f64).round() as i64).clamp(0, h as i64 - 1) as u32;
        // 截图整图解码 + 采样点读值提交计算池（PERF-003）
        let (ar, ag, ab) = matcher::compute::run(move || {
            let img = image::load_from_memory(&screen)
                .map_err(|e| anyhow::anyhow!("解析截图失败: {}", e))?;
            let p = img.to_rgb8().get_pixel(px, py).0;
            Ok((p[0] as i32, p[1] as i32, p[2] as i32))
        })
        .await
        .and_then(|inner| inner)?;
        for branch in expect {
            let hex = match self.typed_value(ctx, &branch.color, "color.expect")? {
                TypedValue::Color(c) => c,
                other => anyhow::bail!(
                    "color.expect 需要 color 类型，得到 {}",
                    other.param_type().as_str()
                ),
            };
            let (er, eg, eb) = hex_to_rgb(&hex)?;
            if (ar - er as i32).abs() <= COLOR_TOLERANCE
                && (ag - eg as i32).abs() <= COLOR_TOLERANCE
                && (ab - eb as i32).abs() <= COLOR_TOLERANCE
            {
                ctx.log(
                    "info",
                    format!("颜色命中 {hex}（实际 {ar:02x}{ag:02x}{ab:02x}）@ 像素 ({px}, {py})"),
                );
                self.emit(
                    &ctx.device_id,
                    ScriptEvent::Hit {
                        tpl: format!("clr {hex}"),
                        x: px.saturating_sub(12),
                        y: py.saturating_sub(12),
                        w: 24,
                        h: 24,
                        score: 1.0,
                    },
                )
                .await;
                if branch.click {
                    // 候选级命中点击：点取样点（1×1 MatchResult 中心即 px,py，复用 click_center）
                    self.click_center(
                        ctx,
                        &matcher::MatchResult {
                            x: px,
                            y: py,
                            width: 1,
                            height: 1,
                            score: 1.0,
                        },
                    )
                    .await?;
                }
                if !branch.steps.is_empty() {
                    // 命中色值分支：先经 judge_delay 再执行（空分支不白等；else 不延迟）
                    self.judge_delay(ctx).await;
                }
                self.run_steps(ctx, &branch.steps).await?;
                return Ok(());
            }
            ctx.log(
                "debug",
                format!("颜色未命中：期望 {hex} 实际 {ar:02x}{ag:02x}{ab:02x} @ ({px}, {py})"),
            );
        }
        ctx.log("info", "颜色全部未命中，执行 else".to_string());
        self.run_steps(ctx, r#else).await?;
        Ok(())
    }

    // ---- call / func -----------------------------------------------------------

    /// call：同分区 yaml/ 脚本。压入目标脚本 config（interval/threshold/
    /// log_level 三键覆盖）与参数作用域（声明默认值 → args 覆盖），返回后恢复。
    /// throw 穿透（exit 标志共享）。
    async fn exec_call(
        &self,
        ctx: &mut Ctx<'_>,
        target: &str,
        args: &[ArgAssign],
    ) -> anyhow::Result<()> {
        let script = ctx
            .cache
            .script(&ctx.resources, target)
            .map_err(|errors| fail_diagnostics(&errors))?;
        let overrides = self.resolve_step_args(ctx, args, &script.params, target)?;
        let bound = merge_args(&script.params, overrides, target)
            .map_err(|errors| fail_diagnostics(&errors))?;
        ctx.log("debug", format!("调用脚本 {target}"));
        self.enter_frame(ctx, script.config.as_ref(), bound.into_iter().collect())?;
        let result = self.run_steps(ctx, &script.steps).await;
        ctx.leave_frame();
        result
    }

    /// func：`文件短路径/函数名`（func/ 下，文件补 .yaml）。绑定
    /// merge_args；继承调用点 config（不压栈）；函数体走完未 return 默认
    /// 返回 true；返回布尔驱动调用点 then/else。
    async fn exec_func(
        &self,
        ctx: &mut Ctx<'_>,
        target: &str,
        args: &[ArgAssign],
    ) -> anyhow::Result<bool> {
        let Some((file_short, func_name)) = split_func_path(target) else {
            anyhow::bail!("函数路径 {target:?} 必须是 <文件短路径>/<函数名>");
        };
        let ff = ctx
            .cache
            .function_file(&ctx.resources, &file_short)
            .map_err(|errors| fail_diagnostics(&errors))?;
        let decl = ff
            .find(&func_name)
            .ok_or_else(|| anyhow::anyhow!("函数 {target} 不存在（文件或函数名未找到）"))?;
        let overrides = self.resolve_step_args(ctx, args, &decl.params, target)?;
        let bound = merge_args(&decl.params, overrides, target)
            .map_err(|errors| fail_diagnostics(&errors))?;
        ctx.log("debug", format!("调用函数 {target}"));
        self.enter_frame(ctx, None, bound.into_iter().collect())?;
        let result = self.run_steps(ctx, &decl.steps).await;
        let ret = ctx.leave_frame();
        // leave_frame 返回 return_value；错误仍向上传播
        result?;
        Ok(ret.unwrap_or(true))
    }

    /// call/func 公共进入：深度 guard + 参数作用域压栈 + （call）config 压栈。
    /// return_value 保存/清空由帧承担（内层函数返回不影响外层）。
    fn enter_frame(
        &self,
        ctx: &mut Ctx<'_>,
        config: Option<&ScriptConfig>,
        scope: HashMap<String, TypedValue>,
    ) -> anyhow::Result<()> {
        if ctx.depth >= MAX_DEPTH {
            anyhow::bail!("runtime.nesting.limit：call/func 嵌套超过 {MAX_DEPTH} 层，疑似无限递归");
        }
        ctx.depth += 1;
        let next = ctx.config.with_script_override(config);
        ctx.push_frame(Some(next), scope);
        Ok(())
    }

    // call/func 公共返回：恢复 config 与 return_value，弹出作用域。
    // （在 exec_call / exec_func 内联实现，见各自函数体。）

    // ---- args 解析 ---------------------------------------------------------------

    /// call/func args → 稀疏类型化覆盖：Ref 从调用点作用域解析（类型须与目标
    /// 声明一致）；Lit 按目标类型重定型（装载层把非布尔/坐标标量存为 Text）。
    fn resolve_step_args(
        &self,
        ctx: &Ctx<'_>,
        args: &[ArgAssign],
        decls: &[ParamDecl],
        target_label: &str,
    ) -> anyhow::Result<Vec<(String, TypedValue)>> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            let Some(decl) = decls.iter().find(|d| d.name == a.name) else {
                anyhow::bail!("args 键 {:?} 不是目标 {target_label} 的参数", a.name);
            };
            let value = match &a.value {
                Cell::Ref(name) => {
                    let v = ctx.lookup(name).ok_or_else(|| {
                        anyhow::anyhow!("$name 引用 {name:?} 未绑定（args.{name}）")
                    })?;
                    if v.param_type() != decl.ty {
                        anyhow::bail!(
                            "args[{}] 的值类型 {} 与目标参数类型 {} 不符",
                            a.name,
                            v.param_type().as_str(),
                            decl.ty.as_str()
                        );
                    }
                    v.clone()
                }
                Cell::Lit(v) => coerce_literal(v, decl.ty).ok_or_else(|| {
                    anyhow::anyhow!(
                        "args[{}] 的值 {v:?} 与目标参数类型 {} 不符",
                        a.name,
                        decl.ty.as_str()
                    )
                })?,
            };
            out.push((a.name.clone(), value));
        }
        Ok(out)
    }

    // ---- 设备/匹配辅助 -----------------------------------------------------------

    /// 截图（错误包装；find 的软重试语义由调用方实现）。
    async fn shot(&self, ctx: &Ctx<'_>) -> anyhow::Result<Vec<u8>> {
        self.shots
            .screenshot(&ctx.device_id)
            .await
            .map_err(|e| anyhow::anyhow!("截图失败: {e:#}"))
    }

    /// 分片睡眠 + 逐片检查 stop：长 wait（如 1h）一口睡满会让「停止中」
    /// 永远等不到步骤边界。
    async fn sleep_interruptible(&self, ctx: &Ctx<'_>, mut left: Duration) {
        while left > Duration::ZERO {
            if ctx.stopped() {
                return;
            }
            let slice = left.min(Duration::from_millis(WAIT_STOP_SLICE_MS));
            tokio::time::sleep(slice).await;
            left -= slice;
        }
    }

    /// 判断类步骤（find/match/color）命中后、执行后续分支前的固定间隔
    /// （config.toml judge_delay_ms，默认 200，0 = 关闭；脚本 config: 三键不覆盖）：
    /// 给游戏 UI 留响应时间。分片睡眠可停；分支为空（无后续步骤）由调用方跳过
    async fn judge_delay(&self, ctx: &mut Ctx<'_>) {
        let ms = self.settings.judge_delay_ms;
        if ms > 0 {
            ctx.log("debug", format!("判断命中，延迟 {ms}ms 再执行后续步骤"));
            self.sleep_interruptible(ctx, Duration::from_millis(ms))
                .await;
        }
    }

    /// 匹配单个模板一次（复用给定截图，不取新帧）：模板路径经 ScriptStore
    /// 分区寻址 + 短名消歧；区域由解析出的实际文件名 # 后缀决定（无后缀回退
    /// 全屏并记一条日志提醒，每运行每模板一条）。未命中推送 Miss 事件。
    async fn match_screen_one(
        &self,
        ctx: &mut Ctx<'_>,
        template: &str,
        screen: Vec<u8>,
    ) -> anyhow::Result<Option<matcher::MatchResult>> {
        let (w, h) = self.screen_size(ctx, &screen).await;
        if w == 0 || h == 0 {
            anyhow::bail!("无法获取屏幕尺寸");
        }
        let path = self
            .scripts
            .resolve_template_path(&ctx.pkg, template)
            .map_err(|e| anyhow::anyhow!("模板 {template} 解析失败: {e:#}"))?;
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| template.to_string());
        let region = matcher::template_region_from_name(&file_name, w, h);
        if region.is_none()
            && !file_name.contains('#')
            && ctx.region_warned.insert(file_name.clone())
        {
            ctx.log(
                "info",
                format!(
                    "模板 {file_name} 未带 #区域后缀，回退全屏搜索（区域写法：xx#l / xx#0_0_500_500）"
                ),
            );
        }
        let mm = self
            .matcher
            .match_template(screen, template, path, ctx.config.threshold, region)
            .await?;
        if mm.is_none() {
            let [x, y, rw, rh] = region.unwrap_or([0, 0, w, h]);
            self.emit(
                &ctx.device_id,
                ScriptEvent::Miss {
                    tpl: template.to_string(),
                    x,
                    y,
                    w: rw,
                    h: rh,
                },
            )
            .await;
        }
        Ok(mm)
    }

    /// 点击命中模板的中心（find 主模板与 block 障碍模板共用）。
    /// 会话判空在日志/事件之前（保序：未连接时不产生点击日志与 Tap 事件）
    async fn click_center(
        &self,
        ctx: &mut Ctx<'_>,
        m: &matcher::MatchResult,
    ) -> anyhow::Result<()> {
        if !self.ctl.has_session(&ctx.device_id) {
            anyhow::bail!("设备未连接");
        }
        let (cx, cy) = (m.x + m.width / 2, m.y + m.height / 2);
        ctx.log("debug", format!("点击模板中心 @ ({cx}, {cy})"));
        self.emit(&ctx.device_id, ScriptEvent::Tap { x: cx, y: cy })
            .await;
        self.ctl.tap(&ctx.device_id, cx as f32, cy as f32).await?;
        Ok(())
    }

    /// 屏幕尺寸：会话视频参数优先，兜底解码截图（计算池）。
    async fn screen_size(&self, ctx: &Ctx<'_>, screen: &[u8]) -> (u32, u32) {
        if let Some((w, h)) = self.ctl.video_size(&ctx.device_id) {
            if w > 0 && h > 0 {
                return (w, h);
            }
        }
        let png = screen.to_vec();
        match matcher::compute::run(move || {
            image::load_from_memory(&png)
                .map(|img| img.dimensions())
                .unwrap_or((0, 0))
        })
        .await
        {
            Ok((w, h)) => (w, h),
            Err(_) => (0, 0),
        }
    }
}

/// str_app/cls_app 包名校验（cls_app 拼进 adb shell 命令，防注入）。
fn validate_pkg(pkg: &str) -> anyhow::Result<String> {
    if pkg.is_empty() {
        anyhow::bail!("缺少应用包名（运行分区为空）");
    }
    if !pkg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
    {
        anyhow::bail!("应用包名字符非法: {pkg}");
    }
    Ok(pkg.to_string())
}

/// 6 位十六进制色值 → RGB（装载层保证格式；防御性解析失败报错）。
fn hex_to_rgb(hex: &str) -> anyhow::Result<(u8, u8, u8)> {
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("颜色 {hex:?} 不是 6 位十六进制");
    }
    Ok((
        u8::from_str_radix(&hex[0..2], 16)?,
        u8::from_str_radix(&hex[2..4], 16)?,
        u8::from_str_radix(&hex[4..6], 16)?,
    ))
}

/// 常用按键映射（Android keycode）；纯数字 keycode 透传。未知键返回 `None`，
/// 由调用方让当前步骤失败——装载/参数层已用 `params::is_valid_key` 拦截，
/// 运行期能到这里的多是 args 显式传入的非法值，同样不静默降级。
pub fn key_code(key: &str) -> Option<u32> {
    let code = match key.to_uppercase().as_str() {
        "HOME" => 3,
        "BACK" => 4,
        "APP_SWITCH" | "RECENTS" => 187,
        "MENU" => 82,
        "VOL_UP" | "VOLUME_UP" => 24,
        "VOL_DOWN" | "VOLUME_DOWN" => 25,
        "POWER" => 26,
        "ENTER" => 66,
        "DEL" | "BACKSPACE" => 67,
        "TAB" => 61,
        "SPACE" => 62,
        "ESC" => 111,
        "SEARCH" => 84,
        "CAMERA" => 27,
        "FOCUS" => 80,
        "NOTIFICATION" => 83,
        "SETTINGS" => 176,
        "MUTE" => 91,
        "HEADSETHOOK" => 79,
        "WAKEUP" => 224,
        "SLEEP" => 223,
        "0" => 7,
        "1" => 8,
        "2" => 9,
        "3" => 10,
        "4" => 11,
        "5" => 12,
        "6" => 13,
        "7" => 14,
        "8" => 15,
        "9" => 16,
        _ => return key.parse::<u32>().ok(),
    };
    Some(code)
}

// ---------------------------------------------------------------------------
// 测试：fake 端口注入，逐类核对严格执行语义（不依赖 DeviceManager / 真设备）
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ports::{DeviceControl, EngineSettings, ScreenshotSource, TemplateMatcher};
    use futures_util::future::BoxFuture;
    use std::sync::Mutex;

    const PKG: &str = "com.test.app";

    /// 纯色 PNG（截图源 / 取色用；尺寸 = fake 会话视频尺寸，坐标映射一致）。
    fn solid_png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut img = image::RgbImage::new(w, h);
        for p in img.pixels_mut() {
            *p = image::Rgb(rgb);
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    /// 截图源 fake：固定返回一张 PNG 并计数。
    struct FakeShots {
        png: Vec<u8>,
        count: std::sync::atomic::AtomicUsize,
    }

    impl FakeShots {
        fn new(png: Vec<u8>) -> Arc<Self> {
            Arc::new(Self {
                png,
                count: std::sync::atomic::AtomicUsize::new(0),
            })
        }
        fn count(&self) -> usize {
            self.count.load(Ordering::SeqCst)
        }
    }

    impl ScreenshotSource for FakeShots {
        fn screenshot(&self, _device_id: &str) -> BoxFuture<'_, anyhow::Result<Vec<u8>>> {
            self.count.fetch_add(1, Ordering::SeqCst);
            let png = self.png.clone();
            Box::pin(async move { Ok(png) })
        }
    }

    /// 设备控制 fake：记录调用序列（tap 坐标已换算为像素，直接断言轨迹）。
    struct FakeCtl {
        calls: Mutex<Vec<String>>,
    }

    impl FakeCtl {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
            })
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl DeviceControl for FakeCtl {
        fn has_session(&self, _device_id: &str) -> bool {
            true
        }
        fn video_size(&self, _device_id: &str) -> Option<(u32, u32)> {
            Some((1000, 500))
        }
        fn adb_serial(&self, _device_id: &str) -> Option<String> {
            Some("fake-serial".into())
        }
        fn tap(&self, _d: &str, x: f32, y: f32) -> BoxFuture<'_, anyhow::Result<()>> {
            self.calls.lock().unwrap().push(format!("tap {x} {y}"));
            Box::pin(async { Ok(()) })
        }
        fn swipe(
            &self,
            _d: &str,
            x1: f32,
            y1: f32,
            x2: f32,
            y2: f32,
            ms: u64,
        ) -> BoxFuture<'_, anyhow::Result<()>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("swipe {x1} {y1} {x2} {y2} {ms}"));
            Box::pin(async { Ok(()) })
        }
        fn key(&self, _d: &str, keycode: u32) -> BoxFuture<'_, anyhow::Result<()>> {
            self.calls.lock().unwrap().push(format!("key {keycode}"));
            Box::pin(async { Ok(()) })
        }
        fn text(&self, _d: &str, text: &str) -> BoxFuture<'_, anyhow::Result<()>> {
            self.calls.lock().unwrap().push(format!("text {text}"));
            Box::pin(async { Ok(()) })
        }
        fn start_app(&self, _d: &str, name: &str) -> BoxFuture<'_, anyhow::Result<()>> {
            self.calls.lock().unwrap().push(format!("start_app {name}"));
            Box::pin(async { Ok(()) })
        }
        fn shell(
            &self,
            serial: &str,
            command: &str,
            _timeout: Duration,
        ) -> BoxFuture<'_, anyhow::Result<()>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("shell {serial} {command}"));
            Box::pin(async { Ok(()) })
        }
    }

    /// 模板匹配 fake：按模板短名查表命中（未注册 = 未命中），记录调用顺序。
    struct FakeMatcher {
        hits: HashMap<String, [u32; 4]>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeMatcher {
        fn new(hits: HashMap<&'static str, [u32; 4]>) -> Arc<Self> {
            Arc::new(Self {
                hits: hits.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
                calls: Mutex::new(Vec::new()),
            })
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl TemplateMatcher for FakeMatcher {
        fn match_template(
            &self,
            _screen_png: Vec<u8>,
            template: &str,
            _path: std::path::PathBuf,
            _threshold: f32,
            _region: Option<[u32; 4]>,
        ) -> BoxFuture<'_, anyhow::Result<Option<matcher::MatchResult>>> {
            self.calls.lock().unwrap().push(template.to_string());
            let hit = self.hits.get(template).copied();
            Box::pin(async move {
                Ok(hit.map(|[x, y, w, h]| matcher::MatchResult {
                    x,
                    y,
                    width: w,
                    height: h,
                    score: 0.99,
                }))
            })
        }
    }

    /// 测试装配：临时目录分区存储 + fake 三端口（Arc 共享以便 spawn 中运行）。
    struct Rig {
        runner: Runner,
        ctl: Arc<FakeCtl>,
        shots: Arc<FakeShots>,
        matcher: Arc<FakeMatcher>,
        store: Arc<ScriptStore>,
        dir: std::path::PathBuf,
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn rig(hits: HashMap<&'static str, [u32; 4]>, png: Vec<u8>, log_level: &str) -> Arc<Rig> {
        rig_with(hits, png, log_level, 0)
    }

    /// rig 变体：显式指定 judge_delay_ms（判断命中后延迟用例）
    fn rig_with(
        hits: HashMap<&'static str, [u32; 4]>,
        png: Vec<u8>,
        log_level: &str,
        judge_delay_ms: u64,
    ) -> Arc<Rig> {
        let dir = std::env::temp_dir().join(format!(
            "gamer-enginetest-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = crate::config::Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let store = Arc::new(ScriptStore::open(&cfg).unwrap());
        let viewers: ViewerMap = Arc::new(Mutex::new(HashMap::new()));
        let ctl = FakeCtl::new();
        let shots = FakeShots::new(png);
        let matcher = FakeMatcher::new(hits);
        let runner = Runner::with_ports(
            EngineSettings {
                interval: "20ms".into(),
                threshold: 0.85,
                log_level: log_level.to_string(),
                judge_delay_ms,
            },
            shots.clone(),
            ctl.clone(),
            matcher.clone(),
            viewers,
            store.clone(),
        );
        Arc::new(Rig {
            runner,
            ctl,
            shots,
            matcher,
            store,
            dir,
        })
    }

    impl Rig {
        /// 在分区 tmpl/ 下落一个占位模板文件（fake matcher 不读内容）。
        fn tmpl(&self, name: &str) {
            let d = self.store.tmpl_dir(PKG);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(name), b"png").unwrap();
        }

        fn save_script(&self, name: &str, content: &str) {
            self.store.save(None, PKG, name, content).unwrap();
        }

        fn save_func(&self, name: &str, content: &str) {
            self.store.save_function(PKG, name, content).unwrap();
        }

        async fn run(
            &self,
            target: RunTarget,
            args: Vec<(&str, TypedValue)>,
        ) -> anyhow::Result<Vec<(String, String)>> {
            self.run_with_stop(target, args, Arc::new(AtomicBool::new(false)))
                .await
        }

        async fn run_with_stop(
            &self,
            target: RunTarget,
            args: Vec<(&str, TypedValue)>,
            stop: Arc<AtomicBool>,
        ) -> anyhow::Result<Vec<(String, String)>> {
            let spec = RunSpec {
                device_id: "dev".into(),
                target,
                args: args.into_iter().map(|(n, v)| (n.to_string(), v)).collect(),
            };
            self.runner.run(&spec, stop, None).await
        }
    }

    fn script_target(name: &str) -> RunTarget {
        RunTarget::Script {
            script_id: format!("{PKG}/{name}"),
            start_index: 0,
        }
    }

    fn logs_contain(logs: &[(String, String)], needle: &str) -> bool {
        logs.iter().any(|(_, m)| m.contains(needle))
    }

    // ---- find ---------------------------------------------------------------

    /// 主模板首轮命中：点中心（像素换算）+ then 执行 + 单帧判定。
    #[tokio::test]
    async fn find_hit_clicks_center_and_runs_then() {
        let r = rig(
            HashMap::from([("main.png", [400u32, 200, 200, 100])]),
            solid_png(100, 100, [0, 0, 0]),
            "info",
        );
        r.tmpl("main.png");
        r.save_script(
            "f.yaml",
            "steps:\n  - find: main.png\n    then:\n      - log: 进入主界面\n",
        );
        let logs = r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert_eq!(r.ctl.calls(), vec!["tap 500 250"]);
        assert!(logs_contain(&logs, "main.png 已找到"));
        assert!(logs_contain(&logs, "进入主界面"));
        assert_eq!(r.shots.count(), 1);
    }

    /// judge_delay（config.toml judge_delay_ms）：find 命中后、then 执行前插入
    /// 固定延迟（总耗时 ≥ 延迟值）；then 为空不白等。
    #[tokio::test]
    async fn judge_delay_applies_between_find_hit_and_then() {
        let r = rig_with(
            HashMap::from([("main.png", [400u32, 200, 200, 100])]),
            solid_png(100, 100, [0, 0, 0]),
            "info",
            200,
        );
        r.tmpl("main.png");
        r.save_script(
            "f.yaml",
            "steps:\n  - find: main.png\n    then:\n      - log: 进入主界面\n",
        );
        let start = Instant::now();
        let logs = r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(200),
            "命中后应插入 judge_delay"
        );
        assert!(logs_contain(&logs, "进入主界面"));
    }

    /// judge_delay：then 为空（无后续步骤）不等待，运行立即结束。
    #[tokio::test]
    async fn judge_delay_skipped_when_find_then_empty() {
        let r = rig_with(
            HashMap::from([("main.png", [400u32, 200, 200, 100])]),
            solid_png(100, 100, [0, 0, 0]),
            "info",
            200,
        );
        r.tmpl("main.png");
        r.save_script("f.yaml", "steps:\n  - find: main.png\n");
        let start = Instant::now();
        r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert!(
            start.elapsed() < Duration::from_millis(150),
            "then 为空不应执行 judge_delay，实际 {:?}",
            start.elapsed()
        );
    }

    /// judge_delay：match 命中候选后、分支执行前插入固定延迟；else 路径不延迟。
    #[tokio::test]
    async fn judge_delay_applies_between_match_hit_and_branch() {
        let r = rig_with(
            HashMap::from([("a.png", [0u32, 0, 100, 50])]),
            solid_png(100, 100, [0, 0, 0]),
            "info",
            200,
        );
        r.tmpl("a.png");
        r.save_script(
            "f.yaml",
            "steps:\n  - match:\n    - a.png:\n      - log: 分支执行\n",
        );
        let start = Instant::now();
        let logs = r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(200),
            "命中分支前应插入 judge_delay"
        );
        assert!(logs_contain(&logs, "分支执行"));
    }

    /// block 有序：主模板未命中后按书写序匹配 block（matcher 调用序断言）；
    /// 命中的 block 点其中心（不点主模板）；主模板持续未命中 → 超时 else。
    #[tokio::test]
    async fn find_block_order_and_block_clicks_its_center() {
        let r = rig(
            HashMap::from([("b2.png", [0u32, 0, 100, 50])]),
            solid_png(100, 100, [0, 0, 0]),
            "info",
        );
        r.tmpl("main.png");
        r.tmpl("b1.png");
        r.tmpl("b2.png");
        r.save_script(
            "f.yaml",
            "steps:\n  - find: main.png\n    block:\n      - b1.png\n      - b2.png\n    timeout: 100ms\n    then:\n      - log: 不应执行\n    else:\n      - log: 超时兜底\n",
        );
        let logs = r.run(script_target("f.yaml"), vec![]).await.unwrap();
        // 每轮匹配顺序：main → b1 → b2
        let calls = r.matcher.calls();
        assert_eq!(&calls[0..3], &vec!["main.png", "b1.png", "b2.png"]);
        // b2 每轮被点击一次，主模板与 b1 从未点击
        let taps = r.ctl.calls();
        assert!(!taps.is_empty());
        assert!(taps.iter().all(|c| c == "tap 50 25"));
        assert!(!logs_contain(&logs, "不应执行"));
        assert!(logs_contain(&logs, "超时兜底"));
        assert!(logs_contain(&logs, "b2.png 出现"));
    }

    /// verify 两击：命中点击后复查仍命中 → 补一击，共两击。
    #[tokio::test]
    async fn find_verify_clicks_twice_when_template_persists() {
        let r = rig(
            HashMap::from([("main.png", [400u32, 200, 200, 100])]),
            solid_png(100, 100, [0, 0, 0]),
            "info",
        );
        r.tmpl("main.png");
        r.save_script("f.yaml", "steps:\n  - find: main.png\n    verify: true\n");
        r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert_eq!(
            r.ctl.calls(),
            vec!["tap 500 250", "tap 500 250"],
            "verify 复查仍命中必须补一击"
        );
    }

    /// 超时走 else、不点击；interval 轮询多次截图。
    #[tokio::test]
    async fn find_timeout_runs_else_without_click() {
        let r = rig(HashMap::new(), solid_png(100, 100, [0, 0, 0]), "info");
        r.tmpl("main.png");
        r.save_script(
            "f.yaml",
            "steps:\n  - find: main.png\n    timeout: 60ms\n    else:\n      - log: else 分支\n",
        );
        let logs = r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert!(r.ctl.calls().is_empty());
        assert!(logs_contain(&logs, "等待模板 main.png 超时（"));
        assert!(logs_contain(&logs, "else 分支"));
        assert!(r.shots.count() >= 2, "interval 轮询应多次截图");
    }

    // ---- match ---------------------------------------------------------------

    /// match：每轮单帧、候选按序、命中即停且不点击；无 timeout 单轮。
    #[tokio::test]
    async fn match_single_frame_per_round_ordered_candidates_no_click() {
        let r = rig(
            HashMap::from([("b.png", [10u32, 10, 50, 40])]),
            solid_png(100, 100, [0, 0, 0]),
            "info",
        );
        r.tmpl("a.png");
        r.tmpl("b.png");
        r.tmpl("c.png");
        r.save_script(
            "f.yaml",
            "steps:\n  - match:\n    - a.png:\n      - log: 分支A\n    - b.png:\n      - log: 分支B\n    - c.png:\n      - log: 分支C\n    else:\n      - log: 全未命中\n",
        );
        let logs = r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert_eq!(r.shots.count(), 1, "一轮候选只截一帧");
        assert_eq!(
            r.matcher.calls(),
            vec!["a.png", "b.png"],
            "命中即停，后续候选不匹配"
        );
        assert!(r.ctl.calls().is_empty(), "match 永不点击");
        assert!(logs_contain(&logs, "match 命中 b.png"));
        assert!(logs_contain(&logs, "分支B"));
        assert!(!logs_contain(&logs, "分支A"));
        assert!(!logs_contain(&logs, "全未命中"));
    }

    /// match 未配 timeout 全未命中 → 立即 else（单轮）。
    #[tokio::test]
    async fn match_no_timeout_single_round_else() {
        let r = rig(HashMap::new(), solid_png(100, 100, [0, 0, 0]), "info");
        r.tmpl("a.png");
        r.save_script(
            "f.yaml",
            "steps:\n  - match:\n    - a.png:\n      - log: 分支A\n    else:\n      - log: else 分支\n",
        );
        let logs = r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert_eq!(r.shots.count(), 1);
        assert!(logs_contain(&logs, "else 分支"));
    }

    /// match 配 timeout：按 interval 轮询到超时才 else。
    #[tokio::test]
    async fn match_with_timeout_polls_then_else() {
        let r = rig(HashMap::new(), solid_png(100, 100, [0, 0, 0]), "info");
        r.tmpl("a.png");
        r.save_script(
            "f.yaml",
            "steps:\n  - match:\n    - a.png:\n      - log: 分支A\n    else:\n      - log: else 分支\n    timeout: 70ms\n",
        );
        let logs = r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert!(r.shots.count() >= 2, "有 timeout 必须轮询多轮");
        assert!(logs_contain(&logs, "else 分支"));
    }

    /// 候选级点击：click 候选命中点模板框中心，未命中的 click 候选不点；
    /// 无 click 候选命中不点（分支级开关互不影响）。
    #[tokio::test]
    async fn match_branch_click_hits_clicked_candidate_center_only() {
        let r = rig(
            HashMap::from([("b.png", [10u32, 10, 50, 40])]),
            solid_png(100, 100, [0, 0, 0]),
            "info",
        );
        r.tmpl("a.png");
        r.tmpl("b.png");
        // a 带 click 但未命中、b 命中但无 click → 零点击
        r.save_script(
            "f.yaml",
            "steps:\n  - match:\n    - a.png:\n        click: true\n    - b.png:\n      - log: 分支B\n",
        );
        let logs = r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert!(
            r.ctl.calls().is_empty(),
            "未命中不点、无 click 不点：{:?}",
            r.ctl.calls()
        );
        assert!(logs_contain(&logs, "分支B"));
        // b 也带 click → 只点 b 中心一次（a 未命中不点）
        r.save_script(
            "g.yaml",
            "steps:\n  - match:\n    - a.png:\n        click: true\n    - b.png:\n        click: true\n        steps:\n          - log: 分支B\n",
        );
        let logs = r.run(script_target("g.yaml"), vec![]).await.unwrap();
        assert_eq!(
            r.ctl.calls(),
            vec!["tap 35 30"],
            "命中候选 click 点模板框中心 (10+50/2, 10+40/2)"
        );
        assert!(logs_contain(&logs, "分支B"));
    }

    /// 参数绑定后候选实际重复 → 截图前拒绝（0 次截图）。
    #[tokio::test]
    async fn match_duplicate_after_args_binding_rejected_before_screenshot() {
        let r = rig(HashMap::new(), solid_png(100, 100, [0, 0, 0]), "info");
        r.tmpl("x.png");
        r.save_script(
            "f.yaml",
            "params:\n  - 'tmpl:p1:参数一'\n  - 'tmpl:p2:参数二'\nsteps:\n  - match:\n    - $p1:\n      - log: A\n    - $p2:\n      - log: B\n",
        );
        let err = r
            .run(
                script_target("f.yaml"),
                vec![
                    ("p1", TypedValue::Tmpl("x.png".into())),
                    ("p2", TypedValue::Tmpl("x.png".into())),
                ],
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("重复"), "{err:#}");
        assert_eq!(r.shots.count(), 0);
    }

    // ---- color ---------------------------------------------------------------

    /// color：按序判色，首个命中执行其分支并结束（容差 30/通道）。
    #[tokio::test]
    async fn color_first_matching_branch_wins_in_order() {
        // 屏幕纯色 ff8800；第一候选 0000ff 未命中、第二候选 ff8800 命中
        let r = rig(
            HashMap::new(),
            solid_png(1000, 500, [0xff, 0x88, 0x00]),
            "info",
        );
        r.save_script(
            "f.yaml",
            "steps:\n  - color:\n      at: [0.5, 0.5]\n      expect:\n        - 0000ff:\n          - log: 蓝分支\n        - ff8800:\n          - log: 橙分支\n    else:\n      - log: else 分支\n",
        );
        let logs = r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert!(logs_contain(&logs, "橙分支"));
        assert!(!logs_contain(&logs, "蓝分支"));
        assert!(!logs_contain(&logs, "else 分支"));
    }

    /// color 全未命中走 else；单次判定不轮询（1 帧）。
    #[tokio::test]
    async fn color_miss_runs_else() {
        let r = rig(HashMap::new(), solid_png(1000, 500, [1, 2, 3]), "info");
        r.save_script(
            "f.yaml",
            "steps:\n  - color:\n      at: [0.5, 0.5]\n      expect:\n        - ff8800:\n          - log: 橙分支\n    else:\n      - log: else 分支\n",
        );
        let logs = r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert!(logs_contain(&logs, "else 分支"));
        assert_eq!(r.shots.count(), 1);
    }

    /// color 候选级点击：click 分支命中点取样点；无 click 分支命中零点击。
    #[tokio::test]
    async fn color_branch_click_taps_sample_point() {
        // 屏幕 1000×500 纯色 ff8800，at [0.5, 0.5] → 取样点 (500, 250)
        let r = rig(
            HashMap::new(),
            solid_png(1000, 500, [0xff, 0x88, 0x00]),
            "info",
        );
        r.save_script(
            "f.yaml",
            "steps:\n  - color:\n      at: [0.5, 0.5]\n      expect:\n        - ff8800:\n            click: true\n        - 0000ff:\n          - log: 蓝分支\n    else:\n      - log: else 分支\n",
        );
        r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert_eq!(
            r.ctl.calls(),
            vec!["tap 500 250"],
            "命中分支 click 点取样点"
        );
        // 无 click 分支命中 → 不点击
        let r = rig(
            HashMap::new(),
            solid_png(1000, 500, [0xff, 0x88, 0x00]),
            "info",
        );
        r.save_script(
            "f.yaml",
            "steps:\n  - color:\n      at: [0.5, 0.5]\n      expect:\n        - ff8800:\n          - log: 橙分支\n",
        );
        r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert!(r.ctl.calls().is_empty(), "无 click 不点击");
    }

    // ---- if / loop / guard ---------------------------------------------------

    /// if 布尔严格：true 走 then / false 走 else；布尔实参可经 args 覆盖。
    #[tokio::test]
    async fn if_branches_on_bool_arg() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.save_script(
            "f.yaml",
            "params:\n  - 'bool:flag:开关'\nsteps:\n  - if: $flag\n    then:\n      - log: 开\n    else:\n      - log: 关\n",
        );
        let logs = r
            .run(
                script_target("f.yaml"),
                vec![("flag", TypedValue::Bool(true))],
            )
            .await
            .unwrap();
        assert!(logs_contain(&logs, "开"));
        let logs = r
            .run(
                script_target("f.yaml"),
                vec![("flag", TypedValue::Bool(false))],
            )
            .await
            .unwrap();
        assert!(logs_contain(&logs, "关"));
    }

    /// loop times 按次数执行。
    #[tokio::test]
    async fn loop_runs_exact_times() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.save_script(
            "f.yaml",
            "steps:\n  - loop:\n      times: 3\n      steps:\n        - log: 第几轮\n",
        );
        let logs = r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert_eq!(logs.iter().filter(|(_, m)| m == "第几轮").count(), 3);
    }

    /// break 跳出最近一层 loop；省略 times 按 0 处理（无限循环），由 break 正常结束。
    #[tokio::test]
    async fn break_exits_nearest_loop() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.save_script(
            "f.yaml",
            "steps:\n  - loop:\n      steps:\n        - loop:\n            times: 2\n            steps:\n              - log: 内层\n              - break\n        - log: 外层\n        - break\n  - log: 循环后\n",
        );
        let logs = r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert_eq!(logs.iter().filter(|(_, m)| m == "内层").count(), 1);
        assert_eq!(logs.iter().filter(|(_, m)| m == "外层").count(), 1);
        assert_eq!(logs.iter().filter(|(_, m)| m == "循环后").count(), 1);
    }

    /// 10 万步 guard：无限 loop 被强制终止（含嵌套子步骤计数）。
    #[tokio::test]
    async fn infinite_loop_hits_step_budget_guard() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "error");
        r.save_script(
            "f.yaml",
            "steps:\n  - loop:\n      steps:\n        - log: x\n",
        );
        let err = r.run(script_target("f.yaml"), vec![]).await.unwrap_err();
        assert!(err.to_string().contains("已执行"), "{err:#}");
    }

    // ---- key 枚举校验（装载层拦截 + 运行期兜底） ------------------------------

    /// key_code 与 params::KEY_NAMES 双向一致：枚举内每键（含别名/小写）都有
    /// 非 0 映射；数字串透传；未知键返回 None。
    #[test]
    fn key_code_matches_key_enum() {
        use crate::script_v2::params::KEY_NAMES;
        for name in KEY_NAMES {
            let code = key_code(name).unwrap_or_else(|| panic!("枚举键 {name} 无 keycode 映射"));
            assert_ne!(code, 0, "枚举键 {name} 映射为 keycode 0");
            assert_eq!(
                key_code(&name.to_ascii_lowercase()),
                Some(code),
                "{name} 应大小写不敏感"
            );
        }
        assert_eq!(key_code("122"), Some(122), "纯数字 keycode 透传");
        assert_eq!(key_code("NOT_A_KEY"), None, "未知键不再降级为 keycode 0");
    }

    /// key 步骤字面量非法键：入口装载即拦截，表现为运行失败且报错含键名
    /// （此前是 warn + keycode 0 静默发送）。
    #[tokio::test]
    async fn key_step_unknown_key_fails_run() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "error");
        r.save_script("f.yaml", "steps:\n  - key: NOT_A_KEY\n");
        let err = r.run(script_target("f.yaml"), vec![]).await.unwrap_err();
        assert!(err.to_string().contains("NOT_A_KEY"), "{err:#}");
        assert!(
            !r.ctl.calls().iter().any(|c| c.starts_with("key ")),
            "不得发送任何 keycode: {:?}",
            r.ctl.calls()
        );
    }

    /// args 显式传入非法 key（TypedValue 绕过装载层字面量拦截的路径）：
    /// 运行期 key_code 兜底让步骤失败，不再降级为 keycode 0。
    #[tokio::test]
    async fn key_args_unknown_key_fails_run() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "error");
        r.save_script(
            "f.yaml",
            "params:\n  - 'key:quit:退出按键:ESC'\nsteps:\n  - key: $quit\n",
        );
        let err = r
            .run(
                script_target("f.yaml"),
                vec![("quit", TypedValue::Key("BOGUS".into()))],
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("BOGUS"), "{err:#}");
        assert!(
            !r.ctl.calls().iter().any(|c| c.starts_with("key ")),
            "不得发送任何 keycode: {:?}",
            r.ctl.calls()
        );
    }

    // ---- call / func / throw / return ----------------------------------------

    /// call：目标 config 三键压栈（log_level=debug 生效）→ 返回后恢复调用者
    /// config（调用者的 wait debug 日志重新被 info 过滤）。
    #[tokio::test]
    async fn call_pushes_config_and_restores_on_return() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.save_script(
            "sub.yaml",
            "config:\n  interval: 20ms\n  threshold: 0.85\n  log_level: debug\nsteps:\n  - wait: 5ms\n  - log: 子脚本信息\n",
        );
        r.save_script(
            "main.yaml",
            "config:\n  interval: 20ms\n  threshold: 0.85\n  log_level: info\nsteps:\n  - wait: 5ms\n  - call: sub.yaml\n  - wait: 5ms\n  - log: 主脚本信息\n",
        );
        let logs = r.run(script_target("main.yaml"), vec![]).await.unwrap();
        let debug_waits = logs
            .iter()
            .filter(|(l, m)| l == "debug" && m.starts_with("等待"))
            .count();
        assert_eq!(
            debug_waits, 1,
            "只有子脚本内的 wait debug 日志可见（config 压栈生效、返回恢复）"
        );
        assert!(logs_contain(&logs, "子脚本信息"));
        assert!(logs_contain(&logs, "主脚本信息"));
    }

    /// call：声明默认值打底 + 具名实参覆盖（七类类型化绑定）。
    #[tokio::test]
    async fn call_binds_defaults_then_overrides() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.save_script(
            "sub.yaml",
            "params:\n  - 'text:msg:消息:\"默认消息\"'\n  - 'coord:pos:位置:[0.1, 0.1]'\nsteps:\n  - log: $msg\n  - tap: $pos\n",
        );
        r.save_script(
            "main.yaml",
            "steps:\n  - call: sub.yaml\n    args:\n      msg: \"显式消息\"\n      pos: [0.9, 0.9]\n",
        );
        let logs = r.run(script_target("main.yaml"), vec![]).await.unwrap();
        assert!(logs_contain(&logs, "显式消息"));
        assert_eq!(r.ctl.calls(), vec!["tap 900 450"]);
        // 缺省调用 → 全默认值
        r.save_script("main2.yaml", "steps:\n  - call: sub.yaml\n");
        let logs = r.run(script_target("main2.yaml"), vec![]).await.unwrap();
        assert!(logs_contain(&logs, "默认消息"));
        assert_eq!(r.ctl.calls(), vec!["tap 900 450", "tap 100 50"]);
    }

    /// throw 跨调用链结束整个运行（失败终态），调用链后续步骤不再执行。
    #[tokio::test]
    async fn throw_crosses_call_chain_and_aborts() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.save_script(
            "sub.yaml",
            "steps:\n  - log: 到这了\n  - throw: 目标不可达\n",
        );
        r.save_script("main.yaml", "steps:\n  - call: sub.yaml\n  - log: 不可达\n");
        let err = r.run(script_target("main.yaml"), vec![]).await.unwrap_err();
        assert!(err.to_string().contains("脚本 throw 终止"), "{err:#}");
    }

    /// return 只退出当前函数：false → else 分支；return 后函数体内后续步骤短路；
    /// 调用点继续执行。
    #[tokio::test]
    async fn return_exits_function_only_and_drives_then_else() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.save_func(
            "check",
            "is_ready:\n  params:\n    - 'bool:ok:是否就绪'\n  steps:\n    - return: $ok\n    - log: 不可达\n",
        );
        r.save_script(
            "main.yaml",
            "steps:\n  - func: check/is_ready\n    args:\n      ok: false\n    then:\n      - log: T-分支\n    else:\n      - log: F-分支\n  - log: 调用点继续\n",
        );
        let logs = r.run(script_target("main.yaml"), vec![]).await.unwrap();
        assert!(logs_contain(&logs, "F-分支"));
        assert!(!logs_contain(&logs, "T-分支"));
        assert!(!logs_contain(&logs, "不可达"), "return 后函数体必须短路");
        assert!(logs_contain(&logs, "调用点继续"));
    }

    /// 函数体走完未 return → 默认返回 true（then 分支）。
    #[tokio::test]
    async fn function_fallthrough_defaults_true() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.save_func("noop", "always:\n  steps:\n    - log: 函数体\n");
        r.save_script(
            "main.yaml",
            "steps:\n  - func: noop/always\n    then:\n      - log: then 分支\n    else:\n      - log: else 分支\n",
        );
        let logs = r.run(script_target("main.yaml"), vec![]).await.unwrap();
        assert!(logs_contain(&logs, "then 分支"));
        assert!(!logs_contain(&logs, "else 分支"));
    }

    /// 同文件函数递归 → 32 层嵌套 guard（静态引用图不拦同文件递归）。
    #[tokio::test]
    async fn same_file_function_recursion_hits_depth_guard() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.save_func("rec", "forever:\n  steps:\n    - func: rec/forever\n");
        r.save_script("main.yaml", "steps:\n  - func: rec/forever\n");
        let err = r.run(script_target("main.yaml"), vec![]).await.unwrap_err();
        assert!(err.to_string().contains("嵌套超过 32 层"), "{err:#}");
    }

    // ---- 入口参数绑定 / start_index -------------------------------------------

    /// 入口必填缺失 → 结构化诊断失败；稀疏覆盖生效（coord 类型化实参）。
    #[tokio::test]
    async fn entry_args_required_and_sparse_override() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.save_script(
            "f.yaml",
            "params:\n  - 'coord:pos:位置'\n  - 'text:msg:消息:\"默认文本\"'\nsteps:\n  - tap: $pos\n  - log: $msg\n",
        );
        // 必填缺失
        let err = r.run(script_target("f.yaml"), vec![]).await.unwrap_err();
        assert!(err.to_string().contains("必填参数 pos"), "{err:#}");
        // 稀疏覆盖：只给必填项，msg 走默认
        let logs = r
            .run(
                script_target("f.yaml"),
                vec![("pos", TypedValue::Coord([0.25, 0.75]))],
            )
            .await
            .unwrap();
        assert_eq!(r.ctl.calls(), vec!["tap 250 375"]);
        assert!(logs_contain(&logs, "默认文本"));
    }

    /// start_index：0<n<len 从该步执行；越界回退从头。
    #[tokio::test]
    async fn start_index_slices_entry_steps() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.save_script("f.yaml", "steps:\n  - log: 一\n  - log: 二\n  - log: 三\n");
        let logs = r
            .run(
                RunTarget::Script {
                    script_id: format!("{PKG}/f.yaml"),
                    start_index: 1,
                },
                vec![],
            )
            .await
            .unwrap();
        assert!(!logs_contain(&logs, "一"));
        assert!(logs_contain(&logs, "二"));
        assert!(logs_contain(&logs, "三"));
        // 越界回退从头
        let logs = r
            .run(
                RunTarget::Script {
                    script_id: format!("{PKG}/f.yaml"),
                    start_index: 99,
                },
                vec![],
            )
            .await
            .unwrap();
        assert!(logs_contain(&logs, "一"));
    }

    /// 函数测试入口（RunTarget::Function）：缺省第一个函数；指定函数 + args；
    /// 不存在的函数报错。
    #[tokio::test]
    async fn function_entry_binds_args_and_resolves_first_function_by_default() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.save_func(
            "lib",
            "first:\n  steps:\n    - log: 第一个函数\nsecond:\n  params:\n    - 'text:who:称呼'\n  steps:\n    - log: $who\n",
        );
        // function 缺省 = 第一个函数
        let logs = r
            .run(
                RunTarget::Function {
                    pkg: PKG.into(),
                    file: "lib".into(),
                    function: None,
                    start_index: 0,
                },
                vec![],
            )
            .await
            .unwrap();
        assert!(logs_contain(&logs, "第一个函数"));
        // 指定函数 + 必填 args
        let logs = r
            .run(
                RunTarget::Function {
                    pkg: PKG.into(),
                    file: "lib".into(),
                    function: Some("second".into()),
                    start_index: 0,
                },
                vec![("who", TypedValue::Text("引擎".into()))],
            )
            .await
            .unwrap();
        assert!(logs_contain(&logs, "引擎"));
        // 不存在的函数 → 报错
        let err = r
            .run(
                RunTarget::Function {
                    pkg: PKG.into(),
                    file: "lib".into(),
                    function: Some("nope".into()),
                    start_index: 0,
                },
                vec![],
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("不存在"), "{err:#}");
    }

    // ---- 基础动作 / 取消 -------------------------------------------------------

    /// str_app（+包名冷启动）/ cls_app（adb force-stop，包名 = 运行分区）。
    #[tokio::test]
    async fn str_app_and_cls_app_use_partition_pkg() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.save_script("f.yaml", "steps:\n  - str_app\n  - cls_app\n");
        r.run(script_target("f.yaml"), vec![]).await.unwrap();
        assert_eq!(
            r.ctl.calls(),
            vec![
                format!("start_app +{PKG}"),
                format!("shell fake-serial am force-stop {PKG}")
            ]
        );
    }

    /// 取消：find 轮询中置 stop → 正常返回（不进 else、不误报超时）。
    #[tokio::test]
    async fn cancel_stops_find_polling_without_else() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.tmpl("main.png");
        r.save_script(
            "f.yaml",
            "steps:\n  - find: main.png\n    timeout: 30min\n    else:\n      - log: 不应执行 else\n",
        );
        let stop = Arc::new(AtomicBool::new(false));
        let task = {
            let stop = stop.clone();
            let r = r.clone();
            tokio::spawn(
                async move { r.run_with_stop(script_target("f.yaml"), vec![], stop).await },
            )
        };
        tokio::time::sleep(Duration::from_millis(150)).await;
        stop.store(true, Ordering::SeqCst);
        let logs = task.await.unwrap().unwrap();
        assert!(!logs_contain(&logs, "不应执行 else"));
        assert!(!logs_contain(&logs, "等待模板 main.png 超时（"));
        assert!(r.ctl.calls().is_empty());
    }

    // ---- 快照隔离 --------------------------------------------------------------

    /// 运行开始后修改 call 目标文件：本实例仍用开始时的内容；下一次运行用新内容。
    #[tokio::test]
    async fn snapshot_isolates_running_instance_from_file_changes() {
        let r = rig(HashMap::new(), solid_png(10, 10, [0, 0, 0]), "info");
        r.save_script("sub.yaml", "steps:\n  - log: 旧版本\n");
        r.save_script("main.yaml", "steps:\n  - wait: 250ms\n  - call: sub.yaml\n");
        let stop = Arc::new(AtomicBool::new(false));
        let task = {
            let stop = stop.clone();
            let r = r.clone();
            tokio::spawn(async move {
                r.run_with_stop(script_target("main.yaml"), vec![], stop)
                    .await
            })
        };
        // 运行开始后（wait 窗口内）改写 call 目标
        tokio::time::sleep(Duration::from_millis(80)).await;
        r.save_script("sub.yaml", "steps:\n  - log: 新版本\n");
        let logs = task.await.unwrap().unwrap();
        assert!(
            logs_contain(&logs, "旧版本"),
            "运行中实例必须使用开始时的快照"
        );
        assert!(!logs_contain(&logs, "新版本"));
        // 下一次运行生效
        let logs = r.run(script_target("main.yaml"), vec![]).await.unwrap();
        assert!(logs_contain(&logs, "新版本"));
    }

    // ---- 契约 golden 端到端（tests/fixtures/script_v2 v01~v12）-----------------

    const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/script_v2");

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{FIXTURE_DIR}/{name}")).unwrap()
    }

    /// 按真实分区布局落盘 golden：脚本、函数库、全部被引用模板（fake matcher
    /// 不读模板内容，占位即可；命中表由各用例单独给）。
    fn setup_golden(rig: &Rig, scripts: &[(&str, &str)], funcs: &[(&str, &str)]) {
        for (name, file) in scripts {
            rig.store.save(None, PKG, name, &fixture(file)).unwrap();
        }
        for (name, file) in funcs {
            rig.store.save_function(PKG, name, &fixture(file)).unwrap();
        }
        for tpl in [
            "account.png",
            "retry.png",
            "popup.png",
            "dialog.png",
            "test1.png",
            "test2.png",
            "record_click_20260829_001.png",
            "record_swipe_20260829_002.png",
            "icon.png",
        ] {
            rig.tmpl(tpl);
        }
    }

    /// v01：最小脚本 log + tap。
    #[tokio::test]
    async fn golden_v01_minimal_script() {
        let r = rig(HashMap::new(), solid_png(1000, 500, [0, 0, 0]), "info");
        setup_golden(&r, &[("v01.yaml", "v01_minimal_script.yaml")], &[]);
        let logs = r
            .run(
                RunTarget::Script {
                    script_id: format!("{PKG}/v01.yaml"),
                    start_index: 0,
                },
                vec![],
            )
            .await
            .unwrap();
        assert!(logs_contain(&logs, "最小脚本"));
        assert_eq!(r.ctl.calls(), vec!["tap 500 250"]);
    }

    /// v02：全动作轨迹（str_app → tap → swipe → key → text → log → wait →
    /// wait 随机 → cls_app）。
    #[tokio::test]
    async fn golden_v02_all_actions() {
        let r = rig(HashMap::new(), solid_png(1000, 500, [0, 0, 0]), "debug");
        setup_golden(&r, &[("v02.yaml", "v02_all_actions.yaml")], &[]);
        let logs = r
            .run(
                RunTarget::Script {
                    script_id: format!("{PKG}/v02.yaml"),
                    start_index: 0,
                },
                vec![],
            )
            .await
            .unwrap();
        let calls = r.ctl.calls();
        assert_eq!(calls[0], format!("start_app +{PKG}"));
        assert_eq!(calls[1], "tap 500 250");
        assert_eq!(calls[2], "swipe 100 450 900 50 800");
        assert_eq!(calls[3], "key 111", "ESC keycode");
        assert_eq!(calls[4], "text hello world");
        assert_eq!(
            *calls.last().unwrap(),
            format!("shell fake-serial am force-stop {PKG}")
        );
        assert!(logs_contain(&logs, "全动作脚本"));
        // 两次 wait（1s + [1s,3s] 随机）都有分片等待日志
        assert_eq!(
            logs.iter().filter(|(_, m)| m.starts_with("等待 ")).count(),
            2
        );
    }

    /// v03：函数库作为 RunTarget::Function 运行（默认函数 + 参数默认值）。
    #[tokio::test]
    async fn golden_v03_function_library() {
        let r = rig(
            HashMap::from([("account.png", [100u32, 100, 200, 100])]),
            solid_png(1000, 500, [0, 0, 0]),
            "info",
        );
        setup_golden(&r, &[], &[("v03", "v03_function_library.yaml")]);
        let logs = r
            .run(
                RunTarget::Function {
                    pkg: PKG.into(),
                    file: "v03".into(),
                    function: Some("login".into()),
                    start_index: 0,
                },
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(
            r.ctl.calls(),
            vec!["tap 200 150"],
            "find $account 命中点中心"
        );
        assert!(logs_contain(&logs, "account.png 已找到"));
    }

    /// v04：七类参数默认值全链路（config 三键 + $ref 点击/等待/按键/日志/取色）。
    #[tokio::test]
    async fn golden_v04_params_all_defaults() {
        let r = rig(
            HashMap::from([("account.png", [100u32, 100, 200, 100])]),
            solid_png(1000, 500, [0xff, 0x88, 0x00]),
            "info",
        );
        setup_golden(&r, &[("v04.yaml", "v04_params_all_defaults.yaml")], &[]);
        let logs = r
            .run(
                RunTarget::Script {
                    script_id: format!("{PKG}/v04.yaml"),
                    start_index: 0,
                },
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(r.ctl.calls()[0], "tap 500 400", "$click_pos 默认值");
        assert_eq!(r.ctl.calls()[1], "tap 200 150", "find $account 命中");
        assert!(
            r.ctl.calls().contains(&"key 111".to_string()),
            "$cancel_key"
        );
        assert!(logs_contain(&logs, "示例文本"), "$message 默认值");
        assert!(logs_contain(&logs, "参数色命中"), "$target_color 引用分支");
        assert!(!logs_contain(&logs, "字面色命中"));
        assert!(!logs_contain(&logs, "都未命中"));
    }

    /// v05：七类参数全必填，args 稀疏覆盖驱动执行。
    #[tokio::test]
    async fn golden_v05_params_all_required() {
        let r = rig(
            HashMap::from([("account.png", [100u32, 100, 200, 100])]),
            solid_png(1000, 500, [0x12, 0x34, 0x56]),
            "info",
        );
        setup_golden(&r, &[("v05.yaml", "v05_params_all_required.yaml")], &[]);
        let logs = r
            .run(
                RunTarget::Script {
                    script_id: format!("{PKG}/v05.yaml"),
                    start_index: 0,
                },
                vec![
                    ("account", TypedValue::Tmpl("account.png".into())),
                    ("click_pos", TypedValue::Coord([0.5, 0.5])),
                    ("target_color", TypedValue::Color("123456".into())),
                    ("timeout", TypedValue::Time("30s".into())),
                    ("cancel_key", TypedValue::Key("ESC".into())),
                    ("message", TypedValue::Text("必填文本".into())),
                    ("enable", TypedValue::Bool(true)),
                ],
            )
            .await
            .unwrap();
        assert!(!logs.is_empty(), "全必填 args 运行成功即有日志");
        assert_eq!(r.ctl.calls()[0], "tap 500 250");
        assert_eq!(r.ctl.calls()[1], "tap 200 150");
        assert_eq!(
            r.ctl.calls()[2],
            "key 111",
            "$target_color 命中后 $cancel_key"
        );
        assert!(r.ctl.calls().contains(&"text 必填文本".to_string()));
    }

    /// v06：loop(3)×if 布尔分支 + 无限 loop 被 stop 取消（正常返回）。
    #[tokio::test]
    async fn golden_v06_nested_if_loop_with_cancel() {
        let r = rig(HashMap::new(), solid_png(1000, 500, [0, 0, 0]), "info");
        setup_golden(&r, &[("v06.yaml", "v06_nested_if_loop.yaml")], &[]);
        let stop = Arc::new(AtomicBool::new(false));
        let task = {
            let stop = stop.clone();
            let r = r.clone();
            tokio::spawn(async move {
                r.run_with_stop(
                    RunTarget::Script {
                        script_id: format!("{PKG}/v06.yaml"),
                        start_index: 0,
                    },
                    vec![],
                    stop,
                )
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(300)).await;
        stop.store(true, Ordering::SeqCst);
        let logs = task.await.unwrap().unwrap();
        assert_eq!(
            logs.iter().filter(|(_, m)| m == "无障碍物").count(),
            3,
            "retry=false 走 else 三轮"
        );
        assert!(!logs_contain(&logs, "清理障碍"));
    }

    /// v07：match 紧凑缩进（解析）+ 单帧有序候选（执行）：test2 命中。
    #[tokio::test]
    async fn golden_v07_match_compact() {
        let r = rig(
            HashMap::from([("test2.png", [10u32, 10, 50, 40])]),
            solid_png(1000, 500, [0, 0, 0]),
            "info",
        );
        setup_golden(&r, &[("v07.yaml", "v07_match_compact.yaml")], &[]);
        let logs = r
            .run(
                RunTarget::Script {
                    script_id: format!("{PKG}/v07.yaml"),
                    start_index: 0,
                },
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(r.shots.count(), 1);
        assert_eq!(r.matcher.calls(), vec!["test1.png", "test2.png"]);
        assert!(logs_contain(&logs, "命中 test2"));
        assert!(!logs_contain(&logs, "命中 test1"));
        assert!(!logs_contain(&logs, "都未命中"));
    }

    /// v08：color 分支 + else 不含 throw（命中即不触发）。
    #[tokio::test]
    async fn golden_v08_color_branch() {
        let r = rig(
            HashMap::new(),
            solid_png(1000, 500, [0xff, 0x88, 0x00]),
            "info",
        );
        setup_golden(&r, &[("v08.yaml", "v08_color_branch.yaml")], &[]);
        let logs = r
            .run(
                RunTarget::Script {
                    script_id: format!("{PKG}/v08.yaml"),
                    start_index: 0,
                },
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(r.ctl.calls(), vec!["tap 500 250"], "ff8800 分支点击");
        assert!(!logs_contain(&logs, "深蓝分支"));
        assert!(!logs_contain(&logs, "颜色未命中"));
    }

    /// v09：call 带类型化 args（$ref 与字面量）+ 目标延迟默认值。
    #[tokio::test]
    async fn golden_v09_call_script() {
        let r = rig(HashMap::new(), solid_png(1000, 500, [0, 0, 0]), "info");
        setup_golden(
            &r,
            &[
                ("v09.yaml", "v09_call_script.yaml"),
                ("v09_call_script.target.yaml", "v09_call_script.target.yaml"),
            ],
            &[],
        );
        let logs = r
            .run(
                RunTarget::Script {
                    script_id: format!("{PKG}/v09.yaml"),
                    start_index: 0,
                },
                vec![],
            )
            .await
            .unwrap();
        assert!(logs_contain(&logs, "来自父脚本"), "$ref 实参");
        assert!(logs_contain(&logs, "字面量消息"), "字面量实参");
        // 目标内 if $enable（首次 true）→ tap 一次
        assert_eq!(r.ctl.calls(), vec!["tap 500 250"]);
    }

    /// v10：跨文件函数调用 func: common/login + then/else + return。
    #[tokio::test]
    async fn golden_v10_func_call_cross_file() {
        let r = rig(
            HashMap::from([("account.png", [100u32, 100, 200, 100])]),
            solid_png(1000, 500, [0, 0, 0]),
            "info",
        );
        setup_golden(
            &r,
            &[("v10.yaml", "v10_func_call_cross_file.yaml")],
            &[("common", "v10_func_call_cross_file.common.yaml")],
        );
        let logs = r
            .run(
                RunTarget::Script {
                    script_id: format!("{PKG}/v10.yaml"),
                    start_index: 0,
                },
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(r.ctl.calls(), vec!["tap 200 150"]);
        assert!(logs_contain(&logs, "登录成功"), "return true → then");
        assert!(!logs_contain(&logs, "登录失败"));
    }

    /// v11：录制输出形态 find + match → swipe。
    #[tokio::test]
    async fn golden_v11_record_output() {
        let r = rig(
            HashMap::from([
                ("record_click_20260829_001.png", [100u32, 100, 100, 100]),
                ("record_swipe_20260829_002.png", [400u32, 300, 100, 100]),
            ]),
            solid_png(1000, 500, [0, 0, 0]),
            "info",
        );
        setup_golden(&r, &[("v11.yaml", "v11_record_output.yaml")], &[]);
        let logs = r
            .run(
                RunTarget::Script {
                    script_id: format!("{PKG}/v11.yaml"),
                    start_index: 0,
                },
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(
            r.ctl.calls(),
            vec!["tap 150 150", "swipe 500 400 500 100 800",]
        );
        let _ = logs;
    }

    /// v12：任务快照参数形态（全默认值）端到端执行。
    #[tokio::test]
    async fn golden_v12_task_args_snapshot() {
        let r = rig(
            HashMap::from([("icon.png", [100u32, 100, 200, 100])]),
            solid_png(1000, 500, [0, 0, 0]),
            "info",
        );
        setup_golden(&r, &[("v12.yaml", "v12_task_args_snapshot.yaml")], &[]);
        let logs = r
            .run(
                RunTarget::Script {
                    script_id: format!("{PKG}/v12.yaml"),
                    start_index: 0,
                },
                vec![],
            )
            .await
            .unwrap();
        assert!(logs_contain(&logs, "开始任务"), "$message 默认值");
        assert_eq!(
            r.ctl.calls(),
            vec!["tap 500 250", "tap 200 150"],
            "$pos 点击 + find $icon 命中"
        );
    }
}
