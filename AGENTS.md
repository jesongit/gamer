# AGENTS.md

## 项目

GameBot 游戏自动化助手：Rust 服务端（axum + webrtc-rs）+ Vue3/Vite 前端。
scrcpy 采集 Android 设备画面 → WebRTC（H.264 视频轨 + DataChannel 控制）推流到浏览器，
支持触控控制、模板匹配（NCC）、YAML 自动化、定时任务。

核心概念分离（plan：docs/plans/gamer_v3_package_frontend_architecture_plan.md）：
**Android App = 执行目标，Package = 配置数据上下文，Plugin = 能力（Runner/Panel/校验），Task = 调度**。
`Android App != Package`、`Package != Plugin`、`Task != Script`。

## 目录与端口

- `server/` — Rust 服务端，监听 **8443**，静态托管 `web-dist/`（构建产物）
- `web/` — Vue3 + Vite，dev 监听 **5173**，`/api`、`/ws` 代理到 8443；路由为 hash 模式
- `server/config.toml` — 关键项：`adb_path`、`ffmpeg_path`、`scrcpy_server`、`[auth].password_hash`；开发登录密码只从 `GAMER_ADMIN_PASSWORD` 读取并在进程内生成 Argon2id PHC，无默认账号/密码
- `server/data/gamer.db` — SQLite（设备/任务/预设/运行/日志）；业务数据一级作用域 = **Package ID**，文件存储 `server/data/packages/<package-id>/`：`package.toml`（manifest）+ `shared/`（跨插件保留区）+ `plugins/<plugin-id>/`（插件数据，**目录语义归插件**——gamer.yaml：`automations/`（自动化脚本）/`functions/`（函数库）/`templates/`（模板）[/`presets/`（任务预设）]，gamer.keymap：`mappings/`；Core 不解释插件目录内部结构）；资源寻址三元组 **(package_id, plugin_id, path)**（实现在 `server/src/resources.rs` 的 PackageStore）。package-id / plugin-id 语法 `[a-z0-9][a-z0-9._-]*`（`validate_scope_id`，**禁大写**——Android 包名原样不能当 Package id，原名可写进 manifest 的 `[targets.android].packages`）
- package.toml：`id`/`name`/`version`/`author` + `[targets.android].packages`（0..n，0 个 = 通用 Package；仅做兼容性提示不阻断）+ `[plugins."<plugin-id>"] required`（插件依赖声明，允许声明当前未安装的插件）
- 认证：配置只接受 Argon2id PHC `[auth].password_hash`；开发登录密码只用 `GAMER_ADMIN_PASSWORD`，无默认账号/密码。WebRTC 不内置 STUN/TURN，默认 host candidate 直连；Docker/NAT 需配置 `rtc_external_ip/rtc_udp_port/rtc_external_port` 并发布 UDP。
- 数据基线：SQLite schema v3（表：timer_tasks/task_presets/scheduled_runs/logs/devices；schedule JSON 为 `{provider_id,config}` 形态；v1→v2 Timer Core 泛化、v2→v3 Task 模型收口（legacy `tasks` 表 DROP）逐级静态迁移，注册表在 `server/src/migrations.rs`）；`user_version=0`/无版本号数据库不自动迁移，高于目标的库拒绝启动。
- 资源发行：**默认发行零业务资源**——`server/data/packages/` 不进 git。Package 经导入分发：`POST /api/packages/import`（zip/.gamerpkg，可选 `X-Expected-Sha256` 校验头；目标已存在 409 附 manifest 摘要，`?overwrite=true` 校验后原子替换）与 `POST /api/packages/:pkg/export`（可复现打包）。**dormant 插件数据保留**（plan §5-§6）：包内未安装插件的数据目录原样保留、不解释不修改，后续安装该插件即恢复对应能力。**Installed/Editable 双层模型、`data/app-packages/`、`/api/workspace`、`/api/apps/:app/resources`、六目录 kind 已全部删除，不做兼容，旧数据手动迁移**（迁移示例见 `server/data/_migration_backup` 与本仓库 2026-09 迁移提交）。

## 架构分层（ADR-11~14，`docs/reference/adr/`）

