# Gamer 函数体系与插件依赖简化开发计划

> 状态：**已实施**（2026-09-09；执行记录与验收见 §11）
> 项目：`jesongit/gamer`
> 适用阶段：开发期，允许必要的破坏性修改，不新增旧语法或旧目录兼容层
> 目标：简化函数库资源组织，明确 Core 原语与插件函数边界，保留统一函数调用，并建立最小插件依赖与可选功能降级机制。

## 1. 最终设计原则

本轮只针对函数体系、资源发现和插件依赖进行收口，不重新设计整个插件架构，也不重新实现现有 YAML 解释器。

1. 面向用户的函数来源只有两种：**插件函数和 Package 函数**。
2. Core 只提供通用能力，不建立第三套用户函数库。
3. 插件函数随插件发布，不要求独立的全局函数目录。
4. 每个 Package 默认只有一个用户函数库文件：`_function.yaml`。
5. 前端函数编辑器只编辑当前 Package 的默认 `_function.yaml`。
6. 文件名以 `_function` 开头、以 `.yaml` 结尾的文件均识别为函数库，作为手动拆分和未来扩展的加载规则。
7. 不要求函数库独占文件夹，不增加 `kind` 等专门用于区分资源类型的字段。
8. 目录和文件名只负责组织资源，不自动产生函数命名空间。
9. 保留当前唯一解释器和统一函数调用方式，不恢复旧 YAML DSL。
10. 插件依赖只支持必需和可选两种，不建设复杂依赖管理器。
11. 可选依赖缺失时，插件基础功能继续可用，相关功能根据实际能力降级。
12. 开发阶段允许清理旧目录、旧资源格式和重复实现，不保留无实际需求的双轨兼容。

---

## 2. Phase 0：核对最新实现与锁定契约

### 目标

以当前工作树为准确认已经完成的实现，避免依据过期计划重复开发或覆盖新代码。

### 任务

* [ ] 记录当前分支、HEAD、工作树状态和未提交修改。
* [ ] 阅读最新 `AGENTS.md`、YAML 权威设计文档、V1 重构计划及实施报告。
* [ ] 核对当前唯一解释器、函数解析器、函数注册表、Package 函数加载和运行调用链。
* [ ] 核对 `native_funcs.rs` 或当前等价模块中的全部函数及实际 handler。
* [ ] 核对当前 Package 脚本资源根、默认函数库文件、资源扫描、编辑器和导入导出逻辑。
* [ ] 核对插件 manifest、安装、启用、启动、停用、更新和卸载流程。
* [ ] 核对现有 `plugin.call`、YAML 公开 action、UI Bridge、WIT 和 Host API。
* [ ] 确认当前正式 YAML 版本及函数定义格式，不根据旧文档恢复 v3 或旧 `call` 语法。
* [ ] 列出已经完成、需要修改、可以删除和仍需验证的内容。

### 验收

形成简短基线报告，明确实际代码入口、唯一权威文档和本轮最小改动范围。不得为了执行本计划重新创建已有解释器、资源服务或插件运行时。

---

## 3. Phase 1：默认单文件函数库与前缀识别

### 目标

让普通用户只需要编辑一个默认函数库，同时允许高级用户手动拆分文件，避免未来单个文件过大。

### 3.1 默认函数库

每个 Package 的 YAML 插件默认函数库文件为：

```text
_function.yaml
```

前端“函数”页面只编辑这个文件。

默认行为：

* [ ] 首次进入函数编辑功能时，若默认文件不存在，则按当前资源创建机制创建。
* [ ] 不预置多个空函数库文件。
* [ ] 不新增函数库列表、分类树或多文件管理器。
* [ ] 不要求用户理解函数库文件的扫描和加载规则。
* [ ] 默认文件可以定义多个函数，使用当前正式 V1 函数定义格式。
* [ ] 保存、校验、补全和错误提示复用现有 YAML 编辑器能力。

### 3.2 文件识别规则

在 YAML 插件的脚本资源空间内，采用以下规则：

```text
文件名以 _function 开头，且扩展名为 .yaml
    → 函数库

其他 .yaml 文件
    → 自动化
```

例如：

```text
_function.yaml
_function_common.yaml
_function_battle.yaml
_function2.yaml
```

以上均识别为函数库。

```text
daily.yaml
login.yaml
battle.yaml
```

以上识别为自动化。

