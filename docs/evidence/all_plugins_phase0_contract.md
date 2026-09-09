# 全插件正确性修复与交互优化：P0-C 公共实施合同

> 状态：已收敛，待按依赖图实施  
> 任务包：P0-C「契约冻结」  
> 审查基线：`b242c0d586c19bb0fdb996e05879e3f0459ef9ff`（`main`）  
> 本文件唯一写入目标：`docs/evidence/all_plugins_phase0_contract.md`  
> 生成日期：2026-09-09

## 0. 证据口径

- **CONFIRMED**：能由当前基线源码、现有测试输出或已完成 Phase 0 evidence 直接确认。
- **TO_IMPLEMENT**：计划要求的实施约束；不表示当前代码已经具备。
- **NOT_VERIFIED**：当前证据没有覆盖，或当前实现没有可确认的公开契约；本合同不补猜测、不把愿望写成接口。
- 本轮不改业务源码、已有测试或其他文档，不提交代码。

基线问题、源码位置和静态核对见 [Phase 0 baseline](all_plugins_phase0_baseline.md)；复现命令、通过结果及未覆盖范围见 [Phase 0 repro](all_plugins_phase0_repro.md)。复现证据确认了 20 条当前源码失败路径，但既有测试通过不等于这些失败路径已修复。[all_plugins_phase0_repro.md:421](all_plugins_phase0_repro.md:421)

## 1. 任务所有权与独占写入范围

### 1.1 总规则

| 规则 | 合同 |
|---|---|
| 单任务单范围 | 一个任务只能修改下表自己的文件集合；发现需要跨界修改时，停止并转交集成任务。 |
| 测试随代码 | 负责源码的任务同时负责其专属测试；测试不能被另一个任务顺手重写。 |
| 共享热点 | `server/src/api/mod.rs`、`server/src/main.rs`、架构边界测试和跨域装配只由 `P6-INTEGRATOR` 合入。 |
| 现有改动 | 保留基线工作树中已有的 Phase 0 evidence、计划文档和 `server/data/`；不得使用全量暂存或清理命令。 |
| 本任务 | P0-C 仅写本文件；不写源码、测试、计划、AGENTS 或其他 evidence。 |

### 1.2 计划任务所有权

以下是实施顺序使用的公开拆分；它是**任务边界**，不是现状声明。

| 任务 | 独占范围 | 依赖/交付 |
|---|---|---|
| P0-A | `docs/evidence/all_plugins_phase0_baseline.md` | 基线事实 |
| P0-B | `docs/evidence/all_plugins_phase0_repro.md` 及其独立 fixture | 复现证据 |
| P0-C | `docs/evidence/all_plugins_phase0_contract.md` | 本公共合同 |
| P1-T | 模板 composable 及专属测试 | 模板引用/删除/创建链路 |
| P1-VREF | `web/src/components/video/VideoWorkbench.vue`、`videoApi.js` 及专属测试 | 媒体引用同步 |
| P1-VFRAME | `web/src/components/video/VideoTimeline.vue` 及专属测试 | 帧身份/定帧 |
| P1-VDRAFT | `web/src/components/video/VideoDraft.vue` 及专属测试 | 草稿请求与保存 |
| P1/P4-K | `web/src/components/console/useConsoleKeymap.js`、`KeymapPanel.vue` 及专属测试 | Keymap 交互 |
| P2-BE | `server/src/extensions/service.rs` 及生命周期测试 | 生命周期编排 |
| P2-UI | `web/src/workspace/plugin-center/PluginCenter.vue` 及专属测试 | 插件操作 UI |
| P3-SCHEMA | YAML schema/parameter 相关模块及测试 | 参数契约 |
| P3-STEP | StepCard/CellEditor/AddStepPanel 及测试 | 步骤编辑 |
| P3-FORM | ParamEditor/ParamsForm 及测试 | 参数表单 |
| P3-LIB | 函数库、`useConsoleScriptRunner`、ScriptRunner 及测试 | 函数库流程 |
| P3-CONTEXT | `web/src/workspace/context.ts`、runner/resource 相关模块及测试 | Context 分离 |
| P5-MEDIA | video/recording 媒体 UI 及测试 | 媒体库/录制入口 |
| P5-VPROJECT | 项目模块及测试 | 项目资源/版本 |
| P5-VWORKFLOW | 时间轴、TemplateStudio、校准模块及测试 | Stage 工作流 |
| P5-VDRAFT | 视频草稿工作流模块及测试 | 录制到草稿 |
| P5-STAGE | `web/src/components/console/useConsoleStage.js`、Stage 测试 | live/media Stage |
| P6-WEB | 独立 Web 集成测试/fixture | Web 集成证据 |
| P6-SERVER | 独立 Rust 集成测试 | Server 集成证据 |
| P6-E2E-DOC | evidence、reference/guides 文档 | 端到端证据 |
| P6-INTEGRATOR | `server/src/api/mod.rs`、`server/src/main.rs`、架构守卫、最终状态汇总 | 跨域装配与收口 |

