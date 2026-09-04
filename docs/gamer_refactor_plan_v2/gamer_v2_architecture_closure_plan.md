# Gamer V2 架构收尾计划

## 1. 目标

针对 `docs/gamer_refactor_plan_v2/README.md` 中 `8.3` 的已知偏差与待决项进行最终收口。

本阶段不继续扩大 V2 重构范围，主要目标是：

- 修正当前 Timer / Cron 中残留的架构边界问题
- 正式确认 Cron、WASM Runtime State、Timer Runner 的设计决策
- 补齐 Keymap 的真实设备端到端延迟验证
- 更新 V2 文档，使文档与实际实现保持一致
- 为未来自定义 Schedule / Runner 扩展保留清晰接口，但不提前实现没有真实需求的能力

本阶段定位为：

> **V2 Architecture Closure**

不作为新的大型重构阶段。

---

## 2. 总体处理结论

| 项目 | 处理结论 |
|---|---|
| Cron 未迁移至 WASM | 保持 Native，并正式收口 |
| WASM 缺少 `Available / Starting / Stopping` | 不补，作为明确架构决策 |
| 当前只有 `gamer.yaml` Runner | 暂不增加第二 Runner |
| Keymap 缺真实设备 E2E 延迟测试 | 本阶段补齐 |
| Scheduler 存在对 Cron 的直接依赖 | 本阶段修复 |

---

# 3. Phase 1：Timer / Cron 架构收口

## 3.1 目标

保证 TimerCore 和 Scheduler 不直接依赖 Cron 具体实现。

最终架构：

```text
TimerCore
    ↓
ScheduleRegistry
    ├── BuiltinCronSchedule
    └── Future Schedule Providers
```

TimerCore 只负责通用机制：

- TimerTask 持久化
- Trigger 计算调度
- Misfire 处理
- Restart Recovery
- Wakeup
- Claim
- Dispatch
- Runner 调用

TimerCore 不理解：

- Cron 表达式
- 自定义 Schedule 语法
- 插件内部 Schedule 逻辑

## 3.2 修复 Scheduler 对 Cron 的直接依赖

检查当前与以下逻辑类似的代码：

```text
next_enabled_trigger_in_secs()
```

如果当前存在：

```text
Scheduler
    ↓
CronExtension
```

或：

```rust
CronExtension::new()
```

直接参与 Scheduler 的下一次触发时间计算，需要移除。

统一改为：

```text
TimerTask
    ↓
ScheduleSpec
    ↓
ScheduleRegistry
    ↓
ScheduleExtension
```

推荐调用方式：

```text
ScheduleRegistry.next_after(schedule_spec, now)
```

保证 Scheduler 不再知道：

```text
schedule.kind == "cron"
```

## 3.3 评估由 TimerCore 暴露 Next Wakeup

如果 `next_enabled_trigger_in_secs()` 的真实用途只是判断 TimerCore 下一次什么时候需要被唤醒或什么时候存在待执行工作，优先评估是否可以由 TimerCore 直接暴露：

```text
next_wakeup_at()
```

或：

```text
next_wakeup_in()
```

最终优先架构：

```text
TimerCore
    ├── 管理 TimerTask
    ├── 计算 Next Trigger
    └── 暴露 Next Wakeup
             ↓
         Scheduler
```

优于 Scheduler 再遍历所有 TimerTask 重复计算。

本阶段原则：

> 如果该调整需要明显扩大重构范围，则不强制修改。

最低要求：

> Scheduler 必须通过 ScheduleRegistry，而不是直接依赖 CronExtension。

---

# 4. Phase 2：正式确认 Cron 架构决策

## 4.1 Cron 保持 Native

正式确认：

```text
Cron = Builtin Native Schedule Provider
```

不再要求 Cron 迁移至 WASM。

原因：

- Cron 是标准、稳定、纯计算能力
- 不包含 Gamer 业务执行逻辑
- Native 实现简单且性能稳定
- 当前不存在替换 Cron 实现的实际需求
- TimerCore 已经通过 `ScheduleExtension` / `ScheduleRegistry` 隔离 Cron
- 将 Cron 强制迁移到 WASM 不会带来明显收益

## 4.2 明确 Native 与 Extension 的关系

需要在文档中明确：

> Extension 是架构扩展点，不等于 WASM Plugin。

当前可以存在：

```text
ScheduleExtension
    ├── Native Schedule Extension
    └── WASM Schedule Extension
```

因此 CronExtension 是 Native 实现并不违反 Extension 架构。

## 4.3 是否调整 Cron 命名

可选优化：

```text
CronExtension
```

改为：

```text
BuiltinCronSchedule
```

或：

```text
NativeCronSchedule
```

