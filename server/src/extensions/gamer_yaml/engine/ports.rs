//! Engine 的窄端口（OPTIMIZATION_PLAN 12.3 阶段 6.3）：截图源 / 设备控制 /
//! 模板匹配三个按职责最小面切分的 trait。
//!
//! Runner 执行路径只依赖这里的 trait，不再直接持有 DeviceManager 具体类型；
//! 生产在 `Runner::new` 装配 adapter（`DeviceGateway` / `ComputePoolMatcher`，
//! 转发 DeviceManager 与 capability 真实实现），单元测试注入内存 fake（记录
//! 控制调用）。trait 按凝聚力拆分：fake 实现任一个
//! 都不需要携带无关依赖。
//!
//! 异步方法统一返回 `BoxFuture` 以保持对象安全（`Arc<dyn Trait>`，不引入
//! async-trait 依赖）；`futures-util` 已在依赖中。

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;

use crate::capabilities::adapters::{
    DeviceAdapter, FrameAdapter, InputAdapter, ResourceAdapter, TouchAdapter, VisionAdapter,
};
use crate::capabilities::{
    ColorSample, DeviceId, DeviceService, FrameHandle, FramePoint, FrameService, FrameSize,
    InputService, KeyAction, KeyCode, KeyInput, MatchManyRequest, MatchOptions, ResourceService,
    SearchRegion, TemplateQuery, TouchPoint, VisionService,
};
use crate::config::Config;
use crate::core::{
    ResolvedResource, ResourceHandle as CoreResourceHandle, ResourceId, ResourceResolver,
};
use crate::device::DeviceManager;
use crate::matcher;

/// 引擎消费的 config.toml 静态快照（纯数据，装配时从 DeviceManager.cfg 提取
/// 一次，测试可直接手工构造）
#[derive(Clone, Debug)]
pub struct EngineSettings {
    /// find / match 轮询及所有脚本点击后的等待间隔（带单位字符串，如 "500ms"）
    pub interval: String,
    /// 模板匹配阈值（0~1]
    pub threshold: f32,
    /// 运行日志等级 debug / info / warn / error
    pub log_level: String,
    /// 判断类步骤（find/match/color）命中后、执行后续分支前的固定间隔毫秒
    /// （0 = 关闭；仅 config.toml 全局生效，脚本 config: 不覆盖）
    pub judge_delay_ms: u64,
}

impl EngineSettings {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            interval: cfg.interval.clone(),
            threshold: cfg.threshold,
            log_level: cfg.log_level.clone(),
            judge_delay_ms: cfg.judge_delay_ms,
        }
    }
}

/// 截图源返回一个小的帧描述；编码字节和解码存储都留在 Frame capability。
#[derive(Clone, Copy, Debug)]
pub struct ScreenFrame {
    pub handle: FrameHandle,
    pub size: FrameSize,
}

impl ScreenFrame {
    pub fn new(handle: FrameHandle, size: FrameSize) -> Self {
        Self { handle, size }
    }
}

pub trait ScreenshotSource: Send + Sync {
    fn screenshot(&self, device_id: &str) -> BoxFuture<'_, anyhow::Result<ScreenFrame>>;
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

/// Engine 的模板查询。模板由逻辑 ResourceId 标识，主机路径只存在于
/// ResourceAdapter 内部。
#[derive(Clone, Debug)]
pub struct TemplateMatchQuery {
    pub resource: ResourceId,
    pub threshold: f32,
    pub region: Option<[u32; 4]>,
    pub color: bool,
}

/// 模板匹配：在给定截图上匹配一个逻辑模板资源。模板解析由 Resource
/// capability 完成，接口不接收 PathBuf。
pub trait TemplateMatcher: Send + Sync {
    fn match_template(
        &self,
        frame: ScreenFrame,
        query: TemplateMatchQuery,
    ) -> BoxFuture<'_, anyhow::Result<Option<matcher::MatchResult>>>;

    /// 批量请求共享同一份 frame。旧 fake 走默认逐项实现，生产 adapter
    /// 覆盖为 capability `vision.match_many`。
    fn match_many(
        &self,
        frame: ScreenFrame,
        queries: Vec<TemplateMatchQuery>,
    ) -> BoxFuture<'_, anyhow::Result<Vec<Option<matcher::MatchResult>>>> {
        Box::pin(async move {
            let mut results = Vec::with_capacity(queries.len());
            for query in queries {
                results.push(self.match_template(frame, query).await?);
            }
            Ok(results)
        })
    }

