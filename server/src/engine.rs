//! YAML 自动化脚本引擎
//!
//! 支持动作：
//!   wait / log / key / text / tap / swipe /
//!   find(查找模板：interval 检测间隔默认 500ms；timeout 超时默认 6000ms（0=一直找）；
//!        click 支持 true / 模板名 / [x,y] 相对坐标，找到后的点击方式；threshold；region；
//!        then 找到后执行 / else 超时后执行) /
//!   loop / goto / label / call
//!
//! 每个操作（除 wait 动作本身）可用 wait 参数指定操作后的等待毫秒数，
//! 未指定时取脚本顶层 action_wait（如 `action_wait: 500`），脚本也未定义时默认 500ms
//!
//! 找图：截图（帧缓存优先）→ 模板匹配
//! region 支持 a/u/d/l/r/ul/ur/dl/dr 半区/四分之一区

use std::sync::Arc;
use std::time::Duration;

use async_recursion::async_recursion;
use image::GenericImageView;
use serde::Deserialize;
use serde_yaml::Value;
use tracing::warn;

use crate::device::DeviceManager;
use crate::matcher;
use crate::store::Db;

/// 脚本未定义顶层 action_wait 时，操作后的默认等待毫秒数
const DEFAULT_ACTION_WAIT: u64 = 500;

/// 运行器
pub struct Runner {
    pub db: Db,
    pub devices: Arc<DeviceManager>,
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
    pub log_cb: Option<Arc<dyn Fn(String, String) + Send + Sync>>,
}

impl Ctx {
    /// 记录日志：实时回调（如有）并同时收集到 ctx.log
    fn log(&mut self, level: &str, msg: String) {
        if let Some(cb) = &self.log_cb {
            cb(level.to_string(), msg.clone());
        }
        self.log.push((level.to_string(), msg));
    }
}

