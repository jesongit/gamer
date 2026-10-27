//! Deterministic execution of the exact login/exit candidates. No sockets or device.
use crate::{native_funcs, syntax};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, fs, sync::Mutex};
use yaml_interp::{EventSink, HostError, HostErrorKind, HostFunctions};

struct Mock {
    calls: Mutex<Vec<Value>>,
    polls: Mutex<VecDeque<&'static str>>,
    confirmations: Mutex<VecDeque<&'static str>>,
    sees_enter: bool,
    signed_out: bool,
    logout_tapped: Mutex<bool>,
    logout_fails: bool,
    child_cleanup_fails: bool,
    qr_unavailable: bool,
    docker_qr_only: bool,
}
fn failure(message: &str) -> HostError {
    HostError::new(HostErrorKind::Failed, message)
}
fn hit(template: &str) -> Value {
    json!({"template":template,"score":1.0,"center":{"x":0.8,"y":0.4},"_frame":{"mock_frame":71},"_observed_at_ms":100})
}
impl HostFunctions for Mock {
    fn invoke(&self, name: &str, value: Value) -> Result<Value, HostError> {
        let mut args = match value {
            Value::Object(m) => m,
            Value::Null => Map::new(),
            v => {
                let first = native_funcs::native_function(name)
                    .and_then(|f| f.params.first())
                    .ok_or_else(|| failure("UNCONFIGURED_POSITIONAL"))?;
                Map::from_iter([(first.name.into(), v)])
            }
        };
        if let Some(f) = native_funcs::native_function(name) {
            for p in &f.params {
                if !args.contains_key(p.name) {
                    if let Some(v) = &p.default {
                        args.insert(p.name.into(), v.clone());
                    } else if p.required {
                        return Err(failure("MISSING_NATIVE_ARG"));
                    }
                }
            }
        }
        let args = Value::Object(args);
        self.calls
            .lock()
            .unwrap()
            .push(json!({"name":name,"args":args}));
        match name {
            "live_login_state" => Ok(
                json!({"status":match args["action"].as_str().unwrap() {"begin"|"rescan_done"=>"scanning", "poll"=>self.polls.lock().unwrap().pop_front().ok_or_else(|| failure("UNCONFIGURED_POLL"))?, _=>return Err(failure("UNCONFIGURED_ACTION"))}}),
            ),
            "wait_live_confirmation" => Ok(
                json!({"status":self.confirmations.lock().unwrap().pop_front().ok_or_else(|| failure("UNCONFIGURED_CONFIRMATION"))?}),
            ),
            "wait_find" | "observe" => {
                let t = args["template"].as_str().unwrap();
                let found = if t.contains("qr_ready")
                    && (self.qr_unavailable || (self.docker_qr_only && !t.contains("docker")))
                {
                    false
                } else if t.contains("qr_ready")
                    && self
                        .calls
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|c| c["name"] == "reset_input")
                {
                    true
                } else if t.contains("scan_switch") || t.contains("qr_ready") {
                    // A parent QR-ready wait follows explicit rescan. Exit proof
                    // is only available if already signed out or logout worked.
                    self.signed_out || (*self.logout_tapped.lock().unwrap() && !self.logout_fails)
                } else if t.contains("enter_game") {
                    self.sees_enter
                } else {
                    true
                };
                Ok(if found { hit(t) } else { Value::Null })
            }
            "tap" | "hover" => {
                if args["position"]["_frame"]["mock_frame"] != 71 {
                    return Err(failure("MATCH_METADATA_LOST"));
                }
                if args["position"]["template"]
                    .as_str()
                    .is_some_and(|t| t.contains("logout_confirm"))
                {
                    *self.logout_tapped.lock().unwrap() = true;
                }
                Ok(Value::Null)
            }
            "直播云游戏收尾.yaml" => {
                if self.child_cleanup_fails
                    && count(&self.calls.lock().unwrap(), "直播云游戏收尾.yaml") >= 2
                {
                    Err(failure("EXIT_FAILED"))
                } else {
                    Ok(json!(true))
                }
            }
            "云游戏日常.yaml" | "直播日常测试.yaml" => Ok(json!(true)),
            "notify" | "log" | "sleep" | "reset_input" => Ok(Value::Null),
            "eq" => Ok(json!(args["a"] == args["b"])),
            "fail" => Err(HostError::new(
                HostErrorKind::Failed,
                args["code"].as_str().unwrap_or("FAIL"),
            )),
            _ => Err(failure("UNCONFIGURED_NATIVE")),
        }
    }
}
impl EventSink for Mock {
    fn emit(&self, _: Value) {}
}
fn execute(
    script: &syntax::Script,
    host: &Mock,
    params: Map<String, Value>,
) -> Result<Value, String> {
    let mut vars: Map<String, Value> = script
        .params
        .iter()
        .filter_map(|p| p.default.clone().map(|v| (p.name.clone(), v)))
        .collect();
    vars.extend(script.vars.iter().cloned());
    vars.extend(params);
    let program = serde_json::from_value(syntax::build_program(script, &vec![], vars, 0))
        .map_err(|e| e.to_string())?;
    yaml_interp::run(&program, host, Some(host))
}
fn count(calls: &[Value], name: &str) -> usize {
    calls.iter().filter(|c| c["name"] == name).count()
}
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let option = |n: &str| {
        args.iter()
            .position(|v| v == n)
            .and_then(|i| args.get(i + 1))
            .ok_or_else(|| format!("{n} required"))
    };
    let parent_path = option("--candidate")?;
    let cleanup_path = option("--cleanup")?;
    let report_path = option("--report")?;
    let source = fs::read_to_string(parent_path)?;
    let cleanup_source = fs::read_to_string(cleanup_path)?;
    let parent = syntax::parse_script(&source).map_err(|e| format!("parent syntax: {e:?}"))?;
    let cleanup =
        syntax::parse_script(&cleanup_source).map_err(|e| format!("cleanup syntax: {e:?}"))?;
    let mut results = vec![];
    for (name, polls, confirmations, sees_enter, child_cleanup_fails, expect_child, expect_error) in [
        (
            "confirmed",
            vec!["scanning"],
            vec!["confirmed"],
            true,
            false,
            true,
            false,
        ),
        (
            "docker-qr-confirmed",
            vec!["scanning"],
            vec!["confirmed"],
            true,
            false,
            true,
            false,
        ),
        (
            "qr-preparing-timeout",
            vec!["scanning", "timeout", "timeout"],
            vec![],
            false,
            false,
            false,
            false,
        ),
        (
            "qr-preparing-cancel",
            vec!["scanning", "cancelled", "cancelled"],
            vec![],
            false,
            false,
            false,
            false,
        ),
        (
            "scan-timeout",
            vec!["scanning", "timeout"],
            vec![],
            false,
            false,
            false,
            false,
        ),
        (
            "scan-cancel",
            vec!["scanning", "cancelled"],
            vec![],
            false,
            false,
            false,
            false,
        ),
        (
            "confirm-timeout",
            vec!["scanning"],
            vec!["timeout"],
            true,
            false,
            false,
            false,
        ),
        (
            "confirm-cancel",
            vec!["scanning"],
            vec!["cancelled"],
            true,
            false,
            false,
            false,
        ),
        (
            "scan-rescan",
            vec!["rescan", "scanning"],
            vec!["confirmed"],
            true,
            false,
            true,
            false,
        ),
        (
            "confirm-rescan",
            vec!["scanning", "rescan", "scanning"],
            vec!["rescan", "confirmed"],
            true,
            false,
            true,
            false,
        ),
        (
            "cancel-during-rescan",
            vec!["rescan", "cancelled"],
            vec![],
            true,
            false,
            false,
            false,
        ),
        (
            "failed-rescan-exit",
            vec!["rescan"],
            vec![],
            true,
            true,
            false,
            true,
        ),
    ] {
        for test in [false, true] {
            let host = Mock {
                calls: Mutex::new(vec![]),
                polls: Mutex::new(polls.clone().into()),
                confirmations: Mutex::new(confirmations.clone().into()),
                sees_enter,
                signed_out: false,
                logout_tapped: Mutex::new(false),
                logout_fails: false,
                child_cleanup_fails,
                qr_unavailable: name.starts_with("qr-preparing"),
                docker_qr_only: name.starts_with("docker-qr"),
            };
            let outcome = execute(
                &parent,
                &host,
                Map::from_iter([("test_mode".into(), json!(test))]),
            );
            let calls = host.calls.lock().unwrap();
            let child = if test {
                "直播日常测试.yaml"
            } else {
                "云游戏日常.yaml"
            };
            let rescan = calls
                .iter()
                .position(|c| c["name"] == "直播云游戏收尾.yaml");
            let reset = calls.iter().position(|c| c["name"] == "reset_input");
            let ack = calls.iter().position(|c| {
                c["name"] == "live_login_state" && c["args"]["action"] == "rescan_done"
            });
            let checks = json!({"expected_outcome":outcome.is_err()==expect_error&&(!expect_error||outcome.as_ref().err().unwrap().contains("EXIT_FAILED")),
                "only_confirmed_runs_daily":count(&calls,child)==usize::from(expect_child),
                "never_clicks_enter":!calls.iter().any(|c|c["name"]=="tap"&&c["args"]["position"]["template"].as_str().is_some_and(|s|s.contains("enter_game"))),
                "bounded_scan_polls":calls.iter().filter(|c|c["name"]=="wait_find"&&c["args"]["template"].as_str().is_some_and(|s|s.contains("enter_game"))).all(|c|c["args"]["timeout"]=="1s"&&c["args"]["forever"]!=true),
                "exit_before_qr_reset":expect_error||rescan<reset&&(ack.is_none()||reset<ack),
                "every_user_gets_fresh_qr":expect_error||(count(&calls,"直播云游戏收尾.yaml")>=1&&count(&calls,"reset_input")>=1),
                "failed_exit_never_resets_or_acknowledges":!expect_error||(count(&calls,"reset_input")==1&&ack.is_none()),
                "no_mock_errors":outcome.as_ref().err().is_none_or(|e|!e.contains("UNCONFIGURED")&&!e.contains("METADATA"))});
            results.push(json!({"name":format!("{name}-test-{test}"),"passed":checks.as_object().unwrap().values().all(|v|v==true),"checks":checks,"outcome":outcome,"calls":*calls}));
        }
    }
    for (name, reset, signed_out, logout_fails, expect_error) in [
        ("already-qr", false, true, false, false),
        ("logout", false, false, false, false),
        ("normal-end", true, false, false, false),
        ("unverified-exit", false, false, true, true),
    ] {
        let host = Mock {
            calls: Mutex::new(vec![]),
            polls: Mutex::new(VecDeque::new()),
            confirmations: Mutex::new(VecDeque::new()),
            sees_enter: true,
            signed_out,
            logout_tapped: Mutex::new(false),
            logout_fails,
            child_cleanup_fails: false,
            qr_unavailable: false,
            docker_qr_only: false,
        };
        let outcome = execute(
            &cleanup,
            &host,
            Map::from_iter([("reset_before_logout".into(), json!(reset))]),
        );
        let calls = host.calls.lock().unwrap();
        let checks = json!({"verified_exit_or_failure":if expect_error {outcome.as_ref().err().is_some_and(|e|e.contains("LIVE_LOGOUT_NOT_VERIFIED"))}else{outcome==Ok(json!(true))},
            "already_logged_out_never_touches_account":!signed_out||count(&calls,"tap")==0,
            "reset_only_if_requested":count(&calls,"reset_input")==usize::from(reset),
            "logout_proof_observed_after_confirmation":signed_out||calls.iter().position(|c|c["name"]=="tap"&&c["args"]["position"]["template"].as_str().is_some_and(|s|s.contains("logout_confirm"))).is_some_and(|at|calls[at+1..].iter().any(|c|c["name"]=="wait_find"&&c["args"]["template"].as_str().is_some_and(|s|s.contains("scan_switch")||s.contains("qr_ready"))))});
        results.push(json!({"name":name,"passed":checks.as_object().unwrap().values().all(|v|v==true),"checks":checks,"outcome":outcome,"calls":*calls}));
    }
    let passed = results.iter().all(|r| r["passed"] == true);
    let report = json!({"passed":passed,"cases":results.len(),"parent_sha256":format!("{:x}",Sha256::digest(source)),"cleanup_sha256":format!("{:x}",Sha256::digest(cleanup_source)),"results":results,"real_device_or_platform_tested":false,"device_inputs":0,"notifications_sent":0});
    fs::write(report_path, serde_json::to_vec_pretty(&report)?)?;
    println!(
        "{}",
        json!({"passed":passed,"cases":report["cases"],"failures":report["results"].as_array().unwrap().iter().filter(|r|r["passed"]!=true).map(|r|&r["name"]).collect::<Vec<_>>()})
    );
    if !passed {
        std::process::exit(1)
    }
    Ok(())
}
