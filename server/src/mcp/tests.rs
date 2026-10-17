use super::*;
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct FixtureProvider {
    calls: AtomicUsize,
    enabled: AtomicBool,
    admitted: AtomicBool,
    count: usize,
    effect: ToolEffect,
    required_scope: String,
    device_context: bool,
}
impl FixtureProvider {
    fn new(effect: ToolEffect) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            enabled: AtomicBool::new(true),
            admitted: AtomicBool::new(true),
            count: 1,
            effect,
            required_scope: "test.use".into(),
            device_context: false,
        }
    }
}
#[async_trait::async_trait]
impl ToolProvider for FixtureProvider {
    fn tools(&self) -> Vec<ToolDescriptor> {
        if !self.enabled.load(Ordering::Acquire) {
            return vec![];
        }
        (0..self.count).map(|index|ToolDescriptor {
            name:format!("gamer.core.fixture_{index:03}"),description:"A synthetic counter; never accesses a device".into(),
            input_schema:if self.device_context { json!({"type":"object","properties":{"device_id":{"type":"string"},"value":{"type":"integer"}},"required":["device_id"],"additionalProperties":false}) } else { json!({"type":"object","properties":{"value":{"type":"integer"}},"additionalProperties":false}) },
            output_schema:None,required_scopes:[self.required_scope.clone()].into(),effect:self.effect,
            context:ContextRequirements { device_field:self.device_context.then(||"device_id".into()),..Default::default() },
        }).collect()
    }
    async fn authorize(
        &self,
        context: &InvocationContext,
        _name: &str,
        _arguments: &Value,
    ) -> Result<(), McpError> {
        context.check_active()?;
        if !self.admitted.load(Ordering::Acquire) {
            return Err(McpError::new(
                ErrorKind::Conflict,
                "stale_generation",
                "Synthetic session changed generation",
            ));
        }
        Ok(())
    }
    async fn call(
        &self,
        context: InvocationContext,
        name: &str,
        arguments: Value,
    ) -> Result<ToolResult, McpError> {
        context.check_active()?;
        if !self.enabled.load(Ordering::Acquire) {
            return Err(McpError::new(
                ErrorKind::Unavailable,
                "provider_stopped",
                "Synthetic provider stopped",
            ));
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::json(
            json!({"name":name,"value":arguments.get("value").cloned().unwrap_or(Value::Null)}),
        ))
    }
}
fn new_service(root: &Path) -> McpService {
    McpService::open(
        root,
        ServerInfo {
            name: "gamer".into(),
            version: "test".into(),
        },
    )
    .unwrap()
}
fn create_token(service: &McpService, scopes: &[&str], allowed: AllowedObjects) -> CreatedToken {
    service
        .credentials()
        .create(CreateTokenRequest {
            label: "Synthetic fixture".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            allowed,
            expires_in_seconds: 1000,
        })
        .unwrap()
}

#[tokio::test]
async fn core_protocol_works_without_ai_plugins_or_a_device() {
    let root = tempfile::tempdir().unwrap();
    let service = new_service(root.path());
    let token = create_token(&service, &["test.use"], AllowedObjects::default());
    let initialized=service.handle(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),&token.token).await.unwrap().unwrap();
    assert_eq!(initialized["result"]["serverInfo"]["name"], "gamer");
    assert_eq!(
        initialized["result"]["capabilities"]["tools"]["listChanged"],
        false
    );
    let listed = service
        .handle(
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            &token.token,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(listed["result"]["tools"], json!([]));
    assert!(service
        .handle(
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            &token.token
        )
        .await
        .unwrap()
        .is_none());
    service.revoke_token(&token.metadata.token_id).unwrap();
    assert!(service
        .handle(
            json!({"jsonrpc":"2.0","id":3,"method":"ping"}),
            &token.token
        )
        .await
        .is_err());
}

#[tokio::test]
async fn retries_use_operation_ids_and_keep_the_new_rpc_correlation() {
    let root = tempfile::tempdir().unwrap();
    let service = new_service(root.path());
    let provider = Arc::new(FixtureProvider::new(ToolEffect::DraftWrite));
    service
        .register_provider("fixture", provider.clone())
        .unwrap();
    let token = create_token(&service, &["test.use"], AllowedObjects::default());
    let rpc = |id, value| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"gamer.core.fixture_000","arguments":{"operation_id":"same-operation","value":value}}});
    let first = service
        .handle(rpc(1, 1), &token.token)
        .await
        .unwrap()
        .unwrap();
    let retried = service
        .handle(rpc(2, 1), &token.token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first["result"], retried["result"]);
    assert_eq!(retried["id"], 2);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let conflict = service
        .handle(rpc(3, 2), &token.token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        conflict["result"]["structuredContent"]["code"],
        "operation_conflict"
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let missing=service.handle(json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"gamer.core.fixture_000","arguments":{"value":1}}}),&token.token).await.unwrap().unwrap();
    assert_eq!(missing["result"]["isError"], true);
}

