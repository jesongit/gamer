//! Same-listener read transport. Paths and read credentials stay plugin-owned.
use axum::{
    extract::{rejection::PathRejection, ConnectInfo, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use std::net::{IpAddr, SocketAddr};

use super::AppState;
use crate::extensions::{ExtensionError, ExtensionId};

pub(super) async fn get_resource(
    State(state): State<AppState>,
    path: Result<Path<(String, String)>, PathRejection>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
) -> Response {
    if !transport_access(&headers, peer.map(|peer| peer.0), &state.cfg.local_read) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(Path((raw_id, path))) = path else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(id) = ExtensionId::parse(&raw_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match state
        .extensions
        .call_builtin_local_http(&id, &path, headers)
        .await
    {
        Ok(response) => response,
        Err(error) => match error {
            ExtensionError::NotInstalled { .. } | ExtensionError::VersionNotInstalled { .. } => {
                StatusCode::NOT_FOUND.into_response()
            }
            ExtensionError::Permission(_) => StatusCode::FORBIDDEN.into_response(),
            // Lifecycle and internal failures never expose filesystem paths,
            // plugin diagnostics or read credentials on the public surface.
            _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback()
        || matches!(ip, IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback()))
}

fn loopback_url(url: &reqwest::Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<IpAddr>()
                .is_ok_and(is_loopback)
    }) && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
}

fn transport_access(
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    config: &crate::config::LocalReadConfig,
) -> bool {
    let Some(peer) = peer else {
        return false;
    };
    let direct = is_loopback(peer.ip());
    // This opt-in applies only to this read route. Docker must publish the
    // configured host port on 127.0.0.1, and its exact NAT peer must be pinned.
    // Forwarding headers never establish identity; admin and MCP stay unchanged.
    let docker = config.validate().is_none()
        && config.docker_gateway.is_some_and(|ip| ip == peer.ip())
        && config.published_port.is_some();
    if (!direct && !docker)
        || headers.contains_key("forwarded")
        || headers.contains_key("x-forwarded-for")
        || headers.get_all(header::HOST).iter().count() != 1
        || headers.get_all(header::ORIGIN).iter().count() > 1
    {
        return false;
    }
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    if !reqwest::Url::parse(&format!("http://{host}")).is_ok_and(|url| {
        loopback_url(&url)
            && (direct
                || url.port_or_known_default() == config.published_port.map(|port| port.get()))
    }) || !super::auth::origin_allows_headers(headers)
    {
        return false;
    }
    headers.get(header::ORIGIN).is_none_or(|origin| {
        origin
            .to_str()
            .ok()
            .and_then(|origin| reqwest::Url::parse(origin).ok())
            .is_some_and(|url| matches!(url.scheme(), "http" | "https") && loopback_url(&url))
    })
}

#[cfg(test)]
#[path = "extension_local_tests.rs"]
mod tests;
