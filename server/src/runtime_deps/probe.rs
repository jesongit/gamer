use super::*;
use tokio::{
    process::Command,
    time::{timeout, Duration},
};

pub(super) async fn output(path: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new(crate::ocr::settings::engine_argument_path(path));
    command.args(args).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let result = timeout(Duration::from_secs(4), command.output())
        .await
        .context("依赖探测超时")??;
    ensure!(result.status.success(), "程序探测失败");
    ensure!(
        result.stdout.len() + result.stderr.len() <= 1024 * 1024,
        "依赖探测输出过大"
    );
    Ok(format!(
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    ))
}

pub(super) async fn verify(id: &str, path: &Path) -> Result<Option<String>> {
    if id == "browser" && cfg!(windows) {
        // --version on Windows Chrome/Edge can open a real profile. Read PE
        // identity without launching it; the first isolated CDP launch tests IO.
        use std::io::Read;
        let mut signature = [0; 2];
        std::fs::File::open(path)?.read_exact(&mut signature)?;
        ensure!(&signature == b"MZ", "浏览器不是 Windows 可执行程序");
        return Ok(None);
    }
    let args = match id {
        "adb" => vec!["version"],
        "ffmpeg" => vec!["-version"],
        _ => vec!["--version"],
    };
    let text = output(path, &args).await?;
    let identity = match id {
        "adb" => "Android Debug Bridge",
        "ffmpeg" => "ffmpeg version",
        "tesseract" => "tesseract",
        _ => "Chrome",
    };
    ensure!(
        text.contains(identity) || (id == "browser" && text.contains("Chromium")),
        "程序标识不匹配"
    );
    if id == "tesseract" {
        ensure!(
            text.lines().any(|l| l
                .strip_prefix("tesseract ")
                .is_some_and(|v| v.trim_start_matches('v').starts_with(['4', '5'])
                    && v.trim_start_matches('v').as_bytes().get(1) == Some(&b'.'))),
            "OCR 需要 Tesseract 4/5 LSTM 引擎"
        );
    }
    if id == "ffmpeg" {
        let decoders = output(path, &["-hide_banner", "-decoders"]).await?;
        let encoders = output(path, &["-hide_banner", "-encoders"]).await?;
        ensure!(
            decoders.contains("h264") && encoders.contains(" png "),
            "FFmpeg 缺少 H.264 解码或 PNG 编码能力"
        );
        let ffprobe = path.with_file_name(executable("ffprobe"));
        let probe = output(&ffprobe, &["-version"]).await?;
        ensure!(probe.contains("ffprobe version"), "缺少匹配的 FFprobe");
    }
    Ok(text
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.chars().take(180).collect()))
}

pub(super) async fn dependency(cfg: &Config, id: &str) -> Result<Dependency> {
    ensure!(IDS.contains(&id), "未知运行依赖");
    let configured = selection(cfg, id)?;
    let mut result = Dependency {
        id: id.into(),
        status: "missing".into(),
        source: if configured.is_empty() {
            "auto"
        } else {
            "custom"
        }
        .into(),
        version: None,
        path: None,
        configured_path: configured,
        detail: "未找到可用依赖".into(),
        auto_install: install::supported(id),
        restart_required: false,
    };
    if id == "scrcpy-server" {
        result.path = Some(cfg.scrcpy_server.clone());
        result.source = "application".into();
        result.auto_install = false;
        result.status = if cfg.check_scrcpy_jar().is_ok() {
            "ready"
        } else {
            "broken"
        }
        .into();
        result.version = Some(crate::device::scrcpy::SCRCPY_VERSION.into());
        result.detail = "随应用协议版本固定分发".into();
        return Ok(result);
    }
    let path = if id == "tessdata" {
        model_directory(
            cfg,
            existing_engine(cfg, "tesseract").ok().flatten().as_deref(),
        )
    } else {
        let candidates = engine_candidates(cfg, id);
        match candidates {
            Err(error) => Err(error),
            Ok(candidates) => {
                let mut selected = None;
                let mut failure = None;
                for path in candidates {
                    match verify(id, &path).await {
                        Ok(_) => {
                            selected = Some(path);
                            break;
                        }
                        Err(error) => {
                            failure = Some(error);
                            if !result.configured_path.is_empty() {
                                break;
                            }
                        }
                    }
                }
                match (selected, failure) {
                    (Some(path), _) => Ok(Some(path)),
                    (None, Some(error)) => Err(error),
                    _ => Ok(None),
                }
            }
        }
    };
    let checked = async {
        let path = path?.context("未找到可用依赖")?;
        if id == "tessdata" {
            ensure!(
                ["eng", "chi_sim"].iter().all(|l| path
                    .join(format!("{l}.traineddata"))
                    .metadata()
                    .is_ok_and(|m| m.len() > 0)),
                "语言包需要 eng 和 chi_sim"
            );
            result.version = Some("eng, chi_sim".into());
        } else {
            result.version = verify(id, &path).await?;
        }
        if result.configured_path.is_empty() {
            result.source = if path.starts_with(root(cfg)) {
                "managed"
            } else {
                "system"
            }
            .into();
        }
        result.restart_required = match id {
            "adb" => Path::new(&cfg.adb_path) != path,
            "ffmpeg" => Path::new(&cfg.ffmpeg_path) != path,
            "browser" => !cfg.browser_path.is_empty() && Path::new(&cfg.browser_path) != path,
            _ => false,
        };
        result.path = Some(path);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    match checked {
        Ok(()) => {
            result.status = "ready".into();
            result.detail = if id == "browser" && cfg!(windows) {
                "已检测到浏览器；连接时验证 CDP"
            } else {
                "可用"
            }
            .into();
        }
        Err(error) => {
            result.detail = missing(error);
            if !result.configured_path.is_empty() {
                result.status = "broken".into();
            }
        }
    }
    Ok(result)
}
