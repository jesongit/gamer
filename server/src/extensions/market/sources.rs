use super::*;
use crate::package_market::{source::SourceLocation, PublicClient};

impl PluginMarket {
    pub(crate) fn sources(&self) -> anyhow::Result<Vec<crate::market_sources::Source>> {
        self.sources.list()
    }
    pub(crate) fn save_source(
        &self,
        input: &str,
        enabled: bool,
        name: Option<&str>,
        id: Option<&str>,
    ) -> anyhow::Result<Vec<crate::market_sources::Source>> {
        let location = SourceLocation::parse_for(input, "registry.json")?;
        let sources = self.sources.save(location.canonical(), enabled, name, id)?;
        self.source_cache.lock().unwrap().clear();
        Ok(sources)
    }
    pub(crate) fn remove_source(&self, id: &str) -> anyhow::Result<()> {
        self.sources.remove(id)?;
        self.source_cache.lock().unwrap().clear();
        Ok(())
    }
    fn selected_source(&self, id: &str) -> Result<crate::market_sources::Source, String> {
        self.sources()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|s| s.id == id && s.enabled)
            .ok_or_else(|| "插件来源不存在或已停用".into())
    }
    fn discover_source(&self, input: &str) -> anyhow::Result<Vec<Entry>> {
        let location = SourceLocation::parse_for(input, "registry.json")?;
        let client = PublicClient::default();
        let (document, downloads) = match &location {
            SourceLocation::Catalog { url } => return self.static_entries(url, &client),
            _ => {
                let released = location
                    .released_catalog("registry.json", self.beta, |u, l| client.fetch(u, l));
                if released
                    .as_ref()
                    .is_err_and(|e| e.to_string().contains("HTTP 404"))
                {
                    if let Some(url) = location.directory_catalog("registry.json") {
                        return self.static_entries(&url, &client);
                    }
                }
                let (release, doc) = released?
                    .ok_or_else(||anyhow::anyhow!("该仓库尚未发布 registry.json 和 .gplugin 附件；请先在 Release 发布插件安装包"))?;
                let mut downloads = BTreeMap::new();
                for p in doc["plugins"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("目录缺少 plugins"))?
                {
                    if retired_catalog_entry(p) {
                        continue;
                    }
                    let file = format!(
                        "{}-{}.gplugin",
                        p["id"].as_str().unwrap_or_default(),
                        p["version"].as_str().unwrap_or_default()
                    );
                    downloads.insert(
                        file.clone(),
                        location.release_asset(
                            &release,
                            &file,
                            Some(
                                p["size"]
                                    .as_u64()
                                    .ok_or_else(|| anyhow::anyhow!("插件缺少大小"))?,
                            ),
                        )?,
                    );
                }
                (doc, downloads)
            }
        };
        parse_entries_with_downloads(&document, None, &self.web_dir, self.beta, Some(&downloads))
            .map_err(anyhow::Error::msg)
    }
    fn static_entries(
        &self,
        url: &reqwest::Url,
        client: &PublicClient,
    ) -> anyhow::Result<Vec<Entry>> {
        let doc: Value = serde_json::from_slice(&client.fetch(url.as_str(), MAX_JSON)?)?;
        let mut downloads = BTreeMap::new();
        for p in doc["plugins"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("目录缺少 plugins"))?
        {
            let file = format!(
                "{}-{}.gplugin",
                p["id"].as_str().unwrap_or_default(),
                p["version"].as_str().unwrap_or_default()
            );
            downloads.insert(file.clone(), url.join(&file)?.to_string());
        }
        parse_entries_with_downloads(&doc, None, &self.web_dir, self.beta, Some(&downloads))
            .map_err(anyhow::Error::msg)
    }
    fn source_catalog(&self, id: &str, force: bool) -> Result<Catalog, String> {
        let source = self.selected_source(id)?;
        if source.repository == "jesongit/gamer-plugins" {
            return self.catalog(force);
        }
        let key = format!("{}:{}", source.id, source.repository);
        let cache = self
            .source_cache
            .lock()
            .unwrap()
            .entry(key)
            .or_default()
            .clone();
        let mut cache = cache.lock().unwrap();
        if force
            || cache
                .checked
                .is_none_or(|t| t.elapsed() >= REFRESH_INTERVAL)
        {
            match self.discover_source(&source.repository) {
                Ok(entries) => {
                    cache.remote = entries;
                    cache.warning = None;
                }
                Err(e) => {
                    cache.warning = Some(e.to_string());
                }
            }
            cache.checked = Some(Instant::now());
        }
        if cache.remote.is_empty() {
            if let Some(error) = &cache.warning {
                return Err(error.clone());
            }
        }
        Ok(Catalog {
            entries: cache.remote.clone(),
            warning: cache.warning.clone(),
            source: if cache.warning.is_some() {
                "cache"
            } else {
                "remote"
            },
        })
    }
    pub(crate) fn registry_for(&self, id: Option<&str>, force: bool) -> Result<Value, String> {
        let Some(id) = id else {
            return self.registry(force);
        };
        let catalog = self.source_catalog(id, force)?;
        let entries: Vec<_> = catalog
            .entries
            .into_iter()
            .map(|mut e| {
                e.metadata["download_url"] = format!(
                    "{}?source_id={id}",
                    e.metadata["download_url"].as_str().unwrap()
                )
                .into();
                e.metadata
            })
            .collect();
        Ok(
            json!({"schema_version":2,"market_status":{"source":catalog.source,"warning":catalog.warning},"plugins":entries}),
        )
    }
    pub(crate) fn archive_for(
        &self,
        source: Option<&str>,
        id: &str,
        version: &str,
    ) -> Result<Vec<u8>, String> {
        let Some(source) = source else {
            return self.archive(id, version);
        };
        let catalog = self.source_catalog(source, false)?;
        let entry = catalog
            .entries
            .iter()
            .find(|e| e.metadata["id"] == id && e.metadata["version"] == version)
            .ok_or("当前来源目录已变化，请刷新")?;
        let bytes = match &entry.origin {
            Origin::Bundled(path) => read_limited(
                fs::File::open(path).map_err(|e| e.to_string())?,
                MAX_ARCHIVE,
            )?,
            Origin::Remote(url) => PublicClient::default()
                .fetch(url, MAX_ARCHIVE)
                .map_err(|e| e.to_string())?,
        };
        verify_archive(
            &bytes,
            entry.metadata["size"].as_u64().unwrap(),
            entry.metadata["sha256"].as_str().unwrap(),
        )?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Explicit public-source read acceptance; set GAMER_TEST_PLUGIN_SOURCE"]
    fn configured_plugin_source_live_acceptance() {
        let input = std::env::var("GAMER_TEST_PLUGIN_SOURCE").expect("explicit source required");
        let directory = tempfile::tempdir().unwrap();
        let market =
            PluginMarket::with_data(directory.path().join("web"), directory.path().join("data"));
        let canonical = SourceLocation::parse_for(&input, "registry.json")
            .unwrap()
            .canonical();
        let sources = market.save_source(&input, true, None, None).unwrap();
        let source = sources.iter().find(|s| s.repository == canonical).unwrap();
        let catalog = market.registry_for(Some(&source.id), true).unwrap();
        let entries = catalog["plugins"].as_array().unwrap();
        assert!(!entries.is_empty());
        for entry in entries {
            let bytes = market
                .archive_for(
                    Some(&source.id),
                    entry["id"].as_str().unwrap(),
                    entry["version"].as_str().unwrap(),
                )
                .unwrap();
            verify_archive(
                &bytes,
                entry["size"].as_u64().unwrap(),
                entry["sha256"].as_str().unwrap(),
            )
            .unwrap();
        }
        println!(
            "{}",
            json!({"source":canonical,"verified_archives":entries.len(),"installed":false,"production_modified":false})
        );
    }
    #[test]
    fn gitea_preset_downloads_through_host_and_does_not_follow_upstream_registry_urls() {
        let server = crate::package_market::source::test_http::TestServer::new();
        let directory = tempfile::tempdir().unwrap();
        let bytes = b"new";
        let hash = format!("{:x}", Sha256::digest(bytes));
        let registry = json!({"schema_version":2,"plugins":[{"id":"gamer-example","version":"1.0.0","sha256":hash,"size":3,"download_url":"https://github.com/upstream/should-not-be-used.gplugin","execution":{"kind":"wasm"}}]});
        let registry = serde_json::to_vec(&registry).unwrap();
        let release = json!({"id":1,"draft":false,"prerelease":false,"tag_name":"gamer-example-v1.0.0","published_at":"2026-10-28T00:00:00Z","assets":[
            {"name":"registry.json","size":registry.len(),"browser_download_url":format!("{}/attachments/catalog",server.base)},
            {"name":"gamer-example-1.0.0.gplugin","size":3,"browser_download_url":format!("{}/attachments/plugin",server.base)}]});
        server.put(
            "/api/v1/repos/owner/repo/releases?limit=100",
            &serde_json::to_vec(&json!([release])).unwrap(),
        );
        server.put("/attachments/catalog", &registry);
        server.put("/attachments/plugin", bytes);
        let market =
            PluginMarket::with_data(directory.path().join("web"), directory.path().join("data"));
        let presets = market
            .save_source(
                &format!("{}/owner/repo.git", server.base),
                true,
                Some("测试内网"),
                None,
            )
            .unwrap();
        let id = &presets.iter().find(|s| s.name == "测试内网").unwrap().id;
        let catalog = market.registry_for(Some(id), true).unwrap();
        assert!(catalog["plugins"][0]["download_url"]
            .as_str()
            .unwrap()
            .starts_with("/api/extensions/market/"));
        assert!(catalog["plugins"][0]["download_url"]
            .as_str()
            .unwrap()
            .contains(id));
        assert_eq!(
            market
                .archive_for(Some(id), "gamer-example", "1.0.0")
                .unwrap(),
            bytes
        );
        server.put("/attachments/plugin", b"bad");
        assert!(market
            .archive_for(Some(id), "gamer-example", "1.0.0")
            .is_err());
        assert_eq!(server.calls.lock().unwrap().len(), 4);
    }
}
