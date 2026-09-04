# Gamer App Package 导入 / 导出 / 编辑开发计划

## 1. 目标

基于当前 Gamer 架构，完成 App Package 的完整制作、分发、安装、编辑闭环。

本次不引入新的“开发包”“Package Maker”“独立制作工具”等概念，直接复用 Gamer 现有的：

- Script 编辑能力
- Function 编辑能力
- Template 管理与裁剪能力
- Keymap 编辑能力
- Preset / Resource 管理能力
- Script / Function 运行与调试能力
- App Package 安装、激活、版本管理能力

最终只保留三个用户入口：

| 操作 | 含义 |
|---|---|
| 导入 | 导入 `.gamerpkg`，安装并激活游戏包 |
| 导出 | 将当前本地编辑区资源生成 `.gamerpkg` |
| 编辑 | 将已安装 App Package 提取到本地编辑区继续修改 |

核心原则：

1. `.gamerpkg` 是发布物，不直接编辑。
2. 已安装 App Package 保持 immutable。
3. 所有修改都发生在 Gamer 本地编辑区。
4. 本地编辑区直接可运行，无需重新打包才能测试。
5. 导出只是编辑工作的最后一步。
6. 当前仍处于开发阶段，不兼容旧 App Package 格式。
7. 不兼容旧“分区快照 ZIP”导入导出流程。
8. 不增加第二套 Script / Function / Template 编辑体系。
9. 不做 Script / Function 内容推断。
10. 本地编辑目录与 `.gamerpkg` 内部目录尽量保持 1:1。

---

# 2. 最终数据模型

## 2.1 本地编辑区

统一将现有：

```text
yaml/
func/
tmpl/
```

重命名为：

```text
scripts/
functions/
templates/
```

最终：

```text
server/data/<android-package>/
├── package.toml
├── scripts/
├── functions/
├── templates/
├── keymaps/
├── presets/
└── resources/
```

含义：

- `scripts/`：可直接运行、可调度的脚本。
- `functions/`：函数库，不可直接运行。
- `templates/`：图像模板。
- `keymaps/`：按键映射。
- `presets/`：预设。
- `resources/`：其他资源。
- `package.toml`：当前编辑区的包元数据。

---

## 2.2 Gamer Package

新的 `.gamerpkg` 内部结构：

```text
manifest.toml
scripts/
functions/
templates/
keymaps/
presets/
resources/
```

本地编辑区与包结构保持：

```text
Editable Local                .gamerpkg

scripts/       ───────────→   scripts/
functions/     ───────────→   functions/
templates/     ───────────→   templates/
keymaps/       ───────────→   keymaps/
presets/       ───────────→   presets/
resources/     ───────────→   resources/
package.toml   ───────────→   manifest.toml
```

不再维护：

```text
yaml/ → scripts/
func/ → functions/
tmpl/ → templates/
```

这种目录转换逻辑。

---

# 3. App Package Format V2

## 3.1 Manifest 增加格式版本

新的 `manifest.toml`：

```toml
format_version = 2

id = "official.hkrpg"
version = "1.0.0"
name = "崩坏：星穹铁道"

[android]
packages = [
    "com.miHoYo.hkrpg"
]
```

其中：

- `format_version`：App Package 文件格式版本。
- `version`：具体游戏包版本。
- 两者严格区分。

导入规则：

```text
缺少 format_version
→ 拒绝

format_version != 2
→ 拒绝

format_version == 2
→ 正常解析
```

当前不实现：

```text
V1 → V2 自动迁移
```

旧配置手动迁移后重新导出即可。

---

# 4. ResourceKind 调整

当前 App Package 资源类型需要正式加入 Function。

建议统一：

```rust
enum ResourceKind {
    Script,
    Function,
    Template,
    Keymap,
    Preset,
    Resource,
}
```

对应目录：

```text
Script     → scripts/
Function   → functions/
Template   → templates/
Keymap     → keymaps/
Preset     → presets/
Resource   → resources/
```

彻底删除：

```text
scripts/ 同时承担 Script + Function
```

的历史行为。

---

# 5. Script / Function 彻底分离

## 5.1 Script

Script 保持现有语义：

- 顶层存在 `steps`
- 可直接运行
- 可进入任务 / 调度体系
- 支持配置参数
- 可调用其他 Script
- 不作为函数库解析

