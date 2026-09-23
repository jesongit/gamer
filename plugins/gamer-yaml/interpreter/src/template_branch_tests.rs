use super::*;
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

struct Host {
    result: Value,
    calls: Mutex<Vec<(String, Value)>>,
    cancel_after_match: bool,
    cancelled: AtomicBool,
    fail: bool,
}
impl Host {
    fn new(result: Value) -> Self {
        Self {
            result,
            calls: Mutex::new(Vec::new()),
            cancel_after_match: false,
            cancelled: AtomicBool::new(false),
            fail: false,
        }
    }
}
impl HostFunctions for Host {
    fn invoke(&self, name: &str, args: Value) -> Result<Value, HostError> {
        self.calls.lock().unwrap().push((name.into(), args));
        if self.fail {
            return Err(HostError::new(HostErrorKind::Failed, "fixture failure"));
        }
        if name == "find_any" {
            self.cancelled
                .store(self.cancel_after_match, Ordering::Relaxed);
            Ok(self.result.clone())
        } else {
            Ok(Value::Null)
        }
    }
    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

fn program() -> Program {
    serde_json::from_value(json!({
        "vars":{"hit":"outer"},
        "run":[{
            "op":"match_templates", "path":"run[0]", "args":{"expr":"lit","value":{"templates":["a","b"]}},
            "cases":[
                {"do":[{"op":"return","path":"run[0].cases[0].do[0]","value":{"expr":"lit","value":"first"}}]},
                {"as":"hit", "do":[{"op":"fn","fn":"log","path":"run[0].cases[1].do[0]","args":{"expr":"ref","path":"hit.template"}}]}
            ],
            "else":[{"op":"return","path":"run[0].else[0]","value":{"expr":"lit","value":"miss"}}]
        }, {"op":"return","path":"run[1]","value":{"expr":"ref","path":"hit"}}]
    })).unwrap()
}

#[test]
fn first_branch_only_local_binding_and_return_propagate() {
    let host = Host::new(json!({"index":1,"template":"b"}));
    assert_eq!(run(&program(), &host, None).unwrap(), "outer");
    assert_eq!(
        *host.calls.lock().unwrap(),
        vec![
            ("find_any".into(), json!({"templates":["a","b"]})),
            ("log".into(), json!("b"))
        ]
    );
    let host = Host::new(json!({"index":0,"template":"a"}));
    assert_eq!(run(&program(), &host, None).unwrap(), "first");
    assert_eq!(host.calls.lock().unwrap().len(), 1);
    assert_eq!(
        run(&program(), &Host::new(Value::Null), None).unwrap(),
        "miss"
    );
    let mut scoped = program();
    let StepKind::MatchTemplates { cases, .. } = &mut scoped.run[0].kind else {
        panic!()
    };
    cases[1].save_as = Some("__return".into());
    cases[1].body = vec![Step {
        path: "run[0].cases[1].do[0]".into(),
        desc: String::new(),
        kind: StepKind::Return {
            value: Expr::Ref {
                path: "__return.template".into(),
            },
        },
    }];
    assert_eq!(
        run(&scoped, &Host::new(json!({"index":1,"template":"b"})), None).unwrap(),
        "b"
    );
}

#[test]
fn branch_errors_cancellation_and_invalid_host_results_do_not_fall_through() {
    let mut host = Host::new(json!({"index":1,"template":"b"}));
    host.cancel_after_match = true;
    assert!(run(&program(), &host, None)
        .unwrap_err()
        .starts_with("CANCELLED"));
    assert_eq!(host.calls.lock().unwrap().len(), 1);
    let mut host = Host::new(Value::Null);
    host.fail = true;
    assert!(run(&program(), &host, None)
        .unwrap_err()
        .contains("fixture failure"));
    assert_eq!(host.calls.lock().unwrap().len(), 1);
    for result in [json!({}), json!({"index":-1}), json!({"index":12})] {
        assert!(run(&program(), &Host::new(result), None).is_err());
    }
}

#[test]
fn nested_actions_keep_event_paths_and_step_budget() {
    #[derive(Default)]
    struct Events(Mutex<Vec<Value>>);
    impl EventSink for Events {
        fn emit(&self, e: Value) {
            self.0.lock().unwrap().push(e);
        }
    }
    let events = Events::default();
    let mut program = program();
    let StepKind::MatchTemplates { cases, .. } = &mut program.run[0].kind else {
        panic!()
    };
    cases[1].body = vec![Step {
        path: "run[0].cases[1].do[0]".into(),
        desc: "循环".into(),
        kind: StepKind::Repeat {
            times: Expr::Lit {
                value: json!(MAX_STEPS),
            },
            body: vec![],
        },
    }];
    let error = run(&program, &Host::new(json!({"index":1})), Some(&events)).unwrap_err();
    assert!(error.starts_with("STEP_BUDGET_EXCEEDED"));
    let events = events.0.lock().unwrap();
    assert!(events.iter().any(|e| e["ev"] == "step_end"
        && e["path"] == "run[0].cases[1].do[0]"
        && e["ok"] == false));
    assert!(events.iter().any(|e| e["ev"] == "budget"));
}
