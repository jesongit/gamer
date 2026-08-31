//! gamer-launcher 入口：解析 CLI → 定位安装根 → 初始化日志 → 分发子命令。

use clap::Parser;

use gamer_launcher::cli::Cli;
use gamer_launcher::commands;
use gamer_launcher::layout::InstallLayout;
use gamer_launcher::logging;

fn main() {
    let mut cli = Cli::parse();
    // CLI 路径（安装根/密钥目录/manifest 路径）先统一 verbatim 化，
    // 再解析安装根——LongPathsEnabled=0 的主机上 >260 字符路径可用。
    commands::normalize_cli_paths(&mut cli);
    let layout = InstallLayout::resolve(cli.install_root.clone());
    logging::init(&layout, cli.log_level.as_deref());
    tracing::debug!(install_root = %layout.root.display(), "gamer-launcher 启动");
    let code = commands::dispatch(&cli, &layout);
    std::process::exit(code);
}