如果改名会导致大量无意义代码变更，则不做，优先更新文档说明。

---

# 5. Phase 3：为未来自定义 Schedule 保留扩展能力

## 5.1 不实现长期驻留 Trigger Runtime

不要因为未来可能存在 WASM Schedule，就提前增加：

- Persistent WASM Instance
- Long-running Trigger Plugin
- Background WASM Scheduler
- WASM Resident Runtime
- 独立 Trigger Capability 生命周期

当前没有必要。

## 5.2 推荐未来扩展方式

未来如果出现：

```yaml
schedule:
  kind: solar
```

或：

```yaml
schedule:
  kind: game-calendar
```

可以增加：

```text
WasmScheduleExtensionAdapter
```

架构：

```text
TimerCore
    ↓
ScheduleRegistry
    ↓
WasmScheduleExtensionAdapter
    ↓
WASM Plugin
    ↓
next_after(schedule_spec, now)
```

WASM Schedule 只需要完成：

```text
ScheduleSpec + CurrentTime
            ↓
       NextTrigger
```

无需负责等待时间、Timer 唤醒、持久化、Restart Recovery、Misfire 和 Dispatch，这些仍由 TimerCore 负责。

---

# 6. Phase 4：WASM Runtime State 收口

## 6.1 保持当前稳定状态

V2 Runtime State 保持：

```text
Installed
Disabled
Running
Failed
```

不增加：

```text
Available
Starting
Stopping
```

## 6.2 `Available` 不属于 Runtime State

`Available` 表示某个插件存在于插件仓库、Catalog 或远程来源中，但尚未安装。

它属于：

```text
Plugin Catalog
```

而不是：

```text
Plugin Runtime Lifecycle
```

推荐未来模型：

```text
Plugin Catalog
    ├── available versions
    ├── source
    └── metadata

Plugin Installation
    ├── installed
    ├── enabled / disabled
    ├── running
    └── failed
```

因此 V2 不增加 `Available`。

## 6.3 `Starting / Stopping` 不作为稳定持久状态

`Starting` 和 `Stopping` 本质上属于：

```text
Operation State
```

而不是：

```text
Stable Runtime State
```

不建议持久化为插件生命周期状态。

否则需要额外处理启动/停止过程中崩溃、operation timeout、recovery、reconciliation、stale operation、operation_id 等问题，复杂度远高于当前收益。

## 6.4 未来如有 UI 需求

如果前端以后需要显示：

```text
正在启动...
正在停止...
```

推荐增加独立 Operation 信息：

```json
{
  "state": "running",
  "operation": {
    "type": "starting",
    "started_at": 123456789
  }
}
```

而不是增加：

```text
RuntimeState::Starting
RuntimeState::Stopping
```

最终原则：

> Stable State 与 Temporary Operation State 分离。

---

# 7. Phase 5：Timer Runner 架构收口

## 7.1 不增加虚构 Runner

当前只有：

```text
gamer.yaml
```

Runner。

本阶段不为了验证 `TimerRunnerRegistry` 而人为实现 `gamer.macro` 或其他没有真实需求的 Runner。

## 7.2 保持现有抽象

继续保留：

```text
TimerCore
    ↓
TimerRunnerRegistry
    ↓
TimerRunner
```

TimerCore 只依赖 `runner_id`。

未来新增 Runner 不需要修改 TimerCore。

## 7.3 第二个 Runner 出现时再补完整 Lifecycle

等第一个真实第二 Runner 出现时，再统一补：

```text
register_runner()
unregister_runner()
replace_runner()
```

以及：

```text
runner_id -> plugin_id
```

等 Ownership 关系。

## 7.4 同时处理插件卸载问题

未来 Plugin Runner 出现后，需要明确：

```text
Plugin uninstall
    ↓
Runner unregister
    ↓
Existing TimerTask ?
```

届时统一设计：

- 已存在 TimerTask 是否允许继续保留
- Runner 不存在时任务进入什么状态
- Plugin 能否在仍被 TimerTask 引用时卸载
- 是否需要 dependency check
- 是否自动 disable TimerTask
- 是否允许重新安装插件后恢复

当前不提前实现。

---

# 8. Phase 6：Keymap E2E 延迟验证

## 8.1 目标

补齐真实链路：

```text
Browser
    ↓
WebRTC DataChannel
    ↓
Gamer Server
    ↓
WASM Keymap
    ↓
DeviceAction
    ↓
scrcpy control write
```

验证 WASM Extension 架构没有对真实键盘控制产生明显的尾延迟退化。

## 8.2 当前 Benchmark 的定位

当前已有 Benchmark 继续保留，用于验证：

```text
Native Mapping
vs
WASM Mapping
```

纯 Runtime 开销。

