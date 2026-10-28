//! Core MCP facade. Protocol, credentials, receipts and tool routing depend on
//! no model provider, game syntax, plugin implementation or device connection.
mod credentials;
mod protocol;
mod receipts;
mod schema;
pub(crate) mod transport;
mod types;
mod work;
pub(crate) use work::{TemporaryWork, TokenWorkLease, TokenWorkRegistry};

#[cfg(test)]
pub(crate) use credentials::CreatedToken;
pub(crate) use credentials::{
    AllowedObjects, CreateTokenRequest, CredentialStore, ExternalPrincipal,
};
pub(crate) use receipts::{ObjectKind, ObjectScope, ReceiptKey, ReceiptStore};
pub(crate) use types::{
    ContextRequirements, ErrorKind, InvocationContext, McpError, ServerInfo, ToolDescriptor,
    ToolEffect, ToolProvider, ToolResult, PROTOCOL_VERSION,
};

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, RwLock},
};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const PAGE_SIZE: usize = 50;
const MAX_ARGUMENT_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct McpService {
    info: ServerInfo,
    credentials: Arc<CredentialStore>,
    receipts: Arc<ReceiptStore>,
    providers: RwLock<BTreeMap<String, RegisteredProvider>>,
    token_work: Arc<TokenWorkRegistry>,
}

struct CatalogEntry {
    descriptor: ToolDescriptor,
    provider: Arc<dyn ToolProvider>,
}
struct RegisteredProvider {
    provider: Arc<dyn ToolProvider>,
    owner_plugin: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    token_id: String,
    revision: String,
    offset: usize,
}

impl McpService {
    /// `data_root` is the configured Core data root, not a plugin directory.
    pub(crate) fn open(data_root: &Path, info: ServerInfo) -> Result<Self, McpError> {
        let directory = data_root.join("mcp");
        Ok(Self {
            info,
            credentials: Arc::new(CredentialStore::open(&directory)?),
            receipts: Arc::new(ReceiptStore::open(&directory)?),
            providers: RwLock::new(BTreeMap::new()),
            token_work: Arc::new(TokenWorkRegistry::default()),
        })
    }
    pub(crate) fn credentials(&self) -> Arc<CredentialStore> {
        self.credentials.clone()
    }
    #[cfg(test)]
    pub(crate) fn receipts(&self) -> Arc<ReceiptStore> {
        self.receipts.clone()
    }
    pub(crate) fn revoke_token(&self, id: &str) -> Result<bool, McpError> {
        self.credentials.revoke(id)
    }
    pub(crate) async fn drain_token_work(&self, id: &str) {
        self.token_work.drain_token(id).await;
    }
    pub(crate) fn register_provider(
        &self,
        id: &str,
        provider: Arc<dyn ToolProvider>,
    ) -> Result<(), McpError> {
        self.register(id, provider, None)
    }
    #[cfg(test)]
    pub(crate) fn register_plugin_provider(
        &self,
        plugin_id: &str,
        provider: Arc<dyn ToolProvider>,
    ) -> Result<(), McpError> {
        self.register(plugin_id, provider, Some(plugin_id.into()))
    }
    fn register(
        &self,
        id: &str,
        provider: Arc<dyn ToolProvider>,
        owner_plugin: Option<String>,
    ) -> Result<(), McpError> {
        if !protocol::valid_tool_name(id) {
            return Err(McpError::invalid("Invalid tool provider ID"));
        }
        let mut providers = self.providers.write().unwrap();
        if providers.contains_key(id) {
            return Err(McpError::new(
                ErrorKind::Conflict,
                "provider_conflict",
                "Tool provider is already registered",
            ));
        }
        let mut names = std::collections::BTreeSet::new();
        for current in providers
            .values()
            .map(|entry| &entry.provider)
            .chain(std::iter::once(&provider))
        {
            for descriptor in current.tools() {
                let descriptor = normalize_descriptor(descriptor)?;
                if !names.insert(descriptor.name) {
                    return Err(McpError::new(
                        ErrorKind::Conflict,
                        "tool_conflict",
                        "Duplicate tool name",
                    ));
                }
            }
        }
        providers.insert(
            id.into(),
            RegisteredProvider {
                provider,
                owner_plugin,
            },
        );
        Ok(())
    }
    /// HTTP code owns loopback/Host/Origin/content-type checks. Bearer
    /// credentials are independently revalidated for every protocol message.
    pub(crate) async fn handle(
        &self,
        request: Value,
        bearer: &str,
    ) -> Result<Option<Value>, McpError> {
        let principal = self.credentials.authenticate(bearer)?;
        let response = match protocol::parse(request) {
            protocol::Request::Notification => return Ok(None),
            protocol::Request::Error(response) => response,
            protocol::Request::Initialize { id } => protocol::success(
                id,
                json!({"protocolVersion":PROTOCOL_VERSION,"serverInfo":self.info,"capabilities":{"tools":{"listChanged":false}},"instructions":"Use the declared tool schemas. Device control must first be started and resumed by the user in Gamer. Side effects require a distinct operation_id; retry the same operation with the same arguments."}),
            ),
            protocol::Request::Ping { id } => protocol::success(id, json!({})),
            protocol::Request::ToolsList { id, cursor } => {
                match self.list_tools(&principal, cursor.as_deref()) {
                    Ok(page) => protocol::success(id, page),
                    Err(error) => protocol::error(
                        id,
                        if error.kind == ErrorKind::InvalidParams {
                            -32602
                        } else {
                            -32603
                        },
                        error.message,
                    ),
                }
            }
            protocol::Request::ToolsCall {
                id,
                name,
                arguments,
            } => {
                let result = self
                    .call_tool(principal, &name, arguments)
                    .await
                    .unwrap_or_else(|error| ToolResult::error(&error));
                protocol::success(
                    id,
                    serde_json::to_value(result)
                        .map_err(|_| McpError::internal("Cannot serialize tool result"))?,
                )
            }
        };
        Ok(Some(response))
    }

