# Gamer 全插件 Phase 0 复现矩阵

> 任务包：P0-B
> 复核基线：b242c0d586c19bb0fdb996e05879e3f0459ef9ff
> 复核日期：2026-09-09
> 本文件只记录复现与验证状态，不代表问题已经修复。

## 1. 执行口径

覆盖计划登记的 A01-A07、T01、K01-K04、V01-V05、M01-M03，共 20 项。每项均给出入口、前置数据/上下文、操作步骤、应观察行为、当前结果、证据命令与 NOT_VERIFIED 条件。

状态含义：

- CONFIRMED：当前源码存在确定的最小失败路径。
- PASS-BUT-NOT-COVERING：已有测试通过，但没有覆盖该问题的失败路径。
- NOT_VERIFIED：需要真实浏览器、设备、媒体或尚不存在的专用故障注入测试，不能据此宣称通过。

本次没有新增 fixture。现有 web Vitest、server Rust 单测、script-editor fixture 和 video/keymap/plugin-center 测试入口足够承载后续专属复现；失败注入使用独立的最小输入，不写入业务数据。

## 2. 已执行的公共证据

以下命令均在当前工作树执行，未修改业务文件：

~~~powershell
cd E:\code\gamer\web
pnpm test:run -- src/script-editor/__tests__/step_card.test.js src/script-editor/__tests__/cell_editor.test.js src/script-editor/__tests__/add_step_panel.test.js src/script-editor/__tests__/params_form.test.js
# 4 files / 39 tests passed

pnpm test:run -- src/template-upload.test.js src/template-studio.test.js src/template-crop-modal.test.js
# 3 files / 21 tests passed

pnpm test:run -- src/keymap-panel.test.js src/gamer-keymap-extension.test.js src/keymap-control.test.js src/keymap-runtime.test.js
# 4 files / 42 tests passed

pnpm test:run -- src/video-api.test.js src/video-workbench.test.js src/video-project.test.js src/components/video/yamlCapability.test.js
# 3 files / 78 tests passed

pnpm test:run -- src/plugin-center.test.js src/workspace-plugin-panel.test.js
# 2 files / 24 tests passed

cd E:\code\gamer\server
cargo test extensions::service
# 31 tests passed
~~~

并行执行 Vitest 时出现过 WebSocket server error: Port is already in use；video-workbench.test.js 还输出过对 127.0.0.1:3000 的连接拒绝日志，但命令退出码均为 0。这些结果不能解释为真实浏览器/设备验证。第一次执行 cargo test extensions::service --lib 因 crate 无 library target 失败，随后使用上面的正确命令完成 31 项服务测试。

## 3. gamer.yaml / script-editor

### A01 — 必填且无默认值的参数不自动显示

- 入口：web/src/script-editor/components/StepCard.vue 的函数选择、applyFn、argsFromDecls。
- 前置数据/上下文：挂载 call 步骤；函数目录含 demo；resolveParams('demo') 返回 name=required_value、type=string、required=true、default=null。
- 操作步骤：选择 demo；等待 schema 返回；展开步骤参数区。
- 应观察行为：必填参数应出现空的类型化编辑行，并由校验提示填写。
- 当前结果：argsFromDecls 只写入有 default 的声明；该输入得到 kind=none，参数行不会出现。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 4 "function argsFromDecls|d\.default !== null|kind: 'none'" web/src/script-editor/components/StepCard.vue
  cd web
  pnpm test:run -- src/script-editor/__tests__/step_card.test.js -t "Schema 驱动的命名参数编辑"
  ~~~

  后一命令只验证手动点击“+ 参数”的正向路径，不能证明 A01 已通过。
- NOT_VERIFIED：未在真实脚本页面用服务端正式函数 schema 进行浏览器复现；未新增故障测试。

### A02 — schema 类型与值控件类型不一致

