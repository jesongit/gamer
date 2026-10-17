//! Model-independent external control. Management starts/resumes a session;
//! external callers may observe, act, pause, keep alive or release their own.
//! The original Core gate and RunManager remain the ownership authorities.

mod capture;
mod input;
mod runner;
mod types;

#[cfg(test)]
mod tests;

pub(crate) use types::{
    Capture, CaptureMetadata, InputAction, SessionBinding, SessionPhase, SessionRecord,
    StartRequest,
};

use anyhow::{ensure, Context, Result};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex as AsyncMutex, Notify};

use crate::{
    capabilities::{CapabilityRegistry, FrameStamp},
    core::{control::ControlLease, AppPackageId, RunPayload, RunRequest},
    device::DeviceManager,
    resources::{PackageActivity, PackageStore},
    run_manager::{RunManager, RunSource, StartRequest as RunStartRequest},
};

pub(crate) const RUNNER_ID: &str = "gamer-external-control";
const MAX_SESSIONS: usize = 64;
const MAX_RECEIPTS: usize = 4096;

#[derive(Clone)]
pub(crate) struct Runtime {
    pub devices: Arc<DeviceManager>,
    pub packages: Arc<PackageStore>,
    pub runs: Arc<RunManager>,
    pub capabilities: CapabilityRegistry,
    pub journal: Option<crate::store::Db>,
    pub token_is_active: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

#[derive(Clone)]
pub(crate) struct ExternalControlService {
    inner: Arc<State>,
}

struct State {
    runtime: Runtime,
    sessions: Mutex<BTreeMap<String, Arc<Session>>>,
    shutting_down: AtomicBool,
}

struct Session {
    record: Mutex<SessionRecord>,
    lease: Mutex<Option<ControlLease>>,
    transition: AsyncMutex<()>,
    operation: AsyncMutex<()>,
    ending: AtomicBool,
    wake: Notify,
    deadline: Mutex<Instant>,
    observation: Mutex<Option<capture::Observation>>,
    binding: Mutex<Option<FrameStamp>>,
    receipts: Mutex<BTreeMap<String, Receipt>>,
    package_activity: Mutex<Option<PackageActivity>>,
}

#[derive(Clone)]
struct Receipt {
    fingerprint: String,
    result: std::result::Result<Value, String>,
}

impl Session {
    fn belongs_to(&self, token_id: &str) -> Result<()> {
        ensure!(
            self.record.lock().token_id == token_id,
            "scope_denied: session belongs to another token"
        );
        Ok(())
    }

    fn check_generation(&self, generation: u64) -> Result<()> {
        ensure!(
            self.record.lock().generation == generation,
            "stale_generation: observe the current session again"
        );
        Ok(())
    }

    fn running(&self, generation: u64) -> Result<SessionRecord> {
        let record = self.record.lock().clone();
        ensure!(
            record.generation == generation,
            "stale_generation: observe the current session again"
        );
        ensure!(
            record.state == SessionPhase::Running && !self.ending.load(Ordering::Acquire),
            "control_paused: user must resume or start control in Gamer"
        );
        ensure!(
            Instant::now() < *self.deadline.lock(),
            "session_expired: control keepalive deadline elapsed"
        );
        Ok(record)
    }

    fn phase(&self, phase: SessionPhase, reason: Option<String>) {
        let mut record = self.record.lock();
        record.state = phase;
        record.reason = reason;
    }

    fn renew(&self) {
        let ttl = self.record.lock().ttl_seconds;
        *self.deadline.lock() = Instant::now() + Duration::from_secs(ttl);
        self.wake.notify_one();
    }

    fn apply_paused(&self, lease: ControlLease, reason: &str) {
        self.record.lock().generation = lease.generation;
        *self.lease.lock() = Some(lease);
        self.observation.lock().take();
        self.phase(SessionPhase::Paused, Some(reason.into()));
    }

