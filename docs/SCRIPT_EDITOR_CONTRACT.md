# 脚本 v2 语法契约

> 状态：当前无兼容基线；改动本文档需同步 fixture 与双方测试。
> 依据：当前服务端严格 loader、编辑器 Model 与 YAML 文档的共同契约。
> 可执行样例：`server/tests/fixtures/script_v2/`（索引见其 README.md），前端副本
> `web/src/script-editor/__fixtures__/`（逐字节一致，一致性有测试守护）。
> 注意：本文档与 `docs/YAML.md` 描述同一套当前 v2 严格语法；不提供旧格式兼容或自动迁移。

## 1. 本文五方

任何一条语法规则都必须在五方同时成立，五方互为镜像：

1. **Rust AST** — `server/src/script_v2/` 的严格装载与校验目标，禁止在执行循环里按 `serde_yaml::Value` 猜动作；
2. **前端 Model** — 可视化编辑器唯一编辑源，golden JSON 即用该字段名书写；
3. **规范 YAML** — 服务端持久化与导入导出格式，由 codec 统一序列化产出；
4. **API JSON** — 保存/校验/运行/任务接口中模型与参数的 JSON 形态（与前端 Model 同构，见 §6）；
5. **结构化错误码** — `code + message + resource + step_path + field` 五元组（plan §13.2），前端据此定位卡片，不解析中文文案。

## 2. 解析层选型结论（Rust）

**结论：采用 `saphyr-parser 0.0.12`（crates.io，YAML 1.2 事件级解析器），由服务端严格 loader 使用。**

- 需求背景：params 每项必须是「整条单引号」标量（§3.3），而 `serde_yaml 0.9` 反序列化成 `Value` 后**书写样式彻底丢失**——`'bool:enable:x:true'`（单引号）与 `bool:enable:x:true`（无引号）得到完全相同的 `Value::String`，无法校验引号契约。这一点已用测试固化：`server/src/script_v2/fixtures_tests.rs::serde_yaml_loses_scalar_style`。
- 选 `saphyr-parser` 的理由：
- 事件级 API：`Parser` 是 `Iterator<Item = Result<(Event, Span), ScanError>>`，`Event::Scalar(Cow<str>, ScalarStyle, anchor, tag)` 直接携带 `ScalarStyle::{Plain, SingleQuoted, DoubleQuoted, Literal, Folded}`，测试已验证单引号/无引号可区分；
  - 每个事件带 `Span`（行列区间），是错误定位到 `step_path`/`field` 乃至源码行列的基础；
  - YAML 1.2、零拷贝（`Cow<'input, str>`）、saphyr 工作组维护（yaml-rust 的后继项目）；
  - 对 match 紧凑缩进（indentless sequence，§4.1）解析正确，golden 样例 v07/v11 已回归。
- 备选 `yaml-rust2 0.12`：同为 YAML 1.2 且可取样式，但其高层 `YamlLoader` 同样丢样式、事件 API 无 Span、生态位是旧 yaml-rust 的延续维护，故不选。
- 排除「serde_yaml + 源码正则预扫描」方案：对多行标量、注释、引号转义、嵌套结构的样式推断不可靠，且等于把解析做两遍；仅在“只需粗判、不要 span”的场景才值得。
- 当前落点：`server/src/script_v2/` 提供 `parse_script_file()/parse_function_file()`，
  服务端 fixture、仓库示例数据和 API 保存/导入/运行均通过这条严格装载路径。

## 3. 五方字段对照表

### 3.1 资源与文件布局（plan §5）

