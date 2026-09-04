//! Generic resource API（P11.6 / plan §11.2）。
//!
//! 统一资源端点：`/api/apps/:app/resources[/:kind[/:id]]`，
//! kind ∈ scripts | functions | templates | keymaps | presets | resources。
//! Core 只懂「目录类别 + 字节/文本 + 内容版本短码 + 原子写」；内容校验经
//! [`crate::resources::ResourceKindHandler`] 注册表回调给扩展（gamer.yaml →
//! scripts/functions，gamer.keymap → keymaps），未注册时保存不做内容校验
//! （裸 Core 语义，§8.9 验收锚点）。
//!
//! - 文本 kind：GET 返回资源 JSON（content/version/注记）；POST `{name,
//!   content}` 只创建；PUT `{content, name?, expected_version?, force?}`
//!   更新/重命名（乐观并发是通用能力，不是 YAML 语义）；
//! - 字节 kind（templates/resources）：GET 返回原始字节；POST（`?name=`）
//!   只创建；PUT 原始字节替换、或 JSON `{name}` 重命名（templates 重命名经
//!   gamer.yaml 钩子同步改写脚本/函数引用）；DELETE 删除；
//! - `app = "-"` 为跨分区通配（跨分区列表/按整 id 定位）。
//!
//! 模板图片上传/替换 = 原始字节 body（PNG 重编码按文件名 `#1` 颜色标记，
//! matcher 属 Core vision 能力）。

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use super::common::{require_pkg, run_blocking_api, validate_text_field};
use super::{ApiError, AppState};
use crate::matcher;
use crate::resources::{BinaryEntry, ResourceEntry, ResourceKind, ResourceStore};

// ---------- 名称校验 ----------

/// 模板名必须是单个分区目录内的普通文件名（保留 `#` 区域/颜色后缀语法）。
pub(super) fn validate_template_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.starts_with('.') {
        return Err(ApiError::bad_request("模板名不能为空或以 . 开头"));
    }
    if name.len() > 255 {
        return Err(ApiError::bad_request("模板名超过 255 字节"));
    }
    if crate::resources::sanitize_template_name(name).is_none() {
        return Err(ApiError::bad_request(
            "模板名包含非法字符（只允许字母数字 . - _ # 和空格）",
        ));
    }
    Ok(name.to_string())
}

/// 框选短名校验（服务端不再组合文件名；保留为命名编码规则锁定，实际文件名
/// 校验走 [`validate_template_name`]；生产 handler 不再消费，测试锁定用）。
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn validate_short_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    let Some(base) = name.strip_suffix(".png") else {
        return Err(ApiError::bad_request("短名非法（必须以 .png 结尾）"));
    };
    if base.is_empty()
        || base.len() > 251
        || !base
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ApiError::bad_request(
            "短名非法（只允许中英文/数字 - _，以 .png 结尾）",
        ));
    }
    Ok(name.to_string())
}

/// 相对搜索区域 `[x1,y1,x2,y2]`（0~1）→ `x1_y1_x2_y2` ×1000 三位整数后缀。
/// 服务端不再组合模板文件名（前端经 defaultTemplateName 同编码自组合），
/// 保留为命名编码的权威逆变换（测试锁定）。
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn compose_region_suffix(region: [f64; 4]) -> Result<String, ApiError> {
    if region.iter().any(|v| !v.is_finite()) {
        return Err(ApiError::bad_request("region 含非数字值"));
    }
    let to_int3 = |v: f64| ((v.clamp(0.0, 1.0) * 1000.0).round() as u32).min(999);
    let [x1, y1, x2, y2] = region;
    let (a, b, c, d) = (to_int3(x1), to_int3(y1), to_int3(x2), to_int3(y2));
    if c <= a || d <= b {
        return Err(ApiError::bad_request(
            "region 非法（需 x2>x1、y2>y1，为 0~1 相对坐标 [x1,y1,x2,y2]）",
        ));
    }
    Ok(format!("{a:03}_{b:03}_{c:03}_{d:03}"))
}

// ---------- 错误响应 ----------

/// 内容校验失败（400 invalid_yaml；诊断 JSON 形态由扩展校验器定义）。
fn invalid_content_response(diagnostics: Value) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": "invalid_yaml",
            "diagnostics": diagnostics,
        })),
    )
        .into_response()
}

