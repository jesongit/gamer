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

fn base_opts() -> ValidateOptions {
    ValidateOptions {
        expect_current_version: Some("0.2.0".to_string()),
        expect_channel: Some("stable".to_string()),
        launcher_version: Some("0.1.0".to_string()),
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
    }
}

#[test]
fn unsigned_manifest_loads_without_sidecar_or_keys() {
    let tmp = unique_temp_dir("unsigned");
    let source = fixtures_dir().join("valid/manifest-valid-basic.json");
    let target = tmp.join("release.json");
    fs::copy(source, &target).unwrap();
    assert!(validate_manifest_file(&target, &base_opts()).ok);
    assert!(!target.with_extension("sig").exists());
    fs::remove_dir_all(tmp).unwrap();
}

/// invalid fixture 文件名（去 .json）→ 必须命中的错误码。
/// 与 validate-manifest.mjs 的 INVALID_EXPECTATIONS 对齐。
const INVALID_EXPECTATIONS: &[(&str, &str)] = &[
    ("malformed-json", "json-parse-failed"),
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
    assert_eq!(stems.len(), 19, "invalid fixture 应有 19 份: {stems:?}");
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

/// 21 = 2 valid + 19 invalid，全部走同一入口。
#[test]
fn fixture_set_total_is_21() {
    let valid = fs::read_dir(fixtures_dir().join("valid"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .count();
    let invalid = list_invalid_stems().len();
    assert_eq!(valid + invalid, 21, "fixture 总数应为 21（valid+invalid）");
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