| 项 | 值 |
|---|---|
| 应用分区结构 | `data/<pkg>/yaml/`（可执行脚本）、`data/<pkg>/func/`（函数库）、`data/<pkg>/tmpl/`（灰度模板 PNG） |
| 脚本资源 ID | `<pkg>/<文件名>.yaml`（如 `daily/login.yaml`） |
| 函数路径 | `<文件短路径>/<函数名>`（如 `common/login` = `func/common.yaml` 里的 `login`；函数文件多函数共存） |
| 运行选择器 | 只有 `yaml/` 下的脚本可手动运行/立即运行/进入定时任务；`func/` 只能被 `func` 步骤调用或函数测试 API 使用 |
| 模板引用 | 短名（`account.png`）；磁盘名可带 `#` 搜索区域后缀；同扩展名短名必须唯一，歧义为资源错误 |
| 跨分区 | 一律不解析、不回退；模板/函数/子脚本只在当前应用分区查找 |
| 目录即类型 | 不做内容推断；`yaml/` 里必须有顶层 `steps`，`func/` 顶层键全是函数名 |

### 3.2 顶层结构与模型

**Rust AST：**

```rust
enum Resource {
    Script(ScriptFile),
    FunctionLibrary(FunctionFile),
}
struct ScriptFile { params: Vec<ParamDecl>, config: Option<ScriptConfig>, steps: Vec<Step> }
struct FunctionFile { functions: Vec<FunctionDecl> }              // 保持书写顺序
struct FunctionDecl { name: String, params: Vec<ParamDecl>, steps: Vec<Step> }
struct ScriptConfig { interval: Duration, threshold: f64, log_level: LogLevel }  // 可缺省整体
```

**前端 Model（golden JSON 用此字段名）：**

```jsonc
ScriptModel        { params: ParamDecl[], config: ScriptConfig | null, steps: Step[] }
FunctionLibraryModel { file: string /*文件短路径*/, functions: FunctionModel[] }
FunctionModel      { name: string, params: ParamDecl[], steps: Step[] }
ScriptConfig       { interval: string, threshold: number, log_level: "debug"|"info"|"warn"|"error" }
```

**规范 YAML：**

```yaml
# 脚本（yaml/）
params:              # 可选
  - 'tmpl:account:账号模板:account.png'
config:              # 可选；整体省略 = 使用 config.toml 运行时默认
  interval: 500ms
  threshold: 0.85
  log_level: info
steps:               # 必需；可为空列表，不可省略
  - ...

# 函数库（func/）：顶层键 = 函数名，记录只允许 params/steps，无文件级 config
login:
  params:
    - 'time:timeout:等待时间:30s'
  steps:
    - return: true
```

顶层键白名单：脚本只允许 `params/config/steps`；函数记录只允许 `params/steps`。
出现白名单之外的任何顶层键统一报 `script.top_level.unknown_key`，不生成迁移引导。

**API JSON（当前实现）：** `POST /api/scripts` 与 `POST /api/functions` 创建资源，
body 使用 `pkg/name/content`；已有资源用 `PUT` 更新，默认携带 `expected_version`，
`force:true` 才跳过版本比较。响应/错误由服务端返回资源版本或结构化诊断，读取接口返回
当前规范 YAML 与版本信息。

### 3.3 参数声明 ParamDecl

五方对照：

| 方 | 形态 |
|---|---|
| 规范 YAML | 整条单引号标量：`- 'bool:enable:是否启用:true'`（**必须**单引号，无引号非法） |
| 前端 Model | `{ type, name, remark, default }`；`default: null` = 必填 |
| Rust AST | `struct ParamDecl { ty: ParamType, name: String, remark: String, default: Option<Literal> }` |
| API JSON | 同 Model：`{"type":"bool","name":"enable","remark":"是否启用","default":true}` |
| 错误定位 | `step_path = "params[0]"`，`field ∈ {style, declaration, name, default}` |

固定规则（plan §6.1，全部冻结）：

