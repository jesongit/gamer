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
pub(super) struct SaveScriptReq {
    id: Option<String>,
    name: String,
    content: String,
    /// 目标应用分区（设备配置的应用包名）
    pkg: String,
}

pub(super) async fn api_save_script(
    State(st): State<AppState>,
    Json(req): Json<SaveScriptReq>,
) -> Response {
    if let Err(err) = validate_text_field(&req.name, "脚本名", 255) {
        return err.into_response();
    }
    if req.content.trim().is_empty() {
        return ApiError::bad_request("脚本内容不能为空").into_response();
    }
    if req.content.len() > crate::scripts::IMPORT_MAX_YAML_BYTES {
        return ApiError::bad_request("脚本内容超过 1 MiB").into_response();
    }
    let pkg = match require_pkg(Some(&req.pkg)) {
        Ok(pkg) => pkg,
        Err(err) => return err.into_response(),
    };
    match run_blocking_api(move || {
        st.scripts
            .save(req.id.as_deref(), &pkg, &req.name, &req.content)
            .map_err(|e| ApiError::bad_request(e.to_string()))
    })
    .await
    {
        Ok(s) => {
            Json(serde_json::json!({"ok": true, "id": s.id, "package": s.package, "name": s.name}))
                .into_response()
        }
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
