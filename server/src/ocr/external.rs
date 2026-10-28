//! OCR accepts host-resolved evidence IDs, never caller paths or image bytes.
use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use crate::{
    capabilities::{CapabilityRegistry, DeviceHandle, DeviceId, FrameHandle, FrameService},
    mcp::{ErrorKind, InvocationContext, McpError, ObjectKind, TemporaryWork, ToolResult},
    mcp::{ToolDescriptor, ToolEffect, ToolProvider},
    resources::PackageStore,
    store::Db,
    timer_core::TimerRunnerRegistry,
};

#[derive(Clone)]
pub(crate) struct ExternalTools {
    service: Arc<super::OcrService>,
    capabilities: CapabilityRegistry,
    db: Db,
    packages: Arc<PackageStore>,
    runners: Arc<TimerRunnerRegistry>,
    media: Arc<crate::media::MediaService>,
}
impl ExternalTools {
    pub(crate) fn new(
        service: Arc<super::OcrService>,
        capabilities: CapabilityRegistry,
        db: Db,
        packages: Arc<PackageStore>,
        runners: Arc<TimerRunnerRegistry>,
        media: Arc<crate::media::MediaService>,
    ) -> Self {
        Self {
            service,
            capabilities,
            db,
            packages,
            runners,
            media,
        }
    }
    async fn authorize_source(
        &self,
        context: &InvocationContext,
        source: &Source,
    ) -> Result<(), McpError> {
        match source {
            Source::MediaFrame { media_id, .. } => {
                context.require_scope("media.read")?;
                context.authorize_media(media_id)?;
            }
            Source::Target { device_id } => {
                context.require_scope("observe")?;
                context.authorize_device(device_id)?;
            }
            Source::Trace { run_id, .. } => {
                context.require_scope("run.read")?;
                context.authorize_run(run_id)?;
                let record = self
                    .db
                    .stored_run(run_id.clone())
                    .await
                    .map_err(unavailable)?
                    .or_else(|| self.db.trace.retained_record(run_id))
                    .ok_or_else(|| {
                        McpError::new(ErrorKind::NotFound, "run_not_found", "Run does not exist")
                    })?;
                authorize_trace_record(context, &record, &self.runners)?;
            }
            Source::PackagePng {
                package_id,
                plugin_id,
                resource_id,
            } => {
                context.require_scope("resource.read")?;
                context.authorize_package(package_id)?;
                context.authorize_plugin(plugin_id)?;
                crate::resources::sanitize_rel_path(resource_id)
                    .map_err(|_| McpError::invalid("Invalid PNG resource ID"))?;
                if !resource_id.ends_with(".png") {
                    return Err(McpError::invalid("Resource must be a PNG"));
                }
            }
            Source::Sample { sample_id, .. } => {
                context.require_scope("media.read")?;
                context.authorize_media(sample_id)?;
                context.authorize_plugin(crate::extensions::video::VIDEO_EXTENSION_ID)?;
            }
        }
        context.check_active()
    }
    async fn resolve(
        &self,
        context: &InvocationContext,
        source: &Source,
    ) -> Result<(Vec<u8>, Value), McpError> {
        self.authorize_source(context, source).await?;
        let (png, metadata) = match source {
            Source::MediaFrame { media_id, index } => {
                let media = self.media.clone();
                let id = crate::media::MediaId(media_id.clone());
                let index = *index;
                let frame = tokio::task::spawn_blocking(move || {
                    media.extract_frame(
                        &id,
                        &crate::media::FrameRequest {
                            index: Some(index),
                            pts_us: None,
                            max_width: Some(1920),
                        },
                    )
                })
                .await
                .map_err(unavailable)?
                .map_err(unavailable)?;
                let metadata = serde_json::to_value(frame.descriptor).map_err(unavailable)?;
                (
                    frame.png,
                    json!({"kind":"media_frame","media_id":media_id,"index":index,"frame":metadata}),
                )
            }
            Source::Target { device_id } => {
                let frames = self.capabilities.frame().ok_or_else(|| {
                    McpError::new(
                        ErrorKind::Unavailable,
                        "frame_unavailable",
                        "Frame service is unavailable",
                    )
                })?;
                let handle = frames
                    .capture(&DeviceHandle::new(DeviceId::new(device_id)))
                    .await
                    .map_err(unavailable)?;
                let acquired = AcquiredFrame(frames, handle);
                let size = frames.size(handle).await.map_err(unavailable)?;
                let stamp = frames.stamp(handle).await.map_err(unavailable)?;
                let png = frames.image_png(handle, None).await.map_err(unavailable)?;
                drop(acquired);
                (
                    png,
                    json!({"kind":"target","device_id":device_id,"stamp":stamp,"width":size.width,"height":size.height}),
                )
            }
            Source::Trace { run_id, image_id } => {
                let png = self
                    .db
                    .trace
                    .read_image(run_id, image_id, false)
                    .map_err(|_| McpError::invalid("Trace image is unavailable or expired"))?
                    .ok_or_else(|| {
                        McpError::new(
                            ErrorKind::NotFound,
                            "image_not_found",
                            "Trace image does not exist",
                        )
                    })?;
                (
                    png,
                    json!({"kind":"trace","run_id":run_id,"image_id":image_id}),
                )
            }
            Source::PackagePng {
                package_id,
                plugin_id,
                resource_id,
            } => {
                let png = self
                    .packages
                    .read_binary(package_id, plugin_id, resource_id)
                    .map_err(|_| McpError::invalid("PNG resource is unavailable"))?
                    .ok_or_else(|| {
                        McpError::new(
                            ErrorKind::NotFound,
                            "resource_not_found",
                            "PNG resource does not exist",
                        )
                    })?;
                (
                    png,
                    json!({"kind":"package_png","package_id":package_id,"plugin_id":plugin_id,"resource_id":resource_id}),
                )
            }
            Source::Sample {
                sample_id,
                content_sha256,
                frame_id,
            } => {
                let bundle = crate::extensions::video::sample::read_stored_bundle(
                    self.packages.data_root(),
                    sample_id,
                )
                .map_err(|_| McpError::invalid("Sample is unavailable or invalid"))?;
                let manifest: crate::extensions::video::sample::SampleManifest =
                    serde_json::from_value(bundle["manifest"].clone()).map_err(unavailable)?;
                if manifest.id != *sample_id || manifest.content_sha256 != *content_sha256 {
                    return Err(McpError::new(
                        ErrorKind::Conflict,
                        "sample_changed",
                        "Sample content fingerprint does not match",
                    ));
                }
                let frame = manifest
                    .frames
                    .iter()
                    .find(|frame| frame.id == *frame_id)
                    .ok_or_else(|| {
                        McpError::new(
                            ErrorKind::NotFound,
                            "frame_not_found",
                            "Sample frame does not exist",
                        )
                    })?;
                let encoded = bundle["files"]
                    .as_array()
                    .and_then(|files| files.iter().find(|file| file["path"] == frame.path))
                    .and_then(|file| file["base64"].as_str())
                    .ok_or_else(|| McpError::invalid("Sample frame asset is missing"))?;
                let png = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|_| McpError::invalid("Sample frame is invalid"))?;
                if format!("{:x}", Sha256::digest(&png)) != frame.sha256 {
                    return Err(McpError::invalid("Sample frame hash does not match"));
                }
                (
                    png,
                    json!({"kind":"sample","sample_id":sample_id,"content_sha256":content_sha256,"frame_id":frame_id,"sha256":frame.sha256}),
                )
            }
        };
        if png.is_empty() || png.len() > super::MAX_PNG || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(McpError::invalid(
                "Resolved evidence must be a PNG of at most 8 MiB",
            ));
        }
        context.check_active()?;
        Ok((png, metadata))
    }
}