1. 格式 `类型:变量名:备注[:默认值]`；前 3 段必需且非空，第 4 段整体为默认值尾串（`splitn(4, ':')`），因此 text 默认值可含冒号（`text:url:服务地址:https://example.com:8443`）。
2. **整条单引号**：params 每项必须写作单引号 YAML 标量。原因：无引号 plain 标量在尾串为空时会被 YAML 解析成映射（`text:x:名:` → `{text:x:名: null}`）、样式信息丢失无法校验、特殊字符有歧义。违反 → `param.decl.quote_style`。
3. 类型/变量名/备注不得含半角冒号（否则切分错位 → `param.decl.format`）；备注需冒号用全角 `：`。
4. 变量名 `[A-Za-z_][A-Za-z0-9_]*`，同一参数表内唯一（`param.decl.name_invalid` / `param.decl.name_duplicate`）。
5. 类型 ∈ 七类：`tmpl / coord / color / time / key / text / bool`；默认值按声明类型解析为类型化字面量（非法 → `param.default.invalid`）；**空默认值（尾串为空）非法**，不等价于没有默认值（→ `param.default.empty`）。
6. 没有默认值 = 必填；带默认值可在调用/运行 args 中省略。
7. 规范序列化（codec 输出）：有默认值才输出第 4 段；`text` 默认值一律双引号包裹（`"示例"`，空字符串 `""`）；`coord` 默认值 `[0.5, 0.8]`（逗号后一个空格）；`bool` 输出 `true/false`；`color/time/key/tmpl` 原样输出。fixture 前端测试即按该规范形态从 Model 重构原始串比对。

默认值原始尾串 → 类型化字面量：

| 类型 | 原始尾串示例 | Model/API JSON 默认值 | 约束 |
|---|---|---|---|
| tmpl | `account.png` | `"account.png"` | 当前分区唯一模板短名 |
| coord | `[0.5, 0.8]` | `[0.5, 0.8]`（数组） | 两个 0~1 数字 |
| color | `ff8800` | `"ff8800"` | 6 位十六进制；见 §4.2 字符串化 |
| time | `30s` | `"30s"` | 单位 ms/s/m/min/h/d，>0 |
| key | `ESC` | `"ESC"` | 服务端按键枚举 |
| text | `"示例"` | `"示例"`（剥离外层双引号） | 可含冒号/空格；空值须写 `""` |
| bool | `true` | `true`（布尔） | 仅字面 `true/false`，字符串 `"true"` 非法 |

### 3.4 取值单元格 Cell

字段级取值（坐标/模板/颜色/时间/按键/布尔/文本）五方对照：

| 方 | 字面量 | 参数引用 |
|---|---|---|
| 规范 YAML | 字面量本体（坐标为 `[x, y]` 序列，布尔为 `true/false`，其余为标量） | `$name` 完整值引用（不做全文替换，plan §6.1） |
| 前端 Model | `{"lit": <类型化字面量>}` | `{"ref": "<参数名>"}` |
| Rust AST | `enum Cell { Lit(Literal), Ref(String) }`，按字段类型约束 `Literal` 变体 | 同左 |
| API JSON | 同 Model | 同 Model |

- `$name` 是**完整值引用**：解析后的值以 YAML 节点类型绑定；仅 text/log 字面量允许 `$name` 内嵌插值（明确支持的输入框才开放）。
- match 候选模板与 color 候选颜色处于“键位”，YAML 中同样接受 `$name` 引用串。
- 编辑器显示 `$name`，底层保存类型化引用对象，不靠字符串前缀猜类型（plan §9）。

### 3.5 Step 十九种对照

YAML 形态（规范） ↔ Model 字段（`kind` 判别 + 以下字段）。所有分支子列表（`then/else/candidates[].steps/expect[].steps/loop.steps`）递归为 `Step[]`；Model 中显式存在（空列表 `[]`），规范 YAML 省略空分支与默认字段。Rust AST 为 `enum Step { ... }` 对应变体；API JSON 与 Model 同构。

