# AGENTS.md

## 项目

GameBot 游戏自动化助手：Rust 服务端（axum + webrtc-rs）+ Vue3/Vite 前端。
scrcpy 采集 Android 设备画面 → WebRTC（H.264 视频轨 + DataChannel 控制）推流到浏览器，
支持触控控制、模板匹配（NCC）、YAML 自动化、定时任务。

## 目录与端口

- `server/` — Rust 服务端，监听 **8443**，静态托管 `web-dist/`（构建产物）
- `web/` — Vue3 + Vite，dev 监听 **5173**，`/api`、`/ws` 代理到 8443；路由为 hash 模式
- `server/config.toml` — 关键项：`adb_path`、`ffmpeg_path`、`scrcpy_server`、`[auth].password_hash`；开发登录密码只从 `GAMER_ADMIN_PASSWORD` 读取并在进程内生成 Argon2id PHC，无默认账号/密码
- `server/data/gamer.db` — SQLite（设备/任务/预设/运行/日志）；业务资源按应用分区六目录文件存储：`data/<应用包名>/{scripts,functions,templates,keymaps,presets,resources}/` + 工作区元数据 `package.toml`（分区名=设备配置的 pkg，目录即类型、跨分区不解析、无 default 兜底；composite 解析优先级 **EditableLocal（分区目录）> UserOverride > InstalledPackage**，实现在 `server/src/resources.rs` + `server/src/app_packages/composite.rs`）
- 认证：配置只接受 Argon2id PHC `[auth].password_hash`；开发登录密码只用 `GAMER_ADMIN_PASSWORD`，无默认账号/密码。WebRTC 不内置 STUN/TURN，默认 host candidate 直连；Docker/NAT 需配置 `rtc_external_ip/rtc_udp_port/rtc_external_port` 并发布 UDP。
- 数据基线：SQLite schema v3（表：timer_tasks/task_presets/scheduled_runs/logs/devices；schedule JSON 为 `{provider_id,config}` 形态；v1→v2 Timer Core 泛化、v2→v3 Task 模型收口（legacy `tasks` 表 DROP）逐级静态迁移，注册表在 `server/src/migrations.rs`）；`user_version=0`/无版本号数据库不自动迁移，高于目标的库拒绝启动。
- 资源发行：**默认发行零业务资源**——业务分区目录（`data/<pkg>/{scripts,functions,templates,keymaps,presets,resources}/`）不再 git 跟踪；应用资产经 App Package 安装（`POST /api/app-packages/install`，zip/.gamerpkg，Manifest V2 强制 `format_version=2` 且新增 `functions/` 根，可带 `X-Expected-Sha256` 校验头；已装包存 `data/app-packages/`，安装即激活并发布包内 `presets/` 为任务预设），本地分区目录（EditableLocal）继续作为一等资源源可用；存量资产迁移用 Gamer 内置导出（`POST /api/app-packages/export`）与编辑提取（`POST /api/app-packages/:id/:version/edit`）。

## 架构分层（ADR-11~14，`docs/reference/adr/`）

- **Core（`server/src` 顶层）= 稳定机制**：`device/`（采集/会话/帧缓存）、`webrtc/`、`api/`、`store.rs` + `timer_core.rs` + `scheduler.rs` + `run_manager.rs`（任务/调度/运行）、`resources.rs`（ResourceStore，内容无关六目录寻址）、`cron_extension.rs`（Native schedule provider，provider_id=`cron`）、`capabilities/`（device/vision/input/touch/resource/run/runtime/log SDK）、`matcher.rs`（vision NCC）、`extensions/service.rs`（扩展生命周期状态机）。Core 不认识 YAML/Script DSL/Function DSL/Keymap rule/任何具体 Runner 实现。
- **业务归扩展**：`extensions/gamer_yaml/`（YAML parser/校验/序列化/执行引擎/定时 runner/任务参数门禁/wasm host）与 `extensions/keymap/`（keymap DSL 与 WASM 运行时适配）。**Extension ≠ WASM**：cron 是 Native Extension；WASM 只是扩展运行时之一（`extensions/wasm.rs`）。
- 最终形态 = Stable Core + Installable Extensions + App Packages + Generic Tasks（Task = 任意 ScheduleProvider + 任意 Runner）+ Generic Resources + Runner 归扩展所有 + 零 legacy 兼容（不保留旧 API/旧格式/旧数据目录/迁移开关，ADR-14）。
- 边界由测试锁死：`server/src/architecture_guard_tests.rs`（源码边界 67 条白名单双向校验/依赖方向/生命周期全链/裸核/YAML 隔离/Keymap 隔离）+ `web/src/core-shell-boundary.test.js`。

