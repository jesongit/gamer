//! Phase 10 management-only extension contract.
//!
//! The Phase 6 lifecycle endpoints remain the source of truth for bytes and
//! runtime transitions. This adapter adds the read-only dependency view and a
//! pre-install inspection response used by the browser confirmation flow.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::extensions::{ExtensionError, ExtensionInstallContext};

use super::{ApiError, AppState};

/// 与包级导入 `x-expected-sha256` 同名同义（可选）：请求声明的归档
/// SHA-256（hex），服务端在安装前核对，不匹配拒绝（Phase 1 免签名后的
/// 下载完整性兜底）。
const EXPECTED_SHA256_HEADER: &str = "x-expected-sha256";

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
                "execution": snapshot.manifest().execution(),
                "ui": snapshot.manifest().ui().iter().map(ui_json).collect::<Vec<_>>(),
                "source": "local",
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

pub(super) async fn api_inspect_extension(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.is_empty() {
        return ApiError::bad_request("插件归档不能为空").into_response();
    }
    let context = match install_context(&headers) {
        Ok(context) => context,
        Err(response) => return response,
    };
    let inspection = match st.extensions.inspect_with_context(&body, &context) {
        Ok(value) => value,
        Err(error) => return extension_error(error),
    };
    let manifest = inspection.manifest();
    let requested_permissions = manifest.permissions().names();
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
        "source": if context.official { "official" } else { "local" },
        "publisher": null,
        // Phase 1 免签名：不再有 signature 字段；执行类型与 advisory 宿主
        // 版本要求（execution.host_version）透传前端展示。
        "execution": manifest.execution(),
        // Phase 8 §11.2：执行形态变化（wasm↔builtin）显式暴露，确认弹窗
        // 必须提示（builtin→wasm 已在服务端拒绝，这里只会出现 wasm→builtin）。
        "execution_change": inspection.execution_change().map(|change| {
            serde_json::json!({ "from": change.from, "to": change.to })
        }),
        "permissions": requested_permissions,
        "permission_diff": inspection.permission_diff(),
        "host_api": manifest.host_api().iter()
            .map(|(domain, requirement)| (domain.to_string(), requirement.to_string()))
            .collect::<std::collections::BTreeMap<_, _>>(),
        "ui": manifest.ui().iter().map(ui_json).collect::<Vec<_>>(),
        "already_installed": already_installed,
    }))
    .into_response()
}

/// Parse the management boundary once so inspect, install, and update share
/// exactly the same official-source, integrity-pin, and confirmation
/// semantics. 签名/Registry proof 已退役：`official` 只作来源标注。
/// axum `Response` 体量超过 clippy result_large_err 阈值；错误即响应，
/// 箱化不改变语义只增分配。
#[allow(clippy::result_large_err)]
pub(super) fn install_context(headers: &HeaderMap) -> Result<ExtensionInstallContext, Response> {
    let official = headers
        .get("x-gamer-extension-source")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("official"));
    let expected_sha256 = match headers.get(EXPECTED_SHA256_HEADER) {
        Some(value) => {
            let value = match value.to_str() {
                Ok(value) => value.trim().to_ascii_lowercase(),
                Err(error) => {
                    return Err(ApiError::bad_request(format!(
                        "{EXPECTED_SHA256_HEADER} 头无效: {error}"
                    ))
                    .into_response())
                }
            };
            if !(value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())) {
                return Err(ApiError::bad_request(format!(
                    "{EXPECTED_SHA256_HEADER} 必须是 64 位 hex SHA-256"
                ))
                .into_response());
            }
            Some(value)
        }
        None => None,
    };
    let permission_confirmed = headers
        .get("x-gamer-permission-confirm")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| matches!(value, "1" | "true" | "yes"));
    Ok(ExtensionInstallContext {
        official,
        permission_confirmed,
        expected_sha256,
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

fn extension_error(error: ExtensionError) -> Response {
    let api_error = match &error {
        ExtensionError::NotInstalled { .. }
        | ExtensionError::VersionNotInstalled { .. }
        | ExtensionError::UiUnavailable { .. } => ApiError::not_found(error.to_string()),
        ExtensionError::InvalidState(_) => ApiError::internal(error.to_string()),
        ExtensionError::AlreadyInstalled { .. } | ExtensionError::InvalidTransition { .. } => {
            ApiError::conflict(error.to_string())
        }
        ExtensionError::PermissionConfirmationRequired(_)
        | ExtensionError::HostFeatureUnavailable(_) => ApiError::conflict(error.to_string()),
        ExtensionError::ArchiveSha256Mismatch { .. } => ApiError::bad_request(error.to_string()),
        ExtensionError::RuntimeUnavailable(_) => ApiError::service_unavailable(error.to_string()),
        ExtensionError::Io(_)
        | ExtensionError::Json(_)
        | ExtensionError::Zip(_)
        | ExtensionError::Runtime(_) => ApiError::internal(error.to_string()),
        _ => ApiError::bad_request(error.to_string()),
    };
    api_error.into_response()
}
