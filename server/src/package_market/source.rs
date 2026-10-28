//! Download locations, independent of the GitHub-only publishing destination.
use super::repository;
use anyhow::{ensure, Context, Result};
use reqwest::Url;
#[cfg(test)]
pub(crate) mod test_http;

#[derive(Debug, Clone)]
pub(crate) enum SourceLocation {
    Github {
        repository: String,
        prefix: Option<String>,
    },
    Catalog {
        url: Url,
    },
    Gitea {
        origin: Url,
        repository: String,
    },
}

impl SourceLocation {
    pub(crate) fn parse(input: &str) -> Result<Self> {
        Self::parse_for(input, "packages.json")
    }
    pub(crate) fn parse_for(input: &str, catalog: &str) -> Result<Self> {
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
        validate_download_url(&url)?;
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
        let path = url.path().trim_matches('/').trim_end_matches(".git");
        if path.split('/').count() == 2 {
            let repo = repository(path)?;
            let origin = url.join("/")?;
            return Ok(Self::Gitea {
                origin,
                repository: repo,
            });
        }
        // Self-hosted static catalogs may expose packages.json directly or a directory.
        let path = format!("{}/{catalog}", url.path().trim_end_matches('/'));
        url.set_path(&path);
        Ok(Self::Catalog { url })
    }

    pub(crate) fn canonical(&self) -> String {
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
            Self::Gitea { origin, repository } => format!("{origin}{repository}"),
        }
    }

    pub(crate) fn directory_catalog(&self, name: &str) -> Option<Url> {
        if let Self::Gitea { origin, repository } = self {
            return origin.join(&format!("{repository}/{name}")).ok();
        }
        None
    }
    pub(crate) fn wrap(&self, upstream: &str) -> String {
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

    pub(crate) fn released_catalog(
        &self,
        name: &str,
        beta: bool,
        fetch: impl Fn(&str, u64) -> Result<Vec<u8>>,
    ) -> Result<Option<(serde_json::Value, serde_json::Value)>> {
        let api = match self {
            Self::Gitea { origin, repository } => {
                format!("{origin}api/v1/repos/{repository}/releases?limit=100")
            }
            Self::Github { repository, .. } => self.wrap(&format!(
                "https://api.github.com/repos/{repository}/releases?per_page=100"
            )),
            _ => anyhow::bail!("不是仓库来源"),
        };
        let releases: serde_json::Value = serde_json::from_slice(&fetch(&api, 3 * 1024 * 1024)?)
            .context("仓库 Release 接口未返回 JSON，请填写公开 Gitea 仓库或目录 JSON 地址")?;
        let releases = releases.as_array().context("仓库 Release 列表无效")?;
        let Some(release) = releases
            .iter()
            .filter(|r| r["draft"] == false && (beta || r["prerelease"] == false))
            .filter(|r| {
                r["assets"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|a| a["name"] == name))
            })
            .max_by_key(|r| {
                r["published_at"]
                    .as_str()
                    .or_else(|| r["created_at"].as_str())
                    .unwrap_or_default()
            })
        else {
            return Ok(None);
        };
        super::validate_tag(release["tag_name"].as_str().context("Release 缺少标签")?)?;
        let url = self.release_asset(release, name, None)?;
        let doc = serde_json::from_slice(&fetch(&url, 3 * 1024 * 1024)?)
            .with_context(|| format!("Release 中的 {name} 不是有效 JSON"))?;
        Ok(Some((release.clone(), doc)))
    }

    pub(crate) fn release_asset(
        &self,
        release: &serde_json::Value,
        name: &str,
        size: Option<u64>,
    ) -> Result<String> {
        let assets: Vec<_> = release["assets"]
            .as_array()
            .context("Release 缺少附件")?
            .iter()
            .filter(|a| a["name"] == name)
            .collect();
        ensure!(assets.len() == 1, "Release 缺少唯一附件：{name}");
        let asset = assets[0];
        ensure!(
            size.is_none_or(|size| asset["size"].as_u64() == Some(size)),
            "Release 附件大小不符：{name}"
        );
        let raw = asset["browser_download_url"]
            .as_str()
            .context("附件缺少下载地址")?;
        let url = match self {
            Self::Gitea { origin, .. } => {
                let url = origin.join(raw)?;
                ensure!(
                    url.origin() == origin.origin(),
                    "内网附件必须来自所选仓库服务器"
                );
                url
            }
            Self::Github { repository, .. } => {
                let tag = release["tag_name"].as_str().context("Release 缺少标签")?;
                let expected =
                    format!("https://github.com/{repository}/releases/download/{tag}/{name}");
                ensure!(
                    raw == expected || raw == self.wrap(&expected),
                    "附件不属于所选 GitHub 仓库发布"
                );
                Url::parse(&self.wrap(&expected))?
            }
            _ => anyhow::bail!("不是仓库来源"),
        };
        validate_download_url(&url)?;
        Ok(url.into())
    }
}

