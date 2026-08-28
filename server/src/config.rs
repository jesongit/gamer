//! 服务端配置（OPS-004：解析失败即退出 + 启动期校验）
//!
//! 加载语义（`Config::load`）：
//! - **文件存在但解析失败** → 带具体错误位置（toml 报 line/column）直接终止，
//!   不再静默回落默认值——回落会让人误以为参数已生效；
//! - **文件不存在** → 按 `GAMER_PROFILE` 区分：
//!   - `dev`（缺省）：放行内置默认值，但向 stderr 打醒目警告（含期望路径）；
//!   - `prod`/`production`：直接报错退出——生产环境必须显式配置。
//! - 路径字段先做规范化（去首尾空白），再执行启动校验；任一违规项即退出。
//!
//! 外部工具可执行性：`scrcpy_server` 指向的 jar **必检**（缺失退出）；adb / ffmpeg
//! 只探测记录 warn 日志不阻断启动（完整 readiness 端点属阶段 4 OBS-001，
//! `probe_external_tools` 即为它预留的探测函数）。

use std::io;
use std::path::{Path, PathBuf};

use anyhow::bail;
use serde::{Deserialize, Serialize};

/// 鉴权配置（config.toml [auth] 段，阶段 2 SEC-002）
///
/// 凭据来源优先级（在 api/auth.rs 解析，非本文件）：环境变量 GAMER_ADMIN_PASSWORD
/// > 环境变量 GAMER_ADMIN_PASSWORD_FILE 指向的密钥文件 > 本段 password_hash
/// （推荐 Argon2id PHC；兼容旧 `sha256$salt$hex`）> 开发模式内置默认值。启动日志
/// 只打印启用的是哪一级来源，绝不输出凭据内容。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// 会话绝对有效期秒数：自登录起算，到期强制重登（滑动续期无法延长）
    pub session_abs_secs: u64,
    /// 会话空闲有效期秒数：每次认证请求刷新；连续不活动超时即失效
    pub session_idle_secs: u64,
    /// 登录限流：同一来源 IP 在窗口内的最大失败次数，达到后全部拒绝直至窗口滑出
    pub login_max_fails: u32,
    /// 登录限流滑动窗口宽度秒数
    pub login_window_secs: u64,
    /// 管理口令哈希（推荐 Argon2id PHC；兼容旧 `sha256$salt$hex`，长度/格式校验在
    /// validate）。留空 = 不启用，回落环境变量/密钥文件或开发默认值。
    pub password_hash: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            session_abs_secs: 12 * 3600,
            session_idle_secs: 2 * 3600,
            login_max_fails: 10,
            login_window_secs: 300,
            password_hash: String::new(),
        }
    }
}

/// 操作记录 YAML 模板：前端 alt 模式把操作追加到编辑区时使用的格式
///
/// 占位符：{name} 模板名 · {x}/{y} 点击相对坐标（color 的采样点同用）·
/// {fx}/{fy}/{tx}/{ty} 滑动起终点 · {time} 滑动实际时长 ms ·
/// {color} 二次裁切区点击处采样的十六进制颜色（color 色值键）
///
/// 生成的操作记录不写等待参数：步骤间不再统一等待，
/// 轮询类间隔由 config interval 控制（config.toml 或脚本 config: 段）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpTemplates {
    /// find：等模板出现并点击
    #[serde(default)]
    pub find: String,
    /// 屏幕点击
    #[serde(default)]
    pub tap: String,
    /// color 颜色判断记录（{color} 为二次裁切区 alt 点击采样的十六进制颜色）
    #[serde(default)]
    pub color: String,
    /// 屏幕滑动
    #[serde(default)]
    pub swipe: String,
}

