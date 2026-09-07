# Gamer 插件生态简化与视频工作台收尾开发计划

> 状态：待实施（本文件是新一轮开发计划，不代表代码已完成）  
> 日期：2026-09-07  
> 项目：`jesongit/gamer`  
> 建议入库位置：`docs/plans/gamer_plugin_ecosystem_video_finalization_plan.md`  
> 适用阶段：V3 架构开发期，允许必要的破坏性修改，不新增旧架构兼容层  
> 执行目标：修复官方插件发布链，取消插件强制签名，建立用户可自行开发和安装的插件机制，明确宿主与独立插件边界，完成视频工作台原计划的制作闭环。

## 0. 执行说明与事实边界

本计划综合本次讨论中已确认的产品决策、原《Gamer 视频工作台插件开发计划》、仓库 V3 架构约束及 2026-09-07 可访问的公开源码编制。已经存在的实现应复用；本文中的新接口、目录及 manifest 示例属于**拟实施设计**，不得当作现有 API。

本次只完成了公开仓库的只读核对；执行环境无法通过 GitHub DNS 完成克隆，因此没有运行仓库构建、测试或真实设备验收。下文的“已存在”仅表示源码可见，“待核验”表示不能仅凭代码或旧报告认定已经通过。开发 Agent 必须先核对实际工作树、提交版本和最新代码，不得依据旧计划直接覆盖新实现。

### 0.1 本轮已确认的产品决策

| 主题 | 本轮最终决策 |
| --- | --- |
| 官方发布 | 由项目作者自己的 GitHub 仓库/Release 提供官方插件，Gamer 内置官方来源；不建设新的中心化发布服务。 |
| 用户插件 | 任何用户都可以自行开发、构建、导入和使用自己的插件；不要求加入官方仓库、审核、签名或登记公钥。 |
| 签名 | 当前插件体系取消强制签名及必需的 Registry proof；默认构建不生成密钥、不签名。保留必要的完整性、权限和运行时安全检查。 |
| 插件类型 | Extension 不等于 WASM。保留受控 WASM 扩展和宿主预置实现；宿主内置、独立分发、源码仓库位置是不同概念。 |
| Native 边界 | 普通第三方插件不能通过免签名机制加载任意 DLL、动态库或本机可执行文件；宿主预置实现只能通过服务端明确注册的能力启用。 |
| 视频定位 | Core 提供媒体、录制、帧、视觉等通用机制；`gamer.video` 拥有制作业务；`gamer.yaml` 拥有模板、脚本语义及 Runner。暂不为了形式上的插件化强制把 FFmpeg 等底层机制搬进 WASM。 |
| 原计划目标 | 恢复完整制作闭环：录制/导入、精确逐帧、项目与标记、校准、模板制作和离线匹配、YAML 草稿保存/编辑、Package 分发、E2E。 |
| 旧机制 | 开发阶段允许清理旧签名流程、无效 manifest 假入口和不必要的宿主业务耦合；现有数据若需要迁移，提供明确、可检查的手动迁移方案。 |

### 0.2 不在本轮范围

不建设公共第三方市场、开发者账号系统、插件审核平台、付费市场或自动审核服务；不提供任意 Native 代码加载；不把所有官方插件强制改为 WASM；不重新实现 YAML v3、视觉引擎、设备管理器、FFmpeg 解码器或任务调度器；不开发完整视频剪辑器、AI 自动识别和自动可靠脚本生成；不恢复旧 Workspace、旧 YAML DSL 或旧 Package 数据布局。

**特别注意：插件包签名与 Gamer 主程序/launcher 更新签名是两个不同问题。** 本计划仅取消 `.gplugin` 分发签名的强制要求，不要求移除 launcher manifest、完整包更新、依赖下载或容器镜像现有的校验和更新安全机制。

---

## 1. 仓库基线与问题清单

### 1.1 已核对的架构事实

1. `AGENTS.md` 明确区分 Android App、Package、Plugin、Task；PackageStore 使用 `(package_id, plugin_id, path)` 三元组，插件数据目录语义归插件，未安装插件的数据保留。YAML v3 是唯一脚本方案。
2. 当前扩展 manifest、生命周期、归档校验、权限、WASM 运行时和 UI contribution 已有实现；UI 支持 `core`、`declarative`、`iframe`，不需要从零创建插件框架。
3. 当前 `gamer.video` 是无 guest、无 Runner 的宿主 Native 实现，启动仅表示 Running；面板 `VideoWorkbench` 由前端预置组件注册表解析。它不是一个携带完整视频业务代码的独立 WASM 插件。
4. 当前视频 manifest 仍声明 `entry = "plugin.wasm"`，而归档校验要求 entry 是实际存在的 WASM 文件。现有视频生命周期测试使用了最小 WASM 字节作为测试包。应修正模型，不应把这种占位文件变成正式发布约定。
5. `tools/build-plugins.ps1` 当前只构建/打包 Keymap 与 YAML，并强制生成签名和 Registry proof；公开 `registry.json` 只列出这两个插件，尚未包含视频插件。
6. 当前本地安装路径已有无签名状态；官方安装路径在 `ExtensionService::inspect_with_context` 中要求有效签名与 proof。因此无需推倒安装器，应收口和简化现有策略。
7. 视频工作台已有素材库、基本预览、录制入口和文本草稿。时间轴使用约 33ms 的步进，不等同于真实相邻展示帧；面板尚未形成完整 Video Project、标记/校准及模板制作闭环。
8. 仓库后续《视频工作台 V1 实施合同》将草稿限定为“文本返回，不落盘、不执行”，并将媒体引用、工作副本等能力缩小或预留。该合同解释了部分简化实现，但不能代替原计划最终验收。

### 1.2 需要修复的问题与优先级

| 优先级 | 问题 | 本轮处理 |
| --- | --- | --- |
| P0 | 视频插件未进入官方构建和市场，用户无法正常发现/安装 | Phase 1 完成可用发布闭环。 |
| P0 | 宿主 Native 插件仍需要虚假的 WASM entry | Phase 1/2 引入明确的宿主扩展描述和服务端注册校验。 |
| P0 | 官方安装依赖签名、proof 和开发密钥 | Phase 1 移除强制链路，统一无签名安装。 |
| P1 | 普通用户独立开发、打包、调试流程缺少可验证样例 | Phase 3 完成 SDK、样例及零改宿主源码的安装流程。 |
| P1 | YAML/Keymap/Video 存在不同程度的宿主业务耦合 | Phase 4 审计并明确归属，消除阻碍独立插件的通用机制缺口。 |
| P1 | 视频精确帧、项目、校准、模板和草稿联动不完整 | Phase 5–7 完成。 |
| P1 | 媒体引用与 Package 可选素材分发未形成完整闭环 | Phase 8 完成。 |
| P1 | 缺少完整构建、真实设备、平台与 E2E 证据 | Phase 9 统一验收。 |

### 1.3 主要代码落点（实施前再次确认）