以上任务分解和顺序来自 [开发计划的任务包表](../plans/gamer_all_plugins_correctness_ux_fix_plan.md:1152) 及 [依赖顺序](../plans/gamer_all_plugins_correctness_ux_fix_plan.md:1179)。视频域已有的更细所有权矩阵见 [视频合同](../plans/gamer_video_workbench_contracts.md:9)。

## 2. Runtime Context 分离

### 2.1 当前已确认模型

服务端 `AppContext` 当前只有四层中的 Device、App、Package 数据：

| Context | 当前权威字段/含义 | 证据 |
|---|---|---|
| Device | `device_id`，运行目标设备 | [`DeviceId` / `AppContext`](../../server/src/core/models.rs:118) |
| App | `android_package`，Android 安装域和启动/停止目标 | [`AppContext` 权威注释](../../server/src/core/models.rs:141) |
| Package | `content_package`，资源解析和内容上下文 | [`AppContext` 权威注释](../../server/src/core/models.rs:151) |
| Plugin | 调用方扩展身份，不进入 `AppContext` | [`AppContext` Plugin 边界](../../server/src/core/models.rs:161) |
| Stage | 当前不是服务端 `AppContext` 字段；前端 Stage 管理 live/media 来源、代次、帧和输入许可 | [`useConsoleStage` StageSource 注释](../../web/src/components/console/useConsoleStage.js:5) |

服务端明确要求 `android_package` 与 `content_package` 是不同命名空间，不能互相推导或兜底；`RunRequest` 还校验请求 `device_id` 与 `app.device_id` 一致。[AppContext 字段](../../server/src/core/models.rs:170)、[RunRequest 校验](../../server/src/core/models.rs:266)

前端 Package store 已采用 `deviceId`、`androidPackageName`、`currentPackageId`、`activePluginId` 四个命名。[package-store.js](../../web/src/package-store.js:1) 但当前 `createWorkspaceContext` 仍把 `activePackage` 放进 `app.package`，且 snapshot 未呈现明确的 `content_package` 和 Plugin 字段。[context.ts](../../web/src/workspace/context.ts:34) 这属于已确认的对齐缺口，不可宣称已经完成分离。

### 2.2 待实施公共约束

- Device 只承载 `device_id/deviceId`；App 只承载 `android_package/androidPackageName`；Package 只承载 `content_package/currentPackageId`；Plugin 只承载当前扩展身份/调用方 scope；Stage 只承载当前显示源和帧时钟。
- 任何任务不得从 Package ID 推导 Android 包名，也不得用 Android 包名兜底 Package ID。测试辅助构造器即使使用相同字符串，也不能作为生产映射依据；当前 `AppContext::for_test` 只能证明存在测试 helper。[for_test](../../server/src/core/models.rs:176)
- Stage source 切换不得改变 Device、App、Package、Plugin 或运行任务；media Stage 必须拒绝设备输入。当前 Stage 已有 `canDeviceInput` 和输入守卫。[Stage 输入守卫](../../web/src/components/console/useConsoleStage.js:145)
- 需要跨 Context 的调用必须显式传递对应字段；本合同不新增未被当前源码证明的 REST/WASM 接口。

## 3. 异步请求代次与失效规则

### 3.1 当前证据

