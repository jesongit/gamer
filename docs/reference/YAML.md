# YAML 脚本语法（V1 唯一正式方案）

2026-09-19 编辑/诊断补齐：所有 `_function*.yaml` 使用同一画布编辑并按所属文件的 `expected_version` 保存。保存函数库时拒绝原生/包内同名函数；删除或改名仍被引用的函数返回 `yaml.functions.referenced`（列出调用资源），其他文件无法解析且无法确认引用时返回 `yaml.functions.references_unknown`。这不是跨文件自动重构；修复引用后再提交。删除最后一个函数可保存 `functions: {}`。

gamer-yaml 3.1.2 的运行事件可携带 `trace`：`run_id`、`frame_id`、`parent_frame_id`、`source{package_id,plugin_id,path,version,function?}`。定义来源来自执行冻结资源；递归调用有独立帧 ID，`path` 为该调用中的步骤路径。Core 仅转发可选数据，YAML 插件解释语义。不含完整历史源码，当前文件版本已变时 UI 显示身份/差异而不错误高亮。此字段属于运行事件，不是 YAML 新关键字。

从界面建脚本、找图点击、参数到函数复用和定时运行，见 [YAML 自动化教程](../guides/yaml-tutorial.md)（2026-09-14 核对）。

Gamer 自动化脚本只支持 **YAML V1**（Gamer V1 简化计划 Phase 1；无 `version`
字段——出现 `version:` 直接报 `yaml.version.removed` 拒绝诊断，旧 v3/v2 脚本
**无兼容分支、无 fallback、无迁移工具**）。

核心原则：**YAML 只描述流程，所有实际操作都是函数调用**。解释器只认识
函数调用 / `if` / `repeat` / `return` / `match_templates` / `break` 六类步骤；`tap`、`find`、`sleep` 等
都不是语法关键字，而是函数。

- 权威实现：`plugins/gamer-yaml/interpreter/`（唯一解释器，WASM guest 与宿主测试
  同源）+ `plugins/gamer-yaml/host/syntax.rs`（解析/校验/降线）+
  `native_funcs.rs`（原生函数注册表）；前端可视化编辑器（`plugins/gamer-yaml/ui/src/script-editor/`）
  与 Runtime 共用同一 V1 surface DSL；
- 旧 v3 语法文档（docs/yaml-v3/）已删除，历史实现见 git 历史。

## 1. 目录与函数来源

脚本、函数库、模板按 **Package**（数据一级作用域）存放：

```
data/packages/<package-id>/
├── package.toml                      # manifest（id/name/version/author/targets/plugins 依赖）
├── shared/                           # 跨插件保留区（gamer-yaml 不写）
└── plugins/
    ├── gamer-yaml/
    │   ├── automations/              # 自动化脚本 + 函数库（按文件名前缀识别，见 §4）
    │   │   ├── daily.yaml            # 自动化（普通 .yaml）
    │   │   └── _function.yaml        # Package 默认函数库（functions: 包装）
    │   └── templates/                # 模板图片（8-bit 灰度 PNG）
    └── <其他插件>/                    # dormant 数据原样保留，Core 不解释
```

**文件识别规则**（简化计划：识别只用于资源发现，不加 `kind` 字段；目录与
文件名不产生函数命名空间）：

```text
automations/ 内文件名以 _function 开头且以 .yaml 结尾 → 函数库（functions: 包装）
automations/ 内其他 .yaml                            → 自动化脚本
```

旧 `functions/` 专属目录已删除（保存钩子报 `yaml.functions.dir.removed`，
无兼容层、无自动迁移）。

**函数只有两种来源**：

1. **插件函数**：`gamer-yaml` 原生注册表（`native_funcs.rs`，Schema 唯一声明点），
   随插件安装/启用变化；受插件权限约束；其中 `tap / swipe / key / input_text /
   launch / stop_app / sleep / log / find` 是 Core 能力的基础包装，
   `wait_find / tap_template / wait_disappear` 是插件便利函数（复用 find/sleep
   组合，不复制视觉算法），`eq..le` 是纯数据函数；
