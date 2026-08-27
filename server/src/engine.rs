//! YAML 自动化脚本引擎（2026-08-26 语法精简重写，不兼容旧语法）
//!
//! 顶层键只允许 config / func / steps（未知顶层键报错，顺带拦住
//! action_wait / log_level / name 残留）：
//!   config:  可选，覆盖 config.toml 默认；mapping 或 mapping 列表（按序
//!            覆盖）均可；键 = interval / threshold / log_level
//!   func:    可选，自定义函数定义（见下）
//!   steps:   必需，步骤列表
//! 单段脚本可省略段落键直接写内容（normalize_top 归一化，2026-08-27）：
//!   顶层序列 = steps；顶层映射且不含 config/func/steps 任何键 = func
//!   （纯函数库简写；config 不能省略——其子键不是函数名，无法判定归属）。
//!
//! 动作（步骤键，一个步骤只能有一个动作键）：
//!   find:  找图轮询（取代旧 until）。`- find: 主模板`（单个字符串）+ 兄弟键：
//!          block（障碍模板：单模板字符串 / 逗号分隔 / 列表；与主模板重复报错；
//!          每轮主模板未命中后依序尝试，命中即点击其中心并结束本轮）、
//!          verify（bool 默认 false：命中点击后等 interval 重匹配主模板，
//!          仍命中再补一击——共两击，不循环）、timeout（默认 30min，必须 > 0）、
//!          then（命中执行）/ else（超时执行）。
//!          每轮：主模板（新截图）命中 → 点中心 + verify + then 结束；未命中 →
//!          block 依序（命中点中心结束本轮）→ 全未命中等 interval 重开一轮。
//!          所有模板命中都点击中心；threshold 全局（config）；region 由模板名
//!          #后缀 决定（无后缀回退全屏并记一条日志提醒）。
//!          ^1 = 主模板名、^2.. = block 名（依序），then/else 子树内可引用。
//!   color: 找色分支（取代旧 cond，颜色条件之外的形式已删除）。
//!          `- color: [x, y]`（相对坐标）+ 兄弟键 = 6 位十六进制色值（宽容
//!          # / 0x 前缀；容差固定 30/通道）挂命中步骤（可留空）+ else（全未
//!          命中执行）。一次截图按序判定，命中一个执行其步骤结束本步；
//!          不轮询无超时（重试套 loop）。^1 = "[x, y]" 坐标串、^2.. = 色值键
//!          （书写顺序），命中步骤/else 子树内可引用。
//!   loop:  `- loop:` + times（默认 0 = 无限循环）+ steps（必需步骤列表）。
//!   tap / swipe / key / text / log / wait / call / throw / str_app / cls_app：
//!          tap [x,y]（相对）；swipe {fm, to, time}（time 默认 500ms，书写必须
//!          带单位）；wait 2s 或 [1s, 3s] 随机（强制带单位）；call
//!          `子脚本.yml 实参…`（空格分隔 + 括号感知：[x, y] 内部不切分；子脚本
//!          内 $N 引用实参）；throw（原 exit 改名）立即结束整个任务（跨 call）；
//!          str_app / cls_app 只支持裸写（包名 = 设备分区 pkg）。
//!   return: 仅自定义函数内合法，`- return: true|false` 立即返回；
//!          函数体执行完未 return 视为返回 true（2026-08-27 改，旧语义为 false）。
//!
//! func 自定义函数：
//!   func:
//!     - func1:            # 函数名不能是保留字
//!       cond: gate.png    # 可选：执行条件模板（单模板字符串 / 逗号分隔 / 列表）
//!       steps:            # 函数体（与 cond 同为函数定义兄弟键）
//!         - find: $1
//!   调用：`- func1: 实参1 实参2`（空格分隔 + 括号感知；无参写 `- func1:`）+
//!         then（返回 true 执行）/ else（返回 false 执行）。函数体内 $N 指函数
//!         实参（func 段不参与脚本级 $N 替换）；函数体可用全部动作含 call/throw；
//!         嵌套调用上限 32 层。
//!   cond 语义：每个条件模板各取一张新截图匹配一次（不点击），全部命中才执行
//!         函数体；任一未命中 → 函数返回 false。函数体走完未 return → 返回 true。
//!   跨文件调用：`- 子脚本:函数名: 实参…`（子脚本名与 call 同规则解析：优先
//!         同分区、缺扩展名自动补全），函数体与 cond 取自该脚本 func 段。
//!
//! 时间参数（interval / timeout / wait / swipe time）统一强制带单位：
//!   1ms / 1s / 1m / 30min / 1h / 1d（m ≡ min，可小数如 1.5s），裸数字报错。
//! config.toml 默认：interval = "500ms"、threshold = 0.85、log_level = "info"
//!   （debug|info|warn|error，低于配置等级的日志丢弃）；脚本 config: 段可覆盖。
//! interval 只作用于轮询类等待（find 每轮重试、verify 复查）；步骤之间不再
//!   统一等待（旧 action_wait / 步骤级 wait 参数已删除）。
//!
//! 模板引用：短名唯一匹配（写 login.png 引用 login#*.png）；#后缀 = 搜索区域
//!   （半区码 a/u/d/l/r/ul/ur/dl/dr 或 xx#x1_y1_x2_y2 ×1000 相对坐标）；
//!   无 #后缀回退全屏（#a 语义）并记一条日志提醒（每次运行每模板一条）。
//!
//! 已删除（写了显式报错引导迁移）：until（→find）、check（→block）、cond
//!   （→color）、exit（→throw）、goto/label、count/cnt_ivl/cnt_chk/img_ivl/
//!   and_or/click、before/after、步骤级 threshold/region/wait、顶层
//!   action_wait/log_level/name。
//!
//! 找图：截图（帧缓存优先）→ 模板匹配（NCC）
//!
//! 可视化事件：tap/swipe/匹配命中时经 control DataChannel 推送给浏览器投屏页面
//!   （emit → ViewerMap 查当前 viewer；无 viewer 时静默丢弃）

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_recursion::async_recursion;
use image::GenericImageView;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use tracing::warn;

use crate::device::DeviceManager;
use crate::matcher;
use crate::scripts::ScriptStore;
use crate::webrtc::ViewerMap;

/// find 未显式指定 timeout 时的默认超时毫秒数（30 分钟）
const FIND_DEFAULT_TIMEOUT_MS: u64 = 1_800_000;

/// 自定义函数嵌套调用上限（防无限递归）
const MAX_FUNC_DEPTH: usize = 32;

/// 脚本运行可视化事件（服务端 → 浏览器，经 control DataChannel，JSON 格式 {"type":"se","ev":...}）
/// 注意 rename_all="snake_case"：内部标签默认用变体名原样（"Tap"），
/// 前端按小写 "tap"/"swipe"/"hit"/"miss" 匹配（曾因大小写不匹配事件全部被忽略）
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum ScriptEvent {
    /// 引擎点击（设备像素坐标）
    Tap { x: u32, y: u32 },
    /// 引擎滑动（设备像素坐标）
    Swipe { x1: u32, y1: u32, x2: u32, y2: u32 },
    /// 模板匹配命中（设备像素坐标 + 置信度）
    Hit {
        tpl: String,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        score: f32,
    },
    /// 模板匹配未命中（可视化本次搜索区域，设备像素坐标；
    /// 调试定位"在哪找、没找到"——轮询期内每轮刷新，命中前持续可见）
    Miss {
        tpl: String,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    },
}

/// 运行器
pub struct Runner {
    pub devices: Arc<DeviceManager>,
    /// 每设备活跃 viewer 注册表：脚本 tap/swipe/命中可视化事件推送用
    pub viewers: ViewerMap,
    /// 脚本文件存储（data/<pkg>/yaml/，按应用分区）：call 子脚本解析用
    pub scripts: Arc<ScriptStore>,
}

/// 自定义函数定义：可选 cond 模板条件 + 函数体
#[derive(Clone, Debug)]
pub struct FuncDef {
    /// 函数执行条件模板（cond: 单模板字符串 / 逗号分隔 / 列表）；每个模板各取
    /// 一张新截图匹配一次（不点击），全部命中才执行函数体；任一未命中 → 返回 false
    pub cond: Vec<String>,
    /// 函数体步骤（未经 $N 替换，调用时按实参替换）
    pub body: Vec<Value>,
}

/// 脚本运行上下文
pub struct Ctx {
    pub device_id: String,
    pub script_id: String,
    pub log: Vec<(String, String)>, // (level, msg)
    pub stop: Arc<AtomicBool>,
    /// throw 动作已触发（跨 call 子脚本共享）：run 主循环据此提前结束整个脚本运行
    pub exit: Arc<AtomicBool>,
    /// 轮询类间隔（find 每轮重试 / verify 复查），config: 段 > config.toml
    pub interval_ms: u64,
    /// 模板匹配阈值，config: 段 > config.toml
    pub threshold: f32,
    /// 日志等级（0=debug 1=info 2=warn 3=error），低于该等级的日志丢弃
    pub log_level_rank: u8,
    /// 本脚本文件内定义的自定义函数（函数体/条件未经 $N 替换，调用时按实参替换）
    pub funcs: HashMap<String, FuncDef>,
    /// 当前函数嵌套深度（防无限递归）
    pub func_depth: usize,
    /// return 动作的返回值（Some = 正在向上冒泡结束函数）
    pub return_value: Option<bool>,
    /// ^N 上下文绑定栈：find/color 的 then/else 子树执行期间压栈，
    /// 栈顶（最内层）绑定生效
    pub ref_stack: Vec<Vec<String>>,
    /// 已提醒过"无 #区域后缀回退全屏"的模板（每次运行每模板一条）
    pub region_warned: HashSet<String>,
    pub log_cb: Option<Arc<dyn Fn(String, String) + Send + Sync>>,
}

impl Ctx {
    /// 日志等级 → 排序值（success 视同 info：info 级即可见）
    fn level_rank(level: &str) -> u8 {
        match level {
            "debug" => 0,
            "info" | "success" => 1,
            "warn" => 2,
            _ => 3,
        }
    }

    /// 配置字符串 → 等级排序值
    fn parse_level(s: &str) -> Option<u8> {
        match s.trim().to_ascii_lowercase().as_str() {
            "debug" => Some(0),
            "info" => Some(1),
            "warn" | "warning" => Some(2),
            "error" => Some(3),
            _ => None,
        }
    }

    /// 记录日志：实时回调（如有）并同时收集到 ctx.log；
    /// 低于配置等级（log_level）的日志丢弃（不回调、不收集）
    fn log(&mut self, level: &str, msg: String) {
        if Self::level_rank(level) < self.log_level_rank {
            return;
        }
        if let Some(cb) = &self.log_cb {
            cb(level.to_string(), msg.clone());
        }
        self.log.push((level.to_string(), msg));
    }
}

