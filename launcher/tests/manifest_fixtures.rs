//! QA-001：manifest/签名/路径正反例全量测试。
//!
//! 直接遍历 `release/contracts/fixtures/manifest/` 下全部 26 个 fixture：
//! - valid/ 2 份必须全部通过（并额外做「改一字节必须验签失败」的篡改检查）；
//! - invalid/ 24 份必须被其文件名对应的错误码拒绝；
//! - 期望表必须覆盖全部 invalid fixture（新增 fixture 未登记时本测试失败）。
//!
//! 与 node release/contracts/validate-manifest.mjs selftest 相同的统一期望参数：
//! --expect-current-version 0.2.0 --expect-channel stable，信任库为 fixtures/keys。

use std::fs;
use std::path::{Path, PathBuf};

use gamer_launcher::manifest::{validate_manifest_file, ValidateOptions};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .to_path_buf()
}

fn fixtures_dir() -> PathBuf {
    repo_root()
        .join("release")
        .join("contracts")
        .join("fixtures")
        .join("manifest")
}

fn keys_dir() -> PathBuf {
    repo_root()
        .join("release")
        .join("contracts")
        .join("fixtures")
        .join("keys")
}

fn base_opts() -> ValidateOptions {
    ValidateOptions {
        keys_dir: Some(keys_dir()),
        expect_current_version: Some("0.2.0".to_string()),
        expect_channel: Some("stable".to_string()),
        launcher_version: Some("0.1.0".to_string()),
        ..ValidateOptions::default()
    }
}

fn unique_temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gamer-launcher-fixture-tests-{tag}-{}-{}",
        std::process::id(),
        gamer_launcher::state::atomic::now_unix_millis()
    ));
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

#[test]
fn valid_fixtures_pass() {
    let valid_dir = fixtures_dir().join("valid");
    let mut files: Vec<PathBuf> = fs::read_dir(&valid_dir)
        .expect("valid fixture 目录应存在")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 2, "valid fixture 应有 2 份: {files:?}");

    for file in &files {
        let outcome = validate_manifest_file(file, &base_opts());
        assert!(
            outcome.ok,
            "valid fixture {} 应通过，实际错误: {:?}",
            file.display(),
            outcome.errors
        );
        assert_eq!(outcome.info.version.as_deref(), Some("0.2.0"));
        assert_eq!(outcome.info.channel.as_deref(), Some("stable"));
        assert_eq!(outcome.info.platforms, vec!["windows-x86_64".to_string()]);
        assert_eq!(
            outcome.info.key_id.as_deref(),
            Some("test-ed25519-public-1")
        );
    }
}

#[test]
fn tampered_valid_manifest_rejected() {
    // 对每个合法 fixture：改一个字节后用原签名必须验签失败（计划 §11.1）
    let valid_dir = fixtures_dir().join("valid");
    let mut files: Vec<PathBuf> = fs::read_dir(&valid_dir)
        .expect("valid fixture 目录应存在")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort();

    let tmp = unique_temp_dir("tamper");
    for (i, file) in files.iter().enumerate() {
        let raw = fs::read(file).expect("读取 manifest");
        let sig = fs::read(file.with_extension("sig")).expect("读取 .sig");
        let mut flipped = raw.clone();
        // 翻转第一个字母数字字节（与 validate-manifest.mjs 的 flipOneByte 同语义）
        let flipped_at = flipped.iter_mut().enumerate().find_map(|(idx, byte)| {
            if byte.is_ascii_alphanumeric() {
                *byte = if *byte == b'a' { b'b' } else { b'a' };
                Some(idx)
            } else {
                None
            }
        });
        let at = flipped_at.expect("fixture 至少应含一个可翻转字节");
        assert_ne!(flipped, raw, "第 {at} 字节翻转后内容应不同");

        let tampered_path = tmp.join(format!("tampered-{i}.json"));
        let tampered_sig = tmp.join(format!("tampered-{i}.sig"));
        fs::write(&tampered_path, &flipped).expect("写篡改 manifest");
        fs::write(&tampered_sig, &sig).expect("写原签名");

        let outcome = validate_manifest_file(&tampered_path, &base_opts());
        assert!(!outcome.ok, "篡改 1 字节后的 manifest 必须被拒绝");
        assert!(
            outcome.errors.iter().any(|e| e.code == "signature-invalid"),
            "期望 signature-invalid，实际: {:?}",
            outcome.errors
        );
    }
    let _ = fs::remove_dir_all(&tmp);
}

