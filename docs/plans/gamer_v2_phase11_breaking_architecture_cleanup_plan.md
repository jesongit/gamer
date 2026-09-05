# Gamer V2 Phase 11：Breaking Architecture Cleanup 计划

> 目标：在仍处于开发阶段、允许破坏性修改的前提下，完成 Gamer V2 插件化架构的最终收尾。
>
> 本阶段不考虑旧配置、旧 API、旧数据目录、旧 UI、旧任务格式、旧插件行为的兼容性。
>
> 核心原则：**删除过渡层，保留最终架构。**
>
> 📍 **状态：已实施收口（2026-09-05，主链全部合入 main，门禁 server 629 / web 705 全绿）。**
> checklist 回填与 DoD 对照见 §18/§23；执行波次与提交历史以 git log（`a853df2`..`417d732`）为准。

---

## 1. 背景

当前 `gamer_refactor_plan_v2` 的主体重构已经完成，以下基础能力已经基本具备：

- Core / Extension 分层
- Extension Registry
- WASM Extension Runtime
- Extension UI Contribution
- App Package
- Resource Resolver
- YAML vNext
- Keymap WASM
- TimerCore
- Schedule Provider
- Runner 抽象
- App Package 导入 / 导出 / 编辑
- Editable / Override / Installed 资源优先级

但当前代码仍保留了大量为“渐进式重构”和“兼容旧实现”设计的过渡层，导致最终架构存在以下问题：

1. Core 仍直接持有 YAML / Script / Keymap 等业务语义。
2. 前端仍存在“挂着插件名、实际由 Core Vue Component 实现”的 Panel。
3. TaskBoard 仍然围绕 `script_id` 和 YAML Script 设计。
4. `gamer.yaml` Runner 生命周期没有真正与插件生命周期绑定。
5. Core Router 暴露 `/scripts`、`/functions`、`/keymaps` 等业务型 API。
6. 新旧 Task API、Preset API、资源路径、旧数据迁移逻辑仍并存。
7. Keymap 仍存在 Native fallback。
8. YAML v2 / legacy script 等兼容逻辑仍存在。
9. 文档中仍保留大量“回退 / 兼容 / 临时方案”的设计说明。

这些问题在开发阶段可以直接通过 Breaking Change 解决。

---

# 2. 本阶段最终目标

完成 Phase 11 后，Gamer 应满足以下架构。

```text
Gamer Core
│
├── Device Runtime
│   ├── DeviceManager
│   ├── Screen / Frame
│   ├── Touch
│   ├── Keyboard Input
│   └── DeviceAction
│
├── Extension Runtime
│   ├── ExtensionRegistry
│   ├── WASM Host
│   ├── Capability API
│   └── UI Contribution Registry
│
├── Resource System
│   ├── ResourceResolver
│   ├── Editable
│   ├── Override
│   └── Installed Package
│
├── App Package
│
├── RunManager
│
├── TimerCore
│   ├── Task
│   ├── TimerRunnerRegistry
│   └── ScheduleProviderRegistry
│
└── Core UI
    ├── 任务
    ├── 日志
    ├── 设置
    └── 插件管理
```

业务功能全部由 Extension 提供：

```text
gamer.yaml
├── YAML Parser
├── YAML Validator
├── YAML Runtime / Lowering
├── Script / Function 语义
├── YAML Runner
├── 自动化 Panel
├── 函数 Panel
└── 模板 Panel

gamer.keymap
├── Keymap Parser
├── Mapping Rules
├── WASM Runtime
└── 映射 Panel
```

Core 不再认识：

```text
YAML
script_id
function DSL
keymap rule
YamlTimerRunner
ScriptStore
KeymapStore
```

---

# 3. 非目标

本阶段明确不做：

- 不保留旧 Task API。
- 不保留旧 YAML v2 格式。
- 不保留旧数据目录。
- 不保留旧 keymap native runtime。
- 不保留旧 ScriptStore API。
- 不保留旧 `/user-tasks` 路由名称。
- 不做配置自动迁移。
- 不做旧 App Package 自动升级。
- 不维护旧 Snapshot UI。
- 不继续维护 PowerShell 作为正式打包链路。
- 不为旧版本增加 adapter。
- 不新增“兼容模式”开关。

如果本地开发数据因此失效：

> 直接删除开发数据并重新生成。

---

# 4. 总体执行顺序

建议严格按照以下顺序实施：

```text
P11.0 架构基线冻结
        ↓
P11.1 Task Model 泛化
        ↓
P11.2 TimerRunnerRegistry
        ↓
P11.3 gamer.yaml 真正插件化
        ↓
P11.4 gamer.keymap 真正插件化
        ↓
P11.5 前端 Core Panel 清理
        ↓
P11.6 Core API 泛化
        ↓
P11.7 Legacy / Compatibility Cleanup
        ↓
P11.8 App Package 生命周期 E2E
        ↓
P11.9 Architecture Guard Tests
        ↓
P11.10 文档收口
```

不要同时大范围修改所有模块。

优先先把：

```text
Task → Runner → YAML
```

这条主链收干净，再处理 Keymap 和 UI。

---

# 5. P11.0 — 架构基线冻结

## 5.1 目标

在正式删除旧代码之前，明确最终架构边界，防止开发过程中再次引入临时兼容设计。

## 5.2 新增 ADR

建议新增：

```text
docs/gamer_refactor_plan_v2/adr/
├── ADR-11-core-extension-final-boundary.md
├── ADR-12-task-runner-model.md
├── ADR-13-extension-owned-runner.md
└── ADR-14-no-legacy-compatibility.md
```

### ADR-11

明确：

Core 只能拥有：

- 设备能力
- Extension 机制
- Resource 机制
- App Package
- Timer / Task
- Run
- 日志
- 设置

Core 禁止拥有：

- YAML parser
- YAML AST
- Script DSL
- Function DSL
- Keymap rule
- Plugin-specific UI

### ADR-12

Task 统一采用：

```text
Task
├── id
├── name
├── enabled
├── device
├── app_context
├── schedule
├── runner
├── state
└── metadata
```

### ADR-13

Runner 生命周期必须属于 Extension。

### ADR-14

明确：

> V2 正式架构不提供 legacy compatibility。

---

