# AGENTS.md

## 项目

GameBot 游戏自动化助手：Rust 服务端（axum + webrtc-rs）+ Vue3/Vite 前端。
scrcpy 采集 Android 设备画面 → WebRTC（H.264 视频轨 + DataChannel 控制）推流到浏览器，
支持触控控制、模板匹配（NCC）、YAML 自动化、cron 定时任务。

## 目录与端口

- `server/` — Rust 服务端，监听 **8443**，静态托管 `web-dist/`（构建产物）
- `web/` — Vue3 + Vite，dev 监听 **5173**，`/api`、`/ws` 代理到 8443；路由为 hash 模式
- `server/config.toml` — 关键项：`adb_path`、`ffmpeg_path`、`scrcpy_server`、`password`（默认 admin/admin123，前端无鉴权拦截，token 仅本地标记）
- `server/data/gamer.db` — SQLite（设备/任务/日志）；模板/脚本按应用分区文件存储：`data/<应用包名>/tmpl/` 模板图片、`data/<应用包名>/yaml/` YAML 脚本（分区名=设备配置的 pkg，无 default 兜底）

## 常用命令

```powershell
.\gamer.ps1 start|stop|restart|status   # 前后端一起管；-BackendOnly / -FrontendOnly / -Build / -Release
cd server && cargo run                  # 单起后端（日志 GB_LOG 追加到 server/gamer-server.log）
cd web && npm run dev                   # 单起前端
```

## 关键链路（改代码前先看）