| 区域 | 当前可见位置 | 本轮用途 |
| --- | --- | --- |
| 扩展模型/解析 | `server/src/extensions/manifest.rs`、`model.rs` | 明确执行类型、兼容性及 manifest 契约。 |
| 安装与生命周期 | `service.rs`、`store.rs`、`archive.rs` | 免签名安装、版本管理、宿主注册、归档安全。 |
| 签名与发布 | `signature.rs`、`tools/plugin-signer/`、`tools/build-plugins.ps1` | 删除不再需要的强制链路，建立普通 pack/build 工具。 |
| 官方包声明 | `tools/plugins/gamer.yaml/`、`gamer.keymap/`、`gamer.video/` | 统一构建清单及发布描述。 |
| 市场 | `web/public/registry.json`、`web/public/plugins/`、`web/src/workspace/MarketView.vue` | 官方来源、安装/更新、状态展示、错误处理。 |
| UI Bridge | `server/src/extensions/ui.rs`、`host_api.rs`、前端插件面板与组件注册表 | 受控 UI、命令调用、权限和上下文。 |
| YAML/Keymap | `server/src/extensions/gamer_yaml/`、`keymap/`、`server/guests/yaml-guest/`、`server/tests/keymap-guest/` | 梳理业务归属、正式 guest 和可复用 SDK。 |
| 视频扩展 | `server/src/extensions/video/`、`web/src/components/video/` | 插件生命周期、面板、制作业务。 |
| 媒体/录制 | `server/src/media/`、`recording/`、`api/media.rs`、`api/recording.rs` | 核验并补齐媒体和录制基础能力。 |
| 通用画面/视觉 | `device/`、`capabilities/frame.rs`、`vision.rs`、`api/vision.rs`、`ConsoleVideoStage.vue` | 精确帧、来源切换、坐标及输入门禁。 |
| Package | `resources.rs`、`package_archive.rs`、`server/src/extensions/gamer_yaml/resources.rs` | 项目存储、媒体引用、模板/脚本保存、归档。 |
| 测试 | `architecture_guard_tests.rs`、`web/src/core-shell-boundary.test.js`、相关集成测试 | 锁定新边界、安装/更新/E2E。 |

---

## 2. 最终插件模型与职责边界

### 2.1 三个维度必须分开

**执行形态**决定代码在哪里运行：受控 WASM guest、宿主预置实现，或仅提供声明/界面的无后端扩展（如确有需要）。**分发形态**决定是否独立安装和更新：随 Gamer 发行、单独 `.gplugin`、本地开发包。**源码位置**可以是主仓库、单独仓库或用户本地目录，与前两者无必然关系。

因此，“官方插件源码在 Gamer 仓库”不等于“插件内置”；“插件能从市场安装”也不等于“业务代码可独立更新”。UI 的 `runtime="core"` 只说明使用宿主预置组件，不应被当作后端执行类型。

### 2.2 最终允许的插件形态

| 形态 | 代码交付 | 适用场景 | 安装与安全约束 |
| --- | --- | --- | --- |
| 独立 WASM 插件 | `.gplugin` 携带真实 WASM Component；可携带 declarative/iframe UI | 第三方插件、可独立更新的官方业务插件 | 本地或官方来源安装；Host API 权限；资源、时间和并发限制。 |
| 宿主预置插件 | 执行代码已编译进 Gamer；可通过 manifest/安装记录启用 | `gamer.video` 当前过渡形态、确有宿主依赖的官方能力 | 服务端注册表明确允许的 ID、实现和版本；不得从包内动态装载任意原生代码。 |
| 宿主机制 Extension | 随 Core 装配，不一定需要 `.gplugin` | cron 等通用机制 provider | 明确的内部注册和生命周期；不为了市场展示伪造插件包。 |
| 纯 UI/声明扩展 | 如实际需要，只有配置或 UI，无后端执行器 | 简单工具、面板、只读展示 | 必须声明真实执行形态，不能借用空 WASM 冒充。是否增加该类型由实际样例决定。 |

V1 第三方推荐路径是 WASM + Host API；不开放“上传 DLL 即运行”的 Native 插件机制。宿主预置类型的存在，不应被用户误解为所有人都能通过 manifest 启用任意宿主模块。

### 2.3 Core 与插件职责

```text
Gamer Core
  ├─ 设备 / WebRTC / Frame / Vision / Media / Recording / Input
  ├─ PackageStore / 通用资源 / Task / Scheduler / RunManager
  ├─ Extension 生命周期 / Runtime / 权限 / 安装 / 市场来源
  └─ 通用 Stage、UI Bridge、跨插件能力注册与调度
          │
          ├─ gamer.yaml：YAML v3 / 模板 / 函数 / 自动化 / Runner
          ├─ gamer.keymap：映射规则 / 映射执行 / 配置界面
          ├─ gamer.video：素材制作 / 项目 / 时间轴 / 标记 / 草稿选择
          └─ 用户插件：自己的 WASM + UI + Package 数据
```

Core 不解析 YAML 业务语义、不理解视频项目 JSON、不直接管理 Keymap 规则、不把视频工作台做成永久主导航。插件不复制 Core 的设备/视觉/媒体服务，也不直接读写其他插件私有资源目录。跨插件调用通过明确声明、版本化的能力或命令接口完成。

### 2.4 视频插件的明确定位

当前 `gamer.video` 保留为**官方宿主预置插件**，先修复发布和生命周期，使用户能正常安装使用；但要把这一事实如实展示为“需要 Gamer 宿主支持”的插件，而不是宣传成可独立替换全部媒体实现的 WASM 插件。

长期目标是让视频制作业务和 UI 尽量成为可独立分发的插件资产。底层 Media/Recording/Frame/Vision 继续属于 Core；时间轴、项目、标记、制作流程属于 `gamer.video`。本轮先完成通用 Host API、独立 UI 和命令机制，再基于实际复杂度迁移业务。**不以“全部搬进 WASM”为验收条件，也不允许因为当前宿主实现方便，就继续把新增视频业务写入 Core。**

---

## 3. Phase 0：锁定基线、生成差异清单

**目标：** 在不破坏当前已完成工作和数据的前提下，确认真实剩余任务，避免重复开发。

### 任务

- [ ] 记录当前工作树、分支、HEAD、未提交文件；先阅读最新 `AGENTS.md`、V3 ADR、原视频计划和 V1 实施合同。
- [ ] 对照本文件第 1 节逐项确认代码是否仍存在、是否已被新提交替换；不要直接照搬旧路径和接口。
- [ ] 检查当前插件安装、inspect、update、enable、start、disable、uninstall、activate_version 的调用链和测试。
- [ ] 绘制官方 registry → 下载 → 校验 → 安装 → 启用 → 面板/Runner 的真实调用链。
- [ ] 检查 `gamer.video` 的 manifest、Native 注册、UI 注册、媒体 API 和构建清单，形成最小修复清单。
- [ ] 核对当前媒体、录制、输入事件、离线视觉、StageSource、草稿、Package 引用的真实实现，标记“完成/部分完成/未完成/待验证”。
- [ ] 区分插件签名与 launcher 更新签名涉及的代码和配置，不扩大删除范围。
- [ ] 建立新计划进度表及证据目录。只记录实际执行结果，不把旧报告当作本轮测试通过。

### 交付与验收

输出 `docs/evidence/plugin_ecosystem_phase0_baseline.md`，包含 HEAD、模块所有者、发现的冲突、保留/修改/删除清单、测试基线和后续阶段的实际落点。未完成全仓库构建时明确写明原因。Phase 0 不为了整理文档而重写已有业务。

---

## 4. Phase 1：取消强制签名，修复官方市场与视频插件发布

**目标：** 不需要生成签名密钥，就能构建官方插件、在市场看到视频插件并正常安装；用户也可以直接导入自己的无签名包。

### 4.1 安装安全策略

本轮采用**无强制签名的单一默认模型**，不增加 `signature_policy=optional|required` 这类当前没有实际需求的配置矩阵。来源记录、下载完整性、安装兼容性和执行授权分别处理。

