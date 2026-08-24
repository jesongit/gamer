# YAML 自动化脚本语法

本文件是 GameBot YAML 自动化脚本的完整语法说明，以服务端引擎
（`server/src/engine.rs`）实际实现为准。模板图片按应用分区存放在
`data/<应用包名>/tmpl/`，脚本中引用模板名时只需写文件名（如 `shop.png`）。

## 脚本结构

```yaml
name: 每日签到          # 脚本名（可选）
action_wait: 500        # 操作间隔：每个操作执行后的默认等待毫秒数（可选，默认 500）
log_level: info         # 日志级别：info=精简（默认） / debug=详细
steps:                  # 必填，按顺序执行的动作列表
  - wait: 1000
  - log: "开始"
```

- `action_wait` 是当前脚本的操作间隔：每个操作（除 `wait` 动作本身）执行完后
  默认等待这么多毫秒。单个步骤可用 `wait` 参数覆盖（如 `wait: 200`，`0` 不等待）。
- `log_level: debug` 时记录 debug 级日志（每次点击/滑动的坐标、循环次数等），
  默认 `info` 只记录动作级日志。
- `name` 仅作标识，引擎不参与逻辑。

## 书写约定

- 每个动作以 `- 动作名:` 开始
- 无参动作可省略冒号直接写 `- str_app` / `- cls_app`（等价于 `- str_app:`，
  包名回退设备配置）
- 动作的参数与动作键同级缩进（即都缩在 `-` 下面）
- `then` / `else` 与动作键同级，是当前步骤的分支（`then` 支持按命中模板分支，见下文）
- `find` / `until` 不兼容对象写法；模板支持单模板字符串、逗号分隔多模板
  或列表三种写法（见下文「多模板查找」）：

```yaml
- find: sign_btn.png
  threshold: 0.85
  region: u
  click: true
  then:
    - log: "找到并点击"
  else:
    - log: "未找到"
```

## 找图参数（find / until 共用）

`find` / `until` 每轮按配置顺序**连续**匹配全部模板（每个模板独立取最新截图，
模板之间不等待），一轮未命中隔 `interval` 毫秒重开一轮（从第一个模板重新开始），
直到按 `and_or` 判定命中，或 `timeout` 超时。

| 参数 | 默认值 | 说明 |
|---|---|---|
| `interval` | `500` | 检测间隔毫秒：一轮未命中后隔多久重开一轮 |
| `timeout` | `find: 6000` / `until: 1800000` | 超时毫秒。`find` **必须大于 0**；`until` 默认 30 分钟，显式 `0` = 永不超时 |
| `and_or` | `find: and` / `until: or` | 多模板组合逻辑，见下文「多模板查找」 |
| `click` | `true` | 找到后如何点击，见下文（`and` 点第一个模板，`or` 点命中的模板） |
| `threshold` | `0.8` | 匹配阈值 0~1，越大越严格（默认取设备配置 `default_threshold`） |
| `region` | `a` | 搜索区域，见下表；也可由模板名 `#` 后缀按模板各自指定（见「模板名自带区域后缀」） |
| `then` | 不执行 | 找到后执行；列表项可写「模板名: 步骤列表」按命中模板分支，见下文 |

### region 搜索区域

`region` 支持字符串和相对坐标数组两种写法。

字符串取值：

| 值 | 含义 |
|---|---|
| `a` | 全屏（默认） |
| `u` | 上半屏 |
| `d` | 下半屏 |
| `l` | 左半屏 |
| `r` | 右半屏 |
| `ul` | 左上四分之一 |
| `ur` | 右上四分之一 |
| `dl` | 左下四分之一 |
| `dr` | 右下四分之一 |

数组写法 `[x1, y1, x2, y2]`：`(x1, y1)` 是左上角相对坐标，`(x2, y2)` 是右下角相对坐标，
均为 `0~1` 百分比，需要 `x2 > x1` 且 `y2 > y1`：

```yaml
- find: sign_btn.png
  region: [0.000, 0.000, 0.500, 0.500]   # 左上四分之一
- find: shop.png
  region: [0.250, 0.250, 0.750, 0.750]   # 中间区域
```

也支持 `{fm: [x, y], to: [x, y]}` 对象写法（与 `swipe` 的起终点一致）：

```yaml
- find: shop.png
  region:
    fm: [0.250, 0.250]
    to: [0.750, 0.750]
```

