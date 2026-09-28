//! Release-pinned plugin metadata and permission preflight; installation belongs to Core.
use crate::{layout::InstallLayout, manifest::model::Manifest};
use serde::Deserialize;
use std::{fs, io::Read, path::PathBuf};
#[derive(Clone, Deserialize)]
pub struct Choice {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(skip)]
    pub hash: String,
    #[serde(skip)]
    pub size: u64,
}

/// Before shutdown, reject permission expansion. The plugin page owns explicit new grants.
pub fn preflight_update(layout: &InstallLayout, model: &Manifest) -> Result<(), String> {
    let state_path = layout.data_dir().join("extensions/state.json");
    if !state_path.exists() {
        return Ok(());
    }
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(state_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    for choice in choices(layout, model)? {
        let Some(version) = state["plugins"][&choice.id]["active_version"].as_str() else {
            continue;
        };
        if choice.id.contains('/')
            || crate::manifest::pathsafe::check_single_path(&choice.id).is_some()
            || semver::Version::parse(version).is_err()
        {
            return Err("非法插件 id".into());
        }
        if crate::distribution::compare_versions(&choice.version, version)
            != std::cmp::Ordering::Greater
        {
            continue;
        }
        let old_path = layout
            .data_dir()
            .join("extensions")
            .join(&choice.id)
            .join(version)
            .join("manifest.toml");
        let old: Choice = toml::from_str(&fs::read_to_string(old_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let added: Vec<_> = choice
            .permissions
            .iter()
            .filter(|p| !old.permissions.contains(p))
            .cloned()
            .collect();
        if !added.is_empty() {
            return Err(format!(
                "插件 {} 新增权限 {}，请先在插件页确认更新",
                choice.name,
                added.join("、")
            ));
        }
    }
    Ok(())
}
pub fn choices(layout: &InstallLayout, model: &Manifest) -> Result<Vec<Choice>, String> {
    let Some(component) = model
        .platforms
        .get(crate::distribution::PLATFORM)
        .and_then(|p| p.components.iter().find(|c| c.id == "official-plugins"))
    else {
        return Ok(Vec::new());
    };
    let dir = layout
        .runtime_dir()
        .join(&component.id)
        .join(&component.version);
    let mut choices = Vec::new();
    for file in &component.required_files {
        if !file.path.ends_with(".gplugin") {
            continue;
        }
        let path = dir.join(&file.path);
        crate::verification::verify(layout, &path, &file.sha256, file.size as u64)
            .map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(fs::File::open(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let entry = zip.by_name("manifest.toml").map_err(|e| e.to_string())?;
        let mut raw = String::new();
        entry
            .take(1024 * 1024 + 1)
            .read_to_string(&mut raw)
            .map_err(|e| e.to_string())?;
        if raw.len() > 1024 * 1024 {
            return Err("插件说明超过大小上限".into());
        }
        let mut choice: Choice = toml::from_str(&raw).map_err(|e| e.to_string())?;
        choice.path = path;
        choice.hash = file.sha256.clone();
        choice.size = file.size as u64;
        choices.push(choice);
    }
    Ok(choices)
}
