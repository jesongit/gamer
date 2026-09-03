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
    /// Declarative form schema；仅 `declarative` 贡献携带，随 UI 贡献注册表原样透传给前端。
    schema: Option<UiSchema>,
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

    pub(crate) fn schema(&self) -> Option<&UiSchema> {
        self.schema.as_ref()
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

/// Declarative 面板的表单 schema：由宿主原生渲染，控件值经 UI Bridge `plugin.call`
/// 发回插件后端；本结构只描述数据，不携带任何脚本或标记语言。
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct UiSchema {
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    fields: Vec<UiField>,
}

impl UiSchema {
    pub(crate) fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub(crate) fn fields(&self) -> &[UiField] {
        &self.fields
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UiFieldType {
    Text,
    Number,
    Boolean,
    Select,
    Button,
}

impl UiFieldType {
    fn parse(value: &str) -> ExtensionResult<Self> {
        match value.trim() {
            "text" => Ok(Self::Text),
            "number" => Ok(Self::Number),
            "boolean" => Ok(Self::Boolean),
            "select" => Ok(Self::Select),
            "button" => Ok(Self::Button),
            other => Err(ExtensionError::InvalidManifest(format!(
                "ui.contributions.fields.type 不受支持: {other}"
            ))),
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Select => "select",
            Self::Button => "button",
        }
    }
}

/// 控件默认值/下拉候选值。数字以规范化文本保存，保持声明结构可 `Eq` 比较，
/// 序列化时仍输出 JSON number。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UiFieldValue {
    Text(String),
    Number(String),
    Boolean(bool),
}

impl UiFieldValue {
    fn from_toml(value: &toml::Value) -> Option<Self> {
        match value {
            toml::Value::String(text) => Some(Self::Text(text.clone())),
            toml::Value::Integer(number) => Some(Self::Number(number.to_string())),
            toml::Value::Float(number) => {
                // toml 的 float 允许 inf/NaN；JSON number 不接受，直接拒绝。
                number.is_finite().then(|| Self::Number(number.to_string()))
            }
            toml::Value::Boolean(value) => Some(Self::Boolean(*value)),
            _ => None,
        }
    }

    /// 未显式提供 label 时的候选值展示文本。
    fn display(&self) -> &str {
        match self {
            Self::Text(text) | Self::Number(text) => text,
            Self::Boolean(true) => "true",
            Self::Boolean(false) => "false",
        }
    }
}

impl serde::Serialize for UiFieldValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Text(text) => serializer.serialize_str(text),
            Self::Boolean(value) => serializer.serialize_bool(*value),
            Self::Number(text) => {
                let number = serde_json::from_str::<serde_json::Number>(text)
                    .map_err(serde::ser::Error::custom)?;
                number.serialize(serializer)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct UiFieldOption {
    value: UiFieldValue,
    label: String,
}

/// 一个 declarative 表单控件。宿主（PluginPanelHost）按 `type` 原生渲染。
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct UiField {
    #[serde(rename = "type")]
    kind: UiFieldType,
    /// 值键（提交给插件后端的字段名）；button 无值，改为可选标识。
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<UiFieldValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Vec<UiFieldOption>>,
    /// button 点击时通知后端的动作名。
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

impl UiField {
    pub(crate) fn kind(&self) -> UiFieldType {
        self.kind
    }

    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn placeholder(&self) -> Option<&str> {
        self.placeholder.as_deref()
    }

    pub(crate) fn default(&self) -> Option<&UiFieldValue> {
        self.default.as_ref()
    }

    pub(crate) fn options(&self) -> Option<&[UiFieldOption]> {
        self.options.as_deref()
    }

    pub(crate) fn action(&self) -> Option<&str> {
        self.action.as_deref()
    }

    pub(crate) fn description(&self) -> Option<&str> {
        self.description.as_deref()
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
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    fields: Vec<RawUiField>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUiField {
    #[serde(rename = "type", alias = "control")]
    kind: String,
    #[serde(default, alias = "key")]
    name: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    placeholder: Option<String>,
    #[serde(default)]
    default: Option<toml::Value>,
    #[serde(default)]
    options: Option<Vec<RawUiFieldOption>>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// select 候选项：`{ value = ..., label = ... }` 或简写字符串（value = label = 字符串）。
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawUiFieldOption {
    Text(String),
    Map {
        value: toml::Value,
        #[serde(default)]
        label: Option<String>,
    },
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
    let schema = parse_ui_schema(runtime, raw.description, raw.fields)?;
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
        schema,
    })
}

/// declarative 贡献的 fields/description → 表单 schema；iframe 贡献禁止声明。
fn parse_ui_schema(
    runtime: UiRuntime,
    raw_description: Option<String>,
    raw_fields: Vec<RawUiField>,
) -> ExtensionResult<Option<UiSchema>> {
    if runtime != UiRuntime::Declarative {
        if raw_description.is_some() || !raw_fields.is_empty() {
            return Err(ExtensionError::InvalidManifest(
                "iframe contribution 不能声明 declarative 的 fields/description".to_string(),
            ));
        }
        return Ok(None);
    }
    if raw_fields.is_empty() && raw_description.is_none() {
        return Err(ExtensionError::InvalidManifest(
            "declarative contribution 至少需要声明一个 field 或 description".to_string(),
        ));
    }
    let description = clean_ui_text(raw_description, "fields.description")?;
    let fields = raw_fields
        .into_iter()
        .map(parse_ui_field)
        .collect::<ExtensionResult<Vec<_>>>()?;
    Ok(Some(UiSchema {
        description,
        fields,
    }))
}

/// 文案字段清洗：去首尾空白、拒绝控制字符；空值统一归一为 None（必填由调用方判定）。
fn clean_ui_text(value: Option<String>, field: &str) -> ExtensionResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.chars().any(char::is_control) {
        return Err(ExtensionError::InvalidManifest(format!(
            "ui.contributions.{field} 不能包含控制字符"
        )));
    }
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    Ok(Some(value.to_string()))
}

fn parse_ui_field(raw: RawUiField) -> ExtensionResult<UiField> {
    let kind = UiFieldType::parse(&raw.kind)?;
    let label = clean_ui_text(raw.label, "fields.label")?
        .filter(|text| !text.is_empty())
        .ok_or_else(|| {
            ExtensionError::InvalidManifest("ui.contributions.fields.label 不能为空".to_string())
        })?;
    let name = clean_ui_text(raw.name, "fields.name")?;
    let placeholder = clean_ui_text(raw.placeholder, "fields.placeholder")?;
    let action = clean_ui_text(raw.action, "fields.action")?;
    let description = clean_ui_text(raw.description, "fields.description")?;
    let unexpected = |what: &str| -> ExtensionResult<()> {
        Err(ExtensionError::InvalidManifest(format!(
            "ui.contributions.fields.{what} 属性不受 {} 控件支持",
            kind.as_str()
        )))
    };
    let required_name = || -> ExtensionResult<String> {
        name.clone().ok_or_else(|| {
            ExtensionError::InvalidManifest(format!(
                "ui.contributions.fields.{} 控件需要 name",
                kind.as_str()
            ))
        })
    };

    let field = match kind {
        UiFieldType::Text | UiFieldType::Number => {
            let name = required_name()?;
            unexpected_options_or_action(&raw.options, &action)?;
            let message = format!(
                "ui.contributions.fields.{} 控件的 default 类型不匹配",
                kind.as_str()
            );
            let default = parse_typed_default(
                raw.default.as_ref(),
                |value| {
                    matches!((value, kind), (UiFieldValue::Text(_), UiFieldType::Text))
                        || matches!(
                            (value, kind),
                            (UiFieldValue::Number(_), UiFieldType::Number)
                        )
                },
                &message,
            )?;
            UiField {
                kind,
                name: Some(name),
                label,
                placeholder,
                default,
                options: None,
                action: None,
                description,
            }
        }
        UiFieldType::Boolean => {
            let name = required_name()?;
            if placeholder.is_some() {
                unexpected("placeholder")?;
            }
            unexpected_options_or_action(&raw.options, &action)?;
            let default = parse_typed_default(
                raw.default.as_ref(),
                |value| matches!(value, UiFieldValue::Boolean(_)),
                "ui.contributions.fields.boolean 控件的 default 必须是布尔值",
            )?;
            UiField {
                kind,
                name: Some(name),
                label,
                placeholder: None,
                default,
                options: None,
                action: None,
                description,
            }
        }
        UiFieldType::Select => {
            let name = required_name()?;
            if placeholder.is_some() {
                unexpected("placeholder")?;
            }
            if action.is_some() {
                unexpected("action")?;
            }
            let raw_options = raw.options.ok_or_else(|| {
                ExtensionError::InvalidManifest(
                    "ui.contributions.fields.select 控件需要非空 options".to_string(),
                )
            })?;
            if raw_options.is_empty() {
                return Err(ExtensionError::InvalidManifest(
                    "ui.contributions.fields.select 控件需要非空 options".to_string(),
                ));
            }
            let mut options = Vec::with_capacity(raw_options.len());
            for option in raw_options {
                let (value, label) = match option {
                    RawUiFieldOption::Text(text) => {
                        if text.trim().is_empty() {
                            return Err(ExtensionError::InvalidManifest(
                                "ui.contributions.fields.select options 值不能为空".to_string(),
                            ));
                        }
                        (UiFieldValue::Text(text.clone()), text)
                    }
                    RawUiFieldOption::Map { value, label } => {
                        let value = UiFieldValue::from_toml(&value).ok_or_else(|| {
                            ExtensionError::InvalidManifest(
                                "ui.contributions.fields.select options.value 必须是字符串/数字/布尔"
                                    .to_string(),
                            )
                        })?;
                        let label = clean_ui_text(label, "fields.options.label")?
                            .unwrap_or_else(|| value.display().to_string());
                        (value, label)
                    }
                };
                if options
                    .iter()
                    .any(|existing: &UiFieldOption| existing.value == value)
                {
                    return Err(ExtensionError::InvalidManifest(
                        "ui.contributions.fields.select options 值重复".to_string(),
                    ));
                }
                options.push(UiFieldOption { value, label });
            }
            let default = match raw.default.as_ref().map(UiFieldValue::from_toml) {
                None => None,
                Some(None) => {
                    return Err(ExtensionError::InvalidManifest(
                        "ui.contributions.fields.select 控件的 default 必须是标量值".to_string(),
                    ))
                }
                Some(Some(value)) => Some(value),
            };
            if let Some(value) = &default {
                if !options.iter().any(|option| &option.value == value) {
                    return Err(ExtensionError::InvalidManifest(
                        "ui.contributions.fields.select 控件的 default 不在 options 中".to_string(),
                    ));
                }
            }
            UiField {
                kind,
                name: Some(name),
                label,
                placeholder: None,
                default,
                options: Some(options),
                action: None,
                description,
            }
        }
        UiFieldType::Button => {
            if placeholder.is_some() {
                unexpected("placeholder")?;
            }
            if raw.options.is_some() {
                unexpected("options")?;
            }
            if raw.default.is_some() {
                return Err(ExtensionError::InvalidManifest(
                    "ui.contributions.fields.button 控件不支持 default".to_string(),
                ));
            }
            let action = action.ok_or_else(|| {
                ExtensionError::InvalidManifest(
                    "ui.contributions.fields.button 控件需要 action".to_string(),
                )
            })?;
            UiField {
                kind,
                name,
                label,
                placeholder: None,
                default: None,
                options: None,
                action: Some(action),
                description,
            }
        }
    };
    Ok(field)
}

/// text/number 共用的禁止属性检查：select 候选与 button 动作都不属于输入控件。
fn unexpected_options_or_action(
    raw_options: &Option<Vec<RawUiFieldOption>>,
    action: &Option<String>,
) -> ExtensionResult<()> {
    if raw_options.is_some() {
        return Err(ExtensionError::InvalidManifest(
            "ui.contributions.fields.options 属性不受该控件支持".to_string(),
        ));
    }
    if action.is_some() {
        return Err(ExtensionError::InvalidManifest(
            "ui.contributions.fields.action 属性不受该控件支持".to_string(),
        ));
    }
    Ok(())
}

/// `default` 声明 → 类型化值；必须能解析为标量且满足控件类型谓词。
fn parse_typed_default(
    raw_default: Option<&toml::Value>,
    expected: impl Fn(&UiFieldValue) -> bool,
    message: &str,
) -> ExtensionResult<Option<UiFieldValue>> {
    match raw_default.map(UiFieldValue::from_toml) {
        None => Ok(None),
        Some(None) => Err(ExtensionError::InvalidManifest(message.to_string())),
        Some(Some(value)) if expected(&value) => Ok(Some(value)),
        Some(_) => Err(ExtensionError::InvalidManifest(message.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_with_ui(ui: &str) -> Vec<u8> {
        format!(
            "manifest_version = 1\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"Test extension\"\nentry = \"plugin.wasm\"\n{ui}"
        )
        .into_bytes()
    }

    fn parse_ui(ui: &str) -> ExtensionResult<UiContribution> {
        parse_manifest(&manifest_with_ui(ui)).map(|manifest| manifest.ui()[0].clone())
    }

    const DECLARATIVE_HEADER: &str =
        "[ui]\n[[ui.contributions]]\npanel_id = \"settings\"\ntitle = \"设置\"\nruntime = \"declarative\"\n";

    #[test]
    fn declarative_schema_parses_all_control_types_and_serializes_for_the_frontend() {
        let ui = format!(
            "{DECLARATIVE_HEADER}description = \"可选说明\"\n\
             [[ui.contributions.fields]]\ntype = \"text\"\nname = \"api_key\"\nlabel = \"API Key\"\nplaceholder = \"sk-...\"\ndefault = \"abc\"\n\
             [[ui.contributions.fields]]\ntype = \"number\"\nname = \"threads\"\nlabel = \"线程数\"\ndefault = 4\n\
             [[ui.contributions.fields]]\ntype = \"number\"\nname = \"threshold\"\nlabel = \"阈值\"\ndefault = 0.75\n\
             [[ui.contributions.fields]]\ntype = \"boolean\"\nname = \"enabled\"\nlabel = \"启用\"\ndefault = true\n\
             [[ui.contributions.fields]]\ntype = \"select\"\nname = \"mode\"\nlabel = \"模式\"\ndefault = \"fast\"\n\
             [[ui.contributions.fields.options]]\nvalue = \"fast\"\nlabel = \"快速\"\n\
             [[ui.contributions.fields.options]]\nvalue = \"slow\"\n\
             [[ui.contributions.fields]]\ntype = \"button\"\nlabel = \"刷新\"\naction = \"refresh\"\ndescription = \"立即重新加载\"\n"
        );
        let contribution = parse_ui(&ui).unwrap();
        let schema = contribution.schema().unwrap();
        assert_eq!(schema.description(), Some("可选说明"));
        let fields = schema.fields();
        assert_eq!(fields.len(), 6);
        assert_eq!(fields[0].name().unwrap(), "api_key");
        assert!(matches!(fields[0].kind, UiFieldType::Text));
        assert!(matches!(fields[1].default, Some(UiFieldValue::Number(ref n)) if n == "4"));
        assert!(matches!(fields[2].default, Some(UiFieldValue::Number(ref n)) if n == "0.75"));
        assert!(matches!(
            fields[3].default,
            Some(UiFieldValue::Boolean(true))
        ));
        let options = fields[4].options.as_ref().unwrap();
        assert_eq!(options.len(), 2);
        assert_eq!(options[1].label, "slow"); // 简写字符串选项 value = label
        assert_eq!(fields[5].action.as_deref(), Some("refresh"));
        assert!(fields[5].name().is_none());

        // 前端契约：fields 原样透传（数字默认值仍是 JSON number）。
        let json = serde_json::to_value(contribution.schema().unwrap().fields()).unwrap();
        let fields_json = json.as_array().unwrap();
        assert_eq!(fields_json[0]["type"], "text");
        assert_eq!(fields_json[0]["placeholder"], "sk-...");
        assert_eq!(fields_json[1]["default"], 4);
        assert_eq!(fields_json[2]["default"], 0.75);
        assert_eq!(fields_json[4]["options"][1]["value"], "slow");
        assert_eq!(fields_json[5]["action"], "refresh");
        assert!(fields_json[5].get("name").is_none());
    }

    #[test]
    fn unknown_control_type_is_rejected() {
        let ui = format!(
            "{DECLARATIVE_HEADER}[[ui.contributions.fields]]\ntype = \"slider\"\nname = \"v\"\nlabel = \"V\"\n"
        );
        let error = parse_ui(&ui).unwrap_err();
        assert!(
            matches!(error, ExtensionError::InvalidManifest(ref message) if message.contains("slider"))
        );
    }

    #[test]
    fn field_requires_name_label_and_type_matched_defaults() {
        // 缺 name
        let ui = format!(
            "{DECLARATIVE_HEADER}[[ui.contributions.fields]]\ntype = \"text\"\nlabel = \"L\"\n"
        );
        assert!(parse_ui(&ui).is_err());
        // 缺 label
        let ui = format!(
            "{DECLARATIVE_HEADER}[[ui.contributions.fields]]\ntype = \"text\"\nname = \"a\"\n"
        );
        assert!(parse_ui(&ui).is_err());
        // text 默认值给了数字
        let ui = format!(
            "{DECLARATIVE_HEADER}[[ui.contributions.fields]]\ntype = \"text\"\nname = \"a\"\nlabel = \"L\"\ndefault = 3\n"
        );
        assert!(parse_ui(&ui).is_err());
        // boolean 默认值给了字符串
        let ui = format!(
            "{DECLARATIVE_HEADER}[[ui.contributions.fields]]\ntype = \"boolean\"\nname = \"a\"\nlabel = \"L\"\ndefault = \"yes\"\n"
        );
        assert!(parse_ui(&ui).is_err());
    }

    #[test]
    fn select_requires_non_empty_unique_options_and_default_inside_options() {
        // 缺 options
        let ui = format!(
            "{DECLARATIVE_HEADER}[[ui.contributions.fields]]\ntype = \"select\"\nname = \"m\"\nlabel = \"M\"\n"
        );
        assert!(parse_ui(&ui).is_err());
        // default 不在 options 内
        let ui = format!(
            "{DECLARATIVE_HEADER}[[ui.contributions.fields]]\ntype = \"select\"\nname = \"m\"\nlabel = \"M\"\ndefault = \"x\"\n\
             [[ui.contributions.fields.options]]\nvalue = \"y\"\n"
        );
        assert!(parse_ui(&ui).is_err());
        // 重复 value
        let ui = format!(
            "{DECLARATIVE_HEADER}[[ui.contributions.fields]]\ntype = \"select\"\nname = \"m\"\nlabel = \"M\"\n\
             [[ui.contributions.fields.options]]\nvalue = \"y\"\n\
             [[ui.contributions.fields.options]]\nvalue = \"y\"\n"
        );
        assert!(parse_ui(&ui).is_err());
    }

    #[test]
    fn button_requires_action_and_rejects_value_attributes() {
        let ui = format!(
            "{DECLARATIVE_HEADER}[[ui.contributions.fields]]\ntype = \"button\"\nlabel = \"B\"\n"
        );
        assert!(parse_ui(&ui).is_err());
        let ui = format!(
            "{DECLARATIVE_HEADER}[[ui.contributions.fields]]\ntype = \"button\"\nlabel = \"B\"\naction = \"go\"\ndefault = 1\n"
        );
        assert!(parse_ui(&ui).is_err());
    }

    #[test]
    fn iframe_contributions_reject_declarative_schema() {
        let ui = "[ui]\n[[ui.contributions]]\npanel_id = \"p\"\ntitle = \"P\"\nruntime = \"iframe\"\nentry = \"ui/index.html\"\ndescription = \"说明\"\n";
        assert!(parse_ui(ui).is_err());
        let ui = "[ui]\n[[ui.contributions]]\npanel_id = \"p\"\ntitle = \"P\"\nruntime = \"iframe\"\nentry = \"ui/index.html\"\n\
             [[ui.contributions.fields]]\ntype = \"text\"\nname = \"a\"\nlabel = \"L\"\n";
        assert!(parse_ui(ui).is_err());
    }

    #[test]
    fn declarative_without_any_field_or_description_is_rejected() {
        let ui = "[ui]\n[[ui.contributions]]\npanel_id = \"p\"\ntitle = \"P\"\nruntime = \"declarative\"\n";
        assert!(parse_ui(ui).is_err());
    }

    #[test]
    fn declarative_accepts_key_alias_for_name() {
        let ui = format!(
            "{DECLARATIVE_HEADER}[[ui.contributions.fields]]\ntype = \"text\"\nkey = \"api_key\"\nlabel = \"K\"\n"
        );
        let contribution = parse_ui(&ui).unwrap();
        assert_eq!(
            contribution.schema().unwrap().fields()[0].name(),
            Some("api_key")
        );
    }
}
