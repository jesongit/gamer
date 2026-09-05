# 参数（params）与 schema API

> 本文定义 `Program.params` 参数声明的双形态、类型全集、entrypoint 参数
> schema API 与运行前参数校验；实现核对基准为
> `server/src/extensions/gamer_yaml/yaml_vnext.rs`（`parse_params`）、
> `task_params.rs`（v3 参数桥 / `V3_KNOWN_TYPES`）与
> `entrypoint_descriptor.rs`（schema 载荷）。

## 1. 参数声明双形态

`params` 是参数的**唯一来源**（P12.3 Task Params Bridge），必须是列表，元素为
字符串或映射两种形态：

```yaml
version: 3
params:
  - 'text:msg:消息内容:"默认"'        # 字符串形态：type:name[:remark[:default]]
  - 'time:wait:等待:2s'
  - name: count                       # 映射形态：{name, type, default, remark}
    type: int
    remark: 次数
    default: 3
steps:
  - log: $msg
```

- **字符串形态**：`type:name[:remark[:default]]`，按 `:` 切分（最多 4 段）；
  ty/name 为空报 `yaml.v3.params.invalid`；默认值保持原串（消费侧按类型规整）。
- **映射形态**：只允许 `name` / `type` / `default` / `remark` 四个键，未知键报
  `yaml.v3.params.unknown_key`；`type` 缺省为 `value`；`default` 必须是字面量
  （不接受 `$var` 引用，报 `yaml.v3.params.default`）。
- `remark`（字符串形态第 3 段 / 映射 `remark` 键）：随声明保留，透出到参数
  schema 的 `description`；**不参与 psig1 签名**——改备注不会触发定时任务参数
  过期。
- `params` 非列表报 `yaml.v3.params.type`。

## 2. 类型集合（以实现为准）

服务端 `task_params.rs` 的 `V3_KNOWN_TYPES` 是 v3 声明可识别的类型全集
（v2 七类 + v3 别名/扩展）：

| 声明 ty | 含义 | 取值形态 |
|---|---|---|
| `text` / `string` | 文本 | 字符串 |
| `bool` / `boolean` | 布尔 | `true` / `false` |
| `int` / `integer` | 整数 | 整数（或数字串） |
| `number` | 数值 | 数值（或数字串） |
| `value` | 任意标量（缺省类型） | 布尔/数值/字符串/二元坐标 |
| `tmpl` / `template` | 模板引用 | 模板名（分区唯一短名） |
| `coord` | 相对坐标 | `[x, y]`（分量 0~1） |
| `color` | 颜色 | hex 串（大小写不敏感） |
| `time` | 时长 | **带单位时长串**（`300ms` / `2s` / `1m` / `1h`） |
| `key` | 按键名 | 键名（签名规范为大写） |

- **time 是字符串不是 number(ms)**：执行期 TypedValue 为 `Time(带单位串)`，要求
  携带单位的时长书写（见
  `entrypoint_descriptor.rs schema_type` 注释——"执行期解析要求单位串，故不
  映射为 number"）；schema 中 `type` 为 `string` + `param_type: "time"` 透传。
- `key` 类型在 schema 中带内置键名 `enum`（`script_v2/params.rs KEY_NAMES`）。
- 未知类型：schema 端点拒绝（`param.decl.format`）、运行期绑定拒绝——schema
  与绑定同口径，不做宽松放行。

## 3. entrypoint 参数 schema API

`GET /api/runners/:runner_id/entrypoint?entrypoint=<资源id>`（P12.3 / 契约 §7；
前端**不得为取参数而解析 YAML**）。

- `entrypoint` 形态：脚本 = `<pkg>/<脚本>.yaml`；函数 =
  `<pkg>/<库文件>.yaml#<函数名>`（`#函数名` 缺省时取文件内第一个函数）。
- 载荷（`entrypoint_descriptor.rs`）：

