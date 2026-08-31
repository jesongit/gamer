//! 依赖探针（SYS-002）：adb / ffmpeg / scrcpy 三组件的可用性与版本探测。
//!
//! 设计约束：
//! - **懒执行**：启动路径绝不调用；首次 API 请求（`/api/system/info`、
//!   `/health/ready`）才触发探测——探针超时（各 ~3s）不可能阻塞启动；
//! - **超时有界**：外部进程探针走 `tokio::process` + `tokio::time::timeout`，
//!   三个探针并发执行，总时长有界；scrcpy 探针在 blocking 池读 jar 并计算
//!   sha256（完整性探针：可读 + 哈希可算 = 资源完好）；
//! - **失败不 panic**：spawn 失败（NotFound）→ `missing`；超时 / 非零退出 /
//!   输出不可解析 / 文件不可读 → `broken`；
//! - **不泄露路径**：结果只含 `status` / `version` / `source` / `binding`
//!   （release/contracts/system-api-v1.md §2.1），配置路径与 sha256 等
//!   本机信息不进任何 API 响应；
//! - **缓存**：结果按「部署模式 + 三个解析后路径」为键缓存 60s，路径变更
//!   或过期自动重探（生产路径配置不变即稳态零开销）。
//!
//! `source`/`binding` 取值（契约 §2.1）：launcher/Docker 模式由部署物锁定
//! 提供（managed）；direct 模式裸命令名走 PATH 查找（system）、显式路径为
//! 用户配置（custom）。`binding` 中 scrcpy 恒为 `application`（与应用版本
//! 强绑定，禁止独立升级），adb/ffmpeg 在 direct 下不经部署内组件目录（external）。

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::config::Config;
use crate::device::scrcpy::SCRCPY_VERSION;

/// 单个外部进程探针的硬超时（契约口径 ~3s）
pub const PROCESS_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// 探测结果缓存 TTL：稳态下 /health/ready 与 /api/system/info 零子进程开销
pub const CACHE_TTL: Duration = Duration::from_secs(60);

/// 部署模式（system-api-v1 §2.1 `deployment.mode` 枚举）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 直跑（本机手动启动）
    Direct,
    /// 容器（镜像整体换版，external 更新策略）
    Docker,
    /// launcher 便携托管（managed 更新策略）
    Launcher,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Direct => "direct",
            Mode::Docker => "docker",
            Mode::Launcher => "launcher",
        }
    }

    /// 更新策略（契约 §2.1 冻结映射：launcher→managed、docker→external、
    /// direct→unsupported）
    pub fn update_strategy(self) -> &'static str {
        match self {
            Mode::Launcher => "managed",
            Mode::Docker => "external",
            Mode::Direct => "unsupported",
        }
    }

    /// 进程环境探测（生产入口）
    pub fn detect() -> Self {
        Self::detect_from(
            |key| std::env::var(key).ok(),
            Path::new("/.dockerenv").is_file(),
        )
    }

    /// 纯函数探测（测试可注入环境取值与容器特征，不动进程级环境变量）。
    /// 优先级：GAMER_DEPLOYMENT_MODE 显式覆盖 > launcher IPC 注入变量 >
    /// GAMER_DOCKER / /.dockerenv 容器特征 > direct。
    pub fn detect_from(getenv: impl Fn(&str) -> Option<String>, dockerenv_exists: bool) -> Self {
        let non_empty = |v: Option<String>| -> Option<String> {
            v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        };
        // 显式覆盖（既有原型行为保留：容器编排可手动指定模式）
        match non_empty(getenv("GAMER_DEPLOYMENT_MODE"))
            .map(|v| v.to_ascii_lowercase())
            .as_deref()
        {
            Some("docker") => return Mode::Docker,
            Some("launcher") => return Mode::Launcher,
            Some("direct") => return Mode::Direct,
            _ => {}
        }
        // launcher 启动 server 时注入 IPC 环境变量（任一存在即托管模式）
        if non_empty(getenv("GAMER_LAUNCHER_PIPE")).is_some()
            || non_empty(getenv("GAMER_LAUNCHER_IPC_TOKEN")).is_some()
        {
            return Mode::Launcher;
        }
        // GAMER_DOCKER 显式声明，或容器特征文件 /.dockerenv 存在
        if non_empty(getenv("GAMER_DOCKER")).is_some() || dockerenv_exists {
            return Mode::Docker;
        }
        Mode::Direct
    }

    /// launcher 托管且 IPC 通道已建立（以 GAMER_LAUNCHER_IPC_TOKEN 注入为
    /// 准据）：契约 §2.1 capability「launcher 模式且 IPC 通道建立 → 全 true」
    pub fn managed_ipc_provisioned(self, getenv: impl Fn(&str) -> Option<String>) -> bool {
        self == Mode::Launcher
            && getenv("GAMER_LAUNCHER_IPC_TOKEN")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false)
    }
}