    fn apply_resumed(&self, lease: ControlLease) {
        self.record.lock().generation = lease.generation;
        *self.lease.lock() = Some(lease);
        self.observation.lock().take();
        self.receipts.lock().clear();
        self.renew();
        self.phase(SessionPhase::Running, None);
    }

    fn stop(&self, reason: &str) {
        self.ending.store(true, Ordering::Release);
        self.observation.lock().take();
        self.phase(SessionPhase::Stopping, Some(reason.into()));
        self.wake.notify_one();
    }
}

impl ExternalControlService {
    /// Construction only registers an internal RunManager executor. It neither
    /// connects a target nor starts any control/recording/background model work.
    pub(crate) fn new(runtime: Runtime) -> Arc<Self> {
        let inner = Arc::new(State {
            runtime,
            sessions: Mutex::new(BTreeMap::new()),
            shutting_down: AtomicBool::new(false),
        });
        inner.runtime.runs.register_executor(
            RUNNER_ID,
            Arc::new(runner::Executor(Arc::downgrade(&inner))),
        );
        Arc::new(Self { inner })
    }

    pub(crate) async fn start_for_user(
        self: &Arc<Self>,
        request: StartRequest,
    ) -> Result<SessionRecord> {
        ensure!(
            !self.inner.shutting_down.load(Ordering::Acquire),
            "shutting_down"
        );
        validate_identity(&request.token_id, "token_id")?;
        ensure!(
            (10..=86400).contains(&request.ttl_seconds),
            "invalid_args: ttl_seconds must be 10..86400"
        );
        ensure!(
            request.token_label.len() <= 160,
            "invalid_args: token label too long"
        );
        self.inner.active(&request.token_id)?;
        let package = request
            .content_package
            .as_deref()
            .map(AppPackageId::new)
            .transpose()?;
        if let Some(package) = &package {
            self.inner.runtime.packages.manifest(package.as_str())?;
        }
        let app =
            crate::targets::app_context(&self.inner.runtime.devices, &request.device_id, package)?;
        let package_activity = request
            .content_package
            .as_deref()
            .map(|id| self.inner.runtime.packages.acquire_activity(id))
            .transpose()?;
        let id = uuid::Uuid::new_v4().to_string();
        let session = Arc::new(Session {
            record: Mutex::new(SessionRecord {
                session_id: id.clone(),
                run_id: String::new(),
                token_id: request.token_id.clone(),
                token_label: request.token_label,
                device_id: request.device_id.clone(),
                android_package: app.android_package.as_ref().map(|id| id.as_str().into()),
                content_package: request.content_package.clone(),
                state: SessionPhase::Starting,
                generation: 0,
                ttl_seconds: request.ttl_seconds,
                remaining_seconds: request.ttl_seconds,
                reason: None,
                manual_allowed: false,
            }),
            lease: Mutex::new(None),
            transition: AsyncMutex::new(()),
            operation: AsyncMutex::new(()),
            ending: AtomicBool::new(false),
            wake: Notify::new(),
            deadline: Mutex::new(Instant::now() + Duration::from_secs(request.ttl_seconds)),
            observation: Mutex::new(None),
            binding: Mutex::new(None),
            receipts: Mutex::new(BTreeMap::new()),
            package_activity: Mutex::new(package_activity),
        });
        {
            let mut sessions = self.inner.sessions.lock();
            sessions.retain(|_, value| value.record.lock().state != SessionPhase::Finished);
            ensure!(
                sessions.len() < MAX_SESSIONS,
                "session_limit: too many external control sessions"
            );
            sessions.insert(id.clone(), session.clone());
        }
        let entrypoint = request.content_package.as_deref().map_or_else(
            || "external-control/interactive".into(),
            |package| format!("{package}/interactive"),
        );
        let run_request = RunRequest::for_app(
            app,
            RUNNER_ID,
            entrypoint,
            RunPayload::new(json!({"session_id":id})),
        )?;
        let submitted = self.inner.runtime.runs.submit(
            RunStartRequest {
                request: run_request,
                source: RunSource::Manual,
                task_id: None,
                scheduled_at: None,
                realtime_logs: true,
            },
            None,
        );
        let run = match submitted {
            Ok(run) => run,
            Err(error) => {
                self.inner.sessions.lock().remove(&id);
                anyhow::bail!("run_rejected: {error:?}")
            }
        };
        session.record.lock().run_id = run.run_id.clone();
        let state = self.inner.clone();
        let watched = session.clone();
        tokio::spawn(async move {
            state.runtime.runs.wait_terminal(&run.run_id).await;
            // RunManager has released+Dropped its activity lease and archived
            // the slot before this externally visible terminal state. A panic
            // may instead be in the bounded Drop fallback; retry exact-owner
            // cleanup before describing the external controller as finished.
            if tokio::time::timeout(
                Duration::from_secs(10),
                runner::release_control(&state.runtime, &watched),
            )
            .await
            .is_err()
            {
                watched.phase(
                    SessionPhase::Stopping,
                    Some("control_cleanup_pending: retry stop in Gamer".into()),
                );
                return;
            }
            let reason = watched.record.lock().reason.clone();
            watched.phase(SessionPhase::Finished, reason);
            watched.package_activity.lock().take();
            watched.wake.notify_waiters();
        });
        Ok(self.inner.view(&session))
    }

