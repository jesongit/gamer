//! SQLite 持久化：设备、定时任务、运行日志。
//!
//! 数据库从当前 schema v3 空库创建（无 legacy `tasks` 表）；v1 历史库经
//! v1→v2（Timer Core 泛化）与 v2→v3（Task 模型收口）逐级迁移，user_version=0
//! 仍拒绝自动补齐。

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
use rusqlite::types::Type;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::config::Config;
use crate::metrics::Metrics;
use crate::migrations::TARGET_SCHEMA;
use crate::timer_core::{Task, TaskPreset, TaskState};

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

/// Raw SQLite representation used by the generic Timer Core model.
#[derive(Debug, Clone)]
pub(crate) struct TaskStorage {
    pub id: String,
    pub name: String,
    pub device_id: String,
    pub android_package: String,
    pub content_package: Option<String>,
    pub runner_id: String,
    pub entrypoint: String,
    pub payload_json: String,
    pub schedule_json: String,
    pub state: String,
    pub enabled: bool,
    pub next_wakeup: Option<i64>,
    pub last_result: Option<String>,
    pub last_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub preset_id: Option<String>,
    pub suspend_reason: Option<String>,
}

impl TaskStorage {
    fn from_task(task: &Task) -> anyhow::Result<Self> {
        task.validate()?;
        Ok(Self {
            id: task.id.clone(),
            name: task.name.clone(),
            device_id: task.app.device_id.to_string(),
            android_package: task.app.android_package.to_string(),
            content_package: task.app.content_package.as_ref().map(ToString::to_string),
            runner_id: task.runner_id.clone(),
            entrypoint: task.entrypoint.clone(),
            payload_json: serde_json::to_string(&task.payload)?,
            schedule_json: serde_json::to_string(&task.schedule)?,
            state: task.state.as_str().to_string(),
            enabled: task.enabled,
            next_wakeup: task.next_wakeup.map(|value| value.timestamp()),
            last_result: task.last_result.clone(),
            last_run_at: task.last_run_at.map(|value| value.to_rfc3339()),
            created_at: task.created_at.to_rfc3339(),
            updated_at: task.updated_at.to_rfc3339(),
            preset_id: task.preset_id.clone(),
            suspend_reason: task.suspend_reason.clone(),
        })
    }
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

const DB_QUEUE_CAPACITY: usize = 1024;
#[cfg(test)]
const DB_BLOCKING_PERMITS: usize = 2;
const LOG_BATCH_SIZE: usize = 100;
const LOG_PRUNE_BATCH_SIZE: i64 = 500;
const LOG_FLUSH_INTERVAL: Duration = Duration::from_millis(250);
const MAX_LOG_MESSAGE_CHARS: usize = 1024;
/// 目标 schema 版本：权威定义在 `migrations::TARGET_SCHEMA`
/// （DATA-003 兼容常量），此处仅别名沿用
const SCHEMA_VERSION: i64 = TARGET_SCHEMA;

#[cfg(test)]
static DB_BLOCKING_GATE: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(DB_BLOCKING_PERMITS);

type ErasedDbResult = anyhow::Result<Box<dyn Any + Send>>;
type DbTask = Box<dyn FnOnce(&mut Connection) -> ErasedDbResult + Send + 'static>;

struct LogRecord {
    time: String,
    device_id: String,
    script_id: String,
    level: String,
    msg: String,
}

struct PendingLog {
    record: LogRecord,
    completion: Option<oneshot::Sender<anyhow::Result<()>>>,
}

enum DbCommand {
    Call {
        task: DbTask,
        reply: oneshot::Sender<ErasedDbResult>,
    },
    Log {
        record: LogRecord,
        completion: Option<oneshot::Sender<anyhow::Result<()>>>,
    },
    Shutdown,
}

/// Wait for a compatibility RPC from synchronous code. Tokio forbids
/// `oneshot::Receiver::blocking_recv` on a runtime worker, so legacy sync
/// callers use a tiny waiter thread when they happen to run under Tokio.
fn blocking_recv_compat<T: Send + 'static>(receiver: oneshot::Receiver<T>) -> Option<T> {
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::spawn(move || receiver.blocking_recv())
            .join()
            .ok()
            .and_then(Result::ok)
    } else {
        receiver.blocking_recv().ok()
    }
}

/// 数据库主文件路径（DATA-005 maintenance CLI 与 Store::open 共用同一取值，
/// 保证 CLI inspect/migrate 与启动路径打开的是同一个文件）
pub(crate) fn db_path(data_dir: &Path) -> PathBuf {
    data_dir.join("gamer.db")
}

fn open_connection(path: &Path) -> anyhow::Result<Connection> {
    let is_new_database = !path.exists();
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;\nPRAGMA synchronous = NORMAL;\nPRAGMA foreign_keys = ON;",
    )?;
    ensure_schema(&mut conn, is_new_database)?;
    Ok(conn)
}

const SCHEMA_V3_DDL: &str = r#"
CREATE TABLE devices (
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
CREATE TABLE logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    time TEXT NOT NULL,
    device_id TEXT NOT NULL,
    script_id TEXT NOT NULL,
    level TEXT NOT NULL,
    msg TEXT NOT NULL
);
CREATE INDEX idx_logs_time ON logs(time DESC);
CREATE TABLE scheduled_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    scheduled_at INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'running',
    run_id TEXT,
    error TEXT,
    created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX uq_scheduled_runs_task_time
    ON scheduled_runs(task_id, scheduled_at);
CREATE INDEX idx_scheduled_runs_created_at
    ON scheduled_runs(created_at DESC);
CREATE TABLE timer_tasks (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    device_id TEXT NOT NULL,
    android_package TEXT NOT NULL,
    content_package TEXT,
    runner_id TEXT NOT NULL,
    entrypoint TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    schedule_json TEXT NOT NULL,
    state TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    next_wakeup INTEGER,
    last_result TEXT,
    last_run_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    preset_id TEXT,
    suspend_reason TEXT
);
CREATE INDEX idx_timer_tasks_wakeup ON timer_tasks(state, enabled, next_wakeup);
CREATE INDEX idx_timer_tasks_app ON timer_tasks(android_package, content_package);
CREATE TABLE task_presets (
    id TEXT PRIMARY KEY,
    app_package TEXT NOT NULL,
    name TEXT NOT NULL,
    runner_id TEXT NOT NULL,
    entrypoint TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    schedule_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_task_presets_package ON task_presets(app_package);
"#;

fn ensure_schema(conn: &mut Connection, is_new_database: bool) -> anyhow::Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if is_new_database {
        anyhow::ensure!(
            version == 0,
            "new database has unexpected user_version={version}"
        );
        conn.execute_batch(SCHEMA_V3_DDL)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        return validate_schema_v3(conn);
    }

    match version {
        0 => anyhow::bail!(
            "database schema is unversioned (user_version=0); back up and remove gamer.db to rebuild schema v3"
        ),
        SCHEMA_VERSION => validate_schema_v3(conn),
        other => apply_schema_migrations(conn, other),
    }
}

/// Future schema upgrades have one explicit entry point (DATA-001 numbered
/// migration framework, see `crate::migrations`). No migration from the
/// unversioned development database (migration 0) is implemented: `user_version=0`
/// is rejected before this function is ever reached.
fn apply_schema_migrations(conn: &mut Connection, from_version: i64) -> anyhow::Result<()> {
    crate::migrations::run_migrations(conn, from_version, crate::migrations::MIGRATIONS)?;
    validate_schema_v3(conn)
}

/// v3 结构校验。启动路径与 maintenance CLI（DATA-005 migrate 迁移后校验）
/// 共用同一实现，保证「迁移后开放」的判定一致。
pub(crate) fn validate_schema_v3(conn: &Connection) -> anyhow::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let tables = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let expected_tables = [
        "devices",
        "logs",
        "scheduled_runs",
        "task_presets",
        "timer_tasks",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    anyhow::ensure!(
        tables == expected_tables,
        "schema v3 is incomplete: expected tables {expected_tables:?}, found {tables:?}; back up and rebuild gamer.db"
    );

    validate_table(
        conn,
        "devices",
        &[
            ("id", "TEXT", 0, 1),
            ("name", "TEXT", 1, 0),
            ("kind", "TEXT", 1, 0),
            ("addr", "TEXT", 1, 0),
            ("screen_mode", "TEXT", 1, 0),
            ("vd_res", "TEXT", 0, 0),
            ("vd_dpi", "INTEGER", 0, 0),
            ("pkg", "TEXT", 0, 0),
            ("fps", "INTEGER", 0, 0),
            ("created_at", "TEXT", 1, 0),
        ],
    )?;
    validate_table(
        conn,
        "logs",
        &[
            ("id", "INTEGER", 0, 1),
            ("time", "TEXT", 1, 0),
            ("device_id", "TEXT", 1, 0),
            ("script_id", "TEXT", 1, 0),
            ("level", "TEXT", 1, 0),
            ("msg", "TEXT", 1, 0),
        ],
    )?;
    validate_table(
        conn,
        "scheduled_runs",
        &[
            ("id", "INTEGER", 0, 1),
            ("task_id", "TEXT", 1, 0),
            ("scheduled_at", "INTEGER", 1, 0),
            ("state", "TEXT", 1, 0),
            ("run_id", "TEXT", 0, 0),
            ("error", "TEXT", 0, 0),
            ("created_at", "TEXT", 1, 0),
        ],
    )?;

    validate_table(
        conn,
        "timer_tasks",
        &[
            ("id", "TEXT", 0, 1),
            ("name", "TEXT", 1, 0),
            ("device_id", "TEXT", 1, 0),
            ("android_package", "TEXT", 1, 0),
            ("content_package", "TEXT", 0, 0),
            ("runner_id", "TEXT", 1, 0),
            ("entrypoint", "TEXT", 1, 0),
            ("payload_json", "TEXT", 1, 0),
            ("schedule_json", "TEXT", 1, 0),
            ("state", "TEXT", 1, 0),
            ("enabled", "INTEGER", 1, 0),
            ("next_wakeup", "INTEGER", 0, 0),
            ("last_result", "TEXT", 0, 0),
            ("last_run_at", "TEXT", 0, 0),
            ("created_at", "TEXT", 1, 0),
            ("updated_at", "TEXT", 1, 0),
            ("preset_id", "TEXT", 0, 0),
            ("suspend_reason", "TEXT", 0, 0),
        ],
    )?;
    validate_table(
        conn,
        "task_presets",
        &[
            ("id", "TEXT", 0, 1),
            ("app_package", "TEXT", 1, 0),
            ("name", "TEXT", 1, 0),
            ("runner_id", "TEXT", 1, 0),
            ("entrypoint", "TEXT", 1, 0),
            ("payload_json", "TEXT", 1, 0),
            ("schedule_json", "TEXT", 1, 0),
            ("created_at", "TEXT", 1, 0),
        ],
    )?;

    validate_index(conn, "logs", "idx_logs_time", false, &["time"])?;
    validate_index(
        conn,
        "scheduled_runs",
        "uq_scheduled_runs_task_time",
        true,
        &["task_id", "scheduled_at"],
    )?;
    validate_index(
        conn,
        "scheduled_runs",
        "idx_scheduled_runs_created_at",
        false,
        &["created_at"],
    )?;
    validate_index(
        conn,
        "timer_tasks",
        "idx_timer_tasks_wakeup",
        false,
        &["state", "enabled", "next_wakeup"],
    )?;
    validate_index(
        conn,
        "timer_tasks",
        "idx_timer_tasks_app",
        false,
        &["android_package", "content_package"],
    )?;
    validate_index(
        conn,
        "task_presets",
        "idx_task_presets_package",
        false,
        &["app_package"],
    )?;
    Ok(())
}

