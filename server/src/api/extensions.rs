//! Extension package, lifecycle, and UI contribution REST endpoints.
//!
//! The endpoint owns no plugin state: all mutations go through
//! [`ExtensionService`], which serializes lifecycle transitions and refreshes
//! the in-process UI registry after every mutation.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::{ApiError, AppState};
use crate::extensions::{
    ExtensionError, ExtensionId, ExtensionPath, ExtensionService, ExtensionSnapshot,
    ExtensionVersion,
};

pub(super) async fn api_list_extensions(State(st): State<AppState>) -> Response {
    match st.extensions.list() {
        Ok(extensions) => {
            let ui = match st.extensions.ui_contributions() {
                Ok(ui) => ui,
                Err(error) => return extension_error(error),
            };
            Json(serde_json::json!({
                "runtime_available": st.extensions.runtime_available(),
                "extensions": extensions
                    .iter()
                    .map(|snapshot| snapshot_json(&st.extensions, snapshot))
                    .collect::<Vec<_>>(),
                "ui_contributions": ui,
            }))
            .into_response()
        }
        Err(error) => extension_error(error),
    }
}

pub(super) async fn api_list_ui_contributions(State(st): State<AppState>) -> Response {
    match st.extensions.ui_contributions() {
        Ok(contributions) => Json(contributions).into_response(),
        Err(error) => extension_error(error),
    }
}

pub(super) async fn api_install_extension(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.is_empty() {
        return ApiError::bad_request("插件归档不能为空").into_response();
    }
    let context = match super::extensions_management::install_context(&headers) {
        Ok(context) => context,
        Err(response) => return response,
    };
    match st.extensions.install_with_context(&body, &context).await {
        Ok(snapshot) => {
            let snapshot = auto_start_installed(&st.extensions, snapshot).await;
            (
                StatusCode::CREATED,
                Json(snapshot_json(&st.extensions, &snapshot)),
            )
                .into_response()
        }
        Err(error) => extension_error(error),
    }
}

pub(super) async fn api_update_extension(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.is_empty() {
        return ApiError::bad_request("插件归档不能为空").into_response();
    }
    let context = match super::extensions_management::install_context(&headers) {
        Ok(context) => context,
        Err(response) => return response,
    };
    match st.extensions.update_with_context(&body, &context).await {
        Ok(snapshot) => Json(snapshot_json(&st.extensions, &snapshot)).into_response(),
        Err(error) => extension_error(error),
    }
}

/// 用户「启用」（V1 计划 Phase 5）：enable = 启用意图 + 直接启动（幂等，
/// Running 时返回现状）。可选 body 携带 keymap profile / AppContext 数据
/// 通道（与已删除的 /start 同形）。
pub(super) async fn api_enable_extension(
    State(st): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let request = if body.is_empty() {
        StartExtensionRequest::default()
    } else {
        match serde_json::from_slice::<StartExtensionRequest>(&body) {
            Ok(request) => request,
            Err(error) => return ApiError::bad_request(error.to_string()).into_response(),
        }
    };
    let profile = match request.profile.as_deref() {
        None => None,
        Some(name) => {
            // keymap profile 数据通道：数据上下文取请求 AppContext 的
            // package_id（content_package），方案 YAML 从通用资源存储原样读出
            // 交给 guest。门禁按扩展边界谓词判定（id 知识收敛在 keymap 边界）。
            let extension_id = match ExtensionId::parse(&id) {
                Ok(id) => id,
                Err(error) => return extension_error(error),
            };
            if !crate::extensions::is_keymap_extension(&extension_id) {
                return ApiError::bad_request("仅 keymap 插件支持 profile 启动参数")
                    .into_response();
            }
            let package_id = request
                .app_context
                .as_ref()
                .and_then(|context| context.content_package.as_ref())
                .map(|package| package.as_str().to_string());
            let Some(package_id) = package_id else {
                return ApiError::bad_request(
                    "keymap profile 启动必须携带 app_context.content_package（配置 ID）指定数据上下文",
                )
                .into_response();
            };
            match crate::extensions::load_user_profile(&st.packages, &package_id, name) {
                Ok(content) => Some(content),
                Err(error) => return extension_error(error),
            }
        }
    };
    lifecycle(
        &st.extensions,
        &id,
        Lifecycle::Enable(request.app_context, profile),
    )
    .await
}

pub(super) async fn api_disable_extension(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    lifecycle(&st.extensions, &id, Lifecycle::Disable).await
}

