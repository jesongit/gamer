//! Engine 的窄端口（OPTIMIZATION_PLAN 12.3 阶段 6.3）：截图源 / 设备控制 /
//! 模板匹配三个按职责最小面切分的 trait。
//!
//! Runner 执行路径只依赖这里的 trait，不再直接持有 DeviceManager 具体类型；
//! 生产在 `Runner::new` 装配 adapter（`DeviceGateway` / `ComputePoolMatcher`，
//! 逐字节转发 DeviceManager 与 matcher::compute 真实实现），单元测试注入内存
//! fake（预置截图字节 + 记录控制调用）。trait 按凝聚力拆分：fake 实现任一个
//! 都不需要携带无关依赖。
//!
//! 异步方法统一返回 `BoxFuture` 以保持对象安全（`Arc<dyn Trait>`，不引入
//! async-trait 依赖）；`futures-util` 已在依赖中。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;

use crate::config::Config;
use crate::device::DeviceManager;
use crate::matcher;

/// 引擎消费的 config.toml 静态快照（纯数据，装配时从 DeviceManager.cfg 提取
/// 一次，测试可直接手工构造）
#[derive(Clone, Debug)]
pub struct EngineSettings {
    /// find / verify 轮询间隔（带单位字符串，如 "500ms"，语义同 parse_duration）
    pub interval: String,
    /// 模板匹配阈值（0~1]
    pub threshold: f32,
    /// 运行日志等级 debug / info / warn / error
    pub log_level: String,
    /// 数据目录（模板按分区寻址 data/<pkg>/tmpl/）
    pub data_dir: PathBuf,
}

impl EngineSettings {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            interval: cfg.interval.clone(),
            threshold: cfg.threshold,
            log_level: cfg.log_level.clone(),
            data_dir: cfg.data_dir.clone(),
        }
    }
}

/// 截图源：按设备返回最新一帧的 PNG 字节（帧缓存优先/新鲜度语义由生产实现
/// DeviceManager::screenshot 原样保持）
pub trait ScreenshotSource: Send + Sync {
    fn screenshot(&self, device_id: &str) -> BoxFuture<'_, anyhow::Result<Vec<u8>>>;
}

/// 设备控制：Runner 实际用到的最小集合（scrcpy 会话控制 + adb + 会话/设备
/// 元信息）。无会话时 tap/swipe/key/text/start_app 报「设备未连接」——与旧
/// Runner 直接查 session 的报错路径逐字一致
pub trait DeviceControl: Send + Sync {
    /// scrcpy 会话是否存在（「设备未连接」判断点；供日志/事件推送保序的
    /// 前置检查使用）
    fn has_session(&self, device_id: &str) -> bool;
    /// 会话视频尺寸（相对坐标→像素映射、屏幕尺寸优先源）；无会话 → None
    fn video_size(&self, device_id: &str) -> Option<(u32, u32)>;
    /// 设备配置的应用包名（str_app / cls_app）
    fn app_pkg(&self, device_id: &str) -> Option<String>;
    /// adb serial（空串视同未解析；cls_app 拼 force-stop 命令用）
    fn adb_serial(&self, device_id: &str) -> Option<String>;
    /// 点击（像素坐标）
    fn tap(&self, device_id: &str, x: f32, y: f32) -> BoxFuture<'_, anyhow::Result<()>>;
    /// 滑动（像素坐标 + 时长 ms）
    #[allow(clippy::too_many_arguments)]
    fn swipe(
        &self,
        device_id: &str,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        duration_ms: u64,
    ) -> BoxFuture<'_, anyhow::Result<()>>;
    /// 按键注入（keycode 由引擎 key_code() 映射，与旧调用形态一致）
    fn key(&self, device_id: &str, keycode: u32) -> BoxFuture<'_, anyhow::Result<()>>;
    /// 文本注入
    fn text(&self, device_id: &str, text: &str) -> BoxFuture<'_, anyhow::Result<()>>;
    /// 启动应用（"+" 前缀 = 先 force-stop 再启动，scrcpy 定制控制消息）
    fn start_app(&self, device_id: &str, name: &str) -> BoxFuture<'_, anyhow::Result<()>>;
    /// adb shell（cls_app 的 am force-stop）
    fn shell(
        &self,
        serial: &str,
        command: &str,
        timeout: Duration,
    ) -> BoxFuture<'_, anyhow::Result<()>>;
}

/// 模板匹配：在给定截图 PNG 上匹配一个模板文件。模板名 → 文件解析由引擎完成
/// （分区寻址 + 短名消歧），这里只消费解析结果；threshold / region 语义与旧
/// Runner 内联调用完全一致
pub trait TemplateMatcher: Send + Sync {
    fn match_template(
        &self,
        screen_png: Vec<u8>,
        template: &str,
        template_path: PathBuf,
        threshold: f32,
        region: Option<[u32; 4]>,
    ) -> BoxFuture<'_, anyhow::Result<Option<matcher::MatchResult>>>;
}

