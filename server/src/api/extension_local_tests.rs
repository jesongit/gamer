use super::*;
use crate::{
    core::{ActivityLease, RunContext, RunRequest},
    extensions::{BuiltinService, ExtensionResult, ExtensionService},
    run_manager::RunExecutor,
};
use axum::{body::Body, http::Request, Router};
use futures_util::future::BoxFuture;
use std::{
    io::Write,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};
use tower::ServiceExt;

fn native_access(headers: &HeaderMap, peer: Option<SocketAddr>) -> bool {
    super::transport_access(headers, peer, &Default::default())
}

fn headers(host: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, host.parse().unwrap());
    headers
}

#[test]
fn local_http_transport_requires_real_loopback_and_single_local_host() {
    for peer in [
        "127.0.0.1:42",
        "127.9.1.3:42",
        "[::1]:42",
        "[::ffff:127.0.0.1]:42",
    ] {
        assert!(
            native_access(&headers("127.0.0.1:8443"), Some(peer.parse().unwrap())),
            "{peer}"
        );
    }
    for host in [
        "localhost:8443",
        "127.0.0.1:8443",
        "127.9.1.3:8443",
        "[::1]:8443",
        "[::ffff:127.0.0.1]:8443",
    ] {
        assert!(
            native_access(&headers(host), Some("127.0.0.1:42".parse().unwrap())),
            "{host}"
        );
    }
    let mut admin = headers("127.0.0.1:8443");
    admin.insert(header::COOKIE, "gb_sid=fixture-admin".parse().unwrap());
    admin.insert("x-admin-token", "fixture-admin".parse().unwrap());
    for peer in [
        None,
        Some("192.0.2.10:42".parse().unwrap()),
        Some("[::ffff:192.0.2.10]:42".parse().unwrap()),
    ] {
        assert!(
            !native_access(&admin, peer),
            "admin credentials cannot replace a real loopback peer"
        );
    }
    let local = Some("127.0.0.1:42".parse().unwrap());
    assert!(!native_access(&HeaderMap::new(), local));
    for host in [
        "attacker.invalid:8443",
        "localhost.attacker.invalid",
        "user@127.0.0.1:8443",
        "127.0.0.1:8443/path",
        "127.0.0.1:8443?query",
        "127.0.0.1:8443#fragment",
        "127.0.0.1:8443, localhost:8443",
    ] {
        assert!(!native_access(&headers(host), local), "{host}");
    }
    let mut duplicate = headers("127.0.0.1:8443");
    duplicate.append(header::HOST, "localhost:8443".parse().unwrap());
    assert!(!native_access(&duplicate, local));
    for name in ["forwarded", "x-forwarded-for"] {
        let mut forwarded = admin.clone();
        forwarded.insert(name, "127.0.0.1".parse().unwrap());
        assert!(!native_access(&forwarded, local));
    }
}

#[test]
fn local_http_origin_is_optional_but_must_be_one_same_loopback_origin() {
    let local = Some("127.0.0.1:42".parse().unwrap());
    let mut valid = headers("127.0.0.1:8443");
    valid.insert(header::ORIGIN, "http://127.0.0.1:8443".parse().unwrap());
    assert!(native_access(&valid, local));
    for origin in [
        "",
        "null",
        "https://attacker.invalid",
        "http://127.0.0.1:9999",
        "http://localhost:8443",
        "http://127.0.0.1:8443/path",
        "http://user@127.0.0.1:8443",
    ] {
        let mut bad = headers("127.0.0.1:8443");
        bad.insert(header::ORIGIN, origin.parse().unwrap());
        assert!(!native_access(&bad, local), "{origin}");
    }
    valid.append(header::ORIGIN, "http://127.0.0.1:8443".parse().unwrap());
    assert!(!native_access(&valid, local));
}

struct NoExecutor;

fn docker_read_config() -> crate::config::LocalReadConfig {
    crate::config::LocalReadConfig {
        published_port: std::num::NonZeroU16::new(18443),
        docker_gateway: Some("172.17.0.1".parse().unwrap()),
    }
}