目录：

```text
scripts/
```

---

## 5.2 Function

Function 保持现有独立语义：

- 不可直接运行
- 不进入任务列表
- 只通过 `func` 调用
- 支持函数返回值
- 支持 `then / else`
- 一个文件可定义多个函数
- 不作为 Script 解析

目录：

```text
functions/
```

---

## 5.3 禁止内容推断

不允许：

```text
读取 YAML 内容
↓
猜测这是 Script 还是 Function
```

目录即类型：

```text
scripts/   = Script
functions/ = Function
```

所有 Resolver、Index、Loader 都以目录为准。

---

# 6. 本地编辑区正式化

当前：

```text
server/data/<android-package>/
```

不再定义为：

```text
legacy partition
```

而正式定义为：

```text
Editable Local Resources
```

即 Gamer 自身的资源制作 / 编辑工作区。

本地资源不是兼容 fallback，而是一等数据源。

---

# 7. Runtime Resolver 重构

## 7.1 新优先级

推荐：

```text
Editable Local
    ↓
User Override
    ↓
Installed App Package
```

即：

```text
本地编辑区
>
用户 Override
>
已安装官方包
```

原因：

用户点击「编辑」后，资源会复制到：

```text
server/data/<android-package>/
```

之后在 Gamer 内修改并测试。

如果 Installed Package 优先，则本地修改不会立即生效，“编辑”功能失去意义。

---

## 7.2 CompositeSource

统一增加：

```rust
enum CompositeSource {
    EditableLocal,
    UserOverride,
    InstalledPackage,
}
```

后续诊断日志可明确显示：

```text
daily.yaml
来源：EditableLocal
```

方便调试资源覆盖关系。

---

# 8. Resolver 分类型处理

## 8.1 Script

从低到高 merge：

```text
Installed Package/scripts/
User Override/scripts/
Editable Local/scripts/
```

最终本地编辑区优先。

---

## 8.2 Function

独立链路：

```text
Installed Package/functions/
User Override/functions/
Editable Local/functions/
```

新增：

```rust
ActivePackage::function_sources()
CompositeResolver::function_sources()
```

删除任何：

```text
script_sources 同时提供 function
```

的特殊逻辑。

---

## 8.3 Template

```text
Installed Package/templates/
User Override/templates/
Editable Local/templates/
```

---

## 8.4 Keymap

```text
Installed Package/keymaps/
User Override/keymaps/
Editable Local/keymaps/
```

---

## 8.5 Preset / Resource

同样统一按三层模型处理。

---

# 9. 本地 package.toml

每个编辑区增加：

```text
server/data/<android-package>/package.toml
```

示例：

```toml
format_version = 2

id = "official.hkrpg"
version = "1.2.0"
name = "崩坏：星穹铁道"

[android]
packages = [
    "com.miHoYo.hkrpg"
]
```

用途：

- 保存当前 Package ID。
- 保存版本。
- 保存显示名称。
- 保存 Android targets。
- 导出时直接转换为 `manifest.toml`。
- 从 Installed Package 编辑时，由 `manifest.toml` 还原生成。

这样可以完整 round-trip：

```text
manifest.toml
    ↓ 编辑
package.toml
    ↓ 修改
导出
    ↓
manifest.toml
```

---

# 10. 第一阶段：目录与 Schema Breaking Change

## 目标

一次性完成目录统一和 App Package V2。

## 任务

### 10.1 本地目录重命名

修改：

```text
yaml/ → scripts/
func/ → functions/
tmpl/ → templates/
```

需要同步更新：

- Script Store
- Function Store
- Template Store
- API
- Resolver
- Runtime Loader
- Snapshot / Index
- 测试
- 文档
- 前端涉及路径显示的逻辑

不保留旧目录 fallback。

---

### 10.2 App Package 增加 functions/

合法根目录：

```text
scripts/
functions/
templates/
keymaps/
presets/
resources/
```

---

### 10.3 Manifest V2

增加：

```text
format_version = 2
```

没有该字段或版本不匹配直接拒绝。

---

### 10.4 Script / Function Index 分离

确保：

```text
scripts/*
```

只进入 Script Index。

```text
functions/*
```

只进入 Function Index。

不得交叉。

---

## 阶段验收

以下结构可正常加载：

