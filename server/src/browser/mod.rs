//! Browser I/O only. Websites, login, queues and game workflows belong to YAML.
mod pointer_lock;
mod session;
mod transport;
use crate::{config::Config, store::Db};
use serde::{Deserialize, Serialize};
pub(crate) use session::{CdpSession, FrameStamp};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
pub(crate) use transport::ScreencastFrame;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserTarget {
    pub id: String,
    pub name: String,
    pub url: String,
    pub profile_id: String,
    pub width: u32,
    pub height: u32,
}
impl BrowserTarget {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.id.starts_with("browser-") && self.id.len() <= 80,
            "浏览器目标 ID 必须以 browser- 开头且不超过 80 字符"
        );
        crate::resources::validate_scope_id("browser target", &self.id)?;
        crate::resources::validate_scope_id("browser profile", &self.profile_id)?;
        anyhow::ensure!(
            self.profile_id.len() <= 80 && !self.name.trim().is_empty() && self.name.len() <= 255,
            "目标名称或资料 ID 无效"
        );
        let url = reqwest::Url::parse(&self.url)?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none(),
            "仅支持无凭据的 http/https 页面地址"
        );
        anyhow::ensure!(
            (320..=3840).contains(&self.width) && (240..=2160).contains(&self.height),
            "视口尺寸超出支持范围"
        );
        Ok(())
    }
}

