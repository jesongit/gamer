# YAML 自动化脚本语法

本文件是 GameBot YAML 自动化脚本的完整语法说明。模板图片放在 `data/templates/`，
脚本中引用模板名时只需写文件名（如 `shop.png`）。

## 脚本结构

```yaml
name: 每日签到          # 脚本名（可选）
steps:                  # 必填，按顺序执行的动作列表
  - wait: 1000
  - log: "开始"
```

每个 `steps` 列表项是一个动作。动作可以只有一行，也可以带参数和子分支。

## 书写约定

- 每个动作以 `- 动作名:` 开始
- 动作的参数与动作键同级缩进（即都缩在 `-` 下面）
- `then` / `else` 与动作键同级，是当前步骤的分支
- `find` / `click` / `until` 只使用字符串模板写法，不兼容对象写法：

```yaml
- find: sign_btn.png
  threshold: 0.85
  region: u
  then:
    - tap: [0.500, 0.500]
  else:
    - log: "未找到"
```

## 通用参数

### 找图参数（find / click / until）

| 参数 | 默认值 | 说明 |
|---|---|---|
| `threshold` | `0.8` | 匹配阈值 0~1，越大越严格 |
| `region` | `a` | 搜索区域，见下表 |
| `timeout` | 仅 `until` 支持 | 超时毫秒；`0` 表示死等 |

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
- click: shop.png
  region: [0.250, 0.250, 0.750, 0.750]   # 中间区域
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
    from: [0.500, 0.800]
    to: [0.500, 0.200]
    time: 1000          # 1000ms 内完成滑动
```

### 7. click —— 点击模板（自带成功/失败日志）

`click` 只用于“找到模板并点击它”，没有 `timeout`，每次只尝试一次；
`else` 表示模板没找到。

```yaml
- click: shop.png
  threshold: 0.85       # 可选，默认 0.8
  region: a             # 可选，默认 a（全屏）
  log: "点击商店成功"     # 可选，覆盖默认成功日志
  else:
    - log: "没有找到商店按钮"
```

### 8. find —— 找图条件分支

`find` 只判断一次模板是否出现，没有 `timeout`；
`then` 在找到时执行，`else` 在没找到时执行。

```yaml
- find: sign_btn.png
  threshold: 0.85
  region: u            # 只在上半屏找
  then:
    - tap: [0.500, 0.500]
  else:
    - log: "未找到签到按钮"
    - goto: retry
```

### 9. until —— 等待模板出现

`until` 会一直等待模板出现，适合等加载完成、等动画结束。
`timeout` 只在这里支持，默认 `0` 表示不超时（死等）；超时后执行 `else`。

```yaml
- until: done.png
  timeout: 0            # 0 = 死等，默认就是 0
  threshold: 0.85
  region: a
  else:                 # 超时触发
    - log: "等待超时"
```

有限超时：

```yaml
- until: loading_done.png
  timeout: 30000        # 30 秒后仍未出现则走 else
  else:
    - log: "加载超时"
    - goto: fail
```

开始等待时引擎会默认打印日志：`等待模板 xxx 出现，超时 ...`。

### 10. loop —— 次数循环

```yaml
- loop:
    times: 3
    steps:
      - tap: [0.500, 0.800]
      - wait: 500
```

### 11. goto / label —— 跳转

```yaml
- label: retry
- goto: retry
```

### 12. call —— 调用子脚本

```yaml
- call: 子脚本.yml
```

## 完整示例

```yaml
name: 每日签到
steps:
  - until: main_page.png
    timeout: 30000
    else:
      - log: "进入主界面超时"
      - goto: fail

  - click: sign_btn.png
    threshold: 0.85
    region: u
    log: "点击签到按钮"
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
      from: [0.500, 0.800]
      to: [0.500, 0.200]
      time: 800
  - wait: [500, 1000]
  - key: HOME
  - label: end
  - log: "脚本完成"
```