- Stage source 切换当前递增 `generation`，并清理媒体播放/帧状态；媒体帧来自服务端帧表，不假设固定 FPS。[Stage source 切换](../../web/src/components/console/useConsoleStage.js:170)、[逐帧请求](../../web/src/components/console/useConsoleStage.js:285)
- Package store 有 `loading/loadError`，但当前证据未证明它对并发加载具备代次淘汰。[package-store.js](../../web/src/package-store.js:31)
- VideoWorkbench 的项目列表、打开项目和媒体引用同步存在请求先后风险；VideoDraft 的事件加载只有 `loadedOnce`，没有可由现有源码确认的请求序列。[VideoWorkbench load/open](../../web/src/components/video/VideoWorkbench.vue:218)、[VideoDraft load](../../web/src/components/video/VideoDraft.vue:157)
- P0-B 明确记录：异步重排尚无 deferred 测试锁定。[repro NOT_VERIFIED](all_plugins_phase0_repro.md:428)

### 3.2 待实施逻辑合同

每个异步数据流都必须逻辑上绑定以下上下文快照（这是前端/服务端实现约束，不是新增 wire DTO）：

```text
{ device_id?, android_package?, content_package?, plugin_id?,
  resource_id?, resource_version?, draft_revision?, media_id?,
  stage_generation?, calibration_version?, request_seq }
```

- 同一流的 `request_seq` 单调递增；切换 Device/App/Package/Plugin/Stage source 时先使旧序列失效，再发新请求。
- 响应写状态前必须同时匹配流 scope、关键资源身份和当前代次；不匹配的响应只能丢弃，不能覆盖新上下文。
- `finally` 只能清除自己 token 对应的 loading/saving；旧请求结束不得清除新请求的忙状态。
- 空列表、加载失败、旧数据和请求被淘汰必须分开表示；不能以空列表代替错误。当前 VideoWorkbench 失败时会把摘要置空，故此规则属于待实施修正。[当前失败处理](../../web/src/components/video/VideoWorkbench.vue:218)
- 任何实现不得依赖网络返回顺序；必须增加至少覆盖“旧请求晚于新请求返回”的 deferred 测试。测试尚未存在，当前状态为 `NOT_VERIFIED`。

## 4. 保存、版本和冲突状态

### 4.1 当前已确认契约

- Package manifest 使用 `expected_revision`/`force`；既有资源更新使用 `expected_version`/`force`。资源版本和 Package revision 由 `PackageStore` 检查，冲突映射为 HTTP 409。[manifest 更新](../../server/src/resources.rs:688)、[文本资源更新](../../server/src/resources.rs:785)、[API 错误映射](../../server/src/api/packages.rs:1205)
- `force=true` 是显式跳过并发检查的现有入口；当前证据不支持任何自动 force 或自动覆盖结论。
- VideoWorkbench 保存使用 `expectedVersion`，遇 409 保留冲突错误；但保存成功后媒体引用同步是后续步骤，且同步失败没有阻止保存。[项目保存](../../web/src/components/video/VideoWorkbench.vue:356)
- 媒体引用接口是全量替换 `POST /api/media/:id/refs`，body 是 `refs: MediaRef[]`。[媒体 refs API](../../server/src/api/media.rs:429)

### 4.2 待实施状态机

业务编辑器至少要能区分：

`clean` → `dirty` → `saving(token)` → `saved`  
保存异常分支：`save_failed`、`version_conflict`、`saved_sync_pending`、`saved_sync_failed`。

- `dirty` 草稿必须留在本地状态；保存失败和版本冲突不能丢草稿。
- 旧保存成功不能把更新后的草稿标成已保存；保存结果必须匹配提交时的资源/版本/草稿 token。
- 版本冲突必须让用户选择重新加载/重做等既有产品路径；本合同不发明合并 API。
- 项目内容保存成功但媒体 refs 同步失败，必须显示“内容已保存、引用未同步”的独立状态，并可重试；不得伪装成完整成功，也不得静默清空错误。
- Package/资源切换时，有未保存草稿必须阻止切换或明确丢弃；当前完整 UI 守卫覆盖情况为 `NOT_VERIFIED`。

## 5. 插件生命周期和操作状态

### 5.1 当前已确认状态和操作

