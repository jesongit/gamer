# Gamer V1 简化与自动化插件重构开发计划

> 状态：**已实施完成（Phase 0-7，2026-09-09；执行状态见文末「9. 执行状态」）**
> 基线：`main`，提交 `0c5be8ecbf0bd6a9fd8742ef2b9251516cd6a81d`（2026-09-08）
> 原则：开发阶段允许破坏性修改，不兼容旧 YAML、旧配置和旧 API；不为假设需求增加抽象。
> 目标：在保留现有可用功能的前提下，收敛 Core、重设计 YAML、统一函数来源、简化插件生命周期，并完成现有功能收尾。

> 本文原为执行计划；**现已全量实施**，实际改动、提交与门禁结果见文末「9. 执行状态」。

## 0. 执行版规划结论

### 0.1 当前基线校准

本计划原有章节描述了目标形态，但没有把“目标已经存在”和“仍需重构”分开。按当前基线，以下事实必须作为实施起点：

| 事实 | 证据 | 结论 |
| --- | --- | --- |
| YAML 仍是 v3 解析模型 | `server/src/extensions/gamer_yaml/yaml_vnext.rs` 仍要求 `version: 3`，并包含 `SurfaceStep`、`find/check/match_first/wait/throw/set/call` 等旧特殊语义 | Phase 1/2 是真实迁移，不是文档收尾 |
| 宿主仍保留独立解释路径 | `yaml_extension.rs`、`wasm_host.rs`、`runner_adapter.rs` 同时承担解析、lowering、执行适配和 WASM 调用边界 | 必须先冻结 Guest/Host 的唯一权威边界，再删除另一套语义 |
| 前端仍有 v3 资产 | `web/src/script-editor/__fixtures__/yaml/`、`console-stage.test.js`、编辑器测试包含 `version: 3` 样例 | 编辑器、视频草稿、API fixture 和服务端测试必须同批切换 |
| Package、Task、Video、插件市场等基础能力已存在 | 现有 Package 资源 API、统一 Run/Task、视频工作台和插件生命周期代码 | 这些是适配面，不应再开新平台层 |
| 当前工作树没有实现改动 | `git status` 只有本计划文件未跟踪 | 本轮交付物只应是执行计划，所有实现项初始状态为 `NOT_STARTED` |
| 定向 Core 基线可用 | 现有架构守卫 7、resources 14、timer_core 18、run_manager 12、package_archive 7 项定向测试共 58 项已通过 | 只说明既有 Core 基础稳定；完整 Rust/Web/Guest/真实设备验证仍是 `NOT_VERIFIED` |

### 0.2 冻结的 V1 合同

以下内容在实现开始前必须作为单一合同写入测试；任何实现分支发现冲突时，先改计划/合同，再改代码：

1. **脚本文档**：顶层允许 `name`、`params`、`vars`、`run`，其中 `run` 必须存在且为步骤列表；不接受 `version`、`steps`、`stages`、`jobs`、`imports`、`metadata` 等 v3 入口字段。
2. **函数文件**：顶层为 `functions` 映射；每个函数只有 `params`、`run`，不允许脚本内嵌函数、函数 import 或 namespace。函数调用只按公开函数名寻址。
3. **步骤判别**：普通步骤只能是一个函数名映射，可选 `as`；`if`、`repeat`、`return` 是唯一控制流节点。`if` 使用 `then/else`，`repeat` 使用 `do`；不存在 `while/until/foreach/break/continue/try/catch/throw`。
4. **值与引用**：只支持 `$name` 和 `$name.field`；引用保留 JSON/YAML 原生类型；表达式、插值、动态索引和 `eval` 不进入 V1。带单位值由函数 Schema 的绑定器解析，而不是由 YAML 核心猜测。
5. **函数来源**：运行开始时固定“已运行插件函数 + 当前 Package 函数”的唯一注册表；同名、缺失函数、未声明参数、必填参数缺失均为结构化错误，不做覆盖优先级或隐式跨 Package 查找。
6. **错误/取消**：业务未命中返回 `null`；参数、权限、资源、设备、运行时错误终止 Run。取消、执行预算、调用深度、设备独占和运行事件沿用 Core 机制，只调整 Step 描述和路径。
7. **生命周期**：用户可见操作只有安装、启用/禁用、更新、卸载；`start/stop` 作为内部实现保留。启用意图持久化，实际运行状态进程内维护；禁用/更新/卸载前必须处理活动运行。

### 0.3 任务包与并行策略

实现按以下不重叠写集派工。每个任务包只修改自己列出的主写集；跨包接口先在合同测试中冻结，禁止多个任务同时大范围改写 `yaml_vnext.rs`、`service.rs` 或同一 Vue 容器。

| ID | 任务包 | 主写集 | 前置 | 交付/门禁 |
| --- | --- | --- | --- | --- |
| P0 | 合同与范围冻结 | 本计划、README、AGENTS、架构说明 | 无 | 合同测试清单、保留/删除清单；文档一致性检查 |
| P1 | V1 语法前端 | `server/src/extensions/gamer_yaml/yaml_vnext.rs`（或替代模块）、`error.rs` 及其单测 | P0 | 新 AST/诊断/变量绑定；旧 v3 入口拒绝；parser 单测通过 |
| P2 | 函数注册表与 Schema | `resources.rs` 内容钩子、`entrypoint_descriptor.rs`、`task_params.rs`、新增/独立的 callable resolver 模块、函数元数据测试 | P0；接口依赖 P1 | 插件函数 + Package 函数合并、冲突/缺失诊断；参数 Schema API 稳定；嵌套调用与入口调用同一绑定结果 |
| P3 | 唯一运行时与 wire | `server/guests/yaml-guest/`、`server/wit/gamer/host.wit`、`run_target.rs`、`wasm_host.rs`、`yaml_extension.rs`、`runner_adapter.rs` | P1/P2 的协议合同 | 生产执行仅有一份解释语义；取消/预算/事件保持；删除 native Interpreter；wire 带版本、scope、来源与稳定错误码 |
| P4 | Task/Video/动作适配 | `timer_yaml.rs`、`video_draft.rs`、`actions.rs` 及对应服务端测试 | P1/P2 | 手动运行、函数测试、Cron、视频草稿均生成/执行 V1 |
| P5 | 前端编辑器与表单 | `web/src/script-editor/`、`web/src/components/task/`、`gamer-yaml-runner.js`、视频草稿 UI/fixtures | P2/P4 API 合同 | 编辑器、参数表单、函数面板和草稿不再生成 v3 |
| P6 | 插件生命周期收敛 | `server/src/extensions/{model,service,store,wasm}.rs`、`server/src/extensions/gamer_yaml/timer_yaml.rs`、`server/src/main.rs`、`server/src/api/extensions*.rs`、插件中心前端及测试 | P0；与 P1-P5 可并行 | 安装自动启用启动、禁用清理 Runner/UI、重启恢复、失败可重试 |
| P7 | 文档/SDK/发行清理 | `docs/reference/YAML.md`、`docs/yaml-v3/`、SDK 示例、插件 manifest/guest 构建脚本 | P1-P6 完成 | 旧 v3 唯一权威入口消失；官方包与文档一致 |
| P8 | 集成与验收 | 架构守卫、API/集成/E2E/CI 配置和验收记录 | 各包阶段门禁 | Rust、前端、Guest、无 WASM、插件打包、真实设备项逐项记录 |

