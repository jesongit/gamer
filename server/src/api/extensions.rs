//! Extension package, lifecycle, and UI contribution REST endpoints.
//!
//! The endpoint owns no plugin state: all mutations go through
//! [`ExtensionService`], which serializes lifecycle transitions and refreshes
//! the in-process UI registry after every mutation.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::{ApiError, AppState};
use crate::extensions::{
    ExtensionError, ExtensionId, ExtensionPath, ExtensionService, ExtensionSnapshot,
    ExtensionVersion,
};

pub(super) async fn api_list_extensions(State(st): State<AppState>) -> Response {
    match st.extensions.list() {
        Ok(extensions) => {
            let ui = match st.extensions.ui_contributions() {
                Ok(ui) => ui,
                Err(error) => return extension_error(error),
            };
            Json(serde_json::json!({
                "runtime_available": st.extensions.runtime_available(),
                "extensions": extensions.iter().map(snapshot_json).collect::<Vec<_>>(),
                "ui_contributions": ui,
            }))
            .into_response()
        }
        Err(error) => extension_error(error),
    }
}

pub(super) async fn api_list_ui_contributions(State(st): State<AppState>) -> Response {
    match st.extensions.ui_contributions() {
        Ok(contributions) => Json(contributions).into_response(),
        Err(error) => extension_error(error),
    }
}

pub(super) async fn api_install_extension(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.is_empty() {
        return ApiError::bad_request("插件归档不能为空").into_response();
    }
    let context = match super::extensions_management::install_context(&headers) {
        Ok(context) => context,
        Err(response) => return response,
    };
    match st.extensions.install_with_context(&body, &context).await {
        Ok(snapshot) => (StatusCode::CREATED, Json(snapshot_json(&snapshot))).into_response(),
        Err(error) => extension_error(error),
    }
}

pub(super) async fn api_update_extension(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.is_empty() {
        return ApiError::bad_request("插件归档不能为空").into_response();
    }
    let context = match super::extensions_management::install_context(&headers) {
        Ok(context) => context,
        Err(response) => return response,
    };
    match st.extensions.update_with_context(&body, &context).await {
        Ok(snapshot) => Json(snapshot_json(&snapshot)).into_response(),
        Err(error) => extension_error(error),
    }
}

pub(super) async fn api_enable_extension(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    lifecycle(&st.extensions, &id, Lifecycle::Enable).await
}

pub(super) async fn api_disable_extension(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    lifecycle(&st.extensions, &id, Lifecycle::Disable).await
}

pub(super) async fn api_start_extension(
    State(st): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let request = if body.is_empty() {
        StartExtensionRequest::default()
    } else {
        match serde_json::from_slice::<StartExtensionRequest>(&body) {
            Ok(request) => request,
            Err(error) => return ApiError::bad_request(error.to_string()).into_response(),
        }
    };
    lifecycle(&st.extensions, &id, Lifecycle::Start(request.app_context)).await
}

pub(super) async fn api_stop_extension(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    lifecycle(&st.extensions, &id, Lifecycle::Stop).await
}

pub(super) async fn api_uninstall_extension(
    State(st): State<AppState>,
    Path((id, version)): Path<(String, String)>,
    Query(query): Query<UninstallQuery>,
) -> Response {
    let id = match ExtensionId::parse(&id) {
        Ok(id) => id,
        Err(error) => return extension_error(error),
    };
    let version = match ExtensionVersion::parse(&version) {
        Ok(version) => version,
        Err(error) => return extension_error(error),
    };
    let delete_data = query
        .delete_data
        .as_deref()
        .is_some_and(|value| matches!(value, "1" | "true" | "yes"));
    match st.extensions.uninstall(&id, &version).await {
        Ok(true) => {
            // WASM user data is deliberately outside immutable plugin
            // versions. The management flag deletes only this exact,
            // validated plugin-owned directory; the default keeps it.
            if delete_data {
                let data_root = st
                    .extensions
                    .store()
                    .data_root()
                    .join("extension-data")
                    .join(id.as_str());
                match std::fs::symlink_metadata(&data_root) {
                    Ok(metadata) if metadata.is_dir() => {
                        if let Err(error) = std::fs::remove_dir_all(&data_root) {
                            return ApiError::internal(format!(
                                "插件已卸载，但用户数据删除失败: {error}"
                            ))
                            .into_response();
                        }
                    }
                    Ok(_) => return ApiError::internal("插件用户数据路径不是目录").into_response(),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return ApiError::internal(error.to_string()).into_response(),
                }
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => ApiError::not_found("插件版本不存在").into_response(),
        Err(error) => extension_error(error),
    }
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct UninstallQuery {
    #[serde(default)]
    delete_data: Option<String>,
}

pub(super) async fn api_get_extension_ui_asset(
    State(st): State<AppState>,
    Path((id, path)): Path<(String, String)>,
) -> Response {
    let id = match ExtensionId::parse(&id) {
        Ok(id) => id,
        Err(error) => return extension_error(error),
    };
    let path = match ExtensionPath::parse(&format!("ui/{path}")) {
        Ok(path) => path,
        Err(error) => return extension_error(error),
    };
    match st.extensions.read_ui_file(&id, &path) {
        Ok((bytes, _)) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, content_type(path.as_str()))],
            bytes,
        )
            .into_response(),
        Err(error) => extension_error(error),
    }
}