服务端快照包含 `active_version`、`installed_versions`、`state`、`last_error` 等字段。[扩展快照](../../server/src/api/extensions.rs:360) 当前生命周期枚举/持久化路径覆盖 `Installed`、`Enabled`、`Running`、`Disabled`、`Failed`；`enable_and_start` 把启用意图和启动合并，`disable` 在 Running 时停止实例。[enable_and_start](../../server/src/extensions/service.rs:1128)、[disable](../../server/src/extensions/service.rs:1172)

当前还可确认：

- 安装成功后 API 会自动尝试 enable → start，失败时保留 Enabled 和 `last_error`。[安装自动启动](../../server/src/api/extensions.rs:324)
- start 会检查必需依赖、循环和运行时能力；stop/disable 会注销 runner；UI registry 只注册 Running 扩展，Enabled 不出业务 UI。[start 依赖与注册](../../server/src/extensions/service.rs:1208)、[UI registry](../../server/src/extensions/service.rs:1690)
- 同一扩展操作有 operation lock/call gate；Running 更新会被拒绝，Running 卸载也不会自动 stop。[update](../../server/src/extensions/service.rs:1015)、[uninstall](../../server/src/extensions/service.rs:1578)
- 重启会把无进程的持久化 Running 修复为 Enabled 并尝试恢复；恢复失败保留 Enabled + error，不阻断服务启动。[reconcile_startup](../../server/src/extensions/service.rs:1471)
- 现有 PluginCenter 的更新/卸载 UI 没有已确认的 stop/restore 编排；该缺口在 M01 复现中仍存在。[PluginCenter 操作](../../web/src/workspace/plugin-center/PluginCenter.vue:324)、[M01](all_plugins_phase0_repro.md:364)

### 5.2 待实施操作合同

用户可见操作仍限于安装、启用、禁用、更新、卸载；内部 start/stop 只作生命周期实现。操作必须：预检依赖/权限/版本 → 必要时阻止新调用 → 按依赖逆序停止 → 应用版本/数据变更 → 按依赖正序恢复原意图 → 刷新快照和 UI。

操作进度的逻辑状态为：

`preflight`、`await_confirmation`、`blocking_new_work`、`stopping`、`applying`、`restoring`、`completed`、`partial_failure`、`failed`。

这些是待实施的公共展示语义；当前 API 没有由源码确认的 operation id/进度资源，故其 wire 形态为 `NOT_VERIFIED`。现有 `state/last_error` 只能表示生命周期快照，不能被冒充为完整操作进度。

- 同一插件操作串行、可重试、幂等；刷新页面后可从服务端快照恢复显示。
- 必需依赖停止采用依赖方优先、恢复采用被依赖方优先；不自动下载、自动启用、级联停用。
- 更新/卸载不能在 YAML 运行中重跑脚本；Keymap 必须释放输入，不恢复被按住的键；Video 必须安全结束/保存录制会话。当前真实插件运行中的全部边界未经过设备实测，标 `NOT_VERIFIED`。

## 6. 媒体引用、确定帧和录制会话边界

### 6.1 当前已确认

**媒体引用。** Core 的 `MediaRef` 是 `(package_id, plugin_id, kind)`；metadata.json 保存 refs，非空引用删除返回冲突，Package 删除调用 release。[MediaRef](../../server/src/media/mod.rs:30)、[删除保护](../../server/src/media/mod.rs:440)、[release_package](../../server/src/media/mod.rs:791) Core 不解释项目 JSON；Video Project 的资源语义归 `gamer.video` 扩展。[媒体职责](../../server/src/media/mod.rs:1)

**确定帧。** Core 维护按真实 PTS 排序的 frame table，按 `frame_index` 或 `pts_us` 解析，响应带 `X-Frame-Index` 与 `X-Frame-PTS-US`；不使用固定 FPS 推算。[帧模型](../../server/src/media/mod.rs:147)、[帧响应头](../../server/src/api/media.rs:305) 项目标记当前实际包含 `media_id`、`frame_index`、`pts_us`、`calibration_version`。[VideoTimeline 标记](../../web/src/components/video/VideoTimeline.vue:423)

**Stage。** live 与 media 共用舞台状态；media 输入被拒绝；用户 seek 会清除当前确定帧，程序化 seek 保留它。[seek 处理](../../web/src/components/console/useConsoleStage.js:225) 当前 Timeline 的 `onSeeked` 不清除 `currentFrame`，该差异已由 VFRAME 负责，修复前不宣称帧身份已全链路稳定。[VideoTimeline seek](../../web/src/components/video/VideoTimeline.vue:333)