#[test]
fn local_http_docker_requires_pinned_peer_published_port_and_same_origin() {
    let config = docker_read_config();
    let peer = Some("172.17.0.1:42".parse().unwrap());
    let access = |h: &HeaderMap, p| super::transport_access(h, p, &config);
    let mut valid = headers("127.0.0.1:18443");
    valid.insert(header::ORIGIN, "http://127.0.0.1:18443".parse().unwrap());
    assert!(access(&valid, peer));
    assert!(access(&headers("localhost:18443"), peer));
    for p in [
        None,
        Some("172.17.0.2:42".parse().unwrap()),
        Some("192.168.1.5:42".parse().unwrap()),
    ] {
        assert!(!access(&valid, p));
    }
    for host in [
        "127.0.0.1:8443",
        "172.17.0.1:18443",
        "attacker.invalid:18443",
    ] {
        assert!(!access(&headers(host), peer), "{host}");
    }
    for (name, value) in [
        ("origin", "http://127.0.0.1:8443"),
        ("origin", "http://localhost:18443"),
        ("origin", "https://attacker.invalid"),
        ("forwarded", "for=127.0.0.1"),
        ("x-forwarded-for", "127.0.0.1"),
    ] {
        let mut bad = valid.clone();
        bad.insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
        assert!(!access(&bad, peer), "{name}");
    }
    let mut duplicate = valid.clone();
    duplicate.append(header::HOST, "127.0.0.1:18443".parse().unwrap());
    assert!(!access(&duplicate, peer));
    duplicate = valid.clone();
    duplicate.append(header::ORIGIN, "http://127.0.0.1:18443".parse().unwrap());
    assert!(!access(&duplicate, peer));
    let incomplete = crate::config::LocalReadConfig {
        published_port: None,
        ..config
    };
    assert!(!super::transport_access(&valid, peer, &incomplete));
    assert!(super::transport_access(
        &headers("127.0.0.1:8443"),
        Some("127.0.0.1:42".parse().unwrap()),
        &incomplete
    ));
}

impl RunExecutor for NoExecutor {
    fn prepare<'a>(
        &'a self,
        _: &'a RunContext,
        _: &'a RunRequest,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async { anyhow::bail!("local read fixture cannot prepare a target") })
    }
    fn execute<'a>(
        &'a self,
        _: &'a RunContext,
        _: &'a RunRequest,
        _: bool,
        _: Arc<AtomicBool>,
    ) -> BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
        Box::pin(async { anyhow::bail!("local read fixture cannot execute input") })
    }
    fn acquire(&self, _: &RunContext) -> anyhow::Result<Box<dyn ActivityLease>> {
        anyhow::bail!("local read fixture cannot acquire a target")
    }
}

struct ReadBuiltin {
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl BuiltinService for ReadBuiltin {
    fn extension_id(&self) -> &str {
        "gamer-video"
    }
    async fn call(&self, _: &str, _: serde_json::Value) -> ExtensionResult<serde_json::Value> {
        Err(ExtensionError::CallRejected("unused".into()))
    }
    async fn stop(&self) {}
    async fn local_http(&self, path: &str, headers: HeaderMap) -> Response {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if path != "fixture-read-token/page" {
            return StatusCode::NOT_FOUND.into_response();
        }
        if headers.get(header::RANGE).is_some_and(|v| v == "bytes=1-2") {
            return (
                StatusCode::PARTIAL_CONTENT,
                [(header::CONTENT_RANGE, "bytes 1-2/3")],
                "bc",
            )
                .into_response();
        }
        (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "fixture read-only page",
        )
            .into_response()
    }
}

struct Fixture {
    app: Router,
    extensions: Arc<ExtensionService>,
    builtin: Arc<ReadBuiltin>,
    _root: tempfile::TempDir,
}

struct TestServer(tokio::task::JoinHandle<()>);
impl Drop for TestServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn fixture() -> Fixture {
    fixture_with_local_read(Default::default()).await
}

async fn fixture_with_local_read(local_read: crate::config::LocalReadConfig) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let cfg = crate::config::Config {
        data_dir: root.path().into(),
        local_read,
        ..Default::default()
    };
    let db = Arc::new(crate::store::Store::open(&cfg).unwrap());
    let packages = Arc::new(crate::resources::PackageStore::open(&cfg).unwrap());
    packages.ensure_default_package().unwrap();
    let devices = Arc::new(crate::device::DeviceManager::new(db.clone(), cfg.clone()));
    let runs = Arc::new(crate::run_manager::RunManager::new(Arc::new(NoExecutor)));
    let scheduler = Arc::new(crate::scheduler::Scheduler::new(db.clone()));
    let builtin = Arc::new(ReadBuiltin {
        calls: AtomicUsize::new(0),
    });
    let extensions = Arc::new(
        ExtensionService::for_data_root(
            root.path(),
            crate::capabilities::CapabilityRegistry::default(),
        )
        .with_builtin_service(builtin.clone()),
    );
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    archive
        .start_file("manifest.toml", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"manifest_version=2\nid=\"gamer-video\"\nname=\"Local read fixture\"\nversion=\"1.0.0\"\n[execution]\nkind=\"builtin\"\nbuiltin_id=\"gamer-video\"\n").unwrap();
    let installed = extensions
        .install(&archive.finish().unwrap().into_inner())
        .await
        .unwrap();
    extensions.enable(installed.id()).await.unwrap();
    extensions.start(installed.id()).await.unwrap();
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
        extensions.clone(),
    );
    Fixture {
        _root: root,
        app,
        extensions,
        builtin,
    }
}

