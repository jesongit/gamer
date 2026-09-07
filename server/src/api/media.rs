//! 媒体 REST（视频工作台 V1）。实施合同：`docs/plans/gamer_video_workbench_contracts.md` §1。
//!
//! 路由与处理器由媒体服务实施者填充；本文件只承载媒体路由组，
//! 认证与 body 限额组装配在 [`super::build_router_with_extensions`]。
//! 所有响应的素材寻址一律用 media id，不接受/返回服务端绝对路径。

use axum::Router;

use super::AppState;

/// 媒体路由组（挂进受保护组；导入大字节限额由组层统一设定）。
/// 合同端点：
/// - `POST /api/media/import?name=<filename>`（raw bytes → 201 metadata）
/// - `GET  /api/media`（列表）/ `GET|DELETE /api/media/:id`
/// - `GET  /api/media/:id/file`（Range 播放流）
/// - `GET  /api/media/:id/frame?pts_us=|index=&max_width=`（PNG 确定帧）
/// - `POST /api/media/:id/refs`（引用登记/解除；V1 可 501）
pub(super) fn router() -> Router<AppState> {
    Router::new()
}
