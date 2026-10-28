//! Transport-neutral contracts used by Core and plugin tool providers.
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    credentials::{CredentialStore, ExternalPrincipal},
    receipts::{ObjectKind, ObjectScope, ReceiptStore},
};

pub(crate) const PROTOCOL_VERSION: &str = "2025-11-25";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    Unauthorized,
    Forbidden,
    InvalidParams,
    NotFound,
    Conflict,
    Unavailable,
    Internal,
}

#[derive(Clone, Debug)]
pub(crate) struct McpError {
    pub kind: ErrorKind,
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl McpError {
    pub(crate) fn new(kind: ErrorKind, code: &str, message: impl Into<String>) -> Self {
        Self {
            kind,
            code: code.into(),
            message: message.into(),
            retryable: false,
        }
    }
    pub(crate) fn unauthorized() -> Self {
        Self::new(
            ErrorKind::Unauthorized,
            "credential_invalid",
            "MCP credential is invalid, expired or revoked",
        )
    }
    pub(crate) fn forbidden(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Forbidden, "scope_denied", message)
    }
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidParams, "invalid_arguments", message)
    }
    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, "internal_error", message)
    }
    pub(crate) fn unknown_operation() -> Self {
        Self::new(ErrorKind::Conflict, "operation_status_unknown", "The operation may already have executed; do not replay it. Inspect its existing result or ask the user to reconcile it")
    }
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for McpError {}

/// Effects select authorization and durable receipt behavior. An annotation
/// supplied by a client or guest can never turn a write into a read.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolEffect {
    Read,
    DraftWrite,
    ResourceWrite,
    MediaWrite,
    Input,
    RunSubmit,
    TaskWrite,
    ExternalSend,
}
impl ToolEffect {
    pub(crate) fn is_read_only(self) -> bool {
        self == Self::Read
    }
}

/// These fields are selectors, not identities. Providers must also authorize
/// objects obtained indirectly, such as the device belonging to a session.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ContextRequirements {
    pub device_field: Option<String>,
    pub package_field: Option<String>,
    pub plugin_field: Option<String>,
    pub runner_field: Option<String>,
    pub run_field: Option<String>,
    pub task_field: Option<String>,
    pub media_field: Option<String>,
    pub control_session_field: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(default)]
    pub required_scopes: BTreeSet<String>,
    pub effect: ToolEffect,
    #[serde(default)]
    pub context: ContextRequirements,
}