/// 三类被探测组件
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Component {
    Adb,
    Ffmpeg,
    Scrcpy,
}

/// 单个依赖的探测结论（字段名与 system-api-v1 §2.1 冻结表一一对应）
#[derive(Debug, Clone, serde::Serialize)]
pub struct Dependency {
    /// ready | missing | broken（unknown 仅允许出现在探针完成前；本模块
    /// 返回时探针必已完成）
    pub status: &'static str,
    /// 探测到的真实版本；不可得时 None（序列化为 null）
    pub version: Option<String>,
    /// managed | system | custom
    pub source: &'static str,
    /// runtime | application | external
    pub binding: &'static str,
}

/// 三组件探测快照
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub adb: Dependency,
    pub ffmpeg: Dependency,
    pub scrcpy: Dependency,
}

/// 探测快照（带 60s 缓存；键含部署模式与三个解析后路径，配置变更即失效）。
/// 懒执行——只有 API 请求进入这里才产生子进程/文件读取。
pub async fn snapshot(cfg: &Config) -> Snapshot {
    let mode = Mode::detect();
    let key = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        mode.as_str(),
        cfg.adb_path,
        cfg.ffmpeg_path,
        cfg.scrcpy_server.display(),
    );
    {
        let cache = CACHE.lock().unwrap();
        if let Some(entry) = cache
            .as_ref()
            .filter(|e| e.key == key && e.at.elapsed() < CACHE_TTL)
        {
            return entry.snap.clone();
        }
    }
    let snap = probe_all(cfg, mode).await;
    *CACHE.lock().unwrap() = Some(CacheEntry {
        key,
        at: Instant::now(),
        snap: snap.clone(),
    });
    snap
}

struct CacheEntry {
    key: String,
    at: Instant,
    snap: Snapshot,
}

static CACHE: Mutex<Option<CacheEntry>> = Mutex::new(None);

/// 三探针并发执行并装配 source/binding
async fn probe_all(cfg: &Config, mode: Mode) -> Snapshot {
    let (adb, ffmpeg, scrcpy) = tokio::join!(
        probe_process(&cfg.adb_path, &["version"]),
        probe_process(&cfg.ffmpeg_path, &["-version"]),
        probe_scrcpy_jar(&cfg.scrcpy_server),
    );
    let (adb_source, adb_binding) = classify(Component::Adb, mode, &cfg.adb_path);
    let (ffmpeg_source, ffmpeg_binding) = classify(Component::Ffmpeg, mode, &cfg.ffmpeg_path);
    let (scrcpy_source, scrcpy_binding) =
        classify(Component::Scrcpy, mode, path_str(&cfg.scrcpy_server));
    Snapshot {
        adb: Dependency {
            status: adb.0,
            version: adb.1,
            source: adb_source,
            binding: adb_binding,
        },
        ffmpeg: Dependency {
            status: ffmpeg.0,
            version: ffmpeg.1,
            source: ffmpeg_source,
            binding: ffmpeg_binding,
        },
        scrcpy: Dependency {
            status: scrcpy.0,
            version: scrcpy.1,
            source: scrcpy_source,
            binding: scrcpy_binding,
        },
    }
}

fn path_str(p: &Path) -> &str {
    p.to_str().unwrap_or("")
}

