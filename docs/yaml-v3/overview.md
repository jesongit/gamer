# YAML v3 总览

> 本文是 `docs/yaml-v3/` 文档套件的入口：给出 YAML v3 的定位、所有权边界、
> 执行链与 19 类步骤总览，并串联套件内其余文档。权威裁决见
> `docs/reference/adr/ADR-YAML-01~04`，Phase 12 语法契约原文见
> `docs/plans/phase12_v3_dsl_contract.md`；实现事实核对基准为
> `server/src/extensions/gamer_yaml/yaml_vnext.rs`（纯数据前端）、
> `server/src/extensions/gamer_yaml/yaml_extension.rs`（执行/宿主适配）与
> `server/guests/yaml-guest/src/lib.rs`（WASM 解释器）。

## 1. v3 是什么

YAML v3 是 **gamer.yaml 扩展**拥有的自动化脚本 DSL：Parser、Runtime（WASM
guest）、函数库、编辑器语义与本文档全部归 gamer_yaml 扩展所有。**Core 不认识
YAML**——Core 只提供 Capability（device/vision/input/runtime/log/resource）、
Resource 六目录寻址、Task/Run/调度与扩展生命周期（ADR-11~14）。YAML 是完整、
可安装、可卸载的官方 Extension，不是 Core 的内建 DSL。

- **version: 3 是唯一接受的版本**：非 3（缺失 / `version: 2` / 其他值）一律报
  `unsupported yaml version`，无 fallback、无自动升级、无迁移工具
  （[ADR-YAML-01](../reference/adr/ADR-YAML-01-v3-only.md)）。
- 不保留 v2：`script_v2` loader/AST、双 Runtime、双 Editor Codec 均按 Phase 12
  裁决删除；v2 只存在于历史文档与 ADR 中作为需求参考。
- Editor 与 Runtime 共用同一 v3 surface DSL：前端编辑器 Model（19 类判别联合，
  `web/src/script-editor/model.ts` `STEP_KINDS`）与服务端 SurfaceStep 一一对应。

## 2. 最终执行链

```text
Task / 手动运行（POST /api/runs）
        ↓
gamer.yaml Runner（YamlTimerRunner，扩展 start 时注册）
        ↓
YAML v3 Program（parse_surface → lower：surface 步语法 → 小 AST + step 标签）
        ↓
YAML Guest（WASM Component 小 AST 解释器，本地执行预算计数）
        ↓
Capability API（capability.invoke → NativeYamlHost → Core CapabilityRegistry）
        ↓
Gamer Core（设备 / 输入 / 视觉 / 日志 / 资源）
```

细节见 [runtime.md](runtime.md)。

## 3. 19 类步骤总览

| 动作键 | 类别 | 一句话语义 | 详见 |
|---|---|---|---|
| `app.start` | 应用 | 冷启动应用（自动加 `+` 前缀），缺省当前分区包名 | [steps.md](steps.md) |
| `app.stop` | 应用 | 停止应用 | [steps.md](steps.md) |
| `tap` | 输入 | 点相对坐标（tap 后等待 `after_tap`） | [steps.md](steps.md) |
| `swipe` | 输入 | 滑动（from/to/duration） | [steps.md](steps.md) |
| `key` | 输入 | 按键（down/up/press） | [steps.md](steps.md) |
| `text` | 输入 | 输入文本 | [steps.md](steps.md) |
| `wait` | 时序 | 固定 / `{min,max}` 随机等待 | [timing.md](timing.md) |
| `log` | 观测 | 写运行日志（level/message） | [steps.md](steps.md) |
| `set` | 数据 | 给变量赋值 | [expressions.md](expressions.md) |
| `if` | 控制流 | truthy 条件分支 | [steps.md](steps.md) |
| `loop` | 控制流 | 循环（times 缺省 = 无限） | [steps.md](steps.md) |
| `break` | 控制流 | 跳出最近一层 loop | [steps.md](steps.md) |
| `call` | 调用 | 唯一可调用资源入口（`script:` / `function:`） | [call.md](call.md) |
| `return` | 调用 | 从当前脚本/函数返回任意 JSON 值 | [call.md](call.md) |
| `throw` | 调用 | 以错误文案终止运行 | [steps.md](steps.md) |
| `find` | 视觉 | 轮询找模板 → then/else/verify/save | [vision.md](vision.md) |
| `match_first` | 视觉 | 多候选单帧匹配，首个命中执行自己的 steps | [vision.md](vision.md) |
| `check` | 视觉 | 轮询等模板出现，超时抛错 | [vision.md](vision.md) |
| `invoke` | 逃逸口 | 直调 Capability（vision.match / sample_color 等） | [steps.md](steps.md) |

已被删除、校验器给迁移诊断的 v2 语法（`func` / `match.click` / `click_when` /
`wait_for` / `retry` / `color_branch` / `find.click`）见
[steps.md](steps.md) 的迁移表。

## 4. 文档地图

| 文档 | 内容 |
|---|---|
| [program.md](program.md) | Program 顶层结构、version 门禁、函数库 bare-map 形态 |
| [params.md](params.md) | 参数声明双形态、类型集合、schema API、执行前校验 |
| [steps.md](steps.md) | 19 类步骤逐一语法/字段/语义 + 移除语法迁移表 |
| [expressions.md](expressions.md) | `$` 变量、属性路径、字面量、`$match` 上下文、truthy/equals |
| [call.md](call.md) | call 命名空间、实参绑定、返回值泛化、递归深度 |
| [vision.md](vision.md) | find / match_first / check、threshold 三级、match 结果 map |
| [timing.md](timing.md) | defaults.timing 三项、wait 随机区间、无隐藏全局时序 |
| [runtime.md](runtime.md) | 执行链、执行预算、start_index、运行事件 wire 契约 |
| [examples.md](examples.md) | 完整 v3 示例脚本 |

相关外部文档：`docs/reference/YAML.md` §11（v3 章节实现同步）、
`docs/reference/SCRIPT_EDITOR_CONTRACT.md`（可视化编辑器契约）。