```text
manifest.toml
scripts/daily.yaml
functions/common.yaml
templates/login.png
keymaps/default.yaml
```

并满足：

```text
daily.yaml
→ 只能作为 Script

common.yaml
→ 只能作为 Function
```

---

# 11. 第二阶段：Editable Local Resolver

## 目标

让本地编辑区从 legacy fallback 升级为正式 Resource Source。

## 任务

### 11.1 CompositeResolver 加入 EditableLocal

统一由 Resolver 管理：

```text
EditableLocal
UserOverride
InstalledPackage
```

不再让各调用方自己实现本地 fallback。

---

### 11.2 调整资源优先级

最终：

```text
EditableLocal > UserOverride > InstalledPackage
```

所有资源类型使用一致规则。

---

### 11.3 清理旧 Legacy 命名

移除或重命名：

```text
legacy_partition
legacy_fallback
legacy_root
```

统一改成：

```text
editable_local
workspace
local_resources
```

具体命名保持整个项目一致即可。

---

## 阶段验收

同名资源：

```text
Installed = A
Override  = B
Editable  = C
```

最终读取：

```text
C
```

删除 Editable：

```text
B
```

删除 Override：

```text
A
```

---

# 12. 第三阶段：PackageBuilder

## 目标

把当前 PowerShell 打包流程正式下沉到 Rust 后端。

建议新增：

```text
server/src/app_packages/builder.rs
```

## 职责

```text
Editable Local
    ↓
validate
    ↓
collect
    ↓
manifest
    ↓
archive
    ↓
self verify
    ↓
.gamerpkg
```

建议接口：

```rust
PackageBuilder
├── load_metadata()
├── validate_source()
├── collect_resources()
├── build_manifest()
├── build_archive()
└── verify_archive()
```

不要把业务逻辑直接写在 HTTP handler 里。

---

# 13. 导出前 Preflight

导出必须使用 Gamer 现有解析器检查资源。

## 检查范围

### package.toml

检查：

- `format_version`
- Package ID
- version
- name
- Android packages

### scripts/

使用正式 Script Parser / Validator。

### functions/

使用正式 Function Parser / Validator。

### templates/

检查：

- 文件格式
- 是否可读取
- 文件路径是否合法

### keymaps/

使用现有 Keymap Parser。

### presets/

使用现有 Preset Parser。

### resources/

检查：

- 文件路径
- 文件大小
- 非法路径

---

## 原则

禁止创建：

```text
Package 专用 YAML Parser
```

运行时与导出时必须使用同一套解析规则。

---

# 14. PackageBuilder 打包规则

由于本地目录和 Package 目录已经统一，打包过程简化为：

```text
package.toml
    → manifest.toml

scripts/
    → scripts/

functions/
    → functions/

templates/
    → templates/

keymaps/
    → keymaps/

presets/
    → presets/

resources/
    → resources/
```

不再需要目录映射器。

---

# 15. 可复现打包

建议本次一起实现：

- 文件按相对路径排序。
- ZIP 路径统一 `/`。
- UTF-8。
- 固定必要的 ZIP entry 参数。
- 不写入随机 metadata。
- Manifest 输出顺序稳定。

目标：

```text
相同输入
→ 尽可能得到相同 gamerpkg
→ 尽可能得到相同 SHA-256
```

为后续：

- Registry
- 缓存
- 签名
- CDN
- 更新校验

做好基础。

---

# 16. 导出 API

新增：

```http
POST /api/app-packages/export
```

请求：

```json
{
  "android_package": "com.miHoYo.hkrpg"
}
```

后端：

```text
读取 package.toml
    ↓
Preflight Validate
    ↓
PackageBuilder
    ↓
Archive Validator 自检
    ↓
返回 .gamerpkg
```

建议响应：

```text
Content-Type: application/octet-stream
Content-Disposition: attachment; filename="official.hkrpg-1.2.0.gamerpkg"
X-Content-Sha256: ...
```

---

# 17. 第四阶段：导入游戏包

已有 App Package Install 能力尽量直接复用。

新的 UI 语义：

```text
导入
=
选择 .gamerpkg
+
验证
+
安装
+
激活
```

不依赖当前 Android Package。

---

## 17.1 导入规则

导入 `.gamerpkg` 时：

