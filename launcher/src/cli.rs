//! CLI 参数定义（clap derive）。

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "gamer-launcher",
    version,
    about = "GameBot 便携安装启动器/升级器（单实例锁 / 原子 state / manifest 验签 / 依赖修复 / server 监管）"
)]
pub struct Cli {
    /// 安装根目录；缺省取本 exe 所在目录
    #[arg(long, global = true, value_name = "PATH")]
    pub install_root: Option<PathBuf>,

    /// 可信 Ed25519 公钥目录（<key_id>.pem）；缺省依次尝试 GAMER_LAUNCHER_KEYS_DIR、<安装根>/keys、<exe 目录>/keys
    #[arg(long, global = true, value_name = "DIR")]
    pub keys_dir: Option<PathBuf>,

    /// 日志级别 trace|debug|info|warn|error|off（默认 info）
    #[arg(long, global = true, value_name = "LEVEL")]
    pub log_level: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 启动并监管 gamer-server 子进程（LCH-008/OPS-003：env 注入 + 句柄等待 + 就绪探测）
    Start,
    /// 查看当前安装状态（只读，不获取单实例锁）
    Status,
    /// 自检：不带 --manifest 检查安装库存；带 --manifest 对任意 release manifest 做完整验签校验
    Doctor {
        /// 对指定 release manifest 文件跑完整校验（先验签、后解析，fail closed）
        #[arg(long, value_name = "FILE")]
        manifest: Option<PathBuf>,
        /// 显式指定分离签名文件（缺省取 <manifest 去 .json>.sig）
        #[arg(long, value_name = "FILE")]
        sig: Option<PathBuf>,
        /// 显式指定公钥 PEM（优先于 --keys-dir 信任库）
        #[arg(long, value_name = "FILE")]
        key: Option<PathBuf>,
        /// 期望的当前安装版本；manifest 版本低于它时按版本降级拒绝
        #[arg(long, value_name = "X.Y.Z")]
        expect_current_version: Option<String>,
        /// 期望发布通道 stable|beta；不匹配时拒绝
        #[arg(long, value_name = "CHANNEL")]
        expect_channel: Option<String>,
        /// 组件深检：逐文件 sha256 对 manifest（不带 --manifest 时用 manifests/ 缓存）
        #[arg(long)]
        deep: bool,
        /// 深检附版本探针（adb version / ffmpeg -version，与 manifest 组件版本比对）
        #[arg(long)]
        probe: bool,
    },
    /// 修复运行依赖（LCH-007：inventory 深检 → seed/cache/remote → 原子换装 → 复验）
    Repair {
        /// 指定 release manifest；缺省用 manifests/ 缓存（优先匹配当前版本，其次 SemVer 最高）
        #[arg(long, value_name = "FILE")]
        manifest: Option<PathBuf>,
        /// 修复后复验附版本探针
        #[arg(long)]
        probe: bool,
    },
    /// 检查并执行升级（LCH-010：§6.6 全链路；committed 前失败自动回滚，
    /// 回滚也失败进 manual_recovery_required）
    Upgrade {
        /// release manifest 路径或 URL（本地路径 = M2 演练主路径；远端走通道源）
        #[arg(long, value_name = "PATH|URL")]
        manifest: String,
    },
}
