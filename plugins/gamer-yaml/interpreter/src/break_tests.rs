use super::*;
use serde_json::json;
use std::sync::Mutex;

#[derive(Default)]
struct Host(Mutex<Vec<String>>);
impl HostFunctions for Host {
    fn invoke(&self, name: &str, args: Value) -> Result<Value, HostError> {
        if name == "find_any" {
            return Ok(json!({"index":0}));
        }
        self.0
            .lock()
            .unwrap()
            .push(args.as_str().unwrap_or(name).to_string());
        Ok(Value::Null)
    }
    fn cancelled(&self) -> bool {
        false
    }
}
fn log(s: &str) -> Value {
    json!({"op":"fn","fn":"log","args":{"expr":"lit","value":s},"path":s})
}
fn brk() -> Value {
    json!({"op":"break","path":"break"})
}
fn repeat(n: u64, body: Vec<Value>) -> Value {
    json!({"op":"repeat","times":{"expr":"lit","value":n},"do":body,"path":"repeat"})
}

#[test]
fn break_exits_nearest_loop_through_if_and_template_branch() {
    let branch = json!({"op":"match_templates","args":{"expr":"lit","value":{}},"cases":[{"as":"hit","do":[brk()]}],"path":"match"});
    let conditional =
        json!({"op":"if","cond":{"expr":"lit","value":true},"then":[branch],"path":"if"});
    let program: Program = serde_json::from_value(json!({"vars":{"hit":"original"}, "run":[
        repeat(2, vec![repeat(5, vec![log("inner"), conditional, log("skipped")]), log("outer")]),
        repeat(0, vec![log("zero"),brk()]),
        {"op":"return","value":{"expr":"ref","path":"hit"},"path":"return"}, log("after-return")
    ]}))
    .unwrap();
    let host = Host::default();
    let result = run(&program, &host, None).unwrap();
    assert_eq!(result, json!("original"));
    assert_eq!(
        *host.0.lock().unwrap(),
        ["inner", "outer", "inner", "outer"]
    );
}

#[test]
fn break_cannot_escape_function_boundary_or_run_at_top_level() {
    for program in [
        json!({"run":[brk(),log("skipped")]}),
        json!({"functions":{"helper":{"run":[brk()]}},"run":[repeat(2,vec![json!({"op":"fn","fn":"helper","path":"call"})]),log("skipped")]}),
    ] {
        let program: Program = serde_json::from_value(program).unwrap();
        let host = Host::default();
        assert!(run(&program, &host, None)
            .unwrap_err()
            .contains("yaml.break.outside_loop"));
        assert!(host.0.lock().unwrap().is_empty());
    }
}