第一版只使用小写 `_function` 前缀和 `.yaml` 扩展名，不额外增加大小写别名或 `.yml` 双格式规则。

**识别规则只用于资源发现，不代表前端需要支持创建和管理多个函数库。**

### 3.3 目录组织

函数库与自动化共用当前 YAML 插件的脚本资源空间，不要求 `functions/` 专属目录。

建议的最小结构：

```text
packages/
└── <package-id>/
    └── plugins/
        └── gamer.yaml/
            ├── scripts/
            │   ├── daily.yaml
            │   ├── login.yaml
            │   └── _function.yaml
            └── templates/
                └── ...
```

`<package-id>` 使用项目当前实际的 Package ID，不要求等于 Android 包名。

上面的 `scripts/` 只是统一脚本资源根的示意，实施时优先沿用当前仓库已有结构，不为了改名进行无意义搬迁。

不新增 `tools/` 等示例分类目录，也不要求用户创建文件夹管理函数。

如果用户以后确实需要拆分，可以手动添加：

```text
scripts/
├── _function.yaml
├── _function_battle.yaml
└── _function_common.yaml
```

这些文件正常参与加载，但前端仍只提供默认 `_function.yaml` 的函数编辑入口。其他文件可以由用户通过外部编辑器或现有通用资源操作维护，本轮不为此开发额外 UI。

### 3.4 文件内容与函数命名

文件名负责资源发现，文件内容继续使用当前正式 V1 的函数定义格式。

不增加专门用于识别文件类型的：

```yaml
kind: library
```

也不要求普通自动化增加：

```yaml
kind: automation
```

如果当前 V1 已有必要的版本、函数定义或自动化结构字段，应保留原有语义，本轮不重新设计 YAML 格式。

所有当前 Package 函数使用统一命名空间。目录和文件名不自动影响函数调用名。

例如 `_function_battle.yaml` 中定义 `attack`，调用仍为 `attack`，不强制变成 `battle.attack`。

要求：

* [ ] 默认文件与额外 `_function*.yaml` 均正常加载。
* [ ] 一个函数库文件允许定义多个函数。
* [ ] 函数库文件不进入自动化运行列表或定时任务选择器。
* [ ] 不跨 Package 隐式查找函数。
* [ ] 同名函数不得静默覆盖，加载时返回明确冲突诊断。
* [ ] 文件扫描顺序不得决定同名函数的最终实现。
* [ ] 加载失败应标明具体文件、函数和错误位置。
* [ ] 保留当前函数缓存、运行预算和取消机制，不因前缀扫描引入重复解析。

### 3.5 资源操作与迁移

* [ ] 默认函数编辑器使用固定资源路径，不需要扫描后让用户选择文件。
* [ ] 资源扫描按前缀识别其他手动添加的函数库。
* [ ] Package 导入导出保留脚本资源结构。
* [ ] Core 继续只负责 PackageStore 三元组资源寻址，不解析 YAML 业务内容。
* [ ] 删除旧 `functions/` 专属目录语义及不再使用的类型判断代码。
* [ ] 开发阶段不保留旧目录兼容层；已有开发数据需要时提供一次性备份和手动迁移说明。

### 验收

普通用户进入“函数”页面，只看到默认 `_function.yaml` 编辑器；默认文件可正常定义和调用函数。手动添加的 `_function_common.yaml` 等文件能够正常加载，但不需要额外前端管理功能。普通 YAML 自动化不受影响。

---

## 4. Phase 2：Core 原语与插件函数归属收口

### 目标

明确底层能力和便利函数的边界，避免 `wait_find` 等组合业务进入 Core，同时保留当前单一解释器。

### 4.1 Core 只提供通用能力

Core 继续负责：

```text
Input / Touch
    点击、滑动、按键、文本输入等底层设备操作

Frame / Vision
    获取帧、单次模板匹配、颜色采样等

Runtime
    可取消等待、日志、运行上下文

Device
    应用启动、停止及设备状态

Resource
    Package 授权资源读写

Media
    媒体、录制、确定帧等通用能力
```

Core 不负责解释 YAML 函数库，也不提供 `wait_find`、`tap_template`、`daily_login` 等自动化业务函数。

### 4.2 基础函数

`gamer.yaml` 对 Core 能力提供面向脚本的基础函数包装，例如：

```text
tap
swipe
key
input_text
launch
stop_app
sleep
log
find
```

