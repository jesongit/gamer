# Gamer 重构与插件化分阶段计划 V2

> 目标：在保证 **轻量化、高效率、可扩展、低耦合** 的前提下，把 Gamer 逐步演进为：
>
> **Native Core + Core Capability API + Frontend Plugin Workspace + WASM Extensions + App Packages**

---

## 1. 最终目标架构

```text
┌────────────────────────────────────────────────────────────┐
│                     Gamer Web Core                         │
│                                                            │
│  DeviceStage                    Extension Workspace        │
│  WebRTC / scrcpy                Panel Registry             │
│  Overlay / Region Pick          Core Panel / Plugin Panel  │
└───────────────┬──────────────────────────┬─────────────────┘
                │                          │
                │                    UI Bridge API
                │                          │
┌───────────────▼──────────────────────────▼─────────────────┐
│                    Gamer Server                            │
│                                                          │
│ Device / Frame / Vision / Input / Touch / Run / Timer    │
│ Resource / AppPackage / Logging / Storage                │
│                                                          │
│                WASM Extension Host                        │
└───────────────────────┬──────────────────────────────────┘
                        │
                Host Capability API
                        │
           ┌────────────┴────────────┐
           ▼                         ▼
     WASM Extensions             Native Core
     YAML / Keymap / Cron        ADB / scrcpy
     Macro / Workflow            WebRTC / ffmpeg
                                NCC / SQLite
```

应用相关数据单独存在：

```text
App Packages
├── templates/
├── scripts/
├── keymaps/
├── presets/
└── resources/
```

默认发行版不附带任何具体应用模板、脚本或键位映射。

---

## 2. 三条核心边界

### Core Capability

回答：**Gamer 能做什么？**

例如：

- 设备连接与控制
- tap / swipe / key / text
- 启动 / 停止 App
- Frame / Screenshot
- 模板匹配 / match_many
- 色彩采样
- Touch pointer 管理
- RunManager
- Timer
- ResourceResolver
- Logging

### WASM Extension

回答：**如何组合这些能力？**

例如：

- YAML 自动化
- Keymap 规则
- Cron 语义
- Macro
- Workflow
- 第三方自动化逻辑

### App Package

回答：**针对某个应用使用什么数据？**

例如：

- 模板图片
- YAML 脚本
- Keymap YAML
- Task preset
- 其他纯数据资源

---

## 3. 前端边界

左侧长期稳定：

```text
DeviceStage
├── 投屏
├── 基础控制
├── 框选区域
├── Overlay
└── 当前设备 / App Context
```

右侧统一为：

```text
Extension Workspace
├── Core Panels
│   ├── 日志
│   └── 设置
│
└── Plugin Contributions
    ├── 自动化
    ├── 键盘映射
    ├── Cron
    ├── OCR
    └── ...
```

插件不是强制“一插件一 Tab”，而是：

> 一个插件可以贡献 0～N 个 Panel。

---

## 4. 阶段顺序

| 阶段 | 目标 |
|---|---|
| Phase 0 | 基线、Benchmark、兼容性护栏 |
| Phase 1 | Frame 热路径与基础设施优化 |
| Phase 2 | Core 解耦与运行模型泛化 |
| Phase 3 | Core Capability Layer / Vision 数据通路 |
| Phase 4 | App Package / ResourceResolver |
| Phase 5 | Frontend Plugin Workspace |
| Phase 6 | WASM Host / WIT / Extension 生命周期 |
| Phase 7 | Keymap 首个 WASM Extension |
| Phase 8 | YAML vNext + WASM Automation |
| Phase 9 | Timer Core + Cron Extension |
| Phase 10 | 插件中心 / 本地远程导入 / Registry / 安全 |

---

## 5. 两个关键验收点

### Gate A：Phase 4 后

此时尚未引入 WASM，但应已经获得：

- Frame 热路径更低复制
- Core 不再围绕 YAML / script_id 设计
- ResourceId / AppContext 统一
- 默认零业务资源
- App Package 可安装、更新、卸载
- 用户 Override 不被更新覆盖

如果这里的架构收益不成立，应先调整，不继续引入 WASM。

### Gate B：Phase 7 后

Keymap 作为首个真实 WASM 插件跑通后，应验证：

- `.gplugin` 真正可安装/卸载
- Host API 权限边界成立
- WASM 空闲开销可接受
- Input → WASM → DeviceAction 延迟可接受
- UI Panel 生命周期与插件 Runtime 生命周期解耦

通过后再迁移 YAML。

---

## 6. 总体原则

1. **每阶段都必须有独立收益。**
2. **先重构边界，再引入 WASM。**
3. **性能敏感能力留 Native。**
4. **业务编排放 Extension。**
5. **应用数据全部按需下载。**
6. **第三方 UI 不直接 mount 到 Gamer Vue App。**
7. **第三方 Rich UI 使用 sandboxed iframe + UI Bridge。**
8. **插件 UI 不直接访问设备控制 REST API。**
9. **App Package 默认只允许纯数据，不允许注入任意 JS。**
10. **不提前引入 OpenCV / GPU / Process Plugin / 通用 Automation IR。**

---

## 7. 文件列表

- `phase-00-baseline.md`
- `phase-01-frame-and-infra.md`
- `phase-02-core-decoupling.md`
- `phase-03-capability-layer.md`
- `phase-04-app-package.md`
- `phase-05-frontend-workspace.md`
- `phase-06-wasm-host.md`
- `phase-07-keymap-wasm.md`
- `phase-08-yaml-vnext.md`
- `phase-09-cron.md`
- `phase-10-management-registry.md`