2. **当前 Package 函数**：`automations/_function*.yaml`（用户可编辑，默认只有
   `_function.yaml` 一个文件；手动拆分的 `_function_battle.yaml` 等同样参与
   加载），解释器本地执行。

运行前组合为唯一函数名注册表：同名冲突（原生 vs Package、跨文件重复）一律
拒绝，文件顺序不决定胜者；不跨 Package 查找；运行开始时冻结全部函数定义。
**统一命名空间：调用名 = 函数名**，移动/重命名函数库文件不改变调用名。
函数目标寻址 = `<package-id>#<函数名>`（函数测试运行、参数 schema 查询）。
通过 `POST /api/runs` 运行时，`content_package` 只填写配置包 ID（例如 `com.mihoyo.hkrpg`），不能包含 `#函数名` 或 Android 应用名；前端显式传递此字段，服务端缺省按入口第一个 `/` 或 `#` 之前的配置包 ID 解析。
首版原生函数清单：

```text
原子：tap / swipe / key / input_text / launch / stop_app / sleep / log / find
便利：wait_find / tap_template / wait_disappear
比较：eq / ne / gt / ge / lt / le
```

`GET /api/runners/gamer-yaml/functions` 返回原生函数目录（Schema 唯一前端来源）。
前端「函数」页面按函数展示，默认在 `_function.yaml` 新建；额外拆分的
`_function*.yaml` 同样可在画布中编辑，并按实际定义文件保存。

## 2. 脚本格式

```yaml
name: 每日签到            # 可选

params:                   # 可选：运行参数 Schema（类型/必填/默认值/说明）
  retry:
    type: integer
    default: 3
    desc: 重试次数
  secret:
    type: string
    required: true

vars:                     # 可选：字面量表（不做引用解析）
  timeout: 15s

run:                      # 必有（可为空列表）：执行入口
  - launch: com.example.game
  - wait_find:
      template: home.png
      click: false
      timeout: $timeout
    as: home
  - if: $home
    then:
      - claim_daily: {}
    else:
      - log: 未进入主页
  - return: true
```

参数类型：`any / boolean / integer / number / string / list / object /
duration / point / template / key`（别名 bool/int/float/text 解析期归一）。
默认值按类型校验（`yaml.param.default.invalid`）。

## 3. 步骤与表达式

**一个步骤 = 恰好一个动作键**（函数名或 `if/repeat/return/match_templates/break`）+ 可选 `as`；
`then/else/do` 是 if/repeat 的结构键。

```yaml
run:
  - tap: [0.5, 0.8]              # 位置值简写（标量/数组 → 第一个参数）
  - find: login_button.png       # 同上
    as: button                   # as = 返回值赋给变量
  - tap: $button.center          # $name.field 引用（仅点号字段，无索引）
  - swipe:
      from: [0.5, 0.8]
      to: [0.5, 0.2]
      duration: 500ms
  - key: HOME
  - input_text: 你好
  - sleep: 1s                    # duration：带单位串或毫秒数；0 合法
  - log: 未进入主页               # 非字符串值自动转 JSON 文本
  - log:
      message: 带级别
      level: warn
  - launch: com.example.game     # 缺省包名 = 设备配置的应用（冷启动）
  - stop_app: {}
  - repeat: $retry               # 固定次数（非负整数或整数引用）
    do:
      - tap: [0.5, 0.5]
  - return: $button              # 返回值（脚本顶层返回即运行结果）
```

- 所有函数支持可选字符串参数 `name`（也可用变量引用），写在函数参数映射中，
  如 `tap: {name: 点击登录, position: [0.5, 0.8]}`。可视化卡片直接展示该值，
  不再拼接函数名或参数。未填写时，原生函数默认使用中文名（如「点击」「等待」），
  配置包函数默认使用 `description`，未写说明则使用函数名；显式声明的 `name` 参数默认值优先。
  `name` 不改变调用目标，位置值简写仍对应原来的第一个参数。
