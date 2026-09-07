//! Vision 能力位端点（P11.6）：模板「匹配测试」是 vision 能力语义（Core 合法），
//! 自旧 `/api/templates/:name/test` 迁入 `POST /api/capabilities/vision/test`。
//!
//! 寻址 = (pkg, plugin, name) 三元组显式给出：Core 不预设/不猜测任何业务插件
//! id，缺 `plugin` 即 400。支持模板短名（`#` 后缀唯一候选消歧），区域/颜色由
//! 消歧后的实际文件名 `#` 后缀决定；NCC 匹配走专用计算池（PERF-003）。
//!
//! 离线路径（视频工作台 V1，实施合同 §1.1；Phase 5 收口）：请求给 `media_id`
//! （与设备路径互斥）时目标帧来自媒体库精确抽帧（`crate::media`），该路径
//! **不要求设备存在、绝不触达 ADB/设备截图**（帧提取函数签名只依赖
//! `Config`，类型层面排除设备依赖）。帧寻址 = `pts_us`（缺省 0 = 首帧）或
//! `frame_index`（展示序帧索引，Phase 5 真实展示帧映射）；响应携带帧身份
//! （media_id/帧索引/该帧真实 PTS）。抽帧 PNG 经 ffmpeg autorotate，命中
//! 坐标即 oriented 展示空间。

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
    /// 离线路径：目标帧 pts（微秒）；缺省 0 = 首帧（与 frame_index 互斥）。
    pts_us: Option<u64>,
    /// 离线路径：展示序帧索引（Phase 5；与 pts_us 互斥）。
    frame_index: Option<u32>,
}

/// 目标帧解析产物（互斥裁决的唯一实现，handler 与测试共用）。
pub(super) enum VisionTarget {
    /// 在线：设备截图路径。
    Device(String),
    /// 离线：媒体库确定帧 + 帧请求。
    Offline {
        media_id: crate::media::MediaId,
        frame: crate::media::FrameRequest,
    },
}

/// 目标解析（互斥语义集中在此，纯函数可测）：device/media 二选一；
/// 离线帧寻址 pts_us 与 frame_index 二选一（pts_us 缺省 0 = 首帧）。
pub(super) fn resolve_vision_target(
    media_id: Option<&str>,
    device_id: Option<&str>,
    pts_us: Option<u64>,
    frame_index: Option<u32>,
) -> Result<VisionTarget, ApiError> {
    match (media_id, device_id) {
        (Some(_), Some(_)) => {
            return Err(ApiError::bad_request(
                "media_id 与 device_id 互斥（离线/在线二选一）",
            ));
        }
        (None, None) => {
            return Err(ApiError::bad_request("device_id 或 media_id 必填其一"));
        }
        _ => {}
    }
    if let Some(media_id) = media_id {
        if pts_us.is_some() && frame_index.is_some() {
            return Err(ApiError::bad_request(
                "pts_us 与 frame_index 互斥（离线帧寻址二选一）",
            ));
        }
        let frame = crate::media::FrameRequest {
            pts_us: if frame_index.is_some() {
                None
            } else {
                Some(pts_us.unwrap_or(0))
            },
            index: frame_index,
            max_width: None,
        };
        return Ok(VisionTarget::Offline {
            media_id: crate::media::MediaId(media_id.to_string()),
            frame,
        });
    }
    Ok(VisionTarget::Device(
        device_id.expect("互斥校验保证 device_id 存在").to_string(),
    ))
}

