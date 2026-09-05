# YAML 脚本语法（v3 唯一正式方案）

GameBot 自动化脚本只支持 **YAML v3**（`version: 3`，ADR-YAML-01：唯一正式方案）。
非 3 的版本声明（含缺失与历史 v2 形态）在保存 / 描述 / 运行三条路径统一报
`unsupported yaml version`（`yaml.v3.version` / `yaml.v3.version.missing`），
**无兼容分支、无 fallback、无迁移工具**。

- 专题文档套件：`docs/yaml-v3/`（overview / program / params / steps / call /
  expressions / vision / timing / runtime / examples，逐主题给出全部语法与被移除
  v2 语法的迁移对照）；
- 权威裁决：`docs/reference/adr/ADR-YAML-01~04`；
- 实现：`server/src/extensions/gamer_yaml/yaml_vnext.rs`（纯数据前端：Surface
  YAML → small AST）+ `yaml_extension.rs` / WASM guest 解释器；前端可视化编辑器
  （`web/src/script-editor/`）与 Runtime 共用同一 v3 surface DSL。

## 1. 目录与资源边界

脚本、函数库、模板按**应用分区**（设备配置的 pkg，即应用包名）存放，目录即类型：

```
data/<pkg>/
├── scripts/     # 可运行脚本（.yaml/.yml，顶层必须有 steps）
├── functions/   # 函数库（严格 .yaml，顶层键全是函数名）
├── templates/   # 模板图片（默认 8-bit 灰度 PNG，文件名可带 # 搜索区/#1 颜色后缀）
├── keymaps/     # 按键映射方案（WASM keymap 扩展 profile 数据源）
├── presets/     # App Package 发布的任务预设
└── resources/   # 包附带的其他资源
```

- **解析优先级**：同名资源按 **EditableLocal（分区目录）→ UserOverride →
  active App Package** 三层解析，高层覆盖低层；本地分区目录即本地编辑区
  （可执行脚本只能位于 `data/<pkg>/scripts/`，函数库 `functions/`，
  模板 `templates/`）。
- **脚本资源 ID** = `<pkg>/<文件名>.yaml`（如 `daily/login.yaml`，可含子目录）。
  含 `/`，前端拼 URL 必须整体 `encodeURIComponent`。
- **函数路径** = `<文件短路径>/<函数名>`（如 `common/login` = `functions/common.yaml`
  里的 `login`；一个函数库文件可定义多个函数）。
- **运行边界**：只有 `scripts/` 下的脚本可手动运行 / 立即运行 / 进入定时任务；
  `functions/` 只能经 `call`（`function:<文件短路径>/<函数名>`）调用或走函数测试
  API，不进脚本列表与任务选择器。
- **不做内容推断**：`scripts/` 里必须声明 `version: 3` 且有顶层 `steps`；
  `functions/` 为 bare-map（顶层键全是函数名，无 version 键）。
  放错目录按该目录的类型校验，报错即拒。
- **跨分区一律不解析、不回退**：模板 / 函数 / 子脚本只在当前应用分区查找，
  没有 default 兜底；其他目录布局不属于当前资源，也不会被读取或迁移。
- **模板引用写短名**（如 `account.png`）。磁盘文件名可带 `#` **搜索区后缀**
  （后缀在扩展名前，如 `xx#l.png`）：
  - 半区码：`a`=全屏、`u`/`d`/`l`/`r`=上/下/左/右半、`ul`/`ur`/`dl`/`dr`=四角；
  - 数字坐标：`xx#x1_y1_x2_y2`，四段各为相对坐标 ×1000 的整数（如
    `xx#0_0_500_500` = 左上 1/4 区域），需 x2>x1、y2>y1。
  - 颜色标记：末尾再加 `#1`（如 `xx#0_0_500_500#1.png`）表示保留颜色，
    并在灰度 NCC 命中后复核颜色；不带 `#1` 的旧格式均按灰度匹配，脚本 YAML 无需颜色参数。
  脚本写 `xx.png` 而磁盘存在 `xx#l.png` 时按「基名 + `#` 后缀 + 同扩展名」唯一
  匹配；零候选报不存在、多候选报歧义（`resource.tmpl.ambiguous`），不猜测。

## 2. YAML v2（历史档案，已删除）

YAML v2（严格 loader / 19 类步骤 AST / 原生执行器 / 双格式兼容入口）已于
Phase 12（P12.9）整体删除，本文旧版 §2–§10 的 v2 语法描述随之退役；规则档案见
`ADR-YAML-01` 与 git 历史。开发者须知：

