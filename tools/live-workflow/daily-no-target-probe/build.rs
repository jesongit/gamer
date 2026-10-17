use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, env, fs, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let repo = manifest.ancestors().nth(3).unwrap();
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let snapshot = output.join("current-parser");
    fs::create_dir_all(snapshot.join("syntax")).unwrap();
    let mut sources = Vec::new();
    for (source, destination) in [
        ("plugins/gamer-yaml/host/syntax.rs", "syntax.rs"),
        (
            "plugins/gamer-yaml/host/syntax/break_control.rs",
            "break_control.rs",
        ),
        (
            "plugins/gamer-yaml/host/syntax/match_templates.rs",
            "match_templates.rs",
        ),
        ("plugins/gamer-yaml/host/syntax/visual.rs", "visual.rs"),
        (
            "plugins/gamer-yaml/host/syntax/script_calls.rs",
            "syntax/script_calls.rs",
        ),
        ("plugins/gamer-yaml/host/native_funcs.rs", "native_funcs.rs"),
    ] {
        let original = repo.join(source);
        println!("cargo:rerun-if-changed={}", original.display());
        let bytes = fs::read(&original).unwrap();
        let target = snapshot.join(destination);
        fs::write(&target, &bytes).unwrap();
        assert_eq!(
            fs::read(&target).unwrap(),
            bytes,
            "parser snapshots must be byte-identical"
        );
        sources.push(serde_json::json!({"path":source,"sha256":format!("{:x}",Sha256::digest(&bytes)),"byte_identical_snapshot":true}));
    }
    for source in [
        "plugins/gamer-yaml/interpreter/Cargo.toml",
        "plugins/gamer-yaml/interpreter/src/lib.rs",
        "plugins/gamer-yaml/host/settings.rs",
        "tools/live-workflow/daily-no-target-probe/src/main.rs",
        "tools/live-workflow/daily-no-target-probe/src/mock.rs",
        "tools/live-workflow/daily-no-target-probe/src/scenarios.rs",
    ] {
        let original = repo.join(source);
        println!("cargo:rerun-if-changed={}", original.display());
        sources.push(serde_json::json!({"path":source,"sha256":format!("{:x}",Sha256::digest(fs::read(original).unwrap()))}));
    }
    let registry = fs::read_to_string(snapshot.join("native_funcs.rs")).unwrap();
    let permissions: BTreeSet<_> = registry
        .split("Permission::")
        .skip(1)
        .map(|tail| {
            tail.chars()
                .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
                .collect::<String>()
        })
        .collect();
    fs::write(
        output.join("permissions.rs"),
        format!(
            "#[derive(Clone,Copy,Debug)] pub enum Permission {{ {} }}",
            permissions.into_iter().collect::<Vec<_>>().join(",")
        ),
    )
    .unwrap();
    let settings = fs::read_to_string(repo.join("plugins/gamer-yaml/host/settings.rs")).unwrap();
    let timeout = settings
        .lines()
        .find(|line| line.starts_with("pub const DEFAULT_TIMEOUT_SECS:"))
        .unwrap()
        .split('=')
        .nth(1)
        .unwrap()
        .trim()
        .trim_end_matches(';')
        .parse::<u64>()
        .unwrap();
    let path = |name: &str| format!("{:?}", snapshot.join(name).to_string_lossy().to_string());
    fs::write(output.join("parser-modules.rs"),format!("#[path={}] mod syntax;\n#[path={}] mod native_funcs;\nmod settings {{pub const DEFAULT_TIMEOUT_SECS:u64={timeout};}}\n",path("syntax.rs"),path("native_funcs.rs"))).unwrap();
    fs::write(
        output.join("source-hashes.json"),
        serde_json::to_vec_pretty(&sources).unwrap(),
    )
    .unwrap();
}
