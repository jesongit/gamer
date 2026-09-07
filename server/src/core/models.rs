//! Core runtime models shared by runners and boundary adapters.
//!
//! This module intentionally contains data-only types. It does not resolve
//! resources, access devices, or know about YAML, SQLite, WebRTC, or a host
//! filesystem.

#![allow(
    dead_code,
    reason = "core boundary helpers are adopted incrementally by concrete adapters"
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

/// Runtime Context 四层语义（plan §16 权威定义；本类型承载前三层，第四层见下）：
///
/// 1. **Device Context = `device_id`**——运行目标设备。设备登记
///    （DeviceManager）决定 scrcpy 会话、触控/帧链路；`AppContext` 只携带其
///    稳定 id，不携带连接态。
/// 2. **App Context = `android_package`**——纯运行目标：Android 侧已安装的
///    应用包名，`app.start`/`app.stop` 等生命周期操作的缺省目标。权威来源 =
///    设备登记配置的 `pkg`（`DeviceManager::snapshot`），不回退、不从
///    Package id 推导——两个命名空间严格分离。
/// 3. **Package Context = `content_package`**——数据上下文：资源解析域
///    （`packages/<package-id>/plugins/<plugin>/`）的 Package id，模板/脚本/
///    函数寻址全部落在该域；可缺省（无资源语义的运行）。
/// 4. **Plugin Context = 调用方扩展 id**——刻意不进 `AppContext`：调用方
///    扩展身份由扩展宿主实例承载（`extensions::service` 按 extension id
///    启动 host，能力调用经 host 授权），guest 无法伪造。资源三元组
///    [`ResourceId`] 的 plugin 段、`ResourceHandler` 的 plugin 参数、
///    `TimerRunnerRegistry` 的 `owner_extension_id` 均以宿主侧身份为权威。
///
/// 不变量：`android_package` 与 `content_package` 是两个独立命名空间
/// （Android 安装域 vs 内容寻址域），任何生产代码路径不得互相推导或兜底；
/// 值可以相等（例如设备 pkg 与数据包同名），但那只是巧合而非约定。
///
/// A device plus the Android application and optional content package in
/// scope for an operation.
///
/// Runtime Context 前三层的承载体（Device/App/Package；四层语义与不变量见
/// 下方模块级权威注释）。字段名即语义，不改名——`wire`（REST/序列化）已按
/// 该形状定型（plan §16 收口结论）。
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

    /// 测试装配助手：以同一个字符串填充两个命名空间（android_package =
    /// 运行目标、content_package = Package 数据上下文）。仅测试构建存在；
    /// 生产调用方必须用 [`Self::new`] 显式给出两个命名空间的值。
    #[cfg(test)]
    pub fn for_test(
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
///
/// Package Resource 寻址三元组：`(package_id, plugin_id, path)`。`path` 相对
/// `packages/<package-id>/plugins/<plugin-id>/`；插件被 Core 限制在该前缀内。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ResourceId {
    package: String,
    plugin: String,
    path: String,
}

impl<'de> Deserialize<'de> for ResourceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct ResourceIdFields {
            package: String,
            plugin: String,
            path: String,
        }

        let fields = ResourceIdFields::deserialize(deserializer)?;
        Self::new(fields.package, fields.plugin, fields.path).map_err(serde::de::Error::custom)
    }
}

impl ResourceId {
    pub fn new(
        package: impl Into<String>,
        plugin: impl Into<String>,
        path: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let package = package.into();
        let plugin = plugin.into();
        let path = validate_logical_path(&path.into())?;
        validate_scope_identifier("resource package", &package)?;
        validate_scope_identifier("resource plugin", &plugin)?;
        Ok(Self {
            package,
            plugin,
            path,
        })
    }

    /// Parse the canonical `<package>/<plugin>/<relative-resource>` composite
    /// key into a logical id without constructing a host filesystem path.
    pub fn from_composite_key(key: &str) -> Result<Self, ModelError> {
        let mut parts = key.splitn(3, '/');
        let package = parts.next().unwrap_or_default();
        let plugin = parts.next().unwrap_or_default();
        let path = parts.next().unwrap_or_default();
        if plugin.is_empty() || path.is_empty() {
            return Err(ModelError::InvalidLogicalPath {
                reason: "composite key must contain a package, plugin and resource path",
            });
        }
        Self::new(package, plugin, path)
    }

    pub fn package(&self) -> &str {
        &self.package
    }

    pub fn plugin(&self) -> &str {
        &self.plugin
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Canonical `<package>/<plugin>/<relative-resource>` composite key
    /// (日志展示 / 存储统一使用该形态).
    pub fn composite_key(&self) -> String {
        format!("{}/{}/{}", self.package, self.plugin, self.path)
    }
}

impl fmt::Display for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.composite_key())
    }
}

/// package-id / plugin-id 语法（与 `crate::resources::validate_scope_id` 同规
/// 则的本地实现——Core 契约层不反向依赖存储层）：`[a-z0-9][a-z0-9._-]*`，
/// 禁 `.` / `..` / 纯点。
fn validate_scope_identifier(kind: &'static str, value: &str) -> Result<(), ModelError> {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit() => {}
        _ => {
            return Err(ModelError::InvalidLogicalPath {
                reason: "package/plugin must start with a lowercase letter or digit",
            })
        }
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
    {
        return Err(ModelError::InvalidLogicalPath {
            reason: "package/plugin may only contain lowercase letters, digits and . _ -",
        });
    }
    if value == "." || value == ".." || value.matches('.').count() == value.len() {
        return Err(ModelError::InvalidLogicalPath {
            reason: "package/plugin must not be a bare dot sequence",
        });
    }
    if value.len() > 100 {
        return Err(ModelError::InvalidLogicalPath {
            reason: "package/plugin too long",
        });
    }
    let _ = kind;
    Ok(())
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
    fn test_helper_maps_package_to_both_namespaces() {
        let context = AppContext::for_test("device-1", "com.example.game").unwrap();

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
    fn resource_id_is_logical_and_plugin_scoped() {
        let id = ResourceId::new("official.example", "gamer.yaml", "templates/status.png").unwrap();

        assert_eq!(id.package(), "official.example");
        assert_eq!(id.plugin(), "gamer.yaml");
        assert_eq!(id.path(), "templates/status.png");
        assert_eq!(
            id.composite_key(),
            "official.example/gamer.yaml/templates/status.png"
        );
    }

    #[test]
    fn resource_id_composite_key_rejects_traversal_without_pathbuf() {
        let id = ResourceId::from_composite_key("official.example/gamer.yaml/templates/status.png")
            .unwrap();
        assert_eq!(id.path(), "templates/status.png");
        assert!(ResourceId::from_composite_key("official.example/../secret.png").is_err());
        assert!(ResourceId::from_composite_key("official.example\\secret.png").is_err());
        // 插件维度必填：两段式旧形态不再合法
        assert!(ResourceId::from_composite_key("official.example/only-path.png").is_err());
    }

    #[test]
    fn identifiers_reject_empty_control_characters_and_separators() {
        assert!(DeviceId::new(" ").is_err());
        assert!(AppPackageId::new("official/example").is_err());
        assert!(AndroidPackageName::new("com.example\n.game").is_err());
        assert!(ResourceId::new("official.example", "gamer.yaml", "").is_err());
        // 插件维度同样走严格 id 语法（隔离前缀不可被构造出来）
        assert!(ResourceId::new("official.example", "..", "x").is_err());
    }
}
