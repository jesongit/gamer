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

- 连接：浏览器 → `POST /api/devices/:id/connect`（scrcpy 会话，已在线时幂等 no-op）→ WS `/ws/device/:id` 信令（offer/answer；offer 可带 `force:true` 顶替已有 viewer，见"多页面互斥"）→ WebRTC 视频轨 + DataChannel `control`（触控/按键/文本）；脚本运行时引擎的 tap/swipe/匹配命中可视化事件也经该 DataChannel **反向**推送给投屏页面（`{"type":"se","ev":...}`，engine `emit` → viewers 注册表 `control_dc`，定时任务运行同样生效）
- 帧缓存 `FrameCache`（帧环 + 按需解码）：截图/模板匹配时用临时 ffmpeg 解**最新一帧**（天然实时，无陈旧/停滞问题），并为**新 viewer 重放初始帧（SPS/PPS + 最近 GOP）**。ffmpeg 不可用 → 无初始帧 → 浏览器黑屏（见已知坑）
- 单设备单 viewer：新连接踢旧连接（`AppState.viewers` 注册表）
- 视频静默看门狗：静默 ≥20s 时——无 viewer 且无脚本 → 直接断开进低功耗（虚拟屏无应用=黑屏 0 帧，编码器完全空闲，重连只会 churn）；有 viewer 或脚本运行中 → 先 reset_video 请求关键帧探测（黑屏虚拟屏编码器活着会立即出 IDR），探测后 15s 仍静默才断开重连并踢 viewer
- 多页面互斥（服务端仲裁，2026-08-20 重做，取代旧 localStorage 锁 `gb_webrtc_lock`——锁只能管同一浏览器，跨浏览器/跨 PC 管不到）：新页面 offer（不带 force）遇已有活跃 viewer → 服务端回 `{"type":"conflict"}`（不踢不建连）；前端**手动连接**弹窗确认后带 `force:true` 重发 offer 才接管，**自动重连**遇 conflict 直接放弃并提示。接管只换浏览器↔服务端链路：先经旧页面的信令 ws 推 `{"type":"taken_over"}`（`ViewerHandle.notify` 通道 + ws 循环 peer 关闭后 200ms 冲刷窗口保证送达）再关旧 peer，**设备 scrcpy 会话不动**（实测 ~0.3s 无缝切换）；被顶页面收到 taken_over 后断开且不再自动重连（防互顶死循环）
- 配置变更（PUT /api/devices/:id）会踢该设备 viewer（pusher 停 + peer close），前端 onclose → 自动重连恢复画面
- 设备扫描：`POST /api/devices/scan` 执行 `adb devices -l`，按 addr 去重自动入库（逻辑在 `DeviceManager::scan_and_sync`，服务器启动时也自动跑一次）
- App 生命周期 / 低功耗空闲：连接**不再自动启动应用**（由脚本 `str_app`（冷启动，"+" 前缀控制消息）或 Console 启动按钮显式触发）；脚本结束后 `idle_disconnect_secs`（默认 60，0=关）秒内该设备无运行脚本且无 viewer → 自动 `disconnect_device` 进低功耗（熄屏恢复/编码停止/虚拟屏销毁，**adb 链路保留**：WiFi/emu 设备每 60s 补 `adb connect` 保活，启动时自举扫描+连接、不建会话不启动应用）；下次运行脚本/定时任务自动重连（~2-4s）
- 模板/脚本存储（2026-08-21 起按应用分区，取代旧 `data/scripts/<package>/` + 全局 `data/templates/`）：**分区 = 设备配置的应用包名（pkg）**，目录 `data/<pkg>/tmpl/` + `data/<pkg>/yaml/`，无 default 兜底；脚本 id = `<pkg>/<name>.yaml`（**含 `/`，前端拼 URL 必须整体 encodeURIComponent**，axum 会对 `%2F` 解码；tasks.script_id 同格式零改动）。**旧 `package <名字>` YAML 指令已彻底删除**（引擎直接解析 YAML，残留指令行=解析报错）；模板/脚本 API 全部带 pkg 参数（模板 list 可省略=跨分区全列、条目带 pkg 字段，其余必填），`POST /api/scripts` body 加 `pkg`，导入 `POST /api/scripts/import?pkg=<分区>&confirm=1`（pkg 必填）；zip = 分区快照（`yaml/<名>` + `tmpl/<模板>`，两目录均可缺省，不认旧布局）；引擎模板查找 = `data/<script_id 首段>/tmpl/`（script_id 首段即分区，跨分区不回退，导出时才跨分区收集）；`call` 子脚本按名解析（优先同分区）；旧目录布局启动时一次性迁移到 DB 首个配置了 pkg 的设备分区（`scripts::migrate_fs_layout`，目标分区已有数据则跳过）；Console 脚本页签顶部包名下拉（默认/自动跟随设备页签 pkg）统一切换模板+脚本区；ScriptPicker 加 `package` prop 传入则隐藏自带分区下拉（Console 用，TaskScheduler 仍双下拉）

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
| `web/src/views/DeviceList.vue` | 设备列表：刷新(scan) / 连接 / 删除 |
| `web/src/views/Console.vue` | 投屏控制：WebRTC 前端（连接锁防双 PC / 坐标映射 / 框选模板） |