impl ToolDescriptor {
    pub(crate) fn wire(&self) -> Value {
        let mut value = json!({"name":self.name,"description":self.description,"inputSchema":self.input_schema,
            "annotations":{"readOnlyHint":self.effect.is_read_only()}});
        if let Some(schema) = &self.output_schema {
            value["outputSchema"] = schema.clone();
        }
        value
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct ToolResult {
    pub content: Vec<Value>,
    #[serde(
        rename = "structuredContent",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub structured_content: Option<Value>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

impl ToolResult {
    pub(crate) fn json(value: Value) -> Self {
        let value = if value.is_object() {
            value
        } else {
            json!({"value":value})
        };
        Self {
            content: vec![json!({"type":"text","text":value.to_string()})],
            structured_content: Some(value),
            is_error: false,
        }
    }
    pub(crate) fn error(error: &McpError) -> Self {
        let value = json!({"code":error.code,"message":error.message,"retryable":error.retryable});
        Self {
            content: vec![json!({"type":"text","text":value.to_string()})],
            structured_content: Some(value),
            is_error: true,
        }
    }
}

/// Created by the authenticated dispatcher and handed to providers out of
/// band. No JSON argument can create an InvocationContext.
#[derive(Clone)]
pub(crate) struct InvocationContext {
    pub principal: ExternalPrincipal,
    pub tool_name: String,
    pub operation_id: Option<String>,
    pub cancelled: Arc<AtomicBool>,
    /// A caller-selected ID. The control service must verify its actual owner.
    pub control_session: Option<String>,
    /// Authorized selectors, never inferred from a plugin's startup context.
    pub device_id: Option<String>,
    pub package_id: Option<String>,
    credentials: Arc<CredentialStore>,
    receipts: Arc<ReceiptStore>,
    pub(super) token_work: Arc<super::TokenWorkRegistry>,
}

impl InvocationContext {
    pub(crate) fn new(
        principal: ExternalPrincipal,
        tool_name: String,
        operation_id: Option<String>,
        credentials: Arc<CredentialStore>,
        receipts: Arc<ReceiptStore>,
    ) -> Self {
        Self {
            principal,
            tool_name,
            operation_id,
            cancelled: Arc::new(AtomicBool::new(false)),
            control_session: None,
            device_id: None,
            package_id: None,
            credentials,
            receipts,
            token_work: Arc::new(super::TokenWorkRegistry::default()),
        }
    }
    pub(crate) fn register_temporary_work(
        &self,
        work_id: &str,
        work: Arc<dyn super::TemporaryWork>,
    ) -> Result<super::TokenWorkLease, McpError> {
        self.check_active()?;
        self.token_work.register(
            self.credentials.clone(),
            &self.principal.token_id,
            work_id,
            work,
        )
    }
    pub(crate) fn check_active(&self) -> Result<(), McpError> {
        if self.cancelled.load(Ordering::Acquire)
            || !self.credentials.is_active(&self.principal.token_id)
        {
            return Err(McpError::unauthorized());
        }
        Ok(())
    }
    pub(crate) fn require_scope(&self, scope: &str) -> Result<(), McpError> {
        self.check_active()?;
        if self.principal.has_scope(scope) {
            Ok(())
        } else {
            Err(McpError::forbidden(format!("Required scope: {scope}")))
        }
    }
    pub(crate) fn authorize_device(&self, id: &str) -> Result<(), McpError> {
        self.check_active()?;
        self.principal.allowed.authorize("device", id)
    }
    pub(crate) fn authorize_package(&self, id: &str) -> Result<(), McpError> {
        self.check_active()?;
        self.principal.allowed.authorize("package", id)
    }
    pub(crate) fn authorize_plugin(&self, id: &str) -> Result<(), McpError> {
        self.check_active()?;
        self.principal.allowed.authorize("plugin", id)
    }
    pub(crate) fn authorize_runner(&self, id: &str) -> Result<(), McpError> {
        self.check_active()?;
        self.principal.allowed.authorize("runner", id)
    }
    pub(crate) fn authorize_media(&self, id: &str) -> Result<(), McpError> {
        self.check_active()?;
        self.principal.allowed.authorize("media", id)
    }
    pub(crate) fn authorize_run(&self, id: &str) -> Result<(), McpError> {
        self.authorize_owned(ObjectKind::Run, id)
    }
    pub(crate) fn authorize_task(&self, id: &str) -> Result<(), McpError> {
        self.authorize_owned(ObjectKind::Task, id)
    }
    fn authorize_owned(&self, kind: ObjectKind, id: &str) -> Result<(), McpError> {
        self.check_active()?;
        if let Some(owned) = self.receipts.object(kind, id)? {
            if owned.token_id == self.principal.token_id {
                return self.authorize_object_scope(&owned.scope);
            }
            self.principal.allowed.authorize(kind.as_str(), id)?;
            return self.authorize_object_scope(&owned.scope);
        }
        self.principal.allowed.authorize(kind.as_str(), id)
    }
    pub(crate) fn object_scope(
        &self,
        kind: ObjectKind,
        id: &str,
    ) -> Result<Option<ObjectScope>, McpError> {
        self.authorize_owned(kind, id)?;
        Ok(self.receipts.object(kind, id)?.map(|owned| owned.scope))
    }
    pub(crate) fn authorize_object_scope(&self, scope: &ObjectScope) -> Result<(), McpError> {
        if let Some(id) = &scope.device_id {
            self.authorize_device(id)?;
        }
        if let Some(id) = &scope.package_id {
            self.authorize_package(id)?;
        }
        if let Some(id) = &scope.plugin_id {
            self.authorize_plugin(id)?;
        }
        if let Some(id) = &scope.runner_id {
            self.authorize_runner(id)?;
        }
        self.check_active()
    }
    /// Record only host-confirmed IDs after successful acceptance. A provider
    /// must never record ownership of a caller-supplied existing object.
    pub(crate) fn record_owned_object(
        &self,
        kind: ObjectKind,
        id: &str,
        scope: &ObjectScope,
    ) -> Result<(), McpError> {
        self.authorize_object_scope(scope)?;
        self.receipts
            .record_object(kind, id, &self.principal.token_id, scope)
    }
    /// An accepted independent operation must retain its origin even if the
    /// credential is revoked immediately after submission. Check its frozen
    /// grant, without borrowing a newly active credential or granting new work.
    pub(crate) fn record_delegated_object(
        &self,
        kind: ObjectKind,
        id: &str,
        scope: &ObjectScope,
    ) -> Result<(), McpError> {
        for (kind, id) in [
            ("device", &scope.device_id),
            ("package", &scope.package_id),
            ("plugin", &scope.plugin_id),
            ("runner", &scope.runner_id),
        ] {
            if let Some(id) = id {
                self.principal.allowed.authorize(kind, id)?;
            }
        }
        self.receipts
            .record_object(kind, id, &self.principal.token_id, scope)
    }
}

#[async_trait::async_trait]
pub(crate) trait ToolProvider: Send + Sync {
    /// Return only tools currently available. The dispatcher takes a fresh
    /// snapshot on every list/call; providers still enforce lifecycle leases.
    fn tools(&self) -> Vec<ToolDescriptor>;
    fn tools_for_principal(&self, _principal: &ExternalPrincipal) -> Vec<ToolDescriptor> {
        self.tools()
    }
    /// Recheck lifecycle and indirect object/session identities even when the
    /// dispatcher can return a prior receipt. This hook must not cause effects.
    async fn authorize(
        &self,
        context: &InvocationContext,
        _tool_name: &str,
        _arguments: &Value,
    ) -> Result<(), McpError> {
        context.check_active()
    }
    async fn call(
        &self,
        context: InvocationContext,
        tool_name: &str,
        arguments: Value,
    ) -> Result<ToolResult, McpError>;
}