impl Default for OpTemplates {
    fn default() -> Self {
        Self {
            find: "- find: {name}".into(),
            tap: "- tap: [{x}, {y}]".into(),
            color: "- color: [{x}, {y}]\n  {color}:".into(),
            swipe: "- swipe:\n    fm: [{fx}, {fy}]\n    to: [{tx}, {ty}]\n    time: {time}ms"
                .into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Dev,
    Prod,
}

impl Profile {
    /// 从 GAMER_PROFILE 解析："prod"/"production"（大小写不敏感）→ 生产，
    /// 未设置或其余值一律开发模式（保守缺省，保证旧环境行为不变）
    pub fn from_env() -> Self {
        match std::env::var("GAMER_PROFILE") {
            Ok(v)
                if v.trim().eq_ignore_ascii_case("prod")
                    || v.trim().eq_ignore_ascii_case("production") =>
            {
                Profile::Prod
            }
            _ => Profile::Dev,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Dev => "dev",
            Profile::Prod => "prod",
        }
    }
}

/// 一次成功加载的产物：生效配置 + 来源描述（供启动摘要日志）
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub cfg: Config,
    /// 配置来源（如 `file ./config.toml` 或 `built-in defaults (GAMER_PROFILE=dev)`）
    pub source: String,
    pub profile: Profile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// HTTP 监听端口
    pub port: u16,
    /// 数据目录（SQLite、模板图片、脚本）
    pub data_dir: PathBuf,
    /// adb 可执行文件路径
    pub adb_path: String,
    /// ffmpeg 可执行文件路径（帧缓存软解码用）
    pub ffmpeg_path: String,
    /// scrcpy-server jar 路径
    pub scrcpy_server: PathBuf,
    /// 脚本引擎默认 interval（轮询类间隔，带单位时长串如 "500ms"；
    /// 可被脚本内 config: 段覆盖；裸数字非法——引擎 parse_duration 强制单位）
    #[serde(default = "default_interval")]
    pub interval: String,
    /// 默认模板匹配阈值（可被脚本内 config: 段覆盖）
    #[serde(default = "default_threshold")]
    pub threshold: f32,
    /// 引擎日志等级 debug|info|warn|error（可被脚本内 config: 段覆盖）
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// 视频流软解码（供模板匹配取帧）
    pub decode_frames: bool,
    /// scrcpy 最大分辨率（0 = 原始）
    pub max_size: u32,
    /// 码率 Mbps
    pub bitrate_mbps: u32,
    /// 帧率上限（0 = 默认）
    pub fps: u32,
    /// scrcpy 编码器名（空 = 设备默认；可指定 c2.android.avc.encoder 软编避开 MTK 硬件块效应）
    #[serde(default)]
    pub encoder_name: String,
    /// 编码器输出质量探针（关键帧 + 1/30 P 帧起 ffmpeg 解码检测块效应）。
    /// 纯诊断用：60fps 游戏画面下 ~2.5 进程/秒 + ~15MB/s 管道流量抢 pusher 的
    /// CPU/worker，推高单帧 RTP 发送耗时（饱和 → 积压 → 冻结跳帧），默认关闭
    #[serde(default)]
    pub probe_encoder: bool,
    /// 操作记录 YAML 模板（config.toml [op_templates]，可自定义）
    #[serde(default)]
    pub op_templates: OpTemplates,
    /// 空闲低功耗秒数：周期检查（无 viewer 且无脚本运行持续 N 秒）后——
    /// 虚拟屏拆 scrcpy 会话（编码停止/虚拟屏销毁，adb 链路保留，下次脚本/
    /// 投屏自动重连）；镜像模式关物理屏（会话保留，消费者回来即唤醒）。
    /// 0 = 关闭（空闲会话永不进低功耗）
    #[serde(default = "default_idle_power_secs")]
    pub idle_power_secs: u64,
    /// 服务端文件日志按天轮转的保留天数（含今天；文件形如
    /// gamer-server.log.YYYY-MM-DD，超窗旧文件启动及每日零点各清理一次）。
    /// 仅 GB_LOG 指向文件时生效；0 = 永不清理
    #[serde(default = "default_log_retain_days")]
    pub log_retain_days: u32,
    /// 鉴权与会话治理（[auth] 段整体可缺省取默认值）
    #[serde(default)]
    pub auth: AuthConfig,
}

fn default_idle_power_secs() -> u64 {
    300
}

fn default_log_retain_days() -> u32 {
    14
}

fn default_interval() -> String {
    "500ms".into()
}

fn default_threshold() -> f32 {
    0.85
}

fn default_log_level() -> String {
    "info".into()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 8443,
            data_dir: PathBuf::from("./data"),
            adb_path: "adb".into(),
            ffmpeg_path: "ffmpeg".into(),
            scrcpy_server: PathBuf::from("./assets/scrcpy-server.jar"),
            interval: default_interval(),
            threshold: default_threshold(),
            log_level: default_log_level(),
            decode_frames: true,
            max_size: 0,
            bitrate_mbps: 20,
            // 默认 15fps：防止无 config.toml 时 scrcpy 全速发帧（55fps+），
            // 服务端 ffmpeg 软解 + PNG 编解码单核跑满（CPU 100% 持续拖垮进程）
            fps: 15,
            encoder_name: String::new(),
            probe_encoder: false,
            op_templates: OpTemplates::default(),
            idle_power_secs: default_idle_power_secs(),
            log_retain_days: default_log_retain_days(),
            auth: AuthConfig::default(),
        }
    }
}

