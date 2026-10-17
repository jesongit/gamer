//! Explicit plugin tools. No UI button, native action or function becomes an
//! external tool unless its installed manifest declares this contract.
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    io::Read,
    sync::{Arc, Weak},
};

use super::{
    ExtensionError, ExtensionId, ExtensionManifest, ExtensionPath, ExtensionResult,
    ExtensionService, Permission, PermissionSet,
};
use crate::mcp::{
    ContextRequirements, InvocationContext, McpError, ToolDescriptor, ToolEffect, ToolProvider,
    ToolResult,
};

const MAX_SCHEMA_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawToolDeclaration {
    pub tool_id: String,
    pub action: String,
    pub description: String,
    pub input_schema: String,
    #[serde(default)]
    pub output_schema: Option<String>,
    pub required_scopes: BTreeSet<String>,
    #[serde(default)]
    pub required_permissions: Vec<String>,
    pub effect: ToolEffect,
    #[serde(default)]
    pub context: ContextRequirements,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolDeclaration {
    pub tool_id: String,
    pub action: String,
    pub description: String,
    pub input_schema: ExtensionPath,
    pub output_schema: Option<ExtensionPath>,
    pub required_scopes: BTreeSet<String>,
    pub required_permissions: PermissionSet,
    pub effect: ToolEffect,
    pub context: ContextRequirements,
}
impl ToolDeclaration {
    pub(super) fn parse(
        raw: RawToolDeclaration,
        id: &ExtensionId,
        permissions: &PermissionSet,
    ) -> ExtensionResult<Self> {
        if raw.tool_id.is_empty()
            || !raw
                .tool_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || raw.action.trim().is_empty()
            || raw.action.len() > 128
            || raw.action.chars().any(char::is_control)
            || raw.description.trim().is_empty()
            || raw.description.len() > 16 * 1024
            || raw.required_scopes.is_empty()
        {
            return Err(ExtensionError::InvalidManifest(
                "Invalid external tool identity, action, description or scopes".into(),
            ));
        }
        if format!("gamer.plugin.{}.{}", id.as_str(), raw.tool_id).len() > 128 {
            return Err(ExtensionError::InvalidManifest(
                "External tool name exceeds 128 bytes".into(),
            ));
        }
        let required_permissions = PermissionSet::parse(raw.required_permissions)?;
        for permission in required_permissions.iter() {
            if !permissions.allows(permission) {
                return Err(ExtensionError::InvalidManifest(format!(
                    "External tool requires undeclared permission {}",
                    permission.as_str()
                )));
            }
            if raw.effect.is_read_only() && !permission_is_read_only(permission) {
                return Err(ExtensionError::InvalidManifest(
                    "A read-only tool cannot declare write permissions".into(),
                ));
            }
        }
        if super::is_internal_native_action(id, &raw.action)
            || super::native_action_expected_caller(id, &raw.action)
                .is_some_and(|caller| caller != "user-management")
        {
            return Err(ExtensionError::InvalidManifest(
                "Trusted internal collaboration actions cannot be external tools".into(),
            ));
        }
        Ok(Self {
            tool_id: raw.tool_id,
            action: raw.action,
            description: raw.description,
            input_schema: ExtensionPath::parse(&raw.input_schema)?,
            output_schema: raw
                .output_schema
                .map(|path| ExtensionPath::parse(&path))
                .transpose()?,
            required_scopes: raw.required_scopes,
            required_permissions,
            effect: raw.effect,
            context: raw.context,
        })
    }
    pub(crate) fn name(&self, id: &ExtensionId) -> String {
        format!("gamer.plugin.{}.{}", id.as_str(), self.tool_id)
    }
    pub(super) fn descriptor(
        &self,
        id: &ExtensionId,
        input: &[u8],
        output: Option<&[u8]>,
    ) -> ExtensionResult<ToolDescriptor> {
        let parse = |bytes: &[u8]| -> ExtensionResult<Value> {
            if bytes.len() > MAX_SCHEMA_BYTES {
                return Err(ExtensionError::InvalidManifest(
                    "External tool schema exceeds size limit".into(),
                ));
            }
            serde_json::from_slice(bytes).map_err(|_| {
                ExtensionError::InvalidManifest("External tool schema must be JSON".into())
            })
        };
        let descriptor = ToolDescriptor {
            name: self.name(id),
            description: self.description.clone(),
            input_schema: parse(input)?,
            output_schema: output.map(parse).transpose()?,
            required_scopes: self.required_scopes.clone(),
            effect: self.effect,
            context: self.context.clone(),
        };
        crate::mcp::normalize_descriptor(descriptor)
            .map_err(|error| ExtensionError::InvalidManifest(error.to_string()))
    }
}

pub(super) fn permission_is_read_only(permission: Permission) -> bool {
    matches!(
        permission,
        Permission::DeviceRead
            | Permission::VisionMatch
            | Permission::VisionColor
            | Permission::VisionOcr
            | Permission::ResourceRead
            | Permission::RuntimeSleep
            | Permission::MediaRead
            | Permission::MediaEventsRead
            | Permission::UiHost
    )
}
pub(super) fn requires_host_adapter(permission: Permission) -> bool {
    matches!(
        permission,
        Permission::DeviceApp
            | Permission::InputTap
            | Permission::InputSwipe
            | Permission::InputKey
            | Permission::InputText
            | Permission::Touch
            | Permission::RunSubmit
            | Permission::RunControl
    )
}

pub(super) fn validate_archive(manifest: &ExtensionManifest, bytes: &[u8]) -> ExtensionResult<()> {
    if manifest.tools().is_empty() {
        return Ok(());
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    for tool in manifest.tools() {
        let mut read = |path: &ExtensionPath| -> ExtensionResult<Vec<u8>> {
            let mut file = archive.by_name(path.as_str()).map_err(|_| {
                ExtensionError::InvalidArchive(format!(
                    "External tool schema is missing: {}",
                    path.as_str()
                ))
            })?;
            if file.is_dir() || file.size() > MAX_SCHEMA_BYTES as u64 {
                return Err(ExtensionError::InvalidArchive(
                    "Invalid external tool schema file".into(),
                ));
            }
            let mut bytes = Vec::new();
            file.by_ref()
                .take((MAX_SCHEMA_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            if bytes.len() > MAX_SCHEMA_BYTES {
                return Err(ExtensionError::InvalidArchive(
                    "External tool schema exceeds size limit".into(),
                ));
            }
            Ok(bytes)
        };
        let input = read(&tool.input_schema)?;
        let output = tool.output_schema.as_ref().map(&mut read).transpose()?;
        tool.descriptor(manifest.id(), &input, output.as_deref())?;
    }
    Ok(())
}

/// Implemented by host-owned plugin adapters. The service verifies manifest,
/// permissions, scopes and lifecycle first; adapters verify indirect objects
/// and use their existing business service, never the admin action endpoint.
#[async_trait::async_trait]
pub(crate) trait ExternalToolHandler: Send + Sync {
    /// Host-compiled declarations are the authority for native side effects
    /// and minimum permissions. Downloaded metadata cannot downgrade them.
    /// None keeps this handler unavailable to external callers.
    fn trusted_manifest(&self) -> Option<&'static str> {
        None
    }
    async fn authorize(
        &self,
        context: &InvocationContext,
        _action: &str,
        _arguments: &Value,
    ) -> Result<(), McpError> {
        context.check_active()
    }
    async fn call(
        &self,
        context: InvocationContext,
        action: &str,
        arguments: Value,
    ) -> Result<ToolResult, McpError>;
}

pub(super) fn validate_native_contract(
    tool: &ToolDeclaration,
    trusted: &ExtensionManifest,
) -> ExtensionResult<()> {
    let expected = trusted.tools().iter().find(|expected| expected.tool_id == tool.tool_id && expected.action == tool.action)
        .ok_or_else(|| ExtensionError::CallRejected("External action is not declared by the host implementation; update the plugin or host".into()))?;
    if expected.effect != tool.effect
        || expected
            .required_permissions
            .iter()
            .any(|permission| !tool.required_permissions.allows(permission))
        || !expected.required_scopes.is_subset(&tool.required_scopes)
        || expected.context != tool.context
    {
        return Err(ExtensionError::CallRejected("External tool declaration weakens or conflicts with the host action contract; update the plugin".into()));
    }
    Ok(())
}

pub(super) struct PluginToolProvider {
    pub service: Weak<ExtensionService>,
    pub id: ExtensionId,
}
#[async_trait::async_trait]
impl ToolProvider for PluginToolProvider {
    fn tools(&self) -> Vec<ToolDescriptor> {
        self.service
            .upgrade()
            .and_then(|service| service.external_tool_descriptors(&self.id).ok())
            .unwrap_or_default()
    }
    async fn authorize(
        &self,
        context: &InvocationContext,
        name: &str,
        arguments: &Value,
    ) -> Result<(), McpError> {
        self.service
            .upgrade()
            .ok_or_else(|| McpError::internal("Extension host is unavailable"))?
            .authorize_external_tool(context.clone(), &self.id, name, arguments)
            .await
    }
    async fn call(
        &self,
        context: InvocationContext,
        name: &str,
        arguments: Value,
    ) -> Result<ToolResult, McpError> {
        self.service
            .upgrade()
            .ok_or_else(|| McpError::internal("Extension host is unavailable"))?
            .call_extension_from_external(context, &self.id, name, arguments)
            .await
    }
}

pub(super) struct AllPluginTools {
    pub service: Weak<ExtensionService>,
}
impl AllPluginTools {
    fn owner(&self, name: &str) -> Result<(Arc<ExtensionService>, ExtensionId), McpError> {
        let service = self
            .service
            .upgrade()
            .ok_or_else(|| McpError::internal("Extension host unavailable"))?;
        for snapshot in service.list().map_err(error)? {
            if snapshot
                .manifest()
                .tools()
                .iter()
                .any(|tool| tool.name(snapshot.id()) == name)
            {
                return Ok((service, snapshot.id().clone()));
            }
        }
        Err(McpError::forbidden("Unknown plugin tool"))
    }
}
#[async_trait::async_trait]
impl ToolProvider for AllPluginTools {
    fn tools(&self) -> Vec<ToolDescriptor> {
        let Some(service) = self.service.upgrade() else {
            return vec![];
        };
        service
            .list()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|snapshot| {
                service
                    .external_tool_descriptors(snapshot.id())
                    .unwrap_or_default()
            })
            .collect()
    }
    fn tools_for_principal(
        &self,
        principal: &crate::mcp::ExternalPrincipal,
    ) -> Vec<ToolDescriptor> {
        let Some(service) = self.service.upgrade() else {
            return vec![];
        };
        service
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter(|snapshot| {
                principal
                    .allowed
                    .authorize("plugin", snapshot.id().as_str())
                    .is_ok()
            })
            .flat_map(|snapshot| {
                service
                    .external_tool_descriptors(snapshot.id())
                    .unwrap_or_default()
            })
            .collect()
    }
    async fn authorize(
        &self,
        context: &InvocationContext,
        name: &str,
        arguments: &Value,
    ) -> Result<(), McpError> {
        let (service, id) = self.owner(name)?;
        service
            .authorize_external_tool(context.clone(), &id, name, arguments)
            .await
    }
    async fn call(
        &self,
        context: InvocationContext,
        name: &str,
        arguments: Value,
    ) -> Result<ToolResult, McpError> {
        let (service, id) = self.owner(name)?;
        service
            .call_extension_from_external(context, &id, name, arguments)
            .await
    }
}

pub(super) fn error(error: ExtensionError) -> McpError {
    McpError::new(
        crate::mcp::ErrorKind::Unavailable,
        "plugin_unavailable",
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn tools_are_explicit_and_permission_claims_are_bounded_by_the_manifest() {
        let base="manifest_version=2\nid='com.example.tools'\nversion='1.0.0'\nname='Tools'\nentry='plugin.wasm'\npermissions=['resource.read']\n";
        assert!(super::super::parse_manifest(base.as_bytes())
            .unwrap()
            .tools()
            .is_empty());
        let tool="\n[[tools]]\ntool_id='read'\naction='read'\ndescription='Read a resource'\ninput_schema='schemas/read.json'\nrequired_scopes=['resource.read']\nrequired_permissions=['resource.read']\neffect='read'\n";
        let parsed = super::super::parse_manifest(format!("{base}{tool}").as_bytes()).unwrap();
        assert_eq!(
            parsed.tools()[0].name(parsed.id()),
            "gamer.plugin.com.example.tools.read"
        );
        assert!(super::super::parse_manifest(
            format!(
                "{base}{}",
                tool.replace(
                    "required_permissions=['resource.read']",
                    "required_permissions=['input.tap']"
                )
            )
            .as_bytes()
        )
        .is_err());
        assert!(super::super::parse_manifest(format!("{base}{tool}{tool}").as_bytes()).is_err());
        let read_claiming_write = format!("{base}{tool}")
            .replace(
                "permissions=['resource.read']",
                "permissions=['resource.read','resource.write']",
            )
            .replace(
                "required_permissions=['resource.read']",
                "required_permissions=['resource.write']",
            );
        assert!(
            super::super::parse_manifest(read_claiming_write.as_bytes()).is_err(),
            "Read effect cannot bypass durable receipts by declaring a write permission"
        );
        assert!(super::super::parse_manifest(
            format!(
                "{base}{}",
                tool.replace("schemas/read.json", "../private.json")
            )
            .as_bytes()
        )
        .is_err());
    }

    #[test]
    fn all_official_tool_manifests_and_bundled_schemas_pass_real_host_validation() {
        let plugins = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins");
        for id in [
            "gamer-yaml",
            "gamer-video",
            "gamer-keymap",
            "gamer-ocr",
            "gamer-live",
        ] {
            let root = plugins.join(id);
            let bytes = std::fs::read(root.join("manifest.toml")).unwrap();
            let manifest = super::super::parse_manifest(&bytes)
                .unwrap_or_else(|error| panic!("{id}: {error}"));
            assert!(
                !manifest.tools().is_empty(),
                "{id} must explicitly declare its external tools"
            );
            let mut schemas = BTreeSet::new();
            for tool in manifest.tools() {
                schemas.insert(tool.input_schema.as_str());
                if let Some(output) = &tool.output_schema {
                    schemas.insert(output.as_str());
                }
            }
            let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            let options = zip::write::SimpleFileOptions::default();
            archive.start_file("manifest.toml", options).unwrap();
            archive.write_all(&bytes).unwrap();
            for schema in schemas {
                archive.start_file(schema, options).unwrap();
                archive
                    .write_all(&std::fs::read(root.join(schema)).unwrap())
                    .unwrap();
            }
            validate_archive(&manifest, &archive.finish().unwrap().into_inner())
                .unwrap_or_else(|error| panic!("{id}: {error}"));
        }
    }

    struct FixtureBuiltin;
    #[async_trait::async_trait]
    impl crate::extensions::BuiltinService for FixtureBuiltin {
        fn extension_id(&self) -> &str {
            "gamer-video"
        }
        async fn call(&self, _: &str, _: Value) -> super::super::ExtensionResult<Value> {
            Err(ExtensionError::CallRejected(
                "Management actions are unrelated to external tools".into(),
            ))
        }
        async fn stop(&self) {}
    }
    struct BlockingTool {
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }
    #[async_trait::async_trait]
    impl ExternalToolHandler for BlockingTool {
        fn trusted_manifest(&self) -> Option<&'static str> {
            Some(FIXTURE_MANIFEST)
        }
        async fn call(
            &self,
            context: InvocationContext,
            _: &str,
            arguments: Value,
        ) -> Result<ToolResult, McpError> {
            context.authorize_package(arguments["package_id"].as_str().unwrap())?;
            self.started.notify_one();
            self.release.notified().await;
            Ok(ToolResult::json(serde_json::json!({"ok":true})))
        }
    }
    const FIXTURE_MANIFEST: &str = r#"manifest_version=2
id='gamer-video'
version='1.0.0'
name='External tool fixture'
permissions=['resource.read']
[execution]
kind='builtin'
builtin_id='gamer-video'
[[tools]]
tool_id='read'
action='read'
description='Synthetic scoped read'
input_schema='schemas/read.json'
required_scopes=['fixture.read']
required_permissions=['resource.read']
effect='read'
[tools.context]
package_field='package_id'
"#;
    fn fixture_archive(include_schema: bool) -> Vec<u8> {
        fixture_archive_with_manifest(FIXTURE_MANIFEST, include_schema)
    }
    fn fixture_archive_with_manifest(manifest: &str, include_schema: bool) -> Vec<u8> {
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file("manifest.toml", options).unwrap();
        archive.write_all(manifest.as_bytes()).unwrap();
        if include_schema {
            archive.start_file("schemas/read.json", options).unwrap();
            archive.write_all(br#"{"type":"object","properties":{"package_id":{"type":"string"}},"required":["package_id"],"additionalProperties":false}"#).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    #[test]
    fn native_contract_rejects_permission_effect_scope_and_context_downgrades() {
        let manifest = super::super::parse_manifest(FIXTURE_MANIFEST.as_bytes()).unwrap();
        let expected = manifest.tools()[0].clone();
        validate_native_contract(&expected, &manifest).unwrap();
        let mut weakened = expected.clone();
        weakened.required_permissions = PermissionSet::default();
        assert!(validate_native_contract(&weakened, &manifest).is_err());
        weakened = expected.clone();
        weakened.effect = ToolEffect::ResourceWrite;
        assert!(validate_native_contract(&weakened, &manifest).is_err());
        weakened = expected.clone();
        weakened.required_scopes.clear();
        assert!(validate_native_contract(&weakened, &manifest).is_err());
        weakened = expected.clone();
        weakened.context = ContextRequirements::default();
        assert!(validate_native_contract(&weakened, &manifest).is_err());
        weakened = expected;
        weakened.action = "undeclared".into();
        assert!(validate_native_contract(&weakened, &manifest).is_err());
    }

    #[tokio::test]
    async fn native_contract_is_enforced_for_installation_and_existing_metadata() {
        use crate::capabilities::CapabilityRegistry;
        let directory = tempfile::tempdir().unwrap();
        let service =
            ExtensionService::for_data_root(directory.path(), CapabilityRegistry::default());
        let id = ExtensionId::parse("gamer-video").unwrap();
        let tool = Arc::new(BlockingTool {
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        service.register_external_handler(&id, tool).unwrap();
        let weakened = FIXTURE_MANIFEST
            .replace(
                "required_permissions=['resource.read']",
                "required_permissions=[]",
            )
            .replace("permissions=['resource.read']", "permissions=[]");
        let archive = fixture_archive_with_manifest(&weakened, true);
        assert!(
            service.install(&archive).await.is_err(),
            "A token scope cannot replace the actual plugin permission requirement"
        );
        // Seed synthetic metadata as if it had been installed before this host
        // version. Runtime discovery must recheck, not trust install history.
        service.store().install_archive(&archive).unwrap();
        service.enable(&id).await.unwrap();
        service.start(&id).await.unwrap();
        assert!(service.external_tool_descriptors(&id).is_err());
    }

    #[tokio::test]
    async fn dynamic_catalog_checks_schemas_scope_and_actual_operation_lifetime() {
        use crate::{
            capabilities::CapabilityRegistry,
            mcp::{AllowedObjects, CreateTokenRequest, McpService, ServerInfo},
        };
        let directory = tempfile::tempdir().unwrap();
        let service = Arc::new(
            ExtensionService::for_data_root(directory.path(), CapabilityRegistry::default())
                .with_builtin_service(Arc::new(FixtureBuiltin)),
        );
        let tool = Arc::new(BlockingTool {
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let id = ExtensionId::parse("gamer-video").unwrap();
        service
            .register_external_handler(&id, tool.clone())
            .unwrap();
        let mcp = Arc::new(
            McpService::open(
                directory.path(),
                ServerInfo {
                    name: "fixture".into(),
                    version: "0".into(),
                },
            )
            .unwrap(),
        );
        mcp.register_provider("extensions", service.external_tools_provider())
            .unwrap();
        let token = mcp
            .credentials()
            .create(CreateTokenRequest {
                label: "fixture".into(),
                scopes: ["fixture.read".into()].into_iter().collect(),
                allowed: AllowedObjects {
                    packages: ["package-a".into()].into_iter().collect(),
                    plugins: ["gamer-video".into()].into_iter().collect(),
                    ..Default::default()
                },
                expires_in_seconds: 60,
            })
            .unwrap();
        let principal = mcp.credentials().authenticate(&token.token).unwrap();
        assert!(mcp.list_tools(&principal, None).unwrap()["tools"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(
            service.install(&fixture_archive(false)).await.is_err(),
            "Missing declared JSON schema must reject installation"
        );
        service.install(&fixture_archive(true)).await.unwrap();
        service.enable(&id).await.unwrap();
        service.start(&id).await.unwrap();
        assert_eq!(
            mcp.list_tools(&principal, None).unwrap()["tools"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let name = "gamer.plugin.gamer-video.read";
        assert!(mcp
            .call_tool(
                principal.clone(),
                name,
                serde_json::json!({"package_id":"package-b"})
            )
            .await
            .is_err());
        let request = tokio::spawn({
            let mcp = mcp.clone();
            async move {
                mcp.call_tool(
                    principal,
                    name,
                    serde_json::json!({"package_id":"package-a"}),
                )
                .await
            }
        });
        tool.started.notified().await;
        request.abort(); // Transport cancellation cannot release a still executing plugin operation.
        let disable = tokio::spawn({
            let service = service.clone();
            let id = id.clone();
            async move { service.disable(&id).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        assert!(
            !disable.is_finished(),
            "disable must wait for the actual handler to finish"
        );
        tool.release.notify_one();
        disable.await.unwrap().unwrap();
        let principal = mcp.credentials().authenticate(&token.token).unwrap();
        assert!(mcp.list_tools(&principal, None).unwrap()["tools"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}