impl Runner {
    pub fn new(devices: Arc<DeviceManager>, viewers: ViewerMap, scripts: Arc<ScriptStore>) -> Self {
        Self {
            devices,
            viewers,
            scripts,
        }
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

    /// 运行脚本内容（YAML 文本）
    /// `start_step`：从第几个 step 开始运行（0=从头；超出范围时从头）
    /// `run_func`：Some(函数名) = 不跑顶层 steps，直接运行该函数体（同样受
    ///             start_step 控制；无实参，体内 `$N` 保持字面量不替换）——
    ///             Console「从某行运行」点击函数体内的行时使用；start_step=0
    ///             （点击函数名行）时先检查函数 cond，未命中则不执行函数体
    /// `exit`：throw 动作共享标志（call 子脚本传父脚本的，子脚本里 throw 同样
    ///         结束整个任务；None=新建）
    /// `args`：call 传入的实参（主脚本运行传空）——子脚本内 `$1`/`$2`… 引用
    /// （func 段除外，见 take_funcs_and_substitute）；引用超出实参数量直接报错
    // 参数较多为历史签名 + Console「从某行运行」run_func/start_step 组合所需；
    // 统一 RunManager / 模块拆分（OPTIMIZATION_PLAN 阶段 3/6）时再收敛参数对象
    #[allow(clippy::too_many_arguments)]
    pub async fn run(
        &self,
        device_id: &str,
        script_id: &str,
        content: &str,
        stop: Arc<AtomicBool>,
        log_cb: Option<Arc<dyn Fn(String, String) + Send + Sync>>,
        start_step: usize,
        run_func: Option<&str>,
        exit: Option<Arc<AtomicBool>>,
        args: Vec<String>,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let doc: Value = serde_yaml::from_str(content)?;
        // 顶层段落归一化：单段脚本可省略段落键直接写内容（省略写法与显式
        // 写法经归一化后走同一条解析路径，config 不能省略）
        let mut doc = Self::normalize_top(doc)?;
        // func 段原样取出（函数体内 $N 永远指函数实参，不参与脚本级替换），
        // 其余部分（config / steps）做 $N → 实参全文替换
        let funcs_raw = Self::take_funcs_and_substitute(&mut doc, &args)?;
        let (interval_ms, threshold, log_level_rank) = self.parse_script_config(&doc)?;
        let funcs = Self::parse_funcs(funcs_raw)?;
        // steps 可缺省：纯函数库脚本（只有 func）供其他脚本通过 脚本名:函数名 调用
        let steps_raw = doc.get("steps").and_then(|v| v.as_sequence()).cloned();

        let mut ctx = Ctx {
            device_id: device_id.to_string(),
            script_id: script_id.to_string(),
            log: Vec::new(),
            stop,
            exit: exit.unwrap_or_else(|| Arc::new(AtomicBool::new(false))),
            interval_ms,
            threshold,
            log_level_rank,
            funcs,
            func_depth: 0,
            return_value: None,
            ref_stack: Vec::new(),
            region_warned: HashSet::new(),
            log_cb,
        };

        // 直接运行函数体模式：steps 换成函数体（无实参，$N 保持字面量不替换）。
        // 从头运行（start_step=0，Console 点击函数名行）先检查 cond 条件——
        // cond 未命中时函数体不执行（与正常调用语义一致，cond 逻辑可测试）；
        // start_step>0（点击函数体内某行）跳过 cond 直接从该步执行
        let steps = match run_func {
            Some(name) => {
                let def = ctx.funcs.get(name).cloned().ok_or_else(|| {
                    anyhow::anyhow!("函数 {} 未定义（func: 段里没有该函数名）", name)
                })?;
                ctx.log(
                    "info",
                    format!("直接运行函数 {}（无实参，体内 $N 不替换）", name),
                );
                if start_step == 0 && !self.check_func_cond(&mut ctx, &def.cond).await? {
                    ctx.log("info", format!("函数 {} 条件未命中，函数体不执行", name));
                    Vec::new()
                } else {
                    def.body
                }
            }
            None => match steps_raw {
                Some(s) => s,
                None => {
                    if ctx.funcs.is_empty() {
                        anyhow::bail!("脚本需要 steps 或 func 根节点（纯函数库脚本也至少要定义一个函数，供其他脚本通过 脚本名:函数名 调用）");
                    }
                    ctx.log(
                        "info",
                        "纯函数库脚本（无 steps）：仅提供函数，直接运行不做任何动作".to_string(),
                    );
                    Vec::new()
                }
            },
        };

        let mut i = if start_step > 0 && start_step < steps.len() {
            start_step
        } else {
            0
        };
        let mut guard_count = 0usize;
        while i < steps.len() {
            if ctx.stop.load(Ordering::SeqCst) {
                ctx.log("warn", "脚本被停止".to_string());
                break;
            }
            if ctx.exit.load(Ordering::SeqCst) {
                // throw 动作已在 exec_step 里打印结束日志（含 call 子脚本），这里直接结束
                break;
            }
            guard_count += 1;
            if guard_count > 100_000 {
                anyhow::bail!("脚本执行次数超限，疑似死循环");
            }
            self.exec_step(&mut ctx, &steps[i]).await?;
            if ctx.exit.load(Ordering::SeqCst) {
                break;
            }
            i += 1;
        }

        Ok(ctx.log)
    }

    /// 顶层段落归一化：单段脚本可省略 `steps:` / `func:` 段落键直接写内容，
    /// 按内容形态判定归属（config 不能省略——它的子键 interval/threshold/
    /// log_level 不是函数名，省了无法区分）：
    /// - 顶层**序列** → `steps`（函数定义的列表简写与步骤序列无法区分，省略
    ///   func 时一律用映射形式）
    /// - 顶层**映射**且不含 config/func/steps 任何一键 → 整个映射视为 `func`
    ///   段（纯函数库简写：函数定义直接写在顶层）
    /// - 含任一段落键的映射维持白名单校验（未知顶层键报错）
    ///
    /// 返回带段落键的归一化文档；旧顶层键（action_wait/log_level/name）无论
    /// 何种形态都先定向报错
    fn normalize_top(doc: Value) -> anyhow::Result<Value> {
        match &doc {
            Value::Sequence(_) => {
                let mut m = serde_yaml::Mapping::new();
                m.insert(Value::String("steps".into()), doc);
                Ok(Value::Mapping(m))
            }
            Value::Mapping(m) => {
                for k in m.keys() {
                    match k.as_str() {
                        Some("action_wait") => anyhow::bail!(
                            "顶层 action_wait 已删除：操作间隔统一为 config interval（仅轮询类等待，步骤间不再等待）"
                        ),
                        Some("log_level") => anyhow::bail!("顶层 log_level 已删除：改用 config: 段（config.toml 可配全局默认）"),
                        Some("name") => anyhow::bail!("顶层 name 已删除（脚本名即文件名）"),
                        _ => {}
                    }
                }
                let has_section = m
                    .keys()
                    .any(|k| matches!(k.as_str(), Some("config" | "func" | "steps")));
                if has_section {
                    for k in m.keys() {
                        if !matches!(k.as_str(), Some("config" | "func" | "steps")) {
                            anyhow::bail!(
                                "未知顶层键 {:?}（只支持 config / func / steps；单段简写：顶层序列 = steps，无段落键的顶层映射 = func）",
                                k.as_str()
                            );
                        }
                    }
                    Ok(doc)
                } else {
                    // 省略 func: 的纯函数库简写；config 子键混进顶层是常见笔误，定向报错
                    for k in m.keys() {
                        if matches!(k.as_str(), Some("interval" | "threshold")) {
                            anyhow::bail!(
                                "顶层 {:?} 是 config: 段参数（省略段落键的简写只支持纯 steps 序列或纯 func 函数定义，config 必须写 config: 键）",
                                k.as_str()
                            );
                        }
                    }
                    let mut out = serde_yaml::Mapping::new();
                    out.insert(Value::String("func".into()), doc);
                    Ok(Value::Mapping(out))
                }
            }
            _ => Ok(doc),
        }
    }

    /// 从文档取出 func 段（原样返回，不参与 $N 替换）并对剩余部分做实参替换。
    /// 返回 func 段的值（None = 未定义）
    fn take_funcs_and_substitute(
        doc: &mut Value,
        args: &[String],
    ) -> anyhow::Result<Option<Value>> {
        let funcs = doc.get("func").filter(|v| !v.is_null()).cloned();
        if funcs.is_some() {
            if let Some(m) = doc.as_mapping_mut() {
                let old = std::mem::take(m);
                for (k, v) in old {
                    if k.as_str() != Some("func") {
                        m.insert(k, v);
                    }
                }
            }
        }
        Self::substitute_args(doc, args)?;
        Ok(funcs)
    }

    /// config: 段解析（mapping 或 mapping 列表按序覆盖）：
    /// interval / threshold / log_level，默认取 config.toml 同名键
    fn parse_script_config(&self, doc: &Value) -> anyhow::Result<(u64, f32, u8)> {
        let mut interval_ms = Self::parse_duration(
            &Value::String(self.devices.cfg.interval.clone()),
            "config.toml interval",
        )?;
        if interval_ms == 0 {
            anyhow::bail!("config.toml interval 必须 > 0");
        }
        let mut threshold = self.devices.cfg.threshold;
        let mut level = Ctx::parse_level(&self.devices.cfg.log_level).ok_or_else(|| {
            anyhow::anyhow!(
                "config.toml log_level 需要 debug/info/warn/error，收到: {}",
                self.devices.cfg.log_level
            )
        })?;
        match doc.get("config") {
            None | Some(Value::Null) => {}
            Some(Value::Mapping(m)) => {
                Self::apply_config_map(m, &mut interval_ms, &mut threshold, &mut level)?
            }
            Some(Value::Sequence(seq)) => {
                for (i, item) in seq.iter().enumerate() {
                    let m = item.as_mapping().ok_or_else(|| {
                        anyhow::anyhow!("config 列表第 {} 项需要映射（键值按序覆盖）", i + 1)
                    })?;
                    Self::apply_config_map(m, &mut interval_ms, &mut threshold, &mut level)?;
                }
            }
            Some(_) => anyhow::bail!("config 需要 mapping（或 mapping 列表按序覆盖）"),
        }
        Ok((interval_ms, threshold, level))
    }

    fn apply_config_map(
        m: &serde_yaml::Mapping,
        interval_ms: &mut u64,
        threshold: &mut f32,
        level: &mut u8,
    ) -> anyhow::Result<()> {
        for (k, v) in m {
            match k.as_str() {
                Some("interval") => {
                    let ms = Self::parse_duration(v, "config.interval")?;
                    if ms == 0 {
                        anyhow::bail!("config.interval 必须 > 0（轮询间隔，如 500ms）");
                    }
                    *interval_ms = ms;
                }
                Some("threshold") => {
                    let t = v
                        .as_f64()
                        .ok_or_else(|| anyhow::anyhow!("config.threshold 需要数字（0~1）"))?;
                    if !(0.0..=1.0).contains(&t) || t <= 0.0 {
                        anyhow::bail!("config.threshold 需要在 (0, 1] 之间，收到: {}", t);
                    }
                    *threshold = t as f32;
                }
                Some("log_level") => {
                    let s = v.as_str().ok_or_else(|| {
                        anyhow::anyhow!("config.log_level 需要 debug/info/warn/error 字符串")
                    })?;
                    *level = Ctx::parse_level(s).ok_or_else(|| {
                        anyhow::anyhow!("config.log_level 需要 debug/info/warn/error，收到: {}", s)
                    })?;
                }
                other => anyhow::bail!(
                    "config 不支持的键 {:?}（可用：interval / threshold / log_level）",
                    other
                ),
            }
        }
        Ok(())
    }

    /// func 段解析：mapping（函数名: 步骤列表）或 mapping 列表（每项单键）均可；
    /// 函数名不得是保留字；函数体 = 步骤列表（null 视为空，执行完返回 true）。
    /// 函数可带 cond 条件与 steps 键（与函数名同级的兄弟键，YAML 把映射值同级
    /// 缩进的行解析成兄弟键——与 loop 的 times/steps 同构），也兼容 cond 写在
    /// 函数体之后的兄弟键形式：
    ///   - fun1:
    ///     cond: test.png      # 可选：单模板字符串 / 逗号分隔 / 列表
    ///     steps:
    ///       - find: $1
    ///
    /// 映射形式（func:\n  fun1:\n    cond: …\n    steps: …）cond/steps 嵌套在
    /// 函数名值里，同样支持
    fn parse_funcs(v: Option<Value>) -> anyhow::Result<HashMap<String, FuncDef>> {
        let mut map = HashMap::new();
        let Some(v) = v else { return Ok(map) };
        let items: Vec<Value> = match &v {
            Value::Null => return Ok(map),
            Value::Mapping(m) => m
                .iter()
                .map(|(k, val)| {
                    let mut item = serde_yaml::Mapping::new();
                    item.insert(k.clone(), val.clone());
                    Value::Mapping(item)
                })
                .collect(),
            Value::Sequence(seq) => seq.clone(),
            _ => anyhow::bail!("func 需要 函数名: 步骤列表 的映射或映射列表"),
        };
        // 保留字：动作键 + 结构键 + 已删除的旧动作名（防止函数调用撞上迁移报错）
        const RESERVED: [&str; 26] = [
            "log", "key", "text", "tap", "swipe", "find", "color", "loop", "call", "throw",
            "str_app", "cls_app", "wait", "return", "then", "else", "steps", "times", "block",
            "verify", "timeout", "config", "func", "until", "cond", "exit",
        ];
        for (i, item) in items.iter().enumerate() {
            let m = item.as_mapping().ok_or_else(|| {
                anyhow::anyhow!("func 第 {} 项需要映射（函数名: 步骤列表）", i + 1)
            })?;
            // 分离 cond / steps 兄弟键与函数名键
            let mut name_k: Option<(&Value, &Value)> = None;
            let mut cond_v: Option<&Value> = None;
            let mut steps_v: Option<&Value> = None;
            for (k, val) in m {
                match k.as_str() {
                    Some("cond") => cond_v = Some(val),
                    Some("steps") => steps_v = Some(val),
                    _ => {
                        if name_k.is_some() {
                            anyhow::bail!(
                                "func 第 {} 项需要恰好一个 函数名: 键（收到 {} 个）",
                                i + 1,
                                m.len()
                            );
                        }
                        name_k = Some((k, val));
                    }
                }
            }
            let (name_v, body_v) = match name_k {
                Some(kv) => kv,
                None => {
                    if m.len() == 1 && m.contains_key(Value::String("cond".into())) {
                        anyhow::bail!(
                            "函数名 cond 是保留字（cond 是函数条件参数键）——若这是函数体的步骤，说明函数体缩进不对：函数体要比 \"- 函数名:\" 行多缩进（如 4 空格）"
                        );
                    }
                    anyhow::bail!("func 第 {} 项缺少函数名键（收到 {} 个键）", i + 1, m.len());
                }
            };
            let name = name_v
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("func 的函数名需要字符串"))?
                .to_string();
            if RESERVED.contains(&name.as_str()) {
                anyhow::bail!(
                    "函数名 {} 是保留字（动作键 / 结构键）——若这是函数体的步骤，说明函数体缩进不对：函数体要比 \"- 函数名:\" 行多缩进（如 4 空格）",
                    name
                );
            }
            // 函数体判定：函数名键值为序列（旧写法）/ 空（找 steps 兄弟键）/
            // 映射（映射形式嵌套 cond/steps）
            let (body, cond_v) = match body_v {
                Value::Sequence(seq) => {
                    if steps_v.is_some() {
                        anyhow::bail!(
                            "函数 {} 的函数体既直接挂在函数名键，又写了 steps 键（只用一种写法）",
                            name
                        );
                    }
                    (seq.clone(), cond_v)
                }
                Value::Null => {
                    let b = match steps_v {
                        None | Some(Value::Null) => Vec::new(),
                        Some(Value::Sequence(seq)) => seq.clone(),
                        Some(_) => anyhow::bail!("函数 {} 的 steps 需要步骤列表", name),
                    };
                    (b, cond_v)
                }
                Value::Mapping(mm) => {
                    let mut nested_cond = None;
                    let mut nested_steps = None;
                    for (k, val) in mm {
                        match k.as_str() {
                            Some("cond") => nested_cond = Some(val),
                            Some("steps") => nested_steps = Some(val),
                            Some(other) => anyhow::bail!(
                                "函数 {} 的函数体映射只支持 cond / steps 键（收到 {}）",
                                name,
                                other
                            ),
                            None => anyhow::bail!("函数 {} 的函数体映射键需要字符串", name),
                        }
                    }
                    let b = match nested_steps {
                        None | Some(Value::Null) => Vec::new(),
                        Some(Value::Sequence(seq)) => seq.clone(),
                        Some(_) => anyhow::bail!("函数 {} 的 steps 需要步骤列表", name),
                    };
                    (b, nested_cond.or(cond_v))
                }
                _ => anyhow::bail!("函数 {} 的函数体需要步骤列表（或写 cond: + steps:）", name),
            };
            let cond = match cond_v {
                None => Vec::new(),
                Some(c) => Self::parse_tpl_names(c, "cond")
                    .map_err(|e| anyhow::anyhow!("函数 {} 的 cond: {}", name, e))?,
            };
            if map.insert(name.clone(), FuncDef { cond, body }).is_some() {
                anyhow::bail!("函数 {} 重复定义", name);
            }
        }
        Ok(map)
    }

