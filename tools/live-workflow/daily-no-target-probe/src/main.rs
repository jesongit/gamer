//! A standalone, deterministic workflow probe. No sockets, devices or Gamer service.
#![allow(dead_code, unused_imports)]
mod extensions {
    pub mod permissions {
        include!(concat!(env!("OUT_DIR"), "/permissions.rs"));
    }
    pub mod gamer_yaml {
        pub mod resources {
            pub fn is_function_library_path(path: &str) -> bool {
                path.rsplit('/')
                    .next()
                    .is_some_and(|name| name.starts_with("_function") && name.ends_with(".yaml"))
            }
        }
    }
}
include!(concat!(env!("OUT_DIR"), "/parser-modules.rs"));
mod live_confirmation;
mod mock;
mod reward_boundary;
mod scenarios;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .to_path_buf();
    let source_hashes: Value = serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/source-hashes.json"
    )))?;
    for entry in source_hashes.as_array().unwrap() {
        let path = repo.join(entry["path"].as_str().unwrap());
        let current = format!("{:x}", Sha256::digest(fs::read(&path)?));
        if entry["sha256"] != current {
            return Err(format!(
                "source changed since build; rebuild probe: {}",
                path.display()
            )
            .into());
        }
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|value| value == "--self-check") {
        let source = "functions:\n  helper:\n    run: [{return: {whole: true}}]\nrun: [{helper: {}, as: hit}, {return: $hit}]\n";
        let parsed =
            syntax::parse_script(source).map_err(|diagnostics| format!("{diagnostics:?}"))?;
        let program: yaml_interp::Program = serde_json::from_value(syntax::build_program(
            &parsed,
            &vec![],
            Default::default(),
            0,
        ))?;
        let host = mock::MockHost::new(mock::Case::smoke(), Default::default());
        assert_eq!(
            yaml_interp::run(&program, &host, Some(&host))?,
            json!({"whole":true})
        );
        println!(
            "{}",
            json!({"passed":true,"scope":"current authoritative parser/interpreter glue only","runtime_contract":yaml_interp::RUNTIME_CONTRACT,"source_hashes":serde_json::from_str::<Value>(include_str!(concat!(env!("OUT_DIR"),"/source-hashes.json")))?,"production_accessed":false,"device_inputs":0,"listeners_started":0})
        );
        return Ok(());
    }
    let option = |name: &str| {
        args.iter()
            .position(|value| value == name)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    if args.iter().any(|value| value == "--reward-boundary") {
        return reward_boundary::run(&repo, &args, source_hashes);
    }
    if args.iter().any(|value| value == "--live-confirmation") {
        return live_confirmation::run(&repo, &args, source_hashes);
    }
    let candidate =
        PathBuf::from(option("--candidate").ok_or("--candidate YAML path is required")?);
    let report = PathBuf::from(option("--report").ok_or("--report JSON path is required")?);
    let source = fs::read_to_string(&candidate)?;
    let source_hash = format!("{:x}", Sha256::digest(source.as_bytes()));
    let roles: mock::Roles = match option("--roles") {
        Some(path) => serde_json::from_slice(&fs::read(path)?)?,
        None => Default::default(),
    };
    let script = syntax::parse_script(&source)
        .map_err(|diagnostics| format!("candidate syntax: {diagnostics:?}"))?;
    let results = scenarios::run(&script, roles);
    assert_eq!(
        source_hash,
        format!("{:x}", Sha256::digest(fs::read(&candidate)?)),
        "candidate changed during the probe"
    );
    let passed = results.iter().all(|result| result["passed"] == true);
    let parser_hashes: Vec<_> = source_hashes
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["byte_identical_snapshot"] == true)
        .cloned()
        .collect();
    let summary = json!({"passed":passed,"candidate_sha256":source_hash,"scope":"authoritative source parser and interpreter, mocked native functions; navigation and phone-entry behavior only","candidate":{"path":candidate,"sha256":source_hash},"runtime_contract":yaml_interp::RUNTIME_CONTRACT,"yaml_interp_path":repo.join("plugins/gamer-yaml/interpreter"),"current_parser_snapshot_hashes":parser_hashes,"source_hashes":source_hashes,"source_hashes_verified_at_runtime":true,"interpreter_max_steps":yaml_interp::MAX_STEPS,"mock_native_call_budget":200000,"case_count":results.len(),"scenarios":results,"real_game":false,"device_input":false,"production_accessed":false,"listeners_started":0,"limitations":["No real screenshot matching, browser/account/game, scrolling effect, network or fight success is claimed","Native wait_find is a deterministic mock; obstacle priority and input fencing remain covered by the production host tests"]});
    fs::create_dir_all(
        report
            .parent()
            .ok_or("report must have a parent directory")?,
    )?;
    fs::write(&report, serde_json::to_vec_pretty(&summary)?)?;
    println!(
        "{}",
        json!({"passed":passed,"case_count":summary["case_count"],"candidate_sha256":source_hash,"report":report})
    );
    if !passed {
        std::process::exit(1);
    }
    Ok(())
}
