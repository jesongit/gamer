# YAML 脚本语法（v2）

GameBot 自动化脚本的权威语法文档（2026-08 重写）。本文描述**全新 v2 语法**，与旧版
v1 语法完全不兼容（差异清单见文末「与旧语法（v1）的差异」）。规则来源：

- 契约：`docs/SCRIPT_EDITOR_CONTRACT.md`（阶段 0 冻结 + 实现期澄清）；
- 可执行样例：`server/tests/fixtures/script_v2/`（本文所有示例与其同形态，装载由
  `server/src/script_v2/`（装载/校验/序列化）+ `server/src/engine/`（执行）保证）；
- 前端：可视化编辑器（`web/src/script-editor/`）以此为唯一编辑模型，保存时由服务端
  统一序列化为本文的「规范 YAML」。

## 1. 目录与资源边界

脚本、函数库、模板按**应用分区**（设备配置的 pkg，即应用包名）存放，目录即类型：

```
data/<pkg>/
├── yaml/    # 可运行脚本（.yaml/.yml，顶层必须有 steps）
├── func/    # 函数库（严格 .yaml，顶层键全是函数名）
└── tmpl/    # 模板图片（8-bit 灰度 PNG，文件名可带 # 搜索区后缀）
```

- **脚本资源 ID** = `<pkg>/<文件名>.yaml`（如 `daily/login.yaml`，可含子目录）。
  含 `/`，前端拼 URL 必须整体 `encodeURIComponent`。
- **函数路径** = `<文件短路径>/<函数名>`（如 `common/login` = `func/common.yaml`
  里的 `login`；一个函数库文件可定义多个函数）。
- **运行边界**：只有 `yaml/` 下的脚本可手动运行 / 立即运行 / 进入定时任务；
  `func/` 只能被 `func` 步骤调用或走函数测试 API，不进脚本列表与任务选择器。
- **不做内容推断**：`yaml/` 里必须有顶层 `steps`；`func/` 顶层键全是函数名。
  放错目录按该目录的类型校验，报错即拒。
- **跨分区一律不解析、不回退**：模板 / 函数 / 子脚本只在当前应用分区查找，
  没有 default 兜底。旧目录布局（`data/scripts/<package>/` + 全局
  `data/templates/`）由服务端启动时一次性迁移（`scripts::migrate_fs_layout`），
  不再被读取。
- **模板引用写短名**（如 `account.png`）。磁盘文件名可带 `#` **搜索区后缀**
  （后缀在扩展名前，如 `xx#l.png`）：
  - 半区码：`a`=全屏、`u`/`d`/`l`/`r`=上/下/左/右半、`ul`/`ur`/`dl`/`dr`=四角；
  - 数字坐标：`xx#x1_y1_x2_y2`，四段各为相对坐标 ×1000 的整数（如
    `xx#0_0_500_500` = 左上 1/4 区域），需 x2>x1、y2>y1。
  脚本写 `xx.png` 而磁盘存在 `xx#l.png` 时按「基名 + `#` 后缀 + 同扩展名」唯一
  匹配；零候选报不存在、多候选报歧义（`resource.tmpl.ambiguous`），不猜测。

## 2. 顶层结构

### 2.1 脚本（yaml/）

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

- 出现旧语法顶层键（`func` / `name` / `action_wait` / `default_threshold` /
  `package` / `until` / `cond`）报 `script.top_level.legacy_format`（前端展示
  迁移引导）；其余未知键报 `script.top_level.unknown_key`。
- 根节点必须是映射；`steps` 缺失报 `step.field.missing`（field=steps）。

### 2.2 函数库（func/）

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
  运行记录）统一为 6 位十六进制无 `#`。纯数字色值在 YAML 里**必须加引号**
  （`'123456'`）防止被解析成数字丢前导零；含字母色值（`ff8800`）可裸写，编辑器
  保存时对纯数字色值统一加引号输出。

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
- match 候选模板键与 color 候选色键同样接受 `$name`（见 §5.3/§5.4）；
- `$name` 只在引用处生效：call/func 的 `args`、入口运行参数按名字绑定（§6）。

