use super::{ApiError, AppState};
use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

async fn json_call(f: impl FnOnce() -> Result<serde_json::Value> + Send + 'static) -> Response {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(e)) => ApiError::bad_request(e.to_string()).into_response(),
        Err(e) => ApiError::internal(e.to_string()).into_response(),
    }
}
pub(super) async fn list(State(st): State<AppState>) -> Response {
    json_call(move || Ok(serde_json::to_value(st.package_market.sources()?)?)).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SaveSource {
    repository: String,
    enabled: bool,
}
pub(super) async fn save(State(st): State<AppState>, Json(input): Json<SaveSource>) -> Response {
    json_call(move || {
        Ok(serde_json::to_value(
            st.package_market
                .save_source(&input.repository, input.enabled)?,
        )?)
    })
    .await
}
pub(super) async fn remove(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    json_call(move || {
        st.package_market.remove(&id)?;
        Ok(serde_json::json!({"ok":true}))
    })
    .await
}
#[derive(Default, Deserialize)]
pub(super) struct CatalogQuery {
    #[serde(default)]
    refresh: bool,
}
pub(super) async fn catalog(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<CatalogQuery>,
) -> Response {
    json_call(move || st.package_market.catalog(&id, query.refresh)).await
}
#[derive(Deserialize)]
pub(super) struct ArchiveQuery {
    sha256: String,
}
pub(super) async fn archive(
    State(st): State<AppState>,
    Path((source, id, version)): Path<(String, String, String)>,
    Query(query): Query<ArchiveQuery>,
) -> Response {
    match tokio::task::spawn_blocking(move || {
        st.package_market
            .archive(&source, &id, &version, &query.sha256)
    })
    .await
    {
        Ok(Ok(bytes)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/zip"),
                (header::CACHE_CONTROL, "private, no-store"),
            ],
            bytes,
        )
            .into_response(),
        Ok(Err(e)) => ApiError::bad_gateway(e.to_string()).into_response(),
        Err(e) => ApiError::internal(e.to_string()).into_response(),
    }
}