/// 版本冲突 409（CONTRACT §5 资源级错误结构；文本 kind 更新共用）。
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

fn write_error(error: anyhow::Error, resource: &str) -> ApiError {
    let message = error.to_string();
    if message.contains("已存在") {
        ApiError::conflict(message)
    } else if message.contains("不存在") {
        ApiError::not_found(message)
    } else if message.contains("非法")
        || message.contains("不支持")
        || message.contains("名称未变化")
    {
        ApiError::bad_request(message)
    } else {
        tracing::warn!(resource, %message, "resource write failed");
        ApiError::internal(message)
    }
}

/// 保存闭包结果：内容校验失败需要结构化诊断 400，ApiError 承载不了 JSON 体，
/// 故经 blocking 边界带回后在此装配响应。
enum SaveOutcome {
    Saved(Value),
    Invalid(Value),
}

fn kind_of(kind: &str) -> Result<ResourceKind, ApiError> {
    ResourceKind::parse(kind).ok_or_else(|| {
        ApiError::bad_request(format!(
            "未知资源类别: {kind}（可选 scripts/functions/templates/keymaps/presets/resources）"
        ))
    })
}

/// `app = "-"` → 跨分区通配；否则校验为合法分区名。
fn resolve_app_filter(app: &str) -> Result<Option<String>, ApiError> {
    if app == "-" {
        return Ok(None);
    }
    require_pkg(Some(app)).map(Some)
}

fn annotate_entries(
    store: &ResourceStore,
    kind: ResourceKind,
    app: &str,
    entries: &mut [ResourceEntry],
) {
    store.annotate(kind, app, entries);
}

// ---------- 列表 ----------

