//! QA-002：安全解压与原子安装故障测试（LCH-006）。
//! fixture 全部在测试内用 zip crate 构造：zip-slip / 绝对路径 / 反斜杠 / ADS /
//! 保留名 / 大小写碰撞 / 重复条目 / 符号链接条目 / 解压炸弹（总量与单条目）/
//! 白名单外条目 / 缺失与 hash 不符的 required_files，全部必须拒绝且不留半成品；
//! install_staged 对已存在目标拒绝、成功后 staging 原地消失（同卷 rename）。

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{build_zip, cleanup, sha256_hex, unique_root, ZipEntrySpec};
use gamer_launcher::archive::{self, ArchiveError, ExtractOptions};
use gamer_launcher::manifest::model::RequiredFile;

fn staging(root: &Path) -> PathBuf {
    root.join("staging").join("case")
}

fn required_from(entries: &[(&str, &[u8])]) -> Vec<RequiredFile> {
    entries
        .iter()
        .map(|(path, content)| RequiredFile {
            path: path.to_string(),
            size: content.len() as i64,
            sha256: sha256_hex(content),
        })
        .collect()
}

fn extract(
    entries: &[ZipEntrySpec],
    required: &[RequiredFile],
    opts: &ExtractOptions,
) -> (PathBuf, Result<(), ArchiveError>) {
    let root = unique_root("archive");
    let zip_path = root.join("comp.zip");
    build_zip(&zip_path, entries);
    let stage = staging(&root);
    let result = archive::extract_component_zip(&zip_path, &stage, required, opts);
    (root, result)
}

const GOOD: &[(&str, &[u8])] = &[("adb.exe", b"adb-binary"), ("AdbWinApi.dll", b"dll-bytes")];

#[test]
fn good_zip_extracts_and_verifies() {
    let entries = vec![
        ZipEntrySpec::file("adb.exe", b"adb-binary"),
        ZipEntrySpec::dir("sub"),
        ZipEntrySpec::file("sub/AdbWinApi.dll", b"dll-bytes"),
    ];
    let required = vec![
        RequiredFile {
            path: "adb.exe".into(),
            size: 10,
            sha256: sha256_hex(b"adb-binary"),
        },
        RequiredFile {
            path: "sub/AdbWinApi.dll".into(),
            size: 9,
            sha256: sha256_hex(b"dll-bytes"),
        },
    ];
    let (root, result) = extract(&entries, &required, &ExtractOptions::default());
    result.expect("合法 zip 应解压成功");
    let stage = staging(&root);
    assert_eq!(fs::read(stage.join("adb.exe")).unwrap(), b"adb-binary");
    assert!(stage.join("sub").join("AdbWinApi.dll").is_file());
    cleanup(&root);
}

#[test]
fn zip_slip_traversal_rejected() {
    let entries = vec![
        ZipEntrySpec::file("adb.exe", b"adb-binary"),
        ZipEntrySpec::file("../evil.txt", b"evil"),
    ];
    let (root, result) = extract(&entries, &required_from(GOOD), &ExtractOptions::default());
    match result {
        Err(ArchiveError::DangerousEntry { entry, reason }) => {
            assert_eq!(reason, "path-dotdot");
            assert!(entry.contains("evil"));
        }
        other => panic!("zip-slip 必须被拒，实际 {other:?}"),
    }
    // 半成品 staging：整体放弃语义下允许清理（不进入 install），验证无 evil.txt 落地
    assert!(
        !root.join("evil.txt").exists(),
        "穿越文件不得落在 staging 之外"
    );
    assert!(!staging(&root).join("evil.txt").exists());
    cleanup(&root);
}

#[test]
fn absolute_and_backslash_entries_rejected() {
    let entries = vec![ZipEntrySpec::file("/abs/adb.exe", b"x")];
    let (root, result) = extract(&entries, &required_from(GOOD), &ExtractOptions::default());
    assert!(matches!(
        result,
        Err(ArchiveError::DangerousEntry {
            reason: "path-absolute",
            ..
        })
    ));
    cleanup(&root);

    let entries = vec![ZipEntrySpec::file("sub\\adb.exe", b"x")];
    let (root, result) = extract(&entries, &required_from(GOOD), &ExtractOptions::default());
    assert!(
        matches!(
            result,
            Err(ArchiveError::DangerousEntry {
                reason: "path-backslash",
                ..
            })
        ) || matches!(result, Err(ArchiveError::UnexpectedEntry { .. })),
        "反斜杠条目必须被拒，实际 {result:?}"
    );
    cleanup(&root);
}