- 入口：StepCard.vue 的 argType 到 CellEditor.vue 的 type 分支。
- 前置数据/上下文：函数参数声明使用 boolean、integer、template、duration、list、object。
- 操作步骤：选择函数并进入命名参数编辑；观察各参数控件；分别输入布尔、整数、模板名、时长、数组和对象。
- 应观察行为：应分别使用布尔、数值、模板、时长及结构化控件，并保持实际数据类型。
- 当前结果：argType 直接返回 schema 原值；CellEditor 只识别 bool、number、tmpl、time，其余落入文本分支。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n "function argType|type === 'number'|type === 'bool'|type === 'tmpl'|type === 'time'|default: return 'text'" web/src/script-editor/components/StepCard.vue web/src/script-editor/components/CellEditor.vue
  cd web
  pnpm test:run -- src/script-editor/__tests__/cell_editor.test.js src/script-editor/__tests__/step_card.test.js
  ~~~

- NOT_VERIFIED：未用 17 个正式原生函数的完整 schema 做真实页面逐类型录入；现有测试传入的是已转换后的控件类型。

### A03 — 添加步骤与切换函数没有共用 schema 初始化

- 入口：AddStepPanel.vue 的 insertCall；对照 StepCard.vue 的 applyFn。
- 前置数据/上下文：函数 demo 有必填无默认参数和一个有默认参数。
- 操作步骤：从“添加步骤”面板点击 demo；不再切换新卡片的函数；展开新卡片。
- 应观察行为：新卡片应生成与函数切换路径相同的参数结构、默认值和必填占位。
- 当前结果：insertCall 直接执行 createCall(fn)，createCall 默认 kind=none，不调用 resolveParams。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 5 "function insertCall|createCall\(fn\)|function applyFn|argsFromDecls" web/src/script-editor/components/AddStepPanel.vue web/src/script-editor/components/StepCard.vue web/src/script-editor/factories.ts
  cd web
  pnpm test:run -- src/script-editor/__tests__/add_step_panel.test.js
  ~~~

- NOT_VERIFIED：未在实际宿主注入的异步 resolver 下打开浏览器添加面板；已有测试只断言函数名。

### A04 — 单值/命名参数形态切换丢失已有参数

- 入口：StepCard.vue 的“单值”和“命名参数”按钮。
- 前置数据/上下文：参数为 map entries={template:{lit:'login.png'}}，或为 value cell={lit:'login.png'}。
- 操作步骤：展开步骤；点击“单值”；再点击“命名参数”。
- 应观察行为：应保留可转换的当前值，并明确提示不可转换部分。
- 当前结果：toValueArg 固定写入空字符串，toMapArg 固定写入空 entries；原参数被丢弃。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 3 "function toValueArg|function toMapArg|cell: \{ lit: '' \}|entries: \{\}" web/src/script-editor/components/StepCard.vue
  cd web
  pnpm test:run -- src/script-editor/__tests__/step_card.test.js
  ~~~

- NOT_VERIFIED：未在真实编辑页面完成两次切换并检查撤销栈；现有测试未覆盖这两个按钮。

### A05 — 快速切换函数时非最后请求可能生效

- 入口：StepCard.vue 函数下拉的 applyFn。
- 前置数据/上下文：当前函数 old；resolver 为两个可控 deferred promise，A=fn_a、B=fn_b；两次选择连续发生，A 比 B 先返回。
- 操作步骤：选择 A；A 返回前选择 B；先 resolve A，再 resolve B；观察最终 step.fn。
- 应观察行为：最终选择 B 必须生效，A 的旧 schema/参数不能覆盖或阻止 B。
- 当前结果：代码只比较各请求开始时的 prev 与返回时 step.fn。A 先返回时会写入 A；B 随后看到 step.fn 已变化而 return，最终保留 A。现有 guard 只覆盖另一种返回顺序。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 8 "const prev = String\(props\.step\.fn|String\(props\.step\.fn \?\? ''\) !== prev|resolveParams\(next\)" web/src/script-editor/components/StepCard.vue
  cd web
  pnpm test:run -- src/script-editor/__tests__/step_card.test.js
  ~~~