pub struct BrowserManager {
    pub db: Db,
    cfg: Config,
    sessions: parking_lot::RwLock<HashMap<String, Arc<CdpSession>>>,
    gates: parking_lot::Mutex<HashMap<String, Arc<Mutex<()>>>>,
    shutting_down: std::sync::atomic::AtomicBool,
    idle: parking_lot::Mutex<HashMap<String, IdleTimer>>,
    pub events: tokio::sync::broadcast::Sender<crate::core::RuntimeEvent>,
    pub controls: Arc<crate::core::control::ControlRegistry>,
}
impl BrowserManager {
    pub fn with_controls(
        db: Db,
        cfg: Config,
        controls: Arc<crate::core::control::ControlRegistry>,
    ) -> Self {
        Self {
            db,
            cfg,
            sessions: Default::default(),
            gates: Default::default(),
            shutting_down: std::sync::atomic::AtomicBool::new(false),
            idle: Default::default(),
            events: tokio::sync::broadcast::channel(256).0,
            controls,
        }
    }
    pub fn get(&self, id: &str) -> anyhow::Result<BrowserTarget> {
        self.db
            .browser_targets()?
            .into_iter()
            .find(|x| x.id == id)
            .ok_or_else(|| anyhow::anyhow!("浏览器目标不存在: {id}"))
    }
    pub fn session(&self, id: &str) -> anyhow::Result<Arc<CdpSession>> {
        self.sessions
            .read()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("浏览器尚未连接"))
    }
    fn lifecycle_gate(&self, id: &str) -> Arc<Mutex<()>> {
        self.gates.lock().entry(id.into()).or_default().clone()
    }
    pub async fn prepare(&self, id: &str) -> anyhow::Result<Arc<CdpSession>> {
        let gate = self.lifecycle_gate(id);
        let _gate = gate.lock().await;
        anyhow::ensure!(
            !self.shutting_down.load(std::sync::atomic::Ordering::SeqCst),
            "服务正在关闭"
        );
        // Explicitly reconnecting is activity, even if Chrome is still alive.
        self.idle.lock().remove(id);
        if let Ok(session) = self.session(id) {
            if session.is_alive() {
                return Ok(session);
            }
        }
        if let Ok(s) = self.session(id) {
            anyhow::ensure!(
                s.runs.load(std::sync::atomic::Ordering::SeqCst) == 0,
                "目标仍被任务占用"
            );
        }
        self.close_inner(id).await;
        let target = self.get(id)?;
        let session = CdpSession::launch_with_controls(
            &target,
            &self.cfg,
            browser_executable(&self.cfg)?,
            self.controls.clone(),
        )
        .await?;
        self.sessions
            .write()
            .insert(id.to_string(), session.clone());
        Ok(session)
    }
    async fn close_inner(&self, id: &str) {
        self.idle.lock().remove(id);
        let session = self.sessions.write().remove(id);
        if let Some(session) = session {
            session.close().await;
        }
    }
    pub(crate) async fn output_stream(
        &self,
        id: &str,
    ) -> anyhow::Result<(
        Arc<CdpSession>,
        tokio::sync::watch::Receiver<Option<Arc<transport::ScreencastFrame>>>,
    )> {
        let gate = self.lifecycle_gate(id);
        let _gate = gate.lock().await;
        anyhow::ensure!(
            !self.shutting_down.load(std::sync::atomic::Ordering::SeqCst),
            "服务正在关闭"
        );
        let session = self.session(id)?;
        let frames = session.start_media_output().await?;
        self.idle.lock().remove(id);
        Ok((session, frames))
    }
    pub async fn close(
        &self,
        id: &str,
        runs: &crate::run_manager::RunManager,
    ) -> anyhow::Result<()> {
        let gate = self.lifecycle_gate(id);
        let _gate = gate.lock().await;
        anyhow::ensure!(runs.active_for_device(id).is_none(), "目标正在执行任务");
        if let Ok(s) = self.session(id) {
            anyhow::ensure!(
                s.runs.load(std::sync::atomic::Ordering::SeqCst) == 0,
                "目标仍被任务占用"
            );
        }
        self.close_inner(id).await;
        Ok(())
    }
    pub async fn save(
        &self,
        target: BrowserTarget,
        runs: &crate::run_manager::RunManager,
    ) -> anyhow::Result<()> {
        let gate = self.lifecycle_gate(&target.id);
        let _gate = gate.lock().await;
        let old = self.get(&target.id).ok();
        if let Some(old) = &old {
            anyhow::ensure!(
                old.profile_id == target.profile_id,
                "已有目标的账号资料 ID 不可变更，请新增目标"
            );
        }
        // A display-name change does not touch the bound page, its viewport,
        // or its input lease. Only connection settings require a closed target.
        let metadata_only = old.as_ref().is_some_and(|old| {
            old.url == target.url && old.width == target.width && old.height == target.height
        });
        if !metadata_only {
            anyhow::ensure!(
                runs.active_for_device(&target.id).is_none(),
                "目标正在执行任务"
            );
            anyhow::ensure!(
                self.session(&target.id).is_err(),
                "请先关闭浏览器再修改网址或画面尺寸"
            );
        }
        self.db.save_browser_target(target)
    }
    pub async fn remove(
        &self,
        id: &str,
        runs: &crate::run_manager::RunManager,
    ) -> anyhow::Result<()> {
        let gate = self.lifecycle_gate(id);
        let _gate = gate.lock().await;
        anyhow::ensure!(runs.active_for_device(id).is_none(), "目标正在执行任务");
        anyhow::ensure!(self.session(id).is_err(), "请先关闭浏览器再删除目标");
        self.db.delete_browser_target(id)
    }
    pub async fn rebind(
        &self,
        id: &str,
        page: &str,
        runs: &crate::run_manager::RunManager,
    ) -> anyhow::Result<()> {
        let gate = self.lifecycle_gate(id);
        let _gate = gate.lock().await;
        anyhow::ensure!(runs.active_for_device(id).is_none(), "目标正在执行任务");
        let old = self.session(id)?;
        let new = old.rebind(page).await?;
        self.sessions.write().insert(id.into(), new);
        self.idle.lock().remove(id);
        Ok(())
    }

    /// Acquire under the same lifecycle gate as prepare/rebind/reclamation.
    /// A caller racing process shutdown must retry preparation, rather than
    /// receiving a lease for an already closing browser.
    pub(crate) fn acquire(&self, id: &str) -> anyhow::Result<BrowserRunLease> {
        let gate = self.lifecycle_gate(id);
        let _gate = gate
            .try_lock()
            .map_err(|_| anyhow::anyhow!("浏览器连接正在变化，请重新连接后重试"))?;
        anyhow::ensure!(
            !self.shutting_down.load(std::sync::atomic::Ordering::SeqCst),
            "服务正在关闭"
        );
        let session = self.session(id)?;
        anyhow::ensure!(session.is_alive(), "目标已断开");
        session
            .runs
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.idle.lock().remove(id);
        Ok(BrowserRunLease::new(session))
    }

    /// Only explicit policy enables this. Missing previews never imply that a
    /// web page has no business activity (for example, a cloud-game queue).
    pub(crate) async fn reclaim_idle(
        &self,
        now: Instant,
        external_consumer: impl Fn(&str) -> bool,
    ) {
        let timeout = self.cfg.current_settings().browser_idle_secs;
        if timeout == 0 {
            self.idle.lock().clear();
            return;
        }
        let sessions: Vec<_> = self
            .sessions
            .read()
            .iter()
            .map(|(id, session)| (id.clone(), session.clone()))
            .collect();
        for (id, session) in sessions {
            // Each target has its own lifecycle gate: batch preparation of a
            // different target cannot make this target's run acquire fail.
            let gate = self.lifecycle_gate(&id);
            let Ok(_gate) = gate.try_lock() else { continue };
            let timeout = self.cfg.current_settings().browser_idle_secs;
            if timeout == 0 {
                self.idle.lock().clear();
                return;
            }
            if !self
                .session(&id)
                .is_ok_and(|current| Arc::ptr_eq(&current, &session))
            {
                continue;
            }
            let busy = external_consumer(&id)
                || self.controls.status(&id).owner.is_some()
                || !session.can_idle();
            let ready = self.idle.lock().entry(id.clone()).or_default().ready(
                now,
                Duration::from_secs(timeout),
                busy,
            );
            if !ready {
                continue;
            }
            // Reserve the viewer before checking stream ownership again.
            // A WebSocket upgrade that already owns it is an active consumer.
            let Ok(_viewer) = session.viewer.clone().try_lock_owned() else {
                self.idle.lock().remove(&id);
                continue;
            };
            if external_consumer(&id) || !session.try_idle_close().await {
                self.idle.lock().remove(&id);
                continue;
            }
            self.sessions.write().remove(&id);
            self.idle.lock().remove(&id);
            tracing::info!(target_id = %id, idle_secs = timeout,
                "idle browser closed; persistent profile retained");
        }
    }
    pub async fn shutdown(&self) {
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // Include launches that already own their gate but have not inserted
        // their session. New prepare/acquire reject the shutdown marker.
        let ids: Vec<_> = self.gates.lock().keys().cloned().collect();
        for id in ids {
            let gate = self.lifecycle_gate(&id);
            let _gate = gate.lock().await;
            self.close_inner(&id).await;
        }
    }
}