## 常用命令

```powershell
.\gamer.ps1 start|stop|restart|status   # 前后端一起管；-BackendOnly / -FrontendOnly / -Build / -Release
cd server && cargo run                  # 单起后端（日志 GB_LOG 追加到 server/gamer-server.log）
cd web && pnpm dev                      # 单起前端
```

## 关键链路（改代码前先看）

- 连接：浏览器 → `POST /api/devices/:id/connect`（scrcpy 会话，已在线时幂等 no-op）→ WS `/ws/device/:id` 信令（offer/answer；offer 可带 `force:true` 顶替已有 viewer，见"多页面互斥"）→ WebRTC 视频轨 + DataChannel `control`（触控/按键/文本）；脚本运行时引擎的 tap/swipe/匹配命中/未命中可视化事件也经该 DataChannel **反向**推送给投屏页面（`{"type":"se","ev":...}`；`hit`=命中框、`miss`=搜索区域框——`match_one` 未命中统一发出，find 主模板/block 与 match 候选全覆盖；engine `emit` → viewers 注册表 `control_dc`，定时任务运行同样生效）
- 帧缓存 `FrameCache`（帧环 + 按需解码）：截图/模板匹配时用临时 ffmpeg 解**最新一帧**（天然实时，无陈旧/停滞问题），并为**新 viewer 重放初始帧（SPS/PPS + 最近 GOP）**。ffmpeg 不可用 → 无初始帧 → 浏览器黑屏（见 docs/PITFALLS.md）
- 单设备单 viewer：新连接踢旧连接（`AppState.viewers` 注册表）
- 视频静默看门狗（`spawn_watchdog`，2026-08-22 重构）：判死以 `session.connected`（video socket 读取循环退出即 false）为准，**视频静默 ≠ 死链路**。会话确死 → force 拆会话，有脚本/viewer 则立即重连（**唯一允许脚本运行中强拆的路径**——控制 socket 同链路已死，不重连脚本永远卡死）；脚本运行中 + 会话活着 + 静默 → 不处置（静态屏/黑屏正常态）；无 viewer 无脚本 → 交给 idle_power_loop；viewer 在看且未被补帧投喂（last_serve ≥10s）→ reset_video 探测，15s 仍静默才拆开重连踢 viewer
- 多页面互斥（服务端仲裁，2026-08-20 重做，取代旧 localStorage 锁 `gb_webrtc_lock`——锁只能管同一浏览器，跨浏览器/跨 PC 管不到）：新页面 offer（不带 force）遇已有活跃 viewer → 服务端回 `{"type":"conflict"}`（不踢不建连）；前端**手动连接**弹窗确认后带 `force:true` 重发 offer 才接管，**自动重连**遇 conflict 直接放弃并提示。接管只换浏览器↔服务端链路：先经旧页面的信令 ws 推 `{"type":"taken_over"}`（`ViewerHandle.notify` 通道 + ws 循环 peer 关闭后 200ms 冲刷窗口保证送达）再关旧 peer，**设备 scrcpy 会话不动**（实测 ~0.3s 无缝切换）；被顶页面收到 taken_over 后断开且不再自动重连（防互顶死循环）
- 配置变更（PUT /api/devices/:id）：仅**投屏相关参数**（kind/addr/screen_mode/vd_res/vd_dpi/fps，`session_affecting_change` 按生效值归一比较）变更才踢 viewer + 拆会话（前端 onclose → 自动重连恢复画面）；仅改名称/应用包名不拆会话、投屏不中断；投屏参数变更遇脚本运行中仍不踢不拆（运行守卫），新配置下次连接才生效。前端设备管理收在投屏工具条（设备下拉/连接/刷新/新增/设置/删除），配置编辑走「⚙️ 设置」弹窗（DeviceSettingsModal，显式保存/取消，不再自动保存防抖）
- 设备扫描：`POST /api/devices/scan` 执行 `adb devices -l`，按 addr 去重自动入库（逻辑在 `DeviceManager::scan_and_sync`，服务器启动时也自动跑一次）
- App 生命周期 / 空闲低功耗（2026-08-22 重做）：连接**不再自动启动应用**（由脚本 `str_app`（冷启动，"+" 前缀控制消息）或 Console 启动按钮显式触发）；**会话存活由 `DeviceManager::idle_power_loop`（10s 周期）唯一管理**——无 viewer 且无脚本运行持续 `idle_power_secs`（config.toml，默认 300，0=关）秒 → 虚拟屏模式拆 scrcpy 会话（编码停止/虚拟屏销毁，恢复 freezer 禁用/音量静音等设备侧改写，**adb 链路保留**：WiFi/emu 设备每 60s 补 `adb connect` 保活，启动时自举扫描+连接、不建会话不启动应用）；镜像模式关物理屏（keyevent 223，**会话保留**），消费者回来即唤醒。消费者出现（ws viewer 注册 / 脚本 `run_begin` → `notify_activity`）打断空闲计时并即时唤醒已关的屏；镜像 30s 补醒也移入该循环（connect 时的保活任务只管拉满/恢复熄屏超时）。**`disconnect_device` 带运行守卫**：脚本运行中拒绝拆会话（虚拟屏销毁会杀掉屏上游戏），仅 force=true 绕过（删除设备 / 看门狗确认死链路 / 手动 `POST /api/devices/:id/disconnect` 管理动作）；前端"断开连接"按钮只断本页 WebRTC **不再调该接口**。下次运行脚本/定时任务自动重连（~2-4s）
- 任务模型（ADR-12）：Task = 任意 ScheduleProvider + 任意 Runner，嵌套 JSON `schedule{provider_id,config}` + `runner{runner_id,entrypoint,payload}`（无 script_id/cron 顶层字段；YAML 参数在 `runner.payload`、cron 表达式在 `schedule.config`）。REST 唯一入口 `/api/tasks`（CRUD + run/suspend/resume/cancel/enable/disable）与预设 `/api/task-presets`；缺 Runner/provider 的任务落 `TaskState::DependencyMissing`（不删、保留 enabled 原意），Runner 重注册自动恢复
- Runner 生命周期（ADR-13）：`TimerRunnerRegistry` 注册项带 `owner_extension_id`（register/unregister/unregister_owner）；gamer.yaml 的 Runner 由扩展 **start（进入 Running）** 经 `TimerRunnerRegistrar` 钩子注册，stop/disable/uninstall 注销；**enable ≠ start**（Enabled 未 start 的扩展不注册 Runner）；服务重启 `extensions/service.rs::reconcile_startup` 只恢复遗留 Running 记录——升级后存量定时任务需手动 start 一次 gamer.yaml。裸 Core `Scheduler::new(db)` 不预置任何 Runner
- 统一执行：`POST /api/runs`（body `{runner_id, entrypoint, payload, device_id}`，202 + run_id/resolved_args；同设备已有活动运行 409 附当前运行信息）、`GET /api/runners`（含 owner）、`GET /api/schedule-providers`。手动运行的前置存在性校验走 `ResourceStore::get_text`（非 keymap kind 只读本地编辑区，纯包内脚本返回结构化 not_found），真实执行走 gamer_yaml 运行快照（composite 三层合并）。前端 `api.js` 无业务 runner id（gamer.yaml 包装在 `web/src/gamer-yaml-runner.js`，是 runner 注册 id 的唯一前端配置点）
- 资源存储（内容无关）：分区 = 设备配置的应用包名（pkg），目录即类型（六目录）；寻址/枚举/乐观并发（更新必须带 `expected_version`，`force:true` 显式跳过）由 Core ResourceStore 统一承载，内容语义归扩展（`ResourceKindHandler` 注册表回调：gamer_yaml 注册 scripts/functions 校验器与模板 handler，如模板重命名的脚本引用改写）。REST：`GET|POST /api/apps/:app/resources/:kind`、`GET|PUT|DELETE /api/apps/:app/resources/:kind/:id`（app 段可传 `-` 由 id 自带分区；文本 kind 收 JSON、字节 kind（templates）收 PNG 字节 body）。脚本 id = `<pkg>/<名>.yaml`（前端拼 URL 必须整体 `encodeURIComponent`）；函数路径 = `<文件短路径>/<函数名>`；模板为 8-bit 灰度 PNG、短名在当前分区唯一匹配 `#` 后缀。脚本/函数语法与校验规则见 docs/reference/YAML.md（引擎 v2 语法，仍由 gamer_yaml 扩展承载）
- App Package / composite 解析：`server/src/app_packages/` 负责不可变包的安装（staging+原子安装、每版本 `install.json` 记录归档 SHA-256、**同 id+version 重装按 overwrite 整体替换**）、active 版本注册表（`app-packages/active.json`）与 primary 唯一约束（一个 android package 只允许一个 active 内容包，冲突 409）。资源解析顺序 **EditableLocal（分区目录）→ UserOverride → active App Package**：keymaps 经 ResourceStore composite（包内方案只读，本地 `create` 拒绝同名防遮蔽）、脚本/函数库经 gamer_yaml 运行快照三层合并（包内 `scripts/` 只进脚本索引、`functions/` 只进函数索引，索引彻底分离）、模板经扩展侧模板 handler（find/match 与脚本校验同源）。REST：`POST /api/app-packages/install`（zip/.gamerpkg，可选 `X-Expected-Sha256`）、`GET /api/app-packages`、`DELETE /api/app-packages/:id/:version`（卸最后一版挂起既有任务，预设记录保留）、`POST /api/app-packages/:id/activate`、`POST /api/app-packages/export`（本地编辑区 → .gamerpkg，Rust `PackageBuilder`：preflight→collect→manifest→zip→verify，可复现打包固定 mtime）、`POST /api/app-packages/:id/:version/edit`（包整体提取回本地编辑区，staging+Preflight+原子替换）、`GET|PUT /api/workspace/:android_package`（package.toml 元数据+六目录统计）；包内 `presets/*.yaml` 在安装/激活时经 `TimerCore::publish_package_presets` 灌入任务预设（发布 id = `pkg:<包>/<名>` 确定性生成，幂等）
- Console 壳：`web/src/views/Console.vue` 只保留模板装配 + 投屏连接/输入控制接线，右侧面板**全 registry 驱动**——Core 自有 任务/日志/设置（`gamer.core:tasks|logs|settings`，`workspace/core-contributions.ts`），业务面板（gamer.yaml 的 自动化/函数/模板、gamer.keymap 的 映射）由扩展 manifest `runtime="core"` + `component` 键贡献（`workspace/core-component-registry.ts` 是宿主组件名的唯一前端知识），安装即出现、禁用/卸载即消失；裸 Core 右侧 = 任务|日志|设置|+，`DEFAULT_PANEL_KEY='gamer.core:tasks'`。行为契约（hash 路由 panel 同步、连接锁、坐标映射、框选、运行守卫）不变；按域拆分在 `web/src/components/console/use*.js` 组合式函数
- Declarative 插件 UI：manifest.toml 里 `runtime = "declarative"` 的 `[[ui.contributions]]` 可带 `description` + `[[ui.contributions.fields]]`（`type` 限 text/number/boolean/select/button，支持 `name`(alias `key`)/`label`/`placeholder`/`default`/select 的 `options`/button 的 `action` 与字段 `description`，未知控件类型解析报错）；schema 由 `manifest.rs` 校验、`ui.rs` 随 `RegisteredUiContribution` 透传到 `GET /api/extensions` 的 `ui_contributions`，前端 `PluginPanelHost.vue` 原生渲染表单，按钮直接调 `POST /api/extensions/:id/call`（body `{action, values}`）。UI runtime 共三档：declarative | iframe | core（宿主组件）
- 任务表单（TaskBoard 通用化）：`web/src/components/TaskBoard.vue` 是「ScheduleProvider + Runner」通用表单——provider 下拉来自 `GET /api/schedule-providers`（内置 `cron`），runner 下拉来自 `GET /api/runners`；执行目标/参数编辑由 `components/task/runner-editors.ts` 的 `RunnerEditorContribution` 契约按 runner_id 注入（内置 gamer.yaml 编辑器在 `builtin-runner-editors.ts`，分区候选走通用资源 API）
- **WASM 插件生态（Phase 6/7/8/10 已收口为产品能力）**：`wasm-runtime` 进 default feature（lazy init——不装/不启动插件不建 Wasmtime Engine；`--no-default-features` 保留无 WASM 退出路径，CI 有防退化检查）。keymap 扩展启动可带 `profile`（start body `{profile: <分区方案名>}` + `app_context.android_package` 指定分区，从 `data/<pkg>/keymaps/` 读 YAML 原文经 WIT `start(profile)` 传给 guest，guest 内置 WASD 默认规则、profile 覆盖之，缺省/空 profile = 未映射键全部 pass-through；无 keymap 扩展时输入直通 scrcpy）。declarative `plugin.call` 走通用 extension world 的 `call(action, values-json)` 导出，action 必须在该 manifest declarative schema 按钮集合内否则 400。官方市场：`tools/build-plugins.ps1` 把 `server/tests/{keymap,yaml}-guest` 打成签名 `.gplugin` 输出到 `web/public/plugins/` 并生成 `web/public/registry.json`（ed25519 签名，dev keypair 在 `tools/plugin-signing/` 仅本地市场用，公钥内嵌 server 信任锚 `signature.rs`；Registry proof 绑定 id/version/download_url/sha256，官方源安装必须验签通过）

