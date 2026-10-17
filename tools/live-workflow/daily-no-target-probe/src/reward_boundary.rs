//! Reward completion boundary checks using the production parser and interpreter.
//! All native calls are deterministic in-memory mocks. No I/O capability exists.
use crate::{native_funcs, syntax};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, sync::Mutex};
use yaml_interp::{EventSink, HostError, HostErrorKind, HostFunctions};

const GOLD: &str = "daily_500_claimed_check.png";
const CHECKPOINT: &str = "hsr_daily_final_reward";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Reward {
    Already,
    Clicked,
    ClaimThenFull,
    NoTasks,
    Exhausted,
    MissingGold,
    CheckpointTimeout,
    MissingTemplate,
}
#[derive(Clone, Copy, Debug)]
enum Completion {
    True,
    False,
    Error,
}
#[derive(Clone, Copy, Debug)]
enum Route {
    FastPhone,
    FastSupply,
    FastCloud,
    FastPopup,
    Prewait,
    Challenge,
    Guide,
    Training,
    RetryPhone,
    RetrySupply,
    RetryCloud,
    RetryPopup,
}
impl Route {
    const ALL: [Self; 12] = [
        Self::FastPhone,
        Self::FastSupply,
        Self::FastCloud,
        Self::FastPopup,
        Self::Prewait,
        Self::Challenge,
        Self::Guide,
        Self::Training,
        Self::RetryPhone,
        Self::RetrySupply,
        Self::RetryCloud,
        Self::RetryPopup,
    ];
    fn landing(self, first: bool) -> Option<usize> {
        match (self, first) {
            (Self::FastPhone, true) | (Self::RetryPhone, false) => Some(1),
            (Self::FastSupply, true) | (Self::RetrySupply, false) => Some(3),
            (Self::FastCloud, true) | (Self::RetryCloud, false) => Some(0),
            (Self::FastPopup, true) | (Self::RetryPopup, false) => Some(2),
            _ => None,
        }
    }
    fn phone(self) -> bool {
        matches!(
            self,
            Self::FastPhone | Self::FastSupply | Self::RetryPhone | Self::RetrySupply
        )
    }
    fn cloud(self) -> bool {
        matches!(
            self,
            Self::FastCloud | Self::FastPopup | Self::Prewait | Self::RetryCloud | Self::RetryPopup
        )
    }
    fn drain(self) -> bool {
        matches!(self, Self::Challenge | Self::Training)
    }
}

