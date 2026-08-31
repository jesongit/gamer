//! 结构化日志：tracing stdout + 文件双写（logs/launcher.log，不轮转）。

use std::fs;

use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::layout::InstallLayout;

pub fn init(layout: &InstallLayout, level_arg: Option<&str>) {
    let level = level_arg.and_then(parse_level).unwrap_or(LevelFilter::INFO);
    if let Some(raw) = level_arg {
        if parse_level(raw).is_none() {
            eprintln!("警告: 未识别的 --log-level 值 {raw:?}，按 info 处理");
        }
    }
    let stdout_layer = tracing_subscriber::fmt::layer().with_target(false);
    let base = tracing_subscriber::registry()
        .with(level)
        .with(stdout_layer);
    match fs::create_dir_all(layout.logs_dir()) {
        Ok(()) => {
            // 简单文件 appender：logs/launcher.log，追加不轮转（轮转按保留策略后续批次做）。
            let appender = tracing_appender::rolling::never(layout.logs_dir(), "launcher.log");
            let file_layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(appender);
            base.with(file_layer).init();
        }
        // 日志目录建不出来（只读介质等）时退化为仅 stdout，不影响命令执行。
        Err(_) => base.init(),
    }
}

fn parse_level(raw: &str) -> Option<LevelFilter> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "trace" => Some(LevelFilter::TRACE),
        "debug" => Some(LevelFilter::DEBUG),
        "info" => Some(LevelFilter::INFO),
        "warn" | "warning" => Some(LevelFilter::WARN),
        "error" => Some(LevelFilter::ERROR),
        "off" => Some(LevelFilter::OFF),
        _ => None,
    }
}
