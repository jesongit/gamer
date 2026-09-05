# 表达式与变量

> 本文定义 v3 的取值表达式：`$` 变量引用、属性路径、字面量形态、`$match`
> 上下文作用域规则与真值/相等判定；实现核对基准为
> `server/src/extensions/gamer_yaml/yaml_vnext.rs`（`Expr` / `expr_from_yaml`）
> 与 `server/guests/yaml-guest/src/lib.rs`（`evaluate` / `lookup_path` /
> `truthy` / `values_equal`）。

## 1. 表达式形态（刻意小）

v3 的表达式**刻意不是通用表达式语言**：没有算术运算、没有函数调用、没有字符串
拼接。字段取值只有四种形态：

| 形态 | 例子 | 说明 |
|---|---|---|
| 字面量 | `300ms`、`0.9`、`"文本"`、`true`、`null` | 类型化字面量（见 §2） |
| 变量/路径引用 | `$reward.center`、`$list[0]` | `$` + 属性路径（见 §3） |
| 列表 | `[0.5, 0.5]`、`[$a, $b]` | 元素递归求值 |
| 映射 | `{account: $user}` | 值递归求值 |

需要计算/比较逻辑时，用 `set` / `if` 步骤组合表达，而不是扩表达式语法。

## 2. 字面量与类型

| YAML 书写 | 运行时类型 | 备注 |
|---|---|---|
| `300ms` / `2s` / `1m` / `1h` | Duration（时长字段位置） | 仅在时长位置（wait/duration/timeout 等）解析为单位串；单位非法报 `yaml.v3.duration`；纯数字 = 毫秒 |
| `[0.5, 0.3]` | Coordinate | **恰好两个数值元素的数组被定型为坐标**（见 §6 注意事项） |
| `0.9` / `3` | Float / Int | |
| `文本` / `"文本"` | String | |
| `true` / `false` | Bool | |
| `null` / 空 | Null | |
| `#1a2b3c` 类颜色 | 由 capability 返回（Color） | 脚本侧颜色值一般来自 `vision.sample_color` 结果 |

时长书写错误报 `yaml.v3.duration`；普通值形态非法报 `yaml.v3.value`。

## 3. 变量与属性路径

- 变量来源：`params`（运行入参，缺省用声明默认值）、`set` 步骤、`save`、
  `$match` 上下文。
- 路径语法：`$名字`、`$名字.字段`、`$列表[0]`、可组合
  `$m.matches[1].center`——段按 `.` 分割，每段可带 `[索引]` 后缀。
- 求值时自动穿透 typed wire 容器（guest `typed_map_get` / `typed_list_get`），
  脚本作者无需关心 wire 编码。
- **引用未定义变量按运行错误终止**（"未定义变量 $x"），不静默取 null。
- `$match` 是保留上下文名（函数名不得占用，见 [program.md](program.md)）。

## 4. `$match` 上下文规则（ADR-YAML-03）

匹配结果是通用 runtime value（形态见 [vision.md](vision.md)），`$match` 指向
**最近一次** find / match_first 的匹配结果：

- **块内可见**：未 `save` 时 `$match` 仅在对应 find 的 then/else/verify 与
  match_first 候选 `steps` 体内可见。
- **块后复位**：find / match_first 块结束时 `$match` 复位为 null——上下文以块
  为界，不跨块泄漏（lower 在块尾显式 `set match = null`）。
- **save 固化跨步**：`save: reward` 把结果整体镜像到命名变量，后续任意步骤可
  用 `$reward.center`；`$match` 本身仍按块作用域复位。
- match_first 的 **else 体内** `$match` = 整体结果 `{found, matches}`（found =
  任一候选命中）；候选 `steps` 体内 `$match` = 该候选自己的结果。
- find 超时走 else 时 `$match` = 最后一次未命中结果（`{found: false, region}`）。

## 5. truthy 与相等（if 条件口径）

`if.cond` 按通用值语义判定（guest `truthy`）：

| 值 | truthy |
|---|---|
| `null` | false |
| `false` | false；`true` → true |
| Int / Float | `≠ 0` |
| String / Color | 非空串 |
| Duration | `≠ 0ms` |
| Coordinate / Handle | 恒 true |
| List | 非空 |
| **Map** | **有 `found` 键时按其值 truthy**（match 结果 map 可直接当条件）；否则非空 map → true |

lower 把 `cond` 收敛为 `truthy` 条件；小 AST 另有 `equals` / `not` 条件（内部
生成，如 find 的超时判定）。`equals` 口径（`values_equal`）：Int/Float 互通；
Color 忽略大小写与前导 `#`；其余按结构相等。

## 6. 已知语义注意：两元素数字数组被定型为 Coordinate

YAML 中**恰好两个数值元素**的数组（如 `[0.5, 0.3]`）在解析期被定型为
`Coordinate` 标量，而不是普通列表（`expr_from_yaml`；Phase 12 T2 报告记录的
已知语义）。影响：

- 用它做 `tap` / `swipe` / `vision.sample_color` 的坐标实参是预期用法；
- 它的 truthy 恒为 true、不能按列表下标取元素——**把二元数组当普通数据传
  递**（例如希望得到 `[x, y]` 列表再用 `[0]` 取分量）不会按列表语义工作；
- 三元及以上数组保持普通 List。

写脚本时若需要"二元数据列表"语义，改用映射 `{x: 0.5, y: 0.3}` 承载。

## 7. 变量作用域小结

| 作用域 | 规则 |
|---|---|
| 程序参数 | run 级：整个脚本/函数执行期可读写（`set` 覆盖不影响调用方） |
| call 被调方 | 独立变量空间：只有传入实参 + 自身声明默认值（见 [call.md](call.md)） |
| `$match` | 块作用域（find/match_first 块内），块尾复位 null |
| `save` 变量 | 从赋值点起在当前帧内持续可用 |
