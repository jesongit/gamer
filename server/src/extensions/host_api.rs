//! Versioned Host API catalog and the capability-aware authorization facade.

use std::collections::BTreeMap;

use semver::{Version, VersionReq};

use crate::capabilities::CapabilityRegistry;

use super::error::{ExtensionError, ExtensionResult, PermissionError};
use super::manifest::ExtensionManifest;
use super::permissions::{Permission, PermissionSet};

pub(crate) const HOST_API_VERSION: &str = "1.0.0";

/// WIT and Rust use the same eight independently versioned host domains.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum HostApiDomain {
    Device,
    Vision,
    Input,
    Touch,
    Resource,
    Run,
    Runtime,
    Log,
}

impl HostApiDomain {
    pub(crate) const ALL: [Self; 8] = [
        Self::Device,
        Self::Vision,
        Self::Input,
        Self::Touch,
        Self::Resource,
        Self::Run,
        Self::Runtime,
        Self::Log,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::Vision => "vision",
            Self::Input => "input",
            Self::Touch => "touch",
            Self::Resource => "resource",
            Self::Run => "run",
            Self::Runtime => "runtime",
            Self::Log => "log",
        }
    }

    pub(crate) fn all() -> &'static [Self; 8] {
        &Self::ALL
    }
}

impl std::fmt::Display for HostApiDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Host-side supported API versions. A domain can evolve independently while
/// preserving compatibility checks in the manifest boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostApiCatalog {
    versions: BTreeMap<HostApiDomain, Version>,
}

impl Default for HostApiCatalog {
    fn default() -> Self {
        let version = Version::parse(HOST_API_VERSION).expect("host API version is valid");
        Self {
            versions: HostApiDomain::ALL
                .into_iter()
                .map(|domain| (domain, version.clone()))
                .collect(),
        }
    }
}

impl HostApiCatalog {
    pub(crate) fn version(&self, domain: HostApiDomain) -> Option<&Version> {
        self.versions.get(&domain)
    }

    pub(crate) fn validate(&self, manifest: &ExtensionManifest) -> ExtensionResult<()> {
        for (domain, requirement) in manifest.host_api().iter() {
            let Some(supported) = self.version(*domain) else {
                return Err(ExtensionError::UnsupportedHostApi {
                    id: manifest.id().to_string(),
                    domain: domain.to_string(),
                    required: requirement.to_string(),
                    supported: "unavailable".to_string(),
                });
            };
            if !requirement.matches(supported) {
                return Err(ExtensionError::UnsupportedHostApi {
                    id: manifest.id().to_string(),
                    domain: domain.to_string(),
                    required: requirement.to_string(),
                    supported: supported.to_string(),
                });
            }
        }
        Ok(())
    }
}

/// A per-extension Host API view. The registry is only an adapter source;
/// callers still need an explicit permission for every operation.
#[derive(Clone)]
pub(crate) struct HostApi {
    registry: CapabilityRegistry,
    catalog: HostApiCatalog,
    permissions: PermissionSet,
}

impl HostApi {
    pub(crate) fn for_manifest(
        registry: CapabilityRegistry,
        catalog: HostApiCatalog,
        manifest: &ExtensionManifest,
    ) -> ExtensionResult<Self> {
        catalog.validate(manifest)?;
        Ok(Self {
            registry,
            catalog,
            permissions: manifest.permissions().clone(),
        })
    }

    pub(crate) fn authorize(&self, permission: Permission) -> ExtensionResult<()> {
        if self.permissions.allows(permission) {
            Ok(())
        } else {
            Err(ExtensionError::Permission(PermissionError::NotGranted(
                permission.as_str().to_string(),
            )))
        }
    }

    pub(crate) fn api_version(&self, domain: HostApiDomain) -> Option<&Version> {
        self.catalog.version(domain)
    }

    /// Reports whether an adapter for a domain is registered. Registration is
    /// separate from authorization so a missing backend cannot become an
    /// accidental permission escalation.
    pub(crate) fn domain_available(&self, domain: HostApiDomain) -> bool {
        match domain {
            HostApiDomain::Device => self.registry.device().is_some(),
            HostApiDomain::Vision => self.registry.vision().is_some(),
            HostApiDomain::Input => self.registry.input().is_some(),
            HostApiDomain::Touch => self.registry.touch().is_some(),
            HostApiDomain::Resource => self.registry.resource().is_some(),
            HostApiDomain::Run => self.registry.run().is_some(),
            HostApiDomain::Runtime => self.registry.runtime().is_some(),
            HostApiDomain::Log => self.registry.log().is_some(),
        }
    }

    pub(crate) fn registry(&self) -> &CapabilityRegistry {
        &self.registry
    }
}

/// Keep the public requirement type behind the manifest API while still
/// allowing focused tests and future generated WIT adapters to inspect it.
pub(crate) type HostApiRequirement = VersionReq;
