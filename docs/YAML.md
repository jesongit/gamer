# YAML 自动化脚本语法

GameBot 脚本为 YAML 步骤列表，语法以服务端引擎（`server/src/engine.rs`）为准（2026-08-26 语法精简重写，不兼容旧语法——旧写法引擎与前端校验均显式报错引导迁移）。
模板按应用分区存放于 `data/<应用包名>/tmpl/`，脚本中写文件名即可（如 `shop.png`）。

## 脚本结构

顶层键只允许 `config` / `func` / `steps`（未知顶层键报错）；`steps` 可缺省——**纯函数库脚本**（只有 `func`）供其他脚本通过「脚本名:函数名」跨文件调用：

```yaml
config:                 # 可选：覆盖 config.toml 默认值（也可写成映射列表按序覆盖）
  interval: 500ms       # 轮询类间隔（find 每轮重试 / verify 复查）
  threshold: 0.85       # 模板匹配阈值
  log_level: info       # debug / info（默认）/ warn / error，低于等级的日志丢弃
func:                   # 可选：自定义函数定义（见「自定义函数」）
  - wait_tpl:
    - find: $1
    - return: false
steps:                  # 可选（纯函数库可省略）：按顺序执行的动作列表
  - wait: 1s
  - log: "开始"
```

**单段脚本可省略段落键直接写内容**（按内容形态判定，`config` 不能省略——其子键不是函数名，无法判定归属）：

```yaml
# 顶层序列 = 省略 steps:（等价 steps: 包住整个列表）
- find: sign_btn.png
- log: "签到完成"

# 顶层映射（不含 config/func/steps 任何键）= 省略 func: 的纯函数库简写
wait_tpl:
  - find: $1
  - return: false
```

- 动作参数与动作键同级缩进；无参动作可省略冒号：`- str_app` ≡ `- str_app:`
- 一个步骤只能有一个动作键
- 所有坐标均为相对坐标 0~1，与设备分辨率解耦
- **时间参数一律强制带单位**：`1ms / 2s / 1m / 30min / 1h / 1d`（m ≡ min，可小数如 `1.5s`），裸数字报错
- `interval` 只作用于轮询类等待（find 每轮重试、verify 复查）；**步骤之间不再统一等待**（旧 `action_wait` / 步骤级 `wait` 参数已删除）

## find —— 等模板出现并点击

```yaml
- find: sign_btn.png    # 主模板（单个字符串；多目标拆成多步，挡路的写 block）
  timeout: 30min        # 超时执行 else（默认 30min，必须 > 0）
  block:                # 障碍模板（旧名 check）：主模板未命中后依序匹配，
    - pop.png           # 命中即点击其中心并结束本轮；单个可写 block: pop.png
    - ad.png            # （逗号分隔或列表均可）
  verify: false         # 默认 false；true = 命中点击后等 interval 重匹配主模板，
                        # 仍命中再补一击（共两击，不循环、不判超时）
  then:                 # 命中执行
    - log: "找到并点击"
  else:                 # 超时执行
    - log: "等待超时"
```

每轮流程：

```
主模板（新截图）命中 → 点击中心 → verify（若 true）→ then → 结束
主模板未命中 → block 依序匹配 → 命中：点击其中心、结束本轮
                              → 全未命中：等 interval 重开一轮
超时（timeout）→ else
```

- 所有模板（主模板与 block）命中都**点击模板中心**，无"只匹配不点击"
- 截图瞬态失败（会话刚建立首帧未到、无线链路抖动）**不整脚本夭折**：warn 后跳过本轮等 interval 重试，持续失败超过 20s 宽限才带因中止
- `^1` = 主模板名、`^2..` = block 名（书写顺序），在 then/else 子树内可引用（见「^N 上下文引用」）
- 匹配阈值全局配置（`config:` 段或 config.toml `threshold`），无步骤级参数
- 搜索区域由**模板名 `#` 后缀**决定（见下节）；模板无 `#` 后缀时回退全屏搜索（即 `#a` 语义），运行日志会记一条提醒（每次运行每模板一条）

### 搜索区域：模板名 # 后缀

- `hp#l.png` / `xx#dr.png` —— 半区码：`a`=全屏（默认回退值）、`u/d/l/r`=上下左右半、`ul/ur/dl/dr`=四分之一
- `xx#0_0_500_500.png` —— 相对坐标 ×1000（`0.0,0.0 ~ 0.5,0.5` 即左上四分之一），框选生成区域模板的自动命名格式
- **短名引用**：脚本写 `login.png`，精确文件不存在时引擎在同扩展名文件中唯一匹配 `login#*.png`（区域后缀照常生效）；多个候选报错要求写全名

## color —— 找色分支

