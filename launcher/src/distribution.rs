//! 在线/离线发行发现。网络仅提供候选，签名与本机信任库决定能否安装。
use crate::{
    layout::InstallLayout,
    manifest::{model::Manifest, ValidateOptions},
};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const RELEASE_URL: &str =
    "https://github.com/jesongit/gamer/releases/latest/download/gamer-release.json";
pub const PLATFORM: &str = "windows-x86_64";

pub fn keys(layout: &InstallLayout) -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("GAMER_LAUNCHER_KEYS_DIR") {
        if !path.trim().is_empty() {
            return Ok(path.into());
        }
    }
    let dir = layout.root.join("keys");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // 固定内置信任锚，首次单 EXE 运行不依赖额外下载公钥。
    let key = dir.join("prod-ed25519-1.pem");
    let pem = include_bytes!("../../release/keys/prod-ed25519-1.pem");
    if fs::read(&key).ok().as_deref() != Some(pem.as_slice()) {
        fs::write(key, pem).map_err(|e| e.to_string())?;
    }
    Ok(dir)
}
pub fn read(layout: &InstallLayout, path: &Path) -> Result<Manifest, String> {
    let check = crate::manifest::validate_manifest_file(
        path,
        &ValidateOptions {
            keys_dir: Some(keys(layout)?),
            // 先验证后展示；最低启动器版本在执行安装前单独拦截，以便显示更新说明。
            ..Default::default()
        },
    );
    if !check.ok {
        return Err(check
            .errors
            .iter()
            .map(|e| format!("{}: {}", e.code, e.detail))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let raw = fs::read(path).map_err(|e| e.to_string())?;
    Manifest::parse(&serde_json::from_slice(&raw).map_err(|e| e.to_string())?)
}
pub fn current(layout: &InstallLayout) -> Option<String> {
    match crate::state::StateStore::new(&layout.root)
        .load_current()
        .ok()?
    {
        crate::state::atomic::LoadOutcome::Present(c) => Some(c.current),
        _ => None,
    }
}
pub fn cached(layout: &InstallLayout, version: Option<&str>) -> Option<(PathBuf, Manifest)> {
    crate::commands::cached_manifest_candidates(layout)
        .into_iter()
        .filter_map(|p| read(layout, &p).ok().map(|m| (p, m)))
        .filter(|(_, m)| version.is_none_or(|v| m.release.version == v))
        .max_by(|(_, a), (_, b)| compare_versions(&a.release.version, &b.release.version))
}
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    match (
        crate::manifest::semver::parse(a),
        crate::manifest::semver::parse(b),
    ) {
        (Some(a), Some(b)) => {
            if crate::manifest::semver::is_lt(&a, &b) {
                std::cmp::Ordering::Less
            } else if crate::manifest::semver::is_lt(&b, &a) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        }
        _ => a.cmp(b),
    }
}
pub fn signature_url(url: &str) -> String {
    url.strip_suffix(".json")
        .map_or_else(|| format!("{url}.sig"), |s| format!("{s}.sig"))
}
fn fetch(url: &str, deadline: Instant, limit: u64) -> Result<Vec<u8>, String> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("检查更新超时".into());
    }
    let opts = crate::fetch::FetchOptions {
        connect_timeout: remaining,
        read_timeout: remaining,
        overall_timeout: remaining,
        ..Default::default()
    };
    let response = crate::fetch::build_agent(url, &opts)
        .get(url)
        .call()
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("发行清单超过大小上限".into());
    }
    Ok(bytes)
}
pub fn discover(layout: &InstallLayout) -> Result<(PathBuf, Manifest), String> {
    let source =
        std::env::var("GAMER_LAUNCHER_RELEASE_MANIFEST").unwrap_or_else(|_| RELEASE_URL.into());
    if !source.starts_with("https://") && !source.starts_with("http://") {
        let path = PathBuf::from(source);
        let model = read(layout, &path)?;
        return cache(layout, &path, model);
    }
    let path = download_manifest(layout, &source, Duration::from_secs(3))?;
    let model = read(layout, &path)?;
    cache(layout, &path, model)
}
pub(crate) fn download_manifest(
    layout: &InstallLayout,
    url: &str,
    timeout: Duration,
) -> Result<PathBuf, String> {
    let deadline = Instant::now() + timeout;
    let raw = fetch(url, deadline, 3 * 1024 * 1024)?;
    let sig = fetch(&signature_url(url), deadline, 16 * 1024)?;
    let staging = layout.staging_dir().join("discovery");
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let path = staging.join("manifest.json");
    fs::write(&path, raw).map_err(|e| e.to_string())?;
    fs::write(path.with_extension("sig"), sig).map_err(|e| e.to_string())?;
    Ok(path)
}

fn cache(
    layout: &InstallLayout,
    source: &Path,
    model: Manifest,
) -> Result<(PathBuf, Manifest), String> {
    fs::create_dir_all(layout.manifests_dir()).map_err(|e| e.to_string())?;
    let dest = layout
        .manifests_dir()
        .join(format!("{}.json", model.release.version));
    if source != dest {
        let signature = fs::read(source.with_extension("sig")).map_err(|e| e.to_string())?;
        let raw = fs::read(source).map_err(|e| e.to_string())?;
        crate::state::atomic::write_bytes_atomic(&dest.with_extension("sig"), &signature)
            .and_then(|_| crate::state::atomic::write_bytes_atomic(&dest, &raw))
            .map_err(|e| e.to_string())?;
    }
    Ok((dest, model))
}
pub fn plan(
    layout: &InstallLayout,
    manifest: &Manifest,
) -> Result<crate::supervisor::LaunchPlan, String> {
    let platform = manifest
        .platforms
        .get(PLATFORM)
        .ok_or("发行包不支持当前平台")?;
    let app_dir = layout.versions_dir().join(&manifest.release.version);
    if let Some(scrcpy) = platform.components.iter().find(|c| c.id == "scrcpy-server") {
        if scrcpy.version != platform.resources.scrcpy_server.version
            || !scrcpy.required_files.iter().any(|f| {
                f.path == "scrcpy-server.jar" && f.sha256 == platform.resources.scrcpy_server.sha256
            })
        {
            return Err("scrcpy 组件与本体声明的协议版本或哈希不一致".into());
        }
    }
    let component = |id: &str, name: &str| {
        platform
            .components
            .iter()
            .find(|c| c.id == id)
            .map(|c| layout.runtime_dir().join(id).join(&c.version).join(name))
    };
    Ok(crate::supervisor::LaunchPlan {
        exe: app_dir.join(&platform.app.entrypoint),
        cwd: app_dir.clone(),
        app_dir: app_dir.clone(),
        data_dir: layout.data_dir(),
        adb_path: component("adb", "adb.exe"),
        ffmpeg_path: component("ffmpeg", "ffmpeg.exe"),
        scrcpy_server: component("scrcpy-server", "scrcpy-server.jar")
            .unwrap_or_else(|| app_dir.join(&platform.resources.scrcpy_server.path)),
        config_path: layout.config_file(),
        log_path: layout.logs_dir().join("gamer-server.log"),
    })
}