    /// 取色也消费同一个已捕获帧，避免重新从截图格式解码。
    fn sample_color(
        &self,
        _frame: ScreenFrame,
        _point: FramePoint,
    ) -> BoxFuture<'_, anyhow::Result<ColorSample>> {
        Box::pin(async { Err(anyhow::anyhow!("当前 matcher 未提供取色能力")) })
    }
}

// ---- 生产 adapter（Runner::new 装配，转发真实实现，行为零变化）--------------

/// 生产端口适配：截图源 + 设备控制统一转发 DeviceManager
pub struct DeviceGateway {
    devices: Arc<DeviceManager>,
    device: Arc<DeviceAdapter>,
    input: Arc<InputAdapter>,
    frames: Arc<FrameAdapter>,
}

impl DeviceGateway {
    pub fn new(devices: Arc<DeviceManager>, frames: Arc<FrameAdapter>) -> Self {
        let device = Arc::new(DeviceAdapter::new(devices.clone()));
        let touch = Arc::new(TouchAdapter::new(device.clone()));
        let input = Arc::new(InputAdapter::new(device.clone(), touch));
        Self {
            devices,
            device,
            input,
            frames,
        }
    }

    async fn resolve_device(
        &self,
        device_id: &str,
    ) -> anyhow::Result<crate::capabilities::DeviceHandle> {
        self.device
            .resolve(&DeviceId::new(device_id))
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }
}

impl ScreenshotSource for DeviceGateway {
    fn screenshot(&self, device_id: &str) -> BoxFuture<'_, anyhow::Result<ScreenFrame>> {
        let device_id = device_id.to_string();
        Box::pin(async move {
            let device = self.resolve_device(&device_id).await?;
            let handle = self
                .frames
                .capture(&device)
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            let size = self
                .frames
                .size(handle)
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            Ok(ScreenFrame::new(handle, size))
        })
    }
}

impl DeviceControl for DeviceGateway {
    fn has_session(&self, device_id: &str) -> bool {
        self.devices.session(device_id).is_some()
    }

    fn video_size(&self, device_id: &str) -> Option<(u32, u32)> {
        self.devices.session(device_id).map(|s| s.video_size())
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
            let device = self.resolve_device(&device_id).await?;
            self.input
                .tap(
                    &device,
                    TouchPoint::new(x.max(0.0) as u32, y.max(0.0) as u32, 1.0),
                )
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))
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
            let device = self.resolve_device(&device_id).await?;
            self.input
                .swipe(
                    &device,
                    crate::capabilities::SwipeGesture::new(
                        TouchPoint::new(x1.max(0.0) as u32, y1.max(0.0) as u32, 1.0),
                        TouchPoint::new(x2.max(0.0) as u32, y2.max(0.0) as u32, 1.0),
                        Duration::from_millis(duration_ms),
                    ),
                )
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))
        })
    }

    fn key(&self, device_id: &str, keycode: u32) -> BoxFuture<'_, anyhow::Result<()>> {
        let device_id = device_id.to_string();
        Box::pin(async move {
            let device = self.resolve_device(&device_id).await?;
            self.input
                .key(
                    &device,
                    KeyInput::new(KeyCode::new(keycode), KeyAction::Press),
                )
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))
        })
    }

    fn text(&self, device_id: &str, text: &str) -> BoxFuture<'_, anyhow::Result<()>> {
        let device_id = device_id.to_string();
        let text = text.to_string();
        Box::pin(async move {
            let device = self.resolve_device(&device_id).await?;
            self.input
                .text(&device, crate::capabilities::TextInput::new(text))
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))
        })
    }

    fn start_app(&self, device_id: &str, name: &str) -> BoxFuture<'_, anyhow::Result<()>> {
        let device_id = device_id.to_string();
        let name = name.to_string();
        Box::pin(async move {
            let device = self.resolve_device(&device_id).await?;
            self.device
                .start_app(&device, &crate::capabilities::AppId::new(name))
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))
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

