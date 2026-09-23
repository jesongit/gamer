// 同一批 YAML 夹具由前端编辑器、REST 验收和真实 WASM 解释器共同消费。
const ACCEPTANCE_LIBRARY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tools/yaml-tests/_function.yaml"
));
const ACCEPTANCE_FLOW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tools/yaml-tests/flow.yaml"
));
const ACCEPTANCE_NATIVE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tools/yaml-tests/native.yaml"
));

fn acceptance_program(source: &str) -> Value {
    let script = parse_script(source).unwrap();
    let library = parse_function_library(ACCEPTANCE_LIBRARY).unwrap();
    let bound = crate::extensions::gamer_yaml::task_params::bind_entry_args(
        "qa/flow.yaml",
        &script.params,
        &Default::default(),
        true,
    )
    .unwrap();
    let mut values: serde_json::Map<String, Value> = script.vars.iter().cloned().collect();
    values.extend(bound.resolved);
    build_program(&script, &library, values, 0)
}

#[test]
fn yaml_acceptance_function_names_match_frontend() {
    use crate::extensions::gamer_yaml::syntax::is_function_name;
    let cases: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tools/yaml-tests/function-names.json"
    )))
    .unwrap();
    for (group, expected) in [("valid", true), ("invalid", false)] {
        for name in cases[group].as_array().unwrap() {
            let name = name.as_str().unwrap();
            assert_eq!(is_function_name(name), expected, "{name:?}");
            let key = serde_json::to_string(name).unwrap();
            assert_eq!(
                parse_function_library(&format!("functions:\n  {key}:\n    run: []\n")).is_ok(),
                expected,
                "definition {name:?}"
            );
            assert_eq!(
                parse_script(&format!("run:\n  - {key}: {{}}\n")).is_ok(),
                expected,
                "call {name:?}"
            );
        }
    }
    for name in ["if", "repeat", "return"] {
        assert!(parse_function_library(&format!("functions:\n  {name}:\n    run: []\n")).is_err());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn yaml_acceptance_chinese_function_runs_real_wasm() {
    let trace = Arc::new(tests::Trace::default());
    let vision = tests::VisionStub::new(FrameSize::new(1000, 1000));
    let host = tests::vision_host(trace, &vision, tests::LogTrace::new(), &[]);
    let program = acceptance_program(
        "run:\n  - 每日任务跳转: {value: 已跳转}\n    as: result\n  - return: $result\n",
    );
    let result = LazyYamlWasmtimeRuntime::new()
        .run(run_request(
            program,
            host,
            Arc::new(AtomicBool::new(false)),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(result.value, json!("已跳转"));
}

#[test]
fn yaml_acceptance_export_native_catalog() {
    let catalog: Vec<Value> = crate::extensions::gamer_yaml::native_funcs::native_functions()
        .iter()
        .map(crate::extensions::gamer_yaml::native_funcs::native_schema_json)
        .collect();
    let frontend_catalog: Vec<Value> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tools/yaml-tests/native-functions.json"
    )))
    .unwrap();
    assert_eq!(
        catalog, frontend_catalog,
        "前端验收 Schema 必须与服务端注册表同步"
    );
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/yaml-acceptance/native-functions.json");
    fs::create_dir_all(output.parent().unwrap()).unwrap();
    fs::write(output, serde_json::to_vec_pretty(&catalog).unwrap()).unwrap();
    let script = parse_script(ACCEPTANCE_NATIVE).unwrap();
    let called: std::collections::BTreeSet<_> = script
        .run
        .iter()
        .filter_map(|step| match step {
            crate::extensions::gamer_yaml::syntax::SurfaceStep::Call { name, .. } => {
                Some(name.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        called,
        crate::extensions::gamer_yaml::native_funcs::native_names(),
        "每个正式原生函数必须有执行夹具"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn yaml_acceptance_flow_runs_real_wasm() {
    let trace = Arc::new(tests::Trace::default());
    let vision = tests::VisionStub::new(FrameSize::new(1000, 1000));
    let logs = tests::LogTrace::new();
    let host = tests::vision_host(trace, &vision, logs.clone(), &["device.read", "log.write"]);
    let result = LazyYamlWasmtimeRuntime::new()
        .run(run_request(
            acceptance_program(ACCEPTANCE_FLOW),
            host,
            Arc::new(AtomicBool::new(false)),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        result.value,
        json!({
            "echoed": {"mode": "safe", "count": 0}, "early": "done", "scope": "caller",
            "composite": {"point": [0.25, 0.75], "items": ["hello", false, null], "escaped": "$price"}
        })
    );
    assert_eq!(
        logs.messages(),
        ["cycle", "cycle", "disabled", "zero-is-truthy"]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn yaml_acceptance_all_native_functions_run_real_wasm() {
    let trace = Arc::new(tests::Trace::default());
    let vision = tests::VisionStub::new(FrameSize::new(1000, 1000));
    for outcome in [
        tests::stub_outcome(),
        MatchOutcome::NotFound,
        tests::stub_outcome(),
        tests::stub_outcome(),
        tests::stub_outcome(),
        MatchOutcome::NotFound,
    ] {
        vision.push_outcome(outcome);
    }
    let logs = tests::LogTrace::new();
    let sink = tests::EventCollect::new();
    let host = tests::vision_host(
        trace.clone(),
        &vision,
        logs.clone(),
        &[
            "device.read",
            "device.app",
            "input.tap",
            "input.swipe",
            "input.key",
            "input.text",
            "vision.match",
            "resource.read",
            "runtime.sleep",
            "log.write",
        ],
    );
    let result = LazyYamlWasmtimeRuntime::new()
        .run(run_request(
            acceptance_program(ACCEPTANCE_NATIVE),
            host,
            Arc::new(AtomicBool::new(false)),
            Some(sink.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(
        result.value,
        json!({
            "center": {"x": 0.11, "y": 0.07}, "appeared": {"x": 0.11, "y": 0.07},
            "clicked": {"x": 0.11, "y": 0.07}, "gone": true,
            "comparisons": [true, true, true, true, true, true]
        })
    );
    assert_eq!(
        *trace.taps.lock().unwrap(),
        [[250, 750], [110, 70], [110, 70]]
    );
    assert_eq!(*trace.text.lock().unwrap(), ["yaml-acceptance"]);
    assert_eq!(
        *trace.apps.lock().unwrap(),
        [
            ("launch".into(), "+com.example.game".into()),
            ("stop_app".into(), "com.example.game".into())
        ]
    );
    let swipe = trace.swipes.lock().unwrap()[0];
    assert_eq!(
        [
            swipe.start().x(),
            swipe.start().y(),
            swipe.end().x(),
            swipe.end().y()
        ],
        [100, 900, 900, 100]
    );
    assert_eq!(swipe.duration().as_millis(), 1);
    assert_eq!(trace.keys.lock().unwrap()[0].code().value(), 3);
    assert_eq!(
        vision
            .match_calls
            .load(std::sync::atomic::Ordering::Relaxed),
        7
    );
    assert_eq!(
        serde_json::from_str::<Value>(&logs.messages()[0]).unwrap(),
        json!({"status": "ready", "literal": "$price"})
    );
    assert_eq!(sink.of("run_end").len(), 1);
    assert_eq!(sink.of("run_end")[0]["ok"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn yaml_acceptance_template_branches_run_real_wasm() {
    let source = "run:\n  - match_templates:\n      cases:\n        - template: first.png\n          do:\n            - return: first\n        - template: second.png\n          as: hit\n          do:\n            - tap: $hit.center\n            - return: $hit.template\n      else:\n        - return: missing\n  - return: should-not-run\n";
    for matched in [false, true] {
        let trace = Arc::new(tests::Trace::default());
        let vision = tests::VisionStub::new(FrameSize::new(1000, 1000));
        vision.push_outcome(MatchOutcome::NotFound);
        if matched {
            vision.push_outcome(tests::stub_outcome());
        }
        let host = tests::vision_host(
            trace.clone(),
            &vision,
            tests::LogTrace::new(),
            &["vision.match", "resource.read", "input.tap"],
        );
        let sink = tests::EventCollect::new();
        let result = LazyYamlWasmtimeRuntime::new()
            .run(run_request(
                acceptance_program(source),
                host,
                Arc::new(AtomicBool::new(false)),
                Some(sink.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(
            result.value,
            json!(if matched { "second.png" } else { "missing" })
        );
        assert_eq!(trace.taps.lock().unwrap().len(), usize::from(matched));
        let expected_path = if matched {
            "run[0].cases[1].do[0]"
        } else {
            "run[0].else[0]"
        };
        assert!(sink
            .of("step_start")
            .iter()
            .any(|e| e["path"] == expected_path));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn yaml_acceptance_break_runs_real_wasm() {
    let source = r#"run:
  - repeat: 2
    do:
      - repeat: 9
        do:
          - log: inner
          - if: true
            then:
              - break: {}
          - log: skipped
      - log: outer
  - repeat: 9
    do:
      - match_templates:
          cases:
            - template: ready.png
              do:
                - break: {}
          else:
            - break: {}
      - log: skipped
  - log: done
  - return: true
"#;
    let trace = Arc::new(tests::Trace::default());
    let vision = tests::VisionStub::new(FrameSize::new(1000, 1000));
    vision.push_outcome(tests::stub_outcome());
    let logs = tests::LogTrace::new();
    let sink = tests::EventCollect::new();
    let host = tests::vision_host(
        trace,
        &vision,
        logs.clone(),
        &["vision.match", "resource.read", "log.write"],
    );
    let result = LazyYamlWasmtimeRuntime::new()
        .run(run_request(
            acceptance_program(source),
            host,
            Arc::new(AtomicBool::new(false)),
            Some(sink.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(result.value, json!(true));
    assert_eq!(
        logs.messages(),
        ["inner", "outer", "inner", "outer", "done"]
    );
    assert!(sink
        .of("step_end")
        .iter()
        .any(|event| event["path"] == "run[1].do[0].cases[0].do[0]" && event["ok"] == true));
    assert_eq!(sink.of("run_end")[0]["ok"], true);
}