/// source/binding 判定（纯函数，契约 §2.1 枚举；不含任何路径输出）
fn classify(component: Component, mode: Mode, configured: &str) -> (&'static str, &'static str) {
    match mode {
        Mode::Launcher => match component {
            // scrcpy jar 随应用版本目录分发（versions/<semver>/assets），恒 application
            Component::Scrcpy => ("managed", "application"),
            // launcher 管理的 runtime/<id>/<version>/ 独立组件目录
            _ => ("managed", "runtime"),
        },
        Mode::Docker => match component {
            // scrcpy 恒 application（契约冻结），即使随镜像内置
            Component::Scrcpy => ("managed", "application"),
            // Docker 模式恒 managed（随镜像提供并锁定）；镜像内置组件不经
            // 部署内 runtime 目录绑定 → external
            _ => ("managed", "external"),
        },
        Mode::Direct => {
            let binding = match component {
                Component::Scrcpy => "application",
                _ => "external",
            };
            let source = match component {
                // 应用内资产（相对路径形态，如 ./assets/scrcpy-server.jar）随应用
                // 分发 → managed；用户显式保存的绝对路径 → custom
                Component::Scrcpy => {
                    if Path::new(configured).is_absolute() {
                        "custom"
                    } else {
                        "managed"
                    }
                }
                // 裸命令名走 PATH 查找 → system；带目录的显式路径 → custom
                _ => {
                    if is_bare_command(configured) {
                        "system"
                    } else {
                        "custom"
                    }
                }
            };
            (source, binding)
        }
    }
}

fn is_bare_command(path: &str) -> bool {
    Path::new(path)
        .parent()
        .is_some_and(|parent| parent.as_os_str().is_empty())
}

/// 外部进程探针：`<program> <args>` + 硬超时。spawn NotFound → missing；
/// 超时/非零退出/输出异常 → broken；成功 → ready + 解析出的版本号。
async fn probe_process(program: &str, args: &[&str]) -> (&'static str, Option<String>) {
    let mut command = Command::new(program.trim());
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ("missing", None);
        }
        Err(_) => return ("broken", None),
    };
    match tokio::time::timeout(PROCESS_PROBE_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => {
            let version = first_version_token(&String::from_utf8_lossy(&output.stdout));
            match version {
                Some(v) => ("ready", Some(v)),
                None => ("broken", None),
            }
        }
        Ok(Ok(_)) | Ok(Err(_)) | Err(_) => ("broken", None),
    }
}

/// scrcpy 探针：jar 存在 + 可完整读取 + sha256 可算（完整性自检）。版本号
/// 取协议常量（scrcpy 3.3.3 控制协议），绝不输出路径与哈希本身。
async fn probe_scrcpy_jar(path: &Path) -> (&'static str, Option<String>) {
    if !path.is_file() {
        return ("missing", None);
    }
    let path = path.to_path_buf();
    let readable = tokio::task::spawn_blocking(move || {
        let data = std::fs::read(&path)?;
        let digest = Sha256::digest(&data);
        Ok::<String, std::io::Error>(format!("{digest:x}"))
    })
    .await;
    match readable {
        Ok(Ok(_hash)) => ("ready", Some(SCRCPY_VERSION.to_string())),
        Ok(Err(_)) => ("broken", None),
        Err(_) => ("broken", None),
    }
}

/// 从工具输出解析版本号：定位 `version` 词（大小写不敏感）后的第一个合法
/// 版本 token。覆盖 `Android Debug Bridge version 1.0.41` 与
/// `ffmpeg version 7.1.1 ...` 两种输出形态；路径形态 token 一律拒绝。
fn first_version_token(output: &str) -> Option<String> {
    let mut after_version = false;
    for token in output.split_whitespace() {
        if after_version && valid_version_token(token) {
            return Some(token.to_string());
        }
        after_version = token.eq_ignore_ascii_case("version");
    }
    None
}