但它不能替代真实 E2E。

两者职责分开：

```text
Micro Benchmark
    ↓
验证 WASM Runtime 自身开销

E2E Benchmark
    ↓
验证真实 Gamer 控制链路
```

## 8.3 增加 Trace ID

每个测试输入生成：

```text
trace_id
```

并在链路记录：

```text
client_send_ts
server_receive_ts
wasm_begin_ts
wasm_end_ts
device_action_ts
scrcpy_write_ts
```

如果浏览器和 Server 时钟不能可靠同步，则不要直接计算跨机器绝对时间，可以将 Browser RTT 与 Server 内部阶段耗时分开统计。

## 8.4 测试指标

至少统计：

```text
P50
P95
P99
Max
```

阶段包括：

```text
Browser -> Server
Server Receive -> WASM Begin
WASM Execution
WASM End -> DeviceAction
DeviceAction -> scrcpy Write
Server Internal Total
Browser -> scrcpy Write
```

在可可靠测量的情况下统计完整 E2E。

## 8.5 Native / WASM 对照测试

同一套环境分别测试：

```text
Native Keymap
WASM Keymap
```

重点关注：

```text
WASM E2E - Native E2E
```

而不是只关注绝对延迟。

## 8.6 测试场景

至少包含：

### 普通按键

```text
KeyDown
KeyUp
```

例如：

```text
W
A
S
D
Space
```

### 长按

```text
KeyDown
保持 1~3 秒
KeyUp
```

### 组合键

根据当前 Keymap 支持情况选择至少一组真实组合键。

### 连续输入

连续发送一段按键事件，检查：

- 是否丢事件
- 是否乱序
- KeyUp 是否可靠送达
- WASM 是否产生明显 tail latency

## 8.7 Benchmark 输出

建议生成机器可读结果，例如：

```text
benchmarks/results/keymap-e2e.json
```

记录：

```text
commit
device
android_version
browser
connection_type
sample_count
native
wasm
```

并输出 P50 / P95 / P99 / Max。

## 8.8 暂不设过严绝对指标

第一阶段优先建立真实 baseline。

暂不直接规定例如：

```text
P95 < 20ms
```

先通过多次真实测试获得稳定数据。

后续可以逐步增加：

```text
WASM 相比 Native 的 P95 增量不得超过某阈值
```

用于防止未来架构退化。

## 8.9 CI 策略

真实设备 Benchmark 默认：

```text
opt-in
```

不作为普通 CI 必跑项。

普通 CI 继续运行：

- WASM Runtime 单元测试
- Keymap 单元测试
- Micro Benchmark
- Timer / Registry 测试

真实设备测试用于：

- Release 前验证
- 架构大改后验证
- Keymap / WebRTC / scrcpy 链路修改后验证

---

# 9. Phase 7：文档收口

## 9.1 更新 README 8.3

建议将：

```text
已知偏差与待决项
```

调整为：

```text
已确认的架构决策与后续验证
```

避免已经确认合理的设计继续看起来像“未完成任务”。

## 9.2 增加 ADR 结论

README 中记录：

### ADR-01 Cron Provider

```text
Decision: ACCEPTED

Cron 保持 Native Schedule Provider。
TimerCore 通过 ScheduleRegistry 与其隔离。
未来第三方 Schedule 通过独立 Adapter 接入。
```

### ADR-02 WASM Runtime State

```text
Decision: ACCEPTED

稳定 Runtime State：
Installed / Disabled / Running / Failed

Starting / Stopping 属于 Operation State。
Available 属于 Plugin Catalog。
```

### ADR-03 Timer Runner

```text
Decision: DEFERRED

V2 当前只提供 gamer.yaml Runner。

不为了证明抽象而创建虚构 Runner。
第一个真实第二 Runner 出现时再补完整动态生命周期。
```

### Validation-01 Keymap E2E

```text
Status: DONE

补真实 Browser -> Server -> WASM -> scrcpy 链路 Benchmark。
结果：benchmarks/results/keymap-e2e.json（真机 native/WASM 对照 baseline，
WASM Server Internal Total P95 增量 ≈+28µs，无尾延迟退化）。
```

## 9.3 同步 Phase 文档

检查并同步：

```text
phase-06-wasm-host.md
phase-07-keymap-wasm.md
phase-09-cron.md
phase-10-management-registry.md
```

对于已经改变的设计，不再保持“原计划必须完成”的描述。

应明确标记为：

```text
Original Plan
Final Decision
Reason
```

避免以后重新查看计划时误以为实现偏离设计。

---

# 10. 实施顺序

推荐按以下顺序执行。

## Step 1

修复：

```text
Scheduler -> CronExtension
```

直接依赖。

目标：

