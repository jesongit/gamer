//! LCH-008 + OPS-003：server 子进程监管。
//!
//! - 读 `state/current.json` 定位 `versions/<v>/`（缺失/损坏 = 清晰错误，不 panic）；
//! - 注入稳定路径环境变量（UPDATE_CONTRACT §4）：GAMER_APP_DIR / GAMER_DATA_DIR /
//!   GAMER_ADB_PATH / GAMER_FFMPEG_PATH / GAMER_SCRCPY_SERVER / GB_CONFIG / GB_LOG
//!   （launcher 注入绝对路径）；
//! - 透传 GAMER_ADMIN_PASSWORD（登录链路）并默认注入 GAMER_DEPLOYMENT_MODE=launcher
//!   （用户显式设置不覆盖）；
//! - PATH 收敛到最小集（System32）——验收口径：PATH 清空仍能启动；
//! - OPS-003：持有子进程句柄 `wait()` 等待退出（不按端口/进程名判定），
//!   退出码写日志；
//! - 启动后轮询 `http://127.0.0.1:<port>/health/ready`（端口来源 config.toml，
//!   超时有界）确认就绪；就绪与否只影响报告，不影响监管持续。

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::layout::InstallLayout;
use crate::manifest::pathsafe;

pub const HEALTH_PATH: &str = "/health/ready";
pub const DEFAULT_SERVER_PORT: u16 = 8443;
pub const DEFAULT_ENTRYPOINT: &str = "gamer-server.exe";

/// server 启动计划（全部为绝对路径，来自安装根解析）。
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    pub exe: PathBuf,
    pub cwd: PathBuf,
    pub app_dir: PathBuf,
    pub data_dir: PathBuf,
    pub adb_path: Option<PathBuf>,
    pub ffmpeg_path: Option<PathBuf>,
    pub scrcpy_server: PathBuf,
    pub config_path: PathBuf,
    pub log_path: PathBuf,
}

/// 子进程环境：最小系统集（SystemRoot/SystemDrive/TEMP/TMP + PATH=System32，
/// 系统 DLL 加载与 CRT 初始化所需）+ 注入变量。父进程其余环境一概不带，
/// 仅透传 GAMER_ADMIN_PASSWORD（登录链路）并默认注入 GAMER_DEPLOYMENT_MODE=launcher。
pub fn build_child_env(plan: &LaunchPlan) -> BTreeMap<String, String> {
    build_child_env_from(plan, |key| std::env::var(key).ok())
}

/// 额外注入集（批次 3）：候选维护门 + IPC 寻址/令牌。缺省 None/None 时
/// 行为与批次 2 完全一致（server 走 UnsupportedUpdateController 降级）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchExtras {
    /// 存在 = 注入 GAMER_ACTIVATION_GATE=1：候选维护门（只绑端口 + 健康探针 +
    /// activate 端点，不启 scheduler/业务写）。
    pub activation_gate: bool,
    /// 完整 pipe 名（ipc-v1 §1.1），注入 GAMER_LAUNCHER_PIPE。
    pub ipc_pipe: Option<String>,
    /// 本次启动会话令牌，注入 GAMER_LAUNCHER_IPC_TOKEN。
    pub ipc_token: Option<String>,
    /// 回环管理通道令牌，注入 GAMER_ADMIN_TOKEN（升级 drain 的
    /// X-Admin-Token 快捷通道与子进程同源；None = 不注入，server 按自身策略）。
    pub admin_token: Option<String>,
}

impl LaunchExtras {
    /// 候选启动专用：gate + IPC 寻址一并注入。
    pub fn candidate(pipe_name: String, token: String) -> Self {
        Self {
            activation_gate: true,
            ipc_pipe: Some(pipe_name),
            ipc_token: Some(token),
            admin_token: None,
        }
    }

    /// 常规启动：IPC 寻址注入、无维护门。
    pub fn managed(pipe_name: String, token: String) -> Self {
        Self {
            activation_gate: false,
            ipc_pipe: Some(pipe_name),
            ipc_token: Some(token),
            admin_token: None,
        }
    }

