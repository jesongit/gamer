# Gamer

Gamer 是一个通过浏览器操作 Android 设备的游戏自动化工具。它将实时投屏、鼠标键盘控制、图像模板匹配、可视化脚本编辑和定时任务放在同一个工作台中，支持从录制视频生成自动化草稿。

设备统一通过 ADB 接入，画面由 scrcpy 采集，经 WebRTC 传输到浏览器。服务端使用 Rust，前端使用 Vue 3；自动化、按键映射和视频工作台分别由插件提供。

**[官方文档网站](https://jesongit.github.io/gamer/)** · [文档源码](docs/site/) · [本地预览与维护](docs/site/development/documentation.md)

## 可以做什么

- **查看与控制设备**：实时投屏、触控、按键、文本输入、应用启停和截图；连接多个设备并切换操作目标。
- **使用独立虚拟屏**：在支持的 Android 设备上指定分辨率与 DPI，例如 1920×1080；也可以镜像设备主屏。
- **编写自动化**：可视化编辑步骤、参数、分支、循环和函数，使用模板匹配完成找图与点击，也可编辑 YAML 原文。
- **安排任务**：设置执行入口、运行参数、设备与调度时间。任务在服务端执行，关闭浏览器后仍可继续。
- **分析录制视频**：录制设备画面及操作事件，导入视频、逐帧查看、标记时间轴、提取模板和生成脚本草稿。
- **复用配置与能力**：通过配置包组织和分发脚本、模板、映射与视频项目，通过插件扩展编辑和执行能力。

## 快速开始

### 从源码启动

以下命令在 Windows PowerShell 中、仓库根目录执行。先安装这些依赖并加入 `PATH`：

| 依赖 | 用途 |
| --- | --- |
| Rust stable 与对应平台的链接工具链 | 构建服务端、插件 WASM 和打包工具；Windows MSVC 工具链需要 C++ Build Tools |
| Node.js | 前端与插件 UI 构建；当前 CI 使用 Node.js 24，项目声明最低 Node.js 20 |
| pnpm | 版本以 [web/package.json](web/package.json) 的 `packageManager` 为准，当前为 `11.23.0` |
| Android platform-tools / `adb` | 发现、连接和控制 Android 设备 |
| FFmpeg 与 `ffprobe` | 截图解码、模板匹配、视频探测和逐帧分析 |

仓库已包含配套的 `server/assets/scrcpy-server.jar`。设备端协议与该文件绑定，使用仓库提供的版本。

```powershell
# 安装 pnpm；已有匹配版本时可跳过
npm install --global pnpm@11.23.0

# WASM 插件构建所需目标
rustup target add wasm32-unknown-unknown

# 首次准备本机配置，已有配置时保留
if (-not (Test-Path .\server\config.toml)) {
    Copy-Item .\server\config.example.toml .\server\config.toml
}

pnpm --dir web install --frozen-lockfile

# 构建三个官方插件及本地市场索引
.\tools\build-plugins.ps1

# 启动服务端与前端开发服务；首次会编译服务端
.\gamer.ps1 start
```

打开 [http://localhost:5173](http://localhost:5173)。默认服务端地址为 [http://localhost:8443](http://localhost:8443)，前端将 API 和 WebSocket 请求代理到服务端。

默认开发模式下，如果尚未配置密码，首次在本机打开页面会进入管理员密码设置。账号为 `admin`，密码由你设置，成功后自动登录；配置文件只保存 Argon2id 哈希。首次设置仅接受服务端回环来源，请先在运行服务的电脑上完成。

开发或自动化环境也可在启动前设置 `GAMER_ADMIN_PASSWORD`。它在开发模式下优先于配置密码，只在进程内转换为哈希，不写回文件。项目没有预设密码；生产模式使用 `[auth].password_hash`，不接受该明文环境变量。

### 使用 Windows 完整包

已有 Windows x64 完整包时，解压后双击 `gamer-launcher.exe` 即可进入启动流程；也可在解压目录执行：

```powershell
.\gamer-launcher.exe start
```

完整包携带运行依赖和前端产物，使用时无需安装 Rust、Node.js 或 pnpm。依赖检查、修复和状态查询入口为：

```powershell
.\gamer-launcher.exe doctor
.\gamer-launcher.exe repair --probe
.\gamer-launcher.exe status
```

安装细节以包内 `INSTALL.md` 为准。构包、发布及升级说明见 [发布手册](docs/guides/RELEASE.md) 和 [更新指南](docs/guides/UPDATE.md)。

## 第一次使用

1. **安装插件**：进入「插件」页，从本地市场安装所需官方插件，或导入 `.gplugin`。安装前查看权限，安装成功后会自动启用；新安装或更新 UI 后刷新页面。
2. **连接设备**：先用 `adb devices -l` 确认设备已授权且可用，再在工作台扫描设备或填写 ADB 地址。选择屏幕模式，建立投屏连接。
3. **选择应用**：在工作台工具栏选择设备上要操作的 Android 应用，再启动应用。建立投屏连接本身不会自动启动应用。
4. **选择配置**：使用首次启动创建的空白默认配置，或在「配置包」页新建、导入自己的配置包。
5. **开始编排**：在工作台的自动化、函数、模板、映射或视频面板中编辑内容。需要定时执行时，再到「任务」页创建任务。

首次安装没有预置游戏脚本、模板或映射。工作台功能取决于已启用插件及其声明的应用适用范围；没有业务插件时显示空态。

| 页面 | 用途 |
| --- | --- |
| 工作台 | 投屏、设备与应用操作、当前配置选择、插件功能面板和轻状态条；默认首页 |
| 任务 | 创建调度、设置执行目标和参数、手动运行及管理任务状态 |
| 日志 | 查看与筛选运行日志，定位执行问题 |
| 配置包 | 同屏管理本地配置包与配置市场，进行导入、导出和元数据编辑 |
| 插件 | 在统一列表中查看已安装与市场插件，管理安装、启用、更新和卸载 |
| 设置 | 查看服务信息、运行依赖与当前部署支持的更新功能 |

## 设备与屏幕

Gamer 使用 **ADB 地址**识别设备。USB 真机、无线设备和模拟器使用同一套连接流程，设备名称仅作显示。

| 地址形式 | 连接方式 |
| --- | --- |
| USB serial、`emulator-5554` | 使用 `adb devices -l` 中完整且一致的 serial |
| `127.0.0.1:7555`、`192.168.1.20:5555` | 服务端通过 `adb connect` 建立网络连接 |
| 无线调试 mDNS serial | 先用 ADB 完成配对与发现，再扫描接入 |

设备扫描在**运行 Gamer 服务端的电脑**上执行。USB 设备需要授权这台电脑；无线调试需要在 Android 侧开启并完成配对。详细步骤见 [ADB 设备接入](docs/reference/DEVICE_ACCESS.md)。

屏幕模式与 ADB 连接方式分别设置：

- **镜像主屏**：显示并控制设备的物理屏幕。
- **虚拟屏**：使用独立的虚拟显示，可配置宽高、DPI 和帧率。是否可用取决于 Android 系统和应用；不支持时会明确报错，不自动切换屏幕或坐标系。

相同虚拟分辨率可以减少模板适配工作，实际画面仍可能受游戏布局、方向和缩放影响。

默认 WebRTC 使用 host candidate 直连，适合同机或局域网，没有内置 STUN/TURN。跨 NAT 访问需配置 `rtc_external_ip`、`rtc_udp_port`、`rtc_external_port`，并保证浏览器能访问对应 UDP 地址；仅能打开网页不代表视频链路已经可达。

## 配置包与自动化

| 概念 | 含义 | 示例 |
| --- | --- | --- |
| Device / 设备 | 运行与控制的目标 | 一个 ADB serial |
| Android App / 应用 | 设备上要启动的 Android 应用 | `com.example.game` |
| Package / 配置包 | 脚本、模板、映射和项目的数据集合 | `daily-config` |
| Plugin / 插件 | 提供编辑界面或执行能力 | `gamer-yaml` |
| Task / 任务 | 将执行入口、参数、设备与调度组合起来 | 每天执行一次签到脚本 |

应用选择和配置选择独立：一个配置包可以声明适用于多个应用，配置包 ID 也不必与 Android 包名相同。包内数据按插件隔离，未安装插件的数据仍会保留。

官方自动化插件使用 YAML V1。步骤由函数调用、`if`、`repeat`、`return` 组成；可视化编辑器与 YAML 原文编辑使用同一格式。例如：

```yaml
name: 启动并点击
run:
  - launch: {}
  - sleep: 2s
  - tap: [0.5, 0.8]
  - log: 操作完成
```

此例启动设备当前选择的应用，等待后点击相对坐标位置。需要适应画面变化时，可结合模板和 `wait_find` 等函数判断状态。脚本没有 `version` 字段；函数库使用 `automations/_function*.yaml`，默认文件为 `_function.yaml`。

入门操作见 [YAML 自动化教程](docs/guides/yaml-tutorial.md)，完整语法与函数说明见 [YAML 参考](docs/reference/YAML.md)。

### 数据存放与导出

源码启动默认将数据放在 `server/data/`：

```text
server/data/
├── gamer.db                    # 设备、任务、预设、运行与日志
├── extensions/                 # 已安装插件与生命周期状态
├── media/                      # 视频、录制与媒体元数据
└── packages/<package-id>/
    ├── package.toml            # 配置包信息、目标应用、插件依赖
    ├── shared/                # 包内共享保留区
    └── plugins/
        ├── gamer-yaml/         # automations/、templates/
        ├── gamer-keymap/       # mappings/
        └── gamer-video/        # projects/
```

配置包通过 `.gamerpkg` 导入导出，插件通过 `.gplugin` 分发。配置包导出默认登记媒体引用；需要把视频一起带走时，在导出中选择包含媒体。配置包导出不包含全局设备、任务或插件安装状态；完整备份需在停机后保存数据目录和本机配置。

旧 `gamer.*` 插件身份的一次性转换工具见 [convert-plugin-ids.py](tools/convert-plugin-ids.py)，适用范围见 [ADB 与插件拆分报告](docs/reports/gamer-adb-plugin-split-report.md)。新安装无需执行转换。

## 开发与构建

### 仓库结构

```text
gamer/
├── server/                     # Rust Core：设备、WebRTC、资源、媒体、运行、任务和插件机制
├── web/                        # Vue 3 主界面与插件 UI 宿主
├── plugins/
│   ├── gamer-yaml/             # 自动化、函数、模板、WASM 解释器
│   ├── gamer-keymap/           # 按键映射 UI 与 WASM 输入规则
│   └── gamer-video/            # 视频工作台与项目，使用 Core 媒体和录制能力
├── sdk/                        # 插件接口、UI SDK 与独立示例
├── launcher/                   # Windows 安装启动与更新管理
├── tools/                      # 插件构建、质量检查与开发工具
├── release/                    # 依赖锁、发布契约与打包脚本
├── docs/                       # 使用指南、接口参考、设计规范与报告
└── gamer.ps1                   # 本地前后端进程管理
```

### 日常命令

在仓库根目录执行：

```powershell
.\gamer.ps1 status                 # 查看状态和最近日志
.\gamer.ps1 stop                   # 停止前后端
.\gamer.ps1 restart                # 使用现有构建重启
.\gamer.ps1 restart -Build         # 重新构建服务端后重启
.\gamer.ps1 rebuild                # 构建前后端并重启
.\gamer.ps1 start -BackendOnly     # 只启动服务端
.\gamer.ps1 start -FrontendOnly    # 只启动前端开发服务
```

需要在终端直接调试时，可分别运行：

```powershell
# 终端一，从仓库根目录进入 server，确保资产相对路径正确
Set-Location server
cargo run

# 终端二，从仓库根目录启动前端
pnpm --dir web dev
```

已有进程时先停掉对应服务，避免端口冲突。`pnpm --dir web build` 会先构建插件 UI，再将主前端输出至 `server/web-dist/`，由服务端在 8443 端口托管。

### 官方插件开发

| 插件 | 功能 | 开发说明 |
| --- | --- | --- |
| `gamer-yaml` | 自动化、函数、模板与 YAML 解释器 | [README](plugins/gamer-yaml/README.md) |
| `gamer-keymap` | 按键映射编辑与输入规则 | [README](plugins/gamer-keymap/README.md) |
| `gamer-video` | 素材、录制、时间轴与视频项目 | [README](plugins/gamer-video/README.md) |

每个插件目录包含自己的 `manifest.toml`、README、构建入口和 UI 工程。YAML、按键映射插件另有 WASM guest；视频插件使用 builtin 执行类型。

```powershell
# 单独构建一个插件，保留其他插件的发行条目
.\plugins\gamer-yaml\build.ps1
.\plugins\gamer-keymap\build.ps1
.\plugins\gamer-video\build.ps1

# 仅构建并同步某个插件的本地静态 UI
node sdk/ui/build-modules.mjs gamer-yaml
```

插件产物写入 `web/public/plugins/`，市场索引为 `web/public/registry.json`。在「插件」页导入或更新对应归档，保存编辑内容后刷新，即可加载新 UI。已安装插件的 UI 来自安装归档；仅同步本地静态 UI 不会替换已安装的归档。

**独立更新范围**：在现有 SDK/ABI 内，插件 UI 和 WASM 可以独立构建、安装、更新。`host/` 中的 Rust 适配代码仍随服务端编译；修改原生能力、资源校验或 WIT 接口时，需要同步更新服务端。

插件界面支持声明式表单、沙盒 iframe 和宿主 UI 模块。宿主模块需显式声明 `ui.host` 权限，与主页面共享执行环境；安装时应核对权限和来源。第三方插件可从 [SDK 示例](sdk/README.md) 开始，接口与界面要求见 [插件开发指南](docs/guides/plugin-dev.md) 和 [UI 设计规范](docs/design/gamer-ui-spec.md)。

### 验证改动

```powershell
# 前端主界面与各插件 UI 测试
pnpm --dir web test:run

# 服务端测试和静态检查
cargo test --manifest-path server/Cargo.toml
cargo clippy --manifest-path server/Cargo.toml --all-targets -- -D warnings

# 本地完整质量检查，包含构建
.\tools\ci-local.ps1
```

Rust 测试中的 WASM 场景会构建 guest，需要预先安装 `wasm32-unknown-unknown`。默认服务端包含 WASM 运行时；`--no-default-features` 用于验证无 WASM 构建路径，该构建不提供 WASM 插件执行能力。

## 配置与排查

完整配置说明以 [server/config.example.toml](server/config.example.toml) 为入口，本机修改保存在不提交版本控制的 `server/config.toml`。

| 配置项 | 用途 |
| --- | --- |
| `port`、`data_dir` | 服务端端口与数据目录 |
| `adb_path` | `adb` 命令名或可执行文件路径 |
| `ffmpeg_path` | FFmpeg 路径；视频功能还需要对应的 `ffprobe` |
| `scrcpy_server` | 与宿主协议匹配的 scrcpy server 资产路径 |
| `fps`、`bitrate_mbps` | 默认帧率和视频码率 |
| `idle_power_secs` | 无观看者、无脚本运行时的空闲低功耗等待时间 |
| `rtc_external_ip`、`rtc_udp_port`、`rtc_external_port` | WebRTC 外部可达地址与端口 |
| `[auth]` | 密码哈希、会话时限和登录限流 |

常见问题：

- **设备找不到**：在服务端电脑执行 `adb devices -l`，检查授权、在线状态和完整 serial；模拟器的 ADB 端口以其实际配置为准。
- **网页正常但投屏黑屏**：检查 ADB、FFmpeg、scrcpy 路径，以及浏览器到服务端的 WebRTC UDP 连通性。
- **工作台没有功能面板**：检查插件是否安装并启用、是否有启动错误，以及当前应用是否符合插件的目标应用声明。
- **修改代码后没有变化**：Rust 修改后重新构建服务端；已安装插件 UI 修改后重新打包、更新并刷新页面。
- **查看运行问题**：先看「日志」「设置」及 `gamer.ps1 status`，开发服务的文件日志位于 `server/`。更多记录见 [已知问题与排查](docs/PITFALLS.md)。

## 文档入口

| 主题 | 文档 |
| --- | --- |
| 设备连接 | [ADB 设备接入](docs/reference/DEVICE_ACCESS.md) |
| 自动化入门与语法 | [教程](docs/guides/yaml-tutorial.md) · [YAML 参考](docs/reference/YAML.md) |
| 插件开发 | [开发指南](docs/guides/plugin-dev.md) · [Host API](docs/reference/PLUGIN_API.md) · [SDK](sdk/README.md) |
| 界面与交互 | [UI 设计规范](docs/design/gamer-ui-spec.md) · [iframe UI SDK](sdk/ui/README.md) |
| 发布与更新 | [发布手册](docs/guides/RELEASE.md) · [更新指南](docs/guides/UPDATE.md) |
| 架构约定 | [开发约定](AGENTS.md) · [架构决策](docs/reference/adr/) |
| 第三方组件 | [NOTICE](licenses/NOTICE.md) · [依赖版本与来源](release/dependencies.lock.toml) |
