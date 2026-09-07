//! Vision 能力位端点（P11.6）：模板「匹配测试」是 vision 能力语义（Core 合法），
//! 自旧 `/api/templates/:name/test` 迁入 `POST /api/capabilities/vision/test`。
//!
//! 寻址 = (pkg, plugin, name) 三元组显式给出：Core 不预设/不猜测任何业务插件
//! id，缺 `plugin` 即 400。支持模板短名（`#` 后缀唯一候选消歧），区域/颜色由
//! 消歧后的实际文件名 `#` 后缀决定；NCC 匹配走专用计算池（PERF-003）。
//!
//! 离线路径（视频工作台 V1，实施合同 §1.1）：请求给 `media_id`（+可选
//! `pts_us`，缺省 0 = 首帧）时目标帧来自媒体库精确抽帧（`crate::media`），
//! 与设备路径**互斥**——该路径不要求设备存在、绝不触达 ADB/设备截图。
//! 抽帧 PNG 经 ffmpeg autorotate，命中坐标即 oriented 展示空间。

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::Json;
use image::GenericImageView;
use serde::Deserialize;

use super::common::run_blocking_api;
use super::{ApiError, AppState};
use crate::matcher;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct VisionTestReq {
    /// 在线路径：目标设备 id（与 media_id 互斥）。
    device_id: Option<String>,
    /// 模板短名或完整文件名（`plugins/<plugin>/templates/` 内）。
    name: String,
    threshold: Option<f32>,
    region: Option<[u32; 4]>,
    /// 目标 Package id（数据作用域；不再是 Android 包名分区）。
    pkg: String,
    /// 目标插件 id（必填；模板归属 `plugins/<plugin>/templates/`）。
    plugin: String,
    /// 离线路径：媒体素材 id（与 device_id 互斥；给此字段即离线测试）。
    media_id: Option<String>,
    /// 离线路径：目标帧 pts（微秒）；缺省 0 = 首帧。
    pts_us: Option<u64>,
}

pub(super) async fn api_vision_test_template(
    State(st): State<AppState>,
    Json(req): Json<VisionTestReq>,
) -> Response {
    let pkg = req.pkg.trim().to_string();
    if pkg.is_empty() {
        return ApiError::bad_request("pkg 非法（配置 id 不能为空）").into_response();
    }
    let plugin = req.plugin.trim().to_string();
    if plugin.is_empty() {
        return ApiError::bad_request("plugin 必填（模板所在插件 id，如 gamer.yaml）")
            .into_response();
    }
    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 255 {
        return ApiError::bad_request("模板名非法").into_response();
    }
    let media_id = req
        .media_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let device_id = req
        .device_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match (media_id, device_id) {
        (Some(_), Some(_)) => {
            return ApiError::bad_request("media_id 与 device_id 互斥（离线/在线二选一）")
                .into_response();
        }
        (None, None) => {
            return ApiError::bad_request("device_id 或 media_id 必填其一").into_response();
        }
        _ => {}
    }
    let resources = st.packages.clone();
    let (tpl_bytes, resolved_name) = match run_blocking_api(move || {
        let tpl_path = resources
            .resolve_short_path(&pkg, &plugin, &format!("templates/{name}"))
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
    // 目标帧：离线 = 媒体库精确抽帧（media_id 路径绝不触达设备）；在线 = 设备截图
    let (screen, screen_w, screen_h) = if let Some(mid) = media_id {
        let svc = crate::media::service(&st.cfg);
        let media_id = crate::media::MediaId(mid.to_string());
        let frame_req = crate::media::FrameRequest {
            pts_us: Some(req.pts_us.unwrap_or(0)),
            index: None,
            max_width: None,
        };
        let png = match run_blocking_api(move || {
            svc.extract_frame_png(&media_id, &frame_req)
                .map_err(super::media::map_media_err)
        })
        .await
        {
            Ok(png) => png,
            Err(err) => return err.into_response(),
        };
        // 抽帧 PNG 的像素空间即 oriented 展示空间（ffmpeg autorotate）
        match image::load_from_memory(&png) {
            Ok(image) => (png, image.width(), image.height()),
            Err(e) => {
                return ApiError::internal(format!("抽帧 PNG 解码失败: {e}")).into_response();
            }
        }
    } else {
        let device = device_id.expect("互斥校验保证 device_id 存在").to_string();
        let screen = match st.devices.screenshot(&device).await {
            Ok(s) => s,
            Err(e) => return ApiError::bad_gateway(format!("截图失败: {}", e)).into_response(),
        };
        let (w, h) = st
            .devices
            .session(&device)
            .map(|session| session.video_size())
            .filter(|(w, h)| *w > 0 && *h > 0)
            .unwrap_or_else(|| {
                image::load_from_memory(&screen)
                    .map(|image| image.dimensions())
                    .unwrap_or((0, 0))
            });
        (screen, w, h)
    };
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