fn authorize_trace_record(
    context: &InvocationContext,
    record: &Value,
    runners: &TimerRunnerRegistry,
) -> Result<(), McpError> {
    let run = text(record, "run_id")?;
    context.authorize_run(run)?;
    context.authorize_device(text(record, "device_id")?)?;
    if let Some(package) = record["content_package"].as_str() {
        context.authorize_package(package)?;
    }
    let runner = text(record, "runner_id")?;
    context.authorize_runner(runner)?;
    if let Some(registered) = runners.get_runner(runner) {
        return context.authorize_plugin(&registered.owner_extension_id);
    }
    let frozen_owner = context
        .object_scope(ObjectKind::Run, run)?
        .filter(|scope| scope.runner_id.as_deref() == Some(runner))
        .and_then(|scope| scope.plugin_id);
    if let Some(owner) = frozen_owner {
        context.authorize_plugin(&owner)
    } else if context.principal.allowed.plugins.contains("*") {
        Ok(())
    } else {
        Err(McpError::forbidden(
            "The historical Runner owner is unavailable; OCR requires a matching frozen owner or an explicit all-plugins range",
        ))
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Source {
    MediaFrame {
        media_id: String,
        index: u32,
    },
    Target {
        device_id: String,
    },
    Trace {
        run_id: String,
        image_id: String,
    },
    PackagePng {
        package_id: String,
        plugin_id: String,
        resource_id: String,
    },
    Sample {
        sample_id: String,
        content_sha256: String,
        frame_id: String,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    source: Source,
    #[serde(default = "super::default_language")]
    language: String,
    #[serde(default = "super::default_mode")]
    mode: String,
    #[serde(default)]
    whitelist: Option<String>,
    #[serde(default = "super::default_scale")]
    scale: f64,
    #[serde(default = "super::default_timeout")]
    timeout_ms: u64,
}
struct AcquiredFrame<'a>(&'a dyn FrameService, FrameHandle);
impl Drop for AcquiredFrame<'_> {
    fn drop(&mut self) {
        self.0.release(self.1);
    }
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, McpError> {
    value[key]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| McpError::invalid("Run ownership metadata is missing"))
}
fn unavailable(_: impl std::fmt::Display) -> McpError {
    McpError::new(
        ErrorKind::Unavailable,
        "ocr_unavailable",
        "OCR or its authorized image source is unavailable",
    )
}

struct OcrWork {
    cancel: watch::Sender<bool>,
    done: watch::Sender<bool>,
}
struct WorkCompletion(Arc<OcrWork>);
impl Drop for WorkCompletion {
    fn drop(&mut self) {
        // Notify token revocation even when the task unwinds before returning.
        // A waiter itself retains the sender, so sender-drop alone is insufficient.
        self.0.done.send_replace(true);
    }
}
#[async_trait]
impl TemporaryWork for OcrWork {
    async fn cancel_and_wait(&self) {
        let _ = self.cancel.send(true);
        let mut done = self.done.subscribe();
        while !*done.borrow_and_update() {
            if done.changed().await.is_err() {
                break;
            }
        }
    }
}

#[async_trait]
impl ToolProvider for ExternalTools {
    fn tools(&self) -> Vec<ToolDescriptor> {
        vec![crate::mcp_core::tool("gamer.core.ocr_status", "Read OCR availability and languages without local paths.", "ocr.read", ToolEffect::Read, json!({}), &[], Default::default()),
            ToolDescriptor {name:"gamer.core.ocr_recognize".into(), description:"Recognize an authorized target, trace image, Package PNG, sample frame or original media frame by index. No paths or arbitrary image bytes.".into(), input_schema:serde_json::from_str(include_str!("schemas/recognize.json")).expect("OCR schema"), output_schema:None, required_scopes:["ocr.recognize".into()].into(),effect:ToolEffect::Read,context:Default::default()}]
    }
    async fn authorize(
        &self,
        context: &InvocationContext,
        action: &str,
        arguments: &Value,
    ) -> Result<(), McpError> {
        context.check_active()?;
        match action {
            "gamer.core.ocr_status" => {
                context.require_scope("ocr.read")?;
                if arguments != &json!({}) {
                    return Err(McpError::invalid("Status accepts no arguments"));
                }
            }
            "gamer.core.ocr_recognize" => {
                context.require_scope("ocr.recognize")?;
                let request: Request = serde_json::from_value(arguments.clone())
                    .map_err(|_| McpError::invalid("Invalid controlled OCR arguments"))?;
                self.authorize_source(context, &request.source).await?;
            }
            _ => return Err(McpError::forbidden("OCR action is not exposed externally")),
        }
        Ok(())
    }
    async fn call(
        &self,
        context: InvocationContext,
        action: &str,
        arguments: Value,
    ) -> Result<ToolResult, McpError> {
        self.authorize(&context, action, &arguments).await?;
        if action == "gamer.core.ocr_status" {
            let status = self
                .service
                .dispatch("status", json!({}))
                .await
                .map_err(unavailable)?;
            context.check_active()?;
            return Ok(ToolResult::json(
                json!({"available":status["available"],"languages":status["languages"],"pending":status["pending"],"busy":!status["active_pid"].is_null(),"requires_user_configuration":status["available"]!=true}),
            ));
        }
        let request: Request = serde_json::from_value(arguments)
            .map_err(|_| McpError::invalid("Invalid controlled OCR arguments"))?;
        let (cancel, cancelled) = watch::channel(false);
        let (done, _) = watch::channel(false);
        let work = Arc::new(OcrWork { cancel, done });
        let lease = context
            .register_temporary_work(&format!("ocr:{}", uuid::Uuid::new_v4()), work.clone())?;
        let service = self.service.clone();
        let sources = self.clone();
        let result=tokio::spawn(async move {
            let _completion=WorkCompletion(work.clone());
            let _lease=lease;
            // Evidence preparation may spawn ffmpeg too. Keep it inside the
            // owned task so revocation or a dropped HTTP request cannot release
            // the work lease while the media worker is still running.
            let (png, source) = sources.resolve(&context, &request.source).await?;
            context.check_active()?;
            if *cancelled.borrow() {
                return Err(unavailable("OCR preparation cancelled"));
            }
            let values = json!({"png_base64":base64::engine::general_purpose::STANDARD.encode(png),"language":request.language,"mode":request.mode,"whitelist":request.whitelist,"scale":request.scale,"timeout_ms":request.timeout_ms});
            let result={
                let recognition=service.recognize_cancellable(values,Some(cancelled));tokio::pin!(recognition);
                loop {tokio::select!{
                    biased;
                    value=&mut recognition=>break value.map_err(unavailable),
                    _=tokio::time::sleep(Duration::from_millis(100))=>if context.check_active().is_err(){work.cancel.send_replace(true);}
                }}
            };
            // The existing worker joins its own OS child cleanup before the
            // external work lease can be released.
            context.check_active()?;
            result.map(|result|ToolResult::json(json!({"source":source,"result":result})))
        }).await.map_err(unavailable)??;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct PendingCapture {
        entered: tokio::sync::Notify,
        proceed: tokio::sync::Notify,
        finished: std::sync::atomic::AtomicBool,
    }
    #[async_trait]
    impl FrameService for PendingCapture {
        async fn device_size(
            &self,
            _: &DeviceHandle,
        ) -> crate::capabilities::CapabilityResult<crate::capabilities::FrameSize> {
            panic!("No device I/O in the source-lifetime fixture")
        }
        async fn latest(
            &self,
            _: &DeviceHandle,
        ) -> crate::capabilities::CapabilityResult<Option<FrameHandle>> {
            panic!("No latest-frame fallback")
        }
        async fn size(
            &self,
            _: FrameHandle,
        ) -> crate::capabilities::CapabilityResult<crate::capabilities::FrameSize> {
            panic!("Capture fails before allocating pixels")
        }
        async fn capture(
            &self,
            _: &DeviceHandle,
        ) -> crate::capabilities::CapabilityResult<FrameHandle> {
            self.entered.notify_one();
            self.proceed.notified().await;
            self.finished
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Err(crate::capabilities::CapabilityError::Unavailable(
                "synthetic source completed",
            ))
        }
    }
    #[tokio::test]
    async fn mcp_ocr_source_preparation_survives_http_drop_and_revocation_waits_for_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            data_dir: root.path().into(),
            ..Default::default()
        };
        let frames = Arc::new(PendingCapture {
            entered: tokio::sync::Notify::new(),
            proceed: tokio::sync::Notify::new(),
            finished: std::sync::atomic::AtomicBool::new(false),
        });
        let tools = ExternalTools::new(
            Arc::new(super::super::OcrService::new(root.path()).unwrap()),
            CapabilityRegistry::builder()
                .with_frame_service(frames.clone())
                .build(),
            Arc::new(crate::store::Store::open(&cfg).unwrap()),
            Arc::new(PackageStore::open(&cfg).unwrap()),
            Arc::new(TimerRunnerRegistry::new()),
            crate::media::service(&cfg),
        );
        let mcp = Arc::new(
            crate::mcp::McpService::open(
                root.path(),
                crate::mcp::ServerInfo {
                    name: "test".into(),
                    version: "1".into(),
                },
            )
            .unwrap(),
        );
        mcp.register_provider("gamer.ocr", Arc::new(tools)).unwrap();
        let token = mcp
            .credentials()
            .create(crate::mcp::CreateTokenRequest {
                label: "source cleanup".into(),
                scopes: ["observe".into(), "ocr.recognize".into()].into(),
                allowed: crate::mcp::AllowedObjects {
                    devices: ["synthetic".into()].into(),
                    ..Default::default()
                },
                expires_in_seconds: 60,
            })
            .unwrap();
        let principal = mcp.credentials().authenticate(&token.token).unwrap();
        let request = tokio::spawn({
            let mcp = mcp.clone();
            async move {
                mcp.call_tool(
                    principal,
                    "gamer.core.ocr_recognize",
                    json!({"source":{"kind":"target","device_id":"synthetic"}}),
                )
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), frames.entered.notified())
            .await
            .unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        mcp.credentials().revoke(&token.metadata.token_id).unwrap();
        let draining = tokio::spawn({
            let mcp = mcp.clone();
            async move {
                mcp.drain_token_work(&token.metadata.token_id).await;
            }
        });
        tokio::task::yield_now().await;
        assert!(!draining.is_finished());
        assert!(!frames.finished.load(std::sync::atomic::Ordering::SeqCst));
        frames.proceed.notify_one();
        tokio::time::timeout(Duration::from_secs(2), draining)
            .await
            .unwrap()
            .unwrap();
        assert!(frames.finished.load(std::sync::atomic::Ordering::SeqCst));
    }
    #[tokio::test]
    async fn mcp_ocr_media_frame_resolves_authorized_real_pixels_without_device_or_ocr_execution() {
        let root = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            data_dir: root.path().into(),
            ..Default::default()
        };
        let media = crate::media::service(&cfg);
        let clip = root.path().join("clip.mp4");
        let encoded = crate::background_process::command("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=white:s=64x64:r=2:d=1",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&clip)
            .output()
            .unwrap();
        assert!(
            encoded.status.success(),
            "{}",
            String::from_utf8_lossy(&encoded.stderr)
        );
        let item = media
            .import_bytes("recording.mp4", &std::fs::read(clip).unwrap())
            .unwrap();
        let tools = ExternalTools::new(
            Arc::new(super::super::OcrService::new(root.path()).unwrap()),
            CapabilityRegistry::default(),
            Arc::new(crate::store::Store::open(&cfg).unwrap()),
            Arc::new(PackageStore::open(&cfg).unwrap()),
            Arc::new(TimerRunnerRegistry::new()),
            media,
        );
        let credentials =
            Arc::new(crate::mcp::CredentialStore::open(&root.path().join("mcp")).unwrap());
        let mut contexts = Vec::new();
        for (id, scope) in [
            (item.id.0.as_str(), true),
            ("ungranted", true),
            (item.id.0.as_str(), false),
        ] {
            let token = credentials
                .create(crate::mcp::CreateTokenRequest {
                    label: "media OCR fixture".into(),
                    scopes: if scope {
                        ["media.read".into(), "ocr.recognize".into()].into()
                    } else {
                        ["ocr.recognize".into()].into()
                    },
                    allowed: crate::mcp::AllowedObjects {
                        media: [id.into()].into(),
                        ..Default::default()
                    },
                    expires_in_seconds: 60,
                })
                .unwrap();
            contexts.push(InvocationContext::new(
                credentials.authenticate(&token.token).unwrap(),
                "media OCR".into(),
                None,
                credentials.clone(),
                Arc::new(crate::mcp::ReceiptStore::open(&root.path().join("mcp")).unwrap()),
            ));
        }
        let source = Source::MediaFrame {
            media_id: item.id.0.clone(),
            index: 1,
        };
        let (png, metadata) = tools.resolve(&contexts[0], &source).await.unwrap();
        let image = image::load_from_memory(&png).unwrap();
        assert_eq!((image.width(), image.height()), (64, 64));
        assert_eq!(metadata["index"], 1);
        assert_eq!(metadata["kind"], "media_frame");
        for denied in &contexts[1..] {
            assert!(tools.resolve(denied, &source).await.is_err());
        }
        assert!(tools
            .resolve(
                &contexts[0],
                &Source::MediaFrame {
                    media_id: item.id.0,
                    index: 100
                }
            )
            .await
            .is_err());
        let schema: Value = serde_json::from_str(include_str!("schemas/recognize.json")).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        assert!(
            validator.is_valid(&json!({"source":{"kind":"media_frame","media_id":"id","index":1}}))
        );
        for source in [
            json!({"kind":"media_frame","media_id":"id","index":1,"path":"secret"}),
            json!({"kind":"media_frame","media_id":"id","index":-1}),
            json!({"kind":"media_frame","media_id":"id","index":4294967296u64}),
        ] {
            assert!(!validator.is_valid(&json!({"source":source})));
        }
    }
    fn trace_context(root: &std::path::Path, plugin: &str) -> InvocationContext {
        let credentials = Arc::new(crate::mcp::CredentialStore::open(root).unwrap());
        let token = credentials
            .create(crate::mcp::CreateTokenRequest {
                label: "trace authorization".into(),
                scopes: ["run.read".into()].into(),
                allowed: crate::mcp::AllowedObjects {
                    devices: ["phone".into()].into(),
                    packages: ["default".into()].into(),
                    plugins: [plugin.into()].into(),
                    runners: ["fixture.runner".into(), "different.runner".into()].into(),
                    runs: ["fixture-run".into()].into(),
                    ..Default::default()
                },
                expires_in_seconds: 60,
            })
            .unwrap();
        InvocationContext::new(
            credentials.authenticate(&token.token).unwrap(),
            "trace-test".into(),
            None,
            credentials,
            Arc::new(crate::mcp::ReceiptStore::open(root).unwrap()),
        )
    }

    #[test]
    fn trace_ocr_checks_current_runner_owner_and_requires_a_matching_frozen_owner_when_missing() {
        let root = tempfile::tempdir().unwrap();
        let denied = trace_context(&root.path().join("denied"), "gamer-ocr");
        let allowed = trace_context(&root.path().join("allowed"), "fixture.plugin");
        let all = trace_context(&root.path().join("all"), "*");
        let runners = TimerRunnerRegistry::new();
        runners
            .register_runner(
                "fixture.runner",
                "fixture.plugin",
                Arc::new(TimerRunnerRegistry::new()),
            )
            .unwrap();
        let record = json!({"run_id":"fixture-run","device_id":"phone","content_package":"default","runner_id":"fixture.runner"});
        assert!(authorize_trace_record(&denied, &record, &runners).is_err());
        authorize_trace_record(&allowed, &record, &runners).unwrap();
        let scope = crate::mcp::ObjectScope {
            device_id: Some("phone".into()),
            package_id: Some("default".into()),
            plugin_id: Some("fixture.plugin".into()),
            runner_id: Some("fixture.runner".into()),
        };
        allowed
            .record_owned_object(ObjectKind::Run, "fixture-run", &scope)
            .unwrap();
        runners.unregister_runner("fixture.runner").unwrap();
        authorize_trace_record(&allowed, &record, &runners).unwrap();
        assert!(authorize_trace_record(&denied, &record, &runners).is_err());
        authorize_trace_record(&all, &record, &runners).unwrap();
        let mut different = record.clone();
        different["runner_id"] = json!("different.runner");
        assert!(authorize_trace_record(&allowed, &different, &runners).is_err());
        runners
            .register_runner(
                "fixture.runner",
                "replacement.plugin",
                Arc::new(TimerRunnerRegistry::new()),
            )
            .unwrap();
        assert!(
            authorize_trace_record(&allowed, &record, &runners).is_err(),
            "an old receipt cannot authorize a new current Runner owner"
        );
    }
    #[tokio::test]
    async fn temporary_work_completion_notifies_waiters_when_the_task_unwinds() {
        let (cancel, _cancellation) = watch::channel(false);
        let (done, _) = watch::channel(false);
        let work = Arc::new(OcrWork { cancel, done });
        let waiting = tokio::spawn({
            let work = work.clone();
            async move { work.cancel_and_wait().await }
        });
        let task = tokio::spawn(async move {
            let _completion = WorkCompletion(work);
            panic!("synthetic task panic");
        });
        assert!(task.await.unwrap_err().is_panic());
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap();
    }
    #[tokio::test]
    async fn temporary_work_waits_for_cleanup_and_accepts_already_finished_work() {
        let (cancel, mut cancellation) = watch::channel(false);
        let (done, _) = watch::channel(false);
        let work = Arc::new(OcrWork { cancel, done });
        let waiting = tokio::spawn({
            let work = work.clone();
            async move {
                work.cancel_and_wait().await;
            }
        });
        cancellation.changed().await.unwrap();
        assert!(*cancellation.borrow());
        assert!(!waiting.is_finished());
        work.done.send_replace(true);
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), work.cancel_and_wait())
            .await
            .unwrap();
    }
    #[test]
    fn only_controlled_source_ids_are_accepted() {
        for source in [
            json!({"kind":"target","device_id":"d","path":"C:/secret.png"}),
            json!({"kind":"sample","sample_id":"s","frame_id":"f","content_sha256":"h","package_id":"p"}),
            json!({"kind":"path","path":"C:/secret.png"}),
        ] {
            assert!(serde_json::from_value::<Source>(source).is_err());
        }
        assert!(serde_json::from_value::<Request>(
            json!({"source":{"kind":"target","device_id":"d"},"png_base64":"forged"})
        )
        .is_err());
    }
}