impl Runner {
    pub fn new(db: Db, devices: Arc<DeviceManager>) -> Self {
        Self { db, devices }
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

        let mut ctx = Ctx {
            device_id: device_id.to_string(),
            script_id: script_id.to_string(),
            label_index: std::collections::HashMap::new(),
            log: Vec::new(),
            stop,
            action_wait,
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
        // label 不执行
        if step.get("label").is_some() {
            return Ok(());
        }
        // 动作键（除 wait 外）：用于区分 `wait` 动作与操作级 `wait` 参数
        const ACTION_KEYS: [&str; 9] = [
            "log", "key", "text", "tap", "swipe", "find", "loop", "call", "goto",
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
            s.swipe(x1 as f32, y1 as f32, x2 as f32, y2 as f32, dur).await?;
        }
        if step.get("find").is_some() {
            self.exec_find(ctx, step).await?;
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
            let scripts = self.db.list_scripts()?;
            if let Some(s) = scripts.iter().find(|s| s.name == script_name) {
                ctx.log("debug", format!("调用子脚本 {}", script_name));
                let sub_log = self.run(&ctx.device_id, &s.id, &s.content, ctx.stop.clone(), ctx.log_cb.clone(), 0).await?;
                ctx.log.extend(sub_log);
            } else {
                anyhow::bail!("子脚本不存在: {}", script_name);
            }
        }
        // 操作后统一等待：除 wait 动作本身外，每个操作可用 wait 参数指定操作后的等待毫秒数，
        // 未指定时取脚本顶层 action_wait（脚本未定义时默认 500ms）
        if has_action {
            let wait_ms = step.get("wait").and_then(|x| x.as_u64()).unwrap_or(ctx.action_wait);
            if wait_ms > 0 {
                ctx.log("debug", format!("操作后等待 {}ms", wait_ms));
                tokio::time::sleep(Duration::from_millis(wait_ms)).await;
            }
        }
        Ok(())
    }

    /// find：循环查找模板（默认检测间隔 500ms、超时 6000ms，0=一直找），
    /// 找到后按 click 参数处理并执行 then，超时未找到执行 else
    #[async_recursion]
    async fn exec_find(&self, ctx: &mut Ctx, step: &Value) -> anyhow::Result<()> {
        let template = self.template_name(step, "find")?;
        let interval_ms = self.opt_u64(step, "find", "interval").unwrap_or(500);
        let timeout_ms = self.opt_u64(step, "find", "timeout").unwrap_or(6000);
        let threshold = self.opt_f64(step, "find", "threshold")
            .map(|x| x as f32)
            .unwrap_or(self.devices.cfg.default_threshold);
        let timeout_desc = if timeout_ms == 0 {
            "不超时（一直找）".to_string()
        } else {
            format!("{}ms", timeout_ms)
        };
        ctx.log("info", format!("查找模板 {}，超时 {}，检测间隔 {}ms", template, timeout_desc, interval_ms));
        let then_steps = self.opt_value(step, "find", "then").and_then(|v| v.as_sequence()).cloned().unwrap_or_default();
        let else_steps = self.opt_value(step, "find", "else").and_then(|v| v.as_sequence()).cloned().unwrap_or_default();
        let start = std::time::Instant::now();
        loop {
            if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            if timeout_ms > 0 && start.elapsed().as_millis() as u64 > timeout_ms {
                ctx.log("warn", format!("查找模板 {} 超时", template));
                for sub in &else_steps {
                    self.exec_step(ctx, sub).await?;
                }
                break;
            }
            if let Some(m) = self.find_once(ctx, step, threshold).await? {
                ctx.log("success", format!("模板 {} 已找到 @ ({}, {})", template, m.x, m.y));
                if self.exec_find_click(ctx, step, threshold, &m).await? {
                    // click 成功（或未配置 click）→ 执行 then 并结束
                    for sub in &then_steps {
                        self.exec_step(ctx, sub).await?;
                    }
                    break;
                }
                // click 目标（如模板区域内的按钮）尚未找到 → 继续循环
                ctx.log("debug", "click 目标未就绪，继续查找".to_string());
            }
            tokio::time::sleep(Duration::from_millis(interval_ms)).await;
        }
        Ok(())
    }

    /// 处理 find 的 click 参数，返回是否成功点击：
    ///   true            → 点击模板中心
    ///   false/未配置     → 不点击（视为成功，直接执行 then）
    ///   模板名           → 在模板区域内查找该模板，找到后点击其中心（未找到返回 false，继续循环）
    ///   [x, y]          → 点击模板区域内的相对坐标（0~1，如 [0.5, 0.5] = 中心）
    async fn exec_find_click(&self, ctx: &mut Ctx, step: &Value, threshold: f32, m: &matcher::MatchResult) -> anyhow::Result<bool> {
        let click = match step.get("click") {
            Some(v) => v,
            None => return Ok(true),
        };
        let s = self.devices.session(&ctx.device_id).ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
        let box_region = [m.x, m.y, m.width, m.height];
        if let Some(b) = click.as_bool() {
            if !b {
                return Ok(true);
            }
            let (cx, cy) = (m.x + m.width / 2, m.y + m.height / 2);
            ctx.log("success", format!("点击模板中心 @ ({}, {})", cx, cy));
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
            s.tap(cx as f32, cy as f32).await?;
            return Ok(true);
        } else {
            anyhow::bail!("click 只支持 true/false、模板名或 [x, y] 相对坐标");
        }
    }

    /// 执行一次 find 查找（不重试）：解析 region 后匹配，返回完整匹配结果
    async fn find_once(&self, ctx: &Ctx, step: &Value, threshold: f32) -> anyhow::Result<Option<matcher::MatchResult>> {
        let template = self.template_name(step, "find")?;
        let screen = self.devices.screenshot(&ctx.device_id).await
            .map_err(|e| anyhow::anyhow!("截图失败: {}", e))?;
        let (w, h) = self.screen_size(ctx, &screen);
        if w == 0 || h == 0 {
            anyhow::bail!("无法获取屏幕尺寸");
        }
        let region = match step.get("region") {
            Some(rv) => self.parse_region(rv, w, h)?,
            None => None,
        };
        self.match_on_screen(ctx, &template, threshold, region, screen).await
    }

    /// 在给定截图上匹配模板（region 为搜索区域，None=全屏）
    async fn match_on_screen(&self, ctx: &Ctx, template: &str, threshold: f32, region: Option<[u32; 4]>, screen: Vec<u8>) -> anyhow::Result<Option<matcher::MatchResult>> {
        let tpl_dir = self.devices.cfg.data_dir.join("templates");
        // 目录不存在时先创建，避免 std::fs::read 报“系统找不到指定的路径”
        let _ = std::fs::create_dir_all(&tpl_dir);
        let tpl_path = tpl_dir.join(template);
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

    /// 从步骤中取模板名：只支持 `find: shop.png` 字符串写法
    fn template_name(&self, step: &Value, key: &str) -> anyhow::Result<String> {
        let v = step.get(key).ok_or_else(|| anyhow::anyhow!("缺少 {}", key))?;
        v.as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("{} 只支持字符串模板写法，如 `{}: shop.png`", key, key))
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

    /// 解析 region：支持 a/u/d/l/r/ul/ur/dl/dr / [x1, y1, x2, y2] / {fm: [x,y], to: [x,y]}（0~1）
    fn parse_region(&self, v: &Value, w: u32, h: u32) -> anyhow::Result<Option<[u32; 4]>> {
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
            let (x1, y1) = self.parse_rel_coord(map.get("fm").ok_or_else(|| anyhow::anyhow!("region 缺少 fm"))?)?;
            let (x2, y2) = self.parse_rel_coord(map.get("to").ok_or_else(|| anyhow::anyhow!("region 缺少 to"))?)?;
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
    fn parse_rel_coord(&self, v: &Value) -> anyhow::Result<(f64, f64)> {
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
