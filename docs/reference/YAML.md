# YAML 脚本语法（v2）

GameBot 自动化脚本的权威语法文档（2026-08 重写）。本文描述当前唯一受支持的
YAML v2 严格语法；不提供旧格式兼容或自动迁移。规则来源：

- 契约：`docs/reference/SCRIPT_EDITOR_CONTRACT.md`（与当前实现、fixture 同步）；
- 可执行样例：`server/tests/fixtures/script_v2/`（本文所有示例与其同形态，装载由
  `server/src/extensions/gamer_yaml/script_v2/`（装载/校验/序列化）+
  `server/src/extensions/gamer_yaml/engine/`（执行，由 gamer.yaml 扩展承载）保证）；
- 前端：可视化编辑器（`web/src/script-editor/`）以此为唯一编辑模型，保存时由服务端
  统一序列化为本文的「规范 YAML」。

> **YAML v3**（Phase 12 起，ADR-YAML-01：唯一正式方案）语法见本文末尾
> [§11 YAML v3](#11-yaml-v3phase-12-语法契约)；本节其余部分描述 v2。

## 1. 目录与资源边界

脚本、函数库、模板按**应用分区**（设备配置的 pkg，即应用包名）存放，目录即类型：

```
data/<pkg>/
├── scripts/     # 可运行脚本（.yaml/.yml，顶层必须有 steps）
├── functions/   # 函数库（严格 .yaml，顶层键全是函数名）
├── templates/   # 模板图片（默认 8-bit 灰度 PNG，文件名可带 # 搜索区/#1 颜色后缀）
├── keymaps/     # 按键映射方案（WASM keymap 扩展 profile 数据源）
├── presets/     # App Package 发布的任务预设
└── resources/   # 包附带的其他资源
```

- **解析优先级**：同名资源按 **EditableLocal（分区目录）→ UserOverride →
  active App Package** 三层解析，高层覆盖低层；本地分区目录即本地编辑区
  （可执行脚本只能位于 `data/<pkg>/scripts/`，函数库 `functions/`，
  模板 `templates/`）。
- **脚本资源 ID** = `<pkg>/<文件名>.yaml`（如 `daily/login.yaml`，可含子目录）。
  含 `/`，前端拼 URL 必须整体 `encodeURIComponent`。
- **函数路径** = `<文件短路径>/<函数名>`（如 `common/login` = `functions/common.yaml`
  里的 `login`；一个函数库文件可定义多个函数）。
- **运行边界**：只有 `scripts/` 下的脚本可手动运行 / 立即运行 / 进入定时任务；
  `functions/` 只能被 `func` 步骤调用或走函数测试 API，不进脚本列表与任务选择器。
- **不做内容推断**：`scripts/` 里必须有顶层 `steps`；`functions/` 顶层键全是函数名。
  放错目录按该目录的类型校验，报错即拒。
- **跨分区一律不解析、不回退**：模板 / 函数 / 子脚本只在当前应用分区查找，
  没有 default 兜底；其他目录布局不属于当前资源，也不会被读取或迁移。
- **模板引用写短名**（如 `account.png`）。磁盘文件名可带 `#` **搜索区后缀**
  （后缀在扩展名前，如 `xx#l.png`）：
  - 半区码：`a`=全屏、`u`/`d`/`l`/`r`=上/下/左/右半、`ul`/`ur`/`dl`/`dr`=四角；
  - 数字坐标：`xx#x1_y1_x2_y2`，四段各为相对坐标 ×1000 的整数（如
    `xx#0_0_500_500` = 左上 1/4 区域），需 x2>x1、y2>y1。
  - 颜色标记：末尾再加 `#1`（如 `xx#0_0_500_500#1.png`）表示保留颜色，
    并在灰度 NCC 命中后复核颜色；不带 `#1` 的旧格式均按灰度匹配，脚本 YAML 无需颜色参数。
  脚本写 `xx.png` 而磁盘存在 `xx#l.png` 时按「基名 + `#` 后缀 + 同扩展名」唯一
  匹配；零候选报不存在、多候选报歧义（`resource.tmpl.ambiguous`），不猜测。

## 2. 顶层结构

### 2.1 脚本（scripts/）

顶层只允许 **params / config / steps** 三个键（顺序不限）：

```yaml
params:                     # 可选：参数声明（见 §3）
  - 'bool:enable:是否启用:true'
config:                     # 可选：运行配置（见 §4）；整体省略 = 用 config.toml 同名键
  interval: 500ms
  threshold: 0.85
  log_level: info
steps:                      # 必需：可为空列表 steps: []，但不可省略
  - log: 最小脚本
  - tap: [0.5, 0.5]
```

- 顶层出现白名单之外的任何键报 `script.top_level.unknown_key`；服务端不区分旧格式，
  也不生成迁移引导。根节点必须是映射；`steps` 缺失报 `step.field.missing`
  （field=steps）。

### 2.2 函数库（functions/）

顶层键 = 函数名（保持书写顺序），每个函数记录只允许 **params / steps** 两个键；
**没有文件级 config**。函数内的 `steps` 同样必需（可空列表不可省略）。
函数名字符集：unicode 字母/数字/`_`（支持中文，如 `登录确认`），不能以数字开头，
且不得撞动作键/结构键保留字（`log`/`find`/`then`/`steps` 等，服务端浅校验拦截）：

```yaml
login:
  params:
    - 'tmpl:account:账号模板:account.png'
    - 'time:timeout:等待时间:30s'
  steps:
    - find: $account
      timeout: $timeout
    - return: true

is_enabled:
  params:
    - 'bool:enable:开关'
  steps:
    - return: $enable
```

- `return` 只允许出现在函数库（脚本里出现报 `step.return.in_script`）；
- 函数体正常走完未 `return` 视为返回 `true`；
- 函数库不能直接运行 / 调度，只能经 `func` 步骤调用或函数测试 API（§7）。

## 3. 参数声明 params

### 3.1 声明格式

每条声明是一个**整条单引号**的 YAML 标量（无引号非法 → `param.decl.quote_style`）：

```yaml
params:
  - 'tmpl:account:账号模板:account.png'
  - 'coord:click_pos:点击位置:[0.5, 0.8]'
  - 'color:target_color:目标颜色:ff8800'
  - 'time:timeout:最长等待:30s'
  - 'key:cancel_key:取消按键:ESC'
  - 'text:message:提示文本:"示例文本"'
  - 'bool:enable:是否启用:true'
  - 'text:api_url:服务地址:https://example.com:8443'   # 第 4 段整体是默认值，可含冒号
```

固定四规则：

1. 格式 `类型:变量名:备注[:默认值]`，按第 3 个冒号切分（`splitn(4)`）：前三段必需
   且非空，第 4 段整体为默认值尾串——因此 text 默认值可含半角冒号。类型/变量名/
   备注本身不得含半角冒号（备注需要冒号用全角 `：`）→ 违反报 `param.decl.format`。
2. **整条单引号**：无引号 plain 标量在尾串为空时会变成映射、样式信息也会丢失，
   故强制单引号。
3. 变量名 `[A-Za-z_][A-Za-z0-9_]*`（`param.decl.name_invalid`），同一参数表内唯一
   （`param.decl.name_duplicate`）；保留名 `true` / `false` / `null` 与 `gb_` 前缀
   不可用。
4. **空默认值非法**：第 4 段存在但为空（如 `'text:x:名:'`）不等价于没有默认值 →
   `param.default.empty`。省略第 4 段 = **必填**；带默认值可在调用 / 运行 args 中省略。

### 3.2 七类类型与默认值

| 类型 | 默认值写法示例 | 约束 |
|---|---|---|
| tmpl | `account.png` | 当前分区存在的模板短名 |
| coord | `[0.5, 0.8]` | 两个 0~1 的数字（相对坐标） |
| color | `ff8800` | 6 位十六进制，无 `#`；**前导零靠引号保住**（见下） |
| time | `30s` | 单位 ms/s/m/min/h/d（m≡min，可小数），必须 >0 |
| key | `ESC` | 按键名（§5.1 key 表） |
| text | `"示例文本"` | 可含冒号/空格；空串必须写 `""`；外层双引号会被剥离 |
| bool | `true` | 仅字面 `true`/`false`（字符串 `"true"` 非法） |

- 默认值按声明类型解析，非法报 `param.default.invalid`；
- **color 是字符串不是数字**：所有位置（声明默认值、expect 候选、args、任务快照、
  运行记录统一为 6 位十六进制无 `#`。纯数字色值在 YAML 里**必须加引号**
  （`'123456'`）防止被解析成数字丢前导零；含字母色值（`ff8800`）可裸写，编辑器
  规范序列化时对纯数字色值保留引号，含字母色值可裸写。

### 3.3 `$name` 引用

步骤字段处写 `$参数名` 即**完整值引用**（绑定声明类型的值，不做全文替换）：

```yaml
steps:
  - tap: $click_pos        # 引用 coord 参数
  - find: $account         # 模板字段也可引用
    timeout: $timeout
```

- 引用必须占据整个标量（不支持把 `$name` 嵌在文本中间插值）；
- 字段类型与参数类型不符报 `param.ref.type_mismatch`，未声明的名字报
  `param.ref.unknown`；
- match 候选模板键与 color 候选色键同样接受 `$name`（见 §5.3/§5.5）；
- `$name` 只在引用处生效：call/func 的 `args`、入口运行参数按名字绑定（§6）。

## 4. config

```yaml
config:
  interval: 500ms     # 轮询与点击后等待间隔（find/match 轮询、所有脚本点击后），带单位 >0
  threshold: 0.85     # 模板匹配阈值，0~1
  log_level: info     # debug / info / warn / error，低于等级的日志丢弃
```

整体省略 = 使用 `config.toml` 同名键（`interval` / `threshold` / `log_level`）。
不允许未知 config 键；只能是映射（v1 的「映射列表按序覆盖」写法已删除）。

另有仅全局生效的 `config.toml` 键 `judge_delay_ms`（默认 200，0=关闭，脚本 config:
不覆盖）：find / match / color 的**命中路径**在执行后续分支步骤前固定等待该时长
（给游戏 UI 留响应时间）；若命中路径发生点击，先等待 `config.interval`，再追加
`judge_delay_ms`；分支为空（无后续步骤）不追加 `judge_delay_ms`，else / 超时路径不延迟。

## 5. 步骤（18 种）

一个步骤只允许一个动作键（多动作键 → `step.multi_action`）；动作键之外的同级键是
该步骤的字段。步骤按书写顺序执行；空分支 / 默认字段在 YAML 里省略（编辑器保存
的规范 YAML 同样省略）。分支子列表（`then` / `else` / 候选分支 / `loop.steps` /
函数体）递归为步骤列表。

### 5.1 基础动作

```yaml
steps:
  - str_app                     # 冷启动当前分区应用（先 force-stop 再启动）；裸写，带值非法
  - tap: [0.5, 0.5]             # 点击（相对坐标或 $name），完成后等待 config.interval
  - swipe:                      # 滑动：YAML 键为 fm/to/time
      fm: [0.1, 0.9]
      to: [0.9, 0.1]
      time: 800ms
  - key: ESC                    # 按键
  - text: "hello world"         # 输入文本
  - log: 全动作脚本              # 写一条 info 运行日志
  - wait: 1s                    # 固定等待
  - wait: [1s, 3s]              # 随机区间等待（含两端；起点>终点报 step.wait.range_invalid）
  - cls_app                     # 关闭当前分区应用（adb force-stop，投屏不中断）
```

- `str_app` / `cls_app` 不带参数，包名 = 运行分区（设备配置的应用包名）。
- `key` 支持的按键名：`HOME` `BACK` `MENU` `APP_SWITCH`（=`RECENTS`）`VOL_UP`
  （=`VOLUME_UP`）`VOL_DOWN`（=`VOLUME_DOWN`）`POWER` `ENTER` `DEL`（=`BACKSPACE`）
  `TAB` `SPACE` `ESC` `SEARCH` `CAMERA` `FOCUS` `NOTIFICATION` `SETTINGS` `MUTE`
  `HEADSETHOOK` `WAKEUP` `SLEEP` `0`~`9`；纯数字按 Android keycode 透传。
- `time` 一律带单位（ms/s/m/min/h/d），缺单位报 `step.time.format`。

### 5.2 find —— 等待模板出现并点击

```yaml
- find: $account                # 主模板（短名或 $name）
  block:                        # 可选：障碍模板列表，依序处理
    - popup.png
    - dialog.png
  verify: true                  # 可选：默认 false
  timeout: $timeout             # 可选：默认 30min，必须 >0
  then:                         # 可选：命中后执行（默认无）
    - log: 已进入主界面
  else:                         # 可选：超时后执行（默认无）
    - throw: 等待超时
```

每轮：主模板（**新截图**）命中 → 恒点**模板中心** → 等 `config.interval`；
`verify: true` 时重匹配一次，仍命中补一击（补点后也等 `config.interval`，共两击，
适合点击后弹窗关闭类按钮）→ 执行 `then` 结束本步；未命中 → `block` 依序匹配
（命中即点其中心并等 `config.interval`，结束本轮）→全未命中等 `config.interval`
重开一轮。超过 `timeout` 执行 `else`。截图瞬态失败跳过本轮重试（持续失败约 20s
判链路异常带因中止）。

### 5.3 match —— 多模板策略选择

`match` 的候选列表是**紧凑缩进**（无缩进序列，唯一序列化格式）；`else` /
`timeout` 是 `match` 步骤的**兄弟键**，与 `match` 同列。候选值二选一：
**分支步骤列表**（不点击，原形态），或**映射 `{click: true, steps: [...]}`**
（命中后点击该候选模板匹配框的中心；`steps` 省略 = 空分支，即「命中即点」）：

```yaml
- match:
  - test1.png:
    - log: 命中 test1
  - test2.png:
      click: true                  # 命中 → 点击 test2 匹配框中心，无分支步骤
  else:
    - log: 都未命中
  timeout: 30s
```

- 每轮只截**一帧**，候选按书写顺序匹配、首个命中获胜、执行其分支步骤并结束本步；
  默认不点击，候选可各自 `click: true`（点中该候选的匹配框中心，完成后等待
  `config.interval`，语义同 find）。
- 未配 `timeout` 只执行一轮（全未命中立即进 `else`）；配了按 `config.interval`
  轮询到超时才进 `else`。
- 规范序列化不变式：`click: false` ⇔ 列表形态，`click: true` ⇔ 映射形态
  （`click`/`steps` 键比候选模板键深两级——映射值不能与键同列）；候选值映射内
  只允许 `click`/`steps`（其余 → `step.field.unknown`），`click` 非布尔字面量 →
  `step.field.type_mismatch`。
- 候选模板短名不可重复（装载期与参数绑定后都查重 →
  `step.match.candidate_duplicate`）；不接受布尔条件（布尔走 `if`）。
- `- else:` / `- timeout:` 写进候选列表是错误（`step.match.else_in_candidates`）。

### 5.4 check —— 界面断言（轮询匹配，未命中终止）

```yaml
- check: logo.png               # 模板短名或 $name
```

- 省略 `timeout` 时默认 5s；配置后在该时间内按 `config.toml` 的 `interval` 重复截图匹配；`timeout: 0` 只检查首帧。不点击、无分支。
- 命中 → 推送命中框可视化、记「检查通过」日志后继续后续步骤。
- 超时仍未命中 → 记「检查未通过」日志（含 `throw` 文案），按 `throw` 步骤同语义
  **结束整个运行**（含调用链）。
- `timeout` 省略时默认为 5s，必须带时间单位且大于等于 0；`throw` 省略时使用
  `模板名 模板不存在`，显式值必须为非空字符串；模板字段 `throw` 与动作键 `throw` 同名词，
  步骤内存在 `check` 键时该键固定解析为字段，不算第二个动作键。

### 5.5 color —— 单点颜色分支

`at` 与 `expect` 写在 `color` 值映射内；`expect` 是**有序列表**（不用颜色做映射键，
防解析器重排），每项单键映射的候选值二选一：**分支步骤列表**（不点击，原形态），
或**映射 `{click: true, steps: [...]}`**（命中后点击取样点；`steps` 省略 =
空分支）；`else` 与 `color` 键同列：

```yaml
- color:
    at: [0.5, 0.5]
    expect:
      - ff8800:
        - tap: [0.5, 0.5]
      - '123456':
          click: true              # 命中 → 点击取样点，无分支步骤
  else:
    - throw: 颜色未命中
```

- 一次截图、按序判色：实际像素与期望色每通道差 ≤30 视为命中（容差固定 30，吸收
  H.264 有损压缩抖动），命中即执行该色分支并结束本步；全未命中走 `else`。
- **不轮询**（重试套 `loop`）；默认不点击，候选可各自 `click: true`（点取样点，
  完成后等待 `config.interval`，语义同 find 的中心点击）。规范序列化不变式与错误码
  同 §5.3 match 候选。
- 同色候选重复 → `step.color.duplicate`；颜色格式非法 → `step.color.format`；
  纯数字色值必须加引号（§3.2）。

### 5.6 if / loop

```yaml
- if: $enable                   # 条件严格布尔（bool 参数或 true/false），无隐式转换
  then:
    - tap: [0.5, 0.5]
  else:
    - log: 未启用

- loop:                         # times 省略或 0 = 无限（10 万步 guard 兜底，见 §5.8）
    times: 3
    steps:
      - wait: 1s

- loop:                         # 0 次数表示无限，可用 break 跳出最近一层 loop
    steps:
      - break
```

- `if` 条件非布尔报 `step.if.non_bool_cond`；
- `loop` 值是映射：`times` 为非负整数字面量，省略时默认值为 `0`（`0` 表示无限）；
  `steps` 必需且非空
  （缺失 → `step.field.missing`，空 → `step.loop.empty_steps`）。
- `break` 必须位于 loop 子流程内，执行后跳出最近一层 loop；放在 loop 外报
  `step.break.outside_loop`。

### 5.7 call / func —— 子脚本与函数

```yaml
- call: sub/inner.yaml          # 调用同分区 scripts/ 脚本（缺 .yaml 自动补全）
  args:                         # 具名实参（稀疏：未给的参数走声明默认值）
    enable: $enable
    message: "字面量消息"

- func: common/login            # 调用 functions/common.yaml 里的函数 login
  args:
    account: $account
    timeout: 30s
  then:                         # 返回 true → then
    - log: 登录成功
  else:                         # 返回 false → else
    - throw: 登录失败
```

- **call**：压入被调脚本的 `config` 三键覆盖与新的参数作用域，返回后恢复调用者的
  config 与作用域；没有布尔分支。
- **func**：`<文件短路径>/<函数名>`；**继承调用点 config**（不覆盖）；函数体内
  `return: true/false` 立即返回，走完未 return 默认返回 `true`；返回布尔驱动
  调用点 `then` / `else`。
- 两者共用约束：目标必须同分区（跨分区不解析）；路径穿越 / 绝对路径 / 反斜杠报
  `ref.call.path_traversal` / `ref.func.path_traversal`；call 自身 / 跨文件环报
  `ref.call.self_cycle` / `ref.call.cross_cycle`；args 未知键报
  `param.args.unknown`、类型不符报 `param.args.type_mismatch`、必填缺失报
  `param.args.missing_required`。

### 5.8 throw / return 与运行护栏

```yaml
- throw                         # 无原因
- throw: 余额不足                # 带原因
- return: true                  # 仅函数库合法（脚本里报 step.return.in_script）
- return: $enable
```

- `throw` 立即结束整个运行（跨 call/func 调用链），运行以失败终态收场
  （`runtime.engine.throw`，携带原因）。
- `return` 只退出当前函数，值必须是布尔。
- `break` 只退出最近一层 loop，不影响外层调用链。
- **护栏**：call+func 合计嵌套上限 **32 层**（超限 `runtime.nesting.limit`）；
  单次运行累计执行 **10 万步**（含循环体与嵌套子步骤，超限
  `runtime.step.limit` 强制终止）；「停止」在长 wait 中分片（200ms）生效。

## 6. 调用参数绑定

绑定顺序冻结为：**声明默认值 → 显式 args / 入参覆盖**，绑定完成后再做类型校验。

- call/func 的 `args` 是稀疏映射：没给的参数用声明默认值；必填参数没给 →
  `param.args.missing_required`；
- 实参可以是 `$name` 引用（类型须与目标参数一致）或字面量（按目标参数类型解析，
  如 `timeout: 30s` 定型为 time）；
- 手动运行 / 函数测试的入口 `args` 同为稀疏映射（§7）；显式传 `null` 不触发
  默认值，仅 text 接受空串；
- `$name` 引用的查找：call/func 进入压入新作用域，**最内层优先**——被调脚本的
  参数遮蔽调用者同名参数。

## 7. 运行入口与参数

### 7.1 手动运行 / 从步骤运行（Console）

统一执行入口（Console 的 yaml 面板经 `runYamlScript` 包装调用）：

```
POST /api/runs   body { runner_id: "gamer.yaml", entrypoint: "<pkg>/<文件名>.yaml",
                       device_id, payload: { start_index?, args? } }
→ 202 { run_id, state, resolved_args }
```

- `id` = `<pkg>/<文件名>.yaml`；`args` 为稀疏映射：bool=布尔、coord=`[x, y]`
  数组、其余五类=字符串（time 带单位、color 6 位十六进制、tmpl/key 非空）；
- `resolved_args` 是「默认值 → 覆盖」合并后的**全量绑定视图**，前端在运行日志区
  展示本次实际生效的参数；
- `start_index` = 主流程顶层步骤序号（0=从头；Console 摘要卡片「从此步骤运行」
  传入；越界回退从头）；
- args 解析 / 脚本校验失败 → `400 {error:"invalid_args", diagnostics:[五元组]}`；
- 运行实例查询 / 取消：`GET /api/runs/:run_id`、`POST /api/runs/:run_id/cancel`。

### 7.2 函数测试（编辑器 / Console）

同一 `POST /api/runs` 入口，`entrypoint` 带函数后缀：
`<pkg>/<文件短路径>.yaml[#函数名]`（缺省后缀 = 文件第一个函数）；
`start_index` = 函数体内顶层步骤序号；函数入口用 `config.toml` 默认 config
（函数库无文件级 config）。RunRecord 以
`<pkg>/<file>.yaml[#函数]` 标识展示。

### 7.3 定时任务：参数快照与签名门禁

任务保存的是**完整类型化 args 快照**（每个声明参数都有值，与运行 API 的 args
同构）+ 保存时的参数签名，两列随任务持久化：

- 保存 `POST /api/tasks`（body 含 `runner:{runner_id,entrypoint,payload}` 与
  `schedule`；YAML 参数在 `runner.payload.args` 稀疏覆盖）时，服务端
  按脚本当前声明解析成全量快照并计算签名 `param_signature`；
- **调度运行使用快照**，不回读声明默认值、不依赖浏览器在线；脚本默认值后续变化
  不影响已保存任务；
- 脚本参数声明（类型/名称/必填性/默认值）变化 → 重算签名与存储值不一致 → 任务标
  「参数已过期」，保存 / 启用 / 立即运行被 **409
  `param_signature_conflict`** 拦截（body 带 `reason`：
  `signature_mismatch`=声明已变）；
- 编辑任务里「重新确认」带 `reconfirm:true` 重存 → 按当前声明重算快照与签名。

签名算法 `psig1`（按声明顺序，覆盖类型/名称/必填性/默认值，前端服务端各有一份
实现、双向测试锁定）：

```
param_signature := "psig1" + "|" + join(entries, "|")
entry           := type "," name "," required "," canonical_default
required        := "1"(必填) | "0"(有默认值)
canonical_default: bool→true/false；coord→[x,y]（逗号后无空格）；color→小写 hex；
                   key→大写；time→小写且 min 归一为 m；text→转义 \ , |；tmpl→原样
```

示例（fixture v12）：
`psig1|bool,enable,0,true|time,timeout,0,30s|text,message,0,开始任务|coord,pos,0,[0.5,0.5]|color,target,0,123456|key,quit_key,0,ESC|tmpl,icon,0,icon.png`

## 8. 诊断错误

装载 / 校验 / 运行错误统一为结构化五元组
`{ code, message, resource, step_path, field }`：

- `code` 命名空间五域：`resource.*`（模板/脚本/函数/分区）、`param.*`（声明/引用/
  args）、`step.*`（字段与候选）、`ref.*`（call/func 引用图）、`runtime.*`
  （运行期）；完整清单见 `docs/reference/SCRIPT_EDITOR_CONTRACT.md` §5.3；
- `step_path` 定位到步骤（如 `steps[1].then[0]`、`params[0]`、`login.steps[2]`），
  前端按 `code + step_path + field` 定位卡片与控件，`message` 仅展示；
- 保存接口（脚本 / 函数库）带 `expected_version` 版本短码做双页面冲突检测，
  不符返回 `409 {code:"version_conflict"}`。
- 保存、手动运行、函数测试和任务保存都使用同一严格 v2 loader；解析失败返回
  `code/message/resource/step_path/field` 结构化诊断，不按接口分别放宽格式。
- 脚本/函数库创建使用 `POST /api/apps/:app/resources/{scripts|functions}`
  （JSON `{name, content}`，同分区同名返回 409）；已有资源更新使用同路径 `PUT`，
  默认要求 `expected_version`，仅 `force:true` 跳过版本比较。模板上传是创建
  （`POST /api/apps/:app/resources/templates`，PNG 原始字节 body + `?name=`），
  已有模板图像替换用同路径 `PUT`（Content-Type `image/png` = 字节替换；JSON body = 重命名），
  不会由创建接口覆盖。

## 10. 严格基线与不兼容边界

当前基线只接受本文件前述 v2 结构，所有不在白名单中的结构都按未知键或结构错误拒绝：

- 可执行脚本只能位于 `data/<pkg>/scripts/`，函数库只能位于 `data/<pkg>/functions/`，
  模板只能位于 `data/<pkg>/templates/`；分区之间不回退。
- 脚本顶层只能是 `params/config/steps`，函数记录只能是 `params/steps`；
  `color` 的 `else` 只能位于步骤级，与 `color` 同列，候选列表内的 `else` 是结构错误。
- 参数和引用使用当前具名形式（`$name`）；调用使用 `call` 或
  `func: <文件短路径>/<函数名>`，其他结构不属于输入契约。
- 模板短名解析只接受当前分区内唯一文件，创建与图像替换遵循 §9 的独立 API 契约。

## 11. YAML v3（Phase 12 语法契约）

> v3 是唯一正式方案（ADR-YAML-01）：脚本必须声明 `version: 3`，非 3 一律报
> `unsupported yaml version`，无 v2 兼容 / 无 fallback / 无迁移工具。本节是 v3
> 语法契约的实现同步；权威裁决见 `docs/reference/adr/ADR-YAML-01~04`，契约原文
> 见 `docs/plans/phase12_v3_dsl_contract.md`。实现在
> `server/src/extensions/gamer_yaml/yaml_vnext.rs`（纯数据前端）+ WASM guest
> 小 AST 解释器；本节未覆盖的步骤语法（find / match / check / retry 等）沿用
> §5 的语义并按 v3 步法重设计，随 T45 收口后补写。

### 11.1 脚本与函数库

- **脚本**（scripts/）顶层只允许 `version / params / defaults / steps`；缺失或非 3 的
  `version` 报 `yaml.v3.version` / `yaml.v3.version.missing`。`params` 为参数
  唯一来源，字符串 / 映射双形态沿用 §3 形态，`remark`（字符串第 3 段 / 映射
  `remark` 键）随声明保留并透出到参数 schema 的 `description`
  （不参与 `psig1` 签名，改备注不触发任务参数过期）。
- **函数库**（functions/）为 bare-map `{<函数名>: {params, steps}}`，**无
  `version` 键**（目录即类型）；函数名由映射键承载（唯一），每个函数记录只允许
  `params / steps`，`steps` 必需；函数名 unicode 字母/数字/`_`（支持中文）、
  不能以数字开头且不得撞动作键/结构键/`$match` 保留字
  （`yaml.v3.function.name`）。结构非法报 `yaml.v3.function.*` 结构化诊断
  （`yaml.v3.function.file` / `.name` / `.unknown_key` / `.not_found`）。
  保存边界接受 v3 / v2 双形态（v3 优先，失败回落 v2；v2 删除后收敛单形态），
  允许嵌套目录（`function:<文件短路径>/<函数名>` 的短路径可含 `/`）。

### 11.1.1 defaults —— vision threshold 与 timing 兜底（契约 §4）

```yaml
version: 3
defaults:                     # 可选
  vision:
    threshold: 0.80           # 模板匹配阈值兜底
  timing:
    after_tap: 300ms          # 每次 tap 后等待（内置 300ms）
    after_match: 200ms        # 匹配命中后等待（内置 200ms）
    poll_interval: 100ms      # find/check 轮询间隔（内置 100ms）
steps:
  - ...
```

- 只允许上述键，未知键 / 非法形态报 `yaml.v3.defaults.unknown_key` /
  `yaml.v3.defaults.type` / `yaml.v3.defaults.range`；timing 值必须是带单位
  时长字面量（`300ms`/`2s`）或非负整数毫秒，不接受 `$var`。
- **threshold 三级优先**：step `threshold` > `defaults.vision.threshold` >
  Runtime 内置 `0.80`；lower 期解析并注入 `vision.match` / `vision.match_many`
  的 invoke args（缺省省略字段，由 Runtime 兜底）。
- **timing 即语义**：tap 后 / 命中后等待与轮询间隔全部由脚本 defaults 显式
  声明（缺省用内置值），lower 期展开为显式 `runtime.sleep`（可被「停止」取消），
  不存在隐藏的全局 interval / judge_delay。
- 函数库无 defaults 块（bare-map 结构），timing / threshold 走内置兜底。

### 11.3 find / match_first / check 与 `$match` 上下文（ADR-YAML-03）

```yaml
- find:
    template: reward
    timeout: 10s          # 可选；缺省 30min（轮询 poll_interval 至命中）
    threshold: 0.90       # 可选 step override（三级优先见 §11.1.1）
    region: {x: 0.1, y: 0.2, width: 0.3, height: 0.4}   # 可选；相对坐标搜索区
    save: reward          # 可选；命中结果固化到命名变量，跨后续步骤可用
    then:                 # 命中后步骤组（唯一键名；体内 `$match` / `$reward` 可用）
      - tap: {point: $reward.center}
    else:                 # 可选；超时后步骤组（缺省抛 FIND_TIMEOUT: <template>）
      - log: 未找到
    verify:               # 可选；then 执行完后在 timeout 内二次验证
      template: home
      timeout: 5s         # 可选；缺省 30min
```

- **命中路径**：save 固化 → sleep(after_match) → `then` → `verify`（若设，
  不命中抛 `VERIFY_FAILED: <template>`，不走 else）。
- **超时路径**：有 `else` 走 `else`；无 `else` 抛 `FIND_TIMEOUT: <template>`。
- **match 结果值**（save 存入 / `$match` 引用）：
  `{found, score, x, y, width, height, center, region}`；坐标为相对值 0~1
  （center 为命中框中心），`region` 回显本次搜索区域。未 save 时 `$match`
  仅在对应 find/match_first 的 then/else/verify/steps 体内可见（块结束复位），
  save 后跨步可用。

```yaml
- match_first:
    candidates:
      - template: reward
        threshold: 0.9      # 可选候选级 threshold（三级优先同上）
        steps:              # 候选命中后执行（唯一键名；体内 `$match` = 该候选结果）
          - tap: {point: $match.center}
      - template: close
        steps:
          - tap: {point: $match.center}
    else: ...               # 全未命中走 else；缺 else 静默继续
```

- match_first 单帧 `vision.match_many`（候选级 threshold 经 `thresholds`
  平行列表传入）、按书写顺序首个命中候选执行自己的 `steps`。
- `- check: {template, timeout?, threshold?, throw?}`：轮询至出现（每轮
  sleep(poll_interval)），命中 sleep(after_match) 后继续；超时按 `throw`
  文案结束运行（缺省「check 未命中」）。
- **wait 双形态**：`- wait: 300ms` 固定；`- wait: {min: 300ms, max: 700ms}`
  随机区间（min/max 必须同给且 min ≤ max，run 级随机 nonce 播种 splitmix64
  取值，经 `runtime.sleep` 等待、可被停止取消）。

**已删除步骤/字段**（给迁移诊断 `yaml.v3.step.removed` / `yaml.v3.field.removed`）：
`retry`（用 loop 表达）、`wait_for`（与 find 同义）、`click_when` /
`find.click` / 候选 `click`（ADR-YAML-03 click 语法全面移除，用 then + 
`tap: {point: $match.center}` 表达）、`color_branch`（用
`invoke: vision.sample_color` + if 表达）、match_first 顶层 `then`
（候选步骤归各自 `steps`）。

### 11.4 v3 surface 步骤集（19 类）

`app.start` / `app.stop` / `tap` / `swipe` / `key` / `text` / `wait` /
`log` / `set` / `if` / `loop` / `break` / `call` / `return` / `throw` /
`find` / `match_first` / `check` / `invoke`——与前端编辑器
（`web/src/script-editor/model.ts` `STEP_KINDS`）一一对应。

### 11.5 call —— 唯一可调用资源入口（ADR-YAML-02）

```yaml
- call:
    target: script:daily/login        # 或 function:工具/月卡领取
    with:                             # 参数名 → 表达式；`args` 为兼容别名
      account: $user
    save: result                      # 可选；返回值整体存入；无 return → null
```

- **命名空间仅 `script:` / `function:`**；裸 target / 未知前缀在解析期报
  `yaml.v3.call.namespace`（错误信息含 target 原文与合法形态示例）。
- `script:<资源id>`：分区内 `scripts/` 相对路径，`.yaml` 后缀可省略
  （`script:daily/login` → `scripts/daily/login.yaml`）。
- `function:<文件短路径>/<函数名>`：文件短路径按**最后一个 `/`** 分割、可含目录
  （`function:common/login/is_logged_in` = `functions/common/login.yaml` 里的
  `is_logged_in`）；拒绝 `..` / 绝对路径 / 反斜杠 / 空段——穿越报
  `yaml.v3.call.target`，路径形态报 `yaml.v3.call.function_path`。
- 函数与脚本只经 Core ResourceStore（composite 三层）解析，本地编辑区与包内
  资源对 `call` 透明；跨分区一律不解析。
- **返回值泛化**：`return` 可返回 null / bool / number / string / object /
  array 任意 JSON 值；`call` 的 `save` 存返回值整体，被调方无 `return` 即存
  null。删除「函数默认返回 bool」约束，`if` 条件按通用值语义判断。
- **递归深度**上限 32 层，超限报 `CALL_DEPTH_EXCEEDED: depth=N max=32`
  （P12.4 起 depth 由 guest 本地 ExecutionBudget 计数，WIT `programs.resolve`
  不再透传 depth、宿主不做深度守卫）。
- **执行预算**（ADR-YAML-04）：`max_steps = 100_000`（逻辑步：顶层、循环体
  每轮每个子步、if 分支体、call 目标程序体全计，循环每轮迭代本身也计）、
  `max_call_depth = 32`，由 guest 解释器本地计数；超限报
  `STEP_BUDGET_EXCEEDED: consumed=N max=100000` / `CALL_DEPTH_EXCEEDED`，
  错误码原样进入运行错误信息与日志。宿主侧 wasmtime epoch interruption
  仅作取消兜底（用户停止可打断 guest 纯计算段），不做超时强杀。

### 11.6 手动运行 start_index（契约 §8）

guest 解释器支持 program 顶层可选 `start_index`：跳过其前的**顶层**步骤
（与 v2「从此运行」语义一致）；嵌套分支 / 循环体不受影响——lower 后的顶层小
AST 步与 surface 步骤 1:1 对应，序号即顶层 surface 步序号。host 由运行请求
（`YamlWasmRunRequest.start_index`）注入，缺省 `None` = 从头执行。
