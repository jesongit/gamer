# Gamer V3 Package 模型与前端架构收口计划

## 1. 目标

基于当前 V3 插件化架构，进一步统一 Gamer 中以下几个核心概念：

- Device / Android App
- Package
- Plugin
- Resource
- Task
- Market
- Frontend Navigation

重点解决目前仍然存在的概念耦合：

- Android Package Name 同时承担运行目标和资源作用域。
- Package 与 Android 应用绑定过深。
- Plugin Panel 与主页签混在一起。
- Package 数据、插件数据、应用数据边界不清晰。
- Installed Package / Editable Workspace 存在不必要的双层模型。
- 导入、导出、编辑、安装之间的语义复杂。
- 前端仍然部分沿用 V2 / 旧 V3 的信息架构。

最终统一为：

```text
Android App = 执行目标
Package     = 配置数据上下文
Plugin      = 功能与数据解释能力
Task        = 通用调度系统
```

---

## 2. 核心架构原则

### 2.1 Android App

Android 应用只负责：

```text
设备
  ↓
Android App
  ↓
运行目标
```

例如：

```text
com.miHoYo.hkrpg
```

它不再作为 Gamer 配置数据的一级存储作用域。

Android Package Name 主要用于：

- 启动应用
- 停止应用
- 切换应用
- 判断当前运行目标
- Package compatibility 检查
- Plugin Runtime 获取当前目标应用

---

### 2.2 Package

Package 定义为：

> 一套独立、可编辑、可导入、可导出、可分享的 Gamer 配置集合。

Package 使用独立 Package ID：

```text
official.hsr.daily
user.xxx.custom
```

不使用 Android Package Name 作为 Package ID。

Package 是右侧所有非全局配置的一级数据上下文。

例如：

```text
Package: official.hsr.daily

├─ Gamer 插件数据
├─ Keymap 插件数据
├─ OCR 插件数据
└─ 其他插件数据
```

Package 不属于 Android App。

Package 也不属于 Plugin。

---

### 2.3 Plugin

Plugin 定义为：

> Gamer 的能力扩展单位。

Plugin 可以贡献：

- Runner
- Panel
- Editor
- Validator
- Resource Handler
- Market capability
- 其他 Runtime capability

Plugin 自己负责解释：

```text
Package/plugins/<plugin-id>/
```

下面的数据。

Core 不理解插件内部具体格式。

---

### 2.4 Task

Task 保持 Core 能力。

Task 不直接理解：

```text
脚本
自动化
宏
OCR
```

而只理解：

```text
Schedule
+
Runner
+
Entrypoint
+
Payload
```

插件负责注册 Runner。

因此：

```text
Task
  ↓
Runner
  ↓
Plugin Capability
```

---

## 3. Package 新数据模型

建议 Package 本地结构调整为：

```text
packages/
└─ <package-id>/
   │
   ├─ package.toml
   │
   ├─ shared/
   │
   └─ plugins/
       ├─ <plugin-id-a>/
       │   └─ ...
       ├─ <plugin-id-b>/
       │   └─ ...
       └─ <plugin-id-c>/
           └─ ...
```

例如：

```text
packages/
└─ official.hsr.daily/
   │
   ├─ package.toml
   │
   └─ plugins/
       ├─ gamer.yaml/
       │   ├─ automations/
       │   ├─ functions/
       │   └─ templates/
       │
       └─ gamer.keymap/
           └─ mappings/
```

注意：

`automations/functions/templates/mappings` 等目录不属于 Core 规范。

这些目录由对应 Plugin 定义。

Core 只规定：

```text
Package
└─ plugins/<plugin-id>/
```

这一层。

---

## 4. Package Manifest

Package manifest 建议至少包含：

```toml
id = "official.hsr.daily"
name = "星穹铁道日常"
version = "1.0.0"
author = "xxx"
```

### 4.1 Android Target

Android App 作为兼容目标声明，而不是 Package 身份。