# 6. P11.1 — Task Model 完全泛化

这是本阶段最高优先级任务。

## 6.1 当前问题

当前 TaskBoard 和旧 Task API 仍围绕：

```text
cron
script_id
device_id
args
```

设计。

这意味着：

```text
Task = YAML Script Cron Task
```

而不是：

```text
Task = 任意 Runner + 任意 Schedule Provider
```

## 6.2 新 Task 数据模型

建议统一为：

```rust
struct Task {
    id: TaskId,
    name: String,
    enabled: bool,

    device_id: Option<DeviceId>,
    app_context: Option<AppContext>,

    schedule: TaskSchedule,
    runner: TaskRunnerSpec,

    state: TaskState,
    metadata: Map<String, Value>,
}
```

### TaskSchedule

```rust
struct TaskSchedule {
    provider_id: String,
    config: serde_json::Value,
}
```

例如：

```json
{
  "provider_id": "gamer.cron",
  "config": {
    "expression": "0 8 * * *"
  }
}
```

未来可以支持：

```text
gamer.cron
gamer.interval
gamer.manual
thirdparty.calendar
```

### TaskRunnerSpec

```rust
struct TaskRunnerSpec {
    runner_id: String,
    entrypoint: String,
    payload: serde_json::Value,
}
```

YAML 示例：

```json
{
  "runner_id": "gamer.yaml",
  "entrypoint": "daily/login",
  "payload": {
    "account": 1
  }
}
```

未来 Macro：

```json
{
  "runner_id": "gamer.macro",
  "entrypoint": "daily_reward",
  "payload": {}
}
```

## 6.3 删除旧字段

彻底删除：

```text
script_id
cron
script_args
script_path
yaml_script_id
```

Task Core Model 中不得存在任何 YAML-specific 字段。

## 6.4 API 收口

当前：

```text
/api/tasks
/api/user-tasks
```

最终只保留：

```text
/api/tasks
```

将当前真正泛化后的 UserTask 实现升级为正式 Task。

删除旧 `/api/tasks` compatibility adapter。

### 推荐 API

```text
GET    /api/tasks
POST   /api/tasks
GET    /api/tasks/:id
PUT    /api/tasks/:id
DELETE /api/tasks/:id
POST   /api/tasks/:id/run
POST   /api/tasks/:id/enable
POST   /api/tasks/:id/disable
POST   /api/tasks/:id/suspend
POST   /api/tasks/:id/resume
```

如果 enable / disable 已可通过 PUT 表达，可进一步减少 API。

## 6.5 Task Preset

只保留一套：

```text
/api/task-presets
```

删除：

```text
/api/tasks/presets
```

或其它 alias。

Preset 数据也必须使用新 Task Schema。

## 6.6 TaskBoard 重构

TaskBoard 不再直接：

```text
listScripts()
listTemplates()
```

也不再知道：

```text
script_id
```

UI 建议：

```text
任务名称

设备
[选择设备]

触发方式
[Schedule Provider]

执行器
[Runner]

执行目标
[由 Runner Contribution 提供]

参数
[由 Runner Schema / UI Contribution 提供]

启用
```

## 6.7 Runner Editor Contribution

为未来扩展，增加一个轻量 UI contract：

```ts
interface RunnerEditorContribution {
    runnerId: string
    title: string

    listEntrypoints?: ...
    getPayloadSchema?: ...
    renderEditor?: ...
}
```

V1 不需要做过度抽象。

`gamer.yaml` 只需能够提供：

```text
runner_id = gamer.yaml
entrypoint selector
payload editor
```

## 6.8 验收标准

- [ ] Task model 中不存在 `script_id`
- [ ] Task model 中不存在 YAML-specific type
- [ ] `/api/user-tasks` 已删除
- [ ] `/api/tasks` 只对应新 Task Model
- [ ] TaskBoard 不调用 `listScripts()`
- [ ] TaskBoard 不调用 `listTemplates()`
- [ ] TaskBoard 可以显示任意 Runner
- [ ] Task 可以保存未知 Runner
- [ ] Runner 缺失时 Task 进入 dependency missing / suspended 状态
- [ ] Core 不因 Runner 缺失而删除 Task

---

# 7. P11.2 — TimerRunnerRegistry 正式落地

## 7.1 当前问题

目前已经存在 Runner 抽象，但 `gamer.yaml` Runner 仍由 Native Server 代码构造。

这种结构：

```text
Core
└── YamlTimerRunner
```

会导致：

```text
卸载 gamer.yaml
↓
Core 仍然知道 gamer.yaml 怎么执行
```

违反插件生命周期。

## 7.2 新 Registry

新增：

```rust
TimerRunnerRegistry
```

职责：

```text
register
unregister
replace
get
list
owner lookup
```

建议结构：

```rust
struct RegisteredRunner {
    runner_id: String,
    owner_extension_id: String,
    runner: Arc<dyn TimerRunner>,
}
```

## 7.3 API

内部接口：

```rust
register_runner(runner_id, owner_extension_id, runner)
unregister_runner(runner_id)
unregister_owner(extension_id)
get_runner(runner_id)
list_runners()
```

## 7.4 Extension 生命周期绑定

Extension 启动：

```text
load gamer.yaml
↓
initialize
↓
register gamer.yaml runner
```

Extension 禁用：

```text
disable gamer.yaml
↓
unregister all owned runners
```

Extension 卸载：

```text
uninstall gamer.yaml
↓
unregister runner
↓
remove UI contribution
↓
remove runtime
```

## 7.5 Runner 缺失处理

TimerCore 执行 Task：

```text
lookup runner_id
```

若不存在：

```text
TaskState = DEPENDENCY_MISSING
```

建议保留：

```text
missing_dependency = gamer.yaml
```

Task 不删除。

## 7.6 Runner 恢复

重新安装 / 启用 Extension：

```text
runner gamer.yaml registered
```

Task 应自动恢复为：

```text
READY
```

或者等待下一个 schedule。

## 7.7 删除

完成后删除 Native：

```text
YamlTimerRunner
timer_yaml.rs
```

如果其中存在通用逻辑，先搬到：

```text
TimerCore
RunManager
Extension Runtime
```

再删除 YAML-specific 部分。

## 7.8 验收标准

