# YAML 自动化语法

当前仅支持一套无语法版本字段的 YAML。脚本与函数库使用同一解析器，生产执行和离线素材验证共用同一解释器；不提供旧语法兼容或自动迁移。脚本修订号由版本管理保存，不写入源码。

权威实现：`plugins/gamer-yaml/host/syntax.rs`、`host/syntax/`、`plugins/gamer-yaml/interpreter/`。原生函数参数以 `GET /api/runners/gamer-yaml/functions` 和 `host/native_funcs.rs` 注册表为准，不从模型名称或前端表单猜测。

原生数值函数 `number: {text: $ocr_result.text, index: 0}` 提取非负整数；读取完整 `当前值/上限` 时使用 `format: fraction`，严格验证两侧整数和非零分母，`index: 0` 取分子、`index: 1` 取分母，缺项不会回退到另一侧数字。`calculate: {op: floor_div, a: $stamina, b: $cost}` 计算次数，另支持 add/sub/mul/div/min/max。这些仍是普通函数调用，表达式语法保持字面量与引用。`press: {position: $button, duration: 800ms}` 在新鲜匹配位置按住并释放，时长限 200ms..3s，复用现有输入控制与坐标校验。

`find_any: {templates: [on_a#1.png, on_b#1.png], threshold: 0.9, region: [0.83, 0.01, 0.08, 0.09]}` 在同一帧、同一区域按顺序匹配，返回首个命中或 null；未给 `region` 时沿用各模板文件名区域。它仍是普通函数，不新增 YAML 关键字。

## 最小完整自动化

```yaml
name: 领取每日奖励
targets:
  claim:
    template: claim.png
    threshold: 0.8
  confirm:
    template: confirm.png
  done:
    template: claimed.png
run:
  - id: claim_reward
    wait: claim
    timeout: 10s
    as: button
    then:
      - tap: $button
  - optional:
      find: confirm
      timeout: 0ms
      as: popup
      then:
        - tap: $popup
  - finish: done
    timeout: 10s
```

`wait` 只观察，不隐式点击。匹配对象与实际截图来源绑定，后续 `tap` 复用这次观察。所有步骤无异常执行完即为执行成功；任一步报错即结束并判定失败。需要验证游戏状态时，在该位置使用 `wait`、`checkpoint` 或其他明确检查，不要求额外的结束标记。

## 资源与作用域

```text
data/packages/<package-id>/plugins/gamer-yaml/
  automations/daily.yaml
  automations/_function.yaml
  automations/_function_extra.yaml
  templates/claim.png
  samples/<sample-id>.gamersample
```

- 普通 `.yaml` 文件为自动化；文件名以 `_function` 开头且以 `.yaml` 结尾的文件是函数库
- 所有函数库的函数共用配置包内的命名空间；文件和目录名不进入函数名，同名冲突拒绝执行
- 自动化入口为 `<package-id>/<name>.yaml`，函数测试入口为 `<package-id>#<function>`
- `content_package` 是配置包 ID，与 Android 应用包名和设备 ID 分开
- 运行开始冻结脚本、函数和模板的有效资源快照；并发编辑不会将新模板混入旧运行

## 顶层字段和目标

`name`、`params`、`vars`、`targets` 与 `run` 描述脚本。未知字段产生结构化诊断，原文编辑器保留原始内容供修正，不用旧模型重新序列化覆盖。

```yaml
params:
  tries:
    type: integer
    default: 3
vars:
  message: 正在领取
targets:
  claim:
    template: claim.png
    threshold: 0.85
    region: [0.1, 0.2, 0.8, 0.5]
  done:
    template: done.png
run:
  - log: $message
  - wait: claim
    timeout: 8s
    then:
      - tap: claim
  - finish: done
```

目标引用可以是 `targets` 的名称，也可以是内联 `{template: claim.png, threshold: 0.8}`。搜索区域与匹配参数沿用原生视觉能力的坐标约定；保存时以服务端校验为准。默认目标别名为目标名，例如 `wait: claim` 提供 `$claim`；`tap: claim` 是使用该目标已观察结果的简写，不会重新找图。

参数类型保留 `any / boolean / integer / number / string / list / object / duration / point / template / key`。手动运行直接采用默认值，仅缺少必填参数时显示参数表单；任务表单可显式覆盖。

## 脚本内函数与跨脚本调用

