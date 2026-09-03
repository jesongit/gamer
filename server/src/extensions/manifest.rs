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

    Ok(ExtensionManifest {
        manifest_version: raw.manifest_version,
        id,
        version,
        name,
        description,
        entry,
        host_api,
        permissions,
    })
}