#[derive(Default)]
struct State {
    calls: Vec<Value>,
    events: Vec<Value>,
    checkpoints: Vec<Value>,
    rounds: usize,
    claims: usize,
    landing_scans: usize,
}
struct Host {
    reward: Option<Reward>,
    route: Option<Route>,
    completion: Completion,
    poison_close: bool,
    finish_missing: bool,
    state: Mutex<State>,
}
impl Default for Host {
    fn default() -> Self {
        Self {
            reward: None,
            route: None,
            completion: Completion::True,
            poison_close: false,
            finish_missing: false,
            state: Mutex::new(State::default()),
        }
    }
}
fn error(message: impl Into<String>) -> HostError {
    HostError::new(HostErrorKind::Failed, message)
}
fn hit(template: &str) -> Value {
    json!({"template":template,"score":1.0,"center":{"x":0.84,"y":0.28},"_frame":{"synthetic":true}})
}
fn normalized(name: &str, args: Value) -> Result<Value, HostError> {
    let Some(function) = native_funcs::native_function(name) else {
        return Ok(args);
    };
    let mut values = match args.as_object() {
        Some(values) => values.clone(),
        None => function
            .params
            .first()
            .map(|first| Map::from_iter([(first.name.into(), args)]))
            .unwrap_or_default(),
    };
    for parameter in &function.params {
        if !values.contains_key(parameter.name) {
            if let Some(value) = &parameter.default {
                values.insert(parameter.name.into(), value.clone());
            } else if parameter.required {
                return Err(error(format!(
                    "MISSING_NATIVE_ARG:{name}.{}",
                    parameter.name
                )));
            }
        }
    }
    Ok(Value::Object(values))
}
impl HostFunctions for Host {
    fn invoke(&self, name: &str, args: Value) -> Result<Value, HostError> {
        let args = normalized(name, args)?;
        let mut state = self.state.lock().unwrap();
        if state.calls.len() > 10000 {
            return Err(error("MOCK_CALL_BUDGET"));
        }
        state.calls.push(json!({"function":name,"args":args}));
        let template = args["template"].as_str().unwrap_or("");
        match name {
            "eq" | "ne" => {
                let equal = args["a"] == args["b"];
                Ok(json!(if name == "eq" { equal } else { !equal }))
            }
            "notify" | "log" | "sleep" | "reset_input" | "key" => Ok(Value::Null),
            "probe_daily_result" | "probe_phone_result" => match self.completion {
                Completion::True => Ok(json!(true)),
                Completion::False => Ok(json!(false)),
                Completion::Error => Err(error("PROBE_DAILY_FAILURE")),
            },
            "probe_cloud_enter" => Ok(Value::Null),
            "probe_drain" => Ok(json!({"status":"progress"})),
            "find_any" => {
                let templates = args["templates"]
                    .as_array()
                    .ok_or_else(|| error("MOCK_TEMPLATES_REQUIRED"))?;
                if templates.len() == 2 && templates[1].as_str().is_some_and(|s| s.contains("手机"))
                {
                    let mut value = hit(templates[1].as_str().unwrap());
                    value["index"] = json!(1);
                    return Ok(value);
                }
                if templates.len() != 4 {
                    return Err(error("UNCONFIGURED_FIND_ANY"));
                }
                state.landing_scans += 1;
                let index = self
                    .route
                    .and_then(|route| route.landing(state.landing_scans == 1));
                Ok(index
                    .map(|index| {
                        let mut value = hit(templates[index].as_str().unwrap());
                        value["index"] = json!(index);
                        value
                    })
                    .unwrap_or(Value::Null))
            }
            "find" | "observe" => {
                if let Some(reward) = self.reward {
                    if template.contains("commission_title") {
                        return Ok(Value::Null);
                    }
                    if template.contains("training_title") {
                        return Ok(hit(template));
                    }
                    if template.contains("full_active_text") {
                        state.rounds += 1;
                        let full = matches!(
                            reward,
                            Reward::Already
                                | Reward::Clicked
                                | Reward::MissingGold
                                | Reward::CheckpointTimeout
                                | Reward::MissingTemplate
                        ) || (reward == Reward::ClaimThenFull && state.claims > 0);
                        return Ok(if full { hit(template) } else { Value::Null });
                    }
                    if template.contains("claim_label") {
                        return Ok(
                            if matches!(reward, Reward::ClaimThenFull | Reward::Exhausted) {
                                hit(template)
                            } else {
                                Value::Null
                            },
                        );
                    }
                    if template.contains("goto_label") {
                        return Ok(Value::Null);
                    }
                    return Err(error(format!("UNCONFIGURED_REWARD_TEMPLATE:{template}")));
                }
                let present = self.route.is_some_and(|route| match route {
                    Route::Challenge => template.contains("challenge_word"),
                    Route::Guide => template.contains("training_title"),
                    Route::Training => template.contains("survival_index"),
                    _ => false,
                });
                Ok(if present { hit(template) } else { Value::Null })
            }
            "wait_find" => {
                if template.contains("手机") || template.contains("close_blank") {
                    return Ok(hit(template));
                }
                if template.contains("daily_enter_game") {
                    return Ok(if matches!(self.route, Some(Route::Prewait)) {
                        hit(template)
                    } else {
                        Value::Null
                    });
                }
                Err(error(format!("UNCONFIGURED_WAIT:{template}")))
            }
            "tap" => {
                if args["position"]["template"]
                    .as_str()
                    .is_some_and(|s| s.contains("claim_label"))
                {
                    state.claims += 1;
                    return Ok(Value::Null);
                }
                Err(error("UNCONFIGURED_TAP"))
            }
            "tap_template" => {
                if template.contains("daily_last_active") {
                    return Ok(if self.reward == Some(Reward::Clicked) {
                        hit(template)
                    } else {
                        Value::Null
                    });
                }
                if template.contains("close_x_guide") || template.contains("close_x_menu") {
                    return if self.poison_close {
                        Err(error("PROBE_CLOSE_MUST_NOT_RUN"))
                    } else {
                        Ok(hit(template))
                    };
                }
                Err(error(format!("UNCONFIGURED_TAP_TEMPLATE:{template}")))
            }
            "checkpoint" => {
                if args["id"] != CHECKPOINT
                    || template != GOLD
                    || args["region"] != json!([0.8, 0.22, 0.09, 0.15])
                    || args["timeout"] != "15s"
                {
                    return Err(error("CHECKPOINT_CONTRACT_CHANGED"));
                }
                match self.reward {
                    Some(Reward::CheckpointTimeout) => Err(error("VISUAL_TIMEOUT:checkpoint")),
                    Some(Reward::MissingTemplate) => Err(HostError::new(
                        HostErrorKind::NotFound,
                        "TEMPLATE_NOT_FOUND:daily_500_claimed_check.png",
                    )),
                    Some(Reward::MissingGold) => Ok(Value::Null),
                    _ => {
                        let value = hit(template);
                        state
                            .checkpoints
                            .push(json!({"id":CHECKPOINT,"match":value}));
                        Ok(value)
                    }
                }
            }
            "finish" => {
                if self.poison_close {
                    return Err(error("PROBE_FINISH_MUST_NOT_RUN"));
                }
                Ok(if self.finish_missing {
                    Value::Null
                } else {
                    hit(template)
                })
            }
            "fail" => Err(error(format!(
                "SCRIPT_FAIL:{}:{}",
                args["code"].as_str().unwrap_or(""),
                args["message"].as_str().unwrap_or("")
            ))),
            _ => Err(error(format!("UNCONFIGURED_NATIVE:{name}"))),
        }
    }
}
impl EventSink for Host {
    fn emit(&self, event: Value) {
        self.state.lock().unwrap().events.push(event)
    }
}