- [ ] TimerCore 不 import gamer.yaml
- [ ] TimerCore 不构造 YamlTimerRunner
- [ ] RunnerRegistry 支持 register / unregister
- [ ] Runner 有 owner_extension_id
- [ ] Extension disable 后 Runner 消失
- [ ] Extension enable 后 Runner 恢复
- [ ] 相关 Task 不丢失
- [ ] 缺 Runner 的 Task 有明确状态

---

# 8. P11.3 — gamer.yaml 真正移出 Core

这是本阶段最重要的代码移动之一。

## 8.1 最终目标

Core 只知道：

```text
Extension
WASM
Capability
Resource
Run
Task
Runner
```

Core 不知道：

```text
YAML
Script
Function
Step
DSL
Parser
AST
```

## 8.2 需要迁出的模块

检查并处理类似：

```text
server/src/script_v2.*
server/src/yaml_vnext.*
server/src/task_params.*
server/src/timer_yaml.*
server/src/scripts.*
server/src/yaml_extension.*
```

注意：不是机械地全部删除。

需要逐个判断：

```text
通用能力
    ↓
搬到 Core Generic Runtime

YAML 语义
    ↓
搬到 gamer.yaml Extension
```

## 8.3 YAML Parser

最终：

```text
gamer.yaml
├── parser
├── validation
├── AST / IR
└── lowering
```

Core 不负责：

```text
v2 ?
v3 ?
which YAML schema ?
```

## 8.4 删除 YAML v2

既然开发阶段允许 Breaking Change：

只保留一套正式格式：

```text
YAML vNext
```

删除：

```text
script_v2 parser
legacy YAML loader
v2 -> v3 fallback
version guessing
```

格式错误直接返回：

```text
unsupported format
```

不自动转换。

## 8.5 ScriptStore

当前 Core 的：

```text
ScriptStore
```

需要删除。

替代方案：

```text
ResourceResolver
+
Workspace Resource API
```

Script 对 Core 来说只是：

```text
Resource
```

而不是特殊 Store。

## 8.6 Functions

同理：

Core 不需要：

```text
FunctionStore
runFunction()
```

`functions/` 是：

```text
ResourceKind::Functions
```

具体解释和执行由：

```text
gamer.yaml
```

负责。

## 8.7 Templates

需要区分：

### Core 保留

```text
vision.match
vision.match_many
frame
region
```

### gamer.yaml / Extension UI 负责

```text
模板管理 Panel
模板业务组织
模板引用语义
```

ResourceResolver 可以认识：

```text
templates
```

但 Core UI 不需要出现“模板管理”。

## 8.8 Extension Runtime Contract

推荐：

```text
plugin.call(extension_id, method, payload)
```

或通过已有 WASM Host Interface 完成。

执行 YAML：

```text
TimerCore
↓
RunnerRegistry
↓
gamer.yaml Runner
↓
Extension Runtime
↓
WASM
↓
Capability
↓
Core
```

## 8.9 验收标准

- [ ] Core crate/module 不存在 YAML parser
- [ ] Core 不存在 ScriptStore
- [ ] Core 不存在 Function DSL 类型
- [ ] Core 不存在 YAML version fallback
- [ ] gamer.yaml 可以独立解析资源
- [ ] gamer.yaml 可以独立执行
- [ ] gamer.yaml disable 后 YAML Task 进入 dependency missing
- [ ] Core 正常启动且不要求 gamer.yaml 存在

---

# 9. P11.4 — gamer.keymap 真正插件化

## 9.1 当前问题

当前 Keymap 已存在 Extension 语义，但仍有：

- Core KeymapStore
- Host Vue Keymap Panel
- Native Keymap fallback
- Core Mapping Rule 解析

最终需要全部收掉。

## 9.2 Core 保留

Core 只提供：

```text
InputEvent
KeyboardEvent
GamepadEvent
Touch
DeviceAction
Android key passthrough
```

## 9.3 gamer.keymap 负责

```text
keymap.yaml
mapping rule
rule matching
state
key combination
sequence
WASM runtime
UI Panel
```

## 9.4 无 Keymap 插件行为

没有 `gamer.keymap` 时：

```text
Keyboard Input
↓
Core passthrough
↓
Android key event
```

仍然允许：

- 普通按键
- 基础设备控制
- 投屏
- 点击
- 拖动

但不存在：

```text
映射规则
映射 Panel
组合键解释
```

## 9.5 删除 KeymapStore

从：

```text
RuntimeServices
AppState
Router
```

移除：

```text
Arc<KeymapStore>
```

Keymap 文件通过：

```text
ResourceResolver
```

读取。

## 9.6 删除 Host Keymap Panel

删除或重构：

```text
web/src/workspace/keymap-extension.ts
```

不再：

```text
runtime: core
component: VueComponent
```

由 `gamer.keymap` manifest 自己贡献 UI。

## 9.7 删除 Native Mapping Fallback

删除：

```text
if wasm keymap unavailable
    use native mapping engine
```

替换成：

```text
if gamer.keymap unavailable
    passthrough
```

## 9.8 验收标准

- [ ] Core 无 KeymapStore
- [ ] Core 无 mapping rule parser
- [ ] Core 无 native mapping engine
- [ ] Core 无 Keymap Panel
- [ ] gamer.keymap 安装后映射 Panel 出现
- [ ] gamer.keymap 禁用后 Panel 消失
- [ ] gamer.keymap 禁用后 mapping runtime 消失
- [ ] 无插件时基础 Keyboard passthrough 正常

---

# 10. P11.5 — 前端 Core Panel 最终清理

## 10.1 最终 Core Panel

只保留：

```text
任务
日志
设置
插件
```

如果插件管理是独立入口，也可不作为 Panel。

## 10.2 从 Core 删除

删除：

```text
模板
脚本
映射
自动化
函数
```

这些均属于 Extension Contribution。

## 10.3 gamer.yaml Panels

建议：

```text
自动化
函数
模板
```

是否保留“脚本”这个名称，可以根据产品 UI 决定。

推荐避免同时出现：

```text
脚本
自动化
```

造成语义重复。

可以统一：

```text
自动化
```

内部管理 scripts。

## 10.4 gamer.keymap Panel

```text
映射
```

## 10.5 Panel 生命周期

安装插件：

