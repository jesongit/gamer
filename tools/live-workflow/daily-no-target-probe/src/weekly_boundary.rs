//! Saved script control flow with an independent, stateful game model.
//! All inputs, OCR, matching and time are synthetic; no device or socket exists.
use crate::{native_funcs, syntax};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, sync::Mutex};
use yaml_interp::{EventSink, HostError, HostErrorKind, HostFunctions};

struct Case {
    name: &'static str,
    layout: &'static str,
    power: i64,
    weekly: i64,
    fault: &'static str,
    battles: usize,
    switches: usize,
    final_power: i64,
    error: Option<&'static str>,
}
#[derive(Default)]
struct State {
    screen: String,
    layout: String,
    power: i64,
    weekly: i64,
    selected: i64,
    battles: usize,
    switches: usize,
    weekly_reads: usize,
    cost_reads_while_exhausted: usize,
    violations: Vec<String>,
    calls: Vec<Value>,
    events: Vec<Value>,
}
struct Host<'a> {
    case: &'a Case,
    state: Mutex<State>,
}
fn error(message: impl Into<String>) -> HostError {
    HostError::new(HostErrorKind::Failed, message)
}
fn hit(template: &str, x: f64, y: f64) -> Value {
    json!({"template":template,"score":0.99,"center":{"x":x,"y":y},"_frame":{"synthetic":true}})
}
fn cost(state: &State) -> i64 {
    if state.layout == "echo" {
        30
    } else {
        40
    }
}
fn maximum(state: &State) -> i64 {
    if state.layout == "echo" {
        state.weekly.min(3)
    } else {
        6
    }
}
fn normalize(name: &str, args: Value) -> Result<Value, HostError> {
    let schema = native_funcs::native_function(name)
        .ok_or_else(|| error(format!("UNCONFIGURED_NATIVE:{name}")))?;
    let mut values = args.as_object().cloned().unwrap_or_else(|| {
        schema
            .params
            .first()
            .map(|p| Map::from_iter([(p.name.into(), args)]))
            .unwrap_or_default()
    });
    for p in &schema.params {
        if !values.contains_key(p.name) {
            if let Some(default) = &p.default {
                values.insert(p.name.into(), default.clone());
            } else if p.required {
                return Err(error(format!("MISSING_NATIVE_ARG:{name}.{}", p.name)));
            }
        }
    }
    Ok(Value::Object(values))
}
impl Host<'_> {
    fn visible(&self, template: &str, s: &mut State) -> Value {
        let screen = s.screen.as_str();
        let value = if template.contains("weekly_label") {
            if screen == "dungeon"
                && s.layout == "echo"
                && self.case.fault != "weekly_label_missing"
            {
                Some((
                    1609.0 / 1920.0,
                    if s.weekly == 0 { 916.0 } else { 775.0 } / 1080.0,
                ))
            } else {
                None
            }
        } else if template.contains("stamina_cost_icon") {
            if screen == "dungeon" && s.layout == "echo" && s.weekly == 0 {
                s.cost_reads_while_exhausted += 1;
            }
            if screen == "dungeon"
                && !(s.layout == "echo" && s.weekly == 0)
                && self.case.fault != "cost_missing"
            {
                Some((0.70, 0.91))
            } else {
                None
            }
        } else if template.contains("power_anchor") {
            (screen == "dungeon").then_some((0.84, 0.06))
        } else if template.contains("challenge_count_label") {
            (screen == "dungeon" && maximum(s) > 0).then_some((0.80, 0.80))
        } else if template.contains("plus_btn") {
            (screen == "dungeon" && s.selected < maximum(s)).then_some((0.96, 0.82))
        } else if template.contains("minus_btn") {
            (screen == "dungeon" && s.selected > 1).then_some((0.65, 0.82))
        } else if template.contains("challenge_word") {
            (screen == "dungeon"
                && !(self.case.fault == "default_challenge_missing" && s.switches > 0))
                .then_some((0.85625, 0.91))
        } else if template.contains("daily_start_battle") {
            (screen == "team" && self.case.fault != "start_missing").then_some((0.87, 0.91))
        } else if template.contains("exit_level") {
            (screen == "battle" && self.case.fault != "exit_missing").then_some((0.35, 0.91))
        } else if template.contains("training_title") {
            (screen == "guide").then_some((0.10, 0.08))
        } else if template.contains("guide_icon") {
            (screen == "menu" && self.case.fault != "guide_missing").then_some((0.70, 0.69))
        } else if template.contains("survival_index_title") {
            (screen == "index").then_some((0.08, 0.07))
        } else if template.contains("no_target_cavern") {
            (screen == "index").then_some((0.25, 0.60))
        } else if template.contains("no_target_storage") {
            (screen == "cavern_list").then_some((951.0 / 1920.0, 839.0 / 1080.0))
        } else if template.contains("no_target_enter") {
            (screen == "cavern_list").then_some((1559.0 / 1920.0, 816.0 / 1080.0))
        } else if template.contains("close_x_challenge") {
            (screen == "dungeon" && self.case.fault != "close_missing").then_some((0.97, 0.06))
        } else {
            None
        };
        value
            .map(|(x, y)| hit(template, x, y))
            .unwrap_or(Value::Null)
    }
    fn tap(&self, template: &str, s: &mut State) -> Result<(), HostError> {
        if template.contains("plus_btn") {
            if s.selected >= maximum(s) {
                return Err(error("MODEL_PLUS_DISABLED"));
            }
            s.selected += 1;
        } else if template.contains("minus_btn") {
            if s.selected <= 1 {
                return Err(error("MODEL_MINUS_DISABLED"));
            }
            s.selected -= 1;
        } else if template.contains("challenge_word") {
            if s.screen != "dungeon" || (s.layout == "echo" && s.weekly == 0) {
                s.violations
                    .push("challenge clicked with no weekly attempts".into());
                return Err(error("MODEL_EXHAUSTED_CHALLENGE"));
            }
            s.screen = "team".into();
        } else if template.contains("daily_start_battle") {
            if s.screen != "team" {
                return Err(error("MODEL_START_OUTSIDE_TEAM"));
            }
            s.screen = "battle".into();
        } else if template.contains("exit_level") {
            let spent = cost(s) * s.selected;
            if spent > s.power || (s.layout == "echo" && s.selected > s.weekly) {
                return Err(error("MODEL_INVALID_BATCH"));
            }
            s.power -= spent;
            if s.layout == "echo" {
                s.weekly -= s.selected;
            }
            s.battles += 1;
            s.screen = "dungeon".into();
            s.selected = 1;
        } else if template.contains("close_x_challenge") {
            s.screen = "world".into();
        } else if template.contains("guide_icon") {
            s.screen = "guide".into();
        } else if template.contains("no_target_cavern") {
            s.screen = "cavern_list".into();
        } else if template.contains("no_target_enter") {
            s.switches += 1;
            s.layout = if self.case.fault == "wrong_default" {
                "echo"
            } else {
                "cavern"
            }
            .into();
            s.screen = "dungeon".into();
            s.selected = 1;
        } else {
            return Err(error(format!("MODEL_UNCONFIGURED_TAP:{template}")));
        }
        Ok(())
    }
}
impl HostFunctions for Host<'_> {
    fn invoke(&self, name: &str, args: Value) -> Result<Value, HostError> {
        let a = normalize(name, args)?;
        let mut s = self.state.lock().unwrap();
        if s.calls.len() > 12000 {
            return Err(error("MODEL_CALL_BUDGET"));
        }
        let call = json!({"function":name,"args":a,"screen":s.screen,"layout":s.layout,
            "power":s.power,"weekly":s.weekly,"selected":s.selected});
        s.calls.push(call);
        let template = a["template"].as_str().unwrap_or("");
        match name {
            "eq" | "ne" => {
                let eq = match (a["a"].as_f64(), a["b"].as_f64()) {
                    (Some(x), Some(y)) => x == y,
                    _ => a["a"] == a["b"],
                };
                Ok(json!(if name == "eq" { eq } else { !eq }))
            }
            "lt" | "le" | "gt" | "ge" => {
                let x = a["a"].as_f64().ok_or_else(|| error("MODEL_COMPARISON_A"))?;
                let y = a["b"].as_f64().ok_or_else(|| error("MODEL_COMPARISON_B"))?;
                Ok(json!(match name {
                    "lt" => x < y,
                    "le" => x <= y,
                    "gt" => x > y,
                    _ => x >= y,
                }))
            }
            "calculate" => {
                let x = a["a"].as_f64().ok_or_else(|| error("MODEL_CALCULATE_A"))?;
                let y = a["b"].as_f64().ok_or_else(|| error("MODEL_CALCULATE_B"))?;
                let result = match a["op"].as_str().unwrap() {
                    "add" => x + y,
                    "sub" => x - y,
                    "mul" => x * y,
                    "floor_div" => (x / y).floor(),
                    "min" => x.min(y),
                    _ => return Err(error("MODEL_CALCULATE_OP")),
                };
                Ok(if result.fract() == 0.0 {
                    json!(result as i64)
                } else {
                    json!(result)
                })
            }
            "number" => {
                let text = a["text"].as_str().unwrap();
                let values: Vec<_> = if a["format"] == "fraction" {
                    text.split('/').collect()
                } else {
                    vec![text]
                };
                let parsed: Option<Vec<i64>> = values.iter().map(|v| v.parse().ok()).collect();
                let Some(parsed) = parsed else {
                    return Err(error("NUMBER_INVALID"));
                };
                if a["format"] == "fraction" && (parsed.len() != 2 || parsed[1] == 0) {
                    return Err(error("NUMBER_INVALID"));
                }
                Ok(json!(parsed
                    .get(a["index"].as_u64().unwrap_or(0) as usize)
                    .ok_or_else(|| error("NUMBER_INVALID"))?))
            }
            "find" | "wait_find" | "tap_template" => {
                let result = self.visible(template, &mut s);
                if !result.is_null() && (name == "tap_template" || a["click"] == true) {
                    self.tap(template, &mut s)?;
                }
                Ok(result)
            }
            "find_any" => {
                let templates = a["templates"].as_array().unwrap();
                let first = templates[0].as_str().unwrap();
                let index = if first.contains("dungeon_echo_title") {
                    if s.layout == "echo" {
                        0
                    } else {
                        1
                    }
                } else if first.contains("cloud_disconnected") && s.screen == "battle" {
                    1
                } else {
                    return Err(error("MODEL_FIND_ANY"));
                };
                let mut value = hit(templates[index].as_str().unwrap(), 0.08, 0.06);
                value["index"] = json!(index);
                Ok(value)
            }
            "find_text" => {
                if a["region"][1].as_f64().unwrap() > 0.65 {
                    s.weekly_reads += 1;
                    let text = match self.case.fault {
                        "weekly_empty" | "weekly_low" => return Ok(Value::Null),
                        "weekly_unstable" => {
                            if s.weekly_reads % 2 == 0 {
                                "0/3".into()
                            } else {
                                "3/3".into()
                            }
                        }
                        "weekly_overflow" => "4/3".into(),
                        "weekly_malformed" => "/3".into(),
                        _ => format!("{}/3", s.weekly),
                    };
                    Ok(json!({"text":text,"confidence":0.99}))
                } else {
                    Ok(json!({"text":format!("{}/300",s.power),"confidence":0.99}))
                }
            }
            "ocr" => {
                let n = if a["mode"] == "line" {
                    cost(&s) * s.selected
                } else if a["region"][0].as_f64().unwrap() > 0.90 {
                    if self.case.fault == "max_overstates_weekly" {
                        3
                    } else {
                        maximum(&s)
                    }
                } else {
                    s.selected
                };
                Ok(json!({"text":n.to_string(),"confidence":0.99}))
            }
            "tap" => {
                if let Some(template) = a["position"]["template"].as_str() {
                    self.tap(template, &mut s)?;
                } else {
                    let p = a["position"]
                        .as_array()
                        .ok_or_else(|| error("MODEL_RAW_TAP"))?;
                    if s.screen != "guide"
                        || (p[0].as_f64().unwrap() - 0.2515625).abs() > 1e-6
                        || (p[1].as_f64().unwrap() - 0.1953703704).abs() > 1e-6
                    {
                        return Err(error("MODEL_SURVIVAL_TAB_WITHOUT_GUIDE"));
                    }
                    if self.case.fault != "index_missing" {
                        s.screen = "index".into();
                    }
                }
                Ok(Value::Null)
            }
            "key" => {
                s.screen = "menu".into();
                Ok(Value::Null)
            }
            "swipe" => {
                if !matches!(s.screen.as_str(), "index" | "cavern_list") {
                    return Err(error("MODEL_SCROLL_BEFORE_INDEX"));
                }
                Ok(Value::Null)
            }
            "sleep" | "log" => Ok(Value::Null),
            "fail" => Err(error(format!(
                "SCRIPT_FAIL:{}",
                a["code"].as_str().unwrap_or("")
            ))),
            _ => Err(error(format!("MODEL_UNCONFIGURED:{name}"))),
        }
    }
}
impl EventSink for Host<'_> {
    fn emit(&self, event: Value) {
        self.state.lock().unwrap().events.push(event);
    }
}
pub fn run(args: &[String], hashes: Value) -> Result<(), Box<dyn std::error::Error>> {
    let option = |name| {
        args.iter()
            .position(|v| v == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let candidate = PathBuf::from(option("--candidate").ok_or("candidate missing")?);
    let report = PathBuf::from(option("--report").ok_or("report missing")?);
    let source = fs::read_to_string(&candidate)?;
    let sha = format!("{:x}", Sha256::digest(source.as_bytes()));
    let script = syntax::parse_script(&source).map_err(|e| format!("syntax:{e:?}"))?;
    let definition = &script
        .functions
        .iter()
        .find(|(n, _)| n == "drain_stamina")
        .ok_or("missing drain")?
        .1;
    let wire = syntax::build_function_program(
        "drain_stamina",
        definition,
        &script.functions,
        script.vars.iter().cloned().collect(),
        0,
    );
    let program: yaml_interp::Program = serde_json::from_value(wire)?;
    let mut cases = vec![
        ("echo-three-then-cavern", 300, 3, "", 2, 1, 10, None),
        ("already-exhausted-then-cavern", 210, 0, "", 1, 1, 10, None),
        ("partial-weekly-two-then-cavern", 100, 2, "", 2, 1, 0, None),
        ("one-weekly-exact-power", 30, 1, "", 1, 0, 0, None),
        ("exhausted-with-zero-power", 0, 0, "", 0, 0, 0, None),
        ("exhausted-low-power", 20, 0, "", 0, 1, 20, None),
        (
            "weekly-positive-insufficient-power",
            20,
            3,
            "",
            0,
            0,
            20,
            None,
        ),
        (
            "cap-even-if-counter-overstates-weekly",
            100,
            1,
            "max_overstates_weekly",
            2,
            1,
            30,
            None,
        ),
        (
            "missing-weekly-label",
            210,
            0,
            "weekly_label_missing",
            0,
            0,
            210,
            Some("WEEKLY_ATTEMPTS_UNCONFIRMED"),
        ),
        (
            "empty-weekly-is-not-zero",
            210,
            0,
            "weekly_empty",
            0,
            0,
            210,
            Some("WEEKLY_ATTEMPTS_UNCONFIRMED"),
        ),
        (
            "low-weekly-is-not-zero",
            210,
            0,
            "weekly_low",
            0,
            0,
            210,
            Some("WEEKLY_ATTEMPTS_UNCONFIRMED"),
        ),
        (
            "unstable-weekly-is-not-zero",
            210,
            0,
            "weekly_unstable",
            0,
            0,
            210,
            Some("WEEKLY_ATTEMPTS_UNCONFIRMED"),
        ),
        (
            "remaining-exceeds-maximum",
            210,
            0,
            "weekly_overflow",
            0,
            0,
            210,
            Some("WEEKLY_ATTEMPTS_UNCONFIRMED"),
        ),
        (
            "malformed-weekly-fraction",
            210,
            0,
            "weekly_malformed",
            0,
            0,
            210,
            Some("NUMBER_INVALID"),
        ),
        (
            "close-failed-no-navigation",
            210,
            0,
            "close_missing",
            0,
            0,
            210,
            Some("CHALLENGE_CLOSE_FAIL"),
        ),
        (
            "guide-failed-no-tab-click",
            210,
            0,
            "guide_missing",
            0,
            0,
            210,
            Some("GUIDE_ICON_NOT_FOUND"),
        ),
        (
            "index-failed-no-scrolling",
            210,
            0,
            "index_missing",
            0,
            0,
            210,
            Some("DEFAULT_INDEX_TIMEOUT"),
        ),
        (
            "wrong-default-no-retry-loop",
            210,
            0,
            "wrong_default",
            0,
            1,
            210,
            Some("DEFAULT_CAVERN_LAYOUT_UNCONFIRMED"),
        ),
        (
            "default-challenge-not-ready",
            210,
            0,
            "default_challenge_missing",
            0,
            1,
            210,
            Some("DEFAULT_CHALLENGE_TIMEOUT"),
        ),
        (
            "start-not-confirmed",
            300,
            3,
            "start_missing",
            0,
            0,
            300,
            Some("BATTLE_START_TIMEOUT"),
        ),
        (
            "battle-exit-not-confirmed",
            300,
            3,
            "exit_missing",
            0,
            0,
            300,
            Some("EXIT_LEVEL_TIMEOUT"),
        ),
    ]
    .into_iter()
    .map(
        |(name, power, weekly, fault, battles, switches, final_power, error)| Case {
            name,
            layout: "echo",
            power,
            weekly,
            fault,
            battles,
            switches,
            final_power,
            error,
        },
    )
    .collect::<Vec<_>>();
    cases.push(Case {
        name: "ordinary-cavern",
        layout: "cavern",
        power: 210,
        weekly: 0,
        fault: "",
        battles: 1,
        switches: 0,
        final_power: 10,
        error: None,
    });
    cases.push(Case {
        name: "ordinary-missing-cost-not-weekly",
        layout: "cavern",
        power: 210,
        weekly: 0,
        fault: "cost_missing",
        battles: 0,
        switches: 0,
        final_power: 210,
        error: Some("COST_ICON_NOT_FOUND"),
    });
    let mut results = Vec::new();
    for case in &cases {
        let host = Host {
            case,
            state: Mutex::new(State {
                screen: "dungeon".into(),
                layout: case.layout.into(),
                power: case.power,
                weekly: case.weekly,
                selected: 1,
                ..Default::default()
            }),
        };
        let outcome = yaml_interp::run(&program, &host, Some(&host));
        let s = host.state.into_inner().unwrap();
        let expected_outcome = match case.error {
            Some(code) => outcome.as_ref().err().is_some_and(|e| e.contains(code)),
            None => outcome
                .as_ref()
                .is_ok_and(|v| v["status"] == "progress" || v["status"] == "insufficient"),
        };
        let checks = json!({"expected_outcome":expected_outcome,"expected_batches":s.battles==case.battles,
            "expected_switches":s.switches==case.switches,"expected_power":s.power==case.final_power,
            "no_cost_read_after_weekly_exhaustion":s.cost_reads_while_exhausted==0,
            "no_invalid_input":s.violations.is_empty()});
        let passed = checks.as_object().unwrap().values().all(|v| v == true);
        let item = json!({"name":case.name,"passed":passed,"checks":checks,"result":outcome.as_ref().ok(),
            "error":outcome.as_ref().err(),"batches":s.battles,"switches":s.switches,"final_power":s.power,
            "native_calls":s.calls,"events":s.events});
        println!(
            "{}",
            json!({"name":case.name,"passed":passed,"error":item["error"]})
        );
        results.push(item);
    }
    let passed = results.iter().all(|v| v["passed"] == true);
    let summary = json!({"passed":passed,"mode":"weekly-boundary","candidate_sha256":sha,
        "case_count":results.len(),"cases":results,"source_hashes":hashes,
        "scope":"Authoritative parser/interpreter and independent stateful mocked game. All candidate helpers, drain loop, battle controls and fallback navigation are interpreted without replacements. OCR/NCC and game responses are simulated.",
        "production_accessed":false,"device_inputs":0,"listeners_started":0});
    fs::write(report, serde_json::to_vec_pretty(&summary)?)?;
    if !passed {
        return Err("weekly boundary cases failed".into());
    }
    Ok(())
}
