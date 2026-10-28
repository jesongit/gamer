use super::*;
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
};
pub(super) static GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Artifact {
    pub version: String,
    #[serde(default)]
    pub executable: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub files: Vec<RequiredFile>,
}
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct RequiredFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}
pub(super) fn supported(id: &str) -> bool {
    id == "tessdata"
        || (cfg!(all(windows, target_arch = "x86_64"))
            && matches!(id, "adb" | "ffmpeg" | "tesseract" | "browser"))
        || (cfg!(all(target_os = "linux", target_arch = "x86_64")) && id == "browser")
}
pub(super) fn artifact(id: &str) -> Result<Artifact> {
    if id == "tessdata" {
        return Ok(Artifact {
            version: "fast-87416418657359cb625c412a48b6e1d6d41c29bd".into(),
            executable: String::new(),
            url: String::new(),
            sha256: String::new(),
            size: 0,
            files: Vec::new(),
        });
    }
    if matches!(id, "browser" | "7zr" | "7z-package") {
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../../release/runtime/artifacts.lock.json"))?;
        let platform = if cfg!(windows) { "win64" } else { "linux64" };
        return Ok(serde_json::from_value(
            rows.into_iter()
                .find(|r| r["id"] == id && r["platform"] == platform)
                .context("该架构没有固定发行物")?,
        )?);
    }
    if id == "tesseract" {
        let lock: serde_json::Value =
            serde_json::from_str(include_str!("../../../release/runtime/ocr.lock.json"))?;
        return Ok(Artifact {
            version: lock["engine"]["version"]
                .as_str()
                .context("OCR 版本缺失")?
                .into(),
            executable: executable(id),
            url: lock["engine"]["url"].as_str().unwrap_or_default().into(),
            sha256: lock["engine"]["sha256"].as_str().unwrap_or_default().into(),
            size: 128 * 1024 * 1024,
            files: Vec::new(),
        });
    }
    #[derive(Deserialize)]
    struct Lock {
        component: Vec<Component>,
    }
    #[derive(Deserialize)]
    struct Component {
        id: String,
        version: String,
        source_url: String,
        source_sha256: String,
        source_size: u64,
        files: Vec<RequiredFile>,
    }
    let lock: Lock = toml::from_str(include_str!("../../../release/dependencies.lock.toml"))?;
    let c = lock
        .component
        .into_iter()
        .find(|c| c.id == id)
        .context("运行依赖不在发行锁中")?;
    Ok(Artifact {
        version: c.version,
        executable: executable(id),
        url: c.source_url,
        sha256: c.source_sha256,
        size: c.source_size,
        files: c.files,
    })
}

/// Streaming download, bounded size/time, HTTPS and exact content digest.
/// Only compiled lock entries reach this function, never API-provided URLs.
fn download(a: &Artifact, target: &Path) -> Result<()> {
    ensure!(
        a.sha256.len() == 64 && a.size > 0 && a.size <= 512 * 1024 * 1024,
        "非法依赖锁"
    );
    let url = reqwest::Url::parse(&a.url)?;
    ensure!(
        url.scheme() == "https"
            && matches!(
                url.host_str(),
                Some(
                    "dl.google.com"
                        | "github.com"
                        | "raw.githubusercontent.com"
                        | "storage.googleapis.com"
                        | "www.7-zip.org"
                )
            ),
        "依赖来源不受信任"
    );
    let response = ureq::AgentBuilder::new()
        .try_proxy_from_env(true)
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .get(&a.url)
        .call()?;
    let mut reader = response.into_reader().take(a.size + 1);
    let mut out = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(target)?;
    let mut hash = Sha256::new();
    let mut total = 0;
    let mut buffer = [0u8; 65536];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        ensure!(total <= a.size, "下载超出锁定大小");
        hash.update(&buffer[..n]);
        out.write_all(&buffer[..n])?;
    }
    out.sync_all()?;
    ensure!(
        format!("{:x}", hash.finalize()) == a.sha256,
        "下载完整性校验失败"
    );
    ensure!(total == a.size, "下载大小不匹配");
    Ok(())
}

