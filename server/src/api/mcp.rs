//! Core MCP transport and separately authenticated credential management.
use std::net::SocketAddr;

#[cfg(test)]
use axum::body::Bytes;
use axum::{
    extract::{ConnectInfo, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use super::AppState;
use crate::mcp::{AllowedObjects, CreateTokenRequest, ErrorKind, McpError, McpService};

pub(super) async fn post_message(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    request: axum::extract::Request,
) -> Response {
    process_request(&state.mcp, &state.cfg, peer.map(|peer| peer.0), request).await
}

async fn process_request(
    service: &McpService,
    cfg: &crate::config::Config,
    peer: Option<SocketAddr>,
    request: axum::extract::Request,
) -> Response {
    // Reject disallowed/unauthenticated clients before polling or buffering a body.
    let bearer = match crate::mcp::transport::access(
        request.headers(),
        peer,
        cfg.current_settings().mcp_allow_remote && !cfg.local_only,
    ) {
        Ok(bearer) => bearer,
        Err((status, message)) => return transport_error(status, message),
    };
    if let Err(error) = service.credentials().authenticate(bearer) {
        return mcp_error(error);
    }
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, 16 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return transport_error(StatusCode::PAYLOAD_TOO_LARGE, "MCP body exceeds its limit")
        }
    };
    // Authenticate again after the await, including revocation during upload.
    process_message(service, cfg, peer, &parts.headers, &bytes).await
}