    /// 附加回环管理通道令牌（链式；升级 drain 与子进程同源必需）。
    pub fn with_admin_token(mut self, admin_token: Option<String>) -> Self {
        self.admin_token = admin_token;
        self
    }

    /// 附加 IPC 寻址（pipe 名 + 会话令牌）。
    pub fn pipe_with(mut self, ipc: Option<(String, String)>) -> Self {
        if let Some((pipe, token)) = ipc {
            self.ipc_pipe = Some(pipe);
            self.ipc_token = Some(token);
        }
        self
    }
}

/// 纯函数构造（测试注入父环境取值）：键集合 = 最小系统集 + 契约注入集 +
/// GAMER_ADMIN_PASSWORD（显式设置才透传）+ GAMER_DEPLOYMENT_MODE（缺省 launcher）。
pub fn build_child_env_from(
    plan: &LaunchPlan,
    getenv: impl Fn(&str) -> Option<String>,
) -> BTreeMap<String, String> {
    build_child_env_with_extras(plan, &LaunchExtras::default(), getenv)
}

/// 带额外注入集的构造（批次 3）。
pub fn build_child_env_with_extras(
    plan: &LaunchPlan,
    extras: &LaunchExtras,
    getenv: impl Fn(&str) -> Option<String>,
) -> BTreeMap<String, String> {
    let non_empty = |v: Option<String>| -> Option<String> {
        v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    };
    let mut env = BTreeMap::new();
    env.insert("SystemRoot".to_string(), system_root());
    env.insert(
        "SystemDrive".to_string(),
        getenv("SystemDrive").unwrap_or_else(|| "C:".to_string()),
    );
    if let Some(v) = getenv("TEMP") {
        env.insert("TEMP".to_string(), v);
    }
    if let Some(v) = getenv("TMP") {
        env.insert("TMP".to_string(), v);
    }
    env.insert("PATH".to_string(), minimal_path());

    // 登录链路：用户为 launcher 显式设置的管理口令透传给 server（server 启动时
    // 进程内转 Argon2id PHC，不落盘、不进日志）。空白值视同未设置。
    if let Some(v) = non_empty(getenv("GAMER_ADMIN_PASSWORD")) {
        env.insert("GAMER_ADMIN_PASSWORD".to_string(), v);
    }
    // 部署模式：默认注入 launcher 托管（server Mode::detect 认得该枚举值，
    // 使 system/info 的 deployment.mode=managed 链路成立）；用户显式设置不覆盖。
    let mode = non_empty(getenv("GAMER_DEPLOYMENT_MODE")).unwrap_or_else(|| "launcher".to_string());
    env.insert("GAMER_DEPLOYMENT_MODE".to_string(), mode);

    // 批次 3：候选维护门与 IPC 寻址（server 侧行为由并行轨道实现；存在即生效）。
    if extras.activation_gate {
        env.insert("GAMER_ACTIVATION_GATE".to_string(), "1".to_string());
    }
    if let Some(pipe) = non_empty(extras.ipc_pipe.clone()) {
        env.insert("GAMER_LAUNCHER_PIPE".to_string(), pipe);
    }
    if let Some(token) = non_empty(extras.ipc_token.clone()) {
        env.insert("GAMER_LAUNCHER_IPC_TOKEN".to_string(), token);
    }
    // 回环管理通道：launcher 注入的令牌使本机 drain（/api/shutdown +
    // X-Admin-Token）能通过服务端鉴权；空白值视同未设置。
    if let Some(token) = non_empty(extras.admin_token.clone()) {
        env.insert("GAMER_ADMIN_TOKEN".to_string(), token);
    }

    env.insert(
        "GAMER_APP_DIR".to_string(),
        plan.app_dir.to_string_lossy().into_owned(),
    );
    env.insert(
        "GAMER_DATA_DIR".to_string(),
        plan.data_dir.to_string_lossy().into_owned(),
    );
    if let Some(p) = &plan.adb_path {
        env.insert(
            "GAMER_ADB_PATH".to_string(),
            p.to_string_lossy().into_owned(),
        );
    }
    if let Some(p) = &plan.ffmpeg_path {
        env.insert(
            "GAMER_FFMPEG_PATH".to_string(),
            p.to_string_lossy().into_owned(),
        );
    }
    env.insert(
        "GAMER_SCRCPY_SERVER".to_string(),
        plan.scrcpy_server.to_string_lossy().into_owned(),
    );
    env.insert(
        "GB_CONFIG".to_string(),
        plan.config_path.to_string_lossy().into_owned(),
    );
    env.insert(
        "GB_LOG".to_string(),
        plan.log_path.to_string_lossy().into_owned(),
    );
    env
}

