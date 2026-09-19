# YAML 案例教程

先选配置包，在「自动化」中新建脚本，再粘贴案例。找图需要先在同一配置包的「模板」中准备图片；运行前选好设备和目标应用。下面每个脚本代码块都是独立案例，模板名和坐标按实际画面修改。不写 `version` 字段。

`#` 后面是注释。标「可选」的参数行可以整行删除，使用默认行为；被注释掉的参数，去掉开头的 `#` 即可填写。编辑器只自动展开无默认值的必填参数，可选参数和有默认值的参数点击按钮才展开。

所有函数都有可选的 `name` 参数，控制可视化卡片的显示文字；不写时使用对应中文名。配置包函数默认使用说明，未写说明则使用函数名。填写后卡片只展示 `name` 的值，调用目标不变。所有内置 `timeout` 默认都是 `3s`，仍可显式覆盖。

## 1. 等按钮出现，再点击

```yaml
name: 打开指南                 # 可选：脚本说明名，不是文件名
run:                         # 必填：按顺序执行的步骤
  - wait_find:
      name: 等待模板出现 # 可选：可视化显示名称
      template: 指南.png      # 必填，无默认值：要找的模板
      # threshold: 0.8       # 可选，默认 0.8：匹配阈值
      # timeout: 3s         # 可选，默认 3s：最多等待多久
      # interval: 250ms      # 可选，默认 250ms：多久找一次
      # region: [0, 0, 1, 1] # 可选，无固定默认值：[x, y, 宽, 高]，范围 0～1
                            # 不填 region：按模板文件名的区域后缀搜索，无后缀则全屏
    as: guide               # 可选：把结果存为 guide；没找到时结果为 null

  - if: $guide              # 找到了才点击；$guide 表示读取刚才的结果
    then:
      - tap:
          name: 点击 # 可选：可视化显示名称
          position: $guide.center # 必填，无默认值：点击找到的位置
      - sleep: 1s           # 时长必填，无默认值；这里等 1 秒
    else:
      - log:
          name: 日志 # 可选：可视化显示名称
          message: 没找到指南 # 必填，无默认值：日志内容
          # level: info     # 可选，默认 info；可用 debug / info / warn / error
```

坐标范围为 `0～1`，左上角是 `[0,0]`，右下角是 `[1,1]`。时间可写 `500ms`、`1.5s`、`2min`，纯数字表示毫秒。

所有找图函数的 `region` 都可省略：模板文件名的 `#a/u/d/l/r/ul/ur/dl/dr` 后缀分别表示全屏、上、下、左、右、左上、右上、左下、右下区域，无后缀则全屏。例如图片叫 `指南#ur.png`，脚本可用唯一短名 `指南.png`，保留 `.png`。

匹配对象的 `center` 是相对坐标，`score` 是匹配分数，`x/y/width/height` 是像素位置和大小，`region` 是相对区域对象。没找到返回 `null`，先用 `if` 判断，再读取 `.center`。

## 2. 另外三种找图操作

```yaml
run:
  - find:                     # 只找一次，返回匹配对象或 null；没有 timeout / interval 参数
      name: 查找模板 # 可选：可视化显示名称
      template: 指南.png        # 必填，无默认值
      threshold: 0.8           # 可选，默认 0.8
      region: [0, 0, 1, 1]     # 可选；省略时按模板后缀确定区域
    as: current               # 可选：保存返回值
  - log: $current             # 简写：值传给第一个参数，这里就是 message

  - tap_template:             # 找到就点击中心，随后等 300ms；没找到不点击
      name: 点击模板 # 可选：可视化显示名称
      template: 指南.png        # 必填，无默认值
      threshold: 0.8           # 可选，默认 0.8
      timeout: 3s              # 可选，默认 3s；显式写 0ms 只找一次
      interval: 100ms          # 可选，默认 100ms（轮询时生效）
      region: [0, 0, 1, 1]     # 可选；省略时按模板后缀确定区域
    as: tapped                # 可选：匹配对象或 null

  - wait_disappear:           # 等模板消失；已消失返回 true，超时仍在返回 false
      name: 等待模板消失 # 可选：可视化显示名称
      template: 指南.png        # 必填，无默认值
      threshold: 0.8           # 可选，默认 0.8
      timeout: 3s             # 可选，默认 3s
      interval: 250ms          # 可选，默认 250ms
      region: [0, 0, 1, 1]     # 可选；省略时按模板后缀确定区域
    as: gone
  - log: $gone
```

