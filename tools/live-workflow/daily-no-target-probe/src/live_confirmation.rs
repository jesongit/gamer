//! In-memory confirmation/cleanup probe. Executes unchanged candidate ASTs in
//! the current authoritative interpreter; no service, sender or device exists.
use crate::{native_funcs, syntax};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::Path, sync::Mutex};
use yaml_interp::{EventSink, HostError, HostErrorKind, HostFunctions};

#[derive(Default)]
struct Mock {
    status: &'static str,
    scan: bool,
    click_more: bool,
    child_error: bool,
    confirmation_error: bool,
    missing_more: bool,
    calls: Mutex<Vec<Value>>,
    events: Mutex<Vec<Value>>,
}
fn failure(message: impl Into<String>) -> HostError {
    HostError::new(HostErrorKind::Failed, message)
}
fn matched(template: &str) -> Value {
    json!({"template":template,"score":1.0,"center":{"x":0.8,"y":0.4},"_frame":{"mock_frame":71},"_observed_at_ms":100})
}
impl HostFunctions for Mock {
    fn invoke(&self, name: &str, args: Value) -> Result<Value, HostError> {
        let mut args = if let Some(object) = args.as_object() {
            object.clone()
        } else {
            let first = native_funcs::native_function(name)
                .and_then(|f| f.params.first())
                .ok_or_else(|| failure("UNCONFIGURED_POSITIONAL"))?;
            Map::from_iter([(first.name.into(), args)])
        };
        if let Some(function) = native_funcs::native_function(name) {
            for param in &function.params {
                if !args.contains_key(param.name) {
                    if let Some(default) = &param.default {
                        args.insert(param.name.into(), default.clone());
                    } else if param.required {
                        return Err(failure(format!("MISSING_NATIVE_ARG:{name}.{}", param.name)));
                    }
                }
            }
        }
        let args = Value::Object(args);
        self.calls
            .lock()
            .unwrap()
            .push(json!({"function":name,"args":args}));
        let template = args["template"].as_str().unwrap_or("");
        match name {
            "notify" | "log" | "reset_input" => Ok(Value::Null),
            "observe" => Ok(if self.scan {
                matched(template)
            } else {
                Value::Null
            }),
            "wait_find" => Ok(
                if (self.click_more
                    && template.contains("logout_")
                    && !template.contains("confirm")
                    && args["timeout"] == "2s")
                    || (self.missing_more && template.contains("more_"))
                {
                    Value::Null
                } else {
                    matched(template)
                },
            ),
            "wait_live_confirmation" => {
                if self.confirmation_error {
                    Err(failure("PROBE_CONFIRMATION_ERROR"))
                } else {
                    Ok(json!({"status":self.status}))
                }
            }
            "eq" => Ok(json!(args["a"] == args["b"])),
            "tap" | "hover" => {
                if args["position"]["_frame"]["mock_frame"] != 71 {
                    return Err(failure("WHOLE_MATCH_METADATA_LOST"));
                }
                Ok(Value::Null)
            }
            "云游戏日常.yaml" | "直播日常测试.yaml" => {
                if self.child_error {
                    Err(failure("PROBE_CHILD_ERROR"))
                } else {
                    Ok(json!(true))
                }
            }
            "fail" => Err(failure(format!(
                "SCRIPT_FAIL:{}",
                args["code"].as_str().unwrap_or("")
            ))),
            _ => Err(failure(format!("UNCONFIGURED_NATIVE:{name}"))),
        }
    }
}
impl EventSink for Mock {
    fn emit(&self, event: Value) {
        self.events.lock().unwrap().push(event)
    }
}
fn execute(
    script: &syntax::Script,
    overrides: Map<String, Value>,
    host: &Mock,
) -> Result<Value, String> {
    let mut vars: Map<String, Value> = script
        .params
        .iter()
        .filter_map(|p| p.default.clone().map(|v| (p.name.clone(), v)))
        .collect();
    vars.extend(script.vars.iter().cloned());
    vars.extend(overrides);
    let program: yaml_interp::Program =
        serde_json::from_value(syntax::build_program(script, &vec![], vars, 0))
            .map_err(|e| e.to_string())?;
    yaml_interp::run(&program, host, Some(host))
}
fn count(calls: &[Value], name: &str) -> usize {
    calls.iter().filter(|c| c["function"] == name).count()
}
fn taps(calls: &[Value], fragment: &str) -> usize {
    calls
        .iter()
        .filter(|c| {
            c["function"] == "tap"
                && c["args"]["position"]["template"]
                    .as_str()
                    .is_some_and(|t| t.contains(fragment))
        })
        .count()
}
fn report_case(
    name: String,
    outcome: Result<Value, String>,
    host: Mock,
    checks: Vec<(&str, bool)>,
) -> Value {
    let mut checks: Map<String, Value> = checks
        .into_iter()
        .map(|(k, v)| (k.into(), json!(v)))
        .collect();
    checks.insert(
        "no_unconfigured_mock_or_metadata_errors".into(),
        json!(outcome
            .as_ref()
            .err()
            .is_none_or(|e| !e.contains("UNCONFIGURED_")
                && !e.contains("MISSING_NATIVE_ARG")
                && !e.contains("WHOLE_MATCH_METADATA_LOST"))),
    );
    json!({"name":name,"passed":checks.values().all(|v|v==true),"checks":checks,"outcome":{"value":outcome.as_ref().ok(),"error":outcome.as_ref().err()},"native_calls":host.calls.into_inner().unwrap(),"runtime_events":host.events.into_inner().unwrap()})
}
fn parent_case(
    script: &syntax::Script,
    status: &'static str,
    test: bool,
    scan: bool,
    child_error: bool,
    confirmation_error: bool,
) -> Value {
    let host = Mock {
        status,
        scan,
        child_error,
        confirmation_error,
        ..Default::default()
    };
    let outcome = execute(
        script,
        Map::from_iter([
            ("test_mode".into(), json!(test)),
            ("test_fail".into(), json!(true)),
        ]),
        &host,
    );
    let calls = host.calls.lock().unwrap();
    let child = if test {
        "直播日常测试.yaml"
    } else {
        "云游戏日常.yaml"
    };
    let invoked = status == "confirmed" && !confirmation_error;
    let terminal = if confirmation_error {
        outcome
            .as_ref()
            .err()
            .is_some_and(|e| e.contains("PROBE_CONFIRMATION_ERROR"))
    } else if child_error && invoked {
        outcome
            .as_ref()
            .err()
            .is_some_and(|e| e.contains("PROBE_CHILD_ERROR"))
    } else if invoked {
        outcome.is_ok()
    } else {
        outcome == Ok(json!(false))
    };
    let confirmation = calls
        .iter()
        .position(|c| c["function"] == "wait_live_confirmation");
    let business = calls.iter().position(|c| c["function"] == child);
    let checks = vec![
        ("expected_terminal", terminal),
        (
            "only_confirmed_calls_selected_child",
            count(&calls, child) == usize::from(invoked)
                && count(
                    &calls,
                    if test {
                        "云游戏日常.yaml"
                    } else {
                        "直播日常测试.yaml"
                    },
                ) == 0,
        ),
        (
            "confirmation_once_after_enter_observation",
            confirmation.is_some_and(|at| count(&calls[..at], "wait_find") == 1)
                && count(&calls, "wait_live_confirmation") == 1,
        ),
        (
            "confirmation_precedes_business",
            business.is_none_or(|at| confirmation.is_some_and(|gate| gate < at)),
        ),
        (
            "confirmation_timeout_is_five_minutes",
            calls
                .iter()
                .filter(|c| c["function"] == "wait_live_confirmation")
                .all(|c| c["args"]["timeout"] == "5m"),
        ),
        (
            "entry_is_observed_forever_without_click",
            calls
                .iter()
                .filter(|c| c["function"] == "wait_find")
                .all(|c| c["args"]["forever"] == true && c["args"]["click"] != true),
        ),
        (
            "never_enters_game_or_resets_input_before_confirmation",
            taps(&calls, "enter_game") == 0
                && count(&calls, "reset_input") == 0
                && count(&calls, "tap") == usize::from(scan),
        ),
        (
            "original_child_arguments_preserved",
            calls.iter().filter(|c| c["function"] == child).all(|c| {
                c["args"]["notify_success"] == false
                    && c["args"]["notify_failure"] == true
                    && if test {
                        c["args"]["simulate_failure"] == true
                    } else {
                        c["args"]["stop_after_rewards"] == true
                    }
            }),
        ),
        (
            "only_registration_no_notification_send",
            count(&calls, "notify") == 1,
        ),
    ];
    drop(calls);
    report_case(format!("parent-{status}-test-{test}-scan-{scan}-child-error-{child_error}-gate-error-{confirmation_error}"),outcome,host,checks)
}
fn cleanup_case(
    script: &syntax::Script,
    reset: Option<bool>,
    click_more: bool,
    missing_more: bool,
) -> Value {
    let reset_label = reset
        .map(|value| value.to_string())
        .unwrap_or_else(|| "default-true".into());
    let host = Mock {
        click_more,
        missing_more,
        ..Default::default()
    };
    let outcome = execute(
        script,
        reset
            .map(|v| Map::from_iter([("reset_before_logout".into(), json!(v))]))
            .unwrap_or_default(),
        &host,
    );
    let calls = host.calls.lock().unwrap();
    let reset = reset.unwrap_or(true);
    let enter_waits = calls
        .iter()
        .filter(|c| {
            c["function"] == "wait_find"
                && c["args"]["template"]
                    .as_str()
                    .is_some_and(|t| t.contains("enter_game"))
        })
        .count();
    let checks = vec![
        (
            "expected_terminal",
            if missing_more {
                outcome
                    .as_ref()
                    .err()
                    .is_some_and(|e| e.contains("LIVE_CLOUD_WAIT_TIMEOUT"))
            } else {
                outcome.is_ok()
            },
        ),
        (
            "only_requested_reset_and_home_wait",
            count(&calls, "reset_input") == usize::from(reset) && enter_waits == usize::from(reset),
        ),
        (
            "reset_and_home_wait_precede_menu",
            !reset
                || (calls
                    .first()
                    .is_some_and(|call| call["function"] == "reset_input")
                    && calls.get(1).is_some_and(|call| {
                        call["function"] == "wait_find"
                            && call["args"]["template"]
                                .as_str()
                                .is_some_and(|name| name.contains("enter_game"))
                            && call["args"]["timeout"] == "120s"
                    })),
        ),
        (
            "menu_hover_preserved",
            count(&calls, "hover") == usize::from(!missing_more),
        ),
        (
            "click_fallback_preserved",
            taps(&calls, "more_") == usize::from(click_more && !missing_more),
        ),
        (
            "logout_clicked_once",
            taps(&calls, "logout_20261015") == usize::from(!missing_more),
        ),
        (
            "logout_confirmation_clicked_once",
            taps(&calls, "logout_confirm") == usize::from(!missing_more),
        ),
        ("never_clicks_enter_game", taps(&calls, "enter_game") == 0),
        (
            "no_business_or_notifications",
            count(&calls, "云游戏日常.yaml") == 0
                && count(&calls, "直播日常测试.yaml") == 0
                && count(&calls, "notify") == 0,
        ),
        (
            "popup_obstacle_preserved_on_all_waits",
            calls
                .iter()
                .filter(|c| c["function"] == "wait_find")
                .all(|c| {
                    c["args"]["obstacles"] == json!(["live_cloud_add_desktop_later_20261015.png"])
                }),
        ),
    ];
    drop(calls);
    report_case(
        format!(
            "cleanup-reset-{reset_label}-click-fallback-{click_more}-missing-more-{missing_more}"
        ),
        outcome,
        host,
        checks,
    )
}
fn collect_templates(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::String(s) if s.ends_with(".png") => {
            out.insert(s.clone());
        }
        Value::Object(o) => o.values().for_each(|v| collect_templates(v, out)),
        Value::Array(a) => a.iter().for_each(|v| collect_templates(v, out)),
        _ => {}
    }
}
fn references(
    repo: &Path,
    parent: &syntax::Script,
    cleanup: &syntax::Script,
) -> Result<Value, Box<dyn std::error::Error>> {
    let resource = repo.join("server/data/packages/hsr/plugins/gamer-yaml");
    let mut functions = vec![];
    let mut templates = BTreeSet::new();
    for (name, script) in [("parent", parent), ("cleanup", cleanup)] {
        collect_templates(
            &syntax::build_program(script, &vec![], Map::new(), 0),
            &mut templates,
        );
        for called in script.called_functions() {
            let native = native_funcs::native_function(&called).is_some();
            let parsed = if native {
                true
            } else if called.ends_with(".yaml") {
                let source = fs::read_to_string(resource.join("automations").join(&called))?;
                let parsed =
                    syntax::parse_script(&source).map_err(|e| format!("child {called}: {e:?}"))?;
                let unresolved: Vec<_> = parsed
                    .called_functions()
                    .into_iter()
                    .filter(|name| native_funcs::native_function(name).is_none())
                    .collect();
                if !unresolved.is_empty() {
                    return Err(format!("unresolved child functions: {unresolved:?}").into());
                }
                true
            } else {
                false
            };
            functions
                .push(json!({"script":name,"function":called,"native":native,"passed":parsed}));
        }
    }
    let filenames: Vec<_> = fs::read_dir(resource.join("templates"))?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    let mut resolved = vec![];
    for template in templates {
        let stem = template.strip_suffix(".png").unwrap();
        let matches: Vec<_> = filenames
            .iter()
            .filter(|name| {
                *name == &template
                    || name
                        .strip_prefix(stem)
                        .is_some_and(|suffix| suffix.starts_with('#') && suffix.ends_with(".png"))
            })
            .collect();
        let (png, sha, path) = if matches.len() == 1 {
            let path = resource.join("templates").join(matches[0]);
            let bytes = fs::read(&path)?;
            (
                bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                Some(format!("{:x}", Sha256::digest(&bytes))),
                Some(path),
            )
        } else {
            (false, None, None)
        };
        resolved.push(json!({"template":template,"matches":matches,"path":path,"sha256":sha,"png_signature":png,"passed":matches.len()==1&&png}));
    }
    Ok(
        json!({"name":"authoritative-syntax-native-function-and-template-references","passed":functions.iter().all(|v|v["passed"]==true)&&resolved.iter().all(|v|v["passed"]==true),"functions":functions,"templates":resolved,"note":"Candidate and child sources parsed by the current production parser. Template short names resolve uniquely with PNG signatures; no pixel matching is claimed."}),
    )
}
pub fn run(
    repo: &Path,
    args: &[String],
    source_hashes: Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let option = |name: &str| {
        args.iter()
            .position(|v| v == name)
            .and_then(|i| args.get(i + 1))
            .map(std::path::PathBuf::from)
            .ok_or_else(|| format!("{name} required"))
    };
    let parent = option("--parent")?;
    let cleanup = option("--cleanup")?;
    let report = option("--report")?;
    let source = fs::read_to_string(&parent)?;
    let cleanup_source = fs::read_to_string(&cleanup)?;
    let parent_hash = format!("{:x}", Sha256::digest(source.as_bytes()));
    let cleanup_hash = format!("{:x}", Sha256::digest(cleanup_source.as_bytes()));
    let module = repo.join("tools/live-workflow/daily-no-target-probe/src/live_confirmation.rs");
    let module_hash = format!(
        "{:x}",
        Sha256::digest(include_bytes!("live_confirmation.rs"))
    );
    if module_hash != format!("{:x}", Sha256::digest(fs::read(&module)?)) {
        return Err("probe module changed since build".into());
    }
    let parent_script =
        syntax::parse_script(&source).map_err(|e| format!("parent syntax: {e:?}"))?;
    let cleanup_script =
        syntax::parse_script(&cleanup_source).map_err(|e| format!("cleanup syntax: {e:?}"))?;
    let mut scenarios = vec![references(repo, &parent_script, &cleanup_script)?];
    for status in ["confirmed", "cancelled", "timeout"] {
        for test in [false, true] {
            for scan in [false, true] {
                scenarios.push(parent_case(
                    &parent_script,
                    status,
                    test,
                    scan,
                    false,
                    false,
                ))
            }
        }
    }
    for test in [false, true] {
        scenarios.push(parent_case(
            &parent_script,
            "confirmed",
            test,
            true,
            true,
            false,
        ))
    }
    scenarios.push(parent_case(
        &parent_script,
        "confirmed",
        false,
        true,
        false,
        true,
    ));
    for reset in [None, Some(true), Some(false)] {
        for click in [false, true] {
            scenarios.push(cleanup_case(&cleanup_script, reset, click, false))
        }
    }
    scenarios.push(cleanup_case(&cleanup_script, Some(false), false, true));
    if parent_hash != format!("{:x}", Sha256::digest(fs::read(&parent)?))
        || cleanup_hash != format!("{:x}", Sha256::digest(fs::read(&cleanup)?))
    {
        return Err("candidate changed during probe".into());
    }
    let passed = scenarios.iter().all(|s| s["passed"] == true);
    let result = json!({"passed":passed,"mode":"live-confirmation","parent_sha256":parent_hash,"cleanup_sha256":cleanup_hash,"parent":parent,"cleanup":cleanup,"runtime_contract":yaml_interp::RUNTIME_CONTRACT,"source_hashes":source_hashes,"source_hashes_verified_at_runtime":true,"probe_source":{"path":module,"sha256":module_hash},"case_count":scenarios.len(),"scenarios":scenarios,"production_accessed":false,"device_inputs":0,"notifications_sent":0,"listeners_started":0,"limitations":["Only script control flow is proven. Native confirmation identity/deadline persistence, queue business settlement, screenshots, actual logout and notifications are mocked.","Real local cleanup helper is interpreted unchanged; cross-script daily/test calls are recorded sentinels and never execute real routines."]});
    fs::create_dir_all(report.parent().ok_or("report needs parent")?)?;
    fs::write(&report, serde_json::to_vec_pretty(&result)?)?;
    println!(
        "{}",
        json!({"passed":passed,"case_count":result["case_count"],"parent_sha256":parent_hash,"cleanup_sha256":cleanup_hash,"report":report,"failures":result["scenarios"].as_array().unwrap().iter().filter(|s|s["passed"]!=true).map(|s|s["name"].clone()).collect::<Vec<_>>()})
    );
    if !passed {
        std::process::exit(1)
    }
    Ok(())
}
