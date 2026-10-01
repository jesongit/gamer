//! Target input ownership. The operation gate is also the pause/resume barrier.
//! Identities are supplied by trusted callers, never decoded from client JSON.
use serde::Serialize;
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlLease {
    pub target: String,
    pub owner: String,
    pub generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualLease {
    target: String,
    generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlPhase {
    Idle,
    Running,
    Pausing,
    Paused,
    Resuming,
    Stopping,
}
#[derive(Clone, Debug, Serialize)]
pub struct ControlStatus {
    pub phase: ControlPhase,
    pub owner: Option<String>,
    pub generation: u64,
    pub manual_allowed: bool,
}
struct State {
    owner: Option<String>,
    generation: u64,
    manual_generation: u64,
    phase: ControlPhase,
}
struct Target {
    state: Mutex<State>,
    gate: Arc<AsyncMutex<()>>,
}
struct ClaimRecovery {
    target: Arc<Target>,
    lease: ControlLease,
    previous_manual_generation: u64,
    completed: bool,
}
impl Drop for ClaimRecovery {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let mut state = self.target.state.lock().unwrap();
        if state.owner.as_deref() == Some(&self.lease.owner)
            && state.generation == self.lease.generation
            && state.phase == ControlPhase::Resuming
        {
            // Claim has not returned a lease or injected anything. Preserve
            // the existing manual contacts and their ability to send UP.
            state.owner = None;
            state.phase = ControlPhase::Idle;
            state.manual_generation = self.previous_manual_generation;
        }
    }
}
struct TransitionRecovery {
    target: Arc<Target>,
    original: ControlLease,
    current_generation: u64,
    completed: bool,
}
impl Drop for TransitionRecovery {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let mut state = self.target.state.lock().unwrap();
        if state.owner.as_deref() == Some(&self.original.owner)
            && state.generation == self.current_generation
        {
            // Keep the closed transition phase, while restoring the caller's
            // stop/retry identity. Its old decisions remain rejected by phase.
            state.generation = self.original.generation;
        }
    }
}
impl Default for Target {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                owner: None,
                generation: 0,
                manual_generation: 0,
                phase: ControlPhase::Idle,
            }),
            gate: Arc::new(AsyncMutex::new(())),
        }
    }
}
#[derive(Default)]
pub struct ControlRegistry {
    targets: Mutex<HashMap<String, Arc<Target>>>,
}
#[derive(Clone, PartialEq, Eq)]
enum Identity {
    Owned(ControlLease),
    Manual(ManualLease),
    Automatic(String),
    Cleanup(String),
}
#[derive(Clone)]
pub struct ControlIdentity(
    Identity,
    #[cfg_attr(not(any(test, feature = "wasm-runtime")), allow(dead_code))]
    Option<Arc<OwnedMutexGuard<()>>>,
);
tokio::task_local! { static ADMISSION: Identity; }
tokio::task_local! { static ADMISSION_GUARD: Arc<OwnedMutexGuard<()>>; }
pub struct ControlPermit {
    _gate: Option<Arc<OwnedMutexGuard<()>>>,
    identity: Identity,
}
impl ControlPermit {
    pub async fn scope<R>(self, future: impl Future<Output = R>) -> R {
        ADMISSION
            .scope(self.identity.clone(), async {
                if let Some(gate) = self._gate {
                    ADMISSION_GUARD.scope(gate, future).await
                } else {
                    future.await
                }
            })
            .await
    }
}
impl ControlIdentity {
    #[cfg(feature = "wasm-runtime")]
    pub(crate) fn detached(&self) -> Self {
        Self(self.0.clone(), None)
    }
    #[cfg(feature = "wasm-runtime")]
    pub(crate) fn same_origin(&self, other: &Self) -> bool {
        self.0 == other.0
    }
    /// Only trusted host code may propagate an already admitted action to a
    /// worker. The shared guard keeps the barrier closed if its caller cancels.
    #[cfg(any(test, feature = "wasm-runtime"))]
    pub(crate) fn current() -> Option<Self> {
        ADMISSION
            .try_with(|i| Self(i.clone(), ADMISSION_GUARD.try_with(Clone::clone).ok()))
            .ok()
    }
    #[cfg(any(test, feature = "wasm-runtime"))]
    pub(crate) async fn scope<R>(&self, future: impl Future<Output = R>) -> R {
        ADMISSION
            .scope(self.0.clone(), async {
                if let Some(gate) = self.1.clone() {
                    ADMISSION_GUARD.scope(gate, future).await
                } else {
                    future.await
                }
            })
            .await
    }
}
impl ControlRegistry {
    fn target(&self, id: &str) -> Arc<Target> {
        self.targets
            .lock()
            .unwrap()
            .entry(id.into())
            .or_default()
            .clone()
    }
    pub fn status(&self, id: &str) -> ControlStatus {
        let target = self.target(id);
        let state = target.state.lock().unwrap();
        ControlStatus {
            phase: state.phase,
            owner: state.owner.clone(),
            generation: state.generation,
            manual_allowed: matches!(state.phase, ControlPhase::Idle | ControlPhase::Paused),
        }
    }
    pub fn manual_lease(&self, id: &str) -> ManualLease {
        let target = self.target(id);
        let generation = target.state.lock().unwrap().manual_generation;
        ManualLease {
            target: id.into(),
            generation,
        }
    }
    fn check(state: &State, identity: &Identity) -> anyhow::Result<()> {
        match identity {
            Identity::Owned(lease) => {
                anyhow::ensure!(
                    state.owner.as_deref() == Some(&lease.owner)
                        && state.generation == lease.generation,
                    "stale_generation: 控制会话已变化"
                );
                anyhow::ensure!(
                    state.phase == ControlPhase::Running,
                    "ai_paused: AI 当前不能执行操作"
                );
            }
            Identity::Manual(lease) => {
                anyhow::ensure!(
                    state.manual_generation == lease.generation,
                    "stale_generation: 人工输入已过期"
                );
                anyhow::ensure!(
                    matches!(state.phase, ControlPhase::Idle | ControlPhase::Paused),
                    "control_owned: 请先暂停 AI，等待暂停完成后再人工操作"
                );
            }
            Identity::Automatic(_) => anyhow::ensure!(
                state.phase == ControlPhase::Idle,
                "control_owned: 目标控制权属于其他运行"
            ),
            Identity::Cleanup(_) => {}
        }
        Ok(())
    }
    async fn enter(&self, id: &str, identity: Identity) -> anyhow::Result<ControlPermit> {
        let target = self.target(id);
        // Reject immediately while a barrier waits for a currently running action.
        Self::check(&target.state.lock().unwrap(), &identity)?;
        let gate = target.gate.clone().lock_owned().await;
        Self::check(&target.state.lock().unwrap(), &identity)?;
        Ok(ControlPermit {
            _gate: Some(Arc::new(gate)),
            identity,
        })
    }
    /// Capability adapters enter automatically, or inherit the already-held
    /// manual/AI admission (including keymap transformations in the same task).
    pub async fn admit(&self, id: &str) -> anyhow::Result<ControlPermit> {
        if let Ok(identity) = ADMISSION.try_with(Clone::clone) {
            let identity_target = match &identity {
                Identity::Owned(l) => &l.target,
                Identity::Manual(l) => &l.target,
                Identity::Automatic(t) | Identity::Cleanup(t) => t,
            };
            anyhow::ensure!(
                identity_target == id,
                "control_target_mismatch: 操作目标与控制会话不同"
            );
            // An admitted operation may finish after pause starts. Re-checking
            // its generation midway would prevent its UP/cleanup from completing.
            return Ok(ControlPermit {
                _gate: ADMISSION_GUARD.try_with(Clone::clone).ok(),
                identity,
            });
        }
        self.enter(id, Identity::Automatic(id.into())).await
    }
    pub fn identity(&self, id: &str) -> ControlIdentity {
        ControlIdentity(
            ADMISSION
                .try_with(Clone::clone)
                .unwrap_or_else(|_| Identity::Automatic(id.into())),
            None,
        )
    }
    pub async fn admit_identity(
        &self,
        id: &str,
        identity: &ControlIdentity,
    ) -> anyhow::Result<ControlPermit> {
        // Persistent touch handles retain their original owner/generation even
        // when a later viewer or run happens to invoke their cleanup.
        if ADMISSION
            .try_with(|current| match (current, &identity.0) {
                (Identity::Owned(a), Identity::Owned(b)) => a == b,
                (Identity::Manual(a), Identity::Manual(b)) => a == b,
                (Identity::Automatic(a), Identity::Automatic(b)) => a == b,
                _ => false,
            })
            .unwrap_or(false)
        {
            return Ok(ControlPermit {
                _gate: ADMISSION_GUARD.try_with(Clone::clone).ok(),
                identity: identity.0.clone(),
            });
        }
        let target = self.target(id);
        Self::check(&target.state.lock().unwrap(), &identity.0)?;
        self.enter(id, identity.0.clone()).await
    }
    pub async fn manual<R>(
        &self,
        id: &str,
        future: impl Future<Output = anyhow::Result<R>>,
    ) -> anyhow::Result<R> {
        self.manual_with(&self.manual_lease(id), future).await
    }
    pub async fn admit_manual(&self, id: &str) -> anyhow::Result<ControlPermit> {
        if let Ok(identity) = ADMISSION.try_with(Clone::clone) {
            anyhow::ensure!(
                matches!(&identity,Identity::Manual(l) if l.target==id),
                "control_source_mismatch: 人工来源不能覆盖自动化入场身份"
            );
            return Ok(ControlPermit {
                _gate: ADMISSION_GUARD.try_with(Clone::clone).ok(),
                identity,
            });
        }
        self.enter(id, Identity::Manual(self.manual_lease(id)))
            .await
    }
    pub async fn manual_with<R>(
        &self,
        lease: &ManualLease,
        future: impl Future<Output = anyhow::Result<R>>,
    ) -> anyhow::Result<R> {
        if let Ok(identity) = ADMISSION.try_with(Clone::clone) {
            anyhow::ensure!(
                matches!(&identity,Identity::Manual(l) if l==lease),
                "control_source_mismatch: 人工来源不能覆盖已有入场身份"
            );
            return ControlPermit {
                _gate: ADMISSION_GUARD.try_with(Clone::clone).ok(),
                identity,
            }
            .scope(future)
            .await;
        }
        self.enter(&lease.target, Identity::Manual(lease.clone()))
            .await?
            .scope(future)
            .await
    }
    pub async fn execute<R>(
        &self,
        lease: &ControlLease,
        future: impl Future<Output = anyhow::Result<R>>,
    ) -> anyhow::Result<R> {
        self.enter(&lease.target, Identity::Owned(lease.clone()))
            .await?
            .scope(future)
            .await
    }
    pub async fn claim(&self, id: &str, owner: &str) -> anyhow::Result<ControlLease> {
        anyhow::ensure!(!owner.is_empty(), "empty control owner");
        let target = self.target(id);
        let (lease, previous_manual_generation) = {
            let mut s = target.state.lock().unwrap();
            anyhow::ensure!(
                s.phase == ControlPhase::Idle,
                "control_owned: 目标已有控制会话"
            );
            s.owner = Some(owner.into());
            s.generation += 1;
            let previous_manual_generation = s.manual_generation;
            s.manual_generation += 1;
            s.phase = ControlPhase::Resuming;
            (
                ControlLease {
                    target: id.into(),
                    owner: owner.into(),
                    generation: s.generation,
                },
                previous_manual_generation,
            )
        };
        let mut recovery = ClaimRecovery {
            target: target.clone(),
            lease: lease.clone(),
            previous_manual_generation,
            completed: false,
        };
        let _gate = target.gate.lock().await;
        target.state.lock().unwrap().phase = ControlPhase::Running;
        recovery.completed = true;
        Ok(lease)
    }
    async fn transition(
        &self,
        lease: &ControlLease,
        from: ControlPhase,
        during: ControlPhase,
        after: ControlPhase,
        cleanup: impl Future<Output = anyhow::Result<()>>,
    ) -> anyhow::Result<ControlLease> {
        let target = self.target(&lease.target);
        let next = {
            let mut s = target.state.lock().unwrap();
            anyhow::ensure!(
                s.owner.as_deref() == Some(&lease.owner) && s.generation == lease.generation,
                "stale_generation: 控制会话已变化"
            );
            anyhow::ensure!(s.phase == from, "control_transition: 控制状态不允许此操作");
            s.phase = during;
            s.generation += 1;
            s.manual_generation += 1;
            ControlLease {
                generation: s.generation,
                ..lease.clone()
            }
        };
        let mut recovery = TransitionRecovery {
            target: target.clone(),
            original: lease.clone(),
            current_generation: next.generation,
            completed: false,
        };
        let _gate = target.gate.lock().await;
        ADMISSION
            .scope(Identity::Cleanup(lease.target.clone()), cleanup)
            .await?;
        let mut s = target.state.lock().unwrap();
        anyhow::ensure!(
            s.owner.as_deref() == Some(&next.owner) && s.generation == next.generation,
            "stale_generation: 控制会话已变化"
        );
        s.phase = after;
        if after == ControlPhase::Idle {
            s.owner = None;
        }
        recovery.completed = true;
        Ok(next)
    }
    pub async fn pause(
        &self,
        lease: &ControlLease,
        cleanup: impl Future<Output = anyhow::Result<()>>,
    ) -> anyhow::Result<ControlLease> {
        self.transition(
            lease,
            ControlPhase::Running,
            ControlPhase::Pausing,
            ControlPhase::Paused,
            cleanup,
        )
        .await
    }
    pub async fn resume(
        &self,
        lease: &ControlLease,
        cleanup: impl Future<Output = anyhow::Result<()>>,
    ) -> anyhow::Result<ControlLease> {
        self.transition(
            lease,
            ControlPhase::Paused,
            ControlPhase::Resuming,
            ControlPhase::Running,
            cleanup,
        )
        .await
    }
    /// Release is idempotent for stale leases; old session finalizers cannot
    /// clean up input belonging to a newer owner or generation.
    pub async fn release(
        &self,
        lease: &ControlLease,
        cleanup: impl Future<Output = anyhow::Result<()>>,
    ) -> anyhow::Result<()> {
        let target = self.target(&lease.target);
        let from = {
            let s = target.state.lock().unwrap();
            if s.owner.as_deref() != Some(&lease.owner) || s.generation != lease.generation {
                return Ok(());
            }
            s.phase
        };
        self.transition(
            lease,
            from,
            ControlPhase::Stopping,
            ControlPhase::Idle,
            cleanup,
        )
        .await
        .map(|_| ())
    }
    pub async fn cleanup_manual(
        &self,
        lease: &ManualLease,
        cleanup: impl Future<Output = anyhow::Result<()>>,
    ) -> anyhow::Result<()> {
        let target = self.target(&lease.target);
        let _gate = target.gate.lock().await;
        {
            let s = target.state.lock().unwrap();
            if s.manual_generation != lease.generation
                || !matches!(s.phase, ControlPhase::Idle | ControlPhase::Paused)
            {
                return Ok(());
            }
        }
        ADMISSION
            .scope(Identity::Cleanup(lease.target.clone()), cleanup)
            .await
    }
    pub fn manual_scoped(id: &str) -> bool {
        ADMISSION
            .try_with(|i| matches!(i,Identity::Manual(l) if l.target==id))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn cancelled_claim_preserves_inflight_manual_input_and_allows_new_owner() {
        let controls = Arc::new(ControlRegistry::default());
        let viewer = controls.manual_lease("d");
        let entered = Arc::new(tokio::sync::Notify::new());
        let finish = Arc::new(tokio::sync::Notify::new());
        let (c, ticket, e, f) = (
            controls.clone(),
            viewer.clone(),
            entered.clone(),
            finish.clone(),
        );
        let manual = tokio::spawn(async move {
            c.manual_with(&ticket, async {
                e.notify_one();
                f.notified().await;
                Ok(())
            })
            .await
        });
        entered.notified().await;
        let c = controls.clone();
        let claim = tokio::spawn(async move { c.claim("d", "cancelled").await });
        tokio::task::yield_now().await;
        assert_eq!(controls.status("d").phase, ControlPhase::Resuming);
        assert!(!claim.is_finished());
        claim.abort();
        let _ = claim.await;
        let status = controls.status("d");
        assert_eq!(status.phase, ControlPhase::Idle);
        assert!(status.owner.is_none());
        assert!(
            !manual.is_finished(),
            "claim cancellation must not cancel manual input"
        );
        finish.notify_one();
        manual.await.unwrap().unwrap();
        // A contact begun before the reservation can still release itself.
        controls
            .manual_with(&viewer, async { Ok(()) })
            .await
            .unwrap();
        let next = controls.claim("d", "next").await.unwrap();
        assert!(controls
            .manual_with(&viewer, async { Ok(()) })
            .await
            .is_err());
        controls.execute(&next, async { Ok(()) }).await.unwrap();
        controls.release(&next, async { Ok(()) }).await.unwrap();
    }
    #[tokio::test]
    async fn failed_or_cancelled_cleanup_keeps_manual_closed_and_can_be_stopped_with_original_lease(
    ) {
        let controls = Arc::new(ControlRegistry::default());
        let lease = controls.claim("d", "ai").await.unwrap();
        assert!(controls
            .pause(&lease, async { anyhow::bail!("socket unavailable") })
            .await
            .is_err());
        assert!(!controls.status("d").manual_allowed);
        assert!(controls.execute(&lease, async { Ok(()) }).await.is_err());
        assert!(controls
            .release(&lease, async { anyhow::bail!("socket still unavailable") })
            .await
            .is_err());
        controls.release(&lease, async { Ok(()) }).await.unwrap();
        assert!(controls.status("d").manual_allowed);
        let second = controls.claim("d", "next").await.unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let (c, l, e) = (controls.clone(), second.clone(), entered.clone());
        let pause = tokio::spawn(async move {
            c.pause(&l, async {
                e.notify_one();
                std::future::pending::<anyhow::Result<()>>().await
            })
            .await
        });
        entered.notified().await;
        pause.abort();
        let _ = pause.await;
        controls.release(&second, async { Ok(()) }).await.unwrap();
        assert!(controls.status("d").manual_allowed);
    }
    #[tokio::test]
    async fn transferred_admission_keeps_worker_inside_barrier_after_caller_finishes() {
        let controls = Arc::new(ControlRegistry::default());
        let finish = Arc::new(tokio::sync::Notify::new());
        let (c, f) = (controls.clone(), finish.clone());
        let worker = controls
            .manual("d", async {
                let identity = ControlIdentity::current().unwrap();
                Ok(tokio::spawn(async move {
                    identity
                        .scope(async {
                            f.notified().await;
                            let permit = c.admit("d").await.unwrap();
                            permit.scope(async { Ok::<_, anyhow::Error>(()) }).await
                        })
                        .await
                }))
            })
            .await
            .unwrap();
        let c = controls.clone();
        let claim = tokio::spawn(async move { c.claim("d", "ai").await });
        tokio::task::yield_now().await;
        assert!(
            !claim.is_finished(),
            "worker retains shared admission after original caller returns"
        );
        finish.notify_one();
        worker.await.unwrap().unwrap();
        let lease = claim.await.unwrap().unwrap();
        controls.release(&lease, async { Ok(()) }).await.unwrap();
    }
    #[tokio::test]
    async fn pause_drains_injection_before_manual_and_stale_actions_never_execute() {
        let controls = Arc::new(ControlRegistry::default());
        let lease = controls.claim("d", "ai").await.unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let finish = Arc::new(tokio::sync::Notify::new());
        let injected = Arc::new(AtomicUsize::new(0));
        let (c, l, e, f, i) = (
            controls.clone(),
            lease.clone(),
            entered.clone(),
            finish.clone(),
            injected.clone(),
        );
        let action = tokio::spawn(async move {
            c.execute(&l, async {
                i.fetch_add(1, Ordering::SeqCst);
                e.notify_one();
                f.notified().await;
                i.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
        });
        entered.notified().await;
        let (c, l, i) = (controls.clone(), lease.clone(), injected.clone());
        let pause = tokio::spawn(async move {
            c.pause(&l, async {
                assert_eq!(i.load(Ordering::SeqCst), 2);
                Ok(())
            })
            .await
        });
        tokio::task::yield_now().await;
        assert!(!controls.status("d").manual_allowed);
        assert!(controls
            .manual("d", async {
                panic!("manual injected before pause barrier");
                #[allow(unreachable_code)]
                Ok(())
            })
            .await
            .is_err());
        finish.notify_one();
        action.await.unwrap().unwrap();
        let paused = pause.await.unwrap().unwrap();
        controls
            .manual("d", async {
                injected.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .unwrap();
        let resumed = controls.resume(&paused, async { Ok(()) }).await.unwrap();
        assert!(controls
            .execute(&lease, async {
                panic!("stale decision injected");
                #[allow(unreachable_code)]
                Ok(())
            })
            .await
            .is_err());
        controls.execute(&resumed, async { Ok(()) }).await.unwrap();
    }
    #[tokio::test]
    async fn resume_drains_manual_and_old_viewer_cleanup_cannot_release_new_owner() {
        let controls = Arc::new(ControlRegistry::default());
        let viewer = controls.manual_lease("d");
        let first = controls.claim("d", "first").await.unwrap();
        let paused = controls.pause(&first, async { Ok(()) }).await.unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let finish = Arc::new(tokio::sync::Notify::new());
        let (c, e, f) = (controls.clone(), entered.clone(), finish.clone());
        let manual = tokio::spawn(async move {
            c.manual("d", async {
                e.notify_one();
                f.notified().await;
                Ok(())
            })
            .await
        });
        entered.notified().await;
        let (c, p) = (controls.clone(), paused.clone());
        let resume = tokio::spawn(async move { c.resume(&p, async { Ok(()) }).await });
        tokio::task::yield_now().await;
        assert!(!controls.status("d").manual_allowed);
        finish.notify_one();
        manual.await.unwrap().unwrap();
        let current = resume.await.unwrap().unwrap();
        controls
            .cleanup_manual(&viewer, async {
                panic!("old viewer released current owner");
                #[allow(unreachable_code)]
                Ok(())
            })
            .await
            .unwrap();
        controls
            .release(&first, async {
                panic!("old generation released current owner");
                #[allow(unreachable_code)]
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(controls.status("d").owner.as_deref(), Some("first"));
        controls.release(&current, async { Ok(()) }).await.unwrap();
        let new = controls.claim("d", "second").await.unwrap();
        controls
            .release(&current, async {
                panic!("old owner cleaned new owner");
                #[allow(unreachable_code)]
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(controls.status("d").owner.as_deref(), Some("second"));
        controls.release(&new, async { Ok(()) }).await.unwrap();
    }
}
