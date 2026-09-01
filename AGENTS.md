# AGENTS.md

## 项目

GameBot 游戏自动化助手：Rust 服务端（axum + webrtc-rs）+ Vue3/Vite 前端。
scrcpy 采集 Android 设备画面 → WebRTC（H.264 视频轨 + DataChannel 控制）推流到浏览器，
支持触控控制、模板匹配（NCC）、YAML 自动化、cron 定时任务。

## 目录与端口

- `server/` — Rust 服务端，监听 **8443**，静态托管 `web-dist/`（构建产物）
- `web/` — Vue3 + Vite，dev 监听 **5173**，`/api`、`/ws` 代理到 8443；路由为 hash 模式
- `server/config.toml` — 关键项：`adb_path`、`ffmpeg_path`、`scrcpy_server`、`[auth].password_hash`；开发登录密码只从 `GAMER_ADMIN_PASSWORD` 读取并在进程内生成 Argon2id PHC，无默认账号/密码
- `server/data/gamer.db` — SQLite（设备/任务/日志）；脚本/函数库/模板按应用分区文件存储：`data/<应用包名>/yaml/` 可运行脚本、`data/<应用包名>/func/` 函数库、`data/<应用包名>/tmpl/` 模板图片（分区名=设备配置的 pkg，目录即类型、跨分区不解析、无 default 兜底）
- 认证：配置只接受 Argon2id PHC `[auth].password_hash`；开发登录密码只用 `GAMER_ADMIN_PASSWORD`，无默认账号/密码。WebRTC 不内置 STUN/TURN，默认 host candidate 直连；Docker/NAT 需配置 `rtc_external_ip/rtc_udp_port/rtc_external_port` 并发布 UDP。
- 数据基线：SQLite 当前为 schema v1；`user_version=0`/无版本号数据库不自动迁移，后续迁移从 v1→v2 开始。

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
- 模板/脚本存储：分区 = 设备配置的应用包名（pkg），目录即类型 `data/<pkg>/{yaml,func,tmpl}/`；脚本顶层只允许 `params/config/steps` 且 `steps` 必需，函数库顶层键为函数名且记录只允许 `params/steps`。三类资源跨分区不解析、不回退；模板为 8-bit 灰度 PNG，短名在当前分区唯一匹配 `#` 后缀。脚本 id = `<pkg>/<名>.yaml`（前端拼 URL 必须整体 `encodeURIComponent`）；函数路径 = `<文件短路径>/<函数名>`。`POST` 创建、`PUT` 更新默认带 `expected_version`，`force:true` 跳过版本比较；导入/导出为分区快照，模板创建与图像替换分开。
- **YAML 脚本引擎 v2**：严格 loader 由 `server/src/script_v2/` 提供，错误统一为 `{code,message,resource,step_path,field}`；保存、导入、脚本运行、函数测试和任务保存共用该 loader。白名单外顶层键统一为 `script.top_level.unknown_key`，不提供格式迁移；`color` 的 `else` 只能在步骤级，候选列表内写入属于结构错误。

## 关键文件

| 文件 | 职责 |
|---|---|
| `server/src/api/mod.rs` | REST：设备 CRUD / scan / connect / control / 截图 / 模板 / 脚本 / 任务 / 日志 |
| `server/src/api/ws.rs` | WebRTC 信令；取 `frame_cache.initial_frames()` 传给 ViewerSession |
| `server/src/webrtc/` | pusher 推流 / viewer 生命周期 / 初始 GOP 重放 / 静止补帧 / DataChannel 控制转发 |
| `server/src/device/scrcpy.rs` | scrcpy 会话：视频/音频/控制 socket 协议（v3.3.3） |
| `server/src/device/frames.rs` | 帧缓存：帧环（SPS/PPS + GOP）+ 按需解码截图 |
| `server/src/device/mod.rs` | DeviceManager：连接生命周期 / 状态 / 广播 |
| `server/src/store.rs` | SQLite 持久化（设备/任务/日志；任务含 args 快照+param_signature 两列；scripts 表已随文件存储退役） |
| `server/src/scripts.rs` | 按应用分区三目录（yaml/func/tmpl）的脚本/函数库/模板文件存储（分区寻址 / 模板短名消歧 / zip 分区快照导入导出） |
| `server/src/script_v2/` | 脚本 v2 装载/校验/序列化：loader（saphyr 事件级解析保样式+Span→AST）、validate（语义+引用图）、params（声明解析/七类字面量/args 绑定）、serialize（规范 YAML）、model（AST + psig1 签名） |
| `server/src/engine/exec.rs` / `snapshot.rs` / `matcher.rs` / `scheduler.rs` | v2 执行引擎（RunTarget/步骤语义/护栏）/ 运行源码快照 / NCC 模板匹配 / cron 调度 |
| `server/src/api/functions.rs` / `runs.rs` / `tasks.rs` | 函数库 CRUD（func/，expected_version 冲突 409）/ 手动运行+函数测试（RunTarget、稀疏 args→resolved_args）/ 任务保存（快照+签名门禁 409） |
| `server/src/task_params.rs` | 定时任务参数快照与 psig1 签名门禁（脚本缺失/解析失败/无快照/签名不一致明确失败，日志不落参数值） |
| `web/src/script-editor/` | 可视化编辑器核心（model=唯一编辑源、codec=YAML↔Model、校验/诊断、命令栈撤销重做、components 步骤画布/卡片/参数表单；call/func 卡片目标为分区候选下拉（func=文件+函数名两级，targets.ts 注入契约），选定即按目标声明自动生成 args（默认值预填），新插入步骤自动展开） |
| `web/src/recording/` + `web/src/composables/useRecording.js` | 录制核心服务（手势分类/队列/裁切/命名）与编辑器接线（占位插入/定稿/重试/降级/丢弃） |
| `web/src/components/ScriptPicker.vue` | 脚本选择器：`package` prop 传入则锁定分区单下拉（Console），否则分区+脚本双下拉（TaskBoard） |
| `web/src/views/Console.vue` | 投屏控制：WebRTC 前端（连接锁防双 PC / 坐标映射 / 框选模板）+ 设备列表管理（scan/连接/删除）+ 脚本运行模式：只读步骤摘要卡片（ScriptSummary），「▶ 从此运行」→ start_index 提交（顶层步骤序号；点击卡片选中已删，顶部「运行」恒从头跑），有 params 先弹参数表单（RunParamsModal，400 诊断回填、resolved_args 摘要进日志）；call/func 卡片可跳转目标资源 |
| `web/src/components/console/ScriptRunner.vue` / `useScriptEditorShell.js` | 主控制台脚本运行/编辑外壳与共享可视化编辑核心（独立脚本管理页已移除） |
| `web/src/components/LogsPanel.vue` / `TaskBoard.vue` / `SystemPanel.vue` | 投屏控制台右侧面板的日志/任务/设置页签（Console.vue 五页签：模板/脚本/日志/任务/设置；旧独立页面与 URL 已删除） |

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