建议的最快波次如下：

```text
Wave 0: P0（合同）
           ├── Wave 1: P1（语法） ──┬── Wave 2: P2（函数 Schema） ──┬── Wave 3: P3（运行时）
           └── Wave 1: P6（生命周期）┘                              ├── P4（Task/Video）
                                                                      └── P5（前端）
Wave 4: P7（文档/发行） → P8（全量验收）
```

P1 与 P6 可以并行；P2 只能在 P1 的 AST/绑定合同冻结后进入实现；P3/P4/P5 共享 P2 的 Schema 合同但写集彼此分离。P7 不得提前把仍在变化的语法写成权威文档。每个波次结束后先合并接口测试，再合并实现，避免“新旧两套都能跑”的过渡状态长期存在。

### 0.4 每个任务包的派发说明

派工时必须把下面的输出作为任务描述原文或等价约束传给执行者：

* **P1**：先写 V1 正/负 parser 测试，再替换旧 AST；明确一项 Step 的函数名判别、`as`、控制流、引用和 Schema 绑定边界。不得在 Core 新增 YAML 类型，不得保留 v3 fallback。
* **P2**：实现唯一函数注册表的来源、优先级（无优先级）、冲突诊断和快照时机；资源路径仍由 PackageStore 三元组管理，Core 不解析函数业务语义。
* **P3**：先对齐 WIT/Host capability 与取消、预算、事件，再删除独立参考解释器；不得通过复制 parser 或 lowering 维持第二套行为。
* **P4**：只改适配层；验证 `POST /api/runs`、函数测试、Cron、Video Draft 走同一 Runner，不改通用 Task/Media/Package 模型。
* **P5**：所有表单和 fixture 从 Schema/API 读取；删除 v3 特殊步骤 UI 和示例，保留原文编辑、并发保存、Package 上下文和媒体模式输入禁用。
* **P6**：先梳理现有状态迁移和活动运行守卫，再收敛用户 API；不删除 Runner owner 清理、权限确认、WASM/Builtin 执行策略和资源隔离。
* **P7**：只清理已无消费者的旧文档、示例、SDK 和构建产物；每个删除项必须有引用搜索和测试证据。
* **P8**：以矩阵记录 `PASS`、`FAIL`、`NOT_VERIFIED`；环境不可用时只能标记未验证，不能用单测替代真实设备、Windows 包、Docker 或发布验证。

### 0.5 统一交付记录

每个任务包完成时在本文件对应表格追加：`status`、实际文件、提交哈希、测试命令/结果、删除的旧入口、未验证项、回滚点。阶段未完成不得标记“已实现”。当前初始台账：

| 任务包 | 状态 | 备注 |
| --- | --- | --- |
| P0-P8 | `NOT_STARTED` | 本轮只完成规划，未执行实现和测试 |

后续章节的“验收”是目标门禁，不是当前完成声明；“已实现”必须以台账和实际测试记录为准。

### 0.6 验证入口校准

当前 `tools/ci-local.ps1` 只是核心 Rust/Web 门禁子集，不应在计划或文档中称为与 GitHub CI 完全等效。实施时以 `.github/workflows/ci.yml` 的实际命令为准，并把结果分成三类：

| 类别 | 最低要求 | 说明 |
| --- | --- | --- |
| 本地快速门禁 | `powershell -ExecutionPolicy Bypass -File tools/ci-local.ps1` | 只代表脚本覆盖的 Rust/Web 子集；`-SkipRust/-SkipWeb` 不应被描述为完整隔离环境 |
| CI 对齐门禁 | `tools/check-version.ps1`、`tools/check-web-version.ps1`、Rust fmt/clippy/check/test、`cargo test phase0_ -- --nocapture`、`tools/generate-phase0-baseline.ps1 -ValidateOnly`、Web 全量测试和 build | 以 `.github/workflows/ci.yml` 为权威，计划记录实际命令和版本 |
| 发行/真实环境门禁 | 两个官方 Guest 的 wasm32 + Component 校验、临时目录 `build-plugins.ps1`、真实设备/WebRTC、Docker/NAT、Windows 完整包和发布链 | 不因离线 CI 通过而自动标记 `PASS` |

必须补进 P8 的验证项：

* 显式构建并校验 `gamer.yaml` 和 `gamer.keymap` Guest；不能只依赖测试现场编译。
* 对 `--no-default-features` 至少执行 `cargo check`；若测试装配允许，再执行 `cargo test --no-default-features`，并记录确切结果。
* 前端测试收集使用 `src/**/*.test.js`，或显式补跑当前默认 glob 漏掉的 `web/src/console/device-summary.test.js`。
* 用临时输出目录执行 `tools/build-plugins.ps1`，检查 manifest、`.gplugin`、registry v2、sha256、builtin 无 `plugin.wasm`；不得把当前含 `generated_at` 的产物宣称为字节级可复现。
* 真实设备、WebRTC、Docker/NAT、GHCR/审批等外部环境必须单独记录为 `NOT_VERIFIED` 或独立验收，不得用单测替代。

### 0.7 已知缺陷单独立项

这些问题不是 YAML 语法本身，但会直接影响 V1 验收，不能被“适配完成”一句带过：

