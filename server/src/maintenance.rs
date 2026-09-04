//! Maintenance CLI（DATA-005 / release/contracts/schema-policy.md §7）。
//!
//! ```text
//! gamer-server inspect  [--data-dir <path>] [--json]
//! gamer-server migrate --data-dir <path> [--json]
//! ```
//!
//! 在 `main` 最前分支：任何 adb / scheduler / HTTP / 设备扫描 / DeviceManager
//! 初始化之前执行完即退出——零后台服务（契约 §7 硬约束）。inspect 是只读
//! 诊断（DB 不存在判 `missing` 且**不创建文件**）；migrate 只对兼容区间内
//! 缺失版本逐级补迁移，正常结束执行 `wal_checkpoint(TRUNCATE)` 保证主库文件
//! 自洽（不依赖 -wal/-shm 即可被快照或旧 binary 打开）。无子命令时返回
//! None，main 走既有启动流程，行为逐字节不变。
//!
//! 兼容判定与启动路径共用同一实现（`store::db_path` / `migrations::run_migrations`
//! / `store::validate_schema_v1`），保证 launcher preflight（数据副本上
//! inspect+migrate）与实迁结论一致（契约 §7）。
//!
//! 退出码（契约 §7「错误经非 0 退出码区分」）：
//! - `0` = 报告产出且状态 ok / needs_migration
//! - `3` = too_new（数据库比 binary 新）
//! - `4` = unversioned（user_version=0 无版本旧库）
//! - `5` = missing（数据库文件不存在；inspect 不建库，migrate 拒绝执行）
//! - `6` = 迁移/校验执行失败（该级整体回滚、可重试）
//! - `2` = 用法错误
//!
//! `--json` 输出结构化报告（human 可读模式输出同样的字段逐行文本）；错误
//! 信息不含本机路径之外的敏感内容（数据目录本身是 CLI 调用方显式指定的）。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::json;

use crate::migrations::{self, MAX_READ_SCHEMA, MIGRATIONS, MIN_READ_SCHEMA, TARGET_SCHEMA};

/// inspect 的五态判定（契约 §7；version=0 → unversioned，> max → too_new，
/// < target → needs_migration，其余 → ok）
pub(crate) fn classify(version: i64, max_read: i64, target: i64) -> &'static str {
    if version == 0 {
        "unversioned"
    } else if version > max_read {
        "too_new"
    } else if version < target {
        "needs_migration"
    } else {
        "ok"
    }
}

/// 解析出的子命令。`--data-dir` 缺省时按 PATH-001 配置解析规则回落。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Inspect {
        data_dir: Option<PathBuf>,
        json: bool,
    },
    Migrate {
        data_dir: Option<PathBuf>,
        json: bool,
    },
}

/// 解析命令行参数：`Ok(None)` = 无维护子命令（保持既有启动流程）；
/// `Err` = 用法错误（exit 2）。只识别显式的 inspect/migrate 首参，
/// 其余参数形态与既有服务端启动完全兼容。
pub fn parse_args(args: &[String]) -> Result<Option<Command>, String> {
    let Some(first) = args.get(1).map(String::as_str) else {
        return Ok(None);
    };
    let command = match first {
        "inspect" => Command::Inspect {
            data_dir: None,
            json: false,
        },
        "migrate" => Command::Migrate {
            data_dir: None,
            json: false,
        },
        _ => return Ok(None),
    };
    let mut command = command;
    let mut seen_data_dir = false;
    let mut seen_json = false;
    let mut i = 2;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--json" => {
                if seen_json {
                    return Err(usage("--json 重复"));
                }
                seen_json = true;
                set_json(&mut command);
            }
            "--data-dir" => {
                if seen_data_dir {
                    return Err(usage("--data-dir 重复"));
                }
                let Some(value) = args.get(i + 1) else {
                    return Err(usage("--data-dir 缺少路径参数"));
                };
                if value.starts_with("--") {
                    return Err(usage("--data-dir 缺少路径参数"));
                }
                seen_data_dir = true;
                set_data_dir(&mut command, PathBuf::from(value));
                i += 1;
            }
            other => {
                if let Some(value) = other.strip_prefix("--data-dir=") {
                    if seen_data_dir {
                        return Err(usage("--data-dir 重复"));
                    }
                    seen_data_dir = true;
                    set_data_dir(&mut command, PathBuf::from(value));
                } else {
                    return Err(usage(&format!("未知参数 {other:?}")));
                }
            }
        }
        i += 1;
    }
    Ok(Some(command))
}

