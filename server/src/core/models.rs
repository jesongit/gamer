//! Core runtime models shared by runners and boundary adapters.
//!
//! This module intentionally contains data-only types. It does not resolve
//! resources, access devices, or know about YAML, SQLite, WebRTC, or a host
//! filesystem. Existing execution paths can adopt these types incrementally
//! through the legacy constructors below.

#![allow(
    dead_code,
    reason = "Phase 2 model skeleton is adopted incrementally by existing adapters"
)]

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Validation failures produced while constructing a core boundary value.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelError {
    #[error("{kind} must not be empty")]
    EmptyValue { kind: &'static str },
    #[error("{kind} contains a control character")]
    ControlCharacter { kind: &'static str },
    #[error("{kind} must not contain a path separator")]
    PathSeparator { kind: &'static str },
    #[error("resource logical path is invalid: {reason}")]
    InvalidLogicalPath { reason: &'static str },
    #[error("run request device_id does not match app.device_id")]
    DeviceMismatch { request: DeviceId, app: DeviceId },
    #[error("{field} must not be empty")]
    EmptyRequestField { field: &'static str },
    #[error("{field} contains a control character")]
    ControlCharacterRequestField { field: &'static str },
}

macro_rules! string_id {
    ($name:ident, $kind:literal, $doc:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
        #[serde(transparent)]
        #[doc = $doc]
        pub struct $name(String);

        impl $name {
            /// Construct an identifier after trimming surrounding whitespace.
            pub fn new(value: impl Into<String>) -> Result<Self, ModelError> {
                Ok(Self(validate_identifier($kind, &value.into())?))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl TryFrom<&str> for $name {
            type Error = ModelError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = ModelError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.into_string()
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

fn validate_identifier(kind: &'static str, value: &str) -> Result<String, ModelError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ModelError::EmptyValue { kind });
    }
    if value.chars().any(char::is_control) {
        return Err(ModelError::ControlCharacter { kind });
    }
    if value.contains('/') || value.contains('\\') {
        return Err(ModelError::PathSeparator { kind });
    }
    Ok(value.to_owned())
}

string_id!(
    DeviceId,
    "device_id",
    "The stable identity of a connected device."
);

string_id!(
    AndroidPackageName,
    "android_package",
    "The package installed on Android and used by app lifecycle operations. This is deliberately different from [`AppPackageId`]."
);

string_id!(
    AppPackageId,
    "content_package",
    "The logical content/resource package selected for a run."
);

/// Naming aliases for adapters that use `Id` terminology. The aliases retain
/// the distinct underlying Android and content package types.
pub type AndroidPackageId = AndroidPackageName;
pub type ContentPackageId = AppPackageId;

/// A device plus the Android application and optional content package in
/// scope for an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppContext {
    pub device_id: DeviceId,
    pub android_package: AndroidPackageName,
    pub content_package: Option<AppPackageId>,
}

impl AppContext {
    pub fn new(
        device_id: DeviceId,
        android_package: AndroidPackageName,
        content_package: Option<AppPackageId>,
    ) -> Self {
        Self {
            device_id,
            android_package,
            content_package,
        }
    }

    /// Compatibility adapter for the current `<pkg>/...` storage convention.
    /// The old value was ambiguous, so this copies it into both typed
    /// namespaces. New callers should use [`Self::new`].
    pub fn from_legacy_package(
        device_id: impl Into<String>,
        package: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let package = package.into();
        let device_id = DeviceId::new(device_id)?;
        let android_package = AndroidPackageName::new(package.clone())?;
        let content_package = Some(AppPackageId::new(package)?);
        Ok(Self::new(device_id, android_package, content_package))
    }
}

string_id!(
    RunId,
    "run_id",
    "The stable identity assigned to an execution instance."
);

impl RunId {
    /// Generate the same UUID-shaped identity used by the current run manager.
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

/// Runtime context passed to a runner after a request has been accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunContext {
    pub run_id: RunId,
    pub app: AppContext,
}

impl RunContext {
    pub fn new(run_id: RunId, app: AppContext) -> Self {
        Self { run_id, app }
    }

    pub fn device_id(&self) -> &DeviceId {
        &self.app.device_id
    }
}

/// Runner-specific input. The transparent JSON representation keeps the
/// boundary generic while allowing the existing YAML runner to pass its
/// object-shaped argument map unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunPayload(serde_json::Value);

impl RunPayload {
    pub fn new(value: serde_json::Value) -> Self {
        Self(value)
    }

    pub fn empty() -> Self {
        Self(serde_json::Value::Object(serde_json::Map::new()))
    }

    pub fn as_value(&self) -> &serde_json::Value {
        &self.0
    }

    pub fn into_value(self) -> serde_json::Value {
        self.0
    }
}

impl Default for RunPayload {
    fn default() -> Self {
        Self::empty()
    }
}

/// Generic request accepted by any runner implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunRequest {
    pub device_id: DeviceId,
    pub app: AppContext,
    pub runner_id: String,
    pub entrypoint: String,
    pub payload: RunPayload,
}

impl<'de> Deserialize<'de> for RunRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RunRequestFields {
            device_id: DeviceId,
            app: AppContext,
            runner_id: String,
            entrypoint: String,
            payload: RunPayload,
        }

        let fields = RunRequestFields::deserialize(deserializer)?;
        Self::new(
            fields.device_id,
            fields.app,
            fields.runner_id,
            fields.entrypoint,
            fields.payload,
        )
        .map_err(serde::de::Error::custom)
    }
}

impl RunRequest {
    pub fn new(
        device_id: DeviceId,
        app: AppContext,
        runner_id: impl Into<String>,
        entrypoint: impl Into<String>,
        payload: RunPayload,
    ) -> Result<Self, ModelError> {
        let request = Self {
            device_id,
            app,
            runner_id: runner_id.into(),
            entrypoint: entrypoint.into(),
            payload,
        };
        request.validate()?;
        Ok(request)
    }

    /// Construct a request without repeating `app.device_id` at the callsite.
    pub fn for_app(
        app: AppContext,
        runner_id: impl Into<String>,
        entrypoint: impl Into<String>,
        payload: RunPayload,
    ) -> Result<Self, ModelError> {
        Self::new(app.device_id.clone(), app, runner_id, entrypoint, payload)
    }

    pub fn validate(&self) -> Result<(), ModelError> {
        if self.device_id != self.app.device_id {
            return Err(ModelError::DeviceMismatch {
                request: self.device_id.clone(),
                app: self.app.device_id.clone(),
            });
        }
        validate_request_field("runner_id", &self.runner_id)?;
        validate_request_field("entrypoint", &self.entrypoint)
    }
}

fn validate_request_field(field: &'static str, value: &str) -> Result<(), ModelError> {
    if value.trim().is_empty() {
        return Err(ModelError::EmptyRequestField { field });
    }
    if value.chars().any(char::is_control) {
        return Err(ModelError::ControlCharacterRequestField { field });
    }
    Ok(())
}

/// Logical resource identity. This is deliberately not a `PathBuf`: the
/// resolver that comes in a later phase owns host-path mapping.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ResourceId {
    app_package: AppPackageId,
    logical_path: String,
    revision: Option<String>,
}

impl<'de> Deserialize<'de> for ResourceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct ResourceIdFields {
            app_package: AppPackageId,
            logical_path: String,
            revision: Option<String>,
        }

        let fields = ResourceIdFields::deserialize(deserializer)?;
        Self::with_revision(fields.app_package, fields.revision, fields.logical_path)
            .map_err(serde::de::Error::custom)
    }
}

impl ResourceId {
    pub fn new(
        app_package: AppPackageId,
        logical_path: impl Into<String>,
    ) -> Result<Self, ModelError> {
        Self::with_revision(app_package, None::<String>, logical_path)
    }

    pub fn with_revision(
        app_package: AppPackageId,
        revision: Option<String>,
        logical_path: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let logical_path = validate_logical_path(&logical_path.into())?;
        let revision = revision
            .map(|value| validate_identifier("resource revision", &value))
            .transpose()?;
        Ok(Self {
            app_package,
            logical_path,
            revision,
        })
    }

    /// Convert the current `<content-package>/<relative-resource>` key into
    /// a logical id without constructing a host filesystem path.
    pub fn from_legacy_key(key: &str) -> Result<Self, ModelError> {
        let (package, logical_path) =
            key.split_once('/').ok_or(ModelError::InvalidLogicalPath {
                reason: "legacy key must contain a package and resource path",
            })?;
        Self::new(AppPackageId::new(package)?, logical_path)
    }

    pub fn app_package(&self) -> &AppPackageId {
        &self.app_package
    }

    pub fn logical_path(&self) -> &str {
        &self.logical_path
    }

    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }

    /// Legacy display/storage key, retained only for old adapters.
    pub fn legacy_key(&self) -> String {
        format!("{}/{}", self.app_package, self.logical_path)
    }
}

impl fmt::Display for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.revision() {
            Some(revision) => write!(f, "{}@{revision}/{}", self.app_package, self.logical_path),
            None => f.write_str(&self.legacy_key()),
        }
    }
}

