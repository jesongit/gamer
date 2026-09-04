//! In-process UI contribution registry.
//!
//! Contributions are derived from the active manifest and are rebuilt after
//! every lifecycle mutation. This makes disable/uninstall cleanup idempotent
//! and prevents stale panels from surviving a version switch.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use serde::Serialize;

use super::manifest::{ExtensionManifest, UiField, UiRuntime};
use super::model::{ExtensionId, ExtensionVersion};

/// Declarative 表单 schema 的注册快照：字段随 `GET /api/extensions` 的
/// `ui_contributions` 原样透传给前端，由 PluginPanelHost 原生渲染。
#[derive(Clone, Debug, Serialize)]
pub(crate) struct RegisteredUiSchema {
    pub(crate) description: Option<String>,
    pub(crate) fields: Vec<UiField>,
}

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
    /// `runtime = "core"` 贡献的宿主组件键；由前端 core-component-registry 解释。
    pub(crate) component: Option<String>,
    pub(crate) schema: Option<RegisteredUiSchema>,
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
                    component: contribution.component().map(ToOwned::to_owned),
                    schema: contribution.schema().map(|schema| RegisteredUiSchema {
                        description: schema.description().map(ToOwned::to_owned),
                        fields: schema.fields().to_vec(),
                    }),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::parse_manifest;

    fn manifest_with_declarative_ui() -> ExtensionManifest {
        let manifest = "manifest_version = 1\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"Test extension\"\nentry = \"plugin.wasm\"\n\
             [ui]\n[[ui.contributions]]\npanel_id = \"settings\"\ntitle = \"设置\"\nruntime = \"declarative\"\ndescription = \"说明\"\n\
             [[ui.contributions.fields]]\ntype = \"boolean\"\nname = \"enabled\"\nlabel = \"启用\"\ndefault = true\n".to_string();
        parse_manifest(manifest.as_bytes()).unwrap()
    }

    #[test]
    fn registry_passes_declarative_schema_through_to_json() {
        let registry = UiContributionRegistry::default();
        let manifest = manifest_with_declarative_ui();
        registry.register(&manifest);
        let contributions = registry.list();
        assert_eq!(contributions.len(), 1);
        let schema = contributions[0].schema.as_ref().unwrap();
        assert_eq!(schema.description.as_deref(), Some("说明"));
        assert_eq!(schema.fields.len(), 1);
        assert_eq!(schema.fields[0].name(), Some("enabled"));

        let json = serde_json::to_value(&contributions[0]).unwrap();
        assert_eq!(json["runtime"], "declarative");
        assert_eq!(json["schema"]["fields"][0]["type"], "boolean");
        assert_eq!(json["schema"]["fields"][0]["default"], true);
        assert!(json.get("entry").unwrap().is_null());

        registry.clear();
        assert!(registry.list().is_empty());
    }

    #[test]
    fn iframe_contributions_keep_schema_absent() {
        let registry = UiContributionRegistry::default();
        let manifest = parse_manifest(
            b"manifest_version = 1\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"Test extension\"\nentry = \"plugin.wasm\"\n\
              [ui]\n[[ui.contributions]]\npanel_id = \"panel\"\ntitle = \"P\"\nruntime = \"iframe\"\nentry = \"ui/index.html\"\n",
        )
        .unwrap();
        registry.register(&manifest);
        assert!(registry.list()[0].schema.is_none());
    }

    #[test]
    fn core_contributions_pass_component_through_to_json() {
        let manifest_text = "manifest_version = 1\nid = \"gamer.yaml\"\nversion = \"3.0.0\"\nname = \"Gamer YAML vNext\"\nentry = \"plugin.wasm\"\n\
              [ui]\n[[ui.contributions]]\npanel_id = \"automation\"\ntitle = \"自动化\"\nruntime = \"core\"\ncomponent = \"console.scripts\"\nrequires_device = true\n";
        let registry = UiContributionRegistry::default();
        let manifest = parse_manifest(manifest_text.as_bytes()).unwrap();
        registry.register(&manifest);
        let contributions = registry.list();
        assert_eq!(contributions.len(), 1);
        assert_eq!(
            contributions[0].component.as_deref(),
            Some("console.scripts")
        );
        assert!(contributions[0].entry.is_none());
        assert!(contributions[0].schema.is_none());

        // 前端契约：runtime = "core" + component 原样透传，entry 缺省为 null。
        let json = serde_json::to_value(&contributions[0]).unwrap();
        assert_eq!(json["runtime"], "core");
        assert_eq!(json["component"], "console.scripts");
        assert!(json.get("entry").unwrap().is_null());

        // 卸载/停用后注册表清空：非 Enabled|Running 不发布由 service 层保证。
        registry.clear();
        assert!(registry.list().is_empty());
    }
}
