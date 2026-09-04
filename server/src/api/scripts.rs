//! Script file storage endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::common::{require_pkg, run_blocking_api, validate_text_field};
use super::{ApiError, AppState};

// ---------- 脚本 ----------

pub(super) async fn api_list_scripts(State(st): State<AppState>) -> Response {
    match run_blocking_api(move || {
        st.scripts
            .list()
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(s) => Json(s).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SaveScriptReq {
    name: String,
    content: String,
    /// 目标应用分区（设备配置的应用包名）
    pkg: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateScriptReq {
    /// 可选：不提供时沿用原文件名；提供时执行同分区重命名。
    #[serde(default)]
    name: Option<String>,
    content: String,
    /// 更新默认必须带当前内容版本；force=true 才跳过版本门禁。
    #[serde(default)]
    expected_version: Option<String>,
    #[serde(default)]
    force: bool,
}

fn invalid_yaml_json_response(diagnostics: serde_json::Value) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": "invalid_yaml",
            "diagnostics": diagnostics,
        })),
    )
        .into_response()
}

fn script_resource_name(name: &str) -> String {
    let name = name.trim();
    if name.to_lowercase().ends_with(".yaml") || name.to_lowercase().ends_with(".yml") {
        name.to_string()
    } else {
        format!("{name}.yaml")
    }
}

/// POST /api/scripts：只创建，不覆盖已有脚本。
pub(super) async fn api_create_script(
    State(st): State<AppState>,
    Json(req): Json<SaveScriptReq>,
) -> Response {
    if let Err(err) = validate_text_field(&req.name, "脚本名", 255) {
        return err.into_response();
    }
    if req.content.trim().is_empty() {
        return ApiError::bad_request("脚本内容不能为空").into_response();
    }
    if req.content.len() > crate::core::fs::archive_validation::IMPORT_MAX_YAML_BYTES {
        return ApiError::bad_request("脚本内容超过 1 MiB").into_response();
    }
    let pkg = match require_pkg(Some(&req.pkg)) {
        Ok(pkg) => pkg,
        Err(err) => return err.into_response(),
    };
    let parse_pkg = pkg.clone();
    let parse_name = req.name.clone();
    let parse_content = req.content.clone();
    let st_parse = st.clone();
    let parsed = run_blocking_api(move || {
        Ok(
            crate::extensions::gamer_yaml::yaml_extension::validate_compatible_script(
                &st_parse.scripts,
                &parse_pkg,
                &parse_name,
                &parse_content,
            ),
        )
    })
    .await;
    match parsed {
        Err(err) => return err.into_response(),
        Ok(Err(error)) => return invalid_yaml_json_response(error.into_json()),
        Ok(Ok(_)) => {}
    }
    let name = req.name.clone();
    let name_for_response = script_resource_name(&name);
    match run_blocking_api(move || {
        let resource = format!("{pkg}/{name_for_response}");
        if st
            .scripts
            .script_version_for_save(None, &pkg, &name)
            .map_err(|e| ApiError::bad_request(e.to_string()))?
            .is_some()
        {
            return Err(ApiError::conflict(format!("资源已存在: {resource}")));
        }
        st.scripts
            .save(None, &pkg, &name, &req.content)
            .map_err(|e| {
                if e.to_string().contains("已存在") {
                    ApiError::conflict(e.to_string())
                } else {
                    ApiError::bad_request(e.to_string())
                }
            })
    })
    .await
    {
        Ok(s) => Json(serde_json::json!({
            "ok": true,
            "id": s.id,
            "package": s.package,
            "name": s.name,
            "version": s.version(),
        }))
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// PUT /api/scripts/:id：只更新已有脚本；重命名仍以源文件版本做检查。
pub(super) async fn api_update_script(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateScriptReq>,
) -> Response {
    if req.content.trim().is_empty() {
        return ApiError::bad_request("脚本内容不能为空").into_response();
    }
    if req.content.len() > crate::core::fs::archive_validation::IMPORT_MAX_YAML_BYTES {
        return ApiError::bad_request("脚本内容超过 1 MiB").into_response();
    }
    let (pkg, source_name) = match id.split_once('/') {
        Some((pkg, name)) => match require_pkg(Some(pkg)) {
            Ok(pkg) => (pkg, name.to_string()),
            Err(err) => return err.into_response(),
        },
        None => return ApiError::not_found("脚本不存在").into_response(),
    };
    let source = match run_blocking_api({
        let st = st.clone();
        let id = id.clone();
        move || {
            st.scripts
                .get(&id)
                .map_err(|e| ApiError::internal(e.to_string()))
        }
    })
    .await
    {
        Ok(Some(script)) => script,
        Ok(None) => return ApiError::not_found("脚本不存在").into_response(),
        Err(err) => return err.into_response(),
    };
    if !req.force && req.expected_version.is_none() {
        return super::functions::version_required(&id);
    }
    if !req.force {
        let current = source.version();
        if req.expected_version.as_deref() != Some(current.as_str()) {
            return super::functions::version_conflict(&id);
        }
    }
    let target_name = req.name.clone().unwrap_or(source_name);
    if let Err(err) = validate_text_field(&target_name, "脚本名", 255) {
        return err.into_response();
    }
    let parse_pkg = pkg.clone();
    let parse_name = target_name.clone();
    let parse_content = req.content.clone();
    let st_parse = st.clone();
    let parsed = run_blocking_api(move || {
        Ok(
            crate::extensions::gamer_yaml::yaml_extension::validate_compatible_script(
                &st_parse.scripts,
                &parse_pkg,
                &parse_name,
                &parse_content,
            ),
        )
    })
    .await;
    match parsed {
        Err(err) => return err.into_response(),
        Ok(Err(error)) => return invalid_yaml_json_response(error.into_json()),
        Ok(Ok(_)) => {}
    }
    match run_blocking_api(move || {
        st.scripts
            .save(Some(&id), &pkg, &target_name, &req.content)
            .map_err(|e| {
                if e.to_string().contains("已存在") {
                    ApiError::conflict(e.to_string())
                } else {
                    ApiError::bad_request(e.to_string())
                }
            })
    })
    .await
    {
        Ok(s) => Json(serde_json::json!({
            "ok": true,
            "id": s.id,
            "package": s.package,
            "name": s.name,
            "version": s.version(),
        }))
        .into_response(),
        Err(err) => err.into_response(),
    }
}

/// GET /api/scripts/:id：读取单个脚本（含内容版本短码 version，编辑器冲突检测用）
pub(super) async fn api_get_script(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match run_blocking_api(move || {
        st.scripts
            .get(&id)
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(Some(s)) => Json(s).into_response(),
        Ok(None) => ApiError::not_found("脚本不存在").into_response(),
        Err(e) => e.into_response(),
    }
}

pub(super) async fn api_delete_script(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match run_blocking_api(move || {
        st.scripts
            .delete(&id)
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => e.into_response(),
    }
}
