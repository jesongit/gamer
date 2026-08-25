# YAML 自动化脚本语法

GameBot 脚本为 YAML 列表式步骤，语法以服务端引擎（`server/src/engine.rs`）为准。
模板按应用分区存放于 `data/<应用包名>/tmpl/`，脚本中写文件名即可（如 `shop.png`）。

## 脚本结构

```yaml
name: 每日签到          # 脚本名（可选，仅标识）
action_wait: 500        # 每个操作后的默认等待 ms（默认 500；str_app 为 3000）
log_level: info         # info=精简日志（默认） / debug=详细
steps:                  # 必填，按顺序执行的动作列表
  - wait: 1000
  - log: "开始"
```

- 动作参数与动作键同级缩进；无参动作可省略冒号：`- str_app` ≡ `- str_app:`
- 每个步骤可用 `wait` 参数覆盖操作后等待（如 `wait: 200`，`0` 不等待）
- 所有坐标均为相对坐标 0~1，与设备分辨率解耦

## 找图 until —— 等模板出现并点击

唯一找图动作：超时时间内循环匹配，命中即点击模板中心并执行 `then`；超时执行 `else`。

```yaml
- until: sign_btn.png       # 主模板（必填，单个模板名）
  before: [ad.png, pop.png] # 障碍模板（可选）：单个 / 逗号分隔 / 列表
  interval: 500             # 一轮全未命中后的重开间隔（默认 500，必须 > 0）
  img_ivl: 50               # 一轮内相邻两次匹配的间隔（默认 50）
  timeout: 30min            # 超时（默认 30 分钟，必须 > 0）
  threshold: 0.85           # 匹配阈值 0~1（默认取设备配置 default_threshold，=0.8）
  region: u                 # 搜索区域（默认 a=全屏）
  count: 3                  # 连击：总点击次数（含首击），默认 1 单击；命中后按首击坐标无条件连点
  cnt_ivl: 50               # 连击相邻点击间隔（默认 50）
  then:                     # 命中主模板并点击后执行
    - log: "点击成功"
  else:                     # 超时未命中执行
    - log: "没找到"
```

**每轮匹配顺序**：依序匹配 `before` 全部（命中即点击关闭；未命中等 `img_ivl`
匹配下一个；无论命中与否都不结束本轮）→ 匹配主模板（命中即点击中心 →
`then` → 结束步骤）→ 全未命中隔 `interval` 从 before 重开一轮。

要点：

- **命中恒点击模板中心**——没有 `click: false`；只想"判断出现"用 `color`
- `count` 为总点击次数（含首击），上限 100000；命中后按首击坐标**无条件连点**
  （每两次点击隔 `cnt_ivl`，默认 50ms）——不再重新匹配（cnt_chk 已删除）；
  对 before 障碍同样生效
- `before` 与主模板重复会报错；多模板目标请拆成多步，挡路的写 `before`
- 时长参数（timeout/interval/img_ivl/cnt_ivl）写法统一：
  纯数字 `500`（ms）或带单位 `1ms` / `1s` / `30min` / `1h` / `1d`
  （大小写不敏感、可小数 `1.5s`、`ms` 先于 `s` 判定；timeout、interval 必须 > 0）

### 搜索区域 region

| 值 | 含义 |
|---|---|
| `a` | 全屏（默认） |
| `u` / `d` / `l` / `r` | 上 / 下 / 左 / 右半屏 |
| `ul` / `ur` / `dl` / `dr` | 四个四分之一区 |

也支持相对坐标数组 `[x1, y1, x2, y2]` 或对象 `{fm: [x, y], to: [x, y]}`（0~1，
需 x2 > x1、y2 > y1）。显式 `region` 统一作用于本步骤全部模板。

**模板名自带区域后缀**（未显式写 `region` 时按各自后缀匹配，写在扩展名前，大小写不敏感）：

- `xx#l` / `xx#dr`：半区码（同上表）
- `xx#0_0_500_500`：相对坐标 ×1000 的整数（`500` → 0.500）——框选生成的区域模板就是这种命名
- 优先级：显式 `region` > 模板名后缀 > 全屏；后缀解析不出区域则全屏匹配不报错

**短名引用**：写去后缀短名即可引用唯一匹配的带后缀文件，区域照常生效；
同基名多个后缀文件时需写全名消歧。

```yaml
- until: hp#l.png          # hp 只搜左半屏
- until: login.png         # 短名 → 自动解析到 login#910_159_972_716.png
```

### 旧写法迁移（写了会显式报错）

| 旧写法 | 新写法 |
|---|---|
| `- find: a.png` | `- until: a.png` + `timeout: 6s`（默认 30min） |
| `- click: a.png`（无 until） | `- until: a.png` |
| `- click: a.png` + `check: b.png` | `- until: a.png` + `before: b.png` |
| `- check: b.png`（无 color） | `- until: a.png` + `before: b.png` |
| `- until: a.png, b.png`（多主模板） | 拆成多步；挡路的写 `before` |
| until 的 `and_or` / `click` 参数 | 删除（命中恒点击模板中心） |
| until 的 `cnt_chk` 参数 | 删除（命中后按首击坐标无条件连点，想防误点拆成多步 until） |
| then 里写「模板名: 步骤列表」分支 | 已删除，该步骤会被静默跳过 |

## 取点比色 color —— 多检查点颜色判断，不点击

