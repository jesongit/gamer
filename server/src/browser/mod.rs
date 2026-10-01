//! Browser I/O only. Websites, login, queues and game workflows belong to YAML.
mod session;
mod transport;
use crate::{config::Config, store::Db};
use serde::{Deserialize, Serialize};
pub(crate) use session::{CdpSession, FrameStamp};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;

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
    gate: Mutex<()>,
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
            gate: Mutex::new(()),
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
    pub async fn prepare(&self, id: &str) -> anyhow::Result<Arc<CdpSession>> {
        let _gate = self.gate.lock().await;
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
        let session = self.sessions.write().remove(id);
        if let Some(session) = session {
            session.close().await;
        }
    }
    pub async fn close(
        &self,
        id: &str,
        runs: &crate::run_manager::RunManager,
    ) -> anyhow::Result<()> {
        let _gate = self.gate.lock().await;
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
        let _gate = self.gate.lock().await;
        anyhow::ensure!(
            runs.active_for_device(&target.id).is_none(),
            "目标正在执行任务"
        );
        anyhow::ensure!(
            self.session(&target.id).is_err(),
            "请先关闭浏览器再修改配置"
        );
        if let Ok(old) = self.get(&target.id) {
            anyhow::ensure!(
                old.profile_id == target.profile_id,
                "已有目标的账号资料 ID 不可变更，请新增目标"
            );
        }
        self.db.save_browser_target(target)
    }
    pub async fn remove(
        &self,
        id: &str,
        runs: &crate::run_manager::RunManager,
    ) -> anyhow::Result<()> {
        let _gate = self.gate.lock().await;
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
        let _gate = self.gate.lock().await;
        anyhow::ensure!(runs.active_for_device(id).is_none(), "目标正在执行任务");
        let old = self.session(id)?;
        let new = old.rebind(page).await?;
        self.sessions.write().insert(id.into(), new);
        Ok(())
    }
    pub async fn shutdown(&self) {
        let _gate = self.gate.lock().await;
        let ids: Vec<_> = self.sessions.read().keys().cloned().collect();
        for id in ids {
            self.close_inner(&id).await;
        }
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
mod tests;
