//! 服务端配置

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
            swipe: "- swipe:\n    fm: [{fx}, {fy}]\n    to: [{tx}, {ty}]\n    time: {time}ms".into(),
        }
    }
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
    /// 管理员密码
    pub password: String,
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
}

fn default_idle_power_secs() -> u64 {
    300
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
            password: "admin123".into(),
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
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let path = std::env::var("GB_CONFIG").unwrap_or_else(|_| "config.toml".into());
        let cfg = if let Ok(content) = std::fs::read_to_string(&path) {
            toml::from_str(&content).unwrap_or_else(|e| {
                tracing::warn!("config parse error ({}), using defaults", e);
                Self::default()
            })
        } else {
            Self::default()
        };
        std::fs::create_dir_all(&cfg.data_dir)?;
        Ok(cfg)
    }

    pub fn listen_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port)
    }
}
