//! script_v2 单元测试：七类参数、错误码逐域抽查、调用环与静态深度、
//! 序列化规范形态（结构层 loader 测试也在此，经公开 parse 入口断言）。

use super::error::codes;
use super::error::ScriptError;
use super::model::{ParamDecl, ParamType, TypedValue};
use super::params::{
    self, escape_double_quoted, fmt_duration, merge_args, parse_param_decl, parse_time_duration,
    parse_time_ms,
};
use super::validate::{InMemoryResources, ResourceProvider, TemplateAvail};
use super::{parse_function_file, parse_script_file, serialize_function_file, serialize_script};

const T: &str = "t";

fn errs(src: &str) -> Vec<ScriptError> {
    parse_script_file(src, T, &InMemoryResources::new()).unwrap_err()
}

fn errs_with(src: &str, setup: impl FnOnce(&mut InMemoryResources)) -> Vec<ScriptError> {
    let mut p = InMemoryResources::new();
    setup(&mut p);
    parse_script_file(src, T, &p).unwrap_err()
}

fn ok_with(src: &str, setup: impl FnOnce(&mut InMemoryResources)) {
    let mut p = InMemoryResources::new();
    setup(&mut p);
    parse_script_file(src, T, &p).unwrap_or_else(|e| panic!("应合法: {e:?}"));
}

fn assert_code(errors: &[ScriptError], code: &str, step_path: &str, field: &str) {
    assert_code_in(errors, code, step_path, field);
    let e = errors.iter().find(|e| e.code == code).unwrap();
    assert_eq!(e.resource, T);
}

/// 不校验 resource 的版本（跨资源错误如调用环/深度闭边落在别的资源上）。
fn assert_code_in(errors: &[ScriptError], code: &str, step_path: &str, field: &str) {
    let e = errors
        .iter()
        .find(|e| e.code == code)
        .unwrap_or_else(|| panic!("缺少 {code}，实际: {errors:?}"));
    assert_eq!(e.step_path_str(), step_path, "{code} 定位不符: {e:?}");
    assert_eq!(e.field_str(), field, "{code} 字段不符: {e:?}");
}

// ---------------------------------------------------------------------------
// 参数声明与默认值（params.rs）
// ---------------------------------------------------------------------------

mod params_tests {
    use super::*;

    fn decl(raw: &str) -> ParamDecl {
        parse_param_decl(raw).unwrap_or_else(|e| panic!("声明 {raw:?} 应合法: {e:?}"))
    }

    fn bad(raw: &str) -> super::super::params::DeclError {
        parse_param_decl(raw).expect_err("应非法")
    }

