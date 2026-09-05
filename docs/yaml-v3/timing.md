# 时序：defaults.timing 与 wait

> 本文定义 v3 的时序模型：Program 级 `defaults.timing` 三项、wait 双形态与
> 取消语义；权威裁决见 [ADR-YAML-01](../reference/adr/ADR-YAML-01-v3-only.md)
> （废弃隐藏 config timing），实现核对基准为
> `server/src/extensions/gamer_yaml/yaml_vnext.rs`（`parse_defaults` /
> `Lowerer` timing 展开 / `check_wait_range`）。

## 1. defaults.timing 三项

```yaml
version: 3
defaults:                     # 可选；只允许 vision/timing 两组键
  timing:
    after_tap: 300ms          # 每次 tap 后等待（内置 300ms）
    after_match: 200ms        # find/check/match_first 命中后等待（内置 200ms）
    poll_interval: 100ms      # find/check 轮询间隔（内置 100ms）
steps:
  - ...
```

| 项 | 语义 | 内置兜底 |
|---|---|---|
| `after_tap` | 每次 `tap` 之后的等待 | 300ms |
| `after_match` | find / check / match_first **命中后**、进入 then/steps/继续 之前的等待 | 200ms |
| `poll_interval` | find / check / find.verify 的轮询间隔 | 100ms |

- **取值形态**：带单位时长字面量（`300ms` / `2s` / `1m` / `1h`）或非负整数
  毫秒；**不接受 `$var` 引用**（timing 兜底在 lower 期展开，报
  `yaml.v3.defaults.type`）。
- **未知键**：`defaults.timing` 只允许上述三项，其他键（如 v2 的
  `judge_delay`）报 `yaml.v3.defaults.unknown_key`；`defaults` 顶层只允许
  `vision` / `timing`。
- **展开时机**：lower 期把 tap 后 / 命中后等待展开为显式 `runtime.sleep`、把
  轮询间隔展开进轮询体——0ms 展开省略；它们不是 surface 步骤（不发 step 事
  件、不占步骤序号）。
- **可取消**：sleep 走 `runtime.sleep` 既有取消路径，用户「停止」可打断等待。
- **函数库无 defaults 块**（bare-map 结构）：一律走内置兜底。
- 完整 defaults 结构（含 `vision.threshold`）见
  [program.md](program.md)、[vision.md](vision.md)。

## 2. wait 双形态

```yaml
- wait: 300ms                        # 固定等待（标量；或 {duration: 300ms}，time 为别名键）
- wait: {min: 300ms, max: 700ms}     # 随机区间
```

- **随机区间**：`min` / `max` 必须**同给**（缺一报 `yaml.v3.field.missing`）且
  `min ≤ max`（字面量可比时解析期校验，违反报 `yaml.v3.wait.range`；`$var`
  引用留待运行期，区间退化为 min）；不得与 `duration` 混用（报
  `yaml.v3.field.unknown`）。
- **随机实现**：宿主每 run 注入随机 nonce，guest 内 splitmix64 在
  `[min, max]`（毫秒，含端点）取值——同一次运行内多次随机 wait 的序列由同一
  种子连续推进；取值后经 `runtime.sleep` 等待（可被停止取消）。
- 固定与随机 wait 都**可被取消**，且计执行预算（一个逻辑步）。

## 3. 时长书写

时序位置（wait / duration / timeout / defaults.timing 各项）接受：

- 带单位串：`300ms` / `2s` / `1m` / `1h`（单位仅 ms/s/m/h，非法报
  `yaml.v3.duration`）；
- 非负整数：解释为毫秒；
- `$var` 引用（仅步骤字段位置；defaults.timing 不接受）。

## 4. 无隐藏全局时序原则

v2 引擎从 config.toml 读取的隐藏 interval / judge_delay 已废弃
（[ADR-YAML-01](../reference/adr/ADR-YAML-01-v3-only.md)）：**脚本行为必须
自包含**——点击后等待、命中后判定等待、轮询节奏全部由脚本 `defaults.timing`
显式声明（缺省用内置值），不存在跨脚本共享的全局 magic timing；时序参数随脚
本走，改脚本即改行为，换环境不漂移。
