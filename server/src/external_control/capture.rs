use super::*;
use crate::capabilities::{DeviceHandle, DeviceId, FrameHandle, FrameService, TouchPoint};
use image::{GenericImageView, ImageFormat};
use std::io::Cursor;

#[derive(Clone)]
pub(super) struct Observation {
    pub metadata: CaptureMetadata,
}

impl Observation {
    pub(super) fn point(&self, x: f64, y: f64) -> Result<TouchPoint> {
        let meta = &self.metadata;
        ensure!(
            x.is_finite()
                && y.is_finite()
                && x >= 0.0
                && y >= 0.0
                && x < f64::from(meta.width)
                && y < f64::from(meta.height),
            "invalid_args: coordinate outside the returned screenshot"
        );
        ensure!(meta.width > 0 && meta.height > 0, "invalid_frame_size");
        Ok(TouchPoint::new(
            (x * f64::from(meta.original_width) / f64::from(meta.width)).floor() as u32,
            (y * f64::from(meta.original_height) / f64::from(meta.height)).floor() as u32,
            1.0,
        ))
    }
}

struct AcquiredFrame<'a>(&'a dyn FrameService, FrameHandle);
impl Drop for AcquiredFrame<'_> {
    fn drop(&mut self) {
        self.0.release(self.1);
    }
}

impl ExternalControlService {
    /// Without a session this is observation only; the caller must authorize
    /// the requested target. Such a frame can never authorize a control input.
    pub(crate) async fn capture(
        self: &Arc<Self>,
        token_id: &str,
        device_id: &str,
        binding: Option<SessionBinding>,
        max_width: u32,
    ) -> Result<Capture> {
        self.inner.active(token_id)?;
        ensure!(
            (320..=1920).contains(&max_width),
            "invalid_args: max_width must be 320..1920"
        );
        let token_id = token_id.to_owned();
        let device_id = device_id.to_owned();
        let state = self.inner.clone();
        tokio::spawn(async move {
            if let Some(binding) = binding {
                let session = state.external_session(&token_id, &binding.session_id)?;
                let _operation = session.operation.lock().await;
                state.active(&token_id)?;
                let record = session.running(binding.generation)?;
                ensure!(
                    record.device_id == device_id,
                    "scope_denied: session target mismatch"
                );
                let lease = session.lease.lock().clone().context("control_not_ready")?;
                ensure!(
                    lease.generation == binding.generation && lease.owner == record.session_id,
                    "stale_generation"
                );
                let capture = state
                    .runtime
                    .devices
                    .controls
                    .execute(
                        &lease,
                        state.capture_frame(&device_id, Some(&record), max_width),
                    )
                    .await?;
                // Pause, revocation or expiry during encoding must not install
                // an observation for a newer generation or renew its deadline.
                state.active(&token_id)?;
                session.running(binding.generation)?;
                if let (Some(previous), Some(current)) = (
                    session.binding.lock().as_ref(),
                    capture.metadata.source.as_ref(),
                ) {
                    ensure!(
                        previous.target == current.target && previous.epoch == current.epoch,
                        "target_changed: pause and confirm the target before resuming"
                    );
                }
                if let Some(stamp) = &capture.metadata.source {
                    *session.binding.lock() = Some(stamp.clone());
                }
                *session.observation.lock() = Some(Observation {
                    metadata: capture.metadata.clone(),
                });
                state
                    .event(&session, "observation", json!(&capture.metadata))
                    .await;
                // Capture itself is observation and deliberately does not keep
                // direct control alive; only keepalive or admitted input does.
                Ok(capture)
            } else {
                crate::targets::capabilities(&state.runtime.devices, &device_id)?;
                let capture = state.capture_frame(&device_id, None, max_width).await?;
                state.active(&token_id)?;
                Ok(capture)
            }
        })
        .await
        .context("observation_failed: capture task interrupted")?
    }
}

impl State {
    async fn capture_frame(
        &self,
        device_id: &str,
        session: Option<&SessionRecord>,
        max_width: u32,
    ) -> Result<Capture> {
        let frames = self
            .runtime
            .capabilities
            .frame()
            .context("frame_unavailable")?;
        let handle = DeviceHandle::new(DeviceId::new(device_id));
        let frame = frames.capture(&handle).await?;
        let acquired = AcquiredFrame(frames, frame);
        let size = frames.size(frame).await?;
        ensure!(
            size.width > 0
                && size.height > 0
                && size.width <= 8192
                && size.height <= 8192
                && u64::from(size.width) * u64::from(size.height) <= 16 * 1024 * 1024,
            "invalid_frame_size: image exceeds the observation budget"
        );
        let stamp = frames.stamp(frame).await?;
        if session.is_some() {
            ensure!(
                stamp.is_some(),
                "frame_identity_unavailable: cannot authorize control from an unidentified frame"
            );
        }
        let bytes = frames.image_png(frame, None).await?;
        ensure!(bytes.len() <= 28 * 1024 * 1024, "frame_too_large");
        let observation_id = uuid::Uuid::new_v4().to_string();
        let image_id = if let Some(record) = session {
            frames.trace_frame(frame, json!({"run_id":record.run_id,"device_id":device_id,"kind":"external_observation","path":"external_control.capture","observation_id":observation_id,"session_id":record.session_id}), false).await?
        } else {
            None
        };
        drop(acquired);
        let (png, width, height) = tokio::task::spawn_blocking(move || -> Result<_> {
            let image = image::load_from_memory_with_format(&bytes, ImageFormat::Png)?;
            ensure!(
                image.dimensions() == (size.width, size.height),
                "frame_size_mismatch"
            );
            if size.width <= max_width {
                return Ok((bytes, size.width, size.height));
            }
            let resized = image.resize(
                max_width,
                ((u64::from(size.height) * u64::from(max_width)) / u64::from(size.width)).max(1)
                    as u32,
                image::imageops::FilterType::Triangle,
            );
            let (width, height) = resized.dimensions();
            let mut output = Cursor::new(Vec::new());
            resized.write_to(&mut output, ImageFormat::Png)?;
            Ok((output.into_inner(), width, height))
        })
        .await??;
        Ok(Capture {
            png,
            metadata: CaptureMetadata {
                frame_id: observation_id,
                device_id: device_id.into(),
                session_id: session.map(|record| record.session_id.clone()),
                generation: session.map_or(0, |record| record.generation),
                captured_at: chrono::Utc::now().to_rfc3339(),
                width,
                height,
                original_width: size.width,
                original_height: size.height,
                source: stamp,
                image_id,
            },
        })
    }
}
