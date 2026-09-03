//! `.gplugin` manifest parsing and compatibility checks.

use std::collections::BTreeMap;

use semver::VersionReq;
use serde::Deserialize;

use super::error::{ExtensionError, ExtensionResult};
use super::host_api::{HostApiDomain, HostApiRequirement};
use super::model::{validate_display_name, ExtensionId, ExtensionPath, ExtensionVersion};
use super::permissions::PermissionSet;

pub(crate) const MANIFEST_VERSION: u32 = 1;
pub(crate) const MANIFEST_FILE_NAME: &str = "manifest.toml";

/// Parsed, immutable metadata for one installed extension version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExtensionManifest {
    manifest_version: u32,
    id: ExtensionId,
    version: ExtensionVersion,
    name: String,
    description: Option<String>,
    entry: ExtensionPath,
    host_api: HostApiRequirements,
    permissions: PermissionSet,
    ui: Vec<UiContribution>,
}

impl ExtensionManifest {
    pub(crate) fn manifest_version(&self) -> u32 {
        self.manifest_version
    }

    pub(crate) fn id(&self) -> &ExtensionId {
        &self.id
    }

    pub(crate) fn version(&self) -> &ExtensionVersion {
        &self.version
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub(crate) fn entry(&self) -> &ExtensionPath {
        &self.entry
    }

    pub(crate) fn host_api(&self) -> &HostApiRequirements {
        &self.host_api
    }

    pub(crate) fn permissions(&self) -> &PermissionSet {
        &self.permissions
    }

    pub(crate) fn ui(&self) -> &[UiContribution] {
        &self.ui
    }
}

/// A panel contribution is declarative metadata. The actual iframe remains
/// served by the authenticated extension resource endpoint; it is never
/// mounted as a host Vue component.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UiContribution {
    panel_id: String,
    title: String,
    icon: Option<String>,
    order: i32,
    location: String,
    runtime: UiRuntime,
    requires_device: bool,
    preferred_width: Option<u16>,
    entry: Option<ExtensionPath>,
}

impl UiContribution {
    pub(crate) fn panel_id(&self) -> &str {
        &self.panel_id
    }

    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    pub(crate) fn icon(&self) -> Option<&str> {
        self.icon.as_deref()
    }

    pub(crate) fn order(&self) -> i32 {
        self.order
    }

    pub(crate) fn location(&self) -> &str {
        &self.location
    }

    pub(crate) fn runtime(&self) -> UiRuntime {
        self.runtime
    }

    pub(crate) fn requires_device(&self) -> bool {
        self.requires_device
    }

    pub(crate) fn preferred_width(&self) -> Option<u16> {
        self.preferred_width
    }

    pub(crate) fn entry(&self) -> Option<&ExtensionPath> {
        self.entry.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UiRuntime {
    Declarative,
    Iframe,
}

impl UiRuntime {
    fn parse(value: &str) -> ExtensionResult<Self> {
        match value.trim() {
            "declarative" => Ok(Self::Declarative),
            "iframe" => Ok(Self::Iframe),
            other => Err(ExtensionError::InvalidManifest(format!(
                "ui.contributions.runtime 不受支持: {other}"
            ))),
        }
    }
}

/// Domain-specific host requirements from the `[host_api]` table.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct HostApiRequirements(BTreeMap<HostApiDomain, HostApiRequirement>);

impl HostApiRequirements {
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&HostApiDomain, &HostApiRequirement)> + '_ {
        self.0.iter()
    }

