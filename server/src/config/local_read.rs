//! Explicit Docker NAT exception for read-only plugin sources, never admin/MCP.
use serde::{Deserialize, Serialize};
use std::{net::IpAddr, num::NonZeroU16};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LocalReadConfig {
    /// Host loopback port published by Docker; absent means the HTTP listener port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_port: Option<NonZeroU16>,
    /// Exact observed bridge gateway, opt-in only with a loopback-bound publication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docker_gateway: Option<IpAddr>,
}

impl LocalReadConfig {
    pub fn validate(&self) -> Option<String> {
        let gateway = self.docker_gateway?;
        if self.published_port.is_none() {
            return Some("local_read.docker_gateway 必须同时设置 published_port，并仅向宿主机回环地址发布端口".into());
        }
        let private = match gateway {
            IpAddr::V4(ip) => ip.is_private(),
            IpAddr::V6(ip) => ip.is_unique_local(),
        };
        (!private).then(|| "local_read.docker_gateway 必须为明确的 Docker 私有网关 IP，不能使用公网、回环或通配地址".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_read_docker_exception_requires_explicit_valid_pair() {
        assert!(LocalReadConfig::default().validate().is_none());
        let mut config = LocalReadConfig {
            published_port: NonZeroU16::new(18443),
            docker_gateway: Some("172.17.0.1".parse().unwrap()),
        };
        assert!(config.validate().is_none());
        config.published_port = None;
        assert!(config.validate().is_some());
        config.published_port = NonZeroU16::new(18443);
        for ip in ["0.0.0.0", "127.0.0.1", "192.0.2.1", "::", "::1", "ff02::1"] {
            config.docker_gateway = Some(ip.parse().unwrap());
            assert!(config.validate().is_some(), "{ip}");
        }
        for text in [
            "published_port = 0",
            "published_port = 65536",
            "docker_gateway = '172.17.0.0/16'",
            "docker_gateway = 'localhost'",
            "trusted_proxies = ['172.17.0.1']",
        ] {
            assert!(toml::from_str::<LocalReadConfig>(text).is_err(), "{text}");
        }
    }
}
