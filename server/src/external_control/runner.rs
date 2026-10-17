use super::*;
use crate::core::{ActivityLease, RunContext};
use crate::run_manager::RunExecutor;
use futures_util::future::BoxFuture;
use std::sync::Weak;

pub(super) struct Executor(pub Weak<State>);

impl RunExecutor for Executor {
    fn prepare<'a>(
        &'a self,
        context: &'a RunContext,
        request: &'a RunRequest,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let state = self.0.upgrade().context("external_control_unavailable")?;
            let id = request.payload.as_value()["session_id"]
                .as_str()
                .context("invalid_session_payload")?;
            let session = state.session(id)?;
            let _transition = session.transition.lock().await;
            let record = session.record.lock().clone();
            ensure!(
                record.state == SessionPhase::Starting && !session.ending.load(Ordering::Acquire),
                "session_stopped"
            );
            ensure!(
                record.device_id == context.device_id().as_str(),
                "session_target_mismatch"
            );
            state.active(&record.token_id)?;
            state.check_app(&record)?;
            session.record.lock().run_id = context.run_id.as_str().into();
            // No input claim here: a prepare/acquire failure must never leave
            // ownership behind before RunManager owns its activity lease.
            if crate::targets::is_browser(&record.device_id) {
                let browser = state.runtime.devices.browsers.session(&record.device_id)?;
                ensure!(
                    browser.is_alive(),
                    "target_disconnected: connect the browser before starting control"
                );
            }
            Ok(())
        })
    }

    fn acquire(&self, context: &RunContext) -> Result<Box<dyn ActivityLease>> {
        let state = self.0.upgrade().context("external_control_unavailable")?;
        let session = state
            .sessions
            .lock()
            .values()
            .find(|session| session.record.lock().run_id == context.run_id.as_str())
            .cloned()
            .context("session_not_found")?;
        let activity =
            crate::targets::acquire(&state.runtime.devices, context.device_id().as_str())?;
        Ok(Box::new(ExternalActivityLease {
            runtime: state.runtime.clone(),
            session,
            activity: Some(activity),
            released: false,
        }))
    }

    fn execute<'a>(
        &'a self,
        _context: &'a RunContext,
        request: &'a RunRequest,
        _realtime_logs: bool,
        stop: Arc<AtomicBool>,
    ) -> BoxFuture<'a, Result<Vec<(String, String)>>> {
        Box::pin(async move {
            let state = self.0.upgrade().context("external_control_unavailable")?;
            let id = request.payload.as_value()["session_id"]
                .as_str()
                .context("invalid_session_payload")?;
            let session = state.session(id)?;
            {
                let _transition = session.transition.lock().await;
                let record = session.record.lock().clone();
                state.active(&record.token_id)?;
                ensure!(
                    !session.ending.load(Ordering::Acquire) && !stop.load(Ordering::Acquire),
                    "session_stopped"
                );
                let lease = state
                    .runtime
                    .devices
                    .controls
                    .claim(&record.device_id, &record.session_id)
                    .await?;
                // Synchronous storage immediately after claim; the activity
                // lease's panic/abort fallback can now find this exact lease.
                *session.lease.lock() = Some(lease.clone());
                session.record.lock().generation = lease.generation;
                state
                    .runtime
                    .devices
                    .controls
                    .execute(
                        &lease,
                        state
                            .runtime
                            .devices
                            .release_control_inputs(&record.device_id),
                    )
                    .await?;
                if !crate::targets::is_browser(&record.device_id) {
                    state
                        .runtime
                        .devices
                        .controls
                        .execute(
                            &lease,
                            crate::targets::prepare(&state.runtime.devices, &record.device_id),
                        )
                        .await?;
                }
                state.active(&record.token_id)?;
                ensure!(
                    !session.ending.load(Ordering::Acquire) && !stop.load(Ordering::Acquire),
                    "session_stopped"
                );
                state.check_app(&record)?;
                session.phase(SessionPhase::Running, None);
                if let Some(frames) = state.runtime.capabilities.frame() {
                    frames.trace_snapshot(record.run_id.clone(), json!({"runner_id":RUNNER_ID,"device_id":record.device_id,"content_package":record.content_package,"session_id":record.session_id,"token_id":record.token_id,"source":"external_control"})).await?;
                }
                state
                    .event(
                        &session,
                        "state",
                        json!({"state":"running","generation":lease.generation}),
                    )
                    .await;
            }
            loop {
                let notified = session.wake.notified();
                if stop.load(Ordering::Acquire) || session.ending.load(Ordering::Acquire) {
                    break;
                }
                let record = session.record.lock().clone();
                if !(state.runtime.token_is_active)(&record.token_id) {
                    state.request_stop(&session, "token_expired_or_revoked");
                    break;
                }
                // Loss of keepalive pauses for manual takeover and keeps the
                // same slot. Only a user resume can renew an expired TTL.
                if record.state == SessionPhase::Running
                    && Instant::now() >= *session.deadline.lock()
                {
                    if let Err(error) = state
                        .pause_session(&session, record.generation, "control_ttl_expired")
                        .await
                    {
                        tracing::warn!(%error, "External control TTL pause failed; keeping barrier closed");
                    }
                }
                tokio::select! { _ = notified => {}, _ = tokio::time::sleep(Duration::from_millis(250)) => {} }
            }
            let reason = session
                .record
                .lock()
                .reason
                .clone()
                .unwrap_or_else(|| "cancelled".into());
            session.stop(&reason);
            Ok(vec![(
                "info".into(),
                "External control stopped; waiting for input and activity cleanup".into(),
            )])
        })
    }
}

