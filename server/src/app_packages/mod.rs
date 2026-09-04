#![allow(dead_code, unused_imports)]

//! Phase 4 App Package storage boundary.
//!
//! REST wiring (`api/app_packages.rs`) installs/activates packages; resource
//! consumers reach packages through the composite seam in [`composite`]
//! (editable local partition → user-overrides → active App Package) used by
//! `scripts.rs` / `keymaps.rs` / the engine matcher adapters.

mod archive;
/// PackageBuilder：工作区 → `.gamerpkg` 导出流水线（builder/导出 API 专用）。
pub(crate) mod builder;
mod composite;
mod error;
mod manifest;
mod model;
mod presets;
mod resolver;
mod store;
/// 本地编辑区元数据与统计（package.toml / workspace API 专用）。
pub(crate) mod workspace;

pub(crate) use archive::{
    MAX_PACKAGE_ARCHIVE_BYTES, MAX_PACKAGE_ENTRIES, MAX_PACKAGE_FILE_BYTES, MAX_PACKAGE_TOTAL_BYTES,
};
pub(crate) use composite::{
    ActivePackage, CompositeHit, CompositeResolver, CompositeSource, TemplateLookup,
};
pub(crate) use error::{AppPackageError, AppPackageResult};
pub(crate) use manifest::{parse_manifest, PackageManifest, MANIFEST_FORMAT_VERSION};
pub(crate) use model::{
    parse_android_package_name, parse_app_package_id, resource_id, AndroidPackageName,
    AppPackageId, InstalledVersion, ResourceId, ResourceKind, ResourcePath,
};
pub(crate) use presets::PresetDeclaration;
pub(crate) use resolver::{ResolvedResource, ResourceResolver, ResourceSource};
pub(crate) use store::{
    ActiveRegistry, AppPackagePresetHook, AppPackageStore, AppPackageTaskHook, InstallMeta,
    InstalledPackage, SchedulerTaskSuspendedHook, TimerPresetPublishHook, TimerTaskSuspendedHook,
};

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
        format!(
            "format_version = 2\nid = \"{id}\"\nversion = \"{version}\"\n[android]\npackages = [\"{android}\"]\n"
        )
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
format_version = 2
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
format_version = 2
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

    /// Manifest V2 门禁：必填 format_version，且只接受 2。
    #[test]
    fn manifest_requires_format_version_two() {
        let android = "[android]\npackages = [\"com.example.game\"]\n";
        let missing = format!("id = \"official.xxx\"\nversion = \"1.2.0\"\n{android}");
        let err = parse_manifest(missing.as_bytes()).unwrap_err();
        assert!(err.to_string().contains("缺少 format_version"), "{err}");

        for version in ["1", "3"] {
            let wrong = format!(
                "format_version = {version}\nid = \"official.xxx\"\nversion = \"1.2.0\"\n{android}"
            );
            let err = parse_manifest(wrong.as_bytes()).unwrap_err();
            assert!(
                err.to_string().contains("format_version 不为 2"),
                "format_version={version}: {err}"
            );
        }

        let ok =
            format!("format_version = 2\nid = \"official.xxx\"\nversion = \"1.2.0\"\n{android}");
        assert!(parse_manifest(ok.as_bytes()).is_ok());
    }

    /// 包内 functions/ 可安装（ResourceKind::Functions），且函数库内容只进
    /// functions 索引、绝不混入 scripts 索引。
    #[tokio::test]
    async fn install_accepts_functions_directory_and_keeps_indexes_separate() {
        let temp = TempDir::new().unwrap();
        let store = AppPackageStore::new(temp.path());
        let manifest = package_manifest("official.fn", "1.0.0", "com.example.game");
        let package = archive(vec![
            ("manifest.toml", &manifest),
            ("functions/common.yaml", b"login:\n  steps: []\n"),
            ("scripts/daily.yaml", b"steps: []\n"),
        ]);
        store.install_and_activate(&package, None).await.unwrap();

        let resolver = CompositeResolver::new(temp.path());
        let scripts = resolver.script_sources("com.example.game").unwrap();
        assert!(scripts.contains_key("daily.yaml"));
        assert!(
            !scripts.contains_key("common.yaml"),
            "functions/ 内容不得进入脚本索引"
        );
        let functions = resolver.function_sources("com.example.game").unwrap();
        assert!(functions.contains_key("common.yaml"));
        assert!(
            !functions.contains_key("daily.yaml"),
            "scripts/ 内容不得进入函数库索引"
        );
    }

    #[tokio::test]
    async fn fresh_store_has_zero_business_resources() {
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
            store
                .uninstall(&unsafe_package, &InstalledVersion::parse("1.0.0").unwrap())
                .await,
            Err(AppPackageError::InvalidAppPackageId(_))
        ));
    }

    /// list_installed 容错：单个损坏版本目录（无 format_version 的旧格式
    /// manifest / 目录与 manifest 不一致）只跳过 + tracing::warn，不炸整个列表；
    /// uninstall 按目录名删除、无需 manifest 解析，仍可清理这类目录。
    #[tokio::test]
    async fn list_installed_skips_corrupt_version_directories_with_warning() {
        let temp = TempDir::new().unwrap();
        let store = AppPackageStore::new(temp.path());
        let good = archive(vec![(
            "manifest.toml",
            &package_manifest("official.good", "1.0.0", "com.example.game"),
        )]);
        store.install_archive(&good, None).unwrap();

        // 手工放一个旧格式（缺 format_version）的版本目录
        let legacy_root = temp.path().join("app-packages/official.bad/0.9.0");
        std::fs::create_dir_all(&legacy_root).unwrap();
        std::fs::write(
            legacy_root.join("manifest.toml"),
            br#"id = "official.bad"
version = "0.9.0"
[android]
packages = ["com.example.game"]
"#,
        )
        .unwrap();
        // 再放一个目录名与 manifest 不一致的版本目录
        let mismatched_root = temp.path().join("app-packages/official.bad/9.9.9");
        std::fs::create_dir_all(&mismatched_root).unwrap();
        std::fs::write(
            mismatched_root.join("manifest.toml"),
            package_manifest("official.bad", "1.0.0", "com.example.game"),
        )
        .unwrap();

        let installed = store.list_installed().unwrap();
        assert_eq!(installed.len(), 1, "损坏版本目录必须被跳过");
        assert_eq!(installed[0].manifest().id().as_str(), "official.good");

        // uninstall 仍可按 id/version 删除损坏目录（不解析 manifest）
        let bad_id = parse_app_package_id("official.bad").unwrap();
        assert!(store
            .uninstall(&bad_id, &InstalledVersion::parse("0.9.0").unwrap())
            .await
            .unwrap());
        assert!(!legacy_root.exists());
        assert_eq!(store.list_installed().unwrap().len(), 1);
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

        let installed = store.install_archive(&package, None).unwrap();
        assert!(installed.root().join("manifest.toml").is_file());
        let meta = store
            .install_meta(installed.manifest().id(), installed.manifest().version())
            .unwrap()
            .expect("install metadata recorded");
        assert_eq!(meta.sha256, {
            use sha2::{Digest, Sha256};
            format!("{:x}", Sha256::digest(&package))
        });
        assert_eq!(store.list_installed().unwrap().len(), 1);
        assert!(matches!(
            store.install_archive(&package, None),
            Err(AppPackageError::AlreadyInstalled { .. })
        ));
        let wrong_digest = store.install_archive(&package, Some(&"0".repeat(64)));
        assert!(matches!(
            wrong_digest,
            Err(AppPackageError::Sha256Mismatch { .. })
        ));
        let broken = archive(vec![
            (
                "manifest.toml",
                &package_manifest("official.xxx", "1.3.0", "com.example.game"),
            ),
            ("../outside.txt", b"must not stage"),
        ]);
        assert!(store.install_archive(&broken, None).is_err());
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

    #[tokio::test]
    async fn override_wins_and_uninstall_preserves_base_and_override_data() {
        let temp = TempDir::new().unwrap();
        let store = AppPackageStore::new(temp.path());
        let base_marker = temp.path().join("base-capability.marker");
        std::fs::write(&base_marker, b"base remains").unwrap();
        let manifest = package_manifest("official.xxx", "1.2.0", "com.example.game");
        let package = archive(vec![
            ("manifest.toml", &manifest),
            ("templates/main.png", b"installed"),
        ]);
        let installed = store.install_archive(&package, None).unwrap();
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
        let updated = store.install_archive(&updated_package, None).unwrap();
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
            .await
            .unwrap());
        assert!(store
            .uninstall(updated.manifest().id(), updated.manifest().version())
            .await
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
        let installed = store.install_archive(&package, None).unwrap();
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

    /// ResourceResolver 三层顺序：**本地编辑区（分区目录）→ override → 指定
    /// 版本安装包**。运行时读路径（keymap 扩展按包版本加载 profile）与
    /// CompositeResolver 优先级保持一致：用户改本地副本立即生效。
    #[test]
    fn resolver_prefers_editable_local_then_override_then_installed() {
        let temp = TempDir::new().unwrap();
        let store = AppPackageStore::new(temp.path());
        let manifest = package_manifest("official.xxx", "1.2.0", "com.example.game");
        let package = archive(vec![
            ("manifest.toml", &manifest),
            ("keymaps/default.yaml", b"installed"),
        ]);
        let installed = store.install_archive(&package, None).unwrap();
        let android = parse_android_package_name("com.example.game").unwrap();
        let id = resource_id(
            installed.manifest().id().clone(),
            installed.manifest().version(),
            &ResourcePath::parse("keymaps/default.yaml").unwrap(),
        )
        .unwrap();
        let resolver = store.resolver();

        let resolved = resolver.resolve(&android, &id).unwrap().unwrap();
        assert_eq!(resolved.read_bytes().unwrap(), b"installed");

        // 本地编辑区（分区 keymaps/）出现同名文件 → 最高优先
        let local = temp.path().join("com.example.game/keymaps/default.yaml");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(&local, b"editable").unwrap();
        let resolved = resolver.resolve(&android, &id).unwrap().unwrap();
        assert!(matches!(
            resolved.source(),
            ResourceSource::EditableLocal { .. }
        ));
        assert_eq!(resolved.read_bytes().unwrap(), b"editable");

        // 删本地 → override 层可见
        std::fs::remove_file(&local).unwrap();
        let override_file = temp
            .path()
            .join("user-overrides/com.example.game/keymaps/default.yaml");
        std::fs::create_dir_all(override_file.parent().unwrap()).unwrap();
        std::fs::write(&override_file, b"override").unwrap();
        let resolved = resolver.resolve(&android, &id).unwrap().unwrap();
        assert!(matches!(
            resolved.source(),
            ResourceSource::UserOverride { .. }
        ));
        assert_eq!(resolved.read_bytes().unwrap(), b"override");

        // 删 override → 指定版本安装包内容重新可见
        std::fs::remove_file(&override_file).unwrap();
        let resolved = resolver.resolve(&android, &id).unwrap().unwrap();
        assert!(matches!(
            resolved.source(),
            ResourceSource::Installed { .. }
        ));
        assert_eq!(resolved.read_bytes().unwrap(), b"installed");
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
        let store = AppPackageStore::with_task_hook(
            temp.path(),
            Arc::new(TimerTaskSuspendedHook::new(timer)),
        );
        let manifest = package_manifest("official.xxx", "1.2.0", "com.example.game");
        let package = archive(vec![("manifest.toml", &manifest)]);
        let installed = store.install_archive(&package, None).unwrap();
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

        assert!(store
            .uninstall(installed.manifest().id(), installed.manifest().version())
            .await
            .unwrap());
        let saved = db.get_timer_task_async("task-1").await.unwrap().unwrap();
        assert_eq!(saved.state, TimerTaskState::Suspended);
        assert_eq!(
            saved.suspend_reason.as_deref(),
            Some("app package unavailable")
        );

        // A repeated lifecycle notification is idempotent and does not delete
        // the persisted User Task.
        assert!(!store
            .uninstall(installed.manifest().id(), installed.manifest().version())
            .await
            .unwrap());
        assert!(db.get_timer_task_async("task-1").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn install_and_activate_publishes_presets_and_enforces_primary_uniqueness() {
        use std::sync::Mutex;

        #[derive(Default)]
        struct RecordingPresetHook(Mutex<Vec<(String, Vec<String>)>>);

        #[async_trait::async_trait]
        impl AppPackagePresetHook for RecordingPresetHook {
            async fn publish_presets(
                &self,
                package: &AppPackageId,
                presets: &[PresetDeclaration],
            ) -> anyhow::Result<usize> {
                self.0.lock().unwrap().push((
                    package.as_str().to_string(),
                    presets.iter().map(|preset| preset.name.clone()).collect(),
                ));
                Ok(presets.len())
            }
        }

        let temp = TempDir::new().unwrap();
        let hook = std::sync::Arc::new(RecordingPresetHook::default());
        let store = AppPackageStore::with_hooks(
            temp.path(),
            Arc::new(crate::app_packages::store::NoopAppPackageTaskHook),
            hook.clone(),
        );

        let preset_body = br#"name: daily
runner_id: gamer.yaml
entrypoint: run
schedule:
  kind: cron
  value:
    expression: "0 8 * * *"
"#;
        let first = archive(vec![
            (
                "manifest.toml",
                &package_manifest("official.a", "1.0.0", "com.example.game"),
            ),
            ("templates/main.png", b"a-bytes"),
            ("presets/daily.yaml", &preset_body[..]),
        ]);
        let installed = store.install_and_activate(&first, None).await.unwrap();
        assert_eq!(
            store
                .active_version(installed.manifest().id())
                .unwrap()
                .unwrap()
                .as_str(),
            "1.0.0"
        );
        // 作用域内断言并释放锁（clippy await_holding_lock 不识别显式 drop）
        {
            let published = hook.0.lock().unwrap();
            assert_eq!(published.len(), 1);
            assert_eq!(published[0].0, "official.a");
            assert_eq!(published[0].1, vec!["daily".to_string()]);
        }

        // A second content package claiming the same Android target conflicts
        // with the active primary and must not stage anything.
        let conflicting = archive(vec![(
            "manifest.toml",
            &package_manifest("official.b", "2.0.0", "com.example.game"),
        )]);
        let conflict = store
            .install_and_activate(&conflicting, None)
            .await
            .unwrap_err();
        assert!(matches!(conflict, AppPackageError::PrimaryConflict { .. }));
        assert!(store.list_installed().unwrap().len() == 1);

        // Unrelated Android target installs and activates alongside.
        let other = archive(vec![(
            "manifest.toml",
            &package_manifest("official.b", "2.0.0", "com.other.game"),
        )]);
        let other = store.install_and_activate(&other, None).await.unwrap();
        assert_eq!(
            store
                .active_version(other.manifest().id())
                .unwrap()
                .unwrap()
                .as_str(),
            "2.0.0"
        );

        // Explicit activation of a specific version re-targets the registry.
        let upgraded = archive(vec![(
            "manifest.toml",
            &package_manifest("official.a", "1.1.0", "com.example.game"),
        )]);
        let upgraded = store.install_and_activate(&upgraded, None).await.unwrap();
        assert_eq!(
            store
                .active_version(upgraded.manifest().id())
                .unwrap()
                .unwrap()
                .as_str(),
            "1.1.0"
        );

        // Uninstalling the active version clears the registry entry only.
        assert!(store
            .uninstall(upgraded.manifest().id(), upgraded.manifest().version())
            .await
            .unwrap());
        assert!(store
            .active_version(upgraded.manifest().id())
            .unwrap()
            .is_none());
    }
}
