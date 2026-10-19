//! Administrator-owned runtime dependencies. Never writes system installations.
mod install;
mod probe;
#[cfg(test)]
mod tests;

use crate::config::Config;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(crate) const IDS: &[&str] = &[
    "adb",
    "ffmpeg",
    "browser",
    "tesseract",
    "tessdata",
    "scrcpy-server",
];

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    #[serde(default)]
    revision: String,
    #[serde(default)]
    paths: BTreeMap<String, String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct Dependency {
    pub id: String,
    pub status: String,
    pub source: String,
    pub version: Option<String>,
    pub path: Option<PathBuf>,
    pub configured_path: String,
    pub detail: String,
    pub auto_install: bool,
    pub restart_required: bool,
}

pub(crate) fn root(cfg: &Config) -> PathBuf {
    std::env::var_os("GAMER_INSTALL_ROOT")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| cfg.data_dir.parent().unwrap_or(&cfg.data_dir).to_owned())
        .join("runtime")
}
fn preferences_path(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("state/runtime-dependencies.json")
}
fn preferences(cfg: &Config) -> Result<Preferences> {
    match std::fs::read(preferences_path(cfg)) {
        Ok(bytes) => {
            ensure!(bytes.len() <= 32 * 1024, "运行依赖配置过大");
            Ok(serde_json::from_slice(&bytes).context("运行依赖配置损坏，请恢复配置备份")?)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Preferences::default()),
        Err(e) => Err(e.into()),
    }
}
fn ocr_path(cfg: &Config) -> Result<PathBuf> {
    let target = cfg.data_dir.join("state/ocr/settings.dat");
    crate::ocr::settings::migrate(&cfg.data_dir, &target)?;
    Ok(target)
}
pub(crate) fn selection(cfg: &Config, id: &str) -> Result<String> {
    ensure!(IDS.contains(&id), "未知运行依赖");
    let prefs = preferences(cfg)?;
    if matches!(id, "tesseract" | "tessdata") {
        let saved = crate::ocr::settings::Settings::load(&ocr_path(cfg)?)?;
        return Ok(if id == "tesseract" {
            saved.engine_path
        } else {
            saved.tessdata_path
        });
    }
    if let Some(path) = prefs.paths.get(id) {
        return Ok(path.clone());
    }
    if let Some(path) = cfg.runtime_selections.get(id) {
        return Ok(path.clone());
    }
    Ok(match id {
        "adb" if cfg.adb_path != "adb" => cfg.adb_path.clone(),
        "ffmpeg" if cfg.ffmpeg_path != "ffmpeg" => cfg.ffmpeg_path.clone(),
        "browser" => cfg.browser_path.clone(),
        "scrcpy-server" => cfg.scrcpy_server.to_string_lossy().into_owned(),
        _ => String::new(),
    })
}

pub(crate) async fn view(cfg: &Config) -> Result<serde_json::Value> {
    let mut dependencies = Vec::new();
    // Bounded, serial probes. No browser launches, ADB server starts or network.
    for id in IDS {
        dependencies.push(probe::dependency(cfg, id).await?);
    }
    Ok(serde_json::json!({"revision":preferences(cfg)?.revision,"dependencies":dependencies}))
}

pub(crate) async fn inspect(cfg: &Config, id: &str) -> Result<Dependency> {
    probe::dependency(cfg, id).await
}

pub(crate) async fn prepare(cfg: &Config, id: &str) -> Result<Dependency> {
    ensure!(IDS.contains(&id), "未知运行依赖");
    let gate = install::GATE.lock().await;
    let existing = inspect(cfg, id).await?;
    if existing.status == "ready" {
        return Ok(existing);
    }
    ensure!(
        existing.configured_path.is_empty(),
        "指定的依赖未就绪：{}。请修正路径或切换为自动检测",
        existing.detail
    );
    ensure!(
        existing.auto_install,
        "当前平台没有受验证的自动安装源：{}；请指定已有工具，或使用包含依赖的 Docker 镜像",
        id
    );
    install::install(cfg, id).await?;
    let next = inspect(cfg, id).await?;
    ensure!(next.status == "ready", "安装后验证失败：{}", next.detail);
    drop(gate);
    Ok(next)
}

pub(crate) async fn save(cfg: &Config, id: &str, path: &str, revision: &str) -> Result<()> {
    ensure!(
        IDS.contains(&id) && id != "scrcpy-server",
        "该依赖由应用版本固定管理"
    );
    ensure!(path.len() <= 4096 && !path.contains('\0'), "路径无效");
    let _gate = install::GATE.lock().await;
    let mut prefs = preferences(cfg)?;
    ensure!(prefs.revision == revision, "运行依赖配置已变化，请重新读取");
    let path = path.trim();
    if !path.is_empty() {
        crate::ocr::settings::checked_path(path, &cfg.data_dir, id == "tessdata")?;
    }
    if matches!(id, "tesseract" | "tessdata") {
        let target = ocr_path(cfg)?;
        let mut saved = crate::ocr::settings::Settings::load(&target)?;
        if id == "tesseract" {
            saved.engine_path = path.into();
        } else {
            saved.tessdata_path = path.into();
        }
        saved.write(&target)?;
    } else {
        prefs.paths.insert(id.into(), path.into());
    }
    prefs.revision = uuid::Uuid::new_v4().to_string();
    crate::core::fs::atomic_write(&preferences_path(cfg), &serde_json::to_vec_pretty(&prefs)?)?;
    Ok(())
}

