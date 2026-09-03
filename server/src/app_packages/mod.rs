#![allow(dead_code, unused_imports)]

//! Phase 4 App Package storage boundary.
//!
//! This module is intentionally not wired into REST or any existing script,
//! keymap, matcher, WebRTC, or database path yet. It provides the safe storage
//! and resolution seam for those later adapters.

mod archive;
mod error;
mod manifest;
mod model;
mod resolver;
mod store;

pub(crate) use archive::{
    MAX_PACKAGE_ARCHIVE_BYTES, MAX_PACKAGE_ENTRIES, MAX_PACKAGE_FILE_BYTES, MAX_PACKAGE_TOTAL_BYTES,
};
pub(crate) use error::{AppPackageError, AppPackageResult};
pub(crate) use manifest::{parse_manifest, PackageManifest};
pub(crate) use model::{
    parse_android_package_name, parse_app_package_id, resource_id, AndroidPackageName,
    AppPackageId, InstalledVersion, ResourceId, ResourceKind, ResourcePath,
};
pub(crate) use resolver::{ResolvedResource, ResourceResolver, ResourceSource};
pub(crate) use store::{AppPackageStore, InstalledPackage};

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::Arc;

    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;

    use super::*;

    use crate::config::Config;
    use crate::core::AppContext;
    use crate::timer_core::{ScheduleSpec, TimerCore, TimerTask, TimerTaskState};

    fn package_manifest(id: &str, version: &str, android: &str) -> Vec<u8> {
        format!("id = \"{id}\"\nversion = \"{version}\"\n[android]\npackages = [\"{android}\"]\n")
            .into_bytes()
    }

    fn archive(entries: Vec<(&str, &[u8])>) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            for (name, content) in entries {
                writer.start_file(name, options).unwrap();
                writer.write_all(content).unwrap();
            }
            writer.finish().unwrap();
        }
        bytes
    }

    fn duplicate_entry(mut bytes: Vec<u8>, wanted: &str) -> Vec<u8> {
        let eocd = (0..=bytes.len() - 22)
            .rev()
            .find(|&position| bytes[position..position + 4] == [0x50, 0x4b, 0x05, 0x06])
            .unwrap();
        let central_start =
            u32::from_le_bytes(bytes[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        let central_end = eocd;
        let mut cursor = central_start;
        let mut record = None;
        while cursor < central_end {
            assert_eq!(&bytes[cursor..cursor + 4], &[0x50, 0x4b, 0x01, 0x02]);
            let name_len =
                u16::from_le_bytes(bytes[cursor + 28..cursor + 30].try_into().unwrap()) as usize;
            let extra_len =
                u16::from_le_bytes(bytes[cursor + 30..cursor + 32].try_into().unwrap()) as usize;
            let comment_len =
                u16::from_le_bytes(bytes[cursor + 32..cursor + 34].try_into().unwrap()) as usize;
            let end = cursor + 46 + name_len + extra_len + comment_len;
            if &bytes[cursor + 46..cursor + 46 + name_len] == wanted.as_bytes() {
                record = Some(bytes[cursor..end].to_vec());
                break;
            }
            cursor = end;
        }
        let record = record.expect("duplicate fixture target must exist");
        let old_eocd = bytes.split_off(eocd);
        bytes.extend_from_slice(&record);
        let new_eocd = bytes.len();
        bytes.extend_from_slice(&old_eocd);
        let entries = u16::from_le_bytes(bytes[new_eocd + 10..new_eocd + 12].try_into().unwrap());
        bytes[new_eocd + 8..new_eocd + 10].copy_from_slice(&(entries + 1).to_le_bytes());
        bytes[new_eocd + 10..new_eocd + 12].copy_from_slice(&(entries + 1).to_le_bytes());
        let size = u32::from_le_bytes(bytes[new_eocd + 12..new_eocd + 16].try_into().unwrap());
        bytes[new_eocd + 12..new_eocd + 16]
            .copy_from_slice(&(size + record.len() as u32).to_le_bytes());
        bytes
    }

    fn ids() -> (
        AppPackageId,
        InstalledVersion,
        AndroidPackageName,
        ResourcePath,
    ) {
        (
            parse_app_package_id("official.xxx").unwrap(),
            InstalledVersion::parse("1.2.0").unwrap(),
            parse_android_package_name("com.example.game").unwrap(),
            ResourcePath::parse("templates/main.png").unwrap(),
        )
    }

    #[test]
    fn app_package_id_is_not_android_package_name() {
        let content_id = parse_app_package_id("official-game").unwrap();
        assert_eq!(content_id.as_str(), "official-game");
        assert!(parse_android_package_name(content_id.as_str()).is_err());
        assert!(parse_android_package_name("com.example.game").is_ok());
        assert!(parse_app_package_id("com.example.game").is_ok());
    }

    #[test]
    fn manifest_requires_identity_and_unique_android_targets() {
        let valid = package_manifest("official.xxx", "1.2.0", "com.example.game");
        let manifest = parse_manifest(&valid).unwrap();
        assert_eq!(manifest.id().as_str(), "official.xxx");
        assert_eq!(manifest.version().as_str(), "1.2.0");
        assert!(manifest
            .supports_android_package(&parse_android_package_name("com.example.game").unwrap()));

        let duplicate = br#"
id = "official.xxx"
version = "1.2.0"
[android]
packages = ["com.example.game", "com.example.game"]
"#;
        assert!(matches!(
            parse_manifest(duplicate),
            Err(AppPackageError::InvalidManifest(_))
        ));

        let unknown = br#"
id = "official.xxx"
version = "1.2.0"
unexpected = true
[android]
packages = ["com.example.game"]
"#;
        assert!(matches!(
            parse_manifest(unknown),
            Err(AppPackageError::InvalidManifest(_))
        ));
    }

    #[test]
    fn fresh_store_has_zero_business_resources() {
        let temp = TempDir::new().unwrap();
        let store = AppPackageStore::new(temp.path());
        let (package, version, android, path) = ids();
        let id = resource_id(package, &version, &path).unwrap();

        assert!(store.list_installed().unwrap().is_empty());
        assert!(store.resolver().resolve(&android, &id).unwrap().is_none());
        assert!(!temp.path().join("app-packages").exists());

        // Core's generic identifier constructor is intentionally broader than
        // a storage path; the App Package boundary must revalidate it.
        let unsafe_package = AppPackageId::new("..").unwrap();
        assert!(matches!(
            store.uninstall(&unsafe_package, &InstalledVersion::parse("1.0.0").unwrap()),
            Err(AppPackageError::InvalidAppPackageId(_))
        ));
    }

    #[test]
    fn archive_rejects_size_traversal_duplicates_and_entry_count() {
        let oversized = vec![0u8; MAX_PACKAGE_ARCHIVE_BYTES + 1];
        assert!(matches!(
            super::archive::validate_and_read_manifest(&oversized),
            Err(AppPackageError::ArchiveTooLarge { .. })
        ));

        let manifest = package_manifest("official.xxx", "1.2.0", "com.example.game");
        let traversal = archive(vec![
            ("manifest.toml", &manifest),
            ("../outside.txt", b"nope"),
        ]);
        assert!(matches!(
            super::archive::validate_and_read_manifest(&traversal),
            Err(AppPackageError::InvalidArchive(_) | AppPackageError::InvalidResourcePath(_))
        ));

        let duplicate = duplicate_entry(
            archive(vec![
                ("manifest.toml", &manifest),
                ("templates/a.txt", b"one"),
            ]),
            "templates/a.txt",
        );
        assert!(matches!(
            super::archive::validate_and_read_manifest(&duplicate),
            Err(AppPackageError::InvalidArchive(_))
        ));

        let mut many = vec![("manifest.toml", manifest.as_slice())];
        let names: Vec<String> = (0..MAX_PACKAGE_ENTRIES)
            .map(|index| format!("resources/{index}.bin"))
            .collect();
        for name in &names {
            many.push((name.as_str(), b"x"));
        }
        let too_many = archive(many);
        assert!(matches!(
            super::archive::validate_and_read_manifest(&too_many),
            Err(AppPackageError::InvalidArchive(_))
        ));
    }

    #[test]
    fn install_is_immutable_and_staged() {
        let temp = TempDir::new().unwrap();
        let store = AppPackageStore::new(temp.path());
        let manifest = package_manifest("official.xxx", "1.2.0", "com.example.game");
        let package = archive(vec![
            ("manifest.toml", &manifest),
            ("templates/main.png", b"package-bytes"),
            ("scripts/daily.yaml", b"steps: []\n"),
        ]);

        let installed = store.install_archive(&package).unwrap();
        assert!(installed.root().join("manifest.toml").is_file());
        assert_eq!(store.list_installed().unwrap().len(), 1);
        assert!(matches!(
            store.install_archive(&package),
            Err(AppPackageError::AlreadyInstalled { .. })
        ));
        let broken = archive(vec![
            (
                "manifest.toml",
                &package_manifest("official.xxx", "1.3.0", "com.example.game"),
            ),
            ("../outside.txt", b"must not stage"),
        ]);
        assert!(store.install_archive(&broken).is_err());
        assert!(!store
            .data_root()
            .join("app-packages/official.xxx/1.3.0")
            .exists());
        assert!(!store
            .data_root()
            .join("app-packages/.staging")
            .read_dir()
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false));
    }

    #[test]
    fn override_wins_and_uninstall_preserves_base_and_override_data() {
        let temp = TempDir::new().unwrap();
        let store = AppPackageStore::new(temp.path());
        let base_marker = temp.path().join("base-capability.marker");
        std::fs::write(&base_marker, b"base remains").unwrap();
        let manifest = package_manifest("official.xxx", "1.2.0", "com.example.game");
        let package = archive(vec![
            ("manifest.toml", &manifest),
            ("templates/main.png", b"installed"),
        ]);
        let installed = store.install_archive(&package).unwrap();
        let android = parse_android_package_name("com.example.game").unwrap();
        let path = ResourcePath::parse("templates/main.png").unwrap();
        store
            .write_user_override(&android, &path, b"user override")
            .unwrap();

        let id = resource_id(
            installed.manifest().id().clone(),
            installed.manifest().version(),
            &path,
        )
        .unwrap();
        let resolved = store.resolver().resolve(&android, &id).unwrap().unwrap();
        assert_eq!(resolved.read_bytes().unwrap(), b"user override");
        assert!(matches!(
            resolved.source(),
            ResourceSource::UserOverride { .. }
        ));

        let updated_manifest = package_manifest("official.xxx", "1.3.0", "com.example.game");
        let updated_package = archive(vec![
            ("manifest.toml", &updated_manifest),
            ("templates/main.png", b"updated installed bytes"),
        ]);
        let updated = store.install_archive(&updated_package).unwrap();
        let updated_id = resource_id(
            updated.manifest().id().clone(),
            updated.manifest().version(),
            &path,
        )
        .unwrap();
        let updated_resolved = store
            .resolver()
            .resolve(&android, &updated_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated_resolved.read_bytes().unwrap(), b"user override");

        assert!(store
            .uninstall(installed.manifest().id(), installed.manifest().version())
            .unwrap());
        assert!(store
            .uninstall(updated.manifest().id(), updated.manifest().version())
            .unwrap());
        assert!(base_marker.is_file());
        assert!(temp
            .path()
            .join("user-overrides/com.example.game/templates/main.png")
            .is_file());
        assert_eq!(store.list_installed().unwrap().len(), 0);
    }

    #[test]
    fn resolver_returns_installed_resource_and_filters_android_targets() {
        let temp = TempDir::new().unwrap();
        let store = AppPackageStore::new(temp.path());
        let manifest = package_manifest("official.xxx", "1.2.0", "com.example.game");
        let package = archive(vec![
            ("manifest.toml", &manifest),
            ("resources/config.json", b"{}"),
        ]);
        let installed = store.install_archive(&package).unwrap();
        let id = resource_id(
            installed.manifest().id().clone(),
            installed.manifest().version(),
            &ResourcePath::parse("resources/config.json").unwrap(),
        )
        .unwrap();
        let supported = parse_android_package_name("com.example.game").unwrap();
        let unsupported = parse_android_package_name("com.other.game").unwrap();
        let resolved = store.resolver().resolve(&supported, &id).unwrap().unwrap();
        assert_eq!(resolved.read_bytes().unwrap(), b"{}");
        assert!(matches!(
            resolved.source(),
            ResourceSource::Installed { .. }
        ));
        assert!(store
            .resolver()
            .resolve(&unsupported, &id)
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn uninstalling_last_package_version_suspends_tasks_without_deleting_them() {
        let temp = TempDir::new().unwrap();
        let cfg = Config {
            data_dir: temp.path().to_path_buf(),
            ..Default::default()
        };
        let db = Arc::new(crate::store::Store::open(&cfg).unwrap());
        let timer = TimerCore::new(db.clone());
        let store = AppPackageStore::new(temp.path());
        let manifest = package_manifest("official.xxx", "1.2.0", "com.example.game");
        let package = archive(vec![("manifest.toml", &manifest)]);
        let installed = store.install_archive(&package).unwrap();
        let task = TimerTask::new(
            "task-1",
            "Task",
            AppContext::new(
                crate::core::DeviceId::new("device-1").unwrap(),
                crate::core::AndroidPackageName::new("com.example.game").unwrap(),
                Some(crate::core::AppPackageId::new("official.xxx").unwrap()),
            ),
            "runner.example",
            "entry",
            serde_json::json!({}),
            ScheduleSpec::new("cron", serde_json::json!({"expression": "* * * * *"})).unwrap(),
        )
        .unwrap();
        db.upsert_timer_task_async(&task).await.unwrap();

        let (removed, suspended) = store
            .uninstall_and_update_tasks(
                installed.manifest().id(),
                installed.manifest().version(),
                &timer,
            )
            .await
            .unwrap();
        assert!(removed);
        assert_eq!(suspended, 1);
        let saved = db.get_timer_task_async("task-1").await.unwrap().unwrap();
        assert_eq!(saved.state, TimerTaskState::Suspended);
        assert_eq!(
            saved.suspend_reason.as_deref(),
            Some("app package unavailable")
        );

        // A repeated lifecycle notification is idempotent and does not delete
        // the persisted User Task.
        assert_eq!(
            store
                .uninstall_and_update_tasks(
                    installed.manifest().id(),
                    installed.manifest().version(),
                    &timer,
                )
                .await
                .unwrap(),
            (false, 0)
        );
        assert!(db.get_timer_task_async("task-1").await.unwrap().is_some());
    }
}