/// Release failure deliberately keeps RunManager's slot and Core's closed gate.
/// A success terminal is impossible until the original activity is released and
/// Dropped by RunManager. Panic/abort uses the bounded Drop fallback below.
struct ExternalActivityLease {
    runtime: Runtime,
    session: Arc<Session>,
    activity: Option<Box<dyn ActivityLease>>,
    released: bool,
}

pub(super) async fn release_control(runtime: &Runtime, session: &Session) {
    loop {
        let result: Result<()> = {
            let _transition = session.transition.lock().await;
            session.ending.store(true, Ordering::Release);
            session.observation.lock().take();
            let lease = session.lease.lock().clone();
            match lease {
                None => return,
                Some(lease) => match runtime
                    .devices
                    .controls
                    .release(
                        &lease,
                        runtime.devices.release_control_inputs(&lease.target),
                    )
                    .await
                {
                    Ok(()) => {
                        session.lease.lock().take();
                        return;
                    }
                    Err(error) => Err(error),
                },
            }
        };
        if let Err(error) = result {
            session.phase(
                SessionPhase::Stopping,
                Some(format!("input_cleanup_failed: {error}")),
            );
            tracing::warn!(%error, "External control cleanup failed; keeping barrier and run slot closed");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

impl ActivityLease for ExternalActivityLease {
    fn release(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let reason = self.session.record.lock().reason.clone();
            self.session.phase(SessionPhase::Stopping, reason);
            release_control(&self.runtime, &self.session).await;
            if let Some(mut activity) = self.activity.take() {
                activity.release().await;
                drop(activity);
            }
            self.released = true;
        })
    }
}

impl Drop for ExternalActivityLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.session.ending.store(true, Ordering::Release);
        let runtime = self.runtime.clone();
        let session = self.session.clone();
        let activity = self.activity.take();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = tokio::time::timeout(
                    Duration::from_secs(5),
                    release_control(&runtime, &session),
                )
                .await;
                if let Some(mut activity) = activity {
                    activity.release().await;
                    drop(activity);
                }
                // On timeout the Core barrier remains closed; do not fabricate
                // an idle or finished control session.
            });
        }
    }
}