fn ordinary_tree(root: &Path) -> Result<()> {
    let mut cursor = Some(root);
    while let Some(path) = cursor {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            ensure!(!meta.file_type().is_symlink(), "受管理依赖目录不能是链接");
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                ensure!(
                    meta.file_attributes() & 0x400 == 0,
                    "受管理依赖目录不能是重解析点"
                );
            }
        }
        cursor = path.parent();
    }
    Ok(())
}

pub(super) fn extract_zip(source: &Path, target: &Path, files: &[RequiredFile]) -> Result<()> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(source)?)?;
    ensure!(archive.len() <= 15000, "依赖归档条目过多");
    let mut names = std::collections::BTreeSet::new();
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let enclosed = entry
            .enclosed_name()
            .context("依赖归档路径越界")?
            .to_owned();
        ensure!(entry.size() <= 512 * 1024 * 1024, "单个依赖文件过大");
        ensure!(
            !entry
                .name()?
                .split('/')
                .any(|part| part.contains(':') || part.ends_with(['.', ' '])),
            "依赖归档含不安全文件名"
        );
        ensure!(
            !entry.name()?.contains('\\')
                && entry.unix_mode().is_none_or(|m| m & 0o170000 != 0o120000),
            "依赖归档包含链接或非法路径"
        );
        let relative = if files.is_empty() {
            enclosed
        } else {
            let name = enclosed
                .file_name()
                .context("依赖归档文件名缺失")?
                .to_string_lossy();
            if !files.iter().any(|f| f.path == name) {
                continue;
            }
            PathBuf::from(name.as_ref())
        };
        ensure!(
            names.insert(relative.to_string_lossy().to_lowercase()),
            "依赖归档含重复路径"
        );
        total = total.checked_add(entry.size()).context("归档大小溢出")?;
        ensure!(total <= 1024 * 1024 * 1024, "依赖归档过大");
        let destination = target.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(destination)?;
            continue;
        }
        std::fs::create_dir_all(destination.parent().context("依赖目录无效")?)?;
        let mut out = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&destination)?;
        std::io::copy(&mut entry.by_ref().take(512 * 1024 * 1024 + 1), &mut out)?;
        out.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &destination,
                std::fs::Permissions::from_mode(entry.unix_mode().unwrap_or(0o644) & 0o777),
            )?;
        }
    }
    for file in files {
        let path = target.join(&file.path);
        let mut input = std::fs::File::open(path)?;
        ensure!(input.metadata()?.len() == file.size, "运行依赖文件大小不符");
        let mut sha = Sha256::new();
        std::io::copy(&mut input, &mut sha)?;
        ensure!(
            format!("{:x}", sha.finalize()) == file.sha256,
            "运行依赖文件摘要不符"
        );
    }
    Ok(())
}

