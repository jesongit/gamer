# 示例集

> 本文给出 6 个完整（或可独立成段）的 v3 示例，覆盖基本流程、find + save +
> match 上下文、match_first 候选、call 函数与返回值、params + defaults、预算
> 与事件观察；全部语法以 [steps.md](steps.md) / [vision.md](vision.md) /
> [call.md](call.md) 为准。

## 1. 基本流程：tap / wait / log / if

```yaml
version: 3
steps:
  - app.start: com.example.game        # 冷启动（自动加 + 前缀）
  - wait: 3s
  - log: {level: info, message: 开始签到}
  - tap: [0.5, 0.9]                    # 相对坐标（tap 后自动等 after_tap）
  - key: {key: ESC, action: press}
  - swipe: {from: [0.8, 0.5], to: [0.2, 0.5], duration: 300ms}
  - set: {name: 已签到, value: false}
  - set: {name: 标记, value: {found: false}}     # find 的 save 只在命中时写入，先初始化
  - loop:
      times: 5
      steps:
        - find:
            template: 已领取标记
            timeout: 2s
            save: 标记                  # $标记.found 决定分支
            else:
              - log: 本轮未见标记
        - if:
            cond: $标记.found
            then:
              - set: {name: 已签到, value: true}
              - break
        - wait: {min: 500ms, max: 1500ms}           # 随机抖动等待
```

## 2. find + save + $match 上下文 + verify

```yaml
version: 3
defaults:
  vision:
    threshold: 0.85
steps:
  - find:
      template: 奖励弹窗
      timeout: 10s
      threshold: 0.9               # step 级覆盖 defaults
      save: reward
      then:
        - tap: {point: $reward.center}   # save 变量跨步可用
        - wait: 500ms
      else:
        - log: 未出现奖励弹窗
      verify:                      # then 执行完后二次确认操作生效
        template: 主界面
        timeout: 5s                # 不命中抛 VERIFY_FAILED: 主界面
```

要点：未 `save` 时 `$match` 只在 then/else/verify 体内可见；块结束复位 null
（此例的后续步骤引用不到 `$match`）；`save` 的命名变量跨步可用
（[expressions.md](expressions.md) §4）。

## 3. match_first 候选分流

```yaml
version: 3
steps:
  - match_first:
      candidates:
        - template: 领取奖励        # 首个命中候选执行自己的 steps
          steps:
            - tap: {point: $match.center}
            - log: 领取了奖励
        - template: 关闭弹窗
          threshold: 0.95           # 候选级 threshold
          steps:
            - tap: {point: $match.center}
        - template: 跳过引导        # 裸模板候选：命中但不执行额外步骤
      else:
        - log: {level: warn, message: 都没出现}
        - log: {level: debug, message: $match.matches}   # else 体内 $match = 整体结果
```

单帧多模板匹配（`vision.match_many`），按书写顺序取首个命中。

## 4. call 函数 + 返回值 + if

```yaml
# scripts/主任务.yaml
version: 3
params:
  - 'text:account:账号:"玩家一号"'
steps:
  - call:
      target: function:工具/月卡领取       # functions/工具/月卡.yaml 的「月卡领取」
      with: {user: $account}
      save: 领取结果
  - if:
      cond: $领取结果.ok
      then:
        - log: {level: info, message: 月卡已领取}
      else:
        - throw: 月卡领取失败
```

```yaml
# functions/工具/月卡.yaml —— bare-map，无 version 键
月卡领取:
  params:
    - 'text:user:玩家名'
  steps:
    - call:
        target: script:daily/login       # 脚本命名空间同样经 call
        with: {account: $user}
    - find:
        template: 月卡按钮
        timeout: 8s
        then:
          - tap: {point: $match.center}
          - return: {ok: true, count: 1}  # 返回值泛化：任意 JSON
        else:
          - return: {ok: false}           # 超时路径（无 else 时 find 直接抛 FIND_TIMEOUT）
```

要点：`function:` 短路径最后一段是函数名；`save` 收返回值整体，无 `return`
即 null；被调方变量空间独立（只有传入实参 + 自身默认值）。

## 5. params + defaults（表单化运行）

```yaml
version: 3
params:
  - 'text:msg:消息内容:"默认"'          # 字符串双形态
  - 'time:wait:等待:2s'                 # time = 带单位时长串（不是 number(ms)）
  - name: count                         # 映射双形态
    type: int
    remark: 刷本次数
    default: 3
defaults:
  vision:
    threshold: 0.85
  timing:
    after_tap: 200ms
    poll_interval: 250ms
steps:
  - log: $msg                           # 手动运行参数表单预填「默认」
  - log: {level: info, message: $wait}  # time 参数运行时为带单位时长串
  - log: {level: info, message: $count} # int 参数（数值实参以文本过线的限制见 params.md §5）
  - loop:
      times: 10                         # times 写字面量（int 实参显式传参时 times 取不到上限）
      steps:
        - find:
            template: 目标按钮
            timeout: 10s                # 时长位置建议写字面量（见 params.md §5 / vision.md §1）
            then:
              - tap: {point: $match.center}
        - wait: 1s
```

前端表单来自 schema API（`GET /api/runners/:runner_id/entrypoint`，前端不解析
YAML）；`count` 类型 `int` 在 schema 中为 `type: integer` + `param_type: int`；
缺必填 / 未知键 / 类型不符在运行前置 400 回填（[params.md](params.md)）。

## 6. 预算与运行事件（观察用片段）

```yaml
version: 3
steps:
  - loop:                    # 无 times = 无限循环；安全网是执行预算而非永久挂起
      steps:
        - wait: 100ms        # 即使纯空转体（无任何子步），每轮迭代本身也计数
```

- 预算：逻辑步 100_000、call 深度 32——上例空轮询最终以
  `STEP_BUDGET_EXCEEDED: consumed=N max=100000` 终止（而非永久挂起）；用户停止
  则为 `CANCELLED`。错误码进入运行日志，且先发 `budget {kind}` 事件再发
  `run_end {ok:false}`（[runtime.md](runtime.md) §2）。
- 运行事件（DataChannel `{"type":"se","ev":...}`）：每个步骤发
  `step_start {path, desc}` / `step_end {path, ok}`，path 形如
  `steps[0].steps[1]`（顶层第 2 步 loop 体内第 2 步）——前端 ScriptSummary 按
  path 高亮顶层卡片，匹配另有 `vision {template, found, score, center}` 与
  `hit`/`miss` 投屏标记事件。
