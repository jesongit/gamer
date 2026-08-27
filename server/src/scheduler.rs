//! 定时任务调度：cron 表达式 + tokio 后台调度（Docker 内 7×24 运行）

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, TimeZone};
use cron::Schedule;
use std::str::FromStr;

use tracing::{error, info, warn};

use crate::device::DeviceManager;
use crate::engine::Runner;
use crate::scripts::ScriptStore;
use crate::store::{Db, Task};

/// 将 5 字段标准 cron（分 时 日 月 周）规范化为 cron crate 的 7 字段格式
pub fn normalize_cron(expr: &str) -> String {
    let expr = expr.trim();
    if expr.starts_with('@') {
        return expr.to_string(); // @daily/@hourly 等
    }
    let parts: Vec<&str> = expr.split_whitespace().collect();
    match parts.len() {
        5 => format!("0 {} *", expr), // 秒=0，年=*
        6 => format!("0 {}", expr),   // 秒=0
        _ => expr.to_string(),
    }
}

/// 校验 cron 表达式（5/6/7 字段均可）
pub fn validate_cron(expr: &str) -> bool {
    Schedule::from_str(&normalize_cron(expr)).is_ok()
}

pub struct Scheduler {
    db: Db,
    devices: Arc<DeviceManager>,
    runner: Arc<Runner>,
    /// 脚本文件存储：任务运行时按 script_id（package/name）取脚本内容
    scripts: Arc<ScriptStore>,
    /// task_id -> (是否运行中, 上次处理的触发时刻)
    running:
        Arc<tokio::sync::Mutex<std::collections::HashMap<String, (bool, Option<DateTime<Local>>)>>>,
}

impl Scheduler {
    pub fn new(
        db: Db,
        devices: Arc<DeviceManager>,
        viewers: crate::webrtc::ViewerMap,
        scripts: Arc<ScriptStore>,
    ) -> Self {
        let runner = Arc::new(Runner::new(devices.clone(), viewers, scripts.clone()));
        Self {
            db,
            devices,
            runner,
            scripts,
            running: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// 启动调度循环：每 10s 扫描一次所有启用任务
    pub async fn start(&self) {
        let db = self.db.clone();
        let devices = self.devices.clone();
        let runner = self.runner.clone();
        let scripts = self.scripts.clone();
        let running = self.running.clone();
        info!("scheduler started");
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(10));
            loop {
                tick.tick().await;
                let tasks = match db.list_tasks() {
                    Ok(t) => t,
                    Err(e) => {
                        error!("list tasks failed: {}", e);
                        continue;
                    }
                };
                let now = Local::now();
                for task in tasks {
                    if !task.enabled {
                        continue;
                    }
                    // 计算上一个触发点（应执行但尚未执行）
                    let trigger = match Schedule::from_str(&normalize_cron(&task.cron)) {
                        Ok(sched) => {
                            let prev = now - Duration::from_secs(3600);
                            sched.after(&prev).next().filter(|t| *t <= now)
                        }
                        Err(e) => {
                            warn!("invalid cron {}: {}", task.cron, e);
                            None
                        }
                    };
                    if let Some(trigger_time) = trigger {
                        let mut map = running.lock().await;
                        let entry = map.entry(task.id.clone()).or_insert((false, None));
                        // 已运行过该触发点则跳过
                        if entry.1.map(|t| t >= trigger_time).unwrap_or(false) {
                            continue;
                        }
                        if entry.0 {
                            continue; // 正在运行
                        }
                        entry.0 = true;
                        entry.1 = Some(trigger_time);
                        drop(map);
                        let runner2 = runner.clone();
                        let devices2 = devices.clone();
                        let db2 = db.clone();
                        let scripts2 = scripts.clone();
                        let task2 = task.clone();
                        let running2 = running.clone();
                        tokio::spawn(async move {
                            info!(task = %task2.name, "scheduled run triggered");
                            let result =
                                run_task(&runner2, &devices2, &db2, &scripts2, &task2).await;
                            let _ = result;
                            // 只复位运行标志，保留 entry（含已处理的触发时刻）：
                            // 若直接 remove，下个 tick 会把同一触发点再执行一次（任务每 10s 重复触发的 bug）
                            let mut map = running2.lock().await;
                            if let Some(entry) = map.get_mut(&task2.id) {
                                entry.0 = false;
                            }
                        });
                    }
                }
            }
        });
    }

    /// 立即运行任务（手动触发）
    pub async fn run_now(&self, task: &Task) {
        info!(task = %task.name, "manual trigger");
        let _ = run_task(&self.runner, &self.devices, &self.db, &self.scripts, task).await;
    }
}

async fn run_task(
    runner: &Arc<Runner>,
    devices: &Arc<DeviceManager>,
    db: &Db,
    scripts: &Arc<ScriptStore>,
    task: &Task,
) -> anyhow::Result<()> {
    let script = scripts
        .get(&task.script_id)?
        .ok_or_else(|| anyhow::anyhow!("script not found: {}", task.script_id))?;

    // 确保设备在线
    if devices.session(&task.device_id).is_none() {
        devices.connect_device(&task.device_id).await?;
    }

    // 设备运行计数（空闲低功耗守卫；空闲拆会话/关屏由 idle_power_loop 统一管理）
    devices.run_begin(&task.device_id);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let logs = runner
        .run(
            &task.device_id,
            &task.script_id,
            &script.content,
            stop,
            None,
            0,
            None,
            None,
            vec![],
        )
        .await;
    devices.run_end(&task.device_id);
    match logs {
        Ok(entries) => {
            let success = entries.iter().any(|(l, _)| l == "error");
            for (level, msg) in &entries {
                let _ = db.add_log(&task.device_id, &task.script_id, level, msg);
            }
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let mut t = db
                .list_tasks()?
                .into_iter()
                .find(|x| x.id == task.id)
                .unwrap_or_else(|| task.clone());
            t.last_result = Some(if success { "失败" } else { "成功" }.to_string());
            t.last_run_at = Some(now);
            db.upsert_task(&t)?;
            Ok(())
        }
        Err(e) => {
            let _ = db.add_log(
                &task.device_id,
                &task.script_id,
                "error",
                &format!("任务执行失败: {}", e),
            );
            let mut t = task.clone();
            t.last_result = Some("失败".into());
            t.last_run_at = Some(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
            db.upsert_task(&t)?;
            Err(e)
        }
    }
}

/// 计算 cron 下次执行时间（用于 API 预览）
pub fn next_run(cron_expr: &str) -> Option<DateTime<Local>> {
    let sched = Schedule::from_str(&normalize_cron(cron_expr)).ok()?;
    sched
        .after(&Local::now())
        .next()
        .map(|t| Local.timestamp_opt(t.timestamp(), 0).unwrap())
}