- NOT_VERIFIED：未通过浏览器真实事件节奏或新增 deferred fixture 运行；结论来自确定的异步控制流。

### A06 — list/object 默认值按文本编辑

- 入口：ParamEditor.vue 的 cellTypeOf 到默认值 CellEditor。
- 前置数据/上下文：参数为 list 默认 ['a']，或 object 默认 {enabled:true}。
- 操作步骤：展开参数编辑器；编辑默认值；保存模型并重新序列化/解析。
- 应观察行为：数组和对象应使用结构化编辑，模型中保持数组/对象。
- 当前结果：cellTypeOf 对 list/object 走 default=text；CellEditor 文本输入回传字符串，setDefault 直接写入 cell.lit。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 5 "function cellTypeOf|case 'list'|case 'object'|default: return 'text'|function setDefault" web/src/script-editor/components/ParamEditor.vue web/src/script-editor/components/CellEditor.vue
  cd web
  pnpm test:run -- src/script-editor/__tests__/params_form.test.js
  ~~~

- NOT_VERIFIED：未在真实 UI 对 list/object 进行编辑往返；现有参数测试主要覆盖标量和默认值开关。

### A07 — 当前步骤匹配测试没有携带实际步骤参数

- 入口：CellEditor.vue 模板字段“匹配” → seCellTools.matchTemplate(name) → useConsoleTemplates.js 的 testMatch。
- 前置数据/上下文：步骤带有非默认 threshold/region 或其他函数参数；设备已连接；模板存在。
- 操作步骤：修改步骤的实际参数；点击模板字段旁的“匹配”。
- 应观察行为：请求应使用当前步骤参数，只执行匹配，不点击设备。
- 当前结果：matchTemplate 只接收模板名；宿主调用 testMatch(name, stepSemantics=true)，请求只传模板名、设备、阈值、Package，region 显式为 undefined，没有当前步骤参数快照。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 4 "matchTemplate\(name\)|stepSemantics|const region = stepSemantics|api\.testTemplate\(" web/src/script-editor/components/CellEditor.vue web/src/components/console/useConsoleTemplates.js web/src/api.js
  cd web
  pnpm test:run -- src/script-editor/__tests__/cell_editor.test.js
  ~~~

- NOT_VERIFIED：未连接 Android 设备执行真实步骤匹配；未验证服务端对缺失参数的具体响应。

## 4. gamer.yaml 模板资源

### T01 — 模板覆盖先删旧文件再创建新文件

- 入口：useConsoleTemplates.js 的 overwriteTemplate。
- 前置数据/上下文：当前 Package 已有 login.png；裁切 payload 已准备；覆盖确认通过；让 deleteTemplate 成功、createTemplate 失败。
- 操作步骤：打开同名模板覆盖；刷新后仍找到旧模板；注入删除成功、创建抛错；观察旧模板和列表。
- 应观察行为：新图完全校验/处理成功前旧模板应继续可用；失败后应能独立重试。
- 当前结果：实现先 deleteTemplate，设置 deleted=true，再 createTemplate；创建失败时旧文件已经删除。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 8 "async function overwriteTemplate|deleteTemplate\(existing\.name|deleted = true|createTemplate\(payload" web/src/components/console/useConsoleTemplates.js
  cd web
  pnpm test:run -- src/template-upload.test.js src/template-studio.test.js src/template-crop-modal.test.js
  ~~~

- NOT_VERIFIED：未对运行中的服务端注入上传失败/非法 PNG；未执行真实模板覆盖。

## 5. gamer.keymap

### K01 — hold 投屏标记读取 from/to 而非 at

