# Gamer YAML v3 Finalization & V2 Removal 开发计划

> 目标：将 YAML v3 从“可运行的新 Runtime”收口为 Gamer 唯一正式 YAML 方案，并彻底删除 YAML v2、兼容分支、旧编辑器模型和相关过渡代码。
>
> 本计划不追求 v2/v3 逐语法、逐实现完全 parity。
>
> 核心原则：**v2 只作为需求参考，不作为架构模板。**
>
> 对仍有产品价值的能力，在 v3 中按新架构重新设计；对旧 Engine 特有、冗余或不合理的语义直接废弃。

---

## 1. 背景

Phase 11 Breaking Architecture Cleanup 基本完成后，Gamer 的 Core / Extension / Task / Runner / Resource 边界已经明显收敛。

当前 YAML 方向仍存在最后一批问题：

1. YAML v3 Runtime 已存在，但 v2 仍保留。
2. 部分 v2 能力尚未在 v3 中找到正式替代。
3. 前端 Script Editor 仍主要围绕 v2 Model / Codec 设计。
4. Task 参数定义与 v3 `Program.params` 尚未完整接通。
5. v3 Guest 缺少足够的执行预算与深度护栏。
6. Vision 默认 threshold、timing defaults 等行为尚未形成统一 v3 设计。
7. Runtime 可视化事件能力不足。
8. YAML Guest 源码位置仍带有测试夹具性质。
9. 部分 YAML UI 虽已由插件 Contribution 控制生命周期，但实现代码仍编译在 Web Core。
10. 当前 parity report 将一些旧 v2 语法差异视为“缺失”，但其中部分能力不应原样恢复。

因此下一阶段不应继续叫：

```text
v2 / v3 parity
```

而应正式定义为：

```text
YAML v3 Production Readiness
+
YAML v2 Removal
```

---

## 2. 总体目标

完成本计划后：

```text
YAML v3
=
唯一正式 YAML DSL
+
唯一正式 YAML Runtime
+
唯一正式 YAML Editor Model
+
唯一正式 Task 参数来源
```

同时彻底删除：

```text
YAML v2
script_v2
legacy YAML loader
v2/v3 compatibility branch
v2 editor codec
v2-specific tests
legacy func step
legacy match.click
legacy hidden config behavior
```

最终执行链：

```text
Task / Manual Run
        ↓
gamer.yaml Runner
        ↓
YAML v3 Program
        ↓
YAML Guest
        ↓
Capability API
        ↓
Gamer Core
```

---

## 3. 非目标

本阶段明确不做：

- 不实现 v2 语法兼容模式。
- 不支持 v2 自动升级到 v3。
- 不保留双 Runtime。
- 不保留双 Editor Codec。
- 不保留 `func` 旧 Step。
- 不保留 `match.click = true` 这种专用语法。
- 不保留隐藏式 `config.toml` 行为依赖。
- 不保证旧 YAML 文件继续运行。
- 不为旧脚本做迁移工具。
- 不长期维护 v2/v3 双测试集。

开发阶段脚本需要手动调整到 v3。

---

## 4. 总体执行顺序

建议按以下顺序实施：

```text
P12.0 冻结 YAML v3 最终语义
        ↓
P12.1 v3 Editor Model / Codec
        ↓
P12.2 Function 统一并入 call
        ↓
P12.3 Task Params Bridge
        ↓
P12.4 Guest Execution Guard
        ↓
P12.5 Vision / Timing Defaults
        ↓
P12.6 Runtime Visualization Events
        ↓
P12.7 find / match 能力补齐
        ↓
P12.8 Guest 源码正式化
        ↓
P12.9 删除 YAML v2
        ↓
P12.10 YAML UI 进一步插件隔离
        ↓
P12.11 Architecture Guard / E2E
        ↓
P12.12 文档收口
```

其中：

```text
P12.1 ~ P12.6
```

属于 v3 正式化必须项。

```text
P12.7
```

属于能力完善。

```text
P12.9
```

是真正的 Breaking Cleanup。

---

## 5. P12.0 — 冻结 YAML v3 最终语义

### 5.1 目标

在继续编码前，先明确哪些 v2 能力保留、重设计或删除，避免开发过程中为了 parity 又重新把 v2 Engine 结构搬进 v3。