### 模板名自带区域后缀

多模板查找时各模板往往在屏幕的不同区域，一个 `region` 参数管不过来——模板名可携带
`#` 后缀区域，**未显式写 `region` 参数时**按各模板自己的后缀区域匹配：

- `xx#l` / `xx#dr` …：`#` 后为上表半区码（`a`/`u`/`d`/`l`/`r`/`ul`/`ur`/`dl`/`dr`）
- `xx#0_0_500_500`：`#` 后为 `[x1, y1, x2, y2]` 相对坐标 **×1000 的 1~3 位整数**
  （`500` → 0.500），需 `x2 > x1` 且 `y2 > y1`——框选生成区域模板时前端自动命名成
  这种格式
- 后缀写在扩展名之前（`xx#l.png`），大小写不敏感

优先级：**显式 `region` 参数（统一作用于全部模板）> 模板名后缀 > 全屏**。
后缀解析不出区域（`#` 后不是合法码/坐标）按普通模板名全屏匹配，不报错。

**短名引用**：脚本里可以只写去掉 `#` 后缀的短名——`find: login.png` 引用
`login#907_160_973_717.png`，引擎自动解析**唯一**匹配的带后缀文件，区域照常生效；
精确全名永远可用且优先。同基名存在多个后缀文件时短名执行报错并列出候选，
需写全名消歧。

```yaml
- find: hp#l.png, skill#r.png          # hp 只搜左半屏、skill 只搜右半屏
- find: boss.png, hint#0_0_500_500.png   # boss 全屏、hint 只搜左上四分之一
- find: login.png, act_cls.png         # 短名：自动解析到 login#.../act_cls#... 文件
```

### click 参数

`click` 控制找到模板后是否点击、点哪里，**默认 `true`**（点击模板中心）：

| 取值 | 行为 |
|---|---|
| 不写 / `true` | 点击模板中心点 |
| `false` | 只判断模板是否出现，不点击 |
| `button.png`（模板名） | 在找到的模板**区域内**再找 `button.png`，找到就点击它的中心点；区域内没找到则继续循环查找，直到超时走 `else` |
| `[x1, y1]` | 点击模板区域内的相对坐标（`0~1`），如 `[0.5, 0.5]` 是模板中心点 |

```yaml
- find: dialog.png          # 对话框出现后，点它内部的关闭按钮
  click: close_btn.png      # 在 dialog.png 区域内找 close_btn.png 并点击
  else:
    - log: "对话框没出现"

- find: dialog.png
  click: [0.5, 0.1]         # 点击对话框区域内顶部中间的位置
```

### 多模板查找（and_or）

`find` / `until` 的模板支持逗号分隔或列表写法，配合 `and_or` 组合判定：

```yaml
- find: hp_full.png, mp_full.png    # 逗号分隔
  and_or: and                       # 全部找到才命中（find 默认）

- find: [shop_btn.png, mall_btn.png]   # 列表写法（等价）
  and_or: or                        # 任一找到即命中（until 默认）
```

- 一轮 = 按配置顺序**连续**匹配每个模板（各自独立取最新截图，模板间不等待），
  一轮未命中隔 `interval` 重开一轮
- `and`（find 默认）：一轮内**全部找到**才命中；某个模板未命中即本轮失败，
  后面的模板不再匹配
- `or`（until 默认）：**任一找到**即命中；某个模板命中后后面的模板不再匹配
- 命中后的 `click`：`and` 点击**第一个**模板，`or` 点击**命中的**模板；
  `click` 为模板名 / `[x, y]` 时同样作用于该模板的区域内
- 各模板搜索区域不同时，用**模板名 `#` 后缀**按模板指定（如 `find: hp#l.png,
  skill#r.png`），见上文「模板名自带区域后缀」；显式 `region` 参数会统一覆盖全部模板
- 单模板写法与旧版完全兼容（`and_or` 退化为普通命中）

### then 按命中模板分支

`then` 的列表项支持**单键映射**写法：键 = 模板名（必须在 find/until 的模板列表中）、
值 = 命中该模板时执行的步骤；其余普通步骤 = **兜底**（命中的模板没有专属分支时执行）。
`and` / `or` 模式通用：

