# YAML v2 / v3 能力奇偶对照报告（Phase 11 · B3 Phase B）

> **Status: superseded** —— 本报告为 Phase 11 遗留对照，不再作为执行依据；现行计划见
> `gamer_yaml_v3_finalization_v2_removal_plan.md`（Phase 12，已裁决删除 v2，见其
> P12.9 / ADR-YAML-01）。文中"v3 缺口"（G1-G5 / R1-R5）均已在 Phase 12 按最终
> 语义裁决收口（find then/else/verify、call 统一、defaults、预算、运行事件、
> 参数 schema 桥），不代表待办。

> 结论先行：**选 (c) —— v2 引擎暂留 gamer_yaml 扩展内部，删除 Core 可见的双格式分叉（保留单一 loader 入口），v3 补齐缺口后再切默认格式。**
> 缺口共 **5 项语法/语义缺口（G1-G5）+ 5 项运行时策略差异（R1-R5）**（§3/§4），guest 与 surface 均需改动，
> 不满足 (b) 的「缺口 ≤ 3 项且 guest 改动小」门槛。
> 本波（B3）已把双格式分叉收进 `extensions/gamer_yaml` 单一入口
> （`validate_compatible_script`，`server/src/extensions/gamer_yaml/resources.rs:219`），
> Core 侧已无任何 YAML parser/AST/格式判别（ADR-11 验收全过），因此 v2 暂留的成本只剩扩展内部维护，无架构债。

对照基准：v2 = `script_v2` 严格 loader + `engine/exec` 原生执行；
v3 = `yaml_vnext` 纯数据前端（`version:3` 判别）+ `yaml-guest` WASM Component 解释器
（`server/tests/yaml-guest/src/lib.rs`，既是产品实现也是测试夹具）。
文件行号以分支 `p11/yaml-extraction` 提交 `23a94a8` 为准。

---

## 1. 双格式分叉点（本波收口后的现状）

| 分叉点 | 位置 | 现状 |
|---|---|---|
| 保存/导入校验 | `extensions/gamer_yaml/resources.rs:177 validate_compatible_script` | 单一入口，`is_v3_source`（`version:3`，`yaml_vnext.rs:356-369`）判别后各走各的 loader，不做版本猜测、不自动转换 |
| 运行执行 | `extensions/gamer_yaml/engine/runner_adapter.rs` `YamlVnextAdapter::execute` | v3 源走 `run_yaml_vnext` → WASM guest；否则落回 v2 exec |
| 函数测试/函数库 | 仅 v2（`functions/` 目录 + `RunTarget::Function`） | v3 无函数库概念（见 §3-G3） |
| 模板重命名引用改写 | `resources.rs rename_template_references` | 双格式都支持：v2 走 AST 改写、v3 走 `yaml_vnext::rename_template_source`（`yaml_vnext.rs:924`） |
| 定时任务参数门禁 | `task_params::gate_task` → `engine::load_entry_param_decls` | 仅按 v2 loader 读参数声明；v3 脚本作为任务入口的参数解析未接（§4-R5） |

## 2. 逐步骤覆盖表（v2 Step 十九类 ↔ v3 SurfaceStep）

v2 步骤枚举：`script_v2/model.rs:173-246`；v3 surface：`yaml_vnext.rs:231-329`；
v3 派发解析：`yaml_vnext.rs:524-766`；guest 解释器：`tests/yaml-guest/src/lib.rs:39-186`。

