use super::{ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

#[derive(Default, Deserialize)]
pub(super) struct MarketQuery {
    #[serde(default)]
    refresh: bool,
}

pub(super) async fn registry(
    State(st): State<AppState>,
    Query(query): Query<MarketQuery>,
) -> Response {
    match tokio::task::spawn_blocking(move || st.plugin_market.registry(query.refresh)).await {
        Ok(Ok(document)) => Json(document).into_response(),
        Ok(Err(error)) => ApiError::service_unavailable(error).into_response(),
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

pub(super) async fn archive(
    State(st): State<AppState>,
    Path((id, version)): Path<(String, String)>,
) -> Response {
    match tokio::task::spawn_blocking(move || st.plugin_market.archive(&id, &version)).await {
        Ok(Ok(bytes)) => (
            [
                (header::CONTENT_TYPE, "application/zip"),
                (header::CACHE_CONTROL, "private, no-store"),
            ],
            bytes,
        )
            .into_response(),
        Ok(Err(error)) => ApiError::bad_gateway(error).into_response(),
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}