/// 离线目标帧提取：签名只依赖 `MediaService`——**没有 DeviceManager 入口**，
/// 类型层面保证离线路径绝不触达设备/ADB。返回 PNG 与帧身份描述符。
/// （服务单例的装配留在 handler：`crate::media::service(&Config)`。）
fn extract_offline_frame(
    media: &crate::media::MediaService,
    media_id: crate::media::MediaId,
    frame: crate::media::FrameRequest,
) -> anyhow::Result<(Vec<u8>, crate::media::FrameDescriptor)> {
    let extraction = media.extract_frame(&media_id, &frame)?;
    Ok((extraction.png, extraction.descriptor))
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
    let target = match resolve_vision_target(media_id, device_id, req.pts_us, req.frame_index) {
        Ok(target) => target,
        Err(err) => return err.into_response(),
    };
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
    let (screen, screen_w, screen_h, frame_identity) = match target {
        VisionTarget::Offline { media_id, frame } => {
            let cfg = st.cfg.clone();
            let (png, descriptor) = match run_blocking_api(move || {
                let svc = crate::media::service(&cfg);
                extract_offline_frame(&svc, media_id, frame).map_err(super::media::map_media_err)
            })
            .await
            {
                Ok(v) => v,
                Err(err) => return err.into_response(),
            };
            // 抽帧 PNG 的像素空间即 oriented 展示空间（ffmpeg autorotate）
            match image::load_from_memory(&png) {
                Ok(image) => (
                    png,
                    image.width(),
                    image.height(),
                    Some((descriptor.media_id.0, descriptor.index, descriptor.pts_us)),
                ),
                Err(e) => {
                    return ApiError::internal(format!("抽帧 PNG 解码失败: {e}")).into_response();
                }
            }
        }
        VisionTarget::Device(device_id) => {
            let screen = match st.devices.screenshot(&device_id).await {
                Ok(s) => s,
                Err(e) => return ApiError::bad_gateway(format!("截图失败: {}", e)).into_response(),
            };
            let (w, h) = st
                .devices
                .session(&device_id)
                .map(|session| session.video_size())
                .filter(|(w, h)| *w > 0 && *h > 0)
                .unwrap_or_else(|| {
                    image::load_from_memory(&screen)
                        .map(|image| image.dimensions())
                        .unwrap_or((0, 0))
                });
            (screen, w, h, None)
        }
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
    let outcome = matcher::compute::run(move || {
        matcher::match_template(&mr).map_err(|e| ApiError::internal(e.to_string()))
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))
    .and_then(|inner| inner);
    // 帧身份随附（离线路径；在线路径为 null）——调用方拿到的是「解析后的帧」
    let identity = match frame_identity {
        Some((media_id, index, pts_us)) => serde_json::json!({
            "media_id": media_id,
            "frame_index": index,
            "pts_us": pts_us,
        }),
        None => serde_json::Value::Null,
    };
    match outcome {
        Ok(Some(m)) => Json(serde_json::json!({"hit": true, "x": m.x, "y": m.y, "width": m.width, "height": m.height, "score": m.score, "frame": identity})).into_response(),
        Ok(None) => Json(serde_json::json!({"hit": false, "region": miss_region, "frame": identity})).into_response(),
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{FrameRequest, MediaError, MediaErrorKind, MediaId, MediaService};
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    fn target(
        media: Option<&str>,
        device: Option<&str>,
        pts: Option<u64>,
        index: Option<u32>,
    ) -> Result<VisionTarget, ApiError> {
        resolve_vision_target(media, device, pts, index)
    }

    fn bad_request_on(result: Result<VisionTarget, ApiError>) {
        let err = result.err().expect("必须被拒绝");
        let status = err.clone().into_response().status();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// 互斥语义（离线/在线、pts/index）集中裁决：
    /// 离线缺省 pts=0、frame_index 走索引寻址、非法组合 400。
    #[test]
    fn vision_target_resolution_enforces_mutex_semantics() {
        // 双填 / 双空 → 400
        bad_request_on(target(Some("m1"), Some("d1"), None, None));
        bad_request_on(target(None, None, None, None));
        // pts 与 frame_index 双填 → 400
        bad_request_on(target(Some("m1"), None, Some(5), Some(2)));
        // 离线缺省 = 首帧
        match target(Some("m1"), None, None, None).unwrap() {
            VisionTarget::Offline { media_id, frame } => {
                assert_eq!(media_id, MediaId("m1".into()));
                assert_eq!(frame.pts_us, Some(0));
                assert_eq!(frame.index, None);
            }
            VisionTarget::Device(_) => panic!("必须是离线目标"),
        }
        // pts 寻址
        match target(Some("m1"), None, Some(1500), None).unwrap() {
            VisionTarget::Offline { frame, .. } => assert_eq!(frame.pts_us, Some(1500)),
            _ => panic!("必须是离线目标"),
        }
        // index 寻址：pts 缺省不注入（索引优先）
        match target(Some("m1"), None, None, Some(7)).unwrap() {
            VisionTarget::Offline { frame, .. } => {
                assert_eq!(frame.index, Some(7));
                assert_eq!(frame.pts_us, None);
            }
            _ => panic!("必须是离线目标"),
        }
        // 在线
        assert!(matches!(
            target(None, Some("d1"), None, None).unwrap(),
            VisionTarget::Device(_)
        ));
    }

    /// 离线路径不触设备（回归锁）：帧提取只依赖 MediaService——本测试从头到
    /// 尾不构造任何 DeviceManager/adb 入口，离线帧身份校验（越界 = 结构化
    /// FrameNotFound）与提取照常工作。
    #[test]
    fn offline_frame_extraction_needs_no_device_manager() {
        let src = tempfile::tempdir().unwrap();
        let clip = src.path().join("clip.mp4");
        let gen = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=1:size=64x64:rate=30",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&clip)
            .output()
            .expect("生成测试视频失败（本机需 ffmpeg）");
        if !gen.status.success() {
            eprintln!("跳过：本机无 ffmpeg");
            return;
        }
        let bytes = std::fs::read(&clip).unwrap();
        let root = tempfile::tempdir().unwrap();
        let media = MediaService::open(root.path().to_path_buf(), "ffmpeg".into()).unwrap();
        let meta = media.import_bytes("clip.mp4", &bytes).unwrap();

        // pts 寻址：返回帧身份
        let (png, descriptor) = extract_offline_frame(
            &media,
            meta.id.clone(),
            FrameRequest {
                pts_us: Some(0),
                index: None,
                max_width: None,
            },
        )
        .unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(descriptor.media_id, meta.id);
        assert_eq!(descriptor.index, 0);
        assert_eq!(descriptor.pts_us, 0);
        // index 寻址
        let (_, by_index) = extract_offline_frame(
            &media,
            meta.id.clone(),
            FrameRequest {
                pts_us: None,
                index: Some(3),
                max_width: None,
            },
        )
        .unwrap();
        assert_eq!(by_index.index, 3);
        // 帧身份越界 → 结构化错误（不触设备、不空跑解码）
        let err = extract_offline_frame(
            &media,
            meta.id.clone(),
            FrameRequest {
                pts_us: None,
                index: Some(9999),
                max_width: None,
            },
        )
        .unwrap_err();
        assert_eq!(
            err.downcast_ref::<MediaError>().unwrap().kind(),
            MediaErrorKind::FrameNotFound
        );
        // 素材缺失 → media_not_found（同样是离线结构化语义）
        let err = extract_offline_frame(
            &media,
            MediaId("ghost".into()),
            FrameRequest {
                pts_us: Some(0),
                index: None,
                max_width: None,
            },
        )
        .unwrap_err();
        assert_eq!(
            err.downcast_ref::<MediaError>().unwrap().kind(),
            MediaErrorKind::NotFound
        );
    }
}
