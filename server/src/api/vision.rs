//! Vision 能力位端点（P11.6）：模板「匹配测试」是 vision 能力语义（Core 合法），
//! 自旧 `/api/templates/:name/test` 迁入 `POST /api/capabilities/vision/test`。
//!
//! 语义不变：支持模板短名（经 composite 三层消歧），区域/颜色由消歧后的实际
//! 文件名 `#` 后缀决定；NCC 匹配走专用计算池（PERF-003）。

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::Json;
use image::GenericImageView;
use serde::Deserialize;

use super::common::{require_pkg, run_blocking_api};
use super::{ApiError, AppState};
use crate::matcher;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct VisionTestReq {
    device_id: String,
    /// 模板短名或完整文件名（分区 templates/ 内）。
    name: String,
    threshold: Option<f32>,
    region: Option<[u32; 4]>,
    pkg: String,
}

pub(super) async fn api_vision_test_template(
    State(st): State<AppState>,
    Json(req): Json<VisionTestReq>,
) -> Response {
    let pkg = match require_pkg(Some(&req.pkg)) {
        Ok(pkg) => pkg,
        Err(err) => return err.into_response(),
    };
    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 255 {
        return ApiError::bad_request("模板名非法").into_response();
    }
    // 与引擎一致：支持脚本中的模板短名，并以消歧后的实际文件名解析区域后缀。
    // 编辑器的单次预览不应因为省略 #区域后缀而走另一套匹配语义。
    let resources = st.resources.clone();
    let (tpl_bytes, resolved_name) = match run_blocking_api(move || {
        let tpl_path = resources
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