    pub(crate) fn get(&self, domain: HostApiDomain) -> Option<&HostApiRequirement> {
        self.0.get(&domain)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    #[serde(alias = "format_version")]
    manifest_version: u32,
    id: String,
    version: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    entry: String,
    #[serde(default)]
    host_api: RawHostApiRequirements,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    ui: RawUi,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUi {
    #[serde(default)]
    contributions: Vec<RawUiContribution>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUiContribution {
    panel_id: String,
    title: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    order: i32,
    #[serde(default = "default_ui_location")]
    location: String,
    runtime: String,
    #[serde(default)]
    requires_device: bool,
    #[serde(default)]
    preferred_width: Option<u16>,
    #[serde(default)]
    entry: Option<String>,
}

fn default_ui_location() -> String {
    "console.right".to_string()
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHostApiRequirements {
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    vision: Option<String>,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    touch: Option<String>,
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    run: Option<String>,
    #[serde(default)]
    runtime: Option<String>,
    #[serde(default)]
    log: Option<String>,
}

impl RawHostApiRequirements {
    fn into_requirements(self) -> ExtensionResult<HostApiRequirements> {
        let values = [
            (HostApiDomain::Device, self.device),
            (HostApiDomain::Vision, self.vision),
            (HostApiDomain::Input, self.input),
            (HostApiDomain::Touch, self.touch),
            (HostApiDomain::Resource, self.resource),
            (HostApiDomain::Run, self.run),
            (HostApiDomain::Runtime, self.runtime),
            (HostApiDomain::Log, self.log),
        ];
        let mut requirements = BTreeMap::new();
        for (domain, raw) in values {
            let Some(raw) = raw else { continue };
            let requirement = VersionReq::parse(raw.trim()).map_err(|error| {
                ExtensionError::InvalidManifest(format!(
                    "host_api.{} 版本要求无效: {error}",
                    domain.as_str()
                ))
            })?;
            requirements.insert(domain, requirement);
        }
        Ok(HostApiRequirements(requirements))
    }
}

pub(crate) fn parse_manifest(bytes: &[u8]) -> ExtensionResult<ExtensionManifest> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| ExtensionError::InvalidManifest(format!("必须是 UTF-8: {error}")))?;
    let raw: RawManifest =
        toml::from_str(text).map_err(|error| ExtensionError::InvalidManifest(error.to_string()))?;
    if raw.manifest_version != MANIFEST_VERSION {
        return Err(ExtensionError::InvalidManifest(format!(
            "manifest_version={} 不受支持，当前仅支持 {}",
            raw.manifest_version, MANIFEST_VERSION
        )));
    }
    let id = ExtensionId::parse(&raw.id)?;
    let version = ExtensionVersion::parse(&raw.version)?;
    let name = validate_display_name(&raw.name)?;
    let description = match raw.description {
        Some(description) => {
            if description.chars().any(char::is_control) {
                return Err(ExtensionError::InvalidManifest(
                    "description 不能包含控制字符".to_string(),
                ));
            }
            let description = description.trim();
            (!description.is_empty()).then(|| description.to_string())
        }
        None => None,
    };
    let entry = ExtensionPath::parse(&raw.entry)?;
    if !entry.as_str().to_ascii_lowercase().ends_with(".wasm") {
        return Err(ExtensionError::InvalidManifest(
            "entry 必须指向 .wasm 文件".to_string(),
        ));
    }
    if entry.as_str() == MANIFEST_FILE_NAME {
        return Err(ExtensionError::InvalidManifest(
            "entry 不能指向 manifest.toml".to_string(),
        ));
    }
    let host_api = raw.host_api.into_requirements()?;
    let permissions = PermissionSet::parse(raw.permissions)?;
    let ui = raw
        .ui
        .contributions
        .into_iter()
        .map(parse_ui_contribution)
        .collect::<ExtensionResult<Vec<_>>>()?;

    Ok(ExtensionManifest {
        manifest_version: raw.manifest_version,
        id,
        version,
        name,
        description,
        entry,
        host_api,
        permissions,
        ui,
    })
}

fn parse_ui_contribution(raw: RawUiContribution) -> ExtensionResult<UiContribution> {
    let panel_id = ExtensionPath::parse(&raw.panel_id)
        .map_err(|_| ExtensionError::InvalidManifest("ui panel_id 无效".to_string()))?;
    let title = validate_display_name(&raw.title)?;
    let icon = match raw.icon {
        Some(icon) => {
            if icon.chars().any(char::is_control) {
                return Err(ExtensionError::InvalidManifest(
                    "ui icon 不能包含控制字符".to_string(),
                ));
            }
            let icon = icon.trim();
            (!icon.is_empty()).then(|| icon.to_string())
        }
        None => None,
    };
    if raw.location != "console.right" {
        return Err(ExtensionError::InvalidManifest(format!(
            "ui.contributions.location 不受支持: {}",
            raw.location
        )));
    }
    let runtime = UiRuntime::parse(&raw.runtime)?;
    let entry = match (runtime, raw.entry) {
        (UiRuntime::Declarative, Some(_)) => {
            return Err(ExtensionError::InvalidManifest(
                "declarative contribution 不能带 entry".to_string(),
            ));
        }
        (UiRuntime::Declarative, None) => None,
        (UiRuntime::Iframe, Some(entry)) => {
            let entry = ExtensionPath::parse(&entry)?;
            if !entry.as_str().starts_with("ui/") {
                return Err(ExtensionError::InvalidManifest(
                    "iframe contribution entry 必须位于 ui/ 下".to_string(),
                ));
            }
            Some(entry)
        }
        (UiRuntime::Iframe, None) => {
            return Err(ExtensionError::InvalidManifest(
                "iframe contribution 必须指定 entry".to_string(),
            ));
        }
    };
    if raw
        .preferred_width
        .is_some_and(|width| !(200..=800).contains(&width))
    {
        return Err(ExtensionError::InvalidManifest(
            "preferred_width 必须在 200..=800 之间".to_string(),
        ));
    }
    Ok(UiContribution {
        panel_id: panel_id.as_str().to_string(),
        title,
        icon,
        order: raw.order,
        location: raw.location,
        runtime,
        requires_device: raw.requires_device,
        preferred_width: raw.preferred_width,
        entry,
    })
}
