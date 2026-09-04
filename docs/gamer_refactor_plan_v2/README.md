# Gamer 重构与插件化分阶段计划 V2

> 目标：在保证 **轻量化、高效率、可扩展、低耦合** 的前提下，把 Gamer 逐步演进为：
>
> **Native Core + Core Capability API + Frontend Plugin Workspace + WASM Extensions + App Packages**
>
> 📍 **实施状态（2026-09-04）：Phase 0–10 已全部实施完成。验收结论、有意偏离与遗留项见文末「八、实施状态」。**

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

---

## 八、实施状态（2026-09-04 收口）

> Phase 0–10 已全部实施并验收。以下为完成状态、Gate 结论、有意偏离与遗留项；细节以各阶段文档、提交历史（origin/main 之后的提交）与 `benchmarks/` 为准。

### 8.1 阶段状态

| 阶段 | 状态 | 交付要点 |
|---|---|---|
| Phase 0 基线护栏 | ✅ 完成 | `tests/fixtures/` + manifest/SHA-256 锁定、CI 门禁、`benchmarks/baseline.json` 九项指标全部回填（离线七项实测 + 真机两项）、基准测试随树可复现（`#[ignore]` + `GAMER_PERF_ITERS` / `GAMER_PHASE0_ANDROID`） |
| Phase 1 帧热路径 | ✅ 完成 | 帧载荷 `bytes::Bytes` 零拷贝共享（GOP snapshot 指针级共享）、`gop_bytes` 增量记账、ffmpeg 流式写、DB 调用侧 oneshot 异步化、通用文件能力抽至 `core/fs/` |
| Phase 2 Core 解耦 | ✅ 完成 | AppContext / ResourceId / RunRequest 泛化、EventSink、ActivityLease（Viewer/Run/Capture）、RuntimeServices 组合根统一装配；router 只注册路由，后台任务全部在组合根启动 |
| Phase 3 Capability 层 | ✅ 完成 | device / input / touch / frame / vision / resource / run / runtime / log 九域 service + registry + adapters；截图 PPM 直通（capability 链路无 PNG encode/decode 往返）、match_many 单帧解码复用、Matcher 不依赖模板 PathBuf |
| Phase 4 App Package | ✅ 完成 | `.gamerpkg` 安装/激活/卸载 REST、一个 Android 包仅一个 active 内容包、每版本 SHA-256、包内 presets 自动发布为任务预设、复合解析（user-overrides → 包 → 旧分区兜底）、业务资源出库（默认发行零业务资源） |
| Phase 5 前端工作区 | ✅ 完成 | PanelRegistry 动态页签、`?panel=` URL 状态、MessageChannel UI Bridge（`gamer-ui@1`）、sandbox iframe（无 allow-same-origin）、UI/Runtime 生命周期分离、lazy mount/keep_alive；Console.vue 4230 → 1355 行 |
| Phase 6 WASM Host | ✅ 完成 | wasmtime component-model 进默认构建（lazy init，无插件零开销）、WIT 分域 `@1`、权限 allowlist（默认禁 filesystem/network/shell/process）、生命周期状态机 + 全套 REST + 版本回滚、`plugin.call` |
| Phase 7 Keymap WASM | ✅ 完成 | InputEvent → WASM → DeviceAction 真链路、profile 数据通道（分区键位 YAML → guest，未映射键 pass-through 回落）、touch handle 归 Core、官方 keymap 插件产物；进程内 dispatch p95 3.7µs（护栏 100ms） |
| Phase 8 YAML vNext | ✅ 完成 | v3 分层 DSL（Surface → Small AST → Host API）、`invoke` 逃生口、`func` 并入 `call`、`app.start/stop`、返回值泛化（含 handle）、v2/v3 兼容并存、官方 gamer.yaml 插件可安装可运行 |
| Phase 9 Timer Core | ✅ 完成 | TimerCore 持久化/重启恢复/挂起恢复、`wait_terminal` 事件化（去 50ms 轮询）、任务预设与包安装卸载联动、缺 runner 明确依赖错误；Cron 的实现偏离见 8.3-1 |
| Phase 10 插件中心 | ✅ 完成 | 市场/本地导入/URL 导入、ed25519 签名 + 内嵌信任锚、权限 diff 二次确认、版本回滚 UI、「卸载 / 卸载并删除数据」双语义、declarative/iframe/none 三档 UI（declarative Host 已实现） |

### 8.2 Gate 结论

- **Gate A（Phase 4 后）：通过。** ResourceId / AppContext 模型稳定；默认发行零业务资源成立；包可安装/更新/卸载且 user override 不被更新覆盖。
- **Gate B（Phase 7 后）：通过。** `.gplugin` 真装真卸、权限边界成立、无插件时零 WASM 开销、keymap 进程内延迟远低于护栏、UI 与 Runtime 生命周期解耦。浏览器 → 设备全链路端到端延迟未单测（进程内两段均已测且达标），有需要可在真机环境补测。

### 8.3 有意偏离与遗留项

1. **Cron 为 native `ScheduleExtension` 而非 WASM（待决策）**
   现状：cron 表达式解析与触发语义在 `server/src/cron_extension.rs`（native），由 scheduler 静态注册；「TimerCore 不感知 schedule 语义」的目标已达成。与计划「Cron WASM 负责 parse cron」偏离的实现理由：cron 解析是纯计算、无权限/沙箱诉求，跨 WASM 边界只有成本；且现有 WASM 实例生命周期为请求型（call / run），长驻触发器需要新增 timer/trigger capability 面与长驻实例管理，当前收益存疑。
   待选方向（后续决策）：
   - 保持 native，记录为最终形态（成本最低；除非确需第三方自定义触发语义，推荐此项）
   - 迁移为 WASM 长驻扩展（host 需补 trigger capability 与长驻生命周期管理，成本最高）
   - 折中：next/parse 留 native，触发行为（payload 改写、触发条件）交由扩展声明
2. WASM 扩展状态机未做 Available/Starting/Stopping 瞬态（计划为「建议」项；现有 Installed/Disabled/Running/Failed + operation lock 已覆盖全部操作语义）。
3. 生产仅注册 `gamer.yaml` 一个 runner（`ScheduleRegistry` 扩展点已就绪，多 runner 有测试覆盖），未来 `gamer.macro` 等接入无需改 scheduler。
4. Keymap 浏览器 → 设备全链路延迟未单独成测（见 8.2 Gate B 备注）。

### 8.4 用户可感知变化（相对计划实施前）

- **默认发行零业务资源**：业务模板/脚本/键位改经 App Package 安装；既有 `data/<pkg>/` 分区资产可用 `tools/export-app-package.ps1` 打包迁移，本地分区继续兜底生效
- **插件中心**（右侧页签「+」）：市场一键安装官方 keymap / YAML v3 插件，签名校验 + 权限确认，支持版本回滚与两种卸载语义；也支持本地 `.gplugin` 与 URL 导入
- **键位映射可运行于 WASM 扩展**（默认构建已含；无扩展时自动回落原 native 链路，行为不变）
- **YAML v3 脚本**：`app.start/stop`、`invoke` 能力逃生口、`call` 合并 `func`、返回值泛化；v2 脚本完全兼容无需迁移
- **定时任务**：事件化触发（去轮询）、服务重启恢复、依赖缺失明确挂起、任务预设一键实例化
- **性能**：帧零拷贝共享、截图直通解码（P95 340→307ms）、日志/任务写库异步化；基线数据见 `benchmarks/`
