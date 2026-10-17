use super::*;
use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use std::{
    io::Write,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

struct ReadBuiltin {
    calls: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}

#[async_trait]
impl BuiltinService for ReadBuiltin {
    fn extension_id(&self) -> &str {
        "gamer-video"
    }
    async fn call(&self, _: &str, _: serde_json::Value) -> ExtensionResult<serde_json::Value> {
        Err(ExtensionError::CallRejected("unused".into()))
    }
    async fn stop(&self) {}
    async fn local_http(&self, path: &str, _: HeaderMap) -> Response {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if path == "held-read" {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        StatusCode::OK.into_response()
    }
}

fn builtin_archive() -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    archive
        .start_file("manifest.toml", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"manifest_version=2\nid=\"gamer-video\"\nname=\"Local read fixture\"\nversion=\"1.0.0\"\n[execution]\nkind=\"builtin\"\nbuiltin_id=\"gamer-video\"\n").unwrap();
    archive.finish().unwrap().into_inner()
}

#[tokio::test]
async fn local_http_requires_current_process_running_and_holds_disable_lease() {
    let root = tempfile::tempdir().unwrap();
    let builtin = Arc::new(ReadBuiltin {
        calls: AtomicUsize::new(0),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Semaphore::new(0),
    });
    let service = Arc::new(
        ExtensionService::for_data_root(root.path(), CapabilityRegistry::default())
            .with_builtin_service(builtin.clone()),
    );
    let installed = service.install(&builtin_archive()).await.unwrap();
    let id = installed.id().clone();
    assert!(service
        .call_builtin_local_http(&id, "read", HeaderMap::new())
        .await
        .is_err());
    service.enable(&id).await.unwrap();
    assert!(service
        .call_builtin_local_http(&id, "read", HeaderMap::new())
        .await
        .is_err());
    service.start(&id).await.unwrap();
    assert_eq!(
        service
            .call_builtin_local_http(&id, "read", HeaderMap::new())
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    // The persisted Running record belongs to the first instance. A fresh
    // process service cannot use it before its own successful startup.
    let restarted = ExtensionService::for_data_root(root.path(), CapabilityRegistry::default())
        .with_builtin_service(builtin.clone());
    assert!(matches!(
        restarted
            .call_builtin_local_http(&id, "read", HeaderMap::new())
            .await,
        Err(ExtensionError::RuntimeUnavailable(_))
    ));

    let read_service = service.clone();
    let read_id = id.clone();
    let reading = tokio::spawn(async move {
        read_service
            .call_builtin_local_http(&read_id, "held-read", HeaderMap::new())
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), builtin.entered.notified())
        .await
        .unwrap();
    let disable_service = service.clone();
    let disable_id = id.clone();
    let mut disabling = tokio::spawn(async move { disable_service.disable(&disable_id).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut disabling)
            .await
            .is_err(),
        "disable must await the admitted local read"
    );
    builtin.release.add_permits(1);
    assert_eq!(reading.await.unwrap().unwrap().status(), StatusCode::OK);
    assert_eq!(
        disabling.await.unwrap().unwrap().state(),
        ExtensionState::Disabled
    );
    assert!(service
        .call_builtin_local_http(&id, "read", HeaderMap::new())
        .await
        .is_err());
    assert_eq!(builtin.calls.load(Ordering::SeqCst), 2);

    service.enable(&id).await.unwrap();
    service.start(&id).await.unwrap();
    service.shutdown_builtin_service().await;
    assert!(matches!(
        service
            .call_builtin_local_http(&id, "read", HeaderMap::new())
            .await,
        Err(ExtensionError::CallRejected(_))
    ));
    assert_eq!(builtin.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn local_http_default_surface_is_not_found() {
    struct DefaultBuiltin;
    #[async_trait]
    impl BuiltinService for DefaultBuiltin {
        fn extension_id(&self) -> &str {
            "gamer-video"
        }
        async fn call(&self, _: &str, _: serde_json::Value) -> ExtensionResult<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        async fn stop(&self) {}
    }
    assert_eq!(
        DefaultBuiltin
            .local_http("unregistered", HeaderMap::new())
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
