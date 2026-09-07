//! Native implementations of the core capability contracts.
//!
//! These adapters are deliberately thin. They own the translation from logical
//! handles to the existing DeviceManager, FrameCache, PackageStore and DB
//! implementations; no backend handle or host path leaves this module.

mod device;
mod frame;
mod input;
mod log;
mod resource;
mod run;
mod runtime;
mod touch;
mod vision;

pub(crate) use device::DeviceAdapter;
pub(crate) use frame::{FrameAdapter, FrameStore};
pub(crate) use input::InputAdapter;
pub(crate) use log::LogAdapter;
pub(crate) use resource::ResourceAdapter;
pub(crate) use run::RunAdapter;
pub(crate) use runtime::RuntimeAdapter;
pub(crate) use touch::TouchAdapter;
pub(crate) use vision::VisionAdapter;

use std::sync::Arc;

use crate::device::DeviceManager;
use crate::resources::PackageStore;
use crate::run_manager::RunManager;
use crate::store::Db;

use super::CapabilityRegistry;

// ---------------------------------------------------------------------------
// 输入来源标注（录制合同 §2.1：manual | keymap | runner | plugin）
// ---------------------------------------------------------------------------

tokio::task_local! {
    /// 调用方在**同任务内**声明的能力输入来源。能力适配器（input/touch）
    /// 落到录制观察时优先采用；未 scope 的调用方（通用 wasm 插件宿主）保持
    /// 适配器缺省 "plugin"，REST/投屏直发路径仍由 api 层标注 "manual"。
    ///
    /// 注意 task-local 不能跨线程/跨 `tokio::spawn`：经独立实例线程进入
    /// 能力层的调用方（如 YAML guest 经 `block_on_yaml` 派生线程）必须在
    /// 线程内的调用点自行 scope（见 gamer_yaml `invoke_json`）。
    static CALLER_INPUT_SOURCE: &'static str;
}

/// 在调用方来源语义下执行一段能力调用（keymap/runner 等）。
pub(crate) async fn with_caller_input_source<R>(
    source: &'static str,
    fut: impl std::future::Future<Output = R>,
) -> R {
    CALLER_INPUT_SOURCE.scope(source, fut).await
}

/// 当前任务声明的调用方来源；无 scope = None（适配器用各自缺省标注）。
pub(crate) fn caller_input_source() -> Option<&'static str> {
    CALLER_INPUT_SOURCE.try_with(|s| *s).ok()
}

/// 能力适配器统一的录制输入来源：调用方 scope 优先，缺省 "plugin"
/// （扩展能力输入的历史缺省语义，向后兼容）。
pub(crate) async fn with_capability_input_source<R>(
    fut: impl std::future::Future<Output = R>,
) -> R {
    let source = caller_input_source().unwrap_or("plugin");
    crate::recording::with_input_source(source, fut).await
}

