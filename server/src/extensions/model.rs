//! Stable identifiers and lifecycle values for installed extensions.

use std::fmt;
use std::hash::{Hash, Hasher};

use semver::Version;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::core::fs::safe_name;

use super::error::{ExtensionError, ExtensionResult};

const MAX_EXTENSION_ID_BYTES: usize = 128;
const MAX_EXTENSION_VERSION_BYTES: usize = 128;
const MAX_EXTENSION_PATH_BYTES: usize = 1024;
const MAX_DISPLAY_NAME_BYTES: usize = 256;

/// Stable logical identity of a `.gplugin` package. It is never a host path.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ExtensionId(String);

impl ExtensionId {
    pub(crate) fn parse(value: &str) -> ExtensionResult<Self> {
        let value = value.trim();
        if value.is_empty()
            || value.len() > MAX_EXTENSION_ID_BYTES
            || !value.is_ascii()
            || safe_name(value).is_none()
        {
            return Err(ExtensionError::InvalidId(value.to_string()));
        }
        Ok(Self(value.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ExtensionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ExtensionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ExtensionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// SemVer version used as the immutable extension directory name.
///
/// Build metadata is intentionally rejected because `+` is not accepted by
/// the portable Core filesystem name policy. Pre-release versions remain
/// valid and sort according to SemVer.
#[derive(Clone, Debug)]
pub(crate) struct ExtensionVersion {
    semver: Version,
    raw: String,
}

impl ExtensionVersion {
    pub(crate) fn parse(value: &str) -> ExtensionResult<Self> {
        let value = value.trim();
        if value.is_empty()
            || value.len() > MAX_EXTENSION_VERSION_BYTES
            || value.contains('+')
            || safe_name(value).is_none()
        {
            return Err(ExtensionError::InvalidVersion(value.to_string()));
        }
        let version = Version::parse(value)
            .map_err(|error| ExtensionError::InvalidVersion(format!("{value}: {error}")))?;
        Ok(Self {
            semver: version,
            raw: value.to_string(),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.raw
    }

    pub(crate) fn semver(&self) -> &Version {
        &self.semver
    }
}

impl PartialEq for ExtensionVersion {
    fn eq(&self, other: &Self) -> bool {
        self.semver == other.semver
    }
}

impl Eq for ExtensionVersion {}

impl Hash for ExtensionVersion {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.semver.hash(state);
    }
}

impl PartialOrd for ExtensionVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ExtensionVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.semver.cmp(&other.semver)
    }
}

impl fmt::Display for ExtensionVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ExtensionVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ExtensionVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// A validated package-relative path. It is used for the WASM entrypoint and
/// for extracted archive entries; it can never escape the version directory.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ExtensionPath(String);

impl ExtensionPath {
    pub(crate) fn parse(value: &str) -> ExtensionResult<Self> {
        let value = value.trim();
        if value.is_empty()
            || value.len() > MAX_EXTENSION_PATH_BYTES
            || value.starts_with('/')
            || value.ends_with('/')
            || value.contains('\\')
            || value.contains(':')
            || value
                .bytes()
                .any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err(ExtensionError::InvalidPath(value.to_string()));
        }
        let components: Vec<&str> = value.split('/').collect();
        if components.iter().any(|component| {
            component.is_empty()
                || *component == "."
                || *component == ".."
                || safe_name(component).is_none()
        }) {
            return Err(ExtensionError::InvalidPath(value.to_string()));
        }
        Ok(Self(value.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }
}

impl fmt::Display for ExtensionPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub(crate) fn validate_display_name(value: &str) -> ExtensionResult<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_DISPLAY_NAME_BYTES {
        return Err(ExtensionError::InvalidManifest(
            "name 必须是非空且不超过 256 字节的文本".to_string(),
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(ExtensionError::InvalidManifest(
            "name 不能包含控制字符".to_string(),
        ));
    }
    Ok(value.to_string())
}

/// Persistent lifecycle state. Installed is deliberately distinct from
/// Enabled: installing bytes never grants them execution permission.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExtensionState {
    Installed,
    Enabled,
    Running,
    Disabled,
    Failed,
}

impl ExtensionState {
    pub(crate) fn can_start(self) -> bool {
        self == Self::Enabled
    }

    pub(crate) fn is_running(self) -> bool {
        self == Self::Running
    }
}

/// Mutable metadata for one logical extension. Version directories are not
/// represented here so installing a new version never mutates old bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ExtensionRecord {
    pub(crate) id: ExtensionId,
    pub(crate) active_version: Option<ExtensionVersion>,
    pub(crate) state: ExtensionState,
    #[serde(default)]
    pub(crate) last_error: Option<String>,
}

impl ExtensionRecord {
    pub(crate) fn new(id: ExtensionId, active_version: ExtensionVersion) -> Self {
        Self {
            id,
            active_version: Some(active_version),
            state: ExtensionState::Installed,
            last_error: None,
        }
    }
}