- [ ] 本地导入 `.gplugin` 不要求签名、公钥、Registry proof 或官方发布者身份。
- [ ] 官方市场安装不要求插件签名及签名 proof；不再通过 `official=true` 触发强制签名分支。
- [ ] 市场下载必须校验索引声明的包大小和 SHA-256；不匹配则拒绝安装，且不得污染已安装版本。
- [ ] 本地文件可在检查阶段计算 SHA-256 作为包标识和审计信息，但不把“没有预先知道的 SHA-256”当作拒绝本地安装的理由。
- [ ] 保留 manifest、版本、Host API、权限、ZIP 目录/大小/解压限制、原子提交、生命周期和运行时约束。
- [ ] 安装前展示插件 ID、版本、来源、执行形态、权限增量及宿主依赖；普通用户界面不再要求理解密钥和 proof。
- [ ] 已经带有签名但签名格式损坏或验证失败时，不得悄悄改写成“无签名包”并降级安装。开发阶段可以要求重新打包为真正的无签名包；不要为了这个场景保留复杂的信任策略 UI。
- [ ] 不再使用签名状态作为权限自动授予条件；官方来源也不能自动获得任意文件、进程或设备输入能力。
- [ ] 现有签名元数据若仍在已安装记录中，应作为历史信息或通过明确的开发期迁移处理，不得因为移除 verifier 而导致列表/启动崩溃。迁移不需要保留旧签名验证模式。

**安全说明：** 哈希只证明文件与索引一致，不能证明 GitHub 账号或索引没有被攻破。取消签名是基于当前分发规模作出的工程取舍，不应把 SHA-256 宣称为发布者身份认证。

### 4.2 构建与发布工具

- [ ] 将现有 signer 工具的普通打包能力与签名能力解耦，提供不依赖私钥的 `.gplugin` pack 命令。可复用现有 ZIP/manifest 代码，不必重写打包器。
- [ ] `tools/build-plugins.ps1` 默认只执行 guest 构建、资源收集、归档、SHA-256、大小计算、registry 生成和产物校验。
- [ ] 删除该默认链路中的 keygen、私钥路径、信任锚同步、signature.sig、Registry proof 必填项及相关失败门禁。
- [ ] 调整 registry schema，使签名字段不再必需；清理前后端对 `signature.status=valid` 的硬依赖。开发阶段允许直接升级 schema，不需要维护两套发行格式。
- [ ] 官方构建清单统一包含 YAML、Keymap、Video；由 manifest 或统一插件描述读取 ID/版本，禁止在多处硬编码版本号。
- [ ] 视频包根据真实执行形态打包，不使用从其他插件复制的空 WASM 或测试 fixture 作为正式业务入口。
- [ ] 将 Registry 和正式插件包作为明确的 GitHub 发布产物；仓库内 `web/public/plugins/` 可继续作为开发/离线 seed，但不能成为唯一发布来源。
- [ ] GitHub Release 发布流程要能从指定提交构建、生成完整性清单、上传产物、验证下载链接。CI 不需要存储插件签名私钥。
- [ ] 不要求每次开发都发布 Release：本地构建产物可以直接导入；本地测试市场可以使用现有静态托管，不需要新建后台服务。
- [ ] 构建失败必须退出非零状态，不生成部分成功却宣称完整的 registry；旧可用发布产物不能被失败构建覆盖。

### 4.3 官方 GitHub 市场

- [ ] 官方来源由 Gamer 配置/发行信息确定，例如固定的 `jesongit/gamer` Release/registry 地址；不以包内 `publisher="official"` 作为可信依据。
- [ ] 优先复用当前 registry 加下载接口；不为了只支持一个官方 GitHub 就重新实现通用多市场管理系统。
- [ ] 明确 GitHub registry 文件与插件资产的版本对应关系，保证一次发布的索引指向实际存在的文件；若采用 latest 指针，安装前重新核对对应版本和哈希。
- [ ] 使用 HTTPS；若允许代理/镜像，只允许显式配置的来源，不允许插件 manifest 诱导服务端下载任意内部 URL。
- [ ] 市场清楚区分“官方来源”“本地导入”“来源未知/已失效”，并展示安装状态、版本、宿主要求和更新操作；这些是来源信息，不是代码安全认证。
- [ ] 下载失败、404、超时、哈希错误、manifest 不匹配、宿主版本不足、权限未确认等错误分别给出可理解的提示。
- [ ] 安装完成后无需刷新整个 Gamer 服务端即可识别新插件；对于需要宿主版本支持的插件，明确提示升级 Gamer，而不是无意义地反复安装。

### 4.4 立即验证视频入口

先完成第 5 节的最小宿主插件契约修正，再将 `gamer.video` 纳入正式打包与市场。完整验收必须通过：市场出现视频插件 → 下载 → inspect → 安装 → enable/start → 插件菜单出现视频工作台 → 打开基础素材库 → stop/disable 后 UI 消失 → 重启后状态恢复 → 卸载后数据按既定策略保留。

**Phase 1 验收：** 一条无私钥的构建命令能够产生全部官方包和可用 registry；本地无签名 WASM 示例可安装；视频插件正式包不含伪造 WASM；官方市场下载的字节与 SHA-256 一致。保留插件运行时安全测试，不因免签名放行任意原生执行。

---

## 5. Phase 2：明确执行类型，修正宿主插件与独立插件契约

**目标：** 让安装器能够准确区分真实 WASM 与宿主预置实现，避免插件 ID 特判和虚假 entry 逐渐扩散。

### 5.1 Manifest 契约（拟新增）

建议为这次破坏性调整建立明确的新 manifest 契约，例如 `manifest_version=2`。现有 YAML/Keymap/Video 由官方打包工具统一升级；具体字段名以 Phase 0 的结构审计为准，下面仅表达语义：

```toml
manifest_version = 2
id = "gamer.video"
version = "1.0.0"
name = "视频工作台"

[execution]
kind = "builtin"
builtin_id = "gamer.video"
# host_version = ">=..."  # 使用项目实际版本体系确定

[[ui.contributions]]
location = "console.right"
runtime = "core"
component = "VideoWorkbench"
```

独立插件使用 `execution.kind="wasm"` 和真实 `entry="plugin.wasm"`，并明确 Host API 版本要求。`execution.kind` 是**后端执行类型**，`ui.contributions.runtime` 是**界面渲染类型**，不能混用。manifest 不得允许 `builtin_id` 指向任意 Rust 模块、动态库路径或任意可执行文件。

### 5.2 宿主实现注册表

- [ ] 在组合根/扩展机制建立明确的 BuiltinExtensionDescriptor 注册机制，记录 ID、实现、兼容宿主版本、可用命令/贡献和生命周期处理器。
- [ ] 安装 `builtin` 类型时，服务端校验该 ID 已注册且兼容；不存在则返回 `host_feature_unavailable` 类结构化错误。
- [ ] 注册表不能由下载包自行扩展，不能仅靠前端组件名判断是否允许启动。
- [ ] 允许宿主实现无 guest、无常驻实例、无 Runner；不要为所有插件强制实例化 WASM。
- [ ] 将 `service.rs` 中视频 ID 特判逐步收敛到通用注册机制；保留现有能力实现，不必重写视频录制服务。
- [ ] 修正 `manifest.rs`、`archive.rs`、`store.rs` 等处对 entry 永远是 WASM 的假设；按执行类型验证包内必需文件。
- [ ] 对 WASM entry 继续检查真实格式与组件兼容性，不允许通过把执行类型改成 builtin 绕过普通 guest 校验。
- [ ] 官方宿主实现只能通过经过检查的安装记录激活，卸载/停用时正确撤销 UI、命令和 Runner 贡献；不自动删除 Package 数据。

### 5.3 通用插件调用与 UI Bridge

