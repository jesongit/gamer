//! Official release discovery and same-origin downloads. Installation still uses
//! ExtensionService's existing inspect/permission/hash/lifecycle gates.
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

use semver::Version;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const RELEASES: &str = "https://api.github.com/repos/jesongit/gamer-plugins/releases?per_page=100";
const DOWNLOAD_BASE: &str = "https://github.com/jesongit/gamer-plugins/releases/download/";
const MAX_JSON: u64 = 3 * 1024 * 1024;
const MAX_ARCHIVE: u64 = 20 * 1024 * 1024;
const REFRESH_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Clone)]
enum Origin {
    Bundled(PathBuf),
    Remote(String),
}

#[derive(Clone)]
struct Entry {
    metadata: Value,
    origin: Origin,
}

#[derive(Clone, Default)]
struct Catalog {
    entries: Vec<Entry>,
    warning: Option<String>,
    source: &'static str,
}

#[derive(Default)]
struct Cache {
    checked: Option<Instant>,
    remote: Vec<Entry>,
    warning: Option<String>,
}

pub(crate) struct PluginMarket {
    web_dir: PathBuf,
    beta: bool,
    agent: ureq::Agent,
    cache: Mutex<Cache>,
}

impl PluginMarket {
    pub(crate) fn new(web_dir: PathBuf) -> Self {
        Self {
            web_dir,
            beta: !Version::parse(env!("CARGO_PKG_VERSION"))
                .unwrap()
                .pre
                .is_empty(),
            agent: ureq::AgentBuilder::new()
                .https_only(true)
                .try_proxy_from_env(true)
                .timeout_connect(Duration::from_secs(5))
                .timeout(Duration::from_secs(15))
                .user_agent(concat!("Gamer-Plugin-Market/", env!("CARGO_PKG_VERSION")))
                .build(),
            cache: Mutex::new(Cache::default()),
        }
    }

    fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>, String> {
        let response = self
            .agent
            .get(url)
            .call()
            .map_err(|e| format!("官方插件源请求失败：{e}"))?;
        read_limited(response.into_reader(), limit)
    }

    fn discover(&self) -> Result<Vec<Entry>, String> {
        let releases: Value =
            serde_json::from_slice(&self.fetch(RELEASES, MAX_JSON)?).map_err(|e| e.to_string())?;
        let release = select_release(&releases, self.beta)?;
        let tag = release["tag_name"].as_str().ok_or("发布缺少标签")?;
        let bytes = self.fetch(&format!("{DOWNLOAD_BASE}{tag}/registry.json"), MAX_JSON)?;
        let document: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        parse_entries(&document, Some(release), &self.web_dir, self.beta)
    }

    fn catalog(&self, force: bool) -> Result<Catalog, String> {
        self.catalog_with(force, || self.discover())
    }

    fn catalog_with(
        &self,
        force: bool,
        discover: impl FnOnce() -> Result<Vec<Entry>, String>,
    ) -> Result<Catalog, String> {
        // Serialize refreshes to avoid multiple open tabs exhausting GitHub's API quota.
        let mut cache = self.cache.lock().map_err(|_| "插件列表缓存不可用")?;
        if force
            || cache
                .checked
                .is_none_or(|t| t.elapsed() >= REFRESH_INTERVAL)
        {
            match discover() {
                Ok(entries) => {
                    cache.remote = entries;
                    cache.warning = None;
                }
                Err(error) => {
                    tracing::warn!(%error, "官方插件列表刷新失败");
                    cache.warning =
                        Some("暂时无法检查插件新版本，显示已缓存或随软件附带的插件列表".into());
                }
            }
            cache.checked = Some(Instant::now());
        }
        let bundled = fs::File::open(self.web_dir.join("registry.json"))
            .map_err(|e| e.to_string())
            .and_then(|f| read_limited(f, MAX_JSON))
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string()))
            .and_then(|doc| parse_entries(&doc, None, &self.web_dir, true));
        if cache.remote.is_empty() && bundled.is_err() {
            return Err(
                "官方插件源不可用，且本机没有可用的插件列表；已安装插件仍可正常使用".into(),
            );
        }
        let source = if cache.remote.is_empty() {
            "bundled"
        } else if cache.warning.is_some() {
            "cache"
        } else {
            "remote"
        };
        Ok(Catalog {
            entries: merge_entries(bundled.unwrap_or_default(), cache.remote.clone()),
            source,
            warning: cache.warning.clone(),
        })
    }

    pub(crate) fn registry(&self, force: bool) -> Result<Value, String> {
        let catalog = self.catalog(force)?;
        Ok(json!({
            "schema_version": 2,
            "market_status": { "source": catalog.source, "warning": catalog.warning },
            "plugins": catalog.entries.into_iter().map(|e| e.metadata).collect::<Vec<_>>()
        }))
    }

    pub(crate) fn archive(&self, id: &str, version: &str) -> Result<Vec<u8>, String> {
        super::ExtensionId::parse(id).map_err(|e| e.to_string())?;
        Version::parse(version).map_err(|e| e.to_string())?;
        let catalog = self.catalog(false)?;
        let entry = catalog
            .entries
            .iter()
            .find(|e| e.metadata["id"] == id && e.metadata["version"] == version)
            .ok_or("插件版本不在当前列表中，请刷新后重试")?;
        let expected_size = entry.metadata["size"].as_u64().ok_or("插件缺少大小信息")?;
        let bytes = match &entry.origin {
            Origin::Bundled(path) => read_limited(
                fs::File::open(path).map_err(|e| e.to_string())?,
                MAX_ARCHIVE,
            )?,
            Origin::Remote(url) => self.fetch(url, MAX_ARCHIVE)?,
        };
        verify_archive(
            &bytes,
            expected_size,
            entry.metadata["sha256"].as_str().unwrap_or_default(),
        )?;
        Ok(bytes)
    }
}