```text
manifest
↓
contribution registry
↓
Panel 自动出现
```

禁用插件：

```text
Panel 自动隐藏
```

卸载插件：

```text
Panel 完全移除
```

Core 不应有：

```text
if plugin == gamer.yaml
    add panel...
```

## 10.6 删除特殊判断

搜索前端：

```text
gamer.yaml
gamer.keymap
templates
scripts
functions
```

对于 Core Workspace 中出现的 hard-code 全部逐个审查。

最终 Workspace Host 只认识：

```text
PanelContribution
```

## 10.7 验收标准

裸 Gamer：

```text
右侧：
任务 | 日志 | 设置 | +
```

安装 gamer.yaml：

```text
任务 | 日志 | 设置 | 自动化 | 函数 | 模板 | +
```

安装 gamer.keymap：

```text
任务 | 日志 | 设置 | 自动化 | 函数 | 模板 | 映射 | +
```

卸载后对应 Panel 立即消失。

---

# 11. P11.6 — Core REST API 泛化

## 11.1 当前问题

Core Router 仍提供类似：

```text
/api/scripts
/api/functions
/api/keymaps
/api/templates
/api/scripts/:id/run
/api/functions/:id/run
```

这些 API 暴露了 Extension 业务语义。

## 11.2 Generic Resource API

资源编辑统一走：

```text
/api/resources
```

或者：

```text
/api/workspaces/:app/resources
```

推荐结构：

```text
GET /api/apps/:app/resources
GET /api/apps/:app/resources/:kind
GET /api/apps/:app/resources/:kind/:id
PUT /api/apps/:app/resources/:kind/:id
DELETE /api/apps/:app/resources/:kind/:id
```

ResourceKind：

```text
templates
scripts
functions
keymaps
presets
resources
```

注意：Core 可以知道 ResourceKind，但不知道资源内容语义。

例如 Core 可以知道 `scripts` 是一个目录类别，但不能知道 script 怎么解析、怎么执行。

## 11.3 执行 API

删除：

```text
POST /api/scripts/:id/run
POST /api/functions/:id/run
```

执行统一通过：

```text
POST /api/runs
```

Payload：

```json
{
  "runner_id": "gamer.yaml",
  "entrypoint": "daily/login",
  "payload": {}
}
```

或者走 Extension RPC。

## 11.4 是否保留 ResourceKind Enum

可以保留。

因为 `ResourceKind` 属于资源分类，不代表 Core 理解资源业务语义。

## 11.5 验收标准

- [ ] 无 `/api/scripts/:id/run`
- [ ] 无 `/api/functions/:id/run`
- [ ] Resource CRUD 统一
- [ ] 执行统一由 Runner / RunManager
- [ ] Core Router 不 import YAML execution type

---

# 12. P11.7 — Legacy / Compatibility Cleanup

本阶段是明确的“删除阶段”。

## 12.1 Task

删除：

```text
旧 Task model
旧 /api/tasks adapter
/api/user-tasks
旧 script_id task
```

## 12.2 Presets

删除重复：

```text
/api/tasks/presets
```

只留一个正式入口。

## 12.3 YAML

删除：

```text
YAML v2
script_v2
legacy parser
legacy lowering
version fallback
```

## 12.4 Keymap

删除：

```text
native keymap engine
legacy keymap fallback
Core mapping parser
```

## 12.5 App Package

删除：

```text
layout v1 migration
旧目录 migration
旧 package format fallback
旧 Snapshot 主流程
```

如果存在：

```text
format_version = 1
```

直接不支持。

## 12.6 File Migration

检查：

```text
file_migration.rs
```

开发阶段建议：如果其中只剩旧布局迁移逻辑，直接删除整个模块。

如果仍有与当前布局相关的初始化能力，则拆分为：

```text
workspace_init.rs
```

保留初始化，删除 migration。

## 12.7 PowerShell

如果 PowerShell 仅作为旧打包方案：从正式流程和文档移除。

可以保留在 `dev/tools`，但不得成为产品主路径；更推荐删除避免双入口。

## 12.8 前端

删除：

```text
legacy page
legacy tabs
Snapshot old UI
fallback panel
core YAML panels
core keymap panel
```

## 12.9 配置

删除所有：

```text
legacy = true
compat = true
fallback = true
migration = true
```

如果这些配置只为旧架构存在。

## 12.10 测试

删除所有只测试旧行为的测试。

最终只测试正式行为。

---

# 13. P11.8 — App Package 完整生命周期 E2E

App Package 功能主体已经完成，本阶段只做最后闭环。

## 13.1 E2E 流程

必须覆盖：

```text
创建编辑区
↓
编辑资源
↓
运行
↓
导出 Package
↓
删除本地编辑区
↓
安装 Package
↓
运行 Installed Package
↓
Edit
↓
恢复到 Editable
↓
修改
↓
再次运行
↓
再次导出
```

## 13.2 资源类型

至少覆盖：

```text
scripts
functions
templates
keymaps
presets
resources
```

## 13.3 优先级测试

验证：

```text
Editable
    >
Override
    >
Installed
```

## 13.4 编辑恢复

Edit Installed Package：

```text
Installed
↓
PackageBuilder / Extractor
↓
Editable Workspace
```

确保：

- manifest 正确
- metadata 正确
- 资源完整
- hash 重新计算正确
- functions 不丢失
- templates 不丢失
- keymaps 不丢失

## 13.5 安装覆盖

因为不考虑兼容，可选择简单规则：

```text
同 package_id + version
→ overwrite / reinstall
```

不要为了历史包增加复杂迁移。

---

# 14. P11.9 — Architecture Guard Tests

这一阶段非常重要。

目标：防止以后重新把 YAML / Keymap 逻辑写回 Core。

## 14.1 Source Boundary Test

新增脚本扫描，Core 中禁止：

```text
Yaml
yaml_vnext
ScriptStore
script_id
KeymapStore
MappingRule
gamer.yaml specific type
```

允许字符串例外需要白名单。

## 14.2 Dependency Test

保证：

```text
core
```

不能依赖：

```text
gamer_yaml
gamer_keymap
```

但：

```text
gamer_yaml
→ core capability SDK
```

允许。

## 14.3 Extension Lifecycle Test

测试：

```text
install
enable
disable
uninstall
```