fn usage(reason: &str) -> String {
    format!(
        "{reason}\n用法:\n  gamer-server inspect  [--data-dir <path>] [--json]\n  gamer-server migrate --data-dir <path> [--json]"
    )
}

fn set_json(command: &mut Command) {
    match command {
        Command::Inspect { json, .. } | Command::Migrate { json, .. } => *json = true,
    }
}

fn set_data_dir(command: &mut Command, path: PathBuf) {
    match command {
        Command::Inspect { data_dir, .. } | Command::Migrate { data_dir, .. } => {
            *data_dir = Some(path)
        }
    }
}

/// CLI 入口：产出报告、打印、返回退出码（不经 std::process::exit，
/// 由 main 决定退出方式，保证测试可直接驱动）
pub fn run_cli(command: Command) -> i32 {
    match command {
        Command::Inspect { data_dir, json } => {
            let Some(data_dir) = resolve_data_dir(data_dir) else {
                return print_error(json, "cannot resolve data dir (config load failed)");
            };
            let report = inspect(&data_dir);
            print_report(&report, json);
            exit_code(report["status"].as_str().unwrap_or_default())
        }
        Command::Migrate { data_dir, json } => {
            let Some(data_dir) = resolve_data_dir(data_dir) else {
                return print_error(json, "cannot resolve data dir (config load failed)");
            };
            let report = migrate(&data_dir);
            print_report(&report, json);
            exit_code(report["status"].as_str().unwrap_or_default())
        }
    }
}

fn resolve_data_dir(explicit: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(dir) = explicit {
        return Some(dir);
    }
    // 契约 §7：--data-dir 缺省按 PATH-001 配置解析规则回落
    crate::config::Config::load()
        .ok()
        .map(|loaded| loaded.cfg.data_dir)
}

fn exit_code(status: &str) -> i32 {
    match status {
        "ok" | "needs_migration" => 0,
        "too_new" => 3,
        "unversioned" => 4,
        "missing" => 5,
        _ => 6, // 迁移/校验执行失败
    }
}

fn print_report(report: &serde_json::Value, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(report).unwrap_or_default()
        );
    } else {
        match report["command"].as_str() {
            Some("inspect") => println!(
                "data_dir: {}\ndb_exists: {}\nuser_version: {}\nmin_read_schema: {}\nmax_read_schema: {}\ntarget_schema: {}\nstatus: {}\npending_migrations: {}\nfile_layout_v1: {}",
                report["data_dir"],
                report["db_exists"],
                report["user_version"],
                report["schema"]["min_read_schema"],
                report["schema"]["max_read_schema"],
                report["schema"]["target_schema"],
                report["status"],
                report["pending_migrations"],
                report["file_layout_v1"],
            ),
            _ => println!(
                "from: {}\nto: {}\napplied: {}\nok: {}\nstatus: {}\nerror: {}",
                report["from"],
                report["to"],
                report["applied"],
                report["ok"],
                report["status"],
                report["error"],
            ),
        }
    }
}

fn print_error(json: bool, message: &str) -> i32 {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "ok": false, "error": message }))
                .unwrap_or_default()
        );
    } else {
        eprintln!("{message}");
    }
    6
}

/// 兼容常量的 JSON 形态（inspect/migrate 报告共用同一取值源）
fn schema_constants() -> serde_json::Value {
    json!({
        "min_read_schema": MIN_READ_SCHEMA,
        "max_read_schema": MAX_READ_SCHEMA,
        "target_schema": TARGET_SCHEMA,
    })
}

