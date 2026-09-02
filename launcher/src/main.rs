//! gamer-launcher 入口：解析 CLI → 定位安装根 → 初始化日志 → 分发子命令。

use clap::Parser;

use gamer_launcher::cli::Cli;
use gamer_launcher::commands;
use gamer_launcher::layout::InstallLayout;
use gamer_launcher::logging;

fn main() {
    // Full 包的主入口就是双击启动：没有命令参数时按 start 处理，
    // 仍保留 repair/doctor/upgrade 等显式 CLI 子命令供高级维护使用。
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let implicit_start = args.len() == 1;
    if implicit_start {
        args.push("start".into());
    }
    let mut cli = Cli::parse_from(args);
    cli.implicit_start = implicit_start;
    // CLI 路径（安装根/密钥目录/manifest 路径）先统一 verbatim 化，
    // 再解析安装根——LongPathsEnabled=0 的主机上 >260 字符路径可用。
    commands::normalize_cli_paths(&mut cli);
    let layout = InstallLayout::resolve(cli.install_root.clone());
    logging::init(&layout, cli.log_level.as_deref());
    tracing::debug!(install_root = %layout.root.display(), "gamer-launcher 启动");
    let code = commands::dispatch(&cli, &layout);
    std::process::exit(code);
}