fn system_root() -> String {
    std::env::var("SystemRoot")
        .or_else(|_| std::env::var("WINDIR"))
        .unwrap_or_else(|_| "C:\\Windows".to_string())
}

/// 最小 PATH：仅 System32（不继承父进程 PATH；server 及其依赖只允许加载系统 DLL）。
pub fn minimal_path() -> String {
    Path::new(&system_root())
        .join("System32")
        .to_string_lossy()
        .into_owned()
}

/// 按 plan 启动子进程（继承 launcher 的控制台，便于现场观察）。
pub fn spawn_supervised(plan: &LaunchPlan) -> std::io::Result<Child> {
    spawn_child(plan, &[], Stdio::inherit(), Stdio::inherit())
}

/// 带额外注入集的启动入口（批次 3：候选 gate / IPC 寻址）。
pub fn spawn_supervised_with_extras(
    plan: &LaunchPlan,
    extras: &LaunchExtras,
) -> std::io::Result<Child> {
    spawn_child_with_extras(plan, &[], extras, Stdio::inherit(), Stdio::inherit())
}

/// 启动自更新 trampoline helper。helper 只接收绝对路径和数值环境变量，使用
/// 已构建的 launcher 自身作为 helper 镜像；不启动 server，也不继承业务环境。
pub fn spawn_trampoline(
    launcher_exe: &Path,
    trampoline_env: &BTreeMap<String, String>,
) -> std::io::Result<Child> {
    let mut command = Command::new(launcher_exe);
    command
        .arg("status")
        .env_clear()
        .env("SystemRoot", system_root())
        .env("SystemDrive", "C:")
        .env("PATH", minimal_path())
        .envs(trampoline_env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.spawn()
}

/// 可控 stdio 的启动入口（测试用它捕获 `cmd /c set` 输出验证 env 注入）。
pub fn spawn_child(
    plan: &LaunchPlan,
    args: &[&str],
    stdout: Stdio,
    stderr: Stdio,
) -> std::io::Result<Child> {
    spawn_child_with_extras(plan, args, &LaunchExtras::default(), stdout, stderr)
}

pub fn spawn_child_with_extras(
    plan: &LaunchPlan,
    args: &[&str],
    extras: &LaunchExtras,
    stdout: Stdio,
    stderr: Stdio,
) -> std::io::Result<Child> {
    let env = build_child_env_with_extras(plan, extras, |key| std::env::var(key).ok());
    // cwd 受 DOS 当前目录 ~260 上限（verbatim 超限同样 ERROR_DIRECTORY）：
    // 超长时回退同树短祖先；业务路径全部经 env 绝对注入，server 不依赖 cwd。
    let cwd = crate::winutil::fallback_current_dir(&plan.cwd, 240);
    tracing::info!(exe = %plan.exe.display(), cwd = %cwd.display(), env_keys = env.keys().count(), "启动受管子进程");
    Command::new(&plan.exe)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(&env)
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
}

/// 就绪探测参数（有界：整体 deadline + 单次超时 + 轮询间隔）。
#[derive(Debug, Clone)]
pub struct ReadyProbe {
    pub overall_timeout: Duration,
    pub per_attempt_timeout: Duration,
    pub interval: Duration,
}

impl Default for ReadyProbe {
    fn default() -> Self {
        Self {
            overall_timeout: Duration::from_secs(90),
            per_attempt_timeout: Duration::from_secs(2),
            interval: Duration::from_millis(500),
        }
    }
}

/// 轮询 `http://127.0.0.1:<port>/health/ready` 直到 200 或超时。
/// Err 携带最后一次失败原因（连接拒绝 / 状态非 200 / 协议失败 / 超时）。
pub fn wait_for_ready(port: u16, probe: &ReadyProbe) -> Result<(), String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + probe.overall_timeout;
    let mut last: Option<String> = None;
    loop {
        if Instant::now() >= deadline {
            let detail = last.as_deref().unwrap_or("首次探测未及执行");
            return Err(format!(
                "就绪探测超时（{}s）：{detail}",
                probe.overall_timeout.as_secs()
            ));
        }
        match probe_once(addr, HEALTH_PATH, probe.per_attempt_timeout) {
            Ok(true) => return Ok(()),
            Ok(false) => last = Some("HTTP 非 200（尚未就绪）".to_string()),
            Err(reason) => last = Some(reason),
        }
        std::thread::sleep(probe.interval);
    }
}

