//! LCH-004：组件库存与深检。
//!
//! 库存 = manifest `components[].required_files` 在 `runtime/<id>/<version>/`
//! 的落地视图。快速检查 = 文件存在 + size；doctor 深检 = 逐文件 sha256 对
//! manifest；可选版本探针（`adb version` / `ffmpeg -version` 解析版本串与
//! `components[].version` 比对）。每条失败都定位到具体文件与原因。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::digest::{sha256_file_hex, verify_file};
use crate::layout::InstallLayout;
use crate::manifest::model::Component;

/// 版本探针超时（有界，杀进程兜底）。
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// Windows CREATE_NO_WINDOW：探针子进程不弹窗。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 组件规格（从已验签 manifest 组件构建；安装/修复/复验共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentSpec {
    pub id: String,
    pub version: String,
    pub files: Vec<FileSpec>,
    pub artifact_name: String,
    pub artifact_sha256: String,
    pub artifact_size: u64,
    pub artifact_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSpec {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

impl ComponentSpec {
    /// 从 manifest 模型构建；`version` 会被用作目录名，必须先过路径安全检查
    /// （manifest 契约未约束 version 字符集，这里本地兜底防目录逃逸）。
    pub fn from_model(component: &Component) -> Result<ComponentSpec, String> {
        if let Some(reason) = crate::manifest::pathsafe::check_single_path(&component.version) {
            return Err(format!(
                "组件 {} 的 version {:?} 不能用作目录名（{reason}）",
                component.id, component.version
            ));
        }
        if component.artifact.name.contains('/') || component.artifact.name.contains('\\') {
            return Err(format!(
                "组件 {} 的 artifact.name {:?} 必须为单一文件名",
                component.id, component.artifact.name
            ));
        }
        let files = component
            .required_files
            .iter()
            .map(|f| FileSpec {
                path: f.path.clone(),
                size: u64::try_from(f.size).unwrap_or(0),
                sha256: f.sha256.to_ascii_lowercase(),
            })
            .collect();
        Ok(ComponentSpec {
            id: component.id.clone(),
            version: component.version.clone(),
            files,
            artifact_name: component.artifact.name.clone(),
            artifact_sha256: component.artifact.sha256.to_ascii_lowercase(),
            artifact_size: u64::try_from(component.artifact.size).unwrap_or(0),
            artifact_url: component.artifact.url.clone(),
        })
    }

    pub fn install_dir(&self, layout: &InstallLayout) -> PathBuf {
        layout.component_dir(&self.id, &self.version)
    }

    /// required_files 声明字节总和（修复解压的炸弹上限来源）。
    pub fn total_declared_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileCheck {
    Ok,
    Missing,
    SizeMismatch { actual: u64, expected: u64 },
    HashMismatch { actual: String, expected: String },
    Io(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFinding {
    pub path: String,
    pub check: FileCheck,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeCheck {
    Match { reported: String },
    Mismatch { reported: String },
    Failed { reason: String },
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentStatus {
    Ok,
    Damaged,
}

#[derive(Debug, Clone)]
pub struct ComponentFinding {
    pub dir: PathBuf,
    pub status: ComponentStatus,
    pub files: Vec<FileFinding>,
    pub probe: Option<ProbeCheck>,
}

#[derive(Debug, Clone, Copy)]
pub struct CheckOptions {
    /// 深检：逐文件 sha256（false 时只查存在 + size）。
    pub deep: bool,
    /// 运行版本探针（adb/ffmpeg；需要可执行文件真的能跑）。
    pub probe: bool,
}

/// 检查单个组件目录；目录缺失时所有文件报 Missing、状态 Damaged（可修复）。
pub fn check_component(dir: &Path, spec: &ComponentSpec, opts: CheckOptions) -> ComponentFinding {
    let mut files = Vec::new();
    let mut ok = dir.is_dir();
    if !dir.is_dir() {
        for f in &spec.files {
            files.push(FileFinding {
                path: f.path.clone(),
                check: FileCheck::Missing,
            });
        }
    } else {
        for f in &spec.files {
            let p = dir.join(&f.path);
            let check = if !p.is_file() {
                FileCheck::Missing
            } else {
                match std::fs::metadata(&p) {
                    Ok(meta) if meta.len() != f.size => FileCheck::SizeMismatch {
                        actual: meta.len(),
                        expected: f.size,
                    },
                    Ok(_) if !opts.deep => FileCheck::Ok,
                    Ok(_) => match sha256_file_hex(&p) {
                        Ok(actual) if actual == f.sha256 => FileCheck::Ok,
                        Ok(actual) => FileCheck::HashMismatch {
                            actual,
                            expected: f.sha256.clone(),
                        },
                        Err(e) => FileCheck::Io(e.to_string()),
                    },
                    Err(e) => FileCheck::Io(e.to_string()),
                }
            };
            if check != FileCheck::Ok {
                ok = false;
            }
            files.push(FileFinding {
                path: f.path.clone(),
                check,
            });
        }
    }
    let probe = if opts.probe {
        Some(probe_component(&spec.id, dir, &spec.version))
    } else {
        None
    };
    // 探针语义：版本不符（Mismatch）= 组件确坏（可执行但版本错，修复有价值）；
    // 执行失败（Failed）只报告不判死——hash 门禁已保证文件完整性，可执行文件
    // 跑不起来可能是环境原因（架构/沙箱），以修复器 hash 锚定为准。
    if let Some(ProbeCheck::Mismatch { .. }) = probe {
        ok = false;
    }
    ComponentFinding {
        dir: dir.to_path_buf(),
        status: if ok {
            ComponentStatus::Ok
        } else {
            ComponentStatus::Damaged
        },
        files,
        probe,
    }
}

/// 版本探针：adb → `adb.exe version`；ffmpeg → `ffmpeg.exe -version`；
/// 其余组件 Unsupported（无已知探针）。
pub fn probe_component(id: &str, dir: &Path, expected_version: &str) -> ProbeCheck {
    let (exe_name, args, kind) = match id {
        "adb" => ("adb.exe", vec!["version"], ProbeKind::Adb),
        "ffmpeg" => ("ffmpeg.exe", vec!["-version"], ProbeKind::Ffmpeg),
        _ => return ProbeCheck::Unsupported,
    };
    let exe = dir.join(exe_name);
    if !exe.is_file() {
        return ProbeCheck::Failed {
            reason: format!("可执行文件不存在: {}", exe.display()),
        };
    }
    let output = match run_with_timeout(&exe, &args, PROBE_TIMEOUT) {
        Ok(o) => o,
        Err(reason) => return ProbeCheck::Failed { reason },
    };
    let reported = match kind {
        ProbeKind::Adb => parse_adb_version_output(&output),
        ProbeKind::Ffmpeg => parse_ffmpeg_version_output(&output),
    };
    match reported {
        Some(v) if version_matches(&v, expected_version) => ProbeCheck::Match { reported: v },
        Some(v) => ProbeCheck::Mismatch { reported: v },
        None => ProbeCheck::Failed {
            reason: "无法从输出解析版本串".to_string(),
        },
    }
}

#[derive(Debug, Clone, Copy)]
enum ProbeKind {
    Adb,
    Ffmpeg,
}

/// adb `version` 输出中的 `Version 37.0.1-xxxx` 行 → `37.0.1-xxxx`。
pub(crate) fn parse_adb_version_output(text: &str) -> Option<String> {
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Version ") {
            let tok = rest.split_whitespace().next().unwrap_or("");
            if !tok.is_empty() {
                return Some(tok.to_string());
            }
        }
    }
    None
}

/// ffmpeg `-version` 首行 `ffmpeg version <串> Copyright ...` → `<串>`。
pub(crate) fn parse_ffmpeg_version_output(text: &str) -> Option<String> {
    let line = text.lines().next()?;
    let mut it = line.split_whitespace();
    while let Some(tok) = it.next() {
        if tok.eq_ignore_ascii_case("version") {
            return it.next().map(str::to_string).filter(|v| !v.is_empty());
        }
    }
    None
}

/// 探针比对：完全相等，或探针串为 `期望-后缀` 形态（如 adb 的 `37.0.1-0ace…`）。
pub(crate) fn version_matches(reported: &str, expected: &str) -> bool {
    reported == expected || reported.starts_with(&format!("{expected}-"))
}

/// 带超时的子进程运行（stdout+stderr 合并返回）；超时 kill。
fn run_with_timeout(exe: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    let mut child = Command::new(exe)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("启动探针失败: {e}"))?;
    let mut out_pipe = child.stdout.take().expect("stdout 已 piped");
    let mut err_pipe = child.stderr.take().expect("stderr 已 piped");
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut out_pipe, &mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut err_pipe, &mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err("探针超时（已强制终止）".to_string());
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => break Err(format!("等待探针失败: {e}")),
        }
    };
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    match status {
        Ok(status) if status.success() => Ok(format!("{stdout}{stderr}")),
        Ok(status) => Err(format!(
            "探针退出码 {}",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "非正常终止".into())
        )),
        Err(reason) => Err(reason),
    }
}

