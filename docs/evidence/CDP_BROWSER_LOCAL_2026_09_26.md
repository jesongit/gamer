# CDP 浏览器目标本机验证（2026-09-26）

隔离分支：`codex/cdp-browser-target`，基于 Gamer `0a17431a0c6e887060f61be9d49dc35c44d826ab`。主仓运行服务、配置和用户数据库未用于测试；未合并。插件改动在独立插件仓提交，主仓更新固定 gitlink。

## 通过的检查

- Rust 无默认特性编译测试二进制；定向执行 browser、capabilities、YAML 宿主/runner、架构边界、RunManager、TimerCore、store、maintenance、migrations 与浏览器/任务/运行 API，共 139 项单元及 API 测试通过。
- 显式运行默认 ignored 的 `browser_roundtrip_isolation_navigation_and_profile_persistence`，额外 1 项通过：已安装 Edge 无窗口启动，本机 HTML 画面像素与尺寸、截图经 FrameStore/NCC 识别再由 InputService 点击、键盘/文本/修饰键、拖动终点松开、多账号隔离、Cookie/localStorage 重启保留、导航与改绑后旧输入拒绝。
- `cargo check --tests` 默认特性（含 WASM）通过。使用仅本条命令生效的 crates.io 镜像覆盖解决本机镜像缺少锁定依赖。
- Web 全量 Vitest：84 文件、784 项通过；此后补充迟到图片断线回归，浏览器预览专项 3 项通过（最终累计 785 项）。YAML UI：29 文件、257 项通过。
- Web 与 gamer-yaml UI 生产构建通过；`node tools/check-plugin-sdk.mjs` 通过，公开 SDK 固定快照无变更；`git diff --check` 通过。

测试设置 `GAMER_LOCAL_ONLY=1`、`ADB_MDNS=0`；浏览器使用 `127.0.0.1:0` 测试页和临时独立资料目录。未启动真实云平台，也未操作用户日常浏览器或修改防火墙。初次全量 Rust 测试后改用上述定向回归，避免需要网络监听的其他测试；不将早期全量运行计作最终全绿结果。

## 实测边界

定时任务已覆盖保存、读取、执行器调用与共享目标 prepare/acquire 路径；未以真实账号等待 cron 到点或跨服务重启实跑云游戏。现有 Android 行为由相关单元/API/架构测试回归，本次未连接 Android 实机。

本机 HTML 可用不等于云平台兼容：云·星穹铁道登录、排队、视频播放、GPU/受保护内容与特殊游戏输入尚未人工验收。首版预览约 2fps，无音频、CDP 录制或相对鼠标锁定。schema v6 的回退需恢复升级前数据快照。
