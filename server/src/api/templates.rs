//! Template partition listing, upload, rename, deletion, testing, and image reads.

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use image::GenericImageView;
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

/// 框选上传短名校验：unicode 字母数字 + `-` `_` + `.png`。
/// 框选默认名与用户习惯都是中文（如「委托界面.png」），ASCII 白名单会整批拒掉；
/// `#` 仍必须拒绝——它是服务端组合区域后缀的分隔符。完整文件名由服务端组合
/// （短名 + 搜索区域 #×1000 后缀），前端不拼接 # 元数据。
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
/// 与前端 defaultTemplateName 同编码，是引擎 `tpl_region_from_name` 数字分支
/// 的逆变换（后者要求 x2>x1、y2>y1，此处组合前同口径校验）。
pub(super) fn compose_region_suffix(region: [f64; 4]) -> Result<String, ApiError> {
    if region.iter().any(|v| !v.is_finite()) {
        return Err(ApiError::bad_request("region 含非数字值"));
    }
    // 越界夹取到 0~1、×1000 取整并钳到 999（与前端 toInt3 一致）
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
            let dir = scripts.templates_dir(&pkg);
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

/// 模板创建请求为 {short_name, region?, pkg, data_b64, grayscale_only?}。短名冲突
/// 409 且不覆盖；已有图片的替换走独立 image PUT。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UploadTemplateReq {
    short_name: String,
    region: Option<[f64; 4]>,
    data_b64: String,
    pkg: String,
    /// 默认只压缩为灰度；裁切弹窗明确勾选“保留颜色”时传 false。
    #[serde(default = "default_grayscale_only")]
    grayscale_only: bool,
}

fn default_grayscale_only() -> bool {
    true
}

/// 短名冲突检查（新形态专用，plan §11.7：冲突要求改名不覆盖）：分区内存在
/// 同基名文件（任意扩展名，含 `#` 后缀变体，大小写不敏感对齐 Windows FS）即冲突。
/// 短名引用靠「基名 + # 后缀唯一候选」消歧，放行第二个同基名文件会制造歧义。
fn short_name_conflict(dir: &std::path::Path, base: &str) -> bool {
    let base = base.to_ascii_lowercase();
    let prefix = format!("{base}#");
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .any(|n| match n.rsplit_once('.') {
            Some((stem, _)) => {
                let stem = stem.to_ascii_lowercase();
                stem == base || stem.starts_with(&prefix)
            }
            None => false,
        })
}

/// 兼容现有 mod.rs 的 handler 名称；集成负责人可切换到 api_create_template。
pub(super) async fn api_upload_template(
    State(st): State<AppState>,
    Json(req): Json<UploadTemplateReq>,
) -> Response {
    api_create_template(State(st), Json(req)).await
}