例如：

```toml
[targets.android]

packages = [
    "com.miHoYo.hkrpg",
    "com.HoYoverse.hkrpgoversea"
]
```

支持：

```text
0 个 Android Package
```

表示通用 Package。

支持：

```text
多个 Android Package
```

表示一套 Package 可以用于多个 Android 应用。

---

## 5. Plugin Dependency

Package 可以声明 Plugin Dependency。

例如：

```toml
[plugins."gamer.yaml"]
required = true

[plugins."gamer.keymap"]
required = false
```

需要区分：

```text
required
optional
```

### 5.1 Required Plugin 不存在

Package 仍允许导入。

但是标记：

```text
部分功能不可用
```

并提示缺少：

```text
gamer.yaml
```

允许后续安装。

### 5.2 Optional Plugin 不存在

Package 正常导入。

相关数据保持原样。

例如：

```text
plugins/gamer.keymap/
```

即使当前没有安装 `gamer.keymap`，也不删除。

后续安装插件后自动恢复对应能力。

---

## 6. Dormant Plugin Data

明确支持：

```text
Package 包含 Plugin Data
但 Plugin 当前未安装
```

这种状态。

例如：

```text
Package
├─ plugins/gamer.yaml/
└─ plugins/gamer.keymap/
```

当前仅安装：

```text
gamer.yaml
```

那么：

```text
gamer.yaml 数据
    → 正常加载

gamer.keymap 数据
    → 保留
    → 不解释
    → 不修改
```

后续：

```text
安装 gamer.keymap
```

即可直接使用已有数据。

Core 不需要对未知插件的数据执行迁移。

---

## 7. 删除 Installed / Editable 双层 Package 模型

取消：

```text
Installed Package
        ↓
Extract
        ↓
Editable Workspace
```

这种双层结构。

统一为：

```text
Local Package
```

本地 Package 默认：

```text
可读
可写
可运行
可导出
```

`.gamerpkg` 只是 Package 的传输格式。

关系调整为：

```text
Local Package
    ⇅
.gamerpkg
```

---

## 8. Package 导入

导入：

```text
.gamerpkg
    ↓
验证
    ↓
Local Package
```

流程建议：

```text
读取 package
↓
解压到临时目录
↓
验证 manifest
↓
验证 Package ID
↓
验证目录安全
↓
验证 Plugin Data 基础结构
↓
检查目标 Package 是否存在
↓
原子安装 / 替换
```

---

## 9. Package 覆盖升级

如果 Package ID 已存在：

```text
official.hsr.daily
```

再次导入：

```text
official.hsr.daily@1.1.0
```

默认采用：

```text
覆盖更新
```

不实现复杂 Merge。

更新前提示：

```text
当前 Package 已存在。

继续导入将覆盖该 Package 当前数据，
包括用户直接修改或新增的内容。
```

用户确认后：

```text
临时目录
↓
完整校验
↓
原子替换
```

避免半安装状态。

---

## 10. 本地自定义 Package

如果用户希望保留自己的修改，可以：

```text
复制 Package
```

例如：

```text
official.hsr.daily
        ↓
复制为
        ↓
user.hsr.daily
```

然后修改：

```text
user.hsr.daily
```

V3 当前阶段不实现：

```text
Package inheritance
Package overlay
Package merge
```

这些能力等实际需求出现以后再增加。

---

## 11. Package 导出

导出针对当前 Package：

```text
Local Package
        ↓
.gamerpkg
```

导出内容：

```text
package.toml
shared/
plugins/
```

不关心具体 Plugin Data 内部格式。

插件如有特殊导出校验需求，可以通过 Plugin capability 扩展。

---

## 12. Resource System 重构

目前如果 Resource System 仍然以：

```text
data/<android-package>/
```

作为主要数据作用域，需要调整。

新的一级作用域：

```text
Package ID
```

例如：

```text
packages/official.hsr.daily/
```

资源定位从：