这些函数属于 YAML 插件的函数接口，不是 Core 的第三套用户函数库。

可以继续使用 Rust 实现，不要求为了插件化全部改写为 YAML。关键是业务归属清晰，底层能力通过现有受控 Host API 调用。

其中建议明确：

```text
find
    → 单次模板匹配

wait_find
    → 重复调用 find，直到匹配成功或超时
```

不再让 `find` 同时承担单次匹配和等待轮询两种语义。实施时应统一检查现有参数、返回值、阈值、坐标和错误处理，避免拆分后产生两套不一致的匹配逻辑。

### 4.3 插件便利函数

`wait_find`、`wait_disappear`、`tap_template` 等归属 YAML 插件便利函数库。

建议组合关系：

```text
wait_find
    → find
    → sleep
    → 循环 / 超时 / 取消

wait_disappear
    → find
    → sleep
    → 循环 / 超时 / 取消

tap_template
    → find 或 wait_find
    → tap
```

便利函数优先复用已有基础函数和解释器能力。确实需要 Rust 实现时，可以保留插件内部 handler，但不能复制 Core 视觉算法，也不能绕过 Host API。

不强制所有便利函数必须写成 YAML。实现语言由实际复杂度和性能需求决定。

### 4.4 纯数据函数

`eq`、`ne`、`gt` 等比较或纯数据函数继续属于 YAML 插件基础函数。

不为了“原语拆分”把所有纯函数搬进 Core，也不引入新的表达式语言或数学函数框架。

### 4.5 函数来源与统一调用

最终面向用户只有：

```text
插件函数
    ├── gamer.yaml 基础函数
    ├── gamer.yaml 便利函数
    └── 其他插件公开函数（如适用）

Package 函数
    └── 当前 Package 的 _function*.yaml
```

Core 原语是插件使用的底层能力，不作为第三种用户函数来源。

要求：

* [ ] 保留当前唯一解释器。
* [ ] 函数新增不需要修改解释器语法。
* [ ] 基础函数、便利函数和 Package 函数使用统一调用机制。
* [ ] 函数声明继续作为参数、返回值、权限和帮助信息的共同来源。
* [ ] 同名冲突使用明确规则或诊断，不静默覆盖。
* [ ] 保留运行预算、取消、递归深度和权限检查。
* [ ] 删除旧的重复 handler、旧步骤分发及不再使用的兼容代码。

### 验收

`tap`、`find` 等基础函数可以独立调用，`wait_find` 等便利函数通过插件内部组合实现，Package 函数可以调用两者。Core 不新增自动化业务函数，解释器仍只有一套。

---

## 5. Phase 3：最小插件依赖机制

### 目标

允许插件声明必需和可选依赖，同时让缺少可选依赖的插件保留基础功能。

### 5.1 Manifest 声明

在现有插件 manifest 中增加最小依赖字段。以下为建议结构，实际字段及版本格式以当前 manifest schema 为准：

```toml
[[dependencies]]
id = "gamer.yaml"
version = ">=1.0.0"
required = false
```

只需要：

* `id`：目标插件 ID。
* `version`：兼容版本要求。
* `required`：是否为必需依赖。

不增加独立 lockfile、自动下载地址、加载优先级、复杂 feature graph 或依赖管理后台。

### 5.2 必需依赖

当 `required = true`：

* 安装时可以允许包落盘，但必须明确提示依赖缺失。
* 启用或启动前检查依赖是否安装、版本兼容且可用。
* 条件不满足时阻止启动，并返回结构化错误。
* 不自动下载、安装或启用其他插件。
* 用户通过现有插件中心手动处理依赖。

例如：

```text
插件 A → 必需依赖 B

B 未安装 / 未启用 / 版本不兼容
    → A 不能启动
    → 展示具体原因
```

### 5.3 可选依赖

当 `required = false`：

* 依赖缺失不阻止插件启动。
* 插件基础功能继续可用。
* 依赖对应功能入口显示不可用原因。
* 依赖安装、启用或更新后，重新计算相关能力可用性。
* 依赖被停用或卸载后，对应功能降级，不要求重启整个 Gamer。

例如：

```text
gamer.video
    ├── 录制 / 导入 / 播放 / 逐帧 / 标记
    │      → 不依赖 gamer.yaml
    │
    └── 创建模板 / 生成 YAML 草稿 / 打开 YAML 编辑器
           → 可选依赖 gamer.yaml
```

