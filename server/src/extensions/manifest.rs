//! `.gplugin` manifest parsing and compatibility checks.

use std::collections::BTreeMap;

use semver::VersionReq;
use serde::Deserialize;

use super::error::{ExtensionError, ExtensionResult};
use super::host_api::{HostApiDomain, HostApiRequirement};
use super::model::{validate_display_name, ExtensionId, ExtensionPath, ExtensionVersion};
use super::permissions::PermissionSet;

pub(crate) const MANIFEST_VERSION: u32 = 2;
/// 存量安装目录仍可能持有 v1 manifest（旧版本服务端安装的包）。读端（快照/
/// 列表）容忍 v1（等价 wasm 执行类型），安装/更新端只接受 v2。
pub(crate) const MANIFEST_VERSION_LEGACY: u32 = 1;
pub(crate) const MANIFEST_FILE_NAME: &str = "manifest.toml";
/// 约定俗成的 WASM entry 名。builtin 包不得携带该文件（防伪装执行类型）。
pub(crate) const CONVENTIONAL_WASM_ENTRY: &str = "plugin.wasm";

/// 后端执行类型（manifest v2 `[execution]`）。与 `ui.contributions.runtime`
/// （界面渲染类型）严格分离：wasm/builtin 插件都可以带任意 runtime 的 UI。
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ExecutionKind {
    /// 携带真实 guest 字节（`entry` 必填，`\0asm` magic 校验）。
    Wasm,
    /// 宿主预置实现（无 guest、无常驻实例）；`builtin_id` 必须在服务端
    /// BuiltinExtensionDescriptor 注册表中已注册。
    Builtin,
}

/// `[execution]` 表的解析结果。`host_version` 是 advisory 兼容性声明
/// （如 ">=0.6.0"），随 inspect/snapshot 透传前端展示，服务端暂不做硬门禁。
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct ExecutionSpec {
    kind: ExecutionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    builtin_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_version: Option<String>,
}

impl ExecutionSpec {
    pub(crate) fn kind(&self) -> ExecutionKind {
        self.kind
    }

    pub(crate) fn builtin_id(&self) -> Option<&str> {
        self.builtin_id.as_deref()
    }

    pub(crate) fn host_version(&self) -> Option<&str> {
        self.host_version.as_deref()
    }
}