- 存量 v2 脚本**不可运行也不可再保存**：非 `version: 3` 源在保存 / entrypoint
  描述 / 手动运行 / 定时任务门禁 / call resolver 全部统一报
  `yaml.v3.version`（`version` 缺失报 `yaml.v3.version.missing`），不误诊为
  其他结构错误，也没有任何 fallback 路径；
- 常用迁移对照（详见 `docs/yaml-v3/steps.md` §10）：`func` 步骤 →
  `call` + `function:<文件短路径>/<函数名>`；`match` → `match_first`（候选
  `steps`）；`color` → `invoke: vision.sample_color` + `if`；`str_app` /
  `cls_app` → `app.start` / `app.stop`；脚本级 `config:` → `defaults`
  （vision/timing）；参数声明串（`类型:名:备注[:默认]`）与 psig1 签名 wire
  形态保持不变，等价声明的存量任务签名继续可比对；
- `server/tests/fixtures/script_v2/` golden 夹具与 `phase0_tests.rs` 夹具护栏
  随 v2 一并删除。

## 3. YAML v3 语法契约（唯一正式方案）

> v3 是唯一正式方案（ADR-YAML-01）：脚本必须声明 `version: 3`，非 3 一律报
> `unsupported yaml version`，无 v2 兼容 / 无 fallback / 无迁移工具。本节是 v3
> 语法契约的实现同步；权威裁决见 `docs/reference/adr/ADR-YAML-01~04`，契约原文
> 见 `docs/plans/phase12_v3_dsl_contract.md`。实现在
> `server/src/extensions/gamer_yaml/yaml_vnext.rs`（纯数据前端）+ WASM guest
> 小 AST 解释器；全部 19 类 surface 步骤的逐条语法见 `docs/yaml-v3/steps.md`。

### 3.1 脚本与函数库

- **脚本**（scripts/）顶层只允许 `version / params / defaults / steps`；缺失或非 3 的
  `version` 报 `yaml.v3.version` / `yaml.v3.version.missing`。`params` 为参数
  唯一来源，字符串 / 映射双形态（见 `docs/yaml-v3/params.md`），`remark`（字符串第 3 段 / 映射
  `remark` 键）随声明保留并透出到参数 schema 的 `description`
  （不参与 `psig1` 签名，改备注不触发任务参数过期）。
- **函数库**（functions/）为 bare-map `{<函数名>: {params, steps}}`，**无
  `version` 键**（目录即类型）；函数名由映射键承载（唯一），每个函数记录只允许
  `params / steps`，`steps` 必需；函数名 unicode 字母/数字/`_`（支持中文）、
  不能以数字开头且不得撞动作键/结构键/`$match` 保留字
  （`yaml.v3.function.name`）。结构非法报 `yaml.v3.function.*` 结构化诊断
  （`yaml.v3.function.file` / `.name` / `.unknown_key` / `.not_found`）。
  保存边界只接受 v3 bare-map 单形态（P12.9 起），允许嵌套目录
  （`function:<文件短路径>/<函数名>` 的短路径可含 `/`）。

### 3.1.1 defaults —— vision threshold 与 timing 兜底（契约 §4）

```yaml
version: 3
defaults:                     # 可选
  vision:
    threshold: 0.80           # 模板匹配阈值兜底
  timing:
    after_tap: 300ms          # 每次 tap 后等待（内置 300ms）
    after_match: 200ms        # 匹配命中后等待（内置 200ms）
    poll_interval: 100ms      # find/check 轮询间隔（内置 100ms）
steps:
  - ...
```

- 只允许上述键，未知键 / 非法形态报 `yaml.v3.defaults.unknown_key` /
  `yaml.v3.defaults.type` / `yaml.v3.defaults.range`；timing 值必须是带单位
  时长字面量（`300ms`/`2s`）或非负整数毫秒，不接受 `$var`。
- **threshold 三级优先**：step `threshold` > `defaults.vision.threshold` >
  Runtime 内置 `0.80`；lower 期解析并注入 `vision.match` / `vision.match_many`
  的 invoke args（缺省省略字段，由 Runtime 兜底）。
- **timing 即语义**：tap 后 / 命中后等待与轮询间隔全部由脚本 defaults 显式
  声明（缺省用内置值），lower 期展开为显式 `runtime.sleep`（可被「停止」取消），
  不存在隐藏的全局 interval / judge_delay。
- 函数库无 defaults 块（bare-map 结构），timing / threshold 走内置兜底。

### 3.3 find / match_first / check 与 `$match` 上下文（ADR-YAML-03）

