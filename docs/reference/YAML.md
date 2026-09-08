# YAML 脚本语法（V1 唯一正式方案）

GameBot 自动化脚本只支持 **YAML V1**（Gamer V1 简化计划 Phase 1；无 `version`
字段——出现 `version:` 直接报 `yaml.version.removed` 迁移诊断，旧 v3/v2 脚本
**无兼容分支、无 fallback、无迁移工具**）。

核心原则：**YAML 只描述流程，所有实际操作都是函数调用**。解释器只认识
函数调用 / `if` / `repeat` / `return` 四类步骤；`tap`、`find`、`sleep` 等
都不是语法关键字，而是函数。

- 权威实现：`server/guests/yaml-interp/`（唯一解释器，WASM guest 与宿主测试
  同源）+ `server/src/extensions/gamer_yaml/syntax.rs`（解析/校验/降线）+
  `native_funcs.rs`（原生函数注册表）；前端可视化编辑器（`web/src/script-editor/`）
  与 Runtime 共用同一 V1 surface DSL；
- 旧 v3 语法文档（docs/yaml-v3/）已删除，历史实现见 git 历史。

## 1. 目录与函数来源

脚本、函数库、模板按 **Package**（数据一级作用域）存放：

```
data/packages/<package-id>/
├── package.toml                      # manifest（id/name/version/author/targets/plugins 依赖）
├── shared/                           # 跨插件保留区（gamer.yaml 不写）
└── plugins/
    ├── gamer.yaml/
    │   ├── automations/              # 可运行脚本（.yaml/.yml）
    │   ├── functions/                # 函数库（functions: 包装，见 §4）
    │   └── templates/                # 模板图片（8-bit 灰度 PNG）
    └── <其他插件>/                    # dormant 数据原样保留，Core 不解释
```

**函数只有两种来源**（计划 Phase 3）：

1. **插件函数**：`gamer.yaml` 原生注册表（`native_funcs.rs`，Schema 唯一声明点），
   随插件安装/启用变化；受插件权限约束；
2. **当前 Package 函数**：`functions/<分类>.yaml`（用户可编辑），解释器本地执行。

运行前组合为唯一函数名注册表：同名冲突（原生 vs Package、跨文件重复）一律拒绝；
不跨 Package 查找；运行开始时冻结全部函数定义。目录首版清单：

```text
原子：tap / swipe / key / input_text / launch / stop_app / sleep / log / find
便利：wait_find / tap_template / wait_disappear
比较：eq / ne / gt / ge / lt / le
```

`GET /api/runners/gamer.yaml/functions` 返回原生函数目录（Schema 唯一前端来源）。

## 2. 脚本格式

```yaml
name: 每日签到            # 可选

params:                   # 可选：运行参数 Schema（类型/必填/默认值/说明）
  retry:
    type: integer
    default: 3
    desc: 重试次数
  secret:
    type: string
    required: true

vars:                     # 可选：字面量表（不做引用解析）
  timeout: 15s

run:                      # 必有（可为空列表）：执行入口
  - launch: com.example.game
  - wait_find:
      template: home
      timeout: $timeout
    as: home
  - if: $home
    then:
      - claim_daily: {}
    else:
      - log: 未进入主页
  - return: true
```

参数类型：`any / boolean / integer / number / string / list / object /
duration / point / template / key`（别名 bool/int/float/text 解析期归一）。
默认值按类型校验（`yaml.param.default.invalid`）。

## 3. 步骤与表达式

**一个步骤 = 恰好一个动作键**（函数名或 `if/repeat/return`）+ 可选 `as`；
`then/else/do` 是 if/repeat 的结构键。

```yaml
run:
  - tap: [0.5, 0.8]              # 位置值简写（标量/数组 → 第一个参数）
  - find: login_button           # 同上
    as: button                   # as = 返回值赋给变量
  - tap: $button.center          # $name.field 引用（仅点号字段，无索引）
  - swipe:
      from: [0.5, 0.8]
      to: [0.5, 0.2]
      duration: 500ms
  - key: HOME
  - input_text: 你好
  - sleep: 1s                    # duration：带单位串或毫秒数；0 合法
  - log: 未进入主页               # 非字符串值自动转 JSON 文本
  - log:
      message: 带级别
      level: warn
  - launch: com.example.game     # 缺省包名 = 设备配置的应用（冷启动）
  - stop_app: {}
  - repeat: $retry               # 固定次数（非负整数或整数引用）
    do:
      - tap: [0.5, 0.5]
  - return: $button              # 返回值（脚本顶层返回即运行结果）
```

- 无参函数允许 `{}`、空映射或 `sleep:`（null）；
- `if` 条件：`false`/`null` 为假，非空结果为真（无数字/字符串隐式转换；
  比较用 `eq/gt` 等函数）；
- 函数调用独立局部作用域：参数显式传入，`as` 接收返回值；
- `find`/`wait_find` 未命中返回 `null`（不是错误）；`find` timeout 缺省 0
  （单次尝试），`wait_find` 缺省 30s（轮询）。

**表达式只有两种**：字面量、`$name.field` 引用。字符串以 `$` 开头是引用；
字面量 `$` 用 `$$` 转义。无算术、无插值、无 eval、无动态索引。

诊断码命名空间 `yaml.*`（解析/结构）与 `param.args.*`（绑定）；旧 v3 源在
解析层直接报 `yaml.version.removed`，其余旧形态报 `yaml.top.unknown`——
**不接受旧语法**。

## 4. 函数库文件（当前 Package 函数）

```yaml
functions:                        # 顶层必须有 functions: 包装
  claim_daily:
    description: 领取每日奖励      # 可选说明
    params:
      timeout:
        type: duration
        default: 5s
    vars:                         # 可选：函数内字面量
      tag: local
    returns:                      # 可选：仅文档/提示，不做运行时校验
      type: boolean
    run:                          # 必有
      - tap_template:
          template: daily_button
          timeout: $timeout
      - return: true
```

脚本直接调用（两种来源语法一致）：

```yaml
run:
  - claim_daily:
      timeout: 10s
    as: success
```

- 函数名 = 小写标识符 `[a-z_][a-z0-9_]*`，保留字 `if/repeat/return` 不可用；
- 分类文件只是存储与编辑分组，不是 namespace；同一文件内函数名唯一，
  跨文件/与原生函数同名直接报冲突（`yaml.fn.conflict`）；
- 脚本文件不再内嵌局部函数库；可复用函数一律存 `functions/`。

## 5. 错误处理与预算

无 try/catch/throw/on_error。业务未命中（find 未找到）返回 `null`；参数错误、
资源不存在、设备断开、权限不足属于执行错误，终止当前 Run 并给结构化错误。

保留宿主安全机制（计划 §1.6）：取消（stop 标志 + epoch 兜底）、步预算
`STEP_BUDGET_EXCEEDED`（上限 100,000 逻辑步）、调用深度
`CALL_DEPTH_EXCEEDED`（上限 32，Package 函数本地解释递归）。

## 6. 模板引用与重命名

脚本/函数中 `find / wait_find / tap_template / wait_disappear` 的
`template` 实参用模板短名；模板重命名经 AST 同步改写（`yaml.resource.*`，
文本字面量不误改）。V1 起模板引用改写收敛为上述四个函数 + 任意调用步骤的
`template` 键。