- [ ] 审核当前 `native_call_action`、`plugin.call`、declarative action 白名单和宿主组件注册表，形成统一的可发现、可版本检查、可授权的命令/能力注册接口。
- [ ] 普通 WASM 插件应能声明自己的命令并经受控 Host API 调用，不要求在 Core 增加一个新的插件 ID 分支。
- [ ] 跨插件调用必须由目标插件声明公开能力，携带调用者身份、Package Context、能力版本及权限上下文；不得直接调用对方私有 Rust 函数或写对方资源目录。
- [ ] 内置组件键由宿主白名单解析。第三方 manifest 不能通过填写 `runtime="core"` 任意挂载宿主内部 Vue 组件。
- [ ] 复用现有 iframe/declarative 机制；第三方 UI 不能直接 import Core 私有模块，也不能直接获取任意 Host API 对象。
- [ ] 核查 iframe 沙箱、CSP、资源 URL、消息来源校验及身份传递。尤其检查同源 iframe 的脚本是否可能绕过 Bridge 读取宿主 Cookie、DOM 或其他插件数据；如存在风险，应使用真正隔离的 UI origin/sandbox 或等效机制，不能仅凭 iframe 标签宣称安全。
- [ ] 服务端根据已认证会话与安装实例确定调用者权限，不信任前端自行提交的 `plugin_id`、`official`、`permission_confirmed` 等字段作为唯一授权依据。

**Phase 2 验收：** 真实 WASM、宿主预置插件分别能完成完整生命周期；普通插件不能冒充内置 ID、不能请求任意核心组件或原生代码加载；卸载后贡献注销但 Package 数据保留。架构边界测试中不再需要为视频增加新的“假 WASM”例外。

---

## 6. Phase 3：让用户真正可以自己写插件自己用

**目标：** 一个不修改 Gamer 仓库的开发者，能够完成“创建 → 编写 → 构建 → 本地导入 → 授权 → 运行 → 调试 → 更新”。

### 6.1 最小可用 SDK 与模板

- [ ] 整理当前公开 WIT、Host API 版本、权限枚举、manifest schema、UI contribution、PackageStore 和通用调用文档，标注稳定与实验接口。
- [ ] 提供一个正式 Rust/WASM 示例项目，源代码放在 `examples/` 或 `sdk/examples/`（具体名称按现有结构确定），不再用 `server/tests/keymap-guest` 代替所有用户插件的产品模板。
- [ ] 示例至少包括：manifest、真实 guest、读取运行上下文、日志、一个受控 Host API 调用、一个公开 command、一个 declarative 或 iframe 面板、Package 私有数据读写。
- [ ] 提供 `build`、`pack`、`inspect`、`install` 的可重复命令或脚本；开发者不需要自行创建签名密钥。命令名和参数以现有工具体系为准，避免新建多个功能重复的 CLI。
- [ ] 提供无后端业务复杂度的 Hello World，证明不必复制 YAML 或 Keymap 的大量代码才能做一个简单插件。
- [ ] 提供至少一个需要设备/视觉能力的示例，验证权限声明和 Host API 可实际调用。
- [ ] 构建产物必须自包含其声明的 guest 与 UI 资产；第三方插件不能依赖在 Gamer 源码中新增组件注册或私有 Rust 模块才能运行。

### 6.2 本地安装和调试体验

- [ ] 插件中心提供清晰的“导入本地插件”入口，支持文件选择及 inspect 预览，不要求开发者拥有 GitHub 仓库。
- [ ] 显示实际 ID、版本、执行形态、宿主要求、权限、来源和安装状态；本地来源无签名不显示阻塞性错误。
- [ ] 允许本地重复构建并安装新版本，保留既有版本管理语义；正在运行时需要停止再更新的规则应有明确提示或受控流程。
- [ ] 提供开发日志、启动错误、Host API 不兼容、权限拒绝和 guest trap 的结构化诊断，避免所有问题都显示“插件运行失败”。
- [ ] 支持用户自行分享 `.gplugin` 并由他人本地导入；不需要把包上传到官方市场或让作者登记公钥。
- [ ] 插件来源与可更新来源分开：本地导入不自动猜测 GitHub 地址；只有用户明确配置或插件确有可信来源记录时才显示可用更新。
- [ ] 不因未签名而放宽文件路径、网络、设备控制、资源访问或脚本执行权限。

### 6.3 扩展能力的真实边界

公开 SDK 要明确：插件可以申请哪些能力，哪些暂不开放；普通插件只能通过受控 Host API 工作。需要新能力时，先评估是否为通用机制，再由 Core 添加版本化 API，而不是让每个第三方插件获得任意 filesystem/process/shell 权限。

**Phase 3 验收：** 在 Gamer 仓库之外创建全新项目，完成真实构建和安装；不修改宿主代码即可显示独立面板、调用公开能力并保存自己的 Package 数据；断开该插件后其他插件正常工作。提供能让其他开发者照做的完整文档和实际运行证据。

---

## 7. Phase 4：官方插件归属收口与可独立分发能力

**目标：** 消除阻碍第三方生态的通用机制缺口，并让官方插件的实际形态与文档一致；不启动不必要的全量重写。

### 7.1 YAML 与 Keymap

- [ ] 盘点 YAML、Keymap 中的宿主 Rust 业务、guest 业务、UI 资产、Host API 和 Runner 注册，列出每部分的所有者及是否必须随宿主发布。
- [ ] 保留 YAML v3 唯一语法、现有资源格式和 Runner 语义；不重新实现 parser 或执行器。
- [ ] 确认正式 Keymap guest 的产品源码与构建入口，不再把测试 fixture 当作长期官方发布产物。若当前 fixture 已经承担实际产品功能，先迁到明确的正式归属并保持功能，再清理测试专用命名。
- [ ] 将 YAML/Keymap 中可复用的通用命令、UI Bridge、上下文、资源与 Runner 注册能力抽到扩展机制，而不是继续增加 `if plugin_id == ...`。
- [ ] 已在宿主实现、但确实属于某个官方插件的业务继续由其扩展模块拥有；无需为形式上的“纯 WASM”复制同一业务实现。
- [ ] 对能够独立交付的业务和 UI，逐步改为插件包携带的 guest/资产；宿主预置组件只保留明确需要宿主实现或通用能力的部分。
- [ ] 若某个现有官方插件暂时仍要求特定 Gamer 版本，应在 manifest 和市场清晰展示，不得宣称完全独立更新。

### 7.2 视频业务归属

- [ ] 保留 `server/src/media/`、`recording/`、Frame/Vision、设备编码流与资源生命周期等通用服务在 Core。
- [ ] 新增的视频项目、时间轴、标记、校准、草稿选择、制作工作流数据与业务命令归 `gamer.video`。
- [ ] `gamer.yaml` 独占模板格式、YAML v3 生成/保存/校验、编辑器与 Runner；视频插件不复制这些实现。
- [ ] 优先通过独立 UI 资产和公开命令让视频制作逻辑脱离宿主私有组件。如果某部分仍必须是宿主实现，记录原因、对应宿主版本和替代方案。
- [ ] 不为视频插件引入直接 FFmpeg/process/filesystem 权限；需要的媒体功能应通过通用 Media Host API 暴露。
- [ ] 本轮交付可以保留明确标识的官方宿主视频插件，但第三方插件必须已经能使用同一套公开媒体/视觉能力。不能以视频仍是宿主插件为理由限制其他开发者使用媒体机制。

**Phase 4 验收：** 官方插件归属清单与实际代码一致；核心通用能力没有重复实现；第三方示例无需修改宿主即可使用已公开的能力；视频插件的宿主依赖被明确标识。原有 YAML/Keymap 回归通过。

---

## 8. Phase 5：媒体与录制基础能力收口

**目标：** 在已有 V1 媒体/录制实现之上补全制作所需的通用能力，不重新造第二套媒体系统。

### 8.1 MediaStore 与离线帧

