//! YAML 自动化脚本引擎
//!
//! 顶层字段：steps（必需）/ action_wait（操作后默认等待，500ms）/
//!           log_level（debug|info，默认 info：info 级别不记录 debug 日志）；
//!           （旧 `package <名字>` 指令已删除：引擎直接解析 YAML，残留指令行 = 解析报错）
//!
//! 支持动作：
//!   wait / log / key / text / tap / swipe /
//!   str_app(冷启动应用：先 force-stop 再启动，包名可省略回退设备配置) /
//!   cls_app(关闭应用：adb force-stop，不碰会话/投屏) /
//!   until(等模板出现并点击（2026-08-25 重构，找图统一写法）：
//!        `until: a.png` 单个主模板（字符串；多主模板/列表已删除——多目标
//!        拆成多步、挡路的模板写 before）；
//!        before 障碍模板（`before: b.png` / `b.png, c.png` / [b.png, c.png]，
//!        与主模板重复报错）每轮开头依序匹配：命中即点击关闭、未命中等
//!        img_ivl 匹配下一个——无论命中与否都不结束本轮；
//!        一轮 = before 全部 → 主模板（相邻两次匹配隔 img_ivl，默认 50ms）；
//!        主模板命中即点击模板中心并执行 then 结束步骤，全部未命中隔 interval
//!        （必须 > 0，默认 500ms）重开一轮（又从 before 开始）；timeout 超时
//!        （必须 > 0，默认 30min，支持 500 / 500ms / 2s / 30min / 1h / 1d 写法）
//!        超时执行 else；then/else 为普通步骤列表（「模板名: 步骤列表」按命中
//!        模板分支的写法已删除，写了会被静默跳过）；threshold 匹配阈值（默认
//!        config default_threshold）；region 搜索区域（统一作用于全部模板；
//!        未显式时模板名可自带 #后缀区域：xx#l / xx#0_0_500_500，
//!        见 tpl_region_from_name）；
//!        count 连击补点：总点击次数含首击、默认 1（单击），命中后按首击
//!        坐标无条件重复点击、cnt_ivl 相邻点击间隔默认 50ms（写法同 timeout）；
//!        对主模板与 before 障碍模板的点击同样生效（cnt_chk 已删除，写了报错）；
//!        find / click-check 简写及 and_or / click / cnt_chk 参数已删除，写了报错) /
//!   color(多检查点取色（2026-08-25 重写，旧单点 color: [x,y] + check: 色值 已删除）：
//!        - color: {timeout: 5min(默认，必须 > 0), interval: 500ms(默认),
//!          check: [[x, y]: ff8800, ...]}——check 为检查点列表（至少一项），
//!        每项单键映射 `[x, y]: 色值`（6 位十六进制 RRGGBB，宽容接受 "#ff8800"/
//!        [r,g,b]/0x 前缀）；超时时间内逐轮检测（每轮新截图，隔 interval），
//!        任一检查点命中即执行 then 结束，超时执行 else；容差固定 30——
//!        H.264 有损压缩帧间像素抖动，精确匹配不可用；tol/count/cnt_ivl 已删除) /
//!   exit(结束脚本运行：- exit 无参数打印"结束运行脚本"；- exit: 原因 打印
//!        "因 原因 结束运行脚本"；call 子脚本内 exit 同样结束整个任务） /
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

/// until 未显式指定 timeout 时的默认超时毫秒数（30 分钟）
const UNTIL_DEFAULT_TIMEOUT_MS: u64 = 1_800_000;

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
    /// 脚本文件存储（data/<pkg>/yaml/，按应用分区）：call 子脚本解析用
    pub scripts: Arc<ScriptStore>,
}

