//! YAML 自动化脚本引擎
//!
//! 顶层字段：steps（必需）/ action_wait（操作后默认等待，500ms）/
//!           log_level（debug|info，默认 info：info 级别不记录 debug 日志）；
//!           首行可写 `package <名字>` 指令（决定文件存放目录，非标准 YAML，解析前剥离）
//!
//! 支持动作：
//!   wait / log / key / text / tap / swipe /
//!   str_app(冷启动应用：先 force-stop 再启动，包名可省略回退设备配置) /
//!   cls_app(关闭应用：adb force-stop，不碰会话/投屏) /
//!   find(查找模板：支持多模板 `find: a.png, b.png`（逗号分隔）或列表写法，
//!        一轮按配置顺序连续匹配全部模板（各自独立截图），未命中隔 interval（默认 500ms）重开一轮；
//!        and_or=and（默认）一轮内全部找到才命中 / or 任一找到即命中（命中即停，
//!        不再匹配后续模板）；timeout 必须 > 0，默认 6000ms；
//!        click 支持 true（默认，点击模板中心；多模板时 and 点第一个、or 点命中的那个）/
//!        false（不点击）/ 模板名 / [x,y] 相对坐标；
//!        threshold；region（显式参数统一作用于全部模板；未显式时模板名可自带
//!        #后缀区域各自匹配：xx#l / xx#0_0_500_500，见 tpl_region_from_name）；
//!        then 找到后执行——列表项支持「模板名: 步骤列表」单键
//!        映射做按命中模板分支（and/or 通用：命中模板有分支走分支，取书写顺序
//!        第一个匹配的；无匹配分支走其余普通步骤兜底）；else 超时后执行) /
//!   until(一直等到模板出现：参数与 find 完全一致，但 and_or 默认 or、
//!        timeout 默认 30 分钟（1800000ms，显式 0 = 永不超时），超时后执行 else) /
//!   loop / goto / label / call
//!
//! 每个操作（除 wait 动作本身）可用 wait 参数指定操作后的等待毫秒数，
//! 未指定时取脚本顶层 action_wait（如 `action_wait: 500`），脚本也未定义时默认 500ms；
//! str_app 例外：应用启动要 1~3s，未显式指定时默认等 3000ms
//!
//! 找图：截图（帧缓存优先）→ 模板匹配
//! region 支持 a/u/d/l/r/ul/ur/dl/dr 半区/四分之一区
//!
//! 可视化事件：tap/swipe/匹配命中时经 control DataChannel 推送给浏览器投屏页面
//! （emit → ViewerMap 查当前 viewer；无 viewer 时静默丢弃）

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
use crate::store::Db;
use crate::webrtc::ViewerMap;

/// 脚本未定义顶层 action_wait 时，操作后的默认等待毫秒数
const DEFAULT_ACTION_WAIT: u64 = 500;

/// until 未显式指定 timeout 时的默认超时毫秒数（30 分钟；显式 timeout: 0 = 永不超时）
const UNTIL_DEFAULT_TIMEOUT_MS: u64 = 1_800_000;

/// find/until 多模板组合逻辑：And=一轮内全部命中，Or=任一命中（命中即停）
#[derive(Clone, Copy, PartialEq, Eq)]
enum AndOr {
    And,
    Or,
}

/// 脚本运行可视化事件（服务端 → 浏览器，经 control DataChannel，JSON 格式 {"type":"se","ev":...}）
/// 注意 rename_all="snake_case"：内部标签默认用变体名原样（"Tap"），
/// 前端按小写 "tap"/"swipe"/"hit" 匹配（曾因大小写不匹配事件全部被忽略）
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum ScriptEvent {
    /// 引擎点击（设备像素坐标）
    Tap { x: u32, y: u32 },
    /// 引擎滑动（设备像素坐标）
    Swipe { x1: u32, y1: u32, x2: u32, y2: u32 },
    /// 模板匹配命中（设备像素坐标 + 置信度）
    Hit { tpl: String, x: u32, y: u32, w: u32, h: u32, score: f32 },
}

/// 运行器
pub struct Runner {
    pub db: Db,
    pub devices: Arc<DeviceManager>,
    /// 每设备活跃 viewer 注册表：脚本 tap/swipe/命中可视化事件推送用
    pub viewers: ViewerMap,
    /// 脚本文件存储（data/scripts/<package>/）：call 子脚本解析用
    pub scripts: Arc<ScriptStore>,
}

/// 脚本运行上下文
pub struct Ctx {
    pub device_id: String,
    pub script_id: String,
    pub label_index: std::collections::HashMap<String, usize>,
    pub log: Vec<(String, String)>, // (level, msg)
    pub stop: Arc<std::sync::atomic::AtomicBool>,
    /// 脚本顶层 action_wait：步骤未显式写 wait 时操作后的默认等待毫秒数
    pub action_wait: u64,
    /// 脚本顶层 log_level=debug 时记录 debug 日志（默认 info：debug 日志不记录）
    pub log_debug: bool,
    pub log_cb: Option<Arc<dyn Fn(String, String) + Send + Sync>>,
}