缺少 YAML 插件时，视频工作台仍可正常使用媒体功能，只禁用 YAML 相关制作入口。

### 5.4 生命周期与依赖检查

复用现有插件生命周期，不新建第二套状态机。

要求：

* [ ] 检查依赖 ID、版本和实际可用状态。
* [ ] 必需依赖形成的循环应拒绝并返回清晰错误。
* [ ] 可选依赖不触发递归自动启动。
* [ ] 停用被运行中插件必需依赖的插件时，提示先停止依赖方，不做意外的自动级联停用。
* [ ] 可选依赖停用后，新调用应立即不可用。
* [ ] 已开始的调用按现有取消和生命周期规则处理。
* [ ] 插件更新遵守现有版本切换和停止规则。
* [ ] 依赖声明不能自动授予任何权限。

### 5.5 与 Package 依赖分离

保持两层依赖：

```text
插件依赖
    → 插件 A 是否需要插件 B
    → 决定插件启动和部分能力可用性

Package 依赖
    → 某个配置包运行需要哪些插件
    → 决定该 Package 的功能和运行条件
```

例如，视频插件对 YAML 是可选依赖，但某个游戏配置包可以要求 YAML 必须安装。两者不矛盾。

### 验收

必需依赖缺失时插件不能启动；可选依赖缺失时基础功能正常；依赖恢复后对应功能可用；不存在自动下载、自动启用或复杂依赖解析流程。

---

## 6. Phase 4：最小通用跨插件能力调用

### 目标

让插件能够公开自己的能力，其他插件通过 Core 受控调用，不再为每一组插件增加专用 ID 分支。

### 6.1 复用现有机制

先核对现有 `plugin.call`、YAML action、Host API 和 UI Bridge。已有等价能力时直接扩展，不重新设计 RPC 框架。

最小模型：

```text
插件 A
    → 查询目标插件公开能力
    → 检查目标版本、运行状态和权限
    → Core 受控分发
    → 插件 B 执行
    → 返回结果或结构化错误
```

### 6.2 公开能力声明

插件可以声明对外提供的能力，例如：

```text
gamer.yaml
    template.create_from_frame
    vision.test_template
    automation.create_draft
    automation.save_draft
    automation.open_editor
```

能力声明至少包含名称、参数、返回值和必要权限。优先复用现有函数或 action schema，不建设第二套重复类型系统。

插件版本要求与实际能力存在性共同决定是否可调用。当前没有实际需求时，不额外引入独立的能力版本管理体系。

### 6.3 依赖与能力检查的关系

依赖声明只表达插件关系，不能代替运行时能力检查。

例如：

```text
gamer.video
    → 查询 gamer.yaml 是否提供 template.create_from_frame
    → 可用：显示创建模板入口
    → 不可用：禁用入口并说明原因
```

即使 YAML 插件已经安装，也不能假设所有能力都存在。

可选依赖的功能降级由调用插件根据实际能力决定，不要求 Core 建立复杂的功能依赖图。

### 6.4 权限与隔离

跨插件调用不能成为权限绕过通道。

* [ ] 调用方只能调用目标插件明确公开的能力。
* [ ] 服务端根据真实安装实例和认证上下文确定调用者身份。
* [ ] 不信任前端自行提交的 `plugin_id`、`official`、`permission_confirmed` 等字段作为唯一授权依据。
* [ ] 被调插件仍需检查自身权限、Package、设备和资源上下文。
* [ ] 不允许通过被调插件绕过调用方原本无权执行的操作。
* [ ] 不允许直接读写其他插件私有目录。
* [ ] 不允许通过能力调用加载任意 DLL、执行文件或宿主私有 Rust 函数。
* [ ] 调用失败返回结构化诊断，不使用静默 fallback。

### 6.5 不做的内容

不做分布式 RPC、服务发现中心、跨进程消息总线、动态依赖下载、复杂调用编排或全局服务容器。

### 验收

使用一个简单测试插件证明：插件 A 可以声明公开能力，插件 B 不修改 Core 专用分支即可发现和调用；依赖缺失、版本不符、权限拒绝、目标停用均有明确结果。

---

## 7. Phase 5：函数编辑器与插件体验收口

### 目标

保持默认函数编辑体验简单，同时让用户清楚区分函数来源，并理解依赖导致的功能不可用。

