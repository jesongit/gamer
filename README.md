# GameBot 游戏自动化助手

基于 ScrcpyOverWebRTC 方案的轻量自研游戏自动化系统：
**官方开源 scrcpy-server** 采集与控制 + **自研 Rust 服务端** + **WebRTC 低延迟投屏** + **模板匹配 / YAML 自动化 / 定时任务**。

## 特性

- 🖥️ **统一分辨率虚拟屏**（scrcpy new-display）：所有设备可用相同分辨率（如 1920x1080）游玩，
  一套模板通吃所有设备，彻底解决模板匹配兼容性问题；也支持镜像主屏模式
- ⚡ **低延迟控制**：浏览器 → WebRTC DataChannel → 服务端 → scrcpy 控制 socket → 设备，局域网 <10ms
- 🎞️ **流畅画面**：H.264 视频轨经 WebRTC 转推浏览器，不转码零画质损失
- 🔍 **模板匹配**：Rust NCC 引擎（截图优先取自视频流软解码帧缓存，<50ms；fallback adb screencap）
- 📜 **YAML 自动化**：find / click / swipe / text / key / start_app / loop / if / goto / call / random_delay
- ⏰ **定时任务**：cron 表达式，服务端 Docker 内 7×24 运行，浏览器关闭不影响
- 📱 **多设备接入**：redroid 容器 / USB 直连 / 无线 adb / Windows 模拟器

## 架构

```
┌────────────┐  WebRTC (H.264 视频轨 + DataChannel 控制)   ┌──────────────────┐  adb / scrcpy socket   ┌──────────────┐
│   浏览器    │ ◄────────────────────────────────────────► │  Rust 服务端      │ ◄────────────────────► │ Android 设备  │
│ (Vue3 精简) │       WebSocket 信令 + HTTP REST API       │ (axum+webrtc-rs) │                        │ redroid/真机  │
└────────────┘                                            └──────────────────┘                        └──────────────┘
                                                                    │
                                                                    ├─ 自动化引擎：YAML 脚本解释器
                                                                    ├─ 模板匹配：NCC + 帧缓存（ffmpeg 软解）
                                                                    ├─ 定时任务：cron + tokio 调度
                                                                    ├─ 设备管理：adb 直连
                                                                    └─ 持久化：SQLite + 模板图片
```

- **scrcpy-server**：官方开源 jar（锁定 v3.3.3，`server/assets/scrcpy-server.jar`），服务端以 scrcpy
  客户端角色驱动：`adb push` → `adb reverse` 隧道 → `app_process` 启动 → 读视频 socket（H.264 帧 + PTS 头）/
  控制 socket（触控/按键/文本/剪贴板/启动应用）
- **虚拟屏**：启动参数 `new_display=1920x1080/420`，scrcpy server 在设备上创建虚拟显示器；
  游戏通过 StartApp 控制消息（type 16）自动启动到虚拟屏，**无需自己探测 display id**

## 目录结构

```
gamer/
├── server/                 # Rust 服务端
│   ├── src/
│   │   ├── main.rs         # 入口
│   │   ├── config.rs       # 配置（port / data_dir / adb / scrcpy-server / 阈值）
│   │   ├── api/            # HTTP REST + WebSocket 信令
│   │   ├── device/         # adb 封装 + scrcpy 会话 + ffmpeg 帧缓存
│   │   ├── webrtc.rs       # WebRTC peer（H.264 推流 + DataChannel 控制）
│   │   ├── matcher.rs      # NCC 模板匹配引擎
│   │   ├── engine.rs       # YAML 脚本解释器
│   │   ├── scheduler.rs    # cron 定时任务
│   │   └── store.rs        # SQLite 持久化
│   ├── assets/scrcpy-server.jar   # 官方 v3.3.3（构建时下载）
│   └── Dockerfile          # 多阶段构建，内置 adb + ffmpeg
├── web/                    # Vue3 + Vite 前端（精简版）
├── docker-compose.yml      # server + redroid 一键拉起
└── docs/                   # 文档
```

## 快速开始

### 方式一：Docker 一键部署（推荐）

```bash
# 1. 构建服务端镜像
cd server && docker build -t gamer-server .

# 2. 拉起服务端 + redroid 云手机
cd .. && docker compose up -d          # 含 redroid 云手机
docker compose --profile redroid up -d # 仅服务端
```

