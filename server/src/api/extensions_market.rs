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
    source_id: Option<String>,
}

pub(super) async fn registry(
    State(st): State<AppState>,
    Query(query): Query<MarketQuery>,
) -> Response {
    match tokio::task::spawn_blocking(move || {
        st.plugin_market
            .registry_for(query.source_id.as_deref(), query.refresh)
    })
    .await
    {
        Ok(Ok(document)) => Json(document).into_response(),
        Ok(Err(error)) => ApiError::service_unavailable(error).into_response(),
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

pub(super) async fn archive(
    State(st): State<AppState>,
    Path((id, version)): Path<(String, String)>,
    Query(query): Query<MarketQuery>,
) -> Response {
    match tokio::task::spawn_blocking(move || {
        st.plugin_market
            .archive_for(query.source_id.as_deref(), &id, &version)
    })
    .await
    {
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

pub(super) async fn sources(State(st): State<AppState>) -> Response {
    match st.plugin_market.sources() {
        Ok(sources) => Json(sources).into_response(),
        Err(e) => ApiError::bad_request(e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SaveSource {
    repository: String,
    enabled: bool,
    name: Option<String>,
    id: Option<String>,
}
pub(super) async fn save_source(
    State(st): State<AppState>,
    Json(input): Json<SaveSource>,
) -> Response {
    match st.plugin_market.save_source(
        &input.repository,
        input.enabled,
        input.name.as_deref(),
        input.id.as_deref(),
    ) {
        Ok(sources) => Json(sources).into_response(),
        Err(e) => ApiError::bad_request(e.to_string()).into_response(),
    }
}
pub(super) async fn remove_source(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match st.plugin_market.remove_source(&id) {
        Ok(()) => Json(serde_json::json!({"ok":true})).into_response(),
        Err(e) => ApiError::bad_request(e.to_string()).into_response(),
    }
}