### 7.1 默认函数编辑器

YAML 插件继续使用自己的工作区，不新增系统级全局函数库页面。

前端“函数”入口只对应：

```text
当前 Package
    → gamer.yaml
    → _function.yaml
```

要求：

* [ ] 打开“函数”时直接打开默认文件。
* [ ] 默认文件不存在时按当前资源机制创建。
* [ ] 不增加函数库文件选择器。
* [ ] 不增加函数库分类树或多文件管理器。
* [ ] 不为了前缀识别规则增加创建多个函数库的 UI。
* [ ] 编辑、保存、校验和补全复用现有 YAML 编辑器。
* [ ] 手动添加的其他 `_function*.yaml` 正常加载，但不要求前端提供专门编辑入口。

### 7.2 可用函数展示

在 YAML 插件工作区统一展示：

```text
可用函数
    ├── 基础函数
    ├── 插件便利函数
    ├── 其他插件公开函数（如适用）
    └── 当前 Package 函数
```

分类只是 UI 展示，不对应独立全局存储目录。

要求：

* [ ] 显示函数名称、来源、说明、参数、返回值和权限。
* [ ] 支持必要的搜索或来源筛选，优先复用现有 UI。
* [ ] 当前 Package 默认函数可以直接编辑。
* [ ] 插件函数只读展示，不误写入插件安装包。
* [ ] 调用补全和参数提示复用统一函数 schema。
* [ ] 同名冲突给出明确诊断。

不建设复杂的函数库管理界面。

### 7.3 依赖提示

插件中心和插件面板应显示：

* 缺少哪个依赖。
* 当前版本与要求版本。
* 是必需依赖还是可选依赖。
* 哪些功能因此不可用。
* 如何通过现有插件中心安装、启用或更新。

不新增独立依赖管理后台。

### 验收

普通用户只需要编辑 `_function.yaml` 即可编写和调用 Package 函数；插件函数无需复制到 Package；可选依赖缺失时能够明确看到不可用功能及恢复方式。

---

## 8. Phase 6：回归测试、文档与清理

### 8.1 函数与资源测试

* [ ] 默认 `_function.yaml` 正常创建、编辑、保存和加载。
* [ ] `_function_common.yaml`、`_function_battle.yaml` 等额外文件正确识别为函数库。
* [ ] 同一目录多个函数库文件正常加载。
* [ ] 普通 `.yaml` 文件仍识别为自动化。
* [ ] 函数库文件不进入任务运行列表。
* [ ] 一个函数库文件定义多个函数。
* [ ] 手动添加的函数库可以正常调用。
* [ ] 移动或重命名额外函数库文件不改变函数调用名。
* [ ] 同名函数冲突明确报错，不依赖扫描顺序覆盖。
* [ ] Package 函数调用基础函数、便利函数及其他 Package 函数。
* [ ] 函数缺失、非法参数、递归深度、超时和取消处理正确。
* [ ] Package 导入导出保留资源结构。
* [ ] 旧专属目录语义删除后不存在隐式回退或双格式解析。
* [ ] 前端默认函数编辑器不因额外函数库文件存在而变复杂。

### 8.2 基础与便利函数测试

* [ ] `tap`、`swipe`、`key` 等基础函数调用与权限正确。
* [ ] `find` 为单次匹配。
* [ ] `wait_find`、`wait_disappear`、`tap_template` 的组合、超时和取消正确。
* [ ] 比较及纯数据函数正常。
* [ ] 不存在重复视觉算法或重复设备输入实现。
* [ ] 统一解释器与现有运行预算正常。

### 8.3 插件依赖与跨插件测试

* [ ] 必需依赖缺失、版本不兼容、未启用。
* [ ] 可选依赖缺失时基础功能正常。
* [ ] 依赖恢复后功能重新可用。
* [ ] 必需依赖循环。
* [ ] 停用或更新被依赖插件时的生命周期处理。
* [ ] 跨插件能力不存在、权限拒绝、目标停用。
* [ ] 普通插件不能冒充 builtin 或调用未公开的宿主能力。
* [ ] 无签名本地插件仍可正常安装，依赖机制不引入签名要求。

### 8.4 视频工作台回归

至少验证：

```text
无 YAML 插件
    → 导入、录制、播放、逐帧、标记正常

安装并启用 YAML
    → 模板制作、草稿生成与编辑入口可用

停用 YAML
    → 视频基础功能继续正常
    → YAML 相关入口降级
```