| kind | 规范 YAML | Model 字段 |
|---|---|---|
| str_app | `- str_app`（裸标量；带值非法） | `{kind:"str_app"}` |
| cls_app | `- cls_app`（裸标量） | `{kind:"cls_app"}` |
| tap | `- tap: [0.5, 0.5]` 或 `- tap: $pos` | `{kind:"tap", at: Cell<coord>}` |
| swipe | `- swipe:` + `{fm:[..], to:[..], time:800ms}` | `{kind:"swipe", from: Cell<coord>, to: Cell<coord>, time: Cell<time>}`（YAML 键 `fm` ↔ Model 字段 `from`） |
| key | `- key: ESC` / `- key: $cancel_key` | `{kind:"key", key: Cell<key>}` |
| text | `- text: "hello"` / `- text: $message` | `{kind:"text", value: Cell<text>}` |
| log | `- log: 文本` / `- log: $msg` | `{kind:"log", message: Cell<text>}` |
| wait | `- wait: 1s` / `- wait: [1s, 3s]`（随机区间） | `{kind:"wait", duration: Cell<time>, duration_max: Cell<time>\|null}` |
| find | `- find: $account` + 兄弟键 `block`(模板列表)/`verify`(bool)/`timeout`/`then`/`else` | `{kind:"find", template: Cell<tmpl>, block: Cell<tmpl>[], verify: bool, timeout: Cell<time>\|null, then: Step[], else: Step[]}`；命中点击中心，点击后等 `config.interval` |
| match | 见 §4.1 紧凑缩进 | `{kind:"match", candidates: {template: Cell<tmpl>, click: boolean, steps: Step[]}[], else: Step[], timeout: Cell<time>\|null}`；候选默认不点击，`click:true` 命中点模板框中心后等 `config.interval` |
| check | `- check: logo.png` + 兄弟键 `timeout`(可选，默认 5s、0=单次)/`throw`(可选) | `{kind:"check", template: Cell<tmpl>, timeout: Cell<time>\|null, throw: string\|null}`；按 interval 轮询断言，未命中按 throw 文案结束运行（见 §4.6） |
| color | 见 §4.2 | `{kind:"color", at: Cell<coord>, expect: {color: Cell<color>, click: boolean, steps: Step[]}[], else: Step[]}`；候选默认不点击，`click:true` 命中点取样点后等 `config.interval` |
| if | `- if: $enable` + 兄弟键 `then`/`else` | `{kind:"if", cond: Cell<bool>, then: Step[], else: Step[]}` |
| loop | `- loop:` + `{times: 3, steps: [...]}`；`times` 省略 = `0` = 无限 | `{kind:"loop", times: number, steps: Step[]}` |
| break | `- break`（仅 loop 子流程内合法，跳出最近一层 loop） | `{kind:"break"}` |
| call | `- call: sub/inner.yaml` + 兄弟键 `args`（具名映射） | `{kind:"call", target: string, args: {name: Cell}}`；无布尔分支 |
| func | `- func: common/login` + `args`/`then`/`else` | `{kind:"func", target: string, args: {name: Cell}, then: Step[], else: Step[]}` |
| throw | `- throw` / `- throw: 原因` | `{kind:"throw", message: string\|null}` |
| return | `- return: true` / `- return: $enable`（仅函数文件） | `{kind:"return", value: Cell<bool>}` |

规则：一个步骤只允许一个动作键（多动作键 → `step.multi_action`）；动作键之外的同级键是步骤字段（未知字段 → `step.field.unknown`）。**特例**：`check` 的 `throw` 字段与动作键 `throw` 同名词——步骤内存在 `check` 键时该键固定解析为字段，不参与动作键计数（两端 loader 同规则）。

### 3.6 config

| 键 | YAML | Model/API | 约束 |
|---|---|---|---|
| interval | `500ms` | `"500ms"` | 带单位时间，>0；轮询及所有脚本点击后的等待间隔 |
| threshold | `0.85` | `0.85`（数字） | 0~1 |
| log_level | `info` | `"info"` | debug/info/warn/error |

整体省略 = 使用 `config.toml` 同名运行时默认；不允许未知 config 键。

## 4. 文字契约（冻结）

### 4.1 match 紧凑缩进（唯一序列化格式）

```yaml
- match:
  - test1.png:
    - log: 命中 test1
  - test2.png:
    - log: 命中 test2
  else:
    - log: 都未命中
  timeout: 30s
```