```text
Android Package
+
Resource Type
```

逐步调整为：

```text
Package ID
+
Plugin ID
+
Plugin Resource Path
```

例如：

```text
official.hsr.daily
+
gamer.yaml
+
automations/daily.yaml
```

---

## 13. Plugin Resource API

建议逐渐形成统一 Resource API：

```text
PackageResource
```

至少包含：

```text
package_id
plugin_id
path
```

概念上：

```text
read(package_id, plugin_id, path)

write(package_id, plugin_id, path)

delete(package_id, plugin_id, path)

list(package_id, plugin_id, path)
```

Plugin 不应直接自行拼接全局目录。

由 Core 提供安全的数据根目录。

---

## 14. Plugin 数据隔离

每个插件默认只能操作：

```text
packages/<package-id>/plugins/<plugin-id>/
```

自己的数据。

不要允许插件默认写入其他插件目录。

如果未来确实存在插件间协作，再单独设计 capability / permission。

V3 当前不提前增加。

---

## 15. Shared Resources

Package 可以预留：

```text
shared/
```

但不要过度使用。

原则：

```text
能属于 Plugin 的数据
→ 放 Plugin 目录

真正跨 Plugin 的通用数据
→ 才放 shared
```

避免 shared 最终重新变成杂物目录。

---

## 16. Runtime 上下文

运行插件能力时，建议 Runtime Context 明确包含：

```text
device
android_app
package_id
plugin_id
```

形成：

```text
RuntimeContext
├─ Device Context
├─ App Context
├─ Package Context
└─ Plugin Context
```

各自职责独立。

---

## 17. Android App 与 Package Compatibility

当：

```text
当前 Android App
```

与：

```text
当前 Package.targets
```

不匹配时，不需要直接禁止 Package 使用。

建议只做：

```text
Compatibility Warning
```

例如：

```text
当前 Package 声明支持：

com.xxx.cn
com.xxx.global

当前运行应用：

com.other.xxx

该应用不在 Package 声明的兼容列表中。
```

允许用户继续运行。

因为第三方版本、渠道服等场景可能无法提前全部声明。

---

## 18. Plugin Compatibility

切换 Package 时，检查：

```text
Package Plugin Dependencies
```

与：

```text
Installed / Enabled Plugins
```

状态。

形成：

```text
Available
Missing Required
Missing Optional
Disabled
Unknown
```

但 Package 数据始终保留。

---

## 19. Market 模型

Market 统一为：

```text
Market
├─ Plugin Market
└─ Package Market
```

不要使用：

```text
App Market
```

避免与 Android App 混淆。

---

## 20. Plugin Market

负责：

```text
搜索插件
安装插件
升级插件
卸载插件
查看插件能力
```

---

## 21. Package Market

负责：

```text
搜索 Package
安装 Package
升级 Package
查看 Package
```

Package Market 可以展示：

```text
Package Name
Package ID
Version
Android Targets
Required Plugins
Optional Plugins
Author
```

---

## 22. Task 调整

Task 保留现有 Core 模型。

不要再与：

```text
YAML Script
```

形成强绑定。

Task 页面根据当前已启用 Plugin Runner 动态提供：

```text
Runner
Entrypoint
Payload
```

### 22.1 没有 Runner

仍然保留 Task 页面。

页面应表达：

```text
当前没有可以执行的插件能力。
```

而不是表现成“脚本系统坏了”。

---

## 23. 删除旧模型

后端完成新模型后，直接移除旧架构：

```text
data/<android-package>/
```

作为配置主存储的逻辑。

同时删除：

```text
Installed Package
Editable Workspace
Extract Package
Edit Installed Package
```

等旧抽象。

当前仍处于开发阶段，不做兼容层。

旧数据手动迁移即可。

---

## 24. 后端阶段验收

在进入前端重构前，需要确认：