/// 版本 token 规则：受限长度（≤64）的 ASCII 字母数字与 `. ~ - _ +` 组合，
/// 至少含一个数字。不要求含 `.`——锁定的 BtbN 构建串
/// `N-126335-gb32f8d1c23-20260830` 无点（release/dependencies.lock.toml），
/// 曾因「必须含点」被误判非法 → ffmpeg 探针恒 broken、/health/ready 永 503。
/// 空串 / 超长 / 含路径分隔符（`/` `\`）/ 含其他符号一律拒绝（防把命令行
/// 或路径尾巴当版本号上报）。
fn valid_version_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.contains(['/', '\\'])
        && value.chars().any(|ch| ch.is_ascii_digit())
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ".~_+-".contains(ch))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn getenv<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn mode_detection_follows_injection_precedence() {
        // 缺省直跑
        assert_eq!(Mode::detect_from(getenv(&[]), false), Mode::Direct);
        // launcher IPC 注入变量（任一）
        assert_eq!(
            Mode::detect_from(
                getenv(&[("GAMER_LAUNCHER_PIPE", r"\\.\pipe\gamebot")]),
                false
            ),
            Mode::Launcher
        );
        assert_eq!(
            Mode::detect_from(
                getenv(&[("GAMER_LAUNCHER_IPC_TOKEN", "secret-token")]),
                false
            ),
            Mode::Launcher
        );
        // GAMER_DOCKER 显式声明 / /.dockerenv 容器特征
        assert_eq!(
            Mode::detect_from(getenv(&[("GAMER_DOCKER", "1")]), false),
            Mode::Docker
        );
        assert_eq!(Mode::detect_from(getenv(&[]), true), Mode::Docker);
        // 空白值视同未设置
        assert_eq!(
            Mode::detect_from(getenv(&[("GAMER_LAUNCHER_PIPE", "   ")]), false),
            Mode::Direct
        );
        // launcher 与容器特征同时命中：launcher 注入优先
        assert_eq!(
            Mode::detect_from(
                getenv(&[("GAMER_LAUNCHER_PIPE", "x"), ("GAMER_DOCKER", "1")]),
                true
            ),
            Mode::Launcher
        );
        // GAMER_DEPLOYMENT_MODE 显式覆盖一切
        assert_eq!(
            Mode::detect_from(getenv(&[("GAMER_DEPLOYMENT_MODE", "docker")]), false),
            Mode::Docker
        );
        assert_eq!(
            Mode::detect_from(
                getenv(&[("GAMER_DEPLOYMENT_MODE", "Launcher"), ("GAMER_DOCKER", "1")]),
                true
            ),
            Mode::Launcher
        );
    }

    #[test]
    fn mode_strategy_mapping_is_frozen() {
        assert_eq!(Mode::Launcher.as_str(), "launcher");
        assert_eq!(Mode::Docker.as_str(), "docker");
        assert_eq!(Mode::Direct.as_str(), "direct");
        assert_eq!(Mode::Launcher.update_strategy(), "managed");
        assert_eq!(Mode::Docker.update_strategy(), "external");
        assert_eq!(Mode::Direct.update_strategy(), "unsupported");
    }

    #[test]
    fn managed_ipc_gate_requires_launcher_and_token() {
        assert!(
            Mode::Launcher.managed_ipc_provisioned(getenv(&[("GAMER_LAUNCHER_IPC_TOKEN", "t")]))
        );
        // 非 launcher 模式（即使 token 在）不构成 managed
        assert!(!Mode::Docker.managed_ipc_provisioned(getenv(&[("GAMER_LAUNCHER_IPC_TOKEN", "t")])));
        assert!(!Mode::Launcher.managed_ipc_provisioned(getenv(&[])));
        assert!(
            !Mode::Launcher.managed_ipc_provisioned(getenv(&[("GAMER_LAUNCHER_IPC_TOKEN", "  ")]))
        );
    }

    #[test]
    fn source_and_binding_follow_contract_enums() {
        // launcher：managed + runtime（scrcpy 恒 application）
        assert_eq!(
            classify(Component::Adb, Mode::Launcher, "adb"),
            ("managed", "runtime")
        );
        assert_eq!(
            classify(
                Component::Ffmpeg,
                Mode::Launcher,
                "/runtime/ffmpeg/7.1/bin/ffmpeg"
            ),
            ("managed", "runtime")
        );
        assert_eq!(
            classify(
                Component::Scrcpy,
                Mode::Launcher,
                "/app/versions/0.2.0/assets/scrcpy-server.jar"
            ),
            ("managed", "application")
        );
        // docker：恒 managed；镜像内置 adb/ffmpeg → external；scrcpy 恒 application
        assert_eq!(
            classify(Component::Adb, Mode::Docker, "/usr/bin/adb"),
            ("managed", "external")
        );
        assert_eq!(
            classify(
                Component::Scrcpy,
                Mode::Docker,
                "/opt/server/assets/scrcpy-server.jar"
            ),
            ("managed", "application")
        );
        // direct：裸命令 → system/external；显式路径 → custom/external
        assert_eq!(
            classify(Component::Adb, Mode::Direct, "adb"),
            ("system", "external")
        );
        assert_eq!(
            classify(Component::Ffmpeg, Mode::Direct, "ffmpeg"),
            ("system", "external")
        );
        assert_eq!(
            classify(Component::Adb, Mode::Direct, "D:/private/tools/adb.exe"),
            ("custom", "external")
        );
        // direct：应用内相对资产 → managed/application；绝对路径 → custom/application
        assert_eq!(
            classify(
                Component::Scrcpy,
                Mode::Direct,
                "./assets/scrcpy-server.jar"
            ),
            ("managed", "application")
        );
        assert_eq!(
            classify(
                Component::Scrcpy,
                Mode::Direct,
                "C:/games/scrcpy-server.jar"
            ),
            ("custom", "application")
        );
    }

    #[test]
    fn missing_process_reports_missing_without_delay() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let (status, version) = rt.block_on(probe_process(
            "definitely-not-a-real-tool-gamer-probe",
            &["version"],
        ));
        assert_eq!(status, "missing");
        assert_eq!(version, None);
    }

    #[test]
    fn missing_jar_reports_missing_without_delay() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let (status, version) = rt.block_on(probe_scrcpy_jar(Path::new(
            "definitely-not-a-real-file-gamer-probe.jar",
        )));
        assert_eq!(status, "missing");
        assert_eq!(version, None);
    }

    #[test]
    fn tool_version_parser_only_returns_version_token() {
        assert_eq!(
            first_version_token("Android Debug Bridge version 1.0.41\nVersion 35.0.2"),
            Some("1.0.41".into())
        );
        assert_eq!(
            first_version_token("ffmpeg version 7.1.1 Copyright (c)"),
            Some("7.1.1".into())
        );
        assert_eq!(first_version_token("not a version response"), None);
        assert_eq!(
            first_version_token("version 1.2.3.4-rc1+b7"),
            Some("1.2.3.4-rc1+b7".into())
        );
        assert!(!valid_version_token("C:/private/tool.exe"));
    }

    #[test]
    fn version_token_accepts_dotless_btbn_build_tag() {
        // 锁定的 BtbN 构建串无点（release/dependencies.lock.toml §ffmpeg）：
        // 曾被「必须含点」规则误判非法 → ffmpeg 探针恒 broken、/health/ready 永 503
        let btbn = "N-126335-gb32f8d1c23-20260830";
        assert!(valid_version_token(btbn));
        assert_eq!(
            first_version_token(&format!(
                "ffmpeg version {btbn} Copyright (c) 2000-2026 the FFmpeg developers"
            )),
            Some(btbn.to_string())
        );
        // 合理形态：带点的常规版本 + tilde/加号修饰
        assert!(valid_version_token("37.0.1"));
        assert!(valid_version_token("1.2.3~rc1+build.2"));
        assert!(valid_version_token("2026"));
        // 拒绝：空串 / 超长（>64）/ 含路径分隔符 / 含非法符号
        assert!(!valid_version_token(""));
        assert!(!valid_version_token(&"v1a".repeat(22)));
        assert!(!valid_version_token("a/b.exe"));
        assert!(!valid_version_token(r"C:\tools\ffmpeg.exe"));
        assert!(!valid_version_token("ver:1.2"));
        assert!(!valid_version_token("no digits here!"));
    }

    #[test]
    fn snapshot_fields_cover_all_components() {
        let snap = Snapshot {
            adb: Dependency {
                status: "ready",
                version: None,
                source: "system",
                binding: "external",
            },
            ffmpeg: Dependency {
                status: "missing",
                version: None,
                source: "system",
                binding: "external",
            },
            scrcpy: Dependency {
                status: "ready",
                version: Some("3.3.3".into()),
                source: "managed",
                binding: "application",
            },
        };
        assert_eq!(snap.adb.status, "ready");
        assert_eq!(snap.ffmpeg.status, "missing");
        assert_eq!(snap.scrcpy.version.as_deref(), Some("3.3.3"));
    }
}
