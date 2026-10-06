# YAML 自动化语法 v2

当前正式语法必须声明 `version: 2`。旧无版本脚本不静默兼容；请明确改写目标、观察、动作与完成条件。脚本和函数库都使用版本 2，生产执行与离线素材验证共用同一个解析器和解释器。

权威实现：`plugins/gamer-yaml/host/syntax.rs`、`host/syntax/`、`plugins/gamer-yaml/interpreter/`。原生函数参数以 `GET /api/runners/gamer-yaml/functions` 和 `host/native_funcs.rs` 注册表为准，不从模型名称或前端表单猜测。

## 最小完整自动化

```yaml
version: 2
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

`wait` 只观察，不隐式点击。匹配对象与实际截图来源绑定，后续 `tap` 复用这次观察。`finish` 必须观察到已声明的完成画面；执行到文件末尾或点击结束均不等于游戏目标完成。

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

`version`、`name`、`params`、`vars`、`targets` 与 `run` 描述脚本。未知字段产生结构化诊断，原文编辑器保留原始内容供修正，不用旧模型重新序列化覆盖。

```yaml
version: 2
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

### 完成 `finish` 与失败 `fail`

```yaml
- finish: done
  timeout: 10s
```

`finish` 观察到完成目标才标记脚本目标达成。没有到达 `finish` 的脚本不应作为成功自动化保存。函数中的普通 `return` 仍可返回值，不替代顶层自动化的完成证明。

```yaml
- fail: 未满足领取前置条件
```

`fail` 明确停止并给出业务原因。

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
- `launch / stop_app` 是真实设备副作用；离线回放不会执行它们，也不会发送通知或网络请求
- 等待、循环、调用深度受取消与执行预算约束，不存在无界“自动修好”保证

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

## 函数库

```yaml
version: 2
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

函数页编辑完整原文库，保留注释、顺序与所有定义；右侧函数选择器仅决定测试运行入口，不会过滤后再覆盖其他函数。保存使用 `expected_version` 防止覆盖别人刚保存的资源。

## 多素材生成、验证与保存

自动化工作台的“AI 生成与验证”支持：

1. 选择视频素材，或导入可携带的 `.gamersample` 归档，填写目标和脚本位置
2. 通过 AI 生成候选，或选“手写源码离线验证”自行建立候选
3. 在实际素材图上通过同一执行语义校验观察、动作顺序与完成画面
4. 有界自动修正并重新验证所有原始样本；不允许删除失败样本或放宽目标换取通过
5. 查看 YAML、候选模板来源与每份素材报告，再显式确认正式保存

离线验证、原文编辑、模板测试和历史查看不依赖 AI，也不需要连接设备。AI 生成功能会检查插件安装与运行状态、模型配置及绑定该配置版本的视觉能力探测。配置变动后旧探测失效。

报告分类：`passed`（通过）、`failed`（逻辑或匹配失败）、`insufficient_evidence`（证据不足）、`unsupported`（不支持）。只有每份选中素材都通过，才可保存“已验证正式版本”。编辑 YAML、模板或参数会使旧报告失效。

候选草稿保存在独立候选空间，失败不会污染正式脚本或模板。正式保存原子提交脚本、模板与版本记录，并检查原资源版本；历史中的“撤销此变更”将该次变更涉及的脚本与模板恢复到变更前状态，保留无关资源，必须明确确认。通过只证明素材已记录路径，不证明未出现分支或任意新操作的游戏效果。

## 运行记录、错误与图像

运行记录通过 `run_id` 查询，包含步骤、调用、参数、返回值、匹配、输入、取消和错误。诊断包含 `code / path / message`，有定位信息时界面显示源路径；版本不一致时使用只读源提示，不错误跳到当前编辑行。

- `GET /api/runs/:run_id/events?after=0`：事件分页
- `GET /api/runs/:run_id/trace?after=0&limit=100`：图像目录、证据缺口、开关与过期状态
- `GET /api/runs/:run_id/trace/images/:image_id`：受认证保护的原图；过期返回 410
- 同一图像地址加 `?template=true` 可查看其对应的冻结模板图像
- `POST /api/runs/:run_id/trace/retain`：显式保留已结束且未过期的证据，受持久存储限额约束

`trace.frame_id` 是调用栈帧，`image_id` 才是图像身份，两者不混用。步骤详情显示关联缩略图、原图、搜索框与命中框；图像缺失、Trace 关闭、采集缺口与过期均明确提示。文字事件不能被显示成不存在的图像证据。

“交给 AI 分析”仅附上用户选定的脚本和相关运行 ID，由受限工具读取必要上下文。分析不获取设备控制租约；模型只保留待审核建议，不直接修改当前候选或正式资源。用户在自动化面板确认“应用到候选”后，旧报告失效，需重新验证再确认正式保存。

## 验证边界

本地确定性测试可以覆盖解析、无设备素材验证、错误定位、版本冲突、取消、迟到结果和界面保存门槛。真实 Android/CDP 输入效果、实际游戏素材质量与真实模型视觉能力需要单独实测，不能用协议桩或组件测试替代。