/// 单次探测：Ok(true)=就绪(200)；Ok(false)=服务可达但未就绪（5xx 等）；Err=连接/协议失败。
pub fn probe_once(addr: SocketAddr, path: &str, timeout: Duration) -> Result<bool, String> {
    let mut stream =
        TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("连接 {addr} 失败: {e}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| format!("设置读超时失败: {e}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| format!("设置写超时失败: {e}"))?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("发送请求失败: {e}"))?;
    let mut buf = Vec::new();
    match stream.read_to_end(&mut buf) {
        Ok(_) => {}
        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            return Err("响应读取超时".to_string());
        }
        Err(e) => return Err(format!("读取响应失败: {e}")),
    }
    let text = String::from_utf8_lossy(&buf);
    let status = parse_status_code(&text)
        .ok_or_else(|| "无法解析 HTTP 状态行（响应为空或非法）".to_string())?;
    Ok(status == 200)
}

/// 从响应头解析状态码（首行 `HTTP/1.1 200 OK`）。
pub(crate) fn parse_status_code(text: &str) -> Option<u16> {
    let line = text.lines().next()?;
    let mut it = line.split_whitespace();
    it.next()?;
    it.next()?.parse().ok()
}

/// 读取 config.toml 顶层 `port`；缺失/非法时回退默认 8443 并记日志。
pub fn read_configured_port(config_path: &Path) -> u16 {
    let text = match std::fs::read_to_string(config_path) {
        Ok(t) => t,
        Err(_) => {
            tracing::warn!(
                config = %config_path.display(),
                "配置文件不存在，就绪探测端口按默认 {DEFAULT_SERVER_PORT}"
            );
            return DEFAULT_SERVER_PORT;
        }
    };
    match toml::from_str::<toml::Value>(&text) {
        Ok(v) => v
            .get("port")
            .and_then(toml::Value::as_integer)
            .and_then(|p| u16::try_from(p).ok())
            .unwrap_or_else(|| {
                tracing::warn!(
                    "config.toml 缺少合法的顶层 port，就绪探测端口按默认 {DEFAULT_SERVER_PORT}"
                );
                DEFAULT_SERVER_PORT
            }),
        Err(e) => {
            tracing::warn!("config.toml 解析失败（{e}），就绪探测端口按默认 {DEFAULT_SERVER_PORT}");
            DEFAULT_SERVER_PORT
        }
    }
}

