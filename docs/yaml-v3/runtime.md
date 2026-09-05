# 运行时：执行链、预算、start_index 与运行事件

> 本文定义 v3 程序的执行链、执行预算（ExecutionBudget）、手动运行 start_index
> 与运行可视化事件 wire 契约；权威裁决见
> [ADR-YAML-04](../reference/adr/ADR-YAML-04-execution-budget.md) 与
> [ADR-YAML-03](../reference/adr/ADR-YAML-03-match-context.md)（事件），实现
> 核对基准为 `server/src/extensions/gamer_yaml/yaml_extension.rs`、
> `wasm_host.rs`、`server/guests/yaml-guest/src/lib.rs`、
> `server/src/core/events.rs`。

## 1. 执行链

```text
gamer.yaml Runner（扩展 start 注册；手动运行走 POST /api/runs 同链）
  ↓ 读取脚本（ResourceStore composite）
parse_surface（surface YAML → SurfaceProgram；诊断 yaml.v3.*）
  ↓ lower（yaml_vnext::load）
小 AST Program + 每步 StepLabel{path, desc}
  ↓ LazyYamlWasmtimeRuntime（每 run 注入 args / nonce / start_index）
WASM guest 小 AST 解释器（本地预算计数、发 run/step/call 事件）
  ↓ capability.invoke（WIT；__event 私有事件通道旁路）
NativeYamlHost（授权 → Core CapabilityRegistry → 设备/视觉/日志）
```

- guest 与宿主只以 JSON 线交换数据；解释器刻意无 WASI import、不触达 Gamer
  内部。wasm-runtime 是 default feature（lazy init——不运行不建 Engine）；
  关闭该 feature 时 YAML 扩展无可执行运行时（运行请求直接报错），原生参考解
  释器仅测试消费。
- host 侧 wasmtime **epoch interruption 只作取消兜底**：guest 纯计算死循环可
  被以 `CANCELLED` 打断（~10ms tick 检查 stop 标志），与 stop 标志双机制共存；
  epoch **不做超时强杀**——任何终止都有确定错误码。

## 2. 执行预算（ADR-YAML-04）

```text
max_steps      = 100_000   （逻辑步）
max_call_depth = 32
```

- **步数按逻辑步计**：顶层步、loop 体每轮每个子步、if 分支体、call 目标程序
  体全计；**loop 每轮迭代本身也计**（空转体 `loop` 死循环同受约束）——外层
  loop 包裹不得绕过预算。
- **调用深度**：进入 callable +1、返回 -1，超 32 立即终止（与
  [call.md](call.md) 递归约束同值）。
- 计数在 guest 解释器内本地完成（常量与宿主侧原生解释器对齐，两处独立编译、
  测试向量锁定）；预算终止错误以机器可读码开头，原样进入 RunRecord 错误信息
  与运行日志：

```text
STEP_BUDGET_EXCEEDED: consumed=N max=100000
CALL_DEPTH_EXCEEDED: depth=N max=32
CANCELLED: …
```

- 预算数值为初始默认，后续可按压力调整（数值调整不构成语义变更）。
- `budget{kind}` 事件先于 `run_end` 发出（见 §4），运行 UI 能区分「预算耗尽」
  与「用户取消」。

## 3. start_index（「从此运行」）

- 程序 JSON 顶层可选 `start_index`：跳过其前的**顶层** surface 步（嵌套分支/
  循环体不受影响）——lower 后的顶层小 AST 步与 surface 步 1:1 对应（tap 的
  after_tap 等展开物打包进同一步的容器），顶层序号即 surface 步序号。
- host 由运行请求注入：手动运行 `POST /api/runs` 的 payload 携带
  `start_index`（前端「▶ 从此运行」提交顶层步骤序号），缺省 0 = 从头执行；
  `start_index` 超过顶层步数按错误终止（guest 校验）。
- 函数 entrypoint（`<pkg>/<库>.yaml#<函数>`）同样支持（跳过函数体内顶层步）。

## 4. 运行事件 wire 契约（ADR-YAML-03）

v3 脚本运行时经 control DataChannel **反向**推送结构事件：信封
`{"type":"se","ev":...}`（引擎 emit → viewers 注册表 `control_dc`），手动运行
与定时任务同样生效。事件**不携带帧图像数据**；前端运行事件 feed 与步骤高亮由
这些事件驱动。

### step 身份（path 语法）

lower 期为每个 surface 步骤生成稳定 path，语法与前端编辑器 commands 寻址一致：

- 顶层：`steps[0]`、`steps[2]`；
- if / find 分支：`steps[0].then[1]`、`steps[0].else[0]`；
- loop 体：`steps[1].steps[3]`；
- match_first：`steps[2].candidates[0].steps[1]`、超时分支 `steps[2].else[0]`。

同一脚本重复运行、编辑无损往返后路径保持稳定。call 进入被调方后，帧内事件的
path 仍是**该脚本本地路径**（`call_start` 事件宣告帧切换）。

### 事件表

| 事件 | 载荷 | 发射点 |
|---|---|---|
| `run_start` | `{}` | 运行进入（guest / 原生解释器） |
| `run_end` | `{ok, error?}` | 运行退出（ok=false 带 error 原文） |
| `step_start` | `{path, desc}` | 进入 surface 步骤（desc = 中文摘要，如 `find 登录按钮`、`tap 0.5,0.3`、`call script:daily/login`、`wait 300ms`） |
| `step_end` | `{path, ok, error?}` | surface 步骤完成 / 失败（error 缺省省略） |
| `call_start` | `{target, depth}` | 进入 call 目标（depth = 本地调用深度） |
| `vision` | `{template, found, score?, center?}` | 每次真实模板匹配后（宿主侧 vision 能力补发；center 为相对坐标；match_first 每个候选一条） |
| `budget` | `{kind}` | 预算终止：`STEP_BUDGET_EXCEEDED` / `CALL_DEPTH_EXCEEDED` / `CANCELLED`（先于 run_end） |

投屏标记兼容：v2 引擎的 `tap` / `swipe` / `hit` / `miss` 事件（像素坐标）在
v3 保留同形——宿主侧 input.tap / input.swipe / vision 匹配完成后补发，前端
overlay 无需改动即可显示 v3 运行标记（hit 命中框、miss 搜索区域框）。

### 发射通道（零 WIT 变更）

- guest 把事件 JSON 发到私有 `capability.invoke("__event", …)`；宿主**先于权
  限校验**拦截、解析（serde tag=`ev` 白名单即事件词表）、补 run 维度 device
  作用域后转发 EventSink。
- `__event` 不进 CapabilityRegistry、不要求扩展声明权限；解析失败 / 无 sink /
  发射失败一律**静默丢弃**——可视化事件永不影响运行结果。
- **静默展开物**：lower 展开的 timing sleep（after_tap / after_match /
  poll_interval）与 find / check / match_first 轮询体不是 surface 步骤，不发
  step 事件（`vision` 事件每次真实匹配仍会发出）。
- 前端消费：`web/src/components/console/useRunEvents.js`（分发 + feed 状态）→
  `RunEventsPanel.vue`（运行事件 feed，budget / 失败行高亮）+
  `ScriptSummary.vue`（按 path 高亮当前顶层卡片：嵌套路径映射其顶层祖先，如
  `steps[2].then[1]` → 第 3 张卡；失败标红；run_start / 新运行重置）。
