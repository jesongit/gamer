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
- 帧缓存 `FrameCache`（ffmpeg 软解）：提供截图/模板匹配取帧，并为**新 viewer 重放初始帧（SPS/PPS + 最近 GOP）**。ffmpeg 不可用 → 无初始帧 → 浏览器黑屏（见已知坑）
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
| `server/src/device/frames.rs` | ffmpeg 帧缓存（PNG 输出 + GOP 维护） |
| `server/src/device/mod.rs` | DeviceManager：连接生命周期 / 状态 / 广播 |
| `server/src/store.rs` | SQLite 持久化 |
| `server/src/engine.rs` / `matcher.rs` / `scheduler.rs` | YAML 脚本引擎 / NCC 模板匹配 / cron 调度 |
| `web/src/views/DeviceList.vue` | 设备列表：刷新(scan) / 连接 / 删除 |
| `web/src/views/Console.vue` | 投屏控制：WebRTC 前端（连接锁防双 PC / 坐标映射 / 框选模板） |

## 已知坑

- scrcpy 视频 socket 必须保留整个 `TcpStream`（`into_split()` 写半 drop 会发 FIN 导致断流）
- `max_fps=0` / `max_size=0` 等 0 值参数不能传给 scrcpy server
- `config.toml` 的 `ffmpeg_path` 失效 → 帧缓存启动失败 → 新 viewer 无 SPS/PPS 可重放 → **连接后黑屏**（日志特征：`frame cache unavailable`，且无 `pusher replayed initial GOP`）
- 控制 DataChannel 必须由 offerer（浏览器）创建；webrtc-rs answer 只镜像 offer 的 media section
- 模板匹配把截图/模板等比缩放到最长边 540px 加速，命中坐标需映射回原图
- 设备配置（PUT /api/devices/:id）会触发断开重连；删除设备不停止 adb 物理连接，scan 会重新入库
- 多个浏览器页面同时操作同一设备会互踢 WebRTC：已用页面锁 + 自动重连收敛（见关键链路）；调试时注意 chrome-devtools-mcp 的 `select_page` 的 `pageId` 必须是 number 且选中状态不跨 MCP 连接保留
