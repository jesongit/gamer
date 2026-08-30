# GameBot 游戏自动化助手

基于 ScrcpyOverWebRTC 方案的轻量自研游戏自动化系统：
**官方开源 scrcpy-server** 采集与控制 + **自研 Rust 服务端** + **WebRTC 低延迟投屏** + **模板匹配 / YAML 自动化 / 定时任务**。

## 特性

- 🖥️ **统一分辨率虚拟屏**（scrcpy new-display）：所有设备可用相同分辨率（如 1920x1080）游玩，
  一套模板通吃所有设备，彻底解决模板匹配兼容性问题；也支持镜像主屏模式
- ⚡ **低延迟控制**：浏览器 → WebRTC DataChannel → 服务端 → scrcpy 控制 socket → 设备，局域网低延迟
- 🎞️ **流畅画面**：H.264 视频轨经 WebRTC 转推浏览器，不转码零画质损失
- 🔍 **模板匹配**：Rust NCC 引擎（截图优先从 H.264 GOP 帧环按需调用 ffmpeg 解码最新帧；无 ffmpeg 时 fallback adb screencap）；固定夹具 benchmark 脚本已兼容 Windows PowerShell 5.1（parser=0），正式跨平台 p50/p95 报告仍在计划中
- 📜 **YAML 自动化**：当前 v2 严格语法支持 find（找图等待+点击，block 障碍、verify 补点）/ color 颜色分支 / loop / func 自定义函数（具名参数 + return）/ tap / swipe / text / key / call / throw / str_app / cls_app / wait（语法见 [docs/YAML.md](docs/YAML.md)）
- ⏰ **定时任务**：cron 表达式，服务端 Docker 内 7×24 运行，浏览器关闭不影响
- 📱 **多设备接入**：redroid 容器 / USB 直连 / 无线 adb / Windows 模拟器

> 当前代码以无兼容 v2 基线为准，优化与自动升级仍未整体验收；真实设备 DataChannel / WebRTC E2E、生产升级回滚、正式跨平台 p50/p95、多平台基准仍待验证。验收命令以当前提交的 CI/本地门禁结果为准，自动升级设计见 [docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md](docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md)。

## 架构

```
┌────────────┐  WebRTC (H.264 视频轨 + DataChannel 控制)   ┌──────────────────┐  adb / scrcpy socket   ┌──────────────┐
│   浏览器    │ ◄────────────────────────────────────────► │  Rust 服务端      │ ◄────────────────────► │ Android 设备  │
│ (Vue3 精简) │       WebSocket 信令 + HTTP REST API       │ (axum+webrtc-rs) │                        │ redroid/真机  │
└────────────┘                                            └──────────────────┘                        └──────────────┘
                                                                    │
                                                                    ├─ 自动化引擎：YAML 脚本解释器
                                                                    ├─ 模板匹配：NCC + H.264 GOP 帧环（按需 ffmpeg 解码）
                                                                    ├─ 定时任务：cron + tokio 调度
                                                                    ├─ 设备管理：adb 直连
                                                                    └─ 持久化：SQLite + 模板图片
```

- **scrcpy-server**：官方开源 jar（锁定 v3.3.3，`server/assets/scrcpy-server.jar`），服务端以 scrcpy
  客户端角色驱动：`adb push` → `adb reverse` 隧道 → `app_process` 启动 → 读视频 socket（H.264 帧 + PTS 头）/
  控制 socket（触控/按键/文本/剪贴板/启动应用）
- **虚拟屏**：启动参数 `new_display=1920x1080/420`，scrcpy server 在设备上创建虚拟显示器；
  连接不会自动启动应用，由 Console 启动按钮或脚本 `str_app` 显式启动到虚拟屏，**无需自己探测 display id**

## 目录结构

