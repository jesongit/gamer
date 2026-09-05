# ADR-YAML-03：match 结果上下文与 find 能力

> 编号说明：ADR-01~14 是全局架构决策序列（Phase 11 收口产出）；ADR-YAML-xx 是 YAML 域专项 ADR 序列（命名见计划 §5.5），记录 gamer_yaml 扩展 DSL / Runtime 的最终语义裁决，与全局序列互不续号。
>
> 关联计划：`docs/plans/gamer_yaml_v3_finalization_v2_removal_plan.md`（§5.4 match.click 最终裁决、§11 P12.6 Runtime Visualization Events、§12 P12.7 find / match 能力补齐）。

状态：ACCEPTED（2026-09-05）

## 背景

v2 以 `match.click = true` 这类专用语法把「命中后点击」钉死在匹配步里；匹配结果只是隐式上下文，无法跨步引用；运行可视化只有 hit / miss 框事件，前端无法把运行状态精确对齐到编辑器步骤。v3 裁决：匹配结果是通用数据，命中后的动作靠通用步骤表达（计划 §22 规则 1：优先增加通用数据模型，而不是特殊 Step）。

## 决策

### 删除一切 click: true 专用语法

- 删除 v2 `match.click` 与 v3 现存 `find.click` 字段及 click 语义专用步（`click_when` 等），同一专用语法家族不留别名。
- 匹配命中后的动作统一用通用数据表达：find 命中后执行 `then`（或 steps，最终只保留一个键名）步骤组，配合 `$match.center` tap：

```yaml
- find:
    template: reward
    timeout: 5s
    save: reward
    then:
      - tap:
          point: $reward.center
      - wait: 300ms
```

### 匹配结果是通用 runtime value

```text
{ found, score, x, y, width, height, center, region }
```

- `center` 为相对坐标（与 v3 表面坐标约定一致），`region` 为本次搜索区域。
- `find` / `match_first` 通过 `save: <名字>` 把结果固化到变量，跨后续步骤使用（如 `$reward.center`）。
- 未 save 时上下文变量（如 `$match`）仅在对应 find / match 块内有效——作用域以块为界，不跨块泄漏。

### find / match_first 最终形态

- `find`：`template` + `timeout` + `region`（可选）+ `then`（命中后步骤组）+ `else`（超时分支）+ `verify`（动作后二次验证：模板 + timeout）。
- `match_first`：候选各自携带 `steps`，首个命中候选执行自己的步骤组。
- **不再支持 block / steps / then / on_found 多别名并存**——命中后步骤组只有一个正式键名（`then`）。
- verify 示意（找到 → 执行 then → 在 verify.timeout 内二次验证 verify.template，验证失败按 find 失败路径处理）：

```yaml
- find:
    template: login
    timeout: 10s
    then:
      - tap:
          point: $match.center
    verify:
      template: home
      timeout: 5s
```

### Step 身份

v3 lower 阶段为每个 surface step 标注稳定路径，语法 `steps[0].then[1]`（与前端编辑器 commands 路径寻址同语法），作为运行事件与编辑器高亮对齐的公共地址。同一脚本重复运行、编辑无损往返后路径保持稳定。

### 运行可视化事件 wire 契约

运行事件经 DataChannel `{"type":"se","ev":...}` 通道反向推送（engine `emit` → viewers 注册表 `control_dc`，定时任务运行同样生效），在现有 tap / swipe / hit / miss 之外新增：

| 事件 | 载荷 | 说明 |
|---|---|---|
| `run_start` | run 标识 | 运行开始 |
| `run_end` | 结果（ok / error） | 运行结束 |
| `step_start` | `{path, desc}` | 进入步骤 |
| `step_end` | `{path, ok, error}` | 步骤完成 / 失败 |
| `call_start` | `{target, depth}` | 进入 callable |
| `vision` | `{template, found, score, center, region}` | 匹配结果 |
| `budget` | `{kind}` | 预算终止原因（STEP_BUDGET_EXCEEDED / CALL_DEPTH_EXCEEDED / CANCELLED，见 ADR-YAML-04） |

- 事件**不带帧图像数据**；前端步骤高亮、match score / 变量展示、调用栈均由这些轻量事件驱动。

## 后果

- `match.click` / `find.click` / `click_when` 专用语法删除，前端编辑器与校验器同步移除对应表单与规则（改 YAML 引擎必须同步前端校验 / 模板 / 文档）。
- 匹配能力收敛为 `find` / `match_first` + 通用结果上下文：新增"命中后动作"不再扩 step，脚本可自由组合。
- 前端 ScriptSummary / 步骤画布可用 `path` 精确高亮运行中步骤、定位运行错误；事件契约成为编辑器与 Runtime 的公共接口，缺事件或错路径视为 Runtime bug。
