# Phase 10：插件中心、本地/远程导入、Registry 与安全

## 目标

完成面向最终用户的 Extension 管理体验，让无编译环境的用户可以安全地安装、卸载、更新和管理插件。

---

## 1. 右侧插件中心

在 Workspace Tabs 提供：

```text
+
```

入口。

插件中心至少包含：

```text
市场
已安装
本地导入
URL 导入
```

---

## 2. 本地导入

用户选择：

```text
xxx.gplugin
```

服务端流程：

```text
upload
→ size limit
→ zip traversal validation
→ parse manifest
→ host API compatibility
→ hash/signature
→ permission diff
→ user confirmation
→ staging
→ atomic install
→ WASM load
→ UI contribution register
```

---

## 3. 远程导入

支持两条路径。

### Registry

推荐主路径：

```text
registry.json
→ metadata
→ download .gplugin
→ local install
```

### Direct URL

高级用户：

```text
https://example.com/foo.gplugin
```

下载后仍安装到本地。

---

## 4. 不允许远程 URL 直接作为生产 iframe

不要：

```text
plugin UI = remote live website
```

原因：

- 安装时审核代码与运行时代码不一致
- 无法离线
- 远端被劫持风险
- 版本不可固定
- 无法可靠 rollback

正确方式：

```text
remote
→ download fixed version
→ local plugin store
→ local iframe
```

---

## 5. Plugin Store

建议：

```text
plugins/
└── gamer.yaml/
    ├── 3.0.0/
    └── 3.1.0/
```

当前 active version 单独记录。

升级时可以保留最近一个旧版本用于 rollback。

---

## 6. 签名策略

### 官方 Registry

建议：

```text
必须签名
```

### 本地导入

允许未签名，但必须显著提示：

```text
来源未知
发布者未知
请求权限
```

---

## 7. 权限变更

更新插件如果新增：

```text
device.shell
network
filesystem
```

不得静默升级。

必须：

```text
permission diff
→ user confirm
```

---

## 8. UI 来源与状态

插件详情显示：

- name
- id
- version
- publisher
- source
- signature
- runtime state
- permissions
- dependent App Packages / Tasks

---

## 9. 卸载

卸载流程：

```text
close plugin panels
→ stop runtime
→ unregister capability
→ unregister contributions
→ inspect dependencies
→ remove files
```

默认不要删除用户插件数据，提供：

```text
卸载
卸载并删除数据
```

两种语义。

---

## 10. Extension 与 App Package 依赖

App Package manifest 可声明：

```text
requires:
  gamer.yaml@3
  gamer.keymap@1
  vision.template-match@1
```

安装 App Package 时自动检查 Extension。

卸载 Extension 时提示受影响：

- App Package
- Task
- Workflow

---

## 11. Developer Mode

普通用户：

```text
.gplugin
```

开发者模式可额外支持：

```text
加载本地目录
连接本地 UI dev server
热替换 wasm
```

但这些只属于 Developer Mode，不作为生产插件协议。

---

## 12. UI 三档

manifest 支持：

```text
ui.type = none
ui.type = declarative
ui.type = iframe
```

减少不必要的前端重量。

---

## 13. 安全检查清单

安装前至少校验：

- archive traversal
- unpack size
- file count
- duplicate path
- manifest schema
- plugin id
- version
- host API
- requested permissions
- hash
- signature
- iframe sandbox policy

---

## 验收标准

普通无编译环境用户能够：

```text
打开插件中心
→ 从市场安装插件
→ 右侧出现 Tab
→ 使用
→ 更新
→ 卸载
```

也能：

```text
从本地 .gplugin 导入
从 URL 下载并安装
```

全过程不需要 Rust / Node / Cargo 编译环境。

---

# 最终完成状态

Gamer 最终应满足：

```text
默认安装：
Core + Web UI
无具体应用资源

用户需要：
安装 App Package
安装 Extension

第三方扩展：
WASM + sandbox UI

重能力：
继续 Native Core
```

同时保持：

- 左侧投屏稳定
- 右侧功能动态
- Core 不感知具体 YAML / Keymap / App
- 应用数据按需
- 插件真正可安装/删除
- 高性能图像能力不被 WASM 拖慢