## 关键文件

| 文件 | 职责 |
|---|---|
| `server/src/api/mod.rs` | REST 装配：设备 CRUD/scan/connect/control/截图、统一任务 `/api/tasks`（run/suspend/resume/cancel/enable/disable）+ `/api/task-presets`、`POST /api/runs` + run 查询/取消、`GET /api/runners`、`GET /api/schedule-providers`、通用资源 `/api/apps/:app/resources/:kind[/:id]`、`/api/capabilities/vision/test`、`/api/extensions/*`（install/enable/disable/start/stop/call/ui）、App Package（install/list/uninstall/activate/export/edit）、`GET&#124;PUT /api/workspace/:pkg`、日志、system/update |
| `server/src/api/ws.rs` | WebRTC 信令；取 `frame_cache.initial_frames()` 传给 ViewerSession |
| `server/src/app_packages/` | App Package 存储/解析边界：安装（staging+原子、SHA-256、同 id+version overwrite、primary 唯一约束）、active 注册表、composite 解析（EditableLocal→override→包）、包内 presets 解析、PackageBuilder 导出（builder.rs，preflight 目录自洽校验）/workspace 元数据（workspace.rs）/编辑提取（edit.rs） |
| `server/src/resources.rs` | ResourceStore：内容无关六目录寻址（分区=app、目录=kind）+ composite EditableLocal→UserOverride→InstalledPackage + 内容版本短码 + `ResourceKindHandler` 注册表（内容校验/语义回调给扩展；keymap kind 走 composite，其余 kind 直读本地编辑区） |
| `server/src/timer_core.rs` | Timer Core：`Task`/`TaskSchedule{provider_id,config}`/`TaskState`（含 `DependencyMissing`）、`TimerRunnerRegistry`（owner_extension_id + register/unregister/unregister_owner）、ScheduleRegistry、包内 presets 发布 |
| `server/src/scheduler.rs` / `run_manager.rs` | 裸 Core 调度器（`Scheduler::new(db)`，不预置 Runner；经 TimerRunnerRegistry 分发）/ RunManager（统一 RunRecord、同设备单活动运行 409、run_id 查询/取消） |
| `server/src/cron_extension.rs` | Native schedule provider（provider_id=`cron`，5/6 域表达式归一 7 域） |
| `server/src/extensions/service.rs` | 扩展生命周期状态机 + `TimerRunnerRegistrar` 钩子（start 注册/stop 注销 runner）+ `reconcile_startup` 重启对账（恢复遗留 Running） |
| `server/src/extensions/gamer_yaml/` | YAML 扩展边界：`script_v2/`（loader/validate/params/serialize/model，严格诊断 `{code,message,resource,step_path,field}`）、`yaml_vnext.rs`（v3 surface）、`engine/`（exec/snapshot/runner_adapter/ports，v2 引擎 + v3 WASM adapter 双入口 `validate_compatible_script`）、`timer_yaml.rs`（YamlTimerRunner + Registrar）、`task_params.rs`（定时任务参数快照与 psig1 签名门禁）、`resources.rs`（ResourceKindHandler：scripts/functions 校验、模板重命名引用改写）、`wasm_host.rs` |
| `server/src/extensions/keymap/` | keymap 扩展边界：`dsl.rs`（keymap YAML 解析/校验/序列化）+ WASM 运行时适配（无扩展运行时输入直通 scrcpy） |
| `server/src/capabilities/` | Core SDK：device/vision/input/touch/resource/run/runtime/log 域 + adapters 注册表 |
| `server/src/matcher.rs` | vision NCC 模板匹配（Core 侧，等比缩放加速，命中坐标映射回原图） |
| `server/src/store.rs` + `migrations.rs` | SQLite 持久化（devices/timer_tasks/task_presets/scheduled_runs/logs）+ schema v1→v2→v3 静态迁移注册表（`user_version` 权威，v0 拒绝、降级拒绝） |
| `server/src/architecture_guard_tests.rs` | Core/Extension 边界守卫六测试：源码边界白名单双向校验/依赖方向/扩展生命周期全链/裸核 REST/YAML 隔离/Keymap 隔离 |
| `server/src/webrtc/` | pusher 推流 / viewer 生命周期 / 初始 GOP 重放 / 静止补帧 / DataChannel 控制转发 |
| `server/src/device/scrcpy.rs` / `frames.rs` / `mod.rs` | scrcpy 会话协议（v3.3.3）/ 帧缓存（帧环 + 按需解码截图）/ DeviceManager（连接生命周期 / 状态 / 广播） |
| `web/src/workspace/` | 前端壳骨架：registry（PanelRegistry + `DEFAULT_PANEL_KEY='gamer.core:tasks'`）、core-contributions.ts（Core 任务/日志/设置）、core-component-registry.ts（manifest `runtime="core"` 宿主组件解析表）、contribution-manager / lifecycle / PluginPanelHost / plugin-center |
| `web/src/components/task/` | 通用任务表单支撑：runner-editors.ts（`RunnerEditorContribution` 契约注册表）、builtin-runner-editors.ts（gamer.yaml 执行目标+payload 编辑器）、gamer-yaml-resources.ts（分区候选经通用资源 API） |
| `web/src/gamer-yaml-runner.js` | gamer.yaml runner 注册 id 唯一前端配置点 + `api.run` 包装（api.js 保持 runner 无知） |
| `web/src/gamer-keymap-extension.js` | gamer.keymap 扩展 id 唯一前端配置点 + 远端映射运行态判定 `isRemoteKeymapRunning`（输入路由开关；壳/workspace 接线不含扩展 id 字面量） |
| `web/src/script-editor/` | 可视化编辑器核心（model=唯一编辑源、codec=YAML↔Model、校验/诊断、命令栈撤销重做、components 步骤画布/卡片/参数表单；call/func 卡片目标为分区候选下拉（func=文件+函数名两级，targets.ts 注入契约），选定即按目标声明自动生成 args（默认值预填），新插入步骤自动展开）；契约见 docs/reference/SCRIPT_EDITOR_CONTRACT.md |
| `web/src/views/Console.vue` | 投屏控制：WebRTC 前端（连接锁防双 PC / 坐标映射 / 框选模板）+ 设备列表管理（scan/连接/删除）+ registry 装配右侧面板；脚本运行模式：只读步骤摘要卡片（ScriptSummary），「▶ 从此运行」→ start_index 提交（顶层步骤序号；点击卡片选中已删，顶部「运行」恒从头跑），有 params 先弹参数表单（RunParamsModal，400 诊断回填、resolved_args 摘要进日志）；call/func 卡片可跳转目标资源 |
| `web/src/components/console/` | Console 域组合式函数与面板实现：use{ConsolePanelResize,ConsoleDeviceManager,ConsoleTemplates,ConsoleBridgeOverlays,ConsoleScriptRunner,ConsoleKeymap,ConsoleWorkspacePanels}*.js、ScriptRunner.vue（脚本/函数双面板上下文）、TemplateCapture.vue、KeymapPanel.vue |
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
