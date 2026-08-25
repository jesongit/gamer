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
//!        拆成多步、挡路的模板写 check）；
//!        check 障碍模板（`check: b.png` / `b.png, c.png` / [b.png, c.png]，
//!        与主模板重复报错；2026-08-25 前叫 before）每轮开头依序匹配：命中即
//!        点击关闭、未命中等 img_ivl 匹配下一个——无论命中与否都不结束本轮；
//!        一轮 = before 步骤全部 → check 障碍全部 → 主模板（相邻两次匹配隔
//!        img_ivl，默认 50ms）；
//!        主模板命中即点击模板中心并执行 then 结束步骤，本轮不执行 after；
//!        未命中执行 after 步骤后隔 interval（必须 > 0，默认 500ms）重开一轮
//!        （又从 before 开始）；timeout 超时（必须 > 0，默认 30min，支持
//!        500 / 500ms / 2s / 30min / 1h / 1d 写法）超时执行 else；
//!        then/else/before/after 为普通步骤列表（before=每轮匹配前执行、
//!        after=每轮未命中后执行；「模板名: 步骤列表」分支写法已删除）；
//!        threshold 匹配阈值（默认 config default_threshold）；region 搜索区域
//!        （统一作用于全部模板；未显式时模板名可自带 #后缀区域：
//!        xx#l / xx#0_0_500_500，见 tpl_region_from_name）；
//!        count 连击补点：总点击次数含首击、默认 1（单击），命中后按首击
//!        坐标无条件重复点击、cnt_ivl 相邻点击间隔默认 50ms（写法同 timeout）；
//!        对主模板与 check 障碍模板的点击同样生效（cnt_chk 已删除，写了报错）；
//!        verify 生效验证（2026-08-25 增，默认 false 无逻辑）：true 时点击
//!        （含 count 连击）后每 50ms 复查主模板是否仍在屏上，**模板消失**
//!        （点击生效/页面翻走）才执行 then 结束步骤，持续命中到 timeout 执行
//!        else；verify 阶段共用步骤 timeout 与 threshold/region；
//!        find / click-check 简写及 and_or / click / cnt_chk 参数已删除，写了报错) /
//!   cond(条件分支（2026-08-25 增，`engine::exec_cond`；**color 动作已删除**，
//!        颜色判断并入 cond）：`- cond:` 条件列表按序**一次截图**判定，命中
//!        一个 → 执行该条件步骤并结束本步，全部未命中 → 执行 else（cond 兄弟
//!        键）。条件 = 单键映射「条件键: 命中步骤列表」：模板条件键 = 模板名
//!        （- test.png: + 缩进步骤，**冒号必须写**——标量项后挂不了缩进内容）；
//!        颜色条件键 = 6 位十六进制色值（- ff8800:）+ `pos: [x, y]` 兄弟键给
//!        采样相对坐标（**pos 的存在即颜色条件的标志**；容差固定 30）。注意
//!        颜色条件的步骤行须写在色值键**正下方**或缩进 +2（映射键之间不能插
//!        序列项，pos 之后不能跟同列 `- ` 行）。单遍判定不轮询不超时（要重试
//!        套 loop/goto）；threshold/region 只作用于模板条件) /
//!   exit(结束脚本运行：- exit 无参数打印"结束运行脚本"；- exit: 原因 打印
//!        "因 原因 结束运行脚本"；call 子脚本内 exit 同样结束整个任务) /
//!   loop / goto / label /
//!   call(调用子脚本可传参：`- call: 子脚本.yml 实参1 实参2`（空格分隔），
//!        子脚本内 `$1`/`$2`… 引用实参（YAML 裸标量 @ 开头是保留字符非法，
//!        故用 $；替换作用于子脚本全部字符串键值，嵌套 call 转发 $N 同样生效）)
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