- 候选列表是 `match` 键下的**无缩进序列**（indentless sequence，标准 YAML 语法）；每项候选是单键映射，候选值二选一：
  - **列表形态**（原形态）`模板: [分支步骤]`——不点击；
  - **映射形态** `模板: {click: true, steps: [...]}`——命中后点击该候选模板匹配框中心（语义同 find 的中心点击）；`steps` 省略 = 空分支（命中即点）。
  规范序列化不变式：`click: false` ⇔ 列表形态，`click: true` ⇔ 映射形态；映射键比候选模板键**深两级**（YAML 映射值不能与键同列，序列才能同列）。候选值映射内只允许 `click`/`steps`（未知 → `step.field.unknown`），`click` 非布尔字面量 → `step.field.type_mismatch`。
- `else` / `timeout` 是 `match` 步骤的**兄弟键**，与 `match` 同列；**绝不允许**写成候选列表里的 `- else:` / `- :`（→ `step.match.else_in_candidates`）。
- 候选按书写顺序匹配、首个命中获胜、复用同一帧；默认不点击，仅 `click: true` 候选命中后点击并等待 `config.interval`；未配 `timeout` 只执行一轮，配了按 `config.interval` 轮询到超时才进 `else`。
- 候选模板短名不可重复（→ `step.match.candidate_duplicate`）；不接受布尔条件（布尔走 `if`）。
- 语义分工冻结（2026-09-01 候选级 `click` 增补）：`if`=布尔分支、`match`=模板策略选择（默认不点击，候选可选命中点击）、`find`=等待并点击中心、`color`=单点颜色分支（候选可选命中点击）。

### 4.2 color 全位置字符串化 + 候选列表形态

- 颜色在**所有位置**都是字符串：ParamDecl 默认值、`expect` 候选、`args` 实参、任务快照、RunRecord 摘要——统一为 6 位十六进制无 `#`（Model/API JSON 中即 string）。
- 规范 YAML 中**纯数字色值必须加引号**（`'123456'`），防止被解析成数字丢前导零；含字母色值（`ff8800`）可裸写。
- 解析端**不得**让 YAML 1.1 数字解析改变颜色值：事件级解析（§2）天然取原始串；任何基于 plain-object 的解析（serde_yaml Value、js-yaml load）必须把颜色位置重新字符串化。
- `expect` 冻结为**有序列表**，每项是单键映射，候选值与 match 候选同构（§4.1）：列表形态 `颜色: [分支步骤]` = 不点击；映射形态 `颜色: {click: true, steps: [...]}` = 命中后点击取样点并等待 `config.interval`；序列化不变式同 §4.1。**不用**颜色做整个映射的键。原因：纯数字色作为映射键会被 JS plain object 按整数形键重排（js-yaml `load()` 实测把 `'123456'` 排到最前），候选顺序语义被静默破坏——该坑已记录 docs/PITFALLS.md。
- `color` 不轮询；默认不点击，仅 `click: true` 候选命中后点击取样点并等待 `config.interval`；同色候选重复 → `step.color.duplicate`；颜色格式非法 → `step.color.format`。

### 4.3 默认值解析时机

- **保存时**：参数声明与默认值在脚本/函数保存前按 §3.3 解析并类型校验（`tmpl` 默认值此时检查分区内唯一存在）。
- **运行/调用绑定顺序冻结为：声明默认值 → 显式 args/入参覆盖**。绑定完成后再做类型校验与引用解析（plan §6.3）。
- 显式传入的 `null`/空串**不触发**默认值：作为显式值按目标类型校验（仅 text 接受空串，七类都不接受 null）。
- **定时任务快照一次性解析**：任务保存的是经过类型校验的**完整类型化 args 快照**（每个参数都有值），调度运行使用快照，不回读声明默认值、不依赖浏览器在线；声明默认值后续变化不影响已保存任务（plan §12.3）。手动运行的稀疏 args 才走“默认值 → 覆盖”的即时解析。快照形态见 fixture v12 `task_snapshot`。

### 4.4 RunTarget

运行入口统一为二选一（函数不进 RunManager，走函数测试 API）：