```yaml
- find: test1.png, test2.png
  and_or: or
  then:
    - test1.png:           # 命中的是 test1.png 时走这里
        - log: "1"
    - test2.png:           # 命中的是 test2.png 时走这里
        - log: "2"
    - log: "兜底"          # 命中的模板没有上面的分支时执行
```

- 分支选择 = **书写顺序第一个**模板在命中列表里的分支：
  - `or`（命中即停）：命中的恰为一个模板，命中谁走谁——test1 和 test2 都在屏上
    但先命中 test1 时走 test1 的分支（后面不再匹配 test2）
  - `and`（全部命中）：所有模板都在命中列表里，取**书写顺序第一个**分支
- 命中的模板没有专属分支 → 执行兜底的普通步骤（即原有 then 语义）
- 分支的模板名必须写在 `find` / `until` 的模板列表中，拼错会执行报错
  （不会被静默当成普通步骤跳过）
- 命中分支时同样会先按 `click` 参数处理点击，再执行该分支的步骤
- 只写普通步骤（旧写法）不产生任何分支，行为与旧版完全兼容

## 相对坐标

`tap`、`swipe`、`region` 数组、`click` 数组使用相对坐标百分比，`0~1` 之间的小数，
一般保留三位小数：

- `0.500` 表示水平/垂直方向的 50%
- 与设备分辨率解耦，同一套脚本可通用于 1920x1080、1080x1920 等分辨率
  （设备实际分辨率/方向被游戏改变也不影响）

## 动作语法

### 1. wait —— 等待 / 随机延时

```yaml
- wait: 1000            # 固定等待 1000ms
- wait: [500, 1500]     # 随机等待 500~1500ms（模拟人工）
- wait: {min: 300, max: 900}   # 等价写法
```

### 2. log —— 输出日志

```yaml
- log: "任务完成"
```

### 3. key —— 按键

```yaml
- key: HOME            # HOME/BACK/APP_SWITCH/MENU/VOL_UP/VOL_DOWN/POWER/ENTER/TAB/SPACE/ESC...
```

常用按键：`HOME` / `BACK` / `APP_SWITCH`(或 `RECENTS`) / `MENU` / `VOL_UP` / `VOL_DOWN` /
`POWER` / `ENTER` / `DEL`(或 `BACKSPACE`) / `TAB` / `SPACE` / `ESC` / `SEARCH` /
`CAMERA` / `FOCUS` / `NOTIFICATION` / `SETTINGS` / `MUTE` / `HEADSETHOOK` /
`WAKEUP` / `SLEEP` / 数字键 `0`~`9`，也支持直接写 Android keycode 数字。

### 4. text —— 输入文本

```yaml
- text: "hello world"
```

### 5. tap —— 按相对坐标点击

```yaml
- tap: [0.500, 0.500]   # 点击屏幕中心
- tap: [0.125, 0.900]   # 点击左下角附近
- tap: {x: 0.5, y: 0.5} # 等价对象写法
```

### 6. swipe —— 滑动

```yaml
- swipe:
    fm: [0.500, 0.800]   # 起点（旧写法 from 兼容）
    to: [0.500, 0.200]   # 终点
    time: 1000           # 1000ms 内完成滑动（默认 500）
```

### 7. find —— 限时查找模板（找图/点击/等待统一入口）

`find` 每轮检测模板是否出现（多模板见「多模板查找」），直到出现或 `timeout` 超时：

- 找到 → 按 `click` 参数点击（如有）→ 执行 `then`（支持按命中模板分支，见上文）
- 超时未找到 → 执行 `else`
- `timeout` **必须大于 0**；"一直等到出现"请用 `until`（见下）

```yaml
- find: shop.png
  interval: 500         # 检测间隔 ms（默认 500）
  timeout: 6000         # 超时 ms（默认 6000，必须 > 0）
  click: true           # 找到后点击模板中心（默认 true，false 不点击）
  threshold: 0.85       # 匹配阈值（默认 0.8）
  region: a             # 搜索区域（默认 a）
  then:                 # 找到并点击成功后执行
    - log: "点击商店成功"
  else:                 # 超时未找到执行
    - log: "没有找到商店按钮"
    - goto: retry
```

开始查找时引擎会打印日志：`查找模板 xxx，超时 ...，检测间隔 ...ms`
（多模板为 `查找模板 a、b（and 全部命中），...`）。

### 8. until —— 等到模板出现

`until` 与 `find` 参数完全一致（`interval` / `and_or` / `click` / `threshold` /
`region` / `then` / `timeout`），区别在默认值：