    pub(crate) fn list_tools(
        &self,
        principal: &ExternalPrincipal,
        cursor: Option<&str>,
    ) -> Result<Value, McpError> {
        let principal = self.credentials.principal_for_id(&principal.token_id)?;
        let entries = self.catalog(&principal)?;
        let descriptors: Vec<_> = entries
            .values()
            .map(|entry| entry.descriptor.wire())
            .collect();
        // Include the principal's complete grant: a cursor cannot be carried
        // across a different token or a changed authorization/catalogue.
        let revision = hash_json(&json!([principal, descriptors]));
        let offset = match cursor.filter(|value| !value.is_empty()) {
            None => 0,
            Some(raw) => {
                let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(raw)
                    .map_err(|_| McpError::invalid("Invalid tool cursor"))?;
                let cursor: Cursor = serde_json::from_slice(&decoded)
                    .map_err(|_| McpError::invalid("Invalid tool cursor"))?;
                if cursor.token_id != principal.token_id || cursor.revision != revision {
                    return Err(McpError::new(ErrorKind::InvalidParams,"catalog_changed","Tool catalogue or authorization changed; list tools again without a cursor"));
                }
                if cursor.offset > descriptors.len() {
                    return Err(McpError::invalid("Tool cursor offset is out of range"));
                }
                cursor.offset
            }
        };
        let end = (offset + PAGE_SIZE).min(descriptors.len());
        let mut page = json!({"tools":&descriptors[offset..end]});
        if end < descriptors.len() {
            let cursor = serde_json::to_vec(&Cursor {
                token_id: principal.token_id.clone(),
                revision,
                offset: end,
            })
            .map_err(|_| McpError::internal("Cannot serialize tool cursor"))?;
            page["nextCursor"] =
                json!(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(cursor));
        }
        Ok(page)
    }