    #[async_recursion]
    async fn exec_step(&self, ctx: &mut Ctx, step: &Value) -> anyhow::Result<()> {
        // return 冒泡中：嵌套步骤全部跳过（函数体逐层收口）
        if ctx.return_value.is_some() {
            return Ok(());
        }
        // 无参动作简写：`- str_app` / `- throw` 等纯标量步骤等价 `- str_app:`
        // （YAML 里两者解析类型不同：标量 vs 映射，这里统一转成值为 null 的映射）
        let scalar_owned;
        let step = match step.as_str() {
            Some(key) => {
                let mut m = serde_yaml::Mapping::new();
                m.insert(Value::String(key.to_string()), Value::Null);
                scalar_owned = Value::Mapping(m);
                &scalar_owned
            }
            None => step,
        };
        let Some(m) = step.as_mapping() else {
            anyhow::bail!(
                "步骤需要映射（动作键: 值）或无参动作简写（如 - str_app），收到: {:?}",
                step
            );
        };
        if m.is_empty() {
            return Ok(());
        }
        // ^N 上下文替换（find/color 的命中步骤/then/else 子树执行期间，栈顶绑定生效）
        let refs_owned;
        let step = if let Some(refs) = ctx.ref_stack.last() {
            refs_owned = Self::substitute_refs(step, refs)?;
            &refs_owned
        } else {
            step
        };
        // 已删除动作/参数显式报错引导迁移（2026-08-26 语法精简）
        if step.get("until").is_some() {
            anyhow::bail!("until 已改名 find：- find: 主模板 + block: 障碍模板");
        }
        if step.get("check").is_some() {
            anyhow::bail!("check 已改名 block（find 的障碍模板）");
        }
        if step.get("cond").is_some() {
            if ctx.func_depth > 0 {
                anyhow::bail!("cond 是函数级条件（写在 \"- 函数名:\" 行下、与 steps 键同级），函数体步骤不支持 cond；旧颜色判断请用 color");
            }
            anyhow::bail!("cond 已改名 color：颜色判断写 `- color: [x, y]` + 色值键步骤；模板分支用 find + then/else");
        }
        if step.get("exit").is_some() {
            anyhow::bail!("exit 已改名 throw");
        }
        if step.get("goto").is_some() || step.get("label").is_some() {
            anyhow::bail!("goto/label 已删除：循环重试用 loop");
        }
        for k in [
            "count", "cnt_ivl", "cnt_chk", "img_ivl", "and_or", "click", "before", "after",
        ] {
            if step.get(k).is_some() {
                anyhow::bail!("{} 已删除（2026-08-26 语法精简）", k);
            }
        }
        if step.get("threshold").is_some() {
            anyhow::bail!(
                "threshold 步骤参数已删除：匹配阈值全局配置（config: 段或 config.toml threshold）"
            );
        }
        if step.get("region").is_some() {
            anyhow::bail!("region 步骤参数已删除：搜索区域由模板名 #后缀 决定（无后缀回退全屏）");
        }
        // 动作键解析：一个步骤只能有一个
        const ACTION_KEYS: [&str; 14] = [
            "log", "key", "text", "tap", "swipe", "find", "color", "loop", "call", "throw",
            "str_app", "cls_app", "wait", "return",
        ];
        let hits: Vec<&str> = ACTION_KEYS
            .iter()
            .copied()
            .filter(|k| step.get(*k).is_some())
            .collect();
        let mut cross_qual: Option<String> = None;
        let action: String = if let Some(&first) = hits.first() {
            if hits.len() > 1 {
                if hits.contains(&"wait") {
                    anyhow::bail!(
                        "一个步骤只能有一个动作键（{:?}）：wait 是独立动作，操作后等待参数已删除（步骤间不再统一等待，轮询间隔由 config interval 控制）",
                        hits
                    );
                }
                anyhow::bail!("一个步骤只能有一个动作键，收到 {:?}", hits);
            }
            first.to_string()
        } else {
            // 无动作键：已定义函数名 → 函数调用；`脚本名:函数名` → 跨文件函数调用；
            // 否则报未知动作
            let names: Vec<String> = m
                .keys()
                .filter_map(|k| k.as_str().map(|s| s.to_string()))
                .collect();
            if names.is_empty() {
                anyhow::bail!("步骤键需要字符串（旧数组键写法已删除）");
            }
            match names.iter().find(|n| ctx.funcs.contains_key(n.as_str())) {
                Some(name) => name.clone(),
                None => match names.iter().find(|n| n.contains(':')) {
                    // then/else 键不含冒号，不会误配；本地函数名含冒号的情况也走此分支
                    Some(qual) => {
                        cross_qual = Some(qual.clone());
                        qual.clone()
                    }
                    None => {
                        // 带值动作漏写冒号（`- throw 未知界面` 被解析成标量步骤
                        // "throw 未知界面"）→ 定向提示应写成 `- throw: 未知界面`
                        if let Some(hint) = Self::missing_colon_hint(&names) {
                            anyhow::bail!(hint);
                        }
                        anyhow::bail!(
                            "未知动作 {}（可用：find / color / loop / tap / swipe / key / text / log / call / throw / str_app / cls_app / wait / return / 自定义函数 / 脚本名:函数名 跨文件调用；一个步骤只能有一个动作键）",
                            names.join("、")
                        )
                    }
                },
            }
        };

        match action.as_str() {
            "log" => {
                Self::ensure_only_keys(step, "log", &["log"])?;
                let msg = step
                    .get("log")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                ctx.log("info", msg);
            }
            "key" => {
                Self::ensure_only_keys(step, "key", &["key"])?;
                let key = step.get("key").and_then(|v| v.as_str()).unwrap_or("");
                let code = key_code(key);
                ctx.log("debug", format!("按键 {}", key));
                if let Some(s) = self.devices.session(&ctx.device_id) {
                    s.press_key(code).await?;
                } else {
                    anyhow::bail!("设备未连接");
                }
            }
            "text" => {
                Self::ensure_only_keys(step, "text", &["text"])?;
                let text = step.get("text").and_then(|v| v.as_str()).unwrap_or("");
                ctx.log("debug", format!("输入文本 {}", text));
                if let Some(s) = self.devices.session(&ctx.device_id) {
                    s.inject_text(text).await?;
                } else {
                    anyhow::bail!("设备未连接");
                }
            }
            "tap" => {
                Self::ensure_only_keys(step, "tap", &["tap"])?;
                let (rx, ry) = self.relative_pair(step.get("tap").unwrap_or(&Value::Null))?;
                let s = self
                    .devices
                    .session(&ctx.device_id)
                    .ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
                let (w, h) = s.video_size();
                let x = (rx * w as f32).round().clamp(0.0, w as f32) as u32;
                let y = (ry * h as f32).round().clamp(0.0, h as f32) as u32;
                ctx.log(
                    "debug",
                    format!("点击坐标 ({:.3}, {:.3}) → 像素 ({}, {})", rx, ry, x, y),
                );
                self.emit(&ctx.device_id, ScriptEvent::Tap { x, y }).await;
                s.tap(x as f32, y as f32).await?;
            }
            "swipe" => {
                Self::ensure_only_keys(step, "swipe", &["swipe"])?;
                let v = step.get("swipe").unwrap();
                let sm = v.as_mapping().ok_or_else(|| {
                    anyhow::anyhow!("swipe 需要 {{fm: [x,y], to: [x,y], time: 500ms}} 映射")
                })?;
                for k in sm.keys() {
                    match k.as_str() {
                        Some("fm" | "to" | "time") => {}
                        Some("from") => anyhow::bail!("swipe 的 from 已改名 fm"),
                        other => anyhow::bail!("swipe 不支持参数 {:?}", other),
                    }
                }
                let from = sm
                    .get(Value::String("fm".into()))
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("swipe 缺少 fm"))?;
                let to = sm
                    .get(Value::String("to".into()))
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("swipe 缺少 to"))?;
                let dur = match sm.get(Value::String("time".into())) {
                    Some(t) => Self::parse_duration(t, "swipe time")?,
                    None => 500,
                };
                let (rx1, ry1) = self.relative_pair(&from)?;
                let (rx2, ry2) = self.relative_pair(&to)?;
                let s = self
                    .devices
                    .session(&ctx.device_id)
                    .ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
                let (w, h) = s.video_size();
                let x1 = (rx1 * w as f32).round().clamp(0.0, w as f32) as u32;
                let y1 = (ry1 * h as f32).round().clamp(0.0, h as f32) as u32;
                let x2 = (rx2 * w as f32).round().clamp(0.0, w as f32) as u32;
                let y2 = (ry2 * h as f32).round().clamp(0.0, h as f32) as u32;
                ctx.log(
                    "debug",
                    format!(
                        "滑动 ({:.3},{:.3})→({:.3},{:.3}) {}ms",
                        rx1, ry1, rx2, ry2, dur
                    ),
                );
                self.emit(&ctx.device_id, ScriptEvent::Swipe { x1, y1, x2, y2 })
                    .await;
                s.swipe(x1 as f32, y1 as f32, x2 as f32, y2 as f32, dur)
                    .await?;
            }
            "wait" => {
                Self::ensure_only_keys(step, "wait", &["wait"])?;
                let ms = match step.get("wait").unwrap() {
                    Value::Sequence(seq) if seq.len() == 2 => {
                        let a = Self::parse_duration(&seq[0], "wait")?;
                        let b = Self::parse_duration(&seq[1], "wait")?;
                        if b > a {
                            a + rand::random::<u64>() % (b - a)
                        } else {
                            a
                        }
                    }
                    Value::Sequence(_) => {
                        anyhow::bail!("wait 区间需要 [最小, 最大] 两个带单位时长（如 [1s, 3s]）")
                    }
                    other => Self::parse_duration(other, "wait")?,
                };
                ctx.log("debug", format!("等待 {}ms", ms));
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            "find" => self.exec_find(ctx, step).await?,
            "color" => self.exec_color(ctx, step).await?,
            "loop" => self.exec_loop(ctx, step).await?,
            "call" => {
                Self::ensure_only_keys(step, "call", &["call"])?;
                let line = step
                    .get("call")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("call 需要 \"子脚本名 [实参...]\" 字符串（如 - call: test2.yml a.png [0.5, 0.6]）"))?;
                // 空格分隔 + 括号感知：[x, y] 内部的空格不切分
                let parts = Self::split_args(line);
                let (script_name, args) = parts
                    .split_first()
                    .map(|(n, rest)| (n.clone(), rest.to_vec()))
                    .ok_or_else(|| anyhow::anyhow!("call 缺少子脚本名"))?;
                // 子脚本按名解析：优先调用者同分区，其次跨分区（缺扩展名自动补全）
                let caller_pkg = ctx.script_id.split('/').next().unwrap_or_default();
                match self.scripts.resolve_call(caller_pkg, &script_name)? {
                    Some(s) => {
                        if args.is_empty() {
                            ctx.log("debug", format!("调用子脚本 {}", script_name));
                        } else {
                            ctx.log(
                                "debug",
                                format!("调用子脚本 {}（实参 {}）", script_name, args.join(" ")),
                            );
                        }
                        let sub_log = self
                            .run(
                                &ctx.device_id,
                                &s.id,
                                &s.content,
                                ctx.stop.clone(),
                                ctx.log_cb.clone(),
                                0,
                                None,
                                Some(ctx.exit.clone()),
                                args,
                            )
                            .await?;
                        ctx.log.extend(sub_log);
                    }
                    None => anyhow::bail!("子脚本不存在: {}", script_name),
                }
            }
            "throw" => {
                Self::ensure_only_keys(step, "throw", &["throw"])?;
                let msg = step
                    .get("throw")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty());
                match msg {
                    Some(m) => ctx.log("info", format!("因 {} 结束运行脚本", m)),
                    None => ctx.log("info", "结束运行脚本".to_string()),
                }
                ctx.exit.store(true, Ordering::SeqCst);
            }
            "str_app" => {
                Self::ensure_only_keys(step, "str_app", &["str_app"])?;
                Self::ensure_bare_value(step, "str_app")?;
                let pkg = self.resolve_app_pkg(ctx)?;
                // "+" 前缀：先 force-stop 再启动（scrcpy 定制控制消息，
                // 虚拟屏模式下自动启动到虚拟屏，不要用 adb am start——会落到主屏）
                let s = self
                    .devices
                    .session(&ctx.device_id)
                    .ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
                ctx.log("info", format!("冷启动应用 {}", pkg));
                s.start_app(&format!("+{}", pkg)).await?;
            }
            "cls_app" => {
                Self::ensure_only_keys(step, "cls_app", &["cls_app"])?;
                Self::ensure_bare_value(step, "cls_app")?;
                let pkg = self.resolve_app_pkg(ctx)?;
                let serial = self
                    .devices
                    .snapshot(&ctx.device_id)
                    .map(|(d, _, _)| d.addr)
                    .filter(|a| !a.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("设备不存在或未解析出 adb serial"))?;
                ctx.log("info", format!("关闭应用 {}", pkg));
                // adb force-stop：不碰 scrcpy 会话（屏幕/投屏不中断）；幂等，应用未运行也无害。
                // 虚拟屏上应用被杀后画面变桌面或黑屏，流不断，属预期
                self.devices
                    .adb
                    .shell(
                        &serial,
                        &format!("am force-stop {}", pkg),
                        Duration::from_secs(8),
                    )
                    .await?;
            }
            "return" => {
                Self::ensure_only_keys(step, "return", &["return"])?;
                let b = step
                    .get("return")
                    .and_then(|v| v.as_bool())
                    .ok_or_else(|| anyhow::anyhow!("return 需要 true / false"))?;
                if ctx.func_depth == 0 {
                    anyhow::bail!("return 仅可在自定义函数内使用");
                }
                ctx.log("debug", format!("函数 return {}", b));
                ctx.return_value = Some(b);
            }
            name => {
                if let Some(qual) = cross_qual {
                    self.exec_cross_func(ctx, &qual, step).await?;
                } else {
                    self.exec_func(ctx, name, step).await?;
                }
            }
        }
        Ok(())
    }

    /// find：超时时间内轮询等主模板出现并点击。每轮：主模板（新截图）命中 →
    /// 点击中心 → verify（true = 等 interval 重匹配、仍命中补一击，共两击）→
    /// then 结束；未命中 → block 依序匹配（命中即点击中心并结束本轮）→
    /// 全未命中等 interval 重开一轮；超时 → else。
    /// ^1 = 主模板名、^2.. = block 名，then/else 子树内可引用。
    /// threshold 取 ctx（config: 段 > config.toml）；region 由模板名 #后缀决定
    #[async_recursion]
    async fn exec_find(&self, ctx: &mut Ctx, step: &Value) -> anyhow::Result<()> {
        Self::ensure_only_keys(
            step,
            "find",
            &["find", "verify", "timeout", "block", "then", "else"],
        )?;
        let template = match step.get("find") {
            Some(Value::String(s)) => {
                let t = s.trim().to_string();
                if t.is_empty() {
                    anyhow::bail!("find 模板名不能为空");
                }
                if t.contains(',') {
                    anyhow::bail!(
                        "find 只支持单个主模板（多个目标请拆成多步；挡路的模板写 block）"
                    );
                }
                t
            }
            Some(_) => anyhow::bail!("find 只支持单个主模板名字符串（障碍模板写 block）"),
            None => anyhow::bail!("缺少 find"),
        };
        let blocks = match step.get("block") {
            Some(v) => Self::parse_tpl_names(v, "block")?,
            None => Vec::new(),
        };
        if blocks.iter().any(|b| b == &template) {
            anyhow::bail!("block 模板 {} 与 find 主模板重复", template);
        }
        let verify = match step.get("verify") {
            Some(v) => v.as_bool().ok_or_else(|| {
                anyhow::anyhow!(
                    "verify 需要 true / false（true=点击后等 interval 重匹配，仍命中补点一次）"
                )
            })?,
            None => false,
        };
        let timeout_ms = match Self::opt_duration(step, "timeout")? {
            Some(t) if t > 0 => t,
            Some(_) => anyhow::bail!("find 的 timeout 必须 > 0（默认 30min；支持 500ms / 2s / 1m / 30min / 1h / 1d 写法）"),
            None => FIND_DEFAULT_TIMEOUT_MS,
        };
        let then_steps = match step.get("then") {
            Some(v) => Self::steps_value(v)?,
            None => Vec::new(),
        };
        let else_steps = match step.get("else") {
            Some(v) => Self::steps_value(v)?,
            None => Vec::new(),
        };
        let refs: Vec<String> = std::iter::once(template.clone())
            .chain(blocks.iter().cloned())
            .collect();
        if blocks.is_empty() {
            ctx.log(
                "info",
                format!(
                    "等待模板 {}，超时 {}ms，轮询 {}ms",
                    template, timeout_ms, ctx.interval_ms
                ),
            );
        } else {
            ctx.log(
                "info",
                format!(
                    "等待模板 {}（障碍 {}），超时 {}ms，轮询 {}ms",
                    template,
                    blocks.join("、"),
                    timeout_ms,
                    ctx.interval_ms
                ),
            );
        }
        let start = std::time::Instant::now();
        loop {
            if ctx.stop.load(Ordering::SeqCst)
                || ctx.exit.load(Ordering::SeqCst)
                || ctx.return_value.is_some()
            {
                break;
            }
            if start.elapsed().as_millis() as u64 > timeout_ms {
                ctx.log(
                    "warn",
                    format!("等待模板 {} 超时（{}ms）", template, timeout_ms),
                );
                self.run_branch(ctx, &else_steps, &refs).await?;
                break;
            }
            if let Some(mm) = self.match_one(ctx, &template).await? {
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
                    "success",
                    format!("模板 {} 已找到 @ ({}, {})", template, mm.x, mm.y),
                );
                self.click_center(ctx, &mm).await?;
                if verify {
                    tokio::time::sleep(Duration::from_millis(ctx.interval_ms)).await;
                    if let Some(m2) = self.match_one(ctx, &template).await? {
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
                                "verify：模板 {} 仍存在，补点一次 @ ({}, {})",
                                template, m2.x, m2.y
                            ),
                        );
                        self.click_center(ctx, &m2).await?;
                    } else {
                        ctx.log(
                            "debug",
                            format!("verify：模板 {} 已消失，点击已生效", template),
                        );
                    }
                }
                self.run_branch(ctx, &then_steps, &refs).await?;
                break;
            }
            // 主模板未命中 → block 依序（命中即点击其中心并结束本轮）
            for b in &blocks {
                if ctx.stop.load(Ordering::SeqCst) {
                    break;
                }
                if let Some(mm) = self.match_one(ctx, b).await? {
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
                        "success",
                        format!("障碍模板 {} 出现，点击关闭 @ ({}, {})", b, mm.x, mm.y),
                    );
                    self.click_center(ctx, &mm).await?;
                    break;
                }
            }
            if ctx.stop.load(Ordering::SeqCst)
                || ctx.exit.load(Ordering::SeqCst)
                || ctx.return_value.is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(ctx.interval_ms)).await;
        }
        Ok(())
    }

    /// color：`- color: [x, y]`（相对坐标）+ 色值键（6 位十六进制，容差固定
    /// 30/通道）挂命中步骤 + else。一次截图按序判定，命中一个执行其步骤结束
    /// 本步；全未命中走 else。不轮询无超时（重试套 loop）。
    /// ^1 = "[x, y]" 坐标串、^2.. = 色值键（书写顺序）
    #[async_recursion]
    async fn exec_color(&self, ctx: &mut Ctx, step: &Value) -> anyhow::Result<()> {
        let pos_v = step
            .get("color")
            .ok_or_else(|| anyhow::anyhow!("缺少 color 坐标"))?;
        let (rx, ry) =
            Self::parse_rel_coord(pos_v).map_err(|e| anyhow::anyhow!("color 坐标: {}", e))?;
        let m = step.as_mapping().unwrap();
        type ColorCase = ((u8, u8, u8), String, Vec<Value>); // (rgb, 规范化 hex, 命中步骤)
        let mut cases: Vec<ColorCase> = Vec::new();
        let mut else_steps: Vec<Value> = Vec::new();
        for (k, v) in m {
            match k.as_str() {
                Some("color") => continue,
                Some("else") => {
                    else_steps = Self::steps_value(v)
                        .map_err(|e| anyhow::anyhow!("color 的 else: {}", e))?;
                }
                Some(_) => {
                    let (r, g, b) = Self::parse_color(k)
                        .map_err(|e| anyhow::anyhow!("color 的色值键: {}", e))?;
                    let steps = Self::steps_value(v)
                        .map_err(|e| anyhow::anyhow!("color 的色值键: {}", e))?;
                    cases.push(((r, g, b), format!("{:02x}{:02x}{:02x}", r, g, b), steps));
                }
                None => anyhow::bail!(
                    "color 的兄弟键需要色值（6 位十六进制）或 else（旧数组键写法已删除）"
                ),
            }
        }
        if cases.is_empty() {
            anyhow::bail!("color 至少需要一个色值键（如 `ff8800:` 挂命中步骤）");
        }
        let refs: Vec<String> = std::iter::once(format!("[{}, {}]", rx, ry))
            .chain(cases.iter().map(|(_, hex, _)| hex.clone()))
            .collect();
        let screen = self
            .devices
            .screenshot(&ctx.device_id)
            .await
            .map_err(|e| anyhow::anyhow!("截图失败: {}", e))?;
        let (w, h) = self.screen_size(ctx, &screen);
        if w == 0 || h == 0 {
            anyhow::bail!("无法获取屏幕尺寸");
        }
        let img = image::load_from_memory(&screen)
            .map_err(|e| anyhow::anyhow!("解析截图失败: {}", e))?
            .to_rgb8();
        // 每通道容差固定 30（H.264 有损压缩帧间像素抖动，精确匹配实际不可用）
        const TOL: i32 = 30;
        let px = ((rx * w as f64).round() as i64).clamp(0, w as i64 - 1) as u32;
        let py = ((ry * h as f64).round() as i64).clamp(0, h as i64 - 1) as u32;
        let p = img.get_pixel(px, py).0;
        let (ar, ag, ab) = (p[0] as i32, p[1] as i32, p[2] as i32);
        for ((er, eg, eb), hex, steps) in &cases {
            if (ar - *er as i32).abs() <= TOL
                && (ag - *eg as i32).abs() <= TOL
                && (ab - *eb as i32).abs() <= TOL
            {
                ctx.log(
                    "success",
                    format!(
                        "颜色命中 {}（实际 {:02x}{:02x}{:02x}）@ 像素 ({}, {})",
                        hex, ar, ag, ab, px, py
                    ),
                );
                self.emit(
                    &ctx.device_id,
                    ScriptEvent::Hit {
                        tpl: format!("clr {}", hex),
                        x: px.saturating_sub(12),
                        y: py.saturating_sub(12),
                        w: 24,
                        h: 24,
                        score: 1.0,
                    },
                )
                .await;
                self.run_branch(ctx, steps, &refs).await?;
                return Ok(());
            }
            ctx.log(
                "debug",
                format!(
                    "颜色未命中：期望 {} 实际 {:02x}{:02x}{:02x} @ ({}, {})",
                    hex, ar, ag, ab, px, py
                ),
            );
        }
        ctx.log("info", "颜色全部未命中，执行 else".to_string());
        self.run_branch(ctx, &else_steps, &refs).await?;
        Ok(())
    }

    /// loop：times（默认 0 = 无限循环）+ steps（必需）。
    /// times/steps 两种缩进均可：写在 loop 值里（- loop:\n    times: 3）或与
    /// loop 同级作步骤兄弟键（- loop:\n  times: 3——映射值同级会被 YAML 解析成
    /// 兄弟键，干脆两种都认）
    #[async_recursion]
    async fn exec_loop(&self, ctx: &mut Ctx, step: &Value) -> anyhow::Result<()> {
        Self::ensure_only_keys(step, "loop", &["loop", "times", "steps"])?;
        let inner = step.get("loop").and_then(|v| v.as_mapping());
        let get = |k: &str| inner.and_then(|m| m.get(k)).or_else(|| step.get(k));
        let times = get("times").and_then(|x| x.as_u64()).unwrap_or(0);
        let sub_steps = get("steps")
            .and_then(|x| x.as_sequence())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("loop 需要 steps 步骤列表"))?;
        let mut n: u64 = 0;
        loop {
            if times > 0 && n >= times {
                break;
            }
            if ctx.stop.load(Ordering::SeqCst)
                || ctx.exit.load(Ordering::SeqCst)
                || ctx.return_value.is_some()
            {
                break;
            }
            ctx.log("debug", format!("循环第 {} 次", n + 1));
            for sub in &sub_steps {
                if ctx.exit.load(Ordering::SeqCst) || ctx.return_value.is_some() {
                    break;
                }
                self.exec_step(ctx, sub).await?;
            }
            n += 1;
        }
        Ok(())
    }

    /// 自定义函数调用：`- 函数名: 实参1 实参2`（空格分隔 + 括号感知）+
    /// then（返回 true）/ else（返回 false）。
    /// cond 条件（可选）：每个条件模板各取一张新截图匹配一次（不点击），全部
    /// 命中才执行函数体；任一未命中 → 函数返回 false（不执行函数体）。
    /// 函数体副本先做 $N → 实参替换；执行完未 return 视为返回 true；嵌套上限 32 层。
    #[async_recursion]
    async fn exec_func(&self, ctx: &mut Ctx, name: &str, step: &Value) -> anyhow::Result<()> {
        Self::ensure_only_keys(step, name, &[name, "then", "else"])?;
        let def = ctx
            .funcs
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("函数 {} 未定义（func: 段里没有该函数名）", name))?;
        let args = Self::func_args(step, name)?;
        let ret = self.run_func_core(ctx, def, &args, name).await?;
        self.run_func_branch(ctx, step, ret).await
    }

    /// 跨文件自定义函数调用：`- 脚本名:函数名: 实参…`（脚本名与 call 同规则
    /// 解析：优先同分区、缺扩展名自动补全；函数体/cond 取自该脚本 func 段，
    /// 体内 `$N` 由调用点实参替换——函数体执行期间该脚本函数可见，调用者函数
    /// 兜底；return 冒泡、嵌套上限与本地函数一致
    #[async_recursion]
    async fn exec_cross_func(&self, ctx: &mut Ctx, qual: &str, step: &Value) -> anyhow::Result<()> {
        Self::ensure_only_keys(step, qual, &[qual, "then", "else"])?;
        let (script_part, func_part) = qual.split_once(':').expect("含冒号才进此分支");
        let script_name = script_part.trim();
        let func_name = func_part.trim();
        if script_name.is_empty() || func_name.is_empty() {
            anyhow::bail!("跨文件函数调用需要 \"脚本名:函数名\"（如 - test1:fun1: 实参…）");
        }
        // 子脚本按名解析：优先调用者同分区，其次跨分区（缺扩展名自动补全）
        let caller_pkg = ctx.script_id.split('/').next().unwrap_or_default();
        let s = self
            .scripts
            .resolve_call(caller_pkg, script_name)?
            .ok_or_else(|| anyhow::anyhow!("子脚本不存在: {}", script_name))?;
        // 解析被引用脚本的 func 段（函数体 $N 不参与子脚本级替换，由调用点实参替换）；
        // 先做顶层归一化（省略 func: 的纯函数库简写同样可被跨文件调用）
        let doc: Value = serde_yaml::from_str(&s.content)
            .map_err(|e| anyhow::anyhow!("子脚本 {} 解析失败: {}", script_name, e))?;
        let doc = Self::normalize_top(doc)
            .map_err(|e| anyhow::anyhow!("子脚本 {}: {}", script_name, e))?;
        let sub_funcs = Self::parse_funcs(doc.get("func").filter(|v| !v.is_null()).cloned())?;
        let def = sub_funcs
            .get(func_name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("子脚本 {} 未定义函数 {}", s.name, func_name))?;
        let args = Self::func_args(step, qual)?;
        // 函数体执行期间被引用脚本的函数可见（体内裸函数名按该脚本解析），
        // 调用者函数兜底；执行结束恢复（避免子脚本函数泄漏到后续步骤）
        let mut merged = sub_funcs;
        for (k, v) in ctx.funcs.iter() {
            merged.entry(k.clone()).or_insert_with(|| v.clone());
        }
        let saved = std::mem::replace(&mut ctx.funcs, merged);
        // 模板按被引用脚本所在分区解析（与 call 一致；通常与调用者同分区）
        let saved_id = std::mem::replace(&mut ctx.script_id, s.id.clone());
        let ret = self
            .run_func_core(ctx, def, &args, &format!("{}:{}", s.name, func_name))
            .await;
        ctx.script_id = saved_id;
        ctx.funcs = saved;
        let ret = ret?;
        self.run_func_branch(ctx, step, ret).await
    }

    /// 执行函数体（本地/跨文件共用核心）：cond 条件检查 → 函数体 $N → 实参
    /// 替换 → 顺序执行 → return 冒泡。函数体执行完未 return 视为返回 true。
    /// 返回函数返回值；调用点的 then/else 由调用方按返回值选择
    async fn run_func_core(
        &self,
        ctx: &mut Ctx,
        def: FuncDef,
        args: &[String],
        label: &str,
    ) -> anyhow::Result<bool> {
        if ctx.func_depth >= MAX_FUNC_DEPTH {
            anyhow::bail!(
                "自定义函数嵌套过深（上限 {}）：疑似无限递归",
                MAX_FUNC_DEPTH
            );
        }
        if args.is_empty() {
            ctx.log("debug", format!("调用函数 {}", label));
        } else {
            ctx.log(
                "debug",
                format!("调用函数 {}（实参 {}）", label, args.join(" ")),
            );
        }
        let mut body_val = Value::Sequence(def.body);
        Self::substitute_args(&mut body_val, args)?;
        let body = body_val.as_sequence().cloned().unwrap_or_default();
        ctx.func_depth += 1;
        ctx.return_value = None;
        if !self.check_func_cond(ctx, &def.cond).await? {
            // 条件未命中：函数返回 false（不执行函数体）
            ctx.return_value = Some(false);
        } else {
            for sub in &body {
                if ctx.stop.load(Ordering::SeqCst) || ctx.exit.load(Ordering::SeqCst) {
                    break;
                }
                self.exec_step(ctx, sub).await?;
                if ctx.return_value.is_some() {
                    break;
                }
            }
        }
        ctx.func_depth -= 1;
        Ok(ctx.return_value.take().unwrap_or(true))
    }

    /// 函数 cond 条件检查：每个条件模板各取一张新截图匹配一次（不点击）；
    /// 全部命中 → 执行函数体；任一未命中 → 函数返回 false
    async fn check_func_cond(&self, ctx: &mut Ctx, cond: &[String]) -> anyhow::Result<bool> {
        if cond.is_empty() {
            return Ok(true);
        }
        for tpl in cond {
            match self.match_one(ctx, tpl).await? {
                Some(mm) => {
                    self.emit(
                        &ctx.device_id,
                        ScriptEvent::Hit {
                            tpl: tpl.clone(),
                            x: mm.x,
                            y: mm.y,
                            w: mm.width,
                            h: mm.height,
                            score: mm.score,
                        },
                    )
                    .await;
                    ctx.log(
                        "success",
                        format!("函数条件模板 {} 已匹配 @ ({}, {})", tpl, mm.x, mm.y),
                    );
                }
                None => {
                    ctx.log(
                        "info",
                        format!("函数条件模板 {} 未匹配，函数返回 false", tpl),
                    );
                    return Ok(false);
                }
            }
        }
        ctx.log("debug", format!("函数条件全部匹配（{}）", cond.join("、")));
        Ok(true)
    }

    /// 按函数返回值执行调用点的 then（返回 true）/ else（返回 false）分支
    async fn run_func_branch(&self, ctx: &mut Ctx, step: &Value, ret: bool) -> anyhow::Result<()> {
        let branch = if ret { "then" } else { "else" };
        if let Some(steps) = step.get(branch).and_then(|v| v.as_sequence()) {
            for sub in steps {
                if ctx.stop.load(Ordering::SeqCst)
                    || ctx.exit.load(Ordering::SeqCst)
                    || ctx.return_value.is_some()
                {
                    break;
                }
                self.exec_step(ctx, sub).await?;
            }
        }
        Ok(())
    }

    /// 函数调用实参解析：值 = null / 空串 → 无参；字符串 → 空格分隔 + 括号感知切分
    fn func_args(step: &Value, key: &str) -> anyhow::Result<Vec<String>> {
        match step.get(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::String(s)) => {
                let t = s.trim();
                if t.is_empty() {
                    Ok(Vec::new())
                } else {
                    Ok(Self::split_args(t))
                }
            }
            Some(_) => anyhow::bail!(
                "函数 {} 的实参需要空格分隔字符串（坐标写 [x, y]，整体不用引号）",
                key
            ),
        }
    }

    /// 带值动作漏写冒号的定向提示：`- throw 未知界面` 会被 YAML 解析成标量步骤
    /// （键 = "throw 未知界面"），用户本意是 `- throw: 未知界面`。返回提示（无匹配
    /// 返回 None）。动作名后必须跟空白才算带值（函数名 "finder" 不会误伤 find）
    fn missing_colon_hint(names: &[String]) -> Option<String> {
        const ACTIONS: [&str; 14] = [
            "log", "key", "text", "tap", "swipe", "find", "color", "loop", "call", "throw",
            "str_app", "cls_app", "wait", "return",
        ];
        for n in names {
            for act in ACTIONS {
                if let Some(rest) = n.strip_prefix(act) {
                    if rest.starts_with(char::is_whitespace) {
                        return Some(format!(
                            "\"{}\" 是标量步骤（YAML 把 \"- {}\" 解析成字符串）——带值/带原因的动作需写冒号：应为 \"- {}: {}\"（裸写仅限无参动作，如 - str_app / - throw）",
                            n, n, act, rest.trim_start()
                        ));
                    }
                }
            }
        }
        None
    }

    /// 在 ^N 绑定下执行分支步骤（find 的 then/else、color 的命中步骤/else）：
    /// 压栈 → 逐步执行（每步各自按栈顶绑定做替换，嵌套 find/color 内层覆盖
    /// 外层）→ 出栈
    async fn run_branch(
        &self,
        ctx: &mut Ctx,
        steps: &[Value],
        refs: &[String],
    ) -> anyhow::Result<()> {
        if steps.is_empty() {
            return Ok(());
        }
        ctx.ref_stack.push(refs.to_vec());
        for sub in steps {
            if ctx.stop.load(Ordering::SeqCst)
                || ctx.exit.load(Ordering::SeqCst)
                || ctx.return_value.is_some()
            {
                break;
            }
            self.exec_step(ctx, sub).await?;
        }
        ctx.ref_stack.pop();
        Ok(())
    }

    /// 点击命中模板的中心（find 主模板与 block 障碍模板共用）
    async fn click_center(&self, ctx: &mut Ctx, m: &matcher::MatchResult) -> anyhow::Result<()> {
        let s = self
            .devices
            .session(&ctx.device_id)
            .ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
        let (cx, cy) = (m.x + m.width / 2, m.y + m.height / 2);
        ctx.log("debug", format!("点击模板中心 @ ({}, {})", cx, cy));
        self.emit(&ctx.device_id, ScriptEvent::Tap { x: cx, y: cy })
            .await;
        s.tap(cx as f32, cy as f32).await?;
        Ok(())
    }

    /// 步骤的兄弟键白名单校验：出现白名单外的键（含拼写错误/已删除参数残留）
    /// 显式报错，防静默失效
    fn ensure_only_keys(step: &Value, action: &str, allowed: &[&str]) -> anyhow::Result<()> {
        let m = step.as_mapping().unwrap();
        for k in m.keys() {
            let Some(name) = k.as_str() else {
                anyhow::bail!("{} 不支持非字符串参数键（旧数组键写法已删除）", action);
            };
            if !allowed.contains(&name) {
                anyhow::bail!(
                    "{} 不支持参数 {}（可用：{}）",
                    action,
                    name,
                    allowed.join(" / ")
                );
            }
        }
        Ok(())
    }

    /// str_app / cls_app 只接受裸写（值必须为 null 或空串），带值报错
    fn ensure_bare_value(step: &Value, action: &str) -> anyhow::Result<()> {
        match step.get(action) {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(()),
            Some(_) => anyhow::bail!(
                "{} 不支持参数：应用包名固定为设备分区（设备配置 pkg）",
                action
            ),
        }
    }

    /// str_app/cls_app 的应用包名：固定取设备配置 pkg；
    /// 校验仅允许 [A-Za-z0-9_.]（cls_app 要拼进 adb shell 命令，防注入）
    fn resolve_app_pkg(&self, ctx: &Ctx) -> anyhow::Result<String> {
        let pkg = self
            .devices
            .snapshot(&ctx.device_id)
            .and_then(|(d, _, _)| d.pkg)
            .unwrap_or_default();
        if pkg.is_empty() {
            anyhow::bail!("缺少应用包名（设备未配置 pkg）");
        }
        if !pkg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
        {
            anyhow::bail!("应用包名字符非法: {}", pkg);
        }
        Ok(pkg)
    }

    /// 空格分隔 + 括号感知的实参切分：`[x, y]` 内部的空格不算分隔符。
    /// call 与自定义函数调用共用
    fn split_args(line: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut depth = 0usize;
        for ch in line.chars() {
            match ch {
                '[' => {
                    depth += 1;
                    cur.push(ch);
                }
                ']' => {
                    depth = depth.saturating_sub(1);
                    cur.push(ch);
                }
                c if c.is_whitespace() && depth == 0 => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                c => cur.push(c),
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    /// call/函数实参替换：把 `$N`（N≥1）替换为实参。递归作用于所有字符串
    /// （映射键与值、列表项）——find/color 模板名、log 文本、call 行（嵌套
    /// 转发 $N）等全部生效；`$` 后非数字保持原样（"100$" 不受影响）。
    /// 引用 `$N` 超出实参数量 → 报错（含 $N 占位的脚本被直接运行、或实参
    /// 传少，都在此拦截）
    fn substitute_args(v: &mut Value, args: &[String]) -> anyhow::Result<()> {
        match v {
            Value::String(s) => *s = Self::substitute_str(s, args)?,
            Value::Sequence(seq) => {
                for item in seq.iter_mut() {
                    Self::substitute_args(item, args)?;
                }
            }
            Value::Mapping(m) => {
                // 键也要替换（如 color 的色值键），iter_mut 的键不可变 → 重建
                let old = std::mem::take(m);
                for (mut k, mut val) in old {
                    Self::substitute_args(&mut k, args)?;
                    Self::substitute_args(&mut val, args)?;
                    m.insert(k, val);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// 单字符串的 $N 替换：`$` 后跟数字（取最长数字串）= 实参引用，越界报错；
    /// `$` 后非数字 = 字面 $ 原样保留。替换结果不再扫描（实参含 $N 不会二次展开）
    fn substitute_str(s: &str, args: &[String]) -> anyhow::Result<String> {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(pos) = rest.find('$') {
            out.push_str(&rest[..pos]);
            let after = &rest[pos + 1..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                out.push('$');
                rest = after;
                continue;
            }
            let n: usize = digits.parse().unwrap();
            let Some(arg) = args.get(n.checked_sub(1).unwrap_or(usize::MAX)) else {
                anyhow::bail!(
                    "参数引用 ${} 超出实参数量（{} 个）：含 $N 占位的脚本需经 call/函数调用传参运行（参数从 $1 开始）",
                    digits, args.len()
                );
            };
            out.push_str(arg);
            rest = &after[digits.len()..];
        }
        out.push_str(rest);
        Ok(out)
    }

    /// ^N 上下文替换（find/color 分支子树）：递归替换字符串（含映射键），但
    /// 序列里的映射项（步骤）不替换——留给各自执行时按当时的栈顶绑定替换，
    /// 嵌套 find/color 的内层绑定自然覆盖外层；序列里的字符串项（block 模板
    /// 名 / wait 区间等）照常替换。^N 越界报错
    fn substitute_refs(v: &Value, refs: &[String]) -> anyhow::Result<Value> {
        match v {
            Value::String(s) => Ok(Value::String(Self::substitute_ref_str(s, refs)?)),
            Value::Sequence(seq) => {
                let mut out = Vec::with_capacity(seq.len());
                for item in seq {
                    out.push(match item {
                        Value::String(s) => Value::String(Self::substitute_ref_str(s, refs)?),
                        other => other.clone(),
                    });
                }
                Ok(Value::Sequence(out))
            }
            Value::Mapping(m) => {
                let mut out = serde_yaml::Mapping::new();
                for (k, val) in m {
                    let nk = match k {
                        Value::String(s) => Value::String(Self::substitute_ref_str(s, refs)?),
                        other => other.clone(),
                    };
                    let nv = match val {
                        Value::String(s) => Value::String(Self::substitute_ref_str(s, refs)?),
                        Value::Sequence(seq) => {
                            let mut ns = Vec::with_capacity(seq.len());
                            for item in seq {
                                ns.push(match item {
                                    Value::String(s) => {
                                        Value::String(Self::substitute_ref_str(s, refs)?)
                                    }
                                    other => other.clone(),
                                });
                            }
                            Value::Sequence(ns)
                        }
                        Value::Mapping(_) => Self::substitute_refs(val, refs)?,
                        other => other.clone(),
                    };
                    out.insert(nk, nv);
                }
                Ok(Value::Mapping(out))
            }
            other => Ok(other.clone()),
        }
    }

    /// 单字符串的 ^N 替换：`^` 后跟数字（取最长数字串）= 上下文引用，越界
    /// 报错；`^` 后非数字 = 字面 ^ 原样保留。^ 不是 YAML 保留字符（& 是——
    /// 锚点，故弃用 &N 选 ^N）
    fn substitute_ref_str(s: &str, refs: &[String]) -> anyhow::Result<String> {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(pos) = rest.find('^') {
            out.push_str(&rest[..pos]);
            let after = &rest[pos + 1..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                out.push('^');
                rest = after;
                continue;
            }
            let n: usize = digits.parse().unwrap();
            let Some(r) = refs.get(n.checked_sub(1).unwrap_or(usize::MAX)) else {
                anyhow::bail!(
                    "上下文引用 ^{} 超出数量（{} 个：^1 主模板/坐标，^2.. 障碍模板/颜色）",
                    digits,
                    refs.len()
                );
            };
            out.push_str(r);
            rest = &after[digits.len()..];
        }
        out.push_str(rest);
        Ok(out)
    }

    /// 解析时长参数（timeout / interval / wait / swipe time 共用）：
    /// **强制带单位**——字符串支持 1ms / 1s / 1m / 30min / 1h / 1d
    /// （m ≡ min，大小写不敏感、可带小数如 "1.5s"）；裸数字（YAML 数字或
    /// 纯数字串）不再接受，直接报错（2026-08-26 语法精简）
    fn parse_duration(v: &Value, opt: &str) -> anyhow::Result<u64> {
        let Some(s) = v.as_str() else {
            anyhow::bail!(
                "{} 需要带单位时长（如 500ms / 2s / 1m / 30min / 1h / 1d）；裸数字不再接受，收到: {:?}",
                opt, v
            );
        };
        let t = s.trim().to_ascii_lowercase();
        // 后缀匹配顺序：ms 必须在 m 前判（"1ms" 剥掉 "s" 会剩 "1m"）；min 在 m 前
        for (suffix, mult) in [
            ("ms", 1.0f64),
            ("min", 60_000.0),
            ("m", 60_000.0),
            ("s", 1_000.0),
            ("h", 3_600_000.0),
            ("d", 86_400_000.0),
        ] {
            if let Some(num) = t.strip_suffix(suffix) {
                if let Ok(val) = num.trim().parse::<f64>() {
                    if val >= 0.0 {
                        return Ok((val * mult).round() as u64);
                    }
                }
            }
        }
        anyhow::bail!(
            "{} 需要带单位时长（如 500ms / 2s / 1m / 30min / 1h / 1d），收到: {}",
            opt,
            s
        )
    }

    /// 取步骤时长参数（timeout），缺失返回 None；解析失败（格式非法）向上传播
    fn opt_duration(step: &Value, opt: &str) -> anyhow::Result<Option<u64>> {
        match step.get(opt) {
            Some(v) => Self::parse_duration(v, opt).map(Some),
            None => Ok(None),
        }
    }

    /// 步骤列表值：列表，或留空（null）= 无步骤
    fn steps_value(v: &Value) -> anyhow::Result<Vec<Value>> {
        match v {
            Value::Null => Ok(Vec::new()),
            Value::Sequence(seq) => Ok(seq.clone()),
            _ => anyhow::bail!("需要步骤列表（- 键: 换行缩进步骤）或留空"),
        }
    }

    /// 解析 color 的色值键：6 位十六进制 RRGGBB（可带 # / 0x 前缀、大小写
    /// 不限）或 [r, g, b] 数字数组（0~255）；整数（YAML 解析器把 0xff8800
    /// 直接解析成数字时）按 0xRRGGBB 解码
    fn parse_color(v: &Value) -> anyhow::Result<(u8, u8, u8)> {
        if let Some(s) = v.as_str() {
            let t = s
                .trim()
                .trim_start_matches('#')
                .trim_start_matches("0x")
                .to_ascii_lowercase();
            if t.len() == 6 && t.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok((
                    u8::from_str_radix(&t[0..2], 16).unwrap(),
                    u8::from_str_radix(&t[2..4], 16).unwrap(),
                    u8::from_str_radix(&t[4..6], 16).unwrap(),
                ));
            }
            anyhow::bail!(
                "色值需要 6 位十六进制（如 ff8800 或 \"#ff8800\"）或 [r, g, b]，收到: {}",
                s
            );
        }
        if let Some(n) = v.as_u64() {
            if n <= 0xFF_FFFF {
                return Ok(((n >> 16) as u8, (n >> 8) as u8, n as u8));
            }
        }
        if let Some(seq) = v.as_sequence() {
            if seq.len() == 3 {
                let c = seq
                    .iter()
                    .map(|x| {
                        let n = x
                            .as_u64()
                            .or_else(|| x.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
                            .ok_or_else(|| {
                                anyhow::anyhow!("色值数组需要 [r, g, b] 数字（0~255）")
                            })?;
                        if n > 255 {
                            anyhow::bail!("色值分量必须在 0~255，收到: {}", n);
                        }
                        Ok(n as u8)
                    })
                    .collect::<anyhow::Result<Vec<u8>>>()?;
                return Ok((c[0], c[1], c[2]));
            }
        }
        anyhow::bail!(
            "色值只支持 6 位十六进制（ff8800）或 [r, g, b] 数组，收到: {:?}",
            v
        )
    }

    /// 匹配单个模板一次（独立取最新截图，不重试）：按模板名 #后缀解析区域后匹配。
    /// 未命中时推送 Miss 可视化事件（搜索区域）——find 主模板/block/verify/函数
    /// cond 都经这里，四条路径统一获得"在哪找、没找到"的调试反馈
    async fn match_one(
        &self,
        ctx: &mut Ctx,
        template: &str,
    ) -> anyhow::Result<Option<matcher::MatchResult>> {
        let screen = self
            .devices
            .screenshot(&ctx.device_id)
            .await
            .map_err(|e| anyhow::anyhow!("截图失败: {}", e))?;
        let (w, h) = self.screen_size(ctx, &screen);
        if w == 0 || h == 0 {
            anyhow::bail!("无法获取屏幕尺寸");
        }
        let region = self.region_for(ctx, template, w, h)?;
        let mm = self
            .match_on_screen(ctx, template, ctx.threshold, region, screen)
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

    /// 搜索区域：模板名 #后缀（各自独立，见 tpl_region_from_name）> 全屏。
    /// 短名引用时 #后缀在**实际文件名**上（脚本写 login.png 引用
    /// login#910_159_972_716.png，区域须按解析结果取名才生效）。
    /// 无 #后缀（且回退全屏）时记一条日志提醒（每次运行每模板一条）
    fn region_for(
        &self,
        ctx: &mut Ctx,
        template: &str,
        w: u32,
        h: u32,
    ) -> anyhow::Result<Option<[u32; 4]>> {
        let dir = self.tpl_dir_of(ctx);
        let resolved = Self::resolve_template_file(&dir, template).ok();
        let src = resolved
            .as_ref()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| template.to_string());
        let r = Self::tpl_region_from_name(&src, w, h)?;
        if r.is_none()
            && resolved.is_some()
            && !src.contains('#')
            && ctx.region_warned.insert(src.clone())
        {
            ctx.log(
                "info",
                format!(
                    "模板 {} 未带 #区域后缀，回退全屏搜索（区域写法：xx#l / xx#0_0_500_500）",
                    src
                ),
            );
        }
        Ok(r)
    }

    /// 在给定截图上匹配模板（region 为搜索区域，None=全屏）
    async fn match_on_screen(
        &self,
        ctx: &Ctx,
        template: &str,
        threshold: f32,
        region: Option<[u32; 4]>,
        screen: Vec<u8>,
    ) -> anyhow::Result<Option<matcher::MatchResult>> {
        // 模板按脚本所在应用分区解析：data/<pkg>/tmpl/（script_id 首段 = 分区）
        let tpl_dir = self.tpl_dir_of(ctx);
        // 目录不存在时先创建，避免 std::fs::read 报“系统找不到指定的路径”
        let _ = std::fs::create_dir_all(&tpl_dir);
        let tpl_path = Self::resolve_template_file(&tpl_dir, template)?;
        let tpl_bytes = std::fs::read(&tpl_path).map_err(|e| {
            anyhow::anyhow!(
                "读取模板 {} 失败: {} (path={})",
                template,
                e,
                tpl_path.display()
            )
        })?;
        let req = matcher::MatchRequest {
            screen_png: screen,
            template_png: tpl_bytes,
            threshold: Some(threshold),
            region,
        };
        matcher::match_template(&req).map_err(|e| anyhow::anyhow!("模板匹配失败: {}", e))
    }

    /// 模板文件解析：精确名优先（写全名永远可用）；文件不存在时按**短名**解析——
    /// 基名（去扩展名）+ `#` 前缀在同扩展名文件中唯一匹配（如脚本写 login.png
    /// 引用 login#907_160_973_717.png，#后缀区域照常生效）。
    /// 多个候选 → 报错列出候选要求写全名消歧；零候选 → 报不存在
    fn resolve_template_file(
        tpl_dir: &std::path::Path,
        template: &str,
    ) -> anyhow::Result<std::path::PathBuf> {
        let exact = tpl_dir.join(template);
        if exact.is_file() {
            return Ok(exact);
        }
        let Some((base, ext)) = template.rsplit_once('.') else {
            anyhow::bail!("模板 {} 不存在 (path={})", template, exact.display());
        };
        let prefix = format!("{}#", base.to_ascii_lowercase());
        let dotted = format!(".{}", ext.to_ascii_lowercase());
        let mut candidates: Vec<String> = std::fs::read_dir(tpl_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| {
                let lower = n.to_ascii_lowercase();
                lower.starts_with(&prefix) && lower.ends_with(&dotted)
            })
            .collect();
        candidates.sort();
        match candidates.len() {
            1 => Ok(tpl_dir.join(&candidates[0])),
            0 => anyhow::bail!("模板 {} 不存在 (path={})", template, exact.display()),
            _ => anyhow::bail!(
                "模板 {} 匹配到多个候选：{}，请用完整文件名指定",
                template,
                candidates.join("、")
            ),
        }
    }

    /// 脚本所在分区的模板目录：data/<pkg>/tmpl/（script_id 首段 = 分区）
    fn tpl_dir_of(&self, ctx: &Ctx) -> std::path::PathBuf {
        let pkg = ctx.script_id.split('/').next().unwrap_or_default();
        self.devices.cfg.data_dir.join(pkg).join("tmpl")
    }

    /// 解析模板名列表（find 的 block）：字符串（可逗号分隔多模板）或 YAML 字符串列表
    fn parse_tpl_names(v: &Value, key: &str) -> anyhow::Result<Vec<String>> {
        let names: Vec<String> = match v {
            Value::String(s) => s.split(',').map(|p| p.trim().to_string()).collect(),
            Value::Sequence(seq) => seq
                .iter()
                .map(|item| item.as_str().map(|s| s.trim().to_string()))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| anyhow::anyhow!("{} 列表项必须是字符串模板名", key))?,
            _ => anyhow::bail!(
                "{} 只支持模板名字符串（多模板逗号分隔）或列表，如 `{}: a.png, b.png`",
                key,
                key
            ),
        };
        if names.is_empty() || names.iter().any(|n| n.is_empty()) {
            anyhow::bail!("{} 模板名不能为空", key);
        }
        Ok(names)
    }

    /// 从模板名解析自带区域后缀（与前端 parseTplRegion / parseTplRegionCode
    /// 同一套格式）：
    ///   xx#a / xx#l …   → region 参数同款半区码（a/u/d/l/r/ul/ur/dl/dr）
    ///   xx#x1_y1_x2_y2  → 相对坐标 ×1000 的 1~3 位整数（123 → 0.123，0~999），
    ///                      需 x2 > x1 且 y2 > y1（框选生成区域模板的自动命名格式）
    /// 后缀在扩展名之前（xx#l.png）；无 # / 后缀解析不出区域 → None（全屏），
    /// 解析失败不报错（# 属于合法文件名字符，按普通模板名全屏匹配）
    fn tpl_region_from_name(template: &str, w: u32, h: u32) -> anyhow::Result<Option<[u32; 4]>> {
        let lower = template.to_ascii_lowercase();
        let stem = if lower.ends_with(".jpeg") {
            &template[..template.len() - 5]
        } else if lower.ends_with(".png") || lower.ends_with(".jpg") {
            &template[..template.len() - 4]
        } else {
            template
        };
        let Some(idx) = stem.rfind('#') else {
            return Ok(None);
        };
        let suffix = stem[idx + 1..].trim().to_ascii_lowercase();
        if suffix.is_empty() {
            return Ok(None);
        }
        // 半区码：a → 全屏 None
        if let Ok(r) = Self::parse_region(&Value::String(suffix.clone()), w, h) {
            return Ok(r);
        }
        // 数字坐标：4 段 1~3 位整数 ×1000 → 相对坐标，复用 region 数组写法的校验与换算；
        // 校验不过（如 x2 <= x1）视为无区域 → 全屏，不报错
        let nums: Option<Vec<f64>> = suffix
            .split('_')
            .map(|p| {
                p.parse::<u32>()
                    .ok()
                    .filter(|n| *n <= 999)
                    .map(|n| n as f64 / 1000.0)
            })
            .collect();
        if let Some(nums) = nums {
            let seq = Value::Sequence(nums.into_iter().map(Value::from).collect());
            if let Ok(r) = Self::parse_region(&seq, w, h) {
                return Ok(r);
            }
        }
        Ok(None)
    }

    /// 解析 region：支持 a/u/d/l/r/ul/ur/dl/dr / [x1, y1, x2, y2] / {fm: [x,y], to: [x,y]}（0~1）
    fn parse_region(v: &Value, w: u32, h: u32) -> anyhow::Result<Option<[u32; 4]>> {
        if let Some(s) = v.as_str() {
            let (x, y, rw, rh) = match s.to_ascii_lowercase().as_str() {
                "a" => return Ok(None),
                "u" => (0, 0, w, h / 2),
                "d" => (0, h / 2, w, h - h / 2),
                "l" => (0, 0, w / 2, h),
                "r" => (w / 2, 0, w - w / 2, h),
                "ul" => (0, 0, w / 2, h / 2),
                "ur" => (w / 2, 0, w - w / 2, h / 2),
                "dl" => (0, h / 2, w / 2, h - h / 2),
                "dr" => (w / 2, h / 2, w - w / 2, h - h / 2),
                _ => anyhow::bail!("无效 region: {}", s),
            };
            return Ok(Some([x, y, rw, rh]));
        }
        if let Some(seq) = v.as_sequence() {
            if seq.len() != 4 {
                anyhow::bail!("region 数组需要 [x1, y1, x2, y2] 4 个相对坐标");
            }
            let x1 = seq[0]
                .as_f64()
                .ok_or_else(|| anyhow::anyhow!("region 坐标必须是数字"))?;
            let y1 = seq[1]
                .as_f64()
                .ok_or_else(|| anyhow::anyhow!("region 坐标必须是数字"))?;
            let x2 = seq[2]
                .as_f64()
                .ok_or_else(|| anyhow::anyhow!("region 坐标必须是数字"))?;
            let y2 = seq[3]
                .as_f64()
                .ok_or_else(|| anyhow::anyhow!("region 坐标必须是数字"))?;
            if !(0.0..=1.0).contains(&x1)
                || !(0.0..=1.0).contains(&y1)
                || !(0.0..=1.0).contains(&x2)
                || !(0.0..=1.0).contains(&y2)
            {
                anyhow::bail!("region 相对坐标必须在 0~1 之间");
            }
            if x2 <= x1 || y2 <= y1 {
                anyhow::bail!("region 需要 x2 > x1 且 y2 > y1");
            }
            let x = (x1 * w as f64).round() as u32;
            let y = (y1 * h as f64).round() as u32;
            let rw = (((x2 - x1) * w as f64).round() as u32).max(1);
            let rh = (((y2 - y1) * h as f64).round() as u32).max(1);
            return Ok(Some([x, y, rw, rh]));
        }
        if let Some(map) = v.as_mapping() {
            let (x1, y1) = Self::parse_rel_coord(
                map.get("fm")
                    .ok_or_else(|| anyhow::anyhow!("region 缺少 fm"))?,
            )?;
            let (x2, y2) = Self::parse_rel_coord(
                map.get("to")
                    .ok_or_else(|| anyhow::anyhow!("region 缺少 to"))?,
            )?;
            if x2 <= x1 || y2 <= y1 {
                anyhow::bail!("region 需要 to > fm");
            }
            let x = (x1 * w as f64).round() as u32;
            let y = (y1 * h as f64).round() as u32;
            let rw = (((x2 - x1) * w as f64).round() as u32).max(1);
            let rh = (((y2 - y1) * h as f64).round() as u32).max(1);
            return Ok(Some([x, y, rw, rh]));
        }
        anyhow::bail!(
            "region 只支持 a/u/d/l/r/ul/ur/dl/dr / [x1, y1, x2, y2] / {{fm: [x,y], to: [x,y]}}"
        )
    }

    /// 解析相对坐标点 [x, y]（0~1）：tap / color 坐标 / region fm-to 共用
    fn parse_rel_coord(v: &Value) -> anyhow::Result<(f64, f64)> {
        let seq = v
            .as_sequence()
            .ok_or_else(|| anyhow::anyhow!("需要 [x, y] 数组（相对坐标 0~1）"))?;
        if seq.len() != 2 {
            anyhow::bail!("需要 [x, y] 2 个相对坐标");
        }
        let x = seq[0]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("坐标必须是数字"))?;
        let y = seq[1]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("坐标必须是数字"))?;
        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
            anyhow::bail!("相对坐标必须在 0~1 之间");
        }
        Ok((x, y))
    }

    /// 获取屏幕尺寸：优先 scrcpy 会话元信息，兜底解析截图 PNG
    fn screen_size(&self, ctx: &Ctx, screen: &[u8]) -> (u32, u32) {
        if let Some(s) = self.devices.session(&ctx.device_id) {
            let (w, h) = s.video_size();
            if w > 0 && h > 0 {
                return (w, h);
            }
        }
        image::load_from_memory(screen)
            .map(|img| img.dimensions())
            .unwrap_or((0, 0))
    }

    /// 解析相对坐标（百分比 0~1）：支持数组 [x, y] 或对象 {x, y}
    fn relative_pair(&self, v: &Value) -> anyhow::Result<(f32, f32)> {
        if let Some(seq) = v.as_sequence() {
            let x = seq.first().and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            let y = seq.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            return Ok((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
        }
        if v.is_mapping() {
            let x = v.get("x").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            let y = v.get("y").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            return Ok((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
        }
        anyhow::bail!("相对坐标需要 [x, y] 或 {{x, y}}")
    }
}

/// 常用按键映射（Android keycode）
pub fn key_code(key: &str) -> u32 {
    match key.to_uppercase().as_str() {
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
        _ => {
            if let Ok(n) = key.parse::<u32>() {
                n
            } else {
                warn!("unknown key: {}", key);
                0
            }
        }
    }
}

// 供 API 层使用
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct RunRequest {
    pub device_id: String,
    pub script_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Value {
        serde_yaml::from_str(yaml).expect("yaml parse")
    }

    fn parse_steps(yaml: &str) -> Vec<Value> {
        parse(yaml).as_sequence().unwrap().clone()
    }

    /// 模板名 #后缀区域（与前端 parseTplRegion / parseTplRegionCode 同一套格式）：
    /// 半区码 xx#a/xx#l…、数字坐标 xx#x1_y1_x2_y2（×1000 的 1~3 位整数）；
    /// 无 # / 解析不出 → 全屏（None），不报错
    #[test]
    fn tpl_region_from_name_suffix() {
        let (w, h) = (1920u32, 1080u32);
        let r = |name: &str| Runner::tpl_region_from_name(name, w, h).unwrap();
        // 半区码：a=全屏 None、l=左半屏
        assert_eq!(r("xx#a.png"), None);
        assert_eq!(r("xx#l.png"), Some([0, 0, w / 2, h]));
        assert_eq!(r("xx#DR.png"), Some([w / 2, h / 2, w - w / 2, h - h / 2]));
        // 数字坐标：0_0_500_500 → 左上四分之一
        assert_eq!(r("xx#0_0_500_500.png"), Some([0, 0, w / 2, h / 2]));
        // 带扩展名 .jpeg / 无扩展名
        assert_eq!(r("xx#u.jpeg"), Some([0, 0, w, h / 2]));
        assert_eq!(r("xx#u"), Some([0, 0, w, h / 2]));
        // 无 # / 后缀非法（字母不在码表、段数不对、超 3 位、x2<=x1）→ 全屏不报错
        assert_eq!(r("xx.png"), None);
        assert_eq!(r("xx#foo.png"), None);
        assert_eq!(r("xx#100_200.png"), None);
        assert_eq!(r("xx#1000_0_500_500.png"), None);
        assert_eq!(r("xx#500_0_100_500.png"), None);
    }

    /// 模板短名解析：精确名优先；短名（login.png）唯一匹配 login#*.png；
    /// 多候选报错列名；零候选报不存在
    #[test]
    fn template_short_name_resolution() {
        let dir = std::env::temp_dir().join(format!(
            "gamer-tpl-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cleanup = || {
            let _ = std::fs::remove_dir_all(&dir);
        };
        std::fs::write(dir.join("login#907_160_973_717.png"), b"x").unwrap();
        std::fs::write(dir.join("shop.png"), b"x").unwrap();
        // 短名 → 唯一后缀文件
        let p = Runner::resolve_template_file(&dir, "login.png").unwrap();
        assert!(p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("login#"));
        // 精确名直用
        assert!(
            Runner::resolve_template_file(&dir, "shop.png")
                .unwrap()
                .file_name()
                .unwrap()
                == "shop.png"
        );
        // 不存在
        assert!(Runner::resolve_template_file(&dir, "nope.png").is_err());
        // 同基名多后缀 → 报错消歧；有精确同名文件时精确优先不歧义
        std::fs::write(dir.join("hp#l.png"), b"x").unwrap();
        std::fs::write(dir.join("hp#r.png"), b"x").unwrap();
        assert!(Runner::resolve_template_file(&dir, "hp.png").is_err());
        std::fs::write(dir.join("hp.png"), b"x").unwrap();
        assert!(
            Runner::resolve_template_file(&dir, "hp.png")
                .unwrap()
                .file_name()
                .unwrap()
                == "hp.png"
        );
        cleanup();
    }

    /// 时长参数解析（parse_duration，强制带单位）：单位串 1ms/1s/1m/30min/1h/1d
    /// （大小写不敏感、支持小数如 1.5s、m ≡ min）；裸数字（YAML 数字或纯数字
    /// 串）不再接受；非法值（无数字、未知单位、负数）报错
    #[test]
    fn duration_parse() {
        let d = |yaml: &str| Runner::parse_duration(&parse(yaml), "timeout").unwrap();
        assert_eq!(d("\"1ms\""), 1);
        assert_eq!(d("2s"), 2_000);
        assert_eq!(d("\"1m\""), 60_000);
        assert_eq!(d("\"30min\""), 1_800_000);
        assert_eq!(d("1h"), 3_600_000);
        assert_eq!(d("1d"), 86_400_000);
        assert_eq!(d("\"1.5s\""), 1_500);
        assert_eq!(d("\"80 ms\""), 80);
        assert_eq!(d("\"30MIN\""), 1_800_000);
        // 裸数字强制单位：YAML 数字与纯数字字符串都报错
        assert!(Runner::parse_duration(&parse("500"), "timeout").is_err());
        assert!(Runner::parse_duration(&parse("\"500\""), "timeout").is_err());
        // 非法：未知单位 / 空数字 / 负数 / 非字符串非数字
        assert!(Runner::parse_duration(&parse("fast"), "timeout").is_err());
        assert!(Runner::parse_duration(&parse("\"ms\""), "timeout").is_err());
        assert!(Runner::parse_duration(&parse("\"-5s\""), "timeout").is_err());
        assert!(Runner::parse_duration(&parse("true"), "timeout").is_err());
    }

    /// color 色值键解析：6 位十六进制（不带 #，宽容接受 # / 0x 前缀、大小写）、
    /// [r, g, b] 数组、0x 整数；位数不对 / 非法字符 / 分量越界报错
    #[test]
    fn color_key_parse() {
        let c = |yaml: &str| Runner::parse_color(&parse(yaml)).unwrap();
        assert_eq!(c("ff8800"), (0xff, 0x88, 0x00));
        assert_eq!(c("\"#FF8800\""), (0xff, 0x88, 0x00));
        assert_eq!(c("0xff8800"), (0xff, 0x88, 0x00));
        assert_eq!(c("[255, 136, 0]"), (255, 136, 0));
        assert!(Runner::parse_color(&parse("\"ff880\"")).is_err()); // 5 位
        assert!(Runner::parse_color(&parse("\"ff88000\"")).is_err()); // 7 位
        assert!(Runner::parse_color(&parse("red")).is_err()); // 非十六进制
        assert!(Runner::parse_color(&parse("[255, 136, 256]")).is_err()); // 分量越界
        assert!(Runner::parse_color(&parse("[255, 136]")).is_err()); // 不足 3 元
        assert!(Runner::parse_color(&parse("[a, b, c]")).is_err()); // 非数字
    }

    /// 空格分隔 + 括号感知的实参切分：[x, y] 内部的空格不算分隔符
    #[test]
    fn split_args_bracket_aware() {
        assert_eq!(Runner::split_args("a.png b.png"), vec!["a.png", "b.png"]);
        assert_eq!(
            Runner::split_args("sub.yml a.png [0.5, 0.6] ff8800"),
            vec!["sub.yml", "a.png", "[0.5, 0.6]", "ff8800"]
        );
        assert_eq!(Runner::split_args("  f1  "), vec!["f1"]);
        assert!(Runner::split_args("").is_empty());
    }

    /// call/函数实参替换（$N）：替换作用于 steps 与 config（映射键一起），
    /// **func 段排除**（函数体内 $N 永远指函数实参，调用时才替换）；
    /// `$` 后非数字原样保留；引用越界报错。
    /// 另证 YAML 裸标量 `@` 开头非法（保留字符）——参数引用必须用 `$1` 不能用 `@1`
    #[test]
    fn call_args_substitution() {
        let args: Vec<String> = vec!["a.png".into(), "b.png".into()];
        let mut v = parse(
            "steps:\n  - find: $1\n  - log: \"$2 和 $1\"\n  - call: other.yml $1 x.png\n\
             config:\n  interval: 500ms\nfunc:\n  - f1:\n    - find: $1\n    - return: true\n",
        );
        let funcs_raw = Runner::take_funcs_and_substitute(&mut v, &args).unwrap();
        let steps = v.get("steps").unwrap().as_sequence().unwrap();
        assert_eq!(steps[0].get("find").and_then(|x| x.as_str()), Some("a.png"));
        assert_eq!(
            steps[1].get("log").and_then(|x| x.as_str()),
            Some("b.png 和 a.png")
        );
        // 嵌套 call 转发 $N（替换发生在加载时，转发值不再二次展开）
        assert_eq!(
            steps[2].get("call").and_then(|x| x.as_str()),
            Some("other.yml a.png x.png")
        );
        // func 段未被替换（$1 留给函数调用时）
        let funcs_val = funcs_raw.unwrap();
        let f = funcs_val.as_sequence().unwrap()[0].as_mapping().unwrap();
        let body = f
            .get(Value::String("f1".into()))
            .unwrap()
            .as_sequence()
            .unwrap();
        assert_eq!(body[0].get("find").and_then(|x| x.as_str()), Some("$1"));
        // color 的色值键（映射键）替换
        let mut v2 = parse("steps:\n  - color: [0.5, 0.5]\n    $1:\n      - log: x\n");
        Runner::substitute_args(&mut v2, &args).unwrap();
        let c = v2.get("steps").unwrap().as_sequence().unwrap()[0]
            .as_mapping()
            .unwrap();
        assert!(c.get(Value::String("a.png".into())).is_some());
        // `$` 后非数字原样保留（"100$" / "$涨"）
        let mut v3 = parse("steps:\n  - text: \"100$\"\n  - log: \"$涨\"\n");
        Runner::substitute_args(&mut v3, &args).unwrap();
        let s3 = v3.get("steps").unwrap().as_sequence().unwrap();
        assert_eq!(s3[0].get("text").and_then(|x| x.as_str()), Some("100$"));
        assert_eq!(s3[1].get("log").and_then(|x| x.as_str()), Some("$涨"));
        // 实参含 "$1" 不二次展开（替换值不重扫）
        let mut v4 = parse("steps:\n  - log: $1\n");
        Runner::substitute_args(&mut v4, &["$1".to_string()]).unwrap();
        assert_eq!(
            v4.get("steps").unwrap().as_sequence().unwrap()[0]
                .get("log")
                .and_then(|x| x.as_str()),
            Some("$1")
        );
        // 引用越界：未提供实参（主脚本直接运行）/ 序号超出 / 取最长数字串
        let mut v5 = parse("steps:\n  - find: $1\n");
        assert!(Runner::substitute_args(&mut v5, &[]).is_err());
        let mut v6 = parse("steps:\n  - find: $3\n");
        assert!(Runner::substitute_args(&mut v6, &args).is_err());
        let mut v7 = parse("steps:\n  - log: $12\n");
        assert!(Runner::substitute_args(&mut v7, &args).is_err());
        // YAML 裸标量 @ 开头是保留字符，解析直接失败——参数引用必须用 $（不能用 @1）
        assert!(serde_yaml::from_str::<Value>("steps:\n  - find: @1").is_err());
        assert_eq!(
            parse("steps:\n  - find: $1")
                .get("steps")
                .unwrap()
                .as_sequence()
                .unwrap()[0]
                .get("find")
                .and_then(|x| x.as_str()),
            Some("$1")
        );
    }

    /// ^N 上下文替换：步骤自身的标量值（含拼接串）替换；then/else 等步骤列表
    /// （映射项）不替换——留给各自执行时按当时的栈顶绑定；越界报错。
    /// ^ 不是 YAML 保留字符（& 是——锚点），裸写 ^1 合法
    #[test]
    fn ref_substitution() {
        let refs = vec!["main.png".to_string(), "b1.png".to_string()];
        // exec_step 以步骤映射本身调用 substitute_refs（非步骤序列）
        let step =
            parse("- func1: ^1 ^2\n  then:\n    - log: got ^1\n  else:\n    - call: sub.yml ^2")
                .get(0)
                .unwrap()
                .clone();
        let out = Runner::substitute_refs(&step, &refs).unwrap();
        let m = out.as_mapping().unwrap();
        assert_eq!(
            m.get(Value::String("func1".into()))
                .and_then(|v| v.as_str()),
            Some("main.png b1.png")
        );
        // then/else 子树（映射步骤项）不替换
        let then = m
            .get(Value::String("then".into()))
            .unwrap()
            .as_sequence()
            .unwrap();
        assert_eq!(then[0].get("log").and_then(|v| v.as_str()), Some("got ^1"));
        let els = m
            .get(Value::String("else".into()))
            .unwrap()
            .as_sequence()
            .unwrap();
        assert_eq!(
            els[0].get("call").and_then(|v| v.as_str()),
            Some("sub.yml ^2")
        );
        // block 列表（字符串项）替换
        let f = parse("- find: ^1\n  block:\n    - ^2\n    - other.png")
            .get(0)
            .unwrap()
            .clone();
        let out2 = Runner::substitute_refs(&f, &refs).unwrap();
        let fm = out2.as_mapping().unwrap();
        assert_eq!(
            fm.get(Value::String("block".into()))
                .and_then(|v| v.as_sequence().cloned()),
            Some(vec![
                Value::String("b1.png".into()),
                Value::String("other.png".into())
            ])
        );
        // ^ 后非数字原样保留；越界报错
        assert_eq!(
            Runner::substitute_ref_str("a^b 100^", &refs).unwrap(),
            "a^b 100^"
        );
        assert!(Runner::substitute_ref_str("^3", &refs).is_err());
        // ^ 裸标量合法（& 会变锚点导致值丢失——弃用 &N 的原因）
        assert_eq!(
            parse("steps:\n  - func1: ^1")
                .get("steps")
                .unwrap()
                .as_sequence()
                .unwrap()[0]
                .get("func1")
                .and_then(|x| x.as_str()),
            Some("^1")
        );
        let anchored: Value = serde_yaml::from_str("steps:\n  - func1: &1").unwrap();
        assert!(anchored.get("steps").unwrap().as_sequence().unwrap()[0]
            .get("func1")
            .unwrap()
            .is_null());
    }

    /// color 步骤的 YAML 结构（serde_yaml 与前端 js-yaml 同构）：
    /// 色值键挂步骤列表（可留空 null）；else 是 color 的**兄弟键**（与色值键
    /// 同列、不带 -，序列在非 dash 行自动收口回到步骤映射——同 `a:\n- 1\nb: 2` 机制）
    #[test]
    fn color_syntax_parse() {
        let doc = parse(
            "steps:\n  - color: [0.5, 0.5]\n    ff8800:\n      - log: hit\n    aa8899:\n    else:\n      - log: none\n",
        );
        let step = &doc.get("steps").unwrap().as_sequence().unwrap()[0];
        let m = step.as_mapping().unwrap();
        assert_eq!(
            m.get(Value::String("color".into()))
                .unwrap()
                .as_sequence()
                .unwrap()
                .len(),
            2
        );
        assert!(m
            .get(Value::String("ff8800".into()))
            .unwrap()
            .as_sequence()
            .is_some());
        assert!(m.get(Value::String("aa8899".into())).unwrap().is_null());
        assert!(step.get("else").unwrap().as_sequence().is_some());
        // else 不在色值键里（是兄弟键）
        assert_eq!(
            m.get(Value::String("else".into()))
                .and_then(|v| v.as_sequence())
                .map(|s| s.len()),
            Some(1)
        );
        // $N 替换覆盖色值键（substitute_args 替换映射键）
        let mut d2 = parse("steps:\n  - color: [0.5, 0.5]\n    $1:\n      - log: x\n");
        Runner::substitute_args(&mut d2, &["ff8800".into()]).unwrap();
        let s2 = d2.get("steps").unwrap().as_sequence().unwrap();
        assert!(s2[0]
            .as_mapping()
            .unwrap()
            .get(Value::String("ff8800".into()))
            .is_some());
    }

    /// 构建测试 Runner/Ctx（不依赖设备）
    fn test_runner_ctx() -> (Runner, Ctx) {
        let dir = std::env::temp_dir().join(format!(
            "gamer-engine-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = crate::config::Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let db: crate::store::Db = std::sync::Arc::new(crate::store::Store::open(&cfg).unwrap());
        let viewers: crate::webrtc::ViewerMap =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let devices = std::sync::Arc::new(crate::device::DeviceManager::new(
            db.clone(),
            cfg.clone(),
            viewers.clone(),
        ));
        let scripts = std::sync::Arc::new(crate::scripts::ScriptStore::open(&cfg).unwrap());
        let runner = Runner::new(devices, viewers, scripts);
        let ctx = Ctx {
            device_id: "test-dev".into(),
            script_id: "com.test/t.yaml".into(),
            log: Vec::new(),
            stop: Arc::new(AtomicBool::new(false)),
            exit: Arc::new(AtomicBool::new(false)),
            interval_ms: 5,
            threshold: 0.85,
            log_level_rank: 0,
            funcs: HashMap::new(),
            func_depth: 0,
            return_value: None,
            ref_stack: Vec::new(),
            region_warned: HashSet::new(),
            log_cb: None,
        };
        (runner, ctx)
    }

    /// exec_step 的解析期校验回归（2026-08-26 语法精简后）：
    /// 旧写法（until / check / cond / exit / goto / count 等）显式报错引导迁移；
    /// find/color/loop 参数校验都在触碰设备/截图之前报错
    #[tokio::test]
    async fn step_validation() {
        let (runner, mut ctx) = test_runner_ctx();
        async fn run(runner: &Runner, ctx: &mut Ctx, yaml: &str) -> anyhow::Result<()> {
            let step = parse(yaml).get(0).unwrap().clone();
            runner.exec_step(ctx, &step).await
        }
        // 旧动作改名：until → find、check → block、cond → color、exit → throw
        assert!(run(&runner, &mut ctx, "- until: a.png")
            .await
            .unwrap_err()
            .to_string()
            .contains("until 已改名 find"));
        assert!(run(&runner, &mut ctx, "- find: a.png\n  check: b.png")
            .await
            .unwrap_err()
            .to_string()
            .contains("check 已改名 block"));
        assert!(run(&runner, &mut ctx, "- cond:\n  - a.png: x")
            .await
            .unwrap_err()
            .to_string()
            .contains("cond 已改名 color"));
        assert!(run(&runner, &mut ctx, "- exit")
            .await
            .unwrap_err()
            .to_string()
            .contains("exit 已改名 throw"));
        assert!(run(&runner, &mut ctx, "- goto: x")
            .await
            .unwrap_err()
            .to_string()
            .contains("goto/label 已删除"));
        assert!(run(&runner, &mut ctx, "- label: x")
            .await
            .unwrap_err()
            .to_string()
            .contains("goto/label 已删除"));
        // 已删除参数
        for k in [
            "count", "cnt_ivl", "cnt_chk", "img_ivl", "and_or", "click", "before", "after",
        ] {
            assert!(
                run(&runner, &mut ctx, &format!("- find: a.png\n  {}: 1", k))
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("已删除")
            );
        }
        assert!(run(&runner, &mut ctx, "- find: a.png\n  threshold: 0.9")
            .await
            .unwrap_err()
            .to_string()
            .contains("threshold 步骤参数已删除"));
        assert!(run(&runner, &mut ctx, "- find: a.png\n  region: l")
            .await
            .unwrap_err()
            .to_string()
            .contains("region 步骤参数已删除"));
        // 步骤级 wait 参数已删除（wait 现在是独立动作 → 多动作键报错）
        assert!(run(&runner, &mut ctx, "- tap: [0.1, 0.1]\n  wait: 1s")
            .await
            .unwrap_err()
            .to_string()
            .contains("wait 是独立动作"));
        assert!(run(&runner, &mut ctx, "- tap: [0.1, 0.1]\n  log: x")
            .await
            .unwrap_err()
            .to_string()
            .contains("一个步骤只能有一个动作键"));
        // 未知动作
        assert!(run(&runner, &mut ctx, "- var: x")
            .await
            .unwrap_err()
            .to_string()
            .contains("未知动作"));
        // 带值动作漏写冒号（标量步骤）→ 定向提示补冒号
        let e = run(&runner, &mut ctx, "- throw 未知界面")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("需写冒号") && e.contains("- throw: 未知界面"),
            "{}",
            e
        );
        let e = run(&runner, &mut ctx, "- log abc")
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("- log: abc"), "{}", e);
        // find 校验（截图前报错）
        assert!(run(&runner, &mut ctx, "- find: a.png, b.png")
            .await
            .unwrap_err()
            .to_string()
            .contains("单个主模板"));
        assert!(run(&runner, &mut ctx, "- find: a.png\n  block: a.png")
            .await
            .unwrap_err()
            .to_string()
            .contains("与 find 主模板重复"));
        // timeout 裸数字（含 0）先触发强制单位；带单位的 0 才是"必须 > 0"
        assert!(run(&runner, &mut ctx, "- find: a.png\n  timeout: 0")
            .await
            .unwrap_err()
            .to_string()
            .contains("带单位"));
        assert!(run(&runner, &mut ctx, "- find: a.png\n  timeout: 0s")
            .await
            .unwrap_err()
            .to_string()
            .contains("timeout 必须 > 0"));
        assert!(run(&runner, &mut ctx, "- find: a.png\n  timeout: 500")
            .await
            .unwrap_err()
            .to_string()
            .contains("带单位"));
        assert!(run(&runner, &mut ctx, "- find: a.png\n  timeout: fast")
            .await
            .unwrap_err()
            .to_string()
            .contains("带单位"));
        assert!(run(&runner, &mut ctx, "- find: a.png\n  verify: 123")
            .await
            .unwrap_err()
            .to_string()
            .contains("verify 需要 true / false"));
        assert!(run(&runner, &mut ctx, "- find: a.png\n  foo: 1")
            .await
            .unwrap_err()
            .to_string()
            .contains("不支持参数 foo"));
        // 合法 find → 无设备在截图处失败（证明校验未误伤）
        assert!(run(
            &runner,
            &mut ctx,
            "- find: a.png\n  timeout: 30min\n  block: b.png, c.png\n  verify: true"
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("截图失败"));
        // color 校验
        assert!(run(&runner, &mut ctx, "- color: [0.5, 0.5]")
            .await
            .unwrap_err()
            .to_string()
            .contains("至少需要一个色值键"));
        assert!(run(
            &runner,
            &mut ctx,
            "- color: [0.5, 0.5]\n  red:\n    - log: x"
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("色值键"));
        assert!(
            run(&runner, &mut ctx, "- color: [0.5, 0.5]\n  ff8800: log x")
                .await
                .unwrap_err()
                .to_string()
                .contains("步骤列表")
        );
        assert!(run(&runner, &mut ctx, "- color: [1.5, 0.5]\n  ff8800:")
            .await
            .unwrap_err()
            .to_string()
            .contains("0~1"));
        assert!(run(&runner, &mut ctx, "- color: x\n  ff8800:")
            .await
            .unwrap_err()
            .to_string()
            .contains("color 坐标"));
        // 合法 color → 截图处失败
        assert!(run(
            &runner,
            &mut ctx,
            "- color: [0.5, 0.5]\n  ff8800:\n    - log: x\n  else:\n    - log: none"
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("截图失败"));
        // loop：缺 steps 报错；times=3 执行 3 次（log 无需设备）
        assert!(run(&runner, &mut ctx, "- loop:\n  times: 3")
            .await
            .unwrap_err()
            .to_string()
            .contains("需要 steps"));
        ctx.log.clear();
        run(
            &runner,
            &mut ctx,
            "- loop:\n  times: 3\n  steps:\n    - log: x",
        )
        .await
        .unwrap();
        assert_eq!(ctx.log.iter().filter(|(_, m)| m == "x").count(), 3);
        // wait：裸数字报错；带单位可执行
        assert!(run(&runner, &mut ctx, "- wait: 100")
            .await
            .unwrap_err()
            .to_string()
            .contains("带单位"));
        run(&runner, &mut ctx, "- wait: 1ms").await.unwrap();
        run(&runner, &mut ctx, "- wait: [1ms, 2ms]").await.unwrap();
        assert!(run(&runner, &mut ctx, "- wait: [1ms]")
            .await
            .unwrap_err()
            .to_string()
            .contains("[最小, 最大]"));
        // swipe：from 别名报错、time 裸数字报错
        assert!(run(
            &runner,
            &mut ctx,
            "- swipe:\n    from: [0.1, 0.1]\n    to: [0.2, 0.2]"
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("from 已改名 fm"));
        assert!(run(
            &runner,
            &mut ctx,
            "- swipe:\n    fm: [0.1, 0.1]\n    to: [0.2, 0.2]\n    time: 300"
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("带单位"));
        // throw：无参 / 带参均立即设置 exit 标志并打印日志（无需设备）
        ctx.exit.store(false, Ordering::SeqCst);
        run(&runner, &mut ctx, "- throw").await.unwrap();
        assert!(ctx.exit.load(Ordering::SeqCst));
        assert!(ctx.log.iter().any(|(_, m)| m == "结束运行脚本"));
        ctx.exit.store(false, Ordering::SeqCst);
        run(&runner, &mut ctx, "- throw: 体力不足").await.unwrap();
        assert!(ctx.exit.load(Ordering::SeqCst));
        assert!(ctx.log.iter().any(|(_, m)| m == "因 体力不足 结束运行脚本"));
        // str_app：带值报错（裸写才合法）
        assert!(run(&runner, &mut ctx, "- str_app: com.x.y")
            .await
            .unwrap_err()
            .to_string()
            .contains("不支持参数"));
        assert!(run(&runner, &mut ctx, "- cls_app: com.x.y")
            .await
            .unwrap_err()
            .to_string()
            .contains("不支持参数"));
        // return 只能在函数内
        assert!(run(&runner, &mut ctx, "- return: true")
            .await
            .unwrap_err()
            .to_string()
            .contains("return 仅可在自定义函数内使用"));
        assert!(run(&runner, &mut ctx, "- return")
            .await
            .unwrap_err()
            .to_string()
            .contains("return 需要 true / false"));
    }

    /// 自定义函数：$N 实参替换、return true/false 分支、fall-through=true
    /// （2026-08-27 改，旧语义为 false）、函数调用步骤的 then/else 生效
    /// （log/loop 均无需设备）
    #[tokio::test]
    async fn func_call_and_return() {
        let (runner, mut ctx) = test_runner_ctx();
        let f = |body: &str| FuncDef {
            cond: Vec::new(),
            body: parse_steps(body),
        };
        // f1：log 实参 + return true；f2：无 return（fall-through 默认 true）
        ctx.funcs
            .insert("f1".into(), f("- log: got $1\n- return: true"));
        ctx.funcs.insert("f2".into(), f("- log: always"));
        // f1 返回 true → then 分支
        ctx.log.clear();
        run_step(
            &runner,
            &mut ctx,
            "- f1: hello\n  then:\n    - log: T\n  else:\n    - log: F",
        )
        .await
        .unwrap();
        assert!(ctx.log.iter().any(|(_, m)| m == "got hello"));
        assert!(ctx.log.iter().any(|(_, m)| m == "T"));
        assert!(!ctx.log.iter().any(|(_, m)| m == "F"));
        // f2 无 return → 默认 true → then 分支（旧语义为 false 走 else）
        ctx.log.clear();
        run_step(
            &runner,
            &mut ctx,
            "- f2:\n  then:\n    - log: T\n  else:\n    - log: F",
        )
        .await
        .unwrap();
        assert!(ctx.log.iter().any(|(_, m)| m == "always"));
        assert!(ctx.log.iter().any(|(_, m)| m == "T"));
        assert!(!ctx.log.iter().any(|(_, m)| m == "F"));
        // 显式 return: false → else 分支
        ctx.funcs
            .insert("f2b".into(), f("- log: always\n- return: false"));
        ctx.log.clear();
        run_step(
            &runner,
            &mut ctx,
            "- f2b:\n  then:\n    - log: T\n  else:\n    - log: F",
        )
        .await
        .unwrap();
        assert!(ctx.log.iter().any(|(_, m)| m == "F"));
        assert!(!ctx.log.iter().any(|(_, m)| m == "T"));
        // 函数实参越界（体内 $2 但只传 1 个）
        ctx.funcs.insert("f3".into(), f("- log: $2"));
        assert!(run_step(&runner, &mut ctx, "- f3: only-one")
            .await
            .unwrap_err()
            .to_string()
            .contains("超出实参数量"));
        // 函数调用步骤带非法参数
        assert!(run_step(&runner, &mut ctx, "- f1: x\n  foo: 1")
            .await
            .unwrap_err()
            .to_string()
            .contains("不支持参数"));
        // return 后函数体剩余步骤跳过（return 冒泡）
        ctx.funcs
            .insert("f4".into(), f("- return: false\n- log: skipped"));
        ctx.log.clear();
        run_step(&runner, &mut ctx, "- f4:").await.unwrap();
        assert!(!ctx.log.iter().any(|(_, m)| m == "skipped"));
        // 嵌套函数：f5 调 f1（内层 return 不影响外层继续执行）
        ctx.funcs.insert(
            "f5".into(),
            f("- f1: inner\n  else:\n    - log: inner-else\n- log: after-inner\n- return: true"),
        );
        ctx.log.clear();
        run_step(&runner, &mut ctx, "- f5:\n  then:\n    - log: outer-T")
            .await
            .unwrap();
        assert!(ctx.log.iter().any(|(_, m)| m == "got inner"));
        assert!(!ctx.log.iter().any(|(_, m)| m == "inner-else"));
        assert!(ctx.log.iter().any(|(_, m)| m == "after-inner"));
        assert!(ctx.log.iter().any(|(_, m)| m == "outer-T"));
    }

    /// func 段解析（parse_funcs）：旧写法（函数体直接挂函数名键）与 cond/steps
    /// 写法（兄弟键 / 映射嵌套 / cond 在函数体之后）都解析出 cond + 函数体；
    /// cond 单模板字符串 / 逗号分隔 / 列表；错误场景（函数名 cond 保留字、
    /// 函数体非法）报错。注：cond/steps 缩进在 **func: 段内**（项 dash 在 2 列）
    /// 是函数名键的同列兄弟键；单元测试按真实脚本形态（func: 包裹）解析
    #[test]
    fn func_def_parse() {
        let p = |yaml: &str| {
            let doc = parse(yaml);
            Runner::parse_funcs(Some(doc.get("func").cloned().unwrap())).unwrap()
        };
        // 旧写法：无 cond
        let m = p("func:\n  - f1:\n    - log: x");
        assert!(m.get("f1").unwrap().cond.is_empty());
        assert_eq!(m.get("f1").unwrap().body.len(), 1);
        // cond + steps 兄弟键（列表形式）
        let m = p("func:\n  - f1:\n    cond: test.png\n    steps:\n      - log: x");
        assert_eq!(m.get("f1").unwrap().cond, vec!["test.png"]);
        assert_eq!(m.get("f1").unwrap().body.len(), 1);
        // cond 多模板列表
        let m = p(
            "func:\n  - f1:\n    cond:\n      - a.png\n      - b.png\n    steps:\n      - log: x",
        );
        assert_eq!(m.get("f1").unwrap().cond, vec!["a.png", "b.png"]);
        // cond 逗号分隔字符串
        let m = p("func:\n  - f1:\n    cond: a.png, b.png\n    steps:\n      - log: x");
        assert_eq!(m.get("f1").unwrap().cond, vec!["a.png", "b.png"]);
        // cond 在函数体之后（兄弟键）
        let m = p("func:\n  - f1:\n    - log: x\n    cond: test.png");
        assert_eq!(m.get("f1").unwrap().cond, vec!["test.png"]);
        // 映射形式（cond/steps 嵌套在函数名值里）
        let m = p("func:\n  f1:\n    cond: test.png\n    steps:\n      - log: x");
        assert_eq!(m.get("f1").unwrap().cond, vec!["test.png"]);
        // 错误：函数名 cond 是保留字（仅含 cond 键的项）
        assert!(Runner::parse_funcs(Some(
            parse("func:\n  - cond:\n    - log: x")
                .get("func")
                .cloned()
                .unwrap()
        ))
        .unwrap_err()
        .to_string()
        .contains("cond 是保留字"));
        // 错误：函数体非法（标量非列表）
        assert!(Runner::parse_funcs(Some(
            parse("func:\n  - f1: 123").get("func").cloned().unwrap()
        ))
        .unwrap_err()
        .to_string()
        .contains("函数体需要步骤列表"));
        // 错误：cond 模板列表项非法（非字符串）
        assert!(Runner::parse_funcs(Some(
            parse("func:\n  - f1:\n    cond:\n      - 123\n    steps:\n      - log: x")
                .get("func")
                .cloned()
                .unwrap()
        ))
        .unwrap_err()
        .to_string()
        .contains("cond"));
    }

    /// 跨文件函数调用（exec_cross_func）：子脚本按名解析（同分区优先、缺扩展名
    /// 自动补全）、函数体 $N 按调用点实参替换、子脚本内裸函数名解析、调用者
    /// 函数兜底且不泄漏、fall-through 默认 true、函数不存在/子脚本不存在报错、
    /// 模板按被引用脚本分区解析（log 步骤无需设备/模板）
    #[tokio::test]
    async fn cross_file_func_call() {
        let (runner, _ctx) = test_runner_ctx();
        let stop = Arc::new(AtomicBool::new(false));
        // 先落盘子脚本 test1.yaml（与测试分区 com.test 一致）
        let sub = "func:\n  - fun1:\n    - log: cross $1\n    - return: true\n  - fun2:\n    - log: f2 called\n    - fun1: inner\n      then:\n        - log: f2-inner-ok\nsteps:\n  - log: sub-top";
        runner
            .scripts
            .save(None, "com.test", "test1.yaml", sub)
            .unwrap();
        // 同分区调用（写短名 test1）+ 无参调用 + 子脚本内裸函数名互调（fun2 → f1）
        let caller = "func:\n  - own:\n    - log: own $1\nsteps:\n  - test1:fun1: hello\n    then:\n      - log: T1\n    else:\n      - log: F1\n  - test1:fun2:\n    then:\n      - log: T2\n  - test1.yaml:fun1: world\n  - own: back";
        let logs = runner
            .run(
                "dev",
                "com.test/t.yml",
                caller,
                stop.clone(),
                None,
                0,
                None,
                None,
                vec![],
            )
            .await
            .unwrap();
        assert!(logs.iter().any(|(_, m)| m == "cross hello"));
        assert!(logs.iter().any(|(_, m)| m == "cross world"));
        assert!(logs.iter().any(|(_, m)| m == "T1"));
        assert!(!logs.iter().any(|(_, m)| m == "F1"));
        assert!(logs.iter().any(|(_, m)| m == "f2 called"));
        assert!(logs.iter().any(|(_, m)| m == "cross inner")); // 子脚本内 fun2 → fun1 互调
        assert!(logs.iter().any(|(_, m)| m == "f2-inner-ok"));
        assert!(logs.iter().any(|(_, m)| m == "T2")); // fun2 无 return → 默认 true → then
                                                      // 调用者函数在跨文件调用后仍可用（不泄漏子脚本函数：own 在后）
        assert!(logs.iter().any(|(_, m)| m == "own back"));
        // 子脚本不存在
        let caller = "steps:\n  - nope:fun1: x";
        assert!(runner
            .run(
                "dev",
                "com.test/t.yml",
                caller,
                stop.clone(),
                None,
                0,
                None,
                None,
                vec![]
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("子脚本不存在"));
        // 子脚本无该函数
        let caller = "steps:\n  - test1:nope";
        assert!(runner
            .run(
                "dev",
                "com.test/t.yml",
                caller,
                stop.clone(),
                None,
                0,
                None,
                None,
                vec![]
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("未定义函数"));
        // 纯函数库脚本（无 steps）：直接运行不报错且不做动作；跨文件函数可正常调用
        runner
            .scripts
            .save(
                None,
                "com.test",
                "lib_only.yaml",
                "func:\n  - hello:\n    - log: lib hello\nsteps: ~",
            )
            .unwrap();
        let logs = runner
            .run(
                "dev",
                "com.test/lib_only.yaml",
                "func:\n  - hello:\n    - log: lib hello",
                stop.clone(),
                None,
                0,
                None,
                None,
                vec![],
            )
            .await
            .unwrap();
        assert!(logs
            .iter()
            .any(|(_, m)| m == "纯函数库脚本（无 steps）：仅提供函数，直接运行不做任何动作"));
        assert!(!logs.iter().any(|(_, m)| m == "lib hello"));
        // call 一个纯函数库脚本：无动作 + 提示日志（不报错）
        let caller = "steps:\n  - call: lib_only.yaml";
        let logs = runner
            .run(
                "dev",
                "com.test/t.yml",
                caller,
                stop.clone(),
                None,
                0,
                None,
                None,
                vec![],
            )
            .await
            .unwrap();
        assert!(logs
            .iter()
            .any(|(_, m)| m == "纯函数库脚本（无 steps）：仅提供函数，直接运行不做任何动作"));
        // 跨文件函数调用纯函数库：正常执行函数体
        let caller = "steps:\n  - lib_only:hello";
        let logs = runner
            .run(
                "dev",
                "com.test/t.yml",
                caller,
                stop.clone(),
                None,
                0,
                None,
                None,
                vec![],
            )
            .await
            .unwrap();
        assert!(logs.iter().any(|(_, m)| m == "lib hello"));
        // run_func 直接运行函数库脚本的函数体（无 steps 时同样合法）
        let logs = runner
            .run(
                "dev",
                "com.test/lib_only.yaml",
                "func:\n  - hello:\n    - log: lib hello",
                stop.clone(),
                None,
                0,
                Some("hello"),
                None,
                vec![],
            )
            .await
            .unwrap();
        assert!(logs.iter().any(|(_, m)| m == "lib hello"));
        // 省略 func: 的简写函数库（顶层映射直接写函数定义）同样可被跨文件调用
        runner
            .scripts
            .save(
                None,
                "com.test",
                "lib_bare.yaml",
                "hello:\n  - log: bare hello",
            )
            .unwrap();
        let logs = runner
            .run(
                "dev",
                "com.test/t.yml",
                "steps:\n  - lib_bare:hello",
                stop.clone(),
                None,
                0,
                None,
                None,
                vec![],
            )
            .await
            .unwrap();
        assert!(logs.iter().any(|(_, m)| m == "bare hello"));
    }

    async fn run_step(runner: &Runner, ctx: &mut Ctx, yaml: &str) -> anyhow::Result<()> {
        let step = parse(yaml).get(0).unwrap().clone();
        runner.exec_step(ctx, &step).await
    }

    /// run() 顶层校验 + config: 段 + log_level 过滤 + $N 越界（无需设备，
    /// 步骤用 log）
    #[tokio::test]
    async fn run_top_level_and_config() {
        let (runner, _ctx) = test_runner_ctx();
        let stop = Arc::new(AtomicBool::new(false));
        async fn run_yaml(
            runner: &Runner,
            stop: &Arc<AtomicBool>,
            yaml: &str,
        ) -> anyhow::Result<Vec<(String, String)>> {
            runner
                .run(
                    "dev",
                    "com.test/t.yaml",
                    yaml,
                    stop.clone(),
                    None,
                    0,
                    None,
                    None,
                    vec![],
                )
                .await
        }
        // 顶层键白名单：旧 action_wait / log_level / name / 未知键报错
        assert!(
            run_yaml(&runner, &stop, "action_wait: 500\nsteps:\n  - log: x")
                .await
                .unwrap_err()
                .to_string()
                .contains("action_wait 已删除")
        );
        assert!(
            run_yaml(&runner, &stop, "log_level: info\nsteps:\n  - log: x")
                .await
                .unwrap_err()
                .to_string()
                .contains("log_level 已删除")
        );
        assert!(run_yaml(&runner, &stop, "name: x\nsteps:\n  - log: x")
            .await
            .unwrap_err()
            .to_string()
            .contains("name 已删除"));
        assert!(run_yaml(&runner, &stop, "foo: 1\nsteps:\n  - log: x")
            .await
            .unwrap_err()
            .to_string()
            .contains("未知顶层键"));
        // steps 与 func 都没有 → 报错；改后文案明确提示纯函数库需要至少一个函数
        assert!(run_yaml(&runner, &stop, "config:\n  interval: 500ms")
            .await
            .unwrap_err()
            .to_string()
            .contains("需要 steps 或 func"));
        assert!(run_yaml(&runner, &stop, "")
            .await
            .unwrap_err()
            .to_string()
            .contains("需要 steps 或 func"));
        // 顶层序列 = 省略 steps: 的单段脚本（旧版本此写法报"需要 steps 或 func"）
        let logs = run_yaml(&runner, &stop, "- log: a\n- log: b")
            .await
            .unwrap();
        assert!(logs.iter().any(|(_, m)| m == "a"));
        assert!(logs.iter().any(|(_, m)| m == "b"));
        // 无段落键的顶层映射 = 省略 func: 的纯函数库简写（直接运行不做动作）
        let lib = "f1:\n  cond: a.png\n  steps:\n    - log: in f1\nf2:\n  - log: in f2";
        let logs = run_yaml(&runner, &stop, lib).await.unwrap();
        assert!(logs
            .iter()
            .any(|(_, m)| m == "纯函数库脚本（无 steps）：仅提供函数，直接运行不做任何动作"));
        assert!(!logs.iter().any(|(_, m)| m.contains("in f")));
        // run_func 直接运行简写函数库的函数体
        let logs = runner
            .run(
                "dev",
                "com.test/t.yaml",
                lib,
                stop.clone(),
                None,
                0,
                Some("f2"),
                None,
                vec![],
            )
            .await
            .unwrap();
        assert!(logs.iter().any(|(_, m)| m == "in f2"));
        // config 子键裸写顶层（无段落键）→ 定向报错（不能当函数名）
        assert!(run_yaml(&runner, &stop, "interval: 500ms")
            .await
            .unwrap_err()
            .to_string()
            .contains("config: 段参数"));
        // 顶层映射值不是函数体形态 → parse_funcs 报函数体错误
        assert!(run_yaml(&runner, &stop, "foo: 1")
            .await
            .unwrap_err()
            .to_string()
            .contains("函数体需要步骤列表"));
        // 合法脚本：log 正常执行
        let logs = run_yaml(&runner, &stop, "steps:\n  - log: hello")
            .await
            .unwrap();
        assert!(logs.iter().any(|(_, m)| m == "hello"));
        // config 段：interval 裸数字报错；log_level=warn 丢弃 info 日志
        assert!(run_yaml(
            &runner,
            &stop,
            "config:\n  interval: 100\nsteps:\n  - log: x"
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("带单位"));
        assert!(
            run_yaml(&runner, &stop, "config:\n  foo: 1\nsteps:\n  - log: x")
                .await
                .unwrap_err()
                .to_string()
                .contains("不支持的键")
        );
        let logs = run_yaml(
            &runner,
            &stop,
            "config:\n  interval: 100ms\nsteps:\n  - log: hello",
        )
        .await
        .unwrap();
        assert!(logs.iter().any(|(_, m)| m == "hello"));
        let logs = run_yaml(
            &runner,
            &stop,
            "config:\n  log_level: warn\nsteps:\n  - log: dropped",
        )
        .await
        .unwrap();
        assert!(!logs.iter().any(|(_, m)| m == "dropped"));
        // 列表形式 config（按序覆盖）
        let logs = run_yaml(
            &runner,
            &stop,
            "config:\n  - log_level: error\n  - log_level: debug\nsteps:\n  - log: kept",
        )
        .await
        .unwrap();
        assert!(logs.iter().any(|(_, m)| m == "kept"));
        // 主脚本直接运行含 $N 的脚本 → 越界报错（func 段除外）
        assert!(run_yaml(&runner, &stop, "steps:\n  - find: $1")
            .await
            .unwrap_err()
            .to_string()
            .contains("超出实参数量"));
        let logs = run_yaml(
            &runner,
            &stop,
            "func:\n  - f1:\n    - log: $1\nsteps:\n  - f1: ok",
        )
        .await
        .unwrap();
        assert!(logs.iter().any(|(_, m)| m == "ok"));
        // func 定义校验：保留字函数名 / 重复定义 / 非法函数体
        assert!(run_yaml(
            &runner,
            &stop,
            "func:\n  - find:\n    - log: x\nsteps:\n  - log: x"
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("保留字"));
        assert!(
            run_yaml(&runner, &stop, "func:\n  - f1: 123\nsteps:\n  - log: x")
                .await
                .unwrap_err()
                .to_string()
                .contains("函数体需要步骤列表")
        );
        assert!(run_yaml(
            &runner,
            &stop,
            "func:\n  - f1:\n    - log: a\n  - f1:\n    - log: b\nsteps:\n  - log: x"
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("重复定义"));
    }

    /// run_func 直接运行函数体：不跑顶层 steps、start_step 定位函数体内、
    /// 函数未定义报错、体内 $N 保持字面量（无实参不替换）
    #[tokio::test]
    async fn run_func_body() {
        let (runner, _ctx) = test_runner_ctx();
        let stop = Arc::new(AtomicBool::new(false));
        let yaml = "func:\n  - f1:\n    - log: a\n    - log: b\nsteps:\n  - log: top";
        // 从头跑函数体：只执行 f1（顶层 steps 不跑）
        let logs = runner
            .run(
                "dev",
                "com.test/t.yaml",
                yaml,
                stop.clone(),
                None,
                0,
                Some("f1"),
                None,
                vec![],
            )
            .await
            .unwrap();
        assert!(logs.iter().any(|(_, m)| m == "a"));
        assert!(!logs.iter().any(|(_, m)| m == "top"));
        // start_step=1：函数体内从第 2 步开始
        let logs = runner
            .run(
                "dev",
                "com.test/t.yaml",
                yaml,
                stop.clone(),
                None,
                1,
                Some("f1"),
                None,
                vec![],
            )
            .await
            .unwrap();
        assert!(!logs.iter().any(|(_, m)| m == "a"));
        assert!(logs.iter().any(|(_, m)| m == "b"));
        // 未定义函数
        assert!(runner
            .run(
                "dev",
                "com.test/t.yaml",
                yaml,
                stop.clone(),
                None,
                0,
                Some("nope"),
                None,
                vec![]
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("未定义"));
        // 体内 $N 字面量保留：直接运行含 $1 的函数体不报越界（模板解析期才失败）
        let yaml2 = "func:\n  - f2:\n    - log: $1\nsteps:\n  - f2: x";
        let logs = runner
            .run(
                "dev",
                "com.test/t.yaml",
                yaml2,
                stop.clone(),
                None,
                0,
                Some("f2"),
                None,
                vec![],
            )
            .await
            .unwrap();
        assert!(logs.iter().any(|(_, m)| m == "$1"));
        // 从头运行（start_step=0）先检查 cond——Console 点击函数名行 = 整函数
        // 从头跑：cond 需要截图，测试环境无设备 → 报截图失败（证明 cond 被检查）；
        // start_step=1（点击函数体内行）跳过 cond 直接执行体内第 2 步
        let yamlc = "func:\n  - fc:\n    cond: a.png\n    steps:\n      - log: c1\n      - log: c2\nsteps:\n  - log: top";
        let err = runner
            .run(
                "dev",
                "com.test/t.yaml",
                yamlc,
                stop.clone(),
                None,
                0,
                Some("fc"),
                None,
                vec![],
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("截图失败"),
            "cond 应被检查（截图失败），实际: {}",
            err
        );
        let logs = runner
            .run(
                "dev",
                "com.test/t.yaml",
                yamlc,
                stop.clone(),
                None,
                1,
                Some("fc"),
                None,
                vec![],
            )
            .await
            .unwrap();
        assert!(!logs.iter().any(|(_, m)| m == "c1"));
        assert!(logs.iter().any(|(_, m)| m == "c2"));
    }

    /// 顶层段落归一化：单段脚本省略 steps:/func:（2026-08-27）
    #[test]
    fn normalize_top_omission() {
        let p = |yaml: &str| Runner::normalize_top(parse(yaml)).unwrap();
        // 顶层序列 → steps
        let steps = p("- log: a\n- log: b").get("steps").cloned().unwrap();
        assert_eq!(steps.as_sequence().map(|s| s.len()), Some(2));
        // 无段落键的顶层映射 → func（纯函数库简写）
        let func = p("f1:\n  cond: a.png\n  steps:\n    - log: x\nf2:\n  - log: y")
            .get("func")
            .cloned()
            .unwrap();
        let m = Runner::parse_funcs(Some(func)).unwrap();
        assert_eq!(m.get("f1").unwrap().cond, vec!["a.png"]);
        assert_eq!(m.get("f2").unwrap().body.len(), 1);
        // 显式段落键原样保留（不重复包裹）
        assert!(p("steps:\n  - log: a").get("func").is_none());
        assert!(p("func:\n  f1:\n    - log: a").get("steps").is_none());
        // 含段落键时未知顶层键报错；config 子键裸写顶层定向报错
        assert!(Runner::normalize_top(parse("foo: 1\nsteps: []"))
            .unwrap_err()
            .to_string()
            .contains("未知顶层键"));
        assert!(Runner::normalize_top(parse("threshold: 0.9"))
            .unwrap_err()
            .to_string()
            .contains("config: 段参数"));
        assert!(
            Runner::normalize_top(parse("threshold: 0.9\nf1:\n  - log: a"))
                .unwrap_err()
                .to_string()
                .contains("config: 段参数")
        );
    }
}