    pub(crate) fn status(&self, token_id: &str, session_id: &str) -> Result<SessionRecord> {
        let session = self.inner.external_session(token_id, session_id)?;
        Ok(self.inner.view(&session))
    }

    /// Management UI may inspect every controller; MCP must use status above.
    pub(crate) fn list_for_user(&self) -> Vec<SessionRecord> {
        self.inner
            .sessions
            .lock()
            .values()
            .map(|session| self.inner.view(session))
            .collect()
    }

    pub(crate) fn status_for_user(&self, session_id: &str) -> Result<SessionRecord> {
        let session = self.inner.session(session_id)?;
        Ok(self.inner.view(&session))
    }

    pub(crate) fn keepalive(
        &self,
        token_id: &str,
        binding: &SessionBinding,
    ) -> Result<SessionRecord> {
        let session = self.inner.external_session(token_id, &binding.session_id)?;
        session.check_generation(binding.generation)?;
        let phase = session.record.lock().state;
        ensure!(
            matches!(phase, SessionPhase::Running | SessionPhase::Paused),
            "control_transition: cannot renew a stopping or unfinished transition"
        );
        ensure!(
            Instant::now() < *session.deadline.lock(),
            "session_expired: cannot revive an expired control session"
        );
        session.renew();
        Ok(self.inner.view(&session))
    }

    pub(crate) async fn pause(
        self: &Arc<Self>,
        token_id: &str,
        binding: SessionBinding,
    ) -> Result<SessionRecord> {
        let state = self.inner.clone();
        let token_id = token_id.to_owned();
        tokio::spawn(async move {
            let session = state.external_session(&token_id, &binding.session_id)?;
            state
                .pause_session(&session, binding.generation, "paused_by_controller")
                .await?;
            Ok(state.view(&session))
        })
        .await
        .context("operation_status_unknown: pause task interrupted")?
    }

