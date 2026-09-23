//! Release manifest 结构、版本与文件 SHA256 声明校验。
pub mod checks;
pub mod codes;
pub mod model;
pub mod pathsafe;
pub mod semver;

use std::fs;
use std::path::Path;

use serde_json::Value;

/// 校验错误（code 与 validate-manifest.mjs 错误码一致）。
#[derive(Debug, Clone)]
pub struct ManifestError {
    pub code: String,
    pub detail: String,
}

/// 校验结果的摘要信息。
#[derive(Debug, Clone, Default)]
pub struct ManifestInfo {
    pub version: Option<String>,
    pub channel: Option<String>,
    pub platforms: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ValidateOptions {
    /// 期望的当前安装版本；manifest 版本低于它 → version-downgrade
    pub expect_current_version: Option<String>,
    /// 期望通道（stable|beta）；不匹配 → channel-mismatch
    pub expect_channel: Option<String>,
    /// 本 launcher 自身版本；低于 manifest 的 minimum_launcher_version → launcher-too-old
    pub launcher_version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ValidateOutcome {
    pub ok: bool,
    pub errors: Vec<ManifestError>,
    pub info: ManifestInfo,
}

/// 校验单个 manifest 文件（含磁盘 IO）。这是 `doctor --manifest` 与后续下载校验共用的入口。
pub fn validate_manifest_file(manifest_path: &Path, opts: &ValidateOptions) -> ValidateOutcome {
    let info = ManifestInfo::default();
    let fail = |code: &str, detail: String, info: &ManifestInfo| ValidateOutcome {
        ok: false,
        errors: vec![ManifestError {
            code: code.to_string(),
            detail,
        }],
        info: info.clone(),
    };

    // 1) 读原始字节
    let raw = match fs::read(manifest_path) {
        Ok(r) => r,
        Err(e) => return fail(codes::IO_ERROR, format!("无法读取 manifest: {e}"), &info),
    };

    let mut info = info;
    let manifest: Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => {
            return fail(
                codes::JSON_PARSE_FAILED,
                format!("清单不是合法 JSON: {e}"),
                &info,
            )
        }
    };
    let Some(obj) = manifest.as_object() else {
        return fail(
            codes::SCHEMA_INVALID,
            "manifest 根必须是 JSON 对象".to_string(),
            &info,
        );
    };
    info.version = obj
        .get("release")
        .and_then(|r| r.get("version"))
        .and_then(Value::as_str)
        .map(str::to_string);
    info.channel = obj
        .get("release")
        .and_then(|r| r.get("channel"))
        .and_then(Value::as_str)
        .map(str::to_string);
    info.platforms = obj
        .get("platforms")
        .and_then(Value::as_object)
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();

    // 3) 显式语义规则（专属错误码）
    let mut errors = checks::semantic_checks(
        &manifest,
        &checks::Expectations {
            expect_current_version: opts.expect_current_version.as_deref(),
            expect_channel: opts.expect_channel.as_deref(),
        },
    );

    // 4) 结构回退校验（仅在语义无错时执行，与 validate-manifest.mjs 一致）
    if errors.is_empty() {
        for detail in model::structural_errors(&manifest) {
            errors.push(ManifestError {
                code: codes::SCHEMA_INVALID.to_string(),
                detail,
            });
        }
    }

    // 8) launcher 最低版本门禁（计划 §6.2：低于必须拒绝并提示升级 launcher）
    if errors.is_empty() {
        if let (Some(launcher_v), Some(min_v)) =
            (&opts.launcher_version, minimum_launcher_version(obj))
        {
            match (semver::parse(launcher_v), semver::parse(&min_v)) {
                (Some(l), Some(m)) if semver::is_lt(&l, &m) => {
                    errors.push(ManifestError {
                        code: codes::LAUNCHER_TOO_OLD.to_string(),
                        detail: format!(
                            "launcher {launcher_v} 低于 manifest 要求的最低版本 {min_v}，请先升级 launcher"
                        ),
                    });
                }
                _ => {}
            }
        }
    }

    ValidateOutcome {
        ok: errors.is_empty(),
        errors,
        info,
    }
}

fn minimum_launcher_version(manifest: &serde_json::Map<String, Value>) -> Option<String> {
    manifest
        .get("release")?
        .get("minimum_launcher_version")?
        .as_str()
        .map(str::to_string)
}