- **Core（`server/src` 顶层）= 稳定机制**：`device/`（采集/会话/帧缓存）、`webrtc/`、`api/`、`store.rs` + `timer_core.rs` + `scheduler.rs` + `run_manager.rs`（任务/调度/运行）、`resources.rs`（PackageStore，Package 三元组寻址 + `ResourceHandler` 注册表）、`package_archive.rs`（.gamerpkg 归档导入/导出）、`core/models.rs`（Runtime Context 四层数据模型，权威注释在 `AppContext` 上方）、`cron_extension.rs`（Native schedule provider，provider_id=`cron`）、`capabilities/`（device/vision/input/touch/resource/run/runtime/log SDK）、`matcher.rs`（vision NCC）、`extensions/service.rs`（扩展生命周期状态机）。Core 不认识 YAML/Script DSL/Function DSL/Keymap rule/任何具体 Runner 实现。
- **业务归扩展**：`extensions/gamer_yaml/`（**YAML v3 唯一方案**：`yaml_vnext.rs` v3 纯数据前端（version:3 判别 + parse+lower+函数库解析）、`yaml_extension.rs` 原生参考解释器 + NativeYamlHost + 保存/导入校验入口 + WASM runtime 契约、`wasm_host.rs` LazyYamlWasmtimeRuntime、`runner_adapter.rs` v3-only EngineExecutor、`run_target.rs` RunTarget/RunSpec/TypedValue wire、`resources.rs` ResourceHandler 内容钩子（automations/functions 校验 + 模板灰度归一化/重命名引用改写）、`timer_yaml.rs` 定时 runner、`task_params.rs` 任务参数门禁、`entrypoint_descriptor.rs` 参数 schema 描述器、`params.rs` 标量助手、`error.rs` 结构化诊断；v2 parser/AST/loader/执行器/编辑器已彻底删除，非 `version: 3` 一律 `yaml.v3.version(.missing)` 诊断，无 fallback 无迁移工具）与 `extensions/keymap/`（`mod.rs` 输入信封/能力执行器/WASM 运行时管线，映射本体全在 guest；`dsl.rs` keymap YAML 解析/校验/序列化）。**Extension ≠ WASM**：cron 是 Native Extension；WASM 只是扩展运行时之一（`extensions/wasm.rs`）。
- 最终形态 = Stable Core + Installable Extensions + Packages（数据上下文，Local Package 可读/可写/可运行/可导入导出，`.gamerpkg` 只是传输格式）+ Generic Tasks（Task = 任意 ScheduleProvider + 任意 Runner）+ PackageStore 资源 + Runner 归扩展所有 + 零 legacy 兼容（不保留旧 API/旧格式/旧数据目录/迁移开关，ADR-14）。
- 边界由测试锁死：`server/src/architecture_guard_tests.rs`（七测试：源码边界白名单双向校验（含条目存活断言）/依赖方向/v3-only 禁符/扩展生命周期全链/裸核 REST/YAML 隔离/Keymap 隔离）+ `web/src/core-shell-boundary.test.js`。

## 常用命令

```powershell
.\gamer.ps1 start|stop|restart|status   # 前后端一起管；-BackendOnly / -FrontendOnly / -Build / -Release
cd server && cargo run                  # 单起后端（日志 GB_LOG 追加到 server/gamer-server.log）
cd web && pnpm dev                      # 单起前端
```

## 关键链路（改代码前先看）