- 无参或全部参数可省略的函数允许 `{}` 或 null（如 `launch: {}` / `launch:`）；
  `sleep` 的 duration 必填，不能用 `sleep:` 省略；
- `if` 条件：只有 `false`/`null` 为假，其余值均为真（包括 `0`、空字符串、空数组、空对象；
  比较用 `eq/gt` 等函数）；
- 函数调用独立局部作用域：参数显式传入，`as` 接收返回值；
- `find`/`wait_find` 未命中返回 `null`（不是错误）。`find` 是**单次模板匹配**
  （无 timeout/interval 参数）；等待轮询用 `wait_find`（timeout 缺省 10s，
  interval 缺省 250ms，轮询直到命中或超时）。`click` 为 boolean，缺省 `true`：
  命中后按全局点击前后延迟点击模板中心，再返回匹配对象；`click: false` 仅等待，
    不点击目标；未配置障碍时不需要 `input.tap` 权限。超时返回 `null`。
  已有脚本若在 `wait_find` 后单独 `tap`，应加 `click: false` 避免重复点击。
  `obstacles` 缺省 `[]`，接受模板名列表或列表变量引用：每轮共用一帧，先按顺序匹配
  障碍，命中第一个即点击中心并等待后进入下一轮，不再处理旧帧上的其他模板；
  没有障碍命中才匹配目标。障碍点击与等待计入总 `timeout`，不重置计时；
  `click: false` 只关闭目标点击，不关闭障碍处理。障碍共用 `threshold`，搜索区域取
  各自模板文件名后缀，不继承目标 `region`。模板列表中的字面量引用随模板重命名更新。
  `tap_template` 的 timeout 缺省 10s
  （显式传 0ms 只尝试一次）、interval 缺省 100ms，找到后按全局点击前后延迟点击命中中心；
  `wait_disappear` 的 timeout 缺省 10s、interval 缺省 250ms，轮询直到模板消失
  （消失返回 true，超时返回 false）。轮询间隔最低 50ms。

**表达式只有两种**：字面量、`$name.field` 引用。字符串以 `$` 开头是引用；
字面量 `$` 用 `$$` 转义。无算术、无插值、无 eval、无动态索引。
步骤表达式中的数组和对象会递归解析引用，如 `tap: {position: [$x, $y]}`；
`vars` 与参数默认值仍是字面量，不做引用解析。

诊断码命名空间 `yaml.*`（解析/结构）与 `param.args.*`（绑定）；旧 v3 源在
解析层直接报 `yaml.version.removed`，其余旧形态报 `yaml.top.unknown`——
**不接受旧语法**。

### 模板分支 `match_templates`

插件内置复合步骤，字段为 `cases`（1..64 项）、可选 `threshold`（默认 0.8）与可选 `else`。
每个 case 为 `template`（非空模板名或引用）、可选 `as`（局部匹配结果变量）、必填 `do` 步骤列表。
模板使用普通模板引用语义，重命名会同步更新。`as` 只在本分支有效，离开分支后恢复原变量；
分支内其他普通函数调用的 `as` 保持原有语义。顶层步骤不接受 `as`。

匹配通过原生函数 `find_any` 共用一帧并按顺序停止在首个命中，执行该分支后结束本步；
不自动点击、不轮询、不继续匹配点击前的旧画面。全部未命中执行 `else`；错误不当作未命中，
动作错误终止运行，`return` 退出当前脚本/函数。每个分支仍受取消及步数/调用深度预算约束。
运行路径为 `run[0].cases[0].do[0]`、`run[0].else[0]`，与可视化卡片错误定位一致。

```yaml
run:
  - match_templates:
      cases:
        - template: 关闭公告.png
          as: hit
          do:
            - tap: $hit
        - template: 登录按钮.png
          do:
            - log: 已进入登录页
      else:
        - log: 未识别到页面
```

