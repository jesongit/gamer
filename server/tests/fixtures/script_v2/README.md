# script_v2 契约 fixture（阶段 0）

本目录是《脚本录制与可视化编辑器重构计划》（docs/SCRIPT_EDITOR_REDESIGN_PLAN.md）阶段 0
冻结的 YAML 语法契约测试样例。**前端编辑器、服务端解析/校验、YAML 文档三方的行为
以 `docs/SCRIPT_EDITOR_CONTRACT.md` 为准，本目录是其可执行样例。**

- 合法样例：`<id>.yaml` + 期望 JSON `<id>.golden.json`（用拟议前端 Model 字段名描述解析结果）
- 非法样例：`<id>.yaml` + 期望错误 `<id>.expected.json`（code + step_path + field，错误码见 CONTRACT.md 第 5 节）
- 前端副本：`web/src/script-editor/__fixtures__/yaml/`（YAML 逐字节一致）与
  `web/src/script-editor/__fixtures__/json/`（golden/expected JSON 副本），由
  `web/src/script-editor/__fixtures__/fixtures.test.js` 与服务端测试共同守护，
  副本与源文件逐字节一致性也在前端测试中校验（防两目录漂移）。

## golden JSON 结构

```jsonc
{
  "id": "<逻辑 ID>",
  "kind": "valid",
  "description": "一句话用途",
  "files": [
    {
      "file": "<本目录内文件名>",
      "role": "main | call_target | func_library_common",
      "model_kind": "script | function_library",
      "model": { }          // 拟议前端 ScriptModel / FunctionLibraryModel，字段见 CONTRACT.md
    }
  ],
  "task_snapshot": { }      // 仅 v12：定时任务参数快照形态（args 全量类型化 + param_signature）
}
```

非法样例的 expected JSON：

```jsonc
{
  "id": "<逻辑 ID>",
  "kind": "invalid",
  "description": "一句话用途",
  "errors": [ { "code": "...", "step_path": "...", "field": "..." } ]
}
```

`model` 中的取值单元格（Cell）统一为 `{"lit": <类型化字面量>}` 或 `{"ref": "<参数名>"}`；
ParamDecl 为 `{type, name, remark, default}`，`default: null` 表示必填。Model 是
"解析后的完整形态"（默认值填充、then/else 空列表显式存在），规范 YAML 则省略默认
字段——两者关系见 CONTRACT.md 第 3 节对照表。

## 合法样例索引

| 逻辑 ID | 文件 | 覆盖点 |
|---|---|---|
| v01_minimal_script | v01_minimal_script.yaml | 最小脚本：steps 必需、无 params/config |
| v02_all_actions | v02_all_actions.yaml | 全动作：str_app/cls_app/tap/swipe(fm/to/time)/key/text/log/wait/wait 随机区间 |
| v03_function_library | v03_function_library.yaml | 函数库：顶层键=函数名、记录只允许 params/steps、return true/false、find 的 $ref 字段 |
| v04_params_all_defaults | v04_params_all_defaults.yaml | 七类参数全带默认值 + config 三键 + 七类参数全部被引用（含 color 候选键用 $ref） |
| v05_params_all_required | v05_params_all_required.yaml | 七类参数全必填（default 全 null） |
| v06_nested_if_loop | v06_nested_if_loop.yaml | loop(3) > if > find(block+verify+then) 嵌套；无限 loop（times 省略=null） |
| v07_match_compact | v07_match_compact.yaml | match 紧凑缩进（候选=无缩进序列、else/timeout=兄弟键）、双候选 |
| v08_color_branch | v08_color_branch.yaml | color 分支：at + 有序颜色候选列表（单键映射项，纯数字色 '123456' 强制引号；不用颜色做映射键——js-yaml 对整数形键丢顺序） |
| v09_call_script | v09_call_script.yaml + v09_call_script.target.yaml | 脚本 call 带 args（$ref 实参与字面量实参）；目标 delay 有默认值可省略 |
| v10_func_call_cross_file | v10_func_call_cross_file.yaml + v10_func_call_cross_file.common.yaml | 跨文件函数调用 `func: common/login` + 函数库文件（短路径 common） |
| v11_record_output | v11_record_output.yaml | 录制输出形态：点击→单条 find；滑动→match→swipe + throw + 30s timeout |
| v12_task_args_snapshot | v12_task_args_snapshot.yaml | 定时任务参数快照形态：args 全量类型化 + param_signature（psig1 算法） |

## 非法样例索引

| 逻辑 ID | 文件 | 期望错误 |
|---|---|---|
| i01_old_top_format | i01_old_top_format.yaml | script.top_level.legacy_format @ func（旧 func: 段） |
| i02_params_unquoted | i02_params_unquoted.yaml | param.decl.quote_style @ params[0].style（未加单引号） |
| i03_default_type_mismatch | i03_default_type_mismatch.yaml | param.default.invalid @ params[0]、params[1]（bool 传字符串 / time 缺单位） |
| i04_match_candidate_duplicate | i04_match_candidate_duplicate.yaml | step.match.candidate_duplicate @ steps[0].candidates |
| i05_func_path_traversal | i05_func_path_traversal.yaml | ref.func.path_traversal @ steps[0..2].target（..、绝对路径、反斜杠） |
| i06_call_cycle | i06_call_cycle.yaml | ref.call.self_cycle @ steps[0].target（call 自身；跨文件环归阶段 2 引用图） |
| i07_unknown_top_key | i07_unknown_top_key.yaml | script.top_level.unknown_key @ metadata |
| i08_else_in_candidates | i08_else_in_candidates.yaml | step.match.else_in_candidates @ steps[0].candidates（- else 写进候选列表） |
| i09_empty_default | i09_empty_default.yaml | param.default.empty @ params[0].default（text:x:名:） |

## 约定与阶段边界

- 逻辑 ID 即文件主名（不含扩展名）；多文件样例（v09/v10）的辅助文件名 = `<主 ID>.<角色>.yaml`，
  真实目录布局中分别对应 `yaml/<主 ID>.yaml`、`yaml/<目标>.yaml`、`func/common.yaml`（见 CONTRACT.md 第 2 节）。
- 本目录 fixture 只约束 **语法与结构形态**；完整语义校验（资源引用存在性、类型化绑定、运行行为）
  在阶段 2 的服务端实现中覆盖（docs/SCRIPT_EDITOR_REDESIGN_PLAN.md §16.1）。
- 修改任何 fixture 必须同步：golden/expected JSON、web/src/script-editor/__fixtures__/ 副本、
  CONTRACT.md 对照表；两目录一致性由前端测试强制。
- 阶段 0 的预校验实现位于 `server/tests/script_v2_contract/precheck.rs`（最小实现，阶段 2
  迁入 `server/src` 并扩展为完整 parse_script_file()/parse_function_file() 校验器）。
