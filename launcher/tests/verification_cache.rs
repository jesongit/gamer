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
