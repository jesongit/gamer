//! Package REST 端点（plan §2-§15、§23）。
//!
//! 包生命周期与插件资源统一路由（路由形状一次定型；后续任务只填实现）：
//!
//! ```text
//! GET    /api/packages                                   列表 + manifest 摘要
//! POST   /api/packages                                   新建 {id,name,targets,...}
//! GET    /api/packages/:pkg                              manifest + 统计
//! PUT    /api/packages/:pkg                              元数据编辑（expected_revision 条件）
//! DELETE /api/packages/:pkg                              删除
//! POST   /api/packages/:pkg/duplicate                    {new_id} 复制为新包
//!
//! GET    /api/packages/:pkg/plugins/:plugin/resources    递归列表（?prefix= 可选）
//! GET    /api/packages/:pkg/plugins/:plugin/resources/*path   文本 → JSON / 字节 → 原始流
//! PUT    /api/packages/:pkg/plugins/:plugin/resources/*path   JSON {content,...} 或原始字节
//! DELETE /api/packages/:pkg/plugins/:plugin/resources/*path
//!
//! POST   /api/packages/import        zip/.gamerpkg 字节 body（X-Expected-Sha256；
//!                                    已存在 409 附 manifest 摘要；?overwrite=true 校验
//!                                    通过后原子替换）
//! POST   /api/packages/:pkg/export   .gamerpkg 字节流（固定 mtime 可复现打包）
//! ```
//!
//! 插件数据隔离：资源路径恒被限制在
//! `packages/<pkg>/plugins/<plugin>/` 前缀内；shared/ 只随包导出，无插件
//! 写入口。文本与字节 PUT 都经过扩展内容钩子（`ResourceHandler`：文本 v3
//! 校验 / 字节模板灰度归一化），未注册 handler = 裸 Core 透传；归档导入
//! （import）不经过内容校验。导入/复制后自动发布包内
//! `plugins/*/presets/*.yaml` 为任务预设（发布 id `<package-id>:<名>`，幂等）。


use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::common::run_blocking_api;
use super::{ApiError, AppState};
use crate::package_archive::{
    self, extract_archive, export_package, validate_and_read_manifest, ArchiveError,
};
use crate::resources::{is_valid_scope_id, PackageInput, PackageStore};

/// 归档完整性校验头（64 位 hex，大小写不敏感）。
const EXPECTED_SHA256_HEADER: &str = "x-expected-sha256";
/// 导出响应头：归档字节的 SHA-256（64 位 hex）。
const CONTENT_SHA256_HEADER: &str = "x-content-sha256";
/// 字节资源条件更新头（文本资源走 JSON body 的 expected_version 字段）。
const EXPECTED_VERSION_HEADER: &str = "x-expected-version";
const FORCE_HEADER: &str = "x-force";