- `and_or` 默认 `or`：任一模板出现即命中（`find` 默认 `and`）
- `timeout` 默认 **30 分钟**（1800000ms）：超时后执行 `else`；
  显式 `timeout: 0` = 永不超时（此时 `else` 永不执行）

```yaml
- until: loading_done.png   # 一直等到加载完成（不点击）
  click: false

- until: done.png           # 等到出现并点击模板中心（click 默认 true）
  interval: 500
  then:
    - log: "完成了"

- until: page_a.png, page_b.png   # 等任一页面出现（and_or 默认 or）
  timeout: 0                      # 永不超时
```

- 与 `find` 的区别：默认 `or` 组合、超时上限大得多，适合"等到出现为止"的长等待

### 9. str_app —— 冷启动应用

先 force-stop 该应用（无论它是否在运行）再启动，保证进入干净状态。
虚拟屏设备自动启动到虚拟屏。

```yaml
- str_app: com.x.y          # 冷启动指定应用
- str_app                   # 包名省略 → 用设备配置的应用包名
```

- 应用启动要 1~3 秒，`str_app` 后的默认等待是 **3000ms**（其他动作默认 500ms），
  可用 `wait` 参数覆盖
- 固定冷启动语义：已在运行也会先杀掉重启；只想"没启动才启动"用 `cls_app` + 判断自行组合

### 10. cls_app —— 关闭应用

关闭指定应用，**不影响投屏/会话**（视频流继续，脚本可接着 `str_app` 切换别的应用）：

```yaml
- cls_app: com.x.y          # 关闭指定应用
- cls_app                   # 包名省略 → 用设备配置的应用包名
```

- 幂等：应用本来就没在运行也无害，适合放在脚本开头"确保冷状态"
- 虚拟屏设备上应用被杀后画面会变成桌面或黑屏，流不会断，属预期行为

### 11. loop —— 次数循环

```yaml
- loop:
    times: 3
    steps:
      - tap: [0.500, 0.800]
      - wait: 500
```

### 12. goto / label —— 跳转

```yaml
- label: retry
- goto: retry
```

`label` 本身不执行任何操作，只作为 `goto` 的跳转目标（goto 只支持向后跳转的写法，
`label` 需在脚本中先定义）。

### 13. call —— 调用子脚本

```yaml
- call: 子脚本.yml
```

子脚本从 `steps` 开头执行，日志合并到当前脚本；子脚本不存在时报错。

## 空闲低功耗模式

脚本运行结束（成功/失败/手动停止）后，若配置了 `idle_disconnect_secs`（默认 60，0 关闭），
服务端会在延迟 N 秒后检查：该设备**没有正在运行的脚本、没有投屏页面**时，自动断开
scrcpy 会话——恢复熄屏超时、停止设备侧编码、销毁虚拟屏，进入低功耗；**adb 链路保留**，
下一次脚本运行/定时任务会自动重连（约 2~4 秒）。服务器启动时也会自动扫描设备并维持
adb 连接（WiFi 设备周期保活），但不建立会话、不启动应用。

有人开着投屏页面（viewer 在线）时不会自动断开——属于有人值守场景。

## 完整示例

```yaml
name: 每日签到
action_wait: 500
log_level: info

steps:
  - str_app: com.game.example   # 冷启动游戏（已在运行也先杀再启）
  - find: main_page.png
    timeout: 30000
    else:
      - log: "进入主界面超时"
      - goto: fail

  - find: sign_btn.png
    threshold: 0.85
    region: u
    click: true
    then:
      - log: "点击签到按钮"
    else:
      - log: "签到按钮不存在"
      - goto: retry

  - until: signed_done.png      # 一直等到签到完成提示出现
    then:
      - log: "签到成功"
      - goto: end

  - swipe:
      fm: [0.500, 0.800]
      to: [0.500, 0.200]
      time: 800
  - wait: [500, 1000]
  - label: end
  - log: "脚本完成"
  - cls_app: com.game.example   # 收工：关闭游戏（会话保留，空闲后自动进低功耗）

  - label: fail
  - log: "任务失败"
```

脚本结束后无需手动断开：空闲 60 秒（`idle_disconnect_secs`）且无人投屏时，
服务端自动断开 scrcpy 会话进入低功耗，adb 链路保留待命。
