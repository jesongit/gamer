# ADR-13：Runner 生命周期归 Extension 所有

> 位置说明：Phase 11 计划原文路径为 `docs/gamer_refactor_plan_v2/adr/`；2026-09-05 docs 已重组为 reference/guides/plans/evidence 四子目录，ADR 属长期有效的架构决策记录，故定位于 `docs/reference/adr/`。

状态：ACCEPTED（2026-09-05）。**本 ADR SUPERSEDE v2 计划中的「ADR-03 Timer Runner（DEFERRED）」**——原决策"待第一个真实第二 Runner 出现时再补动态生命周期"不再适用：Phase 11 确定立即落地 TimerRunnerRegistry 与插件归属（P11.2），不再等待。原记录见 `docs/plans/archive/gamer_refactor_plan_v2/README.md` 8.3。

## 背景

当前 Runner 抽象已存在，但 `gamer.yaml` Runner 仍由 Native Server 代码构造——卸载 gamer.yaml 后 Core 依然知道它怎么执行，违反插件生命周期（ADR-11 问题 3）。Runner 必须与 Extension 生命周期绑定。

## 决策

Core 提供 `TimerRunnerRegistry`（register / unregister / replace / get / list / owner lookup），注册项带 `owner_extension_id`：

```text
RegisteredRunner = { runner_id, owner_extension_id, runner }
```

Runner 生命周期与 Extension 生命周期严格绑定：

- **Extension 启动**：load → initialize → 注册其所有 Runner。
- **Extension 禁用**：注销其所有 Runner（Runner 消失，Task 不消失）。
- **Extension 卸载**：注销 Runner + 移除 UI contribution + 移除 runtime，三者一并清干净。

Runner 缺失时（TimerCore 执行 Task 查不到 `runner_id`）：

- Task 进入 `DEPENDENCY_MISSING` 状态，记录 `missing_dependency`（如 `gamer.yaml`）；
- **Task 不删除**——用户配置是资产，不因插件暂时离位而丢。

Extension 重新启用 / 重装并重新注册 Runner 后，依赖该 Runner 的 Task 自动恢复 `READY`（或等待下一个 schedule 触发）。

## 后果

- 完成后删除 Native 的 `YamlTimerRunner` / `timer_yaml.rs`；其中通用逻辑先搬入 TimerCore / RunManager / Extension Runtime，再删除 YAML-specific 部分。
- "卸载 gamer.yaml" 从此具备完整语义：其 Runner、UI、解析能力同时消失，悬挂 Task 明确挂起而非静默失败。
- P11.9 的 Extension Lifecycle Test / Bare Core Test 依此验收：无 gamer.yaml 时裸 Core 可启动，Task 呈 DEPENDENCY_MISSING；装回即恢复。