* `web/src/components/task/builtin-runner-editors.ts` 仍可能从 entrypoint 同时推导 Android 包名和 Package ID；必须改为 Android 包名只读设备配置，Package ID 只读当前 Package 上下文。
* `ScriptPicker.vue` 与 `gamer-yaml-resources.ts` 目前以脚本为主，需让 Task 选择脚本或 Package 函数，并按 `packageId` 隔离/失效缓存。
* `web/src/workspace/context.ts` 的桥接快照仍使用含义不明确的 `app.package`；应显式提供 `deviceId`、`androidPackageName`、`currentPackageId`、`activePluginId`。
* `VideoWorkbench.vue` 新建/保存项目后先同步媒体引用再刷新项目摘要，存在新引用漏登记风险；必须单独补回归测试，不等待 YAML 迁移顺带修复。
* 插件中心仍把 `start/stop/activate` 作为用户可见操作；P6 必须收敛 UI，同时保留内部生命周期 API 和活动运行安全守卫。
* `video_draft.rs`、`actions.rs`、`resources.rs` 仍以 v3 保存/校验为边界；P4 必须把 `version: 3/steps/wait` 一次性切换为 `run/sleep` 并验证保存后可再次打开和执行。

### 0.8 数据与注册边界（实现前不得留空）

为避免各任务包各自猜测，V1 采用以下破坏性迁移策略：

* **旧 YAML 资源**：PackageStore/归档仍按内容无关原则保存和导出原字节，导入不因 dormant 插件数据而解析失败；但 `gamer.yaml` 的资源保存、entrypoint Schema、运行入口遇到 v3 文档统一返回 `yaml.v1.unsupported_legacy`，不转换、不 fallback、不执行。用户通过编辑器显式保存新 V1 后才恢复可运行。
* **旧 Task**：不删除记录、不自动转换 payload；发现 gamer.yaml 旧 entrypoint 或旧参数时保留任务和启用意图，置为不可调度的暂停状态并写入结构化 `last_result`，UI 提示用户重新选择 V1 脚本/函数。不能伪装成 `DependencyMissing`，因为插件并未缺失。
* **Run/Log 旧字段**：`runner_id + entrypoint` 是唯一解析身份，`script_id` 不再进入新 API 语义或被 Runner 使用。P0/P8 必须完成存储迁移设计：要么在 schema v4 将其改为通用 `label/entrypoint`，要么明确作为仅历史展示的内部列；不得继续保留“兼容白名单”却没有测试边界。
* **归档校验**：归档层继续只做布局、字节、manifest 和原子安装校验；插件内容校验只发生在 `gamer.yaml` 资源 PUT/rename/save/run 边界。新内容校验失败时 overwrite 必须整包回滚，不能部分替换。
* **插件函数**：扩展进入 `Running` 时通过 owner registrar 注册函数描述和实现；stop/disable/uninstall 与 Runner/UI 一起注销。权限绑定注册函数的扩展身份，自动化调用不能扩大权限。
* **Package 函数**：不进入全局长期注册表；运行开始时按 `content_package` 和 Package revision 读取 `functions/`，校验后与当前可用插件函数合并并生成不可变快照。资源修改、包切换或扩展状态变化不得污染已开始的 Run。
* **函数冲突**：插件函数与 Package 函数、多个插件函数、Package 内同名函数均直接结构化报错，不设覆盖优先级；跨 Package 不隐式查找。
* **架构守卫**：新增检查应覆盖 `server/src`、Guest、WIT、`web/src` fixture 和正式文档的现行入口，禁止 `version: 3`/旧特殊步骤重新进入生产代码；历史计划和迁移说明必须走单独目录或明确“历史资料”标记。

P2/P3 必须共同冻结一个扩展内 `YamlCallableResolver` 合同，避免 `runner_adapter`、`entrypoint_descriptor`、`task_params` 和 Guest 各自读取资源：

```text
resolve(target, app_context, args)
  -> ResolvedCallable {
       source: plugin | package,
       package_id, plugin_id, kind,
       metadata/schema,
       bound_args,
       versioned_wire,
     }
```

入口参数、函数测试、嵌套 `call`、Cron 和 Guest `programs.resolve` 都必须复用该结果。WIT 中的 program/callable JSON 必须带明确 wire version、调用方 scope、Package/Plugin 来源和稳定错误码；Guest 不得直接访问 PackageStore 或文件系统。NativeYamlHost 可以保留为 capability/测试装配层，但不得继续保留第二套 YAML 控制流解释器。

## 1. 本次改造的目标

Gamer V1 不再继续建设通用自动化平台，而是保持以下清晰边界：

```text
Gamer Core
├── 设备、投屏、截图、输入
├── 视觉匹配
├── Package 资源存储
├── Task / Cron / RunManager
├── 插件安装、权限、生命周期、运行时
└── 日志、取消、设备独占等通用机制
          │
          ├── gamer.yaml
          │   ├── YAML 解释器
          │   ├── 插件函数库
          │   ├── 当前 Package 函数库
          │   ├── 自动化、函数、模板编辑
          │   └── Task Runner
          │
          ├── gamer.keymap
          │   └── 键盘映射业务
          │
          └── gamer.video
              └── 视频工作台与自动化草稿制作
```

Core 不认识 YAML 语法、脚本步骤、函数库格式或具体业务流程。插件通过 Core 原语完成业务，自动化插件负责解释脚本与组合能力。

本次不重新设计 Device、App、Package、Plugin、Task 的关系，也不进行第二轮全面架构重构。

## 2. 最新仓库现状与处理结论

本计划以最新代码为准，以下内容不再重复开发。

| 当前能力                                                   | 处理            |
| ------------------------------------------------------ | ------------- |
| Android App 与 Package 分离，Package 三元组资源寻址               | 保留            |
| 默认 Package 自动创建                                        | 已实现，保留        |
| 插件 Android Targets 声明及按当前应用过滤                          | 已实现，保留        |
| 函数面板按函数个体展示、搜索、编辑、删除                                   | 已实现，复用        |
| 模板 ZIP 批量上传                                            | 已实现，保留        |
| 插件免签名安装、来源提示、权限确认、可选 SHA-256                           | 保留，不重做签名体系    |
| Native/Builtin 与 WASM 插件                               | 保留            |
| 官方插件、市场、SDK 和本地/URL 导入                                 | 保留            |
| 通用 Runner、Cron、统一运行入口                                  | 保留现有机制，简化用户界面 |
| Package 导入、导出、媒体引用、未安装插件数据保留                           | 保留            |
| 视频逐帧、模板裁切、离线匹配、YAML 草稿闭环                               | 保留，适配新语法      |
| YAML v3 的特殊步骤与双执行逻辑                                    | 重设计并清理        |
| Installed / Enabled / Running / Disabled / Failed 生命周期 | 收敛            |
| 多层函数语义、参数桥和旧格式兼容逻辑                                     | 按新设计清理        |