/// 脚本运行上下文
pub struct Ctx {
    pub device_id: String,
    pub script_id: String,
    pub label_index: std::collections::HashMap<String, usize>,
    pub log: Vec<(String, String)>, // (level, msg)
    pub stop: Arc<std::sync::atomic::AtomicBool>,
    /// exit 动作已触发（跨 call 子脚本共享）：run 主循环据此提前结束整个脚本运行
    pub exit: Arc<std::sync::atomic::AtomicBool>,
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
    /// `exit`：exit 动作共享标志（call 子脚本传父脚本的，子脚本里 exit 同样结束整个任务；None=新建）
    pub async fn run(
        &self,
        device_id: &str,
        script_id: &str,
        content: &str,
        stop: Arc<std::sync::atomic::AtomicBool>,
        log_cb: Option<Arc<dyn Fn(String, String) + Send + Sync>>,
        start_step: usize,
        exit: Option<Arc<std::sync::atomic::AtomicBool>>,
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
            exit: exit.unwrap_or_else(|| Arc::new(std::sync::atomic::AtomicBool::new(false))),
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
            if ctx.exit.load(std::sync::atomic::Ordering::SeqCst) {
                // exit 动作已在 exec_step 里打印结束日志（含 call 子脚本），这里直接结束
                break;
            }
            guard_count += 1;
            if guard_count > 100_000 {
                anyhow::bail!("脚本执行次数超限，疑似死循环");
            }
            let step = &steps[i];
            self.exec_step(&mut ctx, step).await?;
            // exit 动作已触发（含 call 子脚本）：直接结束，不再处理 goto/后续步骤
            if ctx.exit.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
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
        // exit：立即结束整个脚本运行（含 call 子脚本场景，经 ctx.exit 共享标志）。
        // `- exit` 无参数打印通用提示；`- exit: 体力不足` 按参数打印
        if step.get("exit").is_some() {
            let msg = step.get("exit").and_then(|v| v.as_str()).map(|s| s.trim()).filter(|s| !s.is_empty());
            match msg {
                Some(m) => ctx.log("info", format!("因 {} 结束运行脚本", m)),
                None => ctx.log("info", "结束运行脚本".to_string()),
            }
            ctx.exit.store(true, std::sync::atomic::Ordering::SeqCst);
            return Ok(());
        }
        // 旧写法显式报错引导迁移（2026-08-25 重构：find 与 click/check 简写已删除）
        if step.get("find").is_some() {
            anyhow::bail!("find 已删除：统一改用 until（限时查找写 until + timeout: 6s，默认 30min）");
        }
        // click 键已删除（原 click/check 简写与 until 的 click 点击参数都不再存在）：
        // until 命中恒点击模板中心，before 障碍同样点中心
        if step.get("click").is_some() && step.get("until").is_none() {
            anyhow::bail!("click 已删除：改写 `- until: 主模板` + `before: 障碍模板`（命中恒点击模板中心）");
        }
        // check 键是 color 步骤的检查点列表（- color: 后接兄弟键 check 才合法；其余场合报错）
        if step.get("check").is_some() && step.get("color").is_none() {
            anyhow::bail!("check 只能与 color 配合使用（- color:\\n  check:\\n    - [x, y]: 色值）；until 的障碍模板请写 before");
        }
        // 动作键（除 wait 外）：用于区分 `wait` 动作与操作级 `wait` 参数
        const ACTION_KEYS: [&str; 13] = [
            "log", "key", "text", "tap", "swipe", "until", "color", "loop", "call", "goto", "str_app", "cls_app", "exit",
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
        if let Some(v) = step.get("color") {
            self.exec_color(ctx, step, v).await?;
        }
        if step.get("until").is_some() {
            self.exec_until(ctx, step).await?;
        }
        if let Some(v) = step.get("loop") {
            let times = v.get("times").and_then(|x| x.as_u64()).unwrap_or(1);
            let sub_steps = v.get("steps").and_then(|x| x.as_sequence()).cloned().unwrap_or_default();
            for n in 0..times {
                if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) || ctx.exit.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                ctx.log("debug", format!("循环第 {}/{} 次", n + 1, times));
                for sub in &sub_steps {
                    self.exec_step(ctx, sub).await?;
                    if ctx.exit.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
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
                    let sub_log = self.run(&ctx.device_id, &s.id, &s.content, ctx.stop.clone(), ctx.log_cb.clone(), 0, Some(ctx.exit.clone())).await?;
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

    /// until：超时时间内循环等主模板（单个）出现并点击（轮询间隔 interval
    /// 默认 500ms），出现执行 then，超时执行 else。
    /// 一轮匹配顺序 = before 障碍模板全部（依序：命中即点击关闭，未命中等
    /// img_ivl 匹配下一个——无论命中与否都不结束本轮）→ 主模板（命中即点击
    /// 并结束步骤）；未命中隔 interval 重开一轮（又从 before 开始）。
    /// 相邻两次模板匹配之间隔 img_ivl（默认 50ms）。
    /// threshold/region/count（连击，总次数含首击默认 1，按首击坐标无条件连点）/cnt_ivl 规则
    /// 见模块头；时长参数统一 500 / 500ms / 2s / 30min / 1h / 1d 写法
    /// （见 parse_duration）；timeout 必须 > 0（默认 30 分钟）。
    /// 2026-08-25 重构：find 动作、click/check 简写、and_or/click/cnt_chk 参数、
    /// 多主模板（逗号/列表）与 then 按模板分支均已删除
    #[async_recursion]
    async fn exec_until(&self, ctx: &mut Ctx, step: &Value) -> anyhow::Result<()> {
        // 已删除参数显式报错（防写了静默失效）
        for k in ["and_or", "click"] {
            if step.get(k).is_some() {
                anyhow::bail!("until 的 {} 参数已删除（命中恒点击模板中心）", k);
            }
        }
        if step.get("cnt_chk").is_some() {
            anyhow::bail!("cnt_chk 已删除：命中后按首击坐标无条件连点（想防误点请拆成多步 until）");
        }
        // 主模板只支持单个（字符串；逗号/列表是已删除的多模板写法）
        let template = match step.get("until") {
            Some(Value::String(s)) => {
                let t = s.trim().to_string();
                if t.is_empty() {
                    anyhow::bail!("until 模板名不能为空");
                }
                if t.contains(',') {
                    anyhow::bail!("until 只支持单个主模板（多个目标请拆成多步；障碍模板写 before）");
                }
                t
            }
            Some(_) => anyhow::bail!("until 只支持单个主模板名字符串（障碍模板列表用 before）"),
            None => anyhow::bail!("缺少 until"),
        };
        let before = match step.get("before") {
            Some(v) => Self::parse_tpl_names(v, "before")?,
            None => Vec::new(),
        };
        // before 与主模板重复无意义（同一模板既是障碍又是主目标），显式报错防手误
        if before.iter().any(|b| b == &template) {
            anyhow::bail!("before 模板 {} 与 until 主模板重复", template);
        }
        let interval_ms = self.opt_duration(step, "interval")?.unwrap_or(500);
        if interval_ms == 0 {
            anyhow::bail!("interval 必须大于 0（轮询间隔，支持 500ms / 2s 等写法）");
        }
        let img_ivl_ms = self.opt_duration(step, "img_ivl")?.unwrap_or(50);
        let timeout_ms = match self.opt_duration(step, "timeout")? {
            Some(t) => {
                if t == 0 {
                    anyhow::bail!("until 的 timeout 必须 > 0（默认 30 分钟；支持 500 / 500ms / 2s / 30min / 1h / 1d 写法）");
                }
                t
            }
            None => UNTIL_DEFAULT_TIMEOUT_MS,
        };
        let threshold = self.opt_f64(step, "until", "threshold")
            .map(|x| x as f32)
            .unwrap_or(self.devices.cfg.default_threshold);
        if before.is_empty() {
            ctx.log("info", format!("等待模板 {}，超时 {}ms，轮询 {}ms", template, timeout_ms, interval_ms));
        } else {
            ctx.log("info", format!(
                "等待模板 {}（先处理障碍 {}），超时 {}ms，轮询 {}ms，模板间隔 {}ms",
                template, before.join("、"), timeout_ms, interval_ms, img_ivl_ms
            ));
        }
        let then_steps = self.opt_value(step, "until", "then").and_then(|v| v.as_sequence()).cloned().unwrap_or_default();
        let else_steps = self.opt_value(step, "until", "else").and_then(|v| v.as_sequence()).cloned().unwrap_or_default();
        let start = std::time::Instant::now();
        loop {
            if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            if start.elapsed().as_millis() as u64 > timeout_ms {
                ctx.log("warn", format!("等待模板 {} 超时（{}ms）", template, timeout_ms));
                for sub in &else_steps {
                    self.exec_step(ctx, sub).await?;
                }
                break;
            }
            // 一轮：before 障碍依序匹配（命中即点击关闭，未命中等 img_ivl 匹配
            // 下一个——无论命中与否都不结束本轮）→ 主模板（命中即点击并结束
            // 步骤）；相邻匹配隔 img_ivl
            let mut hit = false;
            let mut stopped = false;
            let total = before.len() + 1;
            for i in 0..total {
                if i > 0 && img_ivl_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(img_ivl_ms)).await;
                }
                if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) {
                    stopped = true;
                    break;
                }
                let is_before = i < before.len();
                let tpl = if is_before { &before[i] } else { &template };
                if let Some(m) = self.match_one(ctx, step, tpl, threshold).await? {
                    self.emit(&ctx.device_id, ScriptEvent::Hit {
                        tpl: tpl.clone(),
                        x: m.x, y: m.y, w: m.width, h: m.height, score: m.score,
                    }).await;
                    if is_before {
                        ctx.log("success", format!("障碍模板 {} 出现，点击关闭 @ ({}, {})", tpl, m.x, m.y));
                        self.click_center(ctx, step, &m).await?;
                    } else {
                        ctx.log("success", format!("模板 {} 已找到 @ ({}, {})", tpl, m.x, m.y));
                        self.click_center(ctx, step, &m).await?;
                        hit = true;
                        break;
                    }
                }
            }
            if stopped {
                break;
            }
            if hit {
                for sub in &then_steps {
                    self.exec_step(ctx, sub).await?;
                }
                break;
            }
            // 本轮主模板未命中 → 间隔 interval 后从 before 重开一轮
            tokio::time::sleep(Duration::from_millis(interval_ms)).await;
        }
        Ok(())
    }

    /// color：取点比色（多检查点超时轮询）。写法与 until 同构——参数均为
    /// 兄弟键（2 空格缩进），只有 check/then/else 的列表项再 +2 空格：
    ///   - color:                # 动作键（值留空，- color 无参简写等价）
    ///     timeout: 5min          # 超时（默认 5min，必须 > 0；写法同 until 时长参数）
    ///     interval: 500ms        # 检测间隔（默认 500ms）
    ///     check:                 # 检查点列表（必填）：超时时间内逐轮检测，任一命中执行 then、超时执行 else
    ///       - [x1, y1]: ff8800   # 该点像素是否为 ff8800（每通道容差固定 30：
    ///                            # H.264 有损压缩帧间抖动，精确匹配实际不可用）
    ///       - [x2, y2]: ff8899
    ///     then: / else:          # 普通步骤列表（若需子步骤，列表项再 +2 空格）
    /// 每轮 = 取最新截图 → 依序检查全部检查点（各自独立取像素），任一命中即执行 then 结束；
    /// 全部未命中隔 interval 重新截图一轮；累计超过 timeout 执行 else。
    /// 旧写法（`- color: [x,y]` + `check: 颜色` 兄弟键单点）已删除，显式报错引导
    async fn exec_color(&self, ctx: &mut Ctx, step: &Value, v: &Value) -> anyhow::Result<()> {
        // 旧写法显式报错：color 值非空（数组/字符串 = 旧单点语法）
        if !v.is_null() {
            anyhow::bail!("color 动作键值留空（- color: 后接兄弟键 timeout/interval/check）；旧 `- color: [x,y]` + `check: 色值` 已删除");
        }
        if step.get("tol").is_some() || step.get("cnt_ivl").is_some() || step.get("count").is_some() {
            anyhow::bail!("color 的 tol/count/cnt_ivl 参数已删除：改定时长 timeout/interval + check 检查点列表（见新语法）");
        }
        let timeout = match step.get("timeout") {
            Some(t) => {
                let ms = Self::parse_duration(t, "timeout")?;
                if ms == 0 {
                    anyhow::bail!("color 的 timeout 必须 > 0（默认 5min）");
                }
                ms
            }
            None => 300_000, // 未写 timeout 时默认 5min
        };
        let interval = match step.get("interval") {
            Some(iv) => {
                let t = Self::parse_duration(iv, "interval")?;
                if t == 0 {
                    anyhow::bail!("color 的 interval 必须大于 0（检测间隔）");
                }
                t
            }
            None => 500,
        };
        // check 检查点列表：每项为单键映射 `[x, y]: 色值`
        let checks = match step.get("check") {
            Some(c) => {
                let seq = c.as_sequence().ok_or_else(|| anyhow::anyhow!("color 的 check 需要检查点列表（- [x, y]: 色值），收到: {:?}", c))?;
                if seq.is_empty() {
                    anyhow::bail!("color 的 check 至少需要一个检查点");
                }
                let mut list = Vec::new();
                for item in seq {
                    let m = item.as_mapping().ok_or_else(|| anyhow::anyhow!("color 检查点需要单键映射 `- [x, y]: 色值`，收到: {:?}", item))?;
                    if m.len() != 1 {
                        anyhow::bail!("color 检查点需要单键映射 `- [x, y]: 色值`，收到: {:?}", item);
                    }
                    let (k, val) = m.iter().next().unwrap();
                    let (rx, ry) = Self::parse_rel_coord(k)?;
                    let (er, eg, eb) = Self::parse_color(val)?;
                    list.push((rx, ry, er, eg, eb));
                }
                list
            }
            None => anyhow::bail!("color 缺少 check 检查点列表（- [x, y]: 色值）"),
        };
        let exp_str: Vec<String> = checks.iter().map(|(_, _, r, g, b)| format!("{:02x}{:02x}{:02x}", r, g, b)).collect();
        let then_steps = step.get("then").and_then(|v| v.as_sequence()).cloned().unwrap_or_default();
        let else_steps = step.get("else").and_then(|v| v.as_sequence()).cloned().unwrap_or_default();
        // 每通道容差固定 30（H.264 有损压缩帧间像素抖动，精确匹配实际不可用）
        const TOL: i32 = 30;
        ctx.log("info", format!("检测颜色 {} 个检查点（{}），间隔 {}ms，超时 {}ms", checks.len(), exp_str.join(" / "), interval, timeout));
        let start = std::time::Instant::now();
        loop {
            if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(());
            }
            if start.elapsed().as_millis() as u64 > timeout {
                ctx.log("info", format!("颜色 {} 超时未命中，执行 else", exp_str.join(" / ")));
                for sub in &else_steps {
                    self.exec_step(ctx, sub).await?;
                }
                break;
            }
            let screen = self.devices.screenshot(&ctx.device_id).await
                .map_err(|e| anyhow::anyhow!("截图失败: {}", e))?;
            let img = image::load_from_memory(&screen)
                .map_err(|e| anyhow::anyhow!("解析截图失败: {}", e))?;
            let (w, h) = img.dimensions();
            if w == 0 || h == 0 {
                anyhow::bail!("无法获取屏幕尺寸");
            }
            let rgb = img.to_rgb8();
            let mut hit = false;
            for (rx, ry, er, eg, eb) in &checks {
                let px = ((rx * w as f64).round() as i64).clamp(0, w as i64 - 1) as u32;
                let py = ((ry * h as f64).round() as i64).clamp(0, h as i64 - 1) as u32;
                let p = rgb.get_pixel(px, py).0;
                let (ar, ag, ab) = (p[0] as i32, p[1] as i32, p[2] as i32);
                let exp = format!("{:02x}{:02x}{:02x}", er, eg, eb);
                if (ar - *er as i32).abs() <= TOL && (ag - *eg as i32).abs() <= TOL && (ab - *eb as i32).abs() <= TOL {
                    ctx.log("success", format!("颜色命中 {}（实际 {:02x}{:02x}{:02x}）@ 像素 ({}, {})", exp, ar, ag, ab, px, py));
                    // 可视化：以采样点为中心的小框（复用模板命中框样式与前端渲染）
                    self.emit(&ctx.device_id, ScriptEvent::Hit {
                        tpl: format!("clr {}", exp),
                        x: px.saturating_sub(12), y: py.saturating_sub(12), w: 24, h: 24, score: 1.0,
                    }).await;
                    for sub in &then_steps {
                        self.exec_step(ctx, sub).await?;
                    }
                    hit = true;
                    break;
                }
                ctx.log("debug", format!("颜色未命中：期望 {} 实际 {:02x}{:02x}{:02x} @ ({}, {})", exp, ar, ag, ab, px, py));
            }
            if hit {
                break;
            }
            // 本轮全部未命中 → 隔 interval 重新截图一轮
            tokio::time::sleep(Duration::from_millis(interval)).await;
        }
        Ok(())
    }