/// until verify=true 时点击后复查主模板的间隔毫秒数
const UNTIL_VERIFY_INTERVAL_MS: u64 = 50;

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
    /// `args`：call 传入的实参（主脚本运行传空）——子脚本内 `$1`/`$2`… 引用，
    /// 解析后先做全文替换（见 substitute_args）；引用超出实参数量直接报错
    pub async fn run(
        &self,
        device_id: &str,
        script_id: &str,
        content: &str,
        stop: Arc<std::sync::atomic::AtomicBool>,
        log_cb: Option<Arc<dyn Fn(String, String) + Send + Sync>>,
        start_step: usize,
        exit: Option<Arc<std::sync::atomic::AtomicBool>>,
        args: Vec<String>,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let mut doc: Value = serde_yaml::from_str(content)?;
        // call 传参替换：$N（N 从 1 起）→ 实参；含 $N 而未提供足够实参（主脚本
        // 直接运行/参数传少）在这里就报错，不会拖到模板匹配才失败
        Self::substitute_args(&mut doc, &args)?;
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
        // until 命中恒点击模板中心，check 障碍同样点中心
        if step.get("click").is_some() && step.get("until").is_none() {
            anyhow::bail!("click 已删除：改写 `- until: 主模板` + `check: 障碍模板`（命中恒点击模板中心）");
        }
        // color 动作已删除（2026-08-25）：颜色判断并入 cond 的颜色条件
        if step.get("color").is_some() {
            anyhow::bail!("color 已删除：颜色判断改用 cond（- ff8800: 命中步骤 + pos: [x, y]）；要轮询等颜色出现用 loop/goto + cond");
        }
        // check 键：until 的障碍模板列表（2026-08-25 前叫 before）；其余场合报错
        if step.get("check").is_some() && step.get("until").is_none() {
            anyhow::bail!("check 只能与 until 配合使用（障碍模板，旧名 before）");
        }
        // 动作键（除 wait 外）：用于区分 `wait` 动作与操作级 `wait` 参数
        const ACTION_KEYS: [&str; 13] = [
            "log", "key", "text", "tap", "swipe", "until", "cond", "loop", "call", "goto", "str_app", "cls_app", "exit",
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
        if let Some(v) = step.get("cond") {
            self.exec_cond(ctx, step, v).await?;
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
            // `- call: 子脚本.yml 实参1 实参2`：首段=子脚本名，其余空格分隔=实参，
            // 子脚本内 `$1`/`$2`… 引用（见 substitute_args）
            let line = v
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("call 需要 \"子脚本名 [实参...]\" 字符串（如 - call: test2.yml a.png b.png）"))?;
            let mut parts = line.split_whitespace();
            let script_name = parts.next().unwrap_or_default();
            let args: Vec<String> = parts.map(str::to_string).collect();
            // 子脚本按名解析：优先调用者同分区，其次跨分区（缺扩展名自动补全）
            let caller_pkg = ctx.script_id.split('/').next().unwrap_or_default();
            match self.scripts.resolve_call(caller_pkg, script_name)? {
                Some(s) => {
                    if args.is_empty() {
                        ctx.log("debug", format!("调用子脚本 {}", script_name));
                    } else {
                        ctx.log("debug", format!("调用子脚本 {}（实参 {}）", script_name, args.join(" ")));
                    }
                    let sub_log = self.run(&ctx.device_id, &s.id, &s.content, ctx.stop.clone(), ctx.log_cb.clone(), 0, Some(ctx.exit.clone()), args).await?;
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
    /// 一轮 = before 步骤全部 → check 障碍模板全部（依序：命中即点击关闭，
    /// 未命中等 img_ivl 匹配下一个——无论命中与否都不结束本轮）→ 主模板
    /// （命中即点击并结束步骤，本轮不执行 after）；未命中执行 after 步骤后
    /// 隔 interval 重开一轮（又从 before 开始）。相邻两次模板匹配之间隔
    /// img_ivl（默认 50ms）。
    /// threshold/region/count（连击，总次数含首击默认 1，按首击坐标无条件连点）/cnt_ivl 规则
    /// 见模块头；时长参数统一 500 / 500ms / 2s / 30min / 1h / 1d 写法
    /// （见 parse_duration）；timeout 必须 > 0（默认 30 分钟）。
    /// verify（2026-08-25 增，默认 false 无逻辑）：true 时命中点击（含 count 连击）
    /// 后每 50ms 复查主模板，**消失**才走 then；持续命中到 timeout 走 else。
    /// 2026-08-25：find 动作、click-check 简写、and_or/click/cnt_chk 参数、
    /// 多主模板（逗号/列表）与 then 按模板分支均已删除；
    /// 障碍模板 before 改名 check，before/after 转为每轮检测前/未命中后执行的步骤列表
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
                    anyhow::bail!("until 只支持单个主模板（多个目标请拆成多步；障碍模板写 check）");
                }
                t
            }
            Some(_) => anyhow::bail!("until 只支持单个主模板名字符串（障碍模板列表用 check）"),
            None => anyhow::bail!("缺少 until"),
        };
        // check 障碍模板（2026-08-25 前叫 before，旧写法显式报错引导迁移）
        let checks = match step.get("check") {
            Some(v) => Self::parse_tpl_names(v, "check")?,
            None => Vec::new(),
        };
        // 旧写法残留：before 值是字符串 / 列表里混模板名（.png 等）= 当年的障碍
        // 模板写法，静默当步骤列表会把 "a.png" 规范成无动作空步骤，显式报错
        if let Some(bv) = step.get("before") {
            let looks_old = match bv {
                Value::String(_) => true,
                Value::Sequence(seq) => seq.iter().any(|item| matches!(item, Value::String(s) if s.contains('.'))),
                _ => false,
            };
            if looks_old {
                anyhow::bail!("until 的 before 障碍模板已改名 check（`before: a.png` → `check: a.png`）；before 现在是每轮匹配前执行的步骤列表");
            }
        }
        // before/after：每轮检测前 / 未命中后执行的步骤列表
        let before_steps = Self::parse_round_steps(step, "before")?;
        let after_steps = Self::parse_round_steps(step, "after")?;
        // check 与主模板重复无意义（同一模板既是障碍又是主目标），显式报错防手误
        if checks.iter().any(|b| b == &template) {
            anyhow::bail!("check 模板 {} 与 until 主模板重复", template);
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
        // verify 生效验证（默认 false 无逻辑）：true = 点击后复查主模板、消失才结束
        let verify = match step.get("verify") {
            Some(v) => v.as_bool().ok_or_else(|| anyhow::anyhow!("verify 需要 true / false（true=点击后每 50ms 复查主模板，消失才结束）"))?,
            None => false,
        };
        if checks.is_empty() && before_steps.is_empty() {
            ctx.log("info", format!("等待模板 {}，超时 {}ms，轮询 {}ms", template, timeout_ms, interval_ms));
        } else {
            ctx.log("info", format!(
                "等待模板 {}（先处理障碍 {}），超时 {}ms，轮询 {}ms，模板间隔 {}ms",
                template, checks.join("、"), timeout_ms, interval_ms, img_ivl_ms
            ));
        }
        let then_steps = self.opt_value(step, "until", "then").and_then(|v| v.as_sequence()).cloned().unwrap_or_default();
        let else_steps = self.opt_value(step, "until", "else").and_then(|v| v.as_sequence()).cloned().unwrap_or_default();
        let start = std::time::Instant::now();
        loop {
            if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) || ctx.exit.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            if start.elapsed().as_millis() as u64 > timeout_ms {
                ctx.log("warn", format!("等待模板 {} 超时（{}ms）", template, timeout_ms));
                for sub in &else_steps {
                    self.exec_step(ctx, sub).await?;
                }
                break;
            }
            // 每轮检测前步骤（before）：如提前触发刷新/关闭干扰物
            for sub in &before_steps {
                self.exec_step(ctx, sub).await?;
            }
            if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) || ctx.exit.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            // 一轮：check 障碍依序匹配（命中即点击关闭，未命中等 img_ivl 匹配
            // 下一个——无论命中与否都不结束本轮）→ 主模板（命中即点击并结束
            // 步骤）；相邻匹配隔 img_ivl
            let mut hit = false;
            let mut stopped = false;
            let total = checks.len() + 1;
            for i in 0..total {
                if i > 0 && img_ivl_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(img_ivl_ms)).await;
                }
                if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) {
                    stopped = true;
                    break;
                }
                let is_check = i < checks.len();
                let tpl = if is_check { &checks[i] } else { &template };
                if let Some(m) = self.match_one(ctx, step, tpl, threshold).await? {
                    self.emit(&ctx.device_id, ScriptEvent::Hit {
                        tpl: tpl.clone(),
                        x: m.x, y: m.y, w: m.width, h: m.height, score: m.score,
                    }).await;
                    if is_check {
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
                // verify 生效验证：点击（含 count 连击，click_center 内已完成）后每
                // 50ms 复查主模板——消失（点击生效/页面翻走）才走 then；持续命中
                // 到 timeout 走 else；stop/exit 直接结束、不执行 then/else
                if verify {
                    ctx.log("info", format!("verify：等待模板 {} 消失（每 {}ms 复查）", template, UNTIL_VERIFY_INTERVAL_MS));
                    let mut gone = false;
                    let mut interrupted = false;
                    loop {
                        if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) || ctx.exit.load(std::sync::atomic::Ordering::SeqCst) {
                            interrupted = true;
                            break;
                        }
                        if start.elapsed().as_millis() as u64 > timeout_ms {
                            ctx.log("warn", format!("verify：等待模板 {} 消失超时（{}ms）", template, timeout_ms));
                            break;
                        }
                        if self.match_one(ctx, step, &template, threshold).await?.is_none() {
                            ctx.log("success", format!("verify：模板 {} 已消失，点击已生效", template));
                            gone = true;
                            break;
                        }
                        ctx.log("debug", format!("verify：模板 {} 仍在屏上，{}ms 后复查", template, UNTIL_VERIFY_INTERVAL_MS));
                        tokio::time::sleep(Duration::from_millis(UNTIL_VERIFY_INTERVAL_MS)).await;
                    }
                    if !gone {
                        if !interrupted {
                            for sub in &else_steps {
                                self.exec_step(ctx, sub).await?;
                            }
                        }
                        break;
                    }
                }
                for sub in &then_steps {
                    self.exec_step(ctx, sub).await?;
                }
                break;
            }
            // 本轮主模板未命中 → after 步骤 → 间隔 interval 后从 before 重开一轮
            for sub in &after_steps {
                self.exec_step(ctx, sub).await?;
            }
            if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) || ctx.exit.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(interval_ms)).await;
        }
        Ok(())
    }

    /// until 的 before/after 步骤列表解析：值必须是步骤列表（YAML 列表），
    /// 非列表（字符串等）报错——until 的 before 原是障碍模板字符串（已改名
    /// check），字符串值直接按旧写法报错防静默失效
    fn parse_round_steps(step: &Value, key: &str) -> anyhow::Result<Vec<Value>> {
        let Some(v) = step.get(key) else {
            return Ok(Vec::new());
        };
        v.as_sequence()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("{} 需要步骤列表（until 每轮匹配前/未命中后执行）；until 的障碍模板请写 check", key))
    }

    /// 条件键的命中步骤值：步骤列表，或留空（null）= 无步骤
    fn opt_steps(v: &Value) -> anyhow::Result<Vec<Value>> {
        match v {
            Value::Null => Ok(Vec::new()),
            Value::Sequence(seq) => Ok(seq.clone()),
            _ => anyhow::bail!("需要步骤列表（- 条件键: 换行缩进步骤）或留空"),
        }
    }


    /// cond：条件分支（2026-08-25 增，color 动作已删除、颜色判断并入于此）。
    /// 条件列表按序**一次截图**逐个判定，命中一个 → 执行该条件的步骤并结束
    /// 本步（后续条件不再判定）；全部未命中 → 执行 else（cond 的兄弟键）。写法：
    ///   - cond:
    ///     - test.png:              # 模板条件：键=模板名，值=命中执行的步骤列表
    ///       - log: 命中模板        #   （YAML 标量项后不能挂缩进内容，模板名后的冒号必须写）
    ///     - ff8800:                # 颜色条件：键=6 位十六进制色值，
    ///       - log: 命中颜色        #   值=命中步骤（写在色值键正下方或缩进 +2，
    ///       pos: [0.5123, 0.8456]  #   pos 之后不能跟同列 - 行——映射键之间不能插序列项），
    ///     else:                    #   pos 兄弟键=采样相对坐标（容差固定 30）
    ///       - log: 都没中
    /// 单遍判定不轮询、无 timeout（要"等出现再分支"用 until，要重试套 loop/goto）；
    /// threshold/region（含模板名 #后缀）只作用于模板条件（颜色条件坐标即位置）。
    #[async_recursion]
    async fn exec_cond(&self, ctx: &mut Ctx, step: &Value, v: &Value) -> anyhow::Result<()> {
        let seq = v
            .as_sequence()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("cond 需要条件列表（模板：- 模板名: 步骤；颜色：- ff8800: 步骤 + pos: [x, y]）"))?;
        if seq.is_empty() {
            anyhow::bail!("cond 至少需要一个条件");
        }
        // 预解析条件：全部校验集中在截图前（无设备也能报格式错）
        enum CondCase {
            Tpl(String, Vec<Value>),
            Clr(f64, f64, u8, u8, u8, Vec<Value>),
        }
        // 模板条件键撞保留字（如把 pos/then/else 当模板名）→ 报错防静默失效
        const RESERVED: [&str; 23] = [
            "wait", "log", "key", "text", "tap", "swipe", "until", "color", "cond", "loop", "call", "goto", "label",
            "str_app", "cls_app", "exit", "then", "else", "steps", "check", "before", "after", "pos",
        ];
        let mut cases = Vec::new();
        for (i, item) in seq.iter().enumerate() {
            let at = format!("cond 第 {} 个条件", i + 1);
            let m = item.as_mapping().ok_or_else(|| anyhow::anyhow!(
                "{} 需要映射：模板条件 - 模板名: 步骤；颜色条件 - ff8800: 步骤 + pos: [x, y]（条件键后的冒号必须写）", at))?;
            // 有 pos 兄弟键 = 颜色条件：除 pos 外恰好一个键 = 色值，值 = 命中步骤（可省略）
            if let Some(pos_v) = m.get(&Value::String("pos".into())) {
                if m.len() != 2 {
                    anyhow::bail!("{} 的颜色条件需要恰好一个色值键 + pos（收到 {} 个键）", at, m.len());
                }
                let (k, val) = m.iter().find(|(k, _)| k.as_str() != Some("pos")).unwrap();
                let (r, g, b) = Self::parse_color(k).map_err(|e| anyhow::anyhow!("{}: {}", at, e))?;
                let (rx, ry) = Self::parse_rel_coord(pos_v).map_err(|e| anyhow::anyhow!("{} 的 pos: {}", at, e))?;
                let steps = Self::opt_steps(val).map_err(|e| anyhow::anyhow!("{} 的色值键: {}", at, e))?;
                cases.push(CondCase::Clr(rx, ry, r, g, b, steps));
                continue;
            }
            // 单键字符串映射 = 模板条件（值为命中执行的步骤列表，可省略）
            if m.len() == 1 {
                let (k, val) = m.iter().next().unwrap();
                let name = k
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("{} 的键需要模板名字符串", at))?;
                if RESERVED.contains(&name.as_str()) {
                    anyhow::bail!("{} 的键 {} 是保留字（条件键应为模板名）", at, name);
                }
                if name.contains(',') {
                    anyhow::bail!("{} 只支持单个模板名（多个目标拆成多个条件项）", at);
                }
                // 键是合法色值但没有 pos → 疑似颜色条件漏写 pos，报错引导
                if Self::parse_color(k).is_ok() {
                    anyhow::bail!("{} 的键 {} 是色值：颜色条件需要 pos: [x, y] 兄弟键给采样坐标", at, name);
                }
                let steps = Self::opt_steps(val).map_err(|e| anyhow::anyhow!("{} {} 的值: {}", at, name, e))?;
                cases.push(CondCase::Tpl(name, steps));
                continue;
            }
            anyhow::bail!("{} 格式不对：模板条件=单键映射（- 模板名: 步骤列表），颜色条件=- 色值: 步骤列表 + pos: [x, y]", at);
        }
        let else_steps = step.get("else").and_then(|v| v.as_sequence()).cloned().unwrap_or_default();
        // 一张截图判定全部条件：模板按序 match_on_screen，颜色按序采样像素
        let screen = self.devices.screenshot(&ctx.device_id).await
            .map_err(|e| anyhow::anyhow!("截图失败: {}", e))?;
        let (w, h) = self.screen_size(ctx, &screen);
        if w == 0 || h == 0 {
            anyhow::bail!("无法获取屏幕尺寸");
        }
        let threshold = self.opt_f64(step, "cond", "threshold")
            .map(|x| x as f32)
            .unwrap_or(self.devices.cfg.default_threshold);
        // 只在有颜色条件时才解码像素（模板条件用不到 RGB）
        let rgb = if cases.iter().any(|c| matches!(c, CondCase::Clr(..))) {
            Some(image::load_from_memory(&screen).map_err(|e| anyhow::anyhow!("解析截图失败: {}", e))?.to_rgb8())
        } else {
            None
        };
        // 每通道容差固定 30（H.264 有损压缩帧间像素抖动，精确匹配实际不可用）
        const TOL: i32 = 30;
        for case in &cases {
            if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) || ctx.exit.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(());
            }
            match case {
                CondCase::Tpl(name, steps) => {
                    let region = self.resolve_region_for(ctx, step, name, w, h)?;
                    if let Some(mm) = self.match_on_screen(ctx, name, threshold, region, screen.clone()).await? {
                        self.emit(&ctx.device_id, ScriptEvent::Hit {
                            tpl: name.clone(), x: mm.x, y: mm.y, w: mm.width, h: mm.height, score: mm.score,
                        }).await;
                        ctx.log("success", format!("cond：命中模板 {} @ ({}, {})", name, mm.x, mm.y));
                        for sub in steps {
                            self.exec_step(ctx, sub).await?;
                        }
                        return Ok(());
                    }
                    ctx.log("debug", format!("cond：模板 {} 未命中", name));
                }
                CondCase::Clr(rx, ry, er, eg, eb, steps) => {
                    let img = rgb.as_ref().unwrap();
                    let px = ((rx * w as f64).round() as i64).clamp(0, w as i64 - 1) as u32;
                    let py = ((ry * h as f64).round() as i64).clamp(0, h as i64 - 1) as u32;
                    let p = img.get_pixel(px, py).0;
                    let (ar, ag, ab) = (p[0] as i32, p[1] as i32, p[2] as i32);
                    let exp = format!("{:02x}{:02x}{:02x}", er, eg, eb);
                    if (ar - *er as i32).abs() <= TOL && (ag - *eg as i32).abs() <= TOL && (ab - *eb as i32).abs() <= TOL {
                        ctx.log("success", format!("cond：颜色命中 {}（实际 {:02x}{:02x}{:02x}）@ 像素 ({}, {})", exp, ar, ag, ab, px, py));
                        self.emit(&ctx.device_id, ScriptEvent::Hit {
                            tpl: format!("clr {}", exp),
                            x: px.saturating_sub(12), y: py.saturating_sub(12), w: 24, h: 24, score: 1.0,
                        }).await;
                        for sub in steps {
                            self.exec_step(ctx, sub).await?;
                        }
                        return Ok(());
                    }
                    ctx.log("debug", format!("cond：颜色未命中：期望 {} 实际 {:02x}{:02x}{:02x} @ ({}, {})", exp, ar, ag, ab, px, py));
                }
            }
        }
        ctx.log("info", "cond：全部条件未命中，执行 else".to_string());
        for sub in &else_steps {
            self.exec_step(ctx, sub).await?;
        }
        Ok(())
    }

    /// 点击命中模板的中心并按 count 连击补点（check 障碍模板与主模板共用）：
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

    /// call 子脚本参数替换（2026-08-25）：把文档里全部 `$N`（N≥1，如 `$1`）
    /// 替换为实参。递归作用于所有字符串（映射键与值、列表项）——until/check
    /// 模板名、log 文本、call 行（嵌套转发 $N）等全部生效；`$` 后非数字保持
    /// 原样（"100$" 不受影响）。引用 `$N` 超出实参数量 → 报错（含 $N 占位的
    /// 脚本被直接运行、或 call 实参传少，都在此拦截）
    fn substitute_args(v: &mut Value, args: &[String]) -> anyhow::Result<()> {
        match v {
            Value::String(s) => *s = Self::substitute_str(s, args)?,
            Value::Sequence(seq) => {
                for item in seq.iter_mut() {
                    Self::substitute_args(item, args)?;
                }
            }
            Value::Mapping(m) => {
                // 键也要替换（如 color 检查点的 [x, y] 键），iter_mut 的键不可变 → 重建
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
                    "参数引用 ${} 超出实参数量（{} 个）：含 $N 占位的脚本需经 call 传参运行（参数从 $1 开始）",
                    digits, args.len()
                );
            };
            out.push_str(arg);
            rest = &after[digits.len()..];
        }
        out.push_str(rest);
        Ok(out)
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

    /// 解析 cond 颜色条件的色值键：6 位十六进制 RRGGBB（可带 # / 0x 前缀、
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

    /// 匹配单个模板一次（独立取最新截图，不重试）：解析 region 后在全屏/区域内匹配
    async fn match_one(&self, ctx: &Ctx, step: &Value, template: &str, threshold: f32) -> anyhow::Result<Option<matcher::MatchResult>> {
        let screen = self.devices.screenshot(&ctx.device_id).await
            .map_err(|e| anyhow::anyhow!("截图失败: {}", e))?;
        let (w, h) = self.screen_size(ctx, &screen);
        if w == 0 || h == 0 {
            anyhow::bail!("无法获取屏幕尺寸");
        }
        let region = self.resolve_region_for(ctx, step, template, w, h)?;
        self.match_on_screen(ctx, template, threshold, region, screen).await
    }

    /// region 解析（match_one 与 cond 的模板条件共用）：显式 region 参数
    /// （全部模板统一） > 模板名 #后缀（各自独立，见 tpl_region_from_name） > 全屏。
    /// 短名引用时 #后缀在**实际文件名**上（脚本写 login.png 引用
    /// login#910_159_972_716.png），区域须按解析结果取名才生效
    fn resolve_region_for(&self, ctx: &Ctx, step: &Value, template: &str, w: u32, h: u32) -> anyhow::Result<Option<[u32; 4]>> {
        match step.get("region") {
            Some(rv) => Self::parse_region(rv, w, h),
            None => Self::tpl_region_from_name(&Self::region_source_name(&self.tpl_dir_of(ctx), template), w, h),
        }
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

    /// cond 颜色条件的色值键解析：6 位十六进制（不带 #，宽容接受 # / 0x 前缀、
    /// 大小写）、[r, g, b] 数组、0x 整数；位数不对 / 非法字符 / 分量越界报错
    #[test]
    fn cond_color_key_parse() {
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

    /// call 传参替换（2026-08-25）：`$N`（N≥1）递归替换子脚本全部字符串键值
    /// （模板名/log 文本/call 行转发/cond 条件键）；`$` 后非数字原样保留；
    /// 引用越界报错。另证 YAML 裸标量 `@` 开头非法（保留字符）——
    /// 参数引用必须用 `$1` 不能用 `@1`
    #[test]
    fn call_args_substitution() {
        let args: Vec<String> = vec!["a.png".into(), "b.png".into()];
        let mut v = parse("steps:\n  - until: $1\n  - log: \"$2 和 $1\"\n  - call: other.yml $1 x.png\n  - cond:\n    - $1:\n        - log: x\n");
        Runner::substitute_args(&mut v, &args).unwrap();
        let steps = v.get("steps").unwrap().as_sequence().unwrap();
        assert_eq!(steps[0].get("until").and_then(|x| x.as_str()), Some("a.png"));
        assert_eq!(steps[1].get("log").and_then(|x| x.as_str()), Some("b.png 和 a.png"));
        // 嵌套 call 转发 $N（替换发生在子脚本加载时，转发值不再二次展开）
        assert_eq!(steps[2].get("call").and_then(|x| x.as_str()), Some("other.yml a.png x.png"));
        // cond 的模板条件键同样替换（substitute_args 连映射键一起替换）
        let cond = steps[3].get("cond").unwrap().as_sequence().unwrap();
        assert_eq!(cond[0].as_mapping().unwrap().iter().next().unwrap().0.as_str(), Some("a.png"));
        // `$` 后非数字原样保留（"100$" / "$涨"）
        let mut v2 = parse("steps:\n  - text: \"100$\"\n  - log: \"$涨\"\n");
        Runner::substitute_args(&mut v2, &args).unwrap();
        let s2 = v2.get("steps").unwrap().as_sequence().unwrap();
        assert_eq!(s2[0].get("text").and_then(|x| x.as_str()), Some("100$"));
        assert_eq!(s2[1].get("log").and_then(|x| x.as_str()), Some("$涨"));
        // 实参含 "$1" 不二次展开（替换值不重扫）
        let mut v3 = parse("steps:\n  - log: $1\n");
        Runner::substitute_args(&mut v3, &["$1".to_string()]).unwrap();
        assert_eq!(v3.get("steps").unwrap().as_sequence().unwrap()[0].get("log").and_then(|x| x.as_str()), Some("$1"));
        // 引用越界：未提供实参（主脚本直接运行）/ 序号超出
        let mut v4 = parse("steps:\n  - until: $1\n");
        assert!(Runner::substitute_args(&mut v4, &[]).is_err());
        let mut v5 = parse("steps:\n  - until: $3\n");
        assert!(Runner::substitute_args(&mut v5, &args).is_err());
        // $N 取最长数字串：$12 是第 12 参（不是 $1 + "2"）
        let mut v6 = parse("steps:\n  - log: $12\n");
        assert!(Runner::substitute_args(&mut v6, &args).is_err());
        // YAML 裸标量 @ 开头是保留字符，解析直接失败——参数引用必须用 $（不能用 @1）
        assert!(serde_yaml::from_str::<Value>("steps:\n  - until: @1").is_err());
        assert_eq!(parse("steps:\n  - until: $1").get("steps").unwrap().as_sequence().unwrap()[0].get("until").and_then(|x| x.as_str()), Some("$1"));
    }

    /// cond 条件分支的 YAML 结构（serde_yaml 与前端 js-yaml 同构）：
    /// 模板条件=单键映射（键=模板名，值=步骤列表）；颜色条件=色值字符串键 +
    /// pos 兄弟键（值=步骤列表，可留空）；else 是 cond 的**兄弟键**（与条件项
    /// 同列、不带 -，序列在非 dash 行自动收口回到步骤映射——同 `a:\n- 1\nb: 2` 机制）
    #[test]
    fn cond_syntax_parse() {
        let doc = parse(
            "steps:\n  - cond:\n    - test.png:\n        - log: tpl\n    - ff8800:\n        - log: clr\n      pos: [0.5, 0.5]\n    - aa8899:\n      pos: [0.2, 0.3]\n    else:\n      - log: none\n",
        );
        let step = &doc.get("steps").unwrap().as_sequence().unwrap()[0];
        let conds = step.get("cond").unwrap().as_sequence().unwrap();
        assert_eq!(conds.len(), 3);
        // 模板条件：单键映射
        let t = conds[0].as_mapping().unwrap();
        let (k, v) = t.iter().next().unwrap();
        assert_eq!(k.as_str(), Some("test.png"));
        assert!(v.as_sequence().is_some());
        // 颜色条件（带步骤）：色值键 + pos 兄弟键
        let c = conds[1].as_mapping().unwrap();
        assert!(c.get(&Value::String("ff8800".into())).unwrap().as_sequence().is_some());
        assert_eq!(c.get(&Value::String("pos".into())).unwrap().as_sequence().unwrap().len(), 2);
        // 颜色条件（步骤留空）：色值键值 null + pos
        let c2 = conds[2].as_mapping().unwrap();
        assert!(c2.get(&Value::String("aa8899".into())).unwrap().is_null());
        assert!(c2.get(&Value::String("pos".into())).unwrap().as_sequence().is_some());
        // else 是 cond 的兄弟键（不在 cond 序列里）
        assert_eq!(conds.iter().filter(|i| i.as_mapping().map_or(false, |m| m.contains_key(&Value::String("else".into())))).count(), 0);
        assert!(step.get("else").unwrap().as_sequence().is_some());
        // $N 替换覆盖 cond 的条件键（substitute_args 替换映射键）
        let mut d2 = parse("steps:\n  - cond:\n    - $1:\n        - log: x\n");
        Runner::substitute_args(&mut d2, &["a.png".into()]).unwrap();
        let s2 = d2.get("steps").unwrap().as_sequence().unwrap();
        let m2 = s2[0].get("cond").unwrap().as_sequence().unwrap()[0].as_mapping().unwrap();
        assert_eq!(m2.iter().next().unwrap().0.as_str(), Some("a.png"));
    }

    /// exec_step / exec_until 的解析期校验回归（2026-08-25 重构后）：
    /// 旧写法（find / click 简写 / 裸 check / until 的 before 障碍）显式报错引导
    /// 迁移；until 参数校验（timeout 必须 > 0、interval > 0、check 与主模板重复）
    /// 都在触碰设备/截图之前报错；合法 until 步骤解析通过、进入匹配循环后在截图处失败（无设备）
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
        assert!(run(&runner, &mut ctx, "- check: b.png").await.unwrap_err().to_string().contains("check 只能与"));
        // until 已删除参数（and_or / click / cnt_chk）显式报错
        assert!(run(&runner, &mut ctx, "- until: a.png\n  and_or: and").await.unwrap_err().to_string().contains("and_or 参数已删除"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  click: false").await.unwrap_err().to_string().contains("click 参数已删除"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  click: btn.png").await.unwrap_err().to_string().contains("click 参数已删除"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  cnt_chk: true").await.unwrap_err().to_string().contains("cnt_chk 已删除"));
        // until 障碍模板 before 已改名 check：字符串 / 模板名列表写法显式报错
        assert!(run(&runner, &mut ctx, "- until: a.png\n  before: b.png").await.unwrap_err().to_string().contains("已改名 check"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  before: [b.png, c.png]").await.unwrap_err().to_string().contains("已改名 check"));
        // until 参数校验（均在截图前报错，无需设备）
        assert!(run(&runner, &mut ctx, "- until: a.png\n  timeout: 0").await.unwrap_err().to_string().contains("timeout 必须 > 0"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  timeout: 0s").await.unwrap_err().to_string().contains("timeout 必须 > 0"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  interval: 0").await.unwrap_err().to_string().contains("interval 必须大于 0"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  interval: -1").await.unwrap_err().to_string().contains("interval"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  check: a.png").await.unwrap_err().to_string().contains("与 until 主模板重复"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  timeout: fast").await.unwrap_err().to_string().contains("带单位时长"));
        // 合法步骤：解析通过、进入循环，无设备在截图处失败（证明校验未误伤）；
        // before/after 步骤列表先于截图执行（log 无需设备）
        assert!(run(&runner, &mut ctx, "- until: a.png\n  timeout: 30min\n  interval: 500ms\n  check: b.png, c.png\n  img_ivl: 50ms").await.unwrap_err().to_string().contains("截图失败"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  before:\n    - log: pre\n  after:\n    - log: post\n  check: b.png").await.unwrap_err().to_string().contains("截图失败"));
        // until 的 before 值是数字等非列表 → 步骤列表报错（字符串值已被上面的旧写法守卫拦截）
        assert!(run(&runner, &mut ctx, "- until: a.png\n  before: 123").await.unwrap_err().to_string().contains("before 需要步骤列表"));
        // 保留参数 threshold/region/count/cnt_ivl 合法（同样走到截图才失败）
        assert!(run(&runner, &mut ctx, "- until: a.png\n  threshold: 0.9\n  region: l\n  count: 3\n  cnt_ivl: 80ms").await.unwrap_err().to_string().contains("截图失败"));
        // verify：布尔（默认 false 无逻辑）；true 合法、非布尔报错（均在截图前）
        assert!(run(&runner, &mut ctx, "- until: a.png\n  verify: true").await.unwrap_err().to_string().contains("截图失败"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  verify: false").await.unwrap_err().to_string().contains("截图失败"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  verify: 123").await.unwrap_err().to_string().contains("verify 需要 true / false"));
        assert!(run(&runner, &mut ctx, "- until: a.png\n  verify: yes请写true").await.unwrap_err().to_string().contains("verify 需要 true / false"));
        // color 动作已删除（颜色判断并入 cond），残留步骤显式报错
        assert!(run(&runner, &mut ctx, "- color:\n  check:\n    - [0.5, 0.5]: ff8800").await.unwrap_err().to_string().contains("color 已删除"));
        assert!(run(&runner, &mut ctx, "- color: [0.5, 0.5]").await.unwrap_err().to_string().contains("color 已删除"));
        // cond 条件分支：格式校验均在截图前报错，合法写法走到截图才失败
        assert!(run(&runner, &mut ctx, "- cond: x").await.unwrap_err().to_string().contains("cond 需要条件列表"));
        assert!(run(&runner, &mut ctx, "- cond: []").await.unwrap_err().to_string().contains("至少需要一个条件"));
        // 标量条件项（用户易写 `- test.png` 漏冒号——带子步骤时 YAML 直接解析失败，纯标量到这里报）
        assert!(run(&runner, &mut ctx, "- cond:\n  - a.png").await.unwrap_err().to_string().contains("冒号必须写"));
        // 模板条件值非步骤列表 / 保留字键 / 逗号多模板
        assert!(run(&runner, &mut ctx, "- cond:\n  - a.png: log x").await.unwrap_err().to_string().contains("步骤列表"));
        assert!(run(&runner, &mut ctx, "- cond:\n  - then:\n      - log: x").await.unwrap_err().to_string().contains("保留字"));
        assert!(run(&runner, &mut ctx, "- cond:\n  - a.png, b.png:\n      - log: x").await.unwrap_err().to_string().contains("单个模板名"));
        // 颜色条件：色值键是合法色值但缺 pos → 引导补 pos；pos 坐标越界；
        // 色值键非法；色值键的步骤值非列表
        assert!(run(&runner, &mut ctx, "- cond:\n  - ff8800:\n      - log: x").await.unwrap_err().to_string().contains("需要 pos"));
        assert!(run(&runner, &mut ctx, "- cond:\n  - ff8800:\n    pos: [1.5, 0.5]").await.unwrap_err().to_string().contains("0~1"));
        assert!(run(&runner, &mut ctx, "- cond:\n  - red:\n    pos: [0.5, 0.5]").await.unwrap_err().to_string().contains("cond 第 1 个条件"));
        assert!(run(&runner, &mut ctx, "- cond:\n  - ff8800: log x\n    pos: [0.5, 0.5]").await.unwrap_err().to_string().contains("步骤列表"));
        // 旧数组键颜色写法（- [x, y]: 色值，已删除）→ 键不是字符串报错
        assert!(run(&runner, &mut ctx, "- cond:\n  - [0.5, 0.5]: ff8800").await.unwrap_err().to_string().contains("模板名字符串"));
        // 合法 cond（模板 + 颜色 + else）→ 无设备在截图处失败
        assert!(run(&runner, &mut ctx, "- cond:\n  - a.png:\n      - log: tpl\n  - ff8800:\n      - log: clr\n    pos: [0.5, 0.5]\n  else:\n    - log: none").await.unwrap_err().to_string().contains("截图失败"));
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