## 4. config

```yaml
config:
  interval: 500ms     # 轮询间隔（find 每轮重试 / verify 复查 / match 轮询），带单位 >0
  threshold: 0.85     # 模板匹配阈值，0~1
  log_level: info     # debug / info / warn / error，低于等级的日志丢弃
```

整体省略 = 使用 `config.toml` 同名键（`interval` / `threshold` / `log_level`）。
不允许未知 config 键；只能是映射（v1 的「映射列表按序覆盖」写法已删除）。

另有仅全局生效的 `config.toml` 键 `judge_delay_ms`（默认 200，0=关闭，脚本 config:
不覆盖）：find / match / color 的**命中路径**在执行后续分支步骤前固定等待该时长
（给游戏 UI 留响应时间）；分支为空（无后续步骤）不等待，else / 超时路径不延迟。

## 5. 步骤（17 种）

一个步骤只允许一个动作键（多动作键 → `step.multi_action`）；动作键之外的同级键是
该步骤的字段。步骤按书写顺序执行；空分支 / 默认字段在 YAML 里省略（编辑器保存
的规范 YAML 同样省略）。分支子列表（`then` / `else` / 候选分支 / `loop.steps` /
函数体）递归为步骤列表。

### 5.1 基础动作

```yaml
steps:
  - str_app                     # 冷启动当前分区应用（先 force-stop 再启动）；裸写，带值非法
  - tap: [0.5, 0.5]             # 点击（相对坐标或 $name）
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

每轮：主模板（**新截图**）命中 → 恒点**模板中心** → `verify: true` 时等
`config.interval` 重匹配一次、仍命中补一击（共两击，适合点击后弹窗关闭类按钮）→
执行 `then` 结束本步；未命中 → `block` 依序匹配（命中即点其中心并结束本轮）→
全未命中等 `config.interval` 重开一轮。超过 `timeout` 执行 `else`。截图瞬态失败
跳过本轮重试（持续失败约 20s 判链路异常带因中止）。

### 5.3 match —— 多模板策略选择（不点击）

`match` 的候选列表是**紧凑缩进**（无缩进序列，唯一序列化格式）；`else` /
`timeout` 是 `match` 步骤的**兄弟键**，与 `match` 同列：

```yaml
- match:
  - test1.png:
    - log: 命中 test1
  - test2.png:
    - log: 命中 test2
  else:
    - log: 都未命中
  timeout: 30s
```

- 每轮只截**一帧**，候选按书写顺序匹配、首个命中获胜、执行其分支步骤并结束本步；
  **不点击**（需要点用 find）。
- 未配 `timeout` 只执行一轮（全未命中立即进 `else`）；配了按 `config.interval`
  轮询到超时才进 `else`。
- 候选模板短名不可重复（装载期与参数绑定后都查重 →
  `step.match.candidate_duplicate`）；不接受布尔条件（布尔走 `if`）。
- `- else:` / `- timeout:` 写进候选列表是错误（`step.match.else_in_candidates`）。

### 5.4 color —— 单点颜色分支

`at` 与 `expect` 写在 `color` 值映射内；`expect` 是**有序列表**（不用颜色做映射键，
防解析器重排），每项是单键映射 `颜色: [分支步骤]`；`else` 与 `color` 键同列：

```yaml
- color:
    at: [0.5, 0.5]
    expect:
      - ff8800:
        - tap: [0.5, 0.5]
      - '123456':
        - log: 深蓝分支
  else:
    - throw: 颜色未命中
