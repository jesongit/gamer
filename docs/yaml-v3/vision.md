# 视觉：find / match_first / check

> 本文定义 v3 模板匹配三步（find / match_first / check）的全语法、threshold
> 三级优先、match 结果 map 与 `$match` 上下文；权威裁决见
> [ADR-YAML-03](../reference/adr/ADR-YAML-03-match-context.md)，实现核对基准
> 为 `server/src/extensions/gamer_yaml/yaml_vnext.rs`（`parse_find` /
> `Lowerer::find` / `check` / `match_first`）与 `yaml_extension.rs`
> （`vision.match` / `vision.match_many`）。

## 1. find —— 轮询找模板 + 命中分支

```yaml
- find:
    template: reward          # 必填；模板名（分区唯一短名）
    timeout: 10s              # 可选；缺省 30min（1800s）
    threshold: 0.90           # 可选；step 级 override（三级优先见 §4）
    region: {x: 0.1, y: 0.2, width: 0.3, height: 0.4}   # 可选；相对坐标搜索区
    save: reward              # 可选；命中结果固化到命名变量，跨后续步骤可用
    then:                     # 命中后步骤组（唯一键名，无 block/steps/on_found 别名）
      - tap: {point: $reward.center}
    else:                     # 可选；超时后步骤组
      - log: 未找到
    verify:                   # 可选；then 执行完后二次验证
      template: home
      timeout: 5s             # 可选；缺省 30min
```

| 字段 | 必填 | 说明 |
|---|---|---|
| `template` | 是 | 模板名或 `$var` 引用 |
| `timeout` | 否 | 轮询上限；缺省 30min；每轮 sleep `poll_interval`（内置 100ms）。**建议写字面量时长**：`$var` 动态 timeout 在 lower 期无法折算迭代上限——运行期引用求值为时长类型值时按 100ms/轮近似折算，其余形态（如 time 参数过线后的字符串）无迭代上限、仅受步预算兜底（以实现为准 `Lowerer::poll_times` / guest `loop`） |
| `threshold` | 否 | 0~1；覆盖 defaults（见 §4） |
| `region` | 否 | `{x,y,width,height}` 或四元数组，分量 0~1 相对坐标 |
| `save` | 否 | 命中结果（match 结果 map）固化到命名变量 |
| `then` | 否 | 命中后步骤组 |
| `else` | 否 | 超时后步骤组 |
| `verify` | 否 | `{template, timeout?}` 二次验证 |

### 执行路径

- **命中**：save 固化 → sleep(`after_match`，内置 200ms) → `then` → `verify`
  （若设）→ 继续后续步骤。
- **超时**：有 `else` 走 `else`；无 `else` 抛 `FIND_TIMEOUT: <template>`（字面
  模板用原名，动态表达式退化为 "template"）。
- **verify**：在 `verify.timeout`（缺省 30min）内轮询 `verify.template`，不命中
  抛 `VERIFY_FAILED: <template>`——**不走 else**（verify 是确认操作生效，静默
  降级会掩盖异常）。verify 继承 find 的 threshold（step 值 > defaults 三级
  口径）。

## 2. match_first —— 多候选单帧匹配

```yaml
- match_first:
    candidates:               # `templates` 为别名键
      - template: reward
        threshold: 0.9        # 可选候选级 threshold
        steps:                # 候选命中后执行（唯一键名；体内 $match = 该候选结果）
          - tap: {point: $match.center}
      - template: close
        steps:
          - tap: {point: $match.center}
    else:                     # 可选；全未命中走 else，缺省静默继续
      - log: 都没出现
```

- 单帧 `vision.match_many`（候选级 threshold 经与 templates 平行的
  `thresholds` 列表传入，缺项用 defaults/内置兜底），按书写顺序**首个命中**
  候选执行自己的 `steps`。
- 候选可以是裸模板表达式（无 steps）。
- else 体内 `$match` = 整体结果 `{found, matches}`（found = 任一命中）。
- 顶层 `then` 不再支持（`yaml.v3.field.removed`）；候选 `click` 已删除。

## 3. check —— 轮询等出现

```yaml
- check: {template: ready, timeout: 30s, threshold: 0.85, throw: 界面没加载出来}
```

- 轮询 `template` 至出现（每轮 sleep `poll_interval`），命中后 sleep
  (`after_match`) 继续。
- 超时按 `throw` 文案结束运行（缺省「check 未命中」）；`timeout` 缺省 30min。

## 4. threshold 三级优先

```text
step 级 threshold  >  defaults.vision.threshold  >  Runtime 内置 0.80
```

- 前两级在 lower 期解析并注入 `vision.match` / `vision.match_many` 的 args
  （都缺省时省略字段，由 matcher 内置 `0.8` 兜底，`server/src/matcher.rs`）。
- `defaults.vision.threshold` 与 step 值都必须是 0~1 的数字（越界报
  `yaml.v3.defaults.range` / 运行期拒绝）。
- 函数库无 defaults 块，走 step 值 > 内置 0.80 两级。

## 5. match 结果 map

`find` 的 save/`$match`、match_first 候选 `$match` 引用的是同一个结果形态：

```yaml
{found: bool, score: number, x, y, width, height, center: {x, y}, region: {...}}
```

| 字段 | 说明 |
|---|---|
| `found` | 是否命中；未命中时结果 map 只有 `found` 与 `region` |
| `score` | NCC 得分 0~1（仅命中时） |
| `x` / `y` / `width` / `height` | 命中框原始数值（matcher 返回的帧像素口径，整数；脚本一般不直接使用，以实现为准 `yaml_extension.rs match_value`） |
| `center` | **命中框中心，相对坐标 `[x, y]`（0~1）**——tap/swipe 用它（`tap: {point: $match.center}`） |
| `region` | 本次搜索区域回显（相对坐标 map；未给 region = 全帧 `{x:0, y:0, width:1, height:1}`） |

`$match` 作用域规则（块内可见 / 块后复位 / save 跨步）见
[expressions.md](expressions.md)。

## 6. 已删除的 click 语法族

v2 `match.click`、v3 `find.click`、候选 `click`、`click_when` 全族删除
（ADR-YAML-03）：命中后动作统一用 `then`/候选 `steps` + `tap: {point:
$match.center}` 表达。诊断与迁移对照见 [steps.md](steps.md) §10。
