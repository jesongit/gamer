# AGENTS.md

## 项目

GameBot 游戏自动化助手：Rust 服务端（axum + webrtc-rs）+ Vue3/Vite 前端。
scrcpy 采集 Android 设备画面 → WebRTC（H.264 视频轨 + DataChannel 控制）推流到浏览器，
支持触控控制、模板匹配（NCC）、YAML 自动化、cron 定时任务。

## 目录与端口

- `server/` — Rust 服务端，监听 **8443**，静态托管 `web-dist/`（构建产物）
- `web/` — Vue3 + Vite，dev 监听 **5173**，`/api`、`/ws` 代理到 8443；路由为 hash 模式
- `server/config.toml` — 关键项：`adb_path`、`ffmpeg_path`、`scrcpy_server`、`password`（默认 admin/admin123，前端无鉴权拦截，token 仅本地标记）
- `server/data/gamer.db` — SQLite（设备/脚本/任务/日志）；`data/templates/` 模板图片

## 常用命令

```powershell
.\gamer.ps1 start|stop|restart|status   # 前后端一起管；-BackendOnly / -FrontendOnly / -Build / -Release
cd server && cargo run                  # 单起后端（日志 GB_LOG 追加到 server/gamer-server.log）
cd web && npm run dev                   # 单起前端
```

## 关键链路（改代码前先看）

- 连接：浏览器 → `POST /api/devices/:id/connect`（scrcpy 会话）→ WS `/ws/device/:id` 信令（offer/answer）→ WebRTC 视频轨 + DataChannel `control`（触控/按键/文本）
- 帧缓存 `FrameCache`（帧环 + 按需解码）：截图/模板匹配时用临时 ffmpeg 解**最新一帧**（天然实时，无陈旧/停滞问题），并为**新 viewer 重放初始帧（SPS/PPS + 最近 GOP）**。ffmpeg 不可用 → 无初始帧 → 浏览器黑屏（见已知坑）
- 单设备单 viewer：新连接踢旧连接（`AppState.viewers` 注册表）
- 视频静默 20s → 看门狗自动重连 scrcpy 会话并踢 viewer
- 多页面互斥：`Console.vue` 用 localStorage 锁（`gb_webrtc_lock`，15s TTL + 8s 心跳）保证同一设备只有一个页面持有连接；被踢/断流（onclose 或视频静默 ~4s 检测）自动重连，但延迟后二次检查锁——他人持锁则放弃并提示（防互踢死循环）；用户手动点连接才强制抢锁
- 配置变更（PUT /api/devices/:id）会踢该设备 viewer（pusher 停 + peer close），前端 onclose → 自动重连恢复画面
- 设备扫描：`POST /api/devices/scan` 执行 `adb devices -l`，按 addr 去重自动入库

## 关键文件

