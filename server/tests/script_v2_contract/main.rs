//! script_v2 契约测试（阶段 0）。
//!
//! 遍历 server/tests/fixtures/script_v2/ 的 golden fixture：
//! - 合法样例：用 saphyr-parser 事件层语法解析 → 构建拟议前端 Model JSON → 与 golden 断言；
//!   本阶段只做「语法 + 结构形态」断言，完整语义校验归阶段 2（plan §16.1）。
//! - 非法样例：断言其被最小预校验函数以期望的 code/step_path/field 结构化拒绝。
//! - PoC：证明选定解析层能取到标量书写样式（params「整条单引号」契约的可行前提）。
//!
//! 契约文档：docs/SCRIPT_EDITOR_CONTRACT.md；fixture 索引：fixtures/script_v2/README.md。

mod model;
mod precheck;
mod yaml_loader;

use precheck::{Diagnostic, ResourceKind};
use std::fs;
use std::path::PathBuf;

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

/// PoC：saphyr-parser 事件层能取到标量书写样式。
/// serde_yaml 0.9 反序列化后样式丢失，无法校验 params「整条单引号」契约；
/// 选型结论（saphyr-parser 0.0.12）与理由见 docs/SCRIPT_EDITOR_CONTRACT.md 第 2 节。
#[test]
fn poc_scalar_style_is_preserved_by_saphyr() {
    let src = "params:\n  - 'bool:enable:开关:true'\n  - bool:plain:备注:x\nsteps:\n  - log: hi\n";
    let root = yaml_loader::load(src).expect("解析失败");
    let params = root
        .get("params")
        .expect("缺少 params")
        .as_seq()
        .expect("params 必须是列表");
    assert_eq!(params[0].as_str(), Some("bool:enable:开关:true"));
    assert_eq!(
        params[0].scalar_style(),
        Some(saphyr_parser::ScalarStyle::SingleQuoted),
        "单引号样式必须可在事件层取到"
    );
    assert_eq!(
        params[1].scalar_style(),
        Some(saphyr_parser::ScalarStyle::Plain),
        "无引号 plain 样式必须与单引号可区分"
    );
}

/// golden 合法样例：语法解析 + 结构形态断言（Model JSON 与 golden 逐字段相等）。
#[test]
fn golden_valid_fixtures_match_model() {
    for id in VALID_IDS {
        let golden: serde_json::Value = serde_json::from_str(&read_fixture(&format!("{id}.golden.json")))
            .unwrap_or_else(|e| panic!("{id}: golden.json 不是合法 JSON: {e}"));
        assert_eq!(golden["id"], *id, "{id}: golden.id 不一致");
        assert_eq!(golden["kind"], "valid", "{id}: golden.kind 应为 valid");

        let files = golden["files"].as_array().expect("golden.files 数组");
        assert!(
            !files.is_empty(),
            "{id}: golden.files 不能为空"
        );
        for entry in files {
            let file = entry["file"].as_str().expect("files[].file");
            let source = read_fixture(file);
            let root = yaml_loader::load(&source)
                .unwrap_or_else(|e| panic!("{id}/{file}: YAML 解析失败: {e}"));
            let built = match entry["model_kind"].as_str().expect("model_kind") {
                "script" => model::build_script_model(&root),
                "function_library" => {
                    let short = func_file_short(id, file);
                    model::build_function_library_model(&root, &short)
                }
                other => panic!("{id}/{file}: 未知 model_kind {other}"),
            };
            let built = built.unwrap_or_else(|e| panic!("{id}/{file}: 模型构建失败: {e}"));
            assert_eq!(
                &built,
                &entry["model"],
                "{id}/{file}: 模型与 golden 不一致\n  实际: {built}"
            );
        }
    }
}

/// v12 专属：定时任务参数快照形态断言（args 全量类型化 + param_signature 可复算）。
#[test]
fn task_args_snapshot_shape() {
    let id = "v12_task_args_snapshot";
    let golden: serde_json::Value =
        serde_json::from_str(&read_fixture(&format!("{id}.golden.json"))).expect("golden JSON");
    let snapshot = &golden["task_snapshot"];

    let source = read_fixture(&format!("{id}.yaml"));
    let root = yaml_loader::load(&source).expect("YAML 解析失败");
    let built = model::build_script_model(&root).expect("模型构建");

    // 1) param_signature 由模型参数表复算，与快照中冻结值一致。
    let recomputed = model::param_signature(&built);
    assert_eq!(
        recomputed,
        snapshot["param_signature"].as_str().expect("param_signature"),
        "{id}: param_signature 复算不一致"
    );

    // 2) args 快照：键集 = 参数名集，值为类型化默认值（本样例全部继承声明默认值）。
    let args = snapshot["args"].as_object().expect("args 对象");
    let params = built["params"].as_array().expect("params 数组");
    assert_eq!(
        args.len(),
        params.len(),
        "{id}: args 键数与参数数不一致"
    );
    for p in params {
        let name = p["name"].as_str().expect("name");
        let default = &p["default"];
        assert_eq!(
            args.get(name).unwrap_or_else(|| panic!("{id}: args 缺少 {name}")),
            default,
            "{id}: args[{name}] 与声明默认值不一致"
        );
    }
    // 3) 快照形态固定字段。
    assert!(snapshot["script_id"].is_string(), "script_id 必须是字符串");
    assert!(snapshot["device_id"].is_string(), "device_id 必须是字符串");
    assert!(snapshot["cron"].is_string(), "cron 必须是字符串");
}

/// 非法样例：最小预校验函数以期望的 code/step_path/field 结构化拒绝。
#[test]
fn invalid_fixtures_are_flagged_by_precheck() {
    for id in INVALID_IDS {
        let expected: serde_json::Value =
            serde_json::from_str(&read_fixture(&format!("{id}.expected.json")))
                .unwrap_or_else(|e| panic!("{id}: expected.json 不是合法 JSON: {e}"));
        assert_eq!(expected["kind"], "invalid", "{id}: expected.kind 应为 invalid");

        let source = read_fixture(&format!("{id}.yaml"));
        let actual = precheck::precheck(id, ResourceKind::Script, &source);
        assert!(
            !actual.is_empty(),
            "{id}: 非法样例必须被预校验拒绝，但返回了空错误列表"
        );

        let mut actual_keys: Vec<(String, String, String)> = actual
            .iter()
            .map(|d: &Diagnostic| (d.code.clone(), d.step_path.clone(), d.field.clone()))
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
            "{id}: 预校验错误集与期望不一致\n  实际: {actual:?}"
        );
        // message/step_path/field 之外，每条错误都必须携带非空 code 与可读 message。
        for d in &actual {
            assert!(!d.code.is_empty(), "{id}: 错误 code 不能为空");
            assert!(!d.message.is_empty(), "{id}: 错误 message 不能为空");
        }
    }
}

/// 反向证明选型动机：serde_yaml 0.9 把单引号与无引号标量反序列化成同一个 Value，
/// 书写样式彻底丢失 —— params「整条单引号」契约只能靠事件级解析层校验。
#[test]
fn serde_yaml_loses_scalar_style() {
    let quoted: serde_yaml::Value = serde_yaml::from_str("- 'bool:enable:开关:true'\n").unwrap();
    let plain: serde_yaml::Value = serde_yaml::from_str("- bool:enable:开关:true\n").unwrap();
    assert_eq!(quoted, plain, "serde_yaml 0.9 应丢失标量样式（两者不可区分）");
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