`wait_find` 等出现，`wait_disappear` 等消失；轮询间隔最低为 `50ms`。超时未命中是正常返回；模板文件不存在、参数错误等会终止运行并报错。

## 3. 启停应用、滑动、按键和输入文字

```yaml
run:
  - launch:
      name: 启动应用 # 可选：可视化显示名称
      package: com.example.game # 可选：改成实际 Android 应用包名；省略则用设备配置的应用
  - sleep:
      name: 等待 # 可选：可视化显示名称
      duration: 2s            # 必填，无默认值：launch 是冷启动，这里等待 2 秒
  - tap: [0.5, 0.5]           # position 的简写；也可写 tap: {position: {x: 0.5, y: 0.5}}
  - swipe:
      name: 滑动 # 可选：可视化显示名称
      from: [0.5, 0.8]        # 必填，无默认值：起点
      to: [0.5, 0.2]          # 必填，无默认值：终点
      duration: 300ms         # 可选，默认 300ms
  - input_text:
      name: 输入文本 # 可选：可视化显示名称
      text: 你好               # 必填，无默认值：先确保输入框已获得焦点
  - key:
      name: 按键 # 可选：可视化显示名称
      key: BACK               # 必填，无默认值：如 BACK / HOME，或数字 keycode
      action: press           # 可选，默认 press；down 为按下，up 为松开
  - stop_app:
      name: 停止应用 # 可选：可视化显示名称
      package: com.example.game # 可选：省略则用设备配置的应用
```

使用设备配置的应用时，写 `launch: {}`、`stop_app: {}` 即可。这里的 `package` 是 Android 应用包名，与保存脚本的配置包 ID 分开。这些操作函数以及 `sleep`、`log` 都返回 `null`。

## 4. 运行参数、变量、循环和提前结束

```yaml
name: 重复点击
params:                       # 可选：运行时可填写或覆盖的参数
  target:
    type: point               # 每个参数必填 type，无默认类型
    required: true            # 必填参数，无默认值；运行前必须传入 target
    desc: 点击位置             # 可选说明
  times:
    type: integer
    default: 3                # 可选参数，默认 3；required 不写默认为 false
  note:
    type: string              # 可选参数，无默认值；本案例不使用它，可不填
vars:                         # 可选：脚本内的固定字面量
  pause: 500ms
  label: 点击完成
run:
  - repeat: $times            # 次数必填：非负整数或整数引用；0 表示不执行
    do:                       # 循环体必填，可写空列表 []
      - tap: $target
      - sleep: $pause
  - log: $label
  - return: {ok: true, count: $times} # 返回对象并结束当前脚本；return 步骤可省略
```

`$times` 读取变量，`$guide.center` 读取对象字段；步骤中的数组、对象也可以嵌套引用。`vars` 和参数 `default` 是字面量，不展开引用。要输出以 `$` 开头的文本，可写 `log: $$price`，输出 `$price`。不支持 `$times + 1`、字符串插值或数组下标。

`if` 的 `then` 必填，`else` 可选。只有 `false` 和 `null` 算条件不成立，`0`、空字符串也算成立；数值判断先调用比较函数。`return` 放在分支或循环内也会结束当前脚本或函数；不支持 `break`、`while`、`try/catch`。

参数的全部类型如下；脚本和自定义函数使用同一套声明方式。`default`、`required`、`desc` 都可选，默认值必须符合 `type`。

