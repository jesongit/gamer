//! Native implementations of the core capability contracts.
//!
//! These adapters are deliberately thin. They own the translation from logical
//! handles to the existing DeviceManager, FrameCache, ScriptStore and DB
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
use crate::scripts::ScriptStore;
use crate::store::Db;

use super::CapabilityRegistry;

/// Assemble the native services that have stable process lifetime. Runtime is
/// intentionally created per run because its cancellation token is per run.
pub(crate) fn build_registry(
    devices: Arc<DeviceManager>,
    scripts: Arc<ScriptStore>,
    db: Db,
) -> CapabilityRegistry {
    let device = Arc::new(DeviceAdapter::new(devices.clone()));
    let touch = Arc::new(TouchAdapter::new(device.clone()));
    let input = Arc::new(InputAdapter::new(device.clone(), touch.clone()));
    let frame_store = Arc::new(FrameStore::new());
    let resource = Arc::new(ResourceAdapter::new(scripts));
    let frame = Arc::new(FrameAdapter::new(devices, frame_store.clone()));
    let vision = Arc::new(VisionAdapter::new(frame_store, resource.clone()));

    CapabilityRegistry::builder()
        .with_device_service(device)
        .with_input_service(input)
        .with_touch_service(touch)
        .with_frame_service(frame)
        .with_vision_service(vision)
        .with_resource_service(resource)
        .with_run_service(Arc::new(RunAdapter))
        .with_log(LogAdapter::new(db))
        .build()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use image::{Rgb, RgbImage};

    use super::*;
    use crate::capabilities::{
        CapabilityError, DeviceHandle, DeviceId, FramePoint, FrameService, LogLevel, LogRecord,
        LogService, MatchManyRequest, MatchOptions, MatchOutcome, ResourceHandle, ResourceService,
        RunRequest, RunService, RuntimeService, TemplateQuery, VisionService,
    };

    fn template_store() -> (tempfile::TempDir, Arc<ScriptStore>) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let store = Arc::new(ScriptStore::open(&cfg).unwrap());
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

    #[tokio::test]
    async fn resource_adapter_resolves_and_opens_logical_template() {
        let (_dir, store) = template_store();
        let template_dir = store.tmpl_dir("com.test.game");
        std::fs::create_dir_all(&template_dir).unwrap();
        std::fs::write(template_dir.join("icon.png"), b"template").unwrap();

        let adapter = ResourceAdapter::new(store);
        let id = super::super::ResourceId::new("com.test.game", "icon.png");
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
        let template_dir = store.tmpl_dir("com.test.game");
        std::fs::create_dir_all(&template_dir).unwrap();
        std::fs::write(template_dir.join("icon.png"), png(&template)).unwrap();

        let frames = Arc::new(FrameStore::new());
        let frame = frames
            .insert(crate::matcher::DecodedFrame::from_rgb(screen))
            .unwrap();
        let resources = Arc::new(ResourceAdapter::new(store));
        let resource = resources
            .resolve(&super::super::ResourceId::new("com.test.game", "icon.png"))
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

    #[tokio::test]
    async fn run_adapter_is_explicitly_blocked_until_request_mapping_is_stable() {
        let adapter = RunAdapter;
        let request = RunRequest::new(
            DeviceHandle::new(DeviceId::new("d1")),
            ResourceHandle::new(),
        );
        assert!(matches!(
            adapter.submit(request).await,
            Err(CapabilityError::Unavailable(_))
        ));
    }
}