impl Ctx {
    /// 记录日志：实时回调（如有）并同时收集到 ctx.log；
    /// log_level=info 时丢弃 debug 日志（不回调、不收集）
    fn log(&mut self, level: &str, msg: String) {
        if level == "debug" && !self.log_debug {
            return;
        }
        if let Some(cb) = &self.log_cb {
            cb(level.to_string(), msg.clone());
        }
        self.log.push((level.to_string(), msg));
    }
}

impl Runner {
    pub fn new(db: Db, devices: Arc<DeviceManager>, viewers: ViewerMap, scripts: Arc<ScriptStore>) -> Self {
        Self { db, devices, viewers, scripts }
    }

    /// 推送脚本可视化事件给该设备当前的 viewer（无 viewer / 通道未开 / 发送失败均静默忽略）
    async fn emit(&self, device_id: &str, ev: ScriptEvent) {
        let dc = {
            let map = self.viewers.lock().unwrap();
            map.get(device_id)
                .and_then(|h| h.control_dc.lock().clone())
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
    pub async fn run(
        &self,
        device_id: &str,
        script_id: &str,
        content: &str,
        stop: Arc<std::sync::atomic::AtomicBool>,
        log_cb: Option<Arc<dyn Fn(String, String) + Send + Sync>>,
        start_step: usize,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let doc: Value = serde_yaml::from_str(content)?;
        let steps = doc.get("steps").and_then(|v| v.as_sequence()).cloned().ok_or_else(|| anyhow::anyhow!("missing steps"))?;
        // 脚本顶层 action_wait：步骤未显式写 wait 时的操作后默认等待
        let action_wait = doc.get("action_wait").and_then(|v| v.as_u64()).unwrap_or(DEFAULT_ACTION_WAIT);
        // 脚本顶层 log_level：debug 记录全部日志，info（默认）不记录 debug 日志
        let log_debug = doc.get("log_level").and_then(|v| v.as_str()).map(|s| s.eq_ignore_ascii_case("debug")).unwrap_or(false);

        let mut ctx = Ctx {
            device_id: device_id.to_string(),
            script_id: script_id.to_string(),
            label_index: std::collections::HashMap::new(),
            log: Vec::new(),
            stop,
            action_wait,
            log_debug,
            log_cb,
        };

        // 预扫描 label
        for (i, step) in steps.iter().enumerate() {
            if let Some(lbl) = step.get("label").and_then(|v| v.as_str()) {
                ctx.label_index.insert(lbl.to_string(), i);
            }
        }

        let mut i = if start_step > 0 && start_step < steps.len() { start_step } else { 0 };
        let mut guard_count = 0usize;
        while i < steps.len() {
            if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) {
                ctx.log("warn", "脚本被停止".to_string());
                break;
            }
            guard_count += 1;
            if guard_count > 100_000 {
                anyhow::bail!("脚本执行次数超限，疑似死循环");
            }
            let step = &steps[i];
            self.exec_step(&mut ctx, step).await?;
            // goto 通过 label_index 处理
            if let Some(target) = step.get("goto").and_then(|v| v.as_str()) {
                match ctx.label_index.get(target) {
                    Some(&idx) => {
                        i = idx;
                        continue;
                    }
                    None => anyhow::bail!("label not found: {}", target),
                }
            }
            i += 1;
        }

        Ok(ctx.log)
    }

    #[async_recursion]
    async fn exec_step(&self, ctx: &mut Ctx, step: &Value) -> anyhow::Result<()> {
        // 无参动作简写：`- str_app` / `- cls_app`（纯标量步骤）等价 `- str_app:`
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
        // label 不执行
        if step.get("label").is_some() {
            return Ok(());
        }
        // 动作键（除 wait 外）：用于区分 `wait` 动作与操作级 `wait` 参数
        const ACTION_KEYS: [&str; 12] = [
            "log", "key", "text", "tap", "swipe", "find", "until", "loop", "call", "goto", "str_app", "cls_app",
        ];
        let has_action = ACTION_KEYS.iter().any(|k| step.get(*k).is_some());
        if step.get("wait").is_some() && !has_action {
            let v = step.get("wait").unwrap();
            let (min, max) = match v {
                Value::Sequence(seq) => (
                    seq.get(0).and_then(|x| x.as_u64()).unwrap_or(0),
                    seq.get(1).and_then(|x| x.as_u64()).unwrap_or(0),
                ),
                Value::Mapping(_) => (
                    v.get("min").and_then(|x| x.as_u64()).unwrap_or(0),
                    v.get("max").and_then(|x| x.as_u64()).unwrap_or(0),
                ),
                _ => (v.as_u64().unwrap_or(0), v.as_u64().unwrap_or(0)),
            };
            let ms = if max > min { min + rand::random::<u64>() % (max - min) } else { min };
            ctx.log("debug", format!("等待 {}ms", ms));
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        if let Some(v) = step.get("log") {
            let msg = v.as_str().unwrap_or("");
            ctx.log("info", msg.to_string());
        }
        if let Some(v) = step.get("key") {
            let key = v.as_str().unwrap_or("");
            let code = key_code(key);
            ctx.log("debug", format!("按键 {}", key));
            if let Some(s) = self.devices.session(&ctx.device_id) {
                s.press_key(code).await?;
            } else {
                anyhow::bail!("设备未连接");
            }
        }
        if let Some(v) = step.get("text") {
            let text = v.as_str().unwrap_or("");
            ctx.log("debug", format!("输入文本 {}", text));
            if let Some(s) = self.devices.session(&ctx.device_id) {
                s.inject_text(text).await?;
            } else {
                anyhow::bail!("设备未连接");
            }
        }
        if let Some(v) = step.get("tap") {
            let (rx, ry) = self.resolve_relative_point(ctx, v)?;
            let s = self.devices.session(&ctx.device_id).ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
            let (w, h) = s.video_size();
            let x = (rx * w as f32).round().clamp(0.0, w as f32) as u32;
            let y = (ry * h as f32).round().clamp(0.0, h as f32) as u32;
            ctx.log("debug", format!("点击坐标 ({:.3}, {:.3}) → 像素 ({}, {})", rx, ry, x, y));
            self.emit(&ctx.device_id, ScriptEvent::Tap { x, y }).await;
            s.tap(x as f32, y as f32).await?;
        }
        if let Some(v) = step.get("swipe") {
            // 新格式 fm/to；兼容旧写法 from/to
            let from = v.get("fm").or_else(|| v.get("from")).cloned().unwrap_or(Value::Null);
            let to = v.get("to").cloned().unwrap_or(Value::Null);
            let (rx1, ry1) = self.relative_pair(&from)?;
            let (rx2, ry2) = self.relative_pair(&to)?;
            let dur = v.get("time").and_then(|x| x.as_u64()).unwrap_or(500);
            let s = self.devices.session(&ctx.device_id).ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
            let (w, h) = s.video_size();
            let x1 = (rx1 * w as f32).round().clamp(0.0, w as f32) as u32;
            let y1 = (ry1 * h as f32).round().clamp(0.0, h as f32) as u32;
            let x2 = (rx2 * w as f32).round().clamp(0.0, w as f32) as u32;
            let y2 = (ry2 * h as f32).round().clamp(0.0, h as f32) as u32;
            ctx.log("debug", format!("滑动 ({:.3},{:.3})→({:.3},{:.3}) {}ms", rx1, ry1, rx2, ry2, dur));
            self.emit(&ctx.device_id, ScriptEvent::Swipe { x1, y1, x2, y2 }).await;
            s.swipe(x1 as f32, y1 as f32, x2 as f32, y2 as f32, dur).await?;
        }
        if step.get("find").is_some() || step.get("until").is_some() {
            // find 与 until 共用实现：until 等价于 timeout 为 0 的 find（一直找到出现为止）
            let key = if step.get("until").is_some() { "until" } else { "find" };
            self.exec_find(ctx, step, key).await?;
        }
        if let Some(v) = step.get("loop") {
            let times = v.get("times").and_then(|x| x.as_u64()).unwrap_or(1);
            let sub_steps = v.get("steps").and_then(|x| x.as_sequence()).cloned().unwrap_or_default();
            for n in 0..times {
                if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                ctx.log("debug", format!("循环第 {}/{} 次", n + 1, times));
                for sub in &sub_steps {
                    self.exec_step(ctx, sub).await?;
                }
            }
        }
        if let Some(v) = step.get("call") {
            let script_name = v.as_str().unwrap_or("");
            // 子脚本按名解析：优先调用者同分区，其次跨分区（缺扩展名自动补全）
            let caller_pkg = ctx.script_id.split('/').next().unwrap_or_default();
            match self.scripts.resolve_call(caller_pkg, script_name)? {
                Some(s) => {
                    ctx.log("debug", format!("调用子脚本 {}", script_name));
                    let sub_log = self.run(&ctx.device_id, &s.id, &s.content, ctx.stop.clone(), ctx.log_cb.clone(), 0).await?;
                    ctx.log.extend(sub_log);
                }
                None => anyhow::bail!("子脚本不存在: {}", script_name),
            }
        }
        if let Some(v) = step.get("str_app") {
            let pkg = self.resolve_app_pkg(ctx, v)?;
            // "+" 前缀：先 force-stop 再启动（scrcpy 定制控制消息，
            // 虚拟屏模式下自动启动到虚拟屏，不要用 adb am start——会落到主屏）
            let s = self.devices.session(&ctx.device_id).ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
            ctx.log("info", format!("冷启动应用 {}", pkg));
            s.start_app(&format!("+{}", pkg)).await?;
        }
        if let Some(v) = step.get("cls_app") {
            let pkg = self.resolve_app_pkg(ctx, v)?;
            let serial = self
                .devices
                .snapshot(&ctx.device_id)
                .map(|(d, _, _)| d.addr)
                .filter(|a| !a.is_empty())
                .ok_or_else(|| anyhow::anyhow!("设备不存在或未解析出 adb serial"))?;
            ctx.log("info", format!("关闭应用 {}", pkg));
            // adb force-stop：不碰 scrcpy 会话（屏幕/投屏不中断）；幂等，应用未运行也无害。
            // 虚拟屏上应用被杀后画面变桌面或黑屏，流不断，属预期
            self.devices.adb.shell(&serial, &format!("am force-stop {}", pkg), Duration::from_secs(8)).await?;
        }
        // 操作后统一等待：除 wait 动作本身外，每个操作可用 wait 参数指定操作后的等待毫秒数，
        // 未指定时取脚本顶层 action_wait（脚本未定义时默认 500ms）；
        // str_app 例外：应用启动要 1~3s，未显式指定时默认等 3000ms
        if has_action {
            let default_wait = if step.get("str_app").is_some() { 3000 } else { ctx.action_wait };
            let wait_ms = step.get("wait").and_then(|x| x.as_u64()).unwrap_or(default_wait);
            if wait_ms > 0 {
                ctx.log("debug", format!("操作后等待 {}ms", wait_ms));
                tokio::time::sleep(Duration::from_millis(wait_ms)).await;
            }
        }
        Ok(())
    }

    /// str_app/cls_app 的应用包名解析：显式值优先，回退设备配置 pkg；
    /// 校验仅允许 [A-Za-z0-9_.]（cls_app 要拼进 adb shell 命令，防注入）
    fn resolve_app_pkg(&self, ctx: &Ctx, v: &Value) -> anyhow::Result<String> {
        let pkg = v
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| self.devices.snapshot(&ctx.device_id).and_then(|(d, _, _)| d.pkg))
            .unwrap_or_default();
        if pkg.is_empty() {
            anyhow::bail!("缺少应用包名（步骤未指定且设备未配置 pkg）");
        }
        if !pkg.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') {
            anyhow::bail!("应用包名字符非法: {}", pkg);
        }
        Ok(pkg)
    }

