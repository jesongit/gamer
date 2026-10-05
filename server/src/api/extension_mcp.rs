//! Local protocol transport. Token storage and tool semantics stay in plugins.
use axum::{
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr};

use super::AppState;
use crate::extensions::{ExtensionError, ExtensionId};

pub(super) async fn post_message(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    let token = match transport_access(&headers, peer.map(|peer| peer.0)) {
        Ok(token) => token,
        Err((status, message)) => return transport_error(status, message),
    };
    let id = match ExtensionId::parse(&raw_id) {
        Ok(id) => id,
        Err(_) => return transport_error(StatusCode::BAD_REQUEST, "invalid extension ID"),
    };
    let request: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return Json(json!({"jsonrpc":"2.0","id":null,
            "error":{"code":-32700,"message":"parse error"}}))
            .into_response()
        }
    };
    match state.extensions.call_builtin_mcp(&id, request, token).await {
        Ok(Some(response)) => Json(response).into_response(),
        // Streamable HTTP permits stateless JSON responses; no SSE session is
        // needed. Notifications are accepted with no response body (202).
        Ok(None) => StatusCode::ACCEPTED.into_response(),
        Err(error) => {
            let status = match error {
                ExtensionError::ProtocolUnauthorized => StatusCode::UNAUTHORIZED,
                ExtensionError::NotInstalled { .. }
                | ExtensionError::VersionNotInstalled { .. } => StatusCode::NOT_FOUND,
                ExtensionError::InvalidTransition { .. } => StatusCode::CONFLICT,
                ExtensionError::RuntimeUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
                ExtensionError::Permission(_) => StatusCode::FORBIDDEN,
                ExtensionError::CallRejected(_) => StatusCode::BAD_REQUEST,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            transport_error_owned(status, error.to_string())
        }
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback()
        || match ip {
            IpAddr::V6(ip) => ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback()),
            _ => false,
        }
}

fn local_host(url: &reqwest::Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host == "[::1]"
            || host.parse::<IpAddr>().is_ok_and(is_loopback)
    })
}

fn transport_access(
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
) -> Result<&str, (StatusCode, &'static str)> {
    if !peer.is_some_and(|peer| is_loopback(peer.ip())) {
        return Err((
            StatusCode::FORBIDDEN,
            "MCP connections must originate from loopback",
        ));
    }
    // Forwarded/X-Forwarded-For are deliberately never consulted. A proxy must
    // not turn a remote connection into an authenticated local MCP request.
    if headers.contains_key("forwarded") || headers.contains_key("x-forwarded-for") {
        return Err((
            StatusCode::FORBIDDEN,
            "proxied MCP connections are not supported",
        ));
    }
    if headers.get_all(header::HOST).iter().count() != 1
        || headers.get_all(header::ORIGIN).iter().count() > 1
        || headers.get_all(header::AUTHORIZATION).iter().count() != 1
    {
        return Err((StatusCode::UNAUTHORIZED, "invalid MCP connection headers"));
    }
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let host_url = reqwest::Url::parse(&format!("http://{host}"));
    if !host_url.is_ok_and(|url| {
        local_host(&url)
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
    }) {
        return Err((StatusCode::FORBIDDEN, "MCP Host must be a loopback address"));
    }
    if !super::auth::origin_allows_headers(headers) {
        return Err((StatusCode::FORBIDDEN, "MCP Origin does not match Host"));
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let parsed = origin
            .to_str()
            .ok()
            .and_then(|origin| reqwest::Url::parse(origin).ok());
        if !parsed.is_some_and(|url| {
            local_host(&url)
                && matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
        }) {
            return Err((
                StatusCode::FORBIDDEN,
                "MCP Origin must be a loopback origin",
            ));
        }
    }
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let Some((scheme, token)) = auth.split_once(' ') else {
        return Err((StatusCode::UNAUTHORIZED, "MCP Bearer token is required"));
    };
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.len() > 4096
        || token.chars().any(char::is_whitespace)
    {
        return Err((StatusCode::UNAUTHORIZED, "invalid MCP Bearer token"));
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !content_type
        .split(';')
        .next()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
    {
        return Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "MCP POST requires application/json",
        ));
    }
    let accept = headers
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|mime| mime.split(';').next().unwrap_or("").trim().to_lowercase())
        .collect::<Vec<_>>();
    if !accept.iter().any(|mime| mime == "application/json")
        || !accept.iter().any(|mime| mime == "text/event-stream")
    {
        return Err((
            StatusCode::NOT_ACCEPTABLE,
            "MCP Accept must include application/json and text/event-stream",
        ));
    }
    if let Some(version) = headers.get("mcp-protocol-version") {
        if !version
            .to_str()
            .is_ok_and(|version| matches!(version, "2025-03-26" | "2025-06-18" | "2025-11-25"))
        {
            return Err((StatusCode::BAD_REQUEST, "unsupported MCP protocol version"));
        }
    }
    Ok(token)
}

fn transport_error(status: StatusCode, message: &'static str) -> Response {
    transport_error_owned(status, message.to_string())
}