```yaml
- find:
    template: reward
    timeout: 10s          # 可选；缺省 30min（轮询 poll_interval 至命中）
    threshold: 0.90       # 可选 step override（三级优先见 §3.1.1）
    region: {x: 0.1, y: 0.2, width: 0.3, height: 0.4}   # 可选；相对坐标搜索区
    save: reward          # 可选；命中结果固化到命名变量，跨后续步骤可用
    then:                 # 命中后步骤组（唯一键名；体内 `$match` / `$reward` 可用）
      - tap: {point: $reward.center}
    else:                 # 可选；超时后步骤组（缺省抛 FIND_TIMEOUT: <template>）
      - log: 未找到
    verify:               # 可选；then 执行完后在 timeout 内二次验证
      template: home
      timeout: 5s         # 可选；缺省 30min
```

- **命中路径**：save 固化 → sleep(after_match) → `then` → `verify`（若设，
  不命中抛 `VERIFY_FAILED: <template>`，不走 else）。
- **超时路径**：有 `else` 走 `else`；无 `else` 抛 `FIND_TIMEOUT: <template>`。
- **match 结果值**（save 存入 / `$match` 引用）：
  `{found, score, x, y, width, height, center, region}`；坐标为相对值 0~1
  （center 为命中框中心），`region` 回显本次搜索区域。未 save 时 `$match`
  仅在对应 find/match_first 的 then/else/verify/steps 体内可见（块结束复位），
  save 后跨步可用。

```yaml
- match_first:
    candidates:
      - template: reward
        threshold: 0.9      # 可选候选级 threshold（三级优先同上）
        steps:              # 候选命中后执行（唯一键名；体内 `$match` = 该候选结果）
          - tap: {point: $match.center}
      - template: close
        steps:
          - tap: {point: $match.center}
    else: ...               # 全未命中走 else；缺 else 静默继续
```

- match_first 单帧 `vision.match_many`（候选级 threshold 经 `thresholds`
  平行列表传入）、按书写顺序首个命中候选执行自己的 `steps`。
- `- check: {template, timeout?, threshold?, throw?}`：轮询至出现（每轮
  sleep(poll_interval)），命中 sleep(after_match) 后继续；超时按 `throw`
  文案结束运行（缺省「check 未命中」）。
- **wait 双形态**：`- wait: 300ms` 固定；`- wait: {min: 300ms, max: 700ms}`
  随机区间（min/max 必须同给且 min ≤ max，run 级随机 nonce 播种 splitmix64
  取值，经 `runtime.sleep` 等待、可被停止取消）。

**已删除步骤/字段**（给迁移诊断 `yaml.v3.step.removed` / `yaml.v3.field.removed`）：
`retry`（用 loop 表达）、`wait_for`（与 find 同义）、`click_when` /
`find.click` / 候选 `click`（ADR-YAML-03 click 语法全面移除，用 then + 
`tap: {point: $match.center}` 表达）、`color_branch`（用
`invoke: vision.sample_color` + if 表达）、match_first 顶层 `then`
（候选步骤归各自 `steps`）。

### 3.4 v3 surface 步骤集（19 类）

`app.start` / `app.stop` / `tap` / `swipe` / `key` / `text` / `wait` /
`log` / `set` / `if` / `loop` / `break` / `call` / `return` / `throw` /
`find` / `match_first` / `check` / `invoke`——与前端编辑器
（`web/src/script-editor/model.ts` `STEP_KINDS`）一一对应。

### 3.5 call —— 唯一可调用资源入口（ADR-YAML-02）

```yaml
- call:
    target: script:daily/login        # 或 function:工具/月卡领取
    with:                             # 参数名 → 表达式；`args` 为兼容别名
      account: $user
    save: result                      # 可选；返回值整体存入；无 return → null
```

- **命名空间仅 `script:` / `function:`**；裸 target / 未知前缀在解析期报
  `yaml.v3.call.namespace`（错误信息含 target 原文与合法形态示例）。
- `script:<资源id>`：分区内 `scripts/` 相对路径，`.yaml` 后缀可省略
  （`script:daily/login` → `scripts/daily/login.yaml`）。
- `function:<文件短路径>/<函数名>`：文件短路径按**最后一个 `/`** 分割、可含目录
  （`function:common/login/is_logged_in` = `functions/common/login.yaml` 里的
  `is_logged_in`）；拒绝 `..` / 绝对路径 / 反斜杠 / 空段——穿越报
  `yaml.v3.call.target`，路径形态报 `yaml.v3.call.function_path`。
- 函数与脚本只经 Core ResourceStore（composite 三层）解析，本地编辑区与包内
  资源对 `call` 透明；跨分区一律不解析。
- **返回值泛化**：`return` 可返回 null / bool / number / string / object /
  array 任意 JSON 值；`call` 的 `save` 存返回值整体，被调方无 `return` 即存
  null。删除「函数默认返回 bool」约束，`if` 条件按通用值语义判断。