    /// Management only. A user may renew a TTL-paused session, but a revoked
    /// credential, ended run or changed browser binding cannot be resumed.
    pub(crate) async fn resume_for_user(
        self: &Arc<Self>,
        session_id: &str,
    ) -> Result<SessionRecord> {
        let state = self.inner.clone();
        let session = state.session(session_id)?;
        tokio::spawn(async move {
            let _transition = session.transition.lock().await;
            let record = session.record.lock().clone();
            state.active(&record.token_id)?;
            ensure!(!session.ending.load(Ordering::Acquire), "session_stopped");
            ensure!(
                record.state == SessionPhase::Paused,
                "control_transition: only paused sessions can resume"
            );
            state.check_app(&record)?;
            if crate::targets::is_browser(&record.device_id) {
                let browser = state.runtime.devices.browsers.session(&record.device_id)?;
                ensure!(
                    browser.is_alive(),
                    "target_disconnected: start a new control session"
                );
                if let Some(stamp) = session.binding.lock().as_ref() {
                    let current = browser.stamp();
                    ensure!(
                        current.target == stamp.target && current.epoch == stamp.epoch,
                        "target_changed: start a new control session"
                    );
                }
            }
            // Android reconnection happens after Core has closed manual input
            // below. Browser activity remains bound to its existing session.
            state.active(&record.token_id)?;
            ensure!(!session.ending.load(Ordering::Acquire), "session_stopped");
            let lease = session.lease.lock().clone().context("control_not_ready")?;
            session.phase(SessionPhase::Resuming, None);
            let next = state
                .runtime
                .devices
                .controls
                .resume(
                    &lease,
                    state
                        .runtime
                        .devices
                        .release_control_inputs(&record.device_id),
                )
                .await?;
            *session.lease.lock() = Some(next.clone());
            session.record.lock().generation = next.generation;
            if !crate::targets::is_browser(&record.device_id) {
                if let Err(error) = state
                    .runtime
                    .devices
                    .controls
                    .execute(
                        &next,
                        crate::targets::prepare(&state.runtime.devices, &record.device_id),
                    )
                    .await
                {
                    state.request_stop(&session, "target_prepare_failed");
                    return Err(error);
                }
                session.binding.lock().take();
            }
            state.active(&record.token_id)?;
            ensure!(!session.ending.load(Ordering::Acquire), "session_stopped");
            session.apply_resumed(next.clone());
            state
                .event(
                    &session,
                    "state",
                    json!({"state":"running","generation":next.generation,"observe_again":true}),
                )
                .await;
            Ok(state.view(&session))
        })
        .await
        .context("operation_status_unknown: resume task interrupted")?
    }

    pub(crate) async fn release(
        self: &Arc<Self>,
        token_id: &str,
        binding: &SessionBinding,
    ) -> Result<SessionRecord> {
        let session = self.inner.external_session(token_id, &binding.session_id)?;
        {
            let _transition = session.transition.lock().await;
            self.inner.active(token_id)?;
            session.check_generation(binding.generation)?;
            self.inner.request_stop(&session, "released_by_controller");
        }
        self.wait_finished(&session).await
    }

    pub(crate) async fn stop_for_user(&self, session_id: &str) -> Result<SessionRecord> {
        let session = self.inner.session(session_id)?;
        self.inner.request_stop(&session, "stopped_by_user");
        self.wait_finished(&session).await
    }

    async fn wait_finished(&self, session: &Arc<Session>) -> Result<SessionRecord> {
        let run_id = session.record.lock().run_id.clone();
        tokio::time::timeout(
            Duration::from_secs(10),
            self.inner.runtime.runs.wait_terminal(&run_id),
        )
        .await
        .context("control_cleanup_pending: query status; the barrier remains closed")?;
        tokio::time::timeout(
            Duration::from_secs(10),
            runner::release_control(&self.inner.runtime, session),
        )
        .await
        .context("control_cleanup_pending: input cleanup still owns the barrier")?;
        let reason = session.record.lock().reason.clone();
        session.phase(SessionPhase::Finished, reason);
        session.package_activity.lock().take();
        Ok(self.inner.view(session))
    }

    /// Called immediately after credential revocation. The executor also checks
    /// expiry independently, including while paused.
    pub(crate) async fn revoke(&self, token_id: &str) {
        let sessions = self
            .inner
            .sessions
            .lock()
            .values()
            .filter(|session| session.record.lock().token_id == token_id)
            .cloned()
            .collect::<Vec<_>>();
        for session in &sessions {
            self.inner.request_stop(session, "token_revoked");
        }
        for session in sessions {
            let _ = self.wait_finished(&session).await;
        }
    }