- 入口：useConsoleKeymap.js 的 keymapOverlay。
- 前置数据/上下文：模型含 action={type:'hold',at:[0.7,0.8]}；投屏视频尺寸可读。
- 操作步骤：加载方案；观察投屏映射覆盖层；按下/释放 hold 绑定。
- 应观察行为：hold 应显示位于 action.at 的点，并与 down/up 生命周期一致。
- 当前结果：overlay 将 hold 与 swipe 一起读取 action.from/action.to；只有 at 的 hold 返回 null，不显示正确点位。面板编辑器本身使用 at。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 8 "type === 'swipe' \|\| type === 'hold'|normalizedPoint\(action\.from\)|normalizedPoint\(action\.to\)|normalizedPoint\(action\.at\)" web/src/components/console/useConsoleKeymap.js
  cd web
  pnpm test:run -- src/keymap-panel.test.js src/keymap-control.test.js
  ~~~

- NOT_VERIFIED：未连接投屏验证 overlay 视觉位置；未在真机验证 hold down/up 是否有残留触点。

### K02 — 方案详情请求缺少上下文/请求代次保护

- 入口：useConsoleKeymap.js 的 onKeymapChange。
- 前置数据/上下文：Package A/方案 A；随后切换 Package B/方案 B；A 的 getKeymap 延迟。
- 操作步骤：选择 A 发起详情请求；返回前切换 B；让 A 最后返回；观察 activeKeymapModel、名称、错误和 loading。
- 应观察行为：响应必须绑定发起时 Package、方案 ID 和请求序号；旧响应不能写入 B，旧 finally 不能清掉新 loading。
- 当前结果：loadKeymaps 有 serial，但 onKeymapChange 没有详情 serial；详情响应直接写入共享 active model，finally 无代次判断。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 7 "let keymapLoadSerial|async function onKeymapChange|api\.getKeymap|finally|keymapLoading\.value = false" web/src/components/console/useConsoleKeymap.js
  cd web
  pnpm test:run -- src/keymap-panel.test.js src/keymap-runtime.test.js
  ~~~

- NOT_VERIFIED：未用两个真实 Package 和延迟网络在浏览器执行乱序切换；未验证输入控制器的真实触摸结果。

### K03 — 刷新清空当前方案，保存和应用耦合

- 入口：useConsoleKeymap.js 的 loadKeymaps、onKeymapSave。
- 前置数据/上下文：Package A 有方案 A，A 已选中或正在使用。
- 操作步骤：点击刷新；或保存方案；观察选中方案、应用模型和输入控制器。
- 应观察行为：刷新列表应保留仍存在的选择；保存应先完成持久化，应用应由明确动作完成。
- 当前结果：loadKeymaps 开始即 resetKeymapSelection 并清空列表；onKeymapSave 保存后重新拉列表，再直接调用 onKeymapChange，保存与应用耦合。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 5 "async function loadKeymaps|resetKeymapSelection\(\)|keymaps\.value = \[\]|async function onKeymapSave|await onKeymapChange" web/src/components/console/useConsoleKeymap.js
  cd web
  pnpm test:run -- src/keymap-panel.test.js
  ~~~

- NOT_VERIFIED：未在真实 Console 工具条完成刷新/保存/应用分离流程；现有测试未覆盖 composable 状态序列。

### K04 — 保存状态未传入面板，缺少 composable 重入保护

- 入口：useConsoleKeymap.js 的 keymapPanelContext → KeymapPanel.vue 的 saving。
- 前置数据/上下文：编辑器打开；onSave 模拟未完成 Promise。
- 操作步骤：连续点击两次“保存方案”；观察按钮 disabled、请求次数和草稿关闭时机。
- 应观察行为：面板应收到独立 saving；保存函数自身应拒绝重入。
- 当前结果：KeymapPanel 读取 context.saving，但 keymapPanelContext 未提供 saving；onKeymapSave 只设置共享 keymapLoading，没有入口级重入 guard。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 5 "const saving = computed|keymapPanelContext|saving:|async function onKeymapSave|keymapLoading\.value = true" web/src/components/console/KeymapPanel.vue web/src/components/console/useConsoleKeymap.js
  cd web
  pnpm test:run -- src/keymap-panel.test.js
  ~~~

