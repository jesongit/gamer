//! Real release smoke: run against an isolated installation whose GUI has reached plugin selection.
//! GAMER_OFFLINE_SMOKE_ROOT must point at a disposable directory with its own listening port.
use gamer_launcher::{distribution, layout::InstallLayout, official_plugins};

#[test]
#[ignore = "requires an isolated full-package installation and its running server"]
fn real_offline_plugin_selection_uses_core_install_api() {
    let root = std::env::var_os("GAMER_OFFLINE_SMOKE_ROOT")
        .expect("explicit isolated smoke root required");
    let layout = InstallLayout::resolve(Some(root.into()));
    let port = gamer_launcher::supervisor::read_configured_port(&layout.config_file());
    assert_ne!(port, 8443, "never use the development server");
    let (_, manifest) = distribution::cached(&layout, None).unwrap();
    let choices = official_plugins::choices(&layout, &manifest).unwrap();
    assert_eq!(choices.len(), 3);
    let credentials: serde_json::Value =
        serde_json::from_slice(&std::fs::read(layout.state_dir().join("admin-token")).unwrap())
            .unwrap();
    let token = credentials["token"].as_str().unwrap();
    let installed = || -> serde_json::Value {
        let response = ureq::get(&format!("http://127.0.0.1:{port}/api/extensions"))
            .set("X-Admin-Token", token)
            .call()
            .unwrap();
        let value: serde_json::Value = serde_json::from_reader(response.into_reader()).unwrap();
        value["extensions"].clone()
    };
    assert!(
        installed().as_array().unwrap().is_empty(),
        "use a fresh isolated installation"
    );
    let selected = vec!["gamer-yaml".to_string(), "gamer-video".to_string()];
    official_plugins::install(&layout, &choices, &selected, token).unwrap();
    let first = installed();
    let rows = first.as_array().unwrap();
    assert_eq!(rows.len(), 2, "unchecked keymap must remain uninstalled");
    for id in &selected {
        assert!(rows.iter().any(|p| p["id"] == *id));
    }
    // Retry the same selection, then add the remaining plugin through the same API.
    let all: Vec<_> = choices.iter().map(|c| c.id.clone()).collect();
    official_plugins::install(&layout, &choices, &all, token).unwrap();
    assert_eq!(installed().as_array().unwrap().len(), 3);
    let recorded: Vec<String> = serde_json::from_slice(
        &std::fs::read(layout.state_dir().join("plugins-choice.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(recorded, all);
}
