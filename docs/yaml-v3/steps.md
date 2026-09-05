# 步骤（19 类）

> 本文逐一给出 v3 全部 19 类 surface 步骤的语法、字段与语义，并给出被移除
> v2 语法的迁移对照；实现核对基准为 `server/src/extensions/gamer_yaml/
> yaml_vnext.rs`（`parse_step` / `Lowerer::step`，SurfaceStep 19 类）。所有
> 步骤均为**单键映射**（动作键 → 载荷），非单键结构报 `yaml.v3.step.shape`；
> `break` / `app.start` / `app.stop` 允许裸标量简写。未知动作键报
> `yaml.v3.step.unknown`。

## 1. 输入

### tap

```yaml
- tap: [0.5, 0.3]               # 二元数值数组 → 坐标字面量
- tap: {point: $reward.center}  # 或 {at: ...}；point/at 双键等价
```

| 字段 | 必填 | 说明 |
|---|---|---|
| `point`（或 `at`）/ 直接值 | 是 | 相对坐标 0~1（`Coordinate`） |

语义：`input.tap` → sleep(`after_tap`，内置 300ms)。tap 后等待是 lower 展开物
（非 surface 步、不发 step 事件），经 `runtime.sleep` 实现故可被「停止」取消。

### swipe

```yaml
- swipe:
    from: [0.2, 0.8]
    to: [0.8, 0.2]
    duration: 300ms        # `time` 为别名键；必填
```

语义：`input.swipe`（20 段插值滑动）。缺 duration 报 `yaml.v3.field.missing`。

### key

```yaml
- key: HOME
- key: {key: ENTER, action: press}
```

| 字段 | 必填 | 说明 |
|---|---|---|
| `key` | 是 | 键名或数字码（HOME/BACK/MENU/APP_SWITCH/VOL_UP/VOL_DOWN/ESC/ENTER/SPACE/TAB/BACKSPACE/DEL 或 Android keycode） |
| `action` | 否 | `down` / `up` / `press`（缺省 press）；非法值报 `yaml.v3.key.action` |

### text

```yaml
- text: hello
- text: {value: $账号}
```

语义：`input.text`；value 必须是字符串。

## 2. 时序

### wait

```yaml
- wait: 300ms                    # 固定（标量或 {duration: ...}，`time` 为别名键）
- wait: {min: 300ms, max: 700ms} # 随机区间
```

语义与随机细节见 [timing.md](timing.md)。

## 3. 应用

### app.start / app.stop

```yaml
- app.start                    # 当前分区包名
- app.start: com.example.game  # 标量 / {package: ...} / {app: ...} 均可
- app.stop: com.example.game
```

语义：`device.start_app` / `device.stop_app`。`app.start` 自动加 `+` 前缀 =
冷启动（str_app 语义）。无 viewer 无脚本时设备可能已被空闲低功耗拆会话，下次
运行自动重连（~2-4s）。

## 4. 观测

### log

```yaml
- log: 开始领取                  # 标量；level 缺省 info
- log: {level: warn, message: $错误}
```

`level` 允许 `trace` / `debug` / `info` / `warn`（`warning`）/ `error`。语义：
`log.write` 进运行日志。

## 5. 数据

### set

```yaml
- set: {name: 计数, value: 3}   # 显式 {name, value}
- set: {计数: 3}                # 单键映射简写
```

语义：把表达式求值结果写入变量 `计数`（见 [expressions.md](expressions.md)）。
两种形态混用其他键报 `yaml.v3.set.shape`。

## 6. 控制流

### if

```yaml
- if:
    cond: $reward.found         # 通用表达式，按 truthy 判定
    then:
      - log: 命中
    else:                       # 可选
      - log: 未命中
```

`cond` 求值失败（未定义变量）按运行错误终止。truthy 口径见
[expressions.md](expressions.md)。

### loop / break