- 连接：浏览器 → `POST /api/devices/:id/connect`（scrcpy 会话，已在线时幂等 no-op）→ WS `/ws/device/:id` 信令（offer/answer；offer 可带 `force:true` 顶替已有 viewer，见"多页面互斥"）→ WebRTC 视频轨 + DataChannel `control`（触控/按键/文本）；脚本运行时引擎的 tap/swipe/匹配命中/未命中可视化事件也经该 DataChannel **反向**推送给投屏页面（`{"type":"se","ev":...}`；`hit`=命中框、`miss`=搜索区域框——2026-08-27 起 `match_one` 未命中统一发出，find 主模板/block/verify/函数 cond 全覆盖；engine `emit` → viewers 注册表 `control_dc`，定时任务运行同样生效）
- 帧缓存 `FrameCache`（帧环 + 按需解码）：截图/模板匹配时用临时 ffmpeg 解**最新一帧**（天然实时，无陈旧/停滞问题），并为**新 viewer 重放初始帧（SPS/PPS + 最近 GOP）**。ffmpeg 不可用 → 无初始帧 → 浏览器黑屏（见 docs/PITFALLS.md）
- 单设备单 viewer：新连接踢旧连接（`AppState.viewers` 注册表）
- 视频静默看门狗（`spawn_watchdog`，2026-08-22 重构）：判死以 `session.connected`（video socket 读取循环退出即 false）为准，**视频静默 ≠ 死链路**。会话确死 → force 拆会话，有脚本/viewer 则立即重连（**唯一允许脚本运行中强拆的路径**——控制 socket 同链路已死，不重连脚本永远卡死）；脚本运行中 + 会话活着 + 静默 → 不处置（静态屏/黑屏正常态）；无 viewer 无脚本 → 交给 idle_power_loop；viewer 在看且未被补帧投喂（last_serve ≥10s）→ reset_video 探测，15s 仍静默才拆开重连踢 viewer
- 多页面互斥（服务端仲裁，2026-08-20 重做，取代旧 localStorage 锁 `gb_webrtc_lock`——锁只能管同一浏览器，跨浏览器/跨 PC 管不到）：新页面 offer（不带 force）遇已有活跃 viewer → 服务端回 `{"type":"conflict"}`（不踢不建连）；前端**手动连接**弹窗确认后带 `force:true` 重发 offer 才接管，**自动重连**遇 conflict 直接放弃并提示。接管只换浏览器↔服务端链路：先经旧页面的信令 ws 推 `{"type":"taken_over"}`（`ViewerHandle.notify` 通道 + ws 循环 peer 关闭后 200ms 冲刷窗口保证送达）再关旧 peer，**设备 scrcpy 会话不动**（实测 ~0.3s 无缝切换）；被顶页面收到 taken_over 后断开且不再自动重连（防互顶死循环）
- 配置变更（PUT /api/devices/:id）：仅**投屏相关参数**（kind/addr/screen_mode/vd_res/vd_dpi/fps，`session_affecting_change` 按生效值归一比较）变更才踢 viewer + 拆会话（前端 onclose → 自动重连恢复画面）；仅改名称/应用包名不拆会话、投屏不中断；投屏参数变更遇脚本运行中仍不踢不拆（运行守卫），新配置下次连接才生效。前端设备管理收在投屏工具条（设备下拉/连接/刷新/新增/设置/删除），配置编辑走「⚙️ 设置」弹窗（DeviceSettingsModal，显式保存/取消，不再自动保存防抖）
- 设备扫描：`POST /api/devices/scan` 执行 `adb devices -l`，按 addr 去重自动入库（逻辑在 `DeviceManager::scan_and_sync`，服务器启动时也自动跑一次）
- App 生命周期 / 空闲低功耗（2026-08-22 重做）：连接**不再自动启动应用**（由脚本 `str_app`（冷启动，"+" 前缀控制消息）或 Console 启动按钮显式触发）；**会话存活由 `DeviceManager::idle_power_loop`（10s 周期）唯一管理**——无 viewer 且无脚本运行持续 `idle_power_secs`（config.toml，默认 300，0=关）秒 → 虚拟屏模式拆 scrcpy 会话（编码停止/虚拟屏销毁，恢复 freezer 禁用/音量静音等设备侧改写，**adb 链路保留**：WiFi/emu 设备每 60s 补 `adb connect` 保活，启动时自举扫描+连接、不建会话不启动应用）；镜像模式关物理屏（keyevent 223，**会话保留**），消费者回来即唤醒。消费者出现（ws viewer 注册 / 脚本 `run_begin` → `notify_activity`）打断空闲计时并即时唤醒已关的屏；镜像 30s 补醒也移入该循环（connect 时的保活任务只管拉满/恢复熄屏超时）。**`disconnect_device` 带运行守卫**：脚本运行中拒绝拆会话（虚拟屏销毁会杀掉屏上游戏），仅 force=true 绕过（删除设备 / 看门狗确认死链路 / 手动 `POST /api/devices/:id/disconnect` 管理动作）；前端"断开连接"按钮只断本页 WebRTC **不再调该接口**。下次运行脚本/定时任务自动重连（~2-4s）
- 模板/脚本存储（2026-08-21 起按应用分区，取代旧 `data/scripts/<package>/` + 全局 `data/templates/`）：**分区 = 设备配置的应用包名（pkg）**，目录 `data/<pkg>/tmpl/` + `data/<pkg>/yaml/`，无 default 兜底；脚本 id = `<pkg>/<name>.yaml`（**含 `/`，前端拼 URL 必须整体 encodeURIComponent**，axum 会对 `%2F` 解码；tasks.script_id 同格式零改动）。旧 `package <名字>` YAML 指令已删除（残留=解析报错）；模板/脚本 API 全部带 pkg 参数（模板 list 可省略=跨分区全列、条目带 pkg 字段，其余必填），`POST /api/scripts` body 加 `pkg`，导入 `POST /api/scripts/import?pkg=<分区>&confirm=1`（pkg 必填）；**导出 = 整分区全量快照**（`GET /api/scripts/export?pkg=<分区>` → `<pkg>.zip`，yaml+tmpl 全量）；zip = 分区快照（`yaml/<名>` + `tmpl/<模板>`，两目录均可缺省，不认旧布局）；引擎模板查找 = `data/<script_id 首段>/tmpl/`（跨分区不回退）；**模板支持短名引用**：脚本写 `login.png` 精确文件不存在时，引擎在同扩展名文件中唯一匹配 `login#*`（`engine::resolve_template_file`，多候选报错列名）；模板名 `#` 后缀 = 区域元数据（半区码或 ×1000 坐标，`engine::tpl_region_from_name`，与前端 parseTplRegion 同格式；**2026-08-26 起 region 步骤参数已删除，区域只由 #后缀 决定**，无后缀回退全屏=#a 语义并记运行日志提醒，`engine::region_for` 每运行每模板一条）；`call` 子脚本按名解析（优先同分区）；旧目录布局启动时一次性迁移（`scripts::migrate_fs_layout`）；Console 脚本页签顶部包名下拉统一切换模板+脚本区（下拉旁导入/导出按钮）；ScriptPicker 加 `package` prop 传入则隐藏自带分区下拉（Console 用，TaskScheduler 仍双下拉）；**模板上传（框选/上传）服务端统一重编码为 8-bit 灰度 PNG**（`matcher::reencode_template_gray_png`，Best 压缩+自适应滤波）：匹配只消费灰度故对匹配**零损失**、体积较 RGB PNG ↓60~75%，缩略图/预览变灰是有意取舍，旧彩色模板不迁移、匹配时照常转灰度
- **YAML 脚本引擎 2026-08-26 语法精简重写**（`engine.rs` 整体重写，不兼容旧语法；引擎+前端校验对旧写法显式报错引导迁移，hkrpg 存量 4 脚本同日已迁移，docs/YAML.md 同步重写）：顶层只允许 **config / func / steps**（未知顶层键报错，action_wait/log_level/name 残留在此拦截）；**steps 可缺省**（纯函数库脚本——只有 func、无 steps，供其他脚本「脚本名:函数名」跨文件调用；直接运行/被 call 只记一条日志不做动作；steps 与 func 都没有才报错）；**steps:/func: 段落键可省略**（2026-08-27，`engine::normalize_top` 归一化，run 与 exec_cross_func 两处共用：单段脚本直接写内容——顶层**序列**=steps、顶层**映射**且不含 config/func/steps 任何键=func 纯函数库简写（省略时 func 用映射形式，列表形式与步骤序列无法区分）；config 不能省（子键非函数名），interval/threshold 裸写顶层定向报错；省略写法的函数库同样可被跨文件调用）；**config:** 段（mapping 或映射列表按序覆盖）可重配置 `interval`/`threshold`/`log_level`，默认取 config.toml 同名键（`interval="500ms"` 带单位串、`threshold=0.85`、`log_level="info"`，旧 `default_threshold` 键删除）；**时间参数一律强制带单位**（`engine::parse_duration`：1ms/1s/1m/30min/1h/1d，m≡min 可小数，裸数字报错）；**interval 仅轮询类等待**（find 每轮重试 / verify 复查），步骤间不再统一等待（action_wait、步骤级 wait 参数、str_app 3s 特例全删）；log_level 四级过滤（debug/info/warn/error，success 视同 info，低于等级日志丢弃）；**find**（取代 until）：`- find: 主模板`（单字符串）+ `block`（障碍模板，取代 check：字符串/逗号/列表，与主模板重复报错）+ `verify`（默认 false；true=命中点击后等 interval 重匹配、仍命中补一击共两击，不循环）+ `timeout`（默认 30min 必须>0）+ then/else；每轮 = 主模板（新截图）命中→点中心→verify→then 结束；未命中→block 依序（命中点中心结束本轮）→全未命中等 interval 重开一轮；所有模板命中恒点中心；**color**（取代 cond，仅颜色条件）：`- color: [x, y]` + 兄弟键=6 位 hex 色值（宽容 #/0x/[r,g,b]，容差固定 30）挂命中步骤（可空）+ `else`；一次截图按序判定命中即结束，不轮询（重试套 loop）；模板条件已删（迁移方向：find 短 timeout + then/else 或 func 封装）；**loop**：times 省略/0=无限 + steps（必需；times/steps 两种缩进均认——映射值同级缩进会被 YAML 解析成兄弟键，干脆双支持）；**func 自定义函数**：`func:` 段定义（列表/映射均可，函数名不得保留字），调用 `- 函数名: 实参…`（**空格分隔+括号感知切分**（`engine::split_args`，[x, y] 内不切分，call 同规则））+ then(true)/else(false)；体内 `$N` 指函数实参（**func 段不参与脚本级 $N 替换**，`engine::take_funcs_and_substitute`），`return: true|false` 仅函数内合法、立即返回，**函数体正常走完未 return 默认返回 true**（2026-08-27 改，旧语义为 false）；函数可带 **cond 执行条件**（函数名级兄弟键：`cond: 模板` + `steps:` 包函数体——cond 后**不能**直接跟同列 `- ` 步骤行；也兼容 cond 写在函数体之后；多模板 = 每个模板各取一张新截图匹配一次（**不点击**）、全部命中才执行函数体，任一未命中函数返回 false），嵌套上限 32 层（return 经 `ctx.return_value` 冒泡，嵌套步骤入口短路）；**跨文件函数调用** `- 脚本名:函数名: 实参…`（脚本名与 call 同规则解析：同分区优先、缺扩展名自动补全；函数体/cond 取自被引用脚本 func 段、$N 由调用点实参替换；执行期间被引用脚本函数可见、调用者函数兜底、结束恢复；模板按被引用脚本分区解析——执行期临时换 `ctx.script_id`；无参调用带 then/else 必须写冒号 `- test1:fun2:`，否则是标量步骤）；**^N 上下文引用**（`engine::substitute_refs` 动态绑定栈）：find 的 then/else 内 ^1=主模板 ^2..=block、color 的命中步骤/else 内 ^1="[x,y]" ^2..=色值键；替换发生在每步执行时按最内层绑定（嵌套 find/color 内层覆盖外层），序列里映射项（子步骤）不预替换；**^ 是特意选的**——& 是 YAML 锚点保留字符（裸写值变 null）故弃用；**throw**（原 exit 改名）：`- throw`/`- throw: 原因`，ctx.exit 跨 call 结束整个任务（值宽容与引擎对齐：非字符串标量如 `- throw: 404` 按无原因处理，仅数组/映射报错）；**call** 传参 $N 规则不变（越界报错）；`Engine::run` 另有 `run_func` 参数（Some(函数名)=不跑顶层 steps 直接运行该函数体、start_step 定位体内、无实参 $N 不替换，Console「从某行运行」用：点**函数名行**=start_step 0 从头运行整函数**先检查 cond**（未命中不执行函数体，2026-08-27 改，旧语义恒跳过 cond）、点函数体内行=从该步跑跳过 cond）；str_app/cls_app 只支持裸写（带值报错，包名=分区）；tap/swipe/key/text/log/wait 不变（wait 支持 [1s,3s] 随机区间，swipe 的 from 别名删除）；一个步骤多个动作键报错；10 万步防死循环 guard 与 ScriptEvent(tap/swipe/hit) 可视化保留；**[op_templates] 新四键** find/tap/color/swipe（until/cond/swipe_region/wait 键删；swipe 的 time 模板带 ms 单位后缀），前端 DEFAULT_OP_TPL、四类记录生成器（模板缩略图→find、二次裁切取色→color、滑动不再生成 region 记录）、validateScriptCode 递归校验（config/func/then/else/loop 子步骤全下钻）同步重写
- **取色入口在二次裁切区**（`Console.vue cropPickColor`）：alt 模式点击裁切画布直接采样生成 color 颜色判断记录，模板在 `[op_templates] color` 键（默认 `- color: [{x}, {y}]\n  {color}:`，色值键挂命中步骤可留空）