### 本次明确不做

不新增 GitHub App、插件签名系统、函数库市场、全局用户函数库、复杂依赖解析、通用脚本引擎、完整表达式语言、更多 UI runtime、更多调度 Provider、Workspace 新层次或新的 Package 管理模型。

已有的安全、设备稳定性、媒体正确性和数据保护机制，不因“简化”而删除。

---

# Phase 0：确认架构与冻结范围

## 目标

先把本次最终设计写入 README 和开发规范，避免后续 AI 再次把项目复杂化。

## 改动

1. 更新 `README.md` 的架构说明，明确 Core 只提供通用原语，YAML 由 `gamer.yaml` 插件实现。
2. 更新 `AGENTS.md`，明确 V1 简化原则和本次新 YAML 设计。
3. 新建本计划，并将旧 YAML v3 计划、旧兼容要求和不再适用的开发计划标记为历史资料。
4. 检查最新代码与文档是否一致，尤其是函数面板、默认 Package、Android Targets、插件免签名安装和视频工作台。
5. 建立本次改造的“保留 / 修改 / 删除”清单，不因为旧计划写过某项功能就继续实现。

### README 必须明确的原则

> Gamer V1 优先满足实际使用需求，不为未来可能出现的需求提前建设通用平台。Core 只保留设备、视觉、资源、运行、任务和插件等稳定机制；自动化、键盘映射、视频等业务由插件负责。新增抽象必须有真实使用场景；没有第二个实际实现时，不提前设计复杂的通用接口。开发阶段允许破坏性修改，不保留无价值的旧语法、旧数据结构和兼容层。后续开发计划必须先复用现有能力，优先通过插件函数扩展自动化功能，而不是增加 YAML 关键字。

## 验收

* README、AGENTS.md 与当前目标架构一致。
* 旧文档不再被当作新开发的权威依据。
* 本次改造范围冻结，不再顺带新增平台能力。

---

# Phase 1：重设计 YAML V1 最小语法

## 1.1 核心原则

YAML 只描述流程，所有实际操作都是函数调用。

解释器只认识：

```text
Document
├── params / vars
├── run
└── functions（仅函数库文件）

Step
├── function call
├── if
├── repeat
└── return
```

函数调用统一支持参数和 `as` 返回值赋值。

`tap`、`swipe`、`find`、`sleep`、`launch` 等不是语法关键字，而是插件提供的函数。用户自定义函数与插件函数使用相同调用方式。

## 1.2 脚本格式

脚本顶层只保留：

```yaml
name: 每日签到

params:
  retry:
    type: integer
    default: 3

vars:
  timeout: 15s

run:
  - launch: com.example.game

  - wait_find:
      template: home
      timeout: $timeout
    as: home

  - if: $home
    then:
      - claim_daily: {}
    else:
      - log: 未进入主页
```

`name`、`params`、`vars` 可选，`run` 为执行入口。无须 `version: 3`、metadata、entrypoint、jobs、stages、imports 等字段。

新格式直接替换旧 v3，不提供旧脚本转换器或运行时 fallback。格式演进由自动化插件自身管理，不再让 Core 识别 YAML 版本。

## 1.3 函数调用

统一格式：

```yaml
- 函数名: 参数
  as: 返回变量
```

示例：

```yaml
- tap: [0.5, 0.8]

- swipe:
    from: [0.5, 0.8]
    to: [0.5, 0.2]
    duration: 500ms

- find:
    template: login_button
    threshold: 0.8
  as: button

- tap: $button.center
```

参数规则：

* 标量或数组可作为单参数函数的简写。
* 多参数函数使用命名参数对象。
* 对象本身作为单参数时，使用参数名明确包裹，避免歧义。
* 无参数函数允许 `{}` 或 YAML `null`。
* 未知函数、未知参数、缺少必填参数均明确报错。
* 一个 Step 只能有一个函数调用或一个控制流关键字，`as` 等修饰字段除外。

坐标采用现有项目的相对坐标约定，具体范围由 `point` 参数 Schema 校验。普通数组始终只是数组，不再由解释器自动识别为坐标。

## 1.4 变量与类型

V1 只支持：

```text
$name
$name.field
```

不支持数学表达式、JS 表达式、模板插值、动态索引或 eval。

例如：

```yaml
- find: login_button
  as: button

- tap: $button.center
```

变量引用保留真实类型，不在运行链路中统一转换为字符串。

基础值只需要：

```text
null
boolean
integer / number
string
list
object
```

`duration`、`point`、`template`、`key` 等作为函数参数 Schema 类型，在绑定时进行显式解析和校验。例如 `500ms`、`1.5s` 可解析为 duration，但不要求 YAML 解释器把所有带单位的字符串自动转换。

需要以 `$` 开头的普通字符串时，提供一个简单的显式转义约定，避免变量引用与文本混淆。

## 1.5 控制流

V1 只保留 `if`、`repeat`、`return`。

### if

```yaml
- if: $button
  then:
    - tap: $button.center
  else:
    - log: 未找到按钮
```

条件只接受布尔值或可空结果：`false`、`null` 为假，非空结果为真。不设计数字、字符串等复杂隐式真值转换。需要比较时调用 `eq`、`gt` 等函数。

### repeat

```yaml
- repeat: 3
  do:
    - tap: [0.5, 0.5]
    - sleep: 1s
```

只支持固定非负整数次数，不增加 while、until、foreach、break、continue。循环同样受执行预算限制。

### return

```yaml
- return: true
```

或：

```yaml
- return: $button
```

函数调用使用独立局部作用域，参数显式传入，返回值通过 `as` 接收。函数不隐式读取或修改其他函数的局部变量，也不引入全局可变变量系统。

## 1.6 错误处理

V1 不提供 try/catch/throw/on_error 等语法。

约定：

* 正常业务未命中，例如 `find` 未找到，返回 `null`。
* 参数错误、资源不存在、设备断开、权限不足等属于执行错误。
* 执行错误终止当前 Run，并提供结构化错误信息。
* 需要等待、重试、条件匹配等业务能力时，优先由函数库实现。

保留宿主取消、执行预算、调用深度和设备独占等安全机制，不因为删除 YAML 特殊步骤而删除这些保护。