```jsonc
// 手动运行 / 从步骤运行 / 定时任务
{ "type": "script",   "script_id": "<pkg>/<name>.yaml", "start_index": 0 }
// 函数测试（Console「点函数名运行」/ 编辑器「测试函数」）
{ "type": "function", "pkg": "<分区>", "file": "common", "function": "login", "start_index": 0 }
```

- `start_index`：主流程顶层步骤序号（从 0）；函数为函数体内顶层步骤序号；不支持任意深层嵌套步骤直接启动。
- 运行前必须解析全部必填参数（后续步骤可能引用）；`args` 为稀疏映射（§4.3）。
- 脚本运行使用 `POST /api/scripts/:id/run`，函数测试使用 `POST /api/functions/:id/run`；
  两者都接受 `device_id/start_index?/args?`，异步成功返回 202 与 `run_id/resolved_args`。

### 4.5 任务参数签名算法（psig1）

`param_signature` 覆盖**类型/名称/必填性/默认值**四要素，声明顺序敏感，用于任务过期检测：

```
param_signature := "psig1" + "|" + join(entries, "|")          # 按声明顺序
entry           := type "," name "," required "," canonical_default
required        := "1"(无默认值) | "0"(有默认值)
canonical_default（required=1 时为空串）：
  bool  → "true" | "false"
  coord → "[x,y]"（逗号后无空格；数字最短十进制表示）
  color → 6 位十六进制小写（无 #）
  key   → 大写
  time  → 小写；"min" 归一为 "m"；数值保持书写形式
  text  → 转义："\\"→"\\\\"、","→"\\,"、"|"→"\\|"（后两者是分隔符）
  tmpl  → 短名原样
```

示例（fixture v12）：`psig1|bool,enable,0,true|time,timeout,0,30s|text,message,0,开始任务|coord,pos,0,[0.5,0.5]|color,target,0,123456|key,quit_key,0,ESC|tmpl,icon,0,icon.png`

- 前缀 `psig1` 是算法版本号；日后算法变更换前缀，旧签名直接判过期。
- 签名与任务快照一起持久化；脚本参数声明变化 → 重算签名不一致 → 任务标“参数已过期”，禁用调度或调度时明确失败（`runtime.task.param_stale`）。
- 服务端（`model::param_signature`）与前端（`fixtures.test.js::paramSignature`）各有一份实现，双向测试锁定。

### 4.6 check 界面断言（2026-09-02 增补）

```yaml
- check: logo.png
```

- **轮询语义**：在 `timeout` 内按 `config.toml` 的 `interval` 重复截图匹配（NCC 同 find），不点击、无分支；`timeout: 0` 只检查一次。
- 命中 → 推送命中框可视化（Hit 事件，与 match 命中同构）后继续后续步骤；在 `timeout` 内仍未命中 → 按 `throw` 步骤同语义**结束整个运行**（含调用链），`throw` 文案进运行日志。
- `timeout` 省略默认为 5s，必须带单位且 `>= 0`；`throw` 可省略，默认文案为 `模板名 模板不存在`，显式值必须非空；模板字面量做分区存在性校验（`resource.tmpl.not_found` / `resource.tmpl.ambiguous`），`$ref` 走通用 tmpl 引用校验。
- `throw` 字段与动作键 `throw` 同名词：步骤内存在 `check` 键时该键固定解析为字段，不参与动作键计数（前端 codec、fixtures.test.js 与服务端 loader 同规则，fixture v13 锁定）。
- 语义分工：`find`=等待并点击、`match`=策略选择、`check`=「界面必须长这样」的断言（不符合即终止并报原因）。

## 5. 结构化错误

### 5.1 错误结构

```jsonc
{
  "code": "step.match.candidate_duplicate",   // 域.主体.问题，见 5.2
  "message": "候选模板 dup.png 重复",          // 人类可读，中文
  "resource": "daily/login.yaml",             // 出错资源 ID（fixture 测试中以用例 ID 代替）
  "step_path": "steps[0]",                    // 定位路径，见 5.3
  "field": "candidates"                       // 出错字段名；顶层/整文件错误可为 ""
}
```