    /// find：循环查找模板（检测间隔 interval 默认 500ms；timeout 必须 > 0，默认 6000ms），
    /// 找到后按 click 参数处理并执行 then（支持按命中模板分支，见 parse_then），超时未找到执行 else；
    /// until：参数与 find 一致，但 and_or 默认 or、timeout 默认 30 分钟
    /// （显式 timeout: 0 = 永不超时，此时 else 不会执行）。
    /// 多模板（2026-08-24）：模板列表支持逗号分隔字符串或 YAML 列表；
    /// 一轮 = 按配置顺序连续匹配全部模板（每个模板独立取最新截图、模板间不等待），
    /// 本轮未命中隔 interval 重开一轮（又从第一个模板开始）；
    /// and_or=and 一轮内全部找到才命中（任一未命中本轮即失败），
    /// or 逐个匹配、任一找到即命中（后续模板不再匹配）；
    /// 单模板写法与旧版完全兼容（and_or 退化为普通命中）
    #[async_recursion]
    async fn exec_find(&self, ctx: &mut Ctx, step: &Value, key: &str) -> anyhow::Result<()> {
        let is_until = key == "until";
        let templates = self.template_names(step, key)?;
        let and_or = self.parse_and_or(step, if is_until { "or" } else { "and" })?;
        let interval_ms = self.opt_u64(step, key, "interval").unwrap_or(500);
        let timeout_ms = if is_until {
            self.opt_u64(step, key, "timeout").unwrap_or(UNTIL_DEFAULT_TIMEOUT_MS)
        } else {
            let t = self.opt_u64(step, key, "timeout").unwrap_or(6000);
            if t == 0 {
                anyhow::bail!("find 的 timeout 必须大于 0（一直找请用 until）");
            }
            t
        };
        let threshold = self.opt_f64(step, key, "threshold")
            .map(|x| x as f32)
            .unwrap_or(self.devices.cfg.default_threshold);
        let timeout_desc = if timeout_ms == 0 {
            "直到出现（不超时）".to_string()
        } else {
            format!("{}ms", timeout_ms)
        };
        let tpl_desc = templates.join("、");
        if templates.len() > 1 {
            let mode = if and_or == AndOr::And { "and 全部命中" } else { "or 任一命中" };
            ctx.log("info", format!("查找模板 {}（{}），超时 {}，检测间隔 {}ms", tpl_desc, mode, timeout_desc, interval_ms));
        } else {
            ctx.log("info", format!("查找模板 {}，超时 {}，检测间隔 {}ms", tpl_desc, timeout_desc, interval_ms));
        }
        let (then_branches, then_steps) = match self.opt_value(step, key, "then") {
            Some(v) => Self::parse_then(v, key, &templates)?,
            None => (Vec::new(), Vec::new()),
        };
        let else_steps = self.opt_value(step, key, "else").and_then(|v| v.as_sequence()).cloned().unwrap_or_default();
        let start = std::time::Instant::now();
        loop {
            if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            if timeout_ms > 0 && start.elapsed().as_millis() as u64 > timeout_ms {
                ctx.log("warn", format!("查找模板 {} 超时", tpl_desc));
                for sub in &else_steps {
                    self.exec_step(ctx, sub).await?;
                }
                break;
            }
            // 一轮：按配置顺序连续逐个匹配（独立截图、模板间不等待）；
            // and 任一未命中即本轮失败，or 命中即本轮成功（后续模板不再匹配）
            let mut hits: Vec<(String, matcher::MatchResult)> = Vec::new();
            let mut stopped = false;
            for tpl in &templates {
                if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) {
                    stopped = true;
                    break;
                }
                match self.match_one(ctx, step, tpl, threshold).await? {
                    Some(m) => {
                        hits.push((tpl.clone(), m));
                        if and_or == AndOr::Or {
                            break;
                        }
                    }
                    None => {
                        if and_or == AndOr::And {
                            break;
                        }
                    }
                }
            }
            if stopped {
                break;
            }
            let hit_round = match and_or {
                AndOr::Or => !hits.is_empty(),
                AndOr::And => hits.len() == templates.len(),
            };
            if hit_round {
                for (name, m) in &hits {
                    ctx.log("success", format!("模板 {} 已找到 @ ({}, {})", name, m.x, m.y));
                    self.emit(&ctx.device_id, ScriptEvent::Hit {
                        tpl: name.clone(),
                        x: m.x, y: m.y, w: m.width, h: m.height, score: m.score,
                    }).await;
                }
                // click 作用目标：and = 第一个模板，or = 命中的模板（命中即停只有一个）
                if self.exec_find_click(ctx, step, threshold, &hits[0].1).await? {
                    // click 成功（或未配置 click）→ 执行命中分支并结束：
                    // then 分支 = 书写顺序第一个模板在命中列表里的（or=命中的恰为一个
                    // 模板即命中谁走谁，and=全命中取先写的），没有则执行兜底步骤
                    let hit_names: Vec<&str> = hits.iter().map(|(n, _)| n.as_str()).collect();
                    match then_branches.iter().find(|(n, _)| hit_names.contains(&n.as_str())) {
                        Some((name, steps)) => {
                            ctx.log("info", format!("命中模板 {}，执行 then 分支", name));
                            for sub in steps {
                                self.exec_step(ctx, sub).await?;
                            }
                        }
                        None => {
                            for sub in &then_steps {
                                self.exec_step(ctx, sub).await?;
                            }
                        }
                    }
                    break;
                }
                // click 目标（如模板区域内的按钮）尚未找到 → 继续循环
                ctx.log("debug", "click 目标未就绪，继续查找".to_string());
            }
            // 本轮未命中（或 click 目标未就绪）→ 间隔 interval 后从第一个模板重开一轮
            tokio::time::sleep(Duration::from_millis(interval_ms)).await;
        }
        Ok(())
    }

    /// 处理 find 的 click 参数，返回是否成功点击：
    ///   true/未配置（默认）→ 点击模板中心
    ///   false           → 不点击（视为成功，直接执行 then）
    ///   模板名           → 在模板区域内查找该模板，找到后点击其中心（未找到返回 false，继续循环）
    ///   [x, y]          → 点击模板区域内的相对坐标（0~1，如 [0.5, 0.5] = 中心）
    /// 多模板命中时 m 为 click 的作用目标：and = 第一个模板，or = 命中的模板
    async fn exec_find_click(&self, ctx: &mut Ctx, step: &Value, threshold: f32, m: &matcher::MatchResult) -> anyhow::Result<bool> {
        let default_click = Value::Bool(true);
        let click = step.get("click").unwrap_or(&default_click);
        let s = self.devices.session(&ctx.device_id).ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
        let box_region = [m.x, m.y, m.width, m.height];
        if let Some(b) = click.as_bool() {
            if !b {
                return Ok(true);
            }
            let (cx, cy) = (m.x + m.width / 2, m.y + m.height / 2);
            ctx.log("success", format!("点击模板中心 @ ({}, {})", cx, cy));
            self.emit(&ctx.device_id, ScriptEvent::Tap { x: cx, y: cy }).await;
            s.tap(cx as f32, cy as f32).await?;
            return Ok(true);
        }
        if let Some(name) = click.as_str() {
            let screen = self.devices.screenshot(&ctx.device_id).await
                .map_err(|e| anyhow::anyhow!("截图失败: {}", e))?;
            match self.match_on_screen(ctx, name, threshold, Some(box_region), screen).await? {
                Some(inner) => {
                    let (cx, cy) = (inner.x + inner.width / 2, inner.y + inner.height / 2);
                    ctx.log("success", format!("模板区域内找到 {}，点击 @ ({}, {})", name, cx, cy));
                    self.emit(&ctx.device_id, ScriptEvent::Hit {
                        tpl: name.to_string(),
                        x: inner.x, y: inner.y, w: inner.width, h: inner.height, score: inner.score,
                    }).await;
                    self.emit(&ctx.device_id, ScriptEvent::Tap { x: cx, y: cy }).await;
                    s.tap(cx as f32, cy as f32).await?;
                    Ok(true)
                }
                None => {
                    ctx.log("debug", format!("模板区域内未找到 {}", name));
                    Ok(false)
                }
            }
        } else if let Some(seq) = click.as_sequence() {
            if seq.len() != 2 {
                anyhow::bail!("click 数组需要 [x, y] 2 个相对坐标");
            }
            let rx = seq[0].as_f64().ok_or_else(|| anyhow::anyhow!("click 坐标必须是数字"))?;
            let ry = seq[1].as_f64().ok_or_else(|| anyhow::anyhow!("click 坐标必须是数字"))?;
            if !(0.0..=1.0).contains(&rx) || !(0.0..=1.0).contains(&ry) {
                anyhow::bail!("click 相对坐标必须在 0~1 之间");
            }
            let cx = m.x + (rx * m.width as f64).round() as u32;
            let cy = m.y + (ry * m.height as f64).round() as u32;
            ctx.log("success", format!("点击模板内相对坐标 ({:.3}, {:.3}) @ ({}, {})", rx, ry, cx, cy));
            self.emit(&ctx.device_id, ScriptEvent::Tap { x: cx, y: cy }).await;
            s.tap(cx as f32, cy as f32).await?;
            return Ok(true);
        } else {
            anyhow::bail!("click 只支持 true/false、模板名或 [x, y] 相对坐标");
        }
    }

    /// 匹配单个模板一次（独立取最新截图，不重试）：解析 region 后在全屏/区域内匹配。
    /// region 优先级：显式 region 参数（全部模板统一） > 模板名 #后缀（各自独立，
    /// 见 tpl_region_from_name） > 全屏（a）——多模板区域不同时靠模板名后缀区分
    async fn match_one(&self, ctx: &Ctx, step: &Value, template: &str, threshold: f32) -> anyhow::Result<Option<matcher::MatchResult>> {
        let screen = self.devices.screenshot(&ctx.device_id).await
            .map_err(|e| anyhow::anyhow!("截图失败: {}", e))?;
        let (w, h) = self.screen_size(ctx, &screen);
        if w == 0 || h == 0 {
            anyhow::bail!("无法获取屏幕尺寸");
        }
        let region = match step.get("region") {
            Some(rv) => Self::parse_region(rv, w, h)?,
            None => Self::tpl_region_from_name(template, w, h)?,
        };
        self.match_on_screen(ctx, &template, threshold, region, screen).await
    }

    /// 在给定截图上匹配模板（region 为搜索区域，None=全屏）
    async fn match_on_screen(&self, ctx: &Ctx, template: &str, threshold: f32, region: Option<[u32; 4]>, screen: Vec<u8>) -> anyhow::Result<Option<matcher::MatchResult>> {
        // 模板按脚本所在应用分区解析：data/<pkg>/tmpl/（script_id 首段 = 分区）
        let pkg = ctx.script_id.split('/').next().unwrap_or_default();
        let tpl_dir = self.devices.cfg.data_dir.join(pkg).join("tmpl");
        // 目录不存在时先创建，避免 std::fs::read 报“系统找不到指定的路径”
        let _ = std::fs::create_dir_all(&tpl_dir);
        let tpl_path = Self::resolve_template_file(&tpl_dir, template)?;
        let tpl_bytes = std::fs::read(&tpl_path)
            .map_err(|e| anyhow::anyhow!("读取模板 {} 失败: {} (path={})", template, e, tpl_path.display()))?;
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
    fn resolve_template_file(tpl_dir: &std::path::Path, template: &str) -> anyhow::Result<std::path::PathBuf> {
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
            _ => anyhow::bail!("模板 {} 匹配到多个候选：{}，请用完整文件名指定", template, candidates.join("、")),
        }
    }

    /// 从步骤中取模板名列表：支持单模板字符串 `find: a.png`、逗号分隔多模板
    /// `find: a.png, b.png` 与 YAML 列表 `find: [a.png, b.png]` 三种写法
    fn template_names(&self, step: &Value, key: &str) -> anyhow::Result<Vec<String>> {
        let v = step.get(key).ok_or_else(|| anyhow::anyhow!("缺少 {}", key))?;
        let names: Vec<String> = match v {
            Value::String(s) => s.split(',').map(|p| p.trim().to_string()).collect(),
            Value::Sequence(seq) => seq
                .iter()
                .map(|item| item.as_str().map(|s| s.trim().to_string()))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| anyhow::anyhow!("{} 列表项必须是字符串模板名", key))?,
            _ => anyhow::bail!("{} 只支持模板名字符串（多模板逗号分隔）或列表，如 `{}: a.png, b.png`", key, key),
        };
        if names.is_empty() || names.iter().any(|n| n.is_empty()) {
            anyhow::bail!("{} 模板名不能为空", key);
        }
        Ok(names)
    }

    /// 解析 and_or 参数：and=一轮内全部命中 / or=任一命中（命中即停）；
    /// def 为默认值（find=and，until=or）
    fn parse_and_or(&self, step: &Value, def: &str) -> anyhow::Result<AndOr> {
        let s = step
            .get("and_or")
            .and_then(|v| v.as_str())
            .unwrap_or(def)
            .trim()
            .to_ascii_lowercase();
        match s.as_str() {
            "and" => Ok(AndOr::And),
            "or" => Ok(AndOr::Or),
            other => anyhow::bail!("and_or 只支持 and / or，收到: {}", other),
        }
    }

    /// 解析 find/until 命中后的 then 步骤（按模板分支增强版，and/or 模式通用）：
    ///   then:
    ///     - test1.png:        # 单键映射且键在模板列表中 = 模板专属分支
    ///         - log: "命中 test1"
    ///     - test2.png:
    ///         - log: "命中 test2"
    ///     - log: "兜底"       # 其余普通步骤 = 兜底，命中的模板没有专属分支时执行
    /// 命中后取**书写顺序第一个**模板在命中列表里的分支执行（or=命中的恰为一个模板，
    /// and=全命中取先写的），没有匹配分支则执行兜底步骤。
    /// 纯普通步骤列表（旧写法）不产生分支，行为与旧版完全一致；
    /// 键为 wait/log 等动作的普通步骤不会撞模板名（分支键必须在模板列表中）
    fn parse_then(v: &Value, key: &str, templates: &[String]) -> anyhow::Result<(Vec<(String, Vec<Value>)>, Vec<Value>)> {
        // 单键 + 列表值 + 键非动作名 = 疑似模板名写错的分支（如扩展名拼错），
        // 不报错会被当普通步骤静默跳过，分支永远不生效
        const STEP_KEYS: [&str; 14] = [
            "wait", "log", "key", "text", "tap", "swipe", "find", "until", "loop", "call", "goto", "label", "str_app", "cls_app",
        ];
        let seq = v.as_sequence().cloned().unwrap_or_default();
        let mut branches = Vec::new();
        let mut fallback = Vec::new();
        for item in seq {
            if let Some(map) = item.as_mapping() {
                if map.len() == 1 {
                    let (k, val) = map.iter().next().unwrap();
                    if let Some(name) = k.as_str() {
                        if templates.iter().any(|t| t == name) {
                            let steps = val
                                .as_sequence()
                                .cloned()
                                .ok_or_else(|| anyhow::anyhow!("then 的 {} 分支步骤必须是列表", name))?;
                            branches.push((name.to_string(), steps));
                            continue;
                        }
                        if val.is_sequence() && !STEP_KEYS.contains(&name) {
                            anyhow::bail!("then 的 {} 不在 {} 的模板列表中（分支写法：- 模板名: 换行缩进的步骤列表）", name, key);
                        }
                    }
                }
            }
            fallback.push(item);
        }
        Ok((branches, fallback))
    }

    /// 取步骤参数：新语法下参数与动作键同级
    fn opt_value<'a>(&self, step: &'a Value, _key: &str, opt: &str) -> Option<&'a Value> {
        step.get(opt)
    }

    fn opt_f64(&self, step: &Value, key: &str, opt: &str) -> Option<f64> {
        self.opt_value(step, key, opt).and_then(|x| x.as_f64())
    }

    fn opt_u64(&self, step: &Value, key: &str, opt: &str) -> Option<u64> {
        self.opt_value(step, key, opt).and_then(|x| x.as_u64())
    }

    /// 从模板名解析自带区域后缀（未显式写 region 参数时使用，与前端
    /// parseTplRegion / parseTplRegionCode 同一套格式）：
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
        // 半区码：与 region 参数的字符串写法完全一致（a → 全屏 None）
        if let Ok(r) = Self::parse_region(&Value::String(suffix.clone()), w, h) {
            return Ok(r);
        }
        // 数字坐标：4 段 1~3 位整数 ×1000 → 相对坐标，复用 region 数组写法的校验与换算；
        // 校验不过（如 x2 <= x1）视为无区域 → 全屏，不报错
        let nums: Option<Vec<f64>> = suffix
            .split('_')
            .map(|p| p.parse::<u32>().ok().filter(|n| *n <= 999).map(|n| n as f64 / 1000.0))
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
            let x1 = seq[0].as_f64().ok_or_else(|| anyhow::anyhow!("region 坐标必须是数字"))?;
            let y1 = seq[1].as_f64().ok_or_else(|| anyhow::anyhow!("region 坐标必须是数字"))?;
            let x2 = seq[2].as_f64().ok_or_else(|| anyhow::anyhow!("region 坐标必须是数字"))?;
            let y2 = seq[3].as_f64().ok_or_else(|| anyhow::anyhow!("region 坐标必须是数字"))?;
            if !(0.0..=1.0).contains(&x1) || !(0.0..=1.0).contains(&y1) || !(0.0..=1.0).contains(&x2) || !(0.0..=1.0).contains(&y2) {
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
            let (x1, y1) = Self::parse_rel_coord(map.get("fm").ok_or_else(|| anyhow::anyhow!("region 缺少 fm"))?)?;
            let (x2, y2) = Self::parse_rel_coord(map.get("to").ok_or_else(|| anyhow::anyhow!("region 缺少 to"))?)?;
            if x2 <= x1 || y2 <= y1 {
                anyhow::bail!("region 需要 to > fm");
            }
            let x = (x1 * w as f64).round() as u32;
            let y = (y1 * h as f64).round() as u32;
            let rw = (((x2 - x1) * w as f64).round() as u32).max(1);
            let rh = (((y2 - y1) * h as f64).round() as u32).max(1);
            return Ok(Some([x, y, rw, rh]));
        }
        anyhow::bail!("region 只支持 a/u/d/l/r/ul/ur/dl/dr / [x1, y1, x2, y2] / {{fm: [x,y], to: [x,y]}}")
    }

    /// 解析 region 内的相对坐标点 [x, y]（0~1）
    fn parse_rel_coord(v: &Value) -> anyhow::Result<(f64, f64)> {
        let seq = v.as_sequence().ok_or_else(|| anyhow::anyhow!("region fm/to 需要 [x, y] 数组"))?;
        if seq.len() != 2 {
            anyhow::bail!("region fm/to 需要 [x, y] 2 个相对坐标");
        }
        let x = seq[0].as_f64().ok_or_else(|| anyhow::anyhow!("region 坐标必须是数字"))?;
        let y = seq[1].as_f64().ok_or_else(|| anyhow::anyhow!("region 坐标必须是数字"))?;
        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
            anyhow::bail!("region 相对坐标必须在 0~1 之间");
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
            let x = seq.get(0).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
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

    /// 解析 tap 相对坐标（百分比 0~1）
    fn resolve_relative_point(&self, _ctx: &Ctx, v: &Value) -> anyhow::Result<(f32, f32)> {
        self.relative_pair(v)
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

    fn tpls(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// then 按模板分支：单键映射且键在模板列表 = 分支，其余 = 兜底步骤；
    /// 旧写法（纯普通步骤）不产生分支，完全兼容
    #[test]
    fn then_branches_parse() {
        let (branches, fallback) = Runner::parse_then(
            &parse("- test1.png:\n    - log: \"1\"\n- test2.png:\n    - log: \"2\"\n- log: \"3\"\n"),
            "find",
            &tpls(&["test1.png", "test2.png"]),
        )
        .unwrap();
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].0, "test1.png");
        assert_eq!(branches[0].1.len(), 1);
        assert_eq!(branches[1].0, "test2.png");
        assert_eq!(fallback.len(), 1);
        assert_eq!(fallback[0].get("log").and_then(|v| v.as_str()), Some("3"));

        // 旧写法：无分支，全部兜底（wait 等单键+列表值的普通步骤不受影响）
        let (b2, f2) = Runner::parse_then(
            &parse("- wait: [500, 1000]\n- log: y\n"),
            "find",
            &tpls(&["test1.png"]),
        )
        .unwrap();
        assert!(b2.is_empty());
        assert_eq!(f2.len(), 2);
    }

    /// 分支模板名不在 find 模板列表（如扩展名拼错）→ 报错，
    /// 避免被当普通步骤静默跳过、分支永远不生效
    #[test]
    fn then_branches_validation() {
        // 键不在模板列表 + 值是列表 → 报错
        assert!(Runner::parse_then(
            &parse("- test1.bmp:\n    - log: \"1\"\n"),
            "find",
            &tpls(&["test1.png"]),
        )
        .is_err());
        // 分支步骤不是列表 → 报错
        assert!(Runner::parse_then(
            &parse("- test1.png: log\n"),
            "find",
            &tpls(&["test1.png"]),
        )
        .is_err());
    }

    /// 命中后分支选择（exec_find 同款规则）：书写顺序第一个模板在命中列表里的分支；
    /// or=命中的恰为一个模板（命中谁走谁），and=全命中取先写的；无匹配走兜底
    #[test]
    fn then_branches_selection() {
        let (branches, fallback) = Runner::parse_then(
            &parse("- test1.png:\n    - log: \"1\"\n- test2.png:\n    - log: \"2\"\n- log: \"3\"\n"),
            "find",
            &tpls(&["test1.png", "test2.png"]),
        )
        .unwrap();
        let pick = |hits: &[&str]| {
            branches
                .iter()
                .find(|(n, _)| hits.contains(&n.as_str()))
                .map(|(n, _)| n.as_str())
                .unwrap_or("fallback")
        };
        // or：命中的恰为 test2 → test2 分支；命中 test1 → test1 分支
        assert_eq!(pick(&["test2.png"]), "test2.png");
        assert_eq!(pick(&["test1.png"]), "test1.png");
        // and：全命中 → 书写顺序第一个（test1）
        assert_eq!(pick(&["test1.png", "test2.png"]), "test1.png");
        // 命中的模板没有分支（单模板 find 复用分支时）→ 兜底
        assert_eq!(pick(&["test3.png"]), "fallback");
        assert_eq!(fallback.len(), 1);
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
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cleanup = || { let _ = std::fs::remove_dir_all(&dir); };
        std::fs::write(dir.join("login#907_160_973_717.png"), b"x").unwrap();
        std::fs::write(dir.join("shop.png"), b"x").unwrap();
        // 短名 → 唯一后缀文件
        let p = Runner::resolve_template_file(&dir, "login.png").unwrap();
        assert!(p.file_name().unwrap().to_string_lossy().starts_with("login#"));
        // 精确名直用
        assert!(Runner::resolve_template_file(&dir, "shop.png").unwrap().file_name().unwrap() == "shop.png");
        // 不存在
        assert!(Runner::resolve_template_file(&dir, "nope.png").is_err());
        // 同基名多后缀 → 报错消歧；有精确同名文件时精确优先不歧义
        std::fs::write(dir.join("hp#l.png"), b"x").unwrap();
        std::fs::write(dir.join("hp#r.png"), b"x").unwrap();
        assert!(Runner::resolve_template_file(&dir, "hp.png").is_err());
        std::fs::write(dir.join("hp.png"), b"x").unwrap();
        assert!(Runner::resolve_template_file(&dir, "hp.png").unwrap().file_name().unwrap() == "hp.png");
        cleanup();
    }
}