    /// 点击命中模板的中心并按 count 连击补点（before 障碍模板与主模板共用）：
    ///   count   总点击次数（含首击），默认 1（单击，不补点）；count ≤ 1 = 仅首击；
    ///           命中后按首击坐标无条件重复点击，不重新匹配（cnt_chk 参数已删除）
    ///   cnt_ivl 相邻两次点击的间隔，默认 50ms（写法同 timeout，见 parse_duration）
    async fn click_center(&self, ctx: &mut Ctx, step: &Value, m: &matcher::MatchResult) -> anyhow::Result<()> {
        let s = self.devices.session(&ctx.device_id).ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
        let (cx, cy) = (m.x + m.width / 2, m.y + m.height / 2);
        ctx.log("debug", format!("点击模板中心 @ ({}, {})", cx, cy));
        self.emit(&ctx.device_id, ScriptEvent::Tap { x: cx, y: cy }).await;
        s.tap(cx as f32, cy as f32).await?;
        self.exec_count_clicks(ctx, step, (cx, cy)).await
    }

    /// until 命中点击后的 count 连击补点：
    ///   count   总点击次数（含首击），默认 1（单击，不补点）；count ≤ 1 = 仅首击
    ///   cnt_ivl 相邻两次点击的间隔，默认 50ms（写法同 timeout，见 parse_duration）
    /// 第 2 次起无条件重复点击首击坐标（目标消失/翻页时可能点到原地，由脚本结构保证）；
    /// 不再重新匹配（cnt_chk 已删除，防止误点新出现的同模板，屏幕变化场景可拆多步）
    async fn exec_count_clicks(&self, ctx: &mut Ctx, step: &Value, first: (u32, u32)) -> anyhow::Result<()> {
        let count = match step.get("count") {
            Some(v) => Self::parse_count(v, "count")?,
            None => 1,
        };
        if count <= 1 {
            return Ok(());
        }
        if count > 100_000 {
            anyhow::bail!("count 过大（上限 100000），收到: {}", count);
        }
        let cnt_ivl = match step.get("cnt_ivl") {
            Some(v) => Self::parse_duration(v, "cnt_ivl")?,
            None => 50,
        };
        let s = self.devices.session(&ctx.device_id).ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
        let (cx, cy) = first;
        for i in 2..=count {
            if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(cnt_ivl)).await;
            if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            ctx.log("debug", format!("连击 {}/{}：首击坐标 @ ({}, {})", i, count, cx, cy));
            self.emit(&ctx.device_id, ScriptEvent::Tap { x: cx, y: cy }).await;
            s.tap(cx as f32, cy as f32).await?;
        }
        Ok(())
    }

    /// 解析点击次数参数：YAML 数字（6）；带引号的数字字符串（"6"）容忍
    fn parse_count(v: &Value, opt: &str) -> anyhow::Result<u64> {
        if let Some(n) = v.as_u64() {
            return Ok(n);
        }
        if let Some(s) = v.as_str() {
            if let Ok(n) = s.trim().parse::<u64>() {
                return Ok(n);
            }
        }
        anyhow::bail!("{} 需要正整数（如 6），收到: {:?}", opt, v)
    }

    /// 解析时长参数（timeout/interval/img_ivl/cnt_ivl 共用）：
    /// 纯数字 = 毫秒（500，YAML 数字或裸数字字符串均可）；
    /// 字符串支持单位 1ms / 1s / 1min / 1h / 1d（大小写不敏感、可带小数如 "1.5s"）
    fn parse_duration(v: &Value, opt: &str) -> anyhow::Result<u64> {
        if let Some(n) = v.as_u64() {
            return Ok(n);
        }
        let Some(s) = v.as_str() else {
            anyhow::bail!("{} 需要毫秒数或带单位时长（如 500 / 500ms / 2s / 30min / 1h / 1d），收到: {:?}", opt, v);
        };
        let t = s.trim().to_ascii_lowercase();
        // 后缀匹配：ms 必须在 s 前判（"1ms" 剥掉 "s" 会剩 "1m" 解析失败）
        for (suffix, mult) in [("ms", 1.0f64), ("min", 60_000.0), ("s", 1_000.0), ("h", 3_600_000.0), ("d", 86_400_000.0)] {
            if let Some(num) = t.strip_suffix(suffix) {
                if let Ok(val) = num.trim().parse::<f64>() {
                    if val >= 0.0 {
                        return Ok((val * mult).round() as u64);
                    }
                }
            }
        }
        if let Ok(n) = t.parse::<u64>() {
            return Ok(n);
        }
        anyhow::bail!("{} 需要毫秒数或带单位时长（如 500 / 500ms / 2s / 30min / 1h / 1d），收到: {}", opt, s)
    }

    /// 解析 color 步骤的 check 颜色：6 位十六进制 RRGGBB（可带 # / 0x 前缀、
    /// 大小写不限）或 [r, g, b] 数字数组（0~255）；整数（YAML 解析器把 0xff8800
    /// 直接解析成数字时）按 0xRRGGBB 解码
    fn parse_color(v: &Value) -> anyhow::Result<(u8, u8, u8)> {
        if let Some(s) = v.as_str() {
            let t = s.trim().trim_start_matches('#').trim_start_matches("0x").to_ascii_lowercase();
            if t.len() == 6 && t.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok((
                    u8::from_str_radix(&t[0..2], 16).unwrap(),
                    u8::from_str_radix(&t[2..4], 16).unwrap(),
                    u8::from_str_radix(&t[4..6], 16).unwrap(),
                ));
            }
            anyhow::bail!("check 颜色需要 6 位十六进制（如 ff8800 或 \"#ff8800\"）或 [r, g, b]，收到: {}", s);
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
                        let n = x.as_u64().or_else(|| x.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
                            .ok_or_else(|| anyhow::anyhow!("check 颜色数组需要 [r, g, b] 数字（0~255）"))?;
                        if n > 255 {
                            anyhow::bail!("check 颜色分量必须在 0~255，收到: {}", n);
                        }
                        Ok(n as u8)
                    })
                    .collect::<anyhow::Result<Vec<u8>>>()?;
                return Ok((c[0], c[1], c[2]));
            }
        }
        anyhow::bail!("check 颜色只支持 6 位十六进制（ff8800）或 [r, g, b] 数组，收到: {:?}", v)
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
            // 短名引用时 #后缀在**实际文件名**上（脚本写 login.png 引用
            // login#910_159_972_716.png），区域须按解析结果取名才生效
            None => Self::tpl_region_from_name(&Self::region_source_name(&self.tpl_dir_of(ctx), template), w, h)?,
        };
        self.match_on_screen(ctx, &template, threshold, region, screen).await
    }

    /// 在给定截图上匹配模板（region 为搜索区域，None=全屏）
    async fn match_on_screen(&self, ctx: &Ctx, template: &str, threshold: f32, region: Option<[u32; 4]>, screen: Vec<u8>) -> anyhow::Result<Option<matcher::MatchResult>> {
        // 模板按脚本所在应用分区解析：data/<pkg>/tmpl/（script_id 首段 = 分区）
        let tpl_dir = self.tpl_dir_of(ctx);
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

    /// 脚本所在分区的模板目录：data/<pkg>/tmpl/（script_id 首段 = 分区）
    fn tpl_dir_of(&self, ctx: &Ctx) -> std::path::PathBuf {
        let pkg = ctx.script_id.split('/').next().unwrap_or_default();
        self.devices.cfg.data_dir.join(pkg).join("tmpl")
    }

    /// #区域后缀的解析来源名：短名引用时后缀在**实际文件名**上（脚本写
    /// login.png → login#910_159_972_716.png，区域随解析结果生效）；
    /// 解析不出文件（不存在/多候选）时回退书写的名字——真正的错误由
    /// match_on_screen 的 resolve_template_file 统一报出，这里不重复报
    fn region_source_name(tpl_dir: &std::path::Path, template: &str) -> String {
        Self::resolve_template_file(tpl_dir, template)
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| template.to_string())
    }

    /// 从步骤中取模板名列表：支持单模板字符串 `find: a.png`、逗号分隔多模板
    /// `find: a.png, b.png` 与 YAML 列表 `find: [a.png, b.png]` 三种写法
    fn template_names(&self, step: &Value, key: &str) -> anyhow::Result<Vec<String>> {
        let v = step.get(key).ok_or_else(|| anyhow::anyhow!("缺少 {}", key))?;
        Self::parse_tpl_names(v, key)
    }

    /// 解析模板名列表：字符串（可逗号分隔多模板）或 YAML 字符串列表
    fn parse_tpl_names(v: &Value, key: &str) -> anyhow::Result<Vec<String>> {
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

    /// 解析 until 命中后的 then 步骤（按模板分支）：
    ///   then:
    ///     - test1.png:        # 单键映射且键在主模板列表中 = 模板专属分支
    ///         - log: "命中 test1"
    ///     - test2.png:
    ///         - log: "命中 test2"
    ///     - log: "兜底"       # 其余普通步骤 = 兜底，命中的模板没有专属分支时执行
    /// 命中的模板有专属分支走分支（多模板 until 任一命中即停，命中的恰为一个），
    /// 没有匹配分支则执行兜底步骤。
    /// 纯普通步骤列表（不带分支键）不产生分支，全部当兜底执行；
    /// 键为 wait/log 等动作的普通步骤不会撞模板名（分支键必须在主模板列表中）
    fn parse_then(v: &Value, key: &str, templates: &[String]) -> anyhow::Result<(Vec<(String, Vec<Value>)>, Vec<Value>)> {
        // 单键 + 列表值 + 键非动作名 = 疑似模板名写错的分支（如扩展名拼错），
        // 不报错会被当普通步骤静默跳过，分支永远不生效
        const STEP_KEYS: [&str; 14] = [
            "wait", "log", "key", "text", "tap", "swipe", "until", "color", "loop", "call", "goto", "label", "str_app", "cls_app",
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

    /// 取步骤时长参数（timeout/interval/img_ivl），缺失返回 None；
    /// 解析失败（格式非法）向上传播错误
    fn opt_duration(&self, step: &Value, opt: &str) -> anyhow::Result<Option<u64>> {
        match step.get(opt) {
            Some(v) => Self::parse_duration(v, opt).map(Some),
            None => Ok(None),
        }
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

    /// 命中后分支选择（exec_until 同款规则）：命中的模板有专属分支走分支，
    /// 没有则走兜底（多模板 until 任一命中即停，命中的恰为一个模板）
    #[test]
    fn then_branches_selection() {
        let (branches, fallback) = Runner::parse_then(
            &parse("- test1.png:\n    - log: \"1\"\n- test2.png:\n    - log: \"2\"\n- log: \"3\"\n"),
            "until",
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
        // 命中的是 test2 → test2 分支；命中 test1 → test1 分支
        assert_eq!(pick(&["test2.png"]), "test2.png");
        assert_eq!(pick(&["test1.png"]), "test1.png");
        // 命中的模板没有分支（单模板 until 复用分支时）→ 兜底
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

    /// 短名引用的区域后缀：区域从**解析后的实际文件名**取（脚本写 login.png →
    /// login#910_159_972_716.png 的后缀生效，否则短名会退化成全屏低分辨率匹配）；
    /// 精确名照旧；文件不存在 → 回退书写名（错误由 match_on_screen 报）
    #[test]
    fn short_name_region_from_resolved_file() {
        let dir = std::env::temp_dir().join(format!(
            "gamer-tplregion-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cleanup = || { let _ = std::fs::remove_dir_all(&dir); };
        std::fs::write(dir.join("login#910_159_972_716.png"), b"x").unwrap();
        std::fs::write(dir.join("shop.png"), b"x").unwrap();
        // 短名 → 解析到带后缀文件 → 区域生效（×1000 相对坐标 → 1920x1080 像素区域）
        let name = Runner::region_source_name(&dir, "login.png");
        assert_eq!(name, "login#910_159_972_716.png");
        assert_eq!(
            Runner::tpl_region_from_name(&name, 1920, 1080).unwrap(),
            Some([1747, 172, 119, 602])
        );
        // 精确名（无后缀）→ 原名；不存在 → 回退书写名
        assert_eq!(Runner::region_source_name(&dir, "shop.png"), "shop.png");
        assert_eq!(Runner::region_source_name(&dir, "nope.png"), "nope.png");
        cleanup();
    }

    /// 时长参数解析（parse_duration）：纯数字（YAML 数字/裸数字串）= ms；
    /// 单位串 1ms/1s/1min/1h/1d（大小写不敏感、支持小数如 1.5s）；
    /// 非法值（无数字、未知单位、负数）报错
    #[test]
    fn duration_parse() {
        let d = |yaml: &str| Runner::parse_duration(&parse(yaml), "timeout").unwrap();
        // 纯数字：YAML 数字与裸数字字符串
        assert_eq!(d("500"), 500);
        assert_eq!(d("\"500\""), 500);
        // 单位（ms 前置于 s 判定，"1ms" 不会被 "s" 剥成 "1m"）
        assert_eq!(d("1ms"), 1);
        assert_eq!(d("2s"), 2_000);
        assert_eq!(d("30min"), 1_800_000);
        assert_eq!(d("1h"), 3_600_000);
        assert_eq!(d("1d"), 86_400_000);
        assert_eq!(d("\"1.5s\""), 1_500);
        assert_eq!(d("\"80 ms\""), 80);
        assert_eq!(d("30MIN"), 1_800_000);
        // 非法：未知单位 / 空数字 / 负数 / 非字符串非数字
        assert!(Runner::parse_duration(&parse("fast"), "timeout").is_err());
        assert!(Runner::parse_duration(&parse("\"ms\""), "timeout").is_err());
        assert!(Runner::parse_duration(&parse("\"1m\""), "timeout").is_err());
        assert!(Runner::parse_duration(&parse("\"-5s\""), "timeout").is_err());
        assert!(Runner::parse_duration(&parse("true"), "timeout").is_err());
    }

    /// count 连击参数解析：count 支持 YAML 数字与带引号数字串；
    /// cnt_ivl 走 parse_duration（数字 / 各单位串）；非法值报错
    #[test]
    fn count_params_parse() {
        assert_eq!(Runner::parse_count(&parse("6"), "count").unwrap(), 6);
        assert_eq!(Runner::parse_count(&parse("\"6\""), "count").unwrap(), 6);
        assert!(Runner::parse_count(&parse("many"), "count").is_err());
        assert_eq!(Runner::parse_duration(&parse("100"), "cnt_ivl").unwrap(), 100);
        assert_eq!(Runner::parse_duration(&parse("100ms"), "cnt_ivl").unwrap(), 100);
        assert_eq!(Runner::parse_duration(&parse("\"80 ms\""), "cnt_ivl").unwrap(), 80);
        assert!(Runner::parse_duration(&parse("fast"), "cnt_ivl").is_err());
    }

    /// color 步骤的 check 颜色解析：6 位十六进制（不带 #，宽容接受 # / 0x 前缀、
    /// 大小写）、[r, g, b] 数组、0x 整数；位数不对 / 非法字符 / 分量越界报错
    #[test]
    fn color_check_parse() {
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

    /// exec_step / exec_until 的解析期校验回归（2026-08-25 重构后）：
    /// 旧写法（find / click 简写 / 裸 check）显式报错引导迁移；until 参数校验
    /// （timeout 必须 > 0、interval > 0、before 与主模板重复）都在触碰设备/截图
    /// 之前报错；合法 until 步骤解析通过、进入匹配循环后在截图处失败（无设备）
    #[tokio::test]
    async fn until_step_validation() {
        let dir = std::env::temp_dir().join(format!(
            "gamer-engine-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = crate::config::Config::default();
        cfg.data_dir = dir.clone();
        let db: crate::store::Db = std::sync::Arc::new(crate::store::Store::open(&cfg).unwrap());
        let viewers: crate::webrtc::ViewerMap =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let devices = std::sync::Arc::new(crate::device::DeviceManager::new(db.clone(), cfg.clone(), viewers.clone()));
        let scripts = std::sync::Arc::new(crate::scripts::ScriptStore::open(&cfg).unwrap());
        let runner = Runner::new(db, devices, viewers, scripts);
        let mut ctx = Ctx {
            device_id: "test-dev".into(),
            script_id: "com.test/t.yaml".into(),
            label_index: Default::default(),
            log: Vec::new(),
            stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            exit: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            action_wait: 0,
            log_debug: true,
            log_cb: None,
        };
        async fn run(runner: &Runner, ctx: &mut Ctx, yaml: &str) -> anyhow::Result<()> {
            let step = parse(yaml).get(0).unwrap().clone();
            runner.exec_step(ctx, &step).await
        }
        // 旧写法：find / click 简写 / 裸 check / check+click
        assert!(run(&runner, &mut ctx, "- find: a.png").await.unwrap_err().to_string().contains("find 已删除"));
        assert!(run(&runner, &mut ctx, "- click: a.png").await.unwrap_err().to_string().contains("click 已删除"));
        assert!(run(&runner, &mut ctx, "- click: a.png\n  check: b.png").await.unwrap_err().to_string().contains("click 已删除"));
        assert!(run(&runner, &mut ctx, "- check: b.png").await.unwrap_err().to_string().contains("check 只能与 color"));
        // until 已删除参数（and_or / click / cnt_chk）显式报错
        assert!(run(&runner, &mut ctx, "- until: a.png\n  and_or: and").await.unwrap_err().to_string().contains("and_or 参数已删除"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  click: false").await.unwrap_err().to_string().contains("click 参数已删除"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  click: btn.png").await.unwrap_err().to_string().contains("click 参数已删除"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  cnt_chk: true").await.unwrap_err().to_string().contains("cnt_chk 已删除"));
        // until 参数校验（均在截图前报错，无需设备）
        assert!(run(&runner, &mut ctx, "- until: a.png\n  timeout: 0").await.unwrap_err().to_string().contains("timeout 必须 > 0"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  timeout: 0s").await.unwrap_err().to_string().contains("timeout 必须 > 0"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  interval: 0").await.unwrap_err().to_string().contains("interval 必须大于 0"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  interval: -1").await.unwrap_err().to_string().contains("interval"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  before: a.png").await.unwrap_err().to_string().contains("与 until 主模板重复"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  timeout: fast").await.unwrap_err().to_string().contains("带单位时长"));
        // 合法步骤：解析通过、进入循环，无设备在截图处失败（证明校验未误伤）
        assert!(run(&runner, &mut ctx, "- until: a.png\n  timeout: 30min\n  interval: 500ms\n  before: b.png, c.png\n  img_ivl: 50ms").await.unwrap_err().to_string().contains("截图失败"));
        // 保留参数 threshold/region/count/cnt_ivl 合法（同样走到截图才失败）
        assert!(run(&runner, &mut ctx, "- until: a.png\n  threshold: 0.9\n  region: l\n  count: 3\n  cnt_ivl: 80ms").await.unwrap_err().to_string().contains("截图失败"));
        // color 新语法（兄弟键 2 空格，与 until 同构）：缺少 check 在截图前报错；合法语法走到截图才失败
        let e = run(&runner, &mut ctx, "- color:\n  timeout: 1min").await.unwrap_err().to_string();
        assert!(e.contains("缺少 check"), "actual: {}", e);
        assert!(run(&runner, &mut ctx, "- color:\n  check:\n    - [0.5, 0.5]: ff8800").await.unwrap_err().to_string().contains("截图失败"));
        assert!(run(&runner, &mut ctx, "- color:\n  check:\n    - [0.5, 0.5]: ff8800\n    - [0.2, 0.3]: ff8899").await.unwrap_err().to_string().contains("截图失败"));
        // 旧 color 单点写法显式报错（color 值非空）
        assert!(run(&runner, &mut ctx, "- color: [0.5, 0.5]\n  check: ff8800").await.unwrap_err().to_string().contains("旧 `- color: [x,y]`"));
        // exit：无参 / 带参均立即设置 exit 标志并打印日志（无需设备）
        ctx.exit.store(false, std::sync::atomic::Ordering::SeqCst);
        run(&runner, &mut ctx, "- exit").await.unwrap();
        assert!(ctx.exit.load(std::sync::atomic::Ordering::SeqCst));
        assert!(ctx.log.iter().any(|(_, m)| m == "结束运行脚本"));
        ctx.exit.store(false, std::sync::atomic::Ordering::SeqCst);
        run(&runner, &mut ctx, "- exit: 体力不足").await.unwrap();
        assert!(ctx.exit.load(std::sync::atomic::Ordering::SeqCst));
        assert!(ctx.log.iter().any(|(_, m)| m == "因 体力不足 结束运行脚本"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