前端拿 `{code, step_path, field}` 定位到卡片与控件，`message` 仅展示；**禁止**解析中文文案定位（plan §13.2、§17.9）。

### 5.2 step_path 语法

- 脚本：`params[0]`、`config`、`steps[0]`、`steps[1].then[0]`、`steps[2].candidates[1].steps[0]`、`steps[3].expect[0].steps[2].else[0]`；
- 函数库：函数名直接作为顶层路径，如 `login.steps[0]`、`is_enabled.params[1]`；
- 顶层/整文件错误 `step_path = ""`，`field` = 顶层键名或 `"yaml"`（语法错误）。

### 5.3 错误码命名空间清单（五域）

| 域 | code | 含义 |
|---|---|---|
| **资源 resource.** | `resource.tmpl.not_found` | 模板短名在当前分区不存在 |
| | `resource.tmpl.ambiguous` | 同短名多个 `#` 后缀候选，歧义 |
| | `resource.tmpl.name_conflict` | 新建/重命名短名与现有模板冲突 |
| | `resource.script.not_found` | call 目标脚本不存在 |
| | `resource.func.not_found` | 函数文件或函数名不存在 |
| | `resource.pkg.mismatch` | 引用跨应用分区 |
| | `resource.file.invalid_name` | 文件短路径非法（空段/非法字符） |
| **参数 param.** | `param.decl.quote_style` | params 项未整条单引号 |
| | `param.decl.format` | 声明不是四段式/类型变量名备注为空/类型未知 |
| | `param.decl.name_invalid` | 变量名不符合 `[A-Za-z_][A-Za-z0-9_]*` |
| | `param.decl.name_duplicate` | 同一参数表内变量名重复 |
| | `param.default.empty` | 空默认值（尾串为空，不等价于无默认值） |
| | `param.default.invalid` | 默认值不能按声明类型解析 |
| | `param.ref.unknown` | `$name` 引用不存在的参数 |
| | `param.ref.type_mismatch` | 引用类型与字段类型不符 |
| | `param.args.unknown` | args 键不是目标参数 |
| | `param.args.missing_required` | 必填参数未出现在 args |
| | `param.args.type_mismatch` | args 值类型与目标参数不符 |
| **步骤 step.** | `step.unknown_action` | 未知动作键 |
| | `step.multi_action` | 一个步骤多个动作键 |
| | `step.field.missing` | 必填字段缺失（如 swipe 缺 to） |
| | `step.field.type_mismatch` | 字段类型错误 |
| | `step.match.candidate_duplicate` | match 候选模板短名重复 |
| | `step.match.else_in_candidates` | `- else`/`- timeout` 写进候选列表 |
| | `step.if.non_bool_cond` | if 条件非布尔 |
| | `step.color.duplicate` | color 颜色候选重复 |
| | `step.color.format` | 颜色不是 6 位十六进制 |
| | `step.coord.range` | 坐标超出 0~1 |
| | `step.time.format` | 时间缺单位/非法/≤0 |
| | `step.wait.range_invalid` | 随机区间起点大于终点 |
| | `step.loop.empty_steps` | loop 子流程为空 |
| | `step.break.outside_loop` | break 不在 loop 子流程内 |
| | `step.return.in_script` | return 出现在脚本（仅函数合法） |
| | `step.nesting.depth` | 步骤嵌套超限 |
| **引用 ref.** | `ref.call.path_traversal` | call 目标路径穿越/绝对路径/反斜杠 |
| | `ref.call.self_cycle` | call 目标是脚本自身 |
| | `ref.call.cross_cycle` | 跨文件调用成环（引用图） |
| | `ref.call.depth` | 调用深度超限（32 层） |
| | `ref.func.path_traversal` | 函数路径穿越/绝对路径/反斜杠 |
| | `ref.func.syntax` | 函数路径不是 `<文件短路径>/<函数名>` |
| | `ref.func.missing_args` | 函数必填参数未传 |
| | `ref.template.ambiguous` | 模板短名解析歧义（同 resource.tmpl.ambiguous 的引用侧视图） |
| **运行 runtime.** | `runtime.step.limit` | 超 10 万步防死循环 guard |
| | `runtime.nesting.limit` | 函数嵌套超 32 层 |
| | `runtime.task.param_stale` | 任务参数签名过期（声明已变） |
| | `runtime.device.busy` | 设备被其他运行占用 |
| | `runtime.device.offline` | 设备离线/会话不可用 |
| | `runtime.frame.unavailable` | 帧不可用（ffmpeg 失败/无帧） |
| | `runtime.cancelled` | 运行被用户取消 |
| | `runtime.engine.throw` | 脚本 throw 终止（携带原因） |
| | `runtime.run.not_found` | 运行实例不存在/已归档 |