/// 解析带单位时长串为毫秒数。与引擎 parse_duration 同口径：
/// 数字部分 + 单位（ms/s/m/min/h/d，m≡min，数字可带小数）；裸数字非法。
/// 返回 None 表示格式非法或数值不可表示。
pub fn duration_str_to_ms(value: &str) -> Option<f64> {
    let v = value.trim();
    let split = v.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let num: f64 = v[..split].parse().ok()?;
    if !num.is_finite() {
        return None;
    }
    let unit = v[split..].trim();
    let mult = match unit {
        "ms" => 1.0,
        "s" => 1_000.0,
        "m" | "min" => 60_000.0,
        "h" => 3_600_000.0,
        "d" => 86_400_000.0,
        _ => return None,
    };
    Some(num * mult)
}

/// 外部工具探测结果（阶段 4 OBS-001 readiness 端点可直接复用）
#[derive(Debug, Clone)]
pub struct ToolProbe {
    pub name: &'static str,
    pub path: String,
    /// Ok(()) 可执行；Err(原因) 探测失败
    pub status: Result<(), String>,
}

impl Config {
    /// 入口：GB_CONFIG 覆盖路径（默认 ./config.toml）+ GAMER_PROFILE 决定 profile
    pub fn load() -> anyhow::Result<LoadedConfig> {
        let path =
            PathBuf::from(std::env::var("GB_CONFIG").unwrap_or_else(|_| "config.toml".into()));
        Self::load_from(&path, Profile::from_env())
    }