fn function<'a>(script: &'a syntax::Script, name: &str) -> Result<&'a syntax::FunctionDef, String> {
    script
        .functions
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
        .ok_or_else(|| format!("missing function {name}"))
}
fn replace(script: &mut syntax::Script, name: &str, native: &str) {
    let source=format!("functions:\n  {name}:\n    run:\n      - {native}: {{}}\n        as: result\n      - return: $result\nrun: []\n");
    let replacement = syntax::parse_script(&source).expect("static probe function syntax");
    let item = script
        .functions
        .iter_mut()
        .find(|(key, _)| key == name)
        .expect("candidate function exists");
    item.1 = replacement.functions[0].1.clone();
}
fn initial_vars(script: &syntax::Script, overrides: Map<String, Value>) -> Map<String, Value> {
    let mut values: Map<String, Value> = script
        .params
        .iter()
        .filter_map(|p| p.default.clone().map(|value| (p.name.clone(), value)))
        .collect();
    values.extend(script.vars.iter().cloned());
    values.extend(overrides);
    values
}
fn execute(
    script: &syntax::Script,
    entry: Option<&str>,
    overrides: Map<String, Value>,
    host: &Host,
) -> Result<Value, String> {
    let vars = initial_vars(script, overrides);
    let wire = match entry {
        Some(name) => syntax::build_function_program(
            name,
            function(script, name)?,
            &script.functions,
            vars,
            0,
        ),
        None => syntax::build_program(script, &vec![], vars, 0),
    };
    let program: yaml_interp::Program = serde_json::from_value(wire).map_err(|e| e.to_string())?;
    yaml_interp::run(&program, host, Some(host))
}
fn call_count(state: &State, name: &str) -> usize {
    state
        .calls
        .iter()
        .filter(|call| call["function"] == name)
        .count()
}
fn closing_count(state: &State) -> usize {
    state
        .calls
        .iter()
        .filter(|call| {
            call["function"] == "finish"
                || (call["function"] == "tap_template"
                    && call["args"]["template"].as_str().is_some_and(|name| {
                        name.contains("close_x_guide") || name.contains("close_x_menu")
                    }))
        })
        .count()
}
fn result(
    name: String,
    outcome: Result<Value, String>,
    host: Host,
    mut checks: Map<String, Value>,
) -> Value {
    let state = host.state.into_inner().unwrap();
    checks.insert(
        "no_unconfigured_mock_calls".into(),
        json!(outcome
            .as_ref()
            .err()
            .is_none_or(|e| !e.contains("UNCONFIGURED_")
                && !e.contains("MISSING_NATIVE_ARG")
                && !e.contains("MOCK_")
                && !e.contains("CHECKPOINT_CONTRACT_CHANGED"))),
    );
    let passed = checks.values().all(|check| check == true);
    json!({"name":name,"passed":passed,"checks":checks,"outcome":{"value":outcome.as_ref().ok(),"error":outcome.as_ref().err()},"native_calls":state.calls,"checkpoints":state.checkpoints,"runtime_events":state.events,"rounds":state.rounds,"task_claims":state.claims})
}
fn checks(items: impl IntoIterator<Item = (&'static str, bool)>) -> Map<String, Value> {
    items
        .into_iter()
        .map(|(key, value)| (key.into(), json!(value)))
        .collect()
}
fn reward_cases(script: &syntax::Script) -> Vec<Value> {
    [
        Reward::Already,
        Reward::Clicked,
        Reward::ClaimThenFull,
        Reward::NoTasks,
        Reward::Exhausted,
        Reward::MissingGold,
        Reward::CheckpointTimeout,
        Reward::MissingTemplate,
    ]
    .into_iter()
    .map(|reward| {
        let host = Host {
            reward: Some(reward),
            ..Default::default()
        };
        let outcome = execute(script, Some("daily_tasks"), Map::new(), &host);
        let state = host.state.lock().unwrap();
        let success = matches!(
            reward,
            Reward::Already | Reward::Clicked | Reward::ClaimThenFull
        );
        let negative = matches!(reward, Reward::NoTasks | Reward::Exhausted);
        let expected = if success {
            outcome.as_ref().is_ok_and(|v| v == true)
        } else if negative {
            outcome.as_ref().is_ok_and(|v| v == false)
        } else {
            outcome.is_err()
        };
        let expected_error = match reward {
            Reward::MissingGold => Some("ACTIVE_REWARD_UNCONFIRMED"),
            Reward::CheckpointTimeout => Some("VISUAL_TIMEOUT"),
            Reward::MissingTemplate => Some("TEMPLATE_NOT_FOUND"),
            _ => None,
        };
        let checks = checks([
            ("expected_completion_or_error", expected),
            (
                "expected_error_code",
                expected_error
                    .is_none_or(|code| outcome.as_ref().err().is_some_and(|e| e.contains(code))),
            ),
            (
                "checkpoint_only_after_positive_gold",
                state.checkpoints.len() == usize::from(success),
            ),
            (
                "gold_uses_checkpoint_not_observe",
                !state
                    .calls
                    .iter()
                    .any(|call| call["function"] == "observe" && call["args"]["template"] == GOLD),
            ),
            ("rewards_do_not_finish_or_close", closing_count(&state) == 0),
            (
                "eight_round_bound",
                state.rounds <= 8
                    && (reward != Reward::Exhausted || (state.rounds == 8 && state.claims == 8)),
            ),
            (
                "already_claimed_does_not_open_reward_popup",
                reward != Reward::Already || call_count(&state, "wait_find") == 0,
            ),
            (
                "clicked_reward_confirms_popup",
                reward != Reward::Clicked || call_count(&state, "wait_find") == 1,
            ),
            (
                "claim_then_full_requires_second_round",
                reward != Reward::ClaimThenFull || (state.rounds == 2 && state.claims == 1),
            ),
        ]);
        drop(state);
        result(format!("daily_tasks-{reward:?}"), outcome, host, checks)
    })
    .collect()
}
fn phone_cases(script: &syntax::Script) -> Vec<Value> {
    [Completion::True, Completion::False, Completion::Error]
        .into_iter()
        .map(|completion| {
            let mut script = script.clone();
            replace(&mut script, "daily_tasks", "probe_daily_result");
            let host = Host {
                completion,
                ..Default::default()
            };
            let outcome = execute(&script, Some("phone_resume_confirm"), Map::new(), &host);
            let state = host.state.lock().unwrap();
            let expected = match completion {
                Completion::True => outcome.as_ref().is_ok_and(|v| v == true),
                Completion::False => outcome.as_ref().is_ok_and(|v| v == false),
                Completion::Error => outcome
                    .as_ref()
                    .err()
                    .is_some_and(|e| e.contains("PROBE_DAILY_FAILURE")),
            };
            let checks = checks([
                (
                    "daily_result_forwarded_without_truthiness_conversion",
                    expected,
                ),
                (
                    "daily_called_once_after_phone_confirmation",
                    call_count(&state, "probe_daily_result") == 1
                        && call_count(&state, "wait_find") == 1,
                ),
                ("no_close_or_finish", closing_count(&state) == 0),
            ]);
            drop(state);
            result(
                format!("phone_resume_confirm-{completion:?}"),
                outcome,
                host,
                checks,
            )
        })
        .collect()
}
fn main_case(
    script: &syntax::Script,
    route: Route,
    completion: Completion,
    live: bool,
    poison_close: bool,
    finish_missing: bool,
) -> Value {
    let mut script = script.clone();
    for (name, native) in [
        ("daily_tasks", "probe_daily_result"),
        ("phone_resume_confirm", "probe_phone_result"),
        ("cloud_enter_game", "probe_cloud_enter"),
        ("drain_stamina", "probe_drain"),
    ] {
        replace(&mut script, name, native)
    }
    let host = Host {
        route: Some(route),
        completion,
        poison_close,
        finish_missing,
        ..Default::default()
    };
    let overrides = if live {
        Map::from_iter([("stop_after_rewards".into(), json!(true))])
    } else {
        Map::new()
    };
    let outcome = execute(&script, None, overrides, &host);
    let state = host.state.lock().unwrap();
    let failing_daily = matches!(completion, Completion::Error);
    let live_unconfirmed = live && matches!(completion, Completion::False);
    let expected_error = if failing_daily {
        Some("PROBE_DAILY_FAILURE")
    } else if live_unconfirmed {
        Some("ACTIVE_REWARD_UNCONFIRMED")
    } else if poison_close && !live {
        Some("PROBE_CLOSE_MUST_NOT_RUN")
    } else if finish_missing && !live {
        Some("VISUAL_TIMEOUT")
    } else {
        None
    };
    let expected_terminal = match expected_error {
        Some(code) => outcome.as_ref().err().is_some_and(|e| e.contains(code)),
        None => outcome.is_ok(),
    };
    let checks = checks([
        ("expected_terminal", expected_terminal),
        (
            "live_confirmed_returns_true",
            !live
                || !matches!(completion, Completion::True)
                || outcome.as_ref().is_ok_and(|v| v == true),
        ),
        (
            "live_or_failed_daily_skips_original_closing",
            !(live || failing_daily) || closing_count(&state) == 0,
        ),
        (
            "ordinary_default_preserves_closing",
            live || failing_daily || closing_count(&state) == if poison_close { 1 } else { 3 },
        ),
        (
            "original_entry_routes_to_expected_completion_function",
            call_count(&state, "probe_phone_result") == usize::from(route.phone())
                && call_count(&state, "probe_daily_result") == usize::from(!route.phone()),
        ),
        (
            "original_cloud_entry_branch_preserved",
            call_count(&state, "probe_cloud_enter") == usize::from(route.cloud()),
        ),
        (
            "original_resume_drain_branch_preserved",
            call_count(&state, "probe_drain") == usize::from(route.drain()),
        ),
        (
            "notifications_are_registration_only",
            call_count(&state, "notify") == 2,
        ),
    ]);
    drop(state);
    result(
        format!(
            "main-{route:?}-completion-{completion:?}-{}-close-{}-finish-{}",
            if live { "live" } else { "ordinary-default" },
            if poison_close { "poison" } else { "ok" },
            if finish_missing { "missing" } else { "ok" }
        ),
        outcome,
        host,
        checks,
    )
}
fn walk<'a>(
    steps: &'a [syntax::SurfaceStep],
    path: &str,
    out: &mut Vec<(String, &'a syntax::SurfaceStep)>,
) {
    use syntax::SurfaceStep::*;
    for (index, step) in steps.iter().enumerate() {
        let at = format!("{path}[{index}]");
        out.push((at.clone(), step));
        match step {
            If {
                then_steps,
                else_steps,
                ..
            } => {
                walk(then_steps, &format!("{at}.then"), out);
                walk(else_steps, &format!("{at}.else"), out)
            }
            Repeat { body, .. } => walk(body, &format!("{at}.do"), out),
            _ => {}
        }
    }
}
fn ast_checks(script: &syntax::Script, parent: &syntax::Script) -> Value {
    let mut main = vec![];
    walk(&script.run, "run", &mut main);
    let bindings:Vec<_>=main.iter().filter_map(|(path,step)|match step {syntax::SurfaceStep::Call{name,save_as,..}if matches!(name.as_str(),"daily_tasks"|"phone_resume_confirm")=>Some(json!({"path":path,"function":name,"as":save_as,"passed":save_as.as_deref()==Some("daily_completed")})),_=>None}).collect();
    let mut parent_steps = vec![];
    walk(&parent.run, "run", &mut parent_steps);
    let parent_calls:Vec<_>=parent_steps.iter().filter_map(|(path,step)|match step {syntax::SurfaceStep::Call{name,args,..}if name=="云游戏日常.yaml"=>Some(json!({"path":path,"function":name,"args":args.to_wire(),"passed":matches!(args,syntax::SurfaceExpr::Map(values)if values.iter().any(|(key,value)|key=="stop_after_rewards"&&*value==syntax::SurfaceExpr::Lit(json!(true))))})),_=>None}).collect();
    let default_false = script.params.iter().any(|p| {
        p.name == "stop_after_rewards"
            && p.ty == syntax::ParamType::Boolean
            && p.default == Some(json!(false))
    });
    let passed = default_false
        && bindings.len() == 10
        && bindings.iter().all(|v| v["passed"] == true)
        && parent_calls.len() == 1
        && parent_calls.iter().all(|v| v["passed"] == true);
    json!({"name":"candidate-and-parent-AST-contract","passed":passed,"checks":{"stop_after_rewards_boolean_default_false":default_false,"ten_main_completion_calls_bound":bindings.len()==10&&bindings.iter().all(|v|v["passed"]==true),"parent_formal_daily_call_passes_true":parent_calls.len()==1&&parent_calls.iter().all(|v|v["passed"]==true)},"main_bindings":bindings,"parent_calls":parent_calls})
}

pub fn run(
    repo: &Path,
    args: &[String],
    source_hashes: Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let option = |name: &str| {
        args.iter()
            .position(|value| value == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or_else(|| format!("{name} is required"))
    };
    let candidate = std::path::PathBuf::from(option("--candidate")?);
    let parent = std::path::PathBuf::from(option("--parent")?);
    let report = std::path::PathBuf::from(option("--report")?);
    let module = repo.join("tools/live-workflow/daily-no-target-probe/src/reward_boundary.rs");
    let module_hash = format!("{:x}", Sha256::digest(include_bytes!("reward_boundary.rs")));
    if module_hash != format!("{:x}", Sha256::digest(fs::read(&module)?)) {
        return Err("reward probe source changed since build; rebuild probe".into());
    }
    let source = fs::read_to_string(&candidate)?;
    let parent_source = fs::read_to_string(&parent)?;
    let sha = format!("{:x}", Sha256::digest(source.as_bytes()));
    let parent_sha = format!("{:x}", Sha256::digest(parent_source.as_bytes()));
    let script = syntax::parse_script(&source).map_err(|e| format!("daily syntax: {e:?}"))?;
    let parent_script =
        syntax::parse_script(&parent_source).map_err(|e| format!("parent syntax: {e:?}"))?;
    let mut scenarios = vec![ast_checks(&script, &parent_script)];
    scenarios.extend(reward_cases(&script));
    scenarios.extend(phone_cases(&script));
    for route in Route::ALL {
        for completion in [Completion::True, Completion::False, Completion::Error] {
            scenarios.push(main_case(&script, route, completion, true, true, false));
            scenarios.push(main_case(&script, route, completion, false, false, false));
        }
    }
    scenarios.push(main_case(
        &script,
        Route::FastPhone,
        Completion::True,
        false,
        true,
        false,
    ));
    scenarios.push(main_case(
        &script,
        Route::FastPhone,
        Completion::True,
        false,
        false,
        true,
    ));
    let passed = scenarios.iter().all(|s| s["passed"] == true);
    if sha != format!("{:x}", Sha256::digest(fs::read(&candidate)?))
        || parent_sha != format!("{:x}", Sha256::digest(fs::read(&parent)?))
    {
        return Err("candidate changed during probe".into());
    }
    let summary = json!({"passed":passed,"mode":"reward-boundary","candidate_sha256":sha,"parent_sha256":parent_sha,"scope":"current authoritative syntax and interpreter; deterministic mocked natives only","candidate":{"path":candidate,"sha256":sha},"parent":{"path":parent,"sha256":parent_sha},"probe_source":{"path":module,"sha256":module_hash},"runtime_contract":yaml_interp::RUNTIME_CONTRACT,"source_hashes":source_hashes,"source_hashes_verified_at_runtime":true,"case_count":scenarios.len(),"scenarios":scenarios,"production_accessed":false,"device_inputs":0,"listeners_started":0,"notifications_sent":0,"limitations":["Native checkpoint image matching, persistence, browser reset and game effects are mocked; this proves script control flow only","Main entry branches are unchanged; only named business function definitions are replaced by deterministic local functions","Notify calls register only in memory; no notification sender or service is linked"]});
    fs::create_dir_all(report.parent().ok_or("report needs parent directory")?)?;
    fs::write(&report, serde_json::to_vec_pretty(&summary)?)?;
    println!(
        "{}",
        json!({"passed":passed,"case_count":summary["case_count"],"failed_scenarios":summary["scenarios"].as_array().unwrap().iter().filter(|s|s["passed"]!=true).map(|s|s["name"].clone()).collect::<Vec<_>>(),"report":report,"candidate_sha256":sha,"parent_sha256":parent_sha})
    );
    if !passed {
        std::process::exit(1)
    }
    Ok(())
}
