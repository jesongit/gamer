# 安装与首次启动

Gamer 可以从源码运行，也支持使用 Windows x64 完整包。设备连接发生在运行服务端的电脑上，浏览器负责操作界面。

## Windows 完整包

如果已有完整包，解压到独立目录，双击 `gamer-launcher.exe`。无参数启动会按 `start` 处理；按包内 `INSTALL.md` 完成首次安装。运行依赖随完整包提供，无需安装 Rust、Node.js 或 pnpm。

在解压目录也可执行：

```powershell
.\gamer-launcher.exe start
.\gamer-launcher.exe status
.\gamer-launcher.exe doctor
```

缺失依赖时使用 `gamer-launcher.exe repair --probe`。完整包中的更新与恢复说明以随包文档为准。

## 从源码启动

安装 Rust stable、对应的链接工具链、Node.js、pnpm、ADB、FFmpeg 与 ffprobe。Windows MSVC 工具链需要 C++ Build Tools；Node.js 可采用当前项目 CI 使用的 24 版本。scrcpy server 已包含在仓库中。

在仓库根目录执行：

```powershell
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

## 日常启动与停止

```powershell
.\gamer.ps1 status
.\gamer.ps1 stop
.\gamer.ps1 restart
.\gamer.ps1 restart -Build  # Rust 代码修改后重新构建
.\gamer.ps1 rebuild         # 构建前后端并重启
```

源码构建前端：`pnpm --dir web build`。产物写入 `server/web-dist/`，此后可直接通过服务端的 8443 端口访问已构建界面。