/// 生产模板匹配适配：模板资源与已解码帧经 Vision capability 提交 NCC 计算池
/// （PERF-003；原 Runner::match_on_screen 内联逻辑原样搬移，只移动执行位置）
pub struct ComputePoolMatcher {
    resources: Arc<ResourceAdapter>,
    vision: Arc<VisionAdapter>,
}

/// Adapter from the current file-backed template store to the core logical
/// resource contract.  The host path is resolved and consumed entirely inside
/// this adapter; callers only receive a logical handle plus bytes.
/// 语义锁定测试专用缝（本地编辑区 → override → 包复合解析经 ResourceAdapter
/// 达成；生产 find/match 直接走 ResourceAdapter）。bin 构建视为 dead code 保留。
#[allow(dead_code)]
pub(crate) struct LegacyResourceResolver {
    resources: Arc<ResourceAdapter>,
}

impl LegacyResourceResolver {
    #[allow(dead_code)]
    pub(crate) fn new(resources: Arc<ResourceAdapter>) -> Self {
        Self { resources }
    }
}

impl ResourceResolver for LegacyResourceResolver {
    fn resolve(&self, id: &ResourceId) -> BoxFuture<'_, anyhow::Result<ResolvedResource>> {
        let capability_id = ComputePoolMatcher::to_capability_resource(id);
        let resources = self.resources.clone();
        let id = id.clone();
        Box::pin(async move {
            let handle = ResourceService::resolve(resources.as_ref(), &capability_id)
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            let bytes = resources
                .read(handle)
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            Ok(ResolvedResource::new(CoreResourceHandle::new(id), bytes))
        })
    }
}

impl ComputePoolMatcher {
    pub fn with_frame_adapter(
        scripts: Arc<crate::scripts::ScriptStore>,
        frames: Arc<FrameAdapter>,
    ) -> Self {
        let frame_store = frames.store.clone();
        let resources = Arc::new(ResourceAdapter::new(scripts));
        let vision = Arc::new(VisionAdapter::new(frame_store, resources.clone()));
        Self { resources, vision }
    }

    fn options(query: &TemplateMatchQuery) -> MatchOptions {
        MatchOptions {
            threshold: Some(query.threshold),
            region: query
                .region
                .map(|[x, y, width, height]| SearchRegion::new(x, y, width, height)),
            color_check: query.color,
        }
    }

    fn to_capability_resource(id: &ResourceId) -> crate::capabilities::ResourceId {
        let name = id
            .logical_path()
            .strip_prefix("templates/")
            .unwrap_or_else(|| id.logical_path());
        crate::capabilities::ResourceId::new(id.app_package().to_string(), name)
    }
}

impl TemplateMatcher for ComputePoolMatcher {
    fn match_template(
        &self,
        frame: ScreenFrame,
        query: TemplateMatchQuery,
    ) -> BoxFuture<'_, anyhow::Result<Option<matcher::MatchResult>>> {
        Box::pin(async move {
            let resource = self
                .resources
                .resolve(&Self::to_capability_resource(&query.resource))
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            let template = TemplateQuery::new(resource, Self::options(&query));
            let result = self
                .vision
                .match_template(frame.handle, template)
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            Ok(match result {
                crate::capabilities::MatchOutcome::Found(m) => Some(m.into()),
                crate::capabilities::MatchOutcome::NotFound => None,
            })
        })
    }

    fn match_many(
        &self,
        frame: ScreenFrame,
        queries: Vec<TemplateMatchQuery>,
    ) -> BoxFuture<'_, anyhow::Result<Vec<Option<matcher::MatchResult>>>> {
        Box::pin(async move {
            let mut request = MatchManyRequest::new(frame.handle);
            for query in &queries {
                let resource = self
                    .resources
                    .resolve(&Self::to_capability_resource(&query.resource))
                    .await
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                request = request.with_template(TemplateQuery::new(resource, Self::options(query)));
            }
            let results = self
                .vision
                .match_many(&request)
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            Ok(results
                .into_iter()
                .map(|result| match result.outcome {
                    crate::capabilities::MatchOutcome::Found(m) => Some(m.into()),
                    crate::capabilities::MatchOutcome::NotFound => None,
                })
                .collect())
        })
    }

    fn sample_color(
        &self,
        frame: ScreenFrame,
        point: FramePoint,
    ) -> BoxFuture<'_, anyhow::Result<ColorSample>> {
        Box::pin(async move {
            self.vision
                .sample_color(frame.handle, point)
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))
        })
    }
}

