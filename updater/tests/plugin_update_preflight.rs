mod common;
use gamer_updater::{layout::InstallLayout, manifest::model::Manifest, official_plugins};
use serde_json::json;
use std::{fs, io::Write};

#[test]
fn only_installed_plugins_are_checked_and_new_permissions_are_rejected_before_shutdown() {
    let layout = InstallLayout::resolve(Some(common::unique_root("plugin-permission-preflight")));
    let directory = layout.runtime_dir().join("official-plugins/1.1.0");
    fs::create_dir_all(&directory).unwrap();
    let archive = directory.join("sample.gplugin");
    let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    zip.start_file("manifest.toml", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"id = 'sample'\nname = 'Sample'\nversion = '1.1.0'\npermissions = ['device.read', 'input.tap']\n").unwrap();
    zip.finish().unwrap();
    let mut value: serde_json::Value = serde_json::from_str(include_str!(
        "../../release/contracts/fixtures/manifest/valid/manifest-valid-basic.json"
    ))
    .unwrap();
    let bytes = fs::read(&archive).unwrap();
    let hash = common::sha256_hex(&bytes);
    value["platforms"]["windows-x86_64"]["components"] = json!([{
        "id": "official-plugins", "version": "1.1.0",
        "artifact": { "name":"bundle.zip", "url":"https://example.invalid/bundle.zip", "size":1, "sha256":hash },
        "required_files": [{"path":"sample.gplugin", "size":bytes.len(), "sha256":hash}]
    }]);
    let manifest = Manifest::parse(&value).unwrap();
    assert!(
        official_plugins::preflight_update(&layout, &manifest).is_ok(),
        "uninstalled plugins are untouched"
    );
    let installed = layout.data_dir().join("extensions/sample/1.0.0");
    fs::create_dir_all(&installed).unwrap();
    fs::write(
        layout.data_dir().join("extensions/state.json"),
        json!({"plugins":{"sample":{"active_version":"1.0.0"}}}).to_string(),
    )
    .unwrap();
    fs::write(
        installed.join("manifest.toml"),
        "id='sample'\nname='Sample'\nversion='1.0.0'\npermissions=['device.read']\n",
    )
    .unwrap();
    let error = official_plugins::preflight_update(&layout, &manifest).unwrap_err();
    assert!(error.contains("input.tap") && error.contains("新增权限"));
    fs::write(
        installed.join("manifest.toml"),
        "id='sample'\nname='Sample'\nversion='1.0.0'\npermissions=['device.read','input.tap']\n",
    )
    .unwrap();
    assert!(official_plugins::preflight_update(&layout, &manifest).is_ok());
    common::cleanup(&layout.root);
}