自动化界面提供脚本库、模板、工作稿与版本、历史候选和运行日志。函数在脚本的 `functions` 中定义；编辑器的「新增函数」追加原文，不重写其他源码。

```yaml
# common.yaml
functions:
  greet:
    params:
      message: {type: string, default: hello}
    run:
      - log: $message
run:
  - greet: {}
```

```yaml
# 同一配置包中的另一个脚本，可调用整个脚本或指定函数
run:
  - common.yaml: {}
  - common.yaml#greet: {message: world}
  - finish: {template: done.png}
```

调用复用普通函数机制，实参、返回值 `as`、变量作用域、取消、预算和调用深度约束一致。整个脚本使用被调用脚本的参数和变量；其中函数使用自身的参数和变量。函数内可引用本脚本顶层 `targets`。不同脚本的同名函数相互隔离；本脚本函数直接按名调用，跨脚本使用 `<相对路径>.yaml#<函数名>`。只解析当前配置包，不接受跨包、绝对路径或 `..`。原生函数名不得被脚本函数覆盖。`finish` 是必需视觉观察的简写，命中后继续后续步骤，不结束调用方。`return` 只返回当前调用；没有 `return` 的被调用脚本在步骤跑完后返回。

正式运行和候选试跑冻结相同的被调用脚本源码。旧函数库仍可参与已存在的调用，但不再提供独立编辑页面；新函数在脚本内编写。

## 视觉流程步骤

### 必需观察 `wait`

```yaml
- id: wait_login
  wait: login
  timeout: 10s
  as: button
  then:
    - tap: $button
```

默认超时 10s。命中后执行 `then`，没有 `then` 时只取得观察结果。超时明确失败。模板缺失、参数错误和目标断开属于错误，不能伪装成未命中。`id` 给步骤稳定的业务身份，错误同时保留源路径。

### 可选观察 `optional`

```yaml
- optional:
    find: confirm
    timeout: 0ms
    as: popup
    then:
      - tap: $popup
```

默认超时 0ms，即立即观察一次；显式正超时才轮询等待。只在未命中时跳过，不能吞掉语法错误、模板缺失、设备错误或权限错误。

也支持简写：

```yaml
- optional: confirm
  then:
    - tap: confirm
```

### 视觉断言 `finish` 与失败 `fail`

```yaml
- finish: done
  timeout: 10s
```

`finish` 观察目标，超时或观察错误就失败；命中后继续后续步骤。它是可选的视觉断言，不是结束任务指令。脚本成功取决于全部步骤无异常执行完成，函数或被调用脚本可通过 `return` 提前返回。

```yaml
- fail: 未满足领取前置条件
```

`fail` 明确停止并给出业务原因。

失败可以提供结构化诊断，便于 AI 区分脚本错误与可恢复的环境故障：

```yaml
- fail:
    code: STAMINA_READ_FAILED
    stage: stamina
    message: 连续采样未取得可信体力值
    expected: 两次一致且处于有效范围
    actual: $power
    retryable: false
```

多目标任务用原生 `checkpoint` 声明完成项。ID 使用固定字符串；模板应只包含
稳定的已完成状态。每个实际执行的 `checkpoint` 都必须观察到目标，否则立即失败；不要求额外统一 `finish`。已完成分支仍须重新观察，不能拿历史检查结果跳过验证。

```yaml
- checkpoint:
    id: daily_rewards
    template: 每日奖励全部已领.png
    threshold: 0.9
    timeout: 5s
- finish: world
```

### 图像证据 `trace`

```yaml
- trace: false
- trace: true
```

开启与关闭均有边界记录。关闭期间不持续采图；运行报错时保留最后实际观察帧，并尽力取得新的错误截图。两者时间和类型分开，不把新截图冒充执行时已见画面。

## 普通函数、表达式与控制流

普通函数调用、`if`、有界 `repeat`、`break`、`return`、参数和变量继续使用现有语义。以下为可嵌入 `run` 的片段，不是缺少版本与完成条件的完整脚本：

```yaml
- find: claim.png
  as: hit
- if: $hit
  then:
    - tap: $hit
  else:
    - log: 未观察到领取按钮
- repeat: 3
  do:
    - log: 检查状态
    - break: {}
- swipe:
    from: [0.5, 0.8]
    to: [0.5, 0.2]
    duration: 500ms
- key: HOME
- sleep: 200ms
```

