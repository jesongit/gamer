//! Package 插件资源重命名端点。
//!
//! `POST /api/packages/:pkg/plugins/:plugin/rename`，body
//! `{path, new_path}`（均相对 `plugins/<plugin>/`）。实现 =
//! [`PackageStore::rename_resource`]：先经该插件注册的
//! [`ResourceHandler::before_rename`] 钩子（gamer.yaml 的模板引用 v3 AST
//! 同步改写），钩子成功后才原子移动文件，失败不动任何文件。
//!
//! 独立成模块的原因：资源 CRUD 面已在 api/packages.rs 一次定型，重命名是
//! 后接的钩子组合端点（模板重命名语义），不占 CRUD 路由形状。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::common::run_blocking_api;
use super::{ApiError, AppState};
use crate::resources::{is_valid_scope_id, PackageStore};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RenameResourceReq {
    /// 当前资源路径（相对 `plugins/<plugin>/`，如 `templates/reward.png`）。
    path: String,
    /// 新路径（同插件内移动/重命名）。
    new_path: String,
}

/// POST /api/packages/:pkg/plugins/:plugin/rename
pub(super) async fn api_rename_plugin_resource(
    State(st): State<AppState>,
    Path((pkg, plugin)): Path<(String, String)>,
    Json(req): Json<RenameResourceReq>,
) -> Response {
    if !is_valid_scope_id(&plugin) {
        return ApiError::bad_request(format!("plugin id 非法: {plugin:?}")).into_response();
    }
    let path = req.path.trim().to_string();
    let new_path = req.new_path.trim().to_string();
    if path.is_empty() || new_path.is_empty() {
        return ApiError::bad_request("path / new_path 不能为空").into_response();
    }
    let renamed = run_blocking_api(move || -> Result<serde_json::Value, ApiError> {
        let store = store_of(&st);
        // 包必须存在（缺包 404 优先于资源 404）
        store.manifest(&pkg).map_err(not_found_or_internal)?;
        store
            .rename_resource(&pkg, &plugin, &path, &new_path)
            .map_err(rename_error)?;
        Ok(json!({
            "ok": true,
            "package": pkg,
            "plugin": plugin,
            "path": new_path,
            "old_path": path,
        }))
    })
    .await;
    match renamed {
        Ok(value) => Json(value).into_response(),
        Err(e) => e.into_response(),
    }
}

fn store_of(st: &AppState) -> Arc<PackageStore> {
    st.packages.clone()
}

/// rename_resource 错误 → HTTP 语义（与包资源写路径同口径：不存在 404、
/// 已存在 409、输入非法 400）。
#[allow(clippy::result_large_err)]
fn rename_error(e: anyhow::Error) -> ApiError {
    let message = e.to_string();
    if message.contains("不存在") {
        ApiError::not_found(message)
    } else if message.contains("已存在") {
        ApiError::conflict(message)
    } else if message.contains("名称未变化")
        || message.contains("非法")
        || message.contains("不能")
        || message.contains("必须")
        || message.contains("拒绝")
    {
        ApiError::bad_request(message)
    } else {
        tracing::warn!(%message, "package resource rename failed");
        ApiError::internal(message)
    }
}

#[allow(clippy::result_large_err)]
fn not_found_or_internal(e: anyhow::Error) -> ApiError {
    if e.downcast_ref::<crate::resources::PackageNotFound>().is_some() {
        return ApiError::not_found(e.to_string());
    }
    if e.to_string().contains("不存在") {
        return ApiError::not_found(e.to_string());
    }
    tracing::warn!(error = %e, "package rename internal error");
    ApiError::internal(e.to_string())
}
