//! 服务端日志基础设施（OPS-003：文件日志按天轮转 + 非阻塞 worker 落盘）
//!
//! 形态由环境变量 `GB_LOG` 决定：
//! - **stdout**：未设置、留空、或值恰为 `stdout`（大小写不敏感）时走纯 stdout——
//!   容器部署天然处于此形态（镜像里不设 GB_LOG），轮转与保留全权交给容器日志驱动；
//! - **滚动文件**：其余值视作基准路径（gamer.ps1 传入 `server/gamer-server.log`），
//!   实际写出 `<目录>/<文件名>.YYYY-MM-DD`（如 `gamer-server.log.2026-08-27`）按天翻新，
//!   不再存在单文件无限增长问题；旧版的"单文件追加"模式已移除；
//! - 文件模式下磁盘 IO 全部由独立 worker 线程承担（`tracing_appender::non_blocking`）：
//!   业务/Tokio 线程只入内存缓冲不等磁盘；返回的 `WorkerGuard` 必须绑定在 `main`
//!   栈帧上直至进程退出，由其 drop 冲刷残余日志。
//!
//! 保留策略：删除超出保留窗口的旧日期轮转文件（`config.toml` 的 `log_retain_days`，
//! 默认 14 个自然日含今天；0 = 永不清理）。启动时清理一次，此后每到本地零点顺带
//! 再清一次。只有严格命中 `<前缀>.YYYY-MM-DD` 命名的文件才会被识别和删除，
//! 其余文件（含旧版无日期的单文件）一律不动。

use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use chrono::Days;
use chrono::Local;
use chrono::NaiveDate;
use chrono::Timelike;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::RollingFileAppender;
use tracing_appender::rolling::Rotation;
use tracing_subscriber::EnvFilter;

/// GB_LOG 的"强制 stdout"关键字（大小写不敏感）
const STDOUT_KEYWORD: &str = "stdout";
/// RUST_LOG 未设置时的过滤等级（行为与旧实现一致）
const DEFAULT_FILTER: &str = "info";

/// 日志最终形态（供启动摘要展示）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogTarget {
    Stdout,
    RollingFile { dir: PathBuf, prefix: String },
}

/// 清理结果
#[derive(Debug, Default)]
pub struct PruneOutcome {
    /// 已删除的过期轮转文件
    pub deleted: Vec<PathBuf>,
    /// 删除失败（路径, 错误说明）；单个失败不中断其余文件的清理
    pub failures: Vec<(PathBuf, String)>,
}

/// 初始化全局 tracing 订阅器。
///
/// `retain_days`：滚动文件保留天数（仅滚动文件形态消费；0 = 不清理）。
/// 返回 `(最终形态, 可选的 WorkerGuard)`——stdout 形态时 guard 为 None。
/// 必须在 Tokio 运行时内调用（滚动形态会启动每日零点清理任务）。
pub fn init(retain_days: u32) -> anyhow::Result<(LogTarget, Option<WorkerGuard>)> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| DEFAULT_FILTER.into());

    let gb_log = std::env::var("GB_LOG").ok();
    let base = gb_log.as_deref().map(str::trim).filter(|s| !s.is_empty());

    match base {
        Some(raw) if !raw.eq_ignore_ascii_case(STDOUT_KEYWORD) => {
            let (dir, prefix) = split_base_path(Path::new(raw));
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(&dir)
                    .with_context(|| format!("cannot create log dir {}", dir.display()))?;
            }
            // Rotation::DAILY 无需显式日期后缀：默认即产出 `<prefix>.YYYY-MM-DD`
            let appender = RollingFileAppender::builder()
                .rotation(Rotation::DAILY)
                .filename_prefix(&prefix)
                .build(&dir)
                .with_context(|| {
                    format!(
                        "cannot init daily rotating log (dir={}, prefix={prefix})",
                        dir.display()
                    )
                })?;
            let (writer, guard) = tracing_appender::non_blocking(appender);
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                // 文件日志剥离 ANSI 色码，便于 grep 与归档分析
                .with_ansi(false)
                .with_writer(writer)
                .init();

            let target = LogTarget::RollingFile {
                dir: dir.clone(),
                prefix: prefix.clone(),
            };

            // 启动清理一次（订阅器已就位，删除明细直接进日志）；后台任务管翻日后的日常清理
            report_prune(prune_rotated_logs(
                &dir,
                &prefix,
                retain_days,
                Local::now().date_naive(),
            ));
            spawn_retention_loop(dir, prefix, retain_days);
            Ok((target, Some(guard)))
        }
        _ => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
            Ok((LogTarget::Stdout, None))
        }
    }
}