    pub(crate) async fn shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::Release);
        let sessions = self
            .inner
            .sessions
            .lock()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for session in &sessions {
            self.inner.request_stop(session, "service_stopping");
        }
        for session in sessions {
            let _ = self.wait_finished(&session).await;
        }
    }
}

impl State {
    async fn pause_session(&self, session: &Session, generation: u64, reason: &str) -> Result<()> {
        let _transition = session.transition.lock().await;
        let record = session.record.lock().clone();
        self.active(&record.token_id)?;
        session.check_generation(generation)?;
        if record.state == SessionPhase::Paused {
            return Ok(());
        }
        ensure!(
            record.state == SessionPhase::Running && !session.ending.load(Ordering::Acquire),
            "control_transition: cannot pause this state"
        );
        let lease = session.lease.lock().clone().context("control_not_ready")?;
        session.phase(SessionPhase::Pausing, Some(reason.into()));
        session.observation.lock().take();
        let next = self
            .runtime
            .devices
            .controls
            .pause(
                &lease,
                self.runtime
                    .devices
                    .release_control_inputs(&record.device_id),
            )
            .await?;
        session.apply_paused(next.clone(), reason);
        self.event(
            session,
            "state",
            json!({"state":"paused","generation":next.generation,"reason":reason}),
        )
        .await;
        Ok(())
    }

    fn active(&self, token_id: &str) -> Result<()> {
        ensure!(
            (self.runtime.token_is_active)(token_id),
            "token_inactive: token expired or was revoked"
        );
        Ok(())
    }

    fn session(&self, id: &str) -> Result<Arc<Session>> {
        self.sessions
            .lock()
            .get(id)
            .cloned()
            .context("session_not_found")
    }

    fn external_session(&self, token_id: &str, id: &str) -> Result<Arc<Session>> {
        self.active(token_id)?;
        let session = self.session(id)?;
        session.belongs_to(token_id)?;
        Ok(session)
    }

    fn view(&self, session: &Session) -> SessionRecord {
        let mut record = session.record.lock().clone();
        record.remaining_seconds = session
            .deadline
            .lock()
            .saturating_duration_since(Instant::now())
            .as_secs();
        record.manual_allowed = self
            .runtime
            .devices
            .controls
            .status(&record.device_id)
            .manual_allowed;
        record
    }

    fn check_app(&self, record: &SessionRecord) -> Result<()> {
        let package = record
            .content_package
            .as_deref()
            .map(AppPackageId::new)
            .transpose()?;
        let app = crate::targets::app_context(&self.runtime.devices, &record.device_id, package)?;
        ensure!(
            app.android_package.as_ref().map(|id| id.as_str()) == record.android_package.as_deref(),
            "target_changed: configured application changed; start control again"
        );
        Ok(())
    }

    fn request_stop(&self, session: &Session, reason: &str) {
        if session.record.lock().state == SessionPhase::Finished {
            return;
        }
        session.stop(reason);
        let run_id = session.record.lock().run_id.clone();
        if !run_id.is_empty() {
            self.runtime.runs.cancel(&run_id);
        }
    }

    async fn event(&self, session: &Session, kind: &str, data: Value) {
        let record = session.record.lock().clone();
        if let Some(journal) = &self.runtime.journal {
            if !record.run_id.is_empty() {
                let result = journal.append_run_event(record.run_id, json!({"kind":kind,"source":"external_control","session_id":record.session_id,"token_id":record.token_id,"generation":record.generation,"data":data})).await;
                if let Err(error) = result {
                    tracing::warn!(%error, "external control event persistence failed");
                }
            }
        }
    }
}

fn validate_identity(value: &str, label: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 160 && !value.chars().any(char::is_control),
        "invalid_args: invalid {label}"
    );
    Ok(())
}