```
gamer/
├── server/                 # Rust 服务端
│   ├── src/
│   │   ├── main.rs         # 入口
│   │   ├── config.rs       # 配置（port / data_dir / adb / scrcpy-server / 阈值）
│   │   ├── api/            # HTTP REST + WebSocket 信令
│   │   ├── device/         # adb 封装 + scrcpy 会话 + ffmpeg 帧缓存
│   │   ├── webrtc/         # WebRTC peer（H.264 推流 + DataChannel 控制）
│   │   ├── script_v2/      # YAML v2 严格装载、校验、序列化
│   │   ├── engine/         # YAML 脚本执行引擎与调度
│   │   ├── device/         # adb/scrcpy 会话与帧缓存
│   │   └── store.rs        # SQLite 持久化
│   ├── data/               # 按应用分区的 yaml/func/tmpl 种子与运行数据
│   ├── assets/scrcpy-server.jar   # 官方 v3.3.3（仓库自带）
│   └── Dockerfile          # 兼容保留：仅后端镜像（无前端页）
├── web/                    # Vue3 + Vite 前端（精简版）
├── Dockerfile              # 推荐：一体化多阶段镜像（pnpm 前端 + Rust 服务端）
├── docker-compose.yml      # server + redroid 一键拉起
└── docs/YAML.md            # YAML 自动化脚本语法（README 引用）
```

## 依赖清单（Windows / scoop 安装示例）

| 依赖 | 用途 | scoop 安装 |
|---|---|---|
| Rust 工具链（stable） | 编译 Rust 服务端 | `scoop install rustup`，或免 VS 的 GNU 工具链 `scoop install rust` |
| Android platform-tools（adb） | 设备发现 / 连接 / 推送 scrcpy-server | `scoop install adb` |
| ffmpeg | 视频软解码帧缓存（截图 / 模板匹配 / WebRTC 初始 GOP 重放） | `scoop install ffmpeg` |
| Node.js ≥ 20（自带 Corepack） | 前端 Vite dev / 构建（pnpm，`package.json#packageManager` 固定版本） | `scoop install nodejs-lts` |
| scrcpy-server.jar | 设备端采集端（v3.3.3，**仓库已自带** `server/assets/`） | 无需安装 |

一键安装示例（PowerShell）：

```powershell
scoop install git rustup adb ffmpeg nodejs-lts   # 或 scoop install rust 替代 rustup
rustup default stable                            # MSVC 工具链需先装 VS Build Tools（C++ 生成工具）
```

> scoop 安装的 `adb` / `ffmpeg` 会自动加入 PATH，`server/config.toml` 的 `adb_path` / `ffmpeg_path`
> 保持默认值 `"adb"` / `"ffmpeg"` 即可；也可写绝对路径。
> ⚠️ ffmpeg 路径失效会直接导致**连接控制后无画面**（帧缓存启动失败 → WebRTC 无法重放
> SPS/PPS + GOP → 浏览器 H.264 解码器无法初始化），详见下方「已知坑」。

## 快速开始

### 方式一：Docker 一键部署（推荐）

```bash
# 1. 构建一体化镜像（必须在仓库根执行：stage1 pnpm 构建前端 → stage2 cargo 编译服务端 →
#    运行时层内置 adb / ffmpeg / scrcpy jar / 前端静态页，无需宿主机先装 Node）
docker build -t gamer .

# 2. 启动服务端。redroid 声明了 profile：默认 up 只启动 gamer 服务端，
#    需要 redroid 云手机时必须带 --profile redroid（会连同 gamer 一起拉起）
docker compose up -d                    # 仅 gamer 服务端
docker compose --profile redroid up -d  # gamer + redroid 云手机

# USB 直连物理设备（Linux 宿主机）时叠加直通 override：
docker compose -f docker-compose.yml -f docker-compose.usb.yml up -d
```

- 访问 `http://<服务器IP>:8443`，使用管理员账号登录。认证凭据只有配置的 Argon2id PHC `[auth].password_hash`，或开发时由 `GAMER_ADMIN_PASSWORD` 在进程内生成；没有凭据时 fail closed，不存在默认账号或默认密码。登录成功后服务端通过 `HttpOnly; SameSite=Strict` Cookie 维护会话，生产部署请通过 HTTPS 反向代理暴露。
- `gamer` 容器默认**不带特权**运行；网络类设备（redroid / WiFi adb / 模拟器）
  无需宿主机特权，USB 直通所需的 device 映射由 `docker-compose.usb.yml` 承载
