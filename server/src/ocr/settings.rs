use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    pub version: Option<String>,
    #[serde(default)]
    pub engine_path: String,
    #[serde(default)]
    pub tessdata_path: String,
}

/// Preserve the retired plugin's encrypted bytes AND its portable key. Never
/// remove, rewrite or reinterpret the historical installation/private directory.
pub(crate) fn migrate(data: &Path, target: &Path) -> Result<()> {
    if target.exists() {
        return Ok(());
    }
    let old = data.join("extension-data/gamer-ocr/private/settings.dat");
    if !old.exists() {
        return Ok(());
    }
    let parent = target.parent().context("OCR 配置目录无效")?;
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let key = old.with_file_name(".credentials.key");
    let new_key = target.with_file_name(".credentials.key");
    if key.exists() && !new_key.exists() {
        crate::core::fs::atomic_write(&new_key, &std::fs::read(&key)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&new_key, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    crate::core::fs::atomic_write(target, &std::fs::read(old)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(_) => anyhow::bail!("无法读取 OCR 配置"),
        };
        ensure!(bytes.len() <= 32 * 1024, "OCR 配置过大");
        let plain = crate::core::secrets::protect_file(path, &bytes, false)
            .context("无法解密 OCR 配置，请使用保存配置的系统账号")?;
        serde_json::from_slice(&plain).context("OCR 配置损坏")
    }

    pub fn write(&mut self, path: &Path) -> Result<()> {
        self.version = Some(uuid::Uuid::new_v4().to_string());
        let parent = path.parent().context("OCR 配置路径无效")?;
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        let bytes = crate::core::secrets::protect_file(path, &serde_json::to_vec(self)?, true)?;
        crate::core::fs::atomic_write(path, &bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct Engine {
    pub executable: PathBuf,
    pub tessdata: PathBuf,
    pub languages: Vec<String>,
}

// Anchor Config's relative data root once, without changing process CWD.
pub(super) fn anchor_data_root(base: &Path, requested: &Path) -> Result<PathBuf> {
    ensure!(base.is_absolute(), "OCR 工作目录必须是绝对路径");
    let joined = if requested.is_absolute() {
        requested.to_owned()
    } else {
        base.join(requested)
    };
    ensure!(joined.is_absolute(), "OCR 数据目录无效");
    Ok(engine_argument_path(
        &joined.canonicalize().unwrap_or(joined),
    ))
}

// Tesseract appends `/lang.traineddata` itself. Its Windows CRT cannot open
// that mixed spelling after Rust's canonical \\?\ prefix. Security checks keep
// canonical paths; convert only the executable and final CLI arguments.
pub(crate) fn engine_argument_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{}", unc));
        }
        if let Some(disk) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(disk);
        }
    }
    path.to_owned()
}

pub(crate) fn checked_path(raw: &str, data_root: &Path, directory: bool) -> Result<PathBuf> {
    ensure!(raw.len() <= 4096 && !raw.contains('\0'), "OCR 路径无效");
    let requested = Path::new(raw);
    ensure!(requested.is_absolute(), "OCR 路径必须是绝对路径");
    let actual = requested.canonicalize().context("OCR 路径不存在")?;
    ensure!(
        if directory {
            actual.is_dir()
        } else {
            actual.is_file()
        },
        "OCR 路径类型无效"
    );
    // Downloaded archives never supply an executable. Only administrator-selected
    // local tooling or the host dependency directory can be used as an engine.
    for unsafe_root in [data_root.join("extensions"), data_root.join("packages")] {
        if let Ok(root) = unsafe_root.canonicalize() {
            let inside = if cfg!(windows) {
                actual
                    .to_string_lossy()
                    .to_lowercase()
                    .starts_with(&format!(
                        "{}\\",
                        root.to_string_lossy()
                            .to_lowercase()
                            .trim_end_matches(['\\', '/'])
                    ))
                    || actual
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&root.to_string_lossy())
            } else {
                actual.starts_with(root)
            };
            ensure!(!inside, "不能从插件归档或配置包运行 OCR 程序");
        }
    }
    Ok(actual)
}

pub(super) fn resolve(settings: &Settings, data_root: &Path) -> Result<Engine> {
    let executable = if !settings.engine_path.trim().is_empty() {
        checked_path(settings.engine_path.trim(), data_root, false)?
    } else {
        let file = if cfg!(windows) {
            "tesseract.exe"
        } else {
            "tesseract"
        };
        let bundled = data_root
            .parent()
            .unwrap_or(data_root)
            .join("deps")
            .join("tesseract")
            .join(file);
        let cfg = crate::config::Config {
            data_dir: data_root.to_owned(),
            ..Default::default()
        };
        let candidates = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join(file))
                    .collect::<Vec<_>>()
            })
            .chain(std::iter::once(bundled))
            .chain(crate::runtime_deps::managed(&cfg, "tesseract").ok());
        candidates
            .filter_map(|path| {
                path.to_str()
                    .and_then(|raw| checked_path(raw, data_root, false).ok())
            })
            .next()
            .context("未找到 Tesseract，请在文字识别设置中指定本机程序路径")?
    };
    let tessdata = if !settings.tessdata_path.trim().is_empty() {
        checked_path(settings.tessdata_path.trim(), data_root, true)?
    } else {
        let parent = executable.parent().context("OCR 程序目录无效")?;
        let bundled = parent.join("tessdata");
        let cfg = crate::config::Config {
            data_dir: data_root.to_owned(),
            ..Default::default()
        };
        let root =
            if let Some(path) = crate::runtime_deps::model_directory(&cfg, Some(&executable))? {
                path
            } else if bundled.is_dir() {
                bundled
            } else {
                std::env::var_os("TESSDATA_PREFIX")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute() && path.is_dir())
                    .context("未找到 tessdata 语言包目录，请在文字识别设置中指定")?
            };
        checked_path(
            root.to_str().context("OCR 语言包路径无效")?,
            data_root,
            true,
        )?
    };
    let mut languages = Vec::new();
    for entry in std::fs::read_dir(&tessdata)
        .context("无法读取 OCR 语言包目录")?
        .take(512)
    {
        let path = entry?.path();
        if path.extension().and_then(|v| v.to_str()) == Some("traineddata") && path.is_file() {
            if let Some(name) = path.file_stem().and_then(|v| v.to_str()) {
                if safe_language(name) {
                    languages.push(name.to_owned());
                }
            }
        }
    }
    languages.sort();
    languages.dedup();
    ensure!(
        !languages.is_empty(),
        "OCR 语言包目录没有 .traineddata 文件"
    );
    Ok(Engine {
        executable,
        tessdata,
        languages,
    })
}

pub(super) fn safe_language(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