fn validate_logical_path(path: &str) -> Result<String, ModelError> {
    if path.is_empty() {
        return Err(ModelError::InvalidLogicalPath {
            reason: "path must not be empty",
        });
    }
    if path.starts_with('/') || path.ends_with('/') {
        return Err(ModelError::InvalidLogicalPath {
            reason: "path must be relative",
        });
    }
    if path.contains('\\') {
        return Err(ModelError::InvalidLogicalPath {
            reason: "path must use '/' separators",
        });
    }
    if path
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(ModelError::InvalidLogicalPath {
            reason: "path contains an empty or traversal segment",
        });
    }
    if path.chars().any(char::is_control) {
        return Err(ModelError::InvalidLogicalPath {
            reason: "path contains a control character",
        });
    }
    Ok(path.to_owned())
}

/// A resolver-owned capability to a logical resource. It intentionally
/// exposes the resource identity but no host path or file handle.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceHandle(ResourceId);

impl ResourceHandle {
    pub fn new(id: ResourceId) -> Self {
        Self(id)
    }

    pub fn id(&self) -> &ResourceId {
        &self.0
    }

    pub fn into_id(self) -> ResourceId {
        self.0
    }
}

impl From<ResourceId> for ResourceHandle {
    fn from(id: ResourceId) -> Self {
        Self::new(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> AppContext {
        AppContext::new(
            DeviceId::new("device-1").unwrap(),
            AndroidPackageName::new("com.example.game").unwrap(),
            Some(AppPackageId::new("official.example").unwrap()),
        )
    }

    #[test]
    fn app_context_keeps_device_android_and_content_identities_typed() {
        let context = app();

        assert_eq!(context.device_id.as_str(), "device-1");
        assert_eq!(context.android_package.as_str(), "com.example.game");
        assert_eq!(
            context.content_package.as_ref().unwrap().as_str(),
            "official.example"
        );
        assert_ne!(
            context.android_package.as_str(),
            context.content_package.unwrap().as_str()
        );
    }

    #[test]
    fn legacy_package_adapter_preserves_old_single_package_behavior() {
        let context = AppContext::from_legacy_package("device-1", "com.example.game").unwrap();

        assert_eq!(context.android_package.as_str(), "com.example.game");
        assert_eq!(
            context.content_package.as_ref().unwrap().as_str(),
            "com.example.game"
        );
    }

    #[test]
    fn run_context_contains_run_identity_and_app_scope() {
        let context = RunContext::new(RunId::new("run-1").unwrap(), app());

        assert_eq!(context.run_id.as_str(), "run-1");
        assert_eq!(context.device_id().as_str(), "device-1");
    }

    #[test]
    fn run_request_is_runner_agnostic_and_round_trips_payload() {
        let payload = RunPayload::new(serde_json::json!({"args": {"count": 2}}));
        let request = RunRequest::for_app(app(), "gamer.yaml", "daily", payload).unwrap();

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["runner_id"], "gamer.yaml");
        assert_eq!(json["entrypoint"], "daily");
        assert_eq!(json["payload"]["args"]["count"], 2);
        assert_eq!(serde_json::from_value::<RunRequest>(json).unwrap(), request);
    }

    #[test]
    fn run_request_rejects_mismatched_request_and_app_device() {
        let other_device = DeviceId::new("device-2").unwrap();
        let error = RunRequest::new(
            other_device,
            app(),
            "gamer.yaml",
            "daily",
            RunPayload::default(),
        )
        .unwrap_err();

        assert!(matches!(error, ModelError::DeviceMismatch { .. }));
    }

    #[test]
    fn run_request_deserialization_keeps_device_scope_invariant() {
        let request =
            RunRequest::for_app(app(), "gamer.yaml", "daily", RunPayload::default()).unwrap();
        let mut json = serde_json::to_value(request).unwrap();
        json["device_id"] = serde_json::json!("device-2");

        assert!(serde_json::from_value::<RunRequest>(json).is_err());
    }

    #[test]
    fn resource_id_is_logical_and_handle_does_not_expose_a_host_path() {
        let id = ResourceId::with_revision(
            AppPackageId::new("official.example").unwrap(),
            Some("1.2.0".to_string()),
            "templates/status.png",
        )
        .unwrap();
        let handle = ResourceHandle::from(id.clone());

        assert_eq!(id.app_package().as_str(), "official.example");
        assert_eq!(id.logical_path(), "templates/status.png");
        assert_eq!(id.revision(), Some("1.2.0"));
        assert_eq!(id.legacy_key(), "official.example/templates/status.png");
        assert_eq!(handle.id(), &id);
    }

    #[test]
    fn resource_id_legacy_adapter_rejects_traversal_without_pathbuf() {
        let id = ResourceId::from_legacy_key("official.example/templates/status.png").unwrap();
        assert_eq!(id.logical_path(), "templates/status.png");
        assert!(ResourceId::from_legacy_key("official.example/../secret.png").is_err());
        assert!(ResourceId::from_legacy_key("official.example\\secret.png").is_err());
    }

    #[test]
    fn identifiers_reject_empty_control_characters_and_separators() {
        assert!(DeviceId::new(" ").is_err());
        assert!(AppPackageId::new("official/example").is_err());
        assert!(AndroidPackageName::new("com.example\n.game").is_err());
        assert!(ResourceId::new(AppPackageId::new("official.example").unwrap(), "").is_err());
    }
}