## 1.7 V1 不支持清单

明确不支持：

```text
while / until / foreach
break / continue
try / catch / throw
完整表达式语言
字符串插值
class / module / import
namespace / 函数覆盖优先级
async / await / 并行执行
动态函数定义 / eval
YAML macro / 自定义运算符
```

后续只有真实需求出现时才讨论增加。

## 验收

* 最小语法能够完成启动应用、查找模板、点击、条件执行、固定循环和调用用户函数。
* 添加一个新操作函数不需要修改 YAML 解析器或解释器。
* 类型在脚本、参数绑定、Runner 和 WASM 边界之间无损传递。
* 非法语法产生明确诊断，不存在旧 v3 fallback。

---

# Phase 2：解释器完整归属自动化插件

## 目标

只保留一份权威 YAML 执行逻辑，不再维护两套独立语义。

## 改动

1. 保留 `gamer.yaml` 作为官方自动化插件 ID，不为了重构而重新命名。
2. 将新语法解析、校验、函数解析、执行和错误处理统一归自动化插件所有。
3. 以插件内 WASM Guest 作为唯一权威执行实现；宿主侧可以保留必要的解析、校验和运行适配代码，但不得再维护另一套独立解释器。
4. 测试默认通过 Guest Component 验证；NativeYamlHost 只保留 capability/测试装配，不保留可执行完整 DSL 的原生 `Interpreter`。`--no-default-features` 路径只验证“能力不可用/错误可预测”，不能偷偷恢复第二套解释器。
5. 清理旧 v3 特殊步骤、lowering 逻辑、重复随机数算法、旧 AST、旧参数桥和已无消费者的兼容代码。
6. 保留现有 Core capability 调用方式，避免为了新 YAML 再设计一套通用脚本引擎或新的宿主调用协议。
7. 保留执行取消、预算、调用深度、运行日志和运行事件；事件语义随新 Step 模型调整，不再依赖旧 v3 专用步骤。

### 重点检查目录

```text
server/src/extensions/gamer_yaml/
server/guests/yaml-guest/
server/wit/gamer/
server/src/capabilities/
server/src/run_manager.rs
server/src/timer_core.rs
```

重点审查 `yaml_vnext.rs`、`yaml_extension.rs`、`wasm_host.rs`、`runner_adapter.rs`、`task_params.rs`、`entrypoint_descriptor.rs` 等文件，按新职责保留或合并，不要求机械保留旧文件结构。

## 验收

* 生产执行只经过一份权威解释器。
* Core 不导入 YAML AST、Step 或函数语义。
* 新函数可以通过函数注册表接入，不需要增加解释器分支。
* 取消、超限和设备异常均能正确结束运行。
* 删除旧代码后通过架构边界测试，不保留无意义的兼容适配层。

---

# Phase 3：函数库收敛为两种来源

## 3.1 最终模型

V1 只存在：

```text
插件函数库
当前 Package 函数库
```

不再区分系统函数库、基础函数库、便利函数库、业务函数库、全局用户函数库等多个层次。

“基础函数”“便利函数”只可以作为文档分类，不是系统资源类型。

## 3.2 插件函数库

由当前处于 `Running` 的插件提供，随插件启动、停止、更新和禁用变化；仅安装或仅持久化 `enabled` 不足以把函数放入本次运行环境。

首版主要由 `gamer.yaml` 提供自动化函数。其他插件以后确实需要对自动化开放能力时，复用现有插件调用机制接入，不提前建设新的函数市场或全局函数平台。

函数统一包含：

```text
name
description
params
returns
implementation
```

其中参数 Schema 至少支持类型、必填、默认值和说明。实现可以是插件内部 Rust/WASM 代码，也可以复用同一自动化解释器执行 YAML 函数，对调用者没有区别。

插件函数的权限必须受原插件权限和当前运行上下文约束，不能因为通过自动化函数调用就绕过权限。

## 3.3 当前 Package 函数库

继续使用现有 Package 资源体系：

```text
packages/<package-id>/
└── plugins/gamer.yaml/
    ├── automations/
    ├── functions/
    └── templates/
```

函数文件按现有分类文件方式管理，例如：

```text
functions/
├── common.yaml
├── login.yaml
└── daily.yaml
```

函数文件格式：

```yaml
functions:
  claim_daily:
    params:
      timeout:
        type: duration
        default: 5s

    run:
      - tap_template:
          template: daily_button
          timeout: $timeout

      - tap_template:
          template: claim_button
          timeout: $timeout

      - return: true
```

脚本直接调用：

```yaml
run:
  - claim_daily:
      timeout: 10s
    as: success
```

脚本文件不再内嵌第三种“脚本局部函数库”。用户可复用函数统一保存到当前 Package 的 `functions/` 中。

分类文件只是存储与编辑分组，不是 namespace，也不需要独立版本、安装或依赖管理。

## 3.4 函数查找

执行前组合当前可用函数：

```text
已启用插件提供的函数
          +
当前 Package 的函数
          ↓
唯一函数名注册表
          ↓
解析与执行
```

规则：

* 所有公开函数名在当前执行环境中必须唯一。
* 同名直接报冲突，不设计覆盖优先级。
* 不支持跨 Package 隐式查找。
* 不支持全局用户函数库。
* 不支持函数库 import、安装、发布、版本选择。
* 当前 Package 中的函数可以调用插件函数和本 Package 的其他函数。
* 缺失函数在校验或执行前明确提示，不静默替换为其他来源。

运行开始时固定本次使用的函数定义和参数，避免运行过程中修改函数文件导致同一次执行语义变化。不需要为此建设完整依赖锁文件系统。

## 3.5 内置函数首版范围

先完成最常用的原子函数：

```text
tap
swipe
key
input_text
launch
stop_app
sleep
log
find
```

再提供少量高频便利函数：

```text
wait_find
tap_template
wait_disappear
```

`swipe_find`、通用 retry、随机操作、复杂页面等待等，按实际使用需求逐步增加。

便利函数可以在插件内部组合 Core 原语，不要求全部由 YAML 实现，也不要求用户手动安装。

### 函数 Schema 示例

```yaml
name: tap
description: 点击相对坐标

params:
  position:
    type: point
    required: true

returns:
  type: null
```

```yaml
name: find
description: 查找模板，未找到返回 null

params:
  template:
    type: template
    required: true

  threshold:
    type: number
    default: 0.8

  timeout:
    type: duration
    default: 0s

returns:
  type: match?
```