- **Runtime Context 四层（plan §16）**：Device(`device_id`，运行目标设备) / App(`android_package`，**纯运行目标** = 设备配置的 pkg，`app.start`/`app.stop` 缺省目标) / Package(`content_package`，**数据上下文** = 资源解析域 `packages/<package-id>/plugins/<plugin>/` 的 Package id，可缺省) / Plugin(调用方扩展 id，由扩展宿主实例承载、不进 `AppContext`)。`android_package` 与 `content_package` 是两个独立命名空间（Android 安装域 vs 内容寻址域），**严格分离、不互相推导、不兜底**（值可以相同但那只是巧合）；权威注释在 `server/src/core/models.rs` 的 `AppContext` 上方
- 连接：浏览器 → `POST /api/devices/:id/connect`（scrcpy 会话，已在线时幂等 no-op）→ WS `/ws/device/:id` 信令（offer/answer；offer 可带 `force:true` 顶替已有 viewer，见"多页面互斥"）→ WebRTC 视频轨 + DataChannel `control`（触控/按键/文本/启停应用：`start_app`（"+" 前缀冷启动）/`stop_app`，REST `/api/devices/:id/control` 同词表；会话侧 `am force-stop` 仅收**无前缀安全包名**）；脚本运行时引擎（YAML v3）的运行结构事件也经该 DataChannel **反向**推送给投屏页面（`{"type":"se","ev":...}`；词表：`run_start`/`run_end{ok,error}`/`step_start{path,desc}`/`step_end{path,ok}`/`call_start{target,depth}`/`vision{template,found,score?,center?}`（match_first 每候选一条）/`budget{kind}`（STEP_BUDGET_EXCEEDED/CALL_DEPTH_EXCEEDED/CANCELLED，先于 run_end），外加投屏标记 `tap`/`swipe`/`hit`（命中框）/`miss`（搜索区域框，匹配未命中时）——宿主侧 input/vision 完成后补发，find 主模板/block 与 match_first 候选全覆盖；guest 经 `__event` 私有通道上报（先于权限校验拦截、解析失败静默丢弃），step path 语法 `steps[0].then[1]`；engine → viewers 注册表 `control_dc`，定时任务运行同样生效）
- 帧缓存 `FrameCache`（帧环 + 按需解码）：截图/模板匹配时用临时 ffmpeg 解**最新一帧**（天然实时，无陈旧/停滞问题），并为**新 viewer 重放初始帧（SPS/PPS + 最近 GOP）**。ffmpeg 不可用 → 无初始帧 → 浏览器黑屏（见 docs/PITFALLS.md）
- 单设备单 viewer：新连接踢旧连接（`AppState.viewers` 注册表）
- 视频静默看门狗（`spawn_watchdog`，2026-08-22 重构）：判死以 `session.connected`（video socket 读取循环退出即 false）为准，**视频静默 ≠ 死链路**。会话确死 → force 拆会话，有脚本/viewer 则立即重连（**唯一允许脚本运行中强拆的路径**——控制 socket 同链路已死，不重连脚本永远卡死）；脚本运行中 + 会话活着 + 静默 → 不处置（静态屏/黑屏正常态）；无 viewer 无脚本 → 交给 idle_power_loop；viewer 在看且未被补帧投喂（last_serve ≥10s）→ reset_video 探测，15s 仍静默才拆开重连踢 viewer
- 多页面互斥（服务端仲裁，2026-08-20 重做，取代旧 localStorage 锁 `gb_webrtc_lock`——锁只能管同一浏览器，跨浏览器/跨 PC 管不到）：新页面 offer（不带 force）遇已有活跃 viewer → 服务端回 `{"type":"conflict"}`（不踢不建连）；前端**手动连接**弹窗确认后带 `force:true` 重发 offer 才接管，**自动重连**遇 conflict 直接放弃并提示。接管只换浏览器↔服务端链路：先经旧页面的信令 ws 推 `{"type":"taken_over"}`（`ViewerHandle.notify` 通道 + ws 循环 peer 关闭后 200ms 冲刷窗口保证送达）再关旧 peer，**设备 scrcpy 会话不动**（实测 ~0.3s 无缝切换）；被顶页面收到 taken_over 后断开且不再自动重连（防互顶死循环）
- 配置变更（PUT /api/devices/:id）：仅**投屏相关参数**（kind/addr/screen_mode/vd_res/vd_dpi/fps，`session_affecting_change` 按生效值归一比较）变更才踢 viewer + 拆会话（前端 onclose → 自动重连恢复画面）；仅改名称/应用包名不拆会话、投屏不中断；投屏参数变更遇脚本运行中仍不踢不拆（运行守卫），新配置下次连接才生效。前端设备管理收在投屏工具条（设备下拉/连接/刷新/新增/设置/删除 + 启动/停止应用），配置编辑走「⚙️ 设置」弹窗（DeviceSettingsModal，显式保存/取消，不再自动保存防抖）
- 设备扫描：`POST /api/devices/scan` 执行 `adb devices -l`，按 addr 去重自动入库（逻辑在 `DeviceManager::scan_and_sync`，服务器启动时也自动跑一次）
- App 生命周期 / 空闲低功耗（2026-08-22 重做）：连接**不再自动启动应用**（由脚本 `app.start`（冷启动，"+" 前缀控制消息）、工具条启动按钮或任务显式触发）；**会话存活由 `DeviceManager::idle_power_loop`（10s 周期）唯一管理**——无 viewer 且无脚本运行持续 `idle_power_secs`（config.toml，默认 300，0=关）秒 → 虚拟屏模式拆 scrcpy 会话（编码停止/虚拟屏销毁，恢复 freezer 禁用/音量静音等设备侧改写，**adb 链路保留**：WiFi/emu 设备每 60s 补 `adb connect` 保活，启动时自举扫描+连接、不建会话不启动应用）；镜像模式关物理屏（keyevent 223，**会话保留**），消费者回来即唤醒。消费者出现（ws viewer 注册 / 脚本运行经 runner_adapter `acquire_activity` → `notify_activity`）打断空闲计时并即时唤醒已关的屏；镜像 30s 补醒也移入该循环（connect 时的保活任务只管拉满/恢复熄屏超时）。**`disconnect_device` 带运行守卫**：脚本运行中拒绝拆会话（虚拟屏销毁会杀掉屏上游戏），仅 force=true 绕过（删除设备 / 看门狗确认死链路 / 手动 `POST /api/devices/:id/disconnect` 管理动作）；前端"断开连接"按钮只断本页 WebRTC **不再调该接口**。下次运行脚本/定时任务自动重连（~2-4s）
- 任务模型（ADR-12）：Task = 任意 ScheduleProvider + 任意 Runner，嵌套 JSON `schedule{provider_id,config}` + `runner{runner_id,entrypoint,payload}`（无 script_id/cron 顶层字段；YAML 参数在 `runner.payload`、cron 表达式在 `schedule.config`）。gamer.yaml 的 entrypoint 语义 = **`<package-id>/<automations 内相对路径>.yaml`**（资源 id 首段 = Package id，可与 Android 包名不同名；`automations/` 前缀由扩展内部映射，函数测试带 `#函数名` 后缀），payload = 类型化 args 快照。REST 唯一入口 `/api/tasks`（CRUD + run/suspend/resume/cancel/enable/disable）与预设 `/api/task-presets`；缺 Runner/provider 的任务落 `TaskState::DependencyMissing`（不删、保留 enabled 原意），Runner 重注册自动恢复
- Runner 生命周期（ADR-13）：`TimerRunnerRegistry` 注册项带 `owner_extension_id`（register/unregister/unregister_owner）；gamer.yaml 的 Runner 由扩展 **start（进入 Running）** 经 `TimerRunnerRegistrar` 钩子注册，stop/disable/uninstall 注销；**enable ≠ start**（Enabled 未 start 的扩展不注册 Runner）；服务重启 `extensions/service.rs::reconcile_startup` 只恢复遗留 Running 记录——升级后存量定时任务需手动 start 一次 gamer.yaml。裸 Core `Scheduler::new(db)` 不预置任何 Runner
- 统一执行：`POST /api/runs`（body `{runner_id, entrypoint, payload?, device_id, content_package?}`，202 + run_id/resolved_args；同设备已有活动运行 409 附当前运行信息）；`content_package` 缺省按 entrypoint 首段约定解析，`android_package` 只来自设备配置 pkg（缺配置直接拒绝，不回退 Package id）；`GET /api/runners`（含 owner）、`GET /api/schedule-providers`、`GET /api/runners/:runner_id/entrypoint`（参数 schema API，前端不解析 YAML 取参）。手动运行的前置存在性校验在 gamer_yaml runner 内经 PackageStore 读取（缺失 → 结构化 not_found），真实执行走 gamer_yaml v3（脚本/函数源码经 resource 能力按三元组解析，非 `version: 3` 统一版本错误无 fallback）。前端 `api.js` 无业务 runner id（gamer.yaml 包装在 `web/src/gamer-yaml-runner.js`，是 runner 注册 id 的唯一前端配置点；插件 id / 插件内目录字面量收敛在 `web/src/gamer-plugin-ids.js`）
- 资源存储（内容无关）：一级作用域 = Package ID，二级 = plugin_id（**插件数据隔离**：资源 API 把插件限制在自己的 `plugins/<plugin-id>/` 前缀内，不能读写其他插件目录、不能写 shared/）。寻址/枚举/乐观并发（资源更新必须带 `expected_version`，`force:true` 显式跳过；包级 `revision` 计数管元数据编辑）由 Core PackageStore 统一承载；内容语义归扩展（`ResourceHandler` 按 plugin-id 注册：未注册 = 裸 Core 语义不做内容校验；gamer.yaml 注册 automations=v3 load 校验、functions=bare-map 校验、templates=灰度归一化 + 重命名引用改写）。REST：`GET|POST /api/packages/:pkg/plugins/:plugin/resources`、`GET|PUT|DELETE /api/packages/:pkg/plugins/:plugin/resources/*path`（文本收 JSON、templates 收 PNG 字节 body；**PUT 经 gamer.yaml 字节钩子强制灰度归一化，非法 PNG 400**）、`POST /api/packages/:pkg/plugins/:plugin/rename`（经 `before_rename` 钩子同步改写引用后原子移动）。脚本资源 id = `<package-id>/<名>.yaml`（前端拼 URL 必须整体 `encodeURIComponent`）；函数路径 = `<文件短路径>/<函数名>`；模板为 8-bit 灰度 PNG、短名在当前包内唯一匹配 `#` 后缀。脚本/函数语法与校验规则见 docs/yaml-v3/（v3 唯一正式文档套件）与 docs/reference/YAML.md §3（v3 正文，v2 仅为历史注记）
- Package 导入导出（`server/src/package_archive.rs`，`.gamerpkg` = zip）：导入 = 字节上限 → ZIP 中央目录校验（条目数/重复路径/穿越名/加密条目拒绝）→ staging 解压（目录安全逐段校验）→ 验证 package.toml → 验证 id（= 目录名）→ 原子安装/替换；**布局白名单 `package.toml`（必须首个条目）+ `shared/**` + `plugins/<plugin-id>/**`**，其余顶层条目一律拒绝。导出 = 内容（package.toml + shared/ + plugins/）按相对路径排序、固定 mtime → 相同输入逐字节相同归档，导出后自检（重走导入校验）。归档**不经过插件内容校验**（以导出侧为准；内容校验只在资源 PUT/保存路径）。包内 `plugins/*/presets/*.yaml` 在导入/创建时经 `TimerCore::publish_package_presets` 灌入任务预设（发布 id = `<package-id>:<名>`，幂等）。兼容性：`GET /api/packages/:pkg/compatibility` 按 `[targets.android].packages` 出提示不阻断（plan §17）；Package 依赖状态（插件五态）随包详情下发
- Console 壳：`web/src/views/Console.vue` 只保留模板装配 + 投屏连接/输入控制接线。**主导航 = 任务|日志|市场|插件|设置**（`workspace/WorkspaceTabs.vue` + `workspace/PluginWorkspace.vue`）——市场页（`MarketView.vue`：插件清单入口 + Package 远端源安装）与插件二级导航（选中插件后展示其贡献的 Panel，插件 Panel 不再占据主导航）；右侧顶部 Package 上下文条（`PackageContextBar.vue`：当前 Package 下拉 + 导入/导出/新建/复制/删除/详情，`PackageDetailModal.vue` 承载 manifest 全字段/插件五态徽章/内容统计/元数据编辑）。Core 自有 任务/日志/设置（`gamer.core:tasks|logs|settings`，`workspace/core-contributions.ts`），业务面板（gamer.yaml 的 自动化/函数/模板、gamer.keymap 的 映射）由扩展 manifest `runtime="core"` + `component` 键贡献（`workspace/core-component-registry.ts` 是宿主组件名的唯一前端知识），安装即出现、禁用/卸载即消失；`DEFAULT_PANEL_KEY='gamer.core:tasks'`。行为契约（hash 路由 panel 同步、连接锁、坐标映射、框选、运行守卫）不变；按域拆分在 `web/src/components/console/use*.js` 组合式函数
- **前端四 Context 命名（plan §39，禁用旧 `activePkg` 双重语义）**：`deviceId`（设备）/ `androidPackageName`（Android 运行目标，归设备域）/ `currentPackageId`（数据上下文，`web/src/package-store.js` 统一管理、Package-aware UI 一律消费）/ `activePluginId`（当前查看的插件，归 workspace 导航）。两个 package 命名空间严格分离不互相推导
- Declarative 插件 UI：manifest.toml 里 `runtime = "declarative"` 的 `[[ui.contributions]]` 可带 `description` + `[[ui.contributions.fields]]`（`type` 限 text/number/boolean/select/button，支持 `name`(alias `key`)/`label`/`placeholder`/`default`/select 的 `options`/button 的 `action` 与字段 `description`，未知控件类型解析报错）；schema 由 `manifest.rs` 校验、`ui.rs` 随 `RegisteredUiContribution` 透传到 `GET /api/extensions` 的 `ui_contributions`，前端 `PluginPanelHost.vue` 原生渲染表单，按钮直接调 `POST /api/extensions/:id/call`（body `{action, values}`）。UI runtime 共三档：declarative | iframe | core（宿主组件）
- 任务表单（TaskBoard 通用化）：`web/src/components/TaskBoard.vue` 是「ScheduleProvider + Runner」通用表单——provider 下拉来自 `GET /api/schedule-providers`（内置 `cron`），runner 下拉来自 `GET /api/runners`；执行目标/参数编辑由 `components/task/runner-editors.ts` 的 `RunnerEditorContribution` 契约按 runner_id 注入（内置 gamer.yaml 编辑器在 `builtin-runner-editors.ts`，编辑上下文携带 `packageId`，自动化/函数/模板候选经通用 Package 资源 API 枚举，见 `gamer-yaml-resources.ts`）
- **WASM 插件生态（Phase 6/7/8/10 已收口为产品能力）**：`wasm-runtime` 进 default feature（lazy init——不装/不启动插件不建 Wasmtime Engine；`--no-default-features` 保留无 WASM 退出路径，CI 有防退化检查；ci.yml/ci-local.ps1 另有官方 guest（wasm32 构建 + Component 校验）验证关卡）。keymap 扩展启动可带 `profile`（start body `{profile: <方案名>}` + `app_context`（含 device_id/android_package/content_package）指定数据上下文，从 `packages/<package-id>/plugins/gamer.keymap/mappings/` 读 YAML 原文经 WIT `start(profile)` 传给 guest，guest 内置 WASD 默认规则、profile 覆盖之，缺省/空 profile = 未映射键全部 pass-through；无 keymap 扩展时输入直通 scrcpy）。declarative `plugin.call` 走通用 extension world 的 `call(action, values-json)` 导出，action 必须在该 manifest declarative schema 按钮集合内否则 400。官方市场：`tools/build-plugins.ps1` 把 `server/tests/keymap-guest`（测试 fixture）与 `server/guests/yaml-guest`（gamer.yaml 产品 guest，P12.8 迁出 tests）打成签名 `.gplugin` 输出到 `web/public/plugins/` 并生成 `web/public/registry.json`（ed25519 签名，dev keypair 在 `tools/plugin-signing/` 仅本地市场用，公钥内嵌 server 信任锚 `signature.rs`；Registry proof 绑定 id/version/download_url/sha256，官方源安装必须验签通过）