1. 校验 ZIP 安全性。
2. 读取 `manifest.toml`。
3. 校验 `format_version == 2`。
4. 校验 Package ID。
5. 校验 version。
6. 校验 Android targets。
7. 校验资源根目录。
8. 校验 SHA-256。
9. Staging 解包。
10. 原子安装。
11. 激活新版本。

---

## 17.2 Immutable

如果：

```text
official.hkrpg@1.2.0
```

已存在，再导入相同版本：

```text
直接失败
```

不覆盖。

修改后应：

```text
1.2.0 → 1.2.1
```

重新导出。

---

# 18. 第五阶段：编辑已安装游戏包

新增能力：

```http
POST /api/app-packages/:id/:version/edit
```

含义：

> 将指定 immutable App Package 提取成 Gamer 本地编辑资源。

---

# 19. 编辑映射

由于目录已经统一：

```text
Installed                     Editable

manifest.toml        →        package.toml
scripts/             →        scripts/
functions/           →        functions/
templates/           →        templates/
keymaps/             →        keymaps/
presets/             →        presets/
resources/           →        resources/
```

不需要任何资源目录转换。

---

# 20. 编辑不做 Merge

如果当前编辑区已经存在资源：

```text
server/data/<android-package>/
```

V1 不做智能 merge。

只提供：

```text
取消
替换编辑区
```

原因：

自动 merge 无法可靠判断：

- 同名 YAML 谁优先。
- 包内删除资源如何同步。
- Template 哪个版本更合理。
- Function 是否来自用户修改。
- package.toml 如何合并。

保持简单、明确、可预测。

---

# 21. 编辑区替换安全

推荐：

```text
Installed Package
    ↓
.edit-staging/<uuid>
    ↓
完整提取
    ↓
Validate
    ↓
原子替换受 Gamer 管理的资源目录
```

受管理目录：

```text
scripts/
functions/
templates/
keymaps/
presets/
resources/
package.toml
```

避免编辑过程中留下：

```text
一半旧资源
+
一半新资源
```

---

# 22. 多 Android Target

manifest 支持：

```toml
[android]
packages = [
    "com.foo.game",
    "com.foo.game.global"
]
```

编辑时：

### 当前 activePkg 属于 targets

直接使用当前 activePkg。

### 只有一个 target

自动选择。

### 多个 target 且当前无匹配

弹窗选择：

```text
选择编辑到：

○ com.foo.game
○ com.foo.game.global
```

提取后的：

```text
package.toml
```

仍保存完整 targets。

---

# 23. 第六阶段：前端三个入口

直接基于当前 Workspace 顶部操作区改造。

最终：

```text
[导入] [导出] [编辑]
```

---

# 24. 导入

按钮：

```text
导入
```

Tooltip：

```text
导入 Gamer 游戏包
```

文件类型：

```text
.gamerpkg
```

不要求存在 active Android Package。

流程：

```text
选择文件
    ↓
上传
    ↓
Install
    ↓
Activate
    ↓
刷新 Package 状态
```

---

# 25. 导出

按钮：

```text
导出
```

需要：

```text
activePkg != null
```

如果：

```text
package.toml
```

不存在：

弹出初始化界面。

字段：

```text
Package ID
Name
Version
Android Packages
```

默认：

```text
Android Packages = 当前 activePkg
```

保存后再导出。

---

## 导出弹窗建议

```text
游戏包

ID
official.hkrpg

名称
崩坏：星穹铁道

版本
1.2.0

Android Package
com.miHoYo.hkrpg

资源统计

Scripts       23
Functions      7
Templates     86
Keymaps        2
Presets        3
Resources      5

[取消] [导出]
```

---

# 26. 编辑

按钮：

```text
编辑
```

针对当前 Android Package 对应的 Active App Package。

例如：

```text
Android:
com.miHoYo.hkrpg

Active:
official.hkrpg@1.2.0
```

点击：

```text
编辑
```

弹窗：

```text
将 official.hkrpg@1.2.0
导入到 com.miHoYo.hkrpg 编辑区。

当前编辑区中的 Gamer 资源将被替换。

[取消] [开始编辑]
```

成功后：

```text
刷新 Scripts
刷新 Functions
刷新 Templates
刷新 Keymaps
刷新 Presets
刷新 Package Metadata
```

然后直接可以：

```text
修改
→ 保存
→ 运行
```

无需重新打包。

---

# 27. 旧 Snapshot 导入导出清理