- [ ] 核验现有媒体 ID、导入、探测、原子落盘、元数据、文件读取、删除及租约实现；先补缺口，不重复开发。
- [ ] 建立真实展示帧 PTS/索引映射（考虑 VFR、B 帧展示顺序、旋转及时间基准），支持获取指定帧和相邻帧。不能继续把固定 33ms 当作准确逐帧。
- [ ] 精确帧由服务端确定性解码提供；浏览器 `<video>` 仅作为流畅预览，不以 `currentTime` 作为制作模板的唯一帧身份。
- [ ] FrameDescriptor 至少保留来源、media ID、帧索引/PTS、原始及工作尺寸、旋转/坐标变换、校准版本；帧句柄不可变并受租约管理。
- [ ] 复用现有 VisionService/NCC，对离线确定帧执行 match、match_many 和 color sample；无设备时不得触发 ADB 截图。
- [ ] 补齐按需工作副本：保留原始文件，必要时转为可播放/可制作的兼容格式；实际支持矩阵由平台和 FFmpeg 测试确定。
- [ ] 大文件导入、探测、转换及解码应有大小、时长、像素、CPU/内存、并发、磁盘与取消限制；禁止任意服务端路径和 shell 拼接。
- [ ] 媒体删除需考虑活动租约、录制会话及 Package 引用，不得删除仍被有效引用的原始素材。

### 8.2 RecordingService 与输入事件

- [ ] 保留 scrcpy 原始编码流分支录制，不录制浏览器窗口，不依赖浏览器是否在前台。
- [ ] 核验起录 IDR/参数集、原始 PTS、MP4 remux、分段、编码参数变化、设备断连及异常 finalize；不能将不兼容帧静默写入同一轨道。
- [ ] 核验有界队列、磁盘空间、时长/大小配额、取消、状态机、幂等停止和失败恢复。录制失败不能阻塞实时投屏，丢帧/缺失区间必须可诊断。
- [ ] 输入事件覆盖现有统一分发路径中的 manual、keymap、runner、plugin；使用 operation_id 或等价关联方式，避免原始 touch 与 tap/swipe 重复生成语义动作。
- [ ] 使用服务端单调时间和媒体 PTS 映射；事件与视频时间轴采用明确的整数时间单位，不直接混用浏览器时间与媒体 PTS。
- [ ] 文本内容默认脱敏；事件读取与普通媒体播放权限分离，录制不自动赋予设备输入权限。
- [ ] 补录制 owner、授权、设备会话代次、停止权限和多 viewer 场景。注意当前系统的 viewer 互斥模型，不应仅因打开离线视频而改变其他用户设备会话或后台任务。
- [ ] 录制关闭时不得产生持续额外解码/FFmpeg 进程或明显的实时路径开销。

### 8.3 Media Host API 与通用舞台

- [ ] 将媒体查询、导入、确定帧、工作副本、录制状态和事件读取作为版本化 Host API 暴露给被授权插件，复用同一个 MediaService/RecordingService。
- [ ] 确认权限闭集、WIT、Host API catalog、Native/WASM adapter 与文档同步；目录里存在权限名不等于 guest 已经能消费该能力。
- [ ] 保持实时 `FrameService.latest` 原语义；离线取帧必须显式指定媒体和帧身份，不把当前舞台伪装成 Android 设备。
- [ ] StageSource 保留 live/media、sourceId、generation、尺寸和变换信息。异步取帧、裁切、匹配结果必须校验来源代次，避免旧结果应用到新画面。
- [ ] 媒体模式下由舞台产生的鼠标、键盘、映射和触控在统一输入路由处拒绝；独立运行中的已授权后台任务仍按原生命周期工作，不因观看录像而自动暂停。

**Phase 5 验收：** 无设备可导入视频并完成确定帧视觉匹配；24/30/60fps 和 VFR 样本的相邻帧可重复定位；录制中关闭浏览器不影响既定录制生命周期；磁盘不足、断连、重复停止、权限拒绝及媒体切换均有可验证结果。

---

## 9. Phase 6：视频工作台制作项目与时间轴

**目标：** 用户能在视频中持续制作和保存工作，而不是只能播放、看一张截图和复制草稿。

### 9.1 Video Project

- [ ] 建立 `gamer.video` 自己的版本化项目数据结构，关联当前 Package、一个或多个 Media Asset、可选 Recording Session、校准、标记、注释、事件选择和制作进度。
- [ ] 项目数据保存于当前 Package 的 `plugins/gamer.video/` 私有目录；具体内部格式由视频插件定义，Core 只负责三元组寻址和通用资源操作。
- [ ] 原视频保留于全局媒体库，项目只保存逻辑引用；不默认把每个大视频复制到 Package。
- [ ] 支持项目创建、打开、重命名、保存、恢复、删除及素材缺失提示；保存应具备原子性或明确的失败恢复策略。
- [ ] 同一素材可被多个项目使用；删除项目不自动删除原始视频，媒体删除遵守引用/租约规则。
- [ ] 插件停用/卸载后项目数据保留；重新安装并恢复能力后仍可打开；缺少媒体时允许项目处于可诊断的缺失状态。

### 9.2 真实时间轴与标记

- [ ] 时间轴使用真实展示帧索引和时间戳，提供精确上一帧/下一帧、指定帧、播放/暂停、seek、倍速、事件跳转、片段边界和标记。
- [ ] 支持添加、编辑、删除标记及注释，保存到 Video Project；标记引用确定帧/时间，不仅保存浏览器浮点秒数。
- [ ] 自录会话显示同步输入事件及来源；外部视频没有事件时仍可正常制作，不伪造操作日志。
- [ ] 区分浏览器预览帧与服务端制作帧，制作操作必须绑定选定的不可变帧；取帧失败、过期或来源切换时不给出错误的旧画面。
- [ ] 有效画面区域、参考分辨率、旋转、像素比例和黑边校准使用统一坐标变换；不静默非等比拉伸。
- [ ] 保存校准版本，旧标记、模板区域和事件坐标不能被悄悄赋予新的含义；校准变化需要明确转换或提示重新制作。
- [ ] 推荐 1920×1080 制作环境，但不限制导入分辨率；不同 UI 比例和游戏布局仍需人工验证。

### 9.3 前端工作区

- [ ] 保持左侧统一舞台、右侧插件面板结构；安装视频插件后才显示视频工作台入口。
- [ ] 右侧内部组织素材库、项目/时间轴、制作相关页面，不新增永久的 Core 视频业务主页签。
- [ ] 左侧实时/视频来源切换与右侧当前项目联动；媒体切换不改变 Android App、Package 或后台 Runner 的身份。
- [ ] 复用现有 `MediaLibrary.vue`、`VideoTimeline.vue`、`VideoDraft.vue`，按真实职责拆分，不为了重构产生大量空组件。
- [ ] 插件缺失、素材缺失、无设备、录制进行中、取帧失败、来源过期等状态有清晰 UI。

**Phase 6 验收：** 用户导入视频，在无设备情况下创建项目、校准、逐帧、加标记、关闭/重开 Gamer 后恢复；卸载视频插件不删除项目数据；视频模式操作不误触真机。

---

## 10. Phase 7：模板制作、离线测试与 YAML 草稿闭环

**目标：** 从一次性录像真正产出可编辑、可保存、可验证的模板与 YAML v3 脚本草稿。

### 10.1 跨插件能力契约

通过 Phase 2 的通用能力/命令注册机制，由 `gamer.yaml` 提供版本化公开能力。下列名称为拟定语义，最终以现有可复用接口为准：

```text
template.create_from_frame
vision.test_template
automation.create_draft
automation.save_draft / automation.open_editor
```

调用必须携带当前 Package Context、确定帧引用/来源代次、校准/坐标元数据和必要的事件引用。目标插件负责校验、命名冲突、资源保存及返回结构化诊断。视频插件不得直接解析 YAML v3 或修改 `gamer.yaml` 私有目录。缺少 YAML 插件时，视频导入、录制、播放、标记仍可工作，只禁用相关制作操作并提示依赖。

### 10.2 模板制作与测试

