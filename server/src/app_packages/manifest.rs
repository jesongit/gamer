use std::collections::HashSet;

use serde::Deserialize;

use super::error::{AppPackageError, AppPackageResult};
use super::model::{
    parse_android_package_name, parse_app_package_id, AndroidPackageName, AppPackageId,
    InstalledVersion,
};

#[derive(Clone, Debug)]
pub(crate) struct PackageManifest {
    id: AppPackageId,
    version: InstalledVersion,
    name: Option<String>,
    revision: Option<String>,
    android_packages: Vec<AndroidPackageName>,
}

impl PackageManifest {
    pub(crate) fn id(&self) -> &AppPackageId {
        &self.id
    }

    pub(crate) fn version(&self) -> &InstalledVersion {
        &self.version
    }

    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }

    pub(crate) fn android_packages(&self) -> &[AndroidPackageName] {
        &self.android_packages
    }

    pub(crate) fn supports_android_package(&self, package: &AndroidPackageName) -> bool {
        self.android_packages
            .iter()
            .any(|candidate| candidate == package)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPackageManifest {
    id: String,
    version: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    revision: Option<String>,
    android: RawAndroidManifest,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAndroidManifest {
    packages: Vec<String>,
}

pub(crate) fn parse_manifest(bytes: &[u8]) -> AppPackageResult<PackageManifest> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| AppPackageError::InvalidManifest(format!("必须是 UTF-8: {error}")))?;
    let raw: RawPackageManifest = toml::from_str(text)
        .map_err(|error| AppPackageError::InvalidManifest(error.to_string()))?;
    if raw
        .name
        .as_deref()
        .is_some_and(|name| name.trim().is_empty())
    {
        return Err(AppPackageError::InvalidManifest(
            "name 不能为空".to_string(),
        ));
    }

    let id = parse_app_package_id(&raw.id)?;
    let version = InstalledVersion::parse(&raw.version)?;
    if let Some(revision) = raw.revision.as_deref() {
        if revision.trim().is_empty()
            || revision.len() > 128
            || !revision.is_ascii()
            || revision
                .chars()
                .any(|character| !(character.is_ascii_alphanumeric() || ".-_".contains(character)))
        {
            return Err(AppPackageError::InvalidManifest(
                "revision 必须是非空 ASCII 版本标识".to_string(),
            ));
        }
    }

    if raw.android.packages.is_empty() {
        return Err(AppPackageError::InvalidManifest(
            "android.packages 不能为空".to_string(),
        ));
    }
    let mut seen = HashSet::new();
    let mut android_packages = Vec::with_capacity(raw.android.packages.len());
    for package in raw.android.packages {
        let parsed = parse_android_package_name(&package)?;
        if !seen.insert(parsed.clone()) {
            return Err(AppPackageError::InvalidManifest(format!(
                "android.packages 存在重复项: {package}"
            )));
        }
        android_packages.push(parsed);
    }

    Ok(PackageManifest {
        id,
        version,
        name: raw.name,
        revision: raw.revision,
        android_packages,
    })
}
