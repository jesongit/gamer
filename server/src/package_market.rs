//! Public package catalogs from GitHub, prefix mirrors or HTTPS static directories.
//! No credentials, plugin lifecycle or DSL knowledge.
pub(crate) mod source;
use crate::{
    package_archive::{sha256_hex, validate_and_read_manifest},
    resources::{parse_package_toml, validate_scope_id},
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use source::SourceLocation;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

pub(crate) const MAX_JSON: u64 = 2 * 1024 * 1024;
pub(crate) const MAX_ARCHIVE: u64 = 100 * 1024 * 1024;
pub(crate) const MAX_CATALOG_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const MAX_PACKAGES: usize = 128;
pub(crate) use crate::market_sources::Source;

/// Strictly two GitHub path segments; never interpret caller input as a URL/CLI flag.
pub(crate) fn repository(input: &str) -> Result<String> {
    let input = input
        .trim()
        .strip_prefix("https://github.com/")
        .or_else(|| input.trim().strip_prefix("git@github.com:"))
        .unwrap_or(input.trim());
    let input = input
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(input.trim_end_matches('/'));
    let parts: Vec<_> = input.split('/').collect();
    ensure!(
        parts.len() == 2,
        "请输入 GitHub 仓库 URL（https://github.com/owner/repo）"
    );
    for p in &parts {
        ensure!(
            !p.is_empty()
                && p.len() <= 100
                && p.as_bytes()[0].is_ascii_alphanumeric()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                && *p != "."
                && *p != "..",
            "仓库地址无效"
        );
    }
    Ok(input.to_ascii_lowercase())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PackageEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub android_targets: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub web_url_prefixes: Vec<String>,
    pub required_plugins: Vec<String>,
    pub optional_plugins: Vec<String>,
    pub asset_name: String,
    pub size: u64,
    pub sha256: String,
}
impl PackageEntry {
    pub(crate) fn from_archive(bytes: &[u8]) -> Result<Self> {
        let manifest = parse_package_toml(&validate_and_read_manifest(bytes)?)?;
        let entry = Self {
            asset_name: format!("{}-{}.gamerpkg", manifest.id, manifest.version),
            name: manifest.name.unwrap_or_else(|| manifest.id.clone()),
            id: manifest.id,
            version: manifest.version,
            author: manifest.author.unwrap_or_default(),
            android_targets: manifest.android_targets,
            web_url_prefixes: manifest.web_url_prefixes,
            required_plugins: manifest
                .plugins
                .iter()
                .filter(|(_, p)| p.required)
                .map(|(id, _)| id.clone())
                .collect(),
            optional_plugins: manifest
                .plugins
                .iter()
                .filter(|(_, p)| !p.required)
                .map(|(id, _)| id.clone())
                .collect(),
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
        };
        entry.validate()?;
        Ok(entry)
    }
    fn validate(&self) -> Result<()> {
        validate_scope_id("package", &self.id)?;
        let v = semver::Version::parse(&self.version)
            .context("发布的配置包版本必须是 SemVer（如 1.0.0）")?;
        ensure!(
            v.pre.is_empty() && v.build.is_empty(),
            "正式配置目录仅接受不含预发布或构建标记的版本"
        );
        ensure!(
            self.asset_name == format!("{}-{}.gamerpkg", self.id, self.version),
            "配置包文件名与 ID/版本不一致"
        );
        ensure!(
            self.size > 0 && self.size <= MAX_ARCHIVE,
            "配置包大小无效或超过 100 MiB"
        );
        ensure!(
            self.sha256.len() == 64
                && self
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "配置包 SHA256 无效"
        );
        ensure!(
            self.name.len() <= 1024
                && self.author.len() <= 1024
                && self.android_targets.len() <= 128
                && self.web_url_prefixes.len() <= 128,
            "配置包元数据超限"
        );
        for prefix in &self.web_url_prefixes {
            crate::resources::validate_web_prefix(prefix)?;
        }
        for id in self.required_plugins.iter().chain(&self.optional_plugins) {
            validate_scope_id("plugin", id)?;
        }
        Ok(())
    }
    pub(crate) fn verify(&self, bytes: &[u8]) -> Result<()> {
        ensure!(
            self.size == bytes.len() as u64 && self.sha256 == sha256_hex(bytes),
            "配置包大小或 SHA256 不符"
        );
        ensure!(
            *self == Self::from_archive(bytes)?,
            "配置目录与包内元数据不一致"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Catalog {
    pub schema_version: u32,
    pub packages: Vec<PackageEntry>,
}
impl Catalog {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1 && self.packages.len() <= MAX_PACKAGES,
            "不支持的配置目录格式或条目过多"
        );
        let mut ids = BTreeSet::new();
        let mut total = 0;
        for p in &self.packages {
            p.validate()?;
            ensure!(ids.insert(&p.id), "目录中配置 ID 重复");
            total += p.size;
        }
        ensure!(total <= MAX_CATALOG_BYTES, "配置目录归档总量超过 512 MiB");
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PublishedCatalog {
    pub release_id: u64,
    pub tag: String,
    pub digest: String,
    pub catalog: Catalog,
    // Constructed from the configured source, never a catalog-provided download URL.
    archive_base: String,
    archive_urls: BTreeMap<String, String>,
}
impl PublishedCatalog {
    pub(crate) fn fingerprint(&self) -> String {
        format!("{}:{}", self.release_id, self.digest)
    }
}

#[cfg(test)]
type FetchFixture = std::sync::Arc<dyn Fn(&str, u64) -> Result<Vec<u8>> + Send + Sync>;

pub(crate) struct PublicClient {
    agent: ureq::Agent,
    lan_agent: ureq::Agent,
    #[cfg(test)]
    fetch_fixture: Option<FetchFixture>,
}
impl Default for PublicClient {
    fn default() -> Self {
        Self {
            #[cfg(test)]
            fetch_fixture: None,
            agent: ureq::AgentBuilder::new()
                .https_only(false)
                .redirects(0)
                .try_proxy_from_env(true)
                .timeout_connect(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .user_agent(concat!("Gamer-Package-Market/", env!("CARGO_PKG_VERSION")))
                .build(),
            lan_agent: ureq::AgentBuilder::new()
                .https_only(false)
                .redirects(0)
                .timeout_connect(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .user_agent(concat!("Gamer-Market/", env!("CARGO_PKG_VERSION")))
                .build(),
        }
    }
}
pub(crate) fn read_limited(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "远端响应超过大小上限");
    Ok(bytes)
}
impl PublicClient {
    pub(crate) fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        #[cfg(test)]
        if let Some(fetch) = &self.fetch_fixture {
            return fetch(url, limit);
        }
        let mut url = reqwest::Url::parse(url)?;
        let allow_http = url.scheme() == "http";
        for _ in 0..=5 {
            source::validate_download_url(&url)?;
            ensure!(
                allow_http || url.scheme() == "https",
                "来源重定向不能降级到 HTTP"
            );
            let agent = if url.scheme() == "http" {
                &self.lan_agent
            } else {
                &self.agent
            };
            let response = agent
                .get(url.as_str())
                .call()
                .map_err(|error| match error {
                    ureq::Error::Status(status, _) => anyhow!("来源返回 HTTP {status}"),
                    _ => anyhow!("来源请求失败，请检查网络和地址"),
                })?;
            if matches!(response.status(), 301 | 302 | 303 | 307 | 308) {
                url = url.join(response.header("location").context("来源重定向缺少地址")?)?;
                continue;
            }
            ensure!(
                response.status() == 200,
                "来源返回 HTTP {}",
                response.status()
            );
            return read_limited(response.into_reader(), limit);
        }
        bail!("来源重定向次数过多")
    }
    pub(crate) fn latest(&self, repo: &str) -> Result<Option<PublishedCatalog>> {
        let location = SourceLocation::parse(repo)?;
        if matches!(location, SourceLocation::Gitea { .. }) {
            let released = location
                .released_catalog("packages.json", false, |url, limit| self.fetch(url, limit));
            if released
                .as_ref()
                .is_err_and(|e| e.to_string().contains("HTTP 404"))
            {
                if let Some(url) = location.directory_catalog("packages.json") {
                    return self.catalog_from_url(url.as_str()).map(Some);
                }
            }
            let Some((release, document)) = released? else {
                return Ok(None);
            };
            let catalog: Catalog =
                serde_json::from_value(document.clone()).context("配置目录格式无效")?;
            catalog.validate()?;
            let archive_urls = catalog
                .packages
                .iter()
                .map(|p| {
                    Ok((
                        p.asset_name.clone(),
                        location.release_asset(&release, &p.asset_name, Some(p.size))?,
                    ))
                })
                .collect::<Result<_>>()?;
            return Ok(Some(PublishedCatalog {
                release_id: release["id"].as_u64().context("Release ID 缺失")?,
                tag: release["tag_name"].as_str().unwrap().into(),
                digest: sha256_hex(&serde_json::to_vec(&document)?),
                catalog,
                archive_base: String::new(),
                archive_urls,
            }));
        }
        let SourceLocation::Github {
            repository: repo,
            prefix,
        } = &location
        else {
            let SourceLocation::Catalog { url } = &location else {
                unreachable!()
            };
            return self.catalog_from_url(url.as_str()).map(Some);
        };
        if prefix.is_some() {
            // Some prefix proxies implement only release assets, not GitHub API.
            // Probe both through the configured source; never silently use direct GitHub.
            let api = location.wrap(&format!(
                "https://api.github.com/repos/{repo}/releases/latest"
            ));
            let release = self.fetch(&api, MAX_JSON).and_then(|bytes| {
                serde_json::from_slice::<Value>(&bytes).context("Release 接口未返回 JSON")
            });
            return match release {
                Ok(release) if release.get("tag_name").is_some() => self
                    .catalog_from_release(&location, repo, &release)
                    .map(Some),
                _ => self
                    .catalog_from_url(&location.latest_catalog(repo))
                    .map(Some)
                    .map_err(|error| anyhow!("镜像 Release 接口不可用，配置目录探测失败：{error}")),
            };
        }
        let metadata: Value = serde_json::from_slice(
            &self.fetch(&format!("https://api.github.com/repos/{repo}"), MAX_JSON)?,
        )?;
        ensure!(metadata["private"] == false, "首版仅支持公开仓库");
        let canonical = metadata["full_name"]
            .as_str()
            .context("GitHub 仓库元数据缺少 full_name")?;
        ensure!(
            repository(canonical)? == *repo,
            "仓库已重命名，请重新添加规范地址"
        );
        let release = match self
            .agent
            .get(&format!(
                "https://api.github.com/repos/{repo}/releases/latest"
            ))
            .call()
        {
            Ok(response) => {
                serde_json::from_slice::<Value>(&read_limited(response.into_reader(), MAX_JSON)?)?
            }
            Err(ureq::Error::Status(404, _)) => return Ok(None),
            Err(_) => bail!("读取最新 Release 失败，不能将网络错误当作空目录"),
        };
        self.catalog_from_release(&location, canonical, &release)
            .map(Some)
    }
    fn catalog_from_url(&self, url: &str) -> Result<PublishedCatalog> {
        let bytes = self.fetch(url, MAX_JSON)?;
        let catalog: Catalog =
            serde_json::from_slice(&bytes).context("配置目录不是有效的 packages.json")?;
        catalog.validate()?;
        let base = reqwest::Url::parse(url)?.join("./")?;
        Ok(PublishedCatalog {
            release_id: 0,
            tag: String::new(),
            digest: sha256_hex(&bytes),
            catalog,
            archive_base: base.to_string(),
            archive_urls: BTreeMap::new(),
        })
    }
    fn catalog_from_release(
        &self,
        location: &SourceLocation,
        repo: &str,
        release: &Value,
    ) -> Result<PublishedCatalog> {
        ensure!(
            release["draft"] == false && release["prerelease"] == false,
            "配置源必须是正式公开 Release"
        );
        let tag = release["tag_name"].as_str().context("Release 缺少标签")?;
        validate_tag(tag)?;
        let assets = release["assets"].as_array().context("Release 缺少资产")?;
        ensure!(
            assets
                .iter()
                .filter(|a| a["name"] == "packages.json")
                .count()
                == 1,
            "最新 Release 缺少 packages.json，不是有效的配置仓库"
        );
        let bytes = self.fetch(
            &location.wrap(&asset_url(repo, tag, "packages.json")),
            MAX_JSON,
        )?;
        let catalog: Catalog = serde_json::from_slice(&bytes).context("packages.json 格式无效")?;
        catalog.validate()?;
        for p in &catalog.packages {
            ensure!(
                assets
                    .iter()
                    .filter(|a| a["name"] == p.asset_name
                        && a["size"].as_u64() == Some(p.size)
                        && (a["browser_download_url"] == asset_url(repo, tag, &p.asset_name)
                            || a["browser_download_url"]
                                == location.wrap(&asset_url(repo, tag, &p.asset_name))))
                    .count()
                    == 1,
                "Release 资产与配置目录不一致：{}",
                p.id
            );
        }
        Ok(PublishedCatalog {
            release_id: release["id"].as_u64().context("Release ID 缺失")?,
            tag: tag.into(),
            digest: sha256_hex(&bytes),
            catalog,
            archive_base: location.release_base(repo, tag),
            archive_urls: BTreeMap::new(),
        })
    }
    pub(crate) fn archive(&self, repo: &str, tag: &str, entry: &PackageEntry) -> Result<Vec<u8>> {
        let repo = repository(repo)?;
        validate_tag(tag)?;
        self.archive_at(
            &format!("https://github.com/{repo}/releases/download/{tag}/"),
            entry,
        )
    }
    fn archive_at(&self, base: &str, entry: &PackageEntry) -> Result<Vec<u8>> {
        entry.validate()?;
        let bytes = self.fetch(&format!("{base}{}", entry.asset_name), MAX_ARCHIVE)?;
        entry.verify(&bytes)?;
        Ok(bytes)
    }
}
pub(crate) fn validate_tag(tag: &str) -> Result<()> {
    ensure!(
        !tag.is_empty()
            && tag.len() <= 100
            && tag.as_bytes()[0].is_ascii_alphanumeric()
            && tag
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "Release 标签只接受字母、数字、点、横线和下划线"
    );
    Ok(())
}
pub(crate) fn asset_url(repo: &str, tag: &str, name: &str) -> String {
    format!("https://github.com/{repo}/releases/download/{tag}/{name}")
}

#[derive(Default)]
struct Cached {
    checked: Option<Instant>,
    value: Option<PublishedCatalog>,
    error: Option<String>,
}
pub(crate) struct PackageMarket {
    sources: crate::market_sources::SourceStore,
    cache: Mutex<BTreeMap<String, std::sync::Arc<Mutex<Cached>>>>,
    client: PublicClient,
}
impl PackageMarket {
    pub(crate) fn new(data: PathBuf) -> Self {
        Self {
            sources: crate::market_sources::SourceStore::new(data, "package"),
            cache: Mutex::new(BTreeMap::new()),
            client: PublicClient::default(),
        }
    }
    pub(crate) fn sources(&self) -> Result<Vec<Source>> {
        self.sources.list()
    }
    #[cfg(test)]
    pub(crate) fn save_source(&self, input: &str, enabled: bool) -> Result<Vec<Source>> {
        self.save_preset(input, enabled, None, None)
    }
    pub(crate) fn save_preset(
        &self,
        input: &str,
        enabled: bool,
        name: Option<&str>,
        id: Option<&str>,
    ) -> Result<Vec<Source>> {
        let repo = SourceLocation::parse(input)?.canonical();
        let sources = self.sources.save(repo, enabled, name, id)?;
        self.cache.lock().unwrap().clear();
        Ok(sources)
    }
    pub(crate) fn remove(&self, id: &str) -> Result<()> {
        self.sources.remove(id)?;
        self.cache.lock().unwrap().remove(id);
        Ok(())
    }
    fn source(&self, id: &str) -> Result<Source> {
        self.sources()?
            .into_iter()
            .find(|s| s.id == id && s.enabled)
            .context("仓库源不存在或已停用")
    }
    pub(crate) fn catalog(&self, id: &str, refresh: bool) -> Result<Value> {
        let source = self.source(id)?;
        let cached = self
            .cache
            .lock()
            .unwrap()
            .entry(id.into())
            .or_default()
            .clone();
        let mut entry = cached.lock().unwrap();
        if refresh
            || entry
                .checked
                .is_none_or(|t| t.elapsed() > Duration::from_secs(300))
        {
            match self.client.latest(&source.repository) {
                Ok(Some(value)) => {
                    entry.value = Some(value);
                    entry.error = None;
                }
                Ok(None) => {
                    entry.value = None;
                    entry.error = Some("仓库尚无正式 Release".into());
                }
                Err(e) => entry.error = Some(e.to_string()),
            }
            entry.checked = Some(Instant::now());
        }
        Ok(
            json!({"source": source, "packages": entry.value.as_ref().map(|v| v.catalog.packages.clone()).unwrap_or_default(),
            "warning": entry.error, "cached": entry.error.is_some() && entry.value.is_some()}),
        )
    }
    pub(crate) fn archive(
        &self,
        source: &str,
        id: &str,
        version: &str,
        hash: &str,
    ) -> Result<Vec<u8>> {
        let source = self.source(source)?;
        self.catalog(&source.id, false)?;
        let (archive_url, entry) = {
            let cached = self
                .cache
                .lock()
                .unwrap()
                .get(&source.id)
                .cloned()
                .context("没有可用的配置目录")?;
            let entry = cached.lock().unwrap();
            let snapshot = entry.value.as_ref().context("没有可用的配置目录")?;
            let entry = snapshot
                .catalog
                .packages
                .iter()
                .find(|p| p.id == id && p.version == version && p.sha256 == hash)
                .context("目录已变化，请刷新后重试")?;
            (
                snapshot
                    .archive_urls
                    .get(&entry.asset_name)
                    .cloned()
                    .unwrap_or_else(|| format!("{}{}", snapshot.archive_base, entry.asset_name)),
                entry.clone(),
            )
        };
        let bytes = self.client.fetch(&archive_url, MAX_ARCHIVE)?;
        entry.verify(&bytes)?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    type Requests = std::sync::Arc<Mutex<Vec<String>>>;
    #[test]
    fn gitea_source_downloads_only_selected_release_and_rejects_modified_archive() {
        let server = source::test_http::TestServer::new();
        let (directory, bytes) = fixture();
        let (entry, catalog) = catalog_fixture(&bytes);
        let release = json!({"id":7,"tag_name":"demo-v1.0.0","draft":false,"prerelease":false,"published_at":"2026-10-28T00:00:00Z","assets":[
            {"name":"packages.json","size":catalog.len(),"browser_download_url":format!("{}/attachments/catalog",server.base)},
            {"name":entry.asset_name,"size":entry.size,"browser_download_url":format!("{}/attachments/package",server.base)}
        ]});
        server.put(
            "/api/v1/repos/owner/repo/releases?limit=100",
            &serde_json::to_vec(&json!([release])).unwrap(),
        );
        server.put("/attachments/catalog", &catalog);
        server.put("/attachments/package", &bytes);
        let market = PackageMarket::new(directory.path().into());
        let saved = market
            .save_source(&format!("{}/owner/repo.git", server.base), true)
            .unwrap();
        let id = &saved
            .iter()
            .find(|s| s.repository.starts_with(&server.base))
            .unwrap()
            .id;
        assert!(market.catalog(id, true).unwrap()["warning"].is_null());
        assert_eq!(
            market
                .archive(id, &entry.id, &entry.version, &entry.sha256)
                .unwrap(),
            bytes
        );
        server.put("/attachments/package", b"corrupt");
        assert!(market
            .archive(id, &entry.id, &entry.version, &entry.sha256)
            .is_err());
        assert_eq!(
            server.calls.lock().unwrap()[0],
            "/api/v1/repos/owner/repo/releases?limit=100"
        );
    }
    #[test]
    fn http_download_revalidates_redirect_destinations_before_connecting() {
        let server = source::test_http::TestServer::new();
        server.redirect("/escape", "http://8.8.8.8/private");
        assert!(PublicClient::default()
            .fetch(&format!("{}/escape", server.base), 32)
            .unwrap_err()
            .to_string()
            .contains("HTTPS"));
        server.redirect("/local", "/payload");
        server.put("/payload", b"ok");
        assert_eq!(
            PublicClient::default()
                .fetch(&format!("{}/local", server.base), 32)
                .unwrap(),
            b"ok"
        );
    }
    #[test]
    fn static_directory_with_two_segments_still_works_when_it_is_not_a_git_server() {
        let server = source::test_http::TestServer::new();
        let (_directory, archive) = fixture();
        let (entry, catalog) = catalog_fixture(&archive);
        server.put("/team/configs/packages.json", &catalog);
        server.put(&format!("/team/configs/{}", entry.asset_name), &archive);
        let client = PublicClient::default();
        let published = client
            .latest(&format!("{}/team/configs", server.base))
            .unwrap()
            .unwrap();
        assert_eq!(
            client.archive_at(&published.archive_base, &entry).unwrap(),
            archive
        );
        assert_eq!(server.calls.lock().unwrap().len(), 3);
    }
    fn fixture_client(responses: BTreeMap<String, Vec<u8>>) -> (PublicClient, Requests) {
        let requests = Requests::default();
        let observed = requests.clone();
        let client = PublicClient {
            fetch_fixture: Some(std::sync::Arc::new(move |url, limit| {
                observed.lock().unwrap().push(url.into());
                let body = responses.get(url).context("fixture endpoint unavailable")?;
                read_limited(body.as_slice(), limit)
            })),
            ..Default::default()
        };
        (client, requests)
    }
    fn catalog_fixture(bytes: &[u8]) -> (PackageEntry, Vec<u8>) {
        let entry = PackageEntry::from_archive(bytes).unwrap();
        let catalog = serde_json::to_vec(&Catalog {
            schema_version: 1,
            packages: vec![entry.clone()],
        })
        .unwrap();
        (entry, catalog)
    }

    #[test]
    fn arbitrary_prefix_mirror_probes_release_and_freezes_archive_route() {
        let (directory, archive) = fixture();
        let (entry, catalog) = catalog_fixture(&archive);
        let prefix = "https://independent-proxy.example/cache/";
        let source = format!("{prefix}https://github.com/owner/repo");
        let release = json!({"id":1,"draft":false,"prerelease":false,"tag_name":"v1.0.0","assets":[
            {"name":"packages.json"},
            {"name":entry.asset_name,"size":entry.size,"browser_download_url":asset_url("owner/repo","v1.0.0",&entry.asset_name)}]});
        let base = format!("{prefix}https://github.com/owner/repo/releases/download/v1.0.0/");
        let (client, calls) = fixture_client(BTreeMap::from([
            (
                format!("{prefix}https://api.github.com/repos/owner/repo/releases/latest"),
                serde_json::to_vec(&release).unwrap(),
            ),
            (format!("{base}packages.json"), catalog),
            (format!("{base}{}", entry.asset_name), archive.clone()),
        ]));
        let mut market = PackageMarket::new(directory.path().into());
        market.client = client;
        let saved = market.save_source(&source, true).unwrap();
        let saved = saved.iter().find(|s| s.repository == source).unwrap();
        assert_eq!(
            market.catalog(&saved.id, true).unwrap()["packages"][0]["id"],
            "demo"
        );
        assert_eq!(
            market
                .archive(&saved.id, &entry.id, &entry.version, &entry.sha256)
                .unwrap(),
            archive
        );
        assert!(market
            .archive(&saved.id, &entry.id, &entry.version, &"0".repeat(64))
            .is_err());
        assert_eq!(calls.lock().unwrap().len(), 3);
        assert!(calls
            .lock()
            .unwrap()
            .iter()
            .all(|url| url.starts_with(prefix)));
        assert!(PackageMarket::new(directory.path().into())
            .sources()
            .unwrap()
            .iter()
            .any(|s| s.repository == source));
    }

    #[test]
    fn download_only_mirror_uses_latest_asset_catalog_without_direct_bypass() {
        let (_directory, archive) = fixture();
        let (entry, catalog) = catalog_fixture(&archive);
        let base =
            "https://files-only.example/https://github.com/owner/repo/releases/latest/download/";
        let (client, calls) = fixture_client(BTreeMap::from([
            (format!("{base}packages.json"), catalog),
            (format!("{base}{}", entry.asset_name), archive.clone()),
        ]));
        let snapshot = client
            .latest("https://files-only.example/https://github.com/owner/repo")
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.archive_base, base);
        assert_eq!(
            client.archive_at(&snapshot.archive_base, &entry).unwrap(),
            archive
        );
        assert_eq!(calls.lock().unwrap().len(), 3);
        assert!(calls
            .lock()
            .unwrap()
            .iter()
            .all(|url| url.starts_with("https://files-only.example/")));
    }

    #[test]
    fn static_catalog_uses_siblings_and_keeps_hash_and_manifest_checks() {
        let (_directory, archive) = fixture();
        let (entry, catalog) = catalog_fixture(&archive);
        let base = "https://static.example/configs/";
        let (client, calls) = fixture_client(BTreeMap::from([
            (format!("{base}packages.json"), catalog),
            (format!("{base}{}", entry.asset_name), archive.clone()),
        ]));
        let snapshot = client.latest(base).unwrap().unwrap();
        assert_eq!(
            client.archive_at(&snapshot.archive_base, &entry).unwrap(),
            archive
        );
        let mut bad = entry.clone();
        bad.sha256 = "0".repeat(64);
        assert!(client.archive_at(&snapshot.archive_base, &bad).is_err());
        bad = entry;
        bad.name = "different inner identity".into();
        assert!(client.archive_at(&snapshot.archive_base, &bad).is_err());
        assert!(calls
            .lock()
            .unwrap()
            .iter()
            .all(|url| url.starts_with(base)));
    }

    #[test]
    fn mirror_release_validation_errors_are_not_hidden_by_catalog_fallback() {
        let (_directory, archive) = fixture();
        let (entry, catalog) = catalog_fixture(&archive);
        let release = json!({"id":1,"draft":false,"prerelease":false,"tag_name":"v1.0.0","assets":[
            {"name":"packages.json"},{"name":entry.asset_name,"size":entry.size,"browser_download_url":"https://other.example/private"}]});
        let (client,calls)=fixture_client(BTreeMap::from([
            ("https://proxy.example/https://api.github.com/repos/owner/repo/releases/latest".into(),serde_json::to_vec(&release).unwrap()),
            ("https://proxy.example/https://github.com/owner/repo/releases/download/v1.0.0/packages.json".into(),catalog),
        ]));
        assert!(client
            .latest("https://proxy.example/https://github.com/owner/repo")
            .unwrap_err()
            .to_string()
            .contains("资产与配置目录不一致"));
        assert_eq!(calls.lock().unwrap().len(), 2);
    }

    #[test]
    #[ignore = "Explicit public-source download acceptance; set GAMER_TEST_CONFIG_SOURCE"]
    fn configured_source_live_acceptance() {
        let input = std::env::var("GAMER_TEST_CONFIG_SOURCE").expect("explicit source required");
        let directory = tempfile::tempdir().unwrap();
        let market = PackageMarket::new(directory.path().into());
        let canonical = SourceLocation::parse(&input).unwrap().canonical();
        let saved = market.save_source(&input, true).unwrap();
        let source = saved.iter().find(|s| s.repository == canonical).unwrap();
        let catalog = market.catalog(&source.id, true).unwrap();
        assert!(catalog["warning"].is_null(), "{catalog}");
        let entry: PackageEntry = serde_json::from_value(catalog["packages"][0].clone()).unwrap();
        let archive = market
            .archive(&source.id, &entry.id, &entry.version, &entry.sha256)
            .unwrap();
        entry.verify(&archive).unwrap();
        println!(
            "{}",
            json!({"source":canonical,"id":entry.id,"version":entry.version,"bytes":archive.len(),"sha256":sha256_hex(&archive),"imported":false})
        );
    }
    fn fixture() -> (tempfile::TempDir, Vec<u8>) {
        let d = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            data_dir: d.path().into(),
            ..Default::default()
        };
        let store = crate::resources::PackageStore::open(&cfg).unwrap();
        store
            .create_package(crate::resources::PackageInput {
                id: "demo".into(),
                version: Some("1.0.0".into()),
                android_targets: vec!["*".into()],
                ..Default::default()
            })
            .unwrap();
        let bytes = crate::package_archive::export_package(&store, "demo", None, false)
            .unwrap()
            .archive;
        (d, bytes)
    }
    #[test]
    fn catalog_hash_and_inner_identity_are_both_verified() {
        let (_d, bytes) = fixture();
        let entry = PackageEntry::from_archive(&bytes).unwrap();
        entry.verify(&bytes).unwrap();
        let mut wrong = entry.clone();
        wrong.name = "spoofed".into();
        assert!(wrong.verify(&bytes).is_err());
        wrong = entry.clone();
        wrong.sha256 = "0".repeat(64);
        assert!(wrong.verify(&bytes).is_err());
        let mut catalog = Catalog {
            schema_version: 1,
            packages: vec![entry.clone(), entry.clone()],
        };
        assert!(catalog.validate().is_err());
        catalog.packages.pop();
        catalog.packages[0].asset_name = "../../private.gamerpkg".into();
        assert!(catalog.validate().is_err());
    }
    #[test]
    fn snapshots_cannot_redirect_downloads_outside_the_repository() {
        let (_d, bytes) = fixture();
        let entry = PackageEntry::from_archive(&bytes).unwrap();
        let mut raw = serde_json::to_value(Catalog {
            schema_version: 1,
            packages: vec![entry],
        })
        .unwrap();
        raw["packages"][0]["download_url"] = json!("http://127.0.0.1/private");
        assert!(serde_json::from_value::<Catalog>(raw).is_err());
        for tag in ["../main", "a/b", "--flag", "tag?x", "tag#x"] {
            assert!(validate_tag(tag).is_err());
        }
    }
    #[test]
    fn repositories_are_not_urls_or_flags() {
        assert_eq!(
            repository("https://github.com/Owner/Repo.git/").unwrap(),
            "owner/repo"
        );
        for bad in [
            "https://evil.test/o/r",
            "https://u@github.com/o/r",
            "o/r?q=x",
            "o/../r",
            "--help/r",
            "o/%2e%2e",
            "o/r#frag",
        ] {
            assert!(repository(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn source_changes_persist_and_do_not_delete_packages() {
        let d = tempfile::tempdir().unwrap();
        let market = PackageMarket::new(d.path().into());
        let first = market
            .save_source("https://github.com/Owner/Repo", true)
            .unwrap();
        assert_eq!(
            market.save_source("owner/repo.git", false).unwrap().len(),
            3
        );
        assert!(
            !PackageMarket::new(d.path().into())
                .sources()
                .unwrap()
                .iter()
                .find(|s| s.repository == "owner/repo")
                .unwrap()
                .enabled
        );
        for source in first.into_iter().filter(|s| !s.builtin) {
            market.remove(&source.id).unwrap();
        }
        assert_eq!(
            PackageMarket::new(d.path().into()).sources().unwrap().len(),
            2
        );
    }
    #[test]
    fn limit_is_enforced_while_reading() {
        assert!(read_limited(&b"1234"[..], 3).is_err());
    }
}