/// GET /api/apps/:app/resources/:kind
pub(super) async fn api_list_kind_resources(
    State(st): State<AppState>,
    Path((app, kind)): Path<(String, String)>,
) -> Response {
    let Some(kind) = kind_of(&kind).ok() else {
        return ApiError::bad_request("未知资源类别").into_response();
    };
    let apps = match resolve_app_filter(&app) {
        Ok(apps) => apps,
        Err(err) => return err.into_response(),
    };
    match run_blocking_api(move || -> Result<Value, ApiError> {
        let store = st.resources.clone();
        let internal = |e: anyhow::Error| ApiError::internal(e.to_string());
        let apps = match apps {
            Some(app) => vec![app],
            None => store.partitions(),
        };
        if kind.is_text() {
            let mut all: Vec<ResourceEntry> = Vec::new();
            for app in &apps {
                all.extend(store.list_text(app, kind).map_err(internal)?);
            }
            all.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| a.id.cmp(&b.id))
            });
            annotate_entries(&store, kind, &app, &mut all);
            Ok(serde_json::to_value(all).unwrap_or_default())
        } else {
            let mut binaries: Vec<BinaryEntry> = Vec::new();
            for app in &apps {
                binaries.extend(store.list_binary(app, kind).map_err(internal)?);
            }
            binaries.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.id.cmp(&b.id)));
            Ok(serde_json::to_value(binaries).unwrap_or_default())
        }
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/apps/:app/resources：全部类别的资源清单（kind → 条目数组）。
pub(super) async fn api_list_all_resources(
    State(st): State<AppState>,
    Path(app): Path<String>,
) -> Response {
    let apps = match resolve_app_filter(&app) {
        Ok(apps) => apps,
        Err(err) => return err.into_response(),
    };
    match run_blocking_api(move || -> Result<Value, ApiError> {
        let store = st.resources.clone();
        let internal = |e: anyhow::Error| ApiError::internal(e.to_string());
        let apps = match apps {
            Some(app) => vec![app],
            None => store.partitions(),
        };
        let mut out = serde_json::Map::new();
        for kind in ResourceKind::ALL {
            let mut items: Vec<Value> = Vec::new();
            for app in &apps {
                if kind.is_text() {
                    let mut entries = store.list_text(app, kind).map_err(internal)?;
                    annotate_entries(&store, kind, app, &mut entries);
                    items.extend(
                        serde_json::to_value(entries)
                            .unwrap_or_default()
                            .as_array()
                            .cloned()
                            .unwrap_or_default(),
                    );
                } else {
                    let binaries = store.list_binary(app, kind).map_err(internal)?;
                    items.extend(
                        serde_json::to_value(binaries)
                            .unwrap_or_default()
                            .as_array()
                            .cloned()
                            .unwrap_or_default(),
                    );
                }
            }
            out.insert(kind.as_str().to_string(), Value::Array(items));
        }
        Ok(Value::Object(out))
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(e) => e.into_response(),
    }
}

// ---------- 读取 ----------

/// GET /api/apps/:app/resources/:kind/*id：文本 kind → 资源 JSON；字节 kind
/// → 原始字节（no-cache）。
pub(super) async fn api_get_resource(
    State(st): State<AppState>,
    Path((app, kind, id)): Path<(String, String, String)>,
) -> Response {
    let Some(kind) = kind_of(&kind).ok() else {
        return ApiError::bad_request("未知资源类别").into_response();
    };
    let app_filter = match resolve_app_filter(&app) {
        Ok(app) => app,
        Err(err) => return err.into_response(),
    };
    // 文本 kind 的 id 已含分区前缀（"<pkg>/<rel>"）；字节 kind 的 id 为分区
    // 内裸文件名。app 路径段为可选的显式校验（仅文本 kind 可校验前缀）。
    let id = id.trim().to_string();
    if kind.is_text() {
        if let Some(app) = &app_filter {
            if !id.starts_with(&format!("{app}/")) {
                return ApiError::not_found("资源不存在").into_response();
            }
        }
    }
    if kind.is_text() {
        let lookup_id = id.clone();
        match run_blocking_api(move || -> Result<Option<Value>, ApiError> {
            let store = st.resources.clone();
            let internal = |e: anyhow::Error| ApiError::internal(e.to_string());
            let entry = store.get_text(kind, &lookup_id).map_err(internal)?;
            Ok(entry.map(|mut entry| {
                let app = entry.package.clone();
                annotate_entries(&store, kind, &app, std::slice::from_mut(&mut entry));
                serde_json::to_value(entry).unwrap_or_default()
            }))
        })
        .await
        {
            Ok(Some(value)) => Json(value).into_response(),
            Ok(None) => ApiError::not_found("资源不存在").into_response(),
            Err(e) => e.into_response(),
        }
    } else {
        let lookup_id = id.clone();
        match run_blocking_api(move || -> Result<Option<Vec<u8>>, ApiError> {
            let store = st.resources.clone();
            let internal = |e: anyhow::Error| ApiError::internal(e.to_string());
            store.get_binary(kind, &lookup_id).map_err(internal)
        })
        .await
        {
            Ok(Some(bytes)) => {
                let mime = match id
                    .rsplit('.')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "jpg" | "jpeg" => "image/jpeg",
                    _ => "image/png",
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
}

// ---------- 创建 / 更新 / 删除（文本 kind） ----------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateTextReq {
    name: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateTextReq {
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

fn validate_text_content(name: &str, content: &str) -> Result<(), ApiError> {
    validate_text_field(name, "资源名", 255)?;
    if content.trim().is_empty() {
        return Err(ApiError::bad_request("资源内容不能为空"));
    }
    if content.len() > super::common::TEXT_RESOURCE_MAX_BYTES {
        return Err(ApiError::bad_request("资源内容超过 1 MiB"));
    }
    Ok(())
}

/// POST /api/apps/:app/resources/:kind：统一创建入口——文本 kind 收 JSON
/// `{name, content}`，字节 kind 收原始字节 + `?name=` 查询参数。
pub(super) async fn api_create_resource(
    State(st): State<AppState>,
    Path((app, kind)): Path<(String, String)>,
    Query(q): Query<TemplateQuery>,
    content_type: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let rkind = match kind_of(&kind) {
        Ok(k) => k,
        Err(e) => return e.into_response(),
    };
    if rkind.is_text() {
        api_create_text_resource_inner(st, app, rkind, content_type, body).await
    } else {
        api_create_binary_resource_inner(st, app, rkind, q, body).await
    }
}

/// PUT /api/apps/:app/resources/:kind/*id：统一更新入口——文本 kind 收 JSON，
/// 字节 kind 按请求体类型分派（JSON = templates 重命名；其余 = 字节替换）。
pub(super) async fn api_update_resource(
    State(st): State<AppState>,
    Path((app, kind, id)): Path<(String, String, String)>,
    content_type: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let rkind = match kind_of(&kind) {
        Ok(k) => k,
        Err(e) => return e.into_response(),
    };
    if rkind.is_text() {
        let Json(req) = match Json::<UpdateTextReq>::from_bytes(&body) {
            Ok(req) => req,
            Err(error) => return ApiError::bad_request(error.to_string()).into_response(),
        };
        api_update_text_resource_inner(st, app, rkind, id, req).await
    } else {
        api_put_binary_resource_inner(st, app, rkind, id, content_type, body).await
    }
}

async fn api_create_text_resource_inner(
    st: AppState,
    app: String,
    kind: ResourceKind,
    content_type: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if !content_type
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().contains("application/json"))
        .unwrap_or(false)
    {
        return ApiError::bad_request("文本资源创建请求体必须是 application/json").into_response();
    }
    let Ok(req) = serde_json::from_slice::<CreateTextReq>(&body) else {
        return ApiError::bad_request("请求体必须是 JSON 对象 {name, content}").into_response();
    };
    create_text_resource(st, app, kind, req).await
}

/// POST 文本 kind 创建（JSON `{name, content}`；只创建，不覆盖）。
async fn create_text_resource(
    st: AppState,
    app: String,
    kind: ResourceKind,
    req: CreateTextReq,
) -> Response {
    if let Err(err) = validate_text_content(&req.name, &req.content) {
        return err.into_response();
    }
    let name = req.name.trim().to_string();
    let content = req.content;
    match run_blocking_api(move || -> Result<SaveOutcome, ApiError> {
        let store = st.resources.clone();
        let rel = crate::resources::normalize_rel_name(kind, &name)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        if let Err(diagnostics) = store.validate_save(crate::resources::SaveValidation {
            app: &app,
            kind,
            id: &rel,
            content: &content,
            store: &store,
        }) {
            return Ok(SaveOutcome::Invalid(diagnostics));
        }
        let mut entry = store
            .save_text(kind, None, &app, &name, &content)
            .map_err(|e| write_error(e, &format!("{app}/{name}")))?;
        annotate_entries(&store, kind, &app, std::slice::from_mut(&mut entry));
        Ok(SaveOutcome::Saved(
            serde_json::to_value(entry).unwrap_or_default(),
        ))
    })
    .await
    {
        Ok(SaveOutcome::Saved(value)) => (StatusCode::CREATED, Json(value)).into_response(),
        Ok(SaveOutcome::Invalid(diagnostics)) => invalid_content_response(diagnostics),
        Err(e) => e.into_response(),
    }
}

async fn api_create_binary_resource_inner(
    st: AppState,
    app: String,
    rkind: ResourceKind,
    q: TemplateQuery,
    body: axum::body::Bytes,
) -> Response {
    // 分区 = 路径段；?pkg= 仅作显式覆盖（必须一致时由调用方保证，这里以
    // 查询参数优先）
    let pkg = match q.pkg.as_deref() {
        Some(pkg) => match require_pkg(Some(pkg)) {
            Ok(pkg) => pkg,
            Err(e) => return e.into_response(),
        },
        None => app,
    };
    let name = match q.name.as_deref() {
        Some(name) => match validate_template_name(name) {
            Ok(name) => name,
            Err(e) => return e.into_response(),
        },
        None => {
            return ApiError::bad_request(
                "缺少 name 查询参数（完整模板文件名，如 login#001_002_003_004.png）",
            )
            .into_response()
        }
    };
    if body.is_empty() {
        return ApiError::bad_request("字节内容为空").into_response();
    }
    match run_blocking_api(move || -> Result<Value, ApiError> {
        let store = st.resources.clone();
        let bytes = reencode_bytes(&body, &name)?;
        let orig_size = body.len();
        let path = store
            .create_binary(rkind, &pkg, &name, &bytes)
            .map_err(|e| write_error(e, &format!("{pkg}/{name}")))?;
        if rkind == ResourceKind::Templates {
            matcher::invalidate_template_cache_path(&path);
        }
        Ok(serde_json::json!({
            "ok": true,
            "name": name,
            "size": bytes.len(),
            "orig_size": orig_size,
        }))
    })
    .await
    {
        Ok(value) => (StatusCode::CREATED, Json(value)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn api_update_text_resource_inner(
    st: AppState,
    app: String,
    kind: ResourceKind,
    id: String,
    req: UpdateTextReq,
) -> Response {
    // 文本 kind 的 id 已含分区前缀；app 路径段为可选的显式校验。
    let app_filter = resolve_app_filter(&app).ok().flatten();
    if let Some(app) = &app_filter {
        if !id.starts_with(&format!("{app}/")) {
            return ApiError::not_found("资源不存在").into_response();
        }
    }
    let full_id = id.clone();
    if req.content.trim().is_empty() {
        return ApiError::bad_request("资源内容不能为空").into_response();
    }
    if req.content.len() > super::common::TEXT_RESOURCE_MAX_BYTES {
        return ApiError::bad_request("资源内容超过 1 MiB").into_response();
    }
    let source = match run_blocking_api({
        let st = st.clone();
        let full_id = full_id.clone();
        move || -> Result<Option<ResourceEntry>, ApiError> {
            st.resources
                .get_text(kind, &full_id)
                .map_err(|e| ApiError::internal(e.to_string()))
        }
    })
    .await
    {
        Ok(Some(entry)) => entry,
        Ok(None) => return ApiError::not_found("资源不存在").into_response(),
        Err(err) => return err.into_response(),
    };
    if !req.force && req.expected_version.is_none() {
        return version_required(&full_id);
    }
    if !req.force && req.expected_version.as_deref() != Some(source.version().as_str()) {
        return version_conflict(&full_id);
    }
    let target_name = req.name.clone().unwrap_or_else(|| source.name.clone());
    if let Err(err) = validate_text_content(&target_name, &req.content) {
        return err.into_response();
    }
    let content = req.content;
    match run_blocking_api(move || -> Result<SaveOutcome, ApiError> {
        let store = st.resources.clone();
        let rel_for_validation = crate::resources::normalize_rel_name(kind, &target_name)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        if let Err(diagnostics) = store.validate_save(crate::resources::SaveValidation {
            app: &app,
            kind,
            id: &rel_for_validation,
            content: &content,
            store: &store,
        }) {
            return Ok(SaveOutcome::Invalid(diagnostics));
        }
        let mut entry = store
            .save_text(kind, Some(&full_id), &app, &target_name, &content)
            .map_err(|e| write_error(e, &full_id))?;
        annotate_entries(&store, kind, &app, std::slice::from_mut(&mut entry));
        Ok(SaveOutcome::Saved(
            serde_json::to_value(entry).unwrap_or_default(),
        ))
    })
    .await
    {
        Ok(SaveOutcome::Saved(value)) => Json(value).into_response(),
        Ok(SaveOutcome::Invalid(diagnostics)) => invalid_content_response(diagnostics),
        Err(e) => e.into_response(),
    }
}

/// DELETE /api/apps/:app/resources/:kind/*id
pub(super) async fn api_delete_resource(
    State(st): State<AppState>,
    Path((app, kind, id)): Path<(String, String, String)>,
) -> Response {
    let Some(kind) = kind_of(&kind).ok() else {
        return ApiError::bad_request("未知资源类别").into_response();
    };
    // 文本 kind 的 id 已含分区前缀；字节 kind 的 id 为分区内的裸文件名。
    let app_filter = resolve_app_filter(&app).ok().flatten();
    if kind.is_text() {
        if let Some(app) = &app_filter {
            if !id.starts_with(&format!("{app}/")) {
                return ApiError::not_found("资源不存在").into_response();
            }
        }
    }
    let full_id = id.clone();
    let invalidated =
        match run_blocking_api(move || -> Result<Vec<std::path::PathBuf>, ApiError> {
            let store = st.resources.clone();
            if kind.is_text() {
                let exists = store
                    .get_text(kind, &full_id)
                    .map_err(|e| ApiError::internal(e.to_string()))?;
                if exists.is_none() {
                    return Err(ApiError::not_found("资源不存在"));
                }
                store
                    .delete_text(kind, &full_id)
                    .map(|_| Vec::new())
                    .map_err(|e| write_error(e, &full_id))
            } else {
                let app_of = app.clone();
                store
                    .delete_binary(kind, &app, &id)
                    .map(|path| vec![path])
                    .map_err(|e| write_error(e, &format!("{app_of}/{id}")))
            }
        })
        .await
        {
            Ok(paths) => paths,
            Err(e) => return e.into_response(),
        };
    for path in invalidated {
        if kind == ResourceKind::Templates {
            matcher::invalidate_template_cache_path(&path);
        }
    }
    Json(serde_json::json!({"ok": true})).into_response()
}

// ---------- 字节 kind（templates / resources） ----------

#[derive(Deserialize, Default)]
pub(super) struct TemplateQuery {
    pub(super) name: Option<String>,
    pub(super) pkg: Option<String>,
}

fn reencode_bytes(body: &[u8], name: &str) -> Result<Vec<u8>, ApiError> {
    if body.len() > matcher::TEMPLATE_MAX_INPUT_BYTES {
        return Err(ApiError::bad_request(
            "图片超过上传上限（10 MiB），请裁剪后再试",
        ));
    }
    let grayscale_only = !matcher::template_color_from_name(name);
    matcher::reencode_template_png(body, grayscale_only)
        .map_err(|e| ApiError::bad_request(e.to_string()))
}

/// PUT templates/*id：字节替换（非 JSON body）或 JSON 重命名（经 gamer.yaml
/// 钩子同步改写脚本/函数中的模板引用）。
async fn api_put_binary_resource_inner(
    st: AppState,
    app: String,
    rkind: ResourceKind,
    id: String,
    content_type: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let is_json_rename = content_type
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().contains("application/json"))
        .unwrap_or(false);

    if is_json_rename {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RenameReq {
            name: String,
        }
        let Ok(req) = serde_json::from_slice::<RenameReq>(&body) else {
            return ApiError::bad_request("重命名请求体必须是 {\"name\": ...}").into_response();
        };
        let new_name = match validate_template_name(&req.name) {
            Ok(name) => name,
            Err(e) => return e.into_response(),
        };
        let Some(pkg) = resolve_app_filter(&app).ok().flatten() else {
            return ApiError::bad_request("重命名需要明确的应用分区").into_response();
        };
        let dir = st.resources.kind_dir(&pkg, rkind);
        let old_path = dir.join(&id);
        let new_path = dir.join(&new_name);
        let new_name_for_response = new_name.clone();
        let renamed = run_blocking_api(move || -> Result<(), ApiError> {
            let store = st.resources.clone();
            store
                .rename_binary(rkind, &pkg, &id, &new_name)
                .map_err(|e| write_error(e, &format!("{pkg}/{id}")))
        })
        .await;
        match renamed {
            Ok(()) => {
                matcher::invalidate_template_cache_path(&old_path);
                matcher::invalidate_template_cache_path(&new_path);
                Json(serde_json::json!({"ok": true, "name": new_name_for_response})).into_response()
            }
            Err(e) => e.into_response(),
        }
    } else {
        // 字节替换（旧 PUT /api/templates/:name/image 语义）
        if body.is_empty() {
            return ApiError::bad_request("字节内容为空").into_response();
        }
        let name = match validate_template_name(&id) {
            Ok(name) => name,
            Err(e) => return e.into_response(),
        };
        let Some(pkg) = resolve_app_filter(&app).ok().flatten() else {
            return ApiError::bad_request("模板替换需要明确的应用分区").into_response();
        };
        match run_blocking_api(move || -> Result<Value, ApiError> {
            let store = st.resources.clone();
            let bytes = reencode_bytes(&body, &name)?;
            let orig_size = body.len();
            let path = store
                .replace_binary(rkind, &pkg, &name, &bytes)
                .map_err(|e| write_error(e, &format!("{pkg}/{name}")))?;
            if rkind == ResourceKind::Templates {
                matcher::invalidate_template_cache_path(&path);
            }
            Ok(serde_json::json!({
                "ok": true,
                "name": name,
                "size": bytes.len(),
                "orig_size": orig_size,
            }))
        })
        .await
        {
            Ok(value) => Json(value).into_response(),
            Err(e) => e.into_response(),
        }
    }
}
