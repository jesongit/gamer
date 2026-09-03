//! App Package REST 端点：安装 / 列表 / 卸载 / 激活。
//!
//! 端点不持有包状态：全部读写走 [`AppPackageStore`]（staging + 原子安装、
//! active 注册表、primary 唯一约束与预设发布 hook 都在 store 层收口）。
//! 安装归档为 zip/.gamerpkg 字节流，可选 `X-Expected-Sha256` 头做完整性校验。

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use super::{ApiError, AppState};
use crate::app_packages::{AppPackageError, InstalledPackage};

/// 归档完整性校验头（64 位 hex，大小写不敏感）。
const EXPECTED_SHA256_HEADER: &str = "x-expected-sha256";

// axum `Response` 体量超过 clippy result_large_err 阈值；错误即响应，箱化不改变语义
#[allow(clippy::result_large_err)]
fn expected_sha256_of(headers: &HeaderMap) -> Result<Option<String>, Response> {
    let Some(value) = headers.get(EXPECTED_SHA256_HEADER) else {
        return Ok(None);
    };
    let value = match value.to_str() {
        Ok(value) => value.trim().to_string(),
        Err(error) => {
            return Err(
                ApiError::bad_request(format!("{EXPECTED_SHA256_HEADER} 头无效: {error}"))
                    .into_response(),
            )
        }
    };
    if !(value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())) {
        return Err(ApiError::bad_request(format!(
            "{EXPECTED_SHA256_HEADER} 必须是 64 位 hex SHA-256"
        ))
        .into_response());
    }
    Ok(Some(value))
}

pub(super) async fn api_install_app_package(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.is_empty() {
        return ApiError::bad_request("App Package 归档不能为空").into_response();
    }
    let expected = match expected_sha256_of(&headers) {
        Ok(expected) => expected,
        Err(response) => return response,
    };
    match st
        .app_packages
        .install_and_activate(&body, expected.as_deref())
        .await
    {
        Ok(installed) => match package_json(&st, &installed) {
            Ok(json) => (StatusCode::CREATED, Json(json)).into_response(),
            Err(error) => app_package_error(error),
        },
        Err(error) => app_package_error(error),
    }
}

pub(super) async fn api_list_app_packages(State(st): State<AppState>) -> Response {
    let installed = match st.app_packages.list_installed() {
        Ok(installed) => installed,
        Err(error) => return app_package_error(error),
    };
    let mut seen: Vec<String> = Vec::new();
    let mut packages: Vec<Value> = Vec::new();
    for package in &installed {
        let id = package.manifest().id().clone();
        if seen.contains(&id.to_string()) {
            continue;
        }
        seen.push(id.to_string());
        match package_json_for(&st, &installed, &id) {
            Ok(json) => packages.push(json),
            Err(error) => return app_package_error(error),
        }
    }
    Json(serde_json::json!({ "packages": packages })).into_response()
}

pub(super) async fn api_uninstall_app_package(
    State(st): State<AppState>,
    Path((id, version)): Path<(String, String)>,
) -> Response {
    let id = match crate::app_packages::parse_app_package_id(&id) {
        Ok(id) => id,
        Err(error) => return app_package_error(error),
    };
    let version = match crate::app_packages::InstalledVersion::parse(&version) {
        Ok(version) => version,
        Err(error) => return app_package_error(error),
    };
    match st.app_packages.uninstall(&id, &version).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => {
            ApiError::not_found(format!("App Package 未安装: {id}@{version}")).into_response()
        }
        Err(error) => app_package_error(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ActivateReq {
    version: String,
}

pub(super) async fn api_activate_app_package(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ActivateReq>,
) -> Response {
    let id = match crate::app_packages::parse_app_package_id(&id) {
        Ok(id) => id,
        Err(error) => return app_package_error(error),
    };
    let version = match crate::app_packages::InstalledVersion::parse(&req.version) {
        Ok(version) => version,
        Err(error) => return app_package_error(error),
    };
    match st.app_packages.activate(&id, &version).await {
        Ok(installed) => match package_json(&st, &installed) {
            Ok(json) => Json(json).into_response(),
            Err(error) => app_package_error(error),
        },
        Err(error) => app_package_error(error),
    }
}

/// 单个 content package 视图：全部已装版本 + active 版本 + 每版本摘要。
fn package_json(st: &AppState, installed: &InstalledPackage) -> Result<Value, AppPackageError> {
    package_json_for(
        st,
        &st.app_packages.list_installed()?,
        installed.manifest().id(),
    )
}

fn package_json_for(
    st: &AppState,
    installed: &[InstalledPackage],
    id: &crate::app_packages::AppPackageId,
) -> Result<Value, AppPackageError> {
    let active = st.app_packages.active_version(id)?;
    let mut versions = Vec::new();
    let mut name = None;
    let mut android_packages: Vec<String> = Vec::new();
    for package in installed {
        let manifest = package.manifest();
        if manifest.id() != id {
            continue;
        }
        if name.is_none() {
            name = manifest.name().map(str::to_string);
        }
        for target in manifest.android_packages() {
            if !android_packages
                .iter()
                .any(|existing| existing == target.as_str())
            {
                android_packages.push(target.as_str().to_string());
            }
        }
        let meta = st.app_packages.install_meta(id, manifest.version())?;
        versions.push(serde_json::json!({
            "version": manifest.version().as_str(),
            "sha256": meta.as_ref().map(|meta| meta.sha256.clone()),
            "installed_at": meta.as_ref().map(|meta| meta.installed_at.clone()),
            "android_packages": manifest
                .android_packages()
                .iter()
                .map(|target| target.as_str())
                .collect::<Vec<_>>(),
        }));
    }
    Ok(serde_json::json!({
        "id": id.as_str(),
        "name": name,
        "active_version": active.map(|version| version.into_string()),
        "android_packages": android_packages,
        "versions": versions,
    }))
}

fn app_package_error(error: AppPackageError) -> Response {
    let api_error = match &error {
        AppPackageError::AlreadyInstalled { .. } | AppPackageError::PrimaryConflict { .. } => {
            ApiError::conflict(error.to_string())
        }
        AppPackageError::NotInstalled { .. } | AppPackageError::NotActive(_) => {
            ApiError::not_found(error.to_string())
        }
        AppPackageError::Sha256Mismatch { .. }
        | AppPackageError::InvalidAppPackageId(_)
        | AppPackageError::InvalidAndroidPackage(_)
        | AppPackageError::InvalidInstalledVersion(_)
        | AppPackageError::InvalidResourcePath(_)
        | AppPackageError::InvalidManifest(_)
        | AppPackageError::InvalidArchive(_)
        | AppPackageError::ArchiveTooLarge { .. }
        | AppPackageError::InvalidPreset(_) => ApiError::bad_request(error.to_string()),
        AppPackageError::TaskHook(_) | AppPackageError::PresetHook(_) => {
            ApiError::internal(error.to_string())
        }
        AppPackageError::Io(_) | AppPackageError::Zip(_) => ApiError::internal(error.to_string()),
    };
    api_error.into_response()
}
