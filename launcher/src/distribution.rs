//! 在线/离线发行发现。HTTPS 提供清单，安装前校验文件大小与 SHA256。
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

pub fn read(_layout: &InstallLayout, path: &Path) -> Result<Manifest, String> {
    let check = crate::manifest::validate_manifest_file(path, &ValidateOptions::default());
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
        .set("User-Agent", "Gamer-Launcher")
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
    let source = match std::env::var("GAMER_LAUNCHER_RELEASE_MANIFEST") {
        Ok(source) => source,
        Err(_) if env!("CARGO_PKG_VERSION").contains('-') => {
            let bytes = fetch(
                "https://api.github.com/repos/jesongit/gamer/releases?per_page=100",
                Instant::now() + Duration::from_secs(10),
                3 * 1024 * 1024,
            )?;
            select_beta_manifest(&bytes)?
        }
        Err(_) => RELEASE_URL.into(),
    };
    if !source.starts_with("https://") && !source.starts_with("http://") {
        let path = PathBuf::from(source);
        let model = read(layout, &path)?;
        return cache(layout, &path, model);
    }
    let path = download_manifest(layout, &source, Duration::from_secs(3))?;
    let model = read(layout, &path)?;
    cache(layout, &path, model)
}

// GitHub latest excludes prereleases. Beta launchers choose the greatest SemVer
// with a published manifest; stable launchers keep using the stable endpoint.
fn select_beta_manifest(bytes: &[u8]) -> Result<String, String> {
    let releases: Vec<serde_json::Value> =
        serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    releases
        .iter()
        .filter(|r| r["draft"].as_bool() == Some(false))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            let version = tag.strip_prefix('v')?;
            crate::manifest::semver::parse(version)?;
            let expected = format!(
                "https://github.com/jesongit/gamer/releases/download/{tag}/gamer-release.json"
            );
            r["assets"].as_array()?.iter().find(|a| {
                a["name"] == "gamer-release.json" && a["browser_download_url"] == expected
            })?;
            Some((version, expected))
        })
        .max_by(|(a, _), (b, _)| compare_versions(a, b))
        .map(|(_, url)| url)
        .ok_or_else(|| "尚无可用的 Gamer 测试版发行清单".into())
}
pub(crate) fn download_manifest(
    layout: &InstallLayout,
    url: &str,
    timeout: Duration,
) -> Result<PathBuf, String> {
    validate_manifest_url(url)?;
    let deadline = Instant::now() + timeout;
    let raw = fetch(url, deadline, 3 * 1024 * 1024)?;
    let staging = layout.staging_dir().join("discovery");
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let path = staging.join("manifest.json");
    fs::write(&path, raw).map_err(|e| e.to_string())?;
    Ok(path)
}

fn validate_manifest_url(url: &str) -> Result<(), String> {
    if url.starts_with("https://") {
        return Ok(());
    }
    // 显式指定的 loopback HTTP 仅用于本机发行演练；远端必须 HTTPS。
    if let Some(rest) = url.strip_prefix("http://") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let loopback = authority
            .parse::<std::net::SocketAddr>()
            .map(|addr| addr.ip().is_loopback())
            .unwrap_or_else(|_| {
                authority
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
            });
        if loopback {
            return Ok(());
        }
    }
    Err("远程发行清单必须使用 HTTPS".into())
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
        let raw = fs::read(source).map_err(|e| e.to_string())?;
        crate::state::atomic::write_bytes_atomic(&dest, &raw).map_err(|e| e.to_string())?;
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

#[cfg(test)]
mod url_tests {
    use super::validate_manifest_url;

    #[test]
    fn beta_discovery_uses_semver_and_ignores_drafts_and_missing_assets() {
        let release = |v: &str, draft: bool| {
            serde_json::json!({
                "tag_name": format!("v{v}"), "draft": draft,
                "assets": [{ "name": "gamer-release.json", "browser_download_url":
                    format!("https://github.com/jesongit/gamer/releases/download/v{v}/gamer-release.json") }]
            })
        };
        let mut releases = vec![
            release("0.2.0-beta.2", false),
            release("0.2.0-beta.10", false),
            release("0.2.0", true),
            release("0.1.1", false),
        ];
        assert!(
            super::select_beta_manifest(&serde_json::to_vec(&releases).unwrap())
                .unwrap()
                .contains("v0.2.0-beta.10/")
        );
        releases.push(release("0.2.0", false));
        assert!(
            super::select_beta_manifest(&serde_json::to_vec(&releases).unwrap())
                .unwrap()
                .contains("v0.2.0/")
        );
        releases[4]["assets"][0]["browser_download_url"] =
            serde_json::json!("https://example.com/fake.json");
        assert!(
            super::select_beta_manifest(&serde_json::to_vec(&releases).unwrap())
                .unwrap()
                .contains("v0.2.0-beta.10/")
        );
        assert!(super::select_beta_manifest(b"[]").is_err());
    }

    #[test]
    fn remote_manifest_requires_https_but_local_test_server_is_allowed() {
        for url in [
            super::RELEASE_URL,
            "http://127.0.0.1:8000/release.json",
            "http://[::1]:8000/release.json",
        ] {
            assert!(validate_manifest_url(url).is_ok(), "{url}");
        }
        for url in [
            "http://example.com/release.json",
            "http://127.0.0.1.evil:80/release.json",
            "http://127.0.0.1@evil/release.json",
            "ftp://example.com/release.json",
        ] {
            assert!(validate_manifest_url(url).is_err(), "{url}");
        }
    }
}