同时回归 YAML、Keymap、任务调度、Package、插件市场和插件更新，不因函数体系调整破坏已有功能。

### 8.5 文档与清理

更新 README、AGENTS.md、YAML 正式参考、插件 manifest/SDK 文档、函数开发指南和插件依赖说明。

文档必须明确：

1. 函数来源只有插件函数与 Package 函数。
2. Core 原语不是第三套用户函数库。
3. Package 默认函数库为 `_function.yaml`。
4. 前端只编辑默认函数库，不提供多文件函数库管理。
5. `_function*.yaml` 是加载识别规则，用于手动拆分和未来扩展。
6. 函数库不要求专属目录。
7. 不使用 `kind` 字段判断函数库与自动化。
8. 目录和文件名不自动产生函数命名空间。
9. 基础函数与便利函数都属于插件，底层能力由 Core 提供。
10. 插件依赖与 Package 依赖是不同概念。
11. 可选依赖缺失不应阻止插件基础功能。
12. 免签名不等于免权限或允许任意 Native 代码。
13. 当前唯一正式 YAML 版本和调用语法是什么。

删除或标记过时的旧 v3、旧函数目录、旧资源类型声明及旧调用方式说明，避免文档同时宣称多套正式方案。

---

## 9. 实施顺序与约束

建议顺序：

```text
Phase 0  最新基线核对
    ↓
Phase 1  默认 _function.yaml 与前缀识别
    ↓
Phase 2  Core 原语与插件函数收口
    ↓
Phase 3  最小插件依赖
    ↓
Phase 4  通用跨插件能力调用
    ↓
Phase 5  前端与开发体验
    ↓
Phase 6  回归、文档、清理
```

Phase 3 和 Phase 4 可以结合实际代码一起实施，但不要为了依赖机制提前建设复杂 RPC 框架。

每阶段要求：

* 先确认当前真实代码和接口。
* 优先复用现有实现，不重复造轮子。
* 采用“接口 → 实现 → 前端 → 测试 → 文档”的纵向闭环。
* 开发阶段允许破坏性修改，不保留旧语法、旧目录或旧函数分发的双轨兼容。
* 涉及已有开发数据时，提供明确的备份和手动迁移说明。
* 不强制将所有 Rust 便利函数改写成 YAML。
* 不因为前缀识别规则增加多文件管理 UI。
* 不新增与当前需求无关的配置项、状态机、目录或管理页面。
* 每阶段记录实际测试结果，未执行的测试不得写成通过。

## 10. 最终验收标准

* [ ] Core 只提供通用能力，不承担自动化便利函数业务。
* [ ] YAML 保留唯一解释器和统一函数调用方式。
* [ ] `tap`、`find` 等基础函数与 `wait_find` 等便利函数归属清晰。
* [ ] 插件函数随插件发布，不需要独立全局函数库目录。
* [ ] Package 默认只有一个 `_function.yaml` 函数库文件。
* [ ] 前端函数编辑器只编辑默认 `_function.yaml`。
* [ ] `_function*.yaml` 作为扩展加载规则正常工作。
* [ ] 不要求函数库专属目录，不增加多文件函数库管理 UI。
* [ ] 一个函数库文件可以定义多个函数。
* [ ] 不使用 `kind` 字段判断资源类型。
* [ ] 目录和文件名不自动影响函数调用名。
* [ ] 用户可以编写、编辑、调用和导出当前 Package 函数。
* [ ] 插件支持必需依赖与可选依赖。
* [ ] 可选依赖缺失时，插件基础功能仍可使用。
* [ ] 跨插件调用通过受控公开能力完成，不依赖不断增加的插件 ID 特判。
* [ ] 不自动下载或安装依赖，不引入复杂依赖管理器。
* [ ] 现有 YAML、Keymap、Video、任务和 Package 功能回归通过。
* [ ] README、AGENTS.md 和正式参考文档与当前实现一致。
* [ ] 旧函数目录语义、重复 handler 和不再使用的兼容代码已清理。

**最终原则：函数是插件提供的能力，Package 函数是用户自己的配置；Core 只提供机制。默认函数库只有一个文件，前缀规则只负责扩展加载，不应该反过来增加用户的管理负担。**

---

## 11. 执行记录（Phase 0 基线报告，2026-09-09）

