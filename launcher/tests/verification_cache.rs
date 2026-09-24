mod common;
use common::{cleanup, sha256_hex, unique_root};
use gamer_launcher::{layout::InstallLayout, verification::verify};
use std::fs;

#[test]
fn unchanged_files_skip_hash_but_replacement_change_and_manifest_change_invalidate() {
    let root = unique_root("verification");
    let layout = InstallLayout::resolve(Some(root.clone()));
    let path = root.join("app.exe");
    fs::write(&path, b"correct").unwrap();
    let hash = sha256_hex(b"correct");
    assert!(!verify(&layout, &path, &hash, 7).unwrap());
    assert!(verify(&layout, &path, &hash, 7).unwrap());
    assert!(verify(&layout, &path, &sha256_hex(b"changed"), 7).is_err());
    let replacement = root.join("replacement");
    fs::write(&replacement, b"changed").unwrap();
    fs::remove_file(&path).unwrap();
    fs::rename(replacement, &path).unwrap();
    assert!(verify(&layout, &path, &hash, 7).is_err());
    fs::remove_file(&path).unwrap();
    assert!(verify(&layout, &path, &hash, 7).is_err());
    cleanup(&root);
}

#[test]
fn same_volume_install_rename_preserves_verification_but_cache_damage_does_not() {
    let root = unique_root("verification-rename");
    let layout = InstallLayout::resolve(Some(root.clone()));
    let staged = root.join("staged.exe");
    let installed = root.join("installed.exe");
    let hash = sha256_hex(b"verified app");
    fs::write(&staged, b"verified app").unwrap();
    assert!(!verify(&layout, &staged, &hash, 12).unwrap());
    fs::rename(&staged, &installed).unwrap();
    assert!(
        verify(&layout, &installed, &hash, 12).unwrap(),
        "rename must not cause a second hash"
    );
    for receipt in fs::read_dir(layout.state_dir().join("verified")).unwrap() {
        fs::write(receipt.unwrap().path(), b"broken receipt").unwrap();
    }
    assert!(!verify(&layout, &installed, &hash, 12).unwrap());
    cleanup(&root);
}