### 5.2 能力裁决表

保留并重设计：

```text
find.block
find.verify
wait 随机区间
Vision threshold
timing defaults
Task 参数
运行可视化事件
执行步数 / 深度限制
match 结果上下文
```

不原样恢复：

```text
func step
match.click = true
隐藏 config.toml interval
隐藏 judge_delay
v2 AST shape
```

### 5.3 func 最终裁决

旧：

```yaml
- func:
    target: common/check_login
```

不再支持。

统一为：

```yaml
- call:
    target: function:common/check_login
    save: logged_in
```

Script：

```yaml
- call:
    target: script:daily/login
```

未来其它 callable resource：

```text
workflow:
macro:
plugin:
```

仍可以继续走 `call`。

### 5.4 match.click 最终裁决

旧：

```yaml
match:
  reward:
    click: true
```

不再支持。

改成通用结果上下文：

```yaml
- find:
    template: reward
    save: reward_match

- tap:
    point: $reward_match.center
```

或者：

```yaml
- match_first:
    candidates:
      - template: reward
        steps:
          - tap:
              point: $match.center
```

### 5.5 ADR

建议新增：

```text
ADR-YAML-01-v3-only.md
ADR-YAML-02-callable-resource.md
ADR-YAML-03-match-context.md
ADR-YAML-04-execution-budget.md
```

---

## 6. P12.1 — YAML v3 Editor Model / Codec 重写

这是最高优先级任务之一。

### 6.1 当前问题

当前 Runtime 已经主要走 v3，但 `web/src/script-editor/` 中的 `model`、`codec`、`schema`、`validation`、`factories`、`commands`、`components` 仍主要围绕旧 DSL / v2 结构工作。

形成：

```text
Runtime = v3
Editor = v2
```

这种状态不能长期存在。

### 6.2 目标

最终：

```text
Editor Model
    ⇄
YAML v3 Surface DSL
```

一一对应。

不再：

```text
Editor Model
→ v2
→ compatible loader
→ v3
```

### 6.3 推荐结构

```text
web/src/script-editor/
├── model/
│   ├── program.ts
│   ├── step.ts
│   ├── expression.ts
│   └── params.ts
├── codec/
│   ├── decode-v3.ts
│   └── encode-v3.ts
├── schema/
│   └── v3-schema.ts
├── validation/
│   └── validate-v3.ts
├── commands/
├── factories/
└── components/
```

不要保留：

```text
decode-v2.ts
encode-v2.ts
compat-codec.ts
```

### 6.4 Editor Model 原则

Editor Model 不需要完全复制 YAML AST。

建议保留 UI 友好的结构，但必须保证无损 encode / decode，至少满足：

```text
decode(encode(model)) == model
```

以及：

```text
encode(decode(valid_v3_yaml))
```

语义一致。

### 6.5 Step 支持

首批 Editor 必须完整支持：

```text
tap
swipe
wait
call
if
loop
find
match_first
log
return
```

根据实际 v3 DSL 调整。

### 6.6 Expression

避免 UI 为每种 Step 单独处理变量。

统一 Expression Model：

```ts
type Expression =
  | Literal
  | VariableRef
  | PropertyRef
  | Comparison
  | Logical
```

例如：

```text
$result
$match.center
$user.level
```

### 6.7 Params Editor

Program 参数作为一等模型：

```text
Program
├── params
└── steps
```

UI 可以编辑：

```text
name
type
required
default
description
constraints
```

### 6.8 验收标准

- [ ] Editor 只读写 v3
- [ ] 无 v2 codec
- [ ] 新建脚本默认 version 3
- [ ] v3 YAML 可以进入可视化编辑器
- [ ] 可视化编辑后仍保持合法 v3
- [ ] Program.params 可编辑
- [ ] call/function 可编辑
- [ ] match context 可表达

---

## 7. P12.2 — Function Resource 统一并入 call

### 7.1 目标

彻底取消 `func Step`，Function 成为 Callable Resource。

### 7.2 Target Scheme

推荐明确 namespace：

```text
script:<resource-id>
function:<resource-id>
```

例如：

```yaml
- call:
    target: script:daily/login
```

```yaml
- call:
    target: function:common/is_logged_in
    save: logged_in
```

### 7.3 Resolver

新增或完善：

```text
YamlCallableResolver
```

