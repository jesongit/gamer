# call —— 唯一可调用资源入口

> 本文定义 `call` step 的命名空间、实参绑定、返回值与递归约束；权威裁决见
> [ADR-YAML-02](../reference/adr/ADR-YAML-02-callable-resource.md)，实现核对
> 基准为 `server/src/extensions/gamer_yaml/yaml_vnext.rs`（`split_call_target`）
> 与 `server/guests/yaml-guest/src/lib.rs`（call 执行）。

## 1. 语法

```yaml
- call:
    target: script:daily/login        # 或 function:工具/月卡领取
    with:                             # 参数名 → 表达式；`args` 保留为兼容别名
      account: $user
    save: result                      # 可选；返回值整体存入；无 return → null
```

| 字段 | 必填 | 说明 |
|---|---|---|
| `target` | 是 | 带命名空间的可调用资源地址（见 §2） |
| `with`（`args` 别名） | 否 | 实参 map（值是表达式） |
| `save` | 否 | 把被调方返回值整体存入变量 |

未知字段报 `yaml.v3.field.unknown`；target 非字符串报 `yaml.v3.field.string`。

## 2. target 命名空间（必须显式）

**裸 target（无前缀）与未知前缀一律在解析期拒绝**——诊断码
`yaml.v3.call.namespace`，错误信息含 target 原文与合法形态示例
（`script:<脚本id>` / `function:<文件短路径>/<函数名>`）。

- **`script:<资源id>`**：分区内 `scripts/` 相对路径，`.yaml` 后缀可省略。
  `script:daily/login` → `scripts/daily/login.yaml`。被调脚本必须是 v3
  （`version: 3`）。
- **`function:<文件短路径>/<函数名>`**：文件短路径按**最后一个 `/`** 分割、
  可含目录。`function:common/login/is_logged_in` =
  `functions/common/login.yaml` 里的 `is_logged_in`；`function:工具/月卡领取` =
  `functions/工具/月卡.yaml`（若函数名为最后一段）。两段均不能为空，形态错误
  报 `yaml.v3.call.function_path`。

### 路径穿越校验

`script:` id 与 `function:` 路径都走同一穿越校验（`reject_resource_traversal`，
沿用 v2 `split_func_path` 口径并推广）：拒绝 `..` 段、绝对路径（前导 `/`）、
反斜杠、空段（`a//fn`）——违规报 `yaml.v3.call.target`。

## 3. 解析链与分层（ADR-YAML-02）

```text
target
  ↓ parse namespace（script: / function:）
ResourceResolver（Core ResourceStore composite：EditableLocal → UserOverride → InstalledPackage）
  ↓ load resource（脚本走 scripts/、函数走 functions/ 各自寻址，仅当前分区）
v3 parse + lower
  ↓ 参数绑定（进入被调方变量空间）
execute（guest 内）
```

- 脚本与函数只经 ResourceResolver 解析，**不新增旁路文件读取**；本地编辑区
  资源与 App Package 内资源对 `call` 透明（包内函数可被本地脚本调用，反之亦
  然）。
- 跨分区一律不解析。

## 4. 实参绑定与返回值

- **传值进被调方**：`with` 的实参求值后成为被调方的初始变量；被调方读不到
  调用方的其他变量（帧隔离）。被调方声明了 `default` 的参数未被显式传入时，
  由声明默认值补齐。
- **返回值泛化**（ADR-YAML-02）：`return` 可返回 null / bool / number /
  string / object / array 任意 JSON 值；删除了 v2「函数默认返回 bool」约束。
- **`save`**：把返回值整体存入变量；被调方没有 `return` 即存 `null`。
- `if` 条件等使用处按通用值语义（truthy）判断返回值，见
  [expressions.md](expressions.md)。

## 5. 递归深度

每进入一层 callable 深度 +1、返回 -1，上限 **32**（`MAX_CALL_DEPTH`，与执行
预算同值，[ADR-YAML-04](../reference/adr/ADR-YAML-04-execution-budget.md)）。
超限立即终止，错误文本以机器可读码开头：
`CALL_DEPTH_EXCEEDED: depth=N max=32`，原样进入 RunRecord 错误信息与运行日志。
深度计数由 guest 本地 ExecutionBudget 承载（WIT `programs.resolve` 不透传
depth、宿主不做深度守卫）。

## 6. call 与运行事件

进入被调方时发 `call_start {target, depth}` 事件（depth = 本地调用深度）；被调
方内部 step 事件的 path 保持 script-local 形态（`call_start` 宣告帧切换），见
[runtime.md](runtime.md)。

## 7. 未来命名空间预留

`workflow:` / `macro:` / `plugin:` 等未来命名空间预留、走同一 `call` 机制：
新增 namespace 只扩 target parser + resolver 适配，**不新增 step 类型**（计划
§22 规则 2）。当前版本这些前缀同样报 `yaml.v3.call.namespace`（未知前缀）。