pub(super) async fn install(cfg: &Config, id: &str) -> Result<()> {
    let cfg = cfg.clone();
    let id = id.to_string();
    tokio::task::spawn_blocking(move || install_blocking(&cfg, &id))
        .await
        .context("运行依赖安装任务中断")?
}
fn install_blocking(cfg: &Config, id: &str) -> Result<()> {
    let parent = root(cfg);
    ordinary_tree(&parent)?;
    std::fs::create_dir_all(&parent)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.join(".install.lock"))?;
    lock.try_lock()
        .context("其它 Gamer 进程正在准备依赖，请稍后重试")?;
    let artifact = artifact(id)?;
    let destination = parent.join(id).join(&artifact.version);
    ordinary_tree(&destination)?;
    // Never overwrite a previous immutable version, including an invalid one.
    ensure!(
        !destination.exists(),
        "此依赖版本目录已存在但不可用，原文件保留。请指定已有依赖或检查目录"
    );
    let stage = tempfile::Builder::new()
        .prefix(".prepare-")
        .tempdir_in(&parent)?;
    let payload = stage.path().join("payload");
    std::fs::create_dir(&payload)?;
    if id == "tessdata" {
        let lock: serde_json::Value =
            serde_json::from_str(include_str!("../../../release/runtime/ocr.lock.json"))?;
        for row in lock["models"]["files"]
            .as_array()
            .context("OCR 模型锁无效")?
        {
            let a = Artifact {
                url: row["url"].as_str().context("模型 URL 无效")?.into(),
                sha256: row["sha256"].as_str().context("模型摘要无效")?.into(),
                size: row["size_bytes"].as_u64().context("模型大小无效")?,
                ..artifact.clone()
            };
            download(
                &a,
                &payload.join(format!(
                    "{}.traineddata",
                    row["language"].as_str().context("语言无效")?
                )),
            )?;
        }
    } else if id == "tesseract" {
        #[cfg(windows)]
        install_tesseract(stage.path(), &payload)?;
        #[cfg(not(windows))]
        anyhow::bail!("本平台使用已有 Tesseract 或 Docker 内置版本");
    } else {
        let archive = stage.path().join("download.zip");
        download(&artifact, &archive)?;
        extract_zip(&archive, &payload, &artifact.files)?;
    }
    // Validate BEFORE publishing. Prior selected versions and custom tools are
    // untouched. The sync installer runs on the blocking pool, not Tokio workers.
    if id != "tessdata" {
        tokio::runtime::Handle::current().block_on(super::probe::verify(
            id,
            &payload.join(&artifact.executable),
        ))?;
    }
    crate::core::fs::atomic_write(
        &payload.join("gamer-runtime.json"),
        &serde_json::to_vec_pretty(&artifact)?,
    )?;
    std::fs::create_dir_all(destination.parent().context("依赖版本目录无效")?)?;
    std::fs::rename(&payload, &destination).context("发布运行依赖失败，已有版本保持原样")?;
    Ok(())
}

#[cfg(windows)]
fn install_tesseract(stage: &Path, payload: &Path) -> Result<()> {
    use std::process::{Command, Stdio};
    // Bootstrap a pinned extractor locally; no system 7-Zip, PATH edits or
    // executing the Tesseract NSIS installer is required.
    let sevenzr = stage.join("7zr.exe");
    download(&artifact("7zr")?, &sevenzr)?;
    let extra = stage.join("7zip-package.exe");
    download(&artifact("7z-package")?, &extra).context("下载完整解压器失败")?;
    let extracted = stage.join("7zip");
    let status = {
        use std::os::windows::process::CommandExt;
        Command::new(&sevenzr)
            .creation_flags(0x08000000)
            .args(["x", "-y", "-bd"])
            .arg(&extra)
            .arg(format!("-o{}", extracted.display()))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("启动解压器引导失败")?
    };
    ensure!(status.success(), "7-Zip 引导解压失败");
    std::fs::write(
        stage.join("install.ps1"),
        include_bytes!("../../../release/runtime/install-tesseract.ps1"),
    )?;
    std::fs::write(
        stage.join("ocr.lock.json"),
        include_bytes!("../../../release/runtime/ocr.lock.json"),
    )?;
    let status = {
        use std::os::windows::process::CommandExt;
        Command::new("powershell.exe")
            .creation_flags(0x08000000)
            .env(
                "PSModulePath",
                PathBuf::from(
                    std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()),
                )
                .join("System32/WindowsPowerShell/v1.0/Modules"),
            )
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(stage.join("install.ps1"))
            .arg("-RuntimeDir")
            .arg(payload)
            .arg("-SevenZipPath")
            .arg(extracted.join("7z.exe"))
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .context("启动 Tesseract 准备脚本失败")?
    };
    ensure!(status.success(), "Tesseract 准备失败；已保留原有工具和配置");
    Ok(())
}
