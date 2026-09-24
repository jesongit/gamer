# 本机网络模式与真机验收（2026-09-24）

候选版本为 `0.2.0-beta.6`；本轮本地构建为 Windows GNU。beta.5 已构建但保留为未公开草稿，后续发布不得覆盖其标签。

## 防火墙提示定位与处理

新增 `local_only=true` / `GAMER_LOCAL_ONLY=1`，同时限制 HTTP 与 WebRTC ICE 到 IPv4 回环地址。启动器普通启动、更新、回滚及自更新 helper 显式保留环境选择；默认配置仍允许局域网访问。旧版本程序不识别新开关。

用户反馈仍出现提示后，发现全量 Rust 测试中的固定 UDP mux 测试直接监听所有网卡，不经过配置加载环境覆盖。Windows 防火墙规则出现本次 `gamer_server-1cabd5b2694498e7.exe` 测试程序。该项现在 Windows 默认忽略，可显式 `--ignored` 验证；Linux CI 继续正常执行。没有修改系统防火墙规则或通知设置。

## 已执行验证

- 服务端全量 711 通过、7 忽略；之后将上述单项改为 Windows 显式测试，重跑 RTC 模块 7 通过、1 忽略。新增配置校验和真实 loopback ICE gather 均通过。
- 启动器全量 205 通过、6 忽略；服务端与启动器 clippy `--all-targets --all-features -D warnings` 通过。版本与固定插件 SDK 检查通过。
- `backups/beta-release/beta6-local` 从完整包安装，移除本地插件种子后由启动器下载插件仓 Release；SHA 校验、无配置首启、三个插件 Running 和各自 UI 资源均通过。
- 同一隔离安装以 `GAMER_LOCAL_ONLY=1` 启动，实测 HTTP 监听 `127.0.0.1:8443`。
- Android 16 真机经 USB ADB、镜像 scrcpy、浏览器 Edge WebRTC 实际连通：接收到 3008×1880 H.264 画面，读取统计时已解码 10 帧，服务端两条 ICE candidate 均为 `127.0.0.1`。
- DataChannel 发送 WAKEUP 按键后 Android 报告 Awake；截图 API 返回 4,756,415 字节。随后断开会话、关闭测试服务并恢复测试前休眠状态。

真机脚本及日志仅留在本地 `backups/beta-release/device-loopback.cjs`、`beta6-device-r2.log`；不上传设备截图、凭据或完整序列号。首次脚本截图方法误用 GET，得到 405，改为实际 API 的 POST 后完整通过。没有验证游戏内脚本或干净 Windows 桌面交互，不能将上述结果等同于这些范围。