逻辑：

```text
target
↓
parse namespace
↓
ResourceResolver
↓
load resource
↓
parse as YAML v3 callable
↓
execute
```

### 7.4 Function Return

Function 不再有特殊 bool 返回约束。

统一支持：

```text
null
bool
number
string
object
array
```

由 `return` 或等价机制返回。

### 7.5 call save

统一：

```yaml
- call:
    target: function:common/is_logged_in
    save: result
```

后续：

```yaml
- if:
    cond: $result
```

### 7.6 Call Depth

此功能必须与 P12.4 一起考虑。

每次 call：

```text
depth += 1
```

超过 `max_call_depth` 立即终止。

### 7.7 验收标准

- [ ] 无 func Step
- [ ] Function 可通过 call 执行
- [ ] Script 可通过 call 执行
- [ ] call 支持返回值
- [ ] call 支持 save
- [ ] target namespace 明确
- [ ] ResourceResolver 为唯一资源来源

---

## 8. P12.3 — Task Params Bridge

这是正式 Runner 链路的必要能力。

### 8.1 当前问题

v3 已存在 `Program.params`，但 Task 参数 UI / Runner Entrypoint metadata 仍可能依赖旧 loader 或旧 Script metadata。

### 8.2 最终链路

```text
TaskBoard
    ↓
Runner Entrypoint Descriptor
    ↓
gamer.yaml
    ↓
Program.params
```

Core TaskBoard 不解析 YAML。

### 8.3 EntrypointDescriptor

建议 Runner 提供：

```rust
struct EntrypointDescriptor {
    id: String,
    title: Option<String>,
    params_schema: Value,
    metadata: Value,
}
```

### 8.4 YAML Runner

`gamer.yaml` 提供：

```text
list_entrypoints()
describe_entrypoint()
```

内部：

```text
ResourceResolver
↓
parse YAML v3
↓
Program.params
↓
JSON Schema / UI Schema
```

### 8.5 Param 类型

首版支持：

```text
string
number
integer
boolean
enum
```

可选支持：

```text
object
array
```

### 8.6 参数校验

执行前：

```text
Task payload
↓
validate against Program.params
```

错误包括：

```text
missing required param
invalid enum
wrong type
out of range
```

必须在 Guest 正式执行前返回。

### 8.7 手动运行

`RunParamsModal` 与 TaskBoard 使用同一份 `params_schema`，不能维护两套参数定义。

### 8.8 验收标准

- [ ] TaskBoard 不解析 YAML
- [ ] Program.params 是参数唯一来源
- [ ] Task 创建可动态展示参数
- [ ] Manual Run 与 Task 共用 schema
- [ ] 参数错误在执行前阻止
- [ ] 删除 v2 params loader

---

## 9. P12.4 — Guest Execution Guard

此项属于运行安全与稳定性 P0。

### 9.1 目标

增加 `ExecutionBudget`，避免无限 loop、递归 call、意外超大脚本只能依靠用户主动 Cancel。

### 9.2 ExecutionBudget

建议：

```rust
struct ExecutionBudget {
    max_steps: u64,
    consumed_steps: u64,
    max_call_depth: u32,
    current_call_depth: u32,
}
```

### 9.3 默认值

建议：

```text
max_steps = 100_000
max_call_depth = 32
```

后续根据实际压力调整。

### 9.4 Step Budget

每执行一个逻辑 Step：

```text
consumed_steps += 1
```

循环内部每次子 Step 都计数，不能只按顶层 Step 计。

### 9.5 Call Depth

进入 callable：

```text
depth += 1
```

返回：

```text
depth -= 1
```

超出：

```text
CALL_DEPTH_EXCEEDED
```

### 9.6 Cancellation

保留宿主 Cancel。

最终：

```text
Execution Guard
+
Host Cancellation
```

双机制共存。

### 9.7 错误

统一错误：

```text
STEP_BUDGET_EXCEEDED
CALL_DEPTH_EXCEEDED
CANCELLED
```

Run Event 中必须可见。

### 9.8 验收标准

- [ ] 无限 loop 自动中断
- [ ] 超深 call 自动中断
- [ ] Cancel 正常
- [ ] budget 错误可观察
- [ ] budget 不依赖 Host timeout 猜测

---