```

- 一次截图、按序判色：实际像素与期望色每通道差 ≤30 视为命中（容差固定 30，吸收
  H.264 有损压缩抖动），命中即执行该色分支并结束本步；全未命中走 `else`。
- **不轮询、不点击**（重试套 `loop`）。
- 同色候选重复 → `step.color.duplicate`；颜色格式非法 → `step.color.format`；
  纯数字色值必须加引号（§3.2）。

### 5.5 if / loop

```yaml
- if: $enable                   # 条件严格布尔（bool 参数或 true/false），无隐式转换
  then:
    - tap: [0.5, 0.5]
  else:
    - log: 未启用

- loop:                         # times 省略或 0 = 无限（10 万步 guard 兜底，见 §5.7）
    times: 3
    steps:
      - wait: 1s
```

- `if` 条件非布尔报 `step.if.non_bool_cond`；
- `loop` 值是映射：`times` 为非负整数字面量、`steps` 必需且非空
  （缺失 → `step.field.missing`，空 → `step.loop.empty_steps`）。

### 5.6 call / func —— 子脚本与函数

```yaml
- call: sub/inner.yaml          # 调用同分区 yaml/ 脚本（缺 .yaml 自动补全）
  args:                         # 具名实参（稀疏：未给的参数走声明默认值）
    enable: $enable
    message: "字面量消息"

- func: common/login            # 调用 func/<文件短路径>.yaml 里的函数 login
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

### 5.7 throw / return 与运行护栏

```yaml
- throw                         # 无原因
- throw: 余额不足                # 带原因
- return: true                  # 仅函数库合法（脚本里报 step.return.in_script）
- return: $enable
```

- `throw` 立即结束整个运行（跨 call/func 调用链），运行以失败终态收场
  （`runtime.engine.throw`，携带原因）。
- `return` 只退出当前函数，值必须是布尔。
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

