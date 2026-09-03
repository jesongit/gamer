//! Device-consumer activity primitives.
//!
//! Core owns the meaning of a consumer lease, while concrete subsystems decide
//! when to acquire one.  This keeps device power management independent from
//! the kind of consumer (viewer, run, capture, or extension).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// The small set of consumer categories currently known by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActivityKind {
    Viewer,
    Run,
    Capture,
    Extension,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ActivityCounts {
    viewer: usize,
    run: usize,
    capture: usize,
    extension: usize,
}

impl ActivityCounts {
    fn increment(&mut self, kind: ActivityKind) {
        *self.count_mut(kind) += 1;
    }

    fn decrement(&mut self, kind: ActivityKind) {
        let count = self.count_mut(kind);
        *count = count.saturating_sub(1);
    }

    fn count(&self, kind: ActivityKind) -> usize {
        match kind {
            ActivityKind::Viewer => self.viewer,
            ActivityKind::Run => self.run,
            ActivityKind::Capture => self.capture,
            ActivityKind::Extension => self.extension,
        }
    }

    fn total(self) -> usize {
        self.viewer + self.run + self.capture + self.extension
    }

    fn count_mut(&mut self, kind: ActivityKind) -> &mut usize {
        match kind {
            ActivityKind::Viewer => &mut self.viewer,
            ActivityKind::Run => &mut self.run,
            ActivityKind::Capture => &mut self.capture,
            ActivityKind::Extension => &mut self.extension,
        }
    }
}

/// Process-local registry of active device consumers.
#[derive(Debug, Default)]
pub struct DeviceActivity {
    counts: Mutex<HashMap<String, ActivityCounts>>,
}

impl DeviceActivity {
    /// Acquire one activity lease.  The lease is intentionally independent of
    /// any device implementation and releases itself on drop.
    pub fn acquire(
        self: &Arc<Self>,
        device_id: impl Into<String>,
        kind: ActivityKind,
    ) -> DeviceLease {
        let device_id = device_id.into();
        self.counts
            .lock()
            .unwrap()
            .entry(device_id.clone())
            .or_default()
            .increment(kind);
        DeviceLease {
            activity: Arc::downgrade(self),
            device_id,
            kind,
            released: AtomicBool::new(false),
        }
    }

    /// Whether any consumer currently keeps a device active.
    pub fn has_active(&self, device_id: &str) -> bool {
        self.counts
            .lock()
            .unwrap()
            .get(device_id)
            .is_some_and(|counts| counts.total() > 0)
    }

    /// Whether a specific consumer category is active.  This is useful for
    /// policy decisions such as distinguishing a run from a passive viewer;
    /// power management itself should use [`Self::has_active`].
    pub fn has_kind(&self, device_id: &str, kind: ActivityKind) -> bool {
        self.counts
            .lock()
            .unwrap()
            .get(device_id)
            .is_some_and(|counts| counts.count(kind) > 0)
    }

    pub fn active_count(&self, device_id: &str) -> usize {
        self.counts
            .lock()
            .unwrap()
            .get(device_id)
            .copied()
            .map(ActivityCounts::total)
            .unwrap_or_default()
    }

    fn release(&self, device_id: &str, kind: ActivityKind) {
        let mut counts = self.counts.lock().unwrap();
        let Some(current) = counts.get_mut(device_id) else {
            return;
        };
        current.decrement(kind);
        if current.total() == 0 {
            counts.remove(device_id);
        }
    }
}

/// RAII lease for one device consumer.
#[derive(Debug)]
pub struct DeviceLease {
    activity: Weak<DeviceActivity>,
    device_id: String,
    kind: ActivityKind,
    released: AtomicBool,
}

/// Semantic aliases keep call sites self-documenting while all lease
/// implementations continue to share the same device activity registry.
pub type ViewerLease = DeviceLease;
pub type RunLease = DeviceLease;
pub type CaptureLease = DeviceLease;
pub type ExtensionLease = DeviceLease;

impl DeviceLease {
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn kind(&self) -> ActivityKind {
        self.kind
    }

    /// Explicitly release before drop when a caller needs a precise boundary.
    pub fn release(&self) {
        if self.released.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(activity) = self.activity.upgrade() {
            activity.release(&self.device_id, self.kind);
        }
    }
}

impl Drop for DeviceLease {
    fn drop(&mut self) {
        self.release();
    }
}

/// Generic lease returned by runner implementations.  RunManager owns it but
/// does not know which device subsystem created it.
pub trait ActivityLease: Send + Sync + 'static {}

impl ActivityLease for DeviceLease {}

/// A no-op lease for tests and adapters that do not own a device resource.
#[derive(Debug, Default)]
pub struct NoopLease;

impl ActivityLease for NoopLease {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leases_count_by_kind_and_release_on_drop() {
        let activity = Arc::new(DeviceActivity::default());
        let run = activity.acquire("d1", ActivityKind::Run);
        let viewer = activity.acquire("d1", ActivityKind::Viewer);

        assert!(activity.has_active("d1"));
        assert!(activity.has_kind("d1", ActivityKind::Run));
        assert_eq!(activity.active_count("d1"), 2);

        drop(run);
        assert!(activity.has_active("d1"));
        assert!(!activity.has_kind("d1", ActivityKind::Run));
        assert_eq!(activity.active_count("d1"), 1);

        drop(viewer);
        assert!(!activity.has_active("d1"));
        assert_eq!(activity.active_count("d1"), 0);
    }

    #[test]
    fn explicit_release_is_idempotent() {
        let activity = Arc::new(DeviceActivity::default());
        let lease = activity.acquire("d1", ActivityKind::Capture);
        lease.release();
        lease.release();
        assert!(!activity.has_active("d1"));
    }
}