- NOT_VERIFIED：未用延迟保存请求在真实面板双击验证；现有测试的 onSave 为同步/立即完成回调。

## 6. gamer.video

### V01 — 媒体引用字段 camelCase/snake_case 不一致

- 入口：VideoWorkbench.vue 的 syncProjectMediaRefs → videoApi.setMediaRefs。
- 前置数据/上下文：媒体 API 返回 snake_case 引用；当前 Package 为 pkg，插件为 gamer.video。
- 操作步骤：保存或创建项目触发引用同步；读取受影响媒体；观察 refs POST body。
- 应观察行为：API 边界只做一次明确转换，并发送 package_id/plugin_id/kind。
- 当前结果：Workbench 新条目使用 snake_case；videoApi.setMediaRefs 读取 entry.packageId/entry.pluginId，输入该条目会在 requireId 处抛出 package_id 不能为空，POST 不按预期发出。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 8 "const next =|package_id: packageId|plugin_id: GAMER_VIDEO_PLUGIN_ID|setMediaRefs|entry\.packageId|entry\.pluginId" web/src/components/video/VideoWorkbench.vue web/src/components/video/videoApi.js
  cd web
  pnpm test:run -- src/video-api.test.js src/video-workbench.test.js
  ~~~

  Workbench 现有测试 mock 了 setMediaRefs，未进入真实封装的字段读取。
- NOT_VERIFIED：未运行后端媒体 refs POST；未验证多窗口/多 Package 的并发保护。

### V02 — 引用同步使用刷新前的项目列表

- 入口：VideoWorkbench.vue 的 saveProject/createProject → syncProjectMediaRefs。
- 前置数据/上下文：项目摘要显示媒体 A；编辑后主素材为 B；A/B 均存在。
- 操作步骤：保存项目；在资源 PUT 成功、loadProjects 之前触发引用同步；观察 A/B refs。
- 应观察行为：引用并集应基于本次提交后的项目集合，B 新增当前 Package 引用，A 仅在仍被使用时保留。
- 当前结果：saveProject 在 syncProjectMediaRefs 之后才 loadProjects；同步函数从旧 projectSummaries 计算 referencedByPackage。B 不在旧列表而不会登记，A 仍可能被错误保留；createProject 有同样顺序。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 9 "await syncProjectMediaRefs|await loadProjects|referencedByPackage|projectSummaries\.value\.flatMap" web/src/components/video/VideoWorkbench.vue
  cd web
  pnpm test:run -- src/video-workbench.test.js -t "媒体引用同步"
  ~~~

- NOT_VERIFIED：未通过真实 API 创建/替换项目并读取媒体 metadata；现有用例只覆盖 mock 列表快照。

### V03 — 引用同步失败后保存按钮不可重试

- 入口：VideoWorkbench.vue 的 syncProjectMediaRefs 错误分支和项目保存按钮。
- 前置数据/上下文：项目 PUT 成功；媒体 refs GET/POST 返回非 404 错误；项目已被标记为干净。
- 操作步骤：保存项目；注入引用同步失败；观察错误提示和保存按钮；不修改项目再次尝试恢复。
- 应观察行为：显示“项目已保存，引用同步失败”，并提供独立可点击重试，无需再次制造脏改动。
- 当前结果：错误分支写入 staleSaveError，文案建议重新保存；但保存成功前已 projectDirty=false，保存按钮 disabled，也没有独立重试入口。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 6 "projectDirty|project-conflict-banner|媒体引用同步失败|重新保存以重试|syncProjectMediaRefs" web/src/components/video/VideoWorkbench.vue
  cd web
  pnpm test:run -- src/video-workbench.test.js -t "引用同步"
  ~~~

- NOT_VERIFIED：未注入非 404 refs 网络失败并在浏览器操作恢复；现有用例未覆盖独立重试按钮。