/// invalid fixture 文件名（去 .json）→ 必须命中的错误码。
/// 与 validate-manifest.mjs 的 INVALID_EXPECTATIONS 对齐。
const INVALID_EXPECTATIONS: &[(&str, &str)] = &[
    ("unsigned-manifest", "unsigned-manifest"),
    ("tampered-manifest-byte", "signature-invalid"),
    ("wrong-key-signature", "signature-invalid"),
    ("sig-format-invalid", "sig-format-invalid"),
    ("unknown-key-id", "unknown-key-id"),
    ("malformed-json-but-signed", "json-parse-failed"),
    ("unknown-schema-version", "unknown-schema-version"),
    ("unknown-platform", "unknown-platform"),
    ("version-not-semver", "version-not-semver"),
    ("version-downgrade", "version-downgrade"),
    ("channel-mismatch", "channel-mismatch"),
    ("jar-binding-mismatch", "jar-binding-mismatch"),
    ("path-absolute", "path-absolute"),
    ("path-drive-letter", "path-drive-letter"),
    ("path-dotdot", "path-dotdot"),
    ("path-ads-colon", "path-ads-colon"),
    ("path-backslash", "path-backslash"),
    ("path-reserved-name", "path-reserved-name"),
    ("path-case-collision", "path-case-collision"),
    ("path-duplicate-entry", "path-duplicate-entry"),
    ("sha256-uppercase", "sha256-uppercase"),
    ("sha256-wrong-length", "sha256-wrong-length"),
    ("size-negative", "size-negative"),
    ("size-oversized", "size-oversized"),
];

fn invalid_fixture_dir() -> PathBuf {
    fixtures_dir().join("invalid")
}

#[test]
fn expectations_cover_all_invalid_fixtures() {
    let stems = list_invalid_stems();
    assert_eq!(stems.len(), 24, "invalid fixture 应有 24 份: {stems:?}");
    for stem in &stems {
        assert!(
            INVALID_EXPECTATIONS.iter().any(|(s, _)| s == stem),
            "invalid fixture {stem} 未在期望表登记（新增 fixture 必须同步登记）"
        );
    }
    assert_eq!(INVALID_EXPECTATIONS.len(), stems.len());
}

#[test]
fn invalid_fixtures_rejected_with_expected_codes() {
    for (stem, expected_code) in INVALID_EXPECTATIONS {
        let path = invalid_fixture_dir().join(format!("{stem}.json"));
        let outcome = validate_manifest_file(&path, &base_opts());
        assert!(
            !outcome.ok,
            "invalid fixture {stem} 必须被拒绝，却通过了校验"
        );
        let got: Vec<&str> = outcome.errors.iter().map(|e| e.code.as_str()).collect();
        assert!(
            got.contains(expected_code),
            "invalid fixture {stem} 期望错误码 {expected_code:?}，实际 {got:?}（{:?}）",
            outcome.errors.iter().map(|e| &e.detail).collect::<Vec<_>>()
        );
    }
}

/// 26 = 2 valid + 24 invalid，全部走同一入口。
#[test]
fn fixture_set_total_is_26() {
    let valid = fs::read_dir(fixtures_dir().join("valid"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .count();
    let invalid = list_invalid_stems().len();
    assert_eq!(valid + invalid, 26, "fixture 总数应为 26（valid+invalid）");
}

fn list_invalid_stems() -> Vec<String> {
    let mut stems: Vec<String> = fs::read_dir(invalid_fixture_dir())
        .expect("invalid fixture 目录应存在")
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })
        .collect();
    stems.sort();
    stems
}