impl From<crate::capabilities::MatchBox> for matcher::MatchResult {
    fn from(value: crate::capabilities::MatchBox) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
            score: value.score,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn legacy_resource_adapter_resolves_logical_id_without_exposing_path() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let store = Arc::new(crate::scripts::ScriptStore::open(&cfg).unwrap());
        std::fs::create_dir_all(store.templates_dir("com.test.game")).unwrap();
        std::fs::write(
            store.templates_dir("com.test.game").join("icon.png"),
            b"template",
        )
        .unwrap();

        let resolver = LegacyResourceResolver::new(Arc::new(ResourceAdapter::new(store)));
        let id = ResourceId::new(
            crate::core::AppPackageId::new("com.test.game").unwrap(),
            "templates/icon.png",
        )
        .unwrap();
        let resource = resolver.resolve(&id).await.unwrap();

        assert_eq!(resource.id(), &id);
        assert_eq!(resource.bytes(), b"template");
    }

    /// Composite 解析缝（三层统一）：模板解析顺序 **本地编辑区（分区）→
    /// user-overrides → active App Package**。生产 find/match 链路经
    /// LegacyResourceResolver → ResourceAdapter → ScriptStore::resolve_template_path
    /// 到达同一实现。
    #[tokio::test]
    async fn resource_resolver_prefers_editable_local_then_override_then_package() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let store = Arc::new(crate::scripts::ScriptStore::open(&cfg).unwrap());
        // 本地编辑区（分区）层
        std::fs::create_dir_all(store.templates_dir("com.test.game")).unwrap();
        std::fs::write(
            store.templates_dir("com.test.game").join("icon.png"),
            b"editable",
        )
        .unwrap();

        // 安装并激活带 templates/icon.png 的 App Package
        let packages = crate::app_packages::AppPackageStore::new(cfg.data_dir.clone());
        let manifest = br#"format_version = 2
id = "official.test"
version = "1.0.0"

[android]
packages = ["com.test.game"]
"#;
        let mut archive = Vec::new();
        {
            use std::io::Write as _;
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut archive));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zw.start_file("manifest.toml", opts).unwrap();
            zw.write_all(manifest).unwrap();
            zw.start_file("templates/icon.png", opts).unwrap();
            zw.write_all(b"package").unwrap();
            zw.finish().unwrap();
        }
        packages.install_and_activate(&archive, None).await.unwrap();

        let resolver = LegacyResourceResolver::new(Arc::new(ResourceAdapter::new(store.clone())));
        let id = ResourceId::new(
            crate::core::AppPackageId::new("com.test.game").unwrap(),
            "templates/icon.png",
        )
        .unwrap();

        // 本地编辑区同名文件胜过包内模板
        let resource = resolver.resolve(&id).await.unwrap();
        assert_eq!(resource.bytes(), b"editable");

        // 删本地副本 → 回落包内模板；user override 再胜过包
        std::fs::remove_file(store.templates_dir("com.test.game").join("icon.png")).unwrap();
        let resource = resolver.resolve(&id).await.unwrap();
        assert_eq!(resource.bytes(), b"package");
        let override_dir = dir.path().join("user-overrides/com.test.game/templates");
        std::fs::create_dir_all(&override_dir).unwrap();
        std::fs::write(override_dir.join("icon.png"), b"override").unwrap();
        let resource = resolver.resolve(&id).await.unwrap();
        assert_eq!(resource.bytes(), b"override");

        // 包内/override 都没有的模板由本地编辑区提供
        std::fs::write(
            store
                .templates_dir("com.test.game")
                .join("only-partition.png"),
            b"partition-only",
        )
        .unwrap();
        let id = ResourceId::new(
            crate::core::AppPackageId::new("com.test.game").unwrap(),
            "templates/only-partition.png",
        )
        .unwrap();
        let resource = resolver.resolve(&id).await.unwrap();
        assert_eq!(resource.bytes(), b"partition-only");
    }
}