/// 只读诊断（契约 §7）：任何情况下不写数据；DB 不存在 → missing 且不创建
/// 数据库文件（只读打开 + 缺席预判双重保证）。
pub(crate) fn inspect(data_dir: &Path) -> serde_json::Value {
    let db_path = crate::store::db_path(data_dir);
    let missing_report = |db_exists: bool, error: Option<String>| {
        let mut report = json!({
            "command": "inspect",
            "data_dir": data_dir.display().to_string(),
            "db_exists": db_exists,
            "user_version": serde_json::Value::Null,
            "schema": schema_constants(),
            "status": "missing",
            "pending_migrations": [],
            "file_layout_v1": file_layout_v1_ok(data_dir),
        });
        if let Some(error) = error {
            report["error"] = serde_json::Value::String(error);
        }
        report
    };

    if !db_path.exists() {
        return missing_report(false, None);
    }

    // 只读优先；运行中服务的 WAL 库在无 -shm 时只读打开可能失败，此时退回
    // 读写打开但绝不执行写操作（user_version 读取不落盘）
    let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .or_else(|_| Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_WRITE));
    let conn = match conn {
        Ok(conn) => conn,
        Err(error) => return missing_report(true, Some(format!("cannot open database: {error}"))),
    };
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap_or(0);
    let status = classify(version, MAX_READ_SCHEMA, TARGET_SCHEMA);
    let pending: Vec<serde_json::Value> = if status == "needs_migration" {
        MIGRATIONS
            .iter()
            .filter(|m| m.from >= version)
            .map(|m| json!({ "from": m.from, "to": m.to, "description": m.description }))
            .collect()
    } else {
        Vec::new()
    };
    json!({
        "command": "inspect",
        "data_dir": data_dir.display().to_string(),
        "db_exists": true,
        "user_version": version,
        "schema": schema_constants(),
        "status": status,
        "pending_migrations": pending,
        "file_layout_v1": file_layout_v1_ok(data_dir),
    })
}

/// 执行缺失迁移（契约 §7）：对 `[min, target)` 内缺失版本逐级执行；missing /
/// unversioned / too_new 拒绝执行且不改数据；正常结束 `wal_checkpoint(TRUNCATE)`。
pub(crate) fn migrate(data_dir: &Path) -> serde_json::Value {
    let db_path = crate::store::db_path(data_dir);
    if !db_path.exists() {
        // missing：拒绝执行，绝不创建数据库文件（OpenFlags 无 CREATE 双保险）
        return json!({
            "command": "migrate",
            "from": serde_json::Value::Null,
            "to": TARGET_SCHEMA,
            "applied": [],
            "ok": false,
            "status": "missing",
            "error": "database file does not exist; migrate never creates a database",
        });
    }
    let conn = match Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_WRITE) {
        Ok(conn) => conn,
        Err(error) => {
            return json!({
                "command": "migrate",
                "from": serde_json::Value::Null,
                "to": TARGET_SCHEMA,
                "applied": [],
                "ok": false,
                "status": "missing",
                "error": format!("cannot open database: {error}"),
            });
        }
    };
    let mut conn = conn;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap_or(0);
    let status = classify(version, MAX_READ_SCHEMA, TARGET_SCHEMA);
    if status == "unversioned" || status == "too_new" {
        // 与启动拒绝路径同一判定与诊断信息（契约 §7）
        let error = if status == "unversioned" {
            "database schema is unversioned (user_version=0); back up and remove gamer.db to rebuild schema v1".to_string()
        } else {
            format!(
                "unsupported database schema version {version}: newer than the highest version \
                 this binary supports (supported range [{MIN_READ_SCHEMA}, {MAX_READ_SCHEMA}], \
                 target {TARGET_SCHEMA}); downgrade is not supported; restore the snapshot taken \
                 before the upgrade"
            )
        };
        return json!({
            "command": "migrate",
            "from": version,
            "to": TARGET_SCHEMA,
            "applied": [],
            "ok": false,
            "status": status,
            "error": error,
        });
    }

    let applied: Vec<serde_json::Value> = MIGRATIONS
        .iter()
        .filter(|m| m.from >= version)
        .map(|m| json!({ "from": m.from, "to": m.to }))
        .collect();
    // 与生产启动路径共用同一迁移实现 + 同一结构校验
    match migrations::run_migrations(&mut conn, version, MIGRATIONS)
        .and_then(|()| crate::store::validate_schema_v1(&conn))
    {
        Ok(()) => {
            // 契约 §2/§7：正常结束执行 wal_checkpoint(TRUNCATE)，主库文件自洽
            let checkpoint = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            });
            json!({
                "command": "migrate",
                "from": version,
                "to": TARGET_SCHEMA,
                "applied": applied,
                "ok": true,
                "status": "ok",
                "checkpointed": checkpoint.is_ok(),
            })
        }
        Err(error) => json!({
            "command": "migrate",
            "from": version,
            "to": TARGET_SCHEMA,
            "applied": [],
            "ok": false,
            "status": "error",
            "error": format!("{error:#}"),
        }),
    }
}

