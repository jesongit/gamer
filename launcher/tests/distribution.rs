mod common;
use common::{cleanup, unique_root};
use gamer_launcher::{distribution, layout::InstallLayout, manifest::model::Manifest};

#[test]
fn launch_uses_pinned_dependencies_even_when_newer_runtime_exists() {
    let root = unique_root("pins");
    let layout = InstallLayout::resolve(Some(root.clone()));
    let value = serde_json::from_str(include_str!(
        "../../release/contracts/fixtures/manifest/valid/manifest-valid-full.json"
    ))
    .unwrap();
    let manifest = Manifest::parse(&value).unwrap();
    for id in ["adb", "ffmpeg"] {
        let dir = layout.runtime_dir().join(id).join("99.0.0");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.exe")), b"wrong version").unwrap();
    }
    let plan = distribution::plan(&layout, &manifest).unwrap();
    let platform = &manifest.platforms[distribution::PLATFORM];
    for (id, path) in [
        ("adb", plan.adb_path.unwrap()),
        ("ffmpeg", plan.ffmpeg_path.unwrap()),
    ] {
        let spec = platform.components.iter().find(|c| c.id == id).unwrap();
        assert_eq!(
            path,
            layout
                .runtime_dir()
                .join(id)
                .join(&spec.version)
                .join(format!("{id}.exe"))
        );
    }
    cleanup(&root);
}
#[test]
fn versions_compare_semantically() {
    assert!(distribution::compare_versions("1.10.0", "1.9.0").is_gt());
    assert!(distribution::compare_versions("1.0.0-beta.1", "1.0.0").is_lt());
}

#[test]
fn scrcpy_component_must_match_the_application_protocol_and_hash() {
    let root = unique_root("scrcpy-binding");
    let layout = InstallLayout::resolve(Some(root.clone()));
    let mut value: serde_json::Value = serde_json::from_str(include_str!(
        "../../release/contracts/fixtures/manifest/valid/manifest-valid-full.json"
    ))
    .unwrap();
    let platform = &mut value["platforms"][distribution::PLATFORM];
    let mut component = platform["components"][0].clone();
    component["id"] = "scrcpy-server".into();
    component["version"] = platform["resources"]["scrcpy_server"]["version"].clone();
    component["required_files"] = serde_json::json!([{
        "path": "scrcpy-server.jar", "size": 1,
        "sha256": platform["resources"]["scrcpy_server"]["sha256"]
    }]);
    platform["components"]
        .as_array_mut()
        .unwrap()
        .push(component);
    assert!(distribution::plan(&layout, &Manifest::parse(&value).unwrap()).is_ok());
    let components = value["platforms"][distribution::PLATFORM]["components"]
        .as_array_mut()
        .unwrap();
    components.last_mut().unwrap()["version"] = "99.0.0".into();
    assert!(distribution::plan(&layout, &Manifest::parse(&value).unwrap()).is_err());
    cleanup(&root);
}

#[test]
fn upgrade_cache_preserves_unsigned_manifest_bytes() {
    use gamer_launcher::{
        manifest::{validate_manifest_file, ValidateOptions},
        state::{CurrentState, StateStore},
        upgrade::engine::{Engine, ManifestSource, UpgradeOptions},
    };
    let root = unique_root("unsigned-cache");
    let layout = InstallLayout::resolve(Some(root.clone()));
    let fixtures =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../release/contracts/fixtures");
    let source = fixtures.join("manifest/valid/manifest-valid-basic.json");
    StateStore::new(&layout.root)
        .write_current(&CurrentState::new("0.1.0", None))
        .unwrap();
    let engine = Engine::new(layout.clone(), UpgradeOptions::default());
    engine
        .phase_check(&ManifestSource::Path(source.clone()))
        .unwrap();
    let cached = layout.manifests_dir().join("0.2.0.json");
    assert_eq!(
        std::fs::read(&source).unwrap(),
        std::fs::read(&cached).unwrap()
    );
    let outcome = validate_manifest_file(&cached, &ValidateOptions::default());
    assert!(outcome.ok, "{:?}", outcome.errors);
    // Re-checking a cache entry also works when source == destination.
    assert!(!cached.with_extension("sig").exists());
    engine.phase_check(&ManifestSource::Path(cached)).unwrap();
    cleanup(&root);
}

#[test]
fn online_check_fetches_only_json_without_signature_or_key_files() {
    use gamer_launcher::{
        state::{CurrentState, StateStore},
        upgrade::engine::{Engine, ManifestSource, UpgradeOptions},
    };
    use std::io::{Read, Write};
    let root = unique_root("unsigned-online");
    let layout = InstallLayout::resolve(Some(root.clone()));
    StateStore::new(&root)
        .write_current(&CurrentState::new("0.1.0", None))
        .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        assert!(request.starts_with(b"GET /release.json HTTP/1.1\r\n"));
        let body = include_bytes!(
            "../../release/contracts/fixtures/manifest/valid/manifest-valid-basic.json"
        );
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        socket.write_all(body).unwrap();
        // The listener closes after one response; a second signature request fails.
    });
    let engine = Engine::new(layout.clone(), UpgradeOptions::default());
    let result = engine.phase_check(&ManifestSource::Url(format!(
        "http://{address}/release.json"
    )));
    server.join().unwrap();
    assert!(result.is_ok(), "{result:?}");
    assert!(layout.manifests_dir().join("0.2.0.json").exists());
    assert!(!layout.manifests_dir().join("0.2.0.sig").exists());
    assert!(!root.join("keys").exists());
    cleanup(&root);
}