## 关键文件

| 文件 | 职责 |
|---|---|
| `server/src/api/mod.rs` | REST 装配：设备 CRUD/scan/connect/control(含 start_app/stop_app)/截图、统一任务 `/api/tasks`（run/suspend/resume/cancel/enable/disable）+ `/api/task-presets`、`POST /api/runs` + run 查询/取消、`GET /api/runners`(+`/:id/entrypoint` schema)、`GET /api/schedule-providers`、Package `/api/packages`（列表/CRUD/duplicate/compatibility/import/export + 插件资源 `.../plugins/:plugin/resources[/*path]` + rename）、`/api/capabilities/vision/test`、`/api/extensions/*`（management/inspect/install/update/enable/disable/activate/start/stop/call/ui）、日志、system/update |
| `server/src/api/packages.rs` / `api/packages_rename.rs` | Package REST：包生命周期（list/create/get/update/delete/duplicate/compatibility）、插件资源三元组 CRUD（文本 JSON / templates PNG 字节，上传组 16MiB）、导入（409 附摘要 + `?overwrite=true` 原子替换）/ 导出、包内 presets 发布；rename 走插件 `before_rename` 钩子后原子移动 |
| `server/src/package_archive.rs` | .gamerpkg 归档：导入校验提取（字节上限→中央目录→staging→manifest→id→原子安装）+ 可复现导出打包（固定 mtime，导出后自检）；布局白名单 `package.toml`/`shared/**`/`plugins/<id>/**` |
| `server/src/resources.rs` | PackageStore：内容无关 Package 三元组寻址（`packages/<package-id>/plugins/<plugin-id>/`）+ package.toml manifest（targets/plugins 依赖/revision 乐观并发）+ 资源级内容版本短码（expected_version/force）+ `ResourceHandler` 注册表（按 plugin-id 回调内容校验/重命名语义；未注册 = 裸 Core）+ 插件数据隔离与 path traversal 防护（`validate_scope_id`） |
| `server/src/core/models.rs` | Runtime Context 四层数据模型（Device/App/Package/Plugin 权威注释 + `AppContext{device_id, android_package, content_package?}`；两命名空间严格分离不互相推导） |
| `server/src/timer_core.rs` | Timer Core：`Task`/`TaskSchedule{provider_id,config}`/`TaskState`（含 `DependencyMissing`）、`TimerRunnerRegistry`（owner_extension_id + register/unregister/unregister_owner）、ScheduleRegistry、包内 presets 发布（`publish_package_presets`，id=`<package-id>:<名>`） |
| `server/src/scheduler.rs` / `run_manager.rs` | 裸 Core 调度器（`Scheduler::new(db)`，不预置 Runner；经 TimerRunnerRegistry 分发）/ RunManager（统一 RunRecord、同设备单活动运行 409、run_id 查询/取消） |
| `server/src/cron_extension.rs` | Native schedule provider（provider_id=`cron`，5/6 域表达式归一 7 域） |
| `server/src/extensions/service.rs` | 扩展生命周期状态机 + `TimerRunnerRegistrar` 钩子（start 注册/stop 注销 runner）+ `reconcile_startup` 重启对账（恢复遗留 Running） |
| `server/src/extensions/gamer_yaml/` | YAML v3 扩展边界（v2 已删）：`yaml_vnext.rs`（v3 surface：parse+lower+函数库解析，严格诊断）、`yaml_extension.rs`（原生参考解释器 + NativeYamlHost + capability invoker + 保存/导入校验入口 + WASM 契约）、`wasm_host.rs`（LazyYamlWasmtimeRuntime：epoch interruption 取消兜底、nonce 注入、`__event` 私有事件通道）、`runner_adapter.rs`（v3-only EngineExecutor + call 目标命名空间解析）、`run_target.rs`（RunTarget/RunSpec/TypedValue，wire 不变）、`params.rs`（标量助手）、`error.rs`（结构化诊断五元组）、`resources.rs`（ResourceHandler 内容钩子：automations=v3 load、functions=bare-map、templates=灰度归一化 + 重命名 v3 AST 引用改写）、`task_params.rs`（v3 参数桥 + psig1 签名门禁）、`entrypoint_descriptor.rs`（参数 schema 描述器）、`timer_yaml.rs`（v3-only runner + Registrar） |
| `server/src/extensions/keymap/` | keymap 扩展边界：`mod.rs`（输入信封/E2E trace/能力执行器/WASM 运行时管线，映射本体全在 guest；无 keymap 运行时输入直通 scrcpy；方案数据在 `packages/<pkg>/plugins/gamer.keymap/mappings/`）+ `dsl.rs`（keymap YAML 解析/校验/序列化） |
| `server/src/capabilities/` | Core SDK：device/vision/input/touch/resource/run/runtime/log 域 + adapters 注册表 |
| `server/src/matcher.rs` | vision NCC 模板匹配（Core 侧，等比缩放加速，命中坐标映射回原图） |
| `server/src/store.rs` + `migrations.rs` | SQLite 持久化（devices/timer_tasks/task_presets/scheduled_runs/logs）+ schema v1→v2→v3 静态迁移注册表（`user_version` 权威，v0 拒绝、降级拒绝） |
| `server/src/architecture_guard_tests.rs` | Core/Extension 边界守卫七测试：源码边界白名单双向校验/依赖方向/v3-only 禁符/扩展生命周期全链/裸核 REST/YAML 隔离/Keymap 隔离 |
| `server/src/webrtc/` | pusher 推流 / viewer 生命周期 / 初始 GOP 重放 / 静止补帧 / DataChannel 控制转发 |
| `server/src/device/scrcpy.rs` / `frames.rs` / `mod.rs` | scrcpy 会话协议（v3.3.3，含 start_app/stop_app）/ 帧缓存（帧环 + 按需解码截图）/ DeviceManager（连接生命周期 / 状态 / 广播） |
| `web/src/workspace/` | 前端壳骨架：registry.ts（PanelRegistry + `DEFAULT_PANEL_KEY='gamer.core:tasks'`）、core-contributions.ts（Core 任务/日志/设置）、core-component-registry.ts（manifest `runtime="core"` 宿主组件解析表）、contribution-manager / lifecycle / PluginPanelHost / plugin-center、WorkspaceTabs.vue + PluginWorkspace.vue（主导航 任务|日志|市场|插件|设置 + 插件二级导航）、MarketView.vue（市场页）、PackageContextBar.vue / PackageDetailModal.vue（Package 上下文条与详情弹窗） |
| `web/src/package-store.js` | Package Context 前端单一来源：包列表 + `currentPackageId`（localStorage 持久化、失效回落首包）+ `selectPackage`/`loadPackages`（plan §38/§39） |
| `web/src/composables/usePackageContext.js` | Package 上下文条逻辑收敛：包 id 规整/校验（与服务端 `validate_scope_id` 对齐）、导入/导出/新建/复制/删除/详情动作，依赖注入可测 |
| `web/src/gamer-plugin-ids.js` | 插件 id（gamer.yaml / gamer.keymap）与插件内目录（automations/functions/templates/mappings）字面量的唯一前端配置点（api.js 资源 URL 拼装与扩展契约点共用） |
| `web/src/components/task/` | 通用任务表单支撑：runner-editors.ts（`RunnerEditorContribution` 契约注册表，编辑上下文携带 `packageId`）、builtin-runner-editors.ts（gamer.yaml 执行目标+payload 编辑器，参数表单走 `GET /api/runners/:runner_id/entrypoint` schema API，前端不解析 YAML 取参）、gamer-yaml-resources.ts（自动化/函数候选经通用 Package 资源 API） |
| `web/src/gamer-yaml-runner.js` | gamer.yaml runner 注册 id 唯一前端配置点 + `api.run` 包装（api.js 保持 runner 无知） |
| `web/src/gamer-keymap-extension.js` | gamer.keymap 扩展 id 唯一前端配置点 + 远端映射运行态判定 `isRemoteKeymapRunning`（输入路由开关；壳/workspace 接线不含扩展 id 字面量） |
| `web/src/script-editor/` | YAML v3 可视化编辑器核心（model=唯一编辑源（19 类步骤判别联合）、codec=YAML↔Model、校验/诊断、命令栈撤销重做、components 步骤画布/卡片/参数表单（DefaultsEditor 承载 defaults）；call 卡片目标为当前 Package 内候选下拉（target 语法 `script:<资源id>` / `function:<文件短路径>/<函数名>`，targets.ts 注入契约），选定即按目标声明自动生成 args（默认值预填），新插入步骤自动展开）；契约见 docs/reference/SCRIPT_EDITOR_CONTRACT.md |
| `web/src/views/Console.vue` | 投屏控制：WebRTC 前端（连接锁防双 PC / 坐标映射 / 框选模板）+ 设备列表管理（scan/连接/删除）+ registry 装配右侧面板；脚本运行模式：只读步骤摘要卡片（ScriptSummary）+ 运行事件 feed（`useRunEvents.js`/`RunEventsPanel.vue`：se 事件分发、budget/失败行高亮、step path 高亮当前顶层卡、失败标红），「▶ 从此运行」→ start_index 提交（顶层步骤序号；点击卡片选中已删，顶部「运行」恒从头跑），有 params 先弹参数表单（RunParamsModal，400 诊断回填、resolved_args 摘要进日志）；call 卡片（script:/function: 目标）可跳转子脚本/函数定义 |
| `web/src/components/console/` | Console 域组合式函数与面板实现：use{ConsolePanelResize,ConsoleDeviceManager,ConsoleTemplates,ConsoleBridgeOverlays,ConsoleScriptRunner,ConsoleKeymap,ConsoleWorkspacePanels,RunEvents}*.js、ScriptRunner.vue（脚本/函数双面板上下文 + RunEventsPanel 运行事件 feed + ScriptSummary 步骤高亮）、TemplateCapture.vue、KeymapPanel.vue |
| `web/src/components/LogsPanel.vue` / `TaskBoard.vue` / `SystemPanel.vue` | Core 自有面板（`gamer.core:logs/tasks/settings`），经 registry 注册 |
| `tools/build-plugins.ps1` + `tools/plugin-signer/` | 官方插件产物链：guest→Component→签名 .gplugin（web/public/plugins/）→registry.json；打包 manifest 源在 `tools/plugins/<id>/manifest.toml`（与 Rust 常量 include_str! 锁同步） |

