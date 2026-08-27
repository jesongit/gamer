//! SQLite 持久化：设备、定时任务、运行日志
//! （脚本已改为文件系统存储 data/scripts/<package>/，见 scripts.rs；scripts 表仅留迁移读取）

use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ScreenMode {
    /// 镜像主屏
    Mirror,
    /// 虚拟屏（统一分辨率）
    Virtual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// 接入类型: redroid / usb / wifi / emu
    pub kind: String,
    /// adb 地址（usb 为空）
    pub addr: String,
    pub screen_mode: ScreenMode,
    /// 虚拟屏分辨率 WxH（如 1920x1080）
    pub vd_res: Option<String>,
    /// 虚拟屏 DPI（0 = 自动）
    pub vd_dpi: Option<u32>,
    /// 连接后自动启动的游戏包名
    pub pkg: Option<String>,
    /// 视频帧率上限（None = 跟随全局配置 / 自动）
    pub fps: Option<u32>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub cron: String,
    pub script_id: String,
    pub device_id: String,
    pub enabled: bool,
    pub last_result: Option<String>,
    pub last_run_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: i64,
    pub time: String,
    pub device_id: String,
    pub script_id: String,
    pub level: String,
    pub msg: String,
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(cfg: &Config) -> anyhow::Result<Self> {
        let path = cfg.data_dir.join("gamer.db");
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS devices (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                addr TEXT NOT NULL DEFAULT '',
                screen_mode TEXT NOT NULL DEFAULT 'mirror',
                vd_res TEXT,
                vd_dpi INTEGER,
                pkg TEXT,
                fps INTEGER,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                cron TEXT NOT NULL,
                script_id TEXT NOT NULL,
                device_id TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                last_result TEXT,
                last_run_at TEXT,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                time TEXT NOT NULL,
                device_id TEXT NOT NULL,
                script_id TEXT NOT NULL,
                level TEXT NOT NULL,
                msg TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_logs_time ON logs(time DESC);
            "#,
        )?;
        // 旧库迁移：devices 表可能缺 fps 列
        let has_fps = conn
            .prepare("PRAGMA table_info(devices)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|c| c == "fps");
        if !has_fps {
            conn.execute("ALTER TABLE devices ADD COLUMN fps INTEGER", [])?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }

    // ---------- 设备 ----------

    pub fn list_devices(&self) -> anyhow::Result<Vec<Device>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, kind, addr, screen_mode, vd_res, vd_dpi, pkg, fps, created_at FROM devices ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Device {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                addr: r.get(3)?,
                screen_mode: match r.get::<_, String>(4)?.as_str() {
                    "virtual" => ScreenMode::Virtual,
                    _ => ScreenMode::Mirror,
                },
                vd_res: r.get(5)?,
                vd_dpi: r.get(6)?,
                pkg: r.get(7)?,
                fps: r.get(8)?,
                created_at: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_device(&self, id: &str) -> anyhow::Result<Option<Device>> {
        Ok(self.list_devices()?.into_iter().find(|d| d.id == id))
    }

    pub fn upsert_device(&self, d: &Device) -> anyhow::Result<()> {
        let conn = self.lock();
        conn.execute(
            r#"INSERT INTO devices (id, name, kind, addr, screen_mode, vd_res, vd_dpi, pkg, fps, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
               ON CONFLICT(id) DO UPDATE SET
                 name=?2, kind=?3, addr=?4, screen_mode=?5, vd_res=?6, vd_dpi=?7, pkg=?8, fps=?9"#,
            rusqlite::params![
                d.id,
                d.name,
                d.kind,
                d.addr,
                match d.screen_mode {
                    ScreenMode::Mirror => "mirror",
                    ScreenMode::Virtual => "virtual",
                },
                d.vd_res,
                d.vd_dpi,
                d.pkg,
                d.fps,
                d.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn delete_device(&self, id: &str) -> anyhow::Result<()> {
        self.lock()
            .execute("DELETE FROM devices WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---------- 定时任务 ----------

    pub fn list_tasks(&self) -> anyhow::Result<Vec<Task>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, cron, script_id, device_id, enabled, last_result, last_run_at, created_at FROM tasks ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Task {
                id: r.get(0)?,
                name: r.get(1)?,
                cron: r.get(2)?,
                script_id: r.get(3)?,
                device_id: r.get(4)?,
                enabled: r.get::<_, i64>(5)? != 0,
                last_result: r.get(6)?,
                last_run_at: r.get(7)?,
                created_at: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn upsert_task(&self, t: &Task) -> anyhow::Result<()> {
        self.lock().execute(
            r#"INSERT INTO tasks (id, name, cron, script_id, device_id, enabled, last_result, last_run_at, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
               ON CONFLICT(id) DO UPDATE SET
                 name=?2, cron=?3, script_id=?4, device_id=?5, enabled=?6, last_result=?7, last_run_at=?8"#,
            rusqlite::params![
                t.id, t.name, t.cron, t.script_id, t.device_id,
                if t.enabled { 1 } else { 0 },
                t.last_result, t.last_run_at, t.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn delete_task(&self, id: &str) -> anyhow::Result<()> {
        self.lock()
            .execute("DELETE FROM tasks WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---------- 日志 ----------

    pub fn add_log(
        &self,
        device_id: &str,
        script_id: &str,
        level: &str,
        msg: &str,
    ) -> anyhow::Result<()> {
        let now = chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string();
        self.lock().execute(
            "INSERT INTO logs (time, device_id, script_id, level, msg) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![now, device_id, script_id, level, msg],
        )?;
        Ok(())
    }

    pub fn list_logs(
        &self,
        device_id: Option<&str>,
        level: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<LogEntry>> {
        let conn = self.lock();
        let mut sql = String::from("SELECT id, time, device_id, script_id, level, msg FROM logs");
        let mut conds = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(d) = device_id {
            conds.push("device_id = ?".to_string());
            params.push(Box::new(d.to_string()));
        }
        if let Some(l) = level {
            conds.push("level = ?".to_string());
            params.push(Box::new(l.to_string()));
        }
        if !conds.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");
        params.push(Box::new(limit));
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
            |r| {
                Ok(LogEntry {
                    id: r.get(0)?,
                    time: r.get(1)?,
                    device_id: r.get(2)?,
                    script_id: r.get(3)?,
                    level: r.get(4)?,
                    msg: r.get(5)?,
                })
            },
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn clear_logs(&self) -> anyhow::Result<()> {
        self.lock().execute("DELETE FROM logs", [])?;
        Ok(())
    }
}

pub type Db = Arc<Store>;
