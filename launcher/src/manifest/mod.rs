//! release manifest v1 解析与验签编排（LCH-003）。
//!
//! 校验顺序 fail closed（manifest-v1.md §4）：
//! 读原始字节 → Ed25519 验签（覆盖原始字节）→ 解析 JSON →
//! 显式语义规则（专属错误码）→ 结构回退校验 → launcher 最低版本门禁。

pub mod checks;
pub mod codes;
pub mod model;
pub mod pathsafe;
pub mod semver;
pub mod sig;

use std::fs;
use std::path::{Path, PathBuf};

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
    pub key_id: Option<String>,
    pub version: Option<String>,
    pub channel: Option<String>,
    pub platforms: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ValidateOptions {
    /// 显式分离签名文件；缺省取 `<manifest 去 .json>.sig`
    pub sig_path: Option<PathBuf>,
    /// 可信公钥目录（`<key_id>.pem`，当前 + 下一把）
    pub keys_dir: Option<PathBuf>,
    /// 显式公钥 PEM（优先于信任库，用于单文件校验场景）
    pub key_path: Option<PathBuf>,
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

/// `<x.json>` → `<x.sig>`；其余情况直接追加 `.sig`。
pub fn default_sig_path(manifest_path: &Path) -> PathBuf {
    let is_json = manifest_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
    if is_json {
        manifest_path.with_extension("sig")
    } else {
        let mut s = manifest_path.as_os_str().to_os_string();
        s.push(".sig");
        PathBuf::from(s)
    }
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

    // 2) detached 签名：未签名 → 直接拒绝
    let sig_path = opts
        .sig_path
        .clone()
        .unwrap_or_else(|| default_sig_path(manifest_path));
    if !sig_path.exists() {
        return fail(
            codes::UNSIGNED_MANIFEST,
            format!("未找到签名文件: {}", sig_path.display()),
            &info,
        );
    }
    let sig_bytes = match fs::read(&sig_path) {
        Ok(b) => b,
        Err(e) => return fail(codes::IO_ERROR, format!("无法读取签名文件: {e}"), &info),
    };
    let parsed_sig = match sig::parse_signature_file(&sig_bytes) {
        Ok(s) => s,
        Err(detail) => return fail(codes::SIG_FORMAT_INVALID, detail, &info),
    };
    let mut info = ManifestInfo {
        key_id: Some(parsed_sig.key_id.clone()),
        ..ManifestInfo::default()
    };

    // 3) 信任库选公钥（未知 key_id / 不可解析 → 拒绝）
    let key = match load_trusted_key(&parsed_sig.key_id, opts) {
        Ok(k) => k,
        Err((code, detail)) => return fail(&code, detail, &info),
    };

    // 4) Ed25519 验签（覆盖原始字节）
    if !sig::verify_signature(&raw, &key, &parsed_sig.signature) {
        return fail(
            codes::SIGNATURE_INVALID,
            format!(
                "Ed25519 验签失败（key_id={}，覆盖 manifest 原始字节；可能被篡改、用错密钥或重新签名）",
                parsed_sig.key_id
            ),
            &info,
        );
    }

    // 5) 先验签、再解析 JSON
    let manifest: Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => {
            return fail(
                codes::JSON_PARSE_FAILED,
                format!("签名合法但字节不是 JSON: {e}"),
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

    // 6) 显式语义规则（专属错误码）
    let mut errors = checks::semantic_checks(
        &manifest,
        &checks::Expectations {
            expect_current_version: opts.expect_current_version.as_deref(),
            expect_channel: opts.expect_channel.as_deref(),
        },
    );

    // 7) 结构回退校验（仅在语义无错时执行，与 validate-manifest.mjs 一致）
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

fn load_trusted_key(
    key_id: &str,
    opts: &ValidateOptions,
) -> Result<ed25519_dalek::VerifyingKey, (String, String)> {
    // --key 显式指定公钥：解析失败按未知 key 处理（fail closed）
    if let Some(key_path) = &opts.key_path {
        let pem = fs::read_to_string(key_path).map_err(|e| {
            (
                codes::IO_ERROR.to_string(),
                format!("无法读取公钥 PEM {}: {e}", key_path.display()),
            )
        })?;
        return sig::parse_public_key_pem(&pem).ok_or_else(|| {
            (
                codes::UNKNOWN_KEY_ID.to_string(),
                format!(
                    "公钥文件 {} 无法解析为 Ed25519 SPKI PEM",
                    key_path.display()
                ),
            )
        });
    }
    if !sig::is_valid_key_id(key_id) {
        return Err((
            codes::UNKNOWN_KEY_ID.to_string(),
            format!("key_id {key_id:?} 不在信任库中"),
        ));
    }
    let Some(dir) = &opts.keys_dir else {
        return Err((
            codes::UNKNOWN_KEY_ID.to_string(),
            "未配置可信公钥目录（--keys-dir），无法校验签名".to_string(),
        ));
    };
    let store = sig::TrustStore::from_dir(dir).map_err(|e| {
        (
            codes::IO_ERROR.to_string(),
            format!("读取信任库目录失败: {e}"),
        )
    })?;
    store.get(key_id).cloned().ok_or_else(|| {
        (
            codes::UNKNOWN_KEY_ID.to_string(),
            format!("信任库 {} 中没有可信公钥 {key_id:?}", dir.display()),
        )
    })
}