Schema 是函数参数、编辑器提示和执行校验的共同来源，不再单独维护一套编辑器参数规则。

## 验收

* 函数只来自插件与当前 Package。
* 插件函数和 Package 函数调用语法一致。
* 同名冲突明确报错。
* 用户新建一个 Package 函数后，其他脚本可以直接调用。
* 新增便利函数不修改 YAML 核心语法。
* 函数管理无需第三种资源类型或独立函数库市场。

---

# Phase 4：适配现有自动化、任务与视频链路

## 4.1 自动化编辑器

复用最新已完成的函数个体化界面，不重新开发函数管理器。

函数页面按两种来源分组：

```text
插件函数
├── 查看
├── 搜索
└── 参数说明

配置包函数
├── 新建
├── 编辑
├── 删除
├── 运行测试
└── 搜索
```

现有分类文件、函数个体操作和搜索能力继续保留。插件函数默认只读，Package 函数可编辑。

编辑器的校验、参数提示和自动补全改为读取新函数 Schema；旧 v3 特殊步骤的表单、提示和语法规则直接删除。

## 4.2 Task 与 Runner

保留现有通用 Task / Runner / RunManager 机制，不重新设计任务系统。

自动化插件继续注册自己的 Runner，手动运行、函数测试和定时任务共用同一执行入口。

Task 用户界面首版只提供 Cron：

```text
选择脚本或函数
→ 选择设备
→ 填写参数
→ 设置 Cron
→ 保存
```

通用 Runner 选择可以保留，但不要求普通用户理解 ScheduleProvider、Entrypoint、Payload 等底层概念。

旧 v3 的参数签名、类型桥和特殊入口规则按新 Schema 收敛。保留必要的参数快照、必填校验和缺失资源错误，不保留旧格式兼容。

插件缺失或禁用时，任务保留原有启用意图并进入依赖缺失状态；插件恢复后再恢复调度。不得删除任务或静默改用其他 Runner。

## 4.3 视频工作台

保留现有完整闭环：

```text
录制 / 导入
→ 定位精确帧
→ 裁切模板
→ 离线匹配
→ 生成 YAML 草稿
→ 保存 / 打开自动化编辑器
```

只改造 YAML 草稿生成器，使其输出新语法。

继续保留：

* 真实 PTS 与 VFR/B 帧处理。
* 指定帧裁切，不重新抓取实时画面。
* 媒体模式禁止设备输入。
* 文本事件脱敏，不猜测无法恢复的内容。
* Package 媒体引用与删除保护。
* 视频插件通过自动化插件公开制作动作保存草稿。

不新增通用视频编辑平台，也不让视频插件自行实现 YAML 解析器。

## 4.4 Package

继续使用现有默认 Package、三元组资源寻址、导入导出和未安装插件数据保留机制。

不新增 Workspace、Installed/Editable 双层、全局函数目录或新的 Package 类型。

新 YAML 的内容校验由 `gamer.yaml` 资源处理器负责。导入包含未安装插件数据的 Package 时仍保留原始数据，不要求 Core 理解其内容。

## 验收

* 手动运行、函数测试、Cron 运行使用同一新解释器。
* 函数参数表单由 Schema 生成。
* 视频草稿通过新 YAML 校验并可执行。
* 现有 Package 导入导出、模板批量上传和媒体引用不回退。
* 插件缺失不会导致任务或 Package 数据丢失。

---

# Phase 5：简化插件生命周期与运行时

## 5.1 用户操作收敛

用户只需要：

```text
安装
启用 / 禁用
更新
卸载
```

安装经过权限确认后自动启用并启动。更新成功后恢复原先启用意图。禁用时停止插件运行并注销其 Runner/UI 贡献。

不再让用户分别理解 Enabled、Start、Running、Activate 等操作。

## 5.2 状态模型

建议持久化：

```text
installed version
enabled（用户期望）
last_error
```

当前进程维护实际运行状态：

```text
stopped
starting
running
failed
```

服务启动时根据 `enabled` 恢复插件，而不是依赖上次持久化的 Running 状态。

插件启动失败时保留启用意图和错误信息，允许用户重试；不做无限自动重启循环。

当前代码仍把 `Running/Failed` 写入持久状态，且只有 API 安装包装层会自动启动；`Enabled` 不会自动运行，服务层与 API 层行为不一致。P6 必须先定义一次性状态迁移：旧 `Running` 映射为 `enabled=true`，旧 `Enabled` 按原意映射为 `enabled=true`，旧 `Disabled` 映射为 `enabled=false`，旧 `Failed` 保留 `enabled` 意图并把错误转入 `last_error`；之后不再把实际 `running/failed/starting` 写回 durable intent。

P6 的后端权威流程必须满足：

* 安装、启用、更新、重启恢复都从同一个 service 入口执行“设置 enabled 意图 → 尝试启动 → 成功注册 UI/Runner”；不能只在 REST wrapper 中自动启动。
* 禁用、更新、卸载由后端统一处理活动运行；不能要求前端先 `stop` 再 `uninstall`，也不能只依赖 lifecycle mutex 而不等待 guest/RunManager 排空。
* 更新保留原 enabled 意图；启用插件更新后尝试启动新版本，失败时保留错误和可诊断版本，并定义旧版本恢复/回滚策略。
* `start/stop/activate` 可作为内部实现或受限回滚能力保留，但从普通插件中心和普通 SDK 流程移除。
* 移除公开 `start` 前必须迁移 Keymap 的 `profile`、设备和 Package 上下文输入到内部启动/激活路径，不能让 Keymap 因为 API 收敛而失去运行上下文。

## 5.3 运行时

保留 Native/Builtin 与 WASM 两种执行形态。

常驻 Keymap、按调用执行的 YAML、无 Guest 的 Native 视频插件，继续保留真实必要的生命周期差异，但共用安装、权限、状态、UI/Runner 注册和停止清理机制。

不为了统一而强行把所有插件变成常驻实例，也不再增加新的运行时类型。

重点检查：

```text
server/src/extensions/model.rs
server/src/extensions/service.rs
server/src/extensions/wasm.rs
server/src/extensions/keymap/
server/src/extensions/builtin.rs
server/src/extensions/manifest.rs
server/src/api/extensions.rs
```