- **递归深度**上限 32 层，超限报 `CALL_DEPTH_EXCEEDED: depth=N max=32`
  （P12.4 起 depth 由 guest 本地 ExecutionBudget 计数，WIT `programs.resolve`
  不再透传 depth、宿主不做深度守卫）。
- **执行预算**（ADR-YAML-04）：`max_steps = 100_000`（逻辑步：顶层、循环体
  每轮每个子步、if 分支体、call 目标程序体全计，循环每轮迭代本身也计）、
  `max_call_depth = 32`，由 guest 解释器本地计数；超限报
  `STEP_BUDGET_EXCEEDED: consumed=N max=100000` / `CALL_DEPTH_EXCEEDED`，
  错误码原样进入运行错误信息与日志。宿主侧 wasmtime epoch interruption
  仅作取消兜底（用户停止可打断 guest 纯计算段），不做超时强杀。

### 3.6 手动运行 start_index（契约 §8）

guest 解释器支持 program 顶层可选 `start_index`：跳过其前的**顶层**步骤
（与 v2「从此运行」语义一致）；嵌套分支 / 循环体不受影响——lower 后的顶层小
AST 步与 surface 步骤 1:1 对应，序号即顶层 surface 步序号。host 由运行请求
（`YamlWasmRunRequest.start_index`）注入，缺省 `None` = 从头执行。

### 3.7 运行可视化事件（P12.6 / ADR-YAML-03 wire 契约）

v3 脚本运行时经 control DataChannel 反向推送运行结构事件（信封
`{"type":"se","ev":...}`；引擎 emit → viewers 注册表 `control_dc`，手动运行与
定时任务同样生效）。事件**不携带帧图像数据**；前端「运行事件 feed」与
ScriptSummary 步骤高亮由这些事件驱动。

**step 身份（path 语法）**：lower 阶段为每个 surface 步骤生成稳定路径挂在
产出的小 AST 步上，语法与前端编辑器寻址一致：

- 顶层：`steps[0]`、`steps[2]`；
- if / find 分支：`steps[0].then[1]`、`steps[0].else[0]`；
- loop 体：`steps[1].steps[3]`；
- match_first：`steps[2].candidates[0].steps[1]`、超时分支 `steps[2].else[0]`。

同一脚本重复运行、编辑无损往返后路径保持稳定。call 进入被调方后帧内事件的
path 仍是该脚本的本地路径（`call_start` 事件宣告帧切换）。

**事件 wire 表**（ev 名 × 载荷 × 发射点）：

| 事件 | 载荷 | 发射点 |
|---|---|---|
| `run_start` | `{}` | 运行进入（guest / 原生解释器） |
| `run_end` | `{ok, error?}` | 运行退出（ok=false 带 error 原文） |
| `step_start` | `{path, desc}` | 进入 surface 步骤（desc = 中文摘要，如 `find 登录按钮`、`tap 0.5,0.3`、`call script:daily/login`、`wait 300ms`） |
| `step_end` | `{path, ok, error?}` | surface 步骤完成 / 失败 |
| `call_start` | `{target, depth}` | 进入 call 目标（depth = 本地调用深度） |
| `vision` | `{template, found, score?, center?}` | 每次模板匹配后（宿主侧 vision 能力补发；center 为相对坐标） |
| `budget` | `{kind}` | 预算终止：`STEP_BUDGET_EXCEEDED` / `CALL_DEPTH_EXCEEDED` / `CANCELLED`（先于 run_end 发出） |

- **投屏标记兼容**：v2 引擎的 `tap` / `swipe` / `hit` / `miss` 事件（设备像素
  坐标）在 v3 保留同形——宿主侧 input.tap / input.swipe / vision 匹配完成后
  补发，前端 overlay 无需改动即可显示 v3 运行标记。
- **发射通道（零 WIT 变更）**：guest 把事件 JSON 发到私有
  `capability.invoke("__event", …)`，宿主先于权限校验拦截转发 EventSink；
  `__event` 不进 CapabilityRegistry、不要求权限声明，解析失败静默丢弃——
  可视化事件永不影响运行结果。
- **静默展开物**：lower 展开的 timing sleep（after_tap / after_match /
  poll_interval）与 find / check / match_first 轮询体不是 surface 步骤，
  不产生 step 事件（vision 事件每次真实匹配仍会发出）。
- **前端消费**：`web/src/components/console/useRunEvents.js`（分发 + feed
  状态）→ `RunEventsPanel.vue`（运行事件 feed，budget / 失败行高亮）+
  `ScriptSummary.vue`（按 path 高亮当前顶层卡片：嵌套路径映射其顶层祖先，
  如 `steps[2].then[1]` → 第 3 张卡；失败标红；run_start / 新运行重置）。