- **运行数据目录**（唯一口径 = 仓库的 `server/data/`，容器内 `/app/data`）：

  | 内容 | 性质 | 来源 |
  |---|---|---|
  | `<应用包名>/tmpl/` `<应用包名>/yaml/` `<应用包名>/func/` | 种子数据 | 随仓库分发（git 跟踪），**不在镜像内** |
  | `gamer.db` | 运行期持久化 | 首次启动自动生成（gitignore） |
  | 其他临时文件 | 运行期产物 | 自动创建（gitignore） |

  镜像不含业务数据、不声明 VOLUME 匿名卷，compose 的绑定挂载不会遮蔽种子分区；
  自定义服务端配置时把本地 `config.toml` 挂到容器 `/app/config.toml`
  （镜像未内置配置文件，缺省走程序默认值）。本机 `cargo run` 与容器不要同时
  使用同一数据目录——同一 SQLite 库被两套进程并行打开有损坏风险。
- redroid 容器启动后，在「设备列表」添加设备：类型 redroid、地址 `redroid:5555`、
  屏幕模式虚拟屏 `1920x1080`、游戏包名填你的游戏

### 方式二：本地开发

```bash
# 服务端
cd server
cargo run

# 前端（开发热更新，代理到 8443）
cd web
corepack enable pnpm    # 首次使用执行一次；Corepack 按 packageManager 字段自动用对 pnpm 版本
pnpm install
VITE_PROXY_TARGET=http://localhost:8443 pnpm dev
# 打开 http://localhost:5173
```

## 设备接入

| 方式 | 设备配置 | 说明 |
|---|---|---|
| redroid 容器 | 类型 `redroid`，地址 `redroid:5555` | Docker 内 Android，与服务端同网 |
| USB 直连 | 类型 `usb`，地址留空 | 容器场景需叠加 `docker-compose.usb.yml` 直通 `/dev/bus/usb` |
| 无线 adb | 类型 `wifi`，地址 `192.168.x.x:5555` | 手机开启无线调试 |
| 模拟器 | 类型 `emu`，地址 `127.0.0.1:7555` | MuMu/雷电等 adb 端口 |

**屏幕模式**：
- `镜像主屏`：投物理屏幕，各设备分辨率不同
- `虚拟屏`：统一分辨率（预设 1920x1080 / 1080x1920 / 1280x720，可自定义宽高+DPI），
  需 Android 10+；连接只建立投屏会话，应用由 Console 启动按钮或脚本 `str_app` 显式启动

**WebRTC 网络**：服务端不内置 STUN/TURN，默认使用 host candidate 直连，适合同机或局域网。
Docker bridge / NAT 场景需在 `server/config.toml` 配置 `rtc_external_ip`、
`rtc_udp_port`、`rtc_external_port` 并发布对应 UDP 端口；跨公网部署需自行提供可达网络路径。

## YAML 脚本语法

YAML 自动化脚本的完整语法、参数说明和详细示例见 **[docs/YAML.md](docs/YAML.md)**。
模板、脚本和函数库按应用分区存放在 `data/<应用包名>/{tmpl,yaml,func}/`（web 端 Console 页框选/上传模板）。

## API 一览

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | /api/login | 登录 |
| POST | /api/logout | 退出并立即使当前 Cookie 会话失效 |
| GET/POST | /api/devices | 设备列表 / 创建 |
| POST | /api/devices/scan | 扫描 `adb devices -l` 并自动注册新设备（前端"刷新"时调用） |
| PUT/DELETE | /api/devices/:id | 更新配置（变更后自动重连）/ 删除 |
| POST | /api/devices/:id/connect | 连接设备 |
| POST | /api/devices/:id/screenshot | 截图（PNG） |
| POST | /api/devices/:id/control | 手动控制（tap/swipe/text/press/home/back/recents/start_app/rotate/clipboard） |
| GET/POST | /api/templates | 模板列表 / 创建 |
| PUT | /api/templates/:name/image | 替换已有模板图像 |
| POST | /api/templates/:name/test | 测试匹配 |
| GET/POST/PUT | /api/scripts | 脚本列表 / 创建 / 更新 |
| POST | /api/scripts/:id/run | 运行脚本（异步 202） |
| GET | /api/runs/:id | 查询运行 |
| POST | /api/runs/:id/cancel | 取消运行 |
| GET/POST/PUT | /api/functions | 函数库列表 / 创建 / 更新 |
| POST | /api/functions/:id/run | 测试函数（异步 202） |
| GET/POST | /api/tasks | 定时任务列表 / 保存 |
| POST | /api/tasks/:id/run | 立即执行 |
| GET/DELETE | /api/logs | 运行日志 / 清空 |
| WS | /ws/device/:id | WebRTC 信令（offer → answer） |