    /// 七类参数逐类合法用例（含默认值解析为类型化字面量）。
    #[test]
    fn seven_types_valid() {
        let d = decl("tmpl:account:账号模板:account.png");
        assert_eq!(d.ty, ParamType::Tmpl);
        assert_eq!(d.default, Some(TypedValue::Tmpl("account.png".into())));

        let d = decl("coord:pos:执行位置:[0.5, 0.8]");
        assert_eq!(d.default, Some(TypedValue::Coord([0.5, 0.8])));

        let d = decl("color:target:目标颜色:FF8800");
        assert_eq!(d.default, Some(TypedValue::Color("FF8800".into())));

        let d = decl("time:timeout:等待:30min");
        assert_eq!(d.default, Some(TypedValue::Time("30min".into())));

        let d = decl("key:quit:退出按键:ESC");
        assert_eq!(d.default, Some(TypedValue::Key("ESC".into())));

        // 第四段整段为默认值尾串：text 可含冒号；外层双引号剥离。
        let d = decl(r#"text:url:服务地址:"https://x.com:8443""#);
        assert_eq!(
            d.default,
            Some(TypedValue::Text("https://x.com:8443".into()))
        );

        let d = decl(r#"text:empty:空文本:"""#);
        assert_eq!(d.default, Some(TypedValue::Text(String::new())));

        let d = decl("bool:enable:是否启用:false");
        assert_eq!(d.default, Some(TypedValue::Bool(false)));

        // 无第四段 = 必填。
        let d = decl("tmpl:confirm:确认按钮");
        assert_eq!(d.default, None);
        assert_eq!(d.remark, "确认按钮");
    }

    /// 七类参数逐类非法默认值。
    #[test]
    fn seven_types_invalid_defaults() {
        // bool：字符串 "true" 非法（i03 fixture 形态）。
        assert_eq!(
            bad(r#"bool:enable:开关:"true""#).code,
            codes::PARAM_DEFAULT_INVALID
        );
        assert_eq!(
            bad("bool:enable:开关:yes").code,
            codes::PARAM_DEFAULT_INVALID
        );
        // time：缺单位 / 零。
        assert_eq!(
            bad("time:timeout:等待:30").code,
            codes::PARAM_DEFAULT_INVALID
        );
        assert_eq!(
            bad("time:timeout:等待:0s").code,
            codes::PARAM_DEFAULT_INVALID
        );
        // coord：非数字 / 一维 / 越界。
        assert_eq!(
            bad("coord:pos:位置:[a, b]").code,
            codes::PARAM_DEFAULT_INVALID
        );
        assert_eq!(
            bad("coord:pos:位置:[0.5]").code,
            codes::PARAM_DEFAULT_INVALID
        );
        assert_eq!(
            bad("coord:pos:位置:[1.5, 0.2]").code,
            codes::PARAM_DEFAULT_INVALID
        );
        // color：5 位 / 非十六进制。
        assert_eq!(bad("color:c:颜色:ff88").code, codes::PARAM_DEFAULT_INVALID);
        assert_eq!(
            bad("color:c:颜色:ff88g0").code,
            codes::PARAM_DEFAULT_INVALID
        );
        // text：转义悬空。
        assert_eq!(
            bad("text:t:文本:\"a\\b\"").code,
            codes::PARAM_DEFAULT_INVALID
        );
    }

    /// key 默认值枚举校验：非法键报 param.default.invalid（消息含非法值），
    /// 具名键（含别名/小写）与纯数字 keycode 合法。
    #[test]
    fn key_default_enum_validation() {
        let e = bad("key:quit:退出按键:NOT_A_KEY");
        assert_eq!(e.code, codes::PARAM_DEFAULT_INVALID);
        assert_eq!(e.field, "default");
        assert!(e.message.contains("NOT_A_KEY"), "{:?}", e.message);
        assert!(e.message.contains("ESC"), "应含合法示例: {:?}", e.message);

        let d = decl("key:quit:退出按键:esc");
        assert_eq!(d.default, Some(TypedValue::Key("esc".into())));
        let d = decl("key:quit:退出按键:VOLUME_UP");
        assert_eq!(d.default, Some(TypedValue::Key("VOLUME_UP".into())));
        let d = decl("key:quit:退出按键:122");
        assert_eq!(d.default, Some(TypedValue::Key("122".into())));
    }

    /// 声明结构非法：段数 / 空段 / 未知类型 / 默认值类型不允许冒号。
    #[test]
    fn decl_format_errors() {
        assert_eq!(bad("tmpl:account").code, codes::PARAM_DECL_FORMAT);
        assert_eq!(bad("tmpl::备注").code, codes::PARAM_DECL_FORMAT);
        assert_eq!(bad(":a:备注").code, codes::PARAM_DECL_FORMAT);
        assert_eq!(bad("string:a:备注").code, codes::PARAM_DECL_FORMAT);
        // 第三个冒号后整段为默认值（splitn(4)），tmpl 收下冒号尾串；
        // color 默认值含冒号则非法。
        let d = decl("tmpl:a:备注:默认:多余");
        assert_eq!(d.default, Some(TypedValue::Tmpl("默认:多余".into())));
        assert_eq!(bad("color:c:备注:12:34").code, codes::PARAM_DEFAULT_INVALID);
    }

    /// 变量名规则：字符集 + 保留名（true/false/null/gb_ 前缀）。
    #[test]
    fn name_rules() {
        assert_eq!(bad("tmpl:1abc:备注").code, codes::PARAM_DECL_NAME_INVALID);
        assert_eq!(bad("tmpl:ab-c:备注").code, codes::PARAM_DECL_NAME_INVALID);
        assert_eq!(bad("tmpl:gb_x:备注").code, codes::PARAM_DECL_NAME_INVALID);
        assert_eq!(bad("tmpl:true:备注").code, codes::PARAM_DECL_NAME_INVALID);
        assert_eq!(bad("tmpl:false:备注").code, codes::PARAM_DECL_NAME_INVALID);
        assert_eq!(bad("tmpl:null:备注").code, codes::PARAM_DECL_NAME_INVALID);
        assert!(params::is_valid_name("_ok1"));
    }

    /// 空默认值非法（不等价于没有默认值）。
    #[test]
    fn empty_default() {
        let e = bad("text:message:提示文本:");
        assert_eq!(e.code, codes::PARAM_DEFAULT_EMPTY);
        assert_eq!(e.field, "default");
    }

    /// 时间单位解析（ms/s/m/min/h/d，m≡min，小数，>0）。
    #[test]
    fn time_units() {
        assert_eq!(parse_time_ms("500ms"), Some(500.0));
        assert_eq!(parse_time_ms("1.5s"), Some(1500.0));
        assert_eq!(parse_time_ms("1m"), Some(60_000.0));
        assert_eq!(parse_time_ms("2min"), Some(120_000.0));
        assert_eq!(parse_time_ms("1h"), Some(3_600_000.0));
        assert_eq!(parse_time_ms("1d"), Some(86_400_000.0));
        assert_eq!(parse_time_ms("30"), None);
        assert_eq!(parse_time_ms("0s"), None);
        assert_eq!(parse_time_ms("s"), None);
        assert_eq!(
            parse_time_duration("500ms"),
            Some(std::time::Duration::from_millis(500))
        );
        assert_eq!(
            fmt_duration(&std::time::Duration::from_millis(500)),
            "500ms"
        );
        assert_eq!(fmt_duration(&std::time::Duration::from_secs(30)), "30s");
        assert_eq!(fmt_duration(&std::time::Duration::from_secs(60)), "1m");
    }

    /// 稀疏 args 合并：默认值打底 → 显式覆盖 → 必填缺失/未知键/类型不符。
    #[test]
    fn merge_args_rules() {
        let decls = vec![
            decl("bool:enable:开关:true"),
            decl("time:delay:延迟:500ms"),
            decl("text:message:提示文本"),
        ];
        // 只覆盖部分：必填 message 缺失 → 报错。
        let err = merge_args(&decls, vec![], T).unwrap_err();
        assert_eq!(err[0].code, codes::PARAM_ARGS_MISSING_REQUIRED);

        let bound = merge_args(
            &decls,
            vec![
                ("message".into(), TypedValue::Text("hi".into())),
                ("delay".into(), TypedValue::Time("1s".into())),
            ],
            T,
        )
        .unwrap();
        assert_eq!(bound[0].1, TypedValue::Bool(true));
        assert_eq!(bound[1].1, TypedValue::Time("1s".into()));
        assert_eq!(bound[2].1, TypedValue::Text("hi".into()));

        // 未知键 + 必填缺失同时报告（错误累积不早退）。
        let err = merge_args(&decls, vec![("nope".into(), TypedValue::Bool(true))], T).unwrap_err();
        assert!(err.iter().any(|e| e.code == codes::PARAM_ARGS_UNKNOWN));
        assert!(err
            .iter()
            .any(|e| e.code == codes::PARAM_ARGS_MISSING_REQUIRED));
        // 类型不符：bool 参数给了文本。
        let err = merge_args(
            &decls,
            vec![
                ("message".into(), TypedValue::Text("hi".into())),
                ("enable".into(), TypedValue::Text("true".into())),
            ],
            T,
        )
        .unwrap_err();
        assert!(err
            .iter()
            .any(|e| e.code == codes::PARAM_ARGS_TYPE_MISMATCH));
    }

    /// text 默认值转义往返：`"` 与 `\` 的反转义与 escape_double_quoted 对称。
    #[test]
    fn text_escape_roundtrip() {
        let value = "a\"b\\c";
        let raw = format!("text:t:文本:\"{}\"", escape_double_quoted(value));
        let d = decl(&raw);
        assert_eq!(d.default, Some(TypedValue::Text(value.into())));
    }
}

// ---------------------------------------------------------------------------
// 结构层（loader.rs）：形态与字段互斥错误码
// ---------------------------------------------------------------------------

mod loader_tests {
    use super::super::loader::{load, NodeKind};
    use super::*;

    /// PoC：saphyr 事件层取到标量书写样式（单引号 vs 无引号可区分）。
    #[test]
    fn scalar_style_is_preserved() {
        let node = load("params:\n  - 'bool:a:b'\n  - bool:c:d\n").unwrap();
        let NodeKind::Map(entries) = &node.kind else {
            panic!("根必须是映射");
        };
        let params = entries
            .iter()
            .find(|e| e.key == "params")
            .expect("params 键")
            .value
            .as_seq()
            .expect("params 列表");
        assert_eq!(
            params[0].as_scalar().unwrap().1,
            saphyr_parser::ScalarStyle::SingleQuoted
        );
        assert_eq!(
            params[1].as_scalar().unwrap().1,
            saphyr_parser::ScalarStyle::Plain
        );
    }

    /// 一个步骤多个动作键 / 未知动作 / 裸标量非法值。
    #[test]
    fn step_action_shape() {
        let e = errs("steps:\n  - tap: [0.5, 0.5]\n    key: ESC\n");
        assert_code(&e, codes::STEP_MULTI_ACTION, "steps[0]", "key");

        let e = errs("steps:\n  - whatever\n");
        assert_code(&e, codes::STEP_UNKNOWN_ACTION, "steps[0]", "");

        let e = errs("steps:\n  - str_app: x\n");
        assert_code(&e, codes::STEP_FIELD_TYPE_MISMATCH, "steps[0]", "");

        let e = errs("steps:\n  - tap: [0.5, 0.5]\n    bogus: 1\n");
        assert_code(&e, codes::STEP_FIELD_UNKNOWN, "steps[0]", "bogus");
    }

    /// 必填字段缺失：swipe 缺 to。
    #[test]
    fn swipe_missing_field() {
        let e = errs("steps:\n  - swipe:\n      fm: [0.1, 0.9]\n      time: 800ms\n");
        assert_code(&e, codes::STEP_FIELD_MISSING, "steps[0]", "to");
    }

    /// check 断言步骤：throw 必填且非空标量、未知字段拒绝。
    #[test]
    fn check_step_shape() {
        let e = errs("steps:\n  - check: logo.png\n");
        assert_code(&e, codes::STEP_FIELD_MISSING, "steps[0]", "throw");

        let e = errs("steps:\n  - check: logo.png\n    throw: \"\"\n");
        assert_code(&e, codes::STEP_FIELD_TYPE_MISMATCH, "steps[0]", "throw");

        let e = errs("steps:\n  - check: logo.png\n    throw:\n      - a\n");
        assert_code(&e, codes::STEP_FIELD_TYPE_MISMATCH, "steps[0]", "throw");

        let e = errs("steps:\n  - check: logo.png\n    throw: x\n    timeout: 3s\n");
        assert_code(&e, codes::STEP_FIELD_UNKNOWN, "steps[0]", "timeout");
    }

    /// key 步骤字面量按键枚举校验：非法键报 step.field.type_mismatch（与前端
    /// checkCellLiteral 同码），具名键与数字键合法。
    #[test]
    fn key_step_literal_enum() {
        let e = errs("steps:\n  - key: NOT_A_KEY\n");
        assert_code(&e, codes::STEP_FIELD_TYPE_MISMATCH, "steps[0]", "key");
        assert!(
            e.iter()
                .find(|e| e.code == codes::STEP_FIELD_TYPE_MISMATCH)
                .map(|e| e.message.contains("NOT_A_KEY"))
                .unwrap_or(false),
            "消息应含非法值: {e:?}"
        );
        ok("steps:\n  - key: ESC\n");
        ok("steps:\n  - key: 122\n");
    }

    /// wait 随机区间起点大于终点。
    #[test]
    fn wait_range_invalid() {
        let e = errs("steps:\n  - wait: [3s, 1s]\n");
        assert_code(&e, codes::STEP_WAIT_RANGE_INVALID, "steps[0]", "duration");
        ok("steps:\n  - wait: [1s, 3s]\n");
    }

    /// loop 子流程为空 / 缺 steps / times 非整数。
    #[test]
    fn loop_shape() {
        let e = errs("steps:\n  - loop:\n      steps: []\n");
        assert_code(&e, codes::STEP_LOOP_EMPTY_STEPS, "steps[0]", "steps");
        let e = errs("steps:\n  - loop:\n      times: 3\n");
        assert_code(&e, codes::STEP_FIELD_MISSING, "steps[0]", "steps");
        let e = errs("steps:\n  - loop:\n      times: x\n      steps:\n        - log: a\n");
        assert_code(&e, codes::STEP_FIELD_TYPE_MISMATCH, "steps[0]", "times");
    }

    /// if 条件非布尔（含被引号包裹的 "true"）。
    #[test]
    fn if_non_bool_cond() {
        let e = errs("steps:\n  - if: yes\n");
        assert_code(&e, codes::STEP_IF_NON_BOOL_COND, "steps[0]", "cond");
        let e = errs("steps:\n  - if: \"true\"\n");
        assert_code(&e, codes::STEP_IF_NON_BOOL_COND, "steps[0]", "cond");
    }

    /// 根结构与顶层键：root_type / 重复键 / list_type。
    #[test]
    fn root_structure() {
        let e = errs("- log: 顶层序列\n");
        assert_code(&e, codes::SCRIPT_ROOT_TYPE, "", "yaml");

        let e = errs("steps:\n  - log: a\nsteps:\n  - log: b\n");
        assert_eq!(e[0].code, codes::YAML_SYNTAX_ERROR);

        let e = errs("steps: not-a-list\n");
        assert_code(&e, codes::STEP_LIST_TYPE, "steps", "steps");

        // 缺 steps。
        let e = errs("params:\n  - 'bool:a:b'\n");
        assert_code(&e, codes::STEP_FIELD_MISSING, "", "steps");
    }

    /// config：未知键与取值域。
    #[test]
    fn config_domain() {
        let e = errs("config:\n  interval: 500ms\n  threshold: 0.85\n  log_level: info\n  extra: 1\nsteps: []\n");
        assert_code(&e, codes::SCRIPT_CONFIG_UNKNOWN_KEY, "config", "extra");

        let e =
            errs("config:\n  interval: 500ms\n  threshold: 1.5\n  log_level: info\nsteps: []\n");
        assert_code(&e, codes::SCRIPT_CONFIG_INVALID, "config", "threshold");

        ok("config:\n  interval: 500ms\n  threshold: 0.85\n  log_level: info\nsteps: []\n");
    }

    /// 函数库记录键白名单。
    #[test]
    fn function_record_keys() {
        let e = parse_function_file("login:\n  extra: 1\n", "lib", &InMemoryResources::new())
            .unwrap_err();
        assert_code_in(&e, codes::FUNC_RECORD_UNKNOWN_KEY, "login", "extra");
        let e = parse_function_file("- bad\n", "lib", &InMemoryResources::new()).unwrap_err();
        assert_code_in(&e, codes::SCRIPT_ROOT_TYPE, "", "yaml");
    }

    fn ok(src: &str) {
        parse_script_file(src, T, &InMemoryResources::new())
            .unwrap_or_else(|e| panic!("应合法: {e:?}"));
    }
}

// ---------------------------------------------------------------------------
// 语义层（validate.rs）：引用 / 资源 / 静态重复 / 调用环 / 深度
// ---------------------------------------------------------------------------

mod validate_tests {
    use super::*;

    /// param 域：$name 引用不存在 / 类型不匹配 / 分支深处的定位。
    #[test]
    fn param_ref_domain() {
        let e = errs("steps:\n  - log: $missing\n");
        assert_code(&e, codes::PARAM_REF_UNKNOWN, "steps[0]", "message");

        let e = errs("params:\n  - 'bool:enable:开关:true'\nsteps:\n  - key: $enable\n");
        assert_code(&e, codes::PARAM_REF_TYPE_MISMATCH, "steps[0]", "key");

        let e = errs("steps:\n  - if: true\n    then:\n      - tap: $pos\n");
        assert_code(&e, codes::PARAM_REF_UNKNOWN, "steps[0].then[0]", "at");
    }

    /// param.args 域：未知键 / 必填缺失 / 类型不符（经 call 目标声明校验）。
    #[test]
    fn param_args_domain() {
        let target =
            "params:\n  - 'bool:enable:开关'\n  - 'time:delay:延迟:500ms'\nsteps:\n  - log: hi\n";
        let setup = |p: &mut InMemoryResources| {
            p.add_script("sub.yaml", target);
            p.add_script("sub2.yaml", target);
            p.add_script("sub3.yaml", target);
        };
        let e = errs_with(
            "steps:\n  - call: sub.yaml\n    args:\n      wrong: true\n",
            setup,
        );
        assert_code(&e, codes::PARAM_ARGS_UNKNOWN, "steps[0]", "args");
        assert_code(&e, codes::PARAM_ARGS_MISSING_REQUIRED, "steps[0]", "args");

        let e = errs_with(
            "steps:\n  - call: sub2.yaml\n    args:\n      enable: \"true\"\n",
            setup,
        );
        assert_code(&e, codes::PARAM_ARGS_TYPE_MISMATCH, "steps[0]", "args");

        // 类型正确 + 默认参数省略 → 合法。
        ok_with(
            "steps:\n  - call: sub3.yaml\n    args:\n      enable: false\n",
            setup,
        );
    }

    /// resource 域：call 脚本缺失 / func 文件或函数缺失 / 模板缺失。
    #[test]
    fn resource_domain() {
        let e = errs("steps:\n  - call: nope.yaml\n");
        assert_code(&e, codes::RESOURCE_SCRIPT_NOT_FOUND, "steps[0]", "target");

        let e = errs_with(
            "steps:\n  - func: common/ghost\n",
            |p: &mut InMemoryResources| {
                p.add_function_file("common", "login:\n  steps:\n    - return: true\n");
            },
        );
        assert_code(&e, codes::RESOURCE_FUNC_NOT_FOUND, "steps[0]", "target");

        let e = errs("steps:\n  - find: ghost.png\n");
        assert_code(&e, codes::RESOURCE_TMPL_NOT_FOUND, "steps[0]", "template");
    }

    /// resource.tmpl.ambiguous：provider 报同短名多个 # 后缀候选（歧义为资源错误）。
    /// InMemoryResources 只做精确名，歧义形态由真实存储侧（# 后缀解析）产生，
    /// 此处以最小 provider 固化校验路径与错误定位。
    #[test]
    fn template_ambiguous() {
        struct Ambiguous;
        impl ResourceProvider for Ambiguous {
            fn script_exists(&self, _resource_id: &str) -> bool {
                false
            }
            fn script_content(&self, _resource_id: &str) -> Option<String> {
                None
            }
            fn function_file_content(&self, _file_short: &str) -> Option<String> {
                None
            }
            fn function_exists(&self, _file_short: &str, _function: &str) -> bool {
                false
            }
            fn resolve_template(&self, _short_name: &str) -> TemplateAvail {
                TemplateAvail::Ambiguous
            }
        }
        let e = parse_script_file("steps:\n  - find: icon.png\n", T, &Ambiguous).unwrap_err();
        assert_code(&e, codes::RESOURCE_TMPL_AMBIGUOUS, "steps[0]", "template");
    }

    /// check 模板域：字面量分区存在性走通用 tmpl 校验；$ref 类型一致即合法。
    #[test]
    fn check_template_domain() {
        let e = errs("steps:\n  - check: missing.png\n    throw: x\n");
        assert_code(&e, codes::RESOURCE_TMPL_NOT_FOUND, "steps[0]", "template");
        ok_with(
            "params:\n  - 'tmpl:logo:主模板'\nsteps:\n  - check: $logo\n    throw: x\n",
            |_| {},
        );
    }

    /// ref 域：路径穿越 / 函数路径语法（恰好一段 `/`）。
    #[test]
    fn ref_path_domain() {
        let e = errs("steps:\n  - call: ../outside.yaml\n");
        assert_code(&e, codes::REF_CALL_PATH_TRAVERSAL, "steps[0]", "target");

        let e = errs("steps:\n  - func: /abs/login\n");
        assert_code(&e, codes::REF_FUNC_PATH_TRAVERSAL, "steps[0]", "target");

        let e = errs("steps:\n  - func: justname\n");
        assert_code(&e, codes::REF_FUNC_SYNTAX, "steps[0]", "target");
        let e = errs("steps:\n  - func: a/b/c\n");
        assert_code(&e, codes::REF_FUNC_SYNTAX, "steps[0]", "target");
    }

    /// 静态重复：match 候选、color 色值（FF8800 vs ff8800 视为重复）、find 主模板与 block。
    #[test]
    fn static_duplicates() {
        let e = errs_with(
            "steps:\n  - match:\n    - dup.png:\n      - log: a\n    - dup.png:\n      - log: b\n",
            |p: &mut InMemoryResources| {
                p.add_template("dup.png");
            },
        );
        assert_code(
            &e,
            codes::STEP_MATCH_CANDIDATE_DUPLICATE,
            "steps[0]",
            "candidates",
        );

        let e = errs(
            "steps:\n  - color:\n      at: [0.5, 0.5]\n      expect:\n        - FF8800:\n          - log: a\n        - ff8800:\n          - log: b\n",
        );
        assert_code(&e, codes::STEP_COLOR_DUPLICATE, "steps[0]", "expect");

        let e = errs_with(
            "steps:\n  - find: main.png\n    block:\n      - main.png\n",
            |p: &mut InMemoryResources| {
                p.add_template("main.png");
            },
        );
        assert_code(&e, codes::STEP_FIND_BLOCK_DUPLICATE, "steps[0]", "block");
    }

    /// 上下文限制：return 仅函数体内。
    #[test]
    fn return_only_in_function_files() {
        let e = errs("steps:\n  - return: true\n");
        assert_code(&e, codes::STEP_RETURN_IN_SCRIPT, "steps[0]", "");

        // 函数文件中 return 合法（脚本中的 return 已由上一段覆盖）。
        parse_function_file(
            "login:\n  params:\n    - 'bool:ok:结果'\n  steps:\n    - return: $ok\n",
            "lib",
            &InMemoryResources::new(),
        )
        .unwrap_or_else(|e| panic!("函数文件内 return 应合法: {e:?}"));
    }

    /// call 自引用 → self_cycle；跨文件互调 → cross_cycle（闭边所在资源上报）。
    #[test]
    fn call_cycles() {
        let e = errs("steps:\n  - call: t.yaml\n");
        assert_code(&e, codes::REF_CALL_SELF_CYCLE, "steps[0]", "target");

        let mut p = InMemoryResources::new();
        p.add_script("a.yaml", "steps:\n  - call: b.yaml\n");
        p.add_script("b.yaml", "steps:\n  - call: a.yaml\n");
        let e = parse_script_file("steps:\n  - call: b.yaml\n", "a.yaml", &p).unwrap_err();
        let cross = e
            .iter()
            .find(|e| e.code == codes::REF_CALL_CROSS_CYCLE)
            .expect("应有跨文件调用环");
        assert_eq!(cross.resource, "b.yaml", "环在闭边（b 的 call 步骤）上报");
        assert_eq!(cross.step_path_str(), "steps[0]");
        assert_eq!(cross.field_str(), "target");
    }

    /// 跨文件函数环（f1 ↔ f2）。
    #[test]
    fn cross_file_func_cycle() {
        let mut p = InMemoryResources::new();
        p.add_function_file("f1", "h:\n  steps:\n    - func: f2/g\n");
        p.add_function_file("f2", "g:\n  steps:\n    - func: f1/h\n");
        let e = parse_function_file("h:\n  steps:\n    - func: f2/g\n", "f1", &p).unwrap_err();
        let cycle = e
            .iter()
            .find(|e| e.code == codes::REF_FUNC_CYCLE)
            .expect("应有跨文件函数环");
        assert_eq!(cycle.resource, "f2");
        assert_eq!(cycle.step_path_str(), "g.steps[0]");
    }

    /// 静态调用深度 32 层：32 层链合法，33 层报 ref.call.depth。
    #[test]
    fn static_call_depth() {
        // a1..a32：a32 无 call —— 链深 32 合法。
        let mut p = InMemoryResources::new();
        for i in 1..=32 {
            let src = if i < 32 {
                format!("steps:\n  - call: a{}.yaml\n", i + 1)
            } else {
                "steps:\n  - log: end\n".to_string()
            };
            p.add_script(format!("a{i}.yaml"), src);
        }
        parse_script_file("steps:\n  - call: a2.yaml\n", "a1.yaml", &p)
            .unwrap_or_else(|e| panic!("32 层调用链应合法: {e:?}"));

        // 再深一层（33）→ 报 ref.call.depth @ a32.steps[0]。
        let mut p = InMemoryResources::new();
        for i in 1..=33 {
            let src = if i < 33 {
                format!("steps:\n  - call: a{}.yaml\n", i + 1)
            } else {
                "steps:\n  - log: end\n".to_string()
            };
            p.add_script(format!("a{i}.yaml"), src);
        }
        let e = parse_script_file("steps:\n  - call: a2.yaml\n", "a1.yaml", &p).unwrap_err();
        assert_code_in(&e, codes::REF_CALL_DEPTH, "steps[0]", "target");
        let depth_err = e.iter().find(|e| e.code == codes::REF_CALL_DEPTH).unwrap();
        assert_eq!(depth_err.resource, "a32.yaml");
    }

    /// 步骤嵌套深度：32 层嵌套合法，33 层报 step.nesting.depth。
    #[test]
    fn step_nesting_depth() {
        let build = |n: usize| {
            let mut src = String::from("steps:\n");
            for i in 0..n {
                let dash = 2 + i * 4;
                let body = dash + 4;
                src.push_str(&" ".repeat(dash));
                src.push_str("- loop:\n");
                src.push_str(&" ".repeat(body));
                src.push_str("steps:\n");
            }
            let tail = 2 + n * 4;
            src.push_str(&" ".repeat(tail));
            src.push_str("- log: bottom\n");
            src
        };
        // 顶层 steps 记 1 层 + 31 个嵌套 loop 的 steps = 32 层 → 合法。
        let src = build(31);
        parse_script_file(&src, T, &InMemoryResources::new())
            .unwrap_or_else(|e| panic!("32 层嵌套应合法: {e:?}"));
        // 32 个嵌套 loop → 最深 33 层 → 报错（定位在最深超限容器）。
        let src = build(32);
        let e = errs(&src);
        let depth_err = e
            .iter()
            .find(|e| e.code == codes::STEP_NESTING_DEPTH)
            .expect("应有 step.nesting.depth");
        assert_eq!(depth_err.field_str(), "steps");
        // 路径 = 顶层 steps + 32 段 ".steps[0]" —— 即第 33 层容器。
        assert_eq!(
            depth_err.step_path_str().matches(".steps").count(),
            32,
            "应定位在第 33 层 steps 容器: {}",
            depth_err.step_path_str()
        );
    }
}

// ---------------------------------------------------------------------------
// 序列化（serialize.rs）：规范形态与往返稳定性
// ---------------------------------------------------------------------------

mod serialize_tests {
    use super::*;

    fn roundtrip(src: &str) -> String {
        let sf = parse_script_file(src, T, &InMemoryResources::new())
            .unwrap_or_else(|e| panic!("应合法: {e:?}"));
        let once = serialize_script(&sf);
        let twice = parse_script_file(&once, T, &InMemoryResources::new())
            .unwrap_or_else(|e| panic!("二次装载失败: {e:?}\n{once}"));
        assert_eq!(once, serialize_script(&twice), "二次序列化字节不稳定");
        once
    }

    /// text 字面量统一双引号；log 消息按 plain 安全规则（含 ": " 时退双引号）。
    #[test]
    fn text_and_log_rendering() {
        assert_eq!(
            roundtrip("steps:\n  - text: \"hello world\"\n  - log: 中文日志\n"),
            "steps:\n  - text: \"hello world\"\n  - log: 中文日志\n"
        );
        // 文本步骤断言：plain 书写也会被规范成双引号。
        assert_eq!(
            roundtrip("steps:\n  - text: hello\n"),
            "steps:\n  - text: \"hello\"\n"
        );
        // log 内容含 ": " → 退双引号；回写仍可解析且深等（由 roundtrip 保证）。
        assert_eq!(
            roundtrip("steps:\n  - log: \"key: value\"\n"),
            "steps:\n  - log: \"key: value\"\n"
        );
    }

    /// color：纯数字色值加单引号（防数字解析丢前导零），字母色值裸写。
    #[test]
    fn color_rendering() {
        assert_eq!(
            roundtrip(
                "steps:\n  - color:\n      at: [0.5, 0.5]\n      expect:\n        - ff8800:\n          - log: a\n        - '123456':\n          - log: b\n"
            ),
            "steps:\n  - color:\n      at: [0.5, 0.5]\n      expect:\n        - ff8800:\n          - log: a\n        - '123456':\n          - log: b\n"
        );
    }

    /// check 规范形态：throw 是步骤级兄弟键，消息按 plain 安全规则输出。
    #[test]
    fn check_rendering() {
        // 用 $ref 模板避免字面量的分区存在性校验（roundtrip 资源表为空）。
        let src =
            "params:\n  - 'tmpl:logo:主模板'\nsteps:\n  - check: $logo\n    throw: 主界面未加载\n";
        assert_eq!(roundtrip(src), src);
        let src =
            "params:\n  - 'tmpl:logo:主模板'\nsteps:\n  - check: $logo\n    throw: \"a: b\"\n";
        assert_eq!(roundtrip(src), src);
    }

    /// 参数声明整条单引号；备注中的 `'` 转义为 `''`；text 默认值双引号包裹。
    #[test]
    fn param_decl_rendering() {
        let src = "params:\n  - 'text:a:it''s:\"b: c\"'\nsteps: []\n";
        assert_eq!(roundtrip(src), src);
    }

    /// 空步骤列表 flow 形态；函数文件函数之间空行分隔、结尾单换行。
    #[test]
    fn empty_steps_and_function_layout() {
        assert_eq!(roundtrip("steps: []\n"), "steps: []\n");
        let ff = parse_function_file(
            "a:\n  steps:\n    - log: x\n\nb:\n  steps:\n    - return: true\n",
            "lib",
            &InMemoryResources::new(),
        )
        .expect("函数库合法");
        assert_eq!(
            serialize_function_file(&ff),
            "a:\n  steps:\n    - log: x\n\nb:\n  steps:\n    - return: true\n"
        );
    }

    /// args 实参引号样式回写：文本实参保持双引号，时间实参保持裸写。
    #[test]
    fn args_style_preserved() {
        let src = "steps:\n  - func: common/login\n    args:\n      timeout: 30s\n      message: \"hi\"\n";
        let mut p = InMemoryResources::new();
        p.add_function_file(
            "common",
            "login:\n  params:\n    - 'time:timeout:等待'\n    - 'text:message:文本'\n  steps:\n    - return: true\n",
        );
        let sf = parse_script_file(src, T, &p).unwrap_or_else(|e| panic!("应合法: {e:?}"));
        assert_eq!(serialize_script(&sf), src);
    }
}
