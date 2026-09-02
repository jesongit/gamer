use std::fmt;

use crate::core::fs::{is_windows_reserved_name, safe_name};
pub(crate) use crate::core::models::{AndroidPackageName, AppPackageId, ResourceId};

use super::error::{AppPackageError, AppPackageResult};

const MAX_ID_BYTES: usize = 128;
const MAX_VERSION_BYTES: usize = 128;
const MAX_ANDROID_PACKAGE_BYTES: usize = 255;
const MAX_RESOURCE_PATH_BYTES: usize = 1024;

/// Parse the canonical Core content-package identity with the stricter
/// filesystem policy required by the installer.
pub(crate) fn parse_app_package_id(value: &str) -> AppPackageResult<AppPackageId> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || safe_name(value).is_none()
        || !value.is_ascii()
    {
        return Err(AppPackageError::InvalidAppPackageId(value.to_string()));
    }
    AppPackageId::new(value.to_string())
        .map_err(|error| AppPackageError::InvalidAppPackageId(error.to_string()))
}

/// Parse the canonical Core Android package identity. It remains a distinct
/// type from [`AppPackageId`] and uses Android's dotted identifier grammar.
pub(crate) fn parse_android_package_name(value: &str) -> AppPackageResult<AndroidPackageName> {
    let value = value.trim();
    let valid = !value.is_empty()
        && value.len() <= MAX_ANDROID_PACKAGE_BYTES
        && value.is_ascii()
        && value.split('.').all(|part| {
            !part.is_empty()
                && part.as_bytes()[0].is_ascii_alphabetic()
                && !is_windows_reserved_name(part)
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        });
    if !valid {
        return Err(AppPackageError::InvalidAndroidPackage(value.to_string()));
    }
    AndroidPackageName::new(value.to_string())
        .map_err(|error| AppPackageError::InvalidAndroidPackage(error.to_string()))
}

/// Version directory name of an immutable installed package.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct InstalledVersion(String);

impl InstalledVersion {
    pub(crate) fn parse(value: &str) -> AppPackageResult<Self> {
        let value = value.trim();
        let valid = !value.is_empty()
            && value.len() <= MAX_VERSION_BYTES
            && value.is_ascii()
            && value.chars().any(|ch| ch.is_ascii_digit())
            && safe_name(value).is_some();
        if !valid {
            return Err(AppPackageError::InvalidInstalledVersion(value.to_string()));
        }
        Ok(Self(value.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InstalledVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum ResourceKind {
    Templates,
    Scripts,
    Keymaps,
    Presets,
    Resources,
}

impl ResourceKind {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "templates" => Some(Self::Templates),
            "scripts" => Some(Self::Scripts),
            "keymaps" => Some(Self::Keymaps),
            "presets" => Some(Self::Presets),
            "resources" => Some(Self::Resources),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Templates => "templates",
            Self::Scripts => "scripts",
            Self::Keymaps => "keymaps",
            Self::Presets => "presets",
            Self::Resources => "resources",
        }
    }
}

/// Package-relative resource path, for example `templates/status.png`.
/// Backslashes and parent/current-directory components are never accepted.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ResourcePath(String);

impl ResourcePath {
    pub(crate) fn parse(value: &str) -> AppPackageResult<Self> {
        let valid = !value.is_empty()
            && value.len() <= MAX_RESOURCE_PATH_BYTES
            && !value.starts_with('/')
            && !value.ends_with('/')
            && !value.contains('\\')
            && !value.contains(':')
            && !value
                .bytes()
                .any(|byte| byte == 0 || byte.is_ascii_control());
        if !valid {
            return Err(AppPackageError::InvalidResourcePath(value.to_string()));
        }

        let components: Vec<&str> = value.split('/').collect();
        if components.iter().any(|component| {
            component.is_empty()
                || *component == "."
                || *component == ".."
                || component.ends_with('.')
                || component.ends_with(' ')
                || is_windows_reserved_name(component)
        }) {
            return Err(AppPackageError::InvalidResourcePath(value.to_string()));
        }
        if ResourceKind::parse(components[0]).is_none() {
            return Err(AppPackageError::InvalidResourcePath(value.to_string()));
        }
        Ok(Self(value.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn kind(&self) -> ResourceKind {
        ResourceKind::parse(self.0.split('/').next().expect("validated path has root"))
            .expect("validated path has a resource root")
    }

    pub(crate) fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }
}

impl fmt::Display for ResourcePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Build a Core resource id whose revision is the immutable installed version.
pub(crate) fn resource_id(
    app_package: AppPackageId,
    version: &InstalledVersion,
    logical_path: &ResourcePath,
) -> AppPackageResult<ResourceId> {
    ResourceId::with_revision(
        app_package,
        Some(version.as_str().to_string()),
        logical_path.as_str().to_string(),
    )
    .map_err(|error| AppPackageError::InvalidResourcePath(error.to_string()))
}