## 10. P12.5 — Vision / Timing Defaults 正式化

### 10.1 原则

不再依赖隐藏 `config.toml` 改变脚本语义，脚本行为必须尽量自包含。

### 10.2 推荐 Program Defaults

```yaml
version: 3

defaults:
  vision:
    threshold: 0.80

  timing:
    after_tap: 300ms
    after_match: 200ms
```

### 10.3 Step Override

```yaml
- find:
    template: login
    threshold: 0.90
    timeout: 10s
```

### 10.4 Threshold 优先级

```text
Step threshold
    >
Program defaults.vision.threshold
    >
Runtime built-in default
```

Runtime built-in default 只作为兜底。

### 10.5 Timing

不要恢复模糊的 `interval` / `judge_delay`。

建议语义化：

```text
after_tap
after_swipe
after_match
poll_interval
```

按实际需求控制数量，避免重新形成大量 global magic timing。

### 10.6 wait random range

支持：

```yaml
- wait:
    min: 300ms
    max: 700ms
```

或者采用 `random` 子结构，但正式实现只保留一种写法。

### 10.7 验收标准

- [ ] threshold 可全局设置
- [ ] threshold 可 Step override
- [ ] 无隐藏 config.toml 依赖
- [ ] timing defaults 明确
- [ ] wait random range 支持
- [ ] 同 YAML 在相同 Runtime 下行为可预测

---

## 11. P12.6 — Runtime Visualization Events

这是产品体验必须能力。

### 11.1 目标

运行脚本时，前端能明确展示：

```text
当前 Step
匹配模板
调用关系
变量结果
错误位置
```

### 11.2 Event Model

建议：

```text
RunStarted
ProgramEntered
StepStarted
StepCompleted
StepFailed
CallStarted
CallCompleted
VisionMatchStarted
VisionMatchResult
VariableUpdated
Log
RunCompleted
RunFailed
```

### 11.3 Step Identity

每个 Step 应有稳定 `step_id`，否则 UI 无法准确高亮当前节点。

### 11.4 Source Location

Runtime Program 可带：

```text
source
line
column
```

至少保留：

```text
resource_id
step_id
```

方便错误回到编辑器。

### 11.5 Vision Event

例如：

```text
VisionMatchResult
├── template
├── score
├── region
├── center
└── success
```

不要将大图像数据直接塞进事件流。

### 11.6 Frontend

Automation Panel 在运行时高亮对应 Step，可选显示最近变量、match score、调用栈。

### 11.7 验收标准

- [ ] 当前 Step 可高亮
- [ ] Runtime 错误可定位到 Step
- [ ] call 关系可观察
- [ ] Vision 结果可观察
- [ ] budget exceeded 可观察
- [ ] Event 不泄漏大量 frame 数据

---

## 12. P12.7 — find / match 能力补齐

此阶段保留 v2 中真正有价值的使用体验，但用 v3 结构重新实现。

### 12.1 find.verify

目标：

```text
找到目标
↓
执行操作
↓
再次验证状态
```

推荐语法示意：

```yaml
- find:
    template: login
    timeout: 10s

    steps:
      - tap:
          point: $match.center

    verify:
      template: home
      timeout: 5s
```

最终语法应以简单、一致为准。

### 12.2 find.block

v2 中 block 的价值是匹配成功后执行一组 Steps。

v3 可正式定义：

```yaml
- find:
    template: reward
    save: reward

    then:
      - tap:
          point: $reward.center
      - wait: 300ms
```

如果已有更统一的 control-flow 设计，则使用现有结构。

不要同时支持：

```text
block
steps
then
on_found
```

多个别名。

### 12.3 match result context

统一：

```text
$match
```

至少提供：

```text
$match.found
$match.score
$match.x
$match.y
$match.width
$match.height
$match.center
$match.region
```

### 12.4 match_first

候选匹配：

```yaml
- match_first:
    candidates:
      - template: reward
        steps:
          - tap:
              point: $match.center

      - template: close
        steps:
          - tap:
              point: $match.center
```

不再使用 `click: true`。

### 12.5 上下文作用域

必须定义 `$match` 作用域。

推荐：仅在对应 find/match block 内稳定有效。

如果 `save` 为变量，例如 `$reward`，则可跨后续 Step 使用。

### 12.6 验收标准

