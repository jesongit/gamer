//! Run-scoped policy supplied by the parent executor, never by guest JSON.
use super::{input_ownership::Permit, AppContext};
use async_trait::async_trait;
use std::{
    collections::HashMap,
    sync::{atomic::AtomicBool, Arc},
};
#[async_trait]
pub trait Policy: Send + Sync {
    async fn before(
        &self,
        context: &AppContext,
        operation: serde_json::Value,
    ) -> anyhow::Result<()>;
    async fn after(&self, _context: &AppContext, _ok: bool) -> anyhow::Result<()> {
        Ok(())
    }
}
#[derive(Clone)]
pub struct Scope {
    pub policy: Arc<dyn Policy>,
    pub input: Permit,
}
static SCOPES: std::sync::LazyLock<parking_lot::Mutex<HashMap<usize, Scope>>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(HashMap::new()));
pub struct Registration(Arc<AtomicBool>);
pub fn register(stop: Arc<AtomicBool>, scope: Scope) -> Registration {
    SCOPES.lock().insert(Arc::as_ptr(&stop) as usize, scope);
    Registration(stop)
}
pub fn lookup(stop: &Arc<AtomicBool>) -> Option<Scope> {
    SCOPES.lock().get(&(Arc::as_ptr(stop) as usize)).cloned()
}
impl Drop for Registration {
    fn drop(&mut self) {
        SCOPES.lock().remove(&(Arc::as_ptr(&self.0) as usize));
    }
}