```
POST /api/scripts/:id/run     body { device_id, start_index?, args? }
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

```
POST /api/functions/:id/run   body { device_id, function?, start_index?, args? }
```

- `id` = `<pkg>/<文件短路径>.yaml`；`function` 缺省 = 文件第一个函数；
  `start_index` = 函数体内顶层步骤序号；函数入口用 `config.toml` 默认 config
  （函数库无文件级 config）。函数运行不占用脚本运行接口，RunRecord 以
  `<pkg>/<file>.yaml[#函数]` 标识展示。

### 7.3 定时任务：参数快照与签名门禁

任务保存的是**完整类型化 args 快照**（每个声明参数都有值，与运行 API 的 args
同构）+ 保存时的参数签名，两列随任务持久化：

- 保存 `POST /api/tasks`（body 含 `script_id`、`cron`、`args` 稀疏覆盖）时，服务端
  按脚本当前声明解析成全量快照并计算签名 `param_signature`；
- **调度运行使用快照**，不回读声明默认值、不依赖浏览器在线；脚本默认值后续变化
  不影响已保存任务；
- 脚本参数声明（类型/名称/必填性/默认值）变化 → 重算签名与存储值不一致 → 任务标
  「参数已过期」，保存 / 启用 / 立即运行被 **409
  `param_signature_conflict`** 拦截（body 带 `reason`：
  `signature_mismatch`=声明已变 / `no_snapshot`=旧任务无快照）；
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

## 8. 录制输出形态

投屏控制台（Console）录制手势，停止后按以下形态生成步骤（fixture v11）：

- **点击** → 单条 `find`（模板短名，无 block/verify/timeout）：

```yaml
steps:
  - find: record_click_20260829_001.png
```

- **滑动** → `match → swipe`（起点模板命中才滑，避免 find 的命中点击破坏手势），
  默认 `else` 为 throw、`timeout: 30s`：

```yaml
  - match:
    - record_swipe_20260829_002.png:
      - swipe:
          fm: [0.5, 0.8]
          to: [0.5, 0.2]
          time: 800ms
    else:
      - throw: 未找到滑动起点
    timeout: 30s
```

- **模板命名**：默认 `record_<click|swipe>_YYYYMMDD_NNN.png`（NNN 三位序号，
  分区内冲突自动顺延）；录制时框选的搜索区域写入完整文件名的 `#` 后缀
  （半区码或 `#x1_y1_x2_y2` 相对 ×1000，见 §1），脚本里仍写短名引用。
- **Alt 组合键**（编辑态，不经录制上传队列）：模板 → `find`；取色 → `color`；
  Alt 拖动 → 裸 `swipe`。
- 录制产出以「占位步骤 + 逐条定稿」写入编辑器命令栈，可撤销；上传失败保留草稿
  可重试 / 降级为坐标 tap / 丢弃。

## 9. 诊断错误

装载 / 校验 / 运行错误统一为结构化五元组
`{ code, message, resource, step_path, field }`：

- `code` 命名空间五域：`resource.*`（模板/脚本/函数/分区）、`param.*`（声明/引用/
  args）、`step.*`（字段与候选）、`ref.*`（call/func 引用图）、`runtime.*`
  （运行期）；完整清单见 `docs/SCRIPT_EDITOR_CONTRACT.md` §5.3；
- `step_path` 定位到步骤（如 `steps[1].then[0]`、`params[0]`、`login.steps[2]`），
  前端按 `code + step_path + field` 定位卡片与控件，`message` 仅展示；
- 保存接口（脚本 / 函数库）带 `expected_version` 版本短码做双页面冲突检测，
  不符返回 `409 {code:"version_conflict"}`。

## 10. 与旧语法（v1）的差异（破坏性）

v2 与 2026-08-26 的 v1 精简语法**不兼容**，装载器对旧写法显式报错引导迁移：

| v1 | v2 |
|---|---|
| 顶层 `func:` 段定义自定义函数 | 删除；函数库独立放 `data/<pkg>/func/`，顶层键=函数名，经 `func: 文件/函数` 调用 |
| `$N` 位置实参 / `^N` 上下文引用 | 删除；改为具名参数 `$name`（params 声明 + args 绑定） |
| `call` 传 $N、`- 脚本名:函数名:` 跨文件函数调用 | 删除；`- call: <脚本>.yaml` + 具名 `args`，函数调用统一走 `- func: 文件/函数` |
| 函数 `cond:` 条件（模板匹配决定是否执行） | 删除；布尔分支用 `if`，模板条件用 `find` 短 timeout + then/else |
| `until:` 步骤 | 删除；由 `find` 取代 |
| 脚本顶层 `name` / `action_wait` / `default_threshold` 键 | 删除；`config` 三键为 interval/threshold/log_level |
| `package <名字>` 指令 | 删除；分区 = 目录名 `data/<pkg>/`，残留=解析报错 |
| `config` 可写映射列表按序覆盖 | 删除；只能写单个映射 |
| `steps:` / `func:` 段落键可省略（单段脚本简写） | 删除；`yaml/` 脚本必须显式 `steps:`（可空列表不可省略） |
| `- color: [x, y]` + 兄弟键色值 | 删除；改为 `- color:` + `at` + `expect` 有序列表（§5.4） |
| `loop` 的 times/steps 两种缩进均认 | 删除；只认 `- loop:` 映射形态（§5.5） |
| `success` 日志级别 | 删除；四级 debug/info/warn/error，success 视同 info 的特例不再存在 |
| 匹配只随 find 隐式发生 | `match`（不点击策略选择）成为正式步骤，录制滑动输出 match→swipe |
| 脚本/函数混存 `data/<pkg>/yaml/`（v1 末期） | 目录即类型三分：`yaml/` + `func/` + `tmpl/`，函数库不可运行/调度 |

保留不变（自 v1 沿用）：`str_app` / `cls_app` / `tap` / `swipe` / `key` / `text` /
`log` / `wait`（含随机区间）；`find` 的 `block` / `verify` / `timeout` / `then` /
`else` 骨架；`throw`；模板 `#` 搜索区后缀与短名引用；脚本 id
`<pkg>/<名>.yaml`；10 万步 guard 与 tap/swipe/hit/miss 可视化事件。