    pub(crate) async fn call_tool(
        &self,
        principal: ExternalPrincipal,
        name: &str,
        arguments: Value,
    ) -> Result<ToolResult, McpError> {
        // Do not accept a stale principal snapshot even when an adapter calls
        // this facade directly rather than through HTTP.
        let principal = self.credentials.principal_for_id(&principal.token_id)?;
        if !arguments.is_object() {
            return Err(McpError::invalid("Tool arguments must be an object"));
        }
        if serde_json::to_vec(&arguments)
            .map_err(|_| McpError::invalid("Invalid tool arguments"))?
            .len()
            > MAX_ARGUMENT_BYTES
        {
            return Err(McpError::invalid("Tool arguments exceed the size limit"));
        }
        let entries = self.catalog(&principal)?;
        let entry = entries.get(name).ok_or_else(|| {
            McpError::forbidden("Tool is unavailable or is outside the credential's scopes")
        })?;
        schema::validate(&entry.descriptor.input_schema, &arguments)?;
        let operation_id = if entry.descriptor.effect.is_read_only() {
            None
        } else {
            let id = arguments
                .get("operation_id")
                .and_then(Value::as_str)
                .ok_or_else(|| McpError::invalid("operation_id is required for side effects"))?;
            if id.trim().is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
                return Err(McpError::invalid(
                    "operation_id must contain 1 to 128 printable bytes",
                ));
            }
            Some(id.to_owned())
        };
        let mut context = InvocationContext::new(
            principal,
            name.into(),
            operation_id.clone(),
            self.credentials.clone(),
            self.receipts.clone(),
        );
        let object_key = bind_context(&mut context, &entry.descriptor.context, &arguments)?;
        context.token_work = self.token_work.clone();
        entry.provider.authorize(&context, name, &arguments).await?;
        let fingerprint = hash_json(&arguments);
        let receipt = operation_id.map(|operation_id| ReceiptKey {
            token_id: context.principal.token_id.clone(),
            tool_name: name.into(),
            object_key,
            operation_id,
        });
        if let Some(key) = &receipt {
            match self.receipts.reserve(key, &fingerprint)? {
                receipts::Admission::Completed(result) => {
                    // Preflight may await and receipt storage may block. A
                    // cached response must obey the same final credential
                    // check as a freshly executed response.
                    context.check_active()?;
                    return Ok(result);
                }
                receipts::Admission::Pending => return Err(McpError::unknown_operation()),
                receipts::Admission::New => {}
            }
        }
        context.check_active()?;
        tracing::debug!(tool = %context.tool_name, token_id = %context.principal.token_id, "MCP tool admitted");
        let mut result = entry
            .provider
            .call(context.clone(), name, arguments)
            .await
            .unwrap_or_else(|error| ToolResult::error(&error));
        if !result.is_error {
            if let Some(output) = &entry.descriptor.output_schema {
                let validation = match &result.structured_content {
                    Some(value) => schema::validate(output, value),
                    None => Err(McpError::invalid(
                        "Tool declared an output schema but returned no structuredContent",
                    )),
                };
                if validation.is_err() {
                    result=ToolResult::error(&McpError::new(ErrorKind::Internal,"result_contract_violation","The tool executed but returned a result outside its declared schema; do not replay the operation"));
                }
            }
        }
        if let Some(key) = &receipt {
            self.receipts.complete(key, &fingerprint, &result)?;
        }
        // A read must not disclose its result after its credential was revoked
        // while waiting. Independent accepted runs keep their own authority.
        context.check_active()?;
        Ok(result)
    }

    fn catalog(
        &self,
        principal: &ExternalPrincipal,
    ) -> Result<BTreeMap<String, CatalogEntry>, McpError> {
        let providers: Vec<_> = self
            .providers
            .read()
            .unwrap()
            .values()
            .map(|entry| (entry.provider.clone(), entry.owner_plugin.clone()))
            .collect();
        let mut entries = BTreeMap::new();
        let mut all_names = std::collections::BTreeSet::new();
        for (provider, owner_plugin) in providers {
            if owner_plugin
                .as_ref()
                .is_some_and(|plugin| principal.allowed.authorize("plugin", plugin).is_err())
            {
                continue;
            }
            for descriptor in provider.tools_for_principal(principal) {
                let descriptor = normalize_descriptor(descriptor)?;
                if !all_names.insert(descriptor.name.clone()) {
                    return Err(McpError::new(
                        ErrorKind::Conflict,
                        "tool_conflict",
                        "Dynamic tool providers returned conflicting names",
                    ));
                }
                if !principal.has_scopes(&descriptor.required_scopes) {
                    continue;
                }
                if !context_has_allowed_objects(&descriptor.context, &principal.allowed) {
                    continue;
                }
                entries.insert(
                    descriptor.name.clone(),
                    CatalogEntry {
                        descriptor,
                        provider: provider.clone(),
                    },
                );
            }
        }
        Ok(entries)
    }

    pub(crate) fn management_catalog(&self) -> Result<Vec<ToolDescriptor>, McpError> {
        let providers: Vec<_> = self
            .providers
            .read()
            .unwrap()
            .values()
            .map(|entry| entry.provider.clone())
            .collect();
        let mut descriptors = BTreeMap::new();
        for provider in providers {
            for descriptor in provider.tools() {
                let descriptor = normalize_descriptor(descriptor)?;
                if descriptors
                    .insert(descriptor.name.clone(), descriptor)
                    .is_some()
                {
                    return Err(McpError::new(
                        ErrorKind::Conflict,
                        "tool_conflict",
                        "Duplicate tool name",
                    ));
                }
            }
        }
        Ok(descriptors.into_values().collect())
    }
}