/// Compatibility name retained for maintenance callers while the validator
/// now checks the complete v3 schema.
pub(crate) fn validate_schema_v1(conn: &Connection) -> anyhow::Result<()> {
    validate_schema_v3(conn)
}

/// v1→v2 migration.  Legacy rows are copied into generic timer rows; the old
/// table remains for the HTTP/YAML compatibility adapter.
pub(crate) fn migrate_v1_to_v2(tx: &rusqlite::Transaction<'_>) -> anyhow::Result<()> {
    tx.execute_batch(
        r#"
CREATE TABLE timer_tasks (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    device_id TEXT NOT NULL,
    android_package TEXT NOT NULL,
    content_package TEXT,
    runner_id TEXT NOT NULL,
    entrypoint TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    schedule_json TEXT NOT NULL,
    state TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    next_wakeup INTEGER,
    last_result TEXT,
    last_run_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    preset_id TEXT,
    suspend_reason TEXT
);
CREATE INDEX idx_timer_tasks_wakeup ON timer_tasks(state, enabled, next_wakeup);
CREATE INDEX idx_timer_tasks_app ON timer_tasks(android_package, content_package);
CREATE TABLE task_presets (
    id TEXT PRIMARY KEY,
    app_package TEXT NOT NULL,
    name TEXT NOT NULL,
    runner_id TEXT NOT NULL,
    entrypoint TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    schedule_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_task_presets_package ON task_presets(app_package);
INSERT OR IGNORE INTO timer_tasks (
    id, name, device_id, android_package, content_package, runner_id,
    entrypoint, payload_json, schedule_json, state, enabled, next_wakeup,
    last_result, last_run_at, created_at, updated_at, preset_id, suspend_reason
)
SELECT
    id,
    name,
    device_id,
    CASE WHEN instr(script_id, '/') > 1
         THEN substr(script_id, 1, instr(script_id, '/') - 1)
         ELSE 'legacy' END,
    CASE WHEN instr(script_id, '/') > 1
         THEN substr(script_id, 1, instr(script_id, '/') - 1)
         ELSE 'legacy' END,
    'gamer.yaml',
    script_id,
    json_object('args', json(args_json), 'param_signature', param_signature),
    json_object('kind', 'cron', 'value', json_object('expression', cron)),
    CASE WHEN enabled <> 0 THEN 'active' ELSE 'suspended' END,
    enabled,
    NULL,
    last_result,
    last_run_at,
    created_at,
    created_at,
    NULL,
    CASE WHEN enabled <> 0 THEN NULL ELSE 'disabled' END
FROM tasks;
"#,
    )?;
    Ok(())
}

/// v2→v3 migration（Task 模型收口，ADR-12）：
/// - 删除 legacy `tasks` 表（script_id+cron 旧契约整体退役，数据不迁移）；
/// - `timer_tasks` / `task_presets` 的 `schedule_json` 从 `{"kind":..,"value":..}`
///   改写为 `{"provider_id":..,"config":..}`（TaskSchedule 新 serde 键）。
pub(crate) fn migrate_v2_to_v3(tx: &rusqlite::Transaction<'_>) -> anyhow::Result<()> {
    tx.execute_batch("DROP TABLE IF EXISTS tasks;")?;
    for table in ["timer_tasks", "task_presets"] {
        let sql = format!("SELECT id, schedule_json FROM {table}");
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for (id, schedule_json) in rows {
            let value: serde_json::Value = serde_json::from_str(&schedule_json)
                .map_err(|error| anyhow::anyhow!("{table} id={id} schedule_json 无效: {error}"))?;
            let rewritten = match value.as_object() {
                Some(obj) if obj.contains_key("kind") => {
                    anyhow::ensure!(
                        value["kind"].is_string(),
                        "{table} id={id} schedule_json.kind 必须是字符串"
                    );
                    serde_json::json!({
                        "provider_id": value["kind"],
                        "config": value.get("value").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                // 已是新形态（provider_id/config）→ 原样保留，迁移幂等
                Some(obj) if obj.contains_key("provider_id") => value,
                other => {
                    anyhow::bail!("{table} id={id} schedule_json 形态无法识别: {other:?}");
                }
            };
            tx.execute(
                &format!("UPDATE {table} SET schedule_json = ?2 WHERE id = ?1"),
                rusqlite::params![id, serde_json::to_string(&rewritten)?],
            )?;
        }
    }
    Ok(())
}

fn validate_table(
    conn: &Connection,
    table: &str,
    expected: &[(&str, &str, i64, i64)],
) -> anyhow::Result<()> {
    let pragma = format!("PRAGMA table_info('{table}')");
    let mut stmt = conn.prepare(&pragma)?;
    let actual = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let expected = expected
        .iter()
        .map(|(name, ty, not_null, primary_key)| {
            (
                (*name).to_string(),
                (*ty).to_string(),
                *not_null,
                *primary_key,
            )
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(
        actual == expected,
        "schema v3 is incomplete: table {table} has unexpected columns; back up and rebuild gamer.db"
    );
    Ok(())
}

fn validate_index(
    conn: &Connection,
    table: &str,
    index: &str,
    expected_unique: bool,
    expected_columns: &[&str],
) -> anyhow::Result<()> {
    let pragma = format!("PRAGMA index_list('{table}')");
    let mut stmt = conn.prepare(&pragma)?;
    let found = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)? != 0))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .find(|(name, _)| name == index);
    let Some((_, is_unique)) = found else {
        anyhow::bail!(
            "schema v3 is incomplete: missing index {index}; back up and rebuild gamer.db"
        );
    };
    anyhow::ensure!(
        is_unique == expected_unique,
        "schema v3 is incomplete: index {index} has unexpected uniqueness; back up and rebuild gamer.db"
    );
    let pragma = format!("PRAGMA index_info('{index}')");
    let mut stmt = conn.prepare(&pragma)?;
    let actual_columns = stmt
        .query_map([], |row| row.get::<_, String>(2))?
        .collect::<Result<Vec<_>, _>>()?;
    let expected_columns = expected_columns
        .iter()
        .map(|column| (*column).to_string())
        .collect::<Vec<_>>();
    anyhow::ensure!(
        actual_columns == expected_columns,
        "schema v3 is incomplete: index {index} has unexpected columns; back up and rebuild gamer.db"
    );
    Ok(())
}

const TIMER_TASK_SELECT: &str = "SELECT id, name, device_id, android_package, content_package, runner_id, entrypoint, payload_json, schedule_json, state, enabled, next_wakeup, last_result, last_run_at, created_at, updated_at, preset_id, suspend_reason FROM timer_tasks";

fn timer_tasks_from_conn(conn: &Connection, suffix: &str) -> anyhow::Result<Vec<Task>> {
    let sql = format!("{TIMER_TASK_SELECT} {suffix}");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], timer_task_storage_from_row)?;
    rows.collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(Task::from_storage)
        .collect()
}

fn timer_task_storage_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskStorage> {
    Ok(TaskStorage {
        id: r.get(0)?,
        name: r.get(1)?,
        device_id: r.get(2)?,
        android_package: r.get(3)?,
        content_package: r.get(4)?,
        runner_id: r.get(5)?,
        entrypoint: r.get(6)?,
        payload_json: r.get(7)?,
        schedule_json: r.get(8)?,
        state: r.get(9)?,
        enabled: r.get::<_, i64>(10)? != 0,
        next_wakeup: r.get(11)?,
        last_result: r.get(12)?,
        last_run_at: r.get(13)?,
        created_at: r.get(14)?,
        updated_at: r.get(15)?,
        preset_id: r.get(16)?,
        suspend_reason: r.get(17)?,
    })
}

fn task_preset_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskPreset> {
    let payload_json: String = r.get(5)?;
    let schedule_json: String = r.get(6)?;
    let created_at: String = r.get(7)?;
    let payload = serde_json::from_str(&payload_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(5, Type::Text, Box::new(error))
    })?;
    let schedule = serde_json::from_str(&schedule_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(6, Type::Text, Box::new(error))
    })?;
    let created_at = DateTime::parse_from_rfc3339(&created_at)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(7, Type::Text, Box::new(error))
        })?;
    Ok(TaskPreset {
        id: r.get(0)?,
        app_package: r.get(1)?,
        name: r.get(2)?,
        runner_id: r.get(3)?,
        entrypoint: r.get(4)?,
        payload,
        schedule,
        created_at,
    })
}

fn write_timer_task(conn: &Connection, row: &TaskStorage) -> anyhow::Result<()> {
    conn.execute(
        r#"INSERT INTO timer_tasks
           (id, name, device_id, android_package, content_package, runner_id,
            entrypoint, payload_json, schedule_json, state, enabled, next_wakeup,
            last_result, last_run_at, created_at, updated_at, preset_id, suspend_reason)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
           ON CONFLICT(id) DO UPDATE SET
            name=?2, device_id=?3, android_package=?4, content_package=?5,
            runner_id=?6, entrypoint=?7, payload_json=?8, schedule_json=?9,
            state=?10, enabled=?11, next_wakeup=?12, last_result=?13,
            last_run_at=?14, created_at=?15, updated_at=?16, preset_id=?17,
            suspend_reason=?18"#,
        rusqlite::params![
            row.id,
            row.name,
            row.device_id,
            row.android_package,
            row.content_package,
            row.runner_id,
            row.entrypoint,
            row.payload_json,
            row.schedule_json,
            row.state,
            if row.enabled { 1 } else { 0 },
            row.next_wakeup,
            row.last_result,
            row.last_run_at,
            row.created_at,
            row.updated_at,
            row.preset_id,
            row.suspend_reason,
        ],
    )?;
    Ok(())
}