#[test]
fn ads_colon_entry_rejected() {
    let entries = vec![ZipEntrySpec::file("adb.exe:hidden", b"x")];
    let (root, result) = extract(&entries, &required_from(GOOD), &ExtractOptions::default());
    assert!(matches!(
        result,
        Err(ArchiveError::DangerousEntry {
            reason: "path-ads-colon",
            ..
        })
    ));
    cleanup(&root);
}

#[test]
fn reserved_name_entry_rejected() {
    let entries = vec![ZipEntrySpec::file("con.txt", b"x")];
    let (root, result) = extract(&entries, &required_from(GOOD), &ExtractOptions::default());
    assert!(matches!(
        result,
        Err(ArchiveError::DangerousEntry {
            reason: "path-reserved-name",
            ..
        })
    ));
    cleanup(&root);
}

#[test]
fn case_collision_rejected() {
    let entries = vec![
        ZipEntrySpec::file("adb.exe", b"one"),
        ZipEntrySpec::file("ADB.EXE", b"two"),
    ];
    let (root, result) = extract(&entries, &required_from(GOOD), &ExtractOptions::default());
    match result {
        Err(ArchiveError::CaseCollision { first, second }) => {
            assert_eq!(first, "adb.exe");
            assert_eq!(second, "ADB.EXE");
        }
        other => panic!("大小写碰撞必须被拒，实际 {other:?}"),
    }
    cleanup(&root);
}

#[test]
fn duplicate_entry_rejected() {
    // zip crate 写入器本身拒绝重复条目名，夹具绕过写入器手工拼 zip 原始字节
    let root = unique_root("archive-dup");
    let zip_path = root.join("comp.zip");
    common::build_raw_zip(
        &zip_path,
        &[
            common::RawZipEntry::file("adb.exe", b"one"),
            common::RawZipEntry::file("adb.exe", b"two"),
        ],
    );
    let stage = staging(&root);
    let result = archive::extract_component_zip(
        &zip_path,
        &stage,
        &required_from(GOOD),
        &ExtractOptions::default(),
    );
    assert!(
        matches!(result, Err(ArchiveError::DuplicateEntry { .. })),
        "重复条目必须被拒，实际 {result:?}"
    );
    cleanup(&root);
}

#[test]
fn symlink_entry_rejected() {
    // zip crate 写入器把 version-made-by 的 system 字节写成 0（DOS），读取侧
    // is_symlink() 永远 false——符号链接夹具须用原始字节直拼（Unix system + mode）
    let root = unique_root("archive-symlink");
    let zip_path = root.join("comp.zip");
    common::build_raw_zip(
        &zip_path,
        &[
            common::RawZipEntry::file("adb.exe", b"adb-binary"),
            common::RawZipEntry::with_unix_mode(
                "AdbWinApi.dll",
                b"C:/Windows/System32/cmd.exe",
                0o120_777,
            ),
        ],
    );
    let stage = staging(&root);
    let result = archive::extract_component_zip(
        &zip_path,
        &stage,
        &required_from(GOOD),
        &ExtractOptions::default(),
    );
    assert!(
        matches!(result, Err(ArchiveError::SymlinkEntry { .. })),
        "符号链接条目必须被拒，实际 {result:?}"
    );
    assert!(
        !stage.join("AdbWinApi.dll").exists(),
        "符号链接条目不得落地"
    );
    cleanup(&root);
}

#[test]
fn bomb_per_file_declared_rejected() {
    let big = vec![b'a'; 100];
    let entries = vec![ZipEntrySpec::file("adb.exe", &big)];
    let required = vec![RequiredFile {
        path: "adb.exe".into(),
        size: 100,
        sha256: sha256_hex(&big),
    }];
    let opts = ExtractOptions {
        max_total_uncompressed: 10,
        max_file_uncompressed: 10,
    };
    let (root, result) = extract(&entries, &required, &opts);
    assert!(
        matches!(result, Err(ArchiveError::BombFile { .. })),
        "单条目炸弹必须被拒，实际 {result:?}"
    );
    cleanup(&root);
}

#[test]
fn bomb_total_declared_rejected() {
    let a = vec![b'a'; 60];
    let b = vec![b'b'; 60];
    let entries = vec![
        ZipEntrySpec::file("adb.exe", &a),
        ZipEntrySpec::file("AdbWinApi.dll", &b),
    ];
    let required = vec![
        RequiredFile {
            path: "adb.exe".into(),
            size: 60,
            sha256: sha256_hex(&a),
        },
        RequiredFile {
            path: "AdbWinApi.dll".into(),
            size: 60,
            sha256: sha256_hex(&b),
        },
    ];
    let opts = ExtractOptions {
        max_total_uncompressed: 100,
        max_file_uncompressed: 100,
    };
    let (root, result) = extract(&entries, &required, &opts);
    assert!(
        matches!(result, Err(ArchiveError::BombTotal { .. })),
        "总量炸弹必须被拒，实际 {result:?}"
    );
    cleanup(&root);
}

