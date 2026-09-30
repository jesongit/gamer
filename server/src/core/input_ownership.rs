//! Optional exclusive input ownership. Watching never acquires ownership.
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use uuid::Uuid;

#[derive(Clone)]
pub struct Permit {
    device: String,
    token: Uuid,
    stop: Arc<AtomicBool>,
}
type Cleanup =
    Arc<dyn Fn() -> futures_util::future::BoxFuture<'static, anyhow::Result<()>> + Send + Sync>;
struct Owner {
    permit: Permit,
    inflight: usize,
    revoked: bool,
    cleanup: Option<Cleanup>,
    takeover: Arc<tokio::sync::Mutex<()>>,
}
#[derive(Default)]
struct InputState {
    owners: HashMap<String, Owner>,
    manual: HashMap<String, usize>,
}
static INPUT: std::sync::LazyLock<Mutex<InputState>> =
    std::sync::LazyLock::new(|| Mutex::new(InputState::default()));
tokio::task_local! { static CALLER: Permit; static ACCEPTED: bool; static CLEANUP: bool; }
pub struct Lease(pub Permit);
impl Drop for Lease {
    fn drop(&mut self) {
        let mut owners = INPUT.lock();
        if let Some(owner) = owners
            .owners
            .get_mut(&self.0.device)
            .filter(|o| o.permit.token == self.0.token)
        {
            if owner.cleanup.is_some() || owner.inflight > 0 {
                owner.revoked = true;
                owner.permit.stop.store(true, Ordering::Release);
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    let device = self.0.device.clone();
                    runtime.spawn(async move {
                        if let Err(error) = takeover(&device).await {
                            tracing::warn!(%device,%error,"input cleanup requires manual retry");
                        }
                    });
                }
                return;
            }
        }
        if owners
            .owners
            .get(&self.0.device)
            .is_some_and(|o| o.permit.token == self.0.token && o.inflight == 0 && !o.revoked)
        {
            owners.owners.remove(&self.0.device);
        }
    }
}
pub struct Operation(Option<(String, Option<Uuid>)>);
impl Drop for Operation {
    fn drop(&mut self) {
        if let Some((device, token)) = &self.0 {
            let mut owners = INPUT.lock();
            if let Some(token) = token {
                if let Some(o) = owners
                    .owners
                    .get_mut(device)
                    .filter(|o| o.permit.token == *token)
                {
                    o.inflight = o.inflight.saturating_sub(1);
                }
            } else if let Some(n) = owners.manual.get_mut(device) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    owners.manual.remove(device);
                }
            }
        }
    }
}
pub fn acquire(device: &str, stop: Arc<AtomicBool>) -> anyhow::Result<Lease> {
    let mut owners = INPUT.lock();
    anyhow::ensure!(
        !owners.owners.contains_key(device) && owners.manual.get(device).copied().unwrap_or(0) == 0,
        "input_owned: 请先接管设备"
    );
    let permit = Permit {
        device: device.into(),
        token: Uuid::new_v4(),
        stop,
    };
    owners.owners.insert(
        device.into(),
        Owner {
            permit: permit.clone(),
            inflight: 0,
            revoked: false,
            cleanup: None,
            takeover: Arc::new(tokio::sync::Mutex::new(())),
        },
    );
    Ok(Lease(permit))
}
pub fn attach_cleanup(permit: &Permit, cleanup: Cleanup) -> anyhow::Result<()> {
    let mut owners = INPUT.lock();
    let owner = owners
        .owners
        .get_mut(&permit.device)
        .filter(|o| o.permit.token == permit.token)
        .ok_or_else(|| anyhow::anyhow!("input ownership expired"))?;
    owner.cleanup = Some(cleanup);
    Ok(())
}
/// Called only after successful host cleanup; Drop may then release immediately.
pub fn cleaned(permit: &Permit) {
    let mut owners = INPUT.lock();
    if let Some(owner) = owners
        .owners
        .get_mut(&permit.device)
        .filter(|o| o.permit.token == permit.token)
    {
        owner.cleanup = None;
    }
}
pub fn admit(device: &str) -> anyhow::Result<Operation> {
    if CLEANUP.try_with(|v| *v).unwrap_or(false) || ACCEPTED.try_with(|v| *v).unwrap_or(false) {
        return Ok(Operation(None));
    }
    let mut owners = INPUT.lock();
    let Some(owner) = owners.owners.get_mut(device) else {
        anyhow::ensure!(
            CALLER.try_with(|_| ()).is_err(),
            "CANCELLED: input ownership expired"
        );
        *owners.manual.entry(device.into()).or_default() += 1;
        return Ok(Operation(Some((device.into(), None))));
    };
    let token = CALLER.try_with(|p| p.token).ok();
    anyhow::ensure!(
        token == Some(owner.permit.token)
            && !owner.revoked
            && !owner.permit.stop.load(Ordering::Acquire),
        "input_owned: 请先接管设备"
    );
    owner.inflight += 1;
    Ok(Operation(Some((device.into(), Some(owner.permit.token)))))
}
pub async fn scope<T>(permit: Permit, future: impl std::future::Future<Output = T>) -> T {
    CALLER.scope(permit, future).await
}
pub async fn operation<T>(
    device: &str,
    future: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    let _lease = admit(device)?;
    ACCEPTED.scope(true, future).await
}
/// Host-only release of already held keys/touches; never used for new input.
pub async fn cleanup<T>(future: impl std::future::Future<Output = T>) -> T {
    CLEANUP.scope(true, future).await
}
pub async fn takeover(device: &str) -> anyhow::Result<()> {
    let (token, gate) = {
        let mut owners = INPUT.lock();
        if let Some(owner) = owners.owners.get_mut(device) {
            owner.revoked = true;
            owner.permit.stop.store(true, Ordering::Release);
            (owner.permit.token, owner.takeover.clone())
        } else {
            return Ok(());
        }
    };
    let _takeover = gate.lock().await;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let ready = {
            let owners = INPUT.lock();
            let Some(owner) = owners
                .owners
                .get(device)
                .filter(|o| o.permit.token == token)
            else {
                return Ok(());
            };
            owner.inflight == 0
        };
        if ready {
            let release = INPUT
                .lock()
                .owners
                .get(device)
                .and_then(|o| o.cleanup.clone());
            if let Some(release) = release {
                tokio::time::timeout_at(deadline, cleanup(release()))
                    .await
                    .map_err(|_| anyhow::anyhow!("input_cleanup_timeout"))??;
            }
            let mut owners = INPUT.lock();
            if owners
                .owners
                .get(device)
                .is_some_and(|o| o.permit.token == token)
            {
                owners.owners.remove(device);
            }
            return Ok(());
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "input_drain_timeout: 设备输入尚未收尾"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
pub fn current(permit: &Permit) -> bool {
    INPUT.lock().owners.get(&permit.device).is_some_and(|o| {
        o.permit.token == permit.token && !o.revoked && !permit.stop.load(Ordering::Acquire)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn stale_permit_never_becomes_manual_input_after_owner_release() {
        let device = Uuid::new_v4().to_string();
        let lease = acquire(&device, Arc::new(AtomicBool::new(false))).unwrap();
        let permit = lease.0.clone();
        takeover(&device).await.unwrap();
        assert!(scope(permit, async { admit(&device) }).await.is_err());
        assert!(admit(&device).is_ok());
    }
    #[tokio::test]
    async fn manual_operation_blocks_acquisition_and_drop_releases_held_input() {
        use std::sync::atomic::AtomicUsize;
        let device = Uuid::new_v4().to_string();
        let manual = admit(&device).unwrap();
        assert!(acquire(&device, Arc::new(AtomicBool::new(false))).is_err());
        drop(manual);
        let lease = acquire(&device, Arc::new(AtomicBool::new(false))).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        attach_cleanup(
            &lease.0,
            Arc::new(move || {
                let calls = calls.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        )
        .unwrap();
        drop(lease);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while count.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        takeover(&device).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(admit(&device).is_ok());
    }
    #[tokio::test]
    async fn takeover_closes_all_new_input_before_drain() {
        let stop = Arc::new(AtomicBool::new(false));
        let lease = acquire("ownership-test", stop.clone()).unwrap();
        assert!(admit("ownership-test").is_err());
        let accepted = scope(lease.0.clone(), async { admit("ownership-test").unwrap() }).await;
        let task = tokio::spawn(async { takeover("ownership-test").await });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(stop.load(Ordering::Acquire));
        assert!(!current(&lease.0));
        assert!(scope(lease.0.clone(), async { admit("ownership-test") })
            .await
            .is_err());
        drop(accepted);
        task.await.unwrap().unwrap();
        assert!(admit("ownership-test").is_ok());
    }
}