```text
Package ID 已成为数据一级作用域
Plugin ID 已成为 Package 内数据二级作用域
Android Package 不再承担数据目录身份
Package 可直接编辑
Package 可直接导入 / 导出
Package 可以覆盖升级
Plugin Data 可以在插件不存在时安全保留
Task 与 Runner 已完全解耦
```

以上全部稳定后，再开始前端修改。

---

# 25. 前端重构阶段

前面的后端、数据模型、API、Resource System 完成后，最后统一进行前端重构。

不要在后端模型尚未稳定时提前重构主要页面。

---

## 26. 前端整体信息架构

最终前端明确存在三个主要上下文：

```text
设备 / Android App
Package
Plugin
```

它们必须在 UI 上表现为三个不同概念。

---

## 27. 左侧设备 / 投屏区域

Android 应用相关功能应放到设备和投屏区域附近。

例如：

```text
设备
当前应用
应用包名

读取应用
启动应用
停止应用
```

不要把 Android Package Name 放在右侧 Package 配置工具栏。

Android App 的定位：

```text
当前执行目标
```

---

## 28. 右侧 Package Context

右侧功能区域顶部显示：

```text
当前 Package
```

例如：

```text
Package
official.hsr.daily ▼

导入
导出
新建
复制
```

右侧所有非 Core / 非全局功能默认使用：

```text
当前 Package ID
```

作为数据上下文。

---

## 29. 主导航

裸 Core 初始主导航调整为：

```text
任务
日志
市场
插件
设置
```

---

## 30. 市场导航

点击：

```text
市场
```

展示：

```text
插件市场
Package 市场
```

市场自身可以使用下拉菜单、二级菜单或其他现有 UI 机制。

不强制具体视觉设计。

---

## 31. 插件导航

主导航只保留：

```text
插件
```

不要继续把插件贡献的所有 Panel 直接注册成主页签。

点击插件后展示：

```text
Gamer
Keymap
OCR
其他已启用插件
```

这里展示的是：

```text
当前已启动 / 已启用的插件
```

---

## 32. Plugin 二级导航

选择插件以后，再展示该插件贡献的 Panel。

例如：

```text
插件
  ↓
Gamer
```

然后出现：

```text
自动化
函数
模板
```

选择：

```text
Keymap
```

则出现：

```text
映射
```

其他插件根据自己的 manifest 动态贡献。

最终关系：

```text
Main Navigation
└─ Plugin
   └─ Plugin Instance
      └─ Plugin Panels
```

---

## 33. Plugin Panel 与 Package

所有 Package-aware Plugin Panel 默认读取：

```text
Current Package ID
```

例如：

```text
Current Package
    official.hsr.daily

Plugin
    Gamer

Panel
    自动化
```

最终访问：

```text
packages/
official.hsr.daily/
plugins/
gamer.yaml/
...
```

---

## 34. 全局 Plugin 页面

未来某些插件可能存在：

```text
不依赖 Package
```

的功能。

因此 Plugin Panel manifest 应允许声明类似：

```text
scope = global
```

或者：

```text
scope = package
```

默认业务型配置页面使用：

```text
package
```

全局工具型页面可以使用：

```text
global
```

但不要因此引入额外复杂的 Package 继承关系。

---

## 35. Task 页面

Task 属于 Core。

Task 页面根据 Runner Registry 动态展示：

```text
当前可调度能力
```

例如：

```text
Gamer / Automation
OCR / Capture
Recorder / Replay
```

具体内容取决于 Plugin 注册的 Runner。

Task 本身不要继续出现 YAML Script 等硬编码概念。

---

## 36. Package 导入 UI

导入 `.gamerpkg` 时展示：

```text
Package ID
Name
Version
Supported Apps
Required Plugins
Optional Plugins
```

如果 Package 已存在：

```text
明确提示覆盖
```

如果缺少 Required Plugin：

```text
提示缺失功能
```

但允许继续导入。

---

## 37. Package 导出 UI

导出针对：

```text
当前 Package
```

不再要求：

```text
从当前 Android App Workspace 导出
```