/// Start conservatively at the first observation after consumers disappear.
/// This can postpone reclamation by one tick, but never shortens the grace.
#[derive(Default)]
struct IdleTimer {
    since: Option<Instant>,
}
impl IdleTimer {
    fn ready(&mut self, now: Instant, timeout: Duration, busy: bool) -> bool {
        if busy || timeout.is_zero() {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now);
        now.saturating_duration_since(since) >= timeout
    }
}

fn browser_executable(cfg: &Config) -> anyhow::Result<PathBuf> {
    if !cfg.browser_path.trim().is_empty() {
        let path = PathBuf::from(&cfg.browser_path);
        anyhow::ensure!(path.is_file(), "配置的浏览器路径不存在");
        return Ok(path);
    }
    for base in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(base) {
            for suffix in [
                "Microsoft/Edge/Application/msedge.exe",
                "Google/Chrome/Application/chrome.exe",
            ] {
                let path = PathBuf::from(&base).join(suffix);
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
    }
    for path in [
        "/usr/bin/chromium",
        "/usr/bin/google-chrome",
        "/usr/bin/chromium-browser",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ] {
        if PathBuf::from(path).is_file() {
            return Ok(path.into());
        }
    }
    anyhow::bail!("未找到浏览器，请配置 browser_path")
}

pub struct BrowserRunLease {
    session: Arc<CdpSession>,
    manual_ticket: crate::core::control::ManualLease,
    released: bool,
}
impl BrowserRunLease {
    pub(crate) fn new(session: Arc<CdpSession>) -> Self {
        let manual_ticket = session.controls.manual_lease(&session.id);
        Self {
            session,
            manual_ticket,
            released: false,
        }
    }

    async fn cleanup(
        session: &CdpSession,
        ticket: &crate::core::control::ManualLease,
        timeout: Option<Duration>,
    ) {
        if session.controls.manual_lease(&session.id) != *ticket {
            return;
        }
        let _ = session
            .controls
            .cleanup_manual(ticket, async {
                if let Some(timeout) = timeout {
                    if tokio::time::timeout(timeout, session.release_inputs())
                        .await
                        .is_err()
                    {
                        // The original ticket has passed admission and still
                        // holds the Core gate, so invalidation cannot hit a
                        // newer owner's session after partially draining inputs.
                        session.invalidate();
                    }
                } else {
                    session.release_inputs().await;
                }
                Ok(())
            })
            .await;
    }
}
impl crate::core::ActivityLease for BrowserRunLease {
    fn release(&mut self) -> futures_util::future::BoxFuture<'_, ()> {
        Box::pin(async move {
            if self.released {
                return;
            }
            Self::cleanup(&self.session, &self.manual_ticket, None).await;
            self.session
                .runs
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            self.released = true;
        })
    }
}
impl Drop for BrowserRunLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let session = self.session.clone();
        let ticket = self.manual_ticket.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                // An abandoned old lease must not invalidate a new owner's
                // session merely because that owner's gate remains busy.
                Self::cleanup(&session, &ticket, Some(Duration::from_secs(3))).await;
                session
                    .runs
                    .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            });
        } else {
            session
                .runs
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod idle_tests;
#[cfg(test)]
mod refresh_cleanup_tests;
#[cfg(test)]
mod tests;
