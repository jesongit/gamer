# Phase 5：Frontend Plugin Workspace

## 目标

在引入 WASM 前，先把现有右侧固定功能区改造成动态 Workspace。先使用现有 Vue 组件验证 Panel Registry，再为后续第三方插件 UI 增加 sandbox iframe Host。

---

## 1. 页面长期边界

左侧：

```text
DeviceStage
├── WebRTC / scrcpy
├── 输入
├── Overlay
├── Region Picker
└── Point Picker
```

右侧：

```text
Extension Workspace
├── Tabs
├── Panel Host
├── Core Panels
└── Plugin Contributions
```

顶部统一维护：

```text
WorkspaceContextBar
├── device
├── android package
└── active App Package
```

---

## 2. Console.vue 拆分

目标：

```text
ConsoleShell.vue
├── DeviceStage.vue
├── WorkspaceContextBar.vue
└── PluginWorkspace.vue
```

Workspace：

```text
workspace/
├── WorkspaceTabs.vue
├── PluginPanelHost.vue
├── CorePanelHost.vue
├── registry.ts
├── bridge.ts
├── context.ts
└── lifecycle.ts
```

---

## 3. PanelRegistry

当前“模板 / 脚本 / 映射 / 日志 / 任务 / 设置”不再硬编码。

统一注册：

```ts
interface PanelContribution {
  pluginId: string
  panelId: string
  title: string
  icon?: string
  order?: number
  location: 'console.right'
  runtime: 'core' | 'iframe' | 'declarative'
  requiresDevice?: boolean
  preferredWidth?: number
}
```

第一阶段：

- LogsPanel：core contribution
- SystemPanel：core contribution
- ScriptRunner：core contribution
- KeymapPanel：core contribution
- TaskBoard：core contribution

行为不变，只改变装配方式。

---

## 4. 一个插件允许贡献多个 Panel

不要设计成：

```text
one plugin == one tab
```

允许：

```text
YAML Plugin
├── Scripts
└── Functions
```

或者：

```text
AI Plugin
├── Assistant
├── History
└── Settings
```

---

## 5. Tab URL 状态

建议：

```text
/console?panel=gamer.yaml:scripts
```

优点：

- 刷新不丢
- 浏览器前进后退有效
- 可复制链接
- 插件卸载后可 fallback

不建议为每个插件动态注册大量 Vue Router route。

---

## 6. DeviceStage Host API

插件 UI 不直接碰左侧 DOM。

Core 提供 UI Interaction API：

```text
video.selectRegion
video.pickPoint
video.showOverlay
video.clearOverlay

workspace.openPanel
toast.show
dialog.confirm
```

例如模板插件需要框选：

```text
Plugin UI
→ video.selectRegion()
→ DeviceStage
→ Region
```

---

## 7. Rich UI 使用 sandboxed iframe

第三方 UI 不直接 mount 为 Vue Component。

推荐：

```html
<iframe sandbox="allow-scripts">
```

默认不允许：

```text
allow-same-origin
```

第三方 UI 不应直接访问：

- parent DOM
- Gamer Pinia/store
- localStorage
- Gamer REST API
- 任意设备控制接口

---

## 8. UI Bridge

使用：

```text
MessageChannel
```

而不是无约束的全局 `window.postMessage`。

第一版 Bridge：

```text
context.get
plugin.call
toast.show
dialog.confirm
workspace.openPanel
video.selectRegion
video.pickPoint
storage.get
storage.set
```

Bridge API 要版本化：

```text
gamer-ui@1
```

---

## 9. 插件 UI 不直接控制设备

不要：

```text
iframe → /api/device/tap
```

推荐：

```text
iframe
→ plugin.call(...)
→ plugin backend / WASM
→ Host Capability
→ Device
```

Browser 不作为权限边界。

---

## 10. UI Runtime 类型

建议支持三档：

### none

无 UI 的 headless extension。

### declarative

只需要设置表单、按钮、状态的插件，由 Host 原生渲染。

### iframe

复杂编辑器 / Keymap / OCR / AI 等 Rich UI。

这样不是每个插件都要带一个完整前端应用。

---

## 11. iframe 生命周期

默认 lazy mount。

建议策略：

```text
keep_alive = none
keep_alive = session
```

Host 可以限制最多保留最近 N 个 iframe。

重要原则：

```text
Plugin Runtime 生命周期
!=
Plugin UI 生命周期
```

切走 Keymap Tab 不代表 Keymap runtime 停止。

---

## 12. App Package 不注入任意 UI

第一版明确：

> App Package 只允许数据，不允许自带任意 JS/HTML。

需要自定义 UI：

```text
做 Extension Plugin
```

App Package 只声明依赖。

---

## 验收标准

- 当前 6 个右侧页面由 PanelRegistry 驱动
- Console.vue 明显缩小
- Tab 可动态增删
- 当前 panel 写入 URL
- DeviceStage 框选能力由 UI Bridge 调用
- iframe PoC 可加载静态测试 UI
- iframe 无法直接访问 Gamer 页面 DOM 与主站 API
- Plugin UI 生命周期与 Runtime 生命周期概念分离

## 注意

此阶段仍可不引入 Wasmtime。
