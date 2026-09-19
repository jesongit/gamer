use super::resources::validate_function_library_file;
use super::yaml_extension::YAML_EXTENSION_ID;
use crate::resources::{PackageInput, PackageStore};

#[test]
fn split_libraries_reject_duplicate_names_and_removed_references() {
    let data = tempfile::tempdir().unwrap();
    let store = PackageStore::open(&crate::config::Config { data_dir: data.path().into(), ..Default::default() }).unwrap();
    store.create_package(PackageInput { id: "qa".into(), ..Default::default() }).unwrap();
    let file = "automations/_function_extra.yaml";
    store.write_text("qa", YAML_EXTENSION_ID, file, "functions:\n  helper:\n    run: []\n", None, false).unwrap();
    store.write_text("qa", YAML_EXTENSION_ID, "automations/main.yaml", "run:\n  - helper: {}\n", None, false).unwrap();
    let error = validate_function_library_file(&store, "qa", file, "functions: {}\n").unwrap_err();
    assert_eq!(error[0]["code"], "yaml.functions.referenced");
    assert!(error[0]["message"].as_str().unwrap().contains("main.yaml"));
    let duplicate = validate_function_library_file(&store, "qa", "automations/_function.yaml", "functions:\n  helper:\n    run: []\n").unwrap_err();
    assert_eq!(duplicate[0]["code"], "yaml.fn.duplicate");
    store.delete_resource("qa", YAML_EXTENSION_ID, "automations/main.yaml").unwrap();
    assert!(validate_function_library_file(&store, "qa", file, "functions: {}\n").is_ok());
}
