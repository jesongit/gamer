//! script_v2 契约 fixture 断言（阶段 0 测试迁移自 tests/script_v2_contract/，
//! 改为调用 src/script_v2 正式装载器）。
//!
//! 放在 src 内的原因：本 crate 只有 bin 目标（无 lib），集成测试无法导入
//! src 模块（沿用 src/engine/fixtures_tests.rs 先例）。yaml_loader/model/
//! precheck 三份测试本地实现已删除，全部走 parse_script_file/
//! parse_function_file（规范序列化往返断言见 serialize 往返用例）。
//!
//! 契约文档：docs/SCRIPT_EDITOR_CONTRACT.md；fixture 索引：
//! tests/fixtures/script_v2/README.md（web/src/script-editor/__fixtures__/
//! 存在逐字节一致副本，由前端测试守护）。

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use super::error::ScriptError;
use super::validate::InMemoryResources;
use super::{parse_function_file, parse_script_file, serialize_function_file, serialize_script};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/script_v2")
}

fn read_fixture(name: &str) -> String {
    fs::read_to_string(fixtures_dir().join(name))
        .unwrap_or_else(|e| panic!("读取 fixture {name} 失败: {e}"))
}

/// 合法样例逻辑 ID（多文件样例的辅助文件与 task_snapshot 在 golden.json 内描述）。
const VALID_IDS: &[&str] = &[
    "v01_minimal_script",
    "v02_all_actions",
    "v03_function_library",
    "v04_params_all_defaults",
    "v05_params_all_required",
    "v06_nested_if_loop",
    "v07_match_compact",
    "v08_color_branch",
    "v09_call_script",
    "v10_func_call_cross_file",
    "v11_record_output",
    "v12_task_args_snapshot",
];

/// 非法样例逻辑 ID。
const INVALID_IDS: &[&str] = &[
    "i01_old_top_format",
    "i02_params_unquoted",
    "i03_default_type_mismatch",
    "i04_match_candidate_duplicate",
    "i05_func_path_traversal",
    "i06_call_cycle",
    "i07_unknown_top_key",
    "i08_else_in_candidates",
    "i09_empty_default",
];

/// 测试资源：call 目标脚本 / 函数库 common / 全部被引用模板短名。
fn fixture_provider() -> InMemoryResources {
    let mut p = InMemoryResources::new();
    p.add_script(
        "v09_call_script.target.yaml",
        read_fixture("v09_call_script.target.yaml"),
    );
    p.add_function_file(
        "common",
        read_fixture("v10_func_call_cross_file.common.yaml"),
    );
    for tpl in [
        "account.png",
        "retry.png",
        "popup.png",
        "dialog.png",
        "test1.png",
        "test2.png",
        "record_click_20260829_001.png",
        "record_swipe_20260829_002.png",
        "icon.png",
        // 非法 fixture 走到语义校验时引用的模板（i04 重复候选）。
        "dup.png",
    ] {
        p.add_template(tpl);
    }
    p
}

/// 函数库文件的短路径：`<主 ID>.<短路径>.yaml` → `<短路径>`；无前缀 → 主 ID 本身。
fn func_file_short(id: &str, filename: &str) -> String {
    let stem = filename.strip_suffix(".yaml").expect("文件名以 .yaml 结尾");
    let prefix = format!("{id}.");
    match stem.strip_prefix(&prefix) {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => id.to_string(),
    }
}