- 一个普通步骤包含一个调用或控制流动作，`as` 接收调用返回值
- `find` 是单次匹配，不点击；新流程推荐用 `wait` 表达必需等待，用 `optional` 表达可选画面
- 底层 `wait_find` 等便利函数仍由原生注册表定义；不要假定它们与 `wait` 的点击和超时语义相同
- `if` 仅将 `false` 和 `null` 视为假；比较调用 `eq / ne / gt / ge / lt / le`
- `repeat` 为非负整数或引用；`break` 退出当前函数内最近循环，不跨函数边界
- 表达式只有字面量与 `$name.field` 引用；`$$` 转义字面量 `$`，无 eval、算术插值或动态索引
- 数组和对象中的步骤实参递归解析引用，变量初值和参数默认值是字面量
- `reset_input: {}` 重置当前运行入口：浏览器刷新当前绑定网页，刷新命令返回即完成此步，不额外等待新文档或页面加载；scrcpy 冷启动设备配置的 App。会清理按住状态并使旧截图失效，保留采集会话与运行租约。无参数，不从配置包推导 App，也不切换到弹出的标签页。通常放在第一步，随后仍须等待并识别入口画面；刷新/启动成功不等于已进入游戏
- `launch / stop_app / reset_input` 是真实设备副作用；素材检查不执行任何脚本动作，也不发送通知或网络请求
- 普通等待有超时；`wait_find: {template: login.png, forever: true, obstacles: [expired.png], interval: 1s}` 可用于直播扫码，持续观察并点击过期刷新障碍，直到命中或主动取消。模板缺失、设备断开与权限错误仍立即失败。`forever` 只作用于该等待函数，不放宽循环步数和调用深度预算
- `hover: $button` 仅移动浏览器鼠标，不按下；支持匹配结果或坐标，复用输入屏障、帧身份及五秒观察有效期。Android 明确报不支持

同帧模板分支保留 `match_templates`，按 case 顺序仅执行第一个命中分支，不自动点击：

```yaml
- match_templates:
    cases:
      - template: announcement.png
        as: hit
        do:
          - tap: $hit
      - template: login.png
        do:
          - log: 登录页
    else:
      - log: 未识别画面
```

`times` 默认 1；显式多轮时使用有界 `times` 和 `interval`。模板分支内的别名只在该分支有效。可选字段和取值范围以权威函数目录与服务端诊断为准。

## 切换直播输出设备

启用直播助手并先开始输出后，可用普通函数切换到当前脚本目标或指定 Android／浏览器目标 ID：

```yaml
run:
  - switch_live_device: {}  # 当前脚本目标
    as: output
  - switch_live_device:
      device_id: browser-hsr-account-2
      timeout: 30s
```

切换保留同一输出地址、编码画幅、帧率、码率和输出方式；不同尺寸的画面等比例居中，直播姬无需重新添加素材。新画面准备成功后才切换，准备失败或取消保留原输出。`timeout` 默认 10 秒，允许 1ms..60s；返回 `{device_id, state, output_id}`，不包含素材令牌或推流地址。要求 `media.stream` 和已启用的 `gamer-live >=0.2.6`；输出未开始、插件不可用、权限不足或新目标失败均明确报错，不自动开播。Android 沿用设备音频；浏览器目前输出画面和静音，不采集电脑声音或麦克风。

## 等待当前直播观众确认

直播队列主脚本在扫码登录成功、观察到进入游戏按钮后，可调用普通原生函数等待本次任务触发者的确认弹幕：

```yaml
- wait_live_confirmation: {timeout: 5m}
  as: decision
- eq: {a: $decision.status, b: confirmed}
  as: confirmed
- if: $confirmed
  then:
    - 日常.yaml: {}
  else:
    - return: false
```

`timeout` 默认 `5m`，允许 `1ms..5m`，向上取整到秒，并受入队时冻结的规则期限限制；重复调用或轮询不会延长期限。返回对象只含 `status`，值为 `confirmed`、`cancelled` 或 `timeout`。当前队列任务的运行 ID、设备和配置包由宿主固定，脚本不能传入用户身份或运行 ID；确认弹幕的匹配、同一发送者校验和结果持久化由直播插件完成。

要求 `runtime.sleep`、已启用且支持确认等待的直播插件，以及启用了确认流程的当前队列主运行。手动运行、缺少规则、插件不可用、连接中断或状态异常均明确失败，不自动进入游戏。主动停止抛出 `CANCELLED`，不继续执行脚本输入。观众取消和确认超时返回普通结果，队列据此选择不刷新页面的退出收尾；普通日常失败仍由原重置收尾处理。此函数不扣除次数。

