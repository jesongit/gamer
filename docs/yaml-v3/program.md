# Program 结构与函数库

> 本文定义 v3 可执行脚本（`automations/` 资源）的顶层结构与 v3 函数库
> （`functions/` 资源）的 bare-map 形态；实现核对基准为
> `server/src/extensions/gamer_yaml/yaml_vnext.rs`（`parse_surface` /
> `parse_function_library`）。

## 1. 脚本顶层结构

```yaml
version: 3                  # 必需；唯一接受的版本
params: []                  # 可选；参数声明（见 params.md）
defaults:                   # 可选；vision threshold / timing 兜底（见 timing.md、vision.md）
  vision:
    threshold: 0.80
  timing:
    after_tap: 300ms
    after_match: 200ms
    poll_interval: 100ms
steps:                      # 必需；顶层步骤列表
  - ...
```

- **顶层键白名单**：只允许 `version` / `params` / `defaults` / `steps`，未知键
  报 `yaml.v3.top_level.unknown_key`；缺 `steps` 报 `yaml.v3.steps.missing`。
- **`version: 3` 唯一**：缺失报 `yaml.v3.version.missing`，非 3（含
  `version: 2`）报 `yaml.v3.version`（"unsupported yaml version" 语义）。
  **无 fallback、无自动升级、无迁移工具**
  （[ADR-YAML-01](../reference/adr/ADR-YAML-01-v3-only.md)）。
- 判别规则：语法上合法的 `version: 3` 文档即为 v3（即使后续字段非法也按 v3
  出诊断，`yaml_vnext::is_v3_source`）；校验器不做版本猜测、不自动转换。

## 2. 函数库（functions/ 资源）

v3 函数库是 **bare-map**：顶层键全部是函数名，**没有 `version` 键**——目录即
类型（`functions/`），v3-ness 由步语法承载（ADR-YAML-02）。

```yaml
# functions/工具/月卡.yaml —— 多函数单文件
月卡领取:
  params:
    - 'text:account:账号'
  steps:
    - call:
        target: script:daily/login
        with: {account: $account}
    - return: {ok: true, count: 1}

签到:
  steps:
    - find:
        template: 签到按钮
        timeout: 5s
        then:
          - tap: {point: $match.center}
```

- 每个函数记录只允许 `params` / `steps` 两个键，`steps` 必需；函数名由映射键
  承载（映射键唯一，天然不允许同名重复定义）。
- 函数名规则：unicode 字母/数字/下划线（支持中文）、不能以数字开头、不得使用
  保留字（动作键 / 结构键 / `$match` 等，`yaml_vnext.rs`
  `RESERVED_FUNCTION_NAMES`）。违反报 `yaml.v3.function.name`。
- 结构诊断：空库报 `yaml.v3.function.file`；未知函数字段报
  `yaml.v3.function.unknown_key`；调用不存在的函数报
  `yaml.v3.function.not_found`；非映射顶层/非映射定义报 `yaml.v3.type`。
- 允许嵌套目录：`function:<文件短路径>/<函数名>` 的短路径可含 `/`（按最后一个
  `/` 分割，见 [call.md](call.md)）。
- 函数库无 `defaults` 块（bare-map 结构）：timing / threshold 一律走运行时
  内置兜底（见 [timing.md](timing.md)）。
- 保存边界（`resources.rs validate_function_library_file`）只接受 v3 bare-map
  单形态：非 v3 源（含 v2 存量形态）报 `yaml.v3.version` 诊断，无 fallback。

## 3. 资源寻址约定

- 脚本资源 id = `<package-id>/<相对路径>.yaml`（首段 = Package id，`[a-z0-9]
  [a-z0-9._-]*`，可与 Android 包名不同名；相对路径落在
  `plugins/gamer.yaml/automations/` 下，`automations/` 前缀由扩展内部映射）；
  call 命名空间 `script:` 引用时 `.yaml` 后缀可省略。
- 跨 Package 一律不解析：call 目标只在**当前 Package** 的 `automations/` /
  `functions/` 目录内寻址，经 Core PackageStore 按三元组
  (package_id, plugin_id, path) 直读，无 Installed/Editable 分层。
- 模板短名在当前 Package 内唯一匹配（`#` 后缀）；参数类型 `tmpl` 声明的是模板
  引用，模板重命名时服务端会同步改写脚本/函数内的引用
  （`rename_template_source` / `rename_template_in_function_library`）。

## 4. 与其他文档的衔接

- 参数声明语法与 schema API → [params.md](params.md)
- defaults（vision / timing）→ [vision.md](vision.md)、[timing.md](timing.md)
- 步骤语法 → [steps.md](steps.md)
