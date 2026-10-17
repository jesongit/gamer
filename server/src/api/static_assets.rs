//! Keep historical public files on disk without serving retired distributions.
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use axum::{
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

fn decoded_path(raw: &str) -> Option<String> {
    if raw.len() > 8192 {
        return None;
    }
    let mut bytes = Vec::with_capacity(raw.len());
    let mut input = raw.bytes();
    while let Some(byte) = input.next() {
        bytes.push(if byte == b'%' {
            let high = char::from(input.next()?).to_digit(16)?;
            let low = char::from(input.next()?).to_digit(16)?;
            ((high << 4) | low) as u8
        } else {
            byte
        });
    }
    let path = String::from_utf8(bytes)
        .ok()?
        .replace('\\', "/")
        .to_ascii_lowercase();
    if path.contains('\0')
        || path.split('/').any(|part| {
            matches!(part, "." | "..") || part.ends_with(['.', ' ']) || part.contains([':', '~'])
        })
    {
        return None;
    }
    Some(
        path.split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

struct CurrentRegistry {
    document: Value,
    retired_ui: BTreeSet<String>,
}

async fn current_registry(root: &Path) -> Result<CurrentRegistry, StatusCode> {
    let file = tokio::fs::File::open(root.join("registry.json"))
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            }
        })?;
    const MAX_BYTES: usize = 3 * 1024 * 1024;
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if bytes.len() > MAX_BYTES {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let mut registry: Value =
        serde_json::from_slice(&bytes).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if registry["schema_version"] != 2 {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let entries = registry["plugins"]
        .as_array_mut()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let current_ui: BTreeSet<_> = entries
        .iter()
        .filter(|entry| !crate::extensions::market::retired_catalog_entry(entry))
        .filter_map(|entry| entry["id"].as_str().map(str::to_ascii_lowercase))
        .collect();
    let retired_ui = entries
        .iter()
        .filter(|entry| crate::extensions::market::retired_catalog_entry(entry))
        .filter_map(|entry| entry["id"].as_str().map(str::to_ascii_lowercase))
        .filter(|id| !current_ui.contains(id))
        .collect();
    entries.retain(|entry| !crate::extensions::market::retired_catalog_entry(entry));
    Ok(CurrentRegistry {
        document: registry,
        retired_ui,
    })
}

pub(super) async fn guard(State(root): State<PathBuf>, request: Request, next: Next) -> Response {
    let Some(path) = decoded_path(request.uri().path()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let mut segments = path.split('/');
    if segments.next() == Some("plugin-ui") {
        if let Some(id) = segments.next() {
            if crate::extensions::market::retired_catalog_entry(&json!({"id":id})) {
                return StatusCode::NOT_FOUND.into_response();
            }
        }
    }
    let ui_plugin = path
        .strip_prefix("plugin-ui/")
        .and_then(|path| path.split_once('/'))
        .map(|(id, _)| id);
    if path != "registry.json"
        && !path.starts_with("plugins/")
        && !path.ends_with(".gplugin")
        && ui_plugin.is_none()
    {
        return next.run(request).await;
    }
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let registry = match current_registry(&root).await {
        Ok(registry) => registry,
        Err(StatusCode::NOT_FOUND) if ui_plugin.is_some() => return next.run(request).await,
        Err(status) => return status.into_response(),
    };
    if let Some(id) = ui_plugin {
        if registry.retired_ui.contains(id) {
            return StatusCode::NOT_FOUND.into_response();
        }
        return next.run(request).await;
    }
    if path == "registry.json" {
        return Json(registry.document).into_response();
    }
    let allowed = registry.document["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| {
            let (Some(id), Some(version)) = (entry["id"].as_str(), entry["version"].as_str())
            else {
                return false;
            };
            crate::extensions::ExtensionId::parse(id).is_ok()
                && semver::Version::parse(version).is_ok()
                && path == format!("plugins/{id}-{version}.gplugin").to_ascii_lowercase()
        });
    if !allowed {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
        Router,
    };
    use tower::ServiceExt;
    use tower_http::services::ServeDir;

    #[tokio::test]
    async fn historical_public_files_are_preserved_but_retired_responses_are_unavailable() {
        let root = tempfile::tempdir().unwrap();
        for path in [
            "plugin-ui/gamer-ai/plugin.js",
            "plugin-ui/gamer-yaml/plugin.js",
            "plugin-ui/gamer-ai-1helper/plugin.js",
            "plugins/gamer-ai-0.1.0.gplugin",
            "plugins/gamer-yaml-0.2.0.gplugin",
            "plugins/gamer-ai-1helper-1.0.0.gplugin",
            "plugins/unlisted-1.0.0.gplugin",
        ] {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"unchanged historical bytes").unwrap();
        }
        let registry = json!({"schema_version":2,"plugins":[
            {"id":"gamer-ai","version":"0.1.0"},
            {"id":"gamer-yaml","version":"0.2.0","permissions":[" ai.connect "]},
            {"id":"gamer-ai-1helper","version":"1.0.0","permissions":["resource.read"]}
        ]})
        .to_string();
        std::fs::write(root.path().join("registry.json"), &registry).unwrap();
        let app = Router::new()
            .fallback_service(ServeDir::new(root.path()))
            .layer(axum::middleware::from_fn_with_state(
                root.path().to_path_buf(),
                guard,
            ));
        for path in [
            "/plugin-ui/gamer-ai/plugin.js",
            "/plugin-ui/%67amer-ai/plugin.js",
            "/plugin-ui//gamer-ai/plugin.js",
            "/plugin-ui/%2fgamer-ai/plugin.js",
            "//plugin-ui/gamer-ai/plugin.js",
            "/plugin-ui/gamer-yaml/plugin.js",
            "/plugin-ui//gamer-yaml/plugin.js",
            "/plugins/gamer-ai-0.1.0.gplugin",
            "/plugins/gamer-yaml-0.2.0.gplugin",
            "/plugins/unlisted-1.0.0.gplugin",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
        for path in [
            "/plugin-ui/gamer-ai./plugin.js",
            "/plugin-ui/gamer-ai%20/plugin.js",
            "/plugin-ui/gamer-ai::$DATA/plugin.js",
            "/PLUGIN~1/gamer-ai/plugin.js",
            "/plugins/GAMER-~1.GPL",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        }
        for path in [
            "/plugins/gamer-ai-1helper-1.0.0.gplugin",
            "/plugin-ui/gamer-ai-1helper/plugin.js",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/registry.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(value["plugins"].as_array().unwrap().len(), 1);
        assert_eq!(value["plugins"][0]["id"], "gamer-ai-1helper");
        assert_eq!(
            std::fs::read_to_string(root.path().join("registry.json")).unwrap(),
            registry
        );
        assert_eq!(
            std::fs::read(root.path().join("plugins/gamer-ai-0.1.0.gplugin")).unwrap(),
            b"unchanged historical bytes"
        );
    }
}
