# All Plugins Phase 0 基线审计证据

## 基线与审计边界

- 审计目录：`E:\code\gamer`
- 审计 HEAD：`b242c0d586c19bb0fdb996e05879e3f0459ef9ff`
- 分支：`main`
- 审计依据：当前工作树源码；已读取 `AGENTS.md`、总计划 `docs/plans/gamer_all_plugins_correctness_ux_fix_plan.md`、YAML/Keymap/Plugin 参考文档及对应前后端源码和测试入口。
- 总计划中的“待实施”仅作为问题清单，不作为当前实现状态。
- 审计方式：静态源码/测试入口核对；本次未运行前端、服务端、浏览器、设备或插件运行测试。

审计开始前工作树已有以下未提交内容，均未修改、未删除、未提交：

- `?? docs/plans/gamer_all_plugins_correctness_ux_fix_plan.md`
- `?? server/data/`（保留既有 `gamer.db`、`gamer.db-shm`、`gamer.db-wal`、`pending_restore.json` 及 `extensions/`、`media/`、`packages/`）

本文件是本次唯一新增写入。

## 状态口径

- **仍存在**：当前源码已有直接证据表明问题路径仍成立。
- **部分修复/仍存在**：已有分支修复，但仍有同一问题的有效路径。
- **已修复待回归**：静态源码已符合预期，但尚未通过本次回归执行确认。
- **需要复现**：静态证据不足，需要运行时或交互复现才能定论。
- **不适用**：当前版本/产品形态不包含该问题。

## 官方插件盘点

当前 `tools/plugins/` 下确认有三个官方插件 manifest：

| 插件 | 版本 | 执行类型 | manifest | SHA-256 |
|---|---:|---|---|---|
| `gamer.yaml` | `3.1.1` | WASM | `tools/plugins/gamer.yaml/manifest.toml` | `A041C9403D46B60B96CA9882C4DEFAA8646C00107CBE654A46E2333EF69224E7` |
| `gamer.keymap` | `1.0.1` | WASM | `tools/plugins/gamer.keymap/manifest.toml` | `246A32E7E7556EC33190F3079AAEAFF27D1A6D4233EE7593A864CF4227D27FE5` |
| `gamer.video` | `1.0.0` | builtin | `tools/plugins/gamer.video/manifest.toml` | `8D5B4C11D09DF74DAA7477B6C222CA811AA3CCBBA455DF3F1EFAE301E718926A` |

相关执行/分发入口已确认：`tools/build-plugins.ps1`、`web/public/plugins/`、`web/public/registry.json`、服务端 `server/src/extensions/builtin.rs`；WASM guest 入口为 `server/guests/yaml-guest` 与 `server/guests/keymap-guest`，视频插件无 guest。

## 逐项基线结论

### A：gamer.yaml 脚本编辑器

| ID | 优先级 | 当前状态 | 已确认源码证据 | 测试/缺口 |
|---|---:|---|---|---|
| A01 | P0 | 仍存在 | `web/src/script-editor/components/StepCard.vue:398-405` 的 `argsFromDecls` 只把有默认值的声明加入 args；无默认值的必填参数不会自动显示，需手工 `addArg`。 | 有 `step_card`/`params_form` 测试入口，但未见无默认必填参数的失败回归；未运行。 |
| A02 | P0 | 仍存在 | `StepCard.vue:411-415` 将 schema 类型原样作为 `argType`；`CellEditor.vue` 只识别 `tmpl/coord/time/number/key/bool/expr/text`，formal `boolean/duration/point/template/list/object` 等会落入错误或文本控件。 | 有 `cell_editor`/`step_card` 测试入口；未见正式类型到控件的完整映射回归；未运行。 |
| A03 | P1 | 仍存在 | `web/src/script-editor/components/AddStepPanel.vue:116-118` 的 `insertCall` 直接 `createCall(fn)` 后插入，没有走 schema/default 初始化；`StepCard` 另有 `applyFn` 初始化路径。 | 有 `add_step_panel` 与 `step_card` 测试入口；未见两条入口共享同一 schema 初始化的回归；未运行。 |
| A04 | P1 | 仍存在 | `StepCard.vue:458-463` 的 `toValueArg`/`toMapArg` 分别直接覆盖为新 cell/空 entries，切换标量与命名参数形态会丢失已有值。 | 有 `step_card`/model 测试入口；未见形态切换保值失败回归；未运行。 |
| A05 | P1 | 仍存在 | `StepCard.vue:374-396` 只用切换前 `prev` 比较 `props.step.fn`；快速 A→B→C 时 B 请求返回期间当前值仍可能是 A，不能阻止旧响应覆盖 C 的最终选择。 | 有 `step_card` 测试入口；未见请求序列/旧响应覆盖回归；未运行。 |
| A06 | P1 | 仍存在 | `web/src/script-editor/components/ParamEditor.vue:113-124` 明确将 list/object 映射为 text；`ParamsForm.vue:76-87` 将其映射为 expr，`CellEditor.vue:351-357` 只把布尔/数字解析为原子值，其余为字符串。 | 有 `params_form`/`cell_editor` 测试入口；未见 list/object 默认值类型保持回归；未运行。 |
| A07 | P1 | 仍存在 | `useConsoleTemplates.js:767-771` 的 CellEditor bridge 只传模板名；`1049-1058` 的 `testMatch` 只组装模板名、设备、threshold、region、package，未携带当前步骤完整 args/schema。 | 有 `cell_editor` 模板匹配测试，但断言只调用 `matchTemplate('login.png')`；未见实际步骤参数传递回归；未运行。 |