**录制。** RecordingService 只消费 scrcpy 编码帧和注入收敛点的输入观察，不负责 YAML/replay；状态包含 recording、finalizing、completed、interrupted、failed、cancelled；每设备一个活动会话，浏览器断开不结束服务端录制，断连/编码变化/磁盘边界会分段或中断。[Recording 职责/状态](../../server/src/recording/mod.rs:1)、[会话启动](../../server/src/recording/mod.rs:850)、[设备边界](../../server/src/recording/mod.rs:1042)

录制事件当前含 `operation_id` 去重字段、`session_id`、source（manual/keymap/runner/plugin）、时间域、坐标空间和 display size；文本默认脱敏等更细规则以源码为准。[InputEventRecord](../../server/src/recording/mod.rs:113) REST 入口为 `/api/recording/start`、`/active`、`/:id/stop`、`/:id/cancel`、`/:id/status`、`/:id/events`。[录制路由](../../server/src/api/recording.rs:19)

### 6.2 待实施边界

- 引用身份必须始终使用完整 `MediaRef` 三元组；项目保存/删除必须以当前项目完整引用集合做全量替换，并把内容保存与 refs 同步结果分开呈现。
- 确定帧身份至少绑定 `media_id + frame_index + pts_us`；涉及视频项目时同时绑定校准版本。Stage source `generation` 只用于淘汰旧异步结果，不能替代媒体帧身份。
- 保存模板、创建标记或生成草稿时必须复用用户选定的确定帧，不能保存时重抓最新 live 帧；媒体模式继续只读。
- 录制会话由服务端权威管理；浏览器断开、面板销毁或页面刷新不得隐式 cancel。设备断连、编码变化、磁盘压力是会话/分段边界，恢复必须由服务端状态决定。
- 当前源码未证明跨 PackageStore、MediaService、项目资源和任务预设之间具备单事务原子性；这部分为 `NOT_VERIFIED`，不得在任务交付中声称“跨域原子提交”。
- 当前证据没有浏览器、真实设备、VFR/B 帧、真实录制、媒体引用删除保护和跨设备分发验证；均保持 `NOT_VERIFIED`。[repro 未覆盖项](all_plugins_phase0_repro.md:428)

## 7. P0 → P6 依赖图

```text
P0-A 基线 ─┐
          ├─> P0-C 契约冻结
P0-B 复现 ─┘
                 │
                 ├─> 并行：P1-* 前端局部修复
                 ├─> 并行：P2-BE 生命周期后端
                 ├─> 并行：P3-SCHEMA / P3-CONTEXT
                 │
                 └─> 并行：P2-UI / P3-STEP / P3-FORM / P3-LIB
                                      │
                                      └─> 并行：P5-MEDIA / P5-VPROJECT /
                                          P5-VWORKFLOW / P5-VDRAFT
                                                          │
                                                          └─> P5-STAGE
                                                                        │
                                                                        └─> 并行：P6-WEB / P6-SERVER / P6-E2E-DOC
                                                                                                      │
                                                                                                      └─> P6-INTEGRATOR
```

可并行只表示文件和契约不冲突；共享热点仍须按第 8 节集成。完整顺序由计划直接确认：[计划依赖图](../plans/gamer_all_plugins_correctness_ux_fix_plan.md:1179)。

## 8. 热点文件集成规则

