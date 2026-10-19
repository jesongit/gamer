//! Authenticated management only. Installation/configuration is not an MCP tool.
use super::{ApiError, AppState};
use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;
use serde_json::Value;

fn error(e: anyhow::Error) -> ApiError {
    ApiError::bad_request(e.to_string())
}
pub(super) async fn list(State(st): State<AppState>) -> Result<Json<Value>, ApiError> {
    crate::runtime_deps::view(&st.cfg)
        .await
        .map(Json)
        .map_err(error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    path: String,
    expected_revision: String,
}
pub(super) async fn save(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Selection>,
) -> Result<Json<Value>, ApiError> {
    crate::runtime_deps::save(&st.cfg, &id, &body.path, &body.expected_revision)
        .await
        .map_err(error)?;
    list(State(st)).await
}
pub(super) async fn prepare(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    crate::runtime_deps::prepare(&st.cfg, &id)
        .await
        .map_err(error)?;
    list(State(st)).await
}