/// Only launch-time bindings change here; existing tasks/processes keep paths.
/// Failed optional preparation remains visible in Settings and does not destroy data.
pub(crate) async fn prepare_startup(cfg: &mut Config) {
    let original = cfg.clone();
    for id in ["adb", "ffmpeg", "browser"] {
        if let Ok(selection) = selection(&original, id) {
            cfg.runtime_selections.insert(id.into(), selection);
        }
        if let Ok(dep) = inspect(&original, id).await {
            let path = if dep.status == "ready" {
                dep.path
            } else if dep.configured_path.is_empty() && dep.auto_install {
                managed(&original, id).ok()
            } else {
                None
            };
            if let Some(path) = path {
                let path = path.to_string_lossy().into_owned();
                match id {
                    "adb" => cfg.adb_path = path,
                    "ffmpeg" => cfg.ffmpeg_path = path,
                    "browser" => cfg.browser_path = path,
                    _ => (),
                }
            }
        }
    }
    // Downloads never block the HTTP listener. Bind planned managed paths now;
    // connections begun after installation use them without rebuilding services.
    tokio::spawn(async move {
        for id in ["adb", "ffmpeg", "browser", "tesseract", "tessdata"] {
            if let Err(error) = prepare(&original, id).await {
                tracing::warn!(dependency=id,%error,"运行依赖尚未就绪，可在设置中修复");
            }
        }
    });
}

pub(crate) fn managed(cfg: &Config, id: &str) -> Result<PathBuf> {
    let artifact = install::artifact(id)?;
    let base = root(cfg).join(id).join(&artifact.version);
    Ok(if id == "tessdata" {
        base
    } else {
        base.join(&artifact.executable)
    })
}
pub(crate) fn executable(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}
pub(crate) fn path_candidates(name: &str) -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|value| {
            std::env::split_paths(&value)
                .map(|p| p.join(executable(name)))
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn engine_candidates(cfg: &Config, id: &str) -> Result<Vec<PathBuf>> {
    let explicit = selection(cfg, id)?;
    if !explicit.is_empty() {
        return Ok(vec![crate::ocr::settings::checked_path(
            &explicit,
            &cfg.data_dir,
            false,
        )?]);
    }
    let mut candidates = if id == "browser" {
        crate::browser::browser_executable(&Config {
            browser_path: String::new(),
            ..cfg.clone()
        })
        .ok()
        .into_iter()
        .collect()
    } else {
        path_candidates(id)
    };
    let legacy = cfg.data_dir.parent().unwrap_or(&cfg.data_dir).join("deps");
    candidates.push(if id == "tesseract" {
        legacy.join("tesseract").join(executable(id))
    } else {
        legacy.join(executable(id))
    });
    if let Ok(path) = managed(cfg, id) {
        candidates.push(path);
    }
    Ok(candidates
        .into_iter()
        .filter(|p| {
            p.is_absolute()
                && p.is_file()
                && crate::ocr::settings::checked_path(&p.to_string_lossy(), &cfg.data_dir, false)
                    .is_ok()
        })
        .collect())
}
pub(crate) fn existing_engine(cfg: &Config, id: &str) -> Result<Option<PathBuf>> {
    Ok(engine_candidates(cfg, id)?.into_iter().next())
}

pub(crate) fn model_directory(cfg: &Config, engine: Option<&Path>) -> Result<Option<PathBuf>> {
    let selected = selection(cfg, "tessdata")?;
    if !selected.is_empty() {
        return Ok(Some(crate::ocr::settings::checked_path(
            &selected,
            &cfg.data_dir,
            true,
        )?));
    }
    let mut candidates = Vec::new();
    if let Some(env) = std::env::var_os("TESSDATA_PREFIX") {
        candidates.push(PathBuf::from(env));
    }
    if let Some(parent) = engine.and_then(Path::parent) {
        candidates.push(parent.join("tessdata"));
    }
    for path in [
        "/usr/share/tesseract-ocr/5/tessdata",
        "/usr/share/tessdata",
        "/usr/local/share/tessdata",
        "/opt/homebrew/share/tessdata",
    ] {
        candidates.push(path.into());
    }
    if let Ok(path) = managed(cfg, "tessdata") {
        candidates.push(path);
    }
    // A single directory supplies every requested language. Never mutate a
    // user's incomplete tessdata, and never mix arbitrary per-language paths.
    Ok(candidates.into_iter().find(|p| {
        p.is_absolute()
            && ["eng", "chi_sim"]
                .iter()
                .all(|l| p.join(format!("{l}.traineddata")).is_file())
    }))
}

fn missing(message: impl std::fmt::Display) -> String {
    message.to_string()
}