/// 文件布局 v1 符合性（诊断用，宽松口径）：数据目录存在，且每个应用分区
/// 子目录内的子目录名都落在 {scripts, functions, templates, keymaps, presets,
/// resources} 白名单内。数据目录不存在 → null；散落文件（gamer.db 等）不计违规。
fn file_layout_v1_ok(data_dir: &Path) -> serde_json::Value {
    const RESOURCE_DIRS: [&str; 6] = [
        "scripts",
        "functions",
        "templates",
        "keymaps",
        "presets",
        "resources",
    ];
    if !data_dir.is_dir() {
        return serde_json::Value::Null;
    }
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return serde_json::Value::Null;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(sub) = std::fs::read_dir(&path) else {
            continue;
        };
        for item in sub.flatten() {
            let name = item.file_name().to_string_lossy().to_string();
            if item.path().is_dir() && !RESOURCE_DIRS.contains(&name.as_str()) {
                return serde_json::Value::Bool(false);
            }
        }
    }
    serde_json::Value::Bool(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("gamer-server".to_string())
            .chain(list.iter().map(|s| s.to_string()))
            .collect()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gamer-maint-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn no_subcommand_keeps_existing_startup_flow() {
        assert_eq!(parse_args(&args(&[])).unwrap(), None);
        assert_eq!(parse_args(&args(&["--help"])).unwrap(), None);
        assert_eq!(parse_args(&args(&["serve", "--x"])).unwrap(), None);
    }

    #[test]
    fn subcommands_parse_with_flags() {
        let dir = temp_dir("parse");
        assert_eq!(
            parse_args(&args(&["inspect"])).unwrap(),
            Some(Command::Inspect {
                data_dir: None,
                json: false
            })
        );
        assert_eq!(
            parse_args(&args(&[
                "inspect",
                "--data-dir",
                dir.to_str().unwrap(),
                "--json"
            ]))
            .unwrap(),
            Some(Command::Inspect {
                data_dir: Some(dir.clone()),
                json: true
            })
        );
        assert_eq!(
            parse_args(&args(&[
                "migrate",
                &format!("--data-dir={}", dir.display()),
                "--json"
            ]))
            .unwrap(),
            Some(Command::Migrate {
                data_dir: Some(dir.clone()),
                json: true
            })
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_flags_are_usage_errors() {
        assert!(parse_args(&args(&["inspect", "--data-dir"])).is_err());
        assert!(parse_args(&args(&["inspect", "--data-dir", "--json"])).is_err());
        assert!(parse_args(&args(&["migrate", "--nope"])).is_err());
        assert!(parse_args(&args(&["inspect", "--json", "--json"])).is_err());
    }

    #[test]
    fn missing_database_is_reported_and_never_created() {
        let dir = temp_dir("missing");
        let report = inspect(&dir);
        assert_eq!(report["status"], "missing");
        assert_eq!(report["db_exists"], false);
        assert_eq!(report["user_version"], serde_json::Value::Null);
        assert_eq!(report["schema"]["target_schema"], TARGET_SCHEMA);
        // inspect 不建库
        assert!(!crate::store::db_path(&dir).exists());
        // migrate 同样拒绝且不建库
        let report = migrate(&dir);
        assert_eq!(report["status"], "missing");
        assert_eq!(report["ok"], false);
        assert!(!crate::store::db_path(&dir).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fresh_v1_database_inspects_ok_and_migrates_noop() {
        let dir = temp_dir("ok");
        let cfg = crate::config::Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let store = crate::store::Store::open(&cfg).unwrap();
        drop(store);

        let report = inspect(&dir);
        assert_eq!(report["status"], "ok");
        assert_eq!(report["user_version"], TARGET_SCHEMA);
        assert_eq!(report["pending_migrations"], serde_json::json!([]));

        let report = migrate(&dir);
        assert_eq!(report["ok"], true);
        assert_eq!(report["from"], TARGET_SCHEMA);
        assert_eq!(report["to"], TARGET_SCHEMA);
        assert_eq!(report["applied"], serde_json::json!([]));
        assert_eq!(report["status"], "ok");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unversioned_and_too_new_are_rejected() {
        // unversioned
        let dir = temp_dir("unversioned");
        let db_path = crate::store::db_path(&dir);
        rusqlite::Connection::open(&db_path).unwrap();
        let report = inspect(&dir);
        assert_eq!(report["status"], "unversioned");
        let report = migrate(&dir);
        assert_eq!(report["status"], "unversioned");
        assert_eq!(report["ok"], false);
        let version: i64 = rusqlite::Connection::open(&db_path)
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, 0, "migrate 拒绝后不得改写 user_version");
        std::fs::remove_dir_all(dir).unwrap();

        // too_new
        let dir = temp_dir("too-new");
        let db_path = crate::store::db_path(&dir);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE future (x TEXT); PRAGMA user_version = 4;")
            .unwrap();
        drop(conn);
        let report = inspect(&dir);
        assert_eq!(report["status"], "too_new");
        let report = migrate(&dir);
        assert_eq!(report["status"], "too_new");
        assert_eq!(report["ok"], false);
        let err = report["error"].as_str().unwrap();
        assert!(err.contains("supported range [1, 3]"), "{err}");
        let version: i64 = rusqlite::Connection::open(&db_path)
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, 4, "too_new 拒绝后不得改写数据");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn exit_codes_distinguish_outcomes() {
        assert_eq!(exit_code("ok"), 0);
        assert_eq!(exit_code("needs_migration"), 0);
        assert_eq!(exit_code("too_new"), 3);
        assert_eq!(exit_code("unversioned"), 4);
        assert_eq!(exit_code("missing"), 5);
        assert_eq!(exit_code("error"), 6);
    }

    #[test]
    fn classify_covers_five_states() {
        assert_eq!(classify(1, 1, 1), "ok");
        assert_eq!(classify(0, 1, 1), "unversioned");
        assert_eq!(classify(2, 1, 1), "too_new");
        // 未来形态（max=2/target=2）：v1 库需要迁移
        assert_eq!(classify(1, 2, 2), "needs_migration");
    }

    #[test]
    fn file_layout_check_flags_foreign_subdirs_only() {
        let dir = temp_dir("layout");
        // 空目录 + db 文件：合规
        std::fs::write(crate::store::db_path(&dir), b"x").unwrap();
        assert_eq!(file_layout_v1_ok(&dir), serde_json::Value::Bool(true));
        // 合规分区
        let pkg = dir.join("com.example.game");
        std::fs::create_dir_all(pkg.join("scripts")).unwrap();
        std::fs::create_dir_all(pkg.join("functions")).unwrap();
        std::fs::create_dir_all(pkg.join("templates")).unwrap();
        assert_eq!(file_layout_v1_ok(&dir), serde_json::Value::Bool(true));
        // 分区内出现白名单外子目录 → 违规
        std::fs::create_dir_all(pkg.join("old_scripts")).unwrap();
        assert_eq!(file_layout_v1_ok(&dir), serde_json::Value::Bool(false));
        // 数据目录不存在 → null
        assert_eq!(
            file_layout_v1_ok(&dir.join("no-such")),
            serde_json::Value::Null
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