- [ ] find block 能力具备
- [ ] verify 能力具备
- [ ] match 结果可引用
- [ ] 无 match.click
- [ ] result scope 清晰
- [ ] Editor 支持

---

## 13. P12.8 — YAML Guest 源码正式化

### 13.1 当前问题

产品 Guest 源码仍位于类似：

```text
server/tests/yaml-guest/
```

的位置，测试 fixture 和正式产品实现职责混淆。

### 13.2 推荐目录

例如：

```text
extensions/
└── gamer-yaml/
    ├── guest/
    │   ├── Cargo.toml
    │   └── src/
    ├── host/
    ├── ui/
    ├── manifest.toml
    └── tests/
```

如果当前仓库已有标准 Extension 目录，则按现有目录规范调整。

### 13.3 构建

正式构建：

```text
build guest
↓
wasm componentize
↓
package into gamer.yaml
```

测试：

```text
复用正式 guest artifact
```

不要反过来让产品构建依赖测试 fixture。

### 13.4 CI

增加：

```text
build gamer.yaml guest
validate component
run runtime tests
```

### 13.5 验收标准

- [ ] 正式 Guest 不在 tests 目录
- [ ] 产品与测试使用同一份 Guest 源码
- [ ] CI 可独立构建 gamer.yaml
- [ ] Server test 不承担产品源码存放职责

---

## 14. P12.9 — 删除 YAML v2

此阶段必须在前面全部完成后一次性执行。

### 14.1 删除 Loader

删除：

```text
load_v2
load_compatible
CompatibleYamlSource
detect_v2_v3
fallback_parse
```

### 14.2 删除 Parser / AST

删除：

```text
script_v2
v2 parser
v2 AST
v2 validator
v2 lowering
```

### 14.3 删除 Runtime

删除：

```text
v2 executor
legacy native YAML engine
```

### 14.4 删除 Editor

删除：

```text
v2 model
v2 codec
v2 schema
v2 validation
v2 factories
```

### 14.5 删除 API 分支

所有：

```text
if version == 2
if legacy
if compatible
```

全部删除。

### 14.6 删除测试

删除：

```text
v2-only tests
compatibility tests
v2 -> v3 parity tests
legacy fixtures
```

只保留 v3 behavior tests。

### 14.7 旧脚本

仓库内正式 sample / examples / game package 全部手动升级到 v3。

不提供 Runtime migration。

### 14.8 Version

如果格式仍需要：

```yaml
version: 3
```

则非 3 直接返回：

```text
unsupported yaml version
```

不要 fallback。

### 14.9 验收标准

全仓搜索：

```text
script_v2
yaml v2
CompatibleYaml
legacy yaml
version == 2
```

除历史 ADR / migration notes 外：

```text
0 production references
```

---

## 15. P12.10 — YAML UI 进一步插件隔离

### 15.1 当前状态

虽然 `gamer.yaml` 已经控制“自动化 / 函数 / 模板” Panel Contribution 生命周期，但部分 Panel 实现仍可能是：

```text
runtime = core
component = console.xxx
```

即 UI 代码仍编译在 Web Core。

### 15.2 最终目标

真正做到卸载 `gamer.yaml` 后，不仅 Panel 消失，YAML Editor / YAML-specific UI Assets 也不属于裸 Core。

### 15.3 推荐方式

如果 Extension UI 支持 iframe / remote asset：

```text
gamer.yaml
├── manifest
├── wasm
└── ui/
```

Panel：

```text
runtime = extension
```

### 15.4 可分阶段

如果当前 Extension UI bundle 基础设施还不成熟，本阶段至少做到 Web Core Workspace 不含任何：

```text
if plugin == gamer.yaml
```

特殊判断。

然后把物理 bundle 拆分作为本阶段后半任务。

### 15.5 验收标准

- [ ] Core Workspace 无 YAML 特判
- [ ] YAML UI 生命周期完全由 manifest 控制
- [ ] 禁用 gamer.yaml 后所有 YAML UI 消失
- [ ] 最终可独立发布 gamer.yaml UI bundle

---

## 16. P12.11 — Architecture Guard / E2E

### 16.1 v3 Only Guard

CI 扫描 Production Source，禁止：

```text
script_v2
CompatibleYaml
legacy yaml loader
```

### 16.2 Generic Core Guard