### V04 — seek 后旧确定帧仍可被制作操作使用

- 入口：VideoTimeline.vue 的视频时间更新、currentFrame、emitCreateTemplate/addMarkerAtCurrentFrame。
- 前置数据/上下文：媒体 A 有帧表；先锁定帧 A；预览可 seek 到 B。
- 操作步骤：锁定 A；拖动或播放到 B；不重新锁帧；点击创建模板或添加标记。
- 应观察行为：主动播放/拖动应使 A 失效；制作应解析并使用 B 的确定帧身份。
- 当前结果：媒体 ID 变化时 watcher 会清理 currentFrame，但 onTimeUpdate/onSeeked 只更新 currentTime，不清理旧 currentFrame；制作入口优先使用 A。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 5 "currentFrame|function onTimeUpdate|function onSeeked|function emitCreateTemplate|function addMarkerAtCurrentFrame" web/src/components/video/VideoTimeline.vue
  cd web
  pnpm test:run -- src/video-workbench.test.js -t "精确帧与逐帧"
  ~~~

- NOT_VERIFIED：未播放真实 VFR 媒体完成 A→B 制作；未验证浏览器事件如何区分程序 seek 与用户 seek。

### V05 — 草稿来源切换可能串用旧会话事件/选择

- 入口：VideoDraft.vue 的 loadEvents 与 recordingId watcher。
- 前置数据/上下文：会话 A 请求延迟；草稿区已有 A 的事件选择；随后切换 B；A/B 事件不同。
- 操作步骤：载入 A；请求未返回或生成态未清理时切换 B；让 A 返回；观察事件、选择、注释、草稿来源和生成请求。
- 应观察行为：切换会话应立即使旧请求失效并清理事件/选择/诊断；A 返回不能覆盖 B。
- 当前结果：组件只在 loadEvents 成功后 resetSelection；recordingId watcher 只更新输入框，并在 !loadedOnce 时自动加载，没有请求代次；loadedOnce 不因会话改变而重置。旧响应可写入新上下文，已加载 A 后切 B 也不会自动读取 B。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 7 "watch\(\(\) => props\.recordingId|loadedOnce|async function loadEvents|recordingEvents|resetSelection|selectedIds" web/src/components/video/VideoDraft.vue
  cd web
  pnpm test:run -- src/video-workbench.test.js -t "草稿"
  ~~~

- NOT_VERIFIED：未在浏览器用两个真实录制会话和延迟请求执行切换；未向服务端提交串台 event_id 做最终验证。

## 7. 插件中心 / 扩展生命周期

### M01 — 更新/卸载直接调用底层接口，Running 时不能自动停止和恢复

- 入口：PluginCenter.vue 的 installArchive/uninstall；后端 service.rs 的 update_with_context/uninstall。
- 前置数据/上下文：插件已安装且 Running；更新归档可 inspect，或准备卸载确认；可有 runner/UI 贡献。
- 操作步骤：在插件中心直接点击更新或卸载；观察 API 调用序列和服务端状态。
- 应观察行为：先完成下载/完整性/manifest 检查，再安全停止受影响实例；更新后按原状态恢复，卸载默认保留数据并清理 runner/UI。
- 当前结果：前端更新直接调用 updateExtension，卸载直接调用 uninstallExtension，没有先调用 disable/stop；后端 update 对 Running 返回 invalid transition，active Running uninstall 也返回 invalid transition。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 8 "updateExtension\(|uninstallExtension\(|record\.state\.is_running|invalid_transition.*update|invalid_transition.*uninstall" web/src/workspace/plugin-center/PluginCenter.vue server/src/extensions/service.rs
  cd server
  cargo test extensions::service
  ~~~

  Rust 31 项生命周期测试通过，证明现有底层守卫/恢复测试未被破坏；不等于 Running 更新/卸载的用户编排已通过。