#[tokio::test]
async fn local_http_real_router_reads_without_admin_and_rejects_token_methods_and_sources() {
    let fixture = fixture().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = fixture.app.clone();
    let mut server = TestServer(tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap()
    }));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("{base}/local/extensions/gamer-video/fixture-read-token/page");
    let response = client.get(&url).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text().await.unwrap(), "fixture read-only page");
    let head = client.head(&url).send().await.unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert!(head.bytes().await.unwrap().is_empty());
    let range = client
        .get(&url)
        .header(header::RANGE, "bytes=1-2")
        .send()
        .await
        .unwrap();
    assert_eq!(range.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(range.headers()[header::CONTENT_RANGE], "bytes 1-2/3");
    assert_eq!(range.text().await.unwrap(), "bc");
    let invalid = client
        .get(format!("{base}/local/extensions/gamer-video/wrong/page"))
        .header(header::COOKIE, "gb_sid=fixture-admin")
        .header("x-admin-token", "fixture-admin")
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::NOT_FOUND);
    assert!(invalid.bytes().await.unwrap().is_empty());
    let before = fixture.builtin.calls.load(Ordering::SeqCst);
    for method in [
        reqwest::Method::POST,
        reqwest::Method::PUT,
        reqwest::Method::DELETE,
        reqwest::Method::OPTIONS,
    ] {
        assert_eq!(
            client.request(method, &url).send().await.unwrap().status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    for (header, value) in [
        (header::ORIGIN.as_str(), "https://attacker.invalid"),
        ("forwarded", "for=127.0.0.1"),
        ("x-forwarded-for", "127.0.0.1"),
        (header::HOST.as_str(), "attacker.invalid"),
    ] {
        let response = client
            .get(&url)
            .header(header, value)
            .header(header::COOKIE, "gb_sid=fixture-admin")
            .header("x-admin-token", "fixture-admin")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(response.bytes().await.unwrap().is_empty());
    }
    assert_eq!(fixture.builtin.calls.load(Ordering::SeqCst), before);
    fixture
        .extensions
        .disable(&ExtensionId::parse("gamer-video").unwrap())
        .await
        .unwrap();
    let disabled = client.get(&url).send().await.unwrap();
    assert_eq!(disabled.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(disabled.bytes().await.unwrap().is_empty());
    let absent = client
        .get(format!(
            "{base}/local/extensions/not-installed/fixture-read-token/page"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(absent.status(), StatusCode::NOT_FOUND);
    assert!(absent.bytes().await.unwrap().is_empty());
    server.0.abort();
    let _ = (&mut server.0).await;
}

#[tokio::test]
async fn local_http_router_rejects_missing_or_remote_connect_info_even_with_admin_headers() {
    let fixture = fixture().await;
    for peer in [
        None,
        Some("192.0.2.10:42".parse::<SocketAddr>().unwrap()),
        Some("[::ffff:192.0.2.10]:42".parse().unwrap()),
    ] {
        let mut request = Request::builder()
            .uri("/local/extensions/gamer-video/fixture-read-token/page")
            .header(header::HOST, "127.0.0.1:8443")
            .header(header::COOKIE, "gb_sid=fixture-admin")
            .header("x-admin-token", "fixture-admin")
            .body(Body::empty())
            .unwrap();
        if let Some(peer) = peer {
            request.extensions_mut().insert(ConnectInfo(peer));
        }
        let response = fixture.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty());
    }
    assert_eq!(fixture.builtin.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_http_docker_router_does_not_grant_mcp_or_management_access() {
    let fixture = fixture_with_local_read(docker_read_config()).await;
    for (method, path, expected) in [
        (
            "GET",
            "/local/extensions/gamer-video/fixture-read-token/page",
            StatusCode::OK,
        ),
        (
            "GET",
            "/local/extensions/gamer-video/wrong/page",
            StatusCode::NOT_FOUND,
        ),
        (
            "POST",
            "/local/extensions/gamer-video/fixture-read-token/page",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        ("POST", "/api/mcp", StatusCode::FORBIDDEN),
        ("GET", "/api/extensions", StatusCode::UNAUTHORIZED),
    ] {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "127.0.0.1:18443")
            .header(header::AUTHORIZATION, "Bearer fixture-read-token")
            .header("x-admin-token", "fixture-read-token")
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo("172.17.0.1:42".parse::<SocketAddr>().unwrap()));
        let response = fixture.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), expected, "{method} {path}");
    }
}
