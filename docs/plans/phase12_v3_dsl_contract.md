# Phase 12 — YAML v3 最终语法契约（并行开发对齐依据）

> 本文件是 Phase 12 并行开发期间的**唯一语法对齐依据**：后端任务（T2 call 统一 / T45 defaults·find / T3 预算 / T7 事件）按此实现；前端任务（T8 编辑器重写）按此编写 codec/编辑器。
> 权威裁决见 `docs/reference/adr/ADR-YAML-01~04`；本契约是它们的可实施展开。实现中发现契约不可行时，**必须报告由主线修订，不得单方面偏离**。

## 1. Program（scripts/ 资源）

```yaml
version: 3
params:                      # 可选；字符串 / 映射双形态（沿用现状）
  - 'int:count:次数:3'
  - name: mode
    type: string
    default: auto
defaults:                    # 可选（T45 实现）
  vision:
    threshold: 0.80
  timing:
    after_tap: 300ms         # tap 后等待
    after_match: 200ms       # 匹配命中后等待
    poll_interval: 100ms     # find/check 轮询间隔
steps:
  - ...
```

- 非 `version: 3` → 错误 `unsupported yaml version`，无 fallback。
- 函数库（functions/ 资源）：bare-map `{<函数名>: {params, steps}}`，**无 version 键**，steps 用 v3 步语法。

## 2. call（T2）

```yaml
- call:
    target: script:daily/login        # 或 function:工具/月卡领取
    with:                             # 参数名 → 表达式；`args` 保留为兼容别名
      account: $user
    save: result                      # 可选；返回值整体存入；无 return → null
```

- 命名空间仅 `script:` / `function:`；裸 target → 解析期诊断（错误码 `yaml.v3.call.namespace`）。
- `function:<文件短路径>/<函数名>`：文件短路径按最后一个 `/` 分割（同 v2 split_func_path，含穿越校验）。
- return 支持任意 JSON 值；递归深度上限 32，超限 `CALL_DEPTH_EXCEEDED`。

## 3. find / match_first / match 上下文（T45）

```yaml
- find:
    template: reward
    timeout: 10s          # 可选；缺省 30min
    threshold: 0.90       # 可选 step override
    region: {...}         # 可选；形态沿用现有 vision.match 实现
    save: reward          # 可选
    then:                 # 命中后步骤组（唯一键名；不设 block/steps/on_found 别名）
      - tap: {point: $reward.center}
    else:                 # 超时后步骤组（可选）
      - log: 未找到
    verify:               # 可选；then 执行完后二次验证
      template: home
      timeout: 5s

- match_first:
    candidates:
      - template: reward
        threshold: 0.9
        steps:            # 候选命中后执行（唯一键名）
          - tap: {point: $match.center}
      - template: close
        steps:
          - tap: {point: $match.center}
    else: ...
```

- match 结果值形态（save 存入 / `$match` 引用）：
  `{found: bool, score: number, x, y, width, height, center: {x, y}, region: {...}}`；坐标为相对值 0~1（沿用现状）。
- 未 save 时 `$match` 仅在对应 find/match_first 的 then/else/verify/steps 体内可见；save 后跨步可用。
- **全面移除 click 语法**：v3 `find.click` 字段、match 候选 click、`click_when` 步骤删除；`wait_for` 若与 find 完全同义一并移除（T45 裁决并报告）；`retry` 同理评估。
- `check` 保留：`- check: {template, timeout, threshold}`（轮询至出现，超时 throw）。

## 4. timing / threshold 优先级（T45）

- threshold：step 值 > `defaults.vision.threshold` > Runtime 内置 0.80。
- timing defaults 取代一切隐藏 config interval / judge_delay；脚本行为自包含。
- wait 随机：`- wait: {min: 300ms, max: 700ms}`（min/max 同给）；标量 `- wait: 300ms` 等价固定值。

## 5. 执行预算（T3）

- `max_steps = 100_000`（逻辑步：顶层 + 循环体 + 分支体每步计数）；`max_call_depth = 32`。
- 错误码：`STEP_BUDGET_EXCEEDED` / `CALL_DEPTH_EXCEEDED` / `CANCELLED`；必须进 Run Event / 日志可观察。
- host 侧 wasmtime epoch interruption 作为取消兜底，与 stop 标志双机制共存。

## 6. step 身份与运行事件（T45 / T7）

- lower 阶段为每个 surface step 标注稳定路径，语法 `steps[0].then[1]`（与前端编辑器 commands 路径寻址一致）；`step_start` / `step_end` 事件携带该 path。
- 事件 wire（DataChannel `{"type":"se","ev":...}`）：
  `run_start` / `run_end{ok,error}` / `step_start{path,desc}` / `step_end{path,ok,error}` / `call_start{target,depth}` / `vision{template,found,score,center}` / `budget{kind}`；事件不携带帧图像数据。
- 现有 `tap/swipe/hit/miss` 事件保留不删（v2 引擎仍在用，v3 可发同形事件）。

## 7. 参数 schema API（T6）

- `Program.params` 是参数唯一来源；服务端提供按 entrypoint 取参数 schema 的 REST 能力（REST 形态由 T6 设计，约束：前端不得为取参数而解析 YAML）。
- schema 需含 name/type/default/required/enum/description，足以渲染表单；v2 存量脚本走同一 descriptor 端点（服务端 v2 解析已存在）。
- 参数类型集合：string / number / integer / boolean / enum（v2 ty 名映射到上述类型；执行期 TypedValue 行为不变）。

## 8. 手动运行 start_index（T2 顺带）

- guest 小 AST 解释器支持 program JSON 顶层可选 `start_index`：跳过其前的顶层 surface 步骤（与 v2「从此运行」语义一致）；host 由 payload 注入。