- NOT_VERIFIED：未运行实际插件 WASM、runner、录制或活动 Run 后从 UI 执行更新/卸载；未验证旧版本恢复和用户数据保留的端到端结果。

### M02 — 市场页对已安装版本显示“更新”而不区分版本关系

- 入口：PluginCenter.vue 市场卡片按钮和 marketUpdate。
- 前置数据/上下文：registry 有 plugin@1.0.0；已安装同版本、较新版本或较旧版本的同 ID 插件。
- 操作步骤：打开市场；分别观察三种版本关系的按钮文案和是否可点击。
- 应观察行为：同版本应显示已安装/无需更新；已装更高版本应显示无可用更新；仅市场版本更高时显示更新到 x。
- 当前结果：市场卡片只判断 installedVersion(entry.id)，只要已安装就显示“更新”；canInstallMarket 只检查 sha256，同版本/降级仍可能进入下载更新路径。marketUpdate 的版本比较只影响已安装卡片，不能修正市场卡片。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 5 "installedVersion\(entry\.id\) \? '更新'|function marketUpdate|compareVersions|function canInstallMarket" web/src/workspace/plugin-center/PluginCenter.vue
  cd web
  pnpm test:run -- src/plugin-center.test.js
  ~~~

- NOT_VERIFIED：未在真实 registry 和已安装快照组合下操作市场 UI；未请求服务端验证同版本/降级响应。

### M03 — 成功提示被随后 refresh 清除

- 入口：PluginCenter.vue 的 runAction/uninstall/refresh。
- 前置数据/上下文：插件中心已打开；enable/disable/uninstall API 成功；刷新 API 正常返回。
- 操作步骤：执行启用、停用或卸载；观察完成后的提示；对照安装/更新路径。
- 应观察行为：成功提示应在刷新完成后仍保留，直到下一次操作或关闭面板。
- 当前结果：runAction/uninstall 先设置 notice，随后 await refresh；refresh 开始调用 clearMessages，会清掉 notice。因此这些成功提示消失。installArchive 在 refresh 后再设 notice，只有部分路径规避。状态：CONFIRMED。
- 证据命令：

  ~~~powershell
  cd E:\code\gamer
  rg -n -C 5 "function clearMessages|async function refresh|notice\.value =|await refresh\(\)" web/src/workspace/plugin-center/PluginCenter.vue
  cd web
  pnpm test:run -- src/plugin-center.test.js
  ~~~

- NOT_VERIFIED：未挂载真实插件中心执行成功后的可视化提示检查；未验证刷新失败时提示保留策略。

## 8. 覆盖结论

- 已建立 A01-A07、T01、K01-K04、V01-V05、M01-M03 共 20 项的最小入口、输入、操作、期望、当前结果、证据命令和 NOT_VERIFIED 条件。
- 当前源码级失败路径确认：A01-A07、T01、K01-K04、V01-V05、M01-M03；A05 的现有 guard 只覆盖部分请求返回顺序，不能视为已修复。
- 已执行并通过的现有回归入口覆盖 script-editor、模板、keymap、video、plugin-center 和扩展生命周期；通过只证明现有测试未回归，不替代本矩阵的专属失败测试。
- 没有新增 fixture，也没有修改业务源码、已有测试、其他文档或公共热点文件。

## 9. NOT_VERIFIED 总表

1. 所有项目均未完成真实浏览器端到端验证。
2. 设备相关项（A07、K01、K02、V04，以及 M01 的运行中插件链路）未连接 Android/真实 WebRTC 投屏验证。
3. 媒体/录制相关项（V01-V05）未使用真实媒体文件、VFR 帧表、录制分段和服务端 metadata/refs 做故障注入。
4. 生命周期项（M01-M03）未在实际安装的 WASM/builtin 插件、活动 runner、录制会话和更新归档上执行用户流程。
5. A05、K02、V02、V05 的异步乱序/并发结论尚未由新增 deferred/mock 专用测试锁定；本文件只给出最小操作顺序和源码路径。