- `server/src/api/mod.rs` 是 REST 装配热点；各域任务只提供域内实现和测试，路由合并由 P6-INTEGRATOR 完成。
- `server/src/main.rs` 是组合根热点；仅由 P6-INTEGRATOR 接入服务、恢复和共享状态。
- `server/src/extensions/service.rs` 是生命周期热点；P2-BE 独占开发，其他任务不得并行改写。
- `web/src/workspace/plugin-center/PluginCenter.vue` 是插件 UI 热点；P2-UI 独占开发，P6-WEB 只通过专属集成测试消费其公开行为。
- `web/src/components/console/useConsoleStage.js` 是 live/media/recording 交汇热点；P5-STAGE 独占修改，Video 子任务通过既有 `videoApi`/事件契约对接。
- `web/src/components/video/VideoWorkbench.vue` 同时涉及项目版本、媒体 refs 和 Stage 请求；P1-VREF/P5-VPROJECT 不得交叉编辑同一提交，冲突交给 P6-INTEGRATOR。
- 架构边界测试必须继续锁住 Core 与 YAML/Keymap/具体 UI 的隔离；服务端守卫入口见 [`architecture_guard_tests.rs`](../../server/src/architecture_guard_tests.rs:1)，Web 守卫见 [`core-shell-boundary.test.js`](../../web/src/core-shell-boundary.test.js:7)。
- 每个任务交付必须列出实际改动文件、测试命令和退出码；未运行的浏览器、设备、媒体或完整 cargo 验证必须显式写 `NOT_VERIFIED`。

## 9. 错误状态与 `NOT_VERIFIED` 口径

### 9.1 已确认错误映射

| 域 | 当前可确认的状态/错误 |
|---|---|
| Package/资源 | 版本缺失或冲突、revision 冲突 → 409；内容非法 → 400；资源/包不存在 → 404；资源更新支持显式 `force`。 |
| Media | 不存在/帧不存在 → 404；仍被引用 → 409 `media_referenced`；不支持 → 415；参数非法 → 400。 |
| Recording | 不存在 → 404；忙/离线 → 409；参数非法 → 400；内部错误 → 500。 |
| Extension | 未安装/版本/UI 不存在 → 404；非法转换、依赖、权限/Host feature、已安装 → 409；调用参数非法 → 400；运行时不可用 → 503；其余运行时/IO → 500。 |
| Web video | `VideoApiError` 保留 HTTP status、code、data；网络失败使用 `network_error`。 |

Package 映射见 [`packages.rs`](../../server/src/api/packages.rs:1205)，Media/Recording 映射见 [`media.rs`](../../server/src/api/media.rs:63) 与 [`recording.rs`](../../server/src/api/recording.rs:36)，Extension 映射见 [`extensions.rs`](../../server/src/api/extensions.rs:436)，Web 错误载体见 [`videoApi.js`](../../web/src/components/video/videoApi.js:13)。

### 9.2 交互状态不得混淆

实现和 evidence 必须分别记录：加载中、加载失败、无数据、旧数据/请求淘汰、保存中、保存失败、版本冲突、内容保存成功但同步失败、权限不足、依赖不可用、生命周期快照和生命周期操作进度。HTTP 成功只能证明该请求成功，不能证明后续 refs 同步、UI 刷新、设备执行或跨域事务成功。

以下内容在当前证据中均为 `NOT_VERIFIED`，除非后续任务补充可复现证据：

- 浏览器真实交互、跨浏览器 viewer 仲裁、WebRTC/ADB 真机链路；
- 真实 Package/媒体/录制数据上的 refs 全生命周期、VFR/B 帧和确定帧一致性；
- 有活动 runner/录制会话时的插件更新、卸载、依赖逆序停启；
- 异步延迟重排、刷新恢复、保存与 refs 同步的组合竞态；
- 完整 `cargo test`、官方插件构建/安装、WASM guest 实际运行及跨设备发行；
- 当前 `WorkspaceContext` 是否已被所有调用方完整改造成四 Context + Stage 分离；
- 操作进度是否存在稳定的服务端 operation id、持久化或恢复接口。

任何交付不得用“测试通过”“接口返回 2xx”或“静态代码看起来正确”替代上述验证；未覆盖就写 `NOT_VERIFIED`，并注明需要的命令、fixture、浏览器、设备或媒体条件。

## 10. P0-C 结论

本合同冻结的是实施边界和证据口径，不宣称 P0/P1/P2/P3/P5/P6 已完成。当前能确认的公共事实是：服务端已有严格的 AppContext 命名空间注释、PackageStore 乐观版本、MediaRef/真实帧表、Recording 会话状态、Extension 生命周期快照与依赖门禁；同时前端仍存在 Context 对齐、异步淘汰、媒体引用参数/时序、确定帧 seek 和插件更新/卸载编排缺口。后续任务必须按本合同和依赖图补实现、补测试、补 evidence；不能把 `NOT_VERIFIED` 改写为成功结论。