`color` 超时时间内逐轮检测若干检查点像素，任一命中走 `then`、超时走 `else`。
参数全部是兄弟键（2 空格缩进，与 `until` 同构），只有 `then`/`else` 的列表项再 +2：

```yaml
- color:                 # 动作键，值留空（- color 无参简写等价）
  timeout: 5min          # 超时（默认 5min，必须 > 0）
  interval: 500ms        # 检测间隔（默认 500ms，每轮取最新截图）
  check:                 # 检查点列表（必填，至少一项）：任一命中即触发 then
    - [0.5123, 0.8456]: ff8800   # 该点像素是否为 ff8800
    - [0.3000, 0.2000]: ff8899   # 多个检查点可写多行
  then:
    - log: "颜色命中"
  else:
    - log: "没恢复"
```

- 色值为 **6 位十六进制 RRGGBB，不带 `#`**——YAML 里 `#` 开头是注释会被吞；
  宽容接受 `"#ff8800"` / `[255, 136, 0]` / `0x` 前缀
- 每通道容差固定 30：H.264 有损压缩帧间像素会抖动，精确匹配不可用
- 每轮 = 新截图 → 依序检查全部检查点；全部未命中隔 `interval` 重来，累计超 `timeout` 走 `else`
- `color` 不点击；需要点击就在 `then` 里写 `until` / `tap`
- 二次裁切区 Alt/alt 模式点击任意处会自动生成 color 记录（所见即所得取色）

## exit —— 结束脚本运行

```yaml
- exit            # 无参数：打印"结束运行脚本"并立即结束
- exit: 体力不足  # 带参数：打印"因 体力不足 结束运行脚本"并立即结束
```

- 立即结束整个脚本运行（call 子脚本内 `exit` 同样结束整个任务）
- 用于"条件不满足提前收工"（如颜色检测 after-else 里直接 exit）

## 动作清单

### wait —— 等待 / 随机延时

```yaml
- wait: 1000            # 固定等待 1000ms
- wait: [500, 1500]     # 随机等待 500~1500ms
- wait: {min: 300, max: 900}   # 等价写法
```

### log —— 输出日志

```yaml
- log: "任务完成"
```

### key —— 按键

```yaml
- key: HOME
```

常用：`HOME` / `BACK` / `APP_SWITCH`(或 `RECENTS`) / `MENU` / `VOL_UP` /
`VOL_DOWN` / `POWER` / `ENTER` / `DEL`(或 `BACKSPACE`) / `TAB` / `SPACE` /
`ESC` / `SEARCH` / `CAMERA` / `FOCUS` / `NOTIFICATION` / `SETTINGS` / `MUTE` /
`HEADSETHOOK` / `WAKEUP` / `SLEEP` / 数字键 `0`~`9`，也支持直接写 keycode 数字。

### text —— 输入文本

```yaml
- text: "hello world"
```

### tap —— 点击

```yaml
- tap: [0.500, 0.500]   # 相对坐标（0~1）
- tap: {x: 0.5, y: 0.5} # 等价对象写法
```

### swipe —— 滑动

```yaml
- swipe:
    fm: [0.500, 0.800]   # 起点（旧写法 from 兼容）
    to: [0.500, 0.200]   # 终点
    time: 1000           # 时长 ms（默认 500）
```

### str_app —— 冷启动应用

先 force-stop 再启动，保证进入干净状态；虚拟屏设备自动启动到虚拟屏。
应用启动要 1~3 秒，`str_app` 后的默认等待是 **3000ms**。

```yaml
- str_app: com.x.y        # 包名省略 → 用设备配置的应用包名
```

### cls_app —— 关闭应用

`adb force-stop`，不碰会话/投屏；幂等，适合放脚本开头确保冷状态。
虚拟屏上应用被杀后画面变桌面/黑屏，流不断，属预期。

```yaml
- cls_app: com.x.y
```

### loop —— 次数循环

```yaml
- loop:
    times: 3
    steps:
      - tap: [0.500, 0.800]
```

### goto / label —— 跳转

`label` 不执行操作，只作跳转目标（goto 只支持向后跳转）。

```yaml
- label: retry
- goto: retry
```

### call —— 调用子脚本

子脚本从 `steps` 开头执行，日志合并到当前脚本；不存在时报错。

```yaml
- call: 子脚本.yml
```

### exit —— 结束脚本运行

见上文「exit —— 结束脚本运行」。

## 完整示例

```yaml
name: 每日签到
action_wait: 500
log_level: info

steps:
  - str_app: com.game.example   # 冷启动游戏
  - until: main_page.png        # 等主界面出现并点击
    timeout: 30s
    else:
      - log: "进入主界面超时"
      - goto: fail

  - until: sign_btn.png         # 等签到按钮出现并点击
    threshold: 0.85
    region: u
    then:
      - log: "点击签到按钮"
    else:
      - log: "签到按钮不存在"

  - until: dialog.png           # 先点掉弹窗，再等签到完成
    before: [act_cls.png, ad.png]
    timeout: 60s
    then:
      - log: "签到完成"
      - goto: end

  - label: end
  - cls_app: com.game.example

  - label: fail
  - log: "任务失败"
```

脚本结束后无需手动断开：空闲 `idle_power_secs`（config.toml，默认 300，0 关闭）
且无人投屏时，服务端自动断开会话进低功耗，adb 链路保留待命。