```json
{
  "runner_id": "gamer.yaml",
  "entrypoint": "com.test.app/v3daily.yaml",
  "kind": "script",
  "format": "yaml-params-v1",
  "schema": {
    "type": "object",
    "properties": {
      "msg":  { "type": "string",  "param_type": "text",  "default": "默认", "description": "消息内容" },
      "wait": { "type": "string",  "param_type": "time",  "default": "2s" },
      "count":{ "type": "integer", "param_type": "int",   "default": 3 },
      "pos":  { "type": "array",   "param_type": "coord", "items": { "type": "number", "minItems": 2, "maxItems": 2 } },
      "key_": { "type": "string",  "param_type": "key",   "enum": ["HOME", "BACK", "..."] }
    },
    "required": ["secret"]
  },
  "signature": "psig1|text,msg,0,默认|..."
}
```

- 属性字段：`type`（JSON Schema 类型映射：bool/boolean→`boolean`、
  int/integer→`integer`、number→`number`、coord→`array`（二元数值）、
  value→`any`、其余（text/string/tmpl/color/time/key）→`string`）；
  `param_type` 透传声明原文（前端 UI 形态按此渲染）；`default`（按类型规整后的
  JSON）；`description`（remark）；`enum`（仅 `key`）；`items`（仅 `coord`）。
- `required`：无 `default` 的声明参数名列表。
- `signature`：当前 psig1 签名（`psig1|ty,name,required,canon|…`，与 v2
  `param_signature` 同 wire 形态——v2→v3 等价声明逐字节一致，迁移不触发任务
  参数过期），前端可做过期预检。
- v2 存量脚本走同一 descriptor 端点（服务端 v2 解析；v2 删除后该分流收敛）。
- 错误：未知 runner → 404 `runner_not_found`；资源缺失 → 404
  `not_found{resource}`；解析失败/未知类型 → 400 `invalid_script{diagnostics}`。

## 4. 执行前校验（POST /api/runs）

手动运行时 gamer.yaml runner 边界做参数绑定前置校验（缺 Runner/资源检查外）：

- 缺必填 → 400 `invalid_args` + 诊断码 `param.args.missing_required`（field
  可回填表单）；
- 未知参数键 → `param.args.unknown`；
- 类型不符 → `param.args.type_mismatch`（如 `time` 实参不带合法单位串、
  `coord` 分量越界 0~1、`color`/`key` 非法取值）。
- 通过后返回 202 + `run_id` + `resolved_args`（默认值合并视图：显式实参覆盖
  声明默认值，未覆盖参数取默认值）。

定时任务侧：任务保存时的参数快照与 psig1 门禁（`gate_task`）沿用同一 v3
声明来源（P12.3），声明变更导致签名不一致时任务进入参数过期待确认态。

## 5. 已知限制（以实现为准）

- TypedValue 执行链 wire 无数值变体：`int` / `number` 的**显式实参**暂以文本
  形态过线（默认值不受影响——缺省参数由 guest 按声明默认值取类型化值）。
  `int` 实参用在 `loop.times` 等强类型位置会退化为字符串——guest 取不到迭代
  上限，loop 退化为无限循环、仅受步预算兜底（`task_params.rs coerce_v3_arg`
  注释）。建议数值参数用于日志/展示场景，`loop.times` 写字面量。
- `time` 实参同理以**字符串形态**过线（`TypedValue::Time` → `Value::String`）：
  用在 `wait` / `timeout` 等强类型时长位置不会被识别为时长（`runtime.sleep`
  报「duration 必须是时间值」）；时长位置请写字面量，time 参数适合日志展示、
  快照签名等场景。`timeout` 动态 `$var` 的折算口径见
  [vision.md](vision.md) §1。
- 函数库 bare-map 无 version 键、无法静态判别 v3-ness：参数解析走「v2 严格
  解析先行、失败后 v3 宽松抽取」双路径（`probe_v3_function_decls`）。
