# ADR-YAML-02：call 统一可调用资源

> 编号说明：ADR-01~14 是全局架构决策序列（Phase 11 收口产出）；ADR-YAML-xx 是 YAML 域专项 ADR 序列（命名见计划 §5.5），记录 gamer_yaml 扩展 DSL / Runtime 的最终语义裁决，与全局序列互不续号。
>
> 关联计划：`docs/plans/gamer_yaml_v3_finalization_v2_removal_plan.md`（§5.3 func 最终裁决、§7 P12.2 Function 统一并入 call）。

状态：ACCEPTED（2026-09-05）

## 背景

v2 用专用 `func` step 调用函数库，target 为裸 `文件/函数` 字符串，脚本调用与函数调用入口分裂；函数返回值被钉死为 bool；资源寻址存在绕过统一资源层的旁路。v3 把「可调用」抽象为通用 `call` + 显式命名空间：脚本、函数与未来形态共用同一 step、同一解析机制（计划 §22 规则 2：优先增加通用 call，而不是 `func` / `script_call` / `workflow_call` 等专用 step）。

## 决策

### target 必须带显式命名空间

- `script:<资源id>`：资源 id = 分区内 `scripts/` 相对路径去 `.yaml` 后缀。例：`script:daily/login`。
- `function:<文件短路径>/<函数名>`：文件 = 分区内 `functions/` 下 `<文件短路径>.yaml`；函数名取最后一段——文件短路径本身可含目录，按最后一个 `/` 分割。例：`function:工具/月卡领取`；`function:common/login/is_logged_in` 指向文件 `common/login.yaml` 中的 `is_logged_in`。
  - 路径语法沿用 v2 `split_func_path` 的路径语法与穿越校验并推广：拒绝 `..` 段、绝对路径、反斜杠；两段（文件短路径、函数名）均非空。
- **裸 target（无前缀）不再接受**，校验报明确诊断（指出缺少 namespace 前缀及合法取值）。

### 函数库文件形态

- 保持**多函数单文件** bare-map：`{<函数名>: {params, steps}}`。
- 函数文件**无 `version` 键**——目录即类型（`functions/`），v3-ness 由步语法承载；校验按 v3 步语法解析函数内 steps。

### 返回值泛化

- `return` 可返回 null / bool / number / string / object / array 任意 JSON 值。
- `call` 的 `save` 把返回值整体存入变量；被调方无 `return` 即存 null。
- 删除「函数默认返回 bool」约束；`if` 条件等使用处按通用值语义判断。

### 解析唯一入口

```text
target
  ↓ parse namespace（script: / function:）
ResourceResolver（Core ResourceStore composite：EditableLocal → UserOverride → InstalledPackage）
  ↓ load resource（脚本走 scripts/ 目录、函数走 functions/ 目录各自寻址）
v3 parse
  ↓ 参数绑定（args 按目标 params 校验 / 重定型）
execute
```

- 脚本与函数只经 ResourceResolver 解析，不新增旁路文件读取；本地编辑区资源与 App Package 内资源对 `call` 透明（包内函数照常可被本地脚本调用，反之亦然）。
- 未来 `workflow:` / `macro:` / `plugin:` 命名空间预留，走同一 `call` 机制——新增 namespace 只扩 target parser + resolver 适配，不新增 step 类型。

### 递归调用深度

- 每进入一层 callable 深度 +1、返回 -1，上限 **32**。
- 本期先在 resolver 层做临时深度守卫；P12.4 ExecutionBudget 落地后由 guest 统一计数，resolver 守卫由正式预算取代（数值与正式预算一致，见 ADR-YAML-04）。

## 后果

- `func` step 删除（P12.2）；v2 裸 target 函数调用需改写为 `function:` 前缀形式，校验器不再放行无前缀写法。
- `call` 成为唯一可调用资源入口：新增执行形态不引入新 step，target namespace 即扩展点。
- 函数库从「布尔判定」泛化为通用子程序：可返回结构化对象 / 数组，配合 `save` 直接驱动后续步骤。
- 前端 call/func 卡片目标下拉（分区候选、两级函数选择）按 `script:` / `function:` 语法生成 target，编辑器与 Runtime 共用同一 target 语法。
