//! Script file storage and partition import/export endpoints.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::common::{require_pkg, run_blocking_api, validate_text_field};
use super::templates::PkgQuery;
use super::{ApiError, AppState};
use crate::matcher;

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

fn invalid_yaml_response(diagnostics: Vec<crate::script_v2::ScriptError>) -> Response {
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
        Ok(st_parse
            .scripts
            .parse_script_content(&parse_pkg, &parse_name, &parse_content))
    })
    .await;
    match parsed {
        Err(err) => return err.into_response(),
        Ok(Err(diagnostics)) => return invalid_yaml_response(diagnostics),
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
        Ok(st_parse
            .scripts
            .parse_script_content(&parse_pkg, &parse_name, &parse_content))
    })
    .await;
    match parsed {
        Err(err) => return err.into_response(),
        Ok(Err(diagnostics)) => return invalid_yaml_response(diagnostics),
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

/// 导出整分区快照 zip（?pkg= 指定应用分区）：yaml/ 全部脚本 + tmpl/ 全部模板
pub(super) async fn api_export_partition(
    State(st): State<AppState>,
    Query(q): Query<PkgQuery>,
) -> Response {
    let pkg = match require_pkg(q.pkg.as_deref()) {
        Ok(pkg) => pkg,
        Err(err) => return err.into_response(),
    };
    match run_blocking_api(move || {
        st.scripts
            .export_partition(&pkg)
            .map_err(|e| ApiError::not_found(e.to_string()))
    })
    .await
    {
        Ok((filename, bytes)) => zip_response(&filename, bytes),
        Err(e) => e.into_response(),
    }
}

/// zip 下载响应：文件名可能是 unicode，用 RFC 5987 filename*
/// （percent-encoded UTF-8），直接塞非 ASCII 进 header 会被 hyper 拒绝
fn zip_response(filename: &str, bytes: Vec<u8>) -> Response {
    let enc: String = filename.bytes().map(|b| format!("%{:02X}", b)).collect();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename*=UTF-8''{}", enc),
        )
        .body(Body::from(bytes))
        .unwrap()
}

#[derive(Deserialize)]
pub(super) struct ImportQuery {
    #[serde(default)]
    confirm: Option<String>,
    /// 目标应用分区（应用包名，必填）
    #[serde(default)]
    pkg: Option<String>,
}

/// 导入分区快照 zip（body 为原始 zip 字节，?pkg= 指定目标分区）。
/// confirm 缺省/false：只解析并返回同名冲突列表（前端二次确认）；
/// confirm=1/true：落盘，同名替换。
pub(super) async fn api_import_script(
    State(st): State<AppState>,
    Query(q): Query<ImportQuery>,
    body: axum::body::Bytes,
) -> Response {
    let confirm = matches!(q.confirm.as_deref(), Some("1") | Some("true"));
    let pkg = match require_pkg(q.pkg.as_deref()) {
        Ok(pkg) => pkg,
        Err(err) => return err.into_response(),
    };
    if confirm {
        let preview = run_blocking_api({
            let st = st.clone();
            let body = body.clone();
            let pkg = pkg.clone();
            move || {
                st.scripts
                    .import(&body, &pkg, false)
                    .map_err(|e| ApiError::bad_request(e.to_string()))
            }
        })
        .await;
        match preview {
            Err(err) => return err.into_response(),
            Ok(rep) => {
                let diagnostics: Vec<_> = rep
                    .scripts
                    .invalid
                    .iter()
                    .chain(&rep.functions.invalid)
                    .chain(&rep.templates.invalid)
                    .flat_map(|entry| entry.diagnostics.iter())
                    .collect();
                if !diagnostics.is_empty() {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": "invalid_yaml",
                            "diagnostics": diagnostics,
                        })),
                    )
                        .into_response();
                }
            }
        }
    }
    match run_blocking_api(move || {
        let rep = st
            .scripts
            .import(&body, &pkg, confirm)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        // confirm=true 落盘成功后对 tmpl 目录做一次目录级主动失效（PERF-002）：
        // 导入可能批量替换模板，逐文件失效不如整目录干净；confirm=false 只解析
        // 未落盘、失败路径未写入，均不动缓存（mtime/size/hash 兜底仍在）
        if confirm {
            matcher::invalidate_template_cache_dir(&st.scripts.tmpl_dir(&pkg));
        }
        Ok(rep)
    })
    .await
    {
        Ok(rep) => Json(rep).into_response(),
        Err(e) => e.into_response(),
    }
}