fn read_limited(reader: impl Read, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("插件源响应超过大小上限".into());
    }
    Ok(bytes)
}

fn select_release(releases: &Value, beta: bool) -> Result<&Value, String> {
    releases
        .as_array()
        .ok_or("插件发布列表格式无效")?
        .iter()
        .filter(|r| r["draft"] == false && (beta || r["prerelease"] == false))
        .filter(|r| {
            let Some(tag) = r["tag_name"].as_str() else {
                return false;
            };
            let Some((id, version)) = tag.rsplit_once("-v") else {
                return false;
            };
            if super::ExtensionId::parse(id).is_err() || Version::parse(version).is_err() {
                return false;
            }
            if !beta && Version::parse(version).is_ok_and(|v| !v.pre.is_empty()) {
                return false;
            }
            r["assets"].as_array().is_some_and(|assets| {
                assets.iter().any(|a| {
                    a["name"] == "registry.json"
                        && a["browser_download_url"]
                            == format!("{DOWNLOAD_BASE}{tag}/registry.json")
                })
            })
        })
        // Every plugin release publishes the complete official catalog. Tags are
        // per-plugin, so version numbers across different plugin IDs are unrelated.
        .max_by_key(|r| r["published_at"].as_str().unwrap_or_default())
        .ok_or_else(|| "当前通道尚无公开的官方插件列表".into())
}

fn parse_entries(
    doc: &Value,
    release: Option<&Value>,
    web_dir: &std::path::Path,
    beta: bool,
) -> Result<Vec<Entry>, String> {
    if doc["schema_version"] != 2 {
        return Err("不支持的官方插件列表格式".into());
    }
    let plugins = doc["plugins"].as_array().ok_or("插件列表缺少 plugins")?;
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for raw in plugins {
        let id = raw["id"].as_str().ok_or("插件缺少 id")?;
        super::ExtensionId::parse(id).map_err(|e| e.to_string())?;
        let version_text = raw["version"].as_str().ok_or("插件缺少版本")?;
        let version = Version::parse(version_text).map_err(|e| e.to_string())?;
        if !beta && !version.pre.is_empty() {
            continue;
        }
        if !seen.insert((id, version_text)) {
            return Err("插件列表存在重复版本".into());
        }
        let hash = raw["sha256"].as_str().ok_or("插件缺少 SHA256")?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("插件 SHA256 无效".into());
        }
        let size = raw["size"]
            .as_u64()
            .filter(|&n| n > 0 && n <= MAX_ARCHIVE)
            .ok_or("插件大小无效")?;
        let filename = format!("{id}-{version_text}.gplugin");
        let url = raw["download_url"].as_str().ok_or("插件缺少下载地址")?;
        let origin = if let Some(release) = release {
            let tag = release["tag_name"].as_str().ok_or("发布缺少标签")?;
            let expected = format!("{DOWNLOAD_BASE}{tag}/{filename}");
            if url != expected {
                return Err("插件下载地址与官方发布不一致".into());
            }
            let asset = release["assets"]
                .as_array()
                .and_then(|assets| {
                    assets.iter().find(|a| {
                        a["name"] == filename
                            && a["browser_download_url"] == expected
                            && a["size"] == size
                    })
                })
                .ok_or("插件归档不在该发布资产中或大小不一致")?;
            if let Some(digest) = asset["digest"]
                .as_str()
                .and_then(|d| d.strip_prefix("sha256:"))
            {
                if !digest.eq_ignore_ascii_case(hash) {
                    return Err("插件列表与发布资产 SHA256 不一致".into());
                }
            }
            Origin::Remote(expected)
        } else {
            if url != format!("/plugins/{filename}") {
                return Err("本地插件下载路径无效".into());
            }
            Origin::Bundled(web_dir.join("plugins").join(filename))
        };
        let mut metadata = raw.clone();
        metadata["download_url"] =
            format!("/api/extensions/market/{id}/{version_text}/archive").into();
        entries.push(Entry { metadata, origin });
    }
    Ok(entries)
}