每一步验证：

- UI Contribution
- Runner
- Runtime
- Capability usage
- Task dependency state

## 14.4 Bare Core Test

新增：

```text
start gamer with zero extensions
```

必须可以：

- 启动 Server
- 打开 UI
- 连接设备
- 投屏
- 点击
- 拖动
- 基础键盘输入
- 查看任务
- 查看日志
- 修改设置

不能要求：

```text
gamer.yaml
gamer.keymap
```

存在。

## 14.5 YAML Isolation Test

没有 `gamer.yaml`：

```text
YAML Task
→ dependency missing
```

安装后：

```text
→ runnable
```

## 14.6 Keymap Isolation Test

没有 `gamer.keymap`：

```text
keyboard
→ passthrough
```

安装后：

```text
keyboard
→ mapping extension
→ DeviceAction
```

---

# 15. P11.10 — 文档最终收口

完成代码后，不保留旧设计文档作为“当前方案”。

## 15.1 README

更新：

```text
docs/gamer_refactor_plan_v2/README.md
```

将 Phase 11 标记：

```text
Breaking Architecture Cleanup
```

## 15.2 架构图

最终架构图中不能再出现：

```text
Native YAML
Native Keymap
Legacy Task
ScriptStore
KeymapStore
```

## 15.3 ADR

将之前：

```text
ADR-03 Runner Registry Deferred
```

改为：

```text
SUPERSEDED
```

由新的 Extension-Owned Runner ADR 替代。

## 15.4 删除兼容说明

文档中搜索：

```text
兼容
fallback
legacy
旧版
回退
migration
v2
native keymap
```

逐个确认。

最终架构文档只描述：

```text
现在是什么
```

而不是：

```text
以前是什么
怎么兼容以前
```

历史决策可以保留在 ADR history 中。

---

# 16. 推荐目录结构

完成后可以逐步向如下结构收敛：

```text
server/
├── core/
│   ├── device/
│   ├── input/
│   ├── run/
│   ├── timer/
│   ├── resource/
│   ├── package/
│   ├── extension/
│   └── capability/
│
├── api/
│
└── main.rs

extensions/
├── gamer-yaml/
│   ├── runtime/
│   ├── parser/
│   ├── runner/
│   ├── ui/
│   └── manifest
│
└── gamer-keymap/
    ├── runtime/
    ├── parser/
    ├── ui/
    └── manifest
```

不要求本阶段强制做物理 crate 拆分。

但逻辑依赖必须先满足：

```text
Extension → Core
Core -X→ Extension
```

---

# 17. 推荐分批提交

不要一个 Commit 完成整个 Phase 11。

推荐：

```text
refactor(task): replace script task with generic runner task
refactor(timer): add extension-owned runner registry
refactor(yaml): move yaml runner out of timer core
refactor(yaml): remove legacy yaml v2 runtime
refactor(resource): remove ScriptStore from core runtime
refactor(keymap): remove native keymap engine
refactor(ui): remove yaml and keymap core panels
refactor(api): unify resource and run APIs
chore(legacy): remove old task and package compatibility
test(architecture): add core-extension boundary guards
docs(v2): finalize phase 11 architecture
```

---

# 18. 开发检查清单

> 2026-09-05 收口回填：每条勾选项附证据（测试名 / 文件存在性 / grep 结果）；未勾项注明原因。
> 证据复跑基线：server `cargo test` 629 passed / `--no-default-features` 609 passed；web 705 passed。

## Task

- [x] 新 Task Model（`timer_core.rs:101 pub struct Task`：id/name/app(AppContext)/schedule/runner/state/metadata 语义齐备）
- [x] schedule.provider_id（`timer_core.rs:32`）
- [x] schedule.config（`timer_core.rs:33`）
- [x] runner.runner_id（`timer_core.rs:105`）
- [x] runner.entrypoint（`timer_core.rs:106`；HTTP 层嵌套为 `runner:{runner_id,entrypoint,payload}`，`api/mod.rs` tasks 组）
- [x] runner.payload（`timer_core.rs:107`）
- [x] 删除 script_id（Task/TaskSpec 零 script_id；grep `server/src/timer_core.rs` 无命中，残留仅为 `run_manager.rs` RunRecord 展示字段——遗留项 b）
- [x] 删除 cron 顶层字段（schedule JSON 统一 `{provider_id,config}`；`migrate_v2_to_v3` 归一存量列）
- [x] `/api/user-tasks` 删除（`api/mod.rs` 路由表无该路径；提交 d0ce2ab）
- [x] `/api/tasks` 替换（统一任务端点组 `api/mod.rs:247-262`：CRUD + run/suspend/resume/cancel/enable/disable）
- [x] TaskBoard 重写（`web/src/components/TaskBoard.vue`：provider/runner 双下拉 + RunnerEditorContribution；提交 832d774/9405084）

## Runner

- [x] TimerRunnerRegistry（`timer_core.rs` TimerRunnerRegistry：register/unregister/get/lookup/contains）
- [x] owner_extension_id（`RegisteredRunner.owner_extension_id`，异主抢注被拒）
- [x] register（`register_runner`，同 owner 同 id 原地替换=unclean restart seam）
- [x] unregister（`unregister_runner`，未知 id 报错防双重注销）
- [x] unregister_owner（`unregister_owner`，幂等返回被摘 runner ids）
- [x] missing dependency state（`TaskState::DependencyMissing`，任务保留且 enabled 原意不变）
- [x] extension lifecycle integration（`extensions/service.rs` TimerRunnerRegistrar 钩子；测试 `lifecycle_hooks_register_runners_on_start_and_disable_running_stops_them`）
- [x] 删除 YamlTimerRunner Native 构造（Core 模块零构造；registrar 由组合根 main.rs 经扩展边界接线，`Scheduler::new(db)` 裸核——ADR-13 组合根拥有注册表）

## YAML