/// 把 GB_LOG 给出的基准路径拆为（目录, 文件名前缀）：
/// `...\server\gamer-server.log` → (`...\server`, `gamer-server.log`)；
/// 无目录成分时回退当前目录；无法取得文件名时按惯例回退 `gamer-server.log`。
fn split_base_path(base: &Path) -> (PathBuf, String) {
    match (base.parent(), base.file_name().and_then(|n| n.to_str())) {
        (Some(parent), Some(name)) if !name.is_empty() => {
            let dir = if parent.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                parent.to_path_buf()
            };
            (dir, name.to_string())
        }
        _ => (PathBuf::from("."), "gamer-server.log".into()),
    }
}

/// 打印一次清理报告（订阅器就位后才调用）
fn report_prune(result: io::Result<PruneOutcome>) {
    match result {
        Ok(o) => {
            if !o.deleted.is_empty() {
                tracing::info!(count = o.deleted.len(), "expired rotated log files removed");
                for p in &o.deleted {
                    tracing::debug!(file = %p.display(), "removed");
                }
            }
            for (p, e) in &o.failures {
                tracing::warn!(file = %p.display(), err = %e, "failed to remove rotated log");
            }
        }
        Err(e) => tracing::warn!(err = %e, "scanning rotated log files failed"),
    }
}

/// 删除早于保留窗口的按天轮转日志文件。
///
/// 只有严格命中 `<prefix>.<YYYY-MM-DD>` 的文件会被动；保留窗口含今天共
/// `keep_days` 个自然日，`keep_days == 0` 表示关闭清理。`today` 由调用方注入，
/// 便于单元测试模拟时间推进而不必真跑一天。
pub fn prune_rotated_logs(
    dir: &Path,
    prefix: &str,
    keep_days: u32,
    today: NaiveDate,
) -> io::Result<PruneOutcome> {
    let mut outcome = PruneOutcome::default();
    if keep_days == 0 {
        return Ok(outcome);
    }
    // 过期判定：日期严格早于 today-(keep_days-1)；u32 巨大时饱和到最早可表日期
    let cutoff = today
        .checked_sub_days(Days::new(u64::from(keep_days.saturating_sub(1))))
        .unwrap_or(NaiveDate::MIN);

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(outcome),
        Err(e) => return Err(e),
    };
    let mut expired = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(date) = name.to_str().and_then(|n| rotated_file_date(n, prefix)) else {
            continue; // 命名不符或日期不可解析：非轮转产物，一概不动
        };
        if date < cutoff {
            expired.push(entry.path());
        }
    }
    expired.sort();
    for path in expired {
        match fs::remove_file(&path) {
            Ok(()) => outcome.deleted.push(path),
            Err(e) => outcome.failures.push((path, e.to_string())),
        }
    }
    Ok(outcome)
}

/// 从文件名提取轮转日期：仅当形如 `<prefix>.<%Y-%m-%d 共 10 字符>` 才返回 Some
fn rotated_file_date(name: &str, prefix: &str) -> Option<NaiveDate> {
    let rest = name.strip_prefix(prefix)?.strip_prefix('.')?;
    if rest.len() != 10 || !rest.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        return None;
    }
    NaiveDate::parse_from_str(rest, "%Y-%m-%d").ok()
}

/// 后台循环：每过一个本地零点顺带清理一次；与启动清理同套规则。
fn spawn_retention_loop(dir: PathBuf, prefix: String, retain_days: u32) {
    if retain_days == 0 {
        return; // 与启动清理口径一致：0 视为关闭保留策略
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(duration_until_next_local_midnight()).await;
            let today = Local::now().date_naive();
            report_prune(prune_rotated_logs(&dir, &prefix, retain_days, today));
        }
    });
}