    /// 纯函数化加载入口：路径与 profile 显式传入，便于测试而免动全局环境变量
    pub fn load_from(path: &Path, profile: Profile) -> anyhow::Result<LoadedConfig> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // 文件不存在：按 profile 分流
                return match profile {
                    Profile::Prod => bail!(
                        "配置文件不存在：{}（GAMER_PROFILE=prod 要求显式配置，\
                         请从 config.example.toml 复制为该路径并按需修改）",
                        path.display()
                    ),
                    Profile::Dev => {
                        eprintln!(
                            "\n[WARNING] 未找到配置文件 {} —— 使用内置默认配置放行\n\
                             [WARNING] （GAMER_PROFILE={} 开发模式；生产部署请提供该文件，\n\
                             [WARNING]   可从 server/config.example.toml 复制修改）\n",
                            path.display(),
                            profile.as_str()
                        );
                        let mut cfg = Config::default();
                        normalize_paths(&mut cfg);
                        ensure_valid(&cfg)?;
                        Ok(LoadedConfig {
                            cfg,
                            source: format!(
                                "built-in defaults (config missing; GAMER_PROFILE={})",
                                profile.as_str()
                            ),
                            profile,
                        })
                    }
                };
            }
            Err(e) => bail!("读取配置文件 {} 失败：{e}", path.display()),
        };

        // 存在但解析失败：带位置信息终止（toml 错误自带 line/column）
        let mut cfg: Config = toml::from_str(&content).map_err(|e| {
            anyhow::anyhow!(
                "配置文件解析失败（{}）——不再静默使用默认值，进程即将退出。\n{e}",
                path.display()
            )
        })?;
        normalize_paths(&mut cfg);
        ensure_valid(&cfg)?;
        Ok(LoadedConfig {
            cfg,
            source: format!("file {}", path.display()),
            profile,
        })
    }

    pub fn listen_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port)
    }

    /// 非敏感生效值摘要（供启动日志展示来源与关键参数；密码/哈希等敏感项绝不输出）
    pub fn non_sensitive_summary(&self) -> String {
        format!(
            "port={} data_dir={} interval=\"{}\" threshold={:.2} log_level={} \
             decode_frames={} max_size={} bitrate_mbps={} fps={} idle_power_secs={}s \
             log_retain_days={}d session_abs_secs={} session_idle_secs={} \
             login_max_fails={}/{}s password_hash={}",
            self.port,
            self.data_dir.display(),
            self.interval,
            self.threshold,
            self.log_level,
            self.decode_frames,
            self.max_size,
            self.bitrate_mbps,
            self.fps,
            self.idle_power_secs,
            self.log_retain_days,
            self.auth.session_abs_secs,
            self.auth.session_idle_secs,
            self.auth.login_max_fails,
            self.auth.login_window_secs,
            if self.auth.password_hash.is_empty() {
                "unset"
            } else {
                "set"
            },
        )
    }

    /// scrcpy-server jar 存在性必检：缺失直接让启动失败（没有它连不了任何设备）
    pub fn check_scrcpy_jar(&self) -> anyhow::Result<()> {
        let p = &self.scrcpy_server;
        if p.exists() {
            return Ok(());
        }
        let abs = std::path::absolute(p)
            .map(|a| a.display().to_string())
            .unwrap_or_else(|_| p.display().to_string());
        bail!(
            "scrcpy_server 指向的 jar 不存在：{abs}（工作目录 {}）",
            {
                std::env::current_dir()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default()
            }
        )
    }

    /// adb / ffmpeg 可执行性探测（只记录不阻断；readiness 端点属 OBS-001，此处预留函数）
    pub fn probe_external_tools(&self) -> Vec<ToolProbe> {
        vec![
            probe_tool("adb", &self.adb_path, &["version"]),
            probe_tool("ffmpeg", &self.ffmpeg_path, &["-version"]),
        ]
    }

    /// 启动期校验：返回全部违规项描述（空 = 通过）。逐项给出明确错误信息，
    /// 调用方在打完清单后以非零码退出
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();

        if self.port == 0 {
            errs.push(format!(
                "port = {} 非法：HTTP 监听端口须在 1-65535",
                self.port
            ));
        }

        match duration_str_to_ms(&self.interval) {
            None => errs.push(format!(
                "interval = \"{}\" 非法：须为带单位的时长串，支持 ms/s/m/min/h/d \
                 （如 \"500ms\"、\"2s\"、\"30min\"）；裸数字不接受",
                self.interval
            )),
            Some(ms) if ms <= 0.0 => errs.push(format!(
                "interval = \"{}\" 非法：轮询间隔必须大于 0",
                self.interval
            )),
            _ => {}
        }

        if !(0.0 < self.threshold && self.threshold <= 1.0) {
            errs.push(format!(
                "threshold = {} 非法：模板匹配阈值须在 (0, 1]，建议 0.7~0.9",
                self.threshold
            ));
        }

        if !(1..=50).contains(&self.bitrate_mbps) {
            errs.push(format!(
                "bitrate_mbps = {} 超出合理区间 [1, 50]：过高会挤占 WebRTC 发送帧预算，\
                 导致投屏积压与周期性冻结（实测约 12 已接近虚拟屏 60fps 编码实用上限）",
                self.bitrate_mbps
            ));
        }

        if self.fps > 120 {
            errs.push(format!(
                "fps = {} 超过上限 120（0 表示交给设备默认帧率）",
                self.fps
            ));
        }

        // 0 = 原始分辨率放行；否则要求 ≥16、≤4096 且为 8 的倍数（编码器对齐约束）
        if self.max_size != 0
            && (self.max_size < 16 || self.max_size > 4096 || !self.max_size.is_multiple_of(8))
        {
            errs.push(format!(
                "max_size = {} 非法：须为 0（原始分辨率）或 8 的倍数且在 [16, 4096]",
                self.max_size
            ));
        }

        if !matches!(self.log_level.as_str(), "debug" | "info" | "warn" | "error") {
            errs.push(format!(
                "log_level = \"{}\" 非法：只接受 debug / info / warn / error",
                self.log_level
            ));
        }

        // [auth] 段（阶段 2）：TTL/限流下限收紧防止自摆乌龙（空闲 ≥60s、窗口 ≥1s）
        if self.auth.session_abs_secs < 60 || self.auth.session_abs_secs > 30 * 86400 {
            errs.push(format!(
                "auth.session_abs_secs = {} 超出合理区间 [60, 2592000]（会话绝对有效期秒）",
                self.auth.session_abs_secs
            ));
        }
        if self.auth.session_idle_secs < 60 || self.auth.session_idle_secs > 7 * 86400 {
            errs.push(format!(
                "auth.session_idle_secs = {} 超出合理区间 [60, 604800]（会话空闲有效期秒）",
                self.auth.session_idle_secs
            ));
        }
        if !(1..=1000).contains(&self.auth.login_max_fails) {
            errs.push(format!(
                "auth.login_max_fails = {} 超出合理区间 [1, 1000]",
                self.auth.login_max_fails
            ));
        }
        if !(1..=86400).contains(&self.auth.login_window_secs) {
            errs.push(format!(
                "auth.login_window_secs = {} 超出合理区间 [1, 86400]",
                self.auth.login_window_secs
            ));
        }
        if !self.auth.password_hash.is_empty() {
            if let Err(e) = crate::api::auth::parse_password_hash(&self.auth.password_hash) {
                errs.push(format!(
                    "auth.password_hash 格式非法：{e}（期望 Argon2id PHC；兼容旧 \
                     sha256$salt$hex）"
                ));
            }
        }

        errs
    }
}

