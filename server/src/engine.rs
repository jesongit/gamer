//! YAML 自动化脚本引擎
//!
//! 支持动作：
//!   wait / find(+then/else) / click / swipe / text / key / start_app /
//!   loop / loop_until_find / if_find / goto / label / call / random_delay / log
//!
//! 找图：截图（帧缓存优先）→ 模板匹配 → 命中坐标存 @found 供后续 click 使用

use std::sync::Arc;
use std::time::Duration;

use async_recursion::async_recursion;
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
    pub found: Option<(u32, u32)>,
    pub label_index: std::collections::HashMap<String, usize>,
    pub log: Vec<(String, String)>, // (level, msg)
    pub stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Runner {
    pub fn new(db: Db, devices: Arc<DeviceManager>) -> Self {
        Self { db, devices }
    }

    /// 运行脚本内容（YAML 文本）
    pub async fn run(&self, device_id: &str, script_id: &str, content: &str, stop: Arc<std::sync::atomic::AtomicBool>) -> anyhow::Result<Vec<(String, String)>> {
        let doc: Value = serde_yaml::from_str(content)?;
        let steps = doc.get("steps").and_then(|v| v.as_sequence()).cloned().ok_or_else(|| anyhow::anyhow!("missing steps"))?;

        let mut ctx = Ctx {
            device_id: device_id.to_string(),
            script_id: script_id.to_string(),
            found: None,
            label_index: std::collections::HashMap::new(),
            log: Vec::new(),
            stop,
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
                ctx.log.push(("warn".into(), "脚本被停止".into()));
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
            let ms = v.as_u64().unwrap_or(0);
            ctx.log.push(("info".into(), format!("等待 {}ms", ms)));
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        if let Some(v) = step.get("random_delay") {
            let min = v.get("min").and_then(|x| x.as_u64()).unwrap_or(0);
            let max = v.get("max").and_then(|x| x.as_u64()).unwrap_or(0);
            let ms = if max > min { min + rand::random::<u64>() % (max - min) } else { min };
            ctx.log.push(("info".into(), format!("随机延时 {}ms", ms)));
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        if let Some(v) = step.get("log") {
            let msg = v.as_str().unwrap_or("");
            ctx.log.push(("info".into(), msg.to_string()));
        }
        if let Some(v) = step.get("key") {
            let key = v.as_str().unwrap_or("");
            let code = key_code(key);
            ctx.log.push(("info".into(), format!("按键 {}", key)));
            if let Some(s) = self.devices.session(&ctx.device_id) {
                s.press_key(code).await?;
            } else {
                anyhow::bail!("设备未连接");
            }
        }
        if let Some(v) = step.get("text") {
            let text = v.as_str().unwrap_or("");
            ctx.log.push(("info".into(), format!("输入文本 {}", text)));
            if let Some(s) = self.devices.session(&ctx.device_id) {
                s.inject_text(text).await?;
            } else {
                anyhow::bail!("设备未连接");
            }
        }
        if let Some(v) = step.get("start_app") {
            let app = if v.is_string() {
                v.as_str().unwrap().to_string()
            } else {
                v.get("package").and_then(|x| x.as_str()).unwrap_or("").to_string()
            };
            ctx.log.push(("info".into(), format!("启动应用 {}", app)));
            if let Some(s) = self.devices.session(&ctx.device_id) {
                s.start_app(&app).await?;
            } else {
                anyhow::bail!("设备未连接");
            }
        }
        if let Some(v) = step.get("click") {
            let (x, y) = self.resolve_point(ctx, v).await?;
            ctx.log.push(("info".into(), format!("点击 ({}, {})", x, y)));
            if let Some(s) = self.devices.session(&ctx.device_id) {
                s.tap(x as f32, y as f32).await?;
            } else {
                anyhow::bail!("设备未连接");
            }
        }
        if let Some(v) = step.get("swipe") {
            let from = v.get("from").and_then(|x| x.as_sequence()).cloned().unwrap_or_default();
            let to = v.get("to").and_then(|x| x.as_sequence()).cloned().unwrap_or_default();
            let dur = v.get("duration").and_then(|x| x.as_u64()).unwrap_or(500);
            let x1 = from.get(0).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            let y1 = from.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            let x2 = to.get(0).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            let y2 = to.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            ctx.log.push(("info".into(), format!("滑动 ({},{})→({},{}) {}ms", x1, y1, x2, y2, dur)));
            if let Some(s) = self.devices.session(&ctx.device_id) {
                s.swipe(x1, y1, x2, y2, dur).await?;
            } else {
                anyhow::bail!("设备未连接");
            }
        }
        if step.get("find").is_some() {
            let hit = self.find_once(ctx, step).await?;
            match hit {
                Some((x, y)) => {
                    ctx.found = Some((x, y));
                    ctx.log.push(("success".into(), format!("找到模板 @ ({}, {})", x, y)));
                    // then 子步骤
                    if let Some(then) = step.get("then").and_then(|v| v.as_sequence()) {
                        for sub in then {
                            self.exec_step(ctx, sub).await?;
                        }
                    }
                }
                None => {
                    ctx.log.push(("warn".into(), "未找到模板".into()));
                    if let Some(else_steps) = step.get("else").and_then(|v| v.as_sequence()) {
                        for sub in else_steps {
                            self.exec_step(ctx, sub).await?;
                        }
                    }
                }
            }
        }
        if step.get("if_find").is_some() {
            let hit = self.find_once(ctx, step).await?;
            if hit.is_some() {
                ctx.log.push(("success".into(), "条件命中".into()));
                if let Some(then) = step.get("then").and_then(|v| v.as_sequence()) {
                    for sub in then {
                        self.exec_step(ctx, sub).await?;
                    }
                }
            } else if let Some(else_steps) = step.get("else").and_then(|v| v.as_sequence()) {
                for sub in else_steps {
                    self.exec_step(ctx, sub).await?;
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
                ctx.log.push(("info".into(), format!("循环第 {}/{} 次", n + 1, times)));
                for sub in &sub_steps {
                    self.exec_step(ctx, sub).await?;
                }
            }
        }
        if let Some(v) = step.get("loop_until_find") {
            let timeout_ms = v.get("timeout").and_then(|x| x.as_u64()).unwrap_or(30000);
            let sub_steps = v.get("steps").and_then(|x| x.as_sequence()).cloned().unwrap_or_default();
            let start = std::time::Instant::now();
            loop {
                if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                if start.elapsed().as_millis() as u64 > timeout_ms {
                    ctx.log.push(("warn".into(), "loop_until_find 超时".into()));
                    break;
                }
                if self.find_once(ctx, step).await?.is_some() {
                    ctx.log.push(("success".into(), "loop_until_find 命中".into()));
                    break;
                }
                for sub in &sub_steps {
                    self.exec_step(ctx, sub).await?;
                }
            }
        }
        if let Some(v) = step.get("call") {
            let script_name = v.as_str().unwrap_or("");
            let scripts = self.db.list_scripts()?;
            if let Some(s) = scripts.iter().find(|s| s.name == script_name) {
                ctx.log.push(("info".into(), format!("调用子脚本 {}", script_name)));
                let sub_log = self.run(&ctx.device_id, &s.id, &s.content, ctx.stop.clone()).await?;
                ctx.log.extend(sub_log);
            } else {
                anyhow::bail!("子脚本不存在: {}", script_name);
            }
        }
        Ok(())
    }

    /// 执行一次找图，返回命中坐标（原始截图坐标系）
    async fn find_once(&self, ctx: &mut Ctx, step: &Value) -> anyhow::Result<Option<(u32, u32)>> {
        let v = step.get("find").or_else(|| step.get("if_find")).or_else(|| step.get("loop_until_find")).unwrap();
        let template = v.get("template").and_then(|x| x.as_str()).unwrap_or("");
        let threshold = v.get("threshold").and_then(|x| x.as_f64()).map(|x| x as f32);
        let timeout_ms = v.get("timeout").and_then(|x| x.as_u64()).unwrap_or(10000);
        let region: Option<[u32; 4]> = v.get("region").and_then(|r| {
            let seq = r.as_sequence()?;
            if seq.len() == 4 {
                Some([
                    seq[0].as_u64()? as u32,
                    seq[1].as_u64()? as u32,
                    seq[2].as_u64()? as u32,
                    seq[3].as_u64()? as u32,
                ])
            } else {
                None
            }
        });

        // 模板文件从数据目录加载
        let tpl_path = self.devices.cfg.data_dir.join("templates").join(template);
        let tpl_bytes = std::fs::read(&tpl_path)?;

        let start = std::time::Instant::now();
        let mut attempt = 0u32;
        loop {
            if ctx.stop.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(None);
            }
            if start.elapsed().as_millis() as u64 > timeout_ms {
                return Ok(None);
            }
            attempt += 1;
            let screen = self.devices.screenshot(&ctx.device_id).await?;
            let req = matcher::MatchRequest {
                screen_png: screen,
                template_png: tpl_bytes.clone(),
                threshold,
                region,
            };
            match matcher::match_template(&req) {
                Ok(Some(m)) => {
                    ctx.log.push(("info".into(), format!("匹配尝试 #{} 得分 {:.3}", attempt, m.score)));
                    return Ok(Some((m.x, m.y)));
                }
                Ok(None) => {
                    if attempt % 5 == 0 {
                        ctx.log.push(("info".into(), format!("匹配尝试 #{} 未命中", attempt)));
                    }
                }
                Err(e) => {
                    warn!("match error: {}", e);
                }
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    /// 解析点击坐标：数字 → 直接坐标；"@found" → 上次找图命中点
    async fn resolve_point(&self, ctx: &Ctx, v: &Value) -> anyhow::Result<(u32, u32)> {
        if let Some(s) = v.as_str() {
            if s == "@found" {
                return ctx.found.ok_or_else(|| anyhow::anyhow!("@found 无命中点，请先 find"));
            }
            anyhow::bail!("invalid click target: {}", s);
        }
        let x = v.get("x").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        let y = v.get("y").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        Ok((x, y))
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