/// 距下一个本地零点的秒数（多加 5s 保证醒来时已明确越过零点边界）
fn duration_until_next_local_midnight() -> Duration {
    let secs = u64::from(Local::now().time().num_seconds_from_midnight());
    Duration::from_secs(86_400 - secs + 5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    const PREFIX: &str = "gamer-server.log";

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "gamer-logtest-{}-{tag}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &Path, name: &str) {
        fs::write(dir.join(name), b"log-line\n").unwrap();
    }

    fn dated_name(y: i32, m: u32, d: u32) -> String {
        format!("{PREFIX}.{y}-{m:02}-{d:02}")
    }

    /// 保留窗口边界 + 无关文件安全性的综合校验（"旧文件不被写穿"等价断言：
    /// 窗口内与活动日文件原样保留，仅窗口外严格命名的轮转产物被删）
    #[test]
    fn prune_keeps_window_and_removes_expired_only() {
        let dir = temp_dir("window");
        let today = NaiveDate::from_ymd_opt(2026, 8, 27).unwrap();

        // 窗口外（应删）
        touch(&dir, &dated_name(2026, 7, 28));
        touch(&dir, &dated_name(2026, 8, 12));
        touch(&dir, &dated_name(2026, 8, 13));
        // 窗口内（应留）：14 天窗 = 08-14 .. 08-27 含今天
        touch(&dir, &dated_name(2026, 8, 14));
        touch(&dir, &dated_name(2026, 8, 26));
        touch(&dir, &dated_name(2026, 8, 27));
        // 不得误伤的三类
        touch(&dir, PREFIX); // 旧版无日期单文件
        touch(&dir, "unrelated.txt"); // 前缀不符
        touch(&dir, "other.log.2026-01-01"); // 他名前缀的历史产物

        let out = prune_rotated_logs(&dir, PREFIX, 14, today).unwrap();
        assert_eq!(out.deleted.len(), 3, "deleted: {:?}", out.deleted);
        assert!(out.failures.is_empty(), "failures: {:?}", out.failures);

        assert!(
            dir.join(dated_name(2026, 8, 14)).exists(),
            "窗口最老一天应保留"
        );
        assert!(dir.join(dated_name(2026, 8, 26)).exists());
        assert!(
            dir.join(dated_name(2026, 8, 27)).exists(),
            "今天的活动文件绝不能删"
        );
        assert!(!dir.join(dated_name(2026, 8, 13)).exists());
        assert!(dir.join(PREFIX).exists());
        assert!(dir.join("unrelated.txt").exists());
        assert!(dir.join("other.log.2026-01-01").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_single_day_window_and_zero_disable() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 27).unwrap();

        // keep_days=1：只剩今天，昨天起全删
        let dir = temp_dir("oneday");
        touch(&dir, &dated_name(2026, 8, 26));
        touch(&dir, &dated_name(2026, 8, 27));
        let out = prune_rotated_logs(&dir, PREFIX, 1, today).unwrap();
        assert_eq!(out.deleted.len(), 1);
        assert!(dir.join(dated_name(2026, 8, 27)).exists());
        let _ = fs::remove_dir_all(&dir);

        // keep_days=0：彻底关闭，远古文件也不动
        let dir = temp_dir("zerodays");
        touch(&dir, &dated_name(1900, 1, 1));
        let out = prune_rotated_logs(&dir, PREFIX, 0, today).unwrap();
        assert!(out.deleted.is_empty());
        assert!(dir.join(dated_name(1900, 1, 1)).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_handles_missing_dir_gracefully() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 27).unwrap();
        let ghost = std::env::temp_dir().join("gamer-logtest-definitely-missing");
        let out = prune_rotated_logs(&ghost, PREFIX, 14, today).unwrap();
        assert!(out.deleted.is_empty());
    }

    /// 集成性小验证：真实 RollingFileAppender（daily）确实产出 `<prefix>.<今天>`
    /// 的按天命名文件，且经非阻塞 worker 正常落盘——"翻日出新文件"的日期推进
    /// 由 tracing-appender 驱动，这里验证当日文件的命名与写入链路；配合上面的
    /// 注入日期清理测试覆盖完整生命周期。
    #[test]
    fn rolling_appender_produces_daily_named_file() {
        let dir = temp_dir("roll");
        let today = Local::now().date_naive();
        let appender = RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix(PREFIX)
            .build(&dir)
            .unwrap();
        let (mut writer, guard) = tracing_appender::non_blocking(appender);
        {
            use std::io::Write as _;
            writeln!(writer, "line-via-nonblocking-worker").unwrap();
        }
        drop(writer);
        drop(guard); // drop 即通知 worker 收尾并冲刷残余

        let expect = dir.join(dated_name(today.year(), today.month(), today.day()));
        let mut found = false;
        for _ in 0..60 {
            if let Ok(content) = fs::read_to_string(&expect) {
                if content.contains("line-via-nonblocking-worker") {
                    found = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            found,
            "expected {} to contain the test line",
            expect.display()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn splits_base_paths() {
        let (dir, prefix) = split_base_path(Path::new("somewhere/server/gamer-server.log"));
        assert_eq!(prefix, "gamer-server.log");
        assert_eq!(dir, PathBuf::from("somewhere/server"));

        let (dir, prefix) = split_base_path(Path::new("gamer-server.log"));
        assert_eq!(prefix, "gamer-server.log");
        assert_eq!(dir, PathBuf::from("."));
    }

    #[test]
    fn rotated_date_parsing_is_strict() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 27).unwrap();
        assert_eq!(
            rotated_file_date("gamer-server.log.2026-08-27", PREFIX),
            Some(today)
        );
        assert_eq!(rotated_file_date("gamer-server.log", PREFIX), None);
        assert_eq!(rotated_file_date("gamer-server.log.notadate", PREFIX), None);
        assert_eq!(rotated_file_date("gamer-server.log.2026-8-7", PREFIX), None);
        assert_eq!(rotated_file_date("x-server.log.2026-08-27", PREFIX), None);
        assert_eq!(
            rotated_file_date("gamer-server.log.9999-99-99", PREFIX),
            None
        );
    }
}