// ---- 生产 adapter（Runner::new 装配，转发真实实现，行为零变化）--------------

/// 生产端口适配：截图源 + 设备控制统一转发 DeviceManager
pub struct DeviceGateway {
    devices: Arc<DeviceManager>,
}

impl DeviceGateway {
    pub fn new(devices: Arc<DeviceManager>) -> Self {
        Self { devices }
    }
}

impl ScreenshotSource for DeviceGateway {
    fn screenshot(&self, device_id: &str) -> BoxFuture<'_, anyhow::Result<Vec<u8>>> {
        let device_id = device_id.to_string();
        Box::pin(async move { self.devices.screenshot(&device_id).await })
    }
}

impl DeviceControl for DeviceGateway {
    fn has_session(&self, device_id: &str) -> bool {
        self.devices.session(device_id).is_some()
    }

    fn video_size(&self, device_id: &str) -> Option<(u32, u32)> {
        self.devices.session(device_id).map(|s| s.video_size())
    }

    fn app_pkg(&self, device_id: &str) -> Option<String> {
        self.devices.snapshot(device_id).and_then(|(d, _, _)| d.pkg)
    }

    fn adb_serial(&self, device_id: &str) -> Option<String> {
        self.devices
            .snapshot(device_id)
            .map(|(d, _, _)| d.addr)
            .filter(|a| !a.is_empty())
    }

    fn tap(&self, device_id: &str, x: f32, y: f32) -> BoxFuture<'_, anyhow::Result<()>> {
        let device_id = device_id.to_string();
        Box::pin(async move {
            let s = self
                .devices
                .session(&device_id)
                .ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
            s.tap(x, y).await
        })
    }

    fn swipe(
        &self,
        device_id: &str,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        duration_ms: u64,
    ) -> BoxFuture<'_, anyhow::Result<()>> {
        let device_id = device_id.to_string();
        Box::pin(async move {
            let s = self
                .devices
                .session(&device_id)
                .ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
            s.swipe(x1, y1, x2, y2, duration_ms).await
        })
    }

    fn key(&self, device_id: &str, keycode: u32) -> BoxFuture<'_, anyhow::Result<()>> {
        let device_id = device_id.to_string();
        Box::pin(async move {
            let s = self
                .devices
                .session(&device_id)
                .ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
            s.press_key(keycode).await
        })
    }

    fn text(&self, device_id: &str, text: &str) -> BoxFuture<'_, anyhow::Result<()>> {
        let device_id = device_id.to_string();
        let text = text.to_string();
        Box::pin(async move {
            let s = self
                .devices
                .session(&device_id)
                .ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
            s.inject_text(&text).await
        })
    }

    fn start_app(&self, device_id: &str, name: &str) -> BoxFuture<'_, anyhow::Result<()>> {
        let device_id = device_id.to_string();
        let name = name.to_string();
        Box::pin(async move {
            let s = self
                .devices
                .session(&device_id)
                .ok_or_else(|| anyhow::anyhow!("设备未连接"))?;
            s.start_app(&name).await
        })
    }

    fn shell(
        &self,
        serial: &str,
        command: &str,
        timeout: Duration,
    ) -> BoxFuture<'_, anyhow::Result<()>> {
        let serial = serial.to_string();
        let command = command.to_string();
        Box::pin(async move {
            self.devices.adb.shell(&serial, &command, timeout).await?;
            Ok(())
        })
    }
}

/// 生产模板匹配适配：模板读取 + PNG 解码 + NCC 提交专用计算池执行
/// （PERF-003；原 Runner::match_on_screen 内联逻辑原样搬移，只移动执行位置）
pub struct ComputePoolMatcher;

impl TemplateMatcher for ComputePoolMatcher {
    fn match_template(
        &self,
        screen_png: Vec<u8>,
        template: &str,
        template_path: PathBuf,
        threshold: f32,
        region: Option<[u32; 4]>,
    ) -> BoxFuture<'_, anyhow::Result<Option<matcher::MatchResult>>> {
        // 错误标签在进入 async 块前构造（future 只借用 &self，不借用 template）
        let tpl_label = format!("{} (path={})", template, template_path.display());
        Box::pin(async move {
            matcher::compute::run(move || {
                let tpl_bytes = std::fs::read(&template_path)
                    .map_err(|e| anyhow::anyhow!("读取模板 {} 失败: {}", tpl_label, e))?;
                let req = matcher::MatchRequest {
                    screen_png,
                    template_png: tpl_bytes,
                    threshold: Some(threshold),
                    region,
                };
                matcher::match_template(&req).map_err(|e| anyhow::anyhow!("模板匹配失败: {}", e))
            })
            .await
            .and_then(|inner| inner)
        })
    }
}