/// 定位当前版本的入口程序：优先取缓存 manifest 的 `entrypoint`
/// （路径安全校验 + 文件存在双门禁），否则回退 `gamer-server.exe`。
pub fn resolve_entrypoint(layout: &InstallLayout, version: &str) -> Result<PathBuf, String> {
    let app_dir = layout.versions_dir().join(version);
    if !app_dir.is_dir() {
        return Err(format!(
            "版本目录不存在: {}（尚未安装或 state/current.json 指向被删版本）",
            app_dir.display()
        ));
    }
    for path in cached_manifests(layout) {
        if let Some(ep) = peek_entrypoint(&path) {
            if pathsafe::check_single_path(&ep).is_none() {
                let exe = app_dir.join(&ep);
                if exe.is_file() {
                    return Ok(exe);
                }
                tracing::debug!(manifest = %path.display(), entrypoint = %ep, "manifest entrypoint 文件不存在，尝试下一候选");
            }
        }
    }
    let fallback = app_dir.join(DEFAULT_ENTRYPOINT);
    if fallback.is_file() {
        return Ok(fallback);
    }
    Err(format!(
        "入口程序不存在: {}（版本目录缺 gamer-server.exe，且无可用 manifest 指定 entrypoint）",
        fallback.display()
    ))
}

fn cached_manifests(layout: &InstallLayout) -> Vec<PathBuf> {
    let dir = layout.manifests_dir();
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    out.sort();
    out
}