### T：模板保存/覆盖

| ID | 优先级 | 当前状态 | 已确认源码证据 | 测试/缺口 |
|---|---:|---|---|---|
| T01 | P0 | 仍存在 | `web/src/components/console/useConsoleTemplates.js:542-578` 的 `overwriteTemplate` 先 `api.deleteTemplate`，再 `api.createTemplate`；新文件创建失败时旧模板已被删除。服务端虽支持资源 PUT 的版本/force 语义，但该覆盖路径未使用安全替换。 | 有 `template-upload.test.js`、`template-crop-modal.test.js`、`template-studio.test.js`；未见“删除成功/创建失败后旧文件仍在”的失败回归；未运行。 |

### K：gamer.keymap

| ID | 优先级 | 当前状态 | 已确认源码证据 | 测试/缺口 |
|---|---:|---|---|---|
| K01 | P1 | 仍存在 | `web/src/components/console/useConsoleKeymap.js:128-163` 对 `hold` 与 `swipe` 先读取 `action.from`；hold 规则使用 `action.at`，因此会提前返回，overlay 标记丢失。 | 有 `keymap-control.test.js` 的 hold 输入测试；未见 hold overlay 使用 `at` 的回归；未运行。 |
| K02 | P0 | 仍存在 | `useConsoleKeymap.js:99-126` 的详情请求未递增/校验请求序列或 Package 上下文；快速切换 scheme 或切包时旧 `getKeymap` 响应可写回当前状态。列表请求虽有 `keymapLoadSerial`，详情请求没有同等保护。 | 有 `keymap-panel.test.js` 等入口；未见快速切换/跨 Package 旧响应回归；未运行。 |
| K03 | P1 | 仍存在 | `useConsoleKeymap.js:82-97` 的 `loadKeymaps` 无条件 `resetKeymapSelection`；`175-200` 的保存成功后强制 `loadKeymaps` 并 `onKeymapChange`，刷新与保存/应用耦合。 | 有 panel/control 测试入口；未见刷新保留选择、保存不隐式 apply 的回归；未运行。 |
| K04 | P1 | 仍存在 | `useConsoleKeymap.js:215-250` 暴露 `loading`/`keymapLoading`，没有 `saving`；`KeymapPanel.vue:136-139` 虽按 `saving` 禁用保存按钮，但父级 context 未提供该状态，无法防止重复保存。 | 有 `keymap-panel.test.js` 保存 payload 测试；未见双击保存/父子 saving 状态联动回归；未运行。 |

### V：gamer.video

| ID | 优先级 | 当前状态 | 已确认源码证据 | 测试/缺口 |
|---|---:|---|---|---|
| V01 | P0 | 仍存在 | `VideoWorkbench.vue:399-407` 组装 snake_case `{package_id, plugin_id, kind}`；`videoApi.js:360-372` 的 `setMediaRefs` 期望输入 camelCase `packageId/pluginId` 后再序列化，集成层契约不一致。 | `video-api.test.js` 只覆盖 camelCase 输入适配；Workbench 测试 mock 了 `setMediaRefs`，未锁定真实对象契约；未运行。 |
| V02 | P0 | 仍存在 | `VideoWorkbench.vue:360-372` 在 `loadProjects()` 前调用 `syncProjectMediaRefs`；`383-407` 从旧的 `projectSummaries` 计算引用集合，可能按旧项目列表同步。 | 有 `video-workbench.test.js` 项目/媒体引用测试；未见刷新顺序失败回归；未运行。 |
| V03 | P1 | 仍存在 | `VideoWorkbench.vue:367` 先把 `projectDirty` 置 false；同步失败 `408-412` 提示“重新保存”，但保存按钮 `64-70` 又因 `!projectDirty` 禁用，用户没有可用重试入口。 | 有保存成功/部分媒体状态测试；未见引用同步失败后的可重试回归；未运行。 |
| V04 | P0 | 仍存在 | `VideoTimeline.vue:340-359` 在 timeupdate/seeked 时只更新 currentTime，不清空 `currentFrame`；`361-480` 后续 marker/template 创建仍可使用旧 exact frame。 | 有 timeline marker/calibration 测试；未见 seek 后旧 frame identity 失效回归；未运行。 |
| V05 | P0 | 仍存在 | `VideoDraft.vue:157-163` 未对 Package/source 切换清理旧 session；`169-187` 加载事件无请求序列/来源校验，旧请求返回可覆盖新选择。成功响应只在返回后重置 selection，不能覆盖切换期间的竞态。 | `video-workbench.test.js:711-850` 覆盖加载/生成/保存及 prop 自动加载；未见 source 切换和旧请求竞态回归；未运行。 |