```yaml
- color: [0.5123, 0.8456]   # 采样相对坐标
  ff8800:                   # 色值键（6 位十六进制，宽容 # / 0x 前缀；容差固定 30/通道）
    - log: "命中颜色"       # 命中执行的步骤（写在色值键正下方，可留空）
  ff8811:
  else:                     # 全部未命中执行
    - log: "都没命中"
```

- 一次截图按序判定，命中一个执行其步骤并结束本步；全部未命中走 `else`（截图瞬态失败自动重试最多 3 次，仍失败才中止）
- 不轮询、无超时（要重试套 `loop`）
- `^1` = `"[x, y]"` 坐标串、`^2..` = 色值键（书写顺序）
- 前端二次裁切区 Alt/alt 模式点击任意处 → 自动生成 color 记录（所见即所得取色）

## ^N 上下文引用

find 的 then/else、color 的命中步骤/else 子树内，`^N` 引用当前步骤的上下文（find：`^1` 主模板、`^2..` block；color：`^1` 坐标串、`^2..` 色值）：

```yaml
- find: menu.png
  block: ad.png
  then:
    - wait_tpl: ^1          # 把主模板名传给函数
    - call: 处理.yml ^2     # 把障碍模板名传给子脚本
  else:
    - log: "没等到 ^1"
```

- 嵌套 find/color 的内层绑定自然覆盖外层（每步执行时按最内层绑定替换）
- `^` 不是 YAML 保留字符，裸写合法（`&` 是锚点保留字符——值会变 null，故弃用）

## 自定义函数 func

```yaml
func:
  - wait_tpl:               # 函数名不能是保留字（动作键 / then / else / steps…）
    - find: $1
      timeout: 6s
    - return: true          # return 仅函数内合法：true / false，立即返回；
                            # 函数体执行完未 return 视为返回 true
  - need_gate:              # 带执行条件（cond）的函数
    cond: gate.png          # 可选：必须匹配该模板才执行函数体，否则函数返回 false
    steps:                  # 函数体（与 cond 同为函数定义兄弟键，用 steps 键包住）
      - find: $1
        timeout: 6s

steps:
  - wait_tpl: sign_btn.png [0.5, 0.6] ff8800   # 调用：空格分隔实参 + then / else
    then:
      - log: "出现了"
    else:
      - log: "没等到"
  - need_gate: act.png
```

- 实参空格分隔、**括号感知切分**：`[x, y]` 内部的空格不算分隔符；无参写 `- wait_tpl:` 或 `- wait_tpl`
- 函数体内 `$1`/`$2`… 指函数实参（`func:` 段不参与脚本级 `$N` 替换）
- 函数体可用全部动作（含 call/throw/嵌套函数调用，嵌套上限 32 层）
- `return: true` → 执行 then；`false` → 执行 else；**函数体正常走完未写 return → 默认返回 true**（2026-08-27 改，旧语义为 false）
- `cond`（可选）：执行条件模板，支持单个（`cond: test.png`）、逗号分隔（`cond: a.png, b.png`）或列表：

```yaml
func:
  - fun1:
    cond:                 # 多模板：每个模板各取一张新截图匹配一次
      - test1.png         # （不点击），全部命中才执行函数体；
      - test2.png         # 任一未命中 → 函数返回 false（不执行函数体）
    steps:
      - find: $1
```

  - 兼容两种写法：`cond:` + `steps:` 兄弟键（如上，推荐），或 cond 写在函数体之后（`- fun1:` 下先步骤列表、最后 `cond: test.png`）
  - 注意 YAML 规则：**cond 后不能直接跟同列 `- ` 步骤行**（bad indentation），必须用 `steps:` 键包住函数体

### 跨文件函数调用

子脚本里的函数可直接调用（脚本名与 call 同规则解析：优先同分区、跨分区兜底、缺扩展名自动补全）；**纯函数库脚本**（只定义 func、没有 steps）同样作为调用对象，函数定义也可用省略 `func:` 的顶层映射简写：

```yaml
# test1.yaml
func:
  - fun1:
    - find: $1
    - find: $2
  - fun2:
    - log xxx

# test2.yaml
steps:
  - test1:fun1: test.png test2.png   # 调用 test1.yaml 的 fun1，实参同本地函数
  - test1:fun2                       # 无参调用
```

- 函数体/cond 取自被引用脚本的 func 段；体内 `$N` 由调用点实参替换
- 函数体执行期间被引用脚本的函数可见（体内裸函数名按被引用脚本解析），调用者函数兜底
- 返回语义与本地函数一致（return / fall-through 默认 true）；then/else、嵌套、上限 32 层同样适用
- 带 then/else 的无参调用记得写冒号：`- test1:fun2:`（否则被解析成标量字符串步骤）

## loop —— 循环