> 分支 `main`，HEAD `dd2eb98`，工作树干净（仅本计划文档新增）。

**已完成、直接复用（不重建）**：

- 唯一解释器 `server/guests/yaml-interp`（run=函数调用/if/repeat/return，两来源函数表运行期冻结）+ 宿主降线 `gamer_yaml/syntax.rs`（V1 无 version 字段，旧源报 `yaml.version.removed`）。
- 原生函数注册表 `native_funcs.rs`：17 个函数（tap/swipe/key/input_text/launch/stop_app/sleep/log/find/wait_find/tap_template/wait_disappear/eq..le），Schema+权限唯一声明点；执行宿主 `yaml_extension.rs` NativeYamlHost（`__fn` 通道 = Schema 校验+权限+capability 组合）。全部函数已归插件（gamer.yaml），Core 无业务函数——Phase 2 只需收口 `find` 双语义。
- 函数注册表组合：`runner_adapter.rs::compose_function_library`（原生 ∪ 当前 Package，同名冲突拒绝、不跨包）；运行预算/取消/递归深度内建于解释器。
- Package 资源：`resources.rs`（PackageStore 三元组 + ResourceHandler 钩子：automations=V1 校验、functions=functions: 包装校验、templates=灰度归一化+重命名引用改写）。
- 插件生命周期：`extensions/service.rs`（enable=启用意图+直接启动、幂等；安装/启用/禁用/更新/卸载；reconcile_startup）。manifest v2 schema 在 `manifest.rs`（deny_unknown_fields，加 [[dependencies]] 需同步 RawManifest）。
- 跨插件动作缝：`extensions/mod.rs::native_call_action` + `gamer_yaml/actions.rs` 版本化公开动作清单（5 动作，清单↔分发双向锁测试）；`service.rs::call_extension` 统一分发（目标必须 Running、declarative 按钮集合门禁、结构化拒绝）。Phase 4 只补「能力发现」读端。
- 前端：script-editor V1（4 类步骤模型/c codec/validation）、函数面板逐函数平铺（useConsoleScriptRunner + useFunctionLibrary + function-list.js）、call 候选=原生目录（GET /api/runners/gamer.yaml/functions）+Package 函数。

**需要修改**：

- 函数存储：`functions/<分类>.yaml` → `automations/_function*.yaml` 前缀识别（resources.rs 钩子/注记/模板改写、compose_function_library、RunTarget::Function 去掉 file 段改为 `<pkg>#<函数名>` 名寻址、timer_yaml submit_manual、entrypoint_descriptor）。前端 api.js listFunctions/listScripts 过滤、useFunctionLibrary、函数面板（分类概念删除，只编辑默认 `_function.yaml`）。
- `find` 双语义收口：`yaml_extension.rs` find=单次匹配（去 timeout/interval 参数）；wait_find/tap_template/wait_disappear 保留轮询。
- manifest `[[dependencies]]`（id/version/required）+ 启动期必需依赖检查 + 循环拒绝 + 停用被依赖方守卫 + 快照透传；gamer.video manifest 声明对 gamer.yaml 可选依赖。
- 能力发现读端：`GET /api/extensions/:id/capabilities`（declarative 按钮 ∪ native 公开动作清单）。
- video 工作台按能力可用性降级；PluginCenter 依赖提示。

**可以直接删除**：`functions/` 目录语义（resources.rs 读写/校验/注记分支）、`FUNCTION_DIR` 前端常量的旧用法、函数分类（分类树/分类必填/`function:<分类>/<名>` 寻址）。

**仍需验证**：全量 cargo test + web test；真机链路与环境受限项不在本轮。

## 12. 实施结果（2026-09-09 收口）

全部 7 个 Phase 已按本计划落地，门禁结果（真实执行，未执行的项不写通过）：

