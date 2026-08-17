//! YAML 自动化脚本引擎
//!
//! 支持动作：
//!   wait / log / key / text / tap / swipe /
//!   click(模板点击) / find(+then/else) / until(+else) /
//!   loop / goto / label / call
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
    pub async fn run(
        &self,
        device_id: &str,
        script_id: &str,
        content: &str,
        stop: Arc<std::sync::atomic::AtomicBool>,
        log_cb: Option<Arc<dyn Fn(String, String) + Send + Sync>>,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let doc: Value = serde_yaml::from_str(content)?;
        let steps = doc.get("steps").and_then(|v| v.as_sequence()).cloned().ok_or_else(|| anyhow::anyhow!("missing steps"))?;

        let mut ctx = Ctx {
            device_id: device_id.to_string(),
            script_id: script_id.to_string(),
            label_index: std::collections::HashMap::new(),
            log: Vec::new(),
            stop,
            log_cb,
        };

        // 预扫描 label
        for (i, step) in steps.iter().enumerate() {
            if let Some(lbl) = step.get("label").and_then(|v| v.as_str()) {
                ctx.label_index.insert(lbl.to_string(), i);
            }
        }

        let mut i = 0usize;
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
        if let Some(v) = step.get("wait") {
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
            let from = v.get("from").cloned().unwrap_or(Value::Null);
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
        if step.get("click").is_some() {
            let template = self.template_name(step, "click")?;
            let hit = self.find_once(ctx, step, "click").await?;
            match hit {
                Some(m) => {
                    let cx = m.x + m.width / 2;
                    let cy = m.y + m.height / 2;
                    let msg = step.get("log")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("点击模板 {} 成功 @ ({}, {})", template, cx, cy));
                    ctx.log("success", msg);
                    let s = self.devices.session(&ctx.device_id).ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
                    s.tap(cx as f32, cy as f32).await?;
                }
                None => {
                    if let Some(else_steps) = step.get("else").and_then(|v| v.as_sequence()) {
                        for sub in else_steps {
                            self.exec_step(ctx, sub).await?;
                        }
                    } else {
                        ctx.log("warn", format!("点击模板 {} 失败", template));
                    }
                }
            }
        }
        if step.get("find").is_some() {
            let template = self.template_name(step, "find")?;
            let hit = self.find_once(ctx, step, "find").await?;
            match hit {
                Some(m) => {
                    ctx.log("success", format!("找到模板 {} @ ({}, {})", template, m.x, m.y));
                    if let Some(then) = step.get("then").and_then(|v| v.as_sequence()) {
                        for sub in then {
                            self.exec_step(ctx, sub).await?;
                        }
                    }
                }
                None => {
                    ctx.log("warn", format!("未找到模板 {}", template));
                    if let Some(else_steps) = step.get("else").and_then(|v| v.as_sequence()) {
                        for sub in else_steps {
                            self.exec_step(ctx, sub).await?;
                        }
                    }
                }
            }
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
        if step.get("until").is_some() {
            self.exec_until(ctx, step, "until", 0).await?;
        }
        if let Some(v) = step.get("call") {
            let script_name = v.as_str().unwrap_or("");
            let scripts = self.db.list_scripts()?;
            if let Some(s) = scripts.iter().find(|s| s.name == script_name) {
                ctx.log("debug", format!("调用子脚本 {}", script_name));
                let sub_log = self.run(&ctx.device_id, &s.id, &s.content, ctx.stop.clone(), ctx.log_cb.clone()).await?;
                ctx.log.extend(sub_log);
            } else {
                anyhow::bail!("子脚本不存在: {}", script_name);
            }
        }
        Ok(())
    }

    /// until：循环等待模板出现，超时后执行 else
    #[async_recursion]
    async fn exec_until(&self, ctx: &mut Ctx, step: &Value, key: &str, default_timeout: u64) -> anyhow::Result<()> {
        let template = self.template_name(step, key)?;
        let timeout_ms = self.opt_u64(step, key, "timeout").unwrap_or(default_timeout);
        let timeout_desc = if timeout_ms == 0 {
            "不超时（死等）".to_string()
        } else {
            format!("{}ms", timeout_ms)
        };
        ctx.log("info", format!("等待模板 {} 出现，超时 {}", template, timeout_desc));
        let sub_steps = self.opt_value(step, key, "steps").and_then(|x| x.as_sequence()).cloned().unwrap_or_default();
        let start = std::time::Instant::now();
        loop {
            if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            if timeout_ms > 0 && start.elapsed().as_millis() as u64 > timeout_ms {
                ctx.log("warn", format!("等待模板 {} 超时", template));
                if let Some(else_steps) = self.opt_value(step, key, "else").and_then(|v| v.as_sequence()) {
                    for sub in else_steps {
                        self.exec_step(ctx, sub).await?;
                    }
                }
                break;
            }
            if let Some(m) = self.find_once(ctx, step, key).await? {
                ctx.log("success", format!("模板 {} 已出现 @ ({}, {})", template, m.x, m.y));
                break;
            }
            for sub in &sub_steps {
                self.exec_step(ctx, sub).await?;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        Ok(())
    }

    /// 执行一次找图（不重试），返回完整匹配结果
    async fn find_once(&self, ctx: &mut Ctx, step: &Value, key: &str) -> anyhow::Result<Option<matcher::MatchResult>> {
        let template = self.template_name(step, key)?;
        let threshold = self.opt_f64(step, key, "threshold")
            .map(|x| x as f32)
            .or(Some(self.devices.cfg.default_threshold));
        let region_value = self.opt_value(step, key, "region").cloned();

        let tpl_dir = self.devices.cfg.data_dir.join("templates");
        // 目录不存在时先创建，避免 std::fs::read 报“系统找不到指定的路径”
        let _ = std::fs::create_dir_all(&tpl_dir);
        let tpl_path = tpl_dir.join(&template);
        let tpl_bytes = std::fs::read(&tpl_path)
            .map_err(|e| anyhow::anyhow!("读取模板 {} 失败: {} (path={})", template, e, tpl_path.display()))?;

        let screen = self.devices.screenshot(&ctx.device_id).await
            .map_err(|e| anyhow::anyhow!("截图失败: {}", e))?;
        let (w, h) = self.screen_size(ctx, &screen);
        if w == 0 || h == 0 {
            anyhow::bail!("无法获取屏幕尺寸");
        }
        let region = match region_value {
            Some(rv) => self.parse_region(&rv, w, h)?,
            None => None,
        };
        let req = matcher::MatchRequest {
            screen_png: screen,
            template_png: tpl_bytes,
            threshold,
            region,
        };
        matcher::match_template(&req).map_err(|e| anyhow::anyhow!("模板匹配失败: {}", e))
    }

    /// 从步骤中取模板名：只支持 `find: shop.png` / `click: shop.png` / `until: shop.png` 字符串写法
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

    /// 解析 region：支持 a/u/d/l/r/ul/ur/dl/dr 或相对坐标 [x1, y1, x2, y2]（0~1）
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
        anyhow::bail!("region 只支持 a/u/d/l/r/ul/ur/dl/dr 或 [x1, y1, x2, y2]")
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