`find_any` 可独立调用，参数 `templates`（模板列表或引用，1..64 项）、`threshold`（默认 0.8）
和通用 `name`；返回匹配对象附 `index`（零基索引）和输入的 `template`，未命中为 `null`。
无点击权限要求；模板各用自己的文件名区域。

`tap` 接受相对坐标，也可直接接收匹配结果：`tap: $hit` 或
`tap: {position: $hit}` 自动点击 `center`，不重新匹配。显式 `$hit.center`
仍有效；匹配结果内的 `x/y` 是像素边框位置，不作为点击坐标。未命中的 `null`
或非法中心坐标会报错且不点击，因此 `find` / `wait_find` 的结果应先用 `if` 判断。
`tap_template` 仍接收模板名称并重新匹配，引用模板名使用 `$hit.template`。

保存时校验能确定的引用类型：参数声明、字面量变量、原生函数匹配结果及模板分支局部结果。
例如把 `$hit` 传给 `tap_template.template` 会报告 `yaml.args.ref_type` 并定位参数；
可视化编辑与服务端保存接口均检查。未知自定义返回值、分支或循环改写后不能确定的值，
继续由运行时校验。直接点击匹配结果及保存期引用类型校验需要包含这些改动的本体，
从 Gamer 0.2.0-beta.7 与自动化插件 0.1.0-beta.2 起支持；beta.6 尚不支持，仅更新插件 UI 不会更新宿主能力。

## 4. 函数库文件（当前 Package 函数）

存储位置 = `automations/` 内文件名以 `_function` 开头的 `.yaml` 文件（默认库
`_function.yaml`；手动拆分可加 `_function_battle.yaml` 等，第一版只识别小写
`_function` 前缀 + `.yaml` 后缀，不接受 `.yml`）。文件内容继续使用
`functions:` 包装，**不新增 `kind` 字段**：

```yaml
functions:                        # 顶层必须有 functions: 包装
  claim_daily:
    description: 领取每日奖励      # 可选说明
    params:
      timeout:
        type: duration
        default: 3s
    vars:                         # 可选：函数内字面量
      tag: local
    returns:                      # 可选：仅文档/提示，不做运行时校验
      type: boolean
    run:                          # 必有
      - tap_template:
          template: daily_button.png
          timeout: $timeout
      - return: true
```

脚本直接调用（两种来源语法一致）：

```yaml
run:
  - claim_daily:
      timeout: 10s
    as: success
```

- 函数名允许中文汉字（CJK 基本区与扩展 A）、小写英文字母、数字、下划线，不能以数字开头，例如 `每日任务跳转`、`领取_daily2`；空格、点号、斜杠等分隔符不可用，保留字 `if/repeat/return/match_templates/break` 不可用；参数名、变量名仍使用 `[a-z_][a-z0-9_]*`；
- **统一命名空间**：文件名与目录只是存储组织，不进入调用名（`_function_battle.yaml`
  里的 `attack` 调用仍写 `attack`）；同一文件内函数名唯一，跨文件/与原生函数
  同名直接报冲突（`yaml.fn.conflict`），文件顺序不决定胜者；
- 一个函数库文件可定义多个函数；函数库文件不进入自动化运行列表与定时任务
  选择器（函数测试运行 = `POST /api/runs`，entrypoint `<pkg>#<函数名>`）；
- 脚本文件不再内嵌局部函数库；可复用函数一律存 `_function*.yaml`。

## 5. 错误处理与预算

无 try/catch/throw/on_error。业务未命中（find 未找到）返回 `null`；参数错误、
资源不存在、设备断开、权限不足属于执行错误，终止当前 Run 并给结构化错误。

保留宿主安全机制（计划 §1.6）：取消（stop 标志 + epoch 兜底）、步预算
`STEP_BUDGET_EXCEEDED`（上限 100,000 逻辑步）、调用深度
`CALL_DEPTH_EXCEEDED`（上限 32，Package 函数本地解释递归）。

