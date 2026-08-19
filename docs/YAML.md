# YAML 自动化脚本语法

本文件是 GameBot YAML 自动化脚本的完整语法说明，以服务端引擎
（`server/src/engine.rs`）实际实现为准。模板图片放在 `data/templates/`，
脚本中引用模板名时只需写文件名（如 `shop.png`）。

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
- `then` / `else` 与动作键同级，是当前步骤的分支
- `find` / `until` 只使用字符串模板写法，不兼容对象写法：

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

`find` 每 `interval` 毫秒检测一次模板是否出现，直到出现或 `timeout` 超时；
`until` 则一直检测到出现为止（等价于旧写法 `find` + `timeout: 0`）。

| 参数 | 默认值 | 说明 |
|---|---|---|
| `interval` | `500` | 检测间隔毫秒：没找到时多久重试一次 |
| `timeout` | `6000` | 超时毫秒，**必须大于 0**（`find` 专用；一直找请用 `until`） |
| `click` | `true` | 找到后如何点击，见下文 |
| `threshold` | `0.8` | 匹配阈值 0~1，越大越严格（默认取设备配置 `default_threshold`） |
| `region` | `a` | 搜索区域，见下表 |

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

`find` 每 `interval` 毫秒检测一次模板是否出现，直到出现或 `timeout` 超时：

- 找到 → 按 `click` 参数点击（如有）→ 执行 `then`
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

开始查找时引擎会打印日志：`查找模板 xxx，超时 ...，检测间隔 ...ms`。

### 8. until —— 一直等到模板出现（等价旧 `find` + `timeout: 0`）

`until` 与 `find` 参数完全一致（`interval` / `click` / `threshold` / `region` /
`then`），只是**永不超时**——一直循环检测直到模板出现：

```yaml
- until: loading_done.png   # 一直等到加载完成（不点击）
  click: false

- until: done.png           # 等到出现并点击模板中心（click 默认 true）
  interval: 500
  then:
    - log: "完成了"
```

- 永不超时，所以 `else` 分支永远不会执行（写了也不报错，属冗余）
- 注意与 `find` 的区别：`find` 有超时、可走 `else` 分支做失败处理

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
