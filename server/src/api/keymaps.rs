//! Keymap partition CRUD endpoints.
//!
//! Resource IDs use the same shape as scripts: `<pkg>/<file>.yaml`; callers
//! must URL-encode the complete ID when it contains `/`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::common::{require_pkg, run_blocking_api, validate_text_field};
use super::templates::PkgQuery;
use super::{ApiError, AppState};
use crate::keymaps::{self, KeymapDiagnostic};

fn invalid_yaml_response(diagnostics: Vec<KeymapDiagnostic>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": "invalid_yaml",
            "diagnostics": diagnostics,
        })),
    )
        .into_response()
}

fn version_conflict(resource: &str) -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "code": "version_conflict",
            "message": "资源已被其他页面修改，请重新加载后再保存",
            "resource": resource,
            "step_path": "",
            "field": ""
        })),
    )
        .into_response()
}

fn version_required(resource: &str) -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "code": "version_required",
            "message": "更新资源必须提供 expected_version，或显式 force:true",
            "resource": resource,
            "step_path": "",
            "field": "expected_version"
        })),
    )
        .into_response()
}

fn validate_keymap_content(content: &str) -> Result<(), ApiError> {
    if content.trim().is_empty() {
        return Err(ApiError::bad_request("映射 YAML 内容不能为空"));
    }
    if content.len() > keymaps::MAX_KEYMAP_YAML_BYTES {
        return Err(ApiError::bad_request("映射 YAML 内容超过 1 MiB"));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SaveKeymapReq {
    /// 目标应用分区（应用包名）。
    pkg: String,
    /// 文件名，缺少 `.yaml`/`.yml` 时自动补 `.yaml`。
    name: String,
    content: String,
}

/// GET /api/keymaps?pkg=<package>：列出指定应用分区的映射方案。
pub(super) async fn api_list_keymaps(
    State(st): State<AppState>,
    Query(q): Query<PkgQuery>,
) -> Response {
    let pkg = match require_pkg(q.pkg.as_deref()) {
        Ok(pkg) => pkg,
        Err(error) => return error.into_response(),
    };
    match run_blocking_api(move || {
        st.keymaps
            .list(&pkg)
            .map_err(|error| ApiError::internal(error.to_string()))
    })
    .await
    {
        Ok(list) => Json(list).into_response(),
        Err(error) => error.into_response(),
    }
}

/// POST /api/keymaps：只创建，不覆盖同名文件。
pub(super) async fn api_create_keymap(
    State(st): State<AppState>,
    Json(req): Json<SaveKeymapReq>,
) -> Response {
    if let Err(error) = validate_text_field(&req.name, "映射方案文件名", 255) {
        return error.into_response();
    }
    if let Err(error) = validate_keymap_content(&req.content) {
        return error.into_response();
    }
    let pkg = match require_pkg(Some(&req.pkg)) {
        Ok(pkg) => pkg,
        Err(error) => return error.into_response(),
    };
    let resource = format!("{pkg}/{}", req.name.trim());
    let parsed = keymaps::parse_keymap_content(&req.content, &resource);
    let keymap = match parsed {
        Ok(keymap) => keymap,
        Err(diagnostics) => return invalid_yaml_response(diagnostics),
    };
    match run_blocking_api(move || {
        st.keymaps
            .create(&pkg, &req.name, &keymap)
            .map_err(|error| write_api_error(error, &resource))
    })
    .await
    {
        Ok(file) => Json(file).into_response(),
        Err(error) => error.into_response(),
    }
}

/// GET /api/keymaps/:id：返回规范化 YAML 原文和结构化模型。
pub(super) async fn api_get_keymap(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match run_blocking_api(move || {
        st.keymaps
            .get(&id)
            .map_err(|error| ApiError::internal(error.to_string()))
    })
    .await
    {
        Ok(Some(file)) => Json(file).into_response(),
        Ok(None) => ApiError::not_found("映射方案不存在").into_response(),
        Err(error) => error.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateKeymapReq {
    /// 可选：在同一应用分区内重命名文件。
    #[serde(default)]
    name: Option<String>,
    content: String,
    #[serde(default)]
    expected_version: Option<String>,
    #[serde(default)]
    force: bool,
}

/// PUT /api/keymaps/:id：更新或重命名已有映射方案。
pub(super) async fn api_update_keymap(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateKeymapReq>,
) -> Response {
    if let Err(error) = validate_keymap_content(&req.content) {
        return error.into_response();
    }
    let source = match run_blocking_api({
        let st = st.clone();
        let id = id.clone();
        move || {
            st.keymaps
                .get(&id)
                .map_err(|error| ApiError::internal(error.to_string()))
        }
    })
    .await
    {
        Ok(Some(file)) => file,
        Ok(None) => return ApiError::not_found("映射方案不存在").into_response(),
        Err(error) => return error.into_response(),
    };
    if !req.force && req.expected_version.is_none() {
        return version_required(&id);
    }
    if !req.force && req.expected_version.as_deref() != Some(source.version.as_str()) {
        return version_conflict(&id);
    }
    let pkg = match id.split_once('/') {
        Some((pkg, _)) => match require_pkg(Some(pkg)) {
            Ok(pkg) => pkg,
            Err(error) => return error.into_response(),
        },
        None => return ApiError::not_found("映射方案不存在").into_response(),
    };
    if let Some(name) = &req.name {
        if let Err(error) = validate_text_field(name, "映射方案文件名", 255) {
            return error.into_response();
        }
    }
    let resource = req
        .name
        .as_deref()
        .map(|name| format!("{pkg}/{name}"))
        .unwrap_or_else(|| id.clone());
    let keymap = match keymaps::parse_keymap_content(&req.content, &resource) {
        Ok(keymap) => keymap,
        Err(diagnostics) => return invalid_yaml_response(diagnostics),
    };
    match run_blocking_api(move || {
        st.keymaps
            .update(
                &id,
                req.name.as_deref(),
                &keymap,
                req.expected_version.as_deref(),
                req.force,
            )
            .map_err(|error| write_api_error(error, &resource))
    })
    .await
    {
        Ok(file) => Json(file).into_response(),
        Err(error) => error.into_response(),
    }
}

/// DELETE /api/keymaps/:id：删除一个映射方案。
pub(super) async fn api_delete_keymap(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let exists = match run_blocking_api({
        let st = st.clone();
        let id = id.clone();
        move || {
            st.keymaps
                .get(&id)
                .map_err(|error| ApiError::internal(error.to_string()))
        }
    })
    .await
    {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(error) => return error.into_response(),
    };
    if !exists {
        return ApiError::not_found("映射方案不存在").into_response();
    }
    match run_blocking_api(move || {
        st.keymaps
            .delete(&id)
            .map_err(|error| write_api_error(error, &id))
    })
    .await
    {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(error) => error.into_response(),
    }
}

fn write_api_error(error: anyhow::Error, resource: &str) -> ApiError {
    let message = error.to_string();
    if message.contains("版本冲突") {
        ApiError::new(
            StatusCode::CONFLICT,
            format!("version_conflict: {resource}"),
        )
    } else if message.contains("必须提供 expected_version") {
        ApiError::new(
            StatusCode::CONFLICT,
            format!("version_required: {resource}"),
        )
    } else if message.contains("已存在") {
        ApiError::conflict(message)
    } else if message.contains("不存在") {
        ApiError::not_found(message)
    } else {
        ApiError::bad_request(message)
    }
}
