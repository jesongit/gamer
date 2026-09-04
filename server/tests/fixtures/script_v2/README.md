# script_v2 契约 fixture

本目录是当前 YAML v2 严格契约的可执行样例。**前端编辑器、服务端解析/校验、YAML
文档三方的行为以 `docs/SCRIPT_EDITOR_CONTRACT.md` 为准，本目录是其可执行样例。**

- 合法样例：`<id>.yaml` + 期望 JSON `<id>.golden.json`（描述当前前端 Model 字段）
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
      "model": { }          // 前端 ScriptModel / FunctionLibraryModel，字段见 CONTRACT.md
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
| v12_task_args_snapshot | v12_task_args_snapshot.yaml | 定时任务参数快照形态：args 全量类型化 + param_signature（psig1 算法） |
| v13_check_step | v13_check_step.yaml | check 界面断言：省略 timeout/throw 使用默认值（模板字面量与 $ref 两形态） |
| v14_branch_click | v14_branch_click.yaml | 候选级命中点击：候选值映射形态 `{click: true, steps: [...]}`（steps 省略 = 命中即点；不点击走列表形态），match/color 各覆盖点击与非点击候选 |

## 非法样例索引

| 逻辑 ID | 文件 | 期望错误 |
|---|---|---|
| i02_params_unquoted | i02_params_unquoted.yaml | param.decl.quote_style @ params[0].style（未加单引号） |
| i03_default_type_mismatch | i03_default_type_mismatch.yaml | param.default.invalid @ params[0]、params[1]（bool 传字符串 / time 缺单位） |
| i04_match_candidate_duplicate | i04_match_candidate_duplicate.yaml | step.match.candidate_duplicate @ steps[0].candidates |
| i05_func_path_traversal | i05_func_path_traversal.yaml | ref.func.path_traversal @ steps[0..2].target（..、绝对路径、反斜杠） |
| i06_call_cycle | i06_call_cycle.yaml | ref.call.self_cycle @ steps[0].target（call 自身；跨文件环由引用图校验） |
| i07_unknown_top_key | i07_unknown_top_key.yaml | script.top_level.unknown_key @ metadata |
| i08_else_in_candidates | i08_else_in_candidates.yaml | step.match.else_in_candidates @ steps[0].candidates（- else 写进候选列表） |
| i09_empty_default | i09_empty_default.yaml | param.default.empty @ params[0].default（text:x:名:） |
| i10_branch_click_type | i10_branch_click_type.yaml | step.field.type_mismatch @ steps[0].candidates[0].click、steps[1].expect[0].click（候选级 click 非布尔字面量） |

## 约定

- 逻辑 ID 即文件主名（不含扩展名）；多文件样例（v09/v10）的辅助文件名 = `<主 ID>.<角色>.yaml`，
  真实目录布局中分别对应 `scripts/<主 ID>.yaml`、`scripts/<目标>.yaml`、`functions/common.yaml`（见 CONTRACT.md 第 2 节）。
- 服务端 fixture 测试直接调用当前严格 `parse_script_file()` /
  `parse_function_file()`，同时覆盖结构、引用、类型绑定和资源存在性；运行行为由执行器测试覆盖。
- 修改任何 fixture 必须同步：golden/expected JSON、web/src/script-editor/__fixtures__/ 副本、
  CONTRACT.md 对照表；两目录一致性由前端测试强制。
- 仓库内 `server/data/<pkg>/{scripts,functions,templates}` 示例也由同一严格 loader 契约测试，避免示例绕过生产解析路径。