/// 路径字段规范化：字符串型路径去除首尾空白；PathBuf 型仅在确有差异时替换
fn normalize_paths(cfg: &mut Config) {
    cfg.adb_path = cfg.adb_path.trim().to_string();
    cfg.ffmpeg_path = cfg.ffmpeg_path.trim().to_string();
    cfg.encoder_name = cfg.encoder_name.trim().to_string();
    for p in [&mut cfg.data_dir, &mut cfg.scrcpy_server] {
        if let Some(s) = p.to_str() {
            let t = s.trim();
            if t != s {
                *p = PathBuf::from(t);
            }
        }
    }
}

fn ensure_valid(cfg: &Config) -> anyhow::Result<()> {
    let errs = cfg.validate();
    if errs.is_empty() {
        return Ok(());
    }
    bail!(
        "配置校验未通过（{} 项）：\n{}",
        errs.len(),
        errs.iter()
            .map(|e| format!("  - {e}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn probe_tool(name: &'static str, path: &str, args: &[&str]) -> ToolProbe {
    let status = match std::process::Command::new(path.trim()).args(args).output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!("exited with {}", out.status)),
        Err(e) => Err(e.to_string()),
    };
    ToolProbe {
        name,
        path: path.to_string(),
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小合法 TOML（必填键齐全）
    fn write_minimal_config(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(
            &path,
            r#"
port = 8443
data_dir = "./data"
adb_path = "adb"
ffmpeg_path = "ffmpeg"
scrcpy_server = "./assets/scrcpy-server.jar"
                    decode_frames = true
max_size = 0
bitrate_mbps = 12
fps = 15
"#,
        )
        .unwrap();
        path
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "gamer-cfgtest-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn minimal_file_loads_with_source_and_summary() {
        let dir = temp_dir("ok");
        let path = write_minimal_config(&dir, "config.toml");
        let loaded = Config::load_from(&path, Profile::Prod).unwrap();
        assert_eq!(loaded.profile, Profile::Prod);
        assert!(loaded.source.contains(path.to_str().unwrap()));
        let summary = loaded.cfg.non_sensitive_summary();
        assert!(summary.contains("port=8443"));
        assert!(!summary.contains("admin123"), "摘要绝不能包含密码");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_error_bails_instead_of_defaults() {
        let dir = temp_dir("broken");
        let path = dir.join("config.toml");
        std::fs::write(&path, "port = \"not-a-number\"\ndata_dir = \"./data\"\n").unwrap();
        let err = Config::load_from(&path, Profile::Dev).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("解析失败"), "{msg}");
        assert!(msg.contains("line"), "应包含错误位置行号信息: {msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_dev_defaults_prod_bails() {
        let dir = temp_dir("missing");
        let ghost = dir.join("no-such-config.toml");

        let dev = Config::load_from(&ghost, Profile::Dev).unwrap();
        assert_eq!(dev.cfg.port, 8443, "开发模式放行默认值");

        let prod = Config::load_from(&ghost, Profile::Prod).unwrap_err();
        let msg = prod.to_string();
        assert!(msg.contains("GAMER_PROFILE"), "{msg}");
        assert!(msg.contains(ghost.to_str().unwrap()), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validation_rejects_bad_port_duration_level_bitrate() {
        // 反例统一用结构体更新语法实例化（避免 Default 后逐字段赋值的 clippy 提示）
        let cases: Vec<(Config, &str)> = vec![
            (
                Config {
                    port: 0,
                    ..Default::default()
                },
                "端口",
            ),
            (
                Config {
                    interval: "500".into(), // 裸数字非法
                    ..Default::default()
                },
                "带单位",
            ),
            (
                Config {
                    interval: "abc".into(),
                    ..Default::default()
                },
                "interval",
            ),
            (
                Config {
                    interval: "0s".into(),
                    ..Default::default()
                },
                "大于 0",
            ),
            (
                Config {
                    log_level: "verbose".into(),
                    ..Default::default()
                },
                "log_level",
            ),
            (
                Config {
                    bitrate_mbps: 9999,
                    ..Default::default()
                },
                "bitrate_mbps",
            ),
            (
                Config {
                    threshold: 1.5,
                    ..Default::default()
                },
                "threshold",
            ),
        ];
        for (cfg, marker) in cases {
            let errs = cfg.validate();
            assert!(
                errs.iter().any(|e| e.contains(marker)),
                "expect violation containing {marker:?}, got {errs:?}"
            );
        }
        // 合法默认值应零违规
        assert!(Config::default().validate().is_empty());
    }

    #[test]
    fn validation_rejects_max_size_violations() {
        for bad in [7u32, 17, 4104, 4097] {
            let cfg = Config {
                max_size: bad,
                ..Default::default()
            };
            assert!(
                cfg.validate().iter().any(|e| e.contains("max_size")),
                "{bad}: {:?}",
                cfg.validate()
            );
        }
        let ok = Config {
            max_size: 1920,
            ..Default::default()
        };
        assert!(ok.validate().iter().all(|e| !e.contains("max_size")));
    }

    #[test]
    fn missing_jar_is_fatal() {
        let dir = temp_dir("jar");
        let missing = Config {
            scrcpy_server: dir.join("not-exist.jar"),
            ..Default::default()
        };
        let err = missing.check_scrcpy_jar().unwrap_err();
        assert!(err.to_string().contains("jar 不存在"));
        let real = write_minimal_config(&dir, "fake.jar"); // 任意存在文件即可通过存在性检查
        let present = Config {
            scrcpy_server: real,
            ..Default::default()
        };
        present.check_scrcpy_jar().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duration_parser_matches_engine_units() {
        assert_eq!(duration_str_to_ms("500ms"), Some(500.0));
        assert_eq!(duration_str_to_ms("2s"), Some(2000.0));
        assert_eq!(duration_str_to_ms("1.5m"), Some(90_000.0));
        assert_eq!(duration_str_to_ms("30min"), Some(30.0 * 60_000.0));
        assert_eq!(duration_str_to_ms("1h"), Some(3_600_000.0));
        assert_eq!(duration_str_to_ms("1d"), Some(86_400_000.0));
        assert_eq!(duration_str_to_ms("500"), None); // 裸数字非法
        assert_eq!(duration_str_to_ms("500xyz"), None); // 未知单位
        assert_eq!(duration_str_to_ms(""), None);
        assert_eq!(duration_str_to_ms("-1s"), None);
    }

    #[test]
    fn auth_defaults_valid_and_bad_password_hash_rejected() {
        // 缺省 [auth] 段必须通过启动校验
        let cfg = Config::default();
        assert!(cfg.validate().is_empty());
        assert_eq!(cfg.auth.session_abs_secs, 12 * 3600);
        assert_eq!(cfg.auth.session_idle_secs, 2 * 3600);
        // 非法哈希格式逐类拒绝
        for bad in [
            "plaintext",
            "sha256$onlysalt",
            "sha256$$0123",      // 盐缺失
            "md5$aabbccdd$0123", // 算法不符
            "sha256$zzzz$$deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef", // 盐非 hex
            "sha256$aabbccdd$nothex", // 摘要非 hex
            "sha256$aabbccdd$abcd",   // 摘要长度 ≠32 字节
        ] {
            let cfg = Config {
                auth: AuthConfig {
                    password_hash: bad.into(),
                    ..Default::default()
                },
                ..Default::default()
            };
            let errs = cfg.validate();
            assert!(
                errs.iter().any(|e| e.contains("password_hash")),
                "{bad}: {errs:?}"
            );
        }
        // 合法样例：salt=aabbccdd11223344（8 字节），digest=32 字节 hex
        let good = format!("sha256$aabbccdd11223344${}", "ab".repeat(32));
        let cfg = Config {
            auth: AuthConfig {
                password_hash: good,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            !cfg.validate().iter().any(|e| e.contains("password_hash")),
            "{:?}",
            cfg.validate()
        );
    }
}