当前旧入口如果仍操作：

```text
Partition Snapshot ZIP
```

则在新的 GamerPkg 流程完成后移除主 UI。

删除：

- 旧导入按钮逻辑。
- 旧导出按钮逻辑。
- `.zip` Snapshot UI。
- 前端旧状态。
- 不再使用的旧 API。
- 相关测试。

如果底层 Snapshot 能力对测试 / Debug 有价值，可以作为内部工具保留，但不再占用用户主入口。

---

# 28. 删除 PowerShell 正式打包流程

Rust `PackageBuilder` 稳定后：

```text
tools/export-app-package.ps1
```

退出正式流程。

建议最终删除，避免同时维护：

```text
Rust Builder
+
PowerShell Builder
```

两套格式实现。

如果未来需要 CLI：

```text
gamer package export
```

也调用相同 Rust 核心。

---

# 29. API 建议

最终相关 API 大致收敛为：

```text
GET    /api/app-packages

POST   /api/app-packages/install

POST   /api/app-packages/export

POST   /api/app-packages/:id/:version/activate

POST   /api/app-packages/:id/:version/edit

DELETE /api/app-packages/:id/:version
```

如当前路由命名已有成熟风格，可按项目现有规范调整，不要求完全采用上述路径。

---

# 30. 错误类型

建议增加明确错误：

```text
UnsupportedPackageFormat
InvalidManifest
InvalidPackageId
InvalidPackageVersion
InvalidAndroidPackage
InvalidResourceRoot
InvalidScript
InvalidFunction
InvalidTemplate
InvalidKeymap
PackageAlreadyInstalled
PackageBuildFailed
PackageEditFailed
EditableWorkspaceNotFound
EditableWorkspaceConflict
```

前端不要只显示：

```text
Internal Server Error
```

尽量映射成明确用户提示。

---

# 31. 测试计划

## 31.1 Manifest

```text
format_version = 2
→ OK

缺少 format_version
→ Fail

format_version = 1
→ Fail

format_version = 3
→ Fail

非法 Package ID
→ Fail

非法 version
→ Fail

非法 Android package
→ Fail
```

---

# 32. Resource Root

合法：

```text
scripts/
functions/
templates/
keymaps/
presets/
resources/
```

非法：

```text
yaml/
func/
tmpl/
abc/
```

在 `.gamerpkg` 中直接拒绝。

---

# 33. Script / Function 分离

Package：

```text
scripts/daily.yaml
functions/common.yaml
```

验证：

```text
Script Index
→ daily

Function Index
→ common
```

不得：

```text
daily 出现在 Function Index
common 出现在 Script Index
```

---

# 34. Resolver Priority

构造：

```text
Installed:
scripts/test.yaml = A

Override:
scripts/test.yaml = B

Editable:
scripts/test.yaml = C
```

最终：

```text
C
```

删除 Editable：

```text
B
```

删除 Override：

```text
A
```

Functions / Templates / Keymaps 同样覆盖测试。

---

# 35. Export Round Trip

准备：

```text
data/com.test.game/
├── package.toml
├── scripts/
├── functions/
├── templates/
├── keymaps/
├── presets/
└── resources/
```

执行：

```text
Export
↓
test.gamerpkg
↓
Archive Validate
↓
Install
```

验证所有文件和 manifest 正确。

---

# 36. Edit Round Trip

继续：

```text
Installed Package
↓
Edit
↓
data/com.test.game
```

验证：

```text
manifest.toml → package.toml

其他目录
→ 1:1 提取
```

资源 hash 应保持一致。

---

# 37. 完整 E2E

至少增加一条完整生命周期测试：

```text
① 创建本地编辑区

② 新建 Script

③ 新建 Function

④ 添加 Template

⑤ 添加 Keymap

⑥ 直接运行测试

⑦ 导出
   official.test@1.0.0

⑧ 删除本地编辑资源

⑨ 导入 GamerPkg

⑩ 激活并运行
   确认使用 Installed Package

⑪ 点击编辑

⑫ 修改 Script

⑬ 修改 Function

⑭ 立即运行
   确认使用 Editable Local

⑮ version 改为 1.0.1

⑯ 再次导出

⑰ 导入 1.0.1

⑱ 验证：
   1.0.0 仍存在
   1.0.1 已激活
```

