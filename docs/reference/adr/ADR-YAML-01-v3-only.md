# ADR-YAML-01：YAML v3 唯一正式方案与 v2 删除

> 编号说明：ADR-01~14 是全局架构决策序列（Phase 11 收口产出）；ADR-YAML-xx 是 YAML 域专项 ADR 序列（命名见计划 §5.5），记录 gamer_yaml 扩展 DSL / Runtime 的最终语义裁决，与全局序列互不续号。
>
> 关联计划：`docs/plans/gamer_yaml_v3_finalization_v2_removal_plan.md`（§5 P12.0 语义冻结、§14 P12.9 删除 v2、§22 最终原则）。

状态：ACCEPTED（2026-09-05）

## 背景

当前 YAML v2（`server/src/extensions/gamer_yaml/script_v2/` + `engine/`）与 v3（`yaml_vnext.rs` surface DSL + `yaml_extension.rs` + WASM guest）并存，前端 Script Editor 仍是纯 v2 Model / Codec，形成 `Runtime = v3 / Editor = v2` 的割裂。此前工作以 "v2/v3 parity" 为目标，容易把旧 Engine 特有结构当作"缺失能力"原样搬回 v3。收口前必须先冻结语义：哪些 v2 能力保留并按 v3 架构重设计、哪些直接废弃，避免开发过程中继续以 parity 名义回流 v2 结构。

## 决策

### version: 3 是唯一接受的版本

- 脚本必须声明 `version: 3`；非 3（缺失、`version: 2`、其他值）一律报 `unsupported yaml version`（v3 诊断码 `yaml.v3.version` / `yaml.v3.version.missing`）。
- **无 fallback、无自动升级、无迁移工具**；不保留双 Runtime、双 Editor Codec。
- 旧 v2 文件不保证继续运行；仓库内正式 sample / examples / game package 手动升级到 v3（不提供 Runtime migration）。

### 语义冻结裁决表

保留并按 v3 架构重设计：

| 能力 | v3 归宿 |
|---|---|
| find block（命中后执行步骤组） | `find.then`，见 ADR-YAML-03 |
| find verify（操作后二次验证） | `find.verify`（模板 + timeout），见 ADR-YAML-03 |
| wait 随机区间 | v3 步语法重设计，正式实现只保留一种写法（min/max 或 random 子结构二选一） |
| vision threshold | Step threshold > `defaults.vision.threshold` > Runtime built-in 兜底 |
| timing defaults | 语义化 Program defaults（after_tap / after_swipe / after_match / poll_interval），数量克制 |
| Task 参数 | `Program.params` 为唯一参数来源（P12.3 Task Params Bridge） |
| 运行可视化事件 | DataChannel wire 契约，见 ADR-YAML-03 |
| 执行步数 / 调用深度预算 | ExecutionBudget，见 ADR-YAML-04 |
| match 结果上下文 | 通用 runtime value + save 固化，见 ADR-YAML-03 |

废弃不恢复：

| 废弃项 | 理由 / 替代 |
|---|---|
| `func` step | 并入通用 `call`，见 ADR-YAML-02 |
| `match.click = true` / `find.click: true` 专用语法 | 改为通用 match 结果上下文（`then` 步骤组 + `$match.center` tap），见 ADR-YAML-03 |
| 隐藏式 config.toml interval / judge_delay | 脚本行为必须自包含（计划 §22 规则 3），时序参数进 Program `defaults`，不恢复模糊的全局 magic timing |
| v2 AST 形态 | v2 只作需求参考、不作架构模板，v3 不复制其 AST / lowering 结构 |
| v2 兼容 loader / 分支 | `load_compatible` / `detect_v2_v3` / `fallback_parse` / `if version == 2` 类分支整体删除 |

### v2 删除范围清单（对应计划 P12.9）

- `script_v2/`：loader / parser / AST / validator / serialize
- engine v2 executor（legacy native YAML 引擎）
- 前端 v2 editor codec / model（decode-v2 / encode-v2 / compat-codec 及 v2 schema / validation / factories）
- 所有 `if version == 2` / `if legacy` / `if compatible` 类 API 分支
- v2-only 测试、v2→v3 parity 测试、legacy fixtures；最终只保留 v3 behavior tests
- 仓库内正式 sample / examples 手动升级 v3

删除完成后，生产代码中检索 `script_v2` / `CompatibleYaml` / `legacy yaml` / `version == 2` 应为 0 引用（历史 ADR / migration notes 除外）。

### 最终执行链

```text
Task / Manual Run
        ↓
gamer.yaml Runner
        ↓
YAML v3 Program
        ↓
YAML Guest（WASM）
        ↓
Capability API
        ↓
Gamer Core
```

YAML 是完整、可安装、可卸载的官方 Extension，不是 Core 的内建 DSL；Core 只提供 Capability / Resource / Task / Run / Extension Runtime。

## 后果

- P12.1 起（Editor / Function / Params / Budget / Vision / Events / find）所有新功能只为 v3 开发；v2 从删除开始即视为不存在（计划 §22 规则 6）。
- 存量 v2 脚本升级即破坏、无兼容窗口——与 ADR-14（不提供 Legacy 兼容）一致；本地开发数据失效直接重写脚本。
- Editor 与 Runtime 共用同一 v3 surface DSL，杜绝 `Editor = v2 / Runtime = v3` 再次出现。
- 换取的收益：单一 Parser / Runtime / Editor Model / 参数来源，版本分支逻辑与双测试集心智负担清零。
