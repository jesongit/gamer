//! Shared, persistent URL presets. Selecting a preset remains a browser preference.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, sync::Mutex};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Source {
    pub id: String,
    pub repository: String,
    pub enabled: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub builtin: bool,
}

pub(crate) struct SourceStore {
    path: PathBuf,
    kind: &'static str,
    lock: Mutex<()>,
}

impl SourceStore {
    pub(crate) fn new(data: PathBuf, kind: &'static str) -> Self {
        Self {
            path: data.join(format!("{kind}-sources.json")),
            kind,
            lock: Mutex::new(()),
        }
    }
    fn defaults(&self) -> Vec<Source> {
        [
            (
                "GitHub",
                format!(
                    "jesongit/gamer-{}",
                    if self.kind == "plugin" {
                        "plugins"
                    } else {
                        "packages"
                    }
                ),
            ),
            (
                "内网",
                format!(
                    "http://192.168.31.10:13000/posase/gamer-{}",
                    if self.kind == "plugin" {
                        "plugins"
                    } else {
                        "packages"
                    }
                ),
            ),
        ]
        .into_iter()
        .map(|(name, repository)| Source {
            id: crate::package_archive::sha256_hex(repository.as_bytes())[..16].into(),
            repository,
            name: name.into(),
            enabled: true,
            builtin: true,
        })
        .collect()
    }
    fn load(&self) -> Result<Vec<Source>> {
        let mut sources: Vec<Source> = match fs::read(&self.path) {
            Ok(bytes) => {
                ensure!(bytes.len() < 65536, "来源预设设置超限");
                serde_json::from_slice(&bytes)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let defaults = self.defaults();
        for source in &mut sources {
            source.builtin = defaults
                .iter()
                .any(|d| d.id == source.id && d.repository == source.repository);
            if source.builtin {
                source.name = defaults
                    .iter()
                    .find(|d| d.id == source.id)
                    .unwrap()
                    .name
                    .clone();
            } else if source.name.is_empty() {
                source.name = source.repository.clone();
            }
        }
        for (index, source) in defaults.into_iter().enumerate() {
            if !sources.iter().any(|s| s.id == source.id) {
                sources.insert(index.min(sources.len()), source);
            }
        }
        Ok(sources)
    }
    pub(crate) fn list(&self) -> Result<Vec<Source>> {
        let _guard = self.lock.lock().unwrap();
        self.load()
    }
    pub(crate) fn save(
        &self,
        repository: String,
        enabled: bool,
        name: Option<&str>,
        id: Option<&str>,
    ) -> Result<Vec<Source>> {
        let name = name.map(str::trim);
        ensure!(
            name.is_none_or(|n| !n.is_empty()
                && n.chars().count() <= 80
                && !n.chars().any(char::is_control)),
            "预设名称须为 1–80 个可见字符"
        );
        let _guard = self.lock.lock().unwrap();
        let mut sources = self.load()?;
        if let Some(name) = name {
            ensure!(
                !sources.iter().any(|s| s.name == name
                    && Some(s.id.as_str()) != id
                    && s.repository != repository),
                "同名预设已存在，请使用不同名称"
            );
        }
        if let Some(id) = id {
            let index = sources
                .iter()
                .position(|s| s.id == id)
                .context("来源预设不存在，请刷新")?;
            ensure!(
                !sources[index].builtin,
                "内置来源不可修改，可另存为自定义预设"
            );
            ensure!(
                !sources
                    .iter()
                    .any(|s| s.id != id && s.repository == repository),
                "该地址已存在于来源预设"
            );
            sources[index].repository = repository;
            sources[index].name = name.unwrap_or(&sources[index].name).to_owned();
            sources[index].enabled = enabled;
        } else if let Some(source) = sources.iter_mut().find(|s| s.repository == repository) {
            ensure!(
                !source.builtin || name.is_none(),
                "该地址已是内置来源，请直接从下拉框选择"
            );
            source.enabled = enabled;
            if !source.builtin {
                if let Some(name) = name {
                    source.name = name.into();
                }
            }
        } else {
            ensure!(sources.len() < 32, "最多保存 32 个来源预设");
            sources.push(Source {
                id: uuid::Uuid::new_v4().simple().to_string(),
                name: name.unwrap_or(&repository).into(),
                repository,
                enabled,
                builtin: false,
            });
        }
        self.write(&sources)?;
        Ok(sources)
    }
    fn write(&self, sources: &[Source]) -> Result<()> {
        fs::create_dir_all(self.path.parent().unwrap())?;
        crate::core::fs::atomic_write(&self.path, &serde_json::to_vec_pretty(sources)?)?;
        Ok(())
    }
    pub(crate) fn remove(&self, id: &str) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        let mut sources = self.load()?;
        let source = sources
            .iter()
            .find(|s| s.id == id)
            .context("来源预设不存在")?;
        ensure!(!source.builtin, "内置来源不可删除");
        sources.retain(|s| s.id != id);
        self.write(&sources)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_sources_are_preserved_and_custom_presets_survive_restart_and_edit() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package-sources.json"),
            br#"[{"id":"old","repository":"owner/repo","enabled":false}]"#,
        )
        .unwrap();
        let store = SourceStore::new(dir.path().into(), "package");
        let sources = store.list().unwrap();
        assert_eq!(sources.iter().filter(|s| s.builtin).count(), 2);
        assert!(!sources.iter().find(|s| s.id == "old").unwrap().enabled);
        let saved = store
            .save(
                "https://source.example/packages.json".into(),
                true,
                Some("我的目录"),
                None,
            )
            .unwrap();
        let id = saved
            .iter()
            .find(|s| s.name == "我的目录")
            .unwrap()
            .id
            .clone();
        assert!(store
            .save(
                "https://different.example/plugins.json".into(),
                true,
                Some("我的目录"),
                None
            )
            .is_err());
        let restarted = SourceStore::new(dir.path().into(), "package");
        restarted
            .save(
                "https://source.example/next.json".into(),
                true,
                Some("新目录"),
                Some(&id),
            )
            .unwrap();
        assert_eq!(
            restarted
                .list()
                .unwrap()
                .iter()
                .find(|s| s.id == id)
                .unwrap()
                .name,
            "新目录"
        );
        restarted.remove(&id).unwrap();
        assert!(!restarted.list().unwrap().iter().any(|s| s.id == id));
        assert!(restarted.remove(&sources[0].id).is_err());
        assert!(restarted
            .save("https://source.example/x".into(), true, Some(" "), None)
            .is_err());
    }
}
