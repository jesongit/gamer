//! In-process UI contribution registry.
//!
//! Contributions are derived from the active manifest and are rebuilt after
//! every lifecycle mutation. This makes disable/uninstall cleanup idempotent
//! and prevents stale panels from surviving a version switch.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use serde::Serialize;

use super::manifest::{ExtensionManifest, UiRuntime};
use super::model::{ExtensionId, ExtensionVersion};

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RegisteredUiContribution {
    pub(crate) plugin_id: ExtensionId,
    pub(crate) version: ExtensionVersion,
    pub(crate) panel_id: String,
    pub(crate) title: String,
    pub(crate) icon: Option<String>,
    pub(crate) order: i32,
    pub(crate) location: String,
    pub(crate) runtime: UiRuntime,
    pub(crate) requires_device: bool,
    pub(crate) preferred_width: Option<u16>,
    pub(crate) entry: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct UiContributionRegistry {
    entries: Arc<RwLock<BTreeMap<(ExtensionId, String), RegisteredUiContribution>>>,
}

impl UiContributionRegistry {
    pub(crate) fn clear(&self) {
        self.entries
            .write()
            .expect("extension UI registry poisoned")
            .clear();
    }

    pub(crate) fn register(&self, manifest: &ExtensionManifest) {
        let mut entries = self
            .entries
            .write()
            .expect("extension UI registry poisoned");
        for contribution in manifest.ui() {
            entries.insert(
                (manifest.id().clone(), contribution.panel_id().to_string()),
                RegisteredUiContribution {
                    plugin_id: manifest.id().clone(),
                    version: manifest.version().clone(),
                    panel_id: contribution.panel_id().to_string(),
                    title: contribution.title().to_string(),
                    icon: contribution.icon().map(ToOwned::to_owned),
                    order: contribution.order(),
                    location: contribution.location().to_string(),
                    runtime: contribution.runtime(),
                    requires_device: contribution.requires_device(),
                    preferred_width: contribution.preferred_width(),
                    entry: contribution.entry().map(|entry| entry.as_str().to_string()),
                },
            );
        }
    }

    pub(crate) fn list(&self) -> Vec<RegisteredUiContribution> {
        self.entries
            .read()
            .expect("extension UI registry poisoned")
            .values()
            .cloned()
            .collect()
    }
}