## 规则

- 开发/运行中踩到的坑（环境、构建、部署、已知限制）必须记入下方「已知坑」；
  每条保持**精简准确**：一句话现象 + 原因 + 解决/规避，不写流水账、不夸大

## 已知坑

- scrcpy 视频 socket 必须保留整个 `TcpStream`（`into_split()` 写半 drop 会发 FIN 导致断流）
- `max_fps=0` / `max_size=0` 等 0 值参数不能传给 scrcpy server
- `config.toml` 的 `ffmpeg_path` 失效 → 帧缓存启动失败 → 新 viewer 无 SPS/PPS 可重放 → **连接后黑屏**（日志特征：`frame cache unavailable`，且无 `pusher replayed initial GOP`）
- 控制 DataChannel 必须由 offerer（浏览器）创建；webrtc-rs answer 只镜像 offer 的 media section
- 模板匹配把截图/模板等比缩放到最长边 540px 加速，命中坐标需映射回原图
- 设备配置（PUT /api/devices/:id）会触发断开重连；删除设备不停止 adb 物理连接，scan 会重新入库（**服务器启动时也自动 scan**，删掉的设备会被重新加回）
- 多个浏览器页面同时操作同一设备：已改服务端协商式接管（conflict 确认 + force 顶替 + taken_over 不重连，见关键链路）；旧 localStorage 锁方案已删除。调试时注意 chrome-devtools-mcp 的 `select_page` 的 `pageId` 必须是 number 且选中状态不跨 MCP 连接保留
- `gamer.ps1 restart` 只停/启现有二进制**不重新编译**：Rust 代码或 config.toml 改动后须 `rebuild`（或 `restart -Build`），否则跑旧 exe、新接口表现为 HTTP 405
- `gamer.ps1` 必须保持 UTF-8 **BOM + CRLF** 编码：被无 BOM/LF 重写后 Windows PowerShell 5.1 解析报错（报错行号与实际不符）；**编辑器/工具改写后 BOM 会悄悄丢失**，改完必须检查并修复：`[IO.File]::WriteAllText($p, ([IO.File]::ReadAllText($p) -replace "`r?`n","`r`n"), (New-Object Text.UTF8Encoding $true))`（修复脚本存于 `.zcode/fix-encoding.ps1`）
- PowerShell（5.1 与 7.3+）在 `$ErrorActionPreference='Stop'` 下把 cargo/npm 的 stderr 输出当错误中断脚本；gamer.ps1 的 `Invoke-NativeChecked` 构建期间临时切回 Continue，成败只看 `$LASTEXITCODE`（注意：`$PSNativeCommandUseErrorActionPreference` 是 7.3+ 变量，5.1 无效）
- 虚拟屏实际分辨率/方向会被游戏改变（如 1920x1080 ↔ 1440x3200），**设备配置里的 width/height 会过期**：前端换算测试区域像素必须优先用实际视频尺寸（`videoElement.videoWidth/Height`），相对坐标（模板名 #x1_y1_x2_y2、region fm/to）本身与分辨率无关
- 帧缓存 = **帧环 + 按需解码**（`frames.rs`）：只缓存 SPS/PPS + 最近 GOP（feed 时纯内存拷贝），截图/匹配时用**临时 ffmpeg** 把最新一帧解码成 PNG（`-vf select=gte(n\,N)` 取 GOP 最后一帧，`-frames:v 1` 只出一张图）——每次全新解码天然实时，**不要做常驻解码 PNG 流**（旧设计：常驻 ffmpeg 软解会静默冻结，截图/匹配永远拿到旧画面，还得加代数/新鲜度/健康检查兜底）。select 的帧索引含 demuxer 对配置帧的计数偏移（±1~2 帧 ≈ ≤100ms 旧，可接受）；解码失败=分辨率切换窗口（新 SPS 已到、新 IDR 未到，config 与旧 GOP 不匹配）→ 清空 GOP 等下一个 IDR 重试一次
- 虚拟屏设备截图**不可静默回退物理屏**（内容/分辨率不同，模板匹配会拿到主屏竖屏数据）：帧缓存按需解码 → adb 虚拟屏 screencap → 直接报错；注意本机 Xiaomi 15 Pro 的 `adb screencap -d` 对 scrcpy 虚拟屏返回非法图（~80B），截图只能靠帧缓存解码
- 挂机（静止画面）时投屏延迟单调累积到秒级（实测 87ms → 3s+ 且不回落）→ 三个叠加根因：① **音频轨参与浏览器 A/V 同步（主时钟），scrcpy 虚拟屏音频流在 Chrome 侧播放时钟异常**（对照实验：`audioTrack.enabled=false` 后停止累积）；② **帧级 burst**——设备 60fps 固定编码（虚拟屏 `max_fps` 不生效）、USB/WiFi 批量到达，pusher 批量全速连发，浏览器 jitter buffer 目标延迟被顶高；③ **关键帧 burst**——`i-frame-interval` 越短扰动越频繁（1s 时实测 perF 缓慢爬升）。服务端 RTP ts 精确 1.0x、网络零丢包、硬解正常。修复：前端静音时禁用音频轨（`toggleAudio`/`ontrack`）+ 延迟看门狗（>1500ms 自动重连）+ 服务端统一帧级 pacer（16ms 固定节奏，`webrtc.rs`）+ 关键帧发送平滑（中间一次 8ms 分批发，总耗时须 < pacer 间隔，否则每秒净积压）+ `i-frame-interval=2`；修复后挂机延迟稳定 140~250ms（残余 = 无线传输 + Chrome jitter buffer 保守目标 ~100-150ms）。**后续（2026-08-20）**：仅 `track.enabled=false` 不够——非 Chrome 内核（实测 ZCode IAB webview）静音轨仍被选为 A/V 主时钟，虚拟屏音频时钟 ~1% 慢漂 → 延迟 +12ms/s 单调累积到看门狗 1.5s 阈值 → 重连清零 → 再累积循环；根治：音频改**按需发送**（control 消息 `{"type":"audio","on":bool}`，前端 `onChannelOpen` 上报当前静音态，服务端 `audio_on` 默认 false 零音频包——任何内核都无从拿音频做主时钟），60fps 游戏画面实测延迟稳定 15~20ms
- **游戏 60fps 高码率时"延迟高、每几秒卡一下"三因叠加**（2026-08-20）：① `bitrate_mbps=40` 时单帧 50~75KB，pusher 单帧 RTP 发送 18~23ms > 16.7ms 帧预算 → 发送饱和慢性积压（每秒净欠 ~10 帧）；② `backlog_limit` 按配置 fps(15) 换算 = 15 帧，而实际流 60fps → 阈值仅 250ms（注释本意 ~1s）；③ 积压清队时队内常无关键帧（IDR 间隔 2s = 120 帧 > 队深 15~35 帧）→ `waiting_key` 干等自然 IDR，画面冻结 0~2s（平均 ~1s），每 4~5s 一次。修复（`webrtc.rs`）：码率降到 12（单帧 ~25KB、send_avg ~5ms，无积压）；`backlog_limit` 下限 60 帧（60fps 下恢复 ~1s 本意）；断链清队时主动 `reset_video` 要 IDR（冻结缩到 ~200ms，限频 2s，环形溢出同路径）；另 ffmpeg 块效应探针（每关键帧 + 1/30 P 帧起阻塞子进程，~2.5 个/s 抢 tokio worker）实测把 send_avg 推高 3~4 倍，已加 `probe_encoder` 开关默认关
- **静态屏（无应用/挂机静止）"连上一会儿就断、一直连不上"的死循环**（2026-08-20，三层叠加）：虚拟屏无内容时编码器 0 帧（正常），断链点在 ①**Chrome 静默丢弃静止补帧的重复 P 帧**——相同 frame_num 的重复 slice 被当冗余副本不解码，`currentTime` 冻结（画面定格本是正确渲染），但前端静默检测只看 currentTime → ~4s 杀连接 → 重连 → 循环（实证：补帧 30fps 严格 1.0x 发出且 write_rtp 有字节，`webkitDecodedFrameCount` 卡死不动）；②**Chrome 入流时例行发 PLI** 被前端当"解码失步"→ reset_video，而 MTK 静态屏对 reset 响应极慢（实测要多次 reset、最长 6s+ 才吐 config+IDR，甚至不吐）→ 补帧被 pending_config 压制 → 浏览器断供加速死亡；③服务端静默看门狗（20s 静默+15s 宽限）在补帧正常投喂时仍按"设备 0 帧"整会话重连踢 viewer（35s 周期）。修复：前端静默检测改**双条件**（currentTime 冻结 && 统计窗口零新增字节，Console.vue `videoBytesAdvanced`）；入流 6s 内 PLI 不触发 reset；pusher 补帧压制（pending_config/waiting_key）限时 3s 自动恢复（旧帧+旧参数集自洽可解码，安全）；`ViewerHandle.last_serve`（pusher 每次发送刷新）让看门狗识别"viewer 正被补帧投喂=会话健康"跳过 nudge/重连。**补帧保持 P 帧形态不要改成重复 IDR**：唤醒后新 P 帧直接续参考链无花屏（IDR 重复会清 DPB，唤醒首个 P 帧必花屏要靠 PLI 兜底）；实测静态屏 96s+ 稳定、点击唤醒后 29fps 干净恢复、ct 1.0x 追平墙钟
- 设备连接方式变化（USB ↔ Android 11+ 无线调试）后 `adb devices` 显示名与配置 serial 失配（`adb -s` 直接 not found）→ 服务端 `resolve_serial`（精确/子串/model 匹配，`adb.rs`）在 `connect_device` 时解析并写回运行时 device.addr；`api_scan_devices` 去重同步更新 addr/kind，避免重复入库。**判断当前传输方式**：`adb devices -l` 显示 `IP:port`=无线、`serial`=USB、`adb-<serial>-..._tcp`=mDNS 无线；无线传输延迟/波动明显高于 USB 直连（插回 USB 线延迟更低，serial 精确匹配自动生效）
- 设备掉到 `offline`（MIUI/USB 偶发，`adb devices` 只认 `device` 状态）时连接报 "cannot resolve host" 是**假错误**：旧代码对非空 addr 一律 `adb connect`，USB serial 被当主机名解析。已修（scrcpy.rs）：仅 `IP:port` 走 adb connect，USB/mDNS 先 `adb reconnect offline` 自救一次，仍不在则明确报"设备不在线"；救不回只能物理拔插/重开 USB 调试。`adb kill-server` 后传输重置，设备也可能从 offline 变成彻底消失（需重新枚举）
- 用 PowerShell `Invoke-WebRequest` 计时大响应接口（MB 级 PNG）会虚高到秒级（客户端缓冲开销），且 PS 5.1 对部分二进制/无 charset 响应直接抛 `NullReferenceException`（非服务端问题）——验证 HTTP 一律用 `curl.exe`
- 点击投屏画面后画面**慢慢浮现黑白/彩色块点并卡顿**（非必现）→ 游戏切分辨率/编码器重启时 scrcpy 发新 SPS/PPS（config 帧）：旧实现 config 一到就喂给 H264Payloader（它不分关键帧，缓存了参数集后**下一个 NALU 必合成 STAP-A**），config 与新 IDR 之间（编码器重启窗口 50~500ms）静止补帧把"新参数集 + 旧分辨率帧"发出去 → 浏览器解码器失步花屏直到下个 IDR；backlog 跳帧的 drain 丢掉 config 帧同理（IDR 前重发旧参数集）。修复（webrtc.rs）：config 帧在取帧阶段提取、只更新 config_nalu（永不丢）、新 IDR 到达前禁止静止补帧、IDR 时才喂参数集合成 STAP-A；frames.rs：config 字节变化即清空 GOP（避免初始重放/按需解码跨参数混喂）
- **花屏自愈链路（PLI 兜底）**：webrtc-rs 默认 interceptor 只有 NACK（responder/generator），**不响应 RTCP PLI**——浏览器解码器失步（丢包/解码错误）发的关键帧请求被静默丢弃，花屏只能等设备固定 IDR（i-frame-interval=2s）。修复：前端 stats 轮询（1s）检测 `inbound-rtp.pliCount` 增量（= 浏览器已请求关键帧 = 解码器失步）→ 经 control DataChannel 发 `{"type":"reset_video"}`（限频 2s）→ 服务端 `handle_control_msg` 调 `session.reset_video()`（scrcpy 控制消息 17，编码器立即输出新 config+IDR）→ ~200ms 自愈。另：ICE 抖动期间跳过 IDR 后须置 `waiting_key`（参考链已断，恢复后丢到下一个 IDR 的 P 帧）。**两个防误伤**（2026-08-20）：连接初期 ~6s 的 PLI 是 Chrome 入流的例行关键帧请求，不触发 reset（静态屏 reset 会打断补帧引发断连死循环）；reset 后 pliCount 仍在涨 = reset 无效（黑屏/静态屏编码器不吐 IDR），指数退避 2s→15s→60s（`pliResetStreak`，一个统计周期无新 PLI 即复位），避免每 3s 重启一次编码器空转
- **点击投屏画面后花屏的隐蔽根因（重放链断裂）**：MTK 编码器实测**忽略 `i-frame-interval=2`**，关键帧实际间隔 ~20-25s（日志特征：config 帧 ~25s 一条而非 2s）→ 帧缓存 GOP 旧上限（400 帧/8MB）在 IDR 后 ~3s 就被字节上限清空 → 新 viewer 连接时帧缓存无完整 GOP → ws.rs reset_video 兜底轮询 3s 拿不到 IDR 时，initial_frames 只有 SPS/PPS → pusher 重放后**裸推实时 P 帧**（解码器有参数集无参考帧）→ 花屏直到 25s 后自然 IDR，表现为"点击后慢慢浮现块点、卡住、非必现"（触发条件=该时刻发生重连/重放）。修复（frames.rs/webrtc.rs）：GOP 上限扩到 800 帧/64MB（覆盖一个完整 IDR 周期）；重放后无 IDR → `waiting_key=true`（丢 P 帧等 IDR，期间禁静止补帧，浏览器保持定格而非花屏）；重放节流 clamp(16,40)→clamp(2,10)ms（大 GOP 重放 ≤~6s，避免连接后长时间停在旧画面）
- **花屏"偶发"的真凶（参数集丢失 + 重放 0 字节，探针盲区）**：① **STAP-A 超限静默丢弃参数集**——H264Payloader（rtp-0.13.0）的 STAP-A 只在 `stap_a_nalu.len() <= mtu(1200B)` 时发送，超限时**静默丢弃整个 STAP-A（含 SPS/PPS）并清空缓存**；IDR slice 实测 85~92KB（MTK 单 slice），首个 NALU 必超限 → 参数集能否到达浏览器完全取决于 IDR 帧是否恰好以小 SEI 开头（概率性）→ 切分辨率/编码器重启（点 logout 触发 scrcpy `Video capture reset` + 新 config 帧）后浏览器用旧参数集解新流 → 花屏直到下一个"侥幸带小 SEI"的 IDR（~25s，非必现）。`verify_rtp_rebuild` 探针过滤 type 7/8，查不出参数集丢失（盲区，MISMATCH=0 不说明参数集到达）。修复（webrtc.rs）：`send_config_nalus`——IDR 前把 SPS/PPS 拆成**独立单 NALU RTP 包**发送（RFC 6184 允许 type 7/8 单包；同 ts + marker=false 与 IDR 合成一帧交付 FFmpeg），参数集必定到达，日志特征 `config SPS/PPS sent as single NALUs`；② **重放整体 0 字节**——webrtc-rs 在 SRTP 实际未就绪时 write_rtp 静默返回 Ok(0)（实证 connected+300ms 后重放 109 帧 4.3MB 全丢，浏览器一帧收不到且 waiting_key 未置位 → P 帧裸推花屏）。修复：重放统计 written，全 0 → 200ms 重试（≤3 次）→ 仍失败则 `session.reset_video()` + `waiting_key=true`（黑屏而非花屏）；日志特征 `initial GOP replay wrote 0 bytes ... retrying` / `replay failed (0 bytes after retries)`
- 脚本可视化事件协议（引擎 → 浏览器，control DataChannel 反向推送）：`{"type":"se","ev":"tap"|"swipe"|"hit",...}`——serde 内部标签枚举（`tag="ev"`）默认序列化**变体名原样**（"Tap"），前端按小写匹配会全部忽略，必须加 `rename_all="snake_case"`（engine.rs `ScriptEvent`，已注释标注）
- 启动应用必须走 scrcpy `TYPE_START_APP` 控制消息（虚拟屏模式下自动启动到虚拟屏；name 支持 `+` 前缀先 force-stop、`?` 前缀按应用名搜索），**不要用 adb `am start`/monkey**——会落到物理主屏，模板匹配全错；`cls_app`（adb `am force-stop`）不碰会话，但虚拟屏上应用被杀后画面变桌面/黑屏、流不断，属预期
- **Windows「USB 选择性暂停」+ 接触不良的 USB 口 = adb 掉线两大真凶**（曾误判为 HyperOS 熄屏杀 adb，勿再走弯路）：①**空闲 15~25s 必死** → 选择性暂停把设备挂起（修：`powercfg /SETACVALUEINDEX SCHEME_CURRENT 2a737441-1930-4402-8d77-b2bebba308a3 48e6b7a6-50f5-4782-a5d4-53bb8f07e226 0` + `SETDCVALUEINDEX` 同值 + `SETACTIVE SCHEME_CURRENT`；机器级设置，换机/重装会丢需重做）；②**大流量突发死**（push 90KB 传完读响应 EOF，纯 adb 手动 push 即可复现，gamer 无关）→ 物理口/线接触不良（实测换口后 1KB~100KB push/pull 全稳）。掉线后手机 adbd 常楔死 offline：`adb reconnect offline` 救不回、`adb kill-server` 只能枚举回 offline，**只能拔插**。诊断技巧：后台 `adb logcat` 抓掉线瞬间（`UsbFfs: connection terminated` / `MiuiSwapService: Usb disconnect`），手机侧比 Windows 侧多活 2.5 分钟说明是主机侧先断
- 主屏保活仅限**镜像会话**（拉满熄屏超时 + 30s WAKEUP + dismiss-keyguard，断开恢复）；**虚拟屏会话跳过**（编码不依赖物理屏管线，熄屏照常出帧，挂机时主屏保持熄屏省电）——熄屏不影响 adb 的前提是上面两条 USB 真凶已排除
- 服务运行中执行 `cargo build` 会因 exe 被占用失败（os error 5 拒绝访问），改代码后须 `gamer.ps1 stop` → `cargo build` → `start`（`rebuild` 已封装此顺序）；`findstr` 搜 minified 前端 bundle（100KB+ 单行）会因 8KB 行长截断假阴性，用 PowerShell `[IO.File]::ReadAllText().Contains()` 验证