| type | 值示例 |
|---|---|
| `any` | `null`、`true`、数字、字符串、列表或对象 |
| `boolean` | `true` / `false` |
| `integer` | `3` |
| `number` | `0.8` |
| `string` | `你好` |
| `list` | `[1, 2, 3]` |
| `object` | `{name: 张三}` |
| `duration` | `500ms` / `1s` / `2min` / `500` |
| `point` | `[0.5, 0.8]` 或 `{x: 0.5, y: 0.8}` |
| `template` | `指南.png` |
| `key` | `HOME` / `BACK` / 数字 keycode |

## 5. 比较后决定是否执行

六个比较函数的 `a`、`b` 都必填，无默认值；`eq/ne` 可比较任意值，`gt/ge/lt/le` 只接受数字，都返回布尔值。

```yaml
vars:
  count: 3
run:
  - eq: {name: 等于, a: $count, b: 3}     # 等于；a、b 必填
    as: equal
  - ne: {name: 不等于, a: $count, b: 0}     # 不等于；a、b 必填
    as: not_equal
  - gt: {name: 大于, a: $count, b: 2}     # 大于；a、b 必填
    as: greater
  - ge: {name: 大于等于, a: $count, b: 3}     # 大于等于；a、b 必填
    as: enough
  - lt: {name: 小于, a: $count, b: 5}     # 小于；a、b 必填
    as: less
  - le: {name: 小于等于, a: $count, b: 3}     # 小于等于；a、b 必填
    as: at_most
  - if: $enough
    then:
      - log: 数量足够
    else:
      - log: 数量不足
  - return: $enough
```

## 6. 把重复步骤做成自定义函数

在当前配置包的「函数」中定义函数。下面是默认函数库 `automations/_function.yaml` 的完整文件格式：

```yaml
functions:                    # 函数库必填的顶层包装，可在下面并列定义多个函数
  每日任务跳转:                 # 调用名；中文函数名可用
    description: 等待并点击入口 # 可选说明
    params:                   # 可选：函数入参
      template:
        type: template        # type 必填
        required: true        # 必填，无默认值
      timeout:
        type: duration
        default: 3s           # 可选，默认 3s
    vars:                     # 可选：只属于这个函数
      done: 已进入任务
    returns:                  # 可选：返回值说明，仅用于提示，不做运行时类型校验
      type: boolean
    run:                      # 必填：函数步骤
      - tap_template:
          template: $template
          timeout: $timeout
        as: hit
      - if: $hit
        then:
          - log: $done
          - return: true      # 提前结束当前函数，把 true 返回给调用方
      - return: false
```

保存函数后，在同一配置包新建脚本调用：

```yaml
run:
  - 每日任务跳转:
      template: 指南.png        # 必填，无默认值
      # timeout: 10s          # 可选；不写使用函数声明的 3s
    as: success               # 可选：接收函数返回值
  - if: $success
    then:
      - log: 可以继续后续任务
    else:
      - log: 入口没找到
  - return: $success
```

函数通过参数接收数据、通过 `return` 返回结果，不能直接读取调用方的局部变量。函数也能调用当前配置包的其他函数。函数名允许汉字、小写字母、数字、下划线，不能以数字开头；不能与内置函数、其他自定义函数或 `if/repeat/return` 重名。参数名和变量名使用小写字母、数字、下划线，不能以数字开头。

默认函数库可放多个函数；手动拆分时文件名用 `_function其他名称.yaml`，同样放在 `automations/`。调用只写函数名，不带文件名或目录；不要把 `functions:` 写进普通脚本。

## 7. 保存与运行

先保存模板和函数，再保存调用它们的脚本。手动运行时填写必填参数；可选参数有默认值的，不填则使用默认值。可先在「函数」中测试一个函数，再运行完整脚本。定时执行时，在「任务」中选择 YAML 执行器、对应配置包和脚本，填写运行参数与时间后保存任务。

本页覆盖当前全部 **18 个内置函数**及四种步骤（函数调用、`if`、`repeat`、`return`）。细节和诊断说明见 [YAML 参考](../reference/YAML.md)。