- 访问 `http://<服务器IP>:8443`，默认账号 `admin / admin123`
- redroid 容器启动后，在「设备列表」添加设备：类型 redroid、地址 `redroid:5555`、
  屏幕模式虚拟屏 `1920x1080`、游戏包名填你的游戏

### 方式二：本地开发

```bash
# 服务端
cd server
cargo run

# 前端（开发热更新，代理到 8443）
cd web
npm install
VITE_PROXY_TARGET=http://localhost:8443 npm run dev
# 打开 http://localhost:5173
```

## 设备接入

| 方式 | 设备配置 | 说明 |
|---|---|---|
| redroid 容器 | 类型 `redroid`，地址 `redroid:5555` | Docker 内 Android，与服务端同网 |
| USB 直连 | 类型 `usb`，地址留空 | 需容器 `--device /dev/bus/usb` 直通 |
| 无线 adb | 类型 `wifi`，地址 `192.168.x.x:5555` | 手机开启无线调试 |
| 模拟器 | 类型 `emu`，地址 `127.0.0.1:7555` | MuMu/雷电等 adb 端口 |

**屏幕模式**：
- `镜像主屏`：投物理屏幕，各设备分辨率不同
- `虚拟屏`：统一分辨率（预设 1920x1080 / 1080x1920 / 1280x720，可自定义宽高+DPI），
  需 Android 10+，连接后自动把配置的游戏包名启动到虚拟屏（`+` 前缀可 force-stop 后启动）

## YAML 脚本语法

```yaml
name: 每日签到
device: auto

steps:
  - wait: 2000                          # 等待毫秒
  - random_delay: {min: 500, max: 1500} # 随机延时（模拟人工）

  # 找图：截图（帧缓存）→ 模板匹配；命中坐标存 @found
  - find: {template: sign_btn.png, timeout: 10000, threshold: 0.85, region: [0, 0, 1080, 1920]}
    then:
      - click: "@found"
    else:
      - log: "未找到签到按钮"
      - goto: retry

  - click: {x: 540, y: 1680}            # 点击坐标
  - swipe: {from: [500, 1800], to: [500, 600], duration: 800}
  - text: "hello world"                 # 输入文本
  - key: HOME                           # HOME/BACK/APP_SWITCH/VOL_UP...
  - start_app: {package: "com.game.xxx"} # 启动到当前虚拟屏

  - loop: {times: 3, steps: [...]}      # 次数循环
  - loop_until_find: {template: done.png, timeout: 30000, steps: [...]}
  - if_find: {template: x.png, then: [...], else: [...]}
  - label: retry
  - call: 子脚本.yml                    # 子流程
  - log: "输出到运行日志"
```

模板文件放在 `data/templates/`（web 端「模板管理」页上传/测试）。

## API 一览

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | /api/login | 登录 |
| GET/POST | /api/devices | 设备列表 / 创建 |
| POST | /api/devices/scan | 扫描 `adb devices -l` 并自动注册新设备（前端"刷新"时调用） |
| PUT/DELETE | /api/devices/:id | 更新配置（变更后自动重连）/ 删除 |
| POST | /api/devices/:id/connect | 连接设备 |
| POST | /api/devices/:id/screenshot | 截图（PNG） |
| POST | /api/devices/:id/control | 手动控制（tap/swipe/text/press/home/back/recents/start_app/rotate/clipboard） |
| GET/POST | /api/templates | 模板列表 / 上传 |
| POST | /api/templates/:name/test | 测试匹配 |
| GET/POST | /api/scripts | 脚本列表 / 保存 |
| POST | /api/scripts/:id/run / stop | 运行 / 停止脚本 |
| GET/POST | /api/tasks | 定时任务列表 / 保存 |
| POST | /api/tasks/:id/run | 立即执行 |
| GET/DELETE | /api/logs | 运行日志 / 清空 |
| WS | /ws/device/:id | WebRTC 信令（offer → answer） |

## 技术要点

- **scrcpy 协议**（对齐 v3.3.3）：视频 socket 先 64B 设备名 + 12B codec meta
  （codec_id+width+height），随后 12B 帧头（pts_and_flags u64 + size u32）+ 负载；
  控制消息类型 0~22（keycode/text/touch/scroll/clipboard/start_app 等），大端序
- **WebRTC**：webrtc-rs 服务端 peer，H.264 Annex-B 帧经 H264Payloader 打包 RTP 推流
  （SPS/PPS 自动 STAP-A），DataChannel `control` 接收浏览器指令