- [ ] 从选定的确定帧框选区域并创建模板，复用现有 TemplateCrop/TemplateCapture 或等价通用组件。
- [ ] 保存时必须使用用户选定帧，不重新抓取最新设备截图；来源改变或帧失效应拒绝或要求重新选择。
- [ ] 通过 YAML 插件现有资源处理器保存模板到当前 Package，保留模板格式校验、灰度归一化、重名处理和引用规则。
- [ ] 支持在同一离线帧上测试模板匹配，调整阈值和搜索区域并展示命中叠加；结果记录来源、帧与坐标空间。
- [ ] 对不同参考尺寸的模板和搜索区域使用一致变换，不只缩放模板图片而遗忘搜索区域及点击坐标。
- [ ] 提供从视频选中帧到模板编辑/测试的明确入口，至少完成正向定位；双向跳转可在核心闭环稳定后增强。

### 10.3 草稿生成、保存和编辑

- [ ] 复用已实现的 `automation.create_draft` 及事件转换逻辑，扩展为可编辑工作流，不另造 YAML 生成器。
- [ ] 支持选择、删除、排序、注释操作事件，生成 tap/swipe/key/wait 等可映射步骤；间隔推导只作为建议值。
- [ ] 多指、文本、未知输入等无法准确映射的事件必须保留诊断，不静默丢弃或编造完整自动化。
- [ ] 保留事件来源、时间及对应视频帧引用，方便从草稿回查录像。
- [ ] 允许生成文本预览后由用户确认，保存为当前 Package 的 YAML 自动化草稿，并打开/定位到现有 YAML 编辑器。
- [ ] 保存/编辑/运行分离：生成草稿不创建定时任务、不自动启动 Runner、不在切回实时设备时机械回放录制事件。
- [ ] 用户显式选择真实设备后，通过现有 YAML 运行与任务机制验证；涉及一次性消耗行为时不应自动重复执行。

**Phase 7 验收：** 一段自录视频能选帧创建模板、离线匹配、选择输入事件、生成合法 YAML v3 草稿、保存并打开编辑器，最后由用户显式选择真实设备验证。外部视频可无事件完成模板制作；缺少 YAML 插件时视频基础功能不受影响。

---

## 11. Phase 8：Package 分发、插件更新与数据生命周期

**目标：** 插件和游戏配置可以独立分发，视频项目及素材引用有明确生命周期，更新/卸载不破坏用户工作。

### 11.1 Package 与媒体引用

- [ ] 保持 Local Package 可直接编辑、运行、导入、导出，`.gamerpkg` 仅为传输格式；不恢复 Installed/Editable 双层 Workspace。
- [ ] 保持 `package.toml + shared/ + plugins/<plugin-id>/` 和三元组资源寻址，Core 不解释视频项目内部 JSON 或 YAML 业务目录。
- [ ] 实现/核验媒体引用登记、解除、查询、删除保护及垃圾回收；删除必须考虑活动录制、打开句柄、Package 引用和并发操作。
- [ ] 默认 Package 导出不包含原始大视频；增加用户显式选择的“包含媒体素材”模式，列出大小和隐私提示。
- [ ] 导入含媒体的 Package 时校验归档路径、大小、哈希、逻辑 ID 和引用映射；文件先安全落盘，再原子提交元数据/项目引用，失败不得留下可被正常使用的半成品。
- [ ] 不含素材导入时保留引用和缺失状态，允许用户重新关联已有媒体，不要求重新制作整个项目。
- [ ] 未安装 `gamer.video`、`gamer.yaml` 等插件时保留 dormant plugin data，不能由 Core 清理或解释其私有内容。
- [ ] Package 覆盖/复制按现有 V3 语义处理，不引入复杂 Merge 或旧格式兼容层。

### 11.2 插件版本与更新

- [ ] 保留现有不可变已安装版本、active_version、stop 后更新/切换等成熟机制，修复与新 manifest/无签名模式的冲突。
- [ ] 官方插件按 ID、SemVer、宿主要求和 Host API 要求检查更新；本地插件不自动从不明来源更新。
- [ ] 更新前展示权限增量及执行形态变化；从 WASM 变为宿主预置类型等敏感变化应明确提示并校验宿主支持。
- [ ] 不能将未知 ID 的本地插件自动提升为官方内置插件；ID 冲突必须按明确的来源/执行类型规则处理。
- [ ] 卸载插件与删除其 Package 数据是两个独立操作；默认保留数据，用户明确要求清理时再走受控删除流程。
- [ ] 若新版本改变插件私有数据结构，由该插件负责版本化迁移；Core 不理解具体业务数据。开发期不做旧架构兼容，但已有数据需要迁移时应先备份并提供手动迁移说明。

**Phase 8 验收：** 不带视频的 Package 可正常导入并提示素材缺失；带媒体导出可完整校验并恢复引用；卸载/重装插件不丢用户数据；本地插件与官方插件均可更新，权限变化必须重新确认；失败更新不破坏已安装可用版本。

---

## 12. Phase 9：统一验收、文档与清理

**目标：** 以实际可复现的证据证明插件生态和视频工作台可交付，而不是只以源码存在或单元测试通过作为完成。

### 12.1 必须覆盖的测试矩阵

| 类别 | 关键场景 | 预期结果 |
| --- | --- | --- |
| 免签名 | 本地无签名真实 WASM、官方无签名包、无私钥构建 | 正常安装/运行，不依赖 keygen 或 proof。 |
| 包完整性 | 哈希错误、文件缺失、ZIP 路径穿越、重复项、解压膨胀、错误 manifest | 明确拒绝，不污染已安装版本。 |
| 宿主边界 | 普通包伪装 builtin、未知 builtin ID、伪造 core 组件、任意 DLL/进程入口 | 服务端拒绝，不能通过免签名绕过。 |
| 权限 | 新增权限、拒绝授权、跨 Package 访问、跨插件私有资源、伪造 caller | 默认拒绝未授权能力，无越权执行。 |
| UI 隔离 | 恶意 iframe/消息、未知动作、错误来源、宿主组件冒充 | 不能直接访问其他插件私有状态或任意 Host API。 |
| 官方市场 | 三插件列表、下载、安装、更新、404/网络失败、哈希错误 | 状态准确，视频插件可发现可使用。 |
| 用户插件 | 仓库外创建、构建、导入、启动、调用、保存数据、更新、卸载 | 不需要改 Gamer 源码或配置公钥。 |
| 视频无设备 | 导入、播放、精确逐帧、项目、标记、校准、离线匹配 | 不触发 ADB/设备截图。 |
| 视频输入 | 媒体模式鼠标/键盘/Keymap/舞台插件输入、切回实时 | 不误发真机，不恢复旧按键状态。 |
| 录制 | 后台录制、断连、编码变化、磁盘不足、重复停止、异常退出 | 不阻塞实时投屏，状态和已保存素材可诊断。 |
| 输入事件 | manual/keymap/runner/plugin、点击/滑动/按键、敏感文本 | 事件关联正确，不重复语义动作，文本默认脱敏。 |
| 帧与坐标 | 24/30/60fps、VFR、旋转、竖屏、黑边、不同参考尺寸 | 相邻帧准确，校准可重复，来源代次正确。 |
| 模板/YAML | 选定帧裁切、离线匹配、草稿生成/保存/编辑、缺少 YAML 插件 | 可重现，不自动执行，缺少依赖有提示。 |
| Package | 不含/包含媒体、缺失引用、覆盖/复制、卸载重装 | 引用正确，数据不丢失，大文件不重复默认复制。 |
| 平台 | Windows 完整包、Docker、直跑；真实设备/模拟器按可用环境 | 明确记录实际通过范围及未验证项。 |

### 12.2 正式测试与性能