pub(crate) fn normalize_descriptor(
    mut descriptor: ToolDescriptor,
) -> Result<ToolDescriptor, McpError> {
    if !protocol::valid_tool_name(&descriptor.name)
        || descriptor.description.trim().is_empty()
        || descriptor.description.len() > 16 * 1024
    {
        return Err(McpError::invalid("Invalid tool name or description"));
    }
    if descriptor.required_scopes.is_empty() {
        return Err(McpError::invalid(
            "Every externally exposed tool must require at least one explicit scope",
        ));
    }
    if descriptor.required_scopes.iter().any(|scope| {
        scope.is_empty()
            || scope.len() > 128
            || !scope
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':'))
    }) {
        return Err(McpError::invalid("Invalid tool scope declaration"));
    }
    if descriptor.input_schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err(McpError::invalid(
            "Tool input schema must declare type: object",
        ));
    }
    let input = descriptor
        .input_schema
        .as_object_mut()
        .ok_or_else(|| McpError::invalid("Tool input schema must be an object"))?;
    input.entry("additionalProperties").or_insert(json!(false));
    if !descriptor.effect.is_read_only() {
        let properties = input
            .entry("properties")
            .or_insert(json!({}))
            .as_object_mut()
            .ok_or_else(|| McpError::invalid("Tool schema properties must be an object"))?;
        properties.insert("operation_id".into(),json!({"type":"string","minLength":1,"maxLength":128,"description":"Unique ID for this operation. Retry the same operation with the same ID and arguments; use a new ID for a new action."}));
        let required = input
            .entry("required")
            .or_insert(json!([]))
            .as_array_mut()
            .ok_or_else(|| McpError::invalid("Tool schema required must be an array"))?;
        if !required
            .iter()
            .any(|value| value.as_str() == Some("operation_id"))
        {
            required.push(json!("operation_id"));
        }
    }
    if serde_json::to_vec(&descriptor)
        .map_err(|_| McpError::invalid("Invalid tool declaration"))?
        .len()
        > 512 * 1024
    {
        return Err(McpError::invalid("Tool declaration exceeds its size limit"));
    }
    schema::compile(&descriptor.input_schema)?;
    if let Some(output) = &descriptor.output_schema {
        schema::compile(output)?;
    }
    Ok(descriptor)
}

fn context_has_allowed_objects(context: &ContextRequirements, allowed: &AllowedObjects) -> bool {
    [
        (&context.device_field, &allowed.devices),
        (&context.package_field, &allowed.packages),
        (&context.plugin_field, &allowed.plugins),
        (&context.runner_field, &allowed.runners),
        (&context.media_field, &allowed.media),
    ]
    .into_iter()
    .all(|(field, objects)| field.is_none() || !objects.is_empty())
}

pub(crate) fn bind_context(
    context: &mut InvocationContext,
    required: &ContextRequirements,
    arguments: &Value,
) -> Result<String, McpError> {
    let mut selectors = BTreeMap::new();
    for (kind, field) in [
        ("device", &required.device_field),
        ("package", &required.package_field),
        ("plugin", &required.plugin_field),
        ("runner", &required.runner_field),
        ("run", &required.run_field),
        ("task", &required.task_field),
        ("media", &required.media_field),
        ("control_session", &required.control_session_field),
    ] {
        let Some(field) = field else {
            continue;
        };
        let id = arguments
            .get(field)
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| McpError::invalid(format!("Required context selector: {field}")))?;
        match kind {
            "device" => {
                context.authorize_device(id)?;
                context.device_id = Some(id.into());
            }
            "package" => {
                context.authorize_package(id)?;
                context.package_id = Some(id.into());
            }
            "plugin" => context.authorize_plugin(id)?,
            "runner" => context.authorize_runner(id)?,
            "run" => context.authorize_run(id)?,
            "task" => context.authorize_task(id)?,
            "media" => context.authorize_media(id)?,
            "control_session" => context.control_session = Some(id.to_owned()),
            _ => unreachable!(),
        }
        selectors.insert(kind, id);
    }
    serde_json::to_string(&selectors).map_err(|_| McpError::internal("Cannot bind tool context"))
}

pub(crate) fn validate_tool_arguments(schema: &Value, arguments: &Value) -> Result<(), McpError> {
    self::schema::validate(schema, arguments)
}

fn hash_json(value: &Value) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(values) => {
                let sorted: BTreeMap<_, _> = values
                    .iter()
                    .map(|(key, value)| (key.clone(), canonical(value)))
                    .collect();
                json!(sorted)
            }
            Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
            _ => value.clone(),
        }
    }
    format!(
        "{:x}",
        Sha256::digest(canonical(value).to_string().as_bytes())
    )
}

#[cfg(test)]
mod tests;