## 按需 OCR 文字识别

Core 提供 OCR 能力，自动化函数目录保留 `ocr` 与 `find_text`，无需 OCR 插件。识别使用原始截图，可以限定小区域；单屏/多画面预览的缩图和帧率不会改变 OCR 输入。Tesseract 每次按需启动，识别结束退出，多账号共用有界工作队列。引擎和模型在启动/设置阶段准备，缺失、超时及权限不足均明确失败，不冒充空识别结果；识别途中不会下载模型。外部受限运行另外需要 `ocr.recognize` scope。

以下为可嵌入 `run` 的片段，原有版本与 `finish` 完成条件仍须保留：

```yaml
- ocr:
    region: [0.25, 0.3, 0.5, 0.2]
    language: chi_sim+eng
    mode: block
  as: queue
- log: $queue.text
- find_text:
    text: 确定
    region: [0.2, 0.5, 0.6, 0.4]
    match: exact
    min_confidence: 0.7
    mode: sparse
  as: confirm_text
- if: $confirm_text
  then:
    - tap: $confirm_text
```

两函数的共同参数：

| 参数 | 默认值 | 说明 |
| --- | --- | --- |
| `region` | 全图 | `[x,y,w,h]` 相对坐标；宽高必须为正，区域须完全位于原图内 |
| `language` | `chi_sim+eng` | 已安装的语言模型；仅英文/数字可以使用 `eng` |
| `mode` | `block` | `line` 单行、`block` 一块文本、`sparse` 分散文本 |
| `whitelist` | 无 | 字符白名单，例如 `0123456789:`；有中文时不要限制为数字 |
| `scale` | `2` | 识别前放大倍数 `1..3`；结果位置仍对应原始截图 |
| `timeout` | `10s` | 单次识别上限 `1ms..15s`，超时明确失败 |

`ocr` 返回 `{text, confidence, lines, words}`，置信度范围为 `0..1`。每行/词包含 `text / confidence / x / y / width / height / center`，边界使用全图像素，`center` 使用相对坐标。原始帧身份 `_frame` 和取景时间 `_observed_at_ms` 随结果保留。

`find_text` 额外要求 `text`，`match` 为 `contains`（默认）或 `exact`，`min_confidence` 默认 `0.5`。按识别返回的行、词顺序取首个满足项；未找到返回 `null`。它只观察一次、不自动点击，可用现有有界 `repeat` 与 `sleep` 轮询。`tap: $结果` 或 `tap: $结果.center` 继续检查帧代次、坐标版本及 5 秒观察有效期；识别耗时不会延长旧画面的有效期。

素材检查不执行 OCR 或脚本流程，只检查语法、素材图片、资源引用和截图中的模板匹配。OCR 数值、等待、分支及完成条件需要实际运行验证。

## 函数库

```yaml
functions:
  announce:
    params:
      message:
        type: string
    run:
      - log: $message
      - return: true
```

调用：`announce: {message: 领取完成}`。函数有独立局部作用域，参数显式传递，返回值通过 `as` 接收。重复函数名拒绝保存和运行；删除或改名仍有引用的函数会给出相关文件诊断。

工作台不再提供独立函数页。函数在脚本顶层 functions 定义；普通调用支持本脚本函数、同包其他脚本及其中函数。原文编辑保留注释和定义，保存仍检查 expected_version。

## 工作稿、版本与素材检查

手动编辑或外部 Agent 通过统一 MCP 保存完整 YAML，新增工作稿小版本；人工检查、试跑后在 Gamer 发布大版本。MCP 不提供正式发布、回退和删除。源码注释继续使用标准 YAML `#`，保存和发布保留原文，不增加语法字段或模型生成门槛。

`.gamersample` 格式版本 2 包含原尺寸 PNG、原始操作事件、操作前后帧引用与来源摘要。制作时从录制提取图片，完成后不依赖原视频。原始像素用于裁切，缩放预览附带尺寸映射；操作事件作为教学证据，不执行录像回放。

“检查语法与素材”的 scope 为 screenshot_templates，检查 ZIP/PNG、源码/引用和 NCC 模板匹配；不运行分支、点击、等待或 OCR，不证明游戏任务完成。脚本、参数、模板或素材变化后重新检查。未绑定素材只能检查语法与引用，不能显示素材匹配通过。