| v2 步骤 | v3 对应 | 覆盖 | 依据 / 差异 |
|---|---|---|---|
| `tap` | `tap` | ✅ | `yaml_vnext.rs:546`；经 `input.tap` capability |
| `swipe` | `swipe` | ✅ | `yaml_vnext.rs:549`；v3 duration 为必填 Expr，v2 `time` 为 Cell（缺省有引擎默认）——语义等价、缺省行为不同 |
| `key` | `key` | ✅+ | `yaml_vnext.rs:569`；v3 增加 `action`（down/up/press），严格超集 |
| `text` | `text` | ✅ | `yaml_vnext.rs:592` |
| `log` | `log` | ✅+ | `yaml_vnext.rs:667`；v3 支持 level 字段 |
| `wait` | `wait` | ⚠️ | `yaml_vnext.rs:595`：单值 duration。**v2 支持随机区间 `[1s,3s]`（`model.rs:193-197` duration_max）→ G4** |
| `find` | `find` | ⚠️ | `yaml_vnext.rs:773-797`：template/timeout/region/click/then/else。**缺 `block`（有序障碍轮询）→ G1；缺 `verify`（二次确认两击）→ G2**。v3 反而多 `region` Expr |
| `match` | `match_first` | ⚠️ | `yaml_vnext.rs:799-835`：候选 `template + steps`。**v2 候选有 `click` 布尔（命中点中心自动点击 + interval，`model.rs:155-159`）→ 并入 G5**；v2 候选重复报错，v3 无此校验 |
| `check` | `check` | ✅ | `yaml_vnext.rs:702-714`：template/timeout/throw(message)，语义一致（未命中按 throw 结束） |
| `color` | `color_branch` | ✅ | `yaml_vnext.rs:838+`：branches（color/click/steps）+ else；click 语义同 v2 |
| `if` | `if` | ⚠️ | v3 `cond` 为通用 Expr（`yaml_vnext.rs:601-609`）；v2 仅布尔、无隐式转换。v3 truthy 规则更宽（guest `truthy`，`lib.rs:298`）——方向为 v3 超集，但空值/数值的真值口径需在契约文档定版 |
| `loop` | `loop` | ✅ | `yaml_vnext.rs:610-619`；times 缺省 = 无限、break 跳出，与 v2 一致；v3 另有 `retry` 语法糖（超集） |
| `break` | `break` | ✅ | `yaml_vnext.rs:620` |
| `call` | `call` | ⚠️ | v2 call = 同分区 scripts/ 脚本（压入目标 config 与参数作用域，32 层上限）；v3 call 经 `YamlProgramResolver` 只解析 **v3 脚本**（`runner_adapter.rs` `ScriptProgramResolver`：「call 目标不是 v3 脚本」），且**无 config 概念** |
| `func` | **无** | ❌ **G3** | v2 函数库（`functions/` 目录、`文件短路径/函数名` 引用、返回布尔走 then/else，`model.rs:240-245`）；v3 surface 无 func（yaml_vnext 全文 grep `func` 零命中），guest 无函数索引 |
| `throw` | `throw` | ✅ | `yaml_vnext.rs:633`（v3 message 必填 Expr；v2 支持裸 `- throw`） |
| `return` | `return` | ✅ | `yaml_vnext.rs:630`（v2 缺省 true；v3 需显式） |
| `str_app` | `app.start` | ✅ | `yaml_vnext.rs:690-700`；`NativeYamlHost` 映射 `device.start_app` 并自动加 `+` 冷启动前缀（`yaml_extension.rs:440-446`），与 v2 str_app "+" 语义一致 |
| `cls_app` | `app.stop` | ✅ | 同上 |
| （无） | `set` / `invoke` / `wait_for` / `click_when` / `retry` | v3 超集 | 通用 capability 逃逸口 + 常用糖 |

## 3. 语法/语义缺口清单（v3 需补齐项）

- **G1 `find.block` 有序障碍轮询**：v2 主模板命中判定需避开 block 区域内出现的干扰模板
  （`model.rs:198-205` block + `exec.rs` 轮询实现）。v3 无对应字段。补齐需 surface `find.block`
  + guest 侧多模板匹配编排（`vision.match_many` 可承载底层，但轮询/遮蔽语义要新写）。
- **G2 `find.verify` 二次确认两击**：v2 点击后复检模板再计命中（防误触动画帧）。v3 无。
- **G3 `func` 函数库**：v2 的 `functions/` 目录、函数测试运行（`RunTarget::Function`）、
  `文件/函数` 引用校验、布尔返回 then/else 均无 v3 形态；v3 `call` 又明确拒绝非 v3 目标，
  两条链路互不兼容。补齐 = functions/ 索引进 v3 resolver + guest call 目标分派 + 参数作用域语义。