fn transport_error_owned(status: StatusCode, message: String) -> Response {
    let mut response = (status, Json(json!({"error":message}))).into_response();
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            axum::http::HeaderValue::from_static("Bearer realm=\"Gamer MCP\""),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{ActivityLease, RunContext, RunRequest},
        run_manager::RunExecutor,
    };
    use futures_util::future::BoxFuture;
    use std::sync::{atomic::AtomicBool, Arc};

    struct NoExecutor;
    impl RunExecutor for NoExecutor {
        fn prepare<'a>(
            &'a self,
            _: &'a RunContext,
            _: &'a RunRequest,
        ) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async { anyhow::bail!("MCP protocol test must not prepare a device") })
        }
        fn execute<'a>(
            &'a self,
            _: &'a RunContext,
            _: &'a RunRequest,
            _: bool,
            _: Arc<AtomicBool>,
        ) -> BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
            Box::pin(async { anyhow::bail!("MCP protocol test must not execute input") })
        }
        fn acquire(&self, _: &RunContext) -> anyhow::Result<Box<dyn ActivityLease>> {
            anyhow::bail!("MCP protocol test must not acquire a device")
        }
    }

    struct Fixture {
        server: Option<tokio::task::JoinHandle<()>>,
        base_url: String,
        token: String,
        _root: tempfile::TempDir,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(server) = self.server.take() {
                server.abort();
            }
        }
    }

    impl Fixture {
        async fn close(mut self) {
            if let Some(server) = self.server.take() {
                server.abort();
                let _ = server.await;
            }
        }
    }

    async fn fixture() -> Fixture {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            data_dir: root.path().into(),
            ..Default::default()
        };
        let db = Arc::new(crate::store::Store::open(&cfg).unwrap());
        let packages = Arc::new(crate::resources::PackageStore::open(&cfg).unwrap());
        packages.ensure_default_package().unwrap();
        let devices = Arc::new(crate::device::DeviceManager::new(db.clone(), cfg.clone()));
        let runs = Arc::new(crate::run_manager::RunManager::new(Arc::new(NoExecutor)));
        let scheduler = Arc::new(crate::scheduler::Scheduler::new(db.clone()));
        let capabilities = crate::capabilities::adapters::build_registry(
            devices.clone(),
            packages.clone(),
            db.clone(),
            runs.clone(),
        );
        let ai = Arc::new(
            crate::extensions::ai::AiService::new(
                crate::extensions::ai::Runtime {
                    devices: devices.clone(),
                    packages: packages.clone(),
                    runs: runs.clone(),
                    scheduler: scheduler.clone(),
                    capabilities: capabilities.clone(),
                },
                root.path(),
            )
            .unwrap(),
        );
        let extensions = Arc::new(
            crate::extensions::ExtensionService::for_data_root(root.path(), capabilities)
                .with_builtin_service(ai.clone())
                .with_runner_registrar(ai.registrar()),
        );
        ai.attach(&extensions);
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in [
            (
                "manifest.toml",
                include_str!("../../../plugins/gamer-ai/manifest.toml"),
            ),
            ("ui/plugin.js", "export const sdkVersion = 1;"),
        ] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes.as_bytes()).unwrap();
        }
        let id = ExtensionId::parse("gamer-ai").unwrap();
        extensions
            .install(&archive.finish().unwrap().into_inner())
            .await
            .unwrap();
        extensions.enable(&id).await.unwrap();
        extensions.start(&id).await.unwrap();
        devices
            .browsers
            .db
            .save_browser_target(crate::browser::BrowserTarget {
                id: "browser-mcp-route".into(),
                name: "MCP transport fixture".into(),
                url: "http://127.0.0.1/".into(),
                profile_id: "mcp-route".into(),
                width: 320,
                height: 240,
            })
            .unwrap();
        let created = extensions.call_extension(&id, "mcp.tokens.create",
            json!({"device_id":"browser-mcp-route","content_package":"default","control":false})).await.unwrap();
        let auth = Arc::new(super::super::auth::AuthState::new(
            super::super::auth::Credential::Unavailable,
            cfg.auth.clone(),
            false,
            None,
        ));
        let shutdown = Arc::new(crate::shutdown::ShutdownCoordinator::new(Arc::new(|| {
            Box::pin(async {})
        })));
        let update = Arc::new(crate::update::service::UpdateService::new(
            Arc::new(crate::update::controller::UnsupportedController),
            crate::update::policy::PolicyStore::load_blocking(
                root.path(),
                crate::update::policy::UpdatePolicy::default(),
            ),
            Arc::new(crate::update::service::UpdateTxn::default()),
            Arc::new(crate::update::workload::Workload::default),
            db.clone(),
        ));
        let app = super::super::build_router_with_extensions(
            db,
            devices,
            runs,
            scheduler,
            cfg,
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            packages,
            shutdown,
            auth,
            update,
            extensions,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Fixture {
            server: Some(server),
            base_url,
            token: created["token"].as_str().unwrap().to_string(),
            _root: root,
        }
    }

    #[tokio::test]
    async fn loopback_http_initializes_and_accepts_notifications_without_admin_session() {
        let fixture = fixture().await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let url = format!("{}/api/extensions/gamer-ai/mcp", fixture.base_url);
        let initialize = client.post(&url).bearer_auth(&fixture.token)
            .header(header::ACCEPT, "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"route-test","version":"1"}}}))
            .send().await.unwrap();
        assert_eq!(initialize.status(), StatusCode::OK);
        assert!(initialize.headers().get("mcp-session-id").is_none());
        let result: Value = initialize.json().await.unwrap();
        assert_eq!(result["id"], 1);
        assert_eq!(result["result"]["serverInfo"]["name"], "gamer-ai");
        let notification = client
            .post(&url)
            .bearer_auth(&fixture.token)
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .send()
            .await
            .unwrap();
        assert_eq!(notification.status(), StatusCode::ACCEPTED);
        assert!(notification.bytes().await.unwrap().is_empty());
        let tools = client
            .post(&url)
            .bearer_auth(&fixture.token)
            .header(header::ACCEPT, "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":"tools","method":"tools/list"}))
            .send()
            .await
            .unwrap();
        assert_eq!(tools.status(), StatusCode::OK);
        let tools: Value = tools.json().await.unwrap();
        assert!(tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| !tool["name"].as_str().unwrap().starts_with("input_")));
        let get = client
            .get(&url)
            .header(header::ACCEPT, "text/event-stream")
            .send()
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::METHOD_NOT_ALLOWED);
        let invalid = client
            .post(&url)
            .bearer_auth("0".repeat(64))
            .header(header::ACCEPT, "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":2,"method":"ping"}))
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
        assert!(invalid.headers().contains_key(header::WWW_AUTHENTICATE));
        let cookie_only = client
            .post(&url)
            .header(header::COOKIE, "gb_sid=unrelated-admin")
            .header("x-admin-token", "unrelated-admin")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":3,"method":"ping"}))
            .send()
            .await
            .unwrap();
        assert_eq!(cookie_only.status(), StatusCode::UNAUTHORIZED);
        let origin = client
            .post(&url)
            .bearer_auth(&fixture.token)
            .header(header::ORIGIN, "https://attacker.invalid")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":4,"method":"ping"}))
            .send()
            .await
            .unwrap();
        assert_eq!(origin.status(), StatusCode::FORBIDDEN);
        fixture.close().await;
    }

    fn headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "127.0.0.1:8443".parse().unwrap());
        headers.insert(header::AUTHORIZATION, "Bearer test-only".parse().unwrap());
        headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        headers.insert(
            header::ACCEPT,
            "application/json, text/event-stream".parse().unwrap(),
        );
        headers
    }

    #[test]
    fn credentials_are_independent_and_remote_or_proxied_clients_cannot_use_mcp() {
        let local = "127.0.0.1:12345".parse().unwrap();
        assert_eq!(
            transport_access(&headers(), Some(local)).unwrap(),
            "test-only"
        );
        assert!(transport_access(&headers(), None).is_err());
        assert!(transport_access(&headers(), Some("192.0.2.3:12345".parse().unwrap())).is_err());
        let mut cookie = headers();
        cookie.remove(header::AUTHORIZATION);
        cookie.insert(header::COOKIE, "gb_sid=admin-session".parse().unwrap());
        cookie.insert("x-admin-token", "admin-token".parse().unwrap());
        assert_eq!(
            transport_access(&cookie, Some(local)).unwrap_err().0,
            StatusCode::UNAUTHORIZED
        );
        let mut proxy = headers();
        proxy.insert("x-forwarded-for", "127.0.0.1".parse().unwrap());
        assert_eq!(
            transport_access(&proxy, Some(local)).unwrap_err().0,
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn rejects_origin_and_host_rebinding_and_requires_streamable_http_headers() {
        let local = Some("127.0.0.1:12345".parse().unwrap());
        let mut valid = headers();
        valid.insert(header::ORIGIN, "http://127.0.0.1:8443".parse().unwrap());
        assert!(transport_access(&valid, local).is_ok());
        valid.insert(header::ORIGIN, "https://attacker.invalid".parse().unwrap());
        assert_eq!(
            transport_access(&valid, local).unwrap_err().0,
            StatusCode::FORBIDDEN
        );
        valid.insert(header::HOST, "attacker.invalid".parse().unwrap());
        assert_eq!(
            transport_access(&valid, local).unwrap_err().0,
            StatusCode::FORBIDDEN
        );
        let mut accept = headers();
        accept.insert(header::ACCEPT, "application/json".parse().unwrap());
        assert_eq!(
            transport_access(&accept, local).unwrap_err().0,
            StatusCode::NOT_ACCEPTABLE
        );
        let mut duplicated = headers();
        duplicated.append(header::AUTHORIZATION, "Bearer other".parse().unwrap());
        assert_eq!(
            transport_access(&duplicated, local).unwrap_err().0,
            StatusCode::UNAUTHORIZED
        );
    }
}
