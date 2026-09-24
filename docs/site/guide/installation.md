# 安装与首次启动

Gamer 可以从源码运行，也支持使用 Windows x64 完整包。设备连接发生在运行服务端的电脑上，浏览器负责操作界面。

## Windows 启动器与完整包

当前已公开 [0.2.0-beta.6 测试版](https://github.com/jesongit/gamer/releases/tag/v0.2.0-beta.6)。下载独立 `gamer-launcher.exe` 放入专用可写目录可在线安装；也可将 `Gamer-0.2.0-beta.6-windows-x64-full.zip` 全部解压后离线安装。首次双击显示“安装 / 取消”，安装完成后选择官方插件，再打开网页设置密码。

完整包带齐本体、启动器、ADB、FFmpeg/FFprobe、scrcpy 和官方插件归档，无需预装 Rust、Node.js 或 pnpm。解开 ZIP 后仍需点击“安装”，把组件展开到运行目录。安装文件使用 HTTPS + SHA256 校验，不要求签名或公钥。

以后双击会在后台检查；没有更新时进入托盘并打开网页，有更新时显示“更新 / 取消”。托盘仅提供“打开 Gamer / 退出 Gamer”。无参数为图形入口，显式 `start` 为命令行监管服务：

在解压目录也可执行：

```powershell
.\gamer-launcher.exe start
.\gamer-launcher.exe status
.\gamer-launcher.exe doctor
```

缺失依赖时使用 `gamer-launcher.exe repair --probe`。已有版本随包说明的修正以对应 Release 勘误和当前更新指南为准。

beta.6 支持在“设置 → 软件更新”完成下载和实际版本切换，失败时回滚；beta.4 及更早版本先用启动器完成升级。保留已有安装的 `config/` 和 `data/`，不要用新包默认配置直接覆盖。详细离线升级步骤见[更新指南](https://github.com/jesongit/gamer/blob/main/docs/guides/UPDATE.md)。

## 从源码启动

安装 Rust stable、对应的链接工具链、Node.js、pnpm、ADB、FFmpeg 与 ffprobe。Windows MSVC 工具链需要 C++ Build Tools；Node.js 可采用当前项目 CI 使用的 24 版本。scrcpy server 已包含在仓库中。

在仓库根目录执行：

```powershell
git clone --recurse-submodules https://github.com/jesongit/gamer.git
Set-Location gamer
npm install --global pnpm@11.23.0
rustup target add wasm32-unknown-unknown

if (-not (Test-Path .\server\config.toml)) {
    Copy-Item .\server\config.example.toml .\server\config.toml
}

pnpm --dir web install --frozen-lockfile
.\tools\build-plugins.ps1
.\gamer.ps1 start
```

首次编译可能需要较长时间。启动后访问 `http://localhost:5173`。服务端默认监听 `http://localhost:8443`；端口号本身不表示已启用 HTTPS。

已有工作区更新后运行 `git submodule update --init --recursive`，恢复主仓固定的插件提交，不使用 `--remote`。普通 `pnpm --dir web build` 只构建主界面壳；官方插件源码在独立的 [gamer-plugins 仓库](https://github.com/jesongit/gamer-plugins)。

## 首次设置密码

默认开发模式下，未配置密码时页面会提示首次设置。账号是 `admin`，密码由你设置，至少 8 位；确认后保存 Argon2id 哈希并自动登录。

首次设置只接受服务端回环来源。请先在服务端电脑上打开本机地址完成设置，再从其他设备访问。

开发环境也可以在启动前提供 `GAMER_ADMIN_PASSWORD`；该值优先于配置中的密码，只在进程内转换为哈希。使用这种方式时，重启服务仍需要提供同一个环境变量。生产模式只使用配置中的 `[auth].password_hash`。参见[服务端配置](../reference/configuration.md)。

## 安装功能插件

进入「插件」页，安装需要的插件：

| 插件 | 提供的功能 |
| --- | --- |
| `gamer-yaml` | 自动化、函数、模板 |
| `gamer-keymap` | 按键映射 |
| `gamer-video` | 视频工作台与项目 |

本地构建产物在 `web/public/plugins/`，可直接导入 `.gplugin`。安装前查看权限；安装成功后自动启用。新安装或更新 UI 后，保存编辑内容并刷新页面。

启动器首装使用发行清单锁定的官方插件合集，完整包携带相同归档。插件页则独立发现插件仓的最新公开目录，手动刷新立即检查；网络失败回退缓存或随包列表，不自动安装或覆盖插件。新增宿主原生能力仍需同步更新 Gamer。

## 仅本机使用

在配置顶层设置 `local_only = true`，或启动前设置 `$env:GAMER_LOCAL_ONLY = '1'`，可同时限制 HTTP 和 WebRTC 为回环地址，适合本机验收并减少外部监听产生的防火墙提示。其他电脑不能访问该实例，USB ADB 仍可用；不能与非默认 NAT 设置并用。默认仍允许局域网访问，详见[服务端配置](../reference/configuration.md)。

## 日常启动与停止

```powershell
.\gamer.ps1 status
.\gamer.ps1 stop
.\gamer.ps1 restart
.\gamer.ps1 restart -Build  # Rust 代码修改后重新构建
.\gamer.ps1 rebuild         # 构建前后端并重启
```

源码构建前端：`pnpm --dir web build`。产物写入 `server/web-dist/`，此后可直接通过服务端的 8443 端口访问已构建界面。
