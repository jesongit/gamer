# ADR-12：统一 Task 模型

> 位置说明：Phase 11 计划原文路径为 `docs/gamer_refactor_plan_v2/adr/`；2026-09-05 docs 已重组为 reference/guides/plans/evidence 四子目录，ADR 属长期有效的架构决策记录，故定位于 `docs/reference/adr/`。

状态：ACCEPTED（2026-09-05）

## 背景

当前 TaskBoard 与旧 Task API 围绕 `cron` / `script_id` / `device_id` / `args` 设计，即 `Task = YAML Script Cron Task`。这使 Task 无法承载未来的 Macro、第三方 Scheduler 等形态，也把 YAML 语义钉进了 Core。

## 决策

Task 统一为以下模型：

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

- `schedule = { provider_id, config }`：调度语义由 ScheduleProvider 解释。例如 `gamer.cron` + `expression`；未来可接 `gamer.interval` / `gamer.manual` / `thirdparty.calendar`。
- `runner = { runner_id, entrypoint, payload }`：执行语义由 Runner 解释。例如 `gamer.yaml` + `daily/login` + args；未来可接 `gamer.macro` 等。
- `state` 含 `DEPENDENCY_MISSING` 等运行依赖状态；`metadata` 为通用键值，不承载调度/执行语义。

Task 与 `script_id`、`cron` 顶层字段彻底无关——`script_id` / `cron` / `script_args` / `script_path` 等旧字段整体删除，YAML 参数改存于 `runner.payload`、cron 表达式改存于 `schedule.config`。

一句话：**Task = 任意 ScheduleProvider + 任意 Runner**。Core 只认这两个抽象，不认任何具体 provider / runner 的业务语义。

## 后果

- P11.1 按此模型重做 Task 数据层、API 与 TaskBoard；P11.7 删除旧 Task API / `/api/user-tasks` / 重复 presets 入口。
- 新增调度方式（interval 等）或执行形态（macro 等）只表现为注册新的 provider / runner，不改 Task 模型与 TimerCore。
- 旧任务数据不迁移（见 ADR-14）：本地开发环境直接删除重建。
