# ADR-11：Core 与 Extension 最终边界

> 位置说明：Phase 11 计划原文路径为 `docs/gamer_refactor_plan_v2/adr/`；2026-09-05 docs 已重组为 reference/guides/plans/evidence 四子目录，ADR 属长期有效的架构决策记录，故定位于 `docs/reference/adr/`。

状态：ACCEPTED（2026-09-05）

## 背景

V2 主体重构完成后，Core 仍直接持有 YAML / Script / Keymap 等业务语义，前端仍存在"挂着插件名、实现留在 Core"的 Panel。Phase 11 在删除旧代码之前冻结最终边界，防止开发过程中再次引入临时兼容设计。

## 决策

Core 只能拥有（稳定机制）：

- 设备能力（DeviceManager / Screen / Frame / Touch / Keyboard Input / DeviceAction）
- Extension 机制（ExtensionRegistry / WASM Host / Capability API / UI Contribution Registry）
- Resource 机制（ResourceResolver：Editable / Override / Installed Package）
- App Package
- Timer / Task（Task / TimerRunnerRegistry / ScheduleProviderRegistry）
- Run（RunManager）
- 日志
- 设置

Core 禁止拥有（业务语义，全部归 Extension）：

- YAML parser
- YAML AST
- Script DSL
- Function DSL
- Keymap rule
- Plugin-specific UI

等价表述：Core 不再认识 `YAML`、`script_id`、`function DSL`、`keymap rule`、`YamlTimerRunner`、`ScriptStore`、`KeymapStore`。

后续新增任何功能前，用以下三个问题裁决归属：

1. **没有任何插件时，这个功能是否仍然成立？** 如果否 → Extension。
2. **Core 是否需要理解这个数据的业务语义？** 如果不需要 → Resource + Extension，而不是新增 Core Store。
3. **卸载插件后，这个功能是否应该消失？** 如果是 → 功能实现、UI、Runner、Parser 都必须归插件所有。

不能只把名字挂到 Extension Registry，而实现仍留在 Core。

## 后果

- P11.3 / P11.4 / P11.5 / P11.6 的删除与迁移以本边界为唯一判据；P11.9 的 Architecture Guard Tests（源边界 / 裸 Core 测试）把本边界锁进 CI。
- 判断问题成立即归 Extension 的能力，即使当前只有内置实现（Native Extension ≠ WASM），也必须经 Registry / Capability API 接入，不得在 Core 内硬编码。
- 违反本边界的新代码一律视为退化，不允许以"临时方便"为由合入。