- [x] parser 移出 Core（`extensions/gamer_yaml/script_v2/loader.rs`；`server/src/script_v2` 已删）
- [x] validator 移出 Core（`script_v2/validate.rs`）
- [x] AST / IR 移出 Core（`script_v2/model.rs`）
- [ ] 删除 script_v2 —— **未勾**：按奇偶报告结论 (c) 推迟（偏差①）。`server/src/script_v2` 物理移出 Core（守卫锁零残留），引擎本体暂留 `extensions/gamer_yaml/script_v2` 承载存量脚本/函数库，待 v3 缺口（G1-G5/R1-R5）清零后删除
- [x] 删除 YAML v2 fallback（版本猜测/自动转换/双入口分叉已删，唯一入口 `validate_compatible_script`（`extensions/gamer_yaml/resources.rs`），非 v3 即 v2、不猜测不转换）
- [x] 删除 ScriptStore（`server/src/scripts.rs` 已删；`resources.rs` 头注"ScriptStore / KeymapStore 消解后的内容无关资源层"）
- [x] functions 使用 ResourceResolver（`ResourceStore` 六目录寻址，`get_text(Functions,…)`；函数测试经统一 `/api/runs`）
- [x] scripts 使用 ResourceResolver（同上；`/api/apps/:app/resources/scripts`）
- [x] gamer.yaml 注册 Runner（`extensions/gamer_yaml/timer_yaml.rs` YamlTimerRunner + YamlTimerRunnerRegistrar）

## Keymap

- [x] 删除 KeymapStore（`server/src/keymaps.rs` 已删；keymaps kind 经 ResourceStore composite，包内方案只读）
- [x] 删除 Native Mapping Engine（提交 f126641：删除 KeymapRunner/InputGateway）
- [x] 删除 Native Mapping fallback（`extensions/keymap/mod.rs` 头注"the native mapping engine has been removed"；无扩展运行时 → 直通 scrcpy）
- [x] 删除 Host Keymap Panel（壳内零硬编码注册：提交 5e5e973；面板改由 gamer.keymap manifest `runtime="core"` + `component="console.keymaps"` 贡献，组件名解析表 `core-component-registry.ts` 为前端唯一知识）
- [x] gamer.keymap 自己贡献 UI（`tools/plugins/gamer.keymap/manifest.toml` `[[ui.contributions]]`）
- [x] 无插件 passthrough 正常（守卫 `architecture_guard_isolation_keymap_missing_extension_passes_input_through`）

## UI

- [x] Core 只保留任务（`workspace/core-contributions.ts` gamer.core:tasks）
- [x] Core 只保留日志（gamer.core:logs）
- [x] Core 只保留设置（gamer.core:settings）
- [x] 插件 Panel 来自 Contribution（manifest ui.contributions 驱动，提交 1d33f3d；`GET /api/extensions` ui_contributions）
- [x] gamer.yaml 自动化（manifest panel_id=automation → console.scripts）
- [x] gamer.yaml 函数（panel_id=functions → console.functions）
- [x] gamer.yaml 模板（panel_id=templates → console.templates）
- [x] gamer.keymap 映射（panel_id=keymaps → console.keymaps）
- [x] 删除特殊 plugin id 判断（`core-shell-boundary.test.js`："useConsoleWorkspacePanels 不做本地面板回退注册"、"runner 注册 id 唯一配置点在 gamer-yaml-runner.js"）

## API

- [x] Resource CRUD 泛化（`/api/apps/:app/resources/:kind[/:id]`，`api/resources.rs`；expected_version/force 乐观并发）
- [x] Run API 泛化（`POST /api/runs`，`api/runs.rs`，`{runner_id,entrypoint,payload,device_id}`）
- [x] 删除 script run API（`api/mod.rs:238-239` 注释：原 `/api/scripts/:id/run` 删除，经 Runner 注册表分发）
- [x] 删除 function run API（同上）
- [x] 删除重复 Task API（legacy `/api/tasks`（script_id+cron）与 `/api/user-tasks` 收口为统一任务端点组）
- [x] 删除 Preset alias（presets 只保留 `/api/task-presets`；`/api/tasks/presets` 已删）

## Legacy

- [x] 删除 file layout migration（提交 48d986f：file_migration 死代码删除）
- [x] 删除 package v1（`app_packages/manifest.rs`："缺少 format_version（当前仅支持 2）"，v1 直接不支持）
- [x] 删除旧 Snapshot 主流程（web 无 Snapshot 面板/页面残留；grep 仅插件中心卸载备份语义）
- [ ] 删除 legacy YAML —— **未勾**：同「删除 script_v2」（偏差①）。删除的是 fallback/版本分支与 Core 侧残留；v2 引擎本体在扩展内保留
- [x] 删除 native keymap fallback（见 Keymap 节）
- [x] 删除旧测试（提交 48d986f 旧行为测试残留清理；现存测试全部针对正式行为）
- [x] 删除兼容配置（`config.rs` grep legacy/compat/fallback 零命中）

## E2E

- [x] Bare Core（守卫 `architecture_guard_bare_core_serves_full_base_api_with_zero_extensions`：零扩展启动全基线 REST）
- [x] Install gamer.yaml（守卫 lifecycle 全链 install 步：只落盘、无 UI、无 runner）
- [x] Disable gamer.yaml（lifecycle 全链：disable 运行中=自动 stop，runner/UI 一并摘除）
- [x] Re-enable gamer.yaml（lifecycle 全链 start 再启：runner 重注册、任务自动恢复 Active；+ 隔离测试恢复分支）
- [x] Install gamer.keymap（`extensions/keymap/mod.rs` `real_keymap_gplugin_invokes_wit_and_native_capabilities`、`real_keymap_guest_consumes_user_profile_yaml`）
- [x] Disable gamer.keymap（直通分支由隔离测试覆盖（无运行时 → passthrough 不吞键）；disable 走同一 ExtensionService 状态机，无 keymap 专属 E2E）
- [x] Task dependency missing（守卫 lifecycle：stop → 任务转 dependency_missing 且保留，日志 missing_dependency=gamer.yaml）
- [x] Task dependency recovery（守卫 lifecycle：start 再启 → 任务自动恢复 Active）
- [x] Package export（`app_package_full_lifecycle_workspace_export_install_edit_rerelease` 十四步主链，`api/tests/app_packages_lifecycle.rs:514`）
- [x] Package install（同上；含同 id+version overwrite 重装分支）
- [x] Package edit（同上：Installed→Editable 整体提取，六类资源/manifest/hash 对账）
- [x] Package re-export（同上：编辑后再次导出复发布）

---