| 文件 | 职责 |
|---|---|
| `server/src/api/mod.rs` | REST：设备 CRUD / scan / connect / control / 截图 / 模板 / 脚本 / 任务 / 日志 |
| `server/src/api/ws.rs` | WebRTC 信令；取 `frame_cache.initial_frames()` 传给 ViewerSession |
| `server/src/webrtc.rs` | pusher 推流 / 初始 GOP 重放 / 静止补帧 / DataChannel 控制转发 |
| `server/src/device/scrcpy.rs` | scrcpy 会话：视频/音频/控制 socket 协议（v3.3.3） |
| `server/src/device/frames.rs` | 帧缓存：帧环（SPS/PPS + GOP）+ 按需解码截图 |
| `server/src/device/mod.rs` | DeviceManager：连接生命周期 / 状态 / 广播 |
| `server/src/store.rs` | SQLite 持久化 |
| `server/src/engine.rs` / `matcher.rs` / `scheduler.rs` | YAML 脚本引擎 / NCC 模板匹配 / cron 调度 |
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
- 设备配置（PUT /api/devices/:id）会触发断开重连；删除设备不停止 adb 物理连接，scan 会重新入库
- 多个浏览器页面同时操作同一设备会互踢 WebRTC：已用页面锁 + 自动重连收敛（见关键链路）；调试时注意 chrome-devtools-mcp 的 `select_page` 的 `pageId` 必须是 number 且选中状态不跨 MCP 连接保留
- `gamer.ps1 restart` 只停/启现有二进制**不重新编译**：Rust 代码或 config.toml 改动后须 `rebuild`（或 `restart -Build`），否则跑旧 exe、新接口表现为 HTTP 405
- `gamer.ps1` 必须保持 UTF-8 **BOM + CRLF** 编码：被无 BOM/LF 重写后 Windows PowerShell 5.1 解析报错（报错行号与实际不符）
- PowerShell（5.1 与 7.3+）在 `$ErrorActionPreference='Stop'` 下把 cargo/npm 的 stderr 输出当错误中断脚本；gamer.ps1 的 `Invoke-NativeChecked` 构建期间临时切回 Continue，成败只看 `$LASTEXITCODE`（注意：`$PSNativeCommandUseErrorActionPreference` 是 7.3+ 变量，5.1 无效）
- 虚拟屏实际分辨率/方向会被游戏改变（如 1920x1080 ↔ 1440x3200），**设备配置里的 width/height 会过期**：前端换算测试区域像素必须优先用实际视频尺寸（`videoElement.videoWidth/Height`），相对坐标（模板名 #x1_y1_x2_y2、region fm/to）本身与分辨率无关
- 帧缓存 = **帧环 + 按需解码**（`frames.rs`）：只缓存 SPS/PPS + 最近 GOP（feed 时纯内存拷贝），截图/匹配时用**临时 ffmpeg** 把最新一帧解码成 PNG（`-vf select=gte(n\,N)` 取 GOP 最后一帧，`-frames:v 1` 只出一张图）——每次全新解码天然实时，**不要做常驻解码 PNG 流**（旧设计：常驻 ffmpeg 软解会静默冻结，截图/匹配永远拿到旧画面，还得加代数/新鲜度/健康检查兜底）。select 的帧索引含 demuxer 对配置帧的计数偏移（±1~2 帧 ≈ ≤100ms 旧，可接受）；解码失败=分辨率切换窗口（新 SPS 已到、新 IDR 未到，config 与旧 GOP 不匹配）→ 清空 GOP 等下一个 IDR 重试一次
- 虚拟屏设备截图**不可静默回退物理屏**（内容/分辨率不同，模板匹配会拿到主屏竖屏数据）：帧缓存按需解码 → adb 虚拟屏 screencap → 直接报错；注意本机 Xiaomi 15 Pro 的 `adb screencap -d` 对 scrcpy 虚拟屏返回非法图（~80B），截图只能靠帧缓存解码
- 挂机（静止画面）时投屏延迟单调累积到秒级（实测 87ms → 3s+ 且不回落）→ 三个叠加根因：① **音频轨参与浏览器 A/V 同步（主时钟），scrcpy 虚拟屏音频流在 Chrome 侧播放时钟异常**（对照实验：`audioTrack.enabled=false` 后停止累积）；② **帧级 burst**——设备 60fps 固定编码（虚拟屏 `max_fps` 不生效）、USB/WiFi 批量到达，pusher 批量全速连发，浏览器 jitter buffer 目标延迟被顶高；③ **关键帧 burst**——`i-frame-interval` 越短扰动越频繁（1s 时实测 perF 缓慢爬升）。服务端 RTP ts 精确 1.0x、网络零丢包、硬解正常。修复：前端静音时禁用音频轨（`toggleAudio`/`ontrack`）+ 延迟看门狗（>1500ms 自动重连）+ 服务端统一帧级 pacer（16ms 固定节奏，`webrtc.rs`）+ 关键帧发送平滑（中间一次 8ms 分批发，总耗时须 < pacer 间隔，否则每秒净积压）+ `i-frame-interval=2`；修复后挂机延迟稳定 140~250ms（残余 = 无线传输 + Chrome jitter buffer 保守目标 ~100-150ms）
- 用 PowerShell `Invoke-WebRequest` 计时大响应接口（MB 级 PNG）会虚高到秒级（客户端缓冲开销），计时用 `curl.exe -w`