/// golden 合法样例：严格装载成功 + AST 序列化出的 Model JSON 与 golden 逐字段相等。
#[test]
fn golden_valid_fixtures_match_model() {
    let provider = fixture_provider();
    for id in VALID_IDS {
        let golden: Value = serde_json::from_str(&read_fixture(&format!("{id}.golden.json")))
            .unwrap_or_else(|e| panic!("{id}: golden.json 不是合法 JSON: {e}"));
        assert_eq!(golden["id"], *id, "{id}: golden.id 不一致");
        assert_eq!(golden["kind"], "valid", "{id}: golden.kind 应为 valid");

        let files = golden["files"].as_array().expect("golden.files 数组");
        assert!(!files.is_empty(), "{id}: golden.files 不能为空");
        for entry in files {
            let file = entry["file"].as_str().expect("files[].file");
            let source = read_fixture(file);
            let model = match entry["model_kind"].as_str().expect("model_kind") {
                "script" => {
                    let sf = parse_script_file(&source, id, &provider)
                        .unwrap_or_else(|e| panic!("{id}/{file}: 装载失败: {e:?}"));
                    serde_json::to_value(&sf).expect("ScriptFile 序列化")
                }
                "function_library" => {
                    let short = func_file_short(id, file);
                    let ff = parse_function_file(&source, &short, &provider)
                        .unwrap_or_else(|e| panic!("{id}/{file}: 装载失败: {e:?}"));
                    json!({
                        "file": short,
                        "functions": serde_json::to_value(&ff.functions).expect("functions 序列化"),
                    })
                }
                other => panic!("{id}/{file}: 未知 model_kind {other}"),
            };
            assert_eq!(
                &model, &entry["model"],
                "{id}/{file}: 模型与 golden 不一致\n  实际: {model}"
            );
        }
    }
}

/// 非法样例：严格装载以期望的 code/step_path/field 结构化拒绝（错误集精确相等）。
#[test]
fn invalid_fixtures_are_rejected_with_structured_errors() {
    let provider = fixture_provider();
    for id in INVALID_IDS {
        let expected: Value = serde_json::from_str(&read_fixture(&format!("{id}.expected.json")))
            .unwrap_or_else(|e| panic!("{id}: expected.json 不是合法 JSON: {e}"));
        assert_eq!(
            expected["kind"], "invalid",
            "{id}: expected.kind 应为 invalid"
        );

        let source = read_fixture(&format!("{id}.yaml"));
        let actual = parse_script_file(&source, id, &provider)
            .err()
            .unwrap_or_else(|| panic!("{id}: 非法样例必须被拒绝，但装载成功"));
        assert!(!actual.is_empty(), "{id}: 非法样例必须产生至少一条错误");

        let mut actual_keys: Vec<(String, String, String)> = actual
            .iter()
            .map(|e: &ScriptError| {
                (
                    e.code.clone(),
                    e.step_path_str().to_string(),
                    e.field_str().to_string(),
                )
            })
            .collect();
        actual_keys.sort();
        let mut expected_keys: Vec<(String, String, String)> = expected["errors"]
            .as_array()
            .expect("errors 数组")
            .iter()
            .map(|e| {
                (
                    e["code"].as_str().expect("code").to_string(),
                    e["step_path"].as_str().expect("step_path").to_string(),
                    e["field"].as_str().expect("field").to_string(),
                )
            })
            .collect();
        expected_keys.sort();
        assert_eq!(
            actual_keys, expected_keys,
            "{id}: 错误集与期望不一致\n  实际: {actual:?}"
        );
        for e in &actual {
            assert!(!e.code.is_empty(), "{id}: 错误 code 不能为空");
            assert!(!e.message.is_empty(), "{id}: 错误 message 不能为空");
            assert_eq!(
                e.resource,
                id.to_string(),
                "{id}: 错误 resource 应为用例 ID"
            );
        }
    }
}