- **WebRTC 快速出画面（关键帧策略）**：
  - scrcpy 启动参数 `video_codec_options=i-frame-interval=2` 强制编码器每 ~2s 产 IDR
  - 服务端帧缓存维护**最近完整 GOP**（自最近 IDR 起的所有帧），
    新 viewer 连接时 pusher **先重放 GOP**（config+IDR+P 帧，RTP 时间戳与实时流同一时间轴），
    浏览器无需等待下一个 IDR 即可开始解码——静态画面下也能立即出画面
- **帧缓存**：ffmpeg 软解视频流缓存最新帧 PNG，模板匹配/截图 <50ms；
  无 ffmpeg 时自动降级 `adb exec-out screencap -p`
- **设备自动发现**：前端"刷新"会调用 `/api/devices/scan`（`adb devices -l`），
  自动注册未入库的设备（USB/无线/模拟器自动识别类型与型号，默认镜像模式），
  已注册设备跳过；注册后可在设备列表 ✏️ 编辑为虚拟屏等配置
- **已知坑**：
  - tokio `TcpStream::into_split()` 的写半在 drop 时会发送 FIN（`shutdown_on_drop`），
    会导致 scrcpy server 关闭连接——视频 socket 必须保留整个 TcpStream
  - `max_fps=0` / `max_size=0` 等 0 值参数不要传给 scrcpy server（与官方客户端行为一致）
  - 模板匹配引擎会把截图与模板等比缩放到最长边 540px 加速，命中坐标会映射回原图

## 真机联调记录（2026-08-16，红米 25079RPDCC / Android 16）

### 镜像模式（display_id=0）
- ✅ 无线 adb（mDNS serial）设备接入 + scrcpy 会话建立，H.264 视频流持续稳定（60fps）
- ✅ 控制注入：tap / swipe / 文本 / HOME / BACK / 音量按键
- ✅ `start_app` 启动星穹铁道（com.miHoYo.hkrpg）成功
- ✅ 模板匹配：真实游戏画面命中（置信度 0.98 / 0.85）
- ✅ YAML 脚本：find → click(@found) → wait → random_delay 全链路执行并输出日志
- ✅ 定时任务：cron 触发 + 立即执行 + 触发点防重复

### 虚拟屏模式（new_display=1920x1080/420）✅ 重点验证
- ✅ scrcpy-server 以 `new_display=1920x1080/420` 启动，设备端创建虚拟显示器（id=91，FLAG_PRESENTATION）
- ✅ 视频流 meta 为 **1920x1080**（虚拟屏分辨率，非物理屏 3008x1880）
- ✅ 连接后自动 `start_app` 把星穹铁道启动到虚拟屏（`on display 91`）
- ✅ 截图（ffmpeg 帧缓存软解视频流）返回 **1920x1080** 虚拟屏画面
- ✅ 模板匹配在虚拟屏分辨率下工作：命中 (757,401) 置信度 **0.97**
- ✅ 触控注入作用于虚拟屏坐标系（1920x1080 归一化）

> 注意：`adb screencap -d` **无法截取 scrcpy 创建的虚拟屏**（返回 "Display Id not valid"），
> 虚拟屏模式截图必须依赖帧缓存（ffmpeg 软解视频流）。Docker 镜像已内置 ffmpeg。

### WebRTC 画面（2026-08-16）✅ 信令/协商/出画面链路已验证
- ✅ WS 信令：浏览器 `ws://host/ws/device/:id` → 发 `{type:'offer', sdp:{type,sdp}}` → 收 answer
- ✅ H264 协商：`negotiated video: payload_type=96 ssrc=...`（动态协商，非硬编码）；
  answer 含 `m=video 9` + `a=sendonly` + host/srflx 候选 + BUNDLE（端口非 0）
- ✅ 修复"连接后黑屏"：根因是 scrcpy 会话长连接下静态画面长时间无 IDR，浏览器无帧可解码。
  方案：`i-frame-interval=2` 强制周期 IDR + 帧缓存维护最近 GOP、pusher 启动时重放
  （实测日志 `pusher replayed initial GOP: 1 frames, 67096 bytes`，浏览器立即出画面）
- ✅ 前端已接入真实 API + WebRTC：设备列表/控制台画面/触控 DataChannel/模板测试/脚本/任务/日志
- ✅ 设备列表"刷新"自动扫描 adb 设备入库（`POST /api/devices/scan`），设备卡片支持 ✏️ 编辑

## 开源协议

- 本项目：MIT
- scrcpy：Apache-2.0（仅使用其开源 scrcpy-server）
