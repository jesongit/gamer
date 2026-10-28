//! Management-only external controller lifecycle. Mount exclusively inside the
//! existing admin-authenticated router; none of these start/resume operations
//! are contributed as MCP tools.
use super::{ApiError, AppState};
use crate::{
    core::AppPackageId,
    external_control::{SessionBinding, SessionRecord, StartRequest},
    mcp::{ErrorKind, ExternalPrincipal, McpError},
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StartControlRequest {
    token_id: String,
    device_id: String,
    content_package: Option<String>,
    #[serde(default = "default_ttl")]
    ttl_seconds: u64,
}

fn default_ttl() -> u64 {
    300
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PauseControlRequest {
    generation: u64,
}

fn authorize(
    principal: &ExternalPrincipal,
    device_id: &str,
    package_id: Option<&str>,
) -> Result<(), ApiError> {
    if !principal.has_scope("control") {
        return Err(ApiError::forbidden("所选令牌没有 control 权限").with_code("scope_denied"));
    }
    principal
        .allowed
        .authorize("device", device_id)
        .map_err(mcp_error)?;
    if let Some(package_id) = package_id {
        principal
            .allowed
            .authorize("package", package_id)
            .map_err(mcp_error)?;
    }
    Ok(())
}

fn mcp_error(error: McpError) -> ApiError {
    let status = match error.kind {
        ErrorKind::Unauthorized => StatusCode::UNAUTHORIZED,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        ErrorKind::InvalidParams => StatusCode::BAD_REQUEST,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Conflict => StatusCode::CONFLICT,
        ErrorKind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    ApiError::new(status, error.to_string())
}

fn control_error(error: anyhow::Error) -> Response {
    let message = error.to_string();
    if message.starts_with("session_not_found") {
        ApiError::not_found(message)
            .with_code("session_not_found")
            .into_response()
    } else if message.starts_with("token_inactive") || message.starts_with("scope_denied") {
        ApiError::forbidden(message)
            .with_code("scope_denied")
            .into_response()
    } else if message.starts_with("control_cleanup_pending") {
        (
            StatusCode::ACCEPTED,
            Json(json!({"pending":true,"code":"control_cleanup_pending","message":message})),
        )
            .into_response()
    } else if [
        "stale_generation",
        "control_",
        "run_rejected",
        "session_stopped",
        "session_expired",
        "target_changed",
        "target_disconnected",
    ]
    .iter()
    .any(|prefix| message.starts_with(prefix))
    {
        ApiError::conflict(message).into_response()
    } else {
        ApiError::bad_request(message).into_response()
    }
}

pub(super) async fn list(State(st): State<AppState>) -> Response {
    Json(json!({"sessions":st.external_control.list_for_user()})).into_response()
}

pub(super) async fn start(
    State(st): State<AppState>,
    Json(request): Json<StartControlRequest>,
) -> Response {
    let principal = match st.mcp.credentials().principal_for_id(&request.token_id) {
        Ok(principal) => principal,
        Err(error) => return mcp_error(error).into_response(),
    };
    let package = match request
        .content_package
        .as_deref()
        .map(AppPackageId::new)
        .transpose()
    {
        Ok(package) => package,
        Err(error) => return ApiError::bad_request(error.to_string()).into_response(),
    };
    // Android application identity is read from device configuration. It cannot
    // be supplied as an owner/AppContext in the request body.
    let app = match crate::targets::app_context(&st.devices, &request.device_id, package) {
        Ok(app) => app,
        Err(error) => return ApiError::bad_request(error.to_string()).into_response(),
    };
    if let Err(error) = authorize(
        &principal,
        app.device_id.as_str(),
        app.content_package.as_ref().map(|id| id.as_str()),
    ) {
        return error.into_response();
    }
    match st
        .external_control
        .start_for_user(StartRequest {
            token_id: principal.token_id,
            token_label: principal.label,
            device_id: app.device_id.as_str().into(),
            content_package: app.content_package.as_ref().map(|id| id.as_str().into()),
            ttl_seconds: request.ttl_seconds,
        })
        .await
    {
        Ok(session) => (StatusCode::ACCEPTED, Json(json!({"session":session}))).into_response(),
        Err(error) => control_error(error),
    }
}

pub(super) async fn pause(
    State(st): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<PauseControlRequest>,
) -> Response {
    let session = match st.external_control.status_for_user(&session_id) {
        Ok(session) => session,
        Err(error) => return control_error(error),
    };
    match st
        .external_control
        .pause(
            &session.token_id,
            SessionBinding {
                session_id,
                generation: request.generation,
            },
        )
        .await
    {
        Ok(session) => Json(json!({"session":session})).into_response(),
        Err(error) => control_error(error),
    }
}

fn authorize_resume(st: &AppState, session: &SessionRecord) -> Result<(), ApiError> {
    let principal = st
        .mcp
        .credentials()
        .principal_for_id(&session.token_id)
        .map_err(mcp_error)?;
    authorize(
        &principal,
        &session.device_id,
        session.content_package.as_deref(),
    )
}

pub(super) async fn resume(State(st): State<AppState>, Path(session_id): Path<String>) -> Response {
    let session = match st.external_control.status_for_user(&session_id) {
        Ok(session) => session,
        Err(error) => return control_error(error),
    };
    if let Err(error) = authorize_resume(&st, &session) {
        return error.into_response();
    }
    match st.external_control.resume_for_user(&session_id).await {
        Ok(session) => Json(json!({"session":session})).into_response(),
        Err(error) => control_error(error),
    }
}

pub(super) async fn stop(State(st): State<AppState>, Path(session_id): Path<String>) -> Response {
    // User stop remains available after credential expiration/revocation.
    match st.external_control.stop_for_user(&session_id).await {
        Ok(session) => Json(json!({"session":session})).into_response(),
        Err(error) => control_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::AllowedObjects;

    #[test]
    fn start_and_resume_require_control_and_exact_target_package_ranges() {
        let mut principal = ExternalPrincipal {
            token_id: "one".into(),
            label: "display-only".into(),
            scopes: ["observe".into()].into(),
            allowed: AllowedObjects {
                devices: ["target".into()].into(),
                packages: ["data".into()].into(),
                ..Default::default()
            },
            expires_at: i64::MAX,
        };
        assert!(authorize(&principal, "target", Some("data")).is_err());
        principal.scopes.insert("control".into());
        assert!(authorize(&principal, "target", Some("data")).is_ok());
        assert!(authorize(&principal, "other", Some("data")).is_err());
        assert!(authorize(&principal, "target", Some("other")).is_err());
    }

    #[test]
    fn management_start_rejects_forged_label_owner_and_android_context() {
        for extra in ["token_label", "owner", "android_package", "app_context"] {
            let mut value = json!({"token_id":"one","device_id":"target","content_package":"data"});
            value[extra] = json!("forged");
            assert!(serde_json::from_value::<StartControlRequest>(value).is_err());
        }
    }
}