## 规则

- 开发/运行中踩到的坑（环境、构建、部署、已知限制）必须记入 [docs/PITFALLS.md](docs/PITFALLS.md)；
  每条保持**精简准确**：一句话现象 + 原因 + 解决/规避，不写流水账、不夸大
- 修改yaml引擎，必须同步检查，前端校验代码，模板代码，yaml 文档
- git 提交遵守下方「Git 提交规范」

## Git 提交规范（Conventional Commits）

- 提交格式：`<type>(<scope>): <描述>`；type 限定：feat / fix / docs / style / refactor / perf / test / build / ci / chore / revert
- scope 用主要改动面（engine / web / api / device / data / scheduler / agents 等），建议写但不强制
- 描述用中文一句话直陈结果；复杂改动按本仓库惯例把要点铺在同一行内展开，需要更多细节时空一行写正文分条
- 主题不同的改动拆成多个提交（引擎 / 前端 / 数据 / 文档分开），单个提交自洽、可独立回滚
- 破坏性变更：type 后紧跟 `!`（如 `feat(engine)!:`），正文末尾另起 `BREAKING CHANGE: <影响与迁移>` 脚注

## 已知坑

清单整体移至 **[docs/PITFALLS.md](docs/PITFALLS.md)**（踩坑记录唯一维护处，每条一句话现象 + 原因 + 解决/规避；新条目追加到该文件末尾），本文件不再内嵌具体条目。