```yaml
- loop:
    times: 5                    # 可选；缺省 = 无限（体内须有 break/throw 或可被取消）
    steps:
      - tap: [0.5, 0.5]
      - wait: 1s
      - if:
          cond: $done
          then:
            - break             # 跳出最近一层 loop
- break
```

- `loop` 必填 `steps`；`times` 接受整数字面量或 `$var` 引用。
- 循环每轮迭代本身也计执行预算（空转体死循环同受约束，见
  [runtime.md](runtime.md)）。
- `break` 允许裸标量（`- break`）。

### throw

```yaml
- throw: 登录失败                # 标量或 {message: ...}；message 必填
```

语义：以 message 文本终止运行（错误文本进入 RunRecord / run_end 事件）。

## 7. 调用

### call / return

见 [call.md](call.md)（命名空间、`with`/`args`、`save`、返回值泛化、深度 32）。

```yaml
- call:
    target: script:daily/login
    with: {account: $user}
    save: result
- return: {ok: true, count: 3}
```

## 8. 视觉

find / match_first / check 全语法见 [vision.md](vision.md)。

```yaml
- find:
    template: 奖励弹窗
    timeout: 10s
    save: reward
    then: [{tap: {point: $reward.center}}]
    else: [{log: 未找到}]
    verify: {template: 主界面, timeout: 5s}
```

## 9. invoke —— 通用 Capability 逃逸口

```yaml
- invoke:
    capability: vision.sample_color
    with: {point: [0.5, 0.5]}
    save: 颜色
```

| 字段 | 必填 | 说明 |
|---|---|---|
| `capability` | 是 | Capability 名（见下） |
| `with`（`args` 别名） | 否 | 参数名 → 表达式 |
| `save` | 否 | 返回值存入变量 |

可用 capability（宿主授权表 `NativeYamlHost::authorize`）：`app.start` /
`app.stop` / `input.tap` / `input.swipe` / `input.key` / `input.text` /
`vision.match` / `vision.match_many` / `vision.sample_color` / `frame.capture` /
`runtime.sleep` / `log.write` / `device.resolve`。未知名直接报「未知
capability」。典型用途：`vision.sample_color` 取色（返回
`{red, green, blue, hex}`，`hex` 形如 `1a2b3c`）配合 `if` 做颜色分支（替代已
删除的 color_branch，见下）。

## 10. 已移除语法与迁移对照

校验器对已删除语法给**专属迁移诊断**（`yaml.v3.step.removed` /
`yaml.v3.field.removed`），错误信息含迁移提示（`removed_step_message`）：

| 移除项 | 诊断码 | v3 表达 |
|---|---|---|
| `func` step（裸 `文件/函数`） | `yaml.v3.step.unknown`（func 不在动作键集） | `call` + `function:<文件短路径>/<函数名>`（ADR-YAML-02） |
| `wait_for` | `yaml.v3.step.removed` | 与 find 同义 → `find`（then=命中分支、else=超时分支） |
| `retry` | `yaml.v3.step.removed` | `loop: {times: N, steps: [...]}` |
| `click_when` | `yaml.v3.step.removed` | `find.then` + `tap: {point: $match.center}`（ADR-YAML-03） |
| `color_branch` | `yaml.v3.step.removed` | `invoke: vision.sample_color`（save）+ `if` 按 `$<变量>.hex` 分支 |
| `find.click: true` | `yaml.v3.field.removed` | `find.then` + `tap: {point: $match.center}` |
| match_first 候选 `click: true` | `yaml.v3.field.removed` | 候选 `steps` + `tap: {point: $match.center}` |
| match_first 顶层 `then` | `yaml.v3.field.removed` | 命中步骤写在每个候选的 `steps` 里 |
| v2 `config` 顶层块 | `yaml.v3.top_level.unknown_key` | `defaults`（vision/threshold + timing，见 timing.md/vision.md） |

设计原则（计划 §22）：优先增加**通用数据模型与通用 call**，而不是专用 Step/
专用语法；同一专用语法家族不留别名。
