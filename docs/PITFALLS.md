# 已知坑

Gamer 开发/运行中踩过的坑记录（环境、构建、部署、已知限制）；由 AGENTS.md 规则约束维护，
每条保持**精简准确**：一句话现象 + 原因 + 解决/规避，不写流水账、不夸大。新条目追加到本文件末尾。

- scrcpy 视频 socket 必须保留整个 `TcpStream`（`into_split()` 写半 drop 会发 FIN 导致断流）
- `max_fps=0` / `max_size=0` 等 0 值参数不能传给 scrcpy server
- `config.toml` 的 `ffmpeg_path` 失效 → 帧缓存启动失败 → 新 viewer 无 SPS/PPS 可重放 → **连接后黑屏**（日志特征：`frame cache unavailable`，且无 `pusher replayed initial GOP`）
- 控制 DataChannel 必须由 offerer（浏览器）创建；webrtc-rs answer 只镜像 offer 的 media section
- 模板匹配把截图/模板等比缩放到最长边 540px 加速，命中坐标需映射回原图
- 设备配置（PUT /api/devices/:id）空闲时触发断开重连（脚本运行中不拆不踢，下次连接生效）；删除设备不停止 adb 物理连接，scan 会重新入库（**服务器启动时也自动 scan**，删掉的设备会被重新加回）
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
- 脚本可视化事件协议（引擎 → 浏览器，control DataChannel 反向推送）：`{"type":"se","ev":"tap"|"swipe"|"hit"|"miss",...}`——serde 内部标签枚举（`tag="ev"`）默认序列化**变体名原样**（"Tap"），前端按小写匹配会全部忽略，必须加 `rename_all="snake_case"`（engine.rs `ScriptEvent`，已注释标注；`miss`=匹配未命中的搜索区域框，2026-08-27 新增）
- **投屏"花屏后永久卡死"= Chrome jitter buffer 目标延迟膨胀的连接级中毒（2026-08-23 定位）**：长时间静止补帧（30fps 重复 P 帧）+ 运动突发后，浏览器 jbuf 目标延迟单调膨胀（实测静止 23min 后 676ms → 滚动突发后 4.9s），表现为**包在到、framesDecoded 在涨、画面却逐位冻结/残缺花屏**——此时 currentTime 照常 1.0x 推进、bytesReceived 照常增长、pliCount 不动（静默参考链损坏不触发 PLI/NACK，协议层无丢包），静默/延迟/PLI 三个现有看门狗**全部失明**，用户只能手动刷新；重连（重建 jbuf）是唯一解药（实测同场景重连后恢复渲染）。修复三件套：① 服务端静止补帧降频 33ms→500ms（webrtc.rs `idle_repeat_ms`，2fps 心搏只维持链路活性——前端静默看门狗要 bytes 增长、api 看门狗要 last_serve——新帧到达经 notify 即时唤醒无延迟代价）；② 前端新增**画面停滞看门狗**（Console.vue，24x14 亮度哈希指纹）：拖动/滚轮后 ~5s 渲染指纹逐位未变 → 先 `reset_video`，再 5s 仍冻结 → 自动重连重建 jbuf（在"拖不动/本就静止"的界面上会多一次 reset+重连，代价可接受）；③ webrtc.rs ICE 断连跳帧改为**任何被跳帧都置 `waiting_key`**（旧代码只看关键帧；P 帧被跳过后参考链断裂且无任何协议信号）。诊断技巧：`inbound-rtp.jitterBufferTargetDelay` 达秒级 = 中毒铁证；`keyFramesDecoded` 长期不增而服务端 IDR 在发 = jbuf 吃帧
- 启动应用必须走 scrcpy `TYPE_START_APP` 控制消息（虚拟屏模式下自动启动到虚拟屏；name 支持 `+` 前缀先 force-stop、`?` 前缀按应用名搜索），**不要用 adb `am start`/monkey**——会落到物理主屏，模板匹配全错；`cls_app`（adb `am force-stop`）不碰会话，但虚拟屏上应用被杀后画面变桌面/黑屏、流不断，属预期
- **启动应用"特别慢"（2026-08-24 实证，单次点击 34s 首帧 vs 真冷启动基线 12~17s）= 僵尸游戏进程 + MIUI Greeze 冻结陷阱**：空闲拆会话**只杀 activity 不杀进程**（旧说法"虚拟屏销毁会杀掉游戏"不准），僵尸滞留后台被 Greeze 冻结；重连建新虚拟屏时桌面小组件（hkrpg `StarRailRecordWidgetProvider`）+ PushService 又把同 uid 空壳进程拉起 → 1s 内再被冻结（`FZ reason=tobg`）；点启动时 AMS 见"进程已存在"走 warm start（无冷启动 boost），**启动 1s 后又被冻结**（`FZ reason=check binder`——activity 未画出首帧 + 主屏熄屏被按后台判），冻 22s 被信号解冻后才继续初始化（logcat 时间线：THAW for Activity Start → 1s 后 FZ → 22s 后 THAW Signal other3 → +10s 首帧）。**关键结论：`cached_apps_freezer_enabled=0` 管不住 HyperOS/Android 16 的 Greeze**（连接时已写入 0 照冻不误，只对已是 TOP 的前台应用有效——8/24 验证的"真睡眠前台存活"仍成立，但**启动窗口期/后台空壳照冻**）。**冻结也打真冷启动**（2026-08-24 晚实证）：activity 切换瞬间（启动后 ~2s）进程瞬时非前台 → `FZ tobg` → 黑屏卡死（+176ms 已出 splash 也白搭），当晚 6 次启动中 1 次；**唤醒主屏不解冻**（THAW 只由 Activity Start / binder / 信号触发）。**已修（三层，2026-08-24）**：① `disconnect_device` 虚拟屏拆会话时 `am force-stop <pkg>` 清僵尸+释放内存（镜像模式不杀）；② `ScrcpySession::start_app` 无前缀纯包名先探进程状态——pidof + **utime 探针**（`/proc/<pid>/stat` 1 秒 utime+stime 零增长=冻结；cgroup.freeze 文件 shell 读不到，帧空闲不可靠因挂机静止画面也无帧）：冻结→升级 `+` 强启，存活未冻结→裸 start（前台切换不重启，挂机中重点击无副作用）；③ **启动冻结自愈看门狗**（`watch_launch_freeze`，虚拟屏模式每次 start_app spawn）：+6s/+14s 探测冻结→裸 start 捅醒（Activity Start 强制 THAW，实测 22:23 案例解冻后 10s 完成启动），两捅无效告警（用户重点击即走 ② 的 "+" 兜底）。注意 MIUI joyose 会在 force-stop ~1.3s 后以 `SRGameStateService` 复活游戏主进程（杀不干净），空壳必然再现——② 的探测是最终保证。诊断技巧：`logcat` 搜 `GreezeManager` 的 FZ/THAW（uid 为游戏 uid）量化冻结时长；`Start proc ... for next-top-activity`=真冷启动（好），`for broadcast/for service`=空壳 warm start（会踩陷阱）；冻结判定用 utime 探针（`cat /proc/<pid>/stat` 取最后 ')' 后第 12/13 个数，1 秒两次比差）
- **Windows「USB 选择性暂停」+ 接触不良的 USB 口 = adb 掉线两大真凶**（曾误判为 HyperOS 熄屏杀 adb，勿再走弯路）：①**空闲 15~25s 必死** → 选择性暂停把设备挂起（修：`powercfg /SETACVALUEINDEX SCHEME_CURRENT 2a737441-1930-4402-8d77-b2bebba308a3 48e6b7a6-50f5-4782-a5d4-53bb8f07e226 0` + `SETDCVALUEINDEX` 同值 + `SETACTIVE SCHEME_CURRENT`；机器级设置，换机/重装会丢需重做）；②**大流量突发死**（push 90KB 传完读响应 EOF，纯 adb 手动 push 即可复现，gamer 无关）→ 物理口/线接触不良（实测换口后 1KB~100KB push/pull 全稳）。掉线后手机 adbd 常楔死 offline：`adb reconnect offline` 救不回、`adb kill-server` 只能枚举回 offline，**只能拔插**。诊断技巧：后台 `adb logcat` 抓掉线瞬间（`UsbFfs: connection terminated` / `MiuiSwapService: Usb disconnect`），手机侧比 Windows 侧多活 2.5 分钟说明是主机侧先断
- 主屏保活仅限**镜像会话**（连接时拉满熄屏超时 + WAKEUP + dismiss-keyguard，断开恢复原值；30s 补醒与空闲关屏 keyevent 223 由 `idle_power_loop` 按"有无消费者"管理，2026-08-22 移入）；**虚拟屏会话不动主屏**（2026-08-24 改）：防冻结改为连接时禁用 cached apps freezer（`settings put global cached_apps_freezer_enabled=0`，`mod.rs apply_virtual_overrides`）+ 媒体音量置 0（`cmd audio set-volume 3 0`，防虚拟屏声音外放，见"HyperOS 音频外放"坑），断开/停机恢复原值，**改写前先落盘 `data/pending_restore.json`、服务启动时残留自愈**（防硬杀后设备卡在改写态）——旧方案（软关屏 `set_display_power` + `stay_awake` + 每 10s 唤醒自愈）已整体移除，其代价是主屏永远点不亮、用户无法正常使用手机；熄屏不影响 adb 的前提是上面两条 USB 真凶已排除
- 服务运行中执行 `cargo build` 会因 exe 被占用失败（os error 5 拒绝访问），改代码后须 `gamer.ps1 stop` → `cargo build` → `start`（`rebuild` 已封装此顺序）；`findstr` 搜 minified 前端 bundle（100KB+ 单行）会因 8KB 行长截断假阴性，用 PowerShell `[IO.File]::ReadAllText().Contains()` 验证
- **虚拟屏应用被系统冻结 = "画面永久定格、输入全无效"的真凶（2026-08-23 定位，2026-08-24 根治）**：Android 15+ 主屏真睡眠（power 键/系统超时）时，副屏（虚拟屏）应用在 ~10-30s 无交互后被系统 cached apps freezer **整体冻结**（ANR trace 全线程 `do_freezer_trap`；输入注入超时/立即失败、零渲染、音频只剩静音包，进程状态仍 TOP、无 MediaCodec 报错——极易误判为"游戏冻死/编码器楔死"）。scrcpy 官方 issue [#5604](https://github.com/Genymobile/scrcpy/issues/5604)；**唤醒主屏即可解冻**（实测）。根治（2026-08-24）：虚拟屏会话期间禁用 freezer（见上条），实测主屏真睡眠 3min+ 虚拟屏渲染存活（时钟秒持续跳变）、睡眠中注入 tap 即时生效、WAKEUP 正常唤醒——主屏想睡就睡、按电源键即亮。旧方案（软关屏+stay_awake+周期唤醒自愈，验证过但主屏永远点不亮）已删除；若冻结再现（如游戏被 MIUI 省电策略单独冻结），先查该游戏"省电策略=无限制"。滚动中的"画面混乱"为冻结前瞬间的撕裂帧，冻结解除后不再出现可再观察
- **冻结/静止的判别与恢复（排障速查）**：注入 2~3 次 swipe/back 后 5s 内零新帧且截图哈希不变 → 应用死（冻结或真死）；查 `/data/anr/temp_anr_*`（`do_freezer_trap` = 冻结，需 root/bugreport 读）+ `dumpsys power | grep mWakefulness`。冻结恢复 = 唤醒主屏（freezer 已随会话禁用，正常路径不会再冻）；应用真死恢复 = `start_app` 重启（编码器/会话没死时**不要拆会话**——虚拟屏销毁会杀游戏，实测拆会话重建还有 ~2-4s 的旧进程收尾日志交错易误判，见下条）
- **静态页 + 熄屏必冻，唤醒后 ~20s 又冻（2026-08-25 实证）**：游戏挂在登录 SDK 页（SdkActivity/ConfirmActivity 静态 View，非持续渲染）时主屏一熄屏 Greeze 立即 `FZ reason=screen off` 整冻——`cached_apps_freezer_enabled=0` **管不住这条**（该设置只保"持续渲染的前台应用"，8/24 的"睡眠 3min+ 存活"测试是渲染中的游戏，静态登录页不在此列）；特征 = 视频 0 帧而音频照流（会话没死，别拆会话）+ viewer 点击打出 ANR（`Input dispatching timed out ... MotionEvent DOWN`；ANR 抓 trace 的信号造成一次假 THAW，随即 `FZ from system` 回冻，logcat 时间线易误读成"解冻过为什么还卡"）。恢复 = 唤醒主屏（keyevent 224）或裸 start 捅（Activity Start 强制 THAW、可不亮屏，实测能撑 ~1 分钟再被 tobg 冻）；进游戏持续渲染后熄屏即安全。**根治（2026-08-25 实证）= Greeze 豁免名单 `MILLET_NO_RESTRICT_APP`**：小米"省电策略=无限制"的底层实现（shell 可写免 root：`settings --user 0 put system MILLET_NO_RESTRICT_APP <逗号分隔包名>`，Greezer 冻结前检查的 no-restrict 集）；实测写入游戏包名后熄屏+静态登录页 2.5min+ 零 FZ、进程持续活跃（对照：写入前每次熄屏秒冻）；Doze 白名单（`dumpsys deviceidle whitelist`）、battery optimization 等 AOSP 层手段对 Greeze **全部无效**（调研见 dingwen07/hyperos-fcm-fix 的 greezer investigation）。注意：① PowerKeeper 会从私有 DB 重新生成该设置——**手机 UI 把游戏省电策略设为"无限制"（长按图标→应用信息→省电策略）才是持久做法**，裸 settings put 可能被覆盖；② 改前先 get 保存原值、按需还原
- **虚拟屏声音在真机扬声器外放 = HyperOS 的 remote submix 重定向静默失效（2026-08-24）**：scrcpy `audio_source=output`（remote submix）在 AOSP 上应把音频从扬声器重定向走（手机静音），但 Xiaomi 15 Pro/HyperOS 上会话正常建立、捕获无报错、扬声器却照常外放——调 scrcpy 参数无解。修复：虚拟屏会话期间 `cmd audio set-volume 3 0`（STREAM_MUSIC 含游戏声），断开恢复原值（与 freezer 同一套 `pending_restore.json` 自愈）。两个坑：① HyperOS 无 `media` 命令，且 `cmd media_session volume --set/--adj` **报成功但实际不生效**，必须 `cmd audio set-volume/get-stream-volume`；② 副作用：浏览器音频随媒体音量一起变静（捕获音量跟随媒体音量，scrcpy #3790）——要浏览器听声就得接受手机外放（同一个音量），暂无两全方案
- **`POST /api/devices/:id/disconnect` 不踢 viewer → 旧 pusher 僵尸**（2026-08-23 实证）：强拆重建会话后，旧 viewer 的 pusher 挂在旧帧广播上继续补帧，浏览器永远显示拆会话前的定格帧，与设备真实画面脱钩（新会话出帧也不推给它），且补帧刷新 last_serve 让看门狗也不处置——REST disconnect 是当前唯一不踢 viewer 的拆会话路径（看门狗两条路径/api_update 都踢）；恢复 = 刷新页面（unload 关 ws → 新页干净连入）；修复方向：disconnect_device(force) 时同步踢 viewer
- **拆旧建新交错期的日志归因坑**（2026-08-23）：`session disconnected` WARN 与 `process exited` 可能是**旧会话/旧进程的迟到收尾**——旧帧 consumer 停在 `recv()` 上（无帧可收永不检查 connected 标志），直到新 connect 的 `adb reverse --remove-all` 切断旧隧道、旧 socket 死亡冲刷出最后一帧时才醒来退出；看时间线极易误判成"新会话被杀"。判断新会话死活看后续：新会话有自己的 config frame/帧流；旧进程退出后音频帧仍在涨 = 音频读循环属新会话
- **`rebuild/stop` 硬杀活会话后 adb 短暂楔死**（2026-08-24 实证，2026-08-25 补刀）：带活 scrcpy 会话强杀 gamer-server 后 2~3 分钟内，connect 卡在 `connecting scrcpy session`，70s 后报 `deadline has elapsed`（= reverse 10s + push 60s 双超时；期间 `adb devices` 看似正常但 reverse/push 挂起）——孤儿 adb shell 子进程/隧道 teardown 期 adb server 卡住。**2026-08-25 实证：优雅停机路径同样会楔**——`POST /api/shutdown` 拆会话后的恢复命令（freezer/音量 restore）先超时，新 server 起来后 connect 反复 `adb timeout: push scrcpy-server.jar`，自动重连每 70s 锤一次还延长楔死；且前端在锤、`adb devices` 显示 device 时传输实际已死（旧 server 缓存状态），手动 `adb reconnect offline` 把传输踢掉后设备直接消失，**重插变「未知 USB 设备(设备描述符请求失败)」**（手机侧 USB 控制器楔死，Windows 层 PnP 还显示 ADB 接口 OK 也没用）——恢复：换 USB 口 / 拔插等 30s+ 多次 / 手机开关 USB 调试或切 USB 模式 / 重启手机；仍不行需管理员在设备管理器卸载未知设备后扫描改动或重启 Windows。**修复两层（2026-08-25）**：① `gamer.ps1` Stop-Backend 退出后 `Reset-AdbServer`——`adb kill-server` 限 3s、不响应（server 主循环被卡）才强杀 5037 监听进程+残留 adb，再 `start-server` 拉起全新 server（强杀是最后手段：传输中途强杀可能搞坏设备端 USB 态）；② 服务端自愈——`connect_device` 连接前 `adb version` 2s 探测（`Adb::probe`），超时先 `Adb::reset_server`（kill-server 3s + 兜底强杀）；连接失败根因是 `adb timeout` 时同样重置后**重试一次**（设备侧也坏则明确报"设备不在线"而不是无限挂 70s）。另：`adb.run` 旧 `select!`（timeout 分支 vs sleep 分支）同刻到期竞态会抛裸 `deadline has elapsed` 且**不 kill 子进程**（泄漏 adb.exe 延长卡死），已改 `timeout()` 单分支统一 kill + 明确报 `adb timeout: [args]`（adb.rs run/run_bytes）。**根治（2026-08-24）**：加优雅停机——`POST /api/shutdown`（踢 viewer 只关 peer 不发 taken_over + `DeviceManager::shutdown_all` force 拆会话/清 reverse 隧道 + watch 信号触发 axum graceful 退出），`gamer.ps1` Stop-Backend 先调它等自然退出、curl 超时/失败兜底硬杀；该端点与其他 API 一致无鉴权（LAN 工具定位）。另（2026-08-25）：gamer.ps1 里对 adb 命令**不要用 `Start-Process -Wait`**——adb 会派生 `fork-server` daemon 子进程，PS 5.1 的 -Wait 实测永久挂起（rebuild 卡死在 Reset-AdbServer），一律 `-PassThru` + `WaitForExit(ms)` 有界等待 + 超时杀客户端
- **YAML 语法设计两坑（2026-08-25 实证，js-yaml 5 与 serde_yaml/libyaml 同规范）**：① **裸标量 `@` 开头非法**——`@` 是 YAML 保留字符，`- until: @1` 解析直接报错（引号 `'@1'` 才行）→ call 传参引用用 `$1`/`$2`（`$` 开头合法，无需引号）；② **标量列表项后不能挂同级键**——`- xxx`（无冒号）+ 缩进的兄弟键是"bad indentation"语法错误（标量折叠跨行撞 `:` 报 mapping values not allowed），**带子内容的键冒号必须写**（如 `- cond:` 的条件键 `- test.png:`；另外映射键之间不能插同列 `- ` 行——cond 颜色条件的步骤须在色值键正下方或 +2 缩进、`pos:` 之后不能再跟同列步骤行）。另：**js-yaml 5 默认 object 构造不支持数组键**——数组键（如 `- [x, y]: 色值`）抛 "object-based map does not support complex keys"（YAML 本身合法、serde_yaml 正常）；当前语法已不用数组键（color/cond 旧数组键写法均已删除），Console `yamlParse` 对该错误返回定向迁移提示
- **YAML 语法设计两坑之二（2026-08-26 实证）**：① **`&` 是锚点保留字符**——裸写 `- func1: &1` 会被解析成锚点、值静默变 null（引号 `'&1'` 才是字符串；混在字符串中间如 `call sub.yml &1` 没问题）→ find/color 上下文引用符选 `^N`（`^` 非 YAML 保留字符，裸写合法，js-yaml 5 与 serde_yaml 均验证通过）；② **映射值同级缩进会被解析成兄弟键**——`- loop:\n  times: 3`（times 与 loop 同列）解析结果是 `{loop: null, times: 3, steps: [...]}` 而非嵌套（序列值才有"dash 同列即值"的特例，func 函数体 `- find: $1` 同列正是靠这条）→ 引擎 exec_loop 对 times/steps **双支持**（loop 值内映射或步骤兄弟键均可），前端校验同步
- **YAML 语法设计两坑之三（2026-08-27 实证，func cond / 跨文件调用）**：① **键后直接跟同列 `- ` 步骤行 = bad indentation**——`- f1:\n    cond: test.png\n    - find: $1`（cond 与步骤同列）js-yaml 5 与 serde_yaml 都拒绝 → 函数体必须用 `steps:` 键包住（`cond:` 与 `steps:` 同为函数名键的兄弟键，靠"映射值同级缩进"那条规则解析成 `{f1: null, cond, steps}`），Console 校验对这种笔误返回定向引导提示；② **无参调用带 then/else 必须写冒号**——`- test1:fun2\n    then: ...` 是"标量列表项后挂同级键"语法错误（`- test1:fun2` 被解析成标量字符串步骤），要写 `- test1:fun2:`；③ **`- 函数名: 实参` 的同列兄弟键缩进基准是函数名键（=dash+2）而非 dash**：cond/steps 与函数名键同列（脚本里 `- f1:` 的 cond 写 4 空格）；同理 serde_yaml 与 js-yaml 对"根级 dash（0 列）"与"func: 段内 dash（2 列）"的同列判定一致，但单元测试若把 func 值拆出来裸解析（根级 dash 0 列）会与真实脚本解析结构不同——parse_funcs 测试必须按 `func:` 包裹的真实形态喂入
- **Docker 镜像启动即 `GLIBC_2.39 not found`（2026-08-27 实证）**：`rust:1.97-slim` 的 OS 底座会随上游滚动（现为 trixie/glibc 2.41），而 `debian:bookworm-slim` 只有 2.36——builder 与 runtime 底座不同代时 cargo 产物里的 glibc 符号版本在运行期才爆。修：运行时底座与 builder 保持同代（根 Dockerfile 用 `debian:trixie-slim`），升级 rust 基础镜像时检查 final 底座是否要同步换
- **BuildKit cache mount + dummy-main 依赖预热会产出空壳二进制（2026-08-27 实证）**：预热层用假 `src/main.rs` 编全部依赖后若不清本 crate 产物，后续真实源码构建层里 cargo 按 **mtime** 比对会把"宿主较老的 src 文件 vs 刚生成的 target 指纹"判为无变化 → `Finished in 0.2s` 直接链接 hello-world 空壳并 cp 进镜像（且该层还能整体 CACHED 复用错误产物）。修：预热层 RUN 结尾必须 `rm -rf target/release/.fingerprint/<crate>* target/release/deps/<crate_下划线>* target/release/<bin>`（根 Dockerfile 有注释），保证业务 crate 恒定重编、依赖恒走缓存
- **Docker Desktop 构建 `load metadata ... auth.docker.io/token` 连接超时而 CLI pull 正常（2026-08-27 实证）**：BuildKit 解析 FROM 元数据直连的 auth.docker.io 被 DNS 污染（解析到 31.13.x.x），报 dial tcp 失败；`docker pull` 走 desktop 代理/hubproxy 正常。规避：构建前先 `docker pull` 各基础镜像落本地，BuildKit 对本地已有镜像 metadata 免外网；另 `# syntax=docker/dockerfile:1` 行也要联网拉前端镜像，无特殊语法需求时不写（RUN --mount 等 Dockerfile 25 内置 frontend 已支持）
- **PowerShell 管道改写含中文的 UTF-8 源码会静默毁坏文件结构（2026-08-27 实证）**：Windows PowerShell 5.1 的 `Get-Content -Raw` 不带 `-Encoding UTF8` 时按系统 GBK 解码 UTF-8，部分多字节序列会把紧随的换行/引号吞进前一个字符（注释行与下一行代码粘连、字符串中途断开），回写后 rustc 报 "unexpected closing delimiter" 而源码肉眼难辨异常；规避：批量替换一律用 UTF-8 感知方式（编辑器/脚本工具，或严格 `-Raw -Encoding UTF8` 读 + 回写后立即编译验证），含中文注释的 .rs/.md 文件禁用无编码参数的 PowerShell 读写管道
- **配置加载不再自建 data 目录后裸容器启动即退出（2026-08-27 实证）**：`config.rs` 启动校验接管目录/文件检查后删除了旧 `create_dir_all(data_dir)` 兜底，裸 `docker run`（无任何卷挂载）时容器内 `/app/data` 不存在 → SQLite `Error code 14: Unable to open the database file` 直接退出；compose 绑定挂载场景不受影响（宿主目录存在，Linux 缺失时 Docker 会自动创建宿主侧目录）。修：根 Dockerfile 运行时层 `RUN mkdir -p /app/data` 预建
- **image crate 0.25 解码限额 API 与惯性写法不同（2026-08-27 实证）**：`ImageReader` 设限额是 `reader.limits(limits)`（&mut 原地设置、返回 ()，不能接在 builder 链尾再 `.decode()`），且 `image::Limits` 是 #[non_exhaustive]——结构体字面量构造直接编译错，必须 `Limits::default()` 后逐字段赋值；像素炸弹防护走 `limits.max_image_width/height + max_alloc` 三闸（单边尺寸/分配上界/解码后像素总数复核），PNG 解码器会在分配任何缓冲前先校验声明尺寸，故 IHDR 伪造的大图只花几百字节就干净报 Limits 错
- **Windows PowerShell 5.1 可能无法解析无 BOM 的 UTF-8 benchmark 脚本（已修复）**：含中文的脚本会按系统代码页误解码并报 parser error；`c4d82e3` 已加 UTF-8 BOM 并收紧格式化写法，改脚本后应继续用 5.1 parser 检查（当前 parser=0）。
- **`cargo test --lib` 在本仓库直接失败**：`gamer-server` 只有 binary target、没有 library target；最小回归应改用 `cargo test <过滤器>` 拆分针对性测试，稳定后再跑全量 `cargo test`。
- **Windows 测试清理 SQLite 临时目录可能报文件占用**：`Store` 的 DB worker 持有 `gamer.db`/WAL 句柄，丢弃发送端后 worker 退出与 `remove_dir_all` 存在时间窗；测试应显式关闭并 join worker（未提供关闭接口时需有界重试清理），不能把占用误判为数据逻辑失败。
- **`cargo test` 本轮实际失败在 `device::frames::tests::request_snapshot_bounds_per_cache_decode_concurrency`**：并发计数断言从 1 变成 2，说明同帧截图合并边界仍有回归；先保留失败证据再决定是否调试测试假设。
- **`cargo clippy --all-targets --all-features -- -D warnings` 曾被 dead_code、`too_many_arguments`、`manual_is_multiple_of` 卡住**：该类告警已由 `5b26eef` 收口为通过，若再次出现通常是新增未用代码或新 lint 回归，而不是旧基线问题。
- **`C:` 盘构建缓存耗尽后最稳妥的恢复方式是只清本仓库 `server/target`**：一次性清全局缓存会把别的项目也拖慢；只删本仓库目标目录并重新跑构建即可把空间和恢复时间控制在当前项目内。
- **Windows 下 SQLite / 文件句柄占用会让清理和覆盖操作短暂失败**：`gamer.db`、WAL 和测试 worker 持有的句柄会和 `remove_dir_all`、原子替换产生时间窗，必要时要显式等待 worker 退出后再清理或重试。
- **`cargo audit` / `cargo-audit` 不在当前工具链里**：`tools/verify-release.ps1` 会把缺失报成“未安装”而不是伪造通过，依赖安全审计要先补装工具再跑正式结果。
- **`cargo-audit` 2026-08-28 结果为 0 vulnerabilities，但仍提示 `bincode` unmaintained**：这次审计可以作为“无未处置高危项”的证据记录，但依赖维护状态仍需后续跟踪，别把“零漏洞”误读成“完全无风险”。
- **Windows 句柄竞争导致原子写偶发失败**：同一路径的替换在文件句柄未完全释放时会短暂被占用，修复为进程内替换锁串行化写入，规避方式是不要并发写同一目标。
- **C: 盘构建缓存耗尽时不应全量清理**：一次性清全局缓存会拖慢其它项目，恢复时只清本仓库 `server/target` 就能把空间和回收成本控制在当前工作区。
- **dev 前端(5173)登录/所有 POST 报「登录失败，请稍后再试」实为 403 forbidden_origin**：vite 代理 `/api` 开 `changeOrigin: true` 会把 Host 改写成 `localhost:8443`，与浏览器 Origin(5173) 不一致，命中后端 Origin↔Host 同源防护拒掉全部 POST/PUT/DELETE（GET 不校验所以页面能正常加载，极具迷惑性）；解决：`changeOrigin` 必须保持 false（与 `/ws` 代理一致），前端 `login()` 已把 403 映射为独立文案便于下次识别。
- **`cargo test` 存量计时敏感用例偶发红**：`store::prune_logs_deletes_all_eligible_rows_in_batches` 曾硬编码 "2026-08-28" 当"新日志"，跨天后被 retain_days=1 正确清理导致 504≠503 必挂（已改为动态 `Local::now()`）；`api::auth` 的 `session_lifecycle_absolute_and_sliding` 与 `session_sliding_idle_expires_and_renews` 都用 1.1s sleep 等 1s 过期，机器负载高时会误报（2026-08-30 并行 release 重编的高负载窗口一次连带 7 红、负载解除后复跑全绿）——单独重跑即过，勿当成回归。
- **小米 HyperOS 设备的传输方式不能靠 `adb devices -l` 判定**：实测 25079RPDCC 的 USB 串号是 16 位大写字母数字（与无线调试连接显示同一串号），且 USB 传输的行里**没有** `usb:` 标记——`usb:` 有是 USB 铁证、没有不能说明是无线；`infer_device_kind` 对无标记设备保守按 usb（kind 只影响保活门控，误判无功能副作用）。
- **adb server 重启（含 `gamer.ps1 restart` 内部 kill-server）会掉无线调试连接，且服务端无法主动救回**：Android 11+ 无线调试的重连由手机侧 mDNS 广播驱动，熄屏/深睡时不广播，`adb connect <串号>` 对裸设备名也无效（非 host:port）；只能等手机亮屏重新广播或插线。服务端无线保活因此只对可寻址的经典网络 adb（含 `:` 或 `.`）补连。
- **冷启动瞬间截图可能双路齐挂**：应用冷启动/画面剧变时帧缓存按需解码撞上 GOP 刷新会被判过期丢弃（两拍后返回"无帧"），同时 `screencap -d` 对切换中的虚拟屏可能返回非图片错误文本——两条截图路径同时失败。find 的轮询语义已改为软失败重试（20s 宽限），无需在设备层再加补丁。
- **小米 HyperOS 上游戏前台时 `screencap -d <虚拟屏>` 恒返回 ~80 字节错误文本**（疑似安全标志 surface），截图只能依赖帧缓存按需解码；帧缓存解码的货币性检查必须按 GOP 代际（snapshot/config generation）判定，不能按 frame_sequence——动态画面下 P 帧逐帧推进序号，按帧序判新会让任何解码（约 1s）永远追不上，动画期间截图 100% 失败。
- **`gamer.ps1 restart` 不重新编译，改完 Rust 代码必须 `-Build` 否则旧二进制继续运行**：restart 只 stop/start，直接启动 `target/debug/gamer-server.exe`（旧产物）；且服务运行中 `cargo build` 链接该 exe 会 os error 5 失败，错以为构建过。E2E 验收曾因此踩坑（/metrics 新指标全 0，实为运行中的 03:00 旧二进制）。解决：`.\gamer.ps1 restart -BackendOnly -Build`（先停服务再构建）。
- **并行开发/验收时多个任务共享同一 cargo target 锁会互相阻塞**：多个 Agent 同时跑 `cargo clippy/test` 会串行排队（正常），但一方写到一半的源码会让另一方编译失败——并行任务的验收命令要在对方收口后复跑一遍才算数。
- **`api::tests::sec_tests::expired_cookie_is_rejected_by_protected_route` 也属计时敏感偶发红**（2026-08-29）：登录后立即请求预期 200，但 `session_abs_secs: 1` 的绝对 TTL 下，并行测试负载只要把 login→before 间隔拖过 1s 就先收到 401（断言在 before 处失败，与测试名暗示的"过期后拒绝"不是同一处）；单独重跑即过，勿当成回归（与既有 `session_lifecycle_absolute_and_sliding` 偶发红同类）。
- **Docker bridge 部署 WebRTC 黑屏的完整根因链有三层，逐层排查别停在第一层**（2026-08-29）：① 容器内网 172.x 候选浏览器不可达——`rtc_external_ip/rtc_udp_port/rtc_external_port` 宣告宿主可达地址（webrtc-ice muxed 候选地址/端口取自 mux conn 的 `local_addr()`，自定义 `Conn` 包装汇报具体 `external_ip:port`，返回 0.0.0.0 会得到 0 个本地候选）；② 前端把 `createOffer()` 原始 SDP 直接发给服务端——里面**没有任何 a=candidate**（候选在 setLocalDescription 后的 `localDescription` 上），服务端零远端候选、ICE 零 pair，连接全靠浏览器对 answer 候选的 prflx 回路（已修：等 gathering complete 发 `pc.localDescription`）；③ 启动日志里 `pingAllCandidates called with no candidate pairs` 若只出现 1-2 次且紧邻 `connected` 属正常瞬态（prflx 注册前的单个检查周期），持续刷屏才是真故障。ICE 日志经 tracing-log 桥进容器日志（tracing-subscriber 默认 feature + `fmt().init()`），`RUST_LOG=webrtc_ice=debug` 有效；viewer 协商完成时会打一条 `ICE candidates: local=[...] offer_remote=N` 供容器排障。
- **`adb -a -P 5037 nodaemon server`（共享宿主 adb server，供 Docker 容器复用）不是常驻服务**（2026-08-29）：它只是以监听 0.0.0.0 的方式拉起一次 server，`gamer.ps1` 的 rebuild/restart（Reset-AdbServer）、服务端 adb 超时自愈（`Adb::reset_server`——共享部署下**任一实例**触发都会杀共享 server）、重启机器都会把 server 变回只听 127.0.0.1 的标准模式，需重跑 `tools/adb-share-start.ps1`；kill-server 期间 USB 设备断连几秒、运行中实例自动重连恢复属预期。容器经 `host.docker.internal:5037` 访问宿主 server 实测未触发 Windows 防火墙弹窗。
- **共享宿主 adb server 下，容器内 Gamer 的 scrcpy 会话必死（`accept video socket timeout`），纯配置无解**（2026-08-29 实证）：Gamer 在容器内 bind 127.0.0.1 随机端口 accept，而 `adb reverse` 的回连方是 **adb server**（共享后位于宿主）——它收到设备侧 localabstract 连接后硬编码连宿主自己的 127.0.0.1:<随机端口>（adb `network_loopback_client`，reverse 的 local 端不支持指定 host），宿主上无人监听 → 设备侧 scrcpy server 已正常启动（容器日志可见 `New display: …`）但 socket 连不上自退。解锁需改 `scrcpy.rs`（bind 地址可配 + 隧道方向反转为 `adb forward`），当前容器实例只承担 adb 层操作（scan/shell/设备管理），实时会话由宿主实例承担，详见 docs/reference/DEVICE_ACCESS.md。
- **Git Bash 里 `docker exec <容器> grep … /app/config.toml` 报 `D:/Scoop/.../app/config.toml: No such file`**：MSYS 自动把容器内绝对路径转换成 Windows 路径；容器内路径参数写成双斜杠开头（`//app/config.toml`）或加 `MSYS_NO_PATHCONV=1` 绕过。
- **js-yaml `load()` 解析 color 候选映射时纯数字色键会丢顺序**（2026-08-29，脚本编辑器重构阶段 0 实测）：`'123456'` 被解析成整数 123456，plain object 的整数形键按数值排在字符串键之前，`Object.keys` 顺序 ≠ YAML 书写顺序，颜色候选按序匹配语义被静默破坏。解决：脚本 v2 契约把 color `expect` 冻结为有序列表（每项单键映射，与 match 候选同构），前端 codec 解析有序映射一律走事件/Map 形态，不能用 `load()` 出来的 plain object 直接取键序。

## 2026-08-29

- 多 Agent 并行改同一仓库时共享 git index：A 任务 `git add` 与 `git commit` 之间 B 任务 stage 的文件会被 A 的裸 `git commit` 误扫提交。解决：`git add <路径>` 后立即 `git commit -- <同一组路径>`（pathspec 提交只含指定路径），永远不用裸 commit；遇 index.lock 等 3 秒重试。
- js-yaml 5（^5.3.0）API 重写：`dump` 无法按节点控制引号样式与缩进，`load` 丢失标量样式；需要规范序列化（params 整条单引号、match 紧凑缩进）须手写输出器（逐标量可借 `dump(lineWidth:-1)` 判 plain 安全性），校验引号样式走 `parseEvents` 事件级 AST（带 `style.singleQuoted`）。
- **Vue `reactive(model)` + `structuredClone` 会 DataCloneError**（2026-08-29，脚本编辑器组件层实测）：命令栈/模型层的 `structuredClone` 快照与 `cloneStepWithNewUuids` 无法克隆 Proxy（Vue reactive 代理及组件里 `{...proxy}` 展开出的嵌套代理字段都算）。解决：`commands.ts::unwrap` 递归按 `__v_raw` 深解包后再 clone（duck-typed，模型层不引 vue）；页面接线固定 `reactive(model)` + `new CommandStack(同一 reactive 实例)`，两处引用必须同源，否则命令改的是代理、组件读不到/反之。
- **@vue/test-utils 的 `setValue()` 对 input/select 都会触发 change**（2026-08-29）：测试里 `setValue` 后再补 `trigger('change')` 会让 `@change` 处理器执行两次（两条 undo 历史）；setValue 自带事件派发，不要叠加手动 trigger。

## 2026-08-30

- **静止屏挂机后回连会陷入"连接风暴"，每 ~11s 断连重连直到画面出现活动内容**（2026-08-30 实证 + 当日修复）：根因是应用被 Greeze **挂机冻结**——画面完全静止 → 编码器零帧输出 → viewer 连接只能重放帧环里那份陈旧的小 GOP（5.7~52KB，浏览器解不出/黑屏）→ 前端黑屏看门狗重连；`reset_video`（request-sync-frame）在冻结+静止下**永远等不到 IDR**（Activity 不渲染就没有帧，实测 7s 内 4 次 reset 全空）→ 循环，直到应用被 freeze-trap 重启产生画面活动才稳定。**修复**：viewer 注册时检测冻结（pidof + /proc stat 1s 零调度，精确命中冻结态、健康静态屏不误伤）→ plain start 捅醒（Activity Start 强制 THAW，应用原地恢复不重启，复用 watch_launch_freeze 节奏 +6s/+8s 复查补捅；脚本运行中/未配 pkg 跳过）。前端配合：黑屏看门狗改两级（8s 先补一次 reset_video，16s 才重连，减少风暴频率）。判据：失败连接伴随微重放 GOP + `reset_video requested by viewer (decoder desync)`，且全程无 `pusher live`；成功连接伴随大 GOP（533KB~1.3MB）。另见放大项：风暴中有 7s 内两条 signaling 并存——前端重连未串行化会拉长风暴（lifecycle connectLock 已有，注意别绕过）。
- **侧边栏"点了没反应"是两类导航级死锁，与具体页面无关**（2026-08-30 修复）：① 路由守卫 `probeSession()` 的 `fetch('/api/session')` 无超时且结论永久缓存——服务端高负载时 pending fetch 挂住全部导航（点任何项都没反应），瞬时不可达还会被缓存成"未认证"；已改为 4s AbortController 超时，网络错误/5xx 视为结论未知不缓存并放行导航（401 仍明确未认证，api 层 401 拦截兜底跳登录）。② `restart -Build`/重新构建后，已打开的旧页面点导航会懒加载旧 hash 的 chunk → 404 → 导航静默失败（URL 变了页面不动）；router.onError 命中 chunk 加载错误时整页刷新一次加载新产物（sessionStorage 标记防刷新循环，afterEach 清除标记）。
- **服务端重启后 WebRTC 每 ~4.2s 重连一次、连十几次，是 ICE 建连层失败，别与 Greeze 黑屏风暴混淆**（2026-08-30 两轮实证 + 当日修复）：判据 = 整个循环**无** `control data channel opened`/`SRTP ready`/`pusher live`，且伴随 `discard success message ... no such remote`（对端合法 ICE 应答被丢弃）；黑屏风暴则 ICE 正常连通、卡在"重放陈旧 GOP 黑屏"，节奏 ~11s。**根因是 webrtc-rs 0.13 默认 mDNS QueryAndGather**：answer 的 host 候选宣告为 `xxx.local`，浏览器必须经 mDNS 解析才能发 ICE 检查——Windows 同机部署下这条链间歇性失效（防火墙/组播/网卡增删敏感），解析失败 = 每轮 4s 双双判死；命中解析 = 11ms 秒连。且 webrtc-rs 从不解析**远端** .local 候选（resolved_addr 恒 0.0.0.0:0），服务端自发起的检查永远不可能成功，连接本就只靠浏览器侧检查驱动（prflx 回路）——`no such remote` 是该死路径的固定噪音。**修复**：`rtc_net::build_rtc_setting_engine` 恒建 SettingEngine 并 `set_ice_multicast_dns_mode(Disabled)`（answer 带明文 IP，浏览器直达；nat1to1/固定端口逻辑不变）；同时前后端去掉 Google STUN（国内不可达、收集白等最长 5s 拖慢 answer，属顺带清理非根因——去掉后风暴复现一轮才定位到 mDNS）；`peer failed/closed` 日志补 `was_connected` 字段区分"已连通后正常终结"与"ICE 从未连通"。跨网不在支持范围（需要者自行反代/组网），容器 NAT 场景仍走 rtc_external_ip 静态宣告。
- **57c1964 严格引擎迁移的漏网之鱼：旧"call 空格切分位置实参"脚本提交即 400，且数据库零记录**（2026-08-30 实证）：`call: 目标.yml 参数.png`（5ff4a36 的空格+括号感知切分）与 `$1` 位置引用在 script_v2 下全删——`call` 目标必须是精确脚本 id、实参走具名 `args:` 映射、模板参数用 `'tmpl:name:备注'` 声明 + `$name` 引用；另 `find.block` 必须是**列表**（标量单模板非法）。判定特征：POST run 返回 400 `invalid_args` 五元组诊断、`logs` 表无该脚本的任何运行记录（失败发生在引擎启动前，与运行中失败的"脚本执行失败"落库行为区分）。迁移脚本清单要挨个过，hkrpg 分区当时只迁了 日常遗器/utils，漏了 三账号日常/通用日常。
- **GB_LOG 设成裸词（如 `GB_LOG=1`）会在 `server/` 根下产出 `1.YYYY-MM-DD` 日志文件**（2026-08-30）：GB_LOG 非空且非 "stdout" 时整体视作日志基准路径，无目录成分 → dir=`.`、prefix=裸词，轮转文件名 `<prefix>.<日期>`。规避：GB_LOG 传完整路径（gamer.ps1 已如此）；`server/*.20??-??-??` 已入 .gitignore 防误入库。
- **全量 `cargo test` 偶发 matcher 统计测试失败（TEST_HITS 3≠2 / 未命中增量 ≠1）并连锁 PoisonError**（2026-08-30 实证，单跑即绿，同日已修）：`matcher::tests` 的统计钩子/生产 metrics 测试断言进程级**精确**计数，而无锁并发的计算池测试会额外产生全屏命中。已治：命中/全屏断言改下界（与生产 metrics 测试同口径），未命中/区域无并发写入者保持精确。
- **改了 Rust 代码但行为没生效：`gamer.ps1 start/restart` 不带 `-Build` 跑的是预构建 release 二进制**（2026-08-31 实证）：Start-Backend 只在二进制不存在或显式 `-Build` 时才 `cargo build --release`，改源码后直接 restart 会继续跑旧逻辑（案例：函数名 unicode 校验已放宽，线上仍报旧文案 `[A-Za-z_][A-Za-z0-9_]*`）。规避：改 Rust 后用 `.\gamer.ps1 restart -BackendOnly -Build`（或先 cargo build --release 再 restart）；前端无此问题（vite dev HMR / web-dist 由 `npm run build` 产出）。
- **当前工作树执行 `pnpm build` 会因 `MainLayout.vue` 导入 `runs.js` 未导出的 `isMissingEndpointError` 而失败**（2026-08-31）：前端运行实现未合流导致导出契约不一致，恢复对应导出/合流前端改动后再验收；本轮文档与 fixture 支线不改业务实现。

## 2026-09-09（函数体系与插件依赖简化）

- **Package 函数库从 `functions/<分类>.yaml` 迁到 `automations/_function*.yaml`（破坏性，无兼容层）**：保存钩子对 functions/ 路径直接报 `yaml.functions.dir.removed`；存量开发数据手动迁移——把旧 `plugins/gamer-yaml/functions/*.yaml` 内容（`functions:` 包装）并入 `automations/_function.yaml`（同名函数合并会冲突报错，先改名）；调用名 = 函数名与文件无关，模板引用改写不受影响。
- **函数运行寻址从 `<pkg>/<文件短路径>.yaml#<函数名>` 收敛为 `<pkg>#<函数名>`**：RunTarget::Function 删 file 段、ManualPayload 删 function 字段；前端 `runYamlFunction` 第一参传 Package id；带路径段的函数 entrypoint 一律 400 invalid_payload。
- **`find` 不再承担轮询语义**（timeout/interval 已从 Schema 删除，传了报「未知参数 timeout」）：等待轮询用 wait_find；find/wait_find/tap_template 共用 `match_once` 单一匹配实现，不会分叉出两套匹配逻辑。

## 2026-08-31（自动升级批次 0/1 实施期）

- **无 BOM 的 UTF-8 `.ps1` 含中文会被 Windows PowerShell 5.1 按 GBK 解析报语法错**：5.1 无 BOM 时按系统代码页读脚本；本批新增 release/packaging、tools 脚本统一写带 BOM 的 UTF-8（修复方式同 gamer.ps1 条目）。
- **cmd 里 `命令 & echo %ERRORLEVEL%` 拿到的是旧值**：`%VAR%` 在整行解析期展开，验收退出码永远显示改动前的 0；用 PowerShell `$LASTEXITCODE` 或分步执行。
- **本机 GitHub 直连超时而代理只配在 git config**：curl/Invoke-WebRequest 不读 `http.proxy`，下载 github.com 资产必须显式走代理；fetch 脚本已内置 `-Proxy` 参数并回退 `HTTPS_PROXY`（dl.google.com 可直连）。
- **platform-tools 没有固定版本下载 URL**：`platform-tools_r<ver>-windows.zip` 命名 404，Google 只保留 latest 入口；adb 只能「latest 下载 + 锁文件内版本号/整包 hash 双门禁」锁定（`dependencies.lock.toml`）。
- **BtbN win64-lgpl 构建没有 libx264**（GPL 组件已排除）：生成 H.264 冒烟流不能照抄 x264 命令，用 libopenh264（LGPL 兼容）；`fetch-ffmpeg.ps1` 按 openh264→x264→硬件编码器顺序自动选。
- **.NET `Process` 重定向大输出先 `WaitForExit` 会父子互锁**：~30KB 的 `ffmpeg -encoders` 输出撑满管道缓冲后双方互等；必须先异步排空 stdout 再等退出。
- **Node 与 Rust 对 manifest `schema_version` 类型判定不一致**：JS `1.0===1` 放行浮点写法，Rust serde 严格拒绝；launcher 取 fail closed 严格语义，Node 校验器用于发布门禁前需补该反例。
- **clippy `doc_lazy_continuation`**：`//!` 文档行以 `+` 开头被当 doc 列表项，续行未缩进直接 `-D warnings` 失败；「A → B」式续行换措辞或缩进。
- **Windows 相对路径拼接产生混合分隔符**：`PathBuf::join` 可得 `config\./data`，PathBuf 相等按组件归一，但 `to_string_lossy()` 后的字符串断言跨平台必挂；测试断言比 PathBuf 不比字符串。
- **bin crate 里未接线的 `pub` 模块照样 dead_code**（`pub` 不豁免）：框架性「先交付后接线」模块（build_info）需带理由注释的 `#![allow(dead_code)]`，接线时移除（file_migration 已随 P11.7 整体删除）。
- **build.rs 声明任何 `rerun-if-*` 后 cargo 即关闭「包内文件变化重跑」默认**：git commit 探测必须显式 `rerun-if-changed` 跟踪 `.git/HEAD`+`.git/refs`，否则同分支新提交的 hash 陈旧。

## 2026-08-31（批次 2/发布链路实施期）

- **zip crate 读取器按条目名建 IndexMap，重复条目被静默折叠**：依赖 `by_index` 遍历的安全解包器感受不到重复条目攻击；解压前须独立定位 EOCD 逐头清点 central directory 条目数，与折叠后不一致即拒。
- **zip crate 写入器把 central directory 的 version-made-by system 写 0**：unix mode（含 symlink 标记）在写入再读出链路丢失、`is_symlink()` 恒 false；符号链接检测不能只信 unix mode，落地后全树 reparse point 扫描兜底，测试夹具须手工拼字节。
- **Windows PowerShell 5.1 的 `Compress-Archive` 对子目录条目用 `\` 分隔**：跨工具验收（manifest 路径规则）直接拦截；组包用 `System.IO.Compression.ZipArchive` 逐文件建条目强制 `/`，组包后保留反斜杠条目自检。
- **PS 方法调用参数列表里 `-f` 的逗号数组被拆成方法参数**：`$list.Add('{0}' -f $a, $b)` 报 FormatException，须写 `-f @($a, $b)` 显式打包（语句层同写法正常，极具迷惑性）。
- **cmd 下 `node -e "code with =>"` 会把 `>` 当重定向**在 cwd 生成垃圾文件（如 0 字节的 `n`）；多行/含特殊字符脚本一律落临时 .mjs 文件再执行。
- **SQLite `TEXT PRIMARY KEY` 在 `PRAGMA table_info` 中 `notnull=0`**（PK 不隐含 NOT NULL）：写 schema 快照 fixture 别按 DDL 直觉填 1。
- **PS 5.1 `Set-Content -Encoding UTF8` 写出带 BOM 文件**：TOML 解析器把 BOM 并进首键报 `missing field`（column 1）；生成 config.toml 用 `-Encoding ASCII` 或无 BOM UTF8。
- **server 的 config.toml 必填键比直觉多**（decode_frames/max_size/bitrate_mbps/fps 无 serde 默认）：手写最小配置缺一个即启动退出，排障时先对照 config.rs 全字段。
- **运行中服务的 WAL 库只读打开可能失败**（无 -shm 时）：maintenance 工具做只读优先、失败退回读写打开但零写入的兜底。
- **launcher `--install-root .` 相对根会让注入的 GAMER_* 稳定路径变相对路径**（违反路径契约，server 端 jar 路径被重复拼接启动失败）：注入前必须把安装根按 cwd 词法规范化为绝对路径。
- **端口就绪探测按「端口 200」判定可被同端口无关进程误满足**：E2E 曾被遗留 dev server 占 8443 误报 PASS；集成验收前先清端口，升级验收需叠加 boot_id/版本断言。
- **PowerShell 的 `New-Item` 没有 `-LiteralPath` 参数**：升级器离线行为测试在 pwsh 7 直接失败；目录创建改用 `[IO.Directory]::CreateDirectory()`，路径检查/文件操作继续使用 LiteralPath。
- **PowerShell 函数会捕获未管道丢弃的 `Copy-Item`/`Move-Item` 输出**：升级快照函数的返回路径会被文件对象污染并写入状态；这类副作用命令必须显式 `| Out-Null`。

## 2026-09-01（升级验收/发布收口轮实测）

- **PowerShell 双引号串不处理 `{}`，`"$Tag^{{commit}}"` 把字面双花括号传给 git refspec 必 fatal**：`{{`→`{` 转义只在 `-f` 格式串内生效，rev-parse/ls-remote 的 peel 语法收到的不是 `^{commit}`；refspec 一律单引号拼接（`check-immutable-release.ps1` 已修）。
- **PS 5.1 `Join-Path` 不认 `\\?\` verbatim 前缀、`tar -C` 进不了 >260 字符目录**：长路径台架须用 `\\?\` 拼接 + robocopy 搬运、launcher exe 用短路径（8.3）中转 spawn——.NET 无法 spawn >260 的 exe，CreateProcess 的 cwd ~260 上限 verbatim 形态也不例外（launcher 侧已用 `fallback_current_dir` 回退短祖先）。
- **server `/api/shutdown` handler 同步 await 完整 drain，客户端 HTTP 读超时主动断开会 drop handler 使 drain 永不完成**：hyper 在客户端断开时取消 handler future，`ShutdownCoordinator::request()` 停在 `(self.drain)().await` 且无自恢复（真机实测 drain 11.6s > 旧读超时 5s）；升级器对 shutdown 请求的读超时必须 ≥ 最长 drain 时长（run 宽限 + 会话拆除断链，现取 shutdown_timeout+5s）。
- **容器内构建没有 .git，build.rs 自动探测必然回落 dev 提交信息**：镜像内 build_info 显示错误的 dev 提交；镜像构建必须显式传 `GAMER_GIT_COMMIT` 等 ARG（Dockerfile 已加，release workflow 注入）。
- **`/api/tasks` 的 `last_run_at` 是固定 UTC Z 串不随 TZ 变化，前端推导服务端时区只能用 `next_run`**（现已带 `%:z` 偏移）：拿 Z 串当本地偏移会在 TZ≠UTC 部署下说谎；`task-tz.js` 对 last_run_at 只认显式数字偏移。
- **MIUI 设备 adb 常见 USB+TLS 双 transport 并存**：扫描入库的 addr 可能被 TLS serial 覆盖（kind 显示 wifi），对设备执行 adb 命令需 `-s <serial>` 显式指定目标传输，别按 addr 形态臆断。
- **本地 Windows rust 门禁全绿但 CI Linux job 挂 clippy/test**：`#[cfg(windows)]`/`#[cfg(unix)]` 互斥代码只在对方平台编译——bin crate 里仅 Windows 路径消费的 pub 常量/枚举变体在 Linux 下报 dead_code，`C:/...` 形态绝对路径在 Linux `is_absolute()==false`；本地复现 CI 链用 `docker run --rm -v "D:\code\gamer:/work" -v gamer-cargo:/usr/local/cargo -v gamer-target:/tmp/target -e CARGO_TARGET_DIR=/tmp/target -w /work/server rust:1.98`（先 `rustup component add clippy rustfmt`，镜像默认不带）跑 clippy/fmt/test。
- **schema 版本化之前的旧开发库（`user_version=0`，含已退役 `scripts` 表、`tasks` 缺 args 两列）会被启动门禁拒绝**：报错只有 `gamer-server.err.log` 一行（`database schema is unversioned`），`gamer.ps1 rebuild` 构建全绿但后端"启动后立即退出"；用 `gamer-server.exe inspect --data-dir server/data --json` 确认 status。不实现 migration 0 是设计决策，别指望自动迁移——要保数据按 `store.rs` 的 `SCHEMA_V2_DDL` 手工重建新库迁数据（见 `baseline-backups/rebuild-v1.sql`）；开发环境不用管，`gamer.ps1` 启动秒退命中该类特征（unversioned / schema 不完整 / 新库版本号异常）即自动把旧库挪入 `baseline-backups/gamer.db.auto-<时间戳>` 后重建重试一次。
- **登录报"账号或密码错误"真因常是「无凭据 fail closed」，换密码试多少次都没用**：认证只认 `[auth].password_hash`（固定参数 Argon2id PHC）或 dev 环境变量 `GAMER_ADMIN_PASSWORD`，config 顶层遗留的 `password = "..."` 明文字段不被消费；两者皆缺时启动即 fail closed，任何账密都 401。排障看启动日志 `credential_source`（`unavailable` = 没配凭据）；PHC 可用 python argon2-cffi 按固定参数生成（工具留档 `baseline-backups/gen_phc.py`）。
- **`GB_LOG` 语义已变为「目录+前缀」按日轮转，不再是单文件追加**：传文件路径会被拆成前缀产出 `gamer-server.log.2026-09-01`，原 `gamer-server.log` 停在旧内容不再更新；`gamer.ps1` 的 Show-Status 与注释仍按单文件读，"最近日志"显示陈旧内容误导排障——先找当日 `*.log.<日期>` 文件。

## 2026-09-01（match/color 候选级命中点击实测）

- **候选映射形态 `{click: true, steps: [...]}` 的键若与候选模板键同列，会被解析成候选映射的第二个键报"必须是单键映射，得到 2 个键"**：YAML 映射值必须比键深一级缩进、序列才能与键同列（紧凑缩进特例只适用于列表形态分支步骤）；手写时 `click`/`steps` 必须比模板名多缩进两级，序列化器已按此冻结（fixture v14 锁死）。
- **服务端开发模式默认静态托管 `server/web-dist`，只修改 `web/src` 不重新构建时页面仍加载旧前端包**：前端改动后执行 `cd web && pnpm build`，或开发调试使用 `pnpm dev`。
- **彩色模板必须用文件名尾部 `#1` 标记**：旧格式仍按灰度匹配，裁切弹窗默认不保留颜色，勾选“保留颜色”后服务端自动追加 `#1` 并执行命中后颜色复核。
- **`core.autocrlf=true` 会把 script_v2 fixture 检出为 CRLF，导致规范 YAML 往返测试误报行尾差异**：fixture 路径已在 `.gitattributes` 固定 LF，测试读取也统一归一化行尾。

## 2026-09-02

- **全量 `cargo test` 的仓库数据严格加载测试会因工作树脚本引用已删除的函数库文件失败**：同步恢复 `data/<pkg>/functions/` 依赖或清理对应脚本引用后再跑全量测试，功能单测仍可独立通过。
- **Windows 上 Rust 全量测试与前端构建并行时 `rustc` 可能内存分配失败并退出**：多个重型编译任务同时占满内存；先结束并行构建，再单独重跑 `cargo test`（必要时降低并行度）。
- **部分 Android 游戏在多指 `ACTION_POINTER_UP` 后不会继续处理仍按住的虚拟键**：scrcpy 触点仍然存在但游戏状态未重建；释放一个键后对剩余触点补发一次 `MOVE` 重新锚定，避免 A+D 松 D 后 A 失效。
## 2026-09-03

- **ZIP 重复路径负例不能直接由 `zip 2.x` 写入器生成**：`ZipWriter::start_file` 会提前拒绝 `Duplicate filename`，安全校验测试需在合法归档中央目录中复制已有记录后再验证重复路径拒绝。
- **PowerShell 用 `web-dist\*` 复制 Vite 产物会压平 `assets` 子目录，发布后 HTML 可打开但 JS/CSS 404**：改为复制整个目录并在打包复核中强制检查 `web-dist/assets`。
- **Full 包只带程序和依赖会漏掉仓库内置 YAML、模板、函数库及 keymap，首次启动的数据目录为空**：打包时复制 `server/data` 下的分区目录，排除根级 SQLite 运行文件，并在解压复核中逐文件检查种子资源。
- **sandbox iframe 的本地 PoC 若直接写源文件路径不会随 Vite 生产构建复制**：用 `?url` 导入生成 `web-dist/assets/iframe-poc-*.html`，Host 才能在开发与构建产物中复用同一静态 UI。
- **多个 Phase 骨架以未提交状态同时存在时，cargo check/test 会先被前置模块的类型错误拦截**：本次工作树命中 Store oneshot、API 借用和 App Package 导出错误，模型层验收应先隔离工作树或合并前置改动。
- **`web/package.json` 未提供 `lint` 脚本时直接运行 `pnpm lint` 会落到 PATH 中的 Android lint 并返回 usage**：本轮前端门禁以现有 `pnpm build`/`pnpm test:run` 为准，需另行引入 linter 才执行 JS lint。
- **Timer Core schema 升级后旧 `tasks` 表仍是 YAML 兼容 API 的入口**：迁移会回填 `timer_tasks`，后续旧任务写入必须走 Store 双写，直接操作单表会让调度状态与 REST 数据分叉。
- **Phase 7 的 keymap WASM harness 仍是独立标量 ABI**：它不经过本轮 Component/WIT Host、权限或 iframe Bridge，测试通过不能据此宣称插件运行链路已验收。
- **Wasmtime bindgen 的 WIT `path` 按 Cargo 包根解析且 `resource` 是保留字**：绑定路径使用 `wit/gamer`，资源域接口命名为 `resources`（Host API 仍为 `resource`），否则会在编译期解析失败。
- **Capability 层若每次匹配都生成新的 frame/resource handle 而不回收，会让长运行内存随匹配次数增长**：FrameStore 限制短帧窗口，ResourceAdapter 按逻辑 ResourceId 复用句柄，跨层只传 Handle。
- **Wasmtime 48 在 Windows 启用 `component-model-async` 后执行 Component 的 `call_async` 会破坏 Tokio 线程上下文并在运行时退出崩溃**：Phase 6 仅启用 `async + component-model`，保留异步 Host bindings，关闭当前未使用的 Component Concurrency 提案。
- **Timer Core 的 `next_wakeup` 持久化为 Unix 秒而内存时间带有更高精度**：重启恢复时会截断亚秒部分，持久化恢复断言与比较必须按秒精度处理。
- **Runner 可能在 `submit` 返回前同步完成**：完成事件若早于 active-run 登记会留下不可取消句柄，需用完成游标/通知和登记前交接处理。
- **YAML Component 的同步 WIT `call` 若沿用通用 async bindgen 会要求 async Store，并在 Windows GNU Tokio fiber 清理时崩溃**：YAML world 保持同步 bindgen，能力调用在独立 current-thread Tokio runtime 中执行；通用 extensions Host 不启用 YAML world。
- **Vitest 默认 include 只覆盖 `src/*.test.js` 与 `src/script-editor/**/*.test.js`**：YAML workspace 测试放在 `src/workspace/` 会被静默跳过，必须放到 `src/` 根级或显式扩展配置。
- **WASM guest fixture 的 Cargo 构建会在各 fixture 目录生成独立 `target/`，不会命中 `server/target/` 规则**：这些目录均为可重建生成物，按 fixture 路径精确忽略，不要提交。

## 2026-09-04

- **match/color 候选不点击且无分支步骤时不能省略 YAML 值**：`- 模板:` 会解析为 `null`，严格 loader 报“步骤必须是列表”；序列化必须写 `- 模板: []`，点击候选仍可用 `click: true` 并省略 `steps`。
- **业务数据出库后全新 clone 无任何脚本/模板/按键映射资源，属预期**：默认发行「零业务资源」，`server/data/<pkg>/{scripts,functions,templates,keymaps,presets,resources}/` 已从 git 索引移除并整体忽略；迁移既有资产用 Gamer 内置导出（`POST /api/app-packages/export`，需先 `PUT /api/workspace/<pkg>` 初始化 package.toml）打成 .gamerpkg 经 `POST /api/app-packages/install` 安装（安装即激活、包内 presets 自动发布），或把旧分区目录整份拷入新机 `server/data/` 作为本地编辑区（EditableLocal 一等源）。
- **`<script setup>` 内隐式全局赋值（如 `reconnectAttempts = 0`）编译期不报错、运行时才抛 ReferenceError**：SFC 编译为严格模式 ES Module，误写未声明变量只会在触发对应回调时崩掉后半段；拆分 `useConsoleDeviceManager` 时改为 `consoleRuntime.reconnectAttempts.value = 0`，同批还发现 `onDisconnect` 引用了不存在的 `stopLogPolling`（断连清理中断），由脚本运行 composable 补齐同名包装。
- **declarative 插件面板的按钮动作链路已收口到 `POST /api/extensions/:id/call`**：服务端校验插件必须 Running 且 `action` 在该 manifest declarative schema 的按钮集合内（`ExtensionError::CallRejected` → 400），声明过的按钮之外没有任意调用入口；guest 侧由通用 extension world 新增的 `call` 导出执行。
- **通用 extension world 新增导出会让旧 guest 直接实例化失败**：`gamer:host/extension@1.0.0` 增加 `call` 后，该 world 的组件必须同时导出 `run` 与 `call`；手写 wat fixture 需自带 `memory`+`realloc`，`result<string,string>` 按 canonical ABI 经调用方 retptr 写回（判别字 + ptr/len 三个 i32，扁平结果超 1 个即走返回区）。
- **Registry proof 的 download_url 原本强制 `https://`，会拒绝随包本地市场的同源相对路径**：官方 .gplugin 随 web-dist 托管为 `/plugins/<id>-<version>.gplugin`，该 URL 已被 proof 签名绑定，验签侧放行 `https://` 或 `/` 开头的绝对路径即可，篡改仍会被哈希/签名校验拦截。
- **PowerShell 5.1 里 `@('a' + (X), 'b' + (Y))` 的逗号优先级高于 `+`**：实际解析为 `'a' + (X,'b') + (Y)`，数组元素被静默吞并成一个字符串；每项各自加括号 `@(('a'+X), ('b'+Y))`。
- **Wasmtime 实例任务进入"命令循环待命"后 stop 必须经通道优雅收尾再 join**：entry 返回后任务停在命令通道 `recv()`，直接 abort 会跳过 Store/Component 正常清理（Windows GNU 下 async fiber unwind 不安全）；`plugin.call` 复用同一通道分发调用。
- **update 安装门禁的 `next_cron_secs` 改读 TimerCore 持久化唤醒游标后，刚保存的任务存在一个"游标未计算"的极短窗口**：`Scheduler::next_wakeup_in_secs()` 读 `timer_tasks.next_wakeup`（与调度循环睡眠同源，不再经 CronExtension 逐任务重算），新任务保存后游标由 notify 唤醒的 run loop 毫秒级补上，窗口内门禁视为无待执行任务；测试模拟需手动 `set_timer_task_wakeup_async` 预置游标（无 run loop）。
- **Keymap E2E 基准断言"scrcpy 写顺序"不能用相对阶段值 `scrcpy_write_us`**：它是各事件"写完成−收到"的时长，处理快慢波动下天然非单调（实测第 2 条 53µs < 第 1 条 102µs 直接误报乱序）；判序必须用进程内单调钟（`KeymapTraceRecord.scrcpy_write_instant`，`#[serde(skip_serializing)]` 不进 JSON）。
- **WASM guest fixture 的内层 `cargo build` 也会吃外层 `CARGO_TARGET_DIR`**：设置该变量跑测试时 fixture wasm 落到外层 target，测试按 `tests/keymap-guest/target/...` 读取落空；已在 `keymap.rs::build_guest_fixture_component` 统一 `env_remove("CARGO_TARGET_DIR")` 根治（组件测试与 Phase 6 真机 E2E 基准共用该入口）。
- **rustdoc 注释某行以 `+` 开头会被解析为文档列表项**（如换行后写 "` + wit-component 编码`"），`clippy -D warnings` 报 `doc list item without indentation`；避免行首出现 `+`/`-`/数字列表前缀。
- **edit 提取 preflight 曾以本地工作区解析包内脚本的 func/call 引用，提取到空工作区必 400**：`PackageBuilder::validate_dir` 的跨文件引用校验读的是本地编辑区 functions/（此时已被删空），`resource.func.not_found` 与「包内脚本/函数内容缺失」无关；已修复为 validate_dir 注入被校验目录自身 scripts/functions 内容作最高优先引用视图（导出路径目录即工作区、行为不变），回归锁在 `builder::tests::validate_dir_resolves_cross_references_from_directory_itself`。
- **本地删空后 `POST /api/scripts/:id/run` 对纯包内脚本返回 404，属当前实现边界而非包损坏**：run 端点的脚本存在性前置校验只读本地编辑区（`ScriptStore::get`），不查 composite 三层；包内资源的运行链路由引擎运行快照（EditableLocal > UserOverride > InstalledPackage）保证，验证包内容用引擎快照/composite 读面（keymap GET、脚本保存期模板校验），或先 edit 提取到本地再运行。
- **Windows `core.autocrlf=true` 把仓库内 LF 的哈希/签名钉死文本检出成 CRLF，直接打爆锁测试**：phase0 夹具（SHA-256 锁）、`tools/plugins/*/manifest.toml`（include_str! 同步锁）、`tools/plugin-signing/gamer-dev-1.pem`（内嵌信任锚锁）逐一报"哈希/内容漂移"；解法：每类钉死文本在 `.gitattributes` 加 `text eol=lf`（二进制 PNG 勿加 `text`，其内部本就可能含 0x0D0A 字节序列），再 `git checkout --` 重 smudge 即恢复；Windows 上新建此类文件也要 LF 落盘。

## 2026-09-05

- **新建/同步 worktree 后，`.gitattributes` 已钉 `text eol=lf` 的夹具仍可能是 CRLF**：attribute 变更（或 merge/rebase 同步）不会对已检出文件重 smudge，phase0 夹具逐一报"SHA-256 漂移"而主工作区全绿；解法：`rm <文件> && git checkout -- <文件>` 强制重 smudge（单 `git checkout --` 不重算），逐个修到全 LF。新 worktree 先跑一遍哈希锁测试再开工。
- **gamer-yaml 扩展的 `start` 不启动任何 WASM 实例，这是刻意的过渡缝而非遗漏**：其 guest 实现的是 `yaml-extension-host` world（按调用惰性实例化，`run_yaml_vnext`/`LazyYamlWasmtimeRuntime`），拿去走通用 `extension-host` 实例 world 的 `LazyWasmtimeRuntime::start` 必然链接失败进 `Failed` 态；P11.2 起 `ExtensionService::start` 对该 id 只做状态迁移 + `TimerRunnerRegistrar` 注册（`stop`/`disable` 相应跳过实例停止），Wave3 把 runner 移进扩展边界后一并消除。
- **恢复缺依赖任务必须精确匹配 `suspend_reason == missing_dependency=<runner_id>`，不能按状态批量恢复**：`dependency_missing` 还会被 schedule provider 缺失（reason 记 provider id）触发，用户手动 suspend/disable 是 `suspended`+`enabled=0`；`resume_timer_task_from_dependency_missing_async` 用「状态+原因」双守卫 WHERE 保证只回 Active 那批真正因该 runner 缺席挂起的任务，且恢复时经 ScheduleRegistry 重算 `next_wakeup`（沿用挂起前的陈旧游标会立即补触发一次）。
- **happy-dom 环境下 `import.meta.url` 不是 `file://` scheme**（location 指向 http），`new URL(rel, import.meta.url)` 读 fixture 文件直接抛 "URL must be of scheme file"；组件测试（`// @vitest-environment happy-dom`）里读源码/夹具用 `join(process.cwd(), 'src', ...)`，只有 node 环境测试能走 import.meta.url 方案（console-components.test.js 即此）。
- **@vue/test-utils 的 DOMWrapper 不透传原生属性访问器**：`wrapper.title` 返回 undefined（不是空串），`find(b => b.title.includes(...))` 会在谓词里自己炸 "reading 'includes' of undefined"；一律 `b.attributes('title')?.includes(...)`。
- **vitest 同文件用例共享模块级单例 store（scriptsData/templatesData/devicesData 等 ref）**：上一用例填充后，下一用例"为空才拉取"的懒加载被短路，列表内容错位且报错点远离根因；组件挂载测试在 afterEach 显式清空这些 data ref。

- **axum 0.7 多个 Router merge 后同一路径+方法注册两个 handler 只在运行期 panic**：`Overlapping method route. Handler for POST ... already exists`（编译期完全无提示，首个请求/测试才炸）；通用资源路由同时出现在 protected_json 与 protected_upload 两组时触发。规避：每个「路径+方法」只注册一次，文本/字节分派收进同一 handler 内按 kind/Content-Type 走，上传体限额组独占 POST/PUT。
- **axum 0.7 通配段 `*id` 的 Path 提取**：`/api/apps/:app/resources/:kind/*id` 里 id 是完整 `<pkg>/<rel>`（%2F 会被解码），handler 里**不要再拼一次 app 前缀**（拼了就是 `<pkg>/<pkg>/<rel>` 必 404）；字节 kind（templates）的 id 是不带分区的裸文件名，文本 kind 才带前缀，app 路径段只做可选前缀校验。
- **`core::fs::safe_name`（safe_name/`sanitize_rel_segments`）拒绝 `#`，而模板文件名合法字符集恰恰含 `#`**（区域/颜色后缀）：字节资源名校验不能走通用分段校验，templates 必须用 `sanitize_template_name`（resources.rs `normalize_binary_name` 分.kind 处理），否则模板创建全 400。
- **模板「同基名冲突」的基名要剥离区域后缀再比对**：新名 `login_btn#000_000_500_500.png` 与存量 `login_btn#100_200_300_400.png` 冲突——从 stem 里 `split('#')` 取首段做基名、再查「等于基名或以 `基名#` 开头」，拿完整 stem 当基名会漏判。
- **` cargo test` 里 `unwrap_err()`/`unwrap()` 要求 Err/Ok 两侧实现 Debug**：给含 `Arc<dyn Trait>` 字段的 Store 手写行为时，trait 对象没有 Debug 会连累 `unwrap_err()` 编译失败；测试里用 `match` + `panic!` 替代。
- **通用资源 API 后 keymap GET 单条返回的是资源条目 JSON（content 原文 + 注记 name/binding_count/valid），不再携带解析模型**：useConsoleKeymap 仍按旧 `rep.model/bindings` 形状取模型会恒报「服务端返回的映射结构无效」，工具条映射下拉与画面可视化全哑；解法：前端按 content YAML 自行解析（js-yaml load），注记 `valid:false` 时带诊断报错并清空选择，不把坏方案装进输入链路（keymap-runtime.test.js 锁定）。
- **服务端 RunRecord 没有也不再有 `script_name` 字段**：`runner_id + entrypoint` 是唯一目标标识，`script_id` 只是服务端保留的兼容展示字段；前端展示名回退链写成 `rec.script_name || rec.script_id` 会静默拿到 undefined，恢复运行态/冲突弹窗统一用 `entrypoint || script_id`。
- **`GET /api/logs` 行内的运行目标字段仍叫 `script_id`（schema v1 logs 表列名）**：Core 日志面板（gamer.core:logs）按「设备+运行目标」分组时直接把它当 opaque entrypoint 展示（`entrypoint || script_id` 兜底），不要为了映射显示名去预拉脚本列表——那会把业务资源知识带回 Core 壳。

## 2026-09-05（Phase 11 W4-A：Legacy 清扫 / 扩展启动对账）

- **删路由后探针断言可能空转**：gate/update-gate 测试曾用已删除的 `/api/scripts/x/run` 探测「业务路由被闸拦截」，catch-all fallback 对任意路径都回 503，断言恒真、不再证明任何事；删端点时要同步全仓搜引用它的测试并把探针换成现役路由（本批改 `/api/runs`）。
- **判定「生产无调用方」别用 `| head` 截断的 grep**：matcher 路径缓存子系统曾被误判为纯死代码，实为「消费端仍在（api/resources、app_packages/edit 的失效调用）+ 生产端早已不产出」的半死子系统；完整判定用 `grep -rn ... | awk -F: '{print $1}' | sort -u`，删子系统时生产者/消费者两端一起清。
- **`#[ignore]` 的测试同样参与编译**：`cargo test --no-default-features` 因 phase0 keymap e2e 测试引用 `wasm-runtime` feature 门控符号而编译失败（测试体根本不会运行）；跨 feature 引用的测试必须自带 `#[cfg(feature = ...)]`。
- **模板缓存按内容哈希（SHA-256）寻址后，写路径失效调用是纯冗余**：新内容天然新键，旧条目只会多占内存等 LRU 驱逐，不影响正确性；P11.7 已整体移除路径键/解析代数缓存与全部失效调用，后续不要再往 `invalidate_template_cache_*` 方向加代码（已不存在）。

## 2026-09-05（Phase 11 W5-B：App Package 生命周期 E2E）

- **通用资源 API 的字节 kind（templates/resources）创建/替换统一走模板 PNG 重编码**：`api_create_binary_resource_inner` 与字节 PUT 替换对任意字节 kind 都先过 `reencode_bytes`（PNG 解码+重编码），`resources/` 分区经 REST 只能放 PNG 文件；非 PNG 附件只能落盘直写（导出/安装侧只查路径与大小、不校验内容类型），E2E 内容对账要以「创建后落盘字节」为准而非上传字节。
- **包卸载挂起把任务 `enabled` 落成 0**：`TimerCore::on_app_package_uninstalled` → `suspend_timer_task_async` 直接 `state='suspended', enabled=0, next_wakeup=NULL`，与 dependency_missing（保留用户 enabled 意图、按 runner 精确恢复）语义不同；按「挂起不改 enabled」写的断言或恢复逻辑会翻车。
## 2026-09-05（Phase 11 W5-A：P11.9 架构守卫测试）

- **通用资源字节 kind 的 GET id 必须整体带分区前缀 `<pkg>/<文件名>`**（URL 里 %2F），`app` 路径段填 `-` 通配只是跳过前缀一致性校验、不会替你补分区名；裸文件名当 id 查必 404（`ResourceStore::get_binary` 按 `split_once('/')` 拆分区）。
- **字节 kind 的 POST/GET 共用 templates 的 PNG 归一化管线**（`reencode_bytes` 不分 kind 一律 `reencode_template_png`），`kind=resources` 目前存不了任意字节文本，只能存 PNG；要"原样字节"得走 App Package 通道。守卫测试（§14.4）因此以 PNG 往返代替任意字节断言。
- **`AuthState` 的会话 cookie 签名密钥按实例随机**：测试里重建 router（新 AuthState）后旧 cookie 全部 401，模拟"进程重启"必须重新 login 拿新会话，不能复用上一台 router 的 cookie。
- **P11.9 源码扫描守卫用「文件+内容片段」行级白名单且双向校验**：只加单向过滤会留死条目——代码改掉后白名单永不命中也不报错，守卫静默失效；`assert_whitelist_alive` 强制每条白名单命中至少一行，条目腐烂即测试失败。

## 2026-09-05（Phase 12 P12.4：Guest Execution Budget）

- **wasmtime epoch_interruption(true) 后 store 缺省 deadline=0 即「已过期」**：任何 wasm 执行（含组件 instantiate）都会立即 trap，store 必须在 instantiate 前 `set_epoch_deadline` + 配好 `epoch_deadline_callback`，否则报「epoch deadline reached」而非业务错误。
- **epoch deadline 回调的错误类型是 `wasmtime::Error` 不是 `anyhow::Error`**：wasmtime 48 把 anyhow fork 成自有 Error，回调签名写 `anyhow::Result<UpdateDeadline>` 编译报 E0271，用 `wasmtime::Error::msg` 构造。
- **epoch 取消会抢先 capability 边界的 kind=cancelled**：stop 置位后若 capability 调用跨过 tick 边界（~10ms），guest 恢复执行的首个 epoch 检查点直接 trap，guest 内已就绪的 kind=cancelled 错误不再冒出——取消判定别只匹配 `kind=cancelled`，要接受 `CANCELLED`（两形态都是合法取消，ADR-YAML-04）。
- **wasmtime Component 的 WIT import 签名变更 = 旧 guest 全灭**：`programs.resolve` 去掉 depth 参数后，旧版 gamer-yaml plugin.wasm（含 web/public/plugins 的官方 .gplugin）在新宿主上 instantiate 直接失败，升级后必须重打/重装插件（`tools/build-plugins.ps1`）。

## 2026-09-05（Phase 12 P12.5/P12.7：v3 defaults 与 find/match 收口）

- **debug 构建下 wasmtime Component 编译可占数秒，会污染 e2e 墙钟断言**：对「等待时长落在区间」类断言，计时前先在同一 runtime 上空跑一次预热（复用已编译模块），否则 sleep 断言被 JIT 编译时间冲垮；或只断言下界。

## 2026-09-05（Phase 12 P12.6：Runtime Visualization Events）

- **仓库有两个同名异型 `DeviceId`**（`capabilities::device::DeviceId` 与 `core::models::DeviceId`）：`RuntimeEvent`/`EventSink` 只认 Core 形态，在 yaml_extension 里拿 struct 里 import 的 capabilities 版直接传会报 E0308；跨域传设备 id 给事件时显式写 `crate::core::DeviceId`。
- **f32 分数经 serde_json 往返不是精确字面量**（0.92f32 → 0.9200000166893005）：断言 vision 事件 score 要按 f64 容差（1e-6）比较，`json!(0.92)` 全等比较必翻车。
- **web 的 vitest 只收 `src/*.test.js` 与 `src/script-editor/**/*.test.js`**：新测试文件放 `src/components/console/` 等子目录会被 `pnpm test:run` **静默跳过**（显式指定文件名才会报 "No test files found"），新前端测试一律放 `src/` 根或改 vitest.config.js include。

## 2026-09-05（Phase 12 P12.8：yaml guest 正式化与官方包重打）

- **.gplugin 不是字节可复现产物**：plugin-signer 打 zip 用当前时间做 entry mtime，同一份 guest 源码两次构建 sha256 不同——别用「sha 没变」判断没重打，registry.json 与 .gplugin 必须同批由 `tools/build-plugins.ps1` 生成（registry 条目 sha256 绑定包文件；2026-09-08 起免签名，无 proof）。
- **扩展 manifest 升版会打挂按版本卸载的测试**：guard 全链测试曾硬编码 `YAML_VERSION="3.0.0"`，升 3.1.0 后 DELETE `/api/extensions/:id/:version` 404；版本一律从 `YAML_EXTENSION_MANIFEST_TOML` 现场解析（`yaml_market_version()`），勿再硬编码。

## 2026-09-05（Phase 12 P12.9：YAML v2 删除）

- **v2 脚本删除后只报版本错误、且「看得见的失败点不止运行一处」**：非 `version: 3` 源在保存/导入/导出 preflight（400 invalid_yaml）、entrypoint 描述（400 invalid_script）、手动运行/任务门禁（版本诊断）统一报 `yaml.v3.version(.missing)`——排查旧脚本问题先看版本键，不要往参数/模板方向猜。
- **模板引用改写对存量 v2 文件静默跳过**：模板重命名只改写可解析的 v3 源（`rename_template_source` 对非 v3 返回 None、函数库解析失败跳过），分区里有删不掉的 v2 存量文件时其模板引用不会跟随改名；v3 保存边界不再做模板存在性校验（归运行期 composite 解析与前端 `yaml.v3.resource.tmpl_not_found`）。
- **手动/定时函数运行现与脚本同走 v3 guest**（原 v2 原生执行器路径已删）：`RunTarget::Function` 经 `yaml_vnext::load_function` 进 `run_yaml_vnext`，函数文件必须是合法 v3 bare-map；参数 wire 仍是七类 TypedValue，int/number 显式实参以文本形态过线（已知限制，见 task_params `coerce_v3_arg`）。

## 2026-09-05（Phase 12 P12.11：验收收口）

- **刚结束的 run 瞬时 GET 404（run_not_found）**：`RunManager::finalize` 先摘活动注册表再入档案（两次独立短锁，中间还夹一条 info! 日志），`get_run` 顺序查两处——202 派发后立刻 GET 无设备快败的 run 恰落在间隙会 404（测试负载下偶发，P12.11 基线实测复现）；测试侧对 run 查询一律轮询容忍 404/非终态（见 isolation 守卫测试），生产侧若要消除需把 finalize 的档案入列与注册表摘除收进同一临界区。
- **全量 `cargo test` 偶现 tokio `is_entered` 线程 panic 打印**：P12 基线起偶见两条 `c.runtime.get().is_entered()` panic 输出（api/tests 大并发区段，线程内无 runtime 上下文调用了 Handle 依赖代码）；panic 被独立线程兜住，测试恒 0 failed / exit 0，属测试进程噪音非产品缺陷——判定回归以 `0 failed` 与退出码为准，排查以单模块复跑定位。
- **v3 宿主曾丢失模板 `#区域` 后缀语义（v2 迁移回归）**：v2 引擎按模板实际文件名 `#` 后缀（`xx#u/d/l/r…` 半区、`xx#0_0_500_500` 千分比矩形、`#1` 彩色标记）限定搜索区域；v3 NativeYamlHost 只透传步骤显式 region，短名解析到带后缀文件后全屏搜索 → 误匹配/点错位。修复：VisionAdapter 在步骤未给 region 时用 `matcher::template_region_from_name(解析后文件名)` 兜底（与匹配预览端点同源）；显式 region 优先。
- **安装即用改变了扩展安装响应状态**：REST 安装现在自动 enable→start（失败降级 Enabled+last_error，不回 201 Failed）。断言安装后 `state=="installed"` 的测试/脚本需改为 `running`（或降级 `enabled`）；test 装配未接 timer registrar 时 gamer-yaml 的 start 会走通用实例路径失败降级——生产 main.rs 已接线，不受影响。
- **v3 宿主坐标系曾硬编码 1000×1000（迁移回归 #2）**：NativeYamlHost 的 `screen` 初始化后从不刷新，center/tap/region 全按 1000×1000 换算，而模板测试端点用真实 `session.video_size()`——非 1000×1000 设备上脚本运行与测试预览位置必然不一致。修复：每次 `capture` 后经 `FrameService::size` 刷新 `screen`（RwLock），匹配/回显/触摸全跟随真实帧分辨率。

## 2026-09-06（Package 一级作用域切换：后端根基重构）

- **数据根已切 `data/packages/<package-id>/`，旧 `data/<android 包名>/` 六目录不再被读写**：包 id/plugin id 严格 `[a-z0-9][a-z0-9._-]*`（禁 `.`/`..`/大写/分隔符），Android 包名（允许大写）**不能**再当资源分区名用——设备 `pkg` 字段只作 Android 运行目标，设备→Package 运行上下文映射归 T2a；旧数据无自动迁移，按目录手工搬进 `packages/<pkg>/plugins/<plugin>/` 即可。
- **`GET .../resources/<子目录>` 是按文件读取（404），不是列表**：递归列表端点是 `GET /api/packages/:pkg/plugins/:plugin/resources`，子目录限定用 `?prefix=`；同理 PUT 文本资源**不再自动补 `.yaml` 扩展名**（Core 内容无关，路径即所写），带裸名写入会得到无扩展名文件。
- **包归档（.gamerpkg）顶层只允许 `package.toml`/`shared/`/`plugins/<plugin-id>/`**：manifest 从 `manifest.toml` 换成 `package.toml` 且**必须是首个条目**（打包器显式写入，collect 时跳过它防 zip 重复条目——曾因目录扫描把 package.toml 再收一遍导致自检 Duplicate filename）；导入默认 409 附已存摘要，`?overwrite=true` 原子替换（旧目录先挪 .staging 再换入，无半安装态）。

## 2026-09-06（Package 模型 + 前端架构全链收口）

- **旧分区迁移到 packages 布局时 package-id 必须小写化**：`validate_scope_id` 只收 `[a-z0-9][a-z0-9._-]*`（拒大写），Android 原名含大写（如 `com.miHoYo.hkrpg`）不能直接当目录名——Package id 落成 `com.mihoyo.hkrpg`，Android 原名写进 package.toml 的 `[targets.android].packages` 做兼容声明；两命名空间严格分离、不互相推导（权威注释 `core/models.rs` AppContext 上方）。
- **模板上传与归档导入的内容校验口径不同**：资源 PUT 经 gamer-yaml 字节钩子强制灰度归一化（解码+重编码，非法 PNG 直接 400），而 .gamerpkg 导入只做布局/manifest/路径安全校验、**不经过插件内容校验**——包内模板以导出侧字节为准，别假设导入后与上传管线同源。
- **脚本资源 id 首段现在是 Package id**：可为纯自定 id（如 `official.hsr.daily`）与 Android 包名完全不同名，按 Android 包名拼脚本 id / entrypoint 会 404（结构化 not_found）；id 形态 `<package-id>/<automations 内相对路径>.yaml`（`automations/` 前缀由 gamer-yaml 内部映射，id 中不写）。
- **并行 agent 共享工作区开发时 git add 必须按文件所有权清单**：各自只 stage 自己地盘的文件，禁改文件被他人改坏时等对方自愈、不要抢修（多双手同改一个文件会产生叠加损坏；本波真实发生过 usePackageContext.js 语法错误由属主 agent 自愈、旁路 agent 抢修反而冲突）。

## 2026-09-07（界面术语 Package→配置 + 本机构建环境）

- **rustc 内存不足不止报分配失败，还会随机崩在不同 crate 的 ICE（`STATUS_STACK_BUFFER_OVERRUN`）**：本机 32G 内存单独跑全量 `cargo test` 也连续三轮各崩在一个不同依赖（curve25519-dalek 数百条假 trait 错误 / wit-parser / regalloc2 / webrtc-util），根因是编译 wasmtime 时 `rustc-LLVM ERROR: out of memory`——别当成代码问题排查；解法 = 关依赖 debuginfo + 降并行 `CARGO_PROFILE_DEV_DEBUG=0 cargo test -j 2`（wasmtime 单 crate debug 编译即数 GB），与「与前端构建并行时内存分配失败」同根源但单独跑也会触发。
- **本机缺 `wasm32-unknown-unknown` target 时 32 个 WASM guest 测试全数失败，属预期环境缺口**：yaml-guest/keymap guest/declarative 插件 roundtrip 等在测试内现场 cargo 构建 guest，报 E0463 `can't find crate for core/std … target may not be installed`；`rustup target add wasm32-unknown-unknown` 即愈，不装则判定回归只看其余 529 项（CI 有官方 guest wasm32 构建关卡兜底）。

## 2026-09-07（视频工作台并行开发）

- **WIT 函数名不得用保留字**：interface 里写 `list: func()` 直接解析失败（`expected type, resource or func, found keyword list`）——`list`/`use`/`type` 等都是 WIT keyword，命名加 `-media` 类后缀；`host.wit` 与 `HostApiDomain::ALL` 是同一契约两半（测试锁死），加域必须同步，但**新 interface 先定义、别急着挂 world extension-host**（宿主 linker 会要求提供全部 world import）。
- **raw Annex-B h264 没有 PTS，`ffmpeg -c copy` remux 会按假设帧率（25fps）伪造容器时间戳**：需要事件↔画面对齐只能自带 muxer 保留原始 PTS（`recording/mp4.rs`）；手写 MP4 两坑：avcC 记录必须带 box 头、dref 必须包在 dinf 里。
- **ffmpeg 8/9 已移除 `-vsync` 选项**：抽帧用 `select` 过滤 + `-frames:v 1` 即可，写 `-vsync 0` 直接报 Unrecognized option。
- **PowerShell 5.1 管道 `cargo … 2>&1 | Select-Object` 会把 stderr 包成 ErrorRecord**：包装脚本退出码可能为 1 而 cargo 实际成功——判断结果看输出尾行（`Finished`/`test result: ok.`），勿信 `$LASTEXITCODE`。
- **Vue `ref()` 对对象值做深度代理**：把 `<video>` 等 DOM 元素存进 ref 后取回的是 proxy ≠ 原元素（`instanceof`/原生 API 判定全失效），存 DOM 用 `shallowRef`。
- **happy-dom 两个媒体测试坑**：`canvas.getContext('2d')` 返回 null（裁切类测试需桩 createElement）；不派发 `seeked`/`timeupdate`（时间轴测试手动 trigger 并预置 `element.currentTime`）。
- **promise 型 API 的参数校验必须发生在 async 函数体内**：箭头函数体里同步调 `requireId` 类校验会在调用点同步 throw，`await expect().rejects` 捕不到、测试假绿或假红。
- **`<img>` 同 src 重复赋值不会重新触发 load**：busy 态由 img 事件驱动时，同 URL 重复点击要显式 no-op、切换素材/清空要显式复位，否则 busy 卡死。
- **YAML v3 `key` 步骤只接受字符串 keycode**（命名键或数字字符串如 `key: "1234"`）：整数形态运行时报「key 必须是按键名字符串」——草稿生成与手写脚本同源注意（词表见 `yaml_extension.rs::key_code`）。
- **内存墙的假错还有「元数据失效」形态，别误判成工具链/target 损坏去 `cargo clean`**：`only metadata stub found for rlib dependency core/object`、`cannot resolve a prelude import`、`cannot find Option/Ok/Some`（shlex/syn 等无辜 crate 报 core 相关错）与 ICE 同根源——并发 rustc 撞提交内存（commit）上限，挂掉的进程留下半截 rmeta 连累下游，每轮崩点不同、看似随机；本机曾因此误清健康的 target 增量缓存。处理同上：`CARGO_PROFILE_DEV_DEBUG=0` + 降 `-j` 重跑，cargo 增量渐进恢复；判据是单独 `rustc` 编译小文件全绿、`cargo check` 全新最小项目也绿。
- **cmd 里 `set X=0 && 下一条` 会把尾随空格赋进变量**：`CARGO_PROFILE_DEV_DEBUG=0 `（带空格）让 cargo 直接报 `error in environment variable ... could not load config key profile.dev.debug` 假失败（与构建无关）；写作 `set "X=0"` 引号形式。
- **PluginCenter 操作成功提示（notice）会被紧随的 `refresh()` 清掉**：`refresh()` 开头 `clearMessages()`，notice 在 refresh 之前赋值等于白写（安装提示曾这样闪没）——统一「先 `await refresh()` 再赋 notice」（activateVersion 与 installArchive 均按此序）。
- **PS 5.1 读 UTF-8 无 BOM 的 .ps1 会按 ANSI 解析**：中文字符串/注释里的多字节序列可能恰好解码出引号类字符，脚本直接 ParserError（报错位置与真实语法无关）——含中文的 .ps1 必须保存为 UTF-8 **带 BOM**（`tools/build-plugins.ps1` 即踩此坑；Write 工具默认无 BOM）。
- **PS 5.1 对 `[pscustomobject]` 赋不存在的属性直接抛错**（`在此对象上找不到属性"X"`）：不能先建对象再 `$o.NewProp = ...` 补属性，所有属性要在 `[pscustomobject]@{}` 字面量里一次声明（占位 `$null` 即可）。
- **Git Bash 里 `sed -i`/`perl -pi` 的模式含 `\n`（反斜杠+字母）会静默不替换**：MSYS 运行时对命令行参数做路径/转义改写，`\n` 到达工具时已被折叠成别的形态，匹配不中却退出码为 0（rust 源码里形如 `manifest_version = 1\nid = ...` 的 Rust 字符串字面量整行替换两次落空）——此类「源码字面量批量改写」用 Edit/Write 工具按唯一锚点改，或改完立即 grep 复核替换是否真的发生。

## 2026-09-07（Phase 8：Package 媒体分发）

- **`MediaMetadata.refs` 为空时 JSON 里字段整体缺席**（`skip_serializing_if = "Vec::is_empty"`）：断言/前端读 `meta["refs"]` 会拿到 undefined 而非 `[]`，要用可选取值兜底；同理 `probe` 缺省也省略。
- **.gamerpkg 布局白名单已扩展 `media/**`（受控：仅 `media/index.json` 与 `media/files/<64hex>`）**：旧版本服务端导入含 media/ 的新归档会 400「顶层条目不在白名单内」——跨版本分发包先确认两端服务端都升到 Phase 8；旧归档（无 media/）不受影响。
- **归档媒体恢复不重跑 ffprobe**（元数据随 `media/index.json` 的 `probe` 原样恢复）：手造媒体索引测试别假设导入后有真实探测值；索引缺 probe 时导入侧按 `codec="unknown"`、宽高 0 兜底。

## 2026-09-07（Phase 7：模板制作/离线测试/草稿闭环）

- **native call 动作响应无 `data` 信封**（`POST /api/extensions/gamer-yaml/call` 的 native 分支顶层即结果 JSON）：前端按有无 `data` 两形态兜底（videoApi），E2E/脚本断言别假设 `{data:{...}}`。
- **NCC 拒绝纯色模板且搜索区需 ≥ 模板+1px**：`template is uniform color`、`template larger than screen`——E2E/测试造模板必须带结构纹理，且别让模板文件名 `#区域` 恰等于模板大小（至少留 1px 余量或显式传全帧 region）。
- **Windows Python 打不开 Git Bash 的 `/tmp/...` 路径**：混用 bash 工具与 python 处理临时文件时，目录用 `cygpath -m "$(mktemp -d)"`（C:/ 风格两边通吃）。
- **heredoc 写入含转义字节的脚本文件**：外层 python 的字符串字面量会把 `\xNN` 形态解释成真实字节落盘，bash 再喂给 python 就成非法源码——生成脚本里的控制字节用 `bytes([...])`/`chr()` 构造，别用反斜杠转义。
- **官方 .gplugin 安装请求头**：`X-Gamer-Extension-Source: official` + `X-Gamer-Permission-Confirm: true`（缺后者有权限声明的插件装不上，报权限确认缺失）。

## 2026-09-08（模板 zip 上传中文文件名）

- **上传带中文文件名的 zip 模板包逐张报「资源路径段非法」400**：中文 Windows 资源管理器/WinRAR 打的 zip 文件名字节是 GBK 且不带 UTF-8 标志（bit 11），fflate 对无标志名按 Latin-1 解码出乱码（`»`/`½`/C1 控制符），服务端 `sanitize_segment` 只放行 Unicode 字母数字与 `. _ - #` 空格。规避：前端解压前按中央目录标志位解码（`template-upload.js` readZipEntryNames，无标志 GBK fatal 严格解码、失败退 Latin-1、zip64 整体退 fflate 原行为），fflate 自产/7-Zip/macOS 包（有标志）不受影响。
- **用外部工具（WinRAR 等）改 .gamerpkg 会导不回来**：服务端中央目录解析强制条目名合法 UTF-8（GBK 字节直接整包拒绝「归档路径必须是 UTF-8」，fail-closed 不落乱码资源），服务端自产包写的是 UTF-8 名 + UTF-8 标志，闭环无损；要改包内容请走 Console 界面或解包后用服务端重打包，别用压缩软件原地改。

## 2026-09-14（函数库自动保存校验）

- **函数库自动保存误报 `wait_find` 等内置函数不存在**：前端校验器把未提供的完整函数目录替换成当前文件函数名，导致原生函数和跨文件函数被误拒；完整目录缺省时跳过函数存在性检查，目录已提供时合并本文件函数再校验，变量、模板和语法校验保持生效。
- **脚本新建后不能继续保存、重开名称为空、合法脚本摘要报解析失败**：YAML API 封装直接透传通用资源响应并丢掉列表正文；在业务边界补齐 id/name/file/content，保留函数库 `.yaml` 后缀，改名调用 rename 并单独追踪名称脏状态。
- **日志字符串、时间和返回文本误报模板不存在**：客户端把所有字符串都送入模板校验；只校验 Schema 声明为 template 的实参，嵌套容器仍递归校验变量引用。
- **简写调用缺默认参数、`tap: $hit.center` 或 Package 函数接收对象时报缺参数**：宿主简写绑定提前返回、解释器无法区分求值后的对象与命名参数表；降线前按首参数包装简写，所有实参走同一默认值绑定流程，显式 null 作为 any 值保留。
- **并发保存不弹版本冲突窗口**：资源 API 只有错误文本，编辑器依赖 `code`；资源写入冲突统一附 `version_conflict` / `version_required` 机器码。
- **模板改名后简写找图仍引用旧文件**：AST 改写只处理命名参数；同步处理四个视觉函数的字面量简写，保持日志文本不变；模板短名需要保留 `.png`。
- **刷新页面后模板列表空白、看起来像模板被删**：模板面板只在挂载时请求一次，当 Package 尚未异步恢复时请求失败且不再加载；监听 Package 恢复/切换重新加载，并用请求序号拒绝旧响应覆盖当前列表，模板磁盘文件无需恢复。
- **中文函数名保存被拒绝**：函数定义与调用曾误用只接受 ASCII 的变量标识符规则；前后端统一使用支持汉字的函数名校验，回归覆盖资源保存、入口参数查询和真实 WASM 调用。
- **函数找图在首次等待后报资源路径非法**：函数入口的 `#函数名` 被带入配置包 ID，且通用前端 run 封装丢弃显式 content_package；修复字段透传和入口分隔解析，提前校验配置 ID，并记录首次 WASM 编译耗时。
- **函数参数默认值被全部展开或可选空值写入脚本**：新步骤只初始化无默认值的必填项，默认参数按按钮显式覆盖；可选参数默认收起，点击后占位只用于展示，空白不落入 YAML，已有实参和显式 null/default 按声明保留。
- **教程缩减后遗漏大部分用法却仍通过测试**：原测试只验证已有案例能解析，补齐注释案例并按原生函数目录断言函数和参数覆盖，同时校验可选参数启用后的版本。

## 2026-09-16（函数显示名称）

- **控制流卡片读取函数 Schema 时可能空指针**：`resolvedSchema?.fn === step.fn` 在两边都是 undefined 时也成立；先确认缓存对象存在，且仅函数调用摘要读取参数 Schema。

- **Windows 能枚举 REDMI K Pad 的 USB/WinUSB 接口，但 `adb devices -l` 为空**（2026-09-18）：本次为 ADB 服务传输状态异常，`adb reconnect offline` 无效；确认无其他在线设备后执行 `adb kill-server`、`adb start-server` 恢复，随后用 `adb -s <serial> shell echo gamer-adb-ok` 验证命令链路，不能仅凭列表为空断言设备已拔出。
- **普通「刷新/连接」不能恢复 ADB 正常返回空列表的故障**：设备「更多 → 强制重连」调用 `POST /api/devices/:id/force-reconnect`，清理旧投屏会话、重启共享 ADB 后重连当前设备；操作会影响其他设备，已有运行/采集/录制或并发连接时返回 409，设备仍未出现时需检查线缆/调试授权。


## 2026-09-19 · 新工作台界面回归要点

- `PanelRegistry.resolve()` 读取普通 Map，不直接订阅 revision。依赖它的计算属性必须同时订阅 `getPanels()` 或 revision，否则 URL 在插件注册前进入时，导航出现而内容保持空白。
- 顶部导航的 Teleport 目标在 Console 挂载后才存在；挂载前禁用 Teleport，避免空目标与热更新共同导致 Vue patch 失败。
- `currentPackageId` 是只读 computed。编辑守卫恢复包选择必须经过 `selectPackage`，不能直接赋 `.value`。同步守卫可能恢复旧值，因此持久化必须保存实际接受的最终 ID。
- inline 编辑器的保存不应销毁画布/撤销栈。运行前保存可能重建步骤 UUID，必须在保存前记录顶层步骤序号；脚本和函数均如此。保存冲突不能继续运行。
- 新建函数先刷新真实函数库并校验加载成功，再决定追加还是创建；不能用暂时为空的客户端缓存判断服务端文件不存在。
- 视频工作台纵向 flex 滚动区的内容块应 `flex-shrink: 0`，否则小高度窗口里项目素材区会与时间轴重叠。左侧舞台与右侧项目共享时间轴时必须交接控制权，卸载右侧恢复左侧播放条。
- 运行终态会清空 `store.runScript`。错误跳转使用保留的 RunRecord entrypoint 校对 Package/资源；事件没有可靠跨函数调用栈时展示详情，不猜测另一个文档的步骤。
- 配置详情通过 Teleport 渲染时，不能依赖父组件 scoped CSS 提供弹窗内边距；详情组件自身负责布局。

- 工作台自定义页签顺序不能按当前可见标题持久化：插件会因应用匹配和启停暂时隐藏；使用稳定 panel key 保留隐藏项位置，仅排序当前可见项，新面板追加。

## 2026-09-19 UI 收口

- Windows 运行中的服务 exe 不能覆盖：日常服务保持运行，验收用 `cargo rustc --bin gamer-server -- -o <隔离路径>`；替换隔离 exe 前等待旧进程真正退出，不能只发 Stop-Process 后立刻复制。
- Windows 原子替换偶发 AccessDenied/SharingViolation：MoveFileEx 仅对 5/32/33 做最多 8 次有界重试（累计等待 140ms）；不先删目标，持续失败仍上报，旧内容保持完整。
- 不透明来源 iframe 外链资产无法依赖 Strict 登录 Cookie，module 还有跨源约束：示例打包成 CSP 哈希授权的自包含 HTML；不加 allow-same-origin，不放宽登录 Cookie。
- sandbox=allow-scripts 不允许表单提交，submit 处理可能根本不触发：桥接操作按钮用 type=button + click，保留 allow-forms 禁止状态。
- 子函数失败会随后触发父调用失败事件：定位保存第一次最内层失败及资源版本，不能用后续退栈路径覆盖。
- 批量上传期间切换配置包可能让后续条目落入新包：操作开始即冻结包 ID，过期结果不回填当前列表；状态反馈也使用操作序号隔离。
- 同插件多个子页面交接状态时，旧页面卸载不能直接按插件 ID 清空；只清除自己实际发布的消息对象。用显式 watch 监听输入，避免 watchEffect 意外订阅反馈存储后形成发布竞争；未接管状态用 undefined 表达。
- 状态条两侧百分比加固定分界会超出容器：使用剩余宽度的 fr 分配。溢出不能只用原生 title 补全，因为被裁掉的复制/详情按钮仍无法点击；补全区应包含实际按钮，并约束视口宽高。
- 任务按钮与列表贴在一起：宿主 flex gap 只分隔直接子元素，不能分隔 TaskBoard 内部工具条和列表；面板自身声明纵向 gap、min-height:0，并让列表独立滚动。
- 市场与插件页重复跳转到同一中心：概览、已启用功能列表和管理弹窗职责重叠；市场直接展示可安装内容，插件页直接展示全部已安装管理，功能入口只归工作台。右栏卡片用容器查询换行，避免大视口下窄面板仍挤成一行。
- 首页默认不能只改 Console 初值：URL 同步仍可能经 PanelRegistry.defaultPanel 回到任务；无 panel 或失效链接统一回 workbench，等插件贡献就绪后再选择可用功能，保留显式直达链接。
- 配置管理与投屏各渲染一份工具栏时会重复挂载确认框；Console 保留单一弹窗宿主，管理页工具栏使用同一上下文但关闭自己的 dialogs，避免重复弹窗或在市场区丢失弹窗。
- 合并插件市场与管理清单不能直接拼接数组：按插件 ID 取并集、市场多版本选最新条目，保留本地独有插件；管理快照读取失败时显示“状态未确认”，不能因空数组误报“未安装”。
- 配置工具栏的 `.pkg-menu .btn` 比公共主按钮规则优先级更高：统一透明背景会让黄色按钮丢失底色而保留深色字；管理工具栏为 primary 显式设置底色、边线和文字，并检查实际计算颜色。
- 日志轮询、筛选和清空会产生交错响应：请求序号只允许最新响应回填，清空成功及组件卸载时使旧请求失效。滚动事件只记录是否贴近底部，不能在用户上翻时直接调用滚底。
- 原生 confirm 换自定义框后会返回 Promise：遗漏 await 会直接越过草稿/权限确认；调用链统一异步等待，配置选择在确认前恢复原值，组件销毁取消未决请求，确认后复核目标未变化。beforeunload 属于浏览器管理，不能用异步自定义框替代。
- 录制历史列表返回空体 HTTP 404：前端已热更新但日常后端仍运行旧二进制；核对 `/api/system/info` 与进程构建时间，无活动脚本/录制时重建并重启后端。列表 404 显示更新指引，不能按单会话 `recording_not_found` 静默当作空列表。
- Windows `cargo rustc -- -o <name>.exe` 在多种输出类型下可能生成带哈希后缀的文件名；替换前先核验实际产物路径，PowerShell 设置 `ErrorActionPreference=Stop`，复制后核对哈希，避免复制失败后再次启动旧服务。

- 模板列表底部的“更多”菜单被截断：绝对定位菜单仍受列表 `overflow:auto` 裁切，提高 z-index 无效；菜单 Teleport 到 body 后用视口坐标定位，底部不足时向上展开，滚动/缩放时关闭。

- 添加步骤列表首次滚动突然变矮：定位函数直接清空 Vue 管理的 `style.maxHeight`，同值更新不会回写，导致高度从视口可用值回落到 CSS 480px；高度统一由响应式 style 限制到 480px/可用空间，菜单内部 scroll 不重新定位，禁止手动清空受控样式。

- **2026-09-20 ADB 统一（覆盖早期按型号猜 serial 的记录）**：设备地址必须为完整 ADB serial 或明确 host:port；切换传输方式后重新扫描或显式更新地址，不再按型号、名称、子串兜底，也不再保存 kind。虚拟显示能力独立验证，失败不自动改镜像模式。
- **2026-09-20 插件拆分**：WIT 路径相对各 guest crate，移到 plugins/<id>/guest 后应指向 ../../../server/wit/...；修改后同时验证 guest 构建和宿主测试。ui.host 模块与宿主共享执行环境，不能当沙盒 iframe 使用；更新插件 UI 需先保存再刷新页面。
- **2026-09-20 旧插件 ID 转换**：仅在服务停机后应用 tools/convert-plugin-ids.py，先演练并保留整目录备份；Windows 下 SQLite backup 连接必须显式关闭，否则原子目录移动会因文件占用失败。不要对用户 YAML 或业务 payload 做全文字符串替换。
- **目录清理**：release/dist 和被忽略目录也可能含实际脚本、模板与数据库，不能直接按 ignored 全删；先核对内容和运行进程。保留运行中的服务文件后可清理旧构建及增量缓存，下次编译会重建；构建产物存在硬链接时，文件长度合计不等于实际释放磁盘空间。
- **VitePress 文档站**：主题配置中的函数会序列化到客户端，不能引用配置文件外层变量，否则 SSR 报 ReferenceError；函数保持自包含。重新构建后重启 preview，避免旧路由映射引用已替换的资源。
- **发布工作流缩进**：Bash heredoc 结束标记仍须保留 YAML run 块的缩进，解析 YAML 后才由 Bash 识别；删除整段发布逻辑后同时做 YAML 解析，不能只检查脚本文本关键词。
- **Git 遗留锁**：长时间不变的空 index.lock 会阻止暂存；先核对 Git 进程与文件时间并确认可独占打开，再仅删除已确认失效的锁，不能直接清理活跃锁。
- **Linux Rust 测试模块路径**：内联模块中的 path 属性经不存在的目录再使用 ../ 回退，在 Windows 可解析而 Linux 会 ENOENT；将子测试放到实际的 service/tests/ 目录并使用普通 mod 声明。
- **前端版本检查**：配置包默认版本不等于产品版本；仅允许具名 PACKAGE_INITIAL_VERSION/PACKAGE_EMPTY_VERSION 常量声明，其他同文件产品版本字面量仍须拒绝。
- **平台条件编译**：Windows 检查不会解析 Unix 分支中的类型；目录同步使用 fs::File 全限定路径，避免仅在 Linux CI 出现未导入 File，平台专用改动须核对对应平台构建。
- **媒体测试 CI 依赖**：干净 Ubuntu runner 未必自带 FFmpeg；CI 显式安装并检查 ffmpeg/ffprobe，本地门禁提前检查 PATH。路径派生测试用当前平台的 Path 组件构造输入，Windows 盘符与反斜杠断言仅在 Windows 执行，生成测试视频失败必须报错而非当作通过。

- **视频工作流状态断链**：录制历史不能因状态 completed 就假定视频/事件仍存在；以 missing_media 与 events_available 展示可用动作，零事件不跳草稿，项目用真实分段关系关联录制。
- **草稿页签切换丢编辑**：条件卸载 VideoDraft 会丢失本地选择和文本；首次访问后保留实例，隐藏时停止发布状态条，首次带来源挂载自动加载，录制来源切换仍走未保存保护。
- **暂停视频的模板匹配来源**：模板列表与步骤测试必须消费 StageSource；媒体模式先冻结确定帧，再用 media_id + frame_index 调视觉接口，不能仍传 device_id 匹配实时设备。播放、跳转或切换来源后丢弃旧匹配结果。
- **视频结束截帧越界**：播放器 duration 常大于最后一帧 PTS；不能直接把 currentTime 当作“首个 PTS ≥ 请求值”的取帧参数。先查帧表，选播放位置之前的最近帧，越过末帧时夹到末帧，再按索引截帧；后端 PTS 查询契约不变。
- **录制重复素材**：第一段应复用起录时创建的媒体目录；未完成的 importing 元数据不进入素材库。已有空占位目录可能存有操作事件，不能为去重直接删除；仅在用户明确删除历史且实际视频已删除后清理。
- **步骤整行展开的事件冒泡**：把标题行作为展开热区时，运行/复制/删除、拖动手柄及编辑正文必须阻止点击冒泡，避免操作控件时误折叠；标题使用原生按钮同步支持键盘展开。
- **视频控制栏与画面手势**：底栏必须位于画面坐标容器外，避免挤压画面后框选坐标错位；手势只在媒体模式接管，框选/取点优先，来源变化/失焦/卸载清理全局拖动监听，全屏时包含底栏。
- **视频长按快捷键**：播放器独占焦点时才处理空格/J/K/方向键；连续跳转自行调度并忽略浏览器 repeat，空格不连发，松键/失焦/隐藏/换素材/卸载清除计时器，编辑和弹窗不应被后台长按影响。
- **新增原生参数未显示**：编辑器“更多参数”来自后端函数 Schema；修改注册表并通过测试后，仍需构建并重启实际服务，再刷新页面以清除函数目录缓存。
- **暂停视频匹配随位置变慢**：旧流程前后端重复抽帧且按 index 从片头解码；交互匹配改用浏览器当前帧 PNG 直传（原始尺寸、10MiB 上限），跳转/播放/换素材后丢弃旧结果，不冒充服务端精确帧身份。
- **rebuild 后反复登录**：仅保存在进程内的会话随重启丢失；现在会话摘要持久化到数据目录，登录页可保持登录 30 天。开发密码哈希每次启动使用随机盐，恢复会话须核验旧哈希对应的密码，不能直接比较哈希字符串。
- **wait_find 障碍模板**：同轮障碍和目标共用一帧，命中首个障碍就点击并等下一轮重新取帧；障碍不继承目标 region，清障碍耗时计入总 timeout，click=false 只禁止目标点击。
- **模板列表控件不一致**：列表项必须复用普通模板 CellEditor，避免原生 datalist 缺少缩略图、拼音搜索、框选和匹配；列表表达式与单值 Cell 之间需保留引用及 `$` 转义语义。
- **模板列表首次显示成多行文本**：必填参数初始化为 `missing` 占位而非数组；模板列表控件需识别占位状态，展示添加模板入口并保留必填提示，不能靠切换引用初始化或自动写入空数组。
- **模板分支编辑与执行**：`match_templates` 的 case `as` 仅在命中分支内生效，动作仍由 YAML 解释器执行；分支重排必须保留子步骤对象身份，否则撤销先前的子步骤编辑会修改已脱离模型的副本。运行路径统一为 `run[n].cases[i].do[j]`。
- **break 作用域**：只能由最近一层 repeat 消费中断信号，if/模板分支需向上传播并恢复局部别名；脚本和函数分别校验，函数边界拒绝逃逸的 break，不能提前终止调用方循环。更新此语法须同时更新宿主与 WASM 插件。
- **函数步骤跳转**：跳转和返回先保存完整文档，保存冲突不切换，目标加载成功后才压入历史；历史保存函数名和步骤路径，重新解析会生成新 UUID，不能只按旧 UUID 恢复选择。跨自动化/函数面板通过宿主 workspace.openPanel 导航。
- **插件 UI 导入宿主模块**：源码单测能直接导入不代表插件可打包；新增宿主导出需同步 `sdk/ui/host-modules.json` 白名单，工作区导航使用宿主提供的同一个 `WORKSPACE_CONTEXT_KEY`，避免重复打包 Symbol 导致注入失效。
- **顶部运行按钮无反应**：Vue 的 `@click="run"` 会将 MouseEvent 传给可选的起始步骤参数，找不到对应 UUID 后静默返回；从头运行必须显式调用 `run()`，步骤运行仍显式传 UUID，回归需实际点击按钮而非直接调用处理函数。
- 修改服务端旧配置项却不影响执行：`interval`、`log_level`、`encoder_name`、`judge_delay_ms`、`update.check_url` 与旧 `[op_templates]` 均无运行时消费者，已清理定义及发行模板；自动化时序/阈值使用函数参数，服务日志过滤使用 `RUST_LOG`。

- 设置页修改运行参数后页面仍显示旧默认值：函数目录原先在 Console 生命周期内只加载一次；进入/激活编辑器时重新拉取目录，自动化 timeout 在运行开始冻结，显式参数优先，登录空闲期限也按会话冻结以免热更新使已有登录突然失效。


- 运行详情通过 REST 按 run_id 增量读取持久化事件。不要重新依赖 WebRTC viewer 或设备+起始时间筛选日志，否则后台运行、快速完成和刷新会漏记录。native `log` 必须附设备；原生匹配/输入事件经 TracedSink 继承当前调用帧与源位置。
- SQLite v5 增加 run_records/run_events。RunManager 在提交前保存记录、统一 finalize 保存终态；服务启动恢复未完成历史为中断。事件写入先于可选 viewer 推送，保留策略按已完成运行整组清理。
- 切换脚本/函数后找不到运行日志：日志本体绑定 run_id；历史入口应按设备枚举并共享该设备的所选 run_id，不能随编辑目标清空。历史列表每页 30 次，使用 before 游标向前加载，刷新最新页须合并而非覆盖已加载历史。
- 已选脚本/函数切回面板后仍提示打开编辑器：KeepAlive 面板共享一个编辑模型，需在激活或选择变化时恢复所选资源；隐藏面板不得抢加载，显式跳转/保存期间不重复加载，已有脚本 ID 可直接读取而不等待列表。

- Vue Test Utils 的 Teleport stub 更新时可能替换弹窗节点，搜索后旧 DOMWrapper 仍指向旧清单；交互后从根 wrapper 重新查询弹窗节点再断言。

- Windows PowerShell 5.1 会按系统编码读取无 BOM 的中文脚本和 JSON，导致假性语法/JSON 错误；发行 `.ps1` 使用 UTF-8 BOM，注册表 JSON 用 `ReadAllText(..., Encoding.UTF8)` 显式读取。
- 完整包禁止复制开发机 `server/data`：子目录也含账号、插件状态和业务数据；只携带签名清单声明的 seeds，数据目录由首次启动初始化。
- 安装后再次启动重复校验：缓存若按 staging 路径寻址，同卷 rename 后会失效；改按卷号/文件 ID 与预期哈希寻址，并继续比较大小和修改时间，文件替换仍重新校验。
- HTTP 续传收到 206 也不代表可直接追加：除 Content-Range 外还需比对返回的强 ETag，验证器变化时拒绝拼接。
- 原生 GUI 的 Windows 测试不能把启动后立即读取 Visible 当最终状态：隐藏窗口的逻辑按 repaint 唤醒，重复启动后的窗口恢复需等待一次事件循环，再断言实例数与可见性。
- Windows 正在运行的 Rust 测试 EXE 无法被链接器覆盖：同一 target 下的测试运行和重新编译需串行；Permission denied 不应通过杀掉所有同名进程解决。
- 旧性能夹具的低方差灰度块由 FFmpeg 生成，与当前 image 灰度公式有舍入差异；NCC 必命中基准从固定 RGB 孪生按产品流程预先生成灰度模板，转换不计入匹配计时。

- 开发脚本按进程名或端口兜底会误认领发行版 Gamer/其他项目 Vite；按本仓库 EXE 与 Vite 完整入口路径识别，发送停机请求前核对端口归属，其他 Gamer 仍运行时跳过共享 ADB 重置。
- 临时 Vite 预览以整个仓库为 root 会监听 release/dist 内新建的安装 staging，Windows 目录句柄导致本体切换报 os error 5；预览完成即退出，监听范围排除发行目录，持续占用需关闭来源，短暂占用由启动器有界重试（首次失败不得误报“旧目录已恢复”）。
- eframe 首帧可能强制显示窗口，单设 ViewportBuilder.visible=false 仍会在启动检查时弹窗；托盘就绪后在 quiet_start 阶段持续发隐藏命令，进入安装/更新/错误或用户主动唤回时才解除。
- Windows Get-Process.MainWindowHandle 在主窗口隐藏后可能指向托盘的 1×1 辅助窗口，导致“窗口可见”误判；验收按进程 ID 和 Gamer 主窗口标题匹配句柄，再检查 IsWindowVisible。
- PowerShell 脚本未调用原生命令时 `$LASTEXITCODE` 可能为空或保留旧值，不能据此判定脚本失败；脚本用终止异常传播失败，原生命令才在调用后立即检查退出码。
- 插件仓搬迁后 pnpm 的 node_modules 记录仍指向原目录，非交互安装会拒绝重建；保留原目录备份，在验收子进程设置 CI=true 后按冻结锁文件重建依赖，不修改用户全局 pnpm 设置。
- Windows 用户临时目录会继承用户目录祖先的 .cargo/config.toml，即使 CARGO_HOME 指向别处；独立克隆缺锁定版本时先查实际 source replacement，本机 C 盘镜像与 D 盘构建源不同，不能为镜像缺包修改产品锁文件。

## 2026-09-24：发行签名移除必须覆盖整条链路

启动器、Node 清单校验器、生成/打包脚本、CI、SDK 快照和演练脚本必须同时移除 `.sig`/公钥依赖，否则本地启动通过而发行仍失败。当前使用 GitHub HTTPS + SHA256；远端禁 HTTP，HTTPS 禁降级重定向。本地 loopback 测试仍可用 HTTP。旧密钥和历史演练证据不代表当前发布要求。

- Windows 的 ADB daemon 会继承启动目录；即使 Gamer 已退出，旧 daemon 仍可能占用 `versions/<版本>` 导致修复 rename 报 os error 32。本次验收核对 EXE 属于目标安装、确认无 Gamer 运行后重启该 ADB，并从稳定安装根启动，修复即通过；不能按进程名批量终止其他安装。