- **Phase 1（默认单文件函数库与前缀识别）**：`resources.rs` 增加
  `is_function_library_path`（basename `_function` 前缀 + `.yaml`，与前端
  `gamer-plugin-ids.js::isFunctionLibraryFile` 同规则）；保存钩子按前缀分发
  校验、旧 `functions/` 路径报 `yaml.functions.dir.removed`；函数名清单注记
  与模板重命名改写只针对 automations/ 内函数库；`compose_function_library`
  改读 automations/ 内 `_function*.yaml`（同名冲突拒绝、文件顺序不决定胜者）；
  `RunTarget::Function` 删 `file` 段（`<pkg>#<函数名>` 纯名字寻址）、
  `entrypoint_descriptor`/`timer_yaml` 同步（带路径段的函数 entrypoint 一律
  400 invalid_payload）；函数库文件不进脚本列表/任务选择器（`listScripts`
  过滤 + Script/任务目标显式拒绝）。前端函数面板：无分类概念，只编辑默认
  `_function.yaml`（编辑态文件徽标只读展示），手动拆分文件只读展示（编辑/
  原文/删除禁用 + 提示），运行按 `<pkg>#<名>`，新建函数 = 载入默认库 +
  `insert_function` 命令追加；NewFunctionDialog 删除。
- **Phase 2（Core 原语与插件函数归属收口）**：`find` 收口为**单次模板匹配**
  （Schema 删 timeout/interval，传了报未知参数）；`wait_find`/`tap_template`
  轮询共用与 find 同一个 `match_once` 实现（发 vision/hit/miss 事件同口径），
  无重复匹配逻辑；基础/便利/纯数据函数全部归属插件（Core 无业务函数——
  基线核对确认无需搬迁）。
- **Phase 3（最小插件依赖）**：manifest v2 新增 `[[dependencies]]
  {id, version?, required?}`（deny_unknown_fields；required 缺省 true、
  version 缺省 `*`、自引用/重复 id/坏版本拒绝）；`start_with_context` 依赖
  门禁（必需依赖已安装 + 版本兼容 + Running，缺失 → 拒绝启动并落
  Enabled + last_error，区别于运行时错误的 Failed；循环拒绝）；`disable`/
  `uninstall` 守卫（被运行中插件必需依赖引用 → 拒绝并提示先停依赖方，
  不级联停用）；快照透传 `dependencies[{id,version_req,required,installed,
  version,state,satisfied,note}]`；gamer.video 官方 manifest 声明对
  gamer.yaml 的**可选**依赖（version `*`、required=false）并重打包
  （`.gplugin` + registry v2，build-plugins.ps1 自检通过）。无自动下载/
  自动启用/lockfile。
- **Phase 4（跨插件能力发现）**：新增 `GET /api/extensions/:id/capabilities`
  → `{id, state, running, actions:[{action, version, surface, summary}]}`，
  actions = declarative 按钮集合（surface `declarative`）∪ gamer.yaml 版本化
  公开动作清单（`native/rest/frontend`，actions.rs 单源）；分发仍统一走
  `POST /api/extensions/:id/call`（Running + 公开集合 + 权限/上下文门禁），
  无新增专用 ID 分支。
- **Phase 5（前端体验收口）**：`yamlCapability.js` 升级为能力发现端点驱动
  （ready = Running，`hasAction(action)` 按公开清单判定；视频导入/录制/播放/
  逐帧/标记不受 gamer.yaml 缺失影响，模板创建/草稿入口按能力降级——沿用
  既有 `yaml-ready` 门禁链路）；PluginCenter 依赖提示区分必需（缺失/未启用
  报错 + 处置指引）与可选（缺失 → 「相关功能入口已降级」提示），依赖实时
  状态来自服务端快照。
- **Phase 6（回归/文档/清理）**：文档收口 `docs/reference/YAML.md`（前缀
  识别、统一命名空间、`<pkg>#<函数名>`、find 单次语义、函数清单归属）、
  `docs/reference/PLUGIN_API.md`（[[dependencies]]、capabilities 端点、
  生命周期端点现状）、`docs/guides/plugin-dev.md`（依赖字段参考）、README、
  AGENTS.md；PITFALLS 追加 3 条（含存量 `functions/` 数据手动迁移说明）；
  删除 NewFunctionDialog.vue 及其测试、fnLib 的 v3 `function:` 目标解析残留。
- **门禁**：后端 `cargo test` 612 passed / 0 failed（含真实 WASM guest e2e）、
  `cargo fmt --check` 通过、`cargo clippy --all-targets` 零警告、
  `cargo check --no-default-features` 通过；前端 vitest 708 passed / 0 failed、
  `pnpm build` 通过；`tools/build-plugins.ps1` 六步自检全部通过。
- **遗留（环境受限，非本轮范围）**：真机 adb 链路、浏览器端到端手工冒烟、
  GitHub Release 发行产物——沿用 V1 计划的 NOT_VERIFIED 口径。