/// POST /api/templates：只创建模板，不覆盖已有图片。
pub(super) async fn api_create_template(
    State(st): State<AppState>,
    Json(req): Json<UploadTemplateReq>,
) -> Response {
    let pkg = match require_pkg(Some(&req.pkg)) {
        Ok(pkg) => pkg,
        Err(err) => return err.into_response(),
    };
    let short = match validate_short_name(&req.short_name) {
        Ok(v) => v,
        Err(err) => return err.into_response(),
    };
    let base = short[..short.len() - 4].to_string(); // 去 ".png"
    let color_suffix = if req.grayscale_only { "" } else { "#1" };
    let name = match req.region {
        Some(region) => match compose_region_suffix(region) {
            Ok(suffix) => format!("{base}#{suffix}{color_suffix}.png"),
            Err(err) => return err.into_response(),
        },
        None => format!("{base}{color_suffix}.png"),
    };
    // base64 合法性与体积先于解码校验（4/3 膨胀后 16MiB ≈ 原始 12MiB 内的护栏）
    const MAX_B64_LEN: usize = (matcher::TEMPLATE_MAX_INPUT_BYTES / 3 + 1) * 4;
    if req.data_b64.len() > MAX_B64_LEN {
        return err_response(
            StatusCode::BAD_REQUEST,
            "图片超过上传上限（10 MiB），请裁剪后再试",
        );
    }
    // base64 解码和 PNG 重编码都可能处理较大的上传内容，连同文件落盘
    // 一并放入 blocking 边界，避免占用 Tokio 核心线程。
    let data_b64 = req.data_b64;
    let grayscale_only = req.grayscale_only;
    let (bytes, orig_size) = match run_blocking_api(move || {
        let orig = base64::engine::general_purpose::STANDARD
            .decode(data_b64)
            .map_err(|e| ApiError::bad_request(format!("base64 解码失败: {}", e)))?;
        let orig_size = orig.len();
        let bytes = matcher::reencode_template_png(&orig, grayscale_only)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        Ok((bytes, orig_size))
    })
    .await
    {
        Ok(result) => result,
        Err(e) => return e.into_response(),
    };
    match run_blocking_api(move || {
        let dir = st.scripts.templates_dir(&pkg);
        // 短名冲突即 409（§11.7 冲突要求改名不自动覆盖）。
        if short_name_conflict(&dir, &base) {
            return Err(ApiError::conflict(format!(
                "短名 {base}.png 已存在，请改名（不会覆盖）"
            )));
        }
        std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
        let path = dir.join(&name);
        crate::core::fs::atomic_write(&path, &bytes)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        // 覆盖上传成功后主动失效该路径的模板预处理缓存（PERF-002；
        // mtime/size/hash 兜底仍在）。失败路径不失效。
        matcher::invalidate_template_cache_path(&path);
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReplaceTemplateImageReq {
    data_b64: String,
}

/// PUT /api/templates/:name/image?pkg=...：只替换已有模板的图片字节。
/// 请求体严格只有 data_b64，名称与分区均来自 URL/query。
pub(super) async fn api_replace_template_image(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<PkgQuery>,
    Json(req): Json<ReplaceTemplateImageReq>,
) -> Response {
    let pkg = match require_pkg(q.pkg.as_deref()) {
        Ok(pkg) => pkg,
        Err(err) => return err.into_response(),
    };
    let name = match validate_template_name(&name) {
        Ok(name) => name,
        Err(err) => return err.into_response(),
    };
    const MAX_B64_LEN: usize = (matcher::TEMPLATE_MAX_INPUT_BYTES / 3 + 1) * 4;
    if req.data_b64.len() > MAX_B64_LEN {
        return err_response(
            StatusCode::BAD_REQUEST,
            "图片超过上传上限（10 MiB），请裁剪后再试",
        );
    }
    let data_b64 = req.data_b64;
    // 替换沿用现有文件名语义：旧格式继续灰度，带 #1 的彩色模板继续保留颜色。
    let grayscale_only = !matcher::template_color_from_name(&name);
    let (bytes, orig_size) = match run_blocking_api(move || {
        let orig = base64::engine::general_purpose::STANDARD
            .decode(data_b64)
            .map_err(|e| ApiError::bad_request(format!("base64 解码失败: {e}")))?;
        let orig_size = orig.len();
        let bytes = matcher::reencode_template_png(&orig, grayscale_only)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        Ok((bytes, orig_size))
    })
    .await
    {
        Ok(result) => result,
        Err(err) => return err.into_response(),
    };
    match run_blocking_api(move || {
        let path = st.scripts.templates_dir(&pkg).join(&name);
        if !path.is_file() {
            return Err(ApiError::not_found("模板不存在"));
        }
        crate::core::fs::atomic_write(&path, &bytes)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        matcher::invalidate_template_cache_path(&path);
        Ok(Json(serde_json::json!({
            "ok": true,
            "name": name,
            "size": bytes.len(),
            "orig_size": orig_size,
        })))
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
        let path = st.scripts.templates_dir(&pkg).join(&name);
        std::fs::remove_file(&path).map_err(|e| ApiError::internal(e.to_string()))?;
        // 删除成功后主动失效该路径缓存（PERF-002）；失败路径不失效
        matcher::invalidate_template_cache_path(&path);
        st.scripts.cleanup_partition(&pkg); // 分区 scripts/templates 都空了则清理目录
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

/// 重命名模板：同步改写当前分区脚本/函数中的模板引用，再改名模板文件。
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
        let dir = st.scripts.templates_dir(&pkg);
        let old_path = dir.join(&old_name);
        let new_path = dir.join(&new_name);
        let updated_scripts = st
            .scripts
            .rename_template(&pkg, &old_name, &new_name)
            .map_err(|e| {
                if e.to_string().contains("模板不存在") {
                    ApiError::not_found(e.to_string())
                } else if e.to_string().contains("同名模板") {
                    ApiError::bad_request(e.to_string())
                } else {
                    ApiError::internal(e.to_string())
                }
            })?;
        // 重命名 = 写新删旧：新旧两条路径的模板缓存与短名解析缓存一起失效。
        matcher::invalidate_template_cache_path(&new_path);
        matcher::invalidate_template_cache_path(&old_path);
        Ok(Json(serde_json::json!({
            "ok": true,
            "name": new_name,
            "updated_scripts": updated_scripts,
        })))
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
        let path = st.scripts.templates_dir(&pkg).join(&name);
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
    // 与引擎一致：支持脚本中的模板短名，并以消歧后的实际文件名解析区域后缀。
    // 编辑器的单次预览不应因为省略 #区域后缀而走另一套匹配语义。
    let scripts = st.scripts.clone();
    let (tpl_bytes, resolved_name) = match run_blocking_api(move || {
        let tpl_path = scripts
            .resolve_template_path(&pkg, &name)
            .map_err(|e| ApiError::not_found(e.to_string()))?;
        let resolved_name = tpl_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.clone());
        let bytes = std::fs::read(&tpl_path).map_err(|_| ApiError::not_found("模板不存在"))?;
        Ok((bytes, resolved_name))
    })
    .await
    {
        Ok(result) => result,
        Err(err) => return err.into_response(),
    };
    let screen = match st.devices.screenshot(&req.device_id).await {
        Ok(s) => s,
        Err(e) => return ApiError::bad_gateway(format!("截图失败: {}", e)).into_response(),
    };
    let (screen_w, screen_h) = st
        .devices
        .session(&req.device_id)
        .map(|session| session.video_size())
        .filter(|(w, h)| *w > 0 && *h > 0)
        .unwrap_or_else(|| {
            image::load_from_memory(&screen)
                .map(|image| image.dimensions())
                .unwrap_or((0, 0))
        });
    let mr = matcher::MatchRequest {
        screen_png: screen,
        template_png: tpl_bytes,
        // 缺省阈值与函数/脚本实际运行的服务端默认值一致；脚本编辑态会显式传
        // 当前脚本 config.threshold 覆盖它。
        threshold: req.threshold.or(Some(st.cfg.threshold)),
        region: req
            .region
            .or_else(|| matcher::template_region_from_name(&resolved_name, screen_w, screen_h)),
        color: matcher::template_color_from_name(&resolved_name),
    };
    let miss_region = mr.region;
    // NCC 匹配（含截图/模板 PNG 解码）走专用计算池（PERF-003），与引擎同一条
    // CPU 预算通道，不再占用 API blocking 池名额
    match matcher::compute::run(move || {
        matcher::match_template(&mr).map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))
    .and_then(|inner| inner)
    {
        Ok(Some(m)) => Json(serde_json::json!({"hit": true, "x": m.x, "y": m.y, "width": m.width, "height": m.height, "score": m.score})).into_response(),
        Ok(None) => Json(serde_json::json!({"hit": false, "region": miss_region})).into_response(),
        Err(e) => e.into_response(),
    }
}