# 19. 最终验收场景

## 场景 A：裸 Gamer

```text
启动 Gamer
```

右侧只有：

```text
任务
日志
设置
+
```

可以：

```text
连接设备
投屏
点击
拖动
基础键盘操作
```

不存在：

```text
映射
自动化
函数
模板
```

## 场景 B：安装 gamer.yaml

安装：

```text
gamer.yaml
```

自动出现：

```text
自动化
函数
模板
```

RunnerRegistry：

```text
gamer.yaml = available
```

创建 Task：

```text
runner = gamer.yaml
entrypoint = daily/login
```

可以正常运行。

## 场景 C：禁用 gamer.yaml

UI：

```text
自动化
函数
模板
```

全部消失。

Runner：

```text
gamer.yaml
```

注销。

Task：

```text
DEPENDENCY_MISSING
```

但 Task 数据仍存在。

## 场景 D：重新启用 gamer.yaml

Runner 自动重新注册。

Task 重新进入：

```text
READY
```

不需要重新创建。

## 场景 E：安装 gamer.keymap

出现：

```text
映射
```

输入：

```text
Keyboard Event
↓
Keymap WASM
↓
DeviceAction
```

## 场景 F：禁用 gamer.keymap

映射 Panel 消失。

输入：

```text
Keyboard Event
↓
Core Passthrough
↓
Android
```

不会回落到 Native Mapping Engine。

---

# 20. Definition of Done

Phase 11 只有同时满足以下条件才算完成。

## Core 边界

```text
Core 不认识 YAML
Core 不认识 Script DSL
Core 不认识 Function DSL
Core 不认识 Keymap Rule
Core 不认识 gamer.yaml Runner 实现
```

## Extension 边界

```text
gamer.yaml 可独立启用 / 禁用
gamer.keymap 可独立启用 / 禁用
UI 随 Extension 生命周期变化
Runner 随 Extension 生命周期变化
```

## Task

```text
Task 与 YAML 无关
Task 与 Cron 无关
Task = ScheduleProvider + Runner
```

## Resource

```text
Core 只管理 Resource
Extension 解释 Resource
```

## Compatibility

```text
无 Legacy Task
无 YAML v2
无 Native Keymap fallback
无旧 App Package migration
无双 API
```

## UI

裸 Core：

```text
任务 | 日志 | 设置 | +
```

其它功能全部由插件贡献。

---

# 21. 最终原则

后续新增任何功能前，都可以用以下三个问题判断应该放在哪里。

### 问题 1

> 没有任何插件时，这个功能是否仍然成立？

如果否：

```text
Extension
```

### 问题 2

> Core 是否需要理解这个数据的业务语义？

如果不需要：

```text
Resource + Extension
```

而不是增加新的 Core Store。

### 问题 3

> 卸载插件后，这个功能是否应该消失？

如果是：

```text
功能实现、UI、Runner、Parser 都必须归插件所有。
```

不能只把名字挂到 Extension Registry，而实现仍留在 Core。

---

# 22. 推荐下一阶段状态

完成 Phase 11 后：

```text
Gamer V2 Architecture
=
Stable Core
+
Installable Extensions
+
App Packages
+
Generic Tasks
+
Generic Resources
```

此后再增加：

```text
Macro
OCR
Workflow
AI Agent
Custom Runner
Custom Scheduler
Image Processing
Game-specific Tools
```

都不应该再修改 Core 的业务模型。

理想状态是：

> 新功能主要通过 Extension SDK、Capability API、Resource API、Runner API 和 UI Contribution 完成。

当达到这一点，V2 插件化重构才真正完成。

---

# 23. Phase 11 收口报告（2026-09-05）

> 主链全部合入 main（收口提交 `417d732`）。门禁基线（收口当日复跑确认）：
> server `cargo test` **629 passed**（7 ignored）/ `cargo test --no-default-features` **609 passed**；
> web `pnpm test` **705 passed**（55 个测试文件）、`pnpm build` 通过。
> 执行波次：F0/B1/B2/F1/B3/W4-A/W4-B/W5-A/W5-B（提交区间 `a853df2..417d732`）。

## 23.1 §20 DoD 逐条对照

### Core 边界

| DoD 条目 | 结论 | 证据 |
|---|---|---|
| Core 不认识 YAML | ✅ | `server/src` 顶层无 YAML parser/AST/格式判别；script_v2/yaml_vnext/engine 仅存在于 `extensions/gamer_yaml/`；守卫 `architecture_guard_source_boundary_core_free_of_yaml_keymap_semantics`（67 条白名单双向校验，`assert_whitelist_alive` 防白名单腐化） |
| Core 不认识 Script DSL / Function DSL | ✅ | `scripts.rs`、`api/{scripts,functions,templates,keymaps}.rs` 文件已删；脚本/函数内容校验经 `YamlScriptValidator` / `YamlFunctionValidator`（ResourceKindHandler 回调） |
| Core 不认识 Keymap Rule | ✅ | keymap DSL 迁至 `extensions/keymap/dsl.rs`；依赖方向守卫 `architecture_guard_dependency_direction_core_never_paths_into_extension_internals` |
| Core 不认识 gamer.yaml Runner 实现 | ✅ | `Scheduler::new(db)` 裸核不预置 Runner；执行经 `TimerRunnerRegistry` 抽象（main.rs 组合根接线 registrar，属装配点而非 Core 模块） |

### Extension 边界

| DoD 条目 | 结论 | 证据 |
|---|---|---|
| gamer.yaml 可独立启用 / 禁用 | ✅ | 守卫 `architecture_guard_lifecycle_extension_full_chain_binds_ui_runner_and_tasks`：install→enable→start→stop→disable→uninstall 全链 HTTP 层走通 |
| gamer.keymap 可独立启用 / 禁用 | ✅ | `real_keymap_gplugin_invokes_wit_and_native_capabilities`（安装 .gplugin → WIT 派发 → capability 动作）；禁用/缺失运行时 → `dispatch_keymap_input` 直通 |
| UI 随 Extension 生命周期变化 | ✅ | UI 贡献随 enable 发布、disable/uninstall 摘除（lifecycle 守卫断言 `ui_contributions` 数量）；前端面板全 registry 驱动 |
| Runner 随 Extension 生命周期变化 | ✅ | TimerRunnerRegistrar 钩子：start 注册 / stop 注销 / disable=自动 stop；lifecycle 守卫显式断言 "enable 不注册 runner（enable ≠ start）" |

