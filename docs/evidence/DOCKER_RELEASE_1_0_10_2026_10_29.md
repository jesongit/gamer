# Docker v1.0.10 / 应用 0.5.9（2026-10-29）

用户授权默认 MCP 令牌覆盖未来新增功能、旧默认令牌直接使用新功能并发布。本次发行未执行生产服务器更新或真实账号/设备操作。

## 固定发行

- [Docker v1.0.10](https://github.com/jesongit/gamer/releases/tag/docker-v1.0.10)，源码 `21b8cdc31d6a54d96050d944c8db07edc94977f4`。
- `posase/gamer:v1.0.10` 与 `posase/gamer:latest` 同为 `sha256:0b103def72c9679babb081970fcc1b2d2a3478950764d487bb31a3b651cc17cd`，平台 `linux/amd64`。
- [YAML 0.3.28 完整插件目录](https://github.com/jesongit/gamer-plugins/releases/tag/gamer-yaml-v0.3.28)，插件源码 `cff76b659d95e21ef7bd1c3aba317a6226eb3d2b`，SDK 宿主 `7fed8dcc3f203f90d29320490f58f4a48999e3dc`。
- 五款未变更插件逐条目比较后复用旧公开归档原字节。插件主仓和发行 tag 均已推送 GitHub 与 LAN；主仓固定 gitlink 及公开插件锁。

## 默认权限与新增 MCP

设置页新令牌使用动态全部 scope。历史设置页默认令牌按原格式识别：七类对象均为单一 `*` 范围，包含 `control`，且没有新格式显式限制标记；认证时获得动态 scope，不需要更换原密钥，不自动改写凭据文件。新格式显式受限令牌和旧有限对象令牌保留限制；到期、撤销、插件显式暴露、运行状态、对象校验、输入屏障及受限外部执行策略继续生效。

新增准确版本/双 SHA256 的正式发布、任务入口 CAS 切换以及已安装插件的版本/归档哈希钉住更新。新增权限使用独立工具及准确权限差异；默认全权限令牌直接覆盖这些工具的 scope，不再要求每次升级重建令牌。保留历史、调度、启用状态及其他插件配置；活动运行/调用阻止更新。

## 检查与公开复核

- [最终主仓 CI](https://github.com/jesongit/gamer/actions/runs/40768048125) 全绿：版本、Web、Rust fmt/clippy、无 WASM 构建、guest、后端测试及 release 构建。Linux 后端 1210 通过、0 失败、53 跳过，DTLS 62 通过。
- [插件发行 CI](https://github.com/jesongit/gamer-plugins/actions/runs/40765418701) 全绿：独立构建、宿主集成、安全门禁及实际 FFmpeg 回环 6 项通过。
- Windows 最终后端 1214 通过、0 失败、54 跳过；Web 壳 885、插件 UI 189、更新器库 94 及集成测试通过。高并发首次运行的原有 OCR 子进程清理用例失败，单独复测两次及低并发全量通过，未修改该用例或逻辑。
- 六份 Rust 锁文件通过严格新公告库审计，七份前端锁文件通过 npm 官方源审计，零漏洞、无豁免。
- 正式镜像来自两个干净 Git 归档，核对源码 label、RootFS、49 个应用文件和 390 个运行包；Chromium 非 root 沙盒、seccomp 四项、英文/简体中文 OCR 通过，无 Node/Cargo 运行环境或用户业务数据。
- 真实跨容器 HTTP、隔离合成数据卷从 v1.0.9（应用 0.5.8）升级。原配置、凭据文件、任务与 schema 7 保留；同一个旧令牌成功完成 YAML 0.3.27 → 0.3.28 更新（准确授予 resource.write）、正式发布与相同操作回执重试、任务入口切换并保留配置。没有真实设备输入。
- 原始录制名称搜索、Alt+Q 事件、真实视频帧与 OCR、素材制作、调度预览、重启持久性、热关闭远程入口、撤销、Origin/代理头和管理员认证隔离均通过。所有专用容器、卷及网络按标签核对后收尾。
- Docker Hub 无凭据匿名拉取成功，远端镜像配置及层与本地受测镜像一致。v1.0 至 v1.0.9 的十个旧版本摘要逐一保持不变，latest 摘要单独核对一致；Docker Hub 说明匿名读回一致。
- 插件九个、Docker 五个公开附件匿名重新下载，与草稿逐字节一致，SHA256SUMS、ZIP 内部 manifest、目录及插件 Schema 全通过。Docker 发行未改变 Windows 的 GitHub latest（v0.5.6）。

固定回执见 [v1.0.10.json](../../deploy/docker/releases/v1.0.10.json)。构建、完整检查、匿名复核和合成验收原始日志保留在忽略目录 `server/target/docker-release-v1.0.10/` 与 `server/target/plugin-release-v0.3.28/`。

本记录区分源码/发行/合成环境验收；它不证明真实云游戏退出、日常脚本完整流程或操作系统全面安全扫描通过。实际服务器、真实配置包及日常任务未由此次发行自动更新。