当前 fixture 已由非法样例覆盖的码：`script.top_level.unknown_key`、
`script.root_type`、`param.decl.quote_style`、`param.decl.format`、`param.decl.name_duplicate`、
`param.default.empty`、`param.default.invalid`、`step.match.candidate_duplicate`、
`step.match.else_in_candidates`、`step.match.candidates_type`、`step.list_type`、
`ref.func.path_traversal`、`ref.call.self_cycle`、`func.record_unknown_key`、`func.record_type`、
`yaml.syntax_error`（其余由当前 loader 或运行时覆盖）。

## 6. API JSON 形态（当前实现）

- **保存**：`POST /api/scripts`、`POST /api/functions` 创建（body 含 `pkg/name/content`）；`PUT` 更新已有资源，默认要求 `expected_version`，`force:true` 跳过版本比较。冲突返回 409。
- **导入/读取**：脚本分区导入支持 preview 与 `confirm=1` 写入；读取返回当前规范 YAML 与版本。保存和导入均由服务端严格 loader 校验。
- **运行**：`POST /api/scripts/:id/run` 与 `POST /api/functions/:id/run` 接受 `device_id/start_index?/args?`；成功异步返回 202 与 `run_id/resolved_args`，参数或 YAML 诊断为结构化错误。
- **定时任务**：任务持久化全量类型化 `args` 快照与 `param_signature`；任务保存、启用和立即运行复用严格 loader 与签名门禁。
- **模板**：`POST /api/templates` 只创建，`PUT /api/templates/:name/image` 只替换既有图像；创建接口不覆盖已有模板。

## 7. fixture 体系

- 逻辑 ID 体系、样例索引、golden/expected JSON 结构：见 `server/tests/fixtures/script_v2/README.md`。
- 前端副本映射：`server/tests/fixtures/script_v2/<file>` ↔ `web/src/script-editor/__fixtures__/yaml|json/<file>`，逐字节一致由 `fixtures.test.js` 的漂移测试强制。
- 服务端断言位于 `server/src/script_v2/fixtures_tests.rs`，直接调用严格 loader，并覆盖仓库
  `server/data/<pkg>/{yaml,func,tmpl}` 示例；前端断言位于
  `web/src/script-editor/__fixtures__/fixtures.test.js`。
- 修改任何契约必须同步本文档、`docs/YAML.md`、双方 fixture 和双方测试；保存、导入、运行、
  函数测试、任务保存均不得绕过严格 loader。


## 附：实现期澄清（阶段 1~3 期间冻结，与 fixture 同步）

- **color 的 `else` 写在步骤级**（与 `color:` 键同列），`at`/`expect` 位于 color 值映射内；
  候选列表内的 `else` 属于结构错误。
- **色值序列化引号**：纯数字色值必须加引号防止 YAML 数字化；含字母色值可裸写。
- **args 字符串实参引号**：呈 time/key/color 形态的串 plain 输出，其余双引号——模型不带目标类型信息，这是唯一可判定规则（v09/v10 双向锁死）。
- **time 单位大小写**：接受不敏感输入，存储原样；psig1 归一化仅在签名侧。
- **config 未知键**：使用当前 `script.config.unknown_key` / `script.config.invalid` 诊断。