```yaml
- loop:
  times: 3              # 省略或 0 = 无限循环
  steps:
    - log: "每一轮"
```

`times`/`steps` 两种缩进均可：与 `loop` 同级（如上，YAML 会解析成步骤兄弟键）或缩进到 `loop` 值内（`- loop:\n    times: 3`）。

## call —— 调用子脚本（可传参）

```yaml
- call: 子脚本.yml
- call: 通用日常.yml act_136.png          # 空格分隔实参（括号感知，[x, y] 不切分）
- call: 处理.yml a.png [0.5, 0.6] ff8800
```

- 子脚本按名解析：优先同分区，其次跨分区（缺扩展名自动补全）
- 子脚本内 `$1`/`$2`… 引用实参（替换作用于子脚本 config/steps 全部字符串，嵌套 call 转发 `$N` 同样生效；`func:` 段除外）
- 含 `$N` 的脚本被直接运行（未传参）→ 启动即报错
- YAML 裸标量 `@` 开头是保留字符非法，参数引用必须用 `$`（不能用 `@1`）

## throw —— 结束任务

```yaml
- throw                  # 打印"结束运行脚本"，立即结束整个任务
- throw: 体力不足        # 打印"因 体力不足 结束运行脚本"
```

跨 call 子脚本同样结束整个任务（原 `exit` 改名）。

## 动作清单

### wait

```yaml
- wait: 2s               # 固定等待
- wait: [1s, 3s]         # 随机区间
```

等待分片进行（200ms 一片），运行中点停止最多 ~200ms 内生效，长 wait 不会卡住「停止中」。

### log

```yaml
- log: 输出文本
```

### key

```yaml
- key: HOME              # HOME/BACK/APP_SWITCH(RECENTS)/MENU/VOL_*/POWER/ENTER/DEL/TAB/SPACE/ESC/…/数字，或原始 keycode
```

### text

```yaml
- text: "hello world"
```

### tap

```yaml
- tap: [0.500, 0.500]
```

### swipe

```yaml
- swipe:
    fm: [0.500, 0.800]   # 起点
    to: [0.500, 0.200]   # 终点
    time: 800ms          # 时长（省略默认 500ms；书写必须带单位）
```

### str_app / cls_app

```yaml
- str_app                # 冷启动应用（scrcpy 控制消息，虚拟屏模式自动进虚拟屏）；
                         # 只支持裸写，包名 = 设备分区（设备配置 pkg）
- cls_app                # 关闭应用（adb am force-stop，不碰会话/投屏；幂等）
```

## 已删除的旧写法（显式报错引导迁移）

| 旧写法 | 迁移目标 |
|---|---|
| `until` | `find`（障碍模板 `check` → `block`） |
| `cond` | 颜色条件 → `color`；模板条件 → `find`（短 timeout）+ then/else 或 func 封装（func 新增 `cond` 函数级执行条件，2026-08-27） |
| `exit` | `throw` |
| `goto` / `label` | `loop` |
| `count` / `cnt_ivl` / `cnt_chk` / `img_ivl` / `and_or` / `click` / `before` / `after` | 已删除（find 语义内置：命中恒点中心、每轮统一 interval） |
| 步骤级 `threshold` / `region` 参数 | `config:` 段配置阈值；区域用模板名 `#` 后缀 |
| 步骤级 `wait` 参数 / 顶层 `action_wait` | 已删除：步骤间不等待，轮询间隔用 `config: interval` |
| 顶层 `log_level` / `name` | `config: log_level`；name 删除（脚本名即文件名） |
| 顶层 `package <名字>` 指令 | 已删除（分区 = 设备配置的 pkg） |

## 完整示例

```yaml
config:
  interval: 500ms
  log_level: info

func:
  - login_ok:            # 等登录页出现并点掉（障碍弹窗自动关闭）
    - find: login.png
      block: act_cls.png
      timeout: 60s
    - return: true

steps:
  - str_app
  - login_ok:
    then:
      - log: "已进登录页"
    else:
      - throw: 启动超时
  - find: act_swt.png
    block: swt_etr.png
  - find: tili_use.png
    timeout: 2s
  - find: close.png
    verify: true         # 点完等 interval 复查，仍命中补一击
  - loop:
    times: 3
    steps:
      - find: cnt_add.png
        timeout: 5s
  - color: [0.7625, 0.9130]
    c74f36:
      - log: "体力确认按钮亮起"
    else:
      - log: "未亮起"
  - call: 日常遗器.yml
  - cls_app
  - log: "日常完成"
```

---

附：空闲自动断开 `idle_power_secs`（config.toml，默认 300s，0=关）——无 viewer 且无脚本运行持续 N 秒后拆会话进低功耗，下次运行自动重连。
