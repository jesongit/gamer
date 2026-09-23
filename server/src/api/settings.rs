use super::{ApiError, AppState};
use crate::settings::{SaveError, SystemSettings};
use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

pub(super) async fn get_settings(State(st): State<AppState>) -> Response {
    match st.cfg.settings_view() {
        Ok(view) => Json(view).into_response(),
        Err(_) => ApiError::internal("读取系统设置失败，请检查配置文件").into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SaveRequest {
    settings: SystemSettings,
    expected_revision: String,
}

pub(super) async fn save_settings(
    State(st): State<AppState>,
    Json(req): Json<SaveRequest>,
) -> Response {
    match st.cfg.save_settings(req.settings, &req.expected_revision) {
        Ok(view) => Json(view).into_response(),
        Err(SaveError::Invalid(message)) => ApiError::bad_request(message).into_response(),
        Err(SaveError::Conflict) => {
            ApiError::conflict("配置已发生变化，请重新读取后再保存").into_response()
        }
        Err(SaveError::Io(error)) => {
            tracing::warn!(%error, "saving system settings failed");
            ApiError::internal("保存设置失败，当前生效设置未改变").into_response()
        }
    }
}