### Task

| DoD 条目 | 结论 | 证据 |
|---|---|---|
| Task 与 YAML 无关 | ✅ | `Task` 结构（timer_core.rs:101）零 script_id/YAML 字段；保存契约 `deny_unknown_fields` |
| Task 与 Cron 无关 | ✅ | 调度只是 `schedule={provider_id,config}` 不透明值；cron 是 provider_id=`cron` 的一个 provider（`cron_extension.rs`） |
| Task = ScheduleProvider + Runner | ✅ | `/api/tasks` 嵌套 JSON（schedule + runner）；`GET /api/runners`、`GET /api/schedule-providers` 列注册项 |

### Resource

| DoD 条目 | 结论 | 证据 |
|---|---|---|
| Core 只管理 Resource | ✅ | `resources.rs` ResourceStore：内容无关六目录寻址 + composite（EditableLocal>UserOverride>InstalledPackage）+ expected_version 乐观并发 |
| Extension 解释 Resource | ✅ | `ResourceKindHandler` 注册表：gamer_yaml 注册 scripts/functions 校验与模板 handler（含重命名引用改写）；keymap 方案语义在扩展侧 |

### Compatibility

| DoD 条目 | 结论 | 证据 |
|---|---|---|
| 无 Legacy Task | ✅ | legacy `tasks` 表 DROP（`migrate_v2_to_v3`）；`/api/user-tasks` 与 presets 别名已删 |
| 无 YAML v2 | ⚠️ 偏差① | v2 引擎本体暂留 `gamer_yaml` 扩展内部（奇偶报告结论 c，缺口 G1-G5/R1-R5）；已删的是 fallback/版本猜测/Core 侧残留——Core 零格式感知 |
| 无 Native Keymap fallback | ✅ | 提交 f126641 删除 KeymapRunner/InputGateway；无插件 = passthrough |
| 无旧 App Package migration | ✅ | `format_version=1` 直接拒绝（"当前仅支持 2"）；无 layout v1 迁移代码（file_migration 已删） |
| 无双 API | ✅ | 唯一入口：`/api/tasks`、`/api/task-presets`、`/api/runs`、`/api/apps/:app/resources`；scripts/functions/templates/keymaps 业务路由零残留 |

### UI

| DoD 条目 | 结论 | 证据 |
|---|---|---|
| 裸 Core = 任务\|日志\|设置\|+ | ✅ | `core-contributions.ts` 仅注册 `gamer.core:tasks/logs/settings`，`DEFAULT_PANEL_KEY='gamer.core:tasks'`；守卫 bare_core 测试 + `web/src/core-shell-boundary.test.js`（api.js 无业务 runner id、壳不做本地面板回退注册等断言） |
| 其它功能全部由插件贡献 | ✅ | gamer.yaml：自动化/函数/模板；gamer.keymap：映射——manifest `runtime="core"` 宿主组件（组件键为扩展知识，`core-component-registry.ts` 为前端唯一解析表）；declarative/iframe 两档照常可用 |

## 23.2 偏差清单（与计划原文的有意偏离）

1. **v2/v3 单格式化推迟**（§8.4/§12.3 原目标"只保留 YAML vNext"）：奇偶报告结论 (c)——v3 缺 G1-G5/R1-R5（见 [phase11_v2_v3_parity_report.md](phase11_v2_v3_parity_report.md)），v2 引擎暂留 gamer_yaml 扩展内部（唯一入口 `validate_compatible_script`），Core 零格式感知，不构成架构债；v3 缺口补齐后删 v2。
2. **同 id+version 安装 409→overwrite**（§13.5 授权）：App Package 同 package_id+version 重装按 overwrite 语义整体替换（stage-then-swap），是本阶段唯一对外行为修改；测试 `install_is_staged_and_same_version_reinstall_overwrites` 锁定。
3. **enable ≠ start**：Runner 注册点在 start（进入 Running）而非 enable——enable 只发布 UI 贡献，start 才注册 Runner；`reconcile_startup` 只恢复遗留 Running 记录。计划 §7.4 的"禁用即注销"落地为 disable=自动 stop。
4. **ADR 目录位置**：计划原文写 `docs/gamer_refactor_plan_v2/adr/`；随 docs 重组（reference/guides/plans/evidence 四子目录）实际落位 `docs/reference/adr/`（ADR-11~14 头部有位置说明）。

## 23.3 遗留项清单

- **a. v2/v3 单格式化**：v3 缺 G1-G5/R1-R5（见 [phase11_v2_v3_parity_report.md](phase11_v2_v3_parity_report.md)），补齐 guest 后删 v2 引擎。
- **b. `RunRecord.script_id` 字段命名**：schema v1 日志列名的历史命名，现为展示字段（run_manager.rs："retained as the legacy display field for the existing HTTP contract"），守卫白名单锁定；更名属数据迁移另案。
- **c. manifest 无 world/execution 声明字段**：gamer.yaml 的执行模型声明在 registrar（`timer_yaml.rs`）；加字段需同步 include_str! 锁与官方包重签。
- **d. `/api/runs` 前置存在性校验只读本地编辑区**：`ResourceStore::get_text` 非 keymap kind 不走 composite，纯包内脚本经统一入口返回结构化 not_found（真实运行走运行快照 composite 链路，E2E 已如实断言）。
- **e. resources 字节 kind（templates）上传共用 PNG 重编码管线**：任意原样字节需走 App Package 通道。
- **f. 前端分区候选读 store 资源形状**（`/api/apps/-/resources/...` 全量过滤）：宜加通用分区发现端点。
- **g. yaml 面板 composable 由壳无条件实例化**（`useConsoleScriptRunner` 等）：严格懒实例化待面板注册事件驱动。
- **h. data_schema fixture 批次按 release 节奏同步**（schema-policy 契约 §3 v3 行已补：提交 8289c5d）。
- **i. 升级注意**：升级后 Enabled-未-start 的 gamer.yaml 不自动注册 Runner（对账只恢复遗留 Running 记录），存量定时任务需手动 start 一次 gamer.yaml。