fn run_worker(mut conn: Connection, rx: Receiver<DbCommand>, metrics: Arc<Metrics>) {
    let mut pending_logs = Vec::new();
    loop {
        let received = if pending_logs.is_empty() {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(LOG_FLUSH_INTERVAL)
        };
        match received {
            Ok(DbCommand::Log { record, completion }) => {
                metrics.db_dequeue();
                pending_logs.push(PendingLog { record, completion });
                if pending_logs.len() >= LOG_BATCH_SIZE
                    || pending_logs
                        .last()
                        .is_some_and(|log| log.completion.is_some())
                {
                    let _ = flush_logs(&mut conn, &mut pending_logs, &metrics);
                }
            }
            Ok(DbCommand::Call { task, reply }) => {
                metrics.db_dequeue();
                if let Err(err) = flush_logs(&mut conn, &mut pending_logs, &metrics) {
                    let _ = reply.send(Err(err));
                    continue;
                }
                let _ = reply.send(task(&mut conn));
            }
            Ok(DbCommand::Shutdown) => {
                if let Err(err) = flush_logs(&mut conn, &mut pending_logs, &metrics) {
                    tracing::error!(error = %err, "database worker flush failed during shutdown");
                }
                break;
            }
            Err(RecvTimeoutError::Timeout) => {
                let _ = flush_logs(&mut conn, &mut pending_logs, &metrics);
            }
            Err(RecvTimeoutError::Disconnected) => {
                if let Err(err) = flush_logs(&mut conn, &mut pending_logs, &metrics) {
                    tracing::error!(error = %err, "database worker flush failed after disconnect");
                }
                break;
            }
        }
    }
}

fn flush_logs(
    conn: &mut Connection,
    pending_logs: &mut Vec<PendingLog>,
    metrics: &Metrics,
) -> anyhow::Result<()> {
    if pending_logs.is_empty() {
        return Ok(());
    }
    let batch = std::mem::take(pending_logs);
    let rows = batch.len();
    let started = Instant::now();
    let result = (|| -> anyhow::Result<()> {
        let tx = conn.transaction()?;
        for log in &batch {
            tx.execute(
                "INSERT INTO logs (time, device_id, script_id, level, msg) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    log.record.time,
                    log.record.device_id,
                    log.record.script_id,
                    log.record.level,
                    log.record.msg,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    })();
    metrics.db_batch(rows, started.elapsed().as_millis() as u64, result.is_err());
    match result {
        Ok(()) => {
            tracing::info!(rows, "database log batch committed");
            for log in batch {
                if let Some(completion) = log.completion {
                    let _ = completion.send(Ok(()));
                }
            }
            Ok(())
        }
        Err(error) => {
            let error_msg = format!("{error:#}");
            tracing::error!(
                rows,
                error = %error_msg,
                "database log batch failed"
            );
            for log in batch {
                if let Some(completion) = log.completion {
                    let _ = completion.send(Err(anyhow::anyhow!(error_msg.clone())));
                }
            }
            Err(anyhow::anyhow!(error_msg))
        }
    }
}

fn sanitize_log_message(msg: &str) -> String {
    let lower = msg.to_ascii_lowercase();
    let markers = [
        "authorization",
        "cookie",
        "password",
        "passwd",
        "token",
        "secret",
        "text=",
        "text:",
        "args=",
        "import body",
        "import content",
        "script content",
        "yaml:",
        "输入文本",
        "实参",
        "导入内容",
        "脚本内容",
    ];
    let sensitive_at = markers.iter().filter_map(|marker| lower.find(marker)).min();
    let safe = match sensitive_at {
        Some(index) => format!("{}[REDACTED]", msg[..index].trim_end()),
        None => msg.to_string(),
    };
    let mut normalized = safe
        .chars()
        .map(|ch| if ch == '\r' || ch == '\n' { ' ' } else { ch })
        .take(MAX_LOG_MESSAGE_CHARS)
        .collect::<String>();
    if safe.chars().count() > MAX_LOG_MESSAGE_CHARS {
        normalized.push('…');
    }
    normalized
}

/// VACUUM 前后的数据库文件大小（字节；主库 + WAL 合计，WAL 模式下二者合看才真实）
#[derive(Debug, Clone, Copy, Serialize)]
pub struct VacuumReport {
    pub before_bytes: u64,
    pub after_bytes: u64,
}

pub struct Store {
    /// 数据库主文件路径（vacuum 前后取文件大小用）
    path: PathBuf,
    tx: SyncSender<DbCommand>,
    worker: Mutex<Option<JoinHandle<()>>>,
    metrics: Arc<Metrics>,
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = self.tx.send(DbCommand::Shutdown);
        if let Some(worker) = self.worker.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = worker.join();
        }
    }
}