## 关键文件

| 文件 | 职责 |
|---|---|
| `server/src/api/mod.rs` | REST：设备 CRUD / scan / connect / control / 截图 / 模板 / 脚本 / 任务 / 日志 |
| `server/src/api/ws.rs` | WebRTC 信令；取 `frame_cache.initial_frames()` 传给 ViewerSession |
| `server/src/webrtc.rs` | pusher 推流 / 初始 GOP 重放 / 静止补帧 / DataChannel 控制转发 |
| `server/src/device/scrcpy.rs` | scrcpy 会话：视频/音频/控制 socket 协议（v3.3.3） |
| `server/src/device/frames.rs` | 帧缓存：帧环（SPS/PPS + GOP）+ 按需解码截图 |
| `server/src/device/mod.rs` | DeviceManager：连接生命周期 / 状态 / 广播 |
| `server/src/store.rs` | SQLite 持久化（设备/任务/日志；scripts 表已随文件存储退役） |
| `server/src/scripts.rs` | 按应用分区的脚本/模板文件存储（分区寻址 / 依赖扫描 / zip 分区快照导入导出 / 旧目录迁移） |
| `server/src/engine.rs` / `matcher.rs` / `scheduler.rs` | YAML 脚本引擎 / NCC 模板匹配 / cron 调度 |
| `web/src/components/ScriptPicker.vue` | 脚本选择器：`package` prop 传入则锁定分区单下拉（Console），否则分区+脚本双下拉（TaskScheduler） |
| `web/src/views/Console.vue` | 投屏控制：WebRTC 前端（连接锁防双 PC / 坐标映射 / 框选模板）+ 设备列表管理（scan/连接/删除）+ 脚本区「从某行运行」：`computeRunLineMap` 按根段落扫描（steps 顶层项→`start_index`、func 函数名行→`func`+0（从头运行整函数，引擎先判 cond）、func 函数体顶层项→`func`+体内序号，经 run API 传引擎；省略 steps:/func: 的简写脚本同样可选中；再次点击选中行取消，非逻辑行点击不改选中） |

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

