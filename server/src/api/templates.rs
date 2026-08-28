//! Template partition listing, upload, rename, deletion, testing, and image reads.

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use serde::Deserialize;

use super::common::{err_response, require_pkg, run_blocking_api};
use super::{ApiError, AppState};
use crate::matcher;

// ---------- 模板（按应用分区 data/<pkg>/tmpl） ----------

#[derive(Deserialize)]
pub(super) struct PkgQuery {
    pub(super) pkg: Option<String>,
}

/// 模板名必须是单个分区目录内的普通文件名。之前的 sanitize 逻辑会把
/// `/`、反斜杠和控制字符静默改成 `_`，容易让调用方误以为写入了原名；
/// 路由层现在明确拒绝这类输入，保留 `#` 区域后缀语法。
pub(super) fn validate_template_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.starts_with('.') {
        return Err(ApiError::bad_request("模板名不能为空或以 . 开头"));
    }
    if name.len() > 255 {
        return Err(ApiError::bad_request("模板名超过 255 字节"));
    }
    if name
        .chars()
        .any(|c| !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '#' | ' ')))
    {
        return Err(ApiError::bad_request(
            "模板名包含非法字符（只允许字母数字 . - _ # 和空格）",
        ));
    }
    Ok(name.to_string())
}

pub(super) async fn api_list_templates(
    State(st): State<AppState>,
    Query(q): Query<PkgQuery>,
) -> Response {
    let scripts = st.scripts.clone();
    let pkgs: Vec<String> = match q.pkg.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => match require_pkg(Some(p)) {
            Ok(v) => vec![v],
            Err(err) => return err.into_response(),
        },
        None => match run_blocking_api({
            let scripts = scripts.clone();
            move || Ok(scripts.partitions())
        })
        .await
        {
            Ok(pkgs) => pkgs,
            Err(err) => return err.into_response(),
        },
    };
    match run_blocking_api(move || {
        let mut out = Vec::new();
        for pkg in pkgs {
            let dir = scripts.tmpl_dir(&pkg);
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    // 模板目录专用：列出所有非隐藏文件（模板名可能带 .png/.jpg，也可能是 随机名字#x1_y1_x2_y2 这种带小数点无后缀名）
                    let fname = e.file_name().to_string_lossy().to_string();
                    if e.path().is_file() && !fname.starts_with('.') {
                        let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                        // mtime（unix 秒）：前端按修改时间倒序排模板列表
                        let mtime = e
                            .metadata()
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        out.push(
                            serde_json::json!({"name": fname, "size": size, "mtime": mtime, "pkg": pkg}),
                        );
                    }
                }
            }
        }
        Ok(out)
    })
    .await
    {
        Ok(out) => Json(out).into_response(),
        Err(err) => err.into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct UploadTemplateReq {
    name: String,
    data_b64: String,
    pkg: String,
}

pub(super) async fn api_upload_template(
    State(st): State<AppState>,
    Json(req): Json<UploadTemplateReq>,
) -> Response {
    let pkg = match require_pkg(Some(&req.pkg)) {
        Ok(pkg) => pkg,
        Err(err) => return err.into_response(),
    };
    let name = match validate_template_name(&req.name) {
        Ok(name) => name,
        Err(err) => return err.into_response(),
    };
    // base64 合法性与体积先于解码校验（4/3 膨胀后 16MiB ≈ 原始 12MiB 内的护栏）
    const MAX_B64_LEN: usize = (matcher::TEMPLATE_MAX_INPUT_BYTES / 3 + 1) * 4;
    if req.data_b64.len() > MAX_B64_LEN {
        return err_response(
            StatusCode::BAD_REQUEST,
            "图片超过上传上限（10 MiB），请裁剪后再试",
        );
    }
    // base64 解码和统一灰度重编码都可能处理较大的上传内容，连同文件落盘
    // 一并放入 blocking 边界，避免占用 Tokio 核心线程。
    let data_b64 = req.data_b64;
    let (bytes, orig_size) = match run_blocking_api(move || {
        let orig = base64::engine::general_purpose::STANDARD
            .decode(data_b64)
            .map_err(|e| ApiError::bad_request(format!("base64 解码失败: {}", e)))?;
        let orig_size = orig.len();
        let bytes = matcher::reencode_template_gray_png(&orig)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        Ok((bytes, orig_size))
    })
    .await
    {
        Ok(result) => result,
        Err(e) => return e.into_response(),
    };
    match run_blocking_api(move || {
        let dir = st.scripts.tmpl_dir(&pkg);
        std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
        let path = dir.join(&name);
        crate::scripts::atomic_write(&path, &bytes)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(Json(
            serde_json::json!({"ok": true, "name": name, "size": bytes.len(), "orig_size": orig_size}),
        ))
    })
    .await
    {
        Ok(resp) => resp.into_response(),
        Err(err) => err.into_response(),
    }
}

pub(super) async fn api_delete_template(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<PkgQuery>,
) -> Response {
    let pkg = match require_pkg(q.pkg.as_deref()) {
        Ok(pkg) => pkg,
        Err(err) => return err.into_response(),
    };
    let name = match validate_template_name(&name) {
        Ok(name) => name,
        Err(err) => return err.into_response(),
    };
    match run_blocking_api(move || {
        let path = st.scripts.tmpl_dir(&pkg).join(&name);
        std::fs::remove_file(&path).map_err(|e| ApiError::internal(e.to_string()))?;
        st.scripts.cleanup_partition(&pkg); // 分区 yaml/tmpl 都空了则清理目录
        Ok(Json(serde_json::json!({"ok": true})))
    })
    .await
    {
        Ok(resp) => resp.into_response(),
        Err(err) => err.into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct RenameTemplateReq {
    name: String,
}

/// 重命名模板：把旧文件字节写入新文件名，再删除旧文件
pub(super) async fn api_rename_template(
    State(st): State<AppState>,
    Path(old_name): Path<String>,
    Query(q): Query<PkgQuery>,
    Json(req): Json<RenameTemplateReq>,
) -> Response {
    let pkg = match require_pkg(q.pkg.as_deref()) {
        Ok(pkg) => pkg,
        Err(err) => return err.into_response(),
    };
    let old_name = match validate_template_name(&old_name) {
        Ok(name) => name,
        Err(err) => return err.into_response(),
    };
    let new_name = match validate_template_name(&req.name) {
        Ok(name) => name,
        Err(err) => return err.into_response(),
    };
    if new_name == old_name {
        return ApiError::bad_request("名称未变化").into_response();
    }
    match run_blocking_api(move || {
        let dir = st.scripts.tmpl_dir(&pkg);
        let old_path = dir.join(&old_name);
        let new_path = dir.join(&new_name);
        if new_path.exists() {
            return Err(ApiError::bad_request("已存在同名模板"));
        }
        let bytes = std::fs::read(&old_path).map_err(|_| ApiError::not_found("模板不存在"))?;
        crate::scripts::atomic_write(&new_path, &bytes)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if std::fs::remove_file(&old_path).is_err() {
            let _ = std::fs::remove_file(&new_path);
            return Err(ApiError::internal("旧模板删除失败"));
        }
        Ok(Json(serde_json::json!({"ok": true, "name": new_name})))
    })
    .await
    {
        Ok(resp) => resp.into_response(),
        Err(err) => err.into_response(),
    }
}

/// 返回模板图片原始字节（PNG/JPEG），供前端缩略图与预览使用。
/// Cache-Control: no-cache —— 模板被同名覆盖上传后浏览器必须重新拉取。
pub(super) async fn api_get_template_image(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<PkgQuery>,
) -> Response {
    let pkg = match require_pkg(q.pkg.as_deref()) {
        Ok(pkg) => pkg,
        Err(err) => return err.into_response(),
    };
    let name = match validate_template_name(&name) {
        Ok(name) => name,
        Err(err) => return err.into_response(),
    };
    match run_blocking_api(move || {
        let path = st.scripts.tmpl_dir(&pkg).join(&name);
        let bytes = std::fs::read(&path).map_err(|_| ApiError::not_found("模板不存在"))?;
        let mime = match path
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "jpg" | "jpeg" => "image/jpeg",
            _ => "image/png",
        };
        Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            bytes,
        ))
    })
    .await
    {
        Ok(resp) => resp.into_response(),
        Err(err) => err.into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct TestTemplateReq {
    device_id: String,
    threshold: Option<f32>,
    region: Option<[u32; 4]>,
    pkg: String,
}

pub(super) async fn api_test_template(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<TestTemplateReq>,
) -> Response {
    let pkg = match require_pkg(Some(&req.pkg)) {
        Ok(pkg) => pkg,
        Err(err) => return err.into_response(),
    };
    let name = match validate_template_name(&name) {
        Ok(name) => name,
        Err(err) => return err.into_response(),
    };
    let tpl_bytes = match run_blocking_api(move || {
        let tpl_path = st.scripts.tmpl_dir(&pkg).join(&name);
        std::fs::read(&tpl_path).map_err(|_| ApiError::not_found("模板不存在"))
    })
    .await
    {
        Ok(bytes) => bytes,
        Err(err) => return err.into_response(),
    };
    let screen = match st.devices.screenshot(&req.device_id).await {
        Ok(s) => s,
        Err(e) => return ApiError::bad_gateway(format!("截图失败: {}", e)).into_response(),
    };
    let mr = matcher::MatchRequest {
        screen_png: screen,
        template_png: tpl_bytes,
        threshold: req.threshold,
        region: req.region,
    };
    match run_blocking_api(move || {
        matcher::match_template(&mr)
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    {
        Ok(Some(m)) => Json(serde_json::json!({"hit": true, "x": m.x, "y": m.y, "width": m.width, "height": m.height, "score": m.score})).into_response(),
        Ok(None) => Json(serde_json::json!({"hit": false})).into_response(),
        Err(e) => e.into_response(),
    }
}
