//! 函数库（data/<pkg>/func/）CRUD 路由：风格对齐 scripts 路由（契约 plan §13.1）。
//!
//! - id = `<pkg>/<文件短路径>.yaml`（含 `/`，前端拼 URL 必须整体 encodeURIComponent，
//!   axum 对 %2F 解码，与 scripts 路由同规则）；
//! - 浅校验（合法 YAML + 顶层键为合法函数名）在存储层强制，完整结构校验归阶段 2；
//! - GET 返回内容版本短码 version；POST/PUT 带可选 expected_version，不匹配返回
//!   409 {code:"version_conflict", message, resource}（CONTRACT §5 错误结构，resource
//!   级错误 step_path/field 留空）；不提供则直接接受（现阶段旧前端兼容）；
//! - func/ 函数库不进脚本列表/运行接口/任务选择器（数据源物理隔离，测试锁死）。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::common::{require_pkg, run_blocking_api, validate_text_field};
use super::templates::PkgQuery;
use super::{ApiError, AppState};
use crate::scripts::IMPORT_MAX_YAML_BYTES;

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

/// expected_version 冲突判定结果（在 blocking 闭包里完成磁盘读取，处理在闭包外）
pub(super) enum VersionCheck {
    Ok,
    /// 磁盘版本与 expected_version 不符（含目标文件不存在）——携带冲突资源 ID
    Conflict {
        resource: String,
    },
}

pub(super) fn check_expected_version(
    current: Option<String>,
    expected: Option<&str>,
    resource: String,
) -> VersionCheck {
    match expected {
        None => VersionCheck::Ok,
        Some(exp) => match current {
            Some(v) if v == exp => VersionCheck::Ok,
            _ => VersionCheck::Conflict { resource },
        },
    }
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
pub(super) struct SaveFunctionReq {
    /// 目标应用分区（设备配置的应用包名）
    pkg: String,
    /// 函数文件短路径（缺 .yaml 扩展名自动补全；upsert 语义对齐 POST /api/scripts）
    name: String,
    content: String,
    /// 可选：客户端持有的当前内容版本短码；与磁盘不符 → 409 version_conflict
    #[serde(default)]
    expected_version: Option<String>,
}

/// POST /api/functions：创建/覆盖函数库文件（upsert）。先做 expected_version
/// 冲突检测，再浅校验 + 原子落盘。
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
    let name = req.name.clone();
    let expected = req.expected_version.clone();
    let st_check = st.clone();
    let pkg_check = pkg.clone();
    let check = run_blocking_api(move || {
        let current = st_check
            .scripts
            .function_version_for_save(&pkg_check, &name)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        Ok(check_expected_version(
            current,
            expected.as_deref(),
            format!("{pkg_check}/{name}"),
        ))
    })
    .await;
    match check {
        Err(e) => return e.into_response(),
        Ok(VersionCheck::Conflict { resource }) => return version_conflict(&resource),
        Ok(VersionCheck::Ok) => {}
    }
    match run_blocking_api(move || {
        st.scripts
            .save_function(&pkg, &req.name, &req.content)
            .map_err(|e| ApiError::bad_request(e.to_string()))
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
pub(super) struct UpdateFunctionReq {
    content: String,
    /// 可选：客户端持有的当前内容版本短码；与磁盘不符 → 409 version_conflict
    #[serde(default)]
    expected_version: Option<String>,
}

/// PUT /api/functions/:id：覆盖更新（不重命名）。404（文件不存在）优先于 409。
pub(super) async fn api_update_function(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateFunctionReq>,
) -> Response {
    if let Err(err) = validate_function_content(&req.content) {
        return err.into_response();
    }
    let check_id = id.clone();
    let expected = req.expected_version.clone();
    let st_check = st.clone();
    let check = run_blocking_api(move || {
        let Some(f) = st_check
            .scripts
            .get_function(&check_id)
            .map_err(|e| ApiError::internal(e.to_string()))?
        else {
            return Err(ApiError::not_found("函数文件不存在"));
        };
        Ok(check_expected_version(
            Some(f.version),
            expected.as_deref(),
            check_id,
        ))
    })
    .await;
    match check {
        Err(e) => return e.into_response(),
        Ok(VersionCheck::Conflict { resource }) => return version_conflict(&resource),
        Ok(VersionCheck::Ok) => {}
    }
    match run_blocking_api(move || {
        // id 合法性（pkg/短路径分段与扩展名）由存储层 resolver 把关
        let (pkg, rel) = id.split_once('/').ok_or_else(|| {
            ApiError::bad_request("非法函数文件 id（应为 <pkg>/<文件短路径>.yaml）")
        })?;
        st.scripts
            .save_function(pkg, rel, &req.content)
            .map_err(|e| ApiError::bad_request(e.to_string()))
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
