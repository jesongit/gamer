use super::*;

#[test]
fn catalog_has_exact_verified_windows_artifacts_and_fixed_models() {
    for id in ["adb", "ffmpeg", "tesseract", "browser", "tessdata"] {
        let a = install::artifact(id).unwrap();
        assert!(!a.version.is_empty());
        if id != "tessdata" {
            assert_eq!(a.sha256.len(), 64);
            assert!(a.url.starts_with("https://"));
        }
    }
}

#[tokio::test]
async fn explicit_missing_path_never_falls_back_or_installs() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = Config {
        data_dir: temp.path().join("data"),
        adb_path: temp
            .path()
            .join("missing-adb")
            .to_string_lossy()
            .into_owned(),
        ..Default::default()
    };
    let original = selection(&cfg, "adb").unwrap();
    let result = inspect(&cfg, "adb").await.unwrap();
    assert_eq!(result.status, "broken");
    assert!(prepare(&cfg, "adb").await.is_err());
    assert_eq!(selection(&cfg, "adb").unwrap(), original);
    assert!(!root(&cfg).exists());
}

#[tokio::test]
async fn config_optimistic_concurrency_and_user_files_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let cfg = Config {
        data_dir: data,
        ..Default::default()
    };
    let custom = temp.path().join("my-adb");
    std::fs::write(&custom, b"original").unwrap();
    save(&cfg, "adb", &custom.to_string_lossy(), "")
        .await
        .unwrap();
    assert!(save(&cfg, "adb", "", "").await.is_err());
    assert_eq!(std::fs::read(&custom).unwrap(), b"original");
    assert_eq!(selection(&cfg, "adb").unwrap(), custom.to_string_lossy());
}

#[test]
fn models_use_a_complete_directory_without_modifying_existing_models() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = Config {
        data_dir: temp.path().join("data"),
        ..Default::default()
    };
    let owned = temp.path().join("custom-models");
    std::fs::create_dir(&owned).unwrap();
    std::fs::write(owned.join("eng.traineddata"), b"user").unwrap();
    let target = ocr_path(&cfg).unwrap();
    let mut settings = crate::ocr::settings::Settings {
        engine_path: String::new(),
        tessdata_path: owned.to_string_lossy().into_owned(),
        version: None,
    };
    settings.write(&target).unwrap();
    assert_eq!(
        model_directory(&cfg, None).unwrap().unwrap(),
        owned.canonicalize().unwrap()
    );
    assert_eq!(
        std::fs::read(owned.join("eng.traineddata")).unwrap(),
        b"user"
    );
}

#[test]
fn archive_traversal_and_wrong_payload_digest_reject_before_publication() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("x.zip");
    let out = temp.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let mut z = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
    z.start_file("../outside", zip::write::SimpleFileOptions::default())
        .unwrap();
    z.write_all(b"bad").unwrap();
    z.finish().unwrap();
    assert!(install::extract_zip(&archive, &out, &[]).is_err());
    assert!(!temp.path().join("outside").exists());
}

#[tokio::test]
#[ignore = "downloads a pinned runtime into an isolated directory; opt-in network acceptance"]
async fn install_missing_tesseract_without_system_7zip() {
    if !cfg!(all(windows, target_arch = "x86_64")) {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let cfg = Config {
        data_dir: temp.path().join("data"),
        ..Default::default()
    };
    // Calls the real installer directly so a developer's system installation
    // cannot turn missing-dependency acceptance into a no-op reuse check.
    install::install(&cfg, "tesseract").await.unwrap();
    let path = managed(&cfg, "tesseract").unwrap();
    let version = probe::verify("tesseract", &path).await.unwrap().unwrap();
    assert!(version.starts_with("tesseract 5.") || version.starts_with("tesseract v5."));
    let prior = std::fs::read(&path).unwrap();
    assert!(install::install(&cfg, "tesseract").await.is_err());
    assert_eq!(std::fs::read(path).unwrap(), prior);
}
