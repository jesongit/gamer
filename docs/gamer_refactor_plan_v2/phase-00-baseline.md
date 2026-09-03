# Phase 0：基线、Benchmark 与兼容性护栏

## 目标

在任何结构性重构前建立可重复验证的性能和行为基线，避免后续出现“感觉更快”“应该没坏”的不可验证状态。

## 本阶段不做

- 不改插件架构
- 不引入 WASM
- 不改变 YAML 语义
- 不改变 App Package 存储方式
- 不重写 WebRTC / scrcpy

## 任务

### 1. 建立核心行为测试集

覆盖：

- 设备发现 / 连接 / 断开
- scrcpy 启动与重连
- WebRTC 投屏
- Screenshot
- 单模板匹配
- 一帧多模板匹配
- tap / swipe / key / text
- App start / stop
- YAML Script Run
- Run cancel
- Scheduler Task
- Keymap
- 日志写入
- 服务重启后的任务与状态恢复

### 2. 固定测试数据

建立 `tests/fixtures/`：

```text
fixtures/
├── scripts/
├── keymaps/
├── templates/
├── tasks/
└── screenshots/
```

至少包含：

- 一套真实脚本
- 一套真实 Keymap
- 多个不同尺寸模板
- 一组成功 / 失败匹配图片
- 一组典型定时任务

### 3. 性能指标

记录：

- server binary size
- server idle RSS
- 空闲 CPU
- scrcpy session 启动耗时
- screenshot P50/P95
- template match P50/P95
- match-many P50/P95
- 典型脚本总耗时
- GOP cache 峰值
- DB log 高压写入时延迟
- WebRTC 投屏稳定性

### 4. 建立对照输出

建议输出：

```text
benchmarks/
└── baseline.json
```

示例字段：

```json
{
  "idle_rss_mb": 0,
  "screenshot_p95_ms": 0,
  "find_p95_ms": 0,
  "match_many_p95_ms": 0
}
```

## 验收标准

- 所有核心场景可以重复运行
- 结果可机器比较
- 后续阶段可明确判断性能回退
- 至少保存一个 release build 基线
- 关键 YAML / Keymap / Task 有兼容样本

## 回滚点

本阶段只增加测试和测量，不应改变运行逻辑。
