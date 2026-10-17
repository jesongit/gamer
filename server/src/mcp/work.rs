//! Only temporary work is tied to credentials. Independent accepted runs and
//! persistent tasks deliberately do not register in this registry.
use super::{CredentialStore, ErrorKind, McpError};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};
use tokio::sync::OnceCell;

#[async_trait::async_trait]
pub(crate) trait TemporaryWork: Send + Sync {
    /// Must be idempotent and wait for actual cleanup, not just request cancel.
    async fn cancel_and_wait(&self);
}
struct Entry {
    work: Arc<dyn TemporaryWork>,
    released: AtomicBool,
    cancelled: OnceCell<()>,
}
impl Entry {
    async fn drain(&self) {
        self.cancelled
            .get_or_init(|| self.work.cancel_and_wait())
            .await;
    }
}
#[derive(Default)]
pub(crate) struct TokenWorkRegistry {
    entries: Mutex<BTreeMap<(String, String), Arc<Entry>>>,
}
pub(crate) struct TokenWorkLease {
    registry: Weak<TokenWorkRegistry>,
    key: (String, String),
    entry: Arc<Entry>,
}
impl Drop for TokenWorkLease {
    fn drop(&mut self) {
        self.entry.released.store(true, Ordering::Release);
        if let Some(registry) = self.registry.upgrade() {
            registry.entries.lock().unwrap().remove(&self.key);
        }
    }
}
impl TokenWorkRegistry {
    pub(crate) fn register(
        self: &Arc<Self>,
        credentials: Arc<CredentialStore>,
        token_id: &str,
        work_id: &str,
        work: Arc<dyn TemporaryWork>,
    ) -> Result<TokenWorkLease, McpError> {
        if !credentials.is_active(token_id) {
            return Err(McpError::unauthorized());
        }
        if work_id.is_empty() || work_id.len() > 256 {
            return Err(McpError::invalid("Invalid temporary work ID"));
        }
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| McpError::internal("Temporary work requires an asynchronous runtime"))?;
        let key = (token_id.to_owned(), work_id.to_owned());
        let entry = Arc::new(Entry {
            work,
            released: AtomicBool::new(false),
            cancelled: OnceCell::new(),
        });
        {
            let mut entries = self.entries.lock().unwrap();
            // Recheck while holding the same registry lock used by revoke's
            // drain snapshot. A registration racing revocation must either be
            // included in that snapshot or fail before creating new work.
            if !credentials.is_active(token_id) {
                return Err(McpError::unauthorized());
            }
            if entries.len() >= 1024 || entries.contains_key(&key) {
                return Err(McpError::new(
                    ErrorKind::Conflict,
                    "temporary_work_conflict",
                    "Temporary work is already registered or at capacity",
                ));
            }
            entries.insert(key.clone(), entry.clone());
        }
        let token = token_id.to_owned();
        let watched = entry.clone();
        runtime.spawn(async move {
            while !watched.released.load(Ordering::Acquire) {
                if !credentials.is_active(&token) {
                    watched.drain().await;
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        Ok(TokenWorkLease {
            registry: Arc::downgrade(self),
            key,
            entry,
        })
    }
    pub(crate) async fn drain_token(&self, token_id: &str) {
        let entries: Vec<_> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter(|((token, _), _)| token == token_id)
            .map(|(_, entry)| entry.clone())
            .collect();
        futures_util::future::join_all(entries.iter().map(|entry| entry.drain())).await;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    struct Work(AtomicUsize);
    #[async_trait::async_trait]
    impl TemporaryWork for Work {
        async fn cancel_and_wait(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[tokio::test]
    async fn revoke_drains_work_once() {
        let root = tempfile::tempdir().unwrap();
        let credentials = Arc::new(CredentialStore::open(root.path()).unwrap());
        let token = credentials
            .create(super::super::CreateTokenRequest {
                label: "work".into(),
                scopes: ["run.submit".into()].into(),
                allowed: Default::default(),
                expires_in_seconds: 60,
            })
            .unwrap();
        let registry = Arc::new(TokenWorkRegistry::default());
        let work = Arc::new(Work(AtomicUsize::new(0)));
        let lease = registry
            .register(
                credentials.clone(),
                &token.metadata.token_id,
                "r",
                work.clone(),
            )
            .unwrap();
        credentials.revoke(&token.metadata.token_id).unwrap();
        registry.drain_token(&token.metadata.token_id).await;
        registry.drain_token(&token.metadata.token_id).await;
        assert_eq!(work.0.load(Ordering::SeqCst), 1);
        drop(lease);
        assert!(registry
            .register(credentials, &token.metadata.token_id, "late", work)
            .is_err());
    }

    #[tokio::test]
    async fn expiration_cancels_temporary_work_without_a_management_request() {
        let root = tempfile::tempdir().unwrap();
        let credentials = Arc::new(CredentialStore::open(root.path()).unwrap());
        let token = credentials
            .create(super::super::CreateTokenRequest {
                label: "expires".into(),
                scopes: ["fixture.read".into()].into(),
                allowed: Default::default(),
                expires_in_seconds: 2,
            })
            .unwrap();
        let registry = Arc::new(TokenWorkRegistry::default());
        let work = Arc::new(Work(AtomicUsize::new(0)));
        let _lease = registry
            .register(
                credentials,
                &token.metadata.token_id,
                "temporary",
                work.clone(),
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(4), async {
            while work.0.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("Expired temporary work must be cancelled by its credential watcher");
        assert_eq!(work.0.load(Ordering::SeqCst), 1);
    }
}