Package Metadata 可以直接编辑。

---

## 38. Package 切换

切换 Package 后：

```text
Plugin Panel
Task 创建器
Package Metadata
```

等 Package-aware UI 自动切换到新的 Package Context。

不要让每个插件自己管理当前 Package。

Current Package 应由 Core Store 统一管理。

---

## 39. Frontend Store 收口

建议形成独立状态：

```text
DeviceContext
AppContext
PackageContext
PluginContext
```

不要继续使用：

```text
activePkg
```

同时表达 Android Package 与 Gamer Package。

变量命名至少明确区分：

```text
androidPackageName
currentPackageId
activePluginId
```

---

## 40. 删除旧前端概念

前端重构完成后删除：

```text
当前包名
```

这种模糊命名。

删除：

```text
读取应用 + 导入 + 导出 + 编辑
```

全部挤在同一工具栏的旧设计。

删除：

```text
编辑 Installed Package
```

相关 UI。

删除插件 Panel 直接占据 Core 主导航的旧逻辑。

---

## 41. 前端重构原则

本阶段重点：

```text
Information Architecture
State Architecture
Navigation Architecture
```

而不是视觉改版。

不需要：

- 大规模 CSS 重写
- 全新设计系统
- 动画重构
- UI Library 替换

优先保证：

```text
架构关系看得懂
操作入口职责明确
数据上下文明确
插件扩展自然
```

---

## 42. 最终用户模型

完成后用户应该非常容易理解 Gamer：

```text
左边：
我现在控制哪个设备、哪个 Android 应用。

右上：
我现在使用哪一个 Package。

插件：
我有哪些能力可以操作当前 Package。

任务：
我什么时候调用插件提供的能力。

市场：
我可以安装新的 Plugin 和 Package。
```

---

## 43. 最终架构

```text
                         Gamer Core
                             │
       ┌─────────────────────┼─────────────────────┐
       │                     │                     │
     Device                Package               Plugin
       │                     │                     │
       ▼                     ▼                     ▼
 Android App            Config Context          Capability
       │                     │                     │
       │             ┌───────┴────────┐            │
       │             │                │            │
       │       gamer.yaml data    keymap data      │
       │                                           │
       └──────────────── Runtime Context ──────────┘
                              │
                              ▼
                           Runner
                              │
                              ▼
                            Task
```

其中始终坚持：

```text
Android App != Package
Package != Plugin
Plugin != Data
Task != Script
```

Package 是数据载体。

Plugin 是能力。

Android App 是执行目标。

Task 是调度。

---

## 44. 推荐实施顺序

严格按照以下顺序实施：

```text
Phase 1
确定 Package 新模型

Phase 2
重构 Package 本地存储结构

Phase 3
重构 Resource System

Phase 4
重构 Plugin Resource API

Phase 5
实现 Plugin Data 隔离与 dormant data

Phase 6
重构 Package Import / Export

Phase 7
实现覆盖安装与校验

Phase 8
调整 Android Target / Plugin Dependency

Phase 9
调整 Runtime Context

Phase 10
调整 Task / Runner 相关耦合

Phase 11
清理旧 Package / Workspace 模型

Phase 12
后端整体测试和架构验收

Phase 13
前端状态模型重构

Phase 14
前端导航架构重构

Phase 15
Android App 区域调整

Phase 16
Package Context 与 Package 管理 UI 调整

Phase 17
Plugin 主入口 + Plugin 二级 Panel 重构

Phase 18
Task / Market 页面调整

Phase 19
清理旧前端组件与旧状态

Phase 20
前后端联调与最终验收
```

原则：

> **先数据模型，后 Runtime；先后端，后前端；最后一次性收口 UI。**

不要在 Phase 1～12 期间为了临时兼容旧前端不断增加转换逻辑。

当前仍处于开发阶段，可以允许旧前端在中间阶段暂时不可用。

最终直接以新架构完成整体收口。
