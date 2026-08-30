# AGENTS.md

## 项目

GameBot 游戏自动化助手：Rust 服务端（axum + webrtc-rs）+ Vue3/Vite 前端。
scrcpy 采集 Android 设备画面 → WebRTC（H.264 视频轨 + DataChannel 控制）推流到浏览器，
支持触控控制、模板匹配（NCC）、YAML 自动化、cron 定时任务。

## 目录与端口

- `server/` — Rust 服务端，监听 **8443**，静态托管 `web-dist/`（构建产物）
- `web/` — Vue3 + Vite，dev 监听 **5173**，`/api`、`/ws` 代理到 8443；路由为 hash 模式
- `server/config.toml` — 关键项：`adb_path`、`ffmpeg_path`、`scrcpy_server`、`password`（默认 admin/admin123，前端无鉴权拦截，token 仅本地标记）
- `server/data/gamer.db` — SQLite（设备/任务/日志）；脚本/函数库/模板按应用分区文件存储：`data/<应用包名>/yaml/` 可运行脚本、`data/<应用包名>/func/` 函数库、`data/<应用包名>/tmpl/` 模板图片（分区名=设备配置的 pkg，目录即类型、跨分区不解析、无 default 兜底）

## 常用命令

```powershell
.\gamer.ps1 start|stop|restart|status   # 前后端一起管；-BackendOnly / -FrontendOnly / -Build / -Release
cd server && cargo run                  # 单起后端（日志 GB_LOG 追加到 server/gamer-server.log）
cd web && npm run dev                   # 单起前端
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
- 模板/脚本存储（2026-08 起目录即类型三分，取代旧 `data/scripts/<package>/` + 全局 `data/templates/`）：**分区 = 设备配置的应用包名（pkg）**，目录 `data/<pkg>/yaml/`（可运行脚本，顶层必须有 steps）+ `data/<pkg>/func/`（函数库，顶层键=函数名，不可运行/调度、不进脚本列表与任务选择器）+ `data/<pkg>/tmpl/`（灰度模板 PNG），跨分区不解析不回退、无 default 兜底；脚本 id = `<pkg>/<名>.yaml`（**含 `/`，前端拼 URL 必须整体 encodeURIComponent**，axum 会对 `%2F` 解码；tasks.script_id 同格式），函数库 id = `<pkg>/<文件短路径>.yaml`、函数路径 = `<文件短路径>/<函数名>`；旧 `package <名字>` 指令已删除（残留=解析报错）；脚本/函数库/模板 API 全部带 pkg 参数（模板 list 可省略=跨分区全列、条目带 pkg 字段，其余必填），`POST /api/scripts`、`POST /api/functions` body 加 `pkg`，导入 `POST /api/scripts/import?pkg=<分区>&confirm=1`（pkg 必填）；**导出 = 整分区全量快照**（`GET /api/scripts/export?pkg=<分区>` → `<pkg>.zip`，yaml+func+tmpl 全量）；zip = 分区快照（`yaml/<名>` + `func/<名>` + `tmpl/<模板>`，三目录均可缺省，不认旧布局）；**模板支持短名引用**：脚本写 `login.png` 精确文件不存在时，在同扩展名文件中唯一匹配 `login#*`（`scripts::resolve_template_path`，零候选/多候选均报错不猜测）；模板名 `#` 后缀 = 搜索区元数据（半区码 a/u/d/l/r/ul/ur/dl/dr 或 `#x1_y1_x2_y2` 相对 ×1000 坐标，`engine::exec::tpl_region_from_name`，后缀在扩展名前；无后缀回退全屏并每运行每模板记一条日志）；旧目录布局启动时一次性迁移（`scripts::migrate_fs_layout`）；Console 与编辑器顶部分区下拉统一切换脚本+函数库+模板区；ScriptPicker 加 `package` prop 传入则锁定分区单下拉（Console 用，TaskScheduler 仍双下拉）；**模板上传（框选/上传）服务端统一重编码为 8-bit 灰度 PNG**（`matcher::reencode_template_gray_png`，Best 压缩+自适应滤波）：匹配只消费灰度故对匹配**零损失**、体积较 RGB PNG ↓60~75%，缩略图/预览变灰是有意取舍，旧彩色模板不迁移、匹配时照常转灰度
- **YAML 脚本引擎 v2（2026-08 全新语法，与 v1 完全不兼容；权威文档 docs/YAML.md，契约 docs/SCRIPT_EDITOR_CONTRACT.md + 可执行样例 server/tests/fixtures/script_v2/）**：目录即类型——`yaml/` 可运行脚本（顶层只许 **params/config/steps**，steps 必需、可空列表不可省略；旧顶层键 func/name/action_wait/default_threshold/package/until/cond 报 legacy_format 引导迁移）+ `func/` 函数库（顶层键=函数名、记录只许 params/steps、无文件级 config、**不可运行/调度**，只被 func 步骤或函数测试 API 调用）；**params** 声明 `'类型:变量名:备注[:默认值]'` **整条单引号强制**（splitn(4) 切分——第 4 段整体为默认值尾串、text 默认可含冒号；空默认值非法；保留名 true/false/null 与 gb_ 前缀），七类 tmpl/coord/color/time/key/text/bool（time 强制带单位 >0、color 全位置 6 位 hex 无 # 字符串且纯数字色值 YAML 里必须引号防前导零、coord 0~1、bool 仅字面 true/false）；步骤字段 **`$name` 完整值引用**（参数作用域栈最内层优先，call/func 进栈出栈；$N/^N 文本替换全删）；**17 种步骤**：str_app/cls_app（裸写，包名=分区）/tap/swipe（YAML 键 fm/to/time）/key/text/log/wait（支持 `[1s,3s]` 随机区间）；**find**：主模板+`block` 障碍列表轮询（每轮新截图）、命中恒点模板中心、`verify: true`=点击后等 interval 重匹配仍命中补一击共两击、`timeout` 默认 30min、超时走 else；**match**：紧凑缩进候选（indentless 序列，唯一序列化格式）、`else`/`timeout` 是 match 兄弟键（写进候选列表报错）、每轮单帧按序匹配首个命中执行其分支、**不点击**、无 timeout 只跑一轮、有则按 config.interval 轮询、候选短名重复装载期+参数绑定后双查重；**color**：`at`+`expect`（值映射内有序列表、每项单键映射 `颜色: [分支]`）、`else` 步骤级、单点单帧按序判色容差 30/通道、不轮询不点击；**if** 严格布尔无隐式转换；**loop** 值=映射 `{times, steps}`（times 省略/0=无限、steps 必需非空）；**call**：同分区 yaml/ 脚本（缺 .yaml 自动补全）+ 具名 `args`、目标 config 三键压栈返回恢复、无布尔分支；**func**：`<文件短路径>/<函数名>`、**继承调用点 config**、`return: true|false` 仅函数库合法、函数体走完未 return 默认 true、返回布尔驱动 then/else；**throw** 跨 call/func 调用链结束整个运行（失败终态）；护栏：call+func 合计嵌套 ≤32 层、10 万步 guard、wait 200ms 分片可停；**装载**=script_v2（saphyr 事件级解析保标量样式+Span → 严格 AST（loader/validate/params/serialize），结构化五元组错误 `{code,message,resource,step_path,field}` 五域 resource/param/step/ref/runtime，前端按 code+step_path 定位不解析文案）；**执行**=engine/exec.rs（RunTarget `script|function` 二选一；运行开始 snapshot.rs 捕获分区源码快照不可变、call/func 懒解析按运行实例缓存；config.toml→脚本 config 三键覆盖；绑定顺序冻结「声明默认值→显式 args/入参覆盖」）；**运行入口**：手动 `POST /api/scripts/:id/run`（稀疏 args→202 `{run_id,resolved_args}`，400 invalid_args 带诊断列表）、函数测试 `POST /api/functions/:id/run`、Console 运行模式=只读摘要卡片「从此步骤运行」（顶层卡片→start_index）+编辑器卡片「从此步骤测试函数」（start_index=函数体内序号）；**定时任务**：保存即解析为**全量类型化 args 快照**+psig1 参数签名（覆盖类型/名称/必填性/默认值，前后端双实现）持久化，调度用快照不回读声明默认值；脚本声明变化→签名不一致→任务标「参数已过期」、保存/启用/立即运行被 409 `param_signature_conflict` 拦截（reason=signature_mismatch/no_snapshot），「重新确认」带 `reconfirm:true` 重算；**录制输出形态**（v11 fixture）：点击→单条 find、滑动→match→swipe（+默认 `else: throw 未找到滑动起点`+`timeout: 30s`），模板命名 `record_<click|swipe>_YYYYMMDD_NNN.png` 冲突顺延、搜索区域写进文件名 `#` 后缀

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
| `server/src/scripts.rs` | 按应用分区三目录（yaml/func/tmpl）的脚本/函数库/模板文件存储（分区寻址 / 模板短名消歧 / zip 分区快照导入导出 / 旧目录迁移） |
| `server/src/script_v2/` | 脚本 v2 装载/校验/序列化：loader（saphyr 事件级解析保样式+Span→AST）、validate（语义+引用图）、params（声明解析/七类字面量/args 绑定）、serialize（规范 YAML）、model（AST + psig1 签名） |
| `server/src/engine/exec.rs` / `snapshot.rs` / `matcher.rs` / `scheduler.rs` | v2 执行引擎（RunTarget/步骤语义/护栏）/ 运行源码快照 / NCC 模板匹配 / cron 调度 |
| `server/src/api/functions.rs` / `runs.rs` / `tasks.rs` | 函数库 CRUD（func/，expected_version 冲突 409）/ 手动运行+函数测试（RunTarget、稀疏 args→resolved_args）/ 任务保存（快照+签名门禁 409） |
| `server/src/task_params.rs` | 定时任务参数快照与 psig1 签名门禁（脚本缺失/解析失败/无快照/签名不一致明确失败，日志不落参数值） |
| `web/src/script-editor/` | 可视化编辑器核心（model=唯一编辑源、codec=YAML↔Model、校验/诊断、命令栈撤销重做、components 步骤画布/卡片/参数表单） |
| `web/src/recording/` + `web/src/composables/useRecording.js` | 录制核心服务（手势分类/队列/裁切/命名）与编辑器接线（占位插入/定稿/重试/降级/丢弃） |
| `web/src/components/ScriptPicker.vue` | 脚本选择器：`package` prop 传入则锁定分区单下拉（Console），否则分区+脚本双下拉（TaskScheduler） |
| `web/src/views/Console.vue` | 投屏控制：WebRTC 前端（连接锁防双 PC / 坐标映射 / 框选模板）+ 设备列表管理（scan/连接/删除）+ 脚本运行模式：只读步骤摘要卡片（ScriptSummary），「▶ 从此运行」→ start_index 提交（顶层步骤序号；点击卡片选中已删，顶部「运行」恒从头跑），有 params 先弹参数表单（RunParamsModal，400 诊断回填、resolved_args 摘要进日志）；call/func 卡片可跳转目标资源 |
| `web/src/views/ScriptEditor.vue` / `TaskScheduler.vue` | 脚本/函数库/模板可视化编辑页（保存版本冲突检测、函数测试入口）/ 定时任务页（运行参数表单、参数过期横幅+三列对比、「重新确认」reconfirm） |

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