该 E2E 通过后，App Package 生命周期才算真正闭环。

---

# 38. 文档同步

重点更新：

```text
README.md
docs/YAML.md
docs/gamer_refactor_plan_v2/phase-04-app-package.md
```

删除：

```text
yaml/
func/
tmpl/
legacy partition fallback
scripts 同时作为 functions
PowerShell 正式打包
旧 Snapshot 主流程
```

统一改成：

```text
scripts/
functions/
templates/
Editable Local
App Package Format V2
导入
导出
编辑
```

---

# 39. 推荐实施顺序

## P0：目录与格式统一

```text
yaml → scripts
func → functions
tmpl → templates

+
ResourceKind::Function

+
format_version = 2
```

这是所有后续工作的基础。

---

## P1：Runtime Resolver

```text
Editable Local 正式化

↓

Editable
>
Override
>
Installed
```

同时彻底拆分 Script / Function source。

---

## P2：PackageBuilder

```text
package.toml
↓
Preflight
↓
PackageBuilder
↓
.gamerpkg
```

实现正式 Rust 导出能力。

---

## P3：Edit / Extract

```text
Installed Package
↓
staging
↓
validate
↓
Editable Local
```

---

## P4：Frontend

实现：

```text
导入
导出
编辑
```

三个入口。

---

## P5：E2E

完成：

```text
编辑
→ 运行
→ 导出
→ 导入
→ 运行
→ 编辑
→ 修改
→ 再导出
```

闭环。

---

## P6：清理

删除：

```text
旧目录支持
旧 Snapshot 主 UI
PowerShell 正式打包
Legacy fallback 命名
旧格式测试
```

更新文档。

---

# 40. 本次明确不做

本阶段不要扩展到：

- Package Registry
- Marketplace
- GitHub Release 自动发布
- 自动更新
- Package Signing
- Git 集成
- Package IDE
- 独立 Development Mode
- 热更新框架
- 旧 Package 自动迁移
- 旧目录 fallback
- Snapshot 兼容
- 智能 Merge
- 在线包商店

这些都不是当前闭环的必要条件。

---

# 41. 最终架构

```text
                 Gamer Workspace
                       │
                       ▼
       server/data/<android-package>/
       ├── package.toml
       ├── scripts/
       ├── functions/
       ├── templates/
       ├── keymaps/
       ├── presets/
       └── resources/
                       │
             ┌─────────┴─────────┐
             │                   │
           编辑/运行             导出
             │                   │
             │                   ▼
             │            PackageBuilder
             │                   │
             │                   ▼
             │              .gamerpkg
             │                   │
             │                   │ 导入
             │                   ▼
             │          AppPackageStore
             │                   │
             │                   ▼
             │         immutable install
             │                   │
             │                 编辑
             │                   │
             └───────────────────┘
```

系统最终只存在两个真正的数据状态：

## Editable Local

```text
可修改
可运行
可调试
可导出
```

## Installed App Package

```text
不可修改
版本化
可激活
可回滚
用于稳定运行
```

`.gamerpkg` 只是二者之间的发布载体。

---

# 42. 完成标准

本次改造满足以下全部条件才算完成：

1. 本地目录统一为 `scripts/functions/templates/...`。
2. `func/`、`yaml/`、`tmpl/` 不再作为正式目录存在。
3. App Package 正式支持 `functions/`。
4. Script 与 Function 完全独立解析。
5. 不再进行 YAML 内容类型推断。
6. `.gamerpkg` 使用 `format_version = 2`。
7. Gamer 内点击「导出」即可生成完整 `.gamerpkg`。
8. Gamer 内点击「导入」即可安装并激活 `.gamerpkg`。
9. Gamer 内点击「编辑」即可将已安装包恢复到本地编辑区。
10. 编辑后的资源可以立即运行。
11. Editable Local 优先于 User Override 和 Installed Package。
12. 已安装 Package 始终 immutable。
13. 修改包必须修改版本号后重新导出。
14. 同版本禁止覆盖安装。
15. 旧版本可继续保留。
16. 新版本可激活。
17. 导出、导入、编辑全部通过 Round Trip 测试。
18. PowerShell 不再承担正式 Package 构建职责。
19. 旧 Snapshot ZIP 不再占用主 UI 的“导入/导出”概念。
20. 不新增重复的 Script / Function / Template 编辑能力。
