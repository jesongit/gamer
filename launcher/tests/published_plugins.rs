//! Real release acceptance, run explicitly against an isolated running installation.
use gamer_launcher::{distribution, layout::InstallLayout, official_plugins};

#[test]
#[ignore = "requires a public release matching this launcher and GAMER_TEST_PUBLIC_ROOT"]
fn discovers_public_release_through_default_channel() {
    assert!(std::env::var_os("GAMER_LAUNCHER_RELEASE_MANIFEST").is_none());
    let root = std::env::var("GAMER_TEST_PUBLIC_ROOT").expect("isolated public discovery root");
    let layout = InstallLayout::resolve(Some(root.into()));
    assert_eq!(
        gamer_launcher::upgrade::engine::ManifestSource::configured(),
        gamer_launcher::upgrade::engine::ManifestSource::Official
    );
    let (_, manifest) = distribution::discover(&layout).unwrap();
    assert_eq!(manifest.release.version, env!("CARGO_PKG_VERSION"));
    let plugins = manifest.platforms[distribution::PLATFORM]
        .components
        .iter()
        .find(|c| c.id == "official-plugins")
        .unwrap();
    assert!(plugins
        .artifact
        .url
        .starts_with("https://github.com/jesongit/gamer-plugins/releases/download/"));
}

#[test]
#[ignore = "requires GAMER_TEST_INSTALL_ROOT and a running isolated Gamer server"]
fn installs_published_plugins_using_launcher_selection_flow() {
    let root = std::env::var("GAMER_TEST_INSTALL_ROOT").expect("isolated installation root");
    let layout = InstallLayout::resolve(Some(root.into()));
    let (_, manifest) = distribution::cached(&layout, None).expect("validated release manifest");
    let token: serde_json::Value =
        serde_json::from_slice(&std::fs::read(layout.state_dir().join("admin-token")).unwrap())
            .unwrap();
    let choices = official_plugins::choices(&layout, &manifest).unwrap();
    let expected = manifest.platforms[distribution::PLATFORM]
        .components
        .iter()
        .find(|c| c.id == "official-plugins")
        .unwrap()
        .required_files
        .len();
    assert_eq!(choices.len(), expected);
    let selected: Vec<_> = choices.iter().map(|c| c.id.clone()).collect();
    official_plugins::install(
        &layout,
        &choices,
        &selected,
        token["token"].as_str().unwrap(),
    )
    .unwrap();
    // Retrying a completed selection must preserve already installed versions.
    official_plugins::install(
        &layout,
        &choices,
        &selected,
        token["token"].as_str().unwrap(),
    )
    .unwrap();
    let saved: Vec<String> = serde_json::from_slice(
        &std::fs::read(layout.state_dir().join("plugins-choice.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved, selected);
}