// ---------- 请求/响应 DTO ----------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AndroidTargetsDto {
    #[serde(default)]
    packages: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetsDto {
    #[serde(default)]
    android: Option<AndroidTargetsDto>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreatePackageReq {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    targets: TargetsDto,
    /// `<plugin-id> → required` 依赖声明（允许声明未安装插件）。
    #[serde(default)]
    plugins: std::collections::BTreeMap<String, bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdatePackageReq {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    targets: Option<TargetsDto>,
    #[serde(default)]
    plugins: Option<std::collections::BTreeMap<String, bool>>,
    #[serde(default)]
    expected_revision: Option<u64>,
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DuplicateReq {
    new_id: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct ListResourcesQuery {
    #[serde(default)]
    prefix: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PutTextResourceReq {
    content: String,
    #[serde(default)]
    expected_version: Option<String>,
    #[serde(default)]
    force: bool,
}

// ---------- JSON 视图 ----------

fn manifest_json(manifest: &crate::resources::PackageManifest) -> Value {
    json!({
        "id": manifest.id,
        "name": manifest.name,
        "version": manifest.version,
        "author": manifest.author,
        "revision": manifest.revision,
        "targets": {
            "android": { "packages": manifest.android_targets },
        },
        "plugins": manifest
            .plugins
            .iter()
            .map(|(plugin, dep)| {
                json!({ "id": plugin, "required": dep.required })
            })
            .collect::<Vec<_>>(),
    })
}

fn store_of(st: &AppState) -> Arc<PackageStore> {
    st.packages.clone()
}

// ---------- 包生命周期 ----------

/// GET /api/packages
pub(super) async fn api_list_packages(State(st): State<AppState>) -> Response {
    match run_blocking_api(move || -> Result<Value, ApiError> {
        let store = store_of(&st);
        let manifests = store.list_packages().map_err(internal)?;
        let packages: Vec<Value> = manifests.iter().map(manifest_json).collect();
        Ok(json!({ "packages": packages }))
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(e) => e.into_response(),
    }
}

/// POST /api/packages
pub(super) async fn api_create_package(
    State(st): State<AppState>,
    Json(req): Json<CreatePackageReq>,
) -> Response {
    match run_blocking_api(move || -> Result<Value, ApiError> {
        let store = store_of(&st);
        let input = PackageInput {
            id: req.id.trim().to_string(),
            name: req.name,
            version: req.version,
            author: req.author,
            android_targets: req
                .targets
                .android
                .map(|a| a.packages)
                .unwrap_or_default(),
            plugins: req.plugins,
        };
        let manifest = store.create_package(input).map_err(store_error)?;
        Ok(manifest_json(&manifest))
    })
    .await
    {
        Ok(value) => (StatusCode::CREATED, Json(value)).into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/packages/:pkg
pub(super) async fn api_get_package(
    State(st): State<AppState>,
    Path(pkg): Path<String>,
) -> Response {
    let extensions = st.extensions.clone();
    match run_blocking_api(move || -> Result<Value, ApiError> {
        let store = store_of(&st);
        let manifest = store.manifest(&pkg).map_err(not_found_or_internal)?;
        let stats = store.stats(&pkg).map_err(not_found_or_internal)?;
        let installed = extensions
            .list()
            .map_err(|e| internal(anyhow::anyhow!(e.to_string())))?;
        Ok(json!({
            "package": manifest_json(&manifest),
            "stats": serde_json::to_value(&stats).unwrap_or_default(),
            "plugin_states": plugin_states_json(&manifest, &stats, &installed),
        }))
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/packages/:pkg/compatibility?android_package=<pkg> — Android Target
/// 兼容性检查（plan §17，warning 语义：不兼容仅提示，不禁止使用）。
/// 0 个声明 target = 通用包恒兼容。
pub(super) async fn api_package_compatibility(
    State(st): State<AppState>,
    Path(pkg): Path<String>,
    Query(q): Query<CompatibilityQuery>,
) -> Response {
    match run_blocking_api(move || -> Result<Value, ApiError> {
        let manifest = store_of(&st).manifest(&pkg).map_err(not_found_or_internal)?;
        let compatible = manifest.android_targets.is_empty()
            || manifest.android_targets.contains(&q.android_package);
        Ok(json!({
            "android_package": q.android_package,
            "compatible": compatible,
            "android_targets": manifest.android_targets,
        }))
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct CompatibilityQuery {
    android_package: String,
}

/// Plugin Dependency 状态（plan §18）：对照扩展注册表，给出每个声明依赖的
/// 可用性；包内存在但 manifest 未声明的插件目录 = `unknown`（dormant 数据
/// 保留语义，只提示不处置）。
///
/// 状态词表：`available`（已安装且 Enabled/Running）| `disabled`（已安装但
/// 未启用/启动失败）| `missing_required` | `missing_optional`（未安装）|
/// `unknown`（盘上存在未声明目录）。
fn plugin_states_json(
    manifest: &crate::resources::PackageManifest,
    stats: &crate::resources::PackageStats,
    installed: &[crate::extensions::ExtensionSnapshot],
) -> Value {
    let state_of = |plugin: &str| {
        installed
            .iter()
            .find(|e| e.id().as_str() == plugin)
            .map(|e| e.state())
    };
    let mut entries: Vec<Value> = manifest
        .plugins
        .iter()
        .map(|(plugin, dep)| {
            let state = match state_of(plugin) {
                Some(crate::extensions::ExtensionState::Enabled)
                | Some(crate::extensions::ExtensionState::Running) => "available",
                Some(_) => "disabled",
                None => {
                    if dep.required {
                        "missing_required"
                    } else {
                        "missing_optional"
                    }
                }
            };
            json!({ "plugin": plugin, "required": dep.required, "state": state })
        })
        .collect();
    // 盘上存在但未声明 = unknown（数据保留，不解释）
    for plugin in &stats.plugins {
        if !manifest.plugins.contains_key(&plugin.plugin) {
            entries.push(json!({
                "plugin": plugin.plugin,
                "required": Value::Null,
                "state": "unknown",
            }));
        }
    }
    Value::Array(entries)
}

/// PUT /api/packages/:pkg — 元数据编辑；缺省字段沿用当前值；id 不可变。
pub(super) async fn api_update_package(
    State(st): State<AppState>,
    Path(pkg): Path<String>,
    Json(req): Json<UpdatePackageReq>,
) -> Response {
    match run_blocking_api(move || -> Result<Value, ApiError> {
        let store = store_of(&st);
        let current = store.manifest(&pkg).map_err(not_found_or_internal)?;
        let input = PackageInput {
            id: pkg.clone(),
            name: req.name.or(current.name.clone()),
            version: req.version.or_else(|| Some(current.version.clone())),
            author: req.author.or(current.author.clone()),
            android_targets: req
                .targets
                .and_then(|t| t.android.map(|a| a.packages))
                .unwrap_or_else(|| current.android_targets.clone()),
            plugins: req.plugins.unwrap_or_else(|| {
                current
                    .plugins
                    .iter()
                    .map(|(plugin, dep)| (plugin.clone(), dep.required))
                    .collect()
            }),
        };
        let manifest = store
            .update_manifest(&pkg, input, req.expected_revision, req.force)
            .map_err(store_error)?;
        Ok(manifest_json(&manifest))
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(e) => e.into_response(),
    }
}

/// DELETE /api/packages/:pkg
pub(super) async fn api_delete_package(
    State(st): State<AppState>,
    Path(pkg): Path<String>,
) -> Response {
    let packages = st.packages.clone();
    let deleted = run_blocking_api({
        let pkg = pkg.clone();
        move || -> Result<bool, ApiError> { packages.delete_package(&pkg).map_err(internal) }
    })
    .await;
    match deleted {
        Ok(true) => {}
        Ok(false) => return ApiError::not_found(format!("Package 不存在: {pkg}")).into_response(),
        Err(e) => return e.into_response(),
    }
    // 旧 App Package 卸载语义的延续：删除包后，绑定该包的任务挂起（幂等、
    // 不删任务行）；预设记录保留。失败不阻塞删除结果（best effort + 日志）。
    if let Err(error) = crate::timer_core::TimerCore::new(st.db.clone())
        .on_app_package_uninstalled(&pkg)
        .await
    {
        tracing::warn!(package = %pkg, %error, "删除包后挂起绑定任务失败");
    }
    StatusCode::NO_CONTENT.into_response()
}

/// POST /api/packages/:pkg/duplicate — 深拷贝为新包（发布包内预设，幂等）。
pub(super) async fn api_duplicate_package(
    State(st): State<AppState>,
    Path(pkg): Path<String>,
    Json(req): Json<DuplicateReq>,
) -> Response {
    let new_id = req.new_id.trim().to_string();
    match run_blocking_api(move || -> Result<Value, ApiError> {
        let store = store_of(&st);
        let manifest = store
            .duplicate_package(&pkg, &new_id)
            .map_err(store_error)?;
        publish_package_presets(&store, st.db.clone(), &manifest.id)
            .map_err(internal)?;
        Ok(manifest_json(&manifest))
    })
    .await
    {
        Ok(value) => (StatusCode::CREATED, Json(value)).into_response(),
        Err(e) => e.into_response(),
    }
}

// ---------- 插件资源 ----------

/// GET /api/packages/:pkg/plugins/:plugin/resources — 递归列表。
pub(super) async fn api_list_plugin_resources(
    State(st): State<AppState>,
    Path((pkg, plugin)): Path<(String, String)>,
    Query(q): Query<ListResourcesQuery>,
) -> Response {
    let prefix = q.prefix.unwrap_or_default();
    match run_blocking_api(move || -> Result<Value, ApiError> {
        let store = store_of(&st);
        if !is_valid_scope_id(&plugin) {
            return Err(ApiError::bad_request(format!("plugin id 非法: {plugin:?}")));
        }
        // 包必须存在（dormant 插件目录可以为空/缺失 → 空列表）
        store.manifest(&pkg).map_err(not_found_or_internal)?;
        let entries = store
            .list(&pkg, &plugin, &prefix)
            .map_err(not_found_or_internal)?;
        let resources = entries
            .iter()
            .map(|entry| serde_json::to_value(entry).unwrap_or_default())
            .collect::<Vec<_>>();
        Ok(json!({ "resources": resources }))
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/packages/:pkg/plugins/:plugin/resources/*path — 文本 → JSON
/// （带 version/注记）；字节 → 原始流（no-cache）。
#[allow(clippy::result_large_err)]
pub(super) async fn api_get_plugin_resource(
    State(st): State<AppState>,
    Path((pkg, plugin, path)): Path<(String, String, String)>,
) -> Response {
    if !is_valid_scope_id(&plugin) {
        return ApiError::bad_request(format!("plugin id 非法: {plugin:?}")).into_response();
    }
    let path = path.trim().to_string();
    // 先按文本读取（UTF-8 且 ≤ 文本上限）；失败按字节返回
    let read_text = {
        let st = st.clone();
        let pkg = pkg.clone();
        let plugin = plugin.clone();
        let path = path.clone();
        run_blocking_api(move || -> Result<Option<Value>, ApiError> {
            let store = store_of(&st);
            let mut entry = match store.read_text(&pkg, &plugin, &path) {
                Ok(Some(entry)) => entry,
                Ok(None) => return Ok(None),
                Err(error) => return Err(not_found_or_internal(error)),
            };
            // 读取侧注记：与列表同一份 handler 注记
            store.annotate_text(&plugin, std::slice::from_mut(&mut entry));
            Ok(Some(serde_json::to_value(&entry).unwrap_or_default()))
        })
        .await
    };
    match read_text {
        Ok(Some(value)) => return Json(value).into_response(),
        Ok(None) => {}
        Err(e) => return e.into_response(),
    }
    let mime_path = path.clone();
    let read_binary = run_blocking_api(move || -> Result<Option<Vec<u8>>, ApiError> {
        let store = store_of(&st);
        store
            .read_binary(&pkg, &plugin, &path)
            .map_err(not_found_or_internal)
    })
    .await;
    match read_binary {
        Ok(Some(bytes)) => {
            let mime = match mime_path.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
                "jpg" | "jpeg" => "image/jpeg",
                "txt" | "json" | "toml" => "text/plain; charset=utf-8",
                _ => "application/octet-stream",
            };
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, mime.to_string()),
                    (header::CACHE_CONTROL, "no-cache".to_string()),
                ],
                bytes,
            )
                .into_response()
        }
        Ok(None) => ApiError::not_found("资源不存在").into_response(),
        Err(e) => e.into_response(),
    }
}

/// PUT /api/packages/:pkg/plugins/:plugin/resources/*path — 文本（JSON body
/// `{content, expected_version?, force?}`，保存前经扩展内容钩子校验）或字节
/// （原始 body；条件更新走 `X-Expected-Version` / `X-Force` 头）。
pub(super) async fn api_put_plugin_resource(
    State(st): State<AppState>,
    Path((pkg, plugin, path)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_valid_scope_id(&plugin) {
        return ApiError::bad_request(format!("plugin id 非法: {plugin:?}")).into_response();
    }
    let path = path.trim().to_string();
    let validate_ctx = (pkg.clone(), plugin.clone());
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().contains("application/json"))
        .unwrap_or(false);
    if is_json {
        let Ok(req) = serde_json::from_slice::<PutTextResourceReq>(&body) else {
            return ApiError::bad_request("请求体必须是 JSON 对象 {content, expected_version?, force?}")
                .into_response();
        };
        if req.content.len() > super::common::TEXT_RESOURCE_MAX_BYTES {
            return ApiError::bad_request("资源内容超过 1 MiB").into_response();
        }
        // 保存前内容校验（未注册 handler = 裸 Core 通过）；诊断 JSON 需要绕过
        // ApiError 边界，直接用 spawn_blocking
        let validation = {
            let st = st.clone();
            let (pkg, plugin) = validate_ctx;
            let path = path.clone();
            let content = req.content.clone();
            tokio::task::spawn_blocking(move || {
                let store = store_of(&st);
                store.validate_save(crate::resources::SaveValidation {
                    package: &pkg,
                    plugin: &plugin,
                    path: &path,
                    content: &content,
                    store: &store,
                })
            })
            .await
            .unwrap_or_else(|e| Err(Value::String(format!("validation worker failed: {e}"))))
        };
        if let Err(diagnostics) = validation {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_content", "diagnostics": diagnostics })),
            )
                .into_response();
        }
        let result = run_blocking_api(move || -> Result<Value, ApiError> {
            let store = store_of(&st);
            let mut entry = store
                .write_text(
                    &pkg,
                    &plugin,
                    &path,
                    &req.content,
                    req.expected_version.as_deref(),
                    req.force,
                )
                .map_err(write_error)?;
            // 响应注记与列表同一份 handler 注记
            store.annotate_text(&plugin, std::slice::from_mut(&mut entry));
            let mut value = serde_json::to_value(&entry).unwrap_or_default();
            if let Some(obj) = value.as_object_mut() {
                obj.insert("ok".into(), Value::Bool(true));
            }
            Ok(value)
        })
        .await;
        match result {
            Ok(value) => Json(value).into_response(),
            Err(e) => e.into_response(),
        }
    } else {
        // 字节资源
        if body.is_empty() {
            return ApiError::bad_request("字节内容为空").into_response();
        }
        let expected = headers
            .get(EXPECTED_VERSION_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let force = headers
            .get(FORCE_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        // 保存前字节钩子（校验 + 可选归一化）：诊断 JSON 与文本路径同一 400
        // 形状。归档导入（POST /api/packages/import）不经过内容校验——包整体
        // 替换语义，内容以导出侧校验为准。
        let validation = {
            let st = st.clone();
            let (pkg, plugin) = validate_ctx;
            let path = path.clone();
            let bytes = body.clone();
            tokio::task::spawn_blocking(move || {
                let store = store_of(&st);
                store.validate_save_binary(crate::resources::SaveBinaryValidation {
                    package: &pkg,
                    plugin: &plugin,
                    path: &path,
                    bytes: &bytes,
                    store: &store,
                })
            })
            .await
            .unwrap_or_else(|e| Err(Value::String(format!("validation worker failed: {e}"))))
        };
        let bytes = match validation {
            Ok(bytes) => bytes,
            Err(diagnostics) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "invalid_content", "diagnostics": diagnostics })),
                )
                    .into_response()
            }
        };
        let result = run_blocking_api(move || -> Result<Value, ApiError> {
            let store = store_of(&st);
            let entry = store
                .write_binary(&pkg, &plugin, &path, &bytes, expected.as_deref(), force)
                .map_err(write_error)?;
            let mut value = serde_json::to_value(&entry).unwrap_or_default();
            if let Some(obj) = value.as_object_mut() {
                obj.insert("ok".into(), Value::Bool(true));
            }
            Ok(value)
        })
        .await;
        match result {
            Ok(value) => Json(value).into_response(),
            Err(e) => e.into_response(),
        }
    }
}

/// DELETE /api/packages/:pkg/plugins/:plugin/resources/*path
pub(super) async fn api_delete_plugin_resource(
    State(st): State<AppState>,
    Path((pkg, plugin, path)): Path<(String, String, String)>,
) -> Response {
    if !is_valid_scope_id(&plugin) {
        return ApiError::bad_request(format!("plugin id 非法: {plugin:?}")).into_response();
    }
    let path = path.trim().to_string();
    match run_blocking_api(move || -> Result<(), ApiError> {
        let store = store_of(&st);
        store
            .delete_resource(&pkg, &plugin, &path)
            .map_err(write_error)?;
        Ok(())
    })
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

// ---------- 导入 / 导出 ----------

/// POST /api/packages/import — zip/.gamerpkg 字节 body。
pub(super) async fn api_import_package(
    State(st): State<AppState>,
    headers: HeaderMap,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Bytes,
) -> Response {
    if body.is_empty() {
        return ApiError::bad_request("Package 归档不能为空").into_response();
    }
    let overwrite = raw
        .as_deref()
        .map(|q| q.split('&').any(|pair| pair == "overwrite=true"))
        .unwrap_or(false);
    let expected = match expected_sha256_of(&headers) {
        Ok(expected) => expected,
        Err(response) => return response,
    };
    if let Some(expected) = &expected {
        let actual = package_archive::sha256_hex(&body);
        if !expected.eq_ignore_ascii_case(&actual) {
            return ApiError::bad_request(format!(
                "归档摘要校验失败（期望 {expected}，实际 {actual}）"
            ))
            .into_response();
        }
    }
    // 解压 → 验证 manifest → 验证 id → 原子安装/替换（照 plan §8）。
    // Ok = 完成；Err(StructuredConflict) = 已存在且未带 overwrite 的结构化 409。
    let staged = run_blocking_api(move || -> Result<ImportOutcome, ApiError> {
        let store = store_of(&st);
        // 1) 解压前先行归档安全校验（limits/中央目录/manifest 可解析）
        validate_and_read_manifest(&body).map_err(archive_error)?;
        // 2) staging 目录解压（目录安全逐段校验）
        let staging = store
            .staging_root()
            .join(uuid::Uuid::new_v4().simple().to_string());
        let _ = std::fs::remove_dir_all(&staging);
        let manifest = extract_archive(&body, &staging)
            .inspect_err(|_| {
                let _ = std::fs::remove_dir_all(&staging);
            })
            .map_err(archive_error)?;
        // 3) 目标存在性 → 409 / 原子替换
        let final_dir = match store.package_dir(&manifest.id) {
            Ok(dir) => dir,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(ApiError::bad_request(error.to_string()));
            }
        };
        if final_dir.exists() {
            if !overwrite {
                let _ = std::fs::remove_dir_all(&staging);
                // 结构化 409：附已存包 manifest 摘要（覆盖确认提示由前端组装）
                let existing = store
                    .try_manifest(&manifest.id)
                    .ok()
                    .flatten()
                    .map(|m| manifest_json(&m))
                    .unwrap_or_else(|| json!({ "id": manifest.id }));
                return Ok(ImportOutcome::Conflict(json!({
                    "error": "package_exists",
                    "message": format!(
                        "Package 已存在: {}。继续导入将覆盖该 Package 当前数据（含直接修改或新增的内容），确认后带 ?overwrite=true 重试",
                        manifest.id
                    ),
                    "existing": existing,
                })));
            }
            // 原子替换：旧目录先移出，再换入 staging，最后删旧目录
            let trash = store
                .staging_root()
                .join(uuid::Uuid::new_v4().simple().to_string());
            if let Err(error) = std::fs::rename(&final_dir, &trash) {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(ApiError::internal(format!("Package 替换失败: {error}")));
            }
            if let Err(error) = std::fs::rename(&staging, &final_dir) {
                let _ = std::fs::rename(&trash, &final_dir);
                let _ = std::fs::remove_dir_all(&staging);
                return Err(ApiError::internal(format!("Package 替换失败: {error}")));
            }
            let _ = std::fs::remove_dir_all(&trash);
        } else if let Err(error) = std::fs::rename(&staging, &final_dir) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(ApiError::internal(format!("Package 安装失败: {error}")));
        }
        // 4) 发布包内预设（plugins/*/presets/*.yaml，幂等）
        publish_package_presets(&store, st.db.clone(), &manifest.id).map_err(internal)?;
        let stored = store.manifest(&manifest.id).map_err(not_found_or_internal)?;
        Ok(ImportOutcome::Done {
            overwritten: overwrite,
            manifest: manifest_json(&stored),
        })
    })
    .await;
    match staged {
        // 覆盖替换 -> 200；全新安装 -> 201
        Ok(ImportOutcome::Done {
            overwritten,
            manifest,
        }) => {
            if overwritten {
                Json(manifest).into_response()
            } else {
                (StatusCode::CREATED, Json(manifest)).into_response()
            }
        }
        Ok(ImportOutcome::Conflict(body)) => (StatusCode::CONFLICT, Json(body)).into_response(),
        Err(e) => e.into_response(),
    }
}

/// POST /api/packages/:pkg/export — .gamerpkg 字节流。
pub(super) async fn api_export_package(
    State(st): State<AppState>,
    Path(pkg): Path<String>,
) -> Response {
    let built = run_blocking_api(move || -> Result<package_archive::BuiltPackage, ApiError> {
        export_package(&store_of(&st), &pkg).map_err(archive_error)
    })
    .await;
    match built {
        Ok(built) => {
            // id/version 已过校验（无分隔符/控制字符），可直接拼入响应头
            let filename = format!("{}-{}.gamerpkg", built.manifest.id, built.manifest.version);
            let mut response = (StatusCode::OK, built.archive).into_response();
            let headers = response.headers_mut();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            if let Ok(value) =
                HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            {
                headers.insert(header::CONTENT_DISPOSITION, value);
            }
            if let Ok(value) = HeaderValue::from_str(&built.sha256) {
                headers.insert(CONTENT_SHA256_HEADER, value);
            }
            response
        }
        Err(e) => e.into_response(),
    }
}

// ---------- 包内预设发布（plugins/*/presets/*.yaml → task_presets） ----------

/// 解析一个包内预设 YAML（`name/runner_id/entrypoint/payload/schedule`，
/// schedule 以 `{kind, value}` 声明）。包安装侧与导入侧同一解析器。
fn parse_package_preset(bytes: &[u8], source: &str) -> anyhow::Result<crate::timer_core::PackagePreset> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RawPreset {
        name: String,
        runner_id: String,
        entrypoint: String,
        #[serde(default)]
        payload: Value,
        schedule: Value,
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|error| anyhow::anyhow!("{source}: 必须是 UTF-8 ({error})"))?;
    let raw: RawPreset = serde_yaml::from_str(text)
        .map_err(|error| anyhow::anyhow!("{source}: {error}"))?;
    for (field, value) in [
        ("name", raw.name.as_str()),
        ("runner_id", raw.runner_id.as_str()),
        ("entrypoint", raw.entrypoint.as_str()),
    ] {
        anyhow::ensure!(!value.trim().is_empty(), "{source}: {field} 不能为空");
    }
    let kind = raw
        .schedule
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("{source}: schedule.kind 必须是非空字符串"))?;
    let schedule = crate::timer_core::TaskSchedule::new(
        kind,
        raw.schedule.get("value").cloned().unwrap_or(Value::Null),
    )?;
    Ok(crate::timer_core::PackagePreset {
        name: raw.name.trim().to_string(),
        runner_id: raw.runner_id.trim().to_string(),
        entrypoint: raw.entrypoint.trim().to_string(),
        payload: raw.payload,
        schedule,
    })
}

/// 读取并发布包内 `plugins/*/presets/*.yaml`（按文件名排序；单个文件非法即
/// 整体失败，避免半套预设静默生效）。发布 id = `<package-id>:<名>`，幂等。
pub(super) fn publish_package_presets(
    store: &PackageStore,
    db: crate::store::Db,
    pkg: &str,
) -> anyhow::Result<usize> {
    let dir = store.package_dir(pkg)?.join("plugins");
    let mut presets = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for entry in rd.flatten() {
            let plugin = entry.file_name().to_string_lossy().to_string();
            if !is_valid_scope_id(&plugin) || !entry.path().is_dir() {
                continue;
            }
            let presets_dir = entry.path().join("presets");
            let Ok(files) = std::fs::read_dir(&presets_dir) else {
                continue; // 该插件无 presets 目录 = 零预设
            };
            let mut paths: Vec<std::path::PathBuf> = files
                .flatten()
                .map(|f| f.path())
                .filter(|p| {
                    p.is_file()
                        && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                            let lower = n.to_ascii_lowercase();
                            !n.starts_with('.')
                                && (lower.ends_with(".yaml") || lower.ends_with(".yml"))
                        })
                })
                .collect();
            paths.sort();
            for path in paths {
                let source = format!(
                    "{plugin}/presets/{}",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                );
                let bytes = std::fs::read(&path)?;
                presets.push(parse_package_preset(&bytes, &source)?);
            }
        }
    }
    if presets.is_empty() {
        return Ok(0);
    }
    // 阻塞上下文内的 async 收口（publish 只写 task_presets 行，不经调度循环）
    let timer = crate::timer_core::TimerCore::new(db);
    tokio::runtime::Handle::current().block_on(timer.publish_package_presets(pkg, &presets))
}

// ---------- 错误辅助 ----------

/// 导入结果：完成（fresh/overwrite）或结构化 409 冲突体（包已存在）。
enum ImportOutcome {
    Done { overwritten: bool, manifest: Value },
    Conflict(Value),
}

#[allow(clippy::result_large_err)]
fn expected_sha256_of(headers: &HeaderMap) -> Result<Option<String>, Response> {
    let Some(value) = headers.get(EXPECTED_SHA256_HEADER) else {
        return Ok(None);
    };
    let value = match value.to_str() {
        Ok(value) => value.trim().to_string(),
        Err(error) => {
            return Err(ApiError::bad_request(format!(
                "{EXPECTED_SHA256_HEADER} 头无效: {error}"
            ))
            .into_response())
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

fn internal(e: anyhow::Error) -> ApiError {
    tracing::warn!(error = %e, "package api internal error");
    ApiError::internal(e.to_string())
}

/// 包存储/manifest 错误到统一 HTTP 语义（404/409/400/500）的映射。
#[allow(clippy::result_large_err)]
fn store_error(e: anyhow::Error) -> ApiError {
    let message = e.to_string();
    if e.downcast_ref::<crate::resources::PackageNotFound>().is_some() {
        ApiError::not_found(message)
    } else if message.contains("已被其他页面修改")
        || message.contains("expected_revision")
        || message.contains("已存在")
        || message.contains("version_conflict")
        || message.contains("version_required")
    {
        ApiError::conflict(message)
    } else if message.contains("不存在") {
        ApiError::not_found(message)
    } else if message.contains("非法")
        || message.contains("不能")
        || message.contains("必须")
        || message.contains("超过")
        || message.contains("不可变")
        || message.contains("禁止")
    {
        ApiError::bad_request(message)
    } else {
        internal(e)
    }
}

#[allow(clippy::result_large_err)]
fn not_found_or_internal(e: anyhow::Error) -> ApiError {
    let is_missing = e.downcast_ref::<crate::resources::PackageNotFound>().is_some()
        || e.to_string().contains("不存在");
    if is_missing {
        ApiError::not_found(e.to_string())
    } else {
        internal(e)
    }
}

#[allow(clippy::result_large_err)]
fn archive_error(e: ArchiveError) -> ApiError {
    match &e {
        ArchiveError::NotFound(_) => ApiError::not_found(e.to_string()),
        ArchiveError::Io(_) | ArchiveError::Zip(_) => {
            internal(anyhow::anyhow!(e.to_string()))
        }
        ArchiveError::ArchiveTooLarge { .. } => ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            e.to_string(),
        ),
        _ => ApiError::bad_request(e.to_string()),
    }
}

#[allow(clippy::result_large_err)]
fn write_error(e: anyhow::Error) -> ApiError {
    let message = e.to_string();
    if message.contains("version_conflict") || message.contains("version_required") {
        ApiError::conflict(message)
    } else if message.contains("不存在") {
        ApiError::not_found(message)
    } else if message.contains("已存在")
        || message.contains("Package 已存在")
    {
        ApiError::conflict(message)
    } else if message.contains("非法")
        || message.contains("不能")
        || message.contains("必须")
        || message.contains("超过")
        || message.contains("名称未变化")
    {
        ApiError::bad_request(message)
    } else {
        tracing::warn!(%message, "package resource write failed");
        ApiError::internal(message)
    }
}
