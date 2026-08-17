//! 服务端配置

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
    /// 默认匹配阈值
    pub default_threshold: f32,
    /// 视频流软解码（供模板匹配取帧）
    pub decode_frames: bool,
    /// scrcpy 最大分辨率（0 = 原始）
    pub max_size: u32,
    /// 码率 Mbps
    pub bitrate_mbps: u32,
    /// 帧率上限（0 = 默认）
    pub fps: u32,
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
            default_threshold: 0.8,
            decode_frames: true,
            max_size: 0,
            bitrate_mbps: 20,
            // 默认 15fps：防止无 config.toml 时 scrcpy 全速发帧（55fps+），
            // 服务端 ffmpeg 软解 + PNG 编解码单核跑满（CPU 100% 持续拖垮进程）
            fps: 15,
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
        std::fs::create_dir_all(cfg.data_dir.join("templates"))?;
        Ok(cfg)
    }

    pub fn listen_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port)
    }
}