async fn process_message(
    service: &McpService,
    cfg: &crate::config::Config,
    peer: Option<SocketAddr>,
    headers: &HeaderMap,
    bytes: &[u8],
) -> Response {
    let bearer = match crate::mcp::transport::access(
        headers,
        peer,
        cfg.current_settings().mcp_allow_remote && !cfg.local_only,
    ) {
        Ok(bearer) => bearer,
        Err((status, message)) => return transport_error(status, message),
    };
    // Authenticate before parsing so an invalid credential never receives an
    // otherwise successful protocol response, including a parse error.
    if let Err(error) = service.credentials().authenticate(bearer) {
        return mcp_error(error);
    }
    let request: Value =
        match serde_json::from_slice(bytes) {
            Ok(request) => request,
            Err(_) => return Json(
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error"}}),
            )
            .into_response(),
        };
    match service.handle(request, bearer).await {
        Ok(Some(response)) => Json(response).into_response(),
        Ok(None) => StatusCode::ACCEPTED.into_response(),
        Err(error) => mcp_error(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TokenSettingsRequest {
    label: String,
    #[serde(default)]
    expires_in_seconds: u64,
}

/// Credential management belongs only in the admin-authenticated
/// route group; they are never tools on the external MCP surface.
pub(super) async fn api_create_token(
    State(state): State<AppState>,
    Json(request): Json<TokenSettingsRequest>,
) -> Response {
    // Settings credentials cover current and future exposed MCP tools. Object,
    // transport, plugin exposure and execution-policy gates remain independent.
    let scopes = ["*".into()].into();
    let all = ["*".into()].into();
    let allowed = AllowedObjects {
        devices: all,
        packages: ["*".into()].into(),
        plugins: ["*".into()].into(),
        runners: ["*".into()].into(),
        runs: ["*".into()].into(),
        tasks: ["*".into()].into(),
        media: ["*".into()].into(),
    };
    match state.mcp.credentials().create(CreateTokenRequest {
        label: request.label,
        expires_in_seconds: request.expires_in_seconds,
        scopes,
        allowed,
    }) {
        Ok(created) => (
            StatusCode::CREATED,
            [(header::CACHE_CONTROL, "no-store")],
            Json(created),
        )
            .into_response(),
        Err(error) => mcp_error(error),
    }
}
pub(super) async fn api_list_tokens(State(state): State<AppState>) -> Response {
    let credentials = state.mcp.credentials();
    let tokens: Vec<_> = credentials
        .list()
        .into_iter()
        .map(|token| {
            let can_view = credentials.can_reveal(&token.token_id);
            let mut value = serde_json::to_value(token).expect("token metadata is serializable");
            value["can_view"] = can_view.into();
            value
        })
        .collect();
    Json(json!({"tokens":tokens})).into_response()
}
pub(super) async fn api_view_token(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match state.mcp.credentials().reveal(&id) {
        Ok(token) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({"token":token})),
        )
            .into_response(),
        Err(error) => mcp_error(error),
    }
}
pub(super) async fn api_revoke_token(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match state.mcp.revoke_token(&id) {
        Ok(false) => transport_error(StatusCode::NOT_FOUND, "MCP credential not found"),
        Ok(true) => {
            state.mcp.drain_token_work(&id).await;
            state.external_control.revoke(&id).await;
            match state.mcp.credentials().remove_revoked(&id) {
                Ok(()) => Json(json!({"token_id":id,"deleted":true})).into_response(),
                Err(error) => mcp_error(error),
            }
        }
        Err(error) => mcp_error(error),
    }
}
pub(super) async fn api_catalog(State(state): State<AppState>) -> Response {
    match state.mcp.management_catalog() {
        Ok(tools) => {
            let scopes: std::collections::BTreeSet<_> = tools
                .iter()
                .flat_map(|tool| tool.required_scopes.iter().cloned())
                .collect();
            Json(json!({
                "endpoint": format!("http://127.0.0.1:{}/api/mcp", state.cfg.port),
                "endpoint_path": "/api/mcp",
                "remote_allowed": state.cfg.current_settings().mcp_allow_remote,
                "local_only": state.cfg.local_only,
                "protocol_version": crate::mcp::PROTOCOL_VERSION,
                "tools": tools,
                "scopes": scopes,
            }))
            .into_response()
        }
        Err(error) => mcp_error(error),
    }
}
fn mcp_error(error: McpError) -> Response {
    let status = match error.kind {
        ErrorKind::Unauthorized => StatusCode::UNAUTHORIZED,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        ErrorKind::InvalidParams => StatusCode::BAD_REQUEST,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Conflict => StatusCode::CONFLICT,
        ErrorKind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut response = (
        status,
        Json(json!({"error":error.code,"message":error.message})),
    )
        .into_response();
    authenticate_header(&mut response, status);
    response
}
fn transport_error(status: StatusCode, message: &str) -> Response {
    let mut response = (status, Json(json!({"error":message}))).into_response();
    authenticate_header(&mut response, status);
    response
}
fn authenticate_header(response: &mut Response, status: StatusCode) {
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            axum::http::HeaderValue::from_static("Bearer realm=\"Gamer MCP\""),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{AllowedObjects, ServerInfo};
    use axum::{
        body::{to_bytes, Body},
        http::Request,
        routing::post,
        Router,
    };
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn endpoint(
        State(service): State<Arc<McpService>>,
        peer: Option<ConnectInfo<SocketAddr>>,
        request: axum::extract::Request,
    ) -> Response {
        process_request(
            &service,
            &Default::default(),
            peer.map(|peer| peer.0),
            request,
        )
        .await
    }
    fn request(method: &str, bearer: Option<&str>, body: Value) -> Request<Body> {
        let mut request = Request::builder()
            .method(method)
            .uri("/api/mcp")
            .header(header::HOST, "127.0.0.1:8443")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header(header::CONTENT_TYPE, "application/json")
            .header("mcp-protocol-version", crate::mcp::PROTOCOL_VERSION);
        if let Some(bearer) = bearer {
            request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
        }
        let mut request = request.body(Body::from(body.to_string())).unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()));
        request
    }
    #[tokio::test]
    async fn independent_bearer_json_notifications_and_get_405_without_a_host_process() {
        let root = tempfile::tempdir().unwrap();
        let service = Arc::new(
            McpService::open(
                root.path(),
                ServerInfo {
                    name: "gamer".into(),
                    version: "test".into(),
                },
            )
            .unwrap(),
        );
        let token = service
            .credentials()
            .create(CreateTokenRequest {
                label: "HTTP fixture".into(),
                scopes: ["device.observe".into()].into(),
                allowed: AllowedObjects::default(),
                expires_in_seconds: 60,
            })
            .unwrap();
        let app = Router::new()
            .route("/api/mcp", post(endpoint))
            .with_state(service.clone());
        let initialized=app.clone().oneshot(request("POST",Some(&token.token),json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}))).await.unwrap();
        assert_eq!(initialized.status(), StatusCode::OK);
        assert!(initialized.headers().get("mcp-session-id").is_none());
        let notification = app
            .clone()
            .oneshot(request(
                "POST",
                Some(&token.token),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            ))
            .await
            .unwrap();
        assert_eq!(notification.status(), StatusCode::ACCEPTED);
        assert!(to_bytes(notification.into_body(), 1024)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            app.clone()
                .oneshot(request("GET", None, json!({})))
                .await
                .unwrap()
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        let mut cookie = request(
            "POST",
            None,
            json!({"jsonrpc":"2.0","id":2,"method":"ping"}),
        );
        cookie
            .headers_mut()
            .insert(header::COOKIE, "gb_sid=admin".parse().unwrap());
        let cookie = app.clone().oneshot(cookie).await.unwrap();
        assert_eq!(cookie.status(), StatusCode::UNAUTHORIZED);
        assert!(cookie.headers().contains_key(header::WWW_AUTHENTICATE));
        service.revoke_token(&token.metadata.token_id).unwrap();
        let revoked = app
            .oneshot(request(
                "POST",
                Some(&token.token),
                json!({"jsonrpc":"2.0","id":3,"method":"ping"}),
            ))
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn remote_and_missing_credentials_are_rejected_before_reading_the_body() {
        let root = tempfile::tempdir().unwrap();
        let service = Arc::new(
            McpService::open(
                root.path(),
                ServerInfo {
                    name: "fixture".into(),
                    version: "1".into(),
                },
            )
            .unwrap(),
        );
        let app = Router::new()
            .route("/api/mcp", post(endpoint))
            .with_state(service);
        for (peer, expected) in [
            ("192.0.2.2:1234", StatusCode::FORBIDDEN),
            ("127.0.0.1:1234", StatusCode::UNAUTHORIZED),
        ] {
            let mut request = request("POST", None, json!({}));
            request
                .extensions_mut()
                .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
            *request.body_mut() =
                Body::from_stream(futures_util::stream::pending::<Result<Bytes, std::io::Error>>());
            let response = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                app.clone().oneshot(request),
            )
            .await
            .expect("admission must not wait for a body")
            .unwrap();
            assert_eq!(response.status(), expected);
        }
    }

    #[tokio::test]
    async fn disabling_remote_access_during_upload_rejects_before_dispatch() {
        let root = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            source_path: Some(root.path().join("config.toml")),
            mcp_allow_remote: true,
            ..Default::default()
        };
        std::fs::write(
            cfg.source_path.as_ref().unwrap(),
            toml::to_string(&cfg).unwrap(),
        )
        .unwrap();
        let service = Arc::new(
            McpService::open(
                root.path(),
                ServerInfo {
                    name: "fixture".into(),
                    version: "1".into(),
                },
            )
            .unwrap(),
        );
        let token = service
            .credentials()
            .create(CreateTokenRequest {
                label: "upload fixture".into(),
                scopes: ["observe".into()].into(),
                allowed: Default::default(),
                expires_in_seconds: 60,
            })
            .unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (continue_tx, continue_rx) = tokio::sync::oneshot::channel();
        let mut request = request("POST", Some(&token.token), json!({}));
        request
            .headers_mut()
            .insert(header::HOST, "192.0.2.10:18443".parse().unwrap());
        *request.body_mut() = Body::from_stream(futures_util::stream::once(async move {
            started_tx.send(()).unwrap();
            continue_rx.await.unwrap();
            Ok::<_, std::io::Error>(Bytes::from_static(
                br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            ))
        }));
        let consumer = cfg.clone();
        let handling = tokio::spawn(async move {
            process_request(
                &service,
                &consumer,
                Some("192.0.2.20:1234".parse().unwrap()),
                request,
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();
        let before = cfg.settings_view().unwrap();
        let mut settings = before.saved;
        settings.mcp_allow_remote = false;
        cfg.save_settings(settings, &before.revision).unwrap();
        continue_tx.send(()).unwrap();
        assert_eq!(handling.await.unwrap().status(), StatusCode::FORBIDDEN);
    }

    struct ImageFixture;
    #[async_trait::async_trait]
    impl crate::mcp::ToolProvider for ImageFixture {
        fn tools(&self) -> Vec<crate::mcp::ToolDescriptor> {
            vec![crate::mcp::ToolDescriptor {
                name: "gamer.core.fixture_image".into(),
                description: "Synthetic PNG; no device or media access".into(),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                output_schema: None,
                required_scopes: ["observe".into()].into(),
                effect: crate::mcp::ToolEffect::Read,
                context: Default::default(),
            }]
        }
        async fn call(
            &self,
            context: crate::mcp::InvocationContext,
            _: &str,
            _: Value,
        ) -> Result<crate::mcp::ToolResult, McpError> {
            use base64::Engine;
            context.check_active()?;
            let mut png = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgba8(2, 3)
                .write_to(&mut png, image::ImageFormat::Png)
                .unwrap();
            let mut result =
                crate::mcp::ToolResult::json(json!({"synthetic":true,"width":2,"height":3}));
            result.content.push(json!({"type":"image","mimeType":"image/png","data":base64::engine::general_purpose::STANDARD.encode(png.into_inner())}));
            Ok(result)
        }
    }

    #[tokio::test]
    async fn real_loopback_http_client_initializes_lists_calls_png_and_observes_revocation() {
        use base64::Engine;
        let root = tempfile::tempdir().unwrap();
        let service = Arc::new(
            McpService::open(
                root.path(),
                ServerInfo {
                    name: "gamer-fixture".into(),
                    version: "1".into(),
                },
            )
            .unwrap(),
        );
        service
            .register_provider("fixture", Arc::new(ImageFixture))
            .unwrap();
        let token = service
            .credentials()
            .create(CreateTokenRequest {
                label: "real HTTP fixture".into(),
                scopes: ["observe".into()].into(),
                allowed: Default::default(),
                expires_in_seconds: 60,
            })
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint_url = format!("http://{}/api/mcp", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/api/mcp", post(endpoint))
            .with_state(service.clone());
        let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
        let serving = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap()
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .unwrap();
        let send = |body: Value| {
            client
                .post(&endpoint_url)
                .bearer_auth(&token.token)
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header("mcp-protocol-version", crate::mcp::PROTOCOL_VERSION)
                .json(&body)
                .send()
        };
        let initialized:Value=send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"reqwest-mcp-fixture","version":"1"}}})).await.unwrap().json().await.unwrap();
        assert_eq!(initialized["result"]["serverInfo"]["name"], "gamer-fixture");
        let notification = send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await
            .unwrap();
        assert_eq!(notification.status(), StatusCode::ACCEPTED);
        let listed: Value = send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            listed["result"]["tools"][0]["name"],
            "gamer.core.fixture_image"
        );
        let called:Value=send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"gamer.core.fixture_image","arguments":{}}})).await.unwrap().json().await.unwrap();
        let block = called["result"]["content"]
            .as_array()
            .unwrap()
            .iter()
            .find(|block| block["type"] == "image")
            .unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(block["data"].as_str().unwrap())
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap();
        assert_eq!((image.width(), image.height()), (2, 3));
        service.revoke_token(&token.metadata.token_id).unwrap();
        assert_eq!(
            send(json!({"jsonrpc":"2.0","id":4,"method":"ping"}))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let _ = shutdown.send(());
        serving.await.unwrap();
    }
}
