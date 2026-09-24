//! 首次可选插件：从已校验的发行组件读取权限，经 Core 安装 API 写入。
use crate::{layout::InstallLayout, manifest::model::Manifest};
use serde::Deserialize;
use std::{fs, io::Read, path::PathBuf, time::Duration};
#[derive(Clone, Deserialize)]
pub struct Choice {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(skip)]
    pub selected: bool,
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(skip)]
    pub hash: String,
    #[serde(skip)]
    pub size: u64,
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
        choice.selected = true;
        choices.push(choice);
    }
    Ok(choices)
}
pub fn install(
    layout: &InstallLayout,
    choices: &[Choice],
    selected: &[String],
    token: &str,
) -> Result<(), String> {
    let port = crate::supervisor::read_configured_port(&layout.config_file());
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .build();
    for choice in choices.iter().filter(|c| selected.contains(&c.id)) {
        crate::digest::verify_file(&choice.path, &choice.hash, choice.size)
            .map_err(|e| e.to_string())?;
        let bytes = fs::read(&choice.path).map_err(|e| e.to_string())?;
        let response = agent
            .post(&format!("http://127.0.0.1:{port}/api/extensions"))
            .set("X-Admin-Token", token)
            .set("Content-Type", "application/zip")
            .set("X-Gamer-Permission-Confirm", "1")
            .set("X-Gamer-Extension-Source", "official")
            .set("X-Expected-Sha256", &choice.hash)
            .send_bytes(&bytes);
        match response {
            Ok(_) => {}
            // 重试时保留已经装好的插件；409 只有核对已安装的 id+version 后才接受。
            Err(ureq::Error::Status(409, _)) => {
                let resp = agent
                    .get(&format!("http://127.0.0.1:{port}/api/extensions"))
                    .set("X-Admin-Token", token)
                    .call()
                    .map_err(|e| e.to_string())?;
                let mut raw = String::new();
                resp.into_reader()
                    .take(4 * 1024 * 1024)
                    .read_to_string(&mut raw)
                    .map_err(|e| e.to_string())?;
                let value: serde_json::Value =
                    serde_json::from_str(&raw).map_err(|e| e.to_string())?;
                let list = value
                    .as_array()
                    .or_else(|| value.get("extensions").and_then(|v| v.as_array()));
                if !list.is_some_and(|list| {
                    list.iter()
                        .any(|p| p["id"] == choice.id && p["active_version"] == choice.version)
                }) {
                    return Err(format!("{} 已有不同版本，请在插件页处理", choice.name));
                }
            }
            Err(e) => return Err(format!("{} 安装失败：{e}", choice.name)),
        }
    }
    crate::state::atomic::write_json_atomic(
        &layout.state_dir().join("plugins-choice.json"),
        selected,
    )
    .map_err(|e| e.to_string())
}
