use super::*;
use crate::capabilities::{AppId, DeviceHandle, DeviceId, KeyAction, SwipeGesture, TextInput};

impl ExternalControlService {
    /// The owned task survives an HTTP disconnect. The same operation_id can
    /// only retrieve its previous result, including a failure after partial I/O.
    pub(crate) async fn input(
        self: &Arc<Self>,
        token_id: &str,
        binding: SessionBinding,
        operation_id: &str,
        frame_id: &str,
        action: InputAction,
    ) -> Result<Value> {
        self.inner.active(token_id)?;
        validate_identity(operation_id, "operation_id")?;
        validate_identity(frame_id, "frame_id")?;
        action.validate()?;
        let state = self.inner.clone();
        let token_id = token_id.to_owned();
        let operation_id = operation_id.to_owned();
        let frame_id = frame_id.to_owned();
        tokio::spawn(async move {
            let session = state.external_session(&token_id, &binding.session_id)?;
            let _operation = session.operation.lock().await;
            state.active(&token_id)?;
            let record = session.running(binding.generation)?;
            let key = format!("{}:{operation_id}", binding.generation);
            let fingerprint = serde_json::to_string(&json!({"frame_id":frame_id,"action":action}))?;
            if let Some(receipt) = session.receipts.lock().get(&key).cloned() {
                ensure!(receipt.fingerprint == fingerprint, "operation_conflict: operation_id parameters differ");
                return receipt.result.map_err(anyhow::Error::msg);
            }
            ensure!(session.receipts.lock().len() < MAX_RECEIPTS, "receipt_limit: pause and resume before sending more input");
            let lease = session.lease.lock().clone().context("control_not_ready")?;
            ensure!(lease.generation == binding.generation && lease.owner == record.session_id && lease.target == record.device_id, "stale_generation");
            let mut logged = serde_json::to_value(&action)?;
            if matches!(action, InputAction::Text { .. }) { logged["text"] = json!("[redacted]"); }
            state.event(&session, "tool_start", json!({"operation_id":operation_id,"frame_id":frame_id,"action":logged})).await;
            // Reserve a receipt before a possibly partial input. A panic leaves
            // an explicit unknown result; retrying never blindly injects twice.
            session.receipts.lock().insert(key.clone(), Receipt { fingerprint: fingerprint.clone(), result: Err("operation_status_unknown: admitted operation has not completed".into()) });
            let result = state.runtime.devices.controls.execute(&lease, async {
                state.active(&token_id)?;
                session.running(binding.generation)?;
                let observation = session.observation.lock().clone().context("stale_frame: capture before input")?;
                ensure!(observation.metadata.generation == binding.generation && observation.metadata.frame_id == frame_id, "stale_frame: capture before input");
                let handle = DeviceHandle::new(DeviceId::new(&record.device_id));
                let frame_service = state.runtime.capabilities.frame().context("frame_unavailable")?;
                let (size, stamp) = frame_service.coordinate_space(&handle).await?;
                ensure!(size.width == observation.metadata.original_width && size.height == observation.metadata.original_height && stamp == observation.metadata.source, "stale_frame: target epoch or coordinate revision changed");
                state.check_app(&record)?;
                let stamp = stamp.context("frame_identity_unavailable")?;
                let handle = handle.with_expected_frame(stamp.clone());
                // Even a partially failed action consumes its observation.
                session.observation.lock().take();
                // Keep the existing recording event vocabulary. Received-call
                // audit above identifies the external origin independently.
                crate::capabilities::adapters::with_caller_input_source("runner", execute_action(&state.runtime, &record, &handle, &observation, &action)).await?;
                Ok(json!({"ok":true,"session_id":record.session_id,"generation":binding.generation,"operation_id":operation_id,"observe_again":true}))
            }).await;
            let cached = result.as_ref().map(Clone::clone).map_err(ToString::to_string);
            session.receipts.lock().insert(key, Receipt { fingerprint, result: cached });
            if result.is_ok() {
                // Do not renew a generation that pause/resume/expiry replaced
                // while an already-admitted action drained under Core's gate.
                if session.running(binding.generation).is_ok() && state.active(&token_id).is_ok() { session.renew(); }
            }
            state.event(&session, "tool_end", json!({"operation_id":operation_id,"frame_id":frame_id,"ok":result.is_ok(),"error":result.as_ref().err().map(ToString::to_string)})).await;
            result
        }).await.context("operation_status_unknown: input task interrupted; retry with the same operation_id")?
    }
}

async fn execute_action(
    runtime: &Runtime,
    record: &SessionRecord,
    handle: &DeviceHandle,
    observation: &capture::Observation,
    action: &InputAction,
) -> Result<()> {
    let input = runtime.capabilities.input().context("input_unavailable")?;
    let stamp = observation.metadata.source.as_ref();
    match action {
        InputAction::Tap { x, y } => {
            input
                .tap_from_frame(handle, observation.point(*x, *y)?, stamp)
                .await?
        }
        InputAction::Hover { x, y } => {
            ensure!(
                crate::targets::is_browser(&record.device_id),
                "capability_unavailable: hover requires a browser target"
            );
            input
                .hover_from_frame(handle, observation.point(*x, *y)?, stamp)
                .await?;
        }
        InputAction::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        } => {
            input
                .swipe_from_frame(
                    handle,
                    SwipeGesture::new(
                        observation.point(*x1, *y1)?,
                        observation.point(*x2, *y2)?,
                        Duration::from_millis(*duration_ms),
                    ),
                    stamp,
                )
                .await?
        }
        InputAction::Press { x, y, duration_ms } => {
            let point = observation.point(*x, *y)?;
            if crate::targets::is_browser(&record.device_id) {
                let browser = runtime.devices.browsers.session(&record.device_id)?;
                browser
                    .input(
                        &json!({"type":"pointer","action":"down","x":point.x(),"y":point.y()}),
                        stamp,
                    )
                    .await?;
                tokio::time::sleep(Duration::from_millis(*duration_ms)).await;
                browser
                    .input(
                        &json!({"type":"pointer","action":"up","x":point.x(),"y":point.y()}),
                        stamp,
                    )
                    .await?;
            } else {
                let touch = runtime.capabilities.touch().context("touch_unavailable")?;
                let contact = touch.begin(handle, point).await?;
                tokio::time::sleep(Duration::from_millis(*duration_ms)).await;
                touch.end(&contact).await?;
            }
        }
        InputAction::Key { key, duration_ms } => {
            if *duration_ms == 0 {
                input.key_named(handle, key, KeyAction::Press).await?;
            } else {
                input.key_named(handle, key, KeyAction::Down).await?;
                tokio::time::sleep(Duration::from_millis(*duration_ms)).await;
                input.key_named(handle, key, KeyAction::Up).await?;
            }
        }
        InputAction::Text { text } => input.text(handle, TextInput::new(text)).await?,
        InputAction::AppLaunch | InputAction::AppStop => {
            ensure!(
                !crate::targets::is_browser(&record.device_id),
                "capability_unavailable: browser has no Android application"
            );
            let app = AppId::new(
                record
                    .android_package
                    .as_ref()
                    .context("android_app_missing")?,
            );
            let device = runtime
                .capabilities
                .device()
                .context("device_unavailable")?;
            if matches!(action, InputAction::AppLaunch) {
                device.start_app(handle, &app).await?;
            } else {
                device.stop_app(handle, &app).await?;
            }
        }
    }
    Ok(())
}