#[derive(Clone)]
enum Lifecycle {
    Enable,
    Disable,
    Start(Option<crate::core::AppContext>),
    Stop,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartExtensionRequest {
    #[serde(default)]
    app_context: Option<crate::core::AppContext>,
}

async fn lifecycle(service: &ExtensionService, raw_id: &str, operation: Lifecycle) -> Response {
    let id = match ExtensionId::parse(raw_id) {
        Ok(id) => id,
        Err(error) => return extension_error(error),
    };
    let result = match operation {
        Lifecycle::Enable => service.enable(&id).await,
        Lifecycle::Disable => service.disable(&id).await,
        Lifecycle::Start(app_context) => service.start_with_context(&id, app_context).await,
        Lifecycle::Stop => service.stop(&id).await,
    };
    match result {
        Ok(snapshot) => Json(snapshot_json(&snapshot)).into_response(),
        Err(error) => extension_error(error),
    }
}

fn snapshot_json(snapshot: &ExtensionSnapshot) -> serde_json::Value {
    let manifest = snapshot.manifest();
    let host_api = manifest
        .host_api()
        .iter()
        .map(|(domain, requirement)| (domain.to_string(), requirement.to_string()))
        .collect::<std::collections::BTreeMap<_, _>>();
    serde_json::json!({
        "id": snapshot.id(),
        "version": manifest.version(),
        "active_version": snapshot.active_version(),
        "installed_versions": snapshot.installed_versions(),
        "name": manifest.name(),
        "description": manifest.description(),
        "entry": manifest.entry().as_str(),
        "state": snapshot.state(),
        "last_error": snapshot.last_error(),
        "signature": snapshot.signature(),
        "host_api": host_api,
        "permissions": manifest.permissions().names(),
        "ui": manifest.ui().iter().map(ui_json).collect::<Vec<_>>(),
    })
}

fn ui_json(contribution: &crate::extensions::UiContribution) -> serde_json::Value {
    serde_json::json!({
        "panel_id": contribution.panel_id(),
        "title": contribution.title(),
        "icon": contribution.icon(),
        "order": contribution.order(),
        "location": contribution.location(),
        "runtime": contribution.runtime(),
        "requires_device": contribution.requires_device(),
        "preferred_width": contribution.preferred_width(),
        "entry": contribution.entry().map(|entry| entry.as_str()),
    })
}

fn content_type(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "application/octet-stream",
    }
}

fn extension_error(error: ExtensionError) -> Response {
    let api_error = match &error {
        ExtensionError::NotInstalled { .. }
        | ExtensionError::VersionNotInstalled { .. }
        | ExtensionError::UiUnavailable { .. } => ApiError::not_found(error.to_string()),
        ExtensionError::InvalidState(_) => ApiError::internal(error.to_string()),
        ExtensionError::AlreadyInstalled { .. } | ExtensionError::InvalidTransition { .. } => {
            ApiError::conflict(error.to_string())
        }
        ExtensionError::RegistryProofRequired
        | ExtensionError::PermissionConfirmationRequired(_) => {
            ApiError::conflict(error.to_string())
        }
        ExtensionError::RuntimeUnavailable(_) => ApiError::service_unavailable(error.to_string()),
        ExtensionError::Io(_)
        | ExtensionError::Json(_)
        | ExtensionError::Zip(_)
        | ExtensionError::Runtime(_) => ApiError::internal(error.to_string()),
        _ => ApiError::bad_request(error.to_string()),
    };
    api_error.into_response()
}