/// Assemble the native services that have stable process lifetime. Runtime is
/// intentionally created per run because its cancellation token is per run.
pub(crate) fn build_registry(
    devices: Arc<DeviceManager>,
    resources: Arc<PackageStore>,
    db: Db,
    runs: Arc<RunManager>,
) -> CapabilityRegistry {
    let device = Arc::new(DeviceAdapter::new(devices.clone()));
    let touch = Arc::new(TouchAdapter::new(device.clone()));
    let input = Arc::new(InputAdapter::new(device.clone(), touch.clone()));
    let frame_store = Arc::new(FrameStore::new());
    let resource = Arc::new(ResourceAdapter::new(resources));
    let frame = Arc::new(FrameAdapter::new(devices.clone(), frame_store.clone()));
    let vision = Arc::new(VisionAdapter::new(frame_store, resource.clone()));

    CapabilityRegistry::builder()
        .with_device_service(device)
        .with_input_service(input)
        .with_touch_service(touch)
        .with_frame_service(frame)
        .with_vision_service(vision)
        .with_resource_service(resource.clone())
        .with_run_service(Arc::new(RunAdapter::new(runs, resource.clone(), devices)))
        .with_log(LogAdapter::new(db))
        .build()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use futures_util::future::BoxFuture;
    use image::{Rgb, RgbImage};

    use super::*;
    use crate::capabilities::{
        CapabilityError, DeviceHandle, DeviceId, FramePoint, FrameService, LogLevel, LogRecord,
        LogService, MatchManyRequest, MatchOptions, MatchOutcome, ResourceHandle, ResourceId,
        ResourceService, RunRequest, RunService, RuntimeService, TemplateQuery, VisionService,
    };

    /// 录制输入来源标注（合同 §2.1）：调用方 scope 的缺省与覆盖语义。
    /// 无 scope = None（适配器落缺省 "plugin"）；scope 内最内层生效。
    #[tokio::test]
    async fn capability_input_source_defaults_to_none_and_honors_scope() {
        let unscoped = with_capability_input_source(async { caller_input_source() }).await;
        assert_eq!(unscoped, None, "无调用方 scope 必须落适配器缺省 plugin");

        let scoped = with_caller_input_source(
            "keymap",
            with_capability_input_source(async { caller_input_source() }),
        )
        .await;
        assert_eq!(scoped, Some("keymap"));

        // 嵌套 scope 以最内层为准（与 recording::with_input_source 同语义）。
        let nested = with_caller_input_source(
            "keymap",
            with_caller_input_source("runner", async { caller_input_source() }),
        )
        .await;
        assert_eq!(nested, Some("runner"));

        // scope 外不残留。
        assert_eq!(caller_input_source(), None);
    }

    fn template_store() -> (tempfile::TempDir, Arc<PackageStore>) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let store = Arc::new(PackageStore::open(&cfg).unwrap());
        (dir, store)
    }

    fn png(image: &RgbImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    /// 建包 + 写一个资源文件（adapter 测试夹具；plugin id 用中性测试插件）。
    fn seed_template(store: &PackageStore, pkg: &str, plugin: &str, path: &str, bytes: &[u8]) {
        store
            .create_package(crate::resources::PackageInput {
                id: pkg.to_string(),
                ..Default::default()
            })
            .unwrap();
        store
            .write_binary(pkg, plugin, path, bytes, None, false)
            .unwrap();
    }

    #[tokio::test]
    async fn resource_adapter_resolves_and_opens_logical_template() {
        let (_dir, store) = template_store();
        seed_template(
            &store,
            "com.test.game",
            "test.plugin",
            "templates/icon.png",
            b"template",
        );

        let adapter = ResourceAdapter::new(store);
        let id = ResourceId::new("com.test.game", "test.plugin", "templates/icon.png").unwrap();
        let handle = adapter.resolve(&id).await.unwrap();
        assert_eq!(adapter.resolve(&id).await.unwrap(), handle);
        let lease = adapter.open(handle).await.unwrap();
        assert_eq!(lease.handle(), handle);
        assert_eq!(lease.byte_len(), Some(8));
        assert_eq!(adapter.read(handle).unwrap(), b"template");
    }

    #[tokio::test]
    async fn vision_adapter_reuses_one_frame_handle_for_many_templates_and_samples_color() {
        let (_dir, store) = template_store();
        let mut screen = RgbImage::from_pixel(32, 24, Rgb([10, 10, 10]));
        for y in 0..4 {
            for x in 0..5 {
                let pixel = if (x + y) % 2 == 0 {
                    Rgb([20, 80, 140])
                } else {
                    Rgb([220, 40, 30])
                };
                screen.put_pixel(11 + x, 7 + y, pixel);
            }
        }
        let mut template = RgbImage::new(5, 4);
        for y in 0..4 {
            for x in 0..5 {
                template.put_pixel(x, y, *screen.get_pixel(11 + x, 7 + y));
            }
        }
        seed_template(
            &store,
            "com.test.game",
            "test.plugin",
            "templates/icon.png",
            &png(&template),
        );

        let frames = Arc::new(FrameStore::new());
        let frame = frames
            .insert(crate::matcher::DecodedFrame::from_rgb(screen))
            .unwrap();
        let resources = Arc::new(ResourceAdapter::new(store));
        let resource = resources
            .resolve(
                &ResourceId::new("com.test.game", "test.plugin", "templates/icon.png").unwrap(),
            )
            .await
            .unwrap();
        let vision = VisionAdapter::new(frames, resources);
        let request = MatchManyRequest::new(frame)
            .with_template(TemplateQuery::new(
                resource,
                MatchOptions {
                    threshold: Some(0.95),
                    ..Default::default()
                },
            ))
            .with_template(TemplateQuery::new(resource, MatchOptions::default()));

        let results = vision.match_many(&request).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(matches!(results[0].outcome, MatchOutcome::Found(_)));
        assert!(matches!(results[1].outcome, MatchOutcome::Found(_)));
        let color = vision
            .sample_color(frame, FramePoint::new(11, 7))
            .await
            .unwrap();
        assert_eq!((color.red, color.green, color.blue), (20, 80, 140));
    }

    #[test]
    fn frame_store_evicts_old_handles_after_short_retention_window() {
        let frames = FrameStore::new();
        let first = frames
            .insert(crate::matcher::DecodedFrame::from_rgb(RgbImage::new(2, 2)))
            .unwrap();
        for _ in 0..32 {
            frames
                .insert(crate::matcher::DecodedFrame::from_rgb(RgbImage::new(2, 2)))
                .unwrap();
        }
        assert!(matches!(
            frames.get(first),
            Err(CapabilityError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn runtime_adapter_reports_cancellation_without_sleeping() {
        let cancelled = Arc::new(AtomicBool::new(true));
        let runtime = RuntimeAdapter::new(cancelled.clone());
        assert!(runtime.cancelled());
        assert!(matches!(
            runtime.sleep(std::time::Duration::from_secs(1)).await,
            Err(CapabilityError::Cancelled)
        ));
        cancelled.store(false, Ordering::SeqCst);
        runtime.sleep(std::time::Duration::ZERO).await.unwrap();
    }

    #[test]
    fn log_adapter_writes_small_structured_record_to_store() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let db = std::sync::Arc::new(crate::store::Store::open(&cfg).unwrap());
        let adapter = LogAdapter::new(db.clone());
        adapter
            .write(
                LogRecord::new(LogLevel::Info, "capability log")
                    .with_device(DeviceHandle::new(DeviceId::new("d1"))),
            )
            .unwrap();
        let logs = db.list_logs(Some("d1"), None, 10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].msg, "capability log");
    }

    struct SuccessfulExecutor;

    impl crate::run_manager::RunExecutor for SuccessfulExecutor {
        fn prepare<'a>(
            &'a self,
            _context: &'a crate::core::RunContext,
            _request: &'a crate::core::RunRequest,
        ) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn execute<'a>(
            &'a self,
            _context: &'a crate::core::RunContext,
            _request: &'a crate::core::RunRequest,
            _realtime_logs: bool,
            _stop: Arc<AtomicBool>,
        ) -> BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
            Box::pin(async { Ok(vec![("info".into(), "run bridge ok".into())]) })
        }

        fn acquire(
            &self,
            _context: &crate::core::RunContext,
        ) -> anyhow::Result<Box<dyn crate::core::ActivityLease>> {
            Ok(Box::new(crate::core::NoopLease))
        }
    }

    #[tokio::test]
    async fn run_adapter_submits_to_run_manager_and_reports_terminal_state() {
        let (dir, store) = template_store();
        seed_template(
            &store,
            "com.test.game",
            "gamer.yaml",
            "automations/daily.yaml",
            b"steps: []\n",
        );
        let resources = Arc::new(ResourceAdapter::new(store));
        let entry = resources
            .resolve(
                &ResourceId::new("com.test.game", "gamer.yaml", "automations/daily.yaml").unwrap(),
            )
            .await
            .unwrap();
        let manager = Arc::new(crate::run_manager::RunManager::new(Arc::new(
            SuccessfulExecutor,
        )));
        // Device/App 上下文取设备登记配置（plan §16）：设备 d1 必须已登记且
        // 配置 pkg；与 entry 的 Package id（com.test.game）同名仅是夹具巧合。
        let cfg = crate::config::Config {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let db: crate::store::Db = Arc::new(crate::store::Store::open(&cfg).unwrap());
        let devices = Arc::new(crate::device::DeviceManager::new(db, cfg));
        devices
            .upsert_device(&crate::store::Device {
                id: "d1".into(),
                name: "d1".into(),
                kind: "usb".into(),
                addr: String::new(),
                screen_mode: crate::store::ScreenMode::Mirror,
                vd_res: None,
                vd_dpi: None,
                pkg: Some("com.test.game".into()),
                fps: None,
                created_at: "2026-01-01 00:00:00".into(),
            })
            .await
            .unwrap();
        let adapter = RunAdapter::new(manager.clone(), resources, devices);
        let request = RunRequest::new(DeviceHandle::new(DeviceId::new("d1")), entry);
        let handle = adapter.submit(request).await.unwrap();
        assert!(matches!(
            adapter.status(handle).await,
            Ok(crate::capabilities::RunStatus::Queued
                | crate::capabilities::RunStatus::Running
                | crate::capabilities::RunStatus::Succeeded)
        ));
        let run_id = adapter.run_id(handle).unwrap();
        let record = manager.wait_terminal(&run_id).await.unwrap();
        assert_eq!(record.state, crate::run_manager::RunState::Success);
        assert_eq!(
            adapter.status(handle).await.unwrap(),
            crate::capabilities::RunStatus::Succeeded
        );
    }
}