/// Declarative 面板最后一公里：`plugin.call` 的服务端入口。插件必须处于
/// Running 且 action 必须出现在其 manifest declarative UI 的按钮集合内；
/// 调用经通用扩展 Component 的 `call` 导出执行，结果原样返回 guest JSON。
pub(super) async fn api_call_extension(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CallExtensionRequest>,
) -> Response {
    let extension_id = match ExtensionId::parse(&id) {
        Ok(id) => id,
        Err(error) => return extension_error(error),
    };
    match st
        .extensions
        .call_extension(&extension_id, &body.action, body.values.unwrap_or_default())
        .await
    {
        Ok(result) => Json(result).into_response(),
        Err(error) => extension_error(error),
    }
}

/// 能力发现（简化计划 Phase 4）：列出目标插件对外公开的动作（declarative
/// 按钮集合 ∪ 原生公开动作清单）与运行状态。其他插件/前端在调用前据此查询，
/// 目标未安装/停用时按 `running:false` + 缺失原因降级功能入口，不再为每一组
/// 插件写专用 ID 分支。动作存在 ≠ 可调用：真正分发仍走 `POST /call`
/// （目标必须 Running、动作必须在公开集合内、权限/上下文门禁照常）。
pub(super) async fn api_extension_capabilities(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let extension_id = match ExtensionId::parse(&id) {
        Ok(id) => id,
        Err(error) => return extension_error(error),
    };
    let snapshot = match st.extensions.snapshot_for(&extension_id) {
        Ok(snapshot) => snapshot,
        Err(error) => return extension_error(error),
    };
    let running = snapshot.state() == crate::extensions::ExtensionState::Running;
    Json(serde_json::json!({
        "id": snapshot.id(),
        "state": snapshot.state(),
        "running": running,
        "actions": st.extensions.capability_actions(snapshot.manifest()),
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CallExtensionRequest {
    action: String,
    #[serde(default)]
    values: Option<serde_json::Value>,
}

pub(super) async fn api_uninstall_extension(
    State(st): State<AppState>,
    Path((id, version)): Path<(String, String)>,
    Query(query): Query<UninstallQuery>,
) -> Response {
    let id = match ExtensionId::parse(&id) {
        Ok(id) => id,
        Err(error) => return extension_error(error),
    };
    let version = match ExtensionVersion::parse(&version) {
        Ok(version) => version,
        Err(error) => return extension_error(error),
    };
    let delete_data = query
        .delete_data
        .as_deref()
        .is_some_and(|value| matches!(value, "1" | "true" | "yes"));
    match st.extensions.uninstall(&id, &version).await {
        Ok(true) => {
            // WASM user data is deliberately outside immutable plugin
            // versions. The management flag deletes only this exact,
            // validated plugin-owned directory; the default keeps it.
            if delete_data {
                let data_root = st
                    .extensions
                    .store()
                    .data_root()
                    .join("extension-data")
                    .join(id.as_str());
                match std::fs::symlink_metadata(&data_root) {
                    Ok(metadata) if metadata.is_dir() => {
                        if let Err(error) = std::fs::remove_dir_all(&data_root) {
                            return ApiError::internal(format!(
                                "插件已卸载，但用户数据删除失败: {error}"
                            ))
                            .into_response();
                        }
                    }
                    Ok(_) => return ApiError::internal("插件用户数据路径不是目录").into_response(),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return ApiError::internal(error.to_string()).into_response(),
                }
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => ApiError::not_found("插件版本不存在").into_response(),
        Err(error) => extension_error(error),
    }
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct UninstallQuery {
    #[serde(default)]
    delete_data: Option<String>,
}

pub(super) async fn api_get_extension_ui_asset(
    State(st): State<AppState>,
    Path((id, path)): Path<(String, String)>,
) -> Response {
    let id = match ExtensionId::parse(&id) {
        Ok(id) => id,
        Err(error) => return extension_error(error),
    };
    let path = match ExtensionPath::parse(&format!("ui/{path}")) {
        Ok(path) => path,
        Err(error) => return extension_error(error),
    };
    match st.extensions.read_ui_file(&id, &path) {
        Ok((bytes, _)) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, content_type(path.as_str()))],
            bytes,
        )
            .into_response(),
        Err(error) => extension_error(error),
    }
}

#[derive(Clone)]
enum Lifecycle {
    /// 启用 = 启用意图 + 直接启动（V1 用户操作收敛；携带可选 start 数据通道）。
    Enable(Option<crate::core::AppContext>, Option<String>),
    Disable,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartExtensionRequest {
    #[serde(default)]
    app_context: Option<crate::core::AppContext>,
    /// keymap 专用：当前 Package 数据上下文（app_context.content_package）
    /// `mappings/` 内的映射方案名；缺省 = guest 内置默认规则。
    #[serde(default)]
    profile: Option<String>,
}

async fn lifecycle(service: &ExtensionService, raw_id: &str, operation: Lifecycle) -> Response {
    let id = match ExtensionId::parse(raw_id) {
        Ok(id) => id,
        Err(error) => return extension_error(error),
    };
    let result = match operation {
        Lifecycle::Enable(app_context, profile) => {
            service.enable_and_start(&id, app_context, profile).await
        }
        Lifecycle::Disable => service.disable(&id).await,
    };
    match result {
        Ok(snapshot) => Json(snapshot_json(service, &snapshot)).into_response(),
        Err(error) => extension_error(error),
    }
}

/// 安装即用（2026-09-05 产品裁决）：安装成功后自动 enable → start，让
/// Runner 与 UI 面板随安装即生效，无需用户再手动「启动」。任一步失败不推翻
/// 安装结果——扩展留在 Enabled/Failed 并带 last_error，可从插件中心手动处置。
/// 已在 Running（同版本覆盖重装）时直接返回原快照。
async fn auto_start_installed(
    service: &ExtensionService,
    snapshot: ExtensionSnapshot,
) -> ExtensionSnapshot {
    if snapshot.state() == crate::extensions::ExtensionState::Running {
        return snapshot;
    }
    let id = snapshot.id().clone();
    match service.enable_and_start(&id, None, None).await {
        Ok(started) => started,
        Err(error) => {
            // 降级为 Enabled（保留错误）而非 Failed：安装结果不被启动失败
            // 推翻，扩展可从插件中心手动 start 重试（如缺依赖的瞬态失败）。
            tracing::warn!(extension = %id, %error, "install auto-start: start failed");
            match service.degrade_start_failure(&id, &error).await {
                Ok(degraded) => degraded,
                Err(degrade_error) => {
                    tracing::warn!(
                        extension = %id,
                        %degrade_error,
                        "install auto-start: controlled failure write skipped"
                    );
                    match service.snapshot_for(&id) {
                        Ok(current) => current,
                        Err(_) => snapshot,
                    }
                }
            }
        }
    }
}

fn snapshot_json(service: &ExtensionService, snapshot: &ExtensionSnapshot) -> serde_json::Value {
    let manifest = snapshot.manifest();
    let host_api = manifest
        .host_api()
        .iter()
        .map(|(domain, requirement)| (domain.to_string(), requirement.to_string()))
        .collect::<std::collections::BTreeMap<_, _>>();
    // 依赖实时状态（简化计划 Phase 3）：必需依赖缺失 → 启动会失败的原因；
    // 可选依赖缺失 → 前端据此降级相关功能入口。
    let dependencies = service.dependency_report(manifest);
    serde_json::json!({
        "id": snapshot.id(),
        "version": manifest.version(),
        "active_version": snapshot.active_version(),
        "installed_versions": snapshot.installed_versions(),
        "name": manifest.name(),
        "description": manifest.description(),
        // builtin 扩展没有 WASM entry（null）；执行类型见 execution。
        "entry": manifest.entry().map(|entry| entry.as_str()),
        "execution": manifest.execution(),
        "targets": android_targets_json(manifest),
        "state": snapshot.state(),
        "last_error": snapshot.last_error(),
        "host_api": host_api,
        "permissions": manifest.permissions().names(),
        "dependencies": dependencies,
        "ui": manifest.ui().iter().map(ui_json).collect::<Vec<_>>(),
    })
}

/// 插件 Android Targets JSON（快照与 pre-install inspect 共用）：空声明归一为
/// `*` 呈现，与「缺省 = 通用」语义一致，前端按当前设备应用过滤插件入口。
pub(super) fn android_targets_json(
    manifest: &crate::extensions::ExtensionManifest,
) -> serde_json::Value {
    let mut packages = manifest.android_targets().to_vec();
    if packages.is_empty() {
        packages.push("*".to_string());
    }
    serde_json::json!({ "android": { "packages": packages } })
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
        "component": contribution.component(),
    })
}

fn content_type(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "application/octet-stream",
    }
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
        ExtensionError::DependencyUnsatisfied { .. }
        | ExtensionError::DependencyOfRunningExtension { .. } => {
            ApiError::conflict(error.to_string())
        }
        ExtensionError::ArchiveSha256Mismatch { .. } => ApiError::bad_request(error.to_string()),
        ExtensionError::RuntimeUnavailable(_) => ApiError::service_unavailable(error.to_string()),
        ExtensionError::CallRejected(_) => ApiError::bad_request(error.to_string()),
        ExtensionError::Io(_)
        | ExtensionError::Json(_)
        | ExtensionError::Zip(_)
        | ExtensionError::Runtime(_) => ApiError::internal(error.to_string()),
        _ => ApiError::bad_request(error.to_string()),
    };
    api_error.into_response()
}
