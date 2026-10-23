//! Download locations, independent of the GitHub-only publishing destination.
use super::repository;
use anyhow::{ensure, Context, Result};
use reqwest::Url;

#[derive(Debug, Clone)]
pub(super) enum SourceLocation {
    Github {
        repository: String,
        prefix: Option<String>,
    },
    Catalog {
        url: Url,
    },
}

impl SourceLocation {
    pub(super) fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        ensure!(
            !input.is_empty() && input.len() <= 2048,
            "配置源地址为空或过长"
        );
        if !input.contains("://") {
            return Ok(Self::Github {
                repository: repository(input)?,
                prefix: None,
            });
        }
        let mut url = Url::parse(input).context("配置源 URL 无效")?;
        ensure!(
            url.scheme() == "https" && url.host_str().is_some(),
            "远端配置源必须使用 HTTPS"
        );
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "公开配置源不能包含账号、密码、查询参数或片段"
        );
        if url.path().ends_with(".json") {
            return Ok(Self::Catalog { url });
        }
        if url.host_str() == Some("github.com") && url.port().is_none() {
            return Ok(Self::Github {
                repository: repository(url.as_str())?,
                prefix: None,
            });
        }
        // Service-independent prefix proxies: no mirror hostname allowlist.
        if let Some((prefix, path)) = url.as_str().rsplit_once("/https://github.com/") {
            return Ok(Self::Github {
                repository: repository(path)?,
                prefix: Some(format!("{prefix}/")),
            });
        }
        // Self-hosted static catalogs may expose packages.json directly or a directory.
        let path = format!("{}/packages.json", url.path().trim_end_matches('/'));
        url.set_path(&path);
        Ok(Self::Catalog { url })
    }

    pub(super) fn canonical(&self) -> String {
        match self {
            Self::Github {
                repository,
                prefix: None,
            } => repository.clone(),
            Self::Github {
                repository,
                prefix: Some(prefix),
            } => format!("{prefix}https://github.com/{repository}"),
            Self::Catalog { url } => url.to_string(),
        }
    }

    pub(super) fn wrap(&self, upstream: &str) -> String {
        match self {
            Self::Github {
                prefix: Some(prefix),
                ..
            } => format!("{prefix}{upstream}"),
            _ => upstream.into(),
        }
    }

    pub(super) fn release_base(&self, repository: &str, tag: &str) -> String {
        self.wrap(&format!(
            "https://github.com/{repository}/releases/download/{tag}/"
        ))
    }

    pub(super) fn latest_catalog(&self, repository: &str) -> String {
        self.wrap(&format!(
            "https://github.com/{repository}/releases/latest/download/packages.json"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_hosts_and_path_prefixes_are_not_hardcoded() {
        for prefix in [
            "https://gh-proxy.com/",
            "https://another-mirror.example/",
            "https://mirror.example/cache/",
        ] {
            let input = format!("{prefix}https://github.com/Owner/Repo.git/");
            let source = SourceLocation::parse(&input).unwrap();
            assert_eq!(
                source.canonical(),
                format!("{prefix}https://github.com/owner/repo")
            );
            assert_eq!(
                source.wrap("https://api.github.com/repos/owner/repo/releases/latest"),
                format!("{prefix}https://api.github.com/repos/owner/repo/releases/latest")
            );
        }
    }

    #[test]
    fn direct_catalogs_and_github_sources_keep_distinct_identities() {
        for input in [
            "Owner/Repo",
            "git@github.com:Owner/Repo.git",
            "https://github.com/Owner/Repo",
        ] {
            assert_eq!(
                SourceLocation::parse(input).unwrap().canonical(),
                "owner/repo"
            );
        }
        assert_eq!(
            SourceLocation::parse("https://mirror.example/configs")
                .unwrap()
                .canonical(),
            "https://mirror.example/configs/packages.json"
        );
        assert_eq!(
            SourceLocation::parse("https://mirror.example/configs/packages.json")
                .unwrap()
                .canonical(),
            "https://mirror.example/configs/packages.json"
        );
        assert_eq!(
            SourceLocation::parse("https://mirror.example/catalog.json")
                .unwrap()
                .canonical(),
            "https://mirror.example/catalog.json"
        );
        let mirrored_catalog = "https://proxy.example/https://github.com/owner/repo/releases/latest/download/packages.json";
        assert_eq!(
            SourceLocation::parse(mirrored_catalog).unwrap().canonical(),
            mirrored_catalog
        );
    }

    #[test]
    fn credentials_flags_and_ambiguous_urls_are_rejected() {
        for input in [
            "http://mirror.example/p/",
            "file:///tmp/packages.json",
            "https://u:p@mirror.example/a",
            "https://mirror.example/a?token=secret",
            "https://mirror.example/a#fragment",
            "https://mirror.example/https://github.com/o/r/extra",
            "https://mirror.example/https://github.com/--help/r",
        ] {
            assert!(SourceLocation::parse(input).is_err(), "{input}");
        }
    }
}