- [ ] 执行仓库实际配置的 Rust 格式检查、静态检查、单元/集成测试、前端 Vitest、lint/build、Architecture Guard 与 Core shell boundary 测试。
- [ ] 使用当前仓库确定的正式构建命令构建 Windows 完整包、Docker 或可用目标；不要沿用已删除的旧脚本命令。
- [ ] 对录制关闭/开启、精确帧延迟、并发解码、CPU/内存、磁盘占用和长期录制建立真实基线；不凭空设定“所有 4K 视频必定实时逐帧”。
- [ ] 使用模拟目标或测试账号完成一次性操作流程，避免为了测试反复消耗真实游戏资源。
- [ ] 检查所有旧签名必需字段、proof 生成、开发密钥指引、虚假 WASM entry、过时 README/AGENTS/ADR 是否仍有残留。
- [ ] 仅清理确认不再使用的旧代码和脚本；不删除 launcher 更新签名、通用哈希校验或仍被使用的安全模块。

### 12.3 文档与最终交付

- [ ] 更新 README、AGENTS.md、ADR、插件开发指南、manifest/Host API/WIT 参考、市场发布说明和视频工作台用户指南。
- [ ] 文档明确“源码位置 ≠ 执行形态 ≠ 分发形态”，说明官方宿主插件与独立 WASM 插件的区别。
- [ ] 提供用户插件从零开发示例、无签名构建与本地安装教程、权限/运行时限制说明。
- [ ] 提供视频完整制作教程：录制/导入 → 项目 → 精确帧/校准 → 标记 → 模板 → 离线测试 → 草稿保存 → 实时验证。
- [ ] 更新原视频计划状态，逐项标明完成、范围调整、延期、未验证，不能只把所有 checkbox 一次性勾完。
- [ ] 输出最终验收报告，附 HEAD、测试命令、退出码、日志/截图、实际平台和剩余问题；未执行的测试明确标记未验证。

**最终验收：** 官方三个插件均可正常发布和安装；第三方用户不修改宿主即可开发、安装和使用自己的受控插件；插件强制签名流程已移除且安全边界保持；视频工作台能够完成原计划的完整制作流程；Package 分发和数据保留正确；正式构建、测试与实际平台证据齐全。

---

## 13. 阶段依赖、实施顺序与并行建议

```text
Phase 0  基线核对
   │
   ├─ Phase 1  免签名 + 市场修复 ──┐
   │        ↑ 依赖 Phase 2 的最小宿主契约修正
   └─ Phase 2  执行类型 / 注册 / Bridge ──┬─ Phase 3  第三方 SDK
                                         ├─ Phase 4  官方插件归属
                                         └─ Phase 5  Media/Recording 收口
                                                  │
                                                  └─ Phase 6  Video Project/时间轴
                                                           │
                                                           └─ Phase 7  模板/YAML 联动
                                                                    │
                                                                    └─ Phase 8  Package/更新
                                                                             │
                                                                             └─ Phase 9  E2E/收尾
```

建议实际执行时先完成 Phase 0，再将 Phase 2 中“宿主插件无 guest 契约”的最小改动与 Phase 1 合并，尽快修复市场中看不到视频插件的问题。随后 Phase 3 的 SDK、Phase 4 的官方插件归属审计、Phase 5 的媒体能力核验可在接口稳定后适度并行；Phase 6–8 按依赖完成纵向闭环。不要先把所有后端写完，再集中补前端和测试。

### 13.1 每阶段工作规则

1. 每个阶段先列出实际文件所有权、接口变更、数据变更、测试和验收点。并行开发时共用文件由集成者统一修改，避免多个 Agent 同时覆盖 `service.rs`、`host_api.rs`、`api/mod.rs`、`main.rs` 等热点文件。
2. 优先复用现有代码和契约；确需改变时先修改唯一权威文档和测试，再实施。不得因为旧计划写过某个文件名，就重新创建已经被替代的模块。
3. 业务功能采用可运行的纵向切片：接口 → 实现 → UI/SDK → 测试 → 文档 → 验收。不要只交付空 API、占位 UI 或 `todo!()`。
4. 当前允许破坏性修改，但必须清楚记录被删除的格式、API、配置和数据迁移方式；不保留无实际需求的双轨兼容层。
5. 任何“完成”都需要实际运行证据；测试失败、环境不具备或真实设备不可用时，不得推断为通过。
6. 不在本轮新增公共市场、Native 动态加载、复杂签名策略、AI 自动化识别等额外工作；新需求先记录，不挤占本轮交付。

### 13.2 Agent 任务与阶段报告格式

每个阶段建议在 `docs/evidence/` 下生成独立报告，并在总计划中维护以下状态：`TODO / IN_PROGRESS / BLOCKED / DONE / NOT_VERIFIED`。报告至少包含：实际 HEAD、完成的工作、修改文件、关键设计偏差、测试命令与结果、数据迁移说明、剩余问题、下一阶段依赖。若使用多个 Agent，先明确独占写权限和集成者；禁止不加区分地提交其他 Agent 的工作树改动。

### 13.3 最终交付清单

- [ ] 无强制签名的官方插件构建和 GitHub 发布链。
- [ ] 可用的 YAML、Keymap、Video 官方市场包及完整性清单。
- [ ] 明确的 builtin/WASM 执行类型和服务端宿主注册机制。
- [ ] 用户可独立构建、导入、调试的 SDK、示例和文档。
- [ ] 通用且受控的 UI Bridge、命令/能力调用、权限与运行时边界。
- [ ] 已完成的媒体/录制/离线帧通用能力及第三方可用接口。
- [ ] Video Project、精确时间轴、标记、校准、模板制作、离线测试和 YAML 草稿闭环。
- [ ] Package 可选媒体分发、引用生命周期及插件数据保留机制。
- [ ] 架构测试、功能回归、真实设备/平台 E2E、性能基线及最终验收报告。

---

## 14. 参考资料与来源说明

以下链接用于开发时核对现有实现；它们并不表示本计划提出的新功能已经存在。执行前应固定实际 HEAD，并优先使用该提交下的源码和文档。

### 用户提供的原计划

- 《Gamer 视频工作台插件开发计划》（本次会话上传文件：`gamer_video_workbench_development_plan(1).md`）。本计划保留其完整制作目标、Core/插件职责、媒体与 Package 分离及 Phase 0–6 的原始验收意图。

### 仓库架构与实施合同