Core 禁止依赖：

```text
YAML AST
YAML Step
Function DSL
```

### 16.3 Editor Round Trip

测试：

```text
v3 YAML
↓
decode
↓
Editor Model
↓
encode
↓
v3 YAML
↓
decode
```

语义一致。

### 16.4 Function Call

```text
script
↓
call function
↓
return value
↓
save
↓
if
```

### 16.5 Recursive Call

测试正常：

```text
A → B → C
```

以及异常：

```text
A → A → ...
```

超过深度时失败。

### 16.6 Infinite Loop

脚本无限 loop 达到 `max_steps` 必须终止。

### 16.7 Task Params

```text
Program.params
↓
TaskBoard
↓
save task
↓
run
```

完整 E2E。

### 16.8 Vision

测试：

```text
default threshold
step override
match context
find verify
```

### 16.9 Events

至少验证：

```text
RunStarted
StepStarted
VisionMatchResult
StepCompleted
RunCompleted
```

失败路径验证：

```text
StepFailed
RunFailed
```

### 16.10 Plugin Lifecycle

```text
install gamer.yaml
↓
UI + Runner available

disable
↓
UI disappears
Runner disappears
Task dependency missing

enable
↓
UI restored
Runner restored
Task runnable
```

---

## 17. P12.12 — 文档收口

### 17.1 parity report

原：

```text
phase11_v2_v3_parity_report.md
```

不要继续当执行计划。

建议在文件顶部明确：

```text
Status: superseded
```

并链接本计划。

### 17.2 README

更新：

```text
YAML v3 = only supported YAML version
```

删除所有 v2 compatible / fallback / legacy YAML 正式说明。

### 17.3 DSL 文档

建立唯一：

```text
docs/yaml-v3/
```

包含：

```text
overview.md
program.md
params.md
steps.md
expressions.md
call.md
vision.md
timing.md
runtime.md
examples.md
```

### 17.4 Examples

所有例子只使用 v3。

---

## 18. 推荐开发批次

### Batch A：v3 基础正式化

```text
refactor(editor): migrate script editor model to yaml v3
feat(yaml): expose v3 program params as runner schema
feat(yaml): unify functions and scripts under call targets
feat(yaml): add guest execution budget
```

### Batch B：运行能力

```text
feat(yaml): add program vision and timing defaults
feat(yaml): add runtime execution events
feat(yaml): add match result context
```

### Batch C：DSL 完善

```text
feat(yaml): add find block and verify
feat(yaml): add random wait range
```

### Batch D：源码结构

```text
refactor(extension): move gamer yaml guest out of test fixtures
build(extension): package official gamer yaml guest
```

### Batch E：Breaking Cleanup

```text
refactor(yaml)!: remove yaml v2 parser and runtime
refactor(editor)!: remove v2 codec and model
chore(yaml)!: remove compatibility loaders and tests
```

### Batch F：插件隔离

```text
refactor(ui): move yaml panels behind extension-owned ui
test(architecture): add yaml v3 only guards
```

---

## 19. 开发检查清单

### DSL

- [ ] func 删除
- [ ] function 通过 call
- [ ] script 通过 call
- [ ] call save
- [ ] match context
- [ ] find block
- [ ] find verify
- [ ] wait random
- [ ] vision defaults
- [ ] timing defaults

### Editor

- [ ] v3 model
- [ ] v3 codec
- [ ] v3 schema
- [ ] v3 validation
- [ ] v3 params UI
- [ ] call editor
- [ ] vision editor
- [ ] find editor
- [ ] 无 v2 codec

### Runner / Task

- [ ] Program.params bridge
- [ ] EntrypointDescriptor
- [ ] Task dynamic params
- [ ] Manual Run dynamic params
- [ ] params validation
- [ ] 无 v2 loader dependency

### Guest

- [ ] max_steps
- [ ] max_call_depth
- [ ] cancellation
- [ ] runtime events
- [ ] match result context
- [ ] 正式源码目录
- [ ] CI build

### Runtime

- [ ] threshold default
- [ ] threshold override
- [ ] timing defaults
- [ ] call resolver
- [ ] function resolver
- [ ] script resolver
- [ ] errors standardized

### Cleanup

