# ADR-YAML-04：执行预算（ExecutionBudget）

> 编号说明：ADR-01~14 是全局架构决策序列（Phase 11 收口产出）；ADR-YAML-xx 是 YAML 域专项 ADR 序列（命名见计划 §5.5），记录 gamer_yaml 扩展 DSL / Runtime 的最终语义裁决，与全局序列互不续号。
>
> 关联计划：`docs/plans/gamer_yaml_v3_finalization_v2_removal_plan.md`（§7.6 Call Depth、§9 P12.4 Guest Execution Guard）。

状态：ACCEPTED（2026-09-05）

## 背景

无限 loop、递归 call、意外超大脚本目前只能依靠用户主动 Cancel；WASM guest 内纯计算死循环不经过 host，取消信号无法打断。需要确定性的执行预算与可观察的终止错误，而不是靠 host timeout 猜测"跑太久就算死"。

## 决策

### ExecutionBudget

```text
max_steps      = 100_000   （逻辑步）
max_call_depth = 32
```

- **步数按逻辑步计**：循环体内每个子步都计数（if / loop / find then / match 候选 steps 展开后的每一步），不能只按顶层步计——否则 `loop: times: ∞` 外包一层即可绕过预算。
- **调用深度**：进入 callable 深度 +1、返回 -1，超过 `max_call_depth` 立即终止。与 ADR-YAML-02 的递归上限同一数值；resolver 层临时守卫在本预算落地后由 guest 统一计数正式化。

### 超限错误码

```text
STEP_BUDGET_EXCEEDED
CALL_DEPTH_EXCEEDED
CANCELLED
```

- 必须出现在 Run Event 与日志中可观察（可视化走 `budget{kind}` 事件，见 ADR-YAML-03）；运行 UI 能区分「预算耗尽」与「用户取消」。

### 职责划分：guest 计数 + host 兜底

- **guest（WASM 解释器）负责步数与深度计数**——这是脚本语义层的护栏，随脚本执行确定性触发，精确到步。
- **host 侧用 wasmtime epoch interruption 作为取消兜底**：guest 内纯计算死循环可被 epoch trap 中断，与宿主 Cancel（stop 标志）双机制共存：
  - guest 步预算：语义层终止，产出 `STEP_BUDGET_EXCEEDED` / `CALL_DEPTH_EXCEEDED`。
  - host epoch interruption：宿主取消通道，纯计算不经过 Capability 调用时仍可打断。
- **预算不依赖 host timeout 猜测**：不引入"运行超时即判死"的模糊语义，任何终止都有确定错误码；`max_steps` / `max_call_depth` 数值为初始默认，后续可按实际压力调整（数值调整不构成语义变更）。

## 后果

- 无限 loop 与超深递归自动中断、无需用户干预；终止原因进 Run Event（计划 §9 验收标准：budget 错误可观察）。
- guest 实现必须在每个逻辑步边界检查预算，并保证执行有周期性让出点（配合 epoch 检查），否则 host 取消对纯计算段无法生效——这是 P12.4 实现的硬性要求。
- 步数计数口径（逻辑步、子步全计）写入引擎测试：预算绕过（外层 loop 包裹）视为实现缺陷。
