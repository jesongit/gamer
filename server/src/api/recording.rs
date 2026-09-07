//! 录制 REST（视频工作台 V1）。实施合同：`docs/plans/gamer_video_workbench_contracts.md` §2。
//!
//! 路由与处理器由录制服务实施者填充；认证与限额组装配在
//! [`super::build_router_with_extensions`]。录制是服务端权威：
//! 浏览器断开不中断录制，重复 stop/cancel 幂等。

use axum::Router;

use super::AppState;

/// 录制路由组（挂进受保护组）。
/// 合同端点：
/// - `POST /api/recording/start`（{device_id} → 202 session）
/// - `POST /api/recording/:id/stop | /cancel`（→ session 终态；幂等）
/// - `GET  /api/recording/:id`（状态）
/// - `GET  /api/recording/active?device_id=`（设备活动会话或 404）
/// - `GET  /api/recording/:id/events`（{schema_version, events:[...]}）
pub(super) fn router() -> Router<AppState> {
    Router::new()
}
