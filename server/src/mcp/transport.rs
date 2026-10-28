//! Independent Streamable HTTP admission, local by default. No admin session or proxy
//! header is accepted as an external MCP identity.
use axum::http::{header, HeaderMap, StatusCode};
use std::net::{IpAddr, SocketAddr};

use super::PROTOCOL_VERSION;

fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback()
        || matches!(ip,IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some_and(|ip|ip.is_loopback()))
}
fn local_host(url: &reqwest::Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host == "[::1]"
            || host.parse::<IpAddr>().is_ok_and(is_loopback)
    })
}

pub(crate) fn access(
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    allow_remote: bool,
) -> Result<&str, (StatusCode, &'static str)> {
    if !peer.is_some_and(|peer| allow_remote || is_loopback(peer.ip())) {
        return Err((
            StatusCode::FORBIDDEN,
            "MCP connections must originate from loopback",
        ));
    }
    if headers.contains_key("forwarded") || headers.contains_key("x-forwarded-for") {
        return Err((
            StatusCode::FORBIDDEN,
            "Proxied MCP connections are not supported",
        ));
    }
    if headers.get_all(header::HOST).iter().count() != 1
        || headers.get_all(header::ORIGIN).iter().count() > 1
        || headers.get_all(header::AUTHORIZATION).iter().count() != 1
        || headers.get_all("mcp-protocol-version").iter().count() > 1
    {
        return Err((StatusCode::UNAUTHORIZED, "Invalid MCP connection headers"));
    }
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if host.parse::<axum::http::uri::Authority>().is_err()
        || !reqwest::Url::parse(&format!("http://{host}")).is_ok_and(|url| {
            (allow_remote || local_host(&url))
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
        })
    {
        return Err((
            StatusCode::FORBIDDEN,
            "Invalid MCP Host or remote MCP access is disabled",
        ));
    }
    // This also rejects mismatched ports and duplicate/opaque origins. Keep
    // the normal same-origin policy shared with management input endpoints.
    if !crate::api::auth::origin_allows_headers(headers) {
        return Err((StatusCode::FORBIDDEN, "MCP Origin does not match Host"));
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let parsed = origin
            .to_str()
            .ok()
            .and_then(|origin| reqwest::Url::parse(origin).ok());
        if !parsed.is_some_and(|url| {
            (allow_remote || local_host(&url))
                && matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
        }) {
            return Err((
                StatusCode::FORBIDDEN,
                "Invalid MCP Origin or remote MCP access is disabled",
            ));
        }
    }
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some((scheme, token)) = auth.split_once(' ') else {
        return Err((StatusCode::UNAUTHORIZED, "MCP Bearer token is required"));
    };
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.len() > 4096
        || token.chars().any(char::is_whitespace)
    {
        return Err((StatusCode::UNAUTHORIZED, "Invalid MCP Bearer token"));
    }
    if !headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
    {
        return Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "MCP POST requires application/json",
        ));
    }
    let accept = headers
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|part| {
            let mut pieces = part.split(';');
            let mime = pieces.next()?.trim().to_ascii_lowercase();
            let acceptable = pieces.all(|parameter| {
                !parameter.trim().to_ascii_lowercase().starts_with("q=")
                    || parameter.trim()[2..]
                        .trim()
                        .parse::<f32>()
                        .is_ok_and(|quality| quality > 0.0)
            });
            acceptable.then_some(mime)
        })
        .collect::<Vec<_>>();
    if !accept.iter().any(|mime| mime == "application/json")
        || !accept.iter().any(|mime| mime == "text/event-stream")
    {
        return Err((
            StatusCode::NOT_ACCEPTABLE,
            "MCP Accept must include application/json and text/event-stream",
        ));
    }
    if let Some(version) = headers.get("mcp-protocol-version") {
        if version.to_str().ok() != Some(PROTOCOL_VERSION) {
            return Err((StatusCode::BAD_REQUEST, "Unsupported MCP protocol version"));
        }
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "127.0.0.1:8443".parse().unwrap());
        headers.insert(header::AUTHORIZATION, "Bearer test".parse().unwrap());
        headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        headers.insert(
            header::ACCEPT,
            "application/json, text/event-stream".parse().unwrap(),
        );
        headers
    }
    #[test]
    fn requires_real_loopback_bearer_same_origin_and_the_declared_protocol() {
        let peer = Some("127.0.0.1:1234".parse().unwrap());
        let valid = headers();
        assert_eq!(access(&valid, peer, false).unwrap(), "test");
        assert!(access(&valid, None, false).is_err());
        assert!(access(&valid, Some("192.0.2.1:1".parse().unwrap()), false).is_err());
        for (name, value) in [
            ("forwarded", "for=127.0.0.1"),
            ("origin", "https://attacker.invalid"),
            ("mcp-protocol-version", "2025-03-26"),
            ("accept", "application/json;q=0, text/event-stream"),
        ] {
            let mut changed = headers();
            changed.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
            assert!(access(&changed, peer, false).is_err(), "{name}");
        }
        let mut cookie = headers();
        cookie.remove(header::AUTHORIZATION);
        cookie.insert(header::COOKIE, "gb_sid=admin".parse().unwrap());
        cookie.insert("x-admin-token", "admin".parse().unwrap());
        assert_eq!(
            access(&cookie, peer, false).unwrap_err().0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn remote_opt_in_keeps_bearer_host_origin_and_proxy_checks() {
        for (peer, host) in [
            ("192.0.2.20:1234", "192.0.2.10:18443"),
            ("[2001:db8::20]:1234", "[2001:db8::10]:18443"),
            ("127.0.0.1:1234", "gamer.example:18443"),
        ] {
            let peer = Some(peer.parse().unwrap());
            let mut valid = headers();
            valid.insert(header::HOST, host.parse().unwrap());
            assert!(access(&valid, peer, false).is_err());
            assert_eq!(access(&valid, peer, true).unwrap(), "test");
            assert!(access(&valid, None, true).is_err());
            valid.insert(header::ORIGIN, format!("https://{host}").parse().unwrap());
            assert_eq!(access(&valid, peer, true).unwrap(), "test");
            for (name, value) in [
                ("origin", "https://attacker.invalid"),
                ("origin", "null"),
                ("origin", "https://gamer.example:9999"),
                ("origin", "https://user:pass@gamer.example:18443"),
                ("forwarded", "for=127.0.0.1"),
                ("x-forwarded-for", "127.0.0.1"),
            ] {
                let mut changed = valid.clone();
                changed.insert(
                    axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                    value.parse().unwrap(),
                );
                assert!(access(&changed, peer, true).is_err(), "{name}: {value}");
            }
            valid.remove(header::ORIGIN);
            valid.remove(header::AUTHORIZATION);
            valid.insert(header::COOKIE, "gb_session=admin".parse().unwrap());
            valid.insert("x-admin-token", "admin".parse().unwrap());
            assert_eq!(
                access(&valid, peer, true).unwrap_err().0,
                StatusCode::UNAUTHORIZED
            );
        }
        let peer = Some("192.0.2.20:1234".parse().unwrap());
        for host in [
            "",
            "user:pass@gamer.example",
            "gamer.example/path",
            "gamer.example?x=1",
            "gamer.example#x",
            "gamer.example:invalid",
        ] {
            let mut invalid = headers();
            invalid.insert(header::HOST, host.parse().unwrap());
            assert!(access(&invalid, peer, true).is_err(), "{host}");
        }
        let mut duplicate = headers();
        duplicate.append(header::HOST, "gamer.example".parse().unwrap());
        assert!(access(&duplicate, peer, true).is_err());
    }
}