```text
Scheduler -> ScheduleRegistry
```

或：

```text
Scheduler -> TimerCore Next Wakeup
```

## Step 2

补相关单元测试：

- Cron Schedule 注册
- Registry 调度
- 不同 `schedule.kind` 的处理
- Unsupported Schedule 错误
- Timer wakeup 计算

## Step 3

更新 README 8.3 和 Phase 9 文档，正式确认 Native Cron。

## Step 4

更新 WASM State 文档，明确：

```text
Available -> Catalog
Starting / Stopping -> Operation
```

## Step 5

更新 Runner 文档，标记：

```text
第二 Runner -> Deferred
```

## Step 6

实现 Keymap E2E tracing。

## Step 7

执行真实设备测试并生成 baseline。

## Step 8

更新 V2 README，将 8.3 收口。

---

# 11. 本阶段明确不做

为防止范围继续膨胀，本阶段明确不做：

- 不把 Cron 强制迁移到 WASM
- 不实现 Persistent WASM Trigger
- 不增加 WASM 长期驻留实例
- 不为了计划补齐 `Starting / Stopping`
- 不为了验证抽象创建第二个 Runner
- 不提前实现复杂 Plugin Runner 生命周期
- 不提前设计完整 Plugin Marketplace
- 不把真实设备 Benchmark 加入普通 CI 必跑流程
- 不对 TimerCore 做与 8.3 无关的大规模重写

---

# 12. 完成标准

本阶段完成后，应满足（2026-09-04 全部达成）：

## Timer

- [x] Scheduler 不直接依赖 Cron 实现（仅存注册缝 `cron_extension::register_builtin`；自检测试 `schedule_computation_is_locked_to_the_registry_abstraction` 锁边界）
- [x] Schedule 计算统一通过抽象层（`ScheduleRegistry.next_after/probe`；`next_enabled_trigger_in_secs()` 已删除，改 `TimerCore::next_wakeup_in()/next_wakeup_at()`）
- [x] Cron 保持 Native（ADR-01）
- [x] TimerCore 不理解 Cron 业务语义
- [x] Future Schedule Provider 可以通过 Registry 扩展

## WASM Runtime

- [x] 文档正式确认稳定状态模型（ADR-02）
- [x] `Available` 从 Runtime State 设计中移除（归 Plugin Catalog）
- [x] `Starting / Stopping` 明确归入 Operation State
- [x] 不增加无必要的状态机复杂度

## Runner

- [x] `gamer.yaml` 继续作为当前唯一 Runner
- [x] TimerCore 保持 Runner 无关
- [x] 第二 Runner 明确 Deferred（ADR-03）
- [x] 文档记录未来需要补的 dynamic lifecycle（计划 7.3/7.4 保留为届时清单）

## Keymap

- [x] 完成真实设备控制链路 Trace（`input_event` 信封可选 `trace_id`/`client_send_ts`，服务端 7 阶段埋点，默认关闭零开销）
- [x] 完成 Native / WASM 对照测试（同设备同会话同 DataChannel 两轮）
- [x] 输出 P50 / P95 / P99（含 Max，见 `benchmarks/results/keymap-e2e.json`）
- [x] 测试 KeyDown / KeyUp / 长按 / 连续输入（normal / long_press / combo / burst）
- [x] 形成第一版 E2E baseline（Validation-01 DONE）

## Docs

- [x] README 8.3 更新（改为「已确认的架构决策与后续验证」，ADR-01/02/03 + Validation-01）
- [x] Phase 6 更新
- [x] Phase 7 更新
- [x] Phase 9 更新
- [x] Phase 10 必要内容更新（经核对无需改动）
- [x] 原计划与最终决策不存在明显冲突（各 phase 文档以 Original Plan / Final Decision / Reason 收口记录标注）

---

# 13. 最终架构原则

完成本阶段后，V2 的核心原则正式确定为：

> **Core 提供稳定机制，Extension 提供可替换语义。**

具体表现为：

```text
Native Core
    ├── Timer
    ├── Persistence
    ├── Device Control
    ├── WebRTC
    ├── Plugin Runtime
    └── Registry

Extension
    ├── Schedule Provider
    ├── Keymap
    ├── Runner
    └── Future Capabilities
```

同时：

> **Extension 不等于 WASM。**

对于稳定、标准、高频且没有第三方替换价值的能力，可以采用 Native Extension。

对于需要第三方扩展、用户自定义或独立发布的业务能力，再通过 WASM 接入。

最终避免两个极端：

```text
所有东西都塞进 Core
```

以及：

```text
为了插件化而把所有东西强制 WASM 化
```

以保持 Gamer 的：

- 轻量化
- 高性能
- 可维护性
- 扩展性
- 清晰边界