脚本运行以 `run_id` 标识一次执行实例。启动脚本、函数测试或“立即运行任务”采用异步返回：
接受后返回 HTTP `202` 和 `run_id/resolved_args`，前端按 `run_id` 查询或取消；同一设备已有活动运行时返回 `409`，并附带当前运行信息，避免不同脚本并发控制同一设备。脚本/函数保存、导入、运行和任务保存共用严格 v2 loader，失败返回结构化诊断。

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
- **帧缓存**：内存保留 SPS/PPS 与最近完整 GOP；截图/模板匹配时按精确帧序号临时调用 ffmpeg 解码，同设备同一帧的并发请求共享一次解码；延迟以固定夹具基准为准，无 ffmpeg 时自动降级 `adb exec-out screencap -p`
- **设备自动发现**：前端"刷新"会调用 `/api/devices/scan`（`adb devices -l`），
  自动注册未入库的设备（USB/无线/模拟器自动识别类型与型号，默认镜像模式），
  已注册设备跳过；注册后可在设备列表 ✏️ 编辑为虚拟屏等配置
- **已知坑**：
  - tokio `TcpStream::into_split()` 的写半在 drop 时会发送 FIN（`shutdown_on_drop`），
    会导致 scrcpy server 关闭连接——视频 socket 必须保留整个 TcpStream
  - `max_fps=0` / `max_size=0` 等 0 值参数不要传给 scrcpy server（与官方客户端行为一致）
  - 模板匹配引擎会把截图与模板等比缩放到最长边 540px 加速，命中坐标会映射回原图
  - **ffmpeg 路径失效 → 连接控制黑屏**：`config.toml` 的 `ffmpeg_path` 指向不存在的
    可执行文件时，帧缓存（FrameCache）启动失败，WebRTC 新 viewer 的初始推流帧
    （SPS/PPS + 最近 GOP）为 None；scrcpy 只在会话开始时发一次 SPS/PPS，后连接的
    浏览器永远收不到参数集，H.264 解码器无法初始化 → 即使 RTP 帧在流也一直黑屏。
    排查：服务端日志出现 `frame cache unavailable` 且无 `pusher replayed initial GOP`。

## 真机联调记录（2026-08-16，红米 25079RPDCC / Android 16）

### 镜像模式（display_id=0）
- ✅ 无线 adb（mDNS serial）设备接入 + scrcpy 会话建立，H.264 视频流持续稳定（60fps）
- ✅ 控制注入：tap / swipe / 文本 / HOME / BACK / 音量按键
- ✅ `start_app` 启动星穹铁道（com.miHoYo.hkrpg）成功
- ✅ 模板匹配：真实游戏画面命中（置信度 0.98 / 0.85）
- ✅ YAML 脚本：until（模板出现并点击）→ wait → tap 全链路执行并输出日志
- ✅ 定时任务：cron 触发 + 立即执行 + 触发点防重复

### 虚拟屏模式（new_display=1920x1080/420）✅ 重点验证
- ✅ scrcpy-server 以 `new_display=1920x1080/420` 启动，设备端创建虚拟显示器（id=91，FLAG_PRESENTATION）
- ✅ 视频流 meta 为 **1920x1080**（虚拟屏分辨率，非物理屏 3008x1880）
- ✅ 已验证 `start_app` 把星穹铁道启动到虚拟屏（`on display 91`）；当前版本由 Console 或脚本显式触发
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