- [Gamer 仓库](https://github.com/jesongit/gamer)
- [AGENTS.md](https://github.com/jesongit/gamer/blob/main/AGENTS.md)
- [V3 Package 与前端架构计划](https://github.com/jesongit/gamer/blob/main/docs/plans/gamer_v3_package_frontend_architecture_plan.md)
- [视频工作台原计划](https://github.com/jesongit/gamer/blob/main/docs/plans/gamer_video_workbench_development_plan.md)
- [视频工作台 V1 实施合同](https://github.com/jesongit/gamer/blob/main/docs/plans/gamer_video_workbench_contracts.md)

### 插件、市场与视频实现

- [扩展生命周期 service.rs](https://github.com/jesongit/gamer/blob/main/server/src/extensions/service.rs)
- [Manifest 解析 manifest.rs](https://github.com/jesongit/gamer/blob/main/server/src/extensions/manifest.rs)
- [归档校验 archive.rs](https://github.com/jesongit/gamer/blob/main/server/src/extensions/archive.rs)
- [插件签名 signature.rs](https://github.com/jesongit/gamer/blob/main/server/src/extensions/signature.rs)
- [官方插件构建脚本](https://github.com/jesongit/gamer/blob/main/tools/build-plugins.ps1)
- [视频插件 manifest](https://github.com/jesongit/gamer/blob/main/tools/plugins/gamer.video/manifest.toml)
- [视频宿主扩展](https://github.com/jesongit/gamer/blob/main/server/src/extensions/video/mod.rs)
- [当前 registry.json](https://github.com/jesongit/gamer/blob/main/web/public/registry.json)
- [视频工作台组件](https://github.com/jesongit/gamer/blob/main/web/src/components/video/VideoWorkbench.vue)
- [视频时间轴组件](https://github.com/jesongit/gamer/blob/main/web/src/components/video/VideoTimeline.vue)
- [视频草稿组件](https://github.com/jesongit/gamer/blob/main/web/src/components/video/VideoDraft.vue)

### 原计划的重要约束

原计划明确不伪造设备、不自动回放一次性操作、不要求所有视频达到 1080p、不恢复旧架构，并要求每阶段保留实际证据。本轮计划只是将插件分发和架构边界的新增决策合并进来，同时继续完成原计划中尚未形成闭环的内容，不以实施合同的简化 V1 范围替代最终产品目标。

---

## 15. 执行状态（2026-09-07，Phase 9 收口回写）

### 15.1 各 Phase 完成状态与关键提交

| Phase | 状态 | 关键提交（main） |
| --- | --- | --- |
| 0 基线核对 | DONE | `c4d4bf8`（计划入库 + 基线审计，8 处计划假设偏差修正，`docs/evidence/plugin_ecosystem_phase0_baseline.md`） |
| 1 免签名 + 市场修复 | DONE | `a925251`（服务端：signature.rs 全删 + `x-expected-sha256`）、`3dac760`（构建链去签 + registry v2 + video 首入市）、`9b6ea9c`（前端免签安装与执行形态展示）、`eae786c`（dev keypair 残留删除） |
| 2 执行类型/注册表/Bridge | DONE | `a925251`（manifest v2 `[execution]` + `extensions/builtin.rs` 注册表 + UI 贡献仅 Running + host_api 第 9 域 media） |
| 3 第三方 SDK | DONE | `fdffc75`（sdk/ 三示例 + plugin-dev.md + PLUGIN_API.md）、`6dbc5f1`（缺口1 修复：通用插件带 import 的 async 运行时 trap + 崩溃循环） |
| 4 官方插件归属收口 | DONE | `4a7439c`（keymap guest 转正 `server/guests/keymap-guest` + 机制层 id 特判收敛边界谓词 + 归属审计）、`92cf82c`（缺口2 修复：录事件来源标注 task-local scope） |
| 5 媒体/录制收口 | DONE | `7b85745`（展示序帧表 PTS/索引映射 + `/api/media/:id/frames*` + 录制 base_pts_us 时间轴映射 + 前端消灭 33ms 步进）；媒体库/录制本体 V1 在计划前已入库（`f859a7e`/`a1b04c7`） |
| 6 Video Project/时间轴 | DONE | `d44a959`（projects/*.json 资源 + 标记帧身份 + 校准四级坐标变换 + 工作台三分区重组） |
| 7 模板/离线测试/草稿闭环 | DONE | `db4d481`（`gamer_yaml/actions.rs` 动作清单唯一声明 + TemplateStudio 定帧建模板 + 草稿可编辑工作流 + `tools/e2e_phase7_offline.sh`） |
| 8 Package 分发/更新/数据生命周期 | DONE | `58f5f27`（归档 `media/**` 白名单 + include_media 导出 + 媒体引用闭环 + 更新执行类型语义 + keymap profile 门禁去 id 比较） |
| 9 统一验收/文档/清理 | DONE（2026-09-08） | 本节 + `docs/evidence/phase9_final_acceptance.md` + ADR-15 + AGENTS/README 收口 + api.js registryProof 死代码清理；门禁证据见验收报告；浏览器实机冒烟由集成者完成（`60eeac8`：三插件市场/免签安装/面板出现全链 PASS，发现并修复业务面板激活回归与 uiType 标签，`docs/evidence/phase9_browser_smoke.md`） |

### 15.2 与计划的偏差汇总（自各阶段 evidence 提炼）

1. **前端是第二道签名门禁**（Phase 0 偏差1）：计划只点名服务端；实际 `installPolicy`/`canInstallMarket` 在浏览器侧也阻断，Phase 1 前后端同步拆除。
2. **签名元数据在版本目录文件（signature.sig）而非已安装记录**（偏差2）：state.json 无签名字段，删 verifier 后旧文件死数据静默忽略，零迁移。
3. **`native_call_action` 特判的是 gamer.yaml 而非 gamer.video**（偏差3）：通用跨插件 RPC 注册表未做，Phase 7 以 `actions.rs` 公开动作清单（版本化唯一声明 + 清单↔分发双向锁测试）代替，覆盖 `template.create_from_frame`/`vision.test_template`/`automation.create_draft`/`automation.save_draft`/`automation.open_editor` 五动作。
4. **registry 元数据单源化**（偏差4）：build-plugins.ps1 硬编码条目元数据 → signer `inspect --meta-out` 以 manifest.toml 为唯一权威源。
5. **UI 贡献语义裁决**（偏差6）：Enabled|Running 可见 → **仅 Running**（stop 即撤面板，Enabled 不再出现半启用面板）。
6. **keymap guest 迁移**：`server/tests/keymap-guest` → `server/guests/keymap-guest`（Phase 4，wasm 代码段逐字节等价、仅符号改名）。
7. **TemplateStudio 视频域自实现**（Phase 7 偏差1）：TemplateCropModal 与 useConsoleTemplates 裁切子系统深耦合，不直接复用；语义等价（定帧冻结 + generation 校验 + 绝不保存时重抓）。
8. **host_api 第 9 域 media 补声明入口**（偏差5）：`[host_api] media` 键随 manifest v2 落地。
9. **视频包发布链重建**（偏差7）：plugin-signer pack 与签名解耦（`--key` 非必填、builtin 包零占位 WASM、`inspect --meta-out`、`verify` 自检），archive/store 的 entry=WASM 假设按执行类型分支。
10. **通用插件运行时两缺陷**（Phase 3 缺口）：带 host import 的插件 start/call 全链 trap + 进程 abort 崩溃循环（`6dbc5f1` exports default:async + catch_unwind 收敛）；录事件来源恒标 plugin（`92cf82c` task-local 调用方 scope）。
11. **`media_refs`/`media_total_bytes` 走 `GET /api/packages/:pkg` 详情响应**，未拆独立端点（api/mod.rs 路由一次定型）；详情变重可后续拆。
12. **sha256/大小为安装完整性主锚**：计划设想的「无签名但保留更强来源证明」未做替代实现，仅 `x-expected-sha256` 可选钉 + registry 条目 sha256（前端官方下载强制）。

### 15.3 NOT_VERIFIED 清单（截至 Phase 9）

- 真机 adb 场景：录制→草稿→真机显式运行全流程、start_app/投屏联动、多 viewer、看门狗交互（无设备环境）。
- ~~浏览器实机点检：市场安装全链手动冒烟、视频工作台 UI 人工走查~~（已完成：`docs/evidence/phase9_browser_smoke.md`，冒烟发现并修复业务面板激活回归；多页面互斥实机走查仍待真机环境）。
- 真实 GitHub Release 发布与下载链路：仅交付本地产物 + `sha256sums.txt`（可上传），未执行真实发布。
- 性能基线（§12.2）：录制开关开销、精确帧延迟、并发解码、CPU/内存、磁盘占用、长录基线——均未建立量化数据。
- Docker/直跑平台矩阵与真实双机 .gamerpkg 媒体分发人工链路。
- 多指/旋转/黑边素材上的模板制作人工体验（机制有坐标/校准测试覆盖）。
- `cargo test --release` 与 Linux（CI ubuntu）平台的本地复跑（本机为 Windows；CI 有等效工作流）。