pub(crate) fn validate_download_url(url: &Url) -> Result<()> {
    ensure!(
        url.username().is_empty() && url.password().is_none() && url.fragment().is_none(),
        "来源 URL 不能包含账号、密码或片段"
    );
    let private = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| match ip {
                    std::net::IpAddr::V4(ip) => ip.is_private() || ip.is_loopback(),
                    std::net::IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local(),
                })
    });
    ensure!(
        url.host_str().is_some()
            && (url.scheme() == "https" || (url.scheme() == "http" && private)),
        "来源须使用 HTTPS；内网或回环 IP 可使用 HTTP"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_repository_presets_accept_git_suffix_and_keep_catalog_types_distinct() {
        for input in [
            "http://192.168.31.10:13000/posase/gamer-plugins.git",
            "http://192.168.31.10:13000/posase/gamer-plugins/",
        ] {
            assert_eq!(
                SourceLocation::parse_for(input, "registry.json")
                    .unwrap()
                    .canonical(),
                "http://192.168.31.10:13000/posase/gamer-plugins"
            );
        }
        assert_eq!(
            SourceLocation::parse_for("https://mirror.example/plugins/", "registry.json")
                .unwrap()
                .canonical(),
            "https://mirror.example/plugins/registry.json"
        );
        for input in [
            "http://8.8.8.8/owner/repo",
            "http://public.example/owner/repo",
            "http://192.168.31.10:13000/a/b?token=secret",
            "http://u:p@192.168.31.10/a/b",
        ] {
            assert!(SourceLocation::parse(input).is_err(), "{input}");
        }
    }

    #[test]
    fn gitea_catalog_pins_same_server_attachments_and_rejects_missing_or_foreign_assets() {
        let source = SourceLocation::parse("http://127.0.0.1:13000/owner/repo").unwrap();
        let release = serde_json::json!({"id":1,"tag_name":"v1.0.0","draft":false,"prerelease":false,"published_at":"2026-10-28T00:00:00Z","assets":[
            {"name":"registry.json","browser_download_url":"http://127.0.0.1:13000/attachments/catalog","size":28},
            {"name":"plugin-1.0.0.gplugin","browser_download_url":"http://127.0.0.1:13000/attachments/plugin","size":3}
        ]});
        let (found, doc) = source
            .released_catalog("registry.json", false, |url, _| {
                if url.ends_with("/releases?limit=100") {
                    Ok(serde_json::to_vec(&serde_json::json!([release.clone()])).unwrap())
                } else {
                    assert!(url.ends_with("/attachments/catalog"));
                    Ok(br#"{"schema_version":2,"plugins":[]}"#.to_vec())
                }
            })
            .unwrap()
            .unwrap();
        assert_eq!(doc["schema_version"], 2);
        assert!(source
            .release_asset(&found, "plugin-1.0.0.gplugin", Some(3))
            .unwrap()
            .ends_with("/attachments/plugin"));
        assert!(source
            .release_asset(&found, "plugin-1.0.0.gplugin", Some(4))
            .is_err());
        let mut foreign = found.clone();
        foreign["assets"][1]["browser_download_url"] = "https://other.example/plugin".into();
        assert!(source
            .release_asset(&foreign, "plugin-1.0.0.gplugin", Some(3))
            .is_err());
        assert!(source
            .released_catalog("registry.json", false, |_, _| Ok(b"[]".to_vec()))
            .unwrap()
            .is_none());
    }

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