素材制作和静态检查不需要设备租约。试跑与正式执行共用唯一权威解释器、Core 运行管理与输入屏障。外部私有试跑受令牌撤销/到期取消；已受理的正式运行独立继续，但始终执行冻结的受限策略，不能借原生函数越权通知或推流。

全部工作稿版本、发布事务、正式脚本、模板、旧候选、素材及原模型用量元数据保留在原位置。旧候选只读并可按原字节导出；显式导入使用未占用的新工作稿名称，不覆盖旧身份或正式资源。详见 [MCP 接入](../guides/external-agent-mcp.md)。

## 运行记录、错误与图像

运行记录通过 `run_id` 查询，包含步骤、调用、参数、返回值、匹配、输入、取消和错误。诊断包含 `code / path / message`，有定位信息时界面显示源路径；版本不一致时使用只读源提示，不错误跳到当前编辑行。

- `GET /api/runs/:run_id/events?after=0`：事件分页
- `GET /api/runs/:run_id/trace?after=0&limit=100`：图像目录、证据缺口、开关与过期状态
- `GET /api/runs/:run_id/trace/images/:image_id`：受认证保护的原图；过期返回 410
- 同一图像地址加 `?template=true` 可查看其对应的冻结模板图像
- `POST /api/runs/:run_id/trace/retain`：显式保留已结束且未过期的证据，受持久存储限额约束

`trace.frame_id` 是调用栈帧，`image_id` 才是图像身份，两者不混用。步骤详情显示关联缩略图、原图、搜索框与命中框；图像缺失、Trace 关闭、采集缺口与过期均明确提示。文字事件不能被显示成不存在的图像证据。

运行详情可复制 Agent 引用，让外部客户端按准确的 run_id 读取日志、判断帧及冻结源码。查看失败版本使用该次运行的原始版本，缺失时明确提示，不用当前最新版本替代。外部建议经工作稿保存和重新验证后，由用户决定发布。

## 验证边界

本地确定性测试可以覆盖解析、无设备素材验证、错误定位、版本冲突、取消、迟到结果和界面保存门槛。真实 Android/CDP 输入效果、实际游戏素材质量与真实模型视觉能力需要单独实测，不能用协议桩或组件测试替代。

## 按结果发送通知

`notify` 是原生插件函数，默认 `when: immediate`，立即提交通知。`when: success` 或 `when: failure` 在当前脚本或函数作用域登记结果通知；该作用域全部步骤执行完毕后发送成功通知，步骤异常或解释器错误发送失败通知。失败正文附错误摘要。`enabled` 是布尔参数，默认 `true`，可直接引用当前脚本的布尔参数：

```yaml
params:
  notify_success: {type: boolean, default: true, desc: 成功是否通知}
  notify_failure: {type: boolean, default: true, desc: 失败是否通知}
run:
  - notify: {when: success, enabled: $notify_success, content: 日常执行成功}
  - notify: {when: failure, enabled: $notify_failure, content: 日常执行失败}
  - reset_input: {}
```

跨脚本调用 `云游戏日常.yaml: {notify_success: false, notify_failure: true}` 会关闭被调用日常的成功通知，保留其失败通知。日常返回后调用方继续执行；同一次错误优先发送最具体的子作用域失败通知，避免外层重复报告。

根作用域的显式结果登记覆盖该脚本已有的全局和任务默认通知规则，`enabled: false` 也不会被默认规则补发。准备阶段发生的错误尚未登记策略，仍沿用已有默认规则。保存的全局和任务通知配置不会被删除或改写。取消使尚未完成的作用域走失败结果；结果订阅仅属于本次运行，后续运行重新登记。

`title` 可选。即时发送的 `channel` 留空使用全局默认通道；结果通知先沿用本作用域脚本已配置的启用结果订阅规则中的通道，没有匹配规则才使用全局默认通道。跨脚本调用按被调用脚本的源码作用域选择，显式 `channel` 优先；不改用户的默认通道配置。通知自动附 Android 或浏览器目标的配置名称；已删除目标使用 ID。即时调用返回发送提交结果，结果登记返回 `{accepted:false,status:"registered",when,enabled,frame_id}`，登记不代表发送成功。通知插件不可用或发送失败只记录原因，不改变脚本执行结果。
