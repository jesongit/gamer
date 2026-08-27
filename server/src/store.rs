//! SQLite 持久化：设备、定时任务、运行日志
//! （脚本已改为文件系统存储 data/scripts/<package>/，见 scripts.rs；scripts 表仅留迁移读取）

use std::sync::{Arc, Mutex};

use chrono::{Local, Utc};
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

/// 低基数数据库指标快照，供 `/metrics` 暴露；不包含用户输入或路径标签。
#[derive(Debug, Clone, Copy, Default)]
pub struct StoreMetrics {
    pub devices: i64,
    pub tasks: i64,
    pub logs: i64,
    pub scheduled_runs: i64,
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(cfg: &Config) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&cfg.data_dir)?;
        let path = cfg.data_dir.join("gamer.db");
        let conn = Connection::open(&path)?;
        // 单连接 Mutex 仍保留以兼容现有调用方；这些连接级 PRAGMA 让未来
        // 增加读连接时也具备一致的锁等待、崩溃恢复和外键约束语义。
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\nPRAGMA synchronous = NORMAL;\nPRAGMA foreign_keys = ON;",
        )?;
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
        // 定时触发记录用于跨 tick/重启幂等。先建表，再清理历史实现可能留下的
        // 重复行，最后建立唯一索引；这样旧库没有 scheduled_runs 表时可直接升级，
        // 已有重复数据时也不会因 CREATE UNIQUE INDEX 失败而无法启动。
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS scheduled_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                scheduled_at INTEGER NOT NULL,
                state TEXT NOT NULL DEFAULT 'running',
                run_id TEXT,
                error TEXT,
                created_at TEXT NOT NULL
            );
            DELETE FROM scheduled_runs
             WHERE id NOT IN (
                 SELECT MIN(id) FROM scheduled_runs GROUP BY task_id, scheduled_at
             );
            CREATE UNIQUE INDEX IF NOT EXISTS uq_scheduled_runs_task_time
                ON scheduled_runs(task_id, scheduled_at);
            CREATE INDEX IF NOT EXISTS idx_scheduled_runs_created_at
                ON scheduled_runs(created_at DESC);
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
        let store = Self {
            conn: Mutex::new(conn),
        };
        if cfg.log_retain_days > 0 {
            if let Err(e) = store.prune_logs(cfg.log_retain_days) {
                tracing::warn!(error = %e, "启动时清理过期运行日志失败");
            }
        }
        Ok(store)
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
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, kind, addr, screen_mode, vd_res, vd_dpi, pkg, fps, created_at\
             FROM devices WHERE id = ?1",
        )?;
        match stmt.query_row([id], |r| {
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
        }) {
            Ok(device) => Ok(Some(device)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
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

    // ---------- 定时触发幂等记录 ----------

    /// 原子领取一个计划触发点。返回 true 表示本次调用取得执行权；false 表示
    /// 该 `(task_id, scheduled_at)` 已由本进程或此前的进程领取。
    pub fn claim_scheduled_run(&self, task_id: &str, scheduled_at: i64) -> anyhow::Result<bool> {
        let created_at = Utc::now().to_rfc3339();
        let changed = self.lock().execute(
            r#"INSERT INTO scheduled_runs
                   (task_id, scheduled_at, state, created_at)
               VALUES (?1, ?2, 'running', ?3)
               ON CONFLICT(task_id, scheduled_at) DO NOTHING"#,
            rusqlite::params![task_id, scheduled_at, created_at],
        )?;
        Ok(changed == 1)
    }

    /// 更新计划触发点的终态或提交结果。未知记录按 no-op 处理，便于服务异常恢复
    /// 时完成钩子与调度错误路径保持幂等。
    pub fn finish_scheduled_run(
        &self,
        task_id: &str,
        scheduled_at: i64,
        state: &str,
        run_id: Option<&str>,
        error: Option<&str>,
    ) -> anyhow::Result<bool> {
        let changed = self.lock().execute(
            r#"UPDATE scheduled_runs
                  SET state = ?3, run_id = COALESCE(?4, run_id), error = ?5
                WHERE task_id = ?1 AND scheduled_at = ?2 AND state = 'running'"#,
            rusqlite::params![task_id, scheduled_at, state, run_id, error],
        )?;
        Ok(changed == 1)
    }

    #[cfg(test)]
    fn scheduled_run_count(&self, task_id: &str, scheduled_at: i64) -> i64 {
        self.lock()
            .query_row(
                "SELECT COUNT(*) FROM scheduled_runs WHERE task_id = ?1 AND scheduled_at = ?2",
                rusqlite::params![task_id, scheduled_at],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[cfg(test)]
    fn scheduled_run_state(&self, task_id: &str, scheduled_at: i64) -> String {
        self.lock()
            .query_row(
                "SELECT state FROM scheduled_runs WHERE task_id = ?1 AND scheduled_at = ?2",
                rusqlite::params![task_id, scheduled_at],
                |r| r.get(0),
            )
            .unwrap()
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

    /// 运行健康探测：只做一个极轻量的数据库 round-trip，不暴露底层错误给 HTTP 客户端。
    pub fn health_check(&self) -> anyhow::Result<()> {
        let conn = self.lock();
        conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?;
        Ok(())
    }

    /// 获取低基数行数指标。表名是代码内固定值，查询不接受外部输入。
    pub fn metrics_snapshot(&self) -> anyhow::Result<StoreMetrics> {
        let conn = self.lock();
        Ok(conn.query_row(
            "SELECT\
                (SELECT COUNT(*) FROM devices),\
                (SELECT COUNT(*) FROM tasks),\
                (SELECT COUNT(*) FROM logs),\
                (SELECT COUNT(*) FROM scheduled_runs)",
            [],
            |r| {
                Ok(StoreMetrics {
                    devices: r.get(0)?,
                    tasks: r.get(1)?,
                    logs: r.get(2)?,
                    scheduled_runs: r.get(3)?,
                })
            },
        )?)
    }

    /// 分批删除过期日志，避免一次大事务长时间占用数据库锁。
    /// 返回本次删除的行数；retain_days=0 表示关闭保留清理。
    pub fn prune_logs(&self, retain_days: u32) -> anyhow::Result<u64> {
        if retain_days == 0 {
            return Ok(0);
        }
        let cutoff = (Local::now() - chrono::Duration::days(retain_days as i64))
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string();
        let conn = self.lock();
        let mut total = 0u64;
        loop {
            let deleted = conn.execute(
                "DELETE FROM logs WHERE id IN (\
                    SELECT id FROM logs WHERE time < ?1 ORDER BY id LIMIT 500)",
                [cutoff.as_str()],
            )?;
            total += deleted as u64;
            if deleted < 500 {
                break;
            }
        }
        Ok(total)
    }
}

pub type Db = Arc<Store>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_config(name: &str) -> (Config, PathBuf) {
        let dir = std::env::temp_dir().join(format!("gamer-store-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let mut cfg = Config::default();
        cfg.data_dir = dir.clone();
        (cfg, dir)
    }

    #[test]
    fn scheduled_claim_is_idempotent() {
        let (cfg, dir) = temp_config("claim");
        let store = Store::open(&cfg).unwrap();
        assert!(store.claim_scheduled_run("task", 1_700_000_000).unwrap());
        assert!(!store.claim_scheduled_run("task", 1_700_000_000).unwrap());
        assert!(store.claim_scheduled_run("task", 1_700_000_001).unwrap());
        assert_eq!(store.scheduled_run_count("task", 1_700_000_000), 1);
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn scheduled_unique_index_migrates_duplicate_legacy_rows() {
        let (cfg, dir) = temp_config("migration");
        let db_path = dir.join("gamer.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE scheduled_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                scheduled_at INTEGER NOT NULL,
                state TEXT NOT NULL,
                run_id TEXT,
                error TEXT,
                created_at TEXT NOT NULL
            );
            INSERT INTO scheduled_runs(task_id, scheduled_at, state, created_at)
                VALUES ('task', 42, 'running', '2026-01-01T00:00:00Z');
            INSERT INTO scheduled_runs(task_id, scheduled_at, state, created_at)
                VALUES ('task', 42, 'running', '2026-01-01T00:00:01Z');",
        )
        .unwrap();
        drop(conn);

        let store = Store::open(&cfg).unwrap();
        assert_eq!(store.scheduled_run_count("task", 42), 1);
        assert!(!store.claim_scheduled_run("task", 42).unwrap());
        assert!(store
            .finish_scheduled_run("task", 42, "success", Some("run-1"), None)
            .unwrap());
        assert!(!store
            .finish_scheduled_run("task", 42, "failed", Some("run-2"), Some("late"))
            .unwrap());
        assert_eq!(store.scheduled_run_state("task", 42), "success");
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sqlite_connection_uses_reliable_pragmas() {
        let (cfg, dir) = temp_config("pragmas");
        let store = Store::open(&cfg).unwrap();
        let conn = store.lock();
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        let busy_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        let foreign_keys: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal.to_ascii_lowercase(), "wal");
        assert_eq!(busy_timeout, 5_000);
        assert_eq!(foreign_keys, 1);
        drop(conn);
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn metrics_snapshot_and_log_retention_are_bounded() {
        let (cfg, dir) = temp_config("metrics-retention");
        let store = Store::open(&cfg).unwrap();
        store
            .lock()
            .execute(
                "INSERT INTO logs(time, device_id, script_id, level, msg)\
                 VALUES ('2000-01-01 00:00:00.000', 'd', 's', 'debug', 'old')",
                [],
            )
            .unwrap();
        store.add_log("d", "s", "info", "new").unwrap();
        let deleted = store.prune_logs(1).unwrap();
        assert_eq!(deleted, 1);
        let metrics = store.metrics_snapshot().unwrap();
        assert_eq!(metrics.logs, 1);
        assert_eq!(store.list_logs(None, None, 10).unwrap()[0].msg, "new");
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }
}
