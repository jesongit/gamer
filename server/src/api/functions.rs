//! 函数库（data/<pkg>/functions/）CRUD 路由：风格对齐 scripts 路由（契约 plan §13.1）。
//!
//! - id = `<pkg>/<文件短路径>.yaml`（含 `/`，前端拼 URL 必须整体 encodeURIComponent，
//!   axum 对 %2F 解码，与 scripts 路由同规则）；
//! - GET 返回内容版本短码 version；POST 只创建，PUT 更新/重命名。PUT 默认要求
//!   expected_version，不提供时只有 force:true 才能跳过版本门禁；
//! - 严格 loader 失败返回统一结构化五元组诊断；
//! - functions/ 函数库不进脚本列表/运行接口/任务选择器（数据源物理隔离，测试锁死）。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::common::{require_pkg, run_blocking_api, validate_text_field};
use super::templates::PkgQuery;
use super::{ApiError, AppState};
use crate::core::fs::archive_validation::IMPORT_MAX_YAML_BYTES;

/// 版本冲突 409：CONTRACT §5 错误结构（resource 级错误，step_path/field 留空）。
/// scripts 与 functions 保存接口共用。
pub(super) fn version_conflict(resource: &str) -> Response {
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

pub(super) fn version_required(resource: &str) -> Response {
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

fn invalid_yaml_response(
    diagnostics: Vec<crate::extensions::gamer_yaml::script_v2::ScriptError>,
) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": "invalid_yaml",
            "diagnostics": diagnostics,
        })),
    )
        .into_response()
}

/// 校验函数文件内容：非空、≤1MiB（与脚本同限）
pub(super) fn validate_function_content(content: &str) -> Result<(), ApiError> {
    if content.trim().is_empty() {
        return Err(ApiError::bad_request("函数文件内容不能为空"));
    }
    if content.len() > IMPORT_MAX_YAML_BYTES {
        return Err(ApiError::bad_request("函数文件内容超过 1 MiB"));
    }
    Ok(())
}

// ---------- /api/functions ----------

/// GET /api/functions?pkg=<分区>（pkg 必填）：
/// 列出分区全部函数库文件（文件短路径 + 顶层函数名清单 + version）
pub(super) async fn api_list_functions(
    State(st): State<AppState>,
    Query(q): Query<PkgQuery>,
) -> Response {
    let pkg = match require_pkg(q.pkg.as_deref()) {
        Ok(pkg) => pkg,
        Err(err) => return err.into_response(),
    };
    match run_blocking_api(move || {
        st.scripts
            .list_functions(&pkg)
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(list) => Json(list).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SaveFunctionReq {
    /// 目标应用分区（设备配置的应用包名）
    pkg: String,
    /// 函数文件短路径（缺 .yaml 扩展名自动补全）
    name: String,
    content: String,
}

/// POST /api/functions：只创建，不覆盖已有函数库文件。
pub(super) async fn api_create_function(
    State(st): State<AppState>,
    Json(req): Json<SaveFunctionReq>,
) -> Response {
    if let Err(err) = validate_text_field(&req.name, "函数文件名", 255) {
        return err.into_response();
    }
    if let Err(err) = validate_function_content(&req.content) {
        return err.into_response();
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
        Ok(st_parse
            .scripts
            .parse_function_content(&parse_pkg, &parse_name, &parse_content))
    })
    .await;
    match parsed {
        Err(err) => return err.into_response(),
        Ok(Err(diagnostics)) => return invalid_yaml_response(diagnostics),
        Ok(Ok(_)) => {}
    }
    let name = req.name.clone();
    let resource = format!(
        "{}/{}",
        pkg,
        if name.to_lowercase().ends_with(".yaml") {
            name.clone()
        } else {
            format!("{name}.yaml")
        }
    );
    match run_blocking_api(move || {
        if st
            .scripts
            .function_version_for_save(&pkg, &name)
            .map_err(|e| ApiError::bad_request(e.to_string()))?
            .is_some()
        {
            return Err(ApiError::conflict(format!("资源已存在: {resource}")));
        }
        st.scripts
            .save_function(&pkg, &req.name, &req.content)
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
        Ok(f) => Json(serde_json::json!({
            "ok": true,
            "id": f.id,
            "pkg": f.pkg,
            "file": f.file,
            "version": f.version,
            "functions": f.functions,
        }))
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/functions/:id：读取单个函数库文件（含 content / version / 函数名清单）
pub(super) async fn api_get_function(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match run_blocking_api(move || {
        st.scripts
            .get_function(&id)
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(Some(f)) => Json(f).into_response(),
        Ok(None) => ApiError::not_found("函数文件不存在").into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateFunctionReq {
    /// 可选：不提供时沿用原文件名；提供时执行同分区重命名。
    #[serde(default)]
    name: Option<String>,
    content: String,
    #[serde(default)]
    expected_version: Option<String>,
    #[serde(default)]
    force: bool,
}

/// PUT /api/functions/:id：更新已有函数库，可选重命名；重命名也检查源版本。
pub(super) async fn api_update_function(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateFunctionReq>,
) -> Response {
    if let Err(err) = validate_function_content(&req.content) {
        return err.into_response();
    }
    let check_id = id.clone();
    let st_check = st.clone();
    let check = run_blocking_api(move || {
        let Some(f) = st_check
            .scripts
            .get_function(&check_id)
            .map_err(|e| ApiError::internal(e.to_string()))?
        else {
            return Err(ApiError::not_found("函数文件不存在"));
        };
        Ok(f)
    })
    .await;
    match check {
        Err(e) => return e.into_response(),
        Ok(f) => {
            if !req.force && req.expected_version.is_none() {
                return version_required(&id);
            }
            if !req.force && req.expected_version.as_deref() != Some(f.version.as_str()) {
                return version_conflict(&id);
            }
        }
    }
    let (pkg, rel) = match id.split_once('/') {
        Some((pkg, rel)) => (pkg.to_string(), rel.to_string()),
        None => return ApiError::bad_request("非法函数文件 id").into_response(),
    };
    let target_name = req.name.clone().unwrap_or(rel);
    if let Err(err) = validate_text_field(&target_name, "函数文件名", 255) {
        return err.into_response();
    }
    let parse_pkg = pkg.clone();
    let parse_name = target_name.clone();
    let parse_content = req.content.clone();
    let st_parse = st.clone();
    let parsed = run_blocking_api(move || {
        Ok(st_parse
            .scripts
            .parse_function_content(&parse_pkg, &parse_name, &parse_content))
    })
    .await;
    match parsed {
        Err(err) => return err.into_response(),
        Ok(Err(diagnostics)) => return invalid_yaml_response(diagnostics),
        Ok(Ok(_)) => {}
    }
    match run_blocking_api(move || {
        st.scripts
            .update_function(&id, Some(&target_name), &req.content)
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
        Ok(f) => Json(serde_json::json!({
            "ok": true,
            "id": f.id,
            "pkg": f.pkg,
            "file": f.file,
            "version": f.version,
            "functions": f.functions,
        }))
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// DELETE /api/functions/:id：删除函数库文件（不存在 → 404）
pub(super) async fn api_delete_function(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match run_blocking_api(move || {
        if st
            .scripts
            .get_function(&id)
            .map_err(|e| ApiError::internal(e.to_string()))?
            .is_none()
        {
            return Err(ApiError::not_found("函数文件不存在"));
        }
        st.scripts
            .delete_function(&id)
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => e.into_response(),
    }
}
