# YAML 自动化脚本语法

本文件是 GameBot YAML 自动化脚本的完整语法说明。模板图片放在 `data/templates/`，
脚本中引用模板名时只需写文件名（如 `shop.png`）。

## 脚本结构

```yaml
name: 每日签到          # 脚本名（可选）
action_wait: 500        # 操作间隔：每个操作执行后的默认等待毫秒数（可选，默认 500）
steps:                  # 必填，按顺序执行的动作列表
  - wait: 1000
  - log: "开始"
```

- `action_wait` 是当前脚本的操作间隔：每个操作（除 `wait` 动作本身）执行完后
  默认等待这么多毫秒。单个步骤可用 `wait` 参数覆盖（如 `wait: 200`，`0` 不等待）。
- 每个 `steps` 列表项是一个动作。动作可以只有一行，也可以带参数和子分支。

## 书写约定

- 每个动作以 `- 动作名:` 开始
- 动作的参数与动作键同级缩进（即都缩在 `-` 下面）
- `then` / `else` 与动作键同级，是当前步骤的分支
- `find` 只使用字符串模板写法，不兼容对象写法：

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

## 通用参数

### 找图参数（find）

| 参数 | 默认值 | 说明 |
|---|---|---|
| `interval` | `500` | 检测间隔毫秒：没找到时多久重试一次 |
| `timeout` | `6000` | 超时毫秒；`0` 表示一直找（不超时） |
| `click` | `false` | 找到后如何点击，见下文 |
| `threshold` | `0.8` | 匹配阈值 0~1，越大越严格 |
| `region` | `a` | 搜索区域，见下表 |

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

数组写法 `[x1, y1, x2, y2]`：`(x1, y1)` 是左上角相对坐标，`(x2, y2)` 是右下角相对坐标，均为 `0~1` 百分比。

```yaml
- find: sign_btn.png
  region: [0.000, 0.000, 0.500, 0.500]   # 左上四分之一
- find: shop.png
  region: [0.250, 0.250, 0.750, 0.750]   # 中间区域
```

### find 的 click 参数

`click` 控制找到模板后是否点击、点哪里，默认 `false`（只找不点）：

| 取值 | 行为 |
|---|---|
| `false` / 不写 | 只判断模板是否出现，不点击 |
| `true` | 点击模板中心点 |
| `button.png`（模板名） | 在找到的模板**区域内**再找 `button.png`，找到就点击它的中心点；区域内没找到则继续循环查找，直到超时走 `else` |
| `[x1, y1]` | 点击模板区域内的相对坐标（`0~1`），如 `[0.5, 0.5]` 是模板中心点 |

```yaml
- find: dialog.png          # 对话框出现后，点它内部的关闭按钮
  click: close_btn.png      # 在 dialog.png 区域内找 close_btn.png 并点击
  else:
    - log: "对话框没出现"

- find: dialog.png
  click: [0.5, 0.1]         # 点击对话框区域内顶部中间的位置
  timeout: 0                # 一直等对话框出现
```

### 相对坐标

`tap` 和 `swipe` 使用相对坐标百分比，`0~1` 之间的小数，一般保留三位小数：

- `0.500` 表示水平/垂直方向的 50%
- 与设备分辨率解耦，同一套脚本可通用于 1920x1080、1080x1920 等分辨率

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
- key: HOME            # HOME/BACK/APP_SWITCH/VOL_UP/VOL_DOWN/POWER/ENTER...
```

### 4. text —— 输入文本

```yaml
- text: "hello world"
```

### 5. tap —— 按相对坐标点击

```yaml
- tap: [0.500, 0.500]   # 点击屏幕中心
- tap: [0.125, 0.900]   # 点击左下角附近
```

### 6. swipe —— 滑动

```yaml
- swipe:
    fm: [0.500, 0.800]
    to: [0.500, 0.200]
    time: 1000          # 1000ms 内完成滑动
```

### 7. find —— 查找模板（找图/点击/等待统一入口）

`find` 每 `interval` 毫秒检测一次模板是否出现，直到出现或 `timeout` 超时：

- 找到 → 按 `click` 参数点击（如有）→ 执行 `then`
- 超时未找到 → 执行 `else`

```yaml
- find: shop.png
  interval: 500         # 检测间隔 ms（默认 500）
  timeout: 6000         # 超时 ms（默认 6000，0 = 一直找）
  click: true           # 找到后点击模板中心（默认 false）
  threshold: 0.85       # 匹配阈值（默认 0.8）
  region: a             # 搜索区域（默认 a）
  then:                 # 找到并点击成功后执行
    - log: "点击商店成功"
  else:                 # 超时未找到执行
    - log: "没有找到商店按钮"
    - goto: retry
```

等待模板出现（相当于旧的 `until`，不点击）：

```yaml
- find: loading_done.png
  timeout: 30000        # 30 秒后仍未出现则走 else
  interval: 500
  else:
    - log: "加载超时"
    - goto: fail
```

一直等到出现（`timeout: 0`）：

```yaml
- find: done.png
  timeout: 0            # 0 = 一直找，不超时
```

开始查找时引擎会默认打印日志：`查找模板 xxx，超时 ...，检测间隔 ...ms`。

### 8. loop —— 次数循环

```yaml
- loop:
    times: 3
    steps:
      - tap: [0.500, 0.800]
      - wait: 500
```

### 9. goto / label —— 跳转

```yaml
- label: retry
- goto: retry
```

### 10. call —— 调用子脚本

```yaml
- call: 子脚本.yml
```

## 完整示例

```yaml
name: 每日签到
action_wait: 500

steps:
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

  - find: signed_done.png
    then:
      - log: "签到成功"
      - goto: end
    else:
      - tap: [0.500, 0.500]
      - wait: [300, 800]

  - swipe:
      fm: [0.500, 0.800]
      to: [0.500, 0.200]
      time: 800
  - wait: [500, 1000]
  - key: HOME
  - label: end
  - log: "脚本完成"
```