- **G4 `wait` 随机区间**：`[min,max]` 抖动等待缺失，改动极小（建议下波顺手补）。
- **G5 `match` 候选级 `click`**：v2 命中即点中心并等 interval；v3 match_first 候选只带 steps，
  等价表达需要命中坐标绑定变量（如 `$match.center`）+ 求值器扩展。

## 4. 运行时策略差异（非语法，但影响切默认格式）

- **R1 点击后 interval / judge_delay**：v2 引擎从 config.toml 读 interval（所有脚本点击后统一
  等待）与 judge_delay_ms（命中后延迟进分支，默认 200ms，`exec.rs` 头注 7-13 行）。v3
  capability 链路无这两个策略点（tap 后即返回、命中即分支）。
- **R2 可视化事件**：v2 的 tap/swipe/hit/miss 事件经 EventSink 反向推送到投屏页
  （`exec.rs:750/772/1141/1167/1215/1298`）。v3 guest/capability 链路完全不发事件——
  脚本运行可视化在 v3 下静默。
- **R3 步数/深度护栏**：v2 100k 步 + 32 层嵌套上限；v3 原生参考解释器的 100k 预算仅测试消费
  （`yaml_extension.rs:165`），guest 主循环无步数护栏（`lib.rs:127` 无限循环只靠宿主取消兜底）。
- **R4 阈值/匹配选项**：v2 编辑器显式传脚本 config.threshold、引擎缺省同源；v3 capability
  `vision.match` 用 `MatchOptions::default()`（`capabilities/adapters/vision.rs:113` 用例同源），
  脚本级 threshold 未接。
- **R5 定时任务参数门禁**：`task_params::gate_task`/`probe_script_signature` 走 v2 loader 读参数
  声明（`task_params.rs:73-96`）；v3 脚本作为任务入口时参数声明解析、RunParamsModal 表单、
  psig1 签名均未接。v3 `Program.params` 结构已存在（`yaml_vnext.rs:207-217`），缺 loader 桥。

## 5. 结论（三选一）：**(c)**

- 缺口合计 5 项语法 + 5 项运行时策略，其中 G1/G2/G3 与 R2 触及产品核心体验
  （find 障碍轮询、函数库、运行可视化都是存量 v2 脚本/编辑器的高频能力），不满足 (b) 的门槛；
  v3 当前覆盖的只是「原子步骤 + 简单控制流 + 可调阈值模板查找」子集。
- **本波已完成的分叉收口**：v2/v3 双格式判别只剩 `gamer_yaml` 扩展内部一个入口
  （`validate_compatible_script`），Core 的 ResourceStore / 通用资源 API / RunManager /
  TimerRunnerRegistry 对格式零感知（ADR-11 / 计划 §8.9 验收全过）。v2 暂留的维护成本被
  封闭在扩展目录内，不构成架构债；「删 v2」的时机 = 上述缺口清零之时。
- 建议排序（供 Wave4/5 决策）：G4（wait 区间，几行）→ R4（脚本级 threshold）→
  G3（func 函数库，最大件）→ G5（候选 click + 命中坐标绑定）→ G1/G2（find block/verify）
  → R1/R2（interval/judge_delay/可视化事件经 capability 元数据透传）→ R3（guest 步数护栏）
  → R5（v3 任务参数门禁桥）。

## 6. 遗留项（本波未动、非本波范围）

- 前端可视化编辑器 codec（`web/src/script-editor/`）仅建模 v2；v3 无编辑器形态。
- 存量 v2 脚本无迁移工具（ADR-14 零兼容；G3 落地前 v2 是函数库的唯一承载格式，不可删）。
- `ExtensionService::start/stop` 对 gamer.yaml 的「无常驻实例」特判保留为扩展 id 字符串字面量
  （`extensions/service.rs:29-31`），P11.9 源码扫描守卫如需覆盖 id 字面量需另行定策略。