fn peek_entrypoint(manifest_path: &Path) -> Option<String> {
    let raw = std::fs::read(manifest_path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    value
        .get("platforms")?
        .get("windows-x86_64")?
        .get("app")?
        .get("entrypoint")?
        .as_str()
        .map(str::to_string)
}

/// managed 依赖可执行文件定位：`runtime/<id>/<version>/<exe_name>`，
/// 多版本并存取目录名 SemVer 最大者（非 SemVer 目录名按字典序兜底比较）。
pub fn latest_component_exe(layout: &InstallLayout, id: &str, exe_name: &str) -> Option<PathBuf> {
    let base = layout.runtime_dir().join(id);
    let mut candidates: Vec<(String, PathBuf)> = std::fs::read_dir(&base)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|p| {
            let exe = p.join(exe_name);
            exe.is_file().then(|| {
                (
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default()
                        .to_string(),
                    exe,
                )
            })
        })
        .collect();
    candidates.sort_by(|a, b| {
        match (
            crate::manifest::semver::parse(&a.0),
            crate::manifest::semver::parse(&b.0),
        ) {
            (Some(sa), Some(sb)) => {
                if crate::manifest::semver::is_lt(&sa, &sb) {
                    std::cmp::Ordering::Less
                } else if crate::manifest::semver::is_lt(&sb, &sa) {
                    std::cmp::Ordering::Greater
                } else {
                    a.0.cmp(&b.0)
                }
            }
            _ => a.0.cmp(&b.0),
        }
    });
    candidates.pop().map(|(_, exe)| exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_lines() {
        assert_eq!(
            parse_status_code("HTTP/1.1 200 OK\r\nContent-Type: application/json"),
            Some(200)
        );
        assert_eq!(
            parse_status_code("HTTP/1.1 503 Service Unavailable\n"),
            Some(503)
        );
        assert_eq!(parse_status_code("garbage"), None);
        assert_eq!(parse_status_code(""), None);
    }

    fn plan() -> LaunchPlan {
        LaunchPlan {
            exe: PathBuf::from("gamer-server.exe"),
            cwd: PathBuf::from("."),
            app_dir: PathBuf::from("versions/0.1.0"),
            data_dir: PathBuf::from("data"),
            adb_path: Some(PathBuf::from("runtime/adb/37.0.1/adb.exe")),
            ffmpeg_path: Some(PathBuf::from("runtime/ffmpeg/n/ffmpeg.exe")),
            scrcpy_server: PathBuf::from("versions/0.1.0/assets/scrcpy-server.jar"),
            config_path: PathBuf::from("config/config.toml"),
            log_path: PathBuf::from("logs/gamer-server.log"),
        }
    }

    #[test]
    fn child_env_defaults_to_launcher_mode_without_admin_password() {
        let env = build_child_env_from(&plan(), |_| None);
        assert_eq!(
            env.get("GAMER_DEPLOYMENT_MODE").map(String::as_str),
            Some("launcher"),
            "默认注入 GAMER_DEPLOYMENT_MODE=launcher（server Mode::detect 合法枚举）"
        );
        assert!(
            !env.contains_key("GAMER_ADMIN_PASSWORD"),
            "父进程未设置口令时不得凭空注入"
        );
    }

    #[test]
    fn child_env_passes_through_admin_password_and_keeps_explicit_mode() {
        let getenv = |key: &str| match key {
            "GAMER_ADMIN_PASSWORD" => Some("e2e-admin-pass".to_string()),
            "GAMER_DEPLOYMENT_MODE" => Some("docker".to_string()),
            _ => None,
        };
        let env = build_child_env_from(&plan(), getenv);
        assert_eq!(
            env.get("GAMER_ADMIN_PASSWORD").map(String::as_str),
            Some("e2e-admin-pass"),
            "用户显式设置的登录口令必须透传（登录链路）"
        );
        assert_eq!(
            env.get("GAMER_DEPLOYMENT_MODE").map(String::as_str),
            Some("docker"),
            "用户显式设置的部署模式不得被覆盖"
        );
    }

    #[test]
    fn child_env_treats_blank_values_as_unset() {
        let getenv = |key: &str| match key {
            "GAMER_ADMIN_PASSWORD" => Some("   ".to_string()),
            "GAMER_DEPLOYMENT_MODE" => Some(" \t ".to_string()),
            _ => None,
        };
        let env = build_child_env_from(&plan(), getenv);
        assert!(!env.contains_key("GAMER_ADMIN_PASSWORD"));
        assert_eq!(
            env.get("GAMER_DEPLOYMENT_MODE").map(String::as_str),
            Some("launcher")
        );
    }

    #[test]
    fn extras_inject_gate_and_ipc_vars() {
        let extras = LaunchExtras::candidate(
            "\\\\.\\pipe\\gamebot-launcher-abc".to_string(),
            "tok-123".to_string(),
        );
        let env = build_child_env_with_extras(&plan(), &extras, |_| None);
        assert_eq!(
            env.get("GAMER_ACTIVATION_GATE").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            env.get("GAMER_LAUNCHER_PIPE").map(String::as_str),
            Some("\\\\.\\pipe\\gamebot-launcher-abc")
        );
        assert_eq!(
            env.get("GAMER_LAUNCHER_IPC_TOKEN").map(String::as_str),
            Some("tok-123")
        );
    }

    #[test]
    fn managed_extras_inject_pipe_without_gate() {
        let extras = LaunchExtras::managed("p".to_string(), "t".to_string());
        let env = build_child_env_with_extras(&plan(), &extras, |_| None);
        assert!(
            !env.contains_key("GAMER_ACTIVATION_GATE"),
            "常规启动不得带维护门"
        );
        assert_eq!(
            env.get("GAMER_LAUNCHER_PIPE").map(String::as_str),
            Some("p")
        );
        assert_eq!(
            env.get("GAMER_LAUNCHER_IPC_TOKEN").map(String::as_str),
            Some("t")
        );
    }

    #[test]
    fn default_extras_inject_nothing_new() {
        let env = build_child_env_from(&plan(), |_| None);
        assert!(!env.contains_key("GAMER_ACTIVATION_GATE"));
        assert!(!env.contains_key("GAMER_LAUNCHER_PIPE"));
        assert!(!env.contains_key("GAMER_LAUNCHER_IPC_TOKEN"));
    }

    #[test]
    fn admin_token_extra_injects_loopback_admin_channel() {
        let extras = LaunchExtras::default().with_admin_token(Some("abc123".to_string()));
        let env = build_child_env_with_extras(&plan(), &extras, |_| None);
        assert_eq!(
            env.get("GAMER_ADMIN_TOKEN").map(String::as_str),
            Some("abc123")
        );
        // 空白值视同未设置
        let blank = LaunchExtras::default().with_admin_token(Some("  ".to_string()));
        let env = build_child_env_with_extras(&plan(), &blank, |_| None);
        assert!(!env.contains_key("GAMER_ADMIN_TOKEN"));
    }
}