/// 便捷封装：检查某组件在其安装目录的落地情况。
pub fn check_installed(
    layout: &InstallLayout,
    spec: &ComponentSpec,
    opts: CheckOptions,
) -> ComponentFinding {
    check_component(&spec.install_dir(layout), spec, opts)
}

/// verify_file 的探针用校验封装（供测试/调用方复用语义）。
pub fn file_matches(path: &Path, sha256: &str, size: u64) -> Result<(), String> {
    verify_file(path, sha256, size).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_adb_version_line() {
        let out = "Android Debug Bridge version 1.0.41\r\nVersion 37.0.1-0ace52ae-20250610\r\nInstalled as C:\\x\\adb.exe";
        assert_eq!(
            parse_adb_version_output(out).as_deref(),
            Some("37.0.1-0ace52ae-20250610")
        );
        assert!(parse_adb_version_output("no version here").is_none());
    }

    #[test]
    fn parses_ffmpeg_version_line() {
        let out = "ffmpeg version N-126335-gb32f8d1c23-20260830 Copyright (c) 2000-2026 the FFmpeg developers\nbuilt with...";
        assert_eq!(
            parse_ffmpeg_version_output(out).as_deref(),
            Some("N-126335-gb32f8d1c23-20260830")
        );
        assert!(parse_ffmpeg_version_output("not ffmpeg output").is_none());
    }

    #[test]
    fn version_match_allows_suffix() {
        assert!(version_matches("37.0.1", "37.0.1"));
        assert!(version_matches("37.0.1-0ace52ae", "37.0.1"));
        assert!(!version_matches("37.0.2", "37.0.1"));
        assert!(!version_matches("37.0.1x", "37.0.1"));
        assert!(version_matches(
            "N-126335-gb32f8d1c23-20260830",
            "N-126335-gb32f8d1c23-20260830"
        ));
    }

    #[test]
    fn spec_rejects_unsafe_version_as_dirname() {
        let json: serde_json::Value = serde_json::json!({
            "id": "adb", "version": "../../evil",
            "artifact": { "name": "a.zip", "url": "https://x.invalid/a.zip", "size": 1, "sha256": &"a".repeat(64) },
            "required_files": [ { "path": "adb.exe", "size": 1, "sha256": &"a".repeat(64) } ]
        });
        let comp: crate::manifest::model::Component = serde_json::from_value(json).unwrap();
        assert!(ComponentSpec::from_model(&comp).is_err());
    }
}