/// Parsed, immutable metadata for one installed extension version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExtensionManifest {
    manifest_version: u32,
    id: ExtensionId,
    version: ExtensionVersion,
    name: String,
    description: Option<String>,
    /// WASM entry；仅 `execution.kind = "wasm"` 时存在。
    entry: Option<ExtensionPath>,
    execution: ExecutionSpec,
    host_api: HostApiRequirements,
    permissions: PermissionSet,
    /// Android 应用支持声明（`[targets.android].packages`）。缺省/空 = 通用
    /// （等价 `*`）；`*` = 全部应用；其余按 Android 包名精确匹配。仅作运行
    /// 目标声明，宿主不做硬门禁（前端按当前设备应用过滤插件入口）。
    targets: Vec<String>,
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

    pub(crate) fn entry(&self) -> Option<&ExtensionPath> {
        self.entry.as_ref()
    }

    pub(crate) fn execution(&self) -> &ExecutionSpec {
        &self.execution
    }

    pub(crate) fn host_api(&self) -> &HostApiRequirements {
        &self.host_api
    }

    pub(crate) fn permissions(&self) -> &PermissionSet {
        &self.permissions
    }

    /// Android 应用支持声明（`targets.android.packages`，缺省/空 = 通用）。
    pub(crate) fn android_targets(&self) -> &[String] {
        &self.targets
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
    /// `runtime = "core"` 贡献的宿主组件键（如 "console.scripts"）。组件名的
    /// 解释权在前端 core-component-registry，服务端只保证非空并原样透传。
    component: Option<String>,
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

    pub(crate) fn component(&self) -> Option<&str> {
        self.component.as_deref()
    }

    pub(crate) fn schema(&self) -> Option<&UiSchema> {
        self.schema.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UiRuntime {
    /// 宿主原生 Vue 面板：前端按 `component` 键从 core-component-registry 取组件。
    Core,
    Declarative,
    Iframe,
}

impl UiRuntime {
    fn parse(value: &str) -> ExtensionResult<Self> {
        match value.trim() {
            "core" => Ok(Self::Core),
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
    /// WASM entry；仅 wasm 执行类型必填（builtin 必须缺省）。
    #[serde(default)]
    entry: Option<String>,
    #[serde(default)]
    execution: Option<RawExecution>,
    #[serde(default)]
    host_api: RawHostApiRequirements,
    #[serde(default)]
    permissions: Vec<String>,
    /// Android 应用支持声明（可缺省 = 通用）；结构与 package.toml 的
    /// `[targets.android]` 一致。
    #[serde(default)]
    targets: Option<RawTargets>,
    #[serde(default)]
    ui: RawUi,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTargets {
    #[serde(default)]
    android: Option<RawAndroidTargets>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAndroidTargets {
    #[serde(default)]
    packages: Vec<String>,
}

/// manifest v2 `[execution]` 表（可选；缺省按 `kind = "wasm"` 处理）。
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExecution {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    builtin_id: Option<String>,
    /// advisory 兼容性声明（如 ">=0.6.0"）；服务端透传展示，不做硬门禁。
    #[serde(default)]
    host_version: Option<String>,
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
    component: Option<String>,
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
    #[serde(default)]
    media: Option<String>,
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
            (HostApiDomain::Media, self.media),
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

/// 严格解析（安装/更新/inspect 路径）：只接受当前 `MANIFEST_VERSION`（v2）。
pub(crate) fn parse_manifest(bytes: &[u8]) -> ExtensionResult<ExtensionManifest> {
    parse_manifest_with_versions(bytes, &[MANIFEST_VERSION])
}

/// 读端容忍解析（已安装版本目录扫描）：额外接受 v1 存量 manifest（等价 wasm
/// 执行类型）。快照路径不因 manifest_version 升级而拒绝列出旧安装。
pub(crate) fn parse_manifest_installed(bytes: &[u8]) -> ExtensionResult<ExtensionManifest> {
    parse_manifest_with_versions(bytes, &[MANIFEST_VERSION, MANIFEST_VERSION_LEGACY])
}

fn parse_manifest_with_versions(
    bytes: &[u8],
    accepted_versions: &[u32],
) -> ExtensionResult<ExtensionManifest> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| ExtensionError::InvalidManifest(format!("必须是 UTF-8: {error}")))?;
    let raw: RawManifest =
        toml::from_str(text).map_err(|error| ExtensionError::InvalidManifest(error.to_string()))?;
    if !accepted_versions.contains(&raw.manifest_version) {
        if raw.manifest_version < MANIFEST_VERSION {
            return Err(ExtensionError::InvalidManifest(format!(
                "manifest_version={} 已不受支持，请升级插件包到 manifest_version={MANIFEST_VERSION}",
                raw.manifest_version
            )));
        }
        return Err(ExtensionError::InvalidManifest(format!(
            "manifest_version={} 不受支持，当前仅支持 {}（需要升级 Gamer 宿主）",
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
    let (execution, entry) = build_execution(raw.execution.as_ref(), raw.entry.as_deref())?;
    let host_api = raw.host_api.into_requirements()?;
    let permissions = PermissionSet::parse(raw.permissions)?;
    let targets = parse_android_targets(
        raw.targets
            .and_then(|targets| targets.android)
            .map(|android| android.packages)
            .unwrap_or_default(),
    )?;
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
        execution,
        host_api,
        permissions,
        targets,
        ui,
    })
}

/// `[targets.android].packages` 归一化：trim + 轻校验（复用 Package 侧规则，
/// `*` = 通用）+ 保序去重。缺省/空 = 通用，无需显式 `*`。
fn parse_android_targets(packages: Vec<String>) -> ExtensionResult<Vec<String>> {
    let mut targets: Vec<String> = Vec::new();
    for value in packages {
        let value = value.trim();
        crate::resources::validate_android_target(value).map_err(|error| {
            ExtensionError::InvalidManifest(format!("targets.android.packages 无效: {error}"))
        })?;
        if !targets.iter().any(|existing| existing == value) {
            targets.push(value.to_string());
        }
    }
    Ok(targets)
}

/// `[execution]`（可缺省）+ 顶层 `entry` → （执行类型规约, 已校验的 WASM entry）。
///
/// - 缺省 / `kind = "wasm"`：`entry` 必填、必须 `.wasm` 后缀且不指向 manifest。
/// - `kind = "builtin"`：`entry` 必须缺省；`builtin_id` 必填（id 语法校验，
///   注册存在性由服务层在安装时校验——manifest 层不认识注册表）。
fn build_execution(
    raw: Option<&RawExecution>,
    entry: Option<&str>,
) -> ExtensionResult<(ExecutionSpec, Option<ExtensionPath>)> {
    let parse_entry = |entry: &str| -> ExtensionResult<ExtensionPath> {
        let entry = ExtensionPath::parse(entry)?;
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
        Ok(entry)
    };
    let host_version = match raw.and_then(|raw| raw.host_version.as_deref()) {
        None => None,
        Some(value) => {
            if value.chars().any(char::is_control) {
                return Err(ExtensionError::InvalidManifest(
                    "execution.host_version 不能包含控制字符".to_string(),
                ));
            }
            let value = value.trim();
            if value.len() > 128 {
                return Err(ExtensionError::InvalidManifest(
                    "execution.host_version 超过 128 字符上限".to_string(),
                ));
            }
            (!value.is_empty()).then(|| value.to_string())
        }
    };
    let kind = match raw.and_then(|raw| raw.kind.as_deref()) {
        None | Some("wasm") => {
            let entry_text = entry.ok_or_else(|| {
                ExtensionError::InvalidManifest(
                    "wasm 执行类型需要 entry（指向包内 .wasm 文件）".to_string(),
                )
            })?;
            let parsed_entry = parse_entry(entry_text)?;
            if let Some(raw) = raw {
                if raw.builtin_id.is_some() {
                    return Err(ExtensionError::InvalidManifest(
                        "wasm 执行类型不能声明 builtin_id".to_string(),
                    ));
                }
            }
            (
                ExecutionSpec {
                    kind: ExecutionKind::Wasm,
                    builtin_id: None,
                    host_version,
                },
                Some(parsed_entry),
            )
        }
        Some("builtin") => {
            if entry.is_some() {
                return Err(ExtensionError::InvalidManifest(
                    "builtin 执行类型的 entry 必须缺省（宿主预置实现不携带 guest 字节）"
                        .to_string(),
                ));
            }
            let builtin_id = raw
                .and_then(|raw| raw.builtin_id.as_deref())
                .ok_or_else(|| {
                    ExtensionError::InvalidManifest(
                        "builtin 执行类型需要 builtin_id（宿主注册表中的实现 id）".to_string(),
                    )
                })?;
            let builtin_id = ExtensionId::parse(builtin_id)
                .map_err(|error| {
                    ExtensionError::InvalidManifest(format!("builtin_id 无效: {error}"))
                })?
                .as_str()
                .to_string();
            (
                ExecutionSpec {
                    kind: ExecutionKind::Builtin,
                    builtin_id: Some(builtin_id),
                    host_version,
                },
                None,
            )
        }
        Some(other) => {
            return Err(ExtensionError::InvalidManifest(format!(
                "execution.kind 不受支持: {other}"
            )));
        }
    };
    Ok(kind)
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
    // 组件键只属于 core 贡献：服务端不做组件名白名单（组件名是前端知识），
    // 只要求非空字符串并原样透传；其余 runtime 携带 component 视为拼写错误。
    let component = match runtime {
        UiRuntime::Core => {
            let component = clean_ui_text(raw.component, "component")?.ok_or_else(|| {
                ExtensionError::InvalidManifest(
                    "core contribution 需要 component（宿主组件键，如 \"console.scripts\"）"
                        .to_string(),
                )
            })?;
            Some(component)
        }
        UiRuntime::Declarative | UiRuntime::Iframe => {
            if raw.component.is_some() {
                let name = if runtime == UiRuntime::Declarative {
                    "declarative"
                } else {
                    "iframe"
                };
                return Err(ExtensionError::InvalidManifest(format!(
                    "{name} contribution 不能带 component"
                )));
            }
            None
        }
    };
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
        (UiRuntime::Core, Some(_)) => {
            return Err(ExtensionError::InvalidManifest(
                "core contribution 不能带 entry（面板由宿主组件渲染）".to_string(),
            ));
        }
        (UiRuntime::Core, None) => None,
    };
    // 组件键只属于 core 贡献；其余 runtime 携带即视为 manifest 拼写错误。
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
        component,
        schema,
    })
}

/// declarative 贡献的 fields/description → 表单 schema；其余 runtime 禁止声明。
fn parse_ui_schema(
    runtime: UiRuntime,
    raw_description: Option<String>,
    raw_fields: Vec<RawUiField>,
) -> ExtensionResult<Option<UiSchema>> {
    if runtime != UiRuntime::Declarative {
        if raw_description.is_some() || !raw_fields.is_empty() {
            return Err(ExtensionError::InvalidManifest(
                "非 declarative contribution 不能声明 fields/description".to_string(),
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
            "manifest_version = 2\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"Test extension\"\nentry = \"plugin.wasm\"\n{ui}"
        )
        .into_bytes()
    }

    #[test]
    fn execution_defaults_to_wasm_and_requires_entry() {
        let manifest = b"manifest_version = 2\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"T\"\nentry = \"plugin.wasm\"\n";
        let parsed = parse_manifest(manifest).unwrap();
        assert_eq!(parsed.manifest_version(), MANIFEST_VERSION);
        assert_eq!(parsed.execution().kind(), ExecutionKind::Wasm);
        assert!(parsed.execution().builtin_id().is_none());
        assert_eq!(
            parsed.entry().map(|entry| entry.as_str()),
            Some("plugin.wasm")
        );

        // 缺 entry（且无 [execution]）= 缺省 wasm 却没有 guest 字节 → 拒绝
        let missing = b"manifest_version = 2\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"T\"\n";
        assert!(parse_manifest(missing).is_err());
    }

    #[test]
    fn builtin_execution_requires_builtin_id_and_rejects_entry() {
        let base = |body: &str| -> Vec<u8> { body.as_bytes().to_vec() };
        let valid = base(
            "manifest_version = 2\nid = \"gamer.video\"\nversion = \"1.0.0\"\nname = \"V\"\n\
             [execution]\nkind = \"builtin\"\nbuiltin_id = \"gamer.video\"\n",
        );
        let parsed = parse_manifest(&valid).unwrap();
        assert_eq!(parsed.execution().kind(), ExecutionKind::Builtin);
        assert_eq!(parsed.execution().builtin_id(), Some("gamer.video"));
        assert!(parsed.entry().is_none());

        // 缺 builtin_id
        let no_id = base(
            "manifest_version = 2\nid = \"gamer.video\"\nversion = \"1.0.0\"\nname = \"V\"\n\
             [execution]\nkind = \"builtin\"\n",
        );
        assert!(parse_manifest(&no_id).is_err());
        // builtin 带 entry（伪装 guest）
        let with_entry = base(
            "manifest_version = 2\nid = \"gamer.video\"\nversion = \"1.0.0\"\nname = \"V\"\nentry = \"plugin.wasm\"\n\
             [execution]\nkind = \"builtin\"\nbuiltin_id = \"gamer.video\"\n",
        );
        assert!(parse_manifest(&with_entry).is_err());
        // wasm 声明 builtin_id（伪装内置）
        let wasm_with_builtin = base(
            "manifest_version = 2\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"T\"\nentry = \"plugin.wasm\"\n\
             [execution]\nkind = \"wasm\"\nbuiltin_id = \"gamer.video\"\n",
        );
        assert!(parse_manifest(&wasm_with_builtin).is_err());
        // 未知 kind
        let unknown_kind = base(
            "manifest_version = 2\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"T\"\nentry = \"plugin.wasm\"\n\
             [execution]\nkind = \"native\"\n",
        );
        assert!(parse_manifest(&unknown_kind).is_err());
    }

    #[test]
    fn host_version_is_advisory_passthrough_and_media_host_api_domain_parses() {
        let manifest = b"manifest_version = 2\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"T\"\nentry = \"plugin.wasm\"\n\
             [execution]\nkind = \"wasm\"\nhost_version = \">=0.1.1\"\n\
             [host_api]\nmedia = \"^1.0\"\n";
        let parsed = parse_manifest(manifest).unwrap();
        assert_eq!(parsed.execution().host_version(), Some(">=0.1.1"));
        assert!(parsed.host_api().get(HostApiDomain::Media).is_some());

        // 无效域版本要求仍然拒绝（media 与其他域同语义）
        let bad = b"manifest_version = 2\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"T\"\nentry = \"plugin.wasm\"\n\
             [host_api]\nmedia = \"not-a-version\"\n";
        assert!(parse_manifest(bad).is_err());
    }

    #[test]
    fn android_targets_default_to_universal_and_support_wildcard() {
        // 缺省 [targets] = 通用（空列表，`*` 语义；存量安装manifest 无此段照常解析）
        let default = b"manifest_version = 2\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"T\"\nentry = \"plugin.wasm\"\n";
        assert!(parse_manifest(default)
            .unwrap()
            .android_targets()
            .is_empty());

        // 显式声明：trim + 保序去重 + `*` = 通用合法值
        let explicit = b"manifest_version = 2\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"T\"\nentry = \"plugin.wasm\"\n\
             [targets.android]\npackages = [\"com.example.Game\", \"*\", \" com.other \", \"com.example.Game\"]\n";
        let parsed = parse_manifest(explicit).unwrap();
        assert_eq!(
            parsed.android_targets(),
            &[
                "com.example.Game".to_string(),
                "*".to_string(),
                "com.other".to_string(),
            ]
        );

        // 非法目标（空串）拒绝
        let bad = b"manifest_version = 2\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"T\"\nentry = \"plugin.wasm\"\n\
             [targets.android]\npackages = [\"\"]\n";
        assert!(parse_manifest(bad).is_err());

        // 未知 targets 子字段拒绝（deny_unknown_fields，与 package.toml 同形）
        let unknown = b"manifest_version = 2\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"T\"\nentry = \"plugin.wasm\"\n\
             [targets.windows]\npackages = [\"x\"]\n";
        assert!(parse_manifest(unknown).is_err());
    }

    #[test]
    fn manifest_v1_is_rejected_by_strict_parse_and_accepted_by_installed_parse() {
        let legacy = b"manifest_version = 1\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"T\"\nentry = \"plugin.wasm\"\n";
        assert!(parse_manifest(legacy).is_err());
        let installed = parse_manifest_installed(legacy).unwrap();
        assert_eq!(installed.manifest_version(), MANIFEST_VERSION_LEGACY);
        assert_eq!(installed.execution().kind(), ExecutionKind::Wasm);

        // 未来版本两端都拒绝（提示升级宿主）
        let future =
            b"manifest_version = 3\nid = \"com.example.extension\"\nversion = \"1.0.0\"\nname = \"T\"\nentry = \"plugin.wasm\"\n";
        assert!(parse_manifest(future).is_err());
        assert!(parse_manifest_installed(future).is_err());
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

    const CORE_HEADER: &str = "[ui]\n[[ui.contributions]]\npanel_id = \"scripts\"\ntitle = \"自动化\"\nruntime = \"core\"\n";

    #[test]
    fn core_contribution_parses_component_and_serializes_for_the_frontend() {
        let ui = format!(
            "{CORE_HEADER}component = \"console.scripts\"\nrequires_device = true\npreferred_width = 440\n"
        );
        let contribution = parse_ui(&ui).unwrap();
        assert!(matches!(contribution.runtime(), UiRuntime::Core));
        assert_eq!(contribution.component(), Some("console.scripts"));
        assert!(contribution.entry().is_none());
        assert!(contribution.schema().is_none());
        assert!(contribution.requires_device());
        assert_eq!(contribution.preferred_width(), Some(440));
    }

    #[test]
    fn core_contribution_rejects_missing_blank_or_reserved_attributes() {
        // 缺 component
        assert!(parse_ui(CORE_HEADER).is_err());
        // component 空白
        let ui = format!("{CORE_HEADER}component = \"   \"\n");
        assert!(parse_ui(&ui).is_err());
        // core 不能带 iframe entry
        let ui =
            format!("{CORE_HEADER}component = \"console.scripts\"\nentry = \"ui/index.html\"\n");
        assert!(parse_ui(&ui).is_err());
        // core 不能声明 declarative fields
        let ui = format!(
            "{CORE_HEADER}component = \"console.scripts\"\n\
             [[ui.contributions.fields]]\ntype = \"text\"\nname = \"a\"\nlabel = \"L\"\n"
        );
        assert!(parse_ui(&ui).is_err());
        // component 首尾空白被归一
        let ui = format!("{CORE_HEADER}component = \" console.scripts \"\n");
        assert_eq!(parse_ui(&ui).unwrap().component(), Some("console.scripts"));
    }

    #[test]
    fn non_core_contributions_reject_component() {
        let ui = "[ui]\n[[ui.contributions]]\npanel_id = \"p\"\ntitle = \"P\"\nruntime = \"iframe\"\nentry = \"ui/index.html\"\ncomponent = \"console.scripts\"\n";
        assert!(parse_ui(ui).is_err());
        let ui = "[ui]\n[[ui.contributions]]\npanel_id = \"p\"\ntitle = \"P\"\nruntime = \"declarative\"\ndescription = \"说明\"\ncomponent = \"console.scripts\"\n";
        assert!(parse_ui(ui).is_err());
    }
}
