//! Public GitHub package catalogs. No credentials, plugin lifecycle or DSL knowledge.
use crate::{
    package_archive::{sha256_hex, validate_and_read_manifest},
    resources::{parse_package_toml, validate_scope_id},
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

pub(crate) const MAX_JSON: u64 = 2 * 1024 * 1024;
pub(crate) const MAX_ARCHIVE: u64 = 100 * 1024 * 1024;
pub(crate) const MAX_CATALOG_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const MAX_PACKAGES: usize = 128;
const OFFICIAL_REPOSITORY: &str = "jesongit/gamer-packages";

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
                && self.android_targets.len() <= 128,
            "配置包元数据超限"
        );
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Source {
    pub id: String,
    pub repository: String,
    pub enabled: bool,
}
#[derive(Clone, Debug)]
pub(crate) struct PublishedCatalog {
    pub release_id: u64,
    pub tag: String,
    pub digest: String,
    pub catalog: Catalog,
}
impl PublishedCatalog {
    pub(crate) fn fingerprint(&self) -> String {
        format!("{}:{}", self.release_id, self.digest)
    }
}

pub(crate) struct PublicClient {
    agent: ureq::Agent,
}
impl Default for PublicClient {
    fn default() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .https_only(true)
                .try_proxy_from_env(true)
                .timeout_connect(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .user_agent(concat!("Gamer-Package-Market/", env!("CARGO_PKG_VERSION")))
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
        read_limited(
            self.agent
                .get(url)
                .call()
                .map_err(|_| anyhow!("GitHub 请求失败，请检查网络和公开仓库权限"))?
                .into_reader(),
            limit,
        )
    }
    pub(crate) fn latest(&self, repo: &str) -> Result<Option<PublishedCatalog>> {
        let repo = repository(repo)?;
        let metadata: Value = serde_json::from_slice(
            &self.fetch(&format!("https://api.github.com/repos/{repo}"), MAX_JSON)?,
        )?;
        ensure!(metadata["private"] == false, "首版仅支持公开仓库");
        let canonical = metadata["full_name"]
            .as_str()
            .context("GitHub 仓库元数据缺少 full_name")?;
        ensure!(
            repository(canonical)? == repo,
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
        self.catalog_from_release(canonical, &release).map(Some)
    }
    fn catalog_from_release(&self, repo: &str, release: &Value) -> Result<PublishedCatalog> {
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
        let bytes = self.fetch(&asset_url(repo, tag, "packages.json"), MAX_JSON)?;
        let catalog: Catalog = serde_json::from_slice(&bytes).context("packages.json 格式无效")?;
        catalog.validate()?;
        for p in &catalog.packages {
            ensure!(
                assets
                    .iter()
                    .filter(|a| a["name"] == p.asset_name
                        && a["size"].as_u64() == Some(p.size)
                        && a["browser_download_url"] == asset_url(repo, tag, &p.asset_name))
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
        })
    }
    pub(crate) fn archive(&self, repo: &str, tag: &str, entry: &PackageEntry) -> Result<Vec<u8>> {
        let bytes = self.fetch(&asset_url(repo, tag, &entry.asset_name), MAX_ARCHIVE)?;
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
    path: PathBuf,
    lock: Mutex<()>,
    cache: Mutex<BTreeMap<String, std::sync::Arc<Mutex<Cached>>>>,
    client: PublicClient,
}
impl PackageMarket {
    pub(crate) fn new(data: PathBuf) -> Self {
        Self {
            path: data.join("package-sources.json"),
            lock: Mutex::new(()),
            cache: Mutex::new(BTreeMap::new()),
            client: PublicClient::default(),
        }
    }
    fn load(&self) -> Result<Vec<Source>> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                ensure!(bytes.len() < 65536, "仓库源设置超限");
                Ok(serde_json::from_slice(&bytes)?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![Source {
                id: sha256_hex(OFFICIAL_REPOSITORY.as_bytes())[..16].into(),
                repository: OFFICIAL_REPOSITORY.into(),
                enabled: true,
            }]),
            Err(e) => Err(e.into()),
        }
    }
    pub(crate) fn sources(&self) -> Result<Vec<Source>> {
        let _guard = self.lock.lock().unwrap();
        self.load()
    }
    pub(crate) fn save_source(&self, input: &str, enabled: bool) -> Result<Vec<Source>> {
        let repo = repository(input)?;
        let _guard = self.lock.lock().unwrap();
        let mut sources = self.load()?;
        if let Some(source) = sources.iter_mut().find(|s| s.repository == repo) {
            source.enabled = enabled;
        } else {
            ensure!(sources.len() < 16, "最多添加 16 个仓库源");
            sources.push(Source {
                id: sha256_hex(repo.as_bytes())[..16].into(),
                repository: repo,
                enabled,
            });
        }
        fs::create_dir_all(self.path.parent().unwrap())?;
        crate::core::fs::atomic_write(&self.path, &serde_json::to_vec_pretty(&sources)?)?;
        Ok(sources)
    }
    pub(crate) fn remove(&self, id: &str) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        let mut sources = self.load()?;
        sources.retain(|s| s.id != id);
        crate::core::fs::atomic_write(&self.path, &serde_json::to_vec_pretty(&sources)?)?;
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
        let (tag, entry) = {
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
            (snapshot.tag.clone(), entry.clone())
        };
        self.client.archive(&source.repository, &tag, &entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            2
        );
        assert!(!PackageMarket::new(d.path().into()).sources().unwrap()[1].enabled);
        for source in first {
            market.remove(&source.id).unwrap();
        }
        assert!(PackageMarket::new(d.path().into())
            .sources()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn limit_is_enforced_while_reading() {
        assert!(read_limited(&b"1234"[..], 3).is_err());
    }
}