## 6. 模板引用与重命名

脚本/函数中 `find / wait_find / tap_template / wait_disappear` 的
`template` 实参用模板短名；模板重命名经 AST 同步改写（`yaml.resource.*`，
文本字面量不误改）。V1 起模板引用改写收敛为上述四个函数 + 任意调用步骤的
`template` 键。

### 跳出循环：`break`

`break: {}` 无参数，退出当前脚本或函数内最近一层 `repeat`，继续执行循环后的步骤。
可以放在循环内的 `if`、`match_templates` 分支中；嵌套循环只退出内层。
`return` 则直接结束整个当前脚本或函数。

```yaml
run:
  - repeat: 10
    do:
      - find: ready.png
        as: hit
      - if: $hit
        then:
          - break: {}
      - sleep: 1s
  - log: 检查结束
```

可视化编辑器「添加步骤 → 流程 → 跳出循环」生成同样语法。
空写 `break:` 也表示无参数；不接受值、`as` 或子步骤。
循环外使用报 `yaml.break.outside_loop`，参数形态错误报 `yaml.break.shape`。
每个函数单独校验作用域，被调用函数不能通过 `break` 跳出调用方的循环。

默认模板等待超时可在设置页的“自动化”中修改（初始 10s），作用于 `wait_find`、`tap_template`、`wait_disappear` 未显式传入的 `timeout`。每次运行开始冻结设置，运行途中不改变；显式实参和自定义函数参数默认值优先，可视化编辑器“恢复默认”会移除实参。设置由插件持久化，不是 YAML 关键字或 Core 全局配置。


### 运行详情与调试

运行提交成功后，编辑区自动切换到运行详情；结束后保留，点击「返回编辑」恢复原编辑器。编辑器的「运行详情 / 历史记录」入口可以查询当前设备、当前脚本或函数最近 30 次运行。

服务端按 `run_id` 保存步骤开始/结束、函数调用、实际参数（含原生默认值）、返回值、分支选择、循环进度、模板匹配、点击和 `log` 消息。无需连接投屏；刷新页面或服务重启后仍可查询。只从升级后的新运行开始记录，旧运行没有补录。运行记录与事件跟随设置中的日志保留天数清理，0 表示不自动清理；服务异常重启时未完成记录标为中断。

详情按步骤展示耗时和结果，函数内部步骤、参数与返回值、模板轮询可展开。障碍模板命中及点击也会记录。向上滚动暂停跟随，点击「回到最新」恢复；「复制日志」复制当前已加载事件（长运行分批读取）。错误可按执行源版本定位；原文件已变化或当前有未保存修改时使用只读定位提示，避免错位或丢失编辑。

Core API：`GET /api/runs?device_id=...&entrypoint=...` 返回最近运行，`GET /api/runs/:run_id/events?after=0` 返回 `{events,next,has_more}`（每页最多 500 条）；事件 ID 作为增量游标，均需登录。结构化记录不再依赖 WebRTC 事件缓存。


### 自动化点击前后延迟

设置 → 自动化提供「点击前延迟」「点击后延迟」，默认均为 **300ms**；可配置 0～60000ms 的整数，0 关闭对应等待。设置持久化，每次运行开始冻结，保存后从下一次运行生效，无需重启。步骤中不提供对应参数。

`tap`、`wait_find` 的目标自动点击、`tap_template`、障碍模板点击共用同一逻辑：确定位置 → 点击前等待 → 按下/松开 → 点击后等待。不会重复追加原先模板点击后的固定 300ms。点击前等待不会重新匹配；匹配等待成功后的点击延迟会增加函数总耗时，障碍处理中的延迟计入本轮轮询耗时。

只匹配而不点击（包括无障碍的 `wait_find(click: false)`）不增加点击延迟；滑动、按键、投屏手动点击不受影响。等待期间支持取消，运行详情显示实际的前后等待时间。