#[test]
fn unexpected_entry_rejected() {
    let entries = vec![
        ZipEntrySpec::file("adb.exe", b"adb-binary"),
        ZipEntrySpec::file("extra.txt", b"nope"),
    ];
    let (root, result) = extract(&entries, &required_from(GOOD), &ExtractOptions::default());
    assert!(
        matches!(result, Err(ArchiveError::UnexpectedEntry { .. })),
        "白名单外条目必须被拒，实际 {result:?}"
    );
    cleanup(&root);
}

#[test]
fn unexpected_directory_rejected() {
    let entries = vec![
        ZipEntrySpec::file("adb.exe", b"adb-binary"),
        ZipEntrySpec::dir("unrelated"),
    ];
    let (root, result) = extract(&entries, &required_from(GOOD), &ExtractOptions::default());
    assert!(
        matches!(result, Err(ArchiveError::UnexpectedEntry { .. })),
        "无关目录条目必须被拒，实际 {result:?}"
    );
    cleanup(&root);
}

#[test]
fn missing_required_file_detected() {
    let entries = vec![ZipEntrySpec::file("adb.exe", b"adb-binary")];
    let (root, result) = extract(&entries, &required_from(GOOD), &ExtractOptions::default());
    match result {
        Err(ArchiveError::RequiredFileMissing { path }) => assert_eq!(path, "AdbWinApi.dll"),
        other => panic!("缺失 required_files 必须报错，实际 {other:?}"),
    }
    cleanup(&root);
}

#[test]
fn required_hash_mismatch_detected() {
    // 同长度篡改：size 校验通过、hash 校验必须拦截
    let entries = vec![
        ZipEntrySpec::file("adb.exe", b"TAMPERED!!"),
        ZipEntrySpec::file("AdbWinApi.dll", b"dll-bytes"),
    ];
    let (root, result) = extract(&entries, &required_from(GOOD), &ExtractOptions::default());
    match result {
        Err(ArchiveError::RequiredFileHash { path, .. }) => assert_eq!(path, "adb.exe"),
        other => panic!("hash 不符必须报错，实际 {other:?}"),
    }
    cleanup(&root);
}

#[test]
fn required_size_mismatch_detected() {
    // 不同长度篡改：size 校验先行拦截
    let entries = vec![
        ZipEntrySpec::file("adb.exe", b"TAMPERED!!!"),
        ZipEntrySpec::file("AdbWinApi.dll", b"dll-bytes"),
    ];
    let (root, result) = extract(&entries, &required_from(GOOD), &ExtractOptions::default());
    match result {
        Err(ArchiveError::RequiredFileSize {
            path,
            actual: 11,
            expected: 10,
        }) => {
            assert_eq!(path, "adb.exe");
        }
        other => panic!("size 不符必须报错，实际 {other:?}"),
    }
    cleanup(&root);
}

#[test]
fn nonempty_staging_rejected() {
    let root = unique_root("staging-nonempty");
    let stage = staging(&root);
    fs::create_dir_all(&stage).unwrap();
    fs::write(stage.join("leftover.bin"), b"old").unwrap();
    let zip_path = root.join("comp.zip");
    build_zip(&zip_path, &[ZipEntrySpec::file("adb.exe", b"adb-binary")]);
    let result = archive::extract_component_zip(
        &zip_path,
        &stage,
        &required_from(&[("adb.exe", b"adb-binary")]),
        &ExtractOptions::default(),
    );
    assert!(matches!(result, Err(ArchiveError::StagingInvalid { .. })));
    assert_eq!(
        fs::read(stage.join("leftover.bin")).unwrap(),
        b"old",
        "非空 staging 不得被动"
    );
    cleanup(&root);
}

#[test]
fn install_staged_refuses_existing_target_and_renames_on_success() {
    let root = unique_root("install");
    let stage = staging(&root);
    fs::create_dir_all(&stage).unwrap();
    fs::write(stage.join("adb.exe"), b"adb-binary").unwrap();
    let target = root.join("runtime").join("adb").join("1.0.0");
    fs::create_dir_all(&target).unwrap();

    let result = archive::install_staged(&stage, &target);
    assert!(
        matches!(result, Err(ArchiveError::TargetExists { .. })),
        "目标已存在必须拒绝"
    );
    assert!(stage.is_dir(), "失败后 staging 保留（调用方清理）");

    fs::remove_dir_all(&target).unwrap();
    archive::install_staged(&stage, &target).expect("目标不存在时应 rename 成功");
    assert!(!stage.exists(), "成功后 staging 应原地消失（同卷 rename）");
    assert_eq!(fs::read(target.join("adb.exe")).unwrap(), b"adb-binary");
    cleanup(&root);
}