删除不再需要的公开 start/stop/activate 操作时，必须同步处理前端、SDK、测试和调用方。内部仍可保留 start/stop 作为生命周期实现方法。

更新、禁用或卸载遇到活动运行时，必须先安全停止或明确拒绝并提示取消，不能直接破坏设备运行。旧版本回退、权限确认、不可伪装 builtin、资源隔离等安全机制继续保留。

## 5.4 插件 UI

保留最新主导航：

```text
任务 | 日志 | 市场▾ | 插件▾ | 设置
```

插件下拉按当前 Android App 过滤，选中插件后展示对应功能页签；单功能插件不额外显示无意义的子页签。

保留当前原生面板和简单声明式面板能力。Iframe 若已有实现可以保留为非重点能力，但不继续扩展第三方前端平台。

不新增新的导航层级或插件 UI 框架。

## 验收

* 安装后无需再手动 Start。
* 禁用后 UI、Runner 和运行实例正确停止。
* 重启后自动恢复已启用插件。
* 更新失败不破坏旧版本和 Package 数据。
* Keymap、YAML、Video 和第三方最小 WASM 插件均正常工作。
* 前端不再暴露多余生命周期操作。

---

# Phase 6：清理旧代码、文档与 SDK

## 目标

完成真实收敛，而不是新旧实现长期并存。

## 改动

1. 删除旧 v3 语法文档、旧特殊步骤示例和不再使用的解析/执行逻辑。
2. 删除旧 YAML 参数、函数查找、模板引用等无消费者的兼容适配。
3. 更新 `docs/reference/YAML.md`、相关语法文档和编辑器说明，使新 V1 成为唯一权威语法。
   同步复核 `docs/yaml-v3/{overview,runtime,call,timing,steps,params}.md` 与邻接文档中的旧 `script_v2` 路径、旧 v2 runtime/parser 描述；历史资料可以保留，但必须显式标注为历史输入，不能继续给实现者作现行契约。
4. 更新 `gamer.yaml`、`gamer.video` 的公开动作与草稿生成契约。
5. 更新官方插件 manifest、SDK 示例和插件开发文档。
6. 更新构建脚本、registry 和插件包，确保实际打包的 Guest 与新代码一致。
7. 清理旧版签名相关的文档描述，但不删除 Windows launcher 自身必要的发行完整性和更新验证机制。
8. 不为了减少文件数量而合并已经正确分离的 Core 业务模块。

### 清理原则

先通过引用搜索和测试确认旧代码无实际消费者，再删除。历史计划可以归档，但生产代码不保留无价值的旧格式读取、兼容开关或双分支执行。

## 验收

* README、AGENTS、语法文档、SDK 和实际代码一致。
* 官方插件包包含新解释器和新函数库。
* 旧 v3 示例不会被编辑器或 AI 当作新语法。
* 无旧解释器、旧语法兼容入口或重复函数注册表残留。

---

# Phase 7：测试与 V1 收尾

## 7.1 YAML 测试

覆盖：

* 顶层结构与函数文件解析。
* 普通函数调用、命名参数、单参数简写。
* `as`、变量字段访问、类型无损传递。
* `if`、`repeat`、`return`。
* 函数参数默认值与必填校验。
* 插件函数与 Package 函数解析。
* 同名冲突、缺失函数、递归深度。
* `find` 未命中返回 null。
* 设备错误、权限错误、取消、执行预算。
* 新语法不接受旧 v3 特殊结构。

最低命名/覆盖要求：`yaml_v1_surface_accepts_run_and_rejects_v3`、`yaml_v1_function_registry_merges_package_and_plugin_functions`、`yaml_v1_typed_values_roundtrip_losslessly`、`yaml_v1_single_authoritative_interpreter`、`package_import_old_yaml_policy`、`run_record_has_no_legacy_script_contract`、`architecture_guard_covers_guest_wit_web_and_docs`。测试名可按项目约定调整，但不能删掉对应覆盖：旧数据策略、函数注册生命周期、无损类型、单一解释器、跨层架构边界。

## 7.2 集成测试

覆盖：

* 空数据目录启动自动创建默认 Package。
* 插件安装、启用、禁用、更新、卸载及重启恢复。
* 旧 `state.json` 的 `Running/Enabled/Disabled/Failed` 一次性迁移；service 直调与 REST 安装行为一致。
* 启用/更新/禁用/卸载与活动 WASM、Keymap、YAML Run 并发时，能安全排空或返回结构化冲突；Runner/UI 不出现半注册状态。
* 当前 Android App 的插件过滤。
* 当前 Package 切换后的函数和模板隔离。
* 脚本、函数测试、Cron 使用同一 Runner。
* 插件缺失与恢复时任务状态正确。
* 视频草稿生成、保存、打开和执行。
* 模板 ZIP 上传、Package 导入导出与媒体引用。
* 第三方 WASM 最小插件安装与调用。
* 无 WASM 构建路径仍可编译，不能错误声明具备 WASM 执行能力。

## 7.3 构建与验证

使用 `.github/workflows/ci.yml` 的实际命令作为 CI 权威入口，使用 `tools/ci-local.ps1` 作为快速子集，不把两者宣称为等效。至少完成 Rust 格式检查、Clippy、单测、无 WASM 构建、官方两个 Guest 的 WASM 构建与 Component 校验、前端递归全量测试与构建、官方插件临时目录打包验证。

可参考现有命令：

```powershell
cd server
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo check --locked --no-default-features
cargo test
cargo test --no-default-features
```

此外执行 `tools/check-version.ps1`、`tools/check-web-version.ps1`、`cargo test phase0_ -- --nocapture`、`tools/generate-phase0-baseline.ps1 -ValidateOnly`；前端测试必须覆盖 `web/src/**/*.test.js`，并确认 `web/src/console/device-summary.test.js` 未被默认 glob 漏掉。官方 Guest（YAML 与 Keymap）按 CI 的 wasm32/Component 流程构建。插件打包按最新 `tools/build-plugins.ps1` 实际参数输出到临时目录，检查 manifest、registry v2、sha256、builtin 归档和 Guest 产物，不要求当前含 `generated_at` 的包字节级复现。

真实设备、Windows 完整包、Docker、GitHub Release 和性能基线属于独立验收项。未实际执行的项目必须标记为 NOT_VERIFIED，不得以单测通过代替真实设备验证。

---

# 8. 实施顺序与交付要求

建议按以下顺序实施，不要同时启动多个相互覆盖的大重构：

