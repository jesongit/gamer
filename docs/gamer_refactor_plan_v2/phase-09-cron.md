# Phase 9：Timer Core 与 Cron Extension

## 目标

把可靠计时作为 Core 基础设施，把 Cron 语法和触发规则作为 Extension，彻底解除 Scheduler 对 YAML / ScriptStore 的依赖。

---

## 1. Core Timer Runtime

Core 负责：

- task persistence
- next wakeup
- system time
- cancel
- restore after restart
- error recovery
- suspend/resume

不负责：

- YAML script 语义
- Cron 表达式业务含义
- App-specific automation

---

## 2. Cron Extension

Cron WASM 负责：

```text
parse cron
register schedule semantics
trigger behavior
```

到点后：

```text
Cron Extension
→ run.submit(RunRequest)
```

---

## 3. RunRequest 泛化

任务只应保存：

```text
device
app context
runner_id
entrypoint
payload
schedule
```

例如：

```text
runner_id = gamer.yaml
entrypoint = daily
```

以后可以：

```text
runner_id = gamer.macro
```

无需修改 Scheduler。

---

## 4. 去除 completion polling

如果当前每 50ms 查询 run status：

```text
sleep
→ query
→ sleep
```

改为：

```text
run.wait_terminal(run_id)
```

或 Notify/Event。

---

## 5. Task Preset 与 User Task 分离

App Package 可以提供：

```text
Task Preset
```

用户真正启用后生成：

```text
User Task
```

存在 DB。

Package 卸载时：

```text
依赖 Task → Suspended
```

不要直接删除用户 schedule。

---

## 6. 前端 Task Panel

Cron Extension 可贡献：

```text
任务
```

Panel UI 可以是：

- Declarative UI
- iframe

但 Timer Runtime 不依赖 Panel 是否打开。

---

## 验收标准

- Scheduler 不引用 ScriptStore
- Scheduler 不认识 YAML
- Cron 可触发不同 Runner
- 服务重启后任务恢复
- Plugin UI 关闭不影响定时运行
- 缺少 Runner 时 Task 显示依赖错误，而不是服务启动失败

---

## 收口记录（2026-09-04）

- Original Plan：第 2 节「Cron WASM 负责 parse cron / register schedule semantics / trigger behavior」；第 4 节去 completion polling 改 `run.wait_terminal(run_id)`。
- Final Decision：Cron 保持 **Native Builtin Schedule Provider**（`cron_extension.rs` 经 `register_builtin` 注册缝接入 `ScheduleRegistry`，Scheduler/API 零直接依赖，update 门禁与诊断改用 `TimerCore::next_wakeup_in()/next_wakeup_at()`）；`wait_terminal` 已事件化落地（去 50ms 轮询）。
- Reason：cron 解析为纯计算、无权限/沙箱诉求，跨 WASM 边界只有成本；「TimerCore 不感知 schedule 语义」经 Registry 抽象同样达成，未来第三方触发语义可经独立 Adapter 接入（ADR-01，README 8.3）。命名保持 `CronExtension`——Native 也是 Extension，Extension ≠ WASM。