### M：插件中心与生命周期

| ID | 优先级 | 当前状态 | 已确认源码证据 | 测试/缺口 |
|---|---:|---|---|---|
| M01 | P1 | 仍存在 | `PluginCenter.vue:324-345` 更新直接调用 `updateExtension`，`405-415` 卸载直接调用 `uninstallExtension`；服务端 `server/src/extensions/service.rs:1015-1065` 对 Running 更新明确拒绝，卸载路径也依赖生命周期门禁，UI 没有自动 stop 后恢复原运行态的流程。disable 路径虽可先 stop，但未被 update/uninstall 复用。 | 服务端 `service.rs` 有生命周期、依赖、reconcile 测试；前端有 `plugin-center.test.js` API/helper 入口；未见 update/uninstall 运行态自动 stop/恢复的端到端回归；未运行。 |
| M02 | P2 | 部分修复/仍存在 | 已安装卡片的 `marketUpdate`（`PluginCenter.vue:236-239`）会比较 registry 与已安装版本；但市场卡片（`51-53`）只要存在 installedVersion 就显示“更新”，未比较版本关系，因此同版本/较旧市场版本仍会被标为更新。 | `plugin-center.test.js` 有版本/helper 入口；未见市场卡片同版、低版、高版三态回归；未运行。 |
| M03 | P2 | 部分修复/仍存在 | 安装/更新路径已把 notice 放到 `refresh()` 后（`PluginCenter.vue:324-345`）；但通用 `runAction` 在 `386-397` 先设置 notice，再调用 `refresh()`，而 `refresh()` 的 `219-229` 会 `clearMessages()`，enable/disable 成功提示仍会被清掉。 | `plugin-center.test.js` 有 API/helper 入口；未见 refresh 后 notice 保留的组件回归；未运行。 |

## 已确认的测试入口

| 范围 | 入口 |
|---|---|
| 前端全量/定向 | `web/package.json`：`pnpm test`、`pnpm test:run`；可定向 Vitest 文件或测试名。 |
| YAML 编辑器 | `web/src/script-editor/__tests__/add_step_panel.test.js`、`cell_editor.test.js`、`params_form.test.js`、`step_card.test.js`、`model.test.js`、`validation.test.js`、`roundtrip.test.js`。 |
| 模板 | `web/src/template-upload.test.js`、`web/src/template-crop-modal.test.js`、`web/src/template-studio.test.js`。 |
| Keymap | `web/src/keymap-panel.test.js`、`web/src/keymap-control.test.js`、`web/src/gamer-keymap-extension.test.js`、`web/src/keymap-runtime.test.js`。 |
| Video | `web/src/video-api.test.js`、`web/src/video-project.test.js`、`web/src/video-workbench.test.js`、`web/src/components/video/yamlCapability.test.js`。 |
| 插件中心 | `web/src/plugin-center.test.js`。 |
| 服务端插件生命周期 | `server/src/extensions/service.rs` 内联测试；另有 `server/src/architecture_guard_tests.rs` 与 `server/src/api/tests/` 下 API 集成测试。 |
| 服务端执行 | `server/Cargo.toml`：`cargo test`；默认 feature 含 `wasm-runtime`，可按测试名/模块定向。 |

现有测试入口覆盖了大量基础契约，但针对上述 20 个问题，未确认存在覆盖完整失败路径的回归测试；本次也未执行任何测试命令。

## 未验证项

- 未运行 `pnpm test:run`、`cargo test`、WASM guest 构建/Component 校验或插件打包校验。
- 未启动前后端，未做浏览器交互、网络竞态、真实文件覆盖失败、服务端生命周期操作或 WebRTC/设备测试。
- 未在真实 Package、真实媒体引用、真实 keymap profile、真实 YAML 函数参数和三个官方插件归档上验证上述判断。
- 未验证 `server/data/` 内既有运行时数据库/媒体/包数据与源码契约的业务一致性；该目录仅按要求保留。
- 文档/契约同步风险仍需单独回归：`docs/reference/KEYMAP_SCHEMA.md` 与当前实现路径/模型是否完全一致，需后续核对。

## 后续任务建议

1. 先为 A01-A07、T01、K01-K04、V01-V05、M01-M03 各补最小失败测试，优先锁定 P0：A01/A02、T01、K02、V01/V02/V04/V05。
2. 按领域实施：YAML schema/参数编辑器；模板原子覆盖；Keymap 请求序列/保存状态/overlay；Video 引用同步/帧身份/草稿来源；插件生命周期与市场提示。
3. P0 失败测试通过后，再执行前端全量、服务端相关模块、WASM/插件构建及必要的浏览器/设备验证。
4. 最后复核 Package/Plugin/App 四层上下文、官方插件 manifest/分发产物和文档契约，形成 Phase 0→后续实现任务的验收闭环。
