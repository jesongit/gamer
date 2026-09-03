//! Phase 10 management-only extension contract.
//!
//! The Phase 6 lifecycle endpoints remain the source of truth for bytes and
//! runtime transitions. This adapter adds the read-only dependency view and a
//! pre-install inspection response used by the browser confirmation flow.

use axum::body::Bytes;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::extensions::ExtensionError;

use super::{ApiError, AppState};

pub(super) async fn api_extension_management(State(st): State<AppState>) -> Response {
    let extensions = match st.extensions.list() {
        Ok(value) => value,
        Err(error) => return extension_error(error),
    };
    let tasks = match st.db.list_timer_tasks_async().await {
        Ok(value) => value,
        Err(error) => return ApiError::internal(error.to_string()).into_response(),
    };

    let extension_json = extensions
        .iter()
        .map(|snapshot| {
            let id = snapshot.id().to_string();
            let dependents = tasks
                .iter()
                .filter(|task| task.runner_id == id)
                .map(|task| {
                    serde_json::json!({
                        "id": task.id,
                        "name": task.name,
                        "kind": "task",
                        "state": task.state,
                        "app_package": task.app.content_package,
                    })
                })
                .collect::<Vec<_>>();
            serde_json::json!({
                "id": id,
                "version": snapshot.active_version(),
                "active_version": snapshot.active_version(),
                "installed_versions": snapshot.installed_versions(),
                "name": snapshot.manifest().name(),
                "description": snapshot.manifest().description(),
                "state": snapshot.state(),
                "last_error": snapshot.last_error(),
                "host_api": snapshot.manifest().host_api().iter()
                    .map(|(domain, requirement)| (domain.to_string(), requirement.to_string()))
                    .collect::<std::collections::BTreeMap<_, _>>(),
                "permissions": snapshot.manifest().permissions().names(),
                "ui": snapshot.manifest().ui().iter().map(ui_json).collect::<Vec<_>>(),
                "source": "unknown",
                "signature": { "status": "unknown" },
                "dependent": {
                    "app_packages": dependents.iter()
                        .filter_map(|item| item.get("app_package"))
                        .filter(|value| !value.is_null())
                        .map(|value| serde_json::json!({
                            "id": value,
                            "kind": "app_package",
                        }))
                        .collect::<Vec<_>>(),
                    "tasks": dependents,
                    "workflows": [],
                },
            })
        })
        .collect::<Vec<_>>();

    let dependencies = extension_json
        .iter()
        .filter_map(|item| {
            item.get("id").and_then(|value| value.as_str()).map(|id| {
                (
                    id.to_string(),
                    item.get("dependent")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                )
            })
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    Json(serde_json::json!({
        "schema_version": 1,
        "host_api": crate::extensions::HOST_API_VERSION,
        "runtime_available": st.extensions.runtime_available(),
        "extensions": extension_json,
        "dependencies": dependencies,
    }))
    .into_response()
}

pub(super) async fn api_inspect_extension(State(st): State<AppState>, body: Bytes) -> Response {
    if body.is_empty() {
        return ApiError::bad_request("插件归档不能为空").into_response();
    }
    let inspection = match st.extensions.inspect(&body) {
        Ok(value) => value,
        Err(error) => return extension_error(error),
    };
    let manifest = inspection.manifest();
    let current_permissions = match st.extensions.list() {
        Ok(items) => items
            .into_iter()
            .find(|snapshot| snapshot.id() == manifest.id())
            .map(|snapshot| snapshot.manifest().permissions().names())
            .unwrap_or_default(),
        Err(error) => return extension_error(error),
    };
    let requested_permissions = manifest.permissions().names();
    let added = requested_permissions
        .iter()
        .filter(|permission| !current_permissions.contains(permission))
        .copied()
        .collect::<Vec<_>>();
    let removed = current_permissions
        .iter()
        .filter(|permission| !requested_permissions.contains(permission))
        .copied()
        .collect::<Vec<_>>();
    let unchanged = requested_permissions
        .iter()
        .filter(|permission| current_permissions.contains(permission))
        .copied()
        .collect::<Vec<_>>();
    let already_installed = st
        .extensions
        .list()
        .ok()
        .map(|items| {
            items.iter().any(|snapshot| {
                snapshot.id() == manifest.id()
                    && snapshot
                        .installed_versions()
                        .iter()
                        .any(|version| version == manifest.version())
            })
        })
        .unwrap_or(false);

    Json(serde_json::json!({
        "id": manifest.id(),
        "version": manifest.version(),
        "name": manifest.name(),
        "description": manifest.description(),
        "archive_sha256": inspection.archive_sha256(),
        "source": "unknown",
        "publisher": null,
        "signature": { "status": "unknown" },
        "permissions": requested_permissions,
        "permission_diff": { "added": added, "removed": removed, "unchanged": unchanged },
        "host_api": manifest.host_api().iter()
            .map(|(domain, requirement)| (domain.to_string(), requirement.to_string()))
            .collect::<std::collections::BTreeMap<_, _>>(),
        "ui": manifest.ui().iter().map(ui_json).collect::<Vec<_>>(),
        "already_installed": already_installed,
    }))
    .into_response()
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

fn extension_error(error: ExtensionError) -> Response {
    let api_error = match &error {
        ExtensionError::NotInstalled { .. }
        | ExtensionError::VersionNotInstalled { .. }
        | ExtensionError::UiUnavailable { .. } => ApiError::not_found(error.to_string()),
        ExtensionError::InvalidState(_) => ApiError::internal(error.to_string()),
        ExtensionError::AlreadyInstalled { .. } | ExtensionError::InvalidTransition { .. } => {
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