/// 规范序列化往返：serialize(parse(fixture)) 与原文逐字节一致（含结尾单换行）、
/// parse(serialize(parse)) 深等、二次序列化幂等（CONTRACT §3「规范 YAML」列：
/// fixture 即规范形态，序列化器必须原样回写）。
#[test]
fn golden_fixtures_serialize_roundtrip() {
    let provider = fixture_provider();
    for id in VALID_IDS {
        let golden: Value = serde_json::from_str(&read_fixture(&format!("{id}.golden.json")))
            .unwrap_or_else(|e| panic!("{id}: golden.json 不是合法 JSON: {e}"));
        for entry in golden["files"].as_array().expect("golden.files 数组") {
            let file = entry["file"].as_str().expect("files[].file");
            let source = read_fixture(file);
            match entry["model_kind"].as_str().expect("model_kind") {
                "script" => {
                    let sf = parse_script_file(&source, id, &provider)
                        .unwrap_or_else(|e| panic!("{id}/{file}: 装载失败: {e:?}"));
                    let once = serialize_script(&sf);
                    assert_eq!(once, source, "{id}/{file}: 序列化与原文不一致");
                    let twice = parse_script_file(&once, id, &provider)
                        .unwrap_or_else(|e| panic!("{id}/{file}: 二次装载失败: {e:?}\n{once}"));
                    assert_eq!(sf, twice, "{id}/{file}: parse(serialize(parse)) 不深等");
                    assert_eq!(
                        serialize_script(&twice),
                        once,
                        "{id}/{file}: 二次序列化不幂等"
                    );
                }
                "function_library" => {
                    let short = func_file_short(id, file);
                    let ff = parse_function_file(&source, &short, &provider)
                        .unwrap_or_else(|e| panic!("{id}/{file}: 装载失败: {e:?}"));
                    let once = serialize_function_file(&ff);
                    assert_eq!(once, source, "{id}/{file}: 序列化与原文不一致");
                    let twice = parse_function_file(&once, &short, &provider)
                        .unwrap_or_else(|e| panic!("{id}/{file}: 二次装载失败: {e:?}\n{once}"));
                    assert_eq!(ff, twice, "{id}/{file}: parse(serialize(parse)) 不深等");
                    assert_eq!(
                        serialize_function_file(&twice),
                        once,
                        "{id}/{file}: 二次序列化不幂等"
                    );
                }
                other => panic!("{id}/{file}: 未知 model_kind {other}"),
            }
        }
    }
}

/// v12 专属：定时任务参数快照形态断言（args 全量类型化 + param_signature 可复算）。
#[test]
fn task_args_snapshot_shape() {
    let id = "v12_task_args_snapshot";
    let golden: Value =
        serde_json::from_str(&read_fixture(&format!("{id}.golden.json"))).expect("golden JSON");
    let snapshot = &golden["task_snapshot"];
    let provider = fixture_provider();

    let source = read_fixture(&format!("{id}.yaml"));
    let sf = parse_script_file(&source, id, &provider).expect("装载");

    // 1) param_signature 由 AST 参数表复算，与快照中冻结值一致。
    let recomputed = super::param_signature(&sf.params);
    assert_eq!(
        recomputed,
        snapshot["param_signature"]
            .as_str()
            .expect("param_signature"),
        "{id}: param_signature 复算不一致"
    );

    // 2) args 快照：键集 = 参数名集，值为类型化默认值（本样例全部继承声明默认值）。
    let args = snapshot["args"].as_object().expect("args 对象");
    assert_eq!(args.len(), sf.params.len(), "{id}: args 键数与参数数不一致");
    for p in &sf.params {
        let expected = match &p.default {
            Some(v) => serde_json::to_value(v).expect("TypedValue 序列化"),
            None => Value::Null,
        };
        assert_eq!(
            args.get(&p.name)
                .unwrap_or_else(|| panic!("{id}: args 缺少 {}", p.name)),
            &expected,
            "{id}: args[{}] 与声明默认值不一致",
            p.name
        );
    }
    // 3) 快照形态固定字段。
    assert!(snapshot["script_id"].is_string(), "script_id 必须是字符串");
    assert!(snapshot["device_id"].is_string(), "device_id 必须是字符串");
    assert!(snapshot["cron"].is_string(), "cron 必须是字符串");
}

/// 反向证明选型动机：serde_yaml 0.9 把单引号与无引号标量反序列化成同一个 Value，
/// 书写样式彻底丢失 —— params「整条单引号」契约只能靠事件级解析层校验。
#[test]
fn serde_yaml_loses_scalar_style() {
    let quoted: serde_yaml::Value = serde_yaml::from_str("- 'bool:enable:开关:true'\n").unwrap();
    let plain: serde_yaml::Value = serde_yaml::from_str("- bool:enable:开关:true\n").unwrap();
    assert_eq!(
        quoted, plain,
        "serde_yaml 0.9 应丢失标量样式（两者不可区分）"
    );
}

/// 装载失败的辅助断言：返回首个匹配 code 的错误（不存在则 panic）。
#[allow(dead_code)]
fn first_error<'a>(errors: &'a [ScriptError], code: &str) -> &'a ScriptError {
    errors
        .iter()
        .find(|e| e.code == code)
        .unwrap_or_else(|| panic!("缺少错误 {code}，实际: {errors:?}"))
}