- [ ] script_v2 删除
- [ ] v2 parser 删除
- [ ] v2 AST 删除
- [ ] v2 executor 删除
- [ ] v2 editor 删除
- [ ] compatibility loader 删除
- [ ] fallback 删除
- [ ] v2 tests 删除
- [ ] 旧 examples 删除 / 改写

### Plugin

- [ ] YAML UI 无 Core 特判
- [ ] Runner 生命周期归 gamer.yaml
- [ ] UI 生命周期归 gamer.yaml
- [ ] Guest 生命周期归 gamer.yaml

---

## 20. 最终验收场景

### 场景 A：创建 v3 自动化

```text
新建自动化
↓
可视化编辑
↓
保存 YAML v3
↓
运行
```

编辑器和 Runtime 不经过任何 v2。

### 场景 B：调用 Function

```yaml
- call:
    target: function:common/is_logged_in
    save: logged_in

- if:
    cond: $logged_in
```

正常执行。

### 场景 C：Task Params

脚本：

```yaml
params:
  account:
    type: integer
    required: true
```

TaskBoard 自动生成 `account` 输入框，执行时 payload 正确传入。

### 场景 D：无限循环

超过执行预算：

```text
STEP_BUDGET_EXCEEDED
```

并在运行 UI 中显示。

### 场景 E：Vision Match

```yaml
defaults:
  vision:
    threshold: 0.8
```

单 Step：

```yaml
threshold: 0.92
```

Step override 正常生效。

### 场景 F：Match Context

```yaml
- find:
    template: reward
    save: reward

- tap:
    point: $reward.center
```

正常执行。

### 场景 G：禁用 gamer.yaml

“自动化 / 函数 / 模板” Panel 全部消失，Runner 注销，YAML Task 进入：

```text
DEPENDENCY_MISSING
```

### 场景 H：旧 v2 文件

加载：

```text
version: 2
```

直接：

```text
unsupported yaml version
```

无 fallback。

---

## 21. Definition of Done

只有满足以下全部条件，YAML v3 Finalization 才算完成。

### Runtime

```text
只有 v3 Runtime
```

### Parser

```text
只有 v3 Parser
```

### Editor

```text
只有 v3 Editor Model / Codec
```

### Task

```text
Program.params
=
Task Params 唯一来源
```

### Function

```text
Function
=
callable resource
```

不存在 `func step`。

### Vision

```text
match result
=
通用 runtime value
```

不存在 `match.click`。

### Guard

```text
step budget
call depth
cancellation
```

全部具备。

### Compatibility

Production Code 中：

```text
YAML v2 = 0
Legacy Loader = 0
Compatibility Branch = 0
```

### Plugin Boundary

`gamer.yaml` 拥有：

```text
Parser
Runtime
Runner
Editor Semantics
UI Contribution
```

Core 只提供：

```text
Capability
Resource
Task
Run
Extension Runtime
```

---

## 22. 最终原则

以后扩展 YAML DSL 时，坚持以下规则。

### 规则 1

优先增加通用数据模型，而不是特殊 Step。

例如 `match result` 优于 `match.click`。

### 规则 2

优先增加通用 `call`，而不是 `func` / `script_call` / `workflow_call` / `macro_call`。

### 规则 3

脚本行为尽量自包含，避免隐藏 config + magic defaults 影响执行语义。

### 规则 4

Runtime 能力必须可观察，任何重要执行行为都应进入 Run Event。

### 规则 5

Editor 与 Runtime 共用同一正式 DSL，绝不再次出现：

```text
Editor = v2
Runtime = v3
```

### 规则 6

新功能只为 v3 开发，YAML v2 从删除开始即视为不存在。

---

## 23. 完成后的状态

完成后 Gamer YAML 架构：

```text
gamer.yaml
│
├── YAML v3 Surface DSL
├── Parser
├── Validator
├── Program / Runtime
├── Guest
├── Callable Resolver
├── Runner
├── Params Schema
├── Runtime Events
├── Visual Editor
└── Extension UI
```

而 Gamer Core：

```text
Core
│
├── Capability API
├── ResourceResolver
├── RunManager
├── TimerCore
├── RunnerRegistry
└── Extension Runtime
```

最终达到：

> **YAML 是一个完整、可安装、可卸载、可独立演进的官方 Extension，而不是 Gamer Core 的特殊内建 DSL。**