#[tokio::test]
async fn cached_receipts_do_not_bypass_current_session_admission() {
    let root = tempfile::tempdir().unwrap();
    let service = new_service(root.path());
    let provider = Arc::new(FixtureProvider::new(ToolEffect::Input));
    service
        .register_provider("fixture", provider.clone())
        .unwrap();
    let token = create_token(&service, &["test.use"], AllowedObjects::default());
    let principal = service.credentials().authenticate(&token.token).unwrap();
    let arguments = json!({"operation_id":"tap-once"});
    service
        .call_tool(
            principal.clone(),
            "gamer.core.fixture_000",
            arguments.clone(),
        )
        .await
        .unwrap();
    provider.admitted.store(false, Ordering::Release);
    let error = service
        .call_tool(principal, "gamer.core.fixture_000", arguments)
        .await
        .unwrap_err();
    assert_eq!(error.code, "stale_generation");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cached_receipts_are_not_disclosed_after_revocation_during_awaited_preflight() {
    struct AwaitedPreflight {
        fixture: FixtureProvider,
        pause: AtomicBool,
        entered: tokio::sync::Notify,
        resume: tokio::sync::Notify,
    }
    #[async_trait::async_trait]
    impl ToolProvider for AwaitedPreflight {
        fn tools(&self) -> Vec<ToolDescriptor> {
            self.fixture.tools()
        }
        async fn authorize(
            &self,
            context: &InvocationContext,
            _: &str,
            _: &Value,
        ) -> Result<(), McpError> {
            context.check_active()?;
            if self.pause.load(Ordering::Acquire) {
                self.entered.notify_one();
                self.resume.notified().await;
            }
            Ok(())
        }
        async fn call(
            &self,
            context: InvocationContext,
            name: &str,
            arguments: Value,
        ) -> Result<ToolResult, McpError> {
            self.fixture.call(context, name, arguments).await
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let service = Arc::new(new_service(directory.path()));
    let provider = Arc::new(AwaitedPreflight {
        fixture: FixtureProvider::new(ToolEffect::DraftWrite),
        pause: AtomicBool::new(false),
        entered: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    });
    service
        .register_provider("fixture", provider.clone())
        .unwrap();
    let token = create_token(&service, &["test.use"], AllowedObjects::default());
    let principal = service.credentials().authenticate(&token.token).unwrap();
    let arguments = json!({"operation_id":"cached-private-result", "value":7});
    service
        .call_tool(
            principal.clone(),
            "gamer.core.fixture_000",
            arguments.clone(),
        )
        .await
        .unwrap();
    provider.pause.store(true, Ordering::Release);
    let pending = tokio::spawn({
        let service = service.clone();
        async move {
            service
                .call_tool(principal, "gamer.core.fixture_000", arguments)
                .await
        }
    });
    provider.entered.notified().await;
    service.revoke_token(&token.metadata.token_id).unwrap();
    provider.resume.notify_one();
    assert_eq!(
        pending.await.unwrap().unwrap_err().kind,
        ErrorKind::Unauthorized
    );
    assert_eq!(provider.fixture.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn pending_reservation_after_restart_never_executes_again() {
    let root = tempfile::tempdir().unwrap();
    let service = new_service(root.path());
    let token = create_token(&service, &["test.use"], AllowedObjects::default());
    let args = json!({"operation_id":"crashed-operation","value":1});
    let key = ReceiptKey {
        token_id: token.metadata.token_id.clone(),
        tool_name: "gamer.core.fixture_000".into(),
        object_key: "{}".into(),
        operation_id: "crashed-operation".into(),
    };
    assert!(matches!(
        service.receipts().reserve(&key, &hash_json(&args)).unwrap(),
        receipts::Admission::New
    ));
    drop(service);
    let service = new_service(root.path());
    let provider = Arc::new(FixtureProvider::new(ToolEffect::RunSubmit));
    service
        .register_provider("fixture", provider.clone())
        .unwrap();
    let principal = service.credentials().authenticate(&token.token).unwrap();
    let error = service
        .call_tool(principal, "gamer.core.fixture_000", args)
        .await
        .unwrap_err();
    assert_eq!(error.code, "operation_status_unknown");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn scope_device_and_plugin_grants_are_checked_for_lists_and_guessed_calls() {
    let root = tempfile::tempdir().unwrap();
    let service = new_service(root.path());
    let mut provider = FixtureProvider::new(ToolEffect::Read);
    provider.device_context = true;
    let provider = Arc::new(provider);
    service
        .register_plugin_provider("fixture-plugin", provider.clone())
        .unwrap();
    let token = create_token(
        &service,
        &["test.use"],
        AllowedObjects {
            devices: ["device-a".into()].into(),
            ..Default::default()
        },
    );
    let principal = service.credentials().authenticate(&token.token).unwrap();
    assert_eq!(
        service.list_tools(&principal, None).unwrap()["tools"],
        json!([])
    );
    assert!(service
        .call_tool(
            principal,
            "gamer.core.fixture_000",
            json!({"device_id":"device-a"})
        )
        .await
        .is_err());
    let token = create_token(
        &service,
        &["test.use"],
        AllowedObjects {
            devices: ["device-a".into()].into(),
            plugins: ["fixture-plugin".into()].into(),
            ..Default::default()
        },
    );
    let principal = service.credentials().authenticate(&token.token).unwrap();
    assert_eq!(
        service.list_tools(&principal, None).unwrap()["tools"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(service
        .call_tool(
            principal,
            "gamer.core.fixture_000",
            json!({"device_id":"device-b"})
        )
        .await
        .is_err());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn cursors_are_bound_to_current_catalog_and_principal() {
    let root = tempfile::tempdir().unwrap();
    let service = new_service(root.path());
    let mut provider = FixtureProvider::new(ToolEffect::Read);
    provider.count = 51;
    let provider = Arc::new(provider);
    service
        .register_provider("fixture", provider.clone())
        .unwrap();
    let token = create_token(&service, &["test.use"], AllowedObjects::default());
    let principal = service.credentials().authenticate(&token.token).unwrap();
    let first = service.list_tools(&principal, None).unwrap();
    assert_eq!(first["tools"].as_array().unwrap().len(), 50);
    let cursor = first["nextCursor"].as_str().unwrap();
    let second = service.list_tools(&principal, Some(cursor)).unwrap();
    assert_eq!(second["tools"].as_array().unwrap().len(), 1);
    assert!(second.get("nextCursor").is_none());
    let other = create_token(&service, &["test.use"], AllowedObjects::default());
    let other = service.credentials().authenticate(&other.token).unwrap();
    assert!(service.list_tools(&other, Some(cursor)).is_err());
    provider.enabled.store(false, Ordering::Release);
    assert_eq!(
        service
            .list_tools(&principal, Some(cursor))
            .unwrap_err()
            .code,
        "catalog_changed"
    );
}

#[test]
fn registered_schemas_are_offline_and_side_effects_declare_operation_ids() {
    let mut descriptor = FixtureProvider::new(ToolEffect::Input).tools().remove(0);
    let normalized = normalize_descriptor(descriptor.clone()).unwrap();
    assert!(normalized.input_schema["required"]
        .as_array()
        .unwrap()
        .contains(&json!("operation_id")));
    descriptor.input_schema =
        json!({"type":"object","properties":{"value":{"$ref":"https://example.invalid/schema"}}});
    assert!(normalize_descriptor(descriptor).is_err());
}

#[test]
fn run_and_task_authority_defaults_to_recorded_ownership() {
    let root = tempfile::tempdir().unwrap();
    let service = new_service(root.path());
    let allowed = AllowedObjects {
        devices: ["d".into()].into(),
        packages: ["p".into()].into(),
        plugins: ["yaml".into()].into(),
        runners: ["yaml".into()].into(),
        ..Default::default()
    };
    let first = create_token(&service, &["run.read"], allowed.clone());
    let second = create_token(&service, &["run.read"], allowed);
    let context = |created: &CreatedToken| {
        InvocationContext::new(
            service.credentials().authenticate(&created.token).unwrap(),
            "test".into(),
            None,
            service.credentials(),
            service.receipts(),
        )
    };
    let a = context(&first);
    let b = context(&second);
    let scope = ObjectScope {
        device_id: Some("d".into()),
        package_id: Some("p".into()),
        plugin_id: Some("yaml".into()),
        runner_id: Some("yaml".into()),
    };
    a.record_owned_object(ObjectKind::Run, "r", &scope).unwrap();
    assert!(a.authorize_run("r").is_ok());
    assert!(b.authorize_run("r").is_err());
    assert!(a.authorize_task("existing-user-task").is_err());
    service.revoke_token(&first.metadata.token_id).unwrap();
    assert!(a.authorize_run("r").is_err());
    let explicit = create_token(
        &service,
        &["run.read"],
        AllowedObjects {
            runs: BTreeSet::from(["existing-user-run".into()]),
            ..Default::default()
        },
    );
    assert!(context(&explicit)
        .authorize_run("existing-user-run")
        .is_ok());
}