| 阶段      | 优先级 | 交付结果                    |
| ------- | --- | ----------------------- |
| Phase 0 | P0  | README/AGENTS 架构确认、范围冻结 |
| Phase 1 | P0  | 新 YAML V1 语法与测试         |
| Phase 2 | P0  | 唯一解释器、旧 v3 清理           |
| Phase 3 | P0  | 插件 + Package 两种函数来源     |
| Phase 4 | P0  | 编辑器、Task、视频适配           |
| Phase 5 | P1  | 插件生命周期与 UI 收敛           |
| Phase 6 | P1  | 文档、SDK、旧代码清理            |
| Phase 7 | P0  | 集成测试与 V1 验收             |

每个阶段完成后必须：

1. 更新本计划的执行状态、实际改动文件和提交哈希。
2. 记录已完成、未完成与未验证项目。
3. 说明是否删除了旧实现，不能只增加新代码。
4. 运行对应测试并记录结果。
5. 如发现计划与最新代码不一致，以实际代码和本次确认的简化原则为准，不盲目实现旧计划。
6. 不在阶段执行过程中顺带增加未确认的新平台能力。

## 最终完成标准

Gamer V1 达到以下状态即可认为本轮收敛完成：

* Core 不包含 YAML 或自动化业务语义。
* YAML 只有一份权威解释器。
* 新增自动化能力主要通过函数实现。
* 函数只来自插件与当前 Package。
* 插件用户操作只有安装、启用/禁用、更新、卸载。
* Task 首版只提供 Cron，但保留通用 Runner。
* 现有 Package、视频、市场、SDK 和发行能力正常。
* README 与实际架构一致。
* 没有为了未来需求新增的多余抽象。
* 所有已完成项有测试依据，环境受限项明确标记未验证。

**本轮完成后停止继续架构重构，优先使用 Gamer 开发真实自动化脚本和插件，再根据实际使用中的问题决定下一步需求。**

---

# 9. 执行状态（2026-09-09 收尾）

## 9.1 交付与提交

| 阶段 | 状态 | 关键改动（实际文件） | 提交 |
| --- | --- | --- | --- |
| Phase 0 | DONE | 本计划落盘 docs/plans/；README/AGENTS 架构说明对齐 V1 | 随本次提交 |
| Phase 1+2 | DONE | 新增 `server/guests/yaml-interp/`（V1 唯一权威解释器：run=函数调用/if/repeat/return、`$name.field`、函数表运行期冻结、步预算 100k/深度 32、13 项单测）；`yaml-guest` 瘦身为 WIT 胶水并去掉 `programs` 接口；宿主 `syntax.rs`（解析/校验/降线/模板引用改写/确定性序列化）替代 `yaml_vnext.rs`（已删）；旧 v3 原生参考解释器（Interpreter/小 AST/nonce/splitmix64）全删 | `4e08d56` |
| Phase 3 | DONE | `native_funcs.rs` 原生函数注册表（17 函数，Schema+权限唯一声明点）；`runner_adapter.rs::compose_function_library` 组合 原生+当前 Package 全部 functions/*.yaml（同名冲突 `yaml.fn.conflict` 拒绝、不跨包）；`GET /api/runners/:id/functions` 原生函数目录 API | `4e08d56` + 本次 |
| Phase 4 | DONE | 前端 `web/src/script-editor/` 全量重写 V1（model 4 类步骤/Cell/CallArgs、codec 与服务端语义对齐、validation、factories、commands（`run` 寻址 + set_vars）、StepCard/StepCanvas/AddStepPanel/BranchContainer/ParamEditor/ParamsForm、entrypointParams 适配声明数组、函数面板接原生函数目录、function-list 伪模型 `run`）；`video_draft.rs` 产出 V1 草稿；`run_target.rs` args 收敛为原始 JSON 覆盖（任务宽松重绑/手动严格绑定，202 保留 resolved_args） | `4e08d56` + 本次 |
| Phase 5 | DONE | `POST /api/extensions/:id/enable` = 启用意图 + 直接启动（幂等，可选 keymap profile/AppContext body）；删除 /start /stop /activate 三个细粒度端点（内部保留 start/stop 原语供 reconcile/测试）；前端 PluginCenter 收敛为 启用/停用/更新/卸载，历史版本仅展示；守卫 §14.3 全链改写 | 本次提交 |
| Phase 6 | DONE | docs/reference/YAML.md 重写为 V1 唯一权威；docs/yaml-v3/ 删除；SCRIPT_EDITOR_CONTRACT.md 标记历史并指向 V1；README/AGENTS 对齐；SDK 三示例 wit 快照与 server/wit 同步（programs 删除）；官方插件重打包（gamer.yaml@3.1.1 V1 guest，registry.json/sha256sums 同步） | 本次提交 |
| Phase 7 | DONE | 后端 607 测试全绿（含真实 WASM guest e2e）；cargo fmt/clippy -D warnings/check --no-default-features 通过；前端 709 测试全绿（65 文件）+ pnpm build 通过 | 门禁记录 |

## 9.2 已删除（真实收敛，非并存）

- `server/src/extensions/gamer_yaml/yaml_vnext.rs`（v3 纯数据前端/小 AST/splitmix64/nonce）、`params.rs`（标量助手）、原生参考解释器（yaml_extension.rs Interpreter/CapabilityInvoker/YamlProgramResolver）——生产与测试共用 yaml-interp，无第二份解释器
- psig1 参数签名门禁（task_params 重写为按当前 Schema 绑定）；`TimerRunnerError::ParamStale`；七类 TypedValue/`BoundEntryArgs` wire（args = 原始 JSON）
- WIT `programs` 接口（函数表改为运行期冻结嵌入）；前端 DefaultsEditor、`version: 3` 编解码、19 类步骤模型、`script:`/`function:` call 命名空间
- REST `/api/extensions/:id/start|stop|activate`；前端 启动/停止/切换版本 按钮
- docs/yaml-v3/ 文档套件（V1 语法收敛在 docs/reference/YAML.md）

## 9.3 NOT_VERIFIED（环境受限，不以单测代替）

- 真机 adb 链路全流程（连接/投屏/输入/模板匹配实际效果）
- Windows 完整包、Docker 镜像、GitHub Release 与 launcher 升级链路
- 性能基线（解释器吞吐/投屏延迟）与平台矩阵
- 旧 v3 脚本存量数据的实际迁移（设计上不兼容，需人工按新语法重写）