fn merge_entries(bundled: Vec<Entry>, remote: Vec<Entry>) -> Vec<Entry> {
    let mut entries = BTreeMap::new();
    // Keep bundled versions available offline; new immutable versions come from
    // the plugin repository. Keep the release's pinned bytes for bundled versions.
    for entry in remote.into_iter().chain(bundled) {
        let key = (
            entry.metadata["id"].as_str().unwrap().to_owned(),
            entry.metadata["version"].as_str().unwrap().to_owned(),
        );
        entries.insert(key, entry);
    }
    entries.into_values().collect()
}

fn verify_archive(bytes: &[u8], size: u64, hash: &str) -> Result<(), String> {
    if bytes.len() as u64 != size
        || !format!("{:x}", Sha256::digest(bytes)).eq_ignore_ascii_case(hash)
    {
        return Err("插件下载文件与发布清单不一致（大小或 SHA256 校验失败）".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn document(version: &str) -> Value {
        json!({"schema_version":2,"plugins":[{
            "id":"gamer-example", "name":"Example", "version":version,
            "download_url":format!("/plugins/gamer-example-{version}.gplugin"),
            "sha256":format!("{:x}",Sha256::digest(b"verified plugin")),"size":15,
            "execution":{"kind":"wasm"},"permissions":["ui.host"]
        }]})
    }

    fn release(tag: &str, prerelease: bool, published: &str) -> Value {
        json!({"tag_name":tag,"draft":false,"prerelease":prerelease,"published_at":published,
            "assets":[{"name":"registry.json","browser_download_url":format!("{DOWNLOAD_BASE}{tag}/registry.json")}]})
    }

    fn remote_document(version: &str, r: &mut Value) -> Value {
        let mut doc = document(version);
        let filename = format!("gamer-example-{version}.gplugin");
        let url = format!(
            "{DOWNLOAD_BASE}{}/{filename}",
            r["tag_name"].as_str().unwrap()
        );
        doc["plugins"][0]["download_url"] = url.clone().into();
        r["assets"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":filename,"browser_download_url":url,"size":15,
            "digest":format!("sha256:{}",doc["plugins"][0]["sha256"].as_str().unwrap())}));
        doc
    }

    #[test]
    fn release_discovery_obeys_channel_and_uses_publication_order_across_plugin_ids() {
        let stable = release("gamer-example-v2.0.0", false, "2026-09-20T00:00:00Z");
        let beta = release("gamer-another-v0.1.0-beta.10", true, "2026-09-21T00:00:00Z");
        let mut draft = release("gamer-example-v9.0.0", false, "2026-09-23T00:00:00Z");
        draft["draft"] = true.into();
        let mut forged = release("gamer-example-v8.0.0", false, "2026-09-24T00:00:00Z");
        forged["assets"][0]["browser_download_url"] =
            "https://example.invalid/registry.json".into();
        let releases = json!([draft, stable.clone(), beta.clone(), forged]);
        assert_eq!(select_release(&releases, true).unwrap(), &beta);
        assert_eq!(select_release(&releases, false).unwrap(), &stable);
        assert!(select_release(&json!([beta]), false).is_err());
    }

    #[test]
    fn remote_catalog_pins_asset_identity_size_hash_and_rewrites_download_to_same_origin() {
        let mut r = release("gamer-example-v0.1.0-beta.2", true, "2026-09-21");
        let doc = remote_document("0.1.0-beta.2", &mut r);
        let entries = parse_entries(&doc, Some(&r), Path::new("unused"), true).unwrap();
        assert_eq!(
            entries[0].metadata["download_url"],
            "/api/extensions/market/gamer-example/0.1.0-beta.2/archive"
        );
        assert!(parse_entries(&doc, Some(&r), Path::new("unused"), false)
            .unwrap()
            .is_empty());
        let mut wrong = doc.clone();
        wrong["plugins"][0]["download_url"] = "https://localhost/internal".into();
        assert!(parse_entries(&wrong, Some(&r), Path::new("unused"), true).is_err());
        wrong = doc.clone();
        wrong["plugins"][0]["size"] = 16.into();
        assert!(parse_entries(&wrong, Some(&r), Path::new("unused"), true).is_err());
        wrong = doc.clone();
        wrong["plugins"][0]["sha256"] = "a".repeat(64).into();
        assert!(parse_entries(&wrong, Some(&r), Path::new("unused"), true).is_err());
        wrong = doc.clone();
        wrong["plugins"]
            .as_array_mut()
            .unwrap()
            .push(doc["plugins"][0].clone());
        assert!(parse_entries(&wrong, Some(&r), Path::new("unused"), true).is_err());
    }

    #[test]
    fn offline_catalog_preserves_bundled_downloads_and_last_successful_remote_versions() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("plugins")).unwrap();
        fs::write(
            dir.path().join("registry.json"),
            serde_json::to_vec(&document("0.1.0-beta.1")).unwrap(),
        )
        .unwrap();
        fs::write(
            dir.path()
                .join("plugins/gamer-example-0.1.0-beta.1.gplugin"),
            b"verified plugin",
        )
        .unwrap();
        let market = PluginMarket::new(dir.path().into());
        let bundled = market.catalog_with(true, || Err("offline".into())).unwrap();
        assert_eq!(bundled.source, "bundled");
        assert!(bundled.warning.is_some());
        assert_eq!(
            market.archive("gamer-example", "0.1.0-beta.1").unwrap(),
            b"verified plugin"
        );
        let mut r = release("gamer-example-v0.1.0-beta.2", true, "2026-09-21");
        let doc = remote_document("0.1.0-beta.2", &mut r);
        let online = market
            .catalog_with(true, || parse_entries(&doc, Some(&r), dir.path(), true))
            .unwrap();
        assert_eq!(online.entries.len(), 2);
        assert_eq!(online.source, "remote");
        assert!(online.warning.is_none());
        assert_eq!(
            market
                .catalog_with(false, || panic!("cache should prevent network request"))
                .unwrap()
                .entries
                .len(),
            2
        );
        let offline = market
            .catalog_with(true, || Err("rate limited".into()))
            .unwrap();
        assert_eq!(offline.entries.len(), 2);
        assert_eq!(offline.source, "cache");
        fs::write(
            dir.path()
                .join("plugins/gamer-example-0.1.0-beta.1.gplugin"),
            b"damaged plugin!",
        )
        .unwrap();
        assert!(market.archive("gamer-example", "0.1.0-beta.1").is_err());
        assert!(market.archive("gamer-example", "0.1.0-beta.9").is_err());
    }

    #[test]
    fn bounded_reads_and_archive_hash_reject_truncation_and_changed_bytes() {
        assert!(read_limited(&b"too long"[..], 3).is_err());
        let hash = format!("{:x}", Sha256::digest(b"verified plugin"));
        assert!(verify_archive(b"verified plugin", 15, &hash).is_ok());
        assert!(verify_archive(b"verified plugin", 16, &hash).is_err());
        assert!(verify_archive(b"damaged plugin!", 15, &hash).is_err());
        let mut traversal = document("1.0.0");
        traversal["plugins"][0]["download_url"] = "/plugins/../../secret".into();
        assert!(parse_entries(&traversal, None, Path::new("unused"), true).is_err());
    }

    #[test]
    #[ignore = "real public GitHub plugin repository; explicit online acceptance only"]
    fn published_official_catalog_and_archive_are_downloadable() {
        let dir = tempfile::tempdir().unwrap();
        let market = PluginMarket::new(dir.path().into());
        let registry = market.registry(true).unwrap();
        assert_eq!(registry["market_status"]["source"], "remote");
        let entries = registry["plugins"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
        for entry in entries {
            let bytes = market
                .archive(
                    entry["id"].as_str().unwrap(),
                    entry["version"].as_str().unwrap(),
                )
                .unwrap();
            assert!(bytes.starts_with(b"PK"));
        }
    }
}