#[allow(
    dead_code,
    reason = "synchronous compatibility adapters remain for startup and non-Tokio callers"
)]
impl Store {
    pub fn open(cfg: &Config) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&cfg.data_dir)?;
        let path = db_path(&cfg.data_dir);
        let (tx, rx) = mpsc::sync_channel(DB_QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let metrics = Arc::new(Metrics::default());
        let worker_metrics = metrics.clone();
        let worker = thread::Builder::new()
            .name("gamer-db-worker".into())
            .spawn(move || match open_connection(&path) {
                Ok(conn) => {
                    let _ = ready_tx.send(Ok(()));
                    run_worker(conn, rx, worker_metrics);
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(format!("{error:#}")));
                }
            })?;
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let _ = worker.join();
                anyhow::bail!("database worker initialization failed: {error}");
            }
            Err(error) => {
                let _ = worker.join();
                anyhow::bail!("database worker did not initialize: {error}");
            }
        }
        let store = Self {
            path: db_path(&cfg.data_dir),
            tx,
            worker: Mutex::new(Some(worker)),
            metrics,
        };
        if cfg.log_retain_days > 0 {
            if let Err(e) = store.prune_logs(cfg.log_retain_days) {
                tracing::warn!(error = %e, "启动时清理过期运行日志失败");
            }
        }
        Ok(store)
    }

    pub fn metrics(&self) -> Arc<Metrics> {
        self.metrics.clone()
    }

    fn request<T, F>(&self, task: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> anyhow::Result<T> + Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        let task: DbTask =
            Box::new(move |conn| task(conn).map(|value| Box::new(value) as Box<dyn Any + Send>));
        self.enqueue(DbCommand::Call {
            task,
            reply: reply_tx,
        })?;
        let result = blocking_recv_compat(reply_rx)
            .ok_or_else(|| anyhow::anyhow!("database worker stopped before replying"))??;
        result
            .downcast::<T>()
            .map(|value| *value)
            .map_err(|_| anyhow::anyhow!("database worker returned an unexpected result type"))
    }

    /// Async DB RPC boundary. The worker remains a single synchronous SQLite owner;
    /// only the caller-side reply wait is asynchronous. A full bounded queue is
    /// submitted from Tokio's blocking pool so queue backpressure cannot block a
    /// Tokio worker thread.
    async fn request_async<T, F>(&self, task: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> anyhow::Result<T> + Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        let task: DbTask =
            Box::new(move |conn| task(conn).map(|value| Box::new(value) as Box<dyn Any + Send>));
        self.enqueue_async(DbCommand::Call {
            task,
            reply: reply_tx,
        })
        .await?;
        let result = reply_rx
            .await
            .map_err(|_| anyhow::anyhow!("database worker stopped before replying"))??;
        result
            .downcast::<T>()
            .map(|value| *value)
            .map_err(|_| anyhow::anyhow!("database worker returned an unexpected result type"))
    }

    #[cfg(test)]
    pub async fn with_bounded_blocking<T, F>(task: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    {
        let _permit = DB_BLOCKING_GATE.acquire().await?;
        tokio::task::spawn_blocking(task)
            .await
            .map_err(|error| anyhow::anyhow!("database blocking worker failed: {error}"))?
    }

    fn enqueue(&self, command: DbCommand) -> anyhow::Result<()> {
        self.metrics.db_enqueue();
        if self.tx.send(command).is_err() {
            self.metrics.db_dequeue();
            anyhow::bail!("database worker is not running");
        }
        Ok(())
    }

    // 闭包返回 SendError<DbCommand>（DbCommand 较大）；两个 Err 分支同义处理，
    // 箱化只增分配不改行为
    #[allow(clippy::result_large_err)]
    async fn enqueue_async(&self, command: DbCommand) -> anyhow::Result<()> {
        self.metrics.db_enqueue();
        let tx = self.tx.clone();
        match tokio::task::spawn_blocking(move || tx.send(command)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) | Err(_) => {
                self.metrics.db_dequeue();
                anyhow::bail!("database worker is not running")
            }
        }
    }

    // ---------- 设备 ----------

    pub fn list_devices(&self) -> anyhow::Result<Vec<Device>> {
        self.request(|conn| {
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
        })
    }

    pub fn get_device(&self, id: &str) -> anyhow::Result<Option<Device>> {
        let id = id.to_string();
        self.request(move |conn| {
            let mut stmt = conn.prepare(
                // 注意：`\` 续行会吞掉行首缩进，"created_at" 后必须显式留空格，
                // 否则拼出 created_atFROM 导致 PUT /api/devices/:id 全挂
                "SELECT id, name, kind, addr, screen_mode, vd_res, vd_dpi, pkg, fps, created_at \
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
        })
    }

    pub fn upsert_device(&self, d: &Device) -> anyhow::Result<()> {
        let d = d.clone();
        self.request(move |conn| {
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
        })
    }

    pub fn delete_device(&self, id: &str) -> anyhow::Result<()> {
        let id = id.to_string();
        self.request(move |conn| {
            conn.execute("DELETE FROM devices WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    pub async fn list_devices_async(&self) -> anyhow::Result<Vec<Device>> {
        self.request_async(|conn| {
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
        })
        .await
    }

    pub async fn get_device_async(&self, id: &str) -> anyhow::Result<Option<Device>> {
        let id = id.to_string();
        self.request_async(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, kind, addr, screen_mode, vd_res, vd_dpi, pkg, fps, created_at \
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
        })
        .await
    }

    pub async fn upsert_device_async(&self, d: &Device) -> anyhow::Result<()> {
        let d = d.clone();
        self.request_async(move |conn| {
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
        })
        .await
    }

    pub async fn delete_device_async(&self, id: &str) -> anyhow::Result<()> {
        let id = id.to_string();
        self.request_async(move |conn| {
            conn.execute("DELETE FROM devices WHERE id = ?1", [id])?;
            Ok(())
        })
        .await
    }

    // ---------- Timer Core task persistence ----------

    pub fn list_timer_tasks(&self) -> anyhow::Result<Vec<Task>> {
        self.request(|conn| timer_tasks_from_conn(conn, "ORDER BY created_at"))
    }

    pub async fn list_timer_tasks_async(&self) -> anyhow::Result<Vec<Task>> {
        self.request_async(|conn| timer_tasks_from_conn(conn, "ORDER BY created_at"))
            .await
    }

    pub fn get_timer_task(&self, id: &str) -> anyhow::Result<Option<Task>> {
        let id = id.to_string();
        self.request(move |conn| {
            let mut stmt = conn.prepare(&format!("{TIMER_TASK_SELECT} WHERE id = ?1"))?;
            let mut rows = stmt.query([id])?;
            let Some(row) = rows.next()? else {
                return Ok(None);
            };
            Ok(Some(Task::from_storage(timer_task_storage_from_row(row)?)?))
        })
    }

    pub async fn get_timer_task_async(&self, id: &str) -> anyhow::Result<Option<Task>> {
        let id = id.to_string();
        self.request_async(move |conn| {
            let mut stmt = conn.prepare(&format!("{TIMER_TASK_SELECT} WHERE id = ?1"))?;
            let mut rows = stmt.query([id])?;
            let Some(row) = rows.next()? else {
                return Ok(None);
            };
            Ok(Some(Task::from_storage(timer_task_storage_from_row(row)?)?))
        })
        .await
    }

    pub fn upsert_timer_task(&self, task: &Task) -> anyhow::Result<()> {
        let row = TaskStorage::from_task(task)?;
        self.request(move |conn| write_timer_task(conn, &row))
    }

    pub async fn upsert_timer_task_async(&self, task: &Task) -> anyhow::Result<()> {
        let row = TaskStorage::from_task(task)?;
        self.request_async(move |conn| write_timer_task(conn, &row))
            .await
    }

    pub async fn delete_timer_task_async(&self, id: &str) -> anyhow::Result<()> {
        let id = id.to_string();
        self.request_async(move |conn| {
            conn.execute("DELETE FROM timer_tasks WHERE id = ?1", [id])?;
            Ok(())
        })
        .await
    }

    pub async fn set_timer_task_wakeup_async(
        &self,
        id: &str,
        next_wakeup: Option<i64>,
    ) -> anyhow::Result<()> {
        let id = id.to_string();
        let updated_at = Utc::now().to_rfc3339();
        self.request_async(move |conn| {
            conn.execute(
                "UPDATE timer_tasks SET next_wakeup = ?2, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![id, next_wakeup, updated_at],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn attach_scheduled_run_async(
        &self,
        task_id: &str,
        scheduled_at: i64,
        run_id: &str,
    ) -> anyhow::Result<()> {
        let task_id = task_id.to_string();
        let run_id = run_id.to_string();
        self.request_async(move |conn| {
            conn.execute(
                "UPDATE scheduled_runs SET run_id = ?3 WHERE task_id = ?1 AND scheduled_at = ?2 AND state = 'running'",
                rusqlite::params![task_id, scheduled_at, run_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn set_timer_task_state_async(
        &self,
        id: &str,
        state: TaskState,
        enabled: bool,
        reason: Option<&str>,
    ) -> anyhow::Result<()> {
        let id = id.to_string();
        let state = state.as_str().to_string();
        let reason = reason.map(str::to_string);
        let updated_at = Utc::now().to_rfc3339();
        self.request_async(move |conn| {
            conn.execute(
                "UPDATE timer_tasks SET state = ?2, enabled = ?3, next_wakeup = NULL, suspend_reason = ?4, updated_at = ?5 WHERE id = ?1",
                rusqlite::params![id, state, if enabled { 1 } else { 0 }, reason, updated_at],
            )?;
            Ok(())
        })
        .await
    }

    /// ADR-12 DependencyMissing：runner/schedule provider 缺失时的显式状态。
    /// 任务保留、唤醒游标清空（休眠），`enabled` 保持用户原意；
    /// `reason` 记 `missing_dependency=<id>`（或 runner 提供的依赖说明）。
    pub async fn set_timer_task_dependency_missing_async(
        &self,
        id: &str,
        reason: &str,
    ) -> anyhow::Result<()> {
        let id = id.to_string();
        let reason = reason.to_string();
        let updated_at = Utc::now().to_rfc3339();
        self.request_async(move |conn| {
            conn.execute(
                "UPDATE timer_tasks SET state = 'dependency_missing', suspend_reason = ?2, next_wakeup = NULL, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![id, reason, updated_at],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn suspend_timer_task_async(&self, id: &str, reason: &str) -> anyhow::Result<()> {
        let id = id.to_string();
        let reason = reason.to_string();
        let updated_at = Utc::now().to_rfc3339();
        self.request_async(move |conn| {
            conn.execute(
                "UPDATE timer_tasks SET state = 'suspended', enabled = 0, next_wakeup = NULL, suspend_reason = ?2, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![id, reason, updated_at],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn suspend_timer_tasks_for_package_async(
        &self,
        package: &str,
        reason: &str,
    ) -> anyhow::Result<usize> {
        let package = package.to_string();
        let reason = reason.to_string();
        let updated_at = Utc::now().to_rfc3339();
        self.request_async(move |conn| {
            let changed = conn.execute(
                "UPDATE timer_tasks SET state = 'suspended', enabled = 0, next_wakeup = NULL, suspend_reason = ?2, updated_at = ?3 WHERE android_package = ?1 OR content_package = ?1",
                rusqlite::params![package, reason, updated_at],
            )?;
            Ok(changed)
        })
        .await
    }

    /// ADR-13 runner lifecycle: the owner extension went away, so its runner
    /// disappeared.  Only still-Active (schedulable) tasks of `runner_id`
    /// enter `dependency_missing`; `enabled` keeps the user's intent, the
    /// wakeup cursor is cleared and the reason records the missing runner.
    pub async fn suspend_active_timer_tasks_for_runner_async(
        &self,
        runner_id: &str,
    ) -> anyhow::Result<usize> {
        let runner_id = runner_id.to_string();
        let reason = format!("missing_dependency={runner_id}");
        let updated_at = Utc::now().to_rfc3339();
        self.request_async(move |conn| {
            let changed = conn.execute(
                "UPDATE timer_tasks SET state = 'dependency_missing', suspend_reason = ?2, next_wakeup = NULL, updated_at = ?3 WHERE runner_id = ?1 AND state = 'active' AND enabled = 1",
                rusqlite::params![runner_id, reason, updated_at],
            )?;
            Ok(changed)
        })
        .await
    }

    /// Tasks currently in `dependency_missing` whose reason is exactly
    /// `missing_dependency=<runner_id>` — the recovery set for ADR-13 runner
    /// re-registration.  Manual suspends/disables (`suspended` state) and
    /// tasks missing a *different* dependency never match.
    pub async fn list_timer_tasks_missing_runner_async(
        &self,
        runner_id: &str,
    ) -> anyhow::Result<Vec<Task>> {
        let runner_id = runner_id.to_string();
        let reason = format!("missing_dependency={runner_id}");
        self.request_async(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "{TIMER_TASK_SELECT} WHERE runner_id = ?1 AND state = 'dependency_missing' AND suspend_reason = ?2 ORDER BY created_at"
            ))?;
            let mut rows = stmt.query(rusqlite::params![runner_id, reason])?;
            let mut tasks = Vec::new();
            while let Some(row) = rows.next()? {
                tasks.push(Task::from_storage(timer_task_storage_from_row(row)?)?);
            }
            Ok(tasks)
        })
        .await
    }

    /// Guarded resume of one dependency-missing task: the row only moves back
    /// to `active` (with a freshly computed wakeup cursor and a cleared
    /// reason) while it is still in `dependency_missing` with the exact
    /// `missing_dependency=<runner_id>` reason.  Returns whether the row was
    /// actually resumed.
    pub async fn resume_timer_task_from_dependency_missing_async(
        &self,
        id: &str,
        runner_id: &str,
        next_wakeup: Option<i64>,
    ) -> anyhow::Result<bool> {
        let id = id.to_string();
        let reason = format!("missing_dependency={runner_id}");
        let updated_at = Utc::now().to_rfc3339();
        self.request_async(move |conn| {
            let changed = conn.execute(
                "UPDATE timer_tasks SET state = 'active', suspend_reason = NULL, next_wakeup = ?3, updated_at = ?4 WHERE id = ?1 AND state = 'dependency_missing' AND suspend_reason = ?2",
                rusqlite::params![id, reason, next_wakeup, updated_at],
            )?;
            Ok(changed == 1)
        })
        .await
    }

    pub async fn update_timer_task_result_async(
        &self,
        id: &str,
        result: &str,
        _error: Option<&str>,
    ) -> anyhow::Result<()> {
        let id = id.to_string();
        let result = result.to_string();
        let last_run_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        self.request_async(move |conn| {
            conn.execute(
                "UPDATE timer_tasks SET last_result = ?2, last_run_at = ?3, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![id, result, last_run_at],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn upsert_task_preset_async(&self, preset: &TaskPreset) -> anyhow::Result<()> {
        preset.validate()?;
        let payload_json = serde_json::to_string(&preset.payload)?;
        let schedule_json = serde_json::to_string(&preset.schedule)?;
        let preset = preset.clone();
        self.request_async(move |conn| {
            conn.execute(
                r#"INSERT INTO task_presets
                   (id, app_package, name, runner_id, entrypoint, payload_json, schedule_json, created_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                   ON CONFLICT(id) DO UPDATE SET app_package=?2, name=?3, runner_id=?4,
                     entrypoint=?5, payload_json=?6, schedule_json=?7, created_at=?8"#,
                rusqlite::params![
                    preset.id,
                    preset.app_package,
                    preset.name,
                    preset.runner_id,
                    preset.entrypoint,
                    payload_json,
                    schedule_json,
                    preset.created_at.to_rfc3339(),
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn list_task_presets_async(
        &self,
        app_package: Option<&str>,
    ) -> anyhow::Result<Vec<TaskPreset>> {
        let app_package = app_package.map(str::to_string);
        self.request_async(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, app_package, name, runner_id, entrypoint, payload_json, schedule_json, created_at FROM task_presets WHERE (?1 IS NULL OR app_package = ?1) ORDER BY created_at, id",
            )?;
            let rows = stmt.query_map(rusqlite::params![app_package], task_preset_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    pub async fn get_task_preset_async(&self, id: &str) -> anyhow::Result<Option<TaskPreset>> {
        let id = id.to_string();
        self.request_async(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, app_package, name, runner_id, entrypoint, payload_json, schedule_json, created_at FROM task_presets WHERE id = ?1",
            )?;
            let mut rows = stmt.query([id])?;
            let Some(row) = rows.next()? else {
                return Ok(None);
            };
            Ok(Some(task_preset_from_row(row)?))
        })
        .await
    }

    pub async fn delete_task_preset_async(&self, id: &str) -> anyhow::Result<bool> {
        let id = id.to_string();
        self.request_async(move |conn| {
            Ok(conn.execute("DELETE FROM task_presets WHERE id = ?1", [id])? != 0)
        })
        .await
    }

    // ---------- 定时触发幂等记录 ----------

    /// 原子领取一个计划触发点。返回 true 表示本次调用取得执行权；false 表示
    /// 该 `(task_id, scheduled_at)` 已由本进程或此前的进程领取。
    pub fn claim_scheduled_run(&self, task_id: &str, scheduled_at: i64) -> anyhow::Result<bool> {
        let created_at = Utc::now().to_rfc3339();
        let task_id = task_id.to_string();
        self.request(move |conn| {
            let changed = conn.execute(
                r#"INSERT INTO scheduled_runs
                       (task_id, scheduled_at, state, created_at)
                   VALUES (?1, ?2, 'running', ?3)
                   ON CONFLICT(task_id, scheduled_at) DO NOTHING"#,
                rusqlite::params![task_id, scheduled_at, created_at],
            )?;
            Ok(changed == 1)
        })
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
        let task_id = task_id.to_string();
        let state = state.to_string();
        let run_id = run_id.map(str::to_string);
        let error = error.map(str::to_string);
        self.request(move |conn| {
            let changed = conn.execute(
                r#"UPDATE scheduled_runs
                      SET state = ?3, run_id = COALESCE(?4, run_id), error = ?5
                    WHERE task_id = ?1 AND scheduled_at = ?2 AND state = 'running'"#,
                rusqlite::params![task_id, scheduled_at, state, run_id, error],
            )?;
            Ok(changed == 1)
        })
    }

    pub async fn claim_scheduled_run_async(
        &self,
        task_id: &str,
        scheduled_at: i64,
    ) -> anyhow::Result<bool> {
        let created_at = Utc::now().to_rfc3339();
        let task_id = task_id.to_string();
        self.request_async(move |conn| {
            let changed = conn.execute(
                r#"INSERT INTO scheduled_runs
                       (task_id, scheduled_at, state, created_at)
                   VALUES (?1, ?2, 'running', ?3)
                   ON CONFLICT(task_id, scheduled_at) DO NOTHING"#,
                rusqlite::params![task_id, scheduled_at, created_at],
            )?;
            Ok(changed == 1)
        })
        .await
    }

    pub async fn finish_scheduled_run_async(
        &self,
        task_id: &str,
        scheduled_at: i64,
        state: &str,
        run_id: Option<&str>,
        error: Option<&str>,
    ) -> anyhow::Result<bool> {
        let task_id = task_id.to_string();
        let state = state.to_string();
        let run_id = run_id.map(str::to_string);
        let error = error.map(str::to_string);
        self.request_async(move |conn| {
            let changed = conn.execute(
                r#"UPDATE scheduled_runs
                      SET state = ?3, run_id = COALESCE(?4, run_id), error = ?5
                    WHERE task_id = ?1 AND scheduled_at = ?2 AND state = 'running'"#,
                rusqlite::params![task_id, scheduled_at, state, run_id, error],
            )?;
            Ok(changed == 1)
        })
        .await
    }

    #[cfg(test)]
    pub(crate) fn scheduled_run_count(&self, task_id: &str, scheduled_at: i64) -> i64 {
        let task_id = task_id.to_string();
        self.request(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM scheduled_runs WHERE task_id = ?1 AND scheduled_at = ?2",
                rusqlite::params![task_id, scheduled_at],
                |r| r.get(0),
            )?)
        })
        .unwrap()
    }

    #[cfg(test)]
    pub(crate) fn scheduled_run_state(&self, task_id: &str, scheduled_at: i64) -> String {
        let task_id = task_id.to_string();
        self.request(move |conn| {
            Ok(conn.query_row(
                "SELECT state FROM scheduled_runs WHERE task_id = ?1 AND scheduled_at = ?2",
                rusqlite::params![task_id, scheduled_at],
                |r| r.get(0),
            )?)
        })
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
        let record = LogRecord {
            time: chrono::Local::now()
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string(),
            device_id: device_id.to_string(),
            script_id: script_id.to_string(),
            level: level.to_string(),
            msg: sanitize_log_message(msg),
        };
        let critical = matches!(level.to_ascii_lowercase().as_str(), "success" | "error");
        let is_debug = level.eq_ignore_ascii_case("debug");
        let (completion, completion_rx) = if critical {
            let (tx, rx) = oneshot::channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let command = DbCommand::Log { record, completion };
        self.metrics.db_enqueue();
        let sent = if is_debug && !critical {
            match self.tx.try_send(command) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(_command)) => {
                    self.metrics.db_dequeue();
                    self.metrics.db_drop_debug_log();
                    return Ok(());
                }
                Err(TrySendError::Disconnected(_command)) => Err(()),
            }
        } else {
            self.tx.send(command).map_err(|_| ())
        };
        if sent.is_err() {
            self.metrics.db_dequeue();
            anyhow::bail!("database worker is not running");
        }
        if let Some(reply) = completion_rx {
            blocking_recv_compat(reply)
                .ok_or_else(|| anyhow::anyhow!("database worker stopped before log flush"))??;
        }
        Ok(())
    }

    pub async fn add_log_async(
        &self,
        device_id: &str,
        script_id: &str,
        level: &str,
        msg: &str,
    ) -> anyhow::Result<()> {
        let record = LogRecord {
            time: chrono::Local::now()
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string(),
            device_id: device_id.to_string(),
            script_id: script_id.to_string(),
            level: level.to_string(),
            msg: sanitize_log_message(msg),
        };
        let critical = matches!(level.to_ascii_lowercase().as_str(), "success" | "error");
        let is_debug = level.eq_ignore_ascii_case("debug");
        let (completion, completion_rx) = if critical {
            let (tx, rx) = oneshot::channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let command = DbCommand::Log { record, completion };
        if is_debug && !critical {
            self.metrics.db_enqueue();
            match self.tx.try_send(command) {
                Ok(()) => {}
                Err(TrySendError::Full(_command)) => {
                    self.metrics.db_dequeue();
                    self.metrics.db_drop_debug_log();
                    return Ok(());
                }
                Err(TrySendError::Disconnected(_command)) => {
                    self.metrics.db_dequeue();
                    anyhow::bail!("database worker is not running");
                }
            }
        } else {
            self.enqueue_async(command).await?;
        }
        if let Some(reply) = completion_rx {
            reply
                .await
                .map_err(|_| anyhow::anyhow!("database worker stopped before log flush"))??;
        }
        Ok(())
    }

    pub fn list_logs(
        &self,
        device_id: Option<&str>,
        level: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<LogEntry>> {
        let device_id = device_id.map(str::to_string);
        let level = level.map(str::to_string);
        self.request(move |conn| {
            let mut sql =
                String::from("SELECT id, time, device_id, script_id, level, msg FROM logs");
            let mut conds = Vec::new();
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(d) = device_id {
                conds.push("device_id = ?".to_string());
                params.push(Box::new(d));
            }
            if let Some(l) = level {
                conds.push("level = ?".to_string());
                params.push(Box::new(l));
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
        })
    }

    pub async fn list_logs_async(
        &self,
        device_id: Option<&str>,
        level: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<LogEntry>> {
        let device_id = device_id.map(str::to_string);
        let level = level.map(str::to_string);
        self.request_async(move |conn| {
            let mut sql =
                String::from("SELECT id, time, device_id, script_id, level, msg FROM logs");
            let mut conds = Vec::new();
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(d) = device_id {
                conds.push("device_id = ?".to_string());
                params.push(Box::new(d));
            }
            if let Some(l) = level {
                conds.push("level = ?".to_string());
                params.push(Box::new(l));
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
        })
        .await
    }

    pub fn clear_logs(&self) -> anyhow::Result<()> {
        self.request(|conn| {
            conn.execute("DELETE FROM logs", [])?;
            Ok(())
        })
    }

    pub async fn clear_logs_async(&self) -> anyhow::Result<()> {
        self.request_async(|conn| {
            conn.execute("DELETE FROM logs", [])?;
            Ok(())
        })
        .await
    }

    /// 运行健康探测：只做一个极轻量的数据库 round-trip，不暴露底层错误给 HTTP 客户端。
    pub fn health_check(&self) -> anyhow::Result<()> {
        self.request(|conn| {
            conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?;
            Ok(())
        })
    }

    pub async fn health_check_async(&self) -> anyhow::Result<()> {
        self.request_async(|conn| {
            conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?;
            Ok(())
        })
        .await
    }

    /// 获取低基数行数指标。表名是代码内固定值，查询不接受外部输入。
    pub fn metrics_snapshot(&self) -> anyhow::Result<StoreMetrics> {
        self.request(|conn| {
            Ok(conn.query_row(
                "SELECT\
                    (SELECT COUNT(*) FROM devices),\
                    (SELECT COUNT(*) FROM timer_tasks),\
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
        })
    }

    pub async fn metrics_snapshot_async(&self) -> anyhow::Result<StoreMetrics> {
        self.request_async(|conn| {
            Ok(conn.query_row(
                "SELECT\
                    (SELECT COUNT(*) FROM devices),\
                    (SELECT COUNT(*) FROM timer_tasks),\
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
        })
        .await
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
        self.request(move |conn| {
            let mut total = 0u64;
            let mut batches = 0u64;
            let mut first_deleted_id = None;
            let mut last_deleted_id = None;
            loop {
                let ids = {
                    let mut stmt =
                        conn.prepare("SELECT id FROM logs WHERE time < ?1 ORDER BY id LIMIT ?2")?;
                    let rows = stmt.query_map(
                        rusqlite::params![cutoff.as_str(), LOG_PRUNE_BATCH_SIZE],
                        |r| r.get::<_, i64>(0),
                    )?;
                    rows.collect::<Result<Vec<_>, _>>()?
                };
                if ids.is_empty() {
                    break;
                }
                let batch_first_id = ids[0];
                let batch_last_id = *ids.last().unwrap();
                first_deleted_id.get_or_insert(batch_first_id);
                last_deleted_id = Some(batch_last_id);
                let placeholders = std::iter::repeat_n("?", ids.len())
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!("DELETE FROM logs WHERE id IN ({placeholders})");
                let deleted = conn.execute(&sql, rusqlite::params_from_iter(ids.iter()))?;
                total += deleted as u64;
                batches += 1;
                tracing::info!(
                    retain_days,
                    cutoff = %cutoff,
                    batch = batches,
                    deleted_rows = deleted,
                    id_start = batch_first_id,
                    id_end = batch_last_id,
                    "expired run logs removed"
                );
                if deleted < LOG_PRUNE_BATCH_SIZE as usize {
                    break;
                }
            }
            if total > 0 {
                tracing::info!(
                    retain_days,
                    cutoff = %cutoff,
                    batches,
                    deleted_rows = total,
                    id_start = first_deleted_id.unwrap_or_default(),
                    id_end = last_deleted_id.unwrap_or_default(),
                    "expired run logs cleanup finished"
                );
            }
            Ok(total)
        })
    }

    pub async fn prune_logs_async(&self, retain_days: u32) -> anyhow::Result<u64> {
        if retain_days == 0 {
            return Ok(0);
        }
        let cutoff = (Local::now() - chrono::Duration::days(retain_days as i64))
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string();
        self.request_async(move |conn| {
            let mut total = 0u64;
            let mut batches = 0u64;
            let mut first_deleted_id = None;
            let mut last_deleted_id = None;
            loop {
                let ids = {
                    let mut stmt =
                        conn.prepare("SELECT id FROM logs WHERE time < ?1 ORDER BY id LIMIT ?2")?;
                    let rows = stmt.query_map(
                        rusqlite::params![cutoff.as_str(), LOG_PRUNE_BATCH_SIZE],
                        |r| r.get::<_, i64>(0),
                    )?;
                    rows.collect::<Result<Vec<_>, _>>()?
                };
                if ids.is_empty() {
                    break;
                }
                let batch_first_id = ids[0];
                let batch_last_id = *ids.last().unwrap();
                first_deleted_id.get_or_insert(batch_first_id);
                last_deleted_id = Some(batch_last_id);
                let placeholders = std::iter::repeat_n("?", ids.len())
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!("DELETE FROM logs WHERE id IN ({placeholders})");
                let deleted = conn.execute(&sql, rusqlite::params_from_iter(ids.iter()))?;
                total += deleted as u64;
                batches += 1;
                tracing::info!(
                    retain_days,
                    cutoff = %cutoff,
                    batch = batches,
                    deleted_rows = deleted,
                    id_start = batch_first_id,
                    id_end = batch_last_id,
                    "expired run logs removed"
                );
                if deleted < LOG_PRUNE_BATCH_SIZE as usize {
                    break;
                }
            }
            if total > 0 {
                tracing::info!(
                    retain_days,
                    cutoff = %cutoff,
                    batches,
                    deleted_rows = total,
                    id_start = first_deleted_id.unwrap_or_default(),
                    id_end = last_deleted_id.unwrap_or_default(),
                    "expired run logs cleanup finished"
                );
            }
            Ok(total)
        })
        .await
    }

    /// 手动维护动作（DATA-004）：VACUUM 重建数据库文件、回收已删除行占用的页。
    /// VACUUM 需要独占锁且耗时，故放入 DB worker 线程串行执行——与 worker 内
    /// 其它操作天然互斥，也不阻塞异步调用侧。返回 vacuum 前后的文件字节数
    /// （主库 + WAL 合计；结尾强制 checkpoint 截断 WAL，让主库文件立即反映
    /// 重建后的真实大小）。
    pub fn vacuum(&self) -> anyhow::Result<VacuumReport> {
        let path = self.path.clone();
        self.request(move |conn| {
            let file_bytes = |p: &Path| -> u64 {
                let wal = PathBuf::from(format!("{}-wal", p.display()));
                std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
                    + std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0)
            };
            let before = file_bytes(&path);
            conn.execute_batch("VACUUM")?;
            if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
                tracing::warn!(error = %e, "vacuum 后 WAL checkpoint 失败（不影响 VACUUM 结果）");
            }
            let after = file_bytes(&path);
            tracing::info!(
                before_bytes = before,
                after_bytes = after,
                "database vacuum finished"
            );
            Ok(VacuumReport {
                before_bytes: before,
                after_bytes: after,
            })
        })
    }

    pub async fn vacuum_async(&self) -> anyhow::Result<VacuumReport> {
        let path = self.path.clone();
        self.request_async(move |conn| {
            let file_bytes = |p: &Path| -> u64 {
                let wal = PathBuf::from(format!("{}-wal", p.display()));
                std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
                    + std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0)
            };
            let before = file_bytes(&path);
            conn.execute_batch("VACUUM")?;
            if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
                tracing::warn!(error = %e, "vacuum 后 WAL checkpoint 失败（不影响 VACUUM 结果）");
            }
            let after = file_bytes(&path);
            tracing::info!(
                before_bytes = before,
                after_bytes = after,
                "database vacuum finished"
            );
            Ok(VacuumReport {
                before_bytes: before,
                after_bytes: after,
            })
        })
        .await
    }

    #[cfg(test)]
    fn pragma_snapshot(&self) -> anyhow::Result<(String, i64, i64)> {
        self.request(|conn| {
            Ok((
                conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?,
                conn.query_row("PRAGMA busy_timeout", [], |r| r.get(0))?,
                conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?,
            ))
        })
    }

    #[cfg(test)]
    fn insert_raw_log_for_test(&self, time: &str, msg: &str) -> anyhow::Result<()> {
        let time = time.to_string();
        let msg = msg.to_string();
        self.request(move |conn| {
            conn.execute(
                "INSERT INTO logs(time, device_id, script_id, level, msg) VALUES (?1, 'd', 's', 'debug', ?2)",
                rusqlite::params![time, msg],
            )?;
            Ok(())
        })
    }
}

pub type Db = Arc<Store>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration as StdDuration;

    fn temp_config(name: &str) -> (Config, PathBuf) {
        let dir = std::env::temp_dir().join(format!("gamer-store-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
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
    fn scheduled_finish_is_idempotent_after_claim() {
        let (cfg, dir) = temp_config("finish");
        let store = Store::open(&cfg).unwrap();
        assert!(store.claim_scheduled_run("task", 42).unwrap());
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
        let (journal, busy_timeout, foreign_keys) = store.pragma_snapshot().unwrap();
        assert_eq!(journal.to_ascii_lowercase(), "wal");
        assert_eq!(busy_timeout, 5_000);
        assert_eq!(foreign_keys, 1);
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn metrics_snapshot_and_log_retention_are_bounded() {
        let (cfg, dir) = temp_config("metrics-retention");
        let store = Store::open(&cfg).unwrap();
        store
            .insert_raw_log_for_test("2000-01-01 00:00:00.000", "old")
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

    #[test]
    fn empty_database_creates_complete_schema_v3() {
        let (cfg, dir) = temp_config("schema-v3");
        let db_path = dir.join("gamer.db");
        let store = Store::open(&cfg).unwrap();
        drop(store);

        let conn = Connection::open(&db_path).unwrap();
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        // legacy `tasks` 表在 v3 已删除；新库不得再出现
        let legacy: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'tasks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy, 0);
        let unique_index: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'uq_scheduled_runs_task_time'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unique_index, 1);

        drop(conn);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn v1_database_migrates_legacy_tasks_into_timer_core_rows() {
        let (cfg, dir) = temp_config("timer-v1-migration");
        let db_path = dir.join("gamer.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
CREATE TABLE devices (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL,
    addr TEXT NOT NULL DEFAULT '', screen_mode TEXT NOT NULL DEFAULT 'mirror',
    vd_res TEXT, vd_dpi INTEGER, pkg TEXT, fps INTEGER, created_at TEXT NOT NULL
);
CREATE TABLE tasks (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, cron TEXT NOT NULL,
    script_id TEXT NOT NULL, device_id TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1, last_result TEXT, last_run_at TEXT,
    created_at TEXT NOT NULL, args_json TEXT NOT NULL,
    param_signature TEXT NOT NULL
);
CREATE TABLE logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT, time TEXT NOT NULL,
    device_id TEXT NOT NULL, script_id TEXT NOT NULL, level TEXT NOT NULL,
    msg TEXT NOT NULL
);
CREATE INDEX idx_logs_time ON logs(time DESC);
CREATE TABLE scheduled_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT NOT NULL,
    scheduled_at INTEGER NOT NULL, state TEXT NOT NULL DEFAULT 'running',
    run_id TEXT, error TEXT, created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX uq_scheduled_runs_task_time ON scheduled_runs(task_id, scheduled_at);
CREATE INDEX idx_scheduled_runs_created_at ON scheduled_runs(created_at DESC);
INSERT INTO tasks
  (id, name, cron, script_id, device_id, enabled, created_at, args_json, param_signature)
VALUES ('legacy-1', 'Legacy', '*/5 * * * *',
        'com.example/daily.yaml', 'device-1', 1,
        '2026-08-29T00:00:00Z', '{}', 'psig1|');
PRAGMA user_version = 1;
"#,
        )
        .unwrap();
        drop(conn);

        let store = Store::open(&cfg).unwrap();
        let tasks = store.list_timer_tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].runner_id, "gamer.yaml");
        assert_eq!(tasks[0].entrypoint, "com.example/daily.yaml");
        assert_eq!(tasks[0].payload["args"], serde_json::json!({}));
        // v1→v2 先按旧 JSON 形态落行，v2→v3 统一改写为 provider/config
        assert_eq!(tasks[0].schedule.provider_id, "cron");
        assert_eq!(tasks[0].schedule.config["expression"], "*/5 * * * *");

        let conn = Connection::open(&db_path).unwrap();
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, TARGET_SCHEMA);
        let legacy: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'tasks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy, 0, "v3 迁移后 legacy tasks 表必须删除");
        drop(conn);
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    /// v2→v3（P11.1 Task 模型收口）：legacy `tasks` 表删除；timer_tasks 与
    /// task_presets 的 schedule_json 统一改写为 provider/config 形态。
    #[test]
    fn v2_database_migrates_to_v3_dropping_tasks_and_rewriting_schedule_json() {
        let (cfg, dir) = temp_config("timer-v2-migration");
        let db_path = dir.join("gamer.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
CREATE TABLE devices (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL,
    addr TEXT NOT NULL DEFAULT '', screen_mode TEXT NOT NULL DEFAULT 'mirror',
    vd_res TEXT, vd_dpi INTEGER, pkg TEXT, fps INTEGER, created_at TEXT NOT NULL
);
CREATE TABLE tasks (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, cron TEXT NOT NULL,
    script_id TEXT NOT NULL, device_id TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1, last_result TEXT, last_run_at TEXT,
    created_at TEXT NOT NULL, args_json TEXT NOT NULL,
    param_signature TEXT NOT NULL
);
CREATE TABLE logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT, time TEXT NOT NULL,
    device_id TEXT NOT NULL, script_id TEXT NOT NULL, level TEXT NOT NULL,
    msg TEXT NOT NULL
);
CREATE INDEX idx_logs_time ON logs(time DESC);
CREATE TABLE scheduled_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT NOT NULL,
    scheduled_at INTEGER NOT NULL, state TEXT NOT NULL DEFAULT 'running',
    run_id TEXT, error TEXT, created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX uq_scheduled_runs_task_time ON scheduled_runs(task_id, scheduled_at);
CREATE INDEX idx_scheduled_runs_created_at ON scheduled_runs(created_at DESC);
CREATE TABLE timer_tasks (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, device_id TEXT NOT NULL,
    android_package TEXT NOT NULL, content_package TEXT,
    runner_id TEXT NOT NULL, entrypoint TEXT NOT NULL,
    payload_json TEXT NOT NULL, schedule_json TEXT NOT NULL,
    state TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1,
    next_wakeup INTEGER, last_result TEXT, last_run_at TEXT,
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
    preset_id TEXT, suspend_reason TEXT
);
CREATE INDEX idx_timer_tasks_wakeup ON timer_tasks(state, enabled, next_wakeup);
CREATE INDEX idx_timer_tasks_app ON timer_tasks(android_package, content_package);
CREATE TABLE task_presets (
    id TEXT PRIMARY KEY, app_package TEXT NOT NULL, name TEXT NOT NULL,
    runner_id TEXT NOT NULL, entrypoint TEXT NOT NULL,
    payload_json TEXT NOT NULL, schedule_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_task_presets_package ON task_presets(app_package);
INSERT INTO timer_tasks (id, name, device_id, android_package, content_package,
    runner_id, entrypoint, payload_json, schedule_json, state, enabled, created_at, updated_at)
VALUES ('t1', 'T', 'd1', 'com.example', 'com.example', 'gamer.yaml', 'com.example/daily.yaml',
        '{"args":{}}', '{"kind":"cron","value":{"expression":"0 8 * * *"}}',
        'active', 1, '2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z');
INSERT INTO task_presets (id, app_package, name, runner_id, entrypoint,
    payload_json, schedule_json, created_at)
VALUES ('p1', 'official.xxx', 'P', 'gamer.yaml', 'run', '{}',
        '{"kind":"cron","value":{"expression":"*/5 * * * *"}}', '2026-09-01T00:00:00Z');
PRAGMA user_version = 2;
"#,
        )
        .unwrap();
        drop(conn);

        let store = Store::open(&cfg).unwrap();
        let task = store.get_timer_task("t1").unwrap().unwrap();
        assert_eq!(task.schedule.provider_id, "cron");
        assert_eq!(task.schedule.config["expression"], "0 8 * * *");

        let conn = Connection::open(&db_path).unwrap();
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, TARGET_SCHEMA);
        let legacy: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'tasks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy, 0, "v3 迁移必须删除 legacy tasks 表");
        let preset_schedule: String = conn
            .query_row(
                "SELECT schedule_json FROM task_presets WHERE id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let preset: serde_json::Value = serde_json::from_str(&preset_schedule).unwrap();
        assert_eq!(preset["provider_id"], "cron");
        assert_eq!(preset["config"]["expression"], "*/5 * * * *");
        assert!(preset.get("kind").is_none());
        assert!(preset.get("value").is_none());
        drop(conn);
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn task_preset_is_independent_from_user_timer_task() {
        let (cfg, dir) = temp_config("timer-preset");
        let store = Store::open(&cfg).unwrap();
        let app = crate::core::AppContext::for_test("device-1", "com.example").unwrap();
        let schedule =
            crate::timer_core::TaskSchedule::new("opaque", serde_json::json!({"rule": "every"}))
                .unwrap();
        let task = Task::new(
            "user-task",
            "User task",
            app,
            "runner.example",
            "entrypoint",
            serde_json::json!({"input": 1}),
            schedule.clone(),
        )
        .unwrap();
        let preset = TaskPreset {
            id: "preset-1".into(),
            app_package: "com.example".into(),
            name: "Preset".into(),
            runner_id: "runner.example".into(),
            entrypoint: "entrypoint".into(),
            payload: serde_json::json!({"input": 0}),
            schedule,
            created_at: Utc::now(),
        };
        store.upsert_timer_task_async(&task).await.unwrap();
        store.upsert_task_preset_async(&preset).await.unwrap();

        assert_eq!(store.list_task_presets_async(None).await.unwrap().len(), 1);
        assert_eq!(
            store
                .suspend_timer_tasks_for_package_async("com.example", "package removed")
                .await
                .unwrap(),
            1
        );
        let suspended = store
            .get_timer_task_async("user-task")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(suspended.state, TaskState::Suspended);
        assert_eq!(suspended.suspend_reason.as_deref(), Some("package removed"));
        assert_eq!(suspended.schedule, preset.schedule);

        assert!(store.delete_task_preset_async("preset-1").await.unwrap());
        assert!(store
            .get_task_preset_async("preset-1")
            .await
            .unwrap()
            .is_none());
        assert!(store
            .get_timer_task_async("user-task")
            .await
            .unwrap()
            .is_some());

        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    /// ADR-12 DependencyMissing 状态落库：state/next_wakeup/reason 持久化，
    /// `enabled` 保持用户原意（依赖恢复后的重启语义由 Wave2 决定）。
    #[tokio::test]
    async fn dependency_missing_state_persists_and_preserves_enabled() {
        let (cfg, dir) = temp_config("dependency-missing");
        let store = Store::open(&cfg).unwrap();
        let app = crate::core::AppContext::for_test("device-1", "com.example").unwrap();
        let mut task = Task::new(
            "dm-task",
            "DM",
            app,
            "future.runner",
            "entry",
            serde_json::json!({}),
            crate::timer_core::TaskSchedule::new(
                "cron",
                serde_json::json!({"expression": "* * * * *"}),
            )
            .unwrap(),
        )
        .unwrap();
        task.next_wakeup = Some(Utc::now() + chrono::Duration::minutes(5));
        store.upsert_timer_task_async(&task).await.unwrap();

        store
            .set_timer_task_dependency_missing_async("dm-task", "missing_dependency=future.runner")
            .await
            .unwrap();

        let saved = store
            .get_timer_task_async("dm-task")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.state, TaskState::DependencyMissing);
        assert_eq!(
            saved.suspend_reason.as_deref(),
            Some("missing_dependency=future.runner")
        );
        assert!(saved.next_wakeup.is_none(), "依赖缺失任务必须清空唤醒游标");
        assert!(saved.enabled, "enabled 保持用户原意");
        // DependencyMissing 不可调度
        assert!(!saved.is_schedulable());

        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unversioned_database_is_rejected_without_migration() {
        let (cfg, dir) = temp_config("schema-unversioned");
        let db_path = dir.join("gamer.db");
        Connection::open(&db_path).unwrap();

        let error = match Store::open(&cfg) {
            Ok(_) => panic!("unversioned database must fail fast"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("user_version=0"));
        assert!(error.to_string().contains("unversioned"));

        fs::remove_dir_all(dir).unwrap();
    }

    /// DATA-002：无版本旧库被拒绝后，数据库文件任何字节都不得被改写——
    /// 「拒绝且不改数据」的可观测断言。准备阶段先按 open_connection 的同款
    /// PRAGMA 预置 WAL 位并干净关闭，使快照前后唯一的差异来源就是拒绝路径本身。
    #[test]
    fn unversioned_rejection_leaves_database_bytes_unchanged() {
        let (cfg, dir) = temp_config("schema-bytes");
        let db_path = dir.join("gamer.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
            // 干净关闭：冲刷一切挂起状态，文件字节此后稳定
        }
        let before = fs::read(&db_path).unwrap();

        let error = match Store::open(&cfg) {
            Ok(_) => panic!("unversioned database must fail fast"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unversioned"));

        let after = fs::read(&db_path).unwrap();
        assert_eq!(
            before, after,
            "拒绝无版本旧库时不得改写数据库文件的任何字节"
        );

        fs::remove_dir_all(dir).unwrap();
    }

    /// DATA-002：全新建库得到**确定的 schema v3**——表/列（含顺序、类型、
    /// NOT NULL、PK）与索引（含唯一性、列序）的完整快照与硬编码期望逐一比对。
    /// 任何 DDL 漂移（新增列、改类型、动索引）都必须显式更新此快照并同步
    /// schema-policy 契约，防止「新库 schema」悄悄分叉。
    #[test]
    fn new_database_schema_matches_v3_snapshot() {
        let (cfg, dir) = temp_config("schema-snapshot");
        let db_path = dir.join("gamer.db");
        let store = Store::open(&cfg).unwrap();
        drop(store);

        let conn = Connection::open(&db_path).unwrap();
        let actual = dump_schema(&conn);
        let expected = serde_json::json!({
            "user_version": 3,
            "tables": [
                {
                    "name": "devices",
                    "columns": [
                        { "name": "id", "type": "TEXT", "notnull": 0, "pk": 1 },
                        { "name": "name", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "kind", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "addr", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "screen_mode", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "vd_res", "type": "TEXT", "notnull": 0, "pk": 0 },
                        { "name": "vd_dpi", "type": "INTEGER", "notnull": 0, "pk": 0 },
                        { "name": "pkg", "type": "TEXT", "notnull": 0, "pk": 0 },
                        { "name": "fps", "type": "INTEGER", "notnull": 0, "pk": 0 },
                        { "name": "created_at", "type": "TEXT", "notnull": 1, "pk": 0 }
                    ]
                },
                {
                    "name": "logs",
                    "columns": [
                        { "name": "id", "type": "INTEGER", "notnull": 0, "pk": 1 },
                        { "name": "time", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "device_id", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "script_id", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "level", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "msg", "type": "TEXT", "notnull": 1, "pk": 0 }
                    ]
                },
                {
                    "name": "scheduled_runs",
                    "columns": [
                        { "name": "id", "type": "INTEGER", "notnull": 0, "pk": 1 },
                        { "name": "task_id", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "scheduled_at", "type": "INTEGER", "notnull": 1, "pk": 0 },
                        { "name": "state", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "run_id", "type": "TEXT", "notnull": 0, "pk": 0 },
                        { "name": "error", "type": "TEXT", "notnull": 0, "pk": 0 },
                        { "name": "created_at", "type": "TEXT", "notnull": 1, "pk": 0 }
                    ]
                },
                {
                    "name": "task_presets",
                    "columns": [
                        { "name": "id", "type": "TEXT", "notnull": 0, "pk": 1 },
                        { "name": "app_package", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "name", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "runner_id", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "entrypoint", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "payload_json", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "schedule_json", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "created_at", "type": "TEXT", "notnull": 1, "pk": 0 }
                    ]
                },
                {
                    "name": "timer_tasks",
                    "columns": [
                        { "name": "id", "type": "TEXT", "notnull": 0, "pk": 1 },
                        { "name": "name", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "device_id", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "android_package", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "content_package", "type": "TEXT", "notnull": 0, "pk": 0 },
                        { "name": "runner_id", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "entrypoint", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "payload_json", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "schedule_json", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "state", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "enabled", "type": "INTEGER", "notnull": 1, "pk": 0 },
                        { "name": "next_wakeup", "type": "INTEGER", "notnull": 0, "pk": 0 },
                        { "name": "last_result", "type": "TEXT", "notnull": 0, "pk": 0 },
                        { "name": "last_run_at", "type": "TEXT", "notnull": 0, "pk": 0 },
                        { "name": "created_at", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "updated_at", "type": "TEXT", "notnull": 1, "pk": 0 },
                        { "name": "preset_id", "type": "TEXT", "notnull": 0, "pk": 0 },
                        { "name": "suspend_reason", "type": "TEXT", "notnull": 0, "pk": 0 }
                    ]
                }
            ],
            "indexes": [
                {
                    "name": "idx_logs_time",
                    "table": "logs",
                    "unique": 0,
                    "columns": ["time"]
                },
                {
                    "name": "idx_scheduled_runs_created_at",
                    "table": "scheduled_runs",
                    "unique": 0,
                    "columns": ["created_at"]
                },
                {
                    "name": "idx_task_presets_package",
                    "table": "task_presets",
                    "unique": 0,
                    "columns": ["app_package"]
                },
                {
                    "name": "idx_timer_tasks_app",
                    "table": "timer_tasks",
                    "unique": 0,
                    "columns": ["android_package", "content_package"]
                },
                {
                    "name": "idx_timer_tasks_wakeup",
                    "table": "timer_tasks",
                    "unique": 0,
                    "columns": ["state", "enabled", "next_wakeup"]
                },
                {
                    "name": "uq_scheduled_runs_task_time",
                    "table": "scheduled_runs",
                    "unique": 1,
                    "columns": ["task_id", "scheduled_at"]
                }
            ]
        });
        // P11.1：v3 快照是全量精确比对——legacy tasks 表已删除，任何漂移在此失败
        assert_eq!(actual, expected, "新库 schema 与 v3 快照不一致");

        drop(conn);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unsupported_schema_version_is_rejected_without_migration() {
        let (cfg, dir) = temp_config("schema-unsupported");
        let db_path = dir.join("gamer.db");
        let store = Store::open(&cfg).unwrap();
        drop(store);
        let conn = Connection::open(&db_path).unwrap();
        conn.pragma_update(None, "user_version", 4).unwrap();
        drop(conn);

        let error = match Store::open(&cfg) {
            Ok(_) => panic!("unsupported database must fail fast"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("unsupported database schema version 4"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn incomplete_schema_v1_is_rejected_without_repair() {
        let (cfg, dir) = temp_config("schema-incomplete");
        let db_path = dir.join("gamer.db");
        let store = Store::open(&cfg).unwrap();
        drop(store);
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("DROP TABLE logs;").unwrap();
        drop(conn);

        let error = match Store::open(&cfg) {
            Ok(_) => panic!("incomplete database must fail fast"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("schema v3 is incomplete"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prune_logs_deletes_all_eligible_rows_in_batches() {
        let (cfg, dir) = temp_config("prune-batch");
        let store = Store::open(&cfg).unwrap();
        for idx in 0..(LOG_PRUNE_BATCH_SIZE as usize + 3) {
            let second = idx % 60;
            store
                .insert_raw_log_for_test(&format!("2000-01-01 00:00:{second:02}.000"), "old")
                .unwrap();
        }
        // "新日志"时间戳必须动态取当前时间：硬编码日期会随真实时间推移老化出
        // retain_days 窗口（2026-08-28 硬编码在 08-29 起必挂——被 prune 正确删除）
        let now = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
        store.insert_raw_log_for_test(&now, "new").unwrap();

        let deleted = store.prune_logs(1).unwrap();
        assert_eq!(deleted, (LOG_PRUNE_BATCH_SIZE as u64) + 3);
        let logs = store.list_logs(None, None, 10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].msg, "new");

        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn vacuum_reports_sizes_and_store_stays_usable() {
        let (cfg, dir) = temp_config("vacuum");
        let store = Store::open(&cfg).unwrap();
        for i in 0..300 {
            store
                .add_log("d", "s", "info", &format!("log-{i}"))
                .unwrap();
        }
        let report = store.vacuum().unwrap();
        assert!(report.before_bytes > 0, "vacuum 前应有非零文件大小");
        assert!(report.after_bytes > 0, "vacuum 后应有非零文件大小");
        // vacuum 后数据库仍可正常读写
        assert!(store.health_check().is_ok());
        assert!(store.metrics_snapshot().unwrap().logs >= 300);
        // 幂等：连续 VACUUM 不报错
        let again = store.vacuum().unwrap();
        assert!(again.after_bytes > 0);

        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn device_upsert_get_roundtrip() {
        // 回归：get_device 的 SQL 曾因字符串 `\` 续行吞掉行首缩进拼出
        // created_atFROM，设备回读全挂且无测试覆盖（PUT /api/devices/:id 500）
        let (cfg, dir) = temp_config("device-roundtrip");
        let store = Store::open(&cfg).unwrap();
        let device = Device {
            id: "dev-1".into(),
            name: "投屏机".into(),
            kind: "usb".into(),
            addr: "SERIAL123".into(),
            screen_mode: ScreenMode::Virtual,
            vd_res: Some("1920x1080".into()),
            vd_dpi: Some(420),
            pkg: Some("com.example.game".into()),
            fps: Some(30),
            created_at: "2026-08-29 00:00:00".into(),
        };
        store.upsert_device(&device).unwrap();
        let got = store
            .get_device("dev-1")
            .unwrap()
            .expect("device should exist");
        assert_eq!(got.name, "投屏机");
        assert_eq!(got.addr, "SERIAL123");
        assert!(matches!(got.screen_mode, ScreenMode::Virtual));
        assert_eq!(got.vd_res.as_deref(), Some("1920x1080"));
        assert_eq!(got.vd_dpi, Some(420));
        assert_eq!(got.pkg.as_deref(), Some("com.example.game"));
        assert_eq!(got.fps, Some(30));

        // 更新走同一 UPSERT：字段回读一致
        let mut updated = device.clone();
        updated.name = "改名".into();
        updated.fps = Some(60);
        store.upsert_device(&updated).unwrap();
        let got2 = store.get_device("dev-1").unwrap().unwrap();
        assert_eq!(got2.name, "改名");
        assert_eq!(got2.fps, Some(60));
        assert_eq!(
            got2.created_at, "2026-08-29 00:00:00",
            "created_at 不应被覆盖"
        );

        assert!(store.get_device("missing").unwrap().is_none());
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    /// DATA-002 schema 快照转储：user_version + 全部表（列名/类型/NOT NULL/PK，
    /// 按声明序）+ 全部显式索引（唯一性、列序，按名称排序）。只转储确定形态，
    /// 内部 sqlite_autoindex_*（TEXT PK 隐式索引）不进快照。
    fn dump_schema(conn: &Connection) -> serde_json::Value {
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let mut tables = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for table in names {
            let mut columns = Vec::new();
            let mut cs = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            let rows = cs
                .query_map([], |row| {
                    Ok(serde_json::json!({
                        "name": row.get::<_, String>(1)?,
                        "type": row.get::<_, String>(2)?,
                        "notnull": row.get::<_, i64>(3)?,
                        "pk": row.get::<_, i64>(5)?,
                    }))
                })
                .unwrap();
            for row in rows {
                columns.push(row.unwrap());
            }
            tables.push(serde_json::json!({ "name": table, "columns": columns }));
        }

        let mut indexes = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT name, tbl_name FROM sqlite_master WHERE type = 'index' \
                 AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap();
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for (name, table) in rows {
            let unique: i64 = conn
                .query_row(
                    "SELECT \"unique\" FROM pragma_index_list(?) WHERE name = ?",
                    rusqlite::params![table, name],
                    |row| row.get(0),
                )
                .unwrap();
            let mut columns = Vec::new();
            let mut cs = conn.prepare(&format!("PRAGMA index_info({name})")).unwrap();
            let rows = cs.query_map([], |row| row.get::<_, String>(2)).unwrap();
            for row in rows {
                columns.push(row.unwrap());
            }
            indexes.push(serde_json::json!({
                "name": name,
                "table": table,
                "unique": unique,
                "columns": columns,
            }));
        }

        serde_json::json!({ "user_version": version, "tables": tables, "indexes": indexes })
    }

    #[tokio::test]
    async fn bounded_blocking_helper_limits_parallelism() {
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..6 {
            let active = active.clone();
            let max_active = max_active.clone();
            handles.push(tokio::spawn(async move {
                Store::with_bounded_blocking(move || {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    loop {
                        let current = max_active.load(Ordering::SeqCst);
                        if now <= current {
                            break;
                        }
                        if max_active
                            .compare_exchange(current, now, Ordering::SeqCst, Ordering::SeqCst)
                            .is_ok()
                        {
                            break;
                        }
                    }
                    std::thread::sleep(StdDuration::from_millis(50));
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok::<_, anyhow::Error>(())
                })
                .await
                .unwrap()
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        assert!(max_active.load(Ordering::SeqCst) <= DB_BLOCKING_PERMITS);
    }

    /// Phase 0 基线补充：logs 表高压写入延迟（add_log_async critical 路径，
    /// 端到端含 channel 排队 + 事务提交）。8 并发 × 250 条 error 级日志，
    /// 每条 critical 日志带 completion 并触发批量 flush，测每次调用的墙钟
    /// 延迟 p50/p95/max。默认 #[ignore]，仅基线采集显式运行；本测试只写
    /// 临时目录数据库，不影响仓库数据。
    #[tokio::test]
    #[ignore = "仅 Phase 0 基线采集时运行：cargo test --release db_log -- --ignored --nocapture"]
    async fn db_log_high_pressure_write_latency_p50_p95() {
        let (cfg, dir) = temp_config("dblog-bench");
        let store = Arc::new(Store::open(&cfg).unwrap());
        let concurrency = 8usize;
        let per_task = 250usize;
        let mut handles = Vec::new();
        for task_id in 0..concurrency {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                let mut samples = Vec::with_capacity(per_task);
                for i in 0..per_task {
                    let started = std::time::Instant::now();
                    store
                        .add_log_async(
                            "bench-device",
                            "bench-script",
                            "error",
                            &format!("perf-load t{task_id} n{i}"),
                        )
                        .await
                        .expect("critical log write failed");
                    samples.push(started.elapsed().as_nanos() / 1000);
                }
                samples
            }));
        }
        let mut all = Vec::new();
        for handle in handles {
            all.extend(handle.await.unwrap());
        }
        all.sort_unstable();
        let total = all.len();
        let p = |ratio: f64| -> u128 {
            let rank = ((total as f64) * ratio).ceil() as usize;
            all[(rank - 1).min(total - 1)]
        };
        println!(
            "PERF metric=db_log_write samples={} p50_us={} p95_us={} max_us={} concurrency={} platform={}",
            total,
            p(0.50),
            p(0.95),
            all[total - 1],
            concurrency,
            std::env::consts::OS,
        );
        drop(store);
        fs::remove_dir_all(dir).ok();
    }
}
