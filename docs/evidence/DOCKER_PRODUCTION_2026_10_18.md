# Docker 生产迁移证据（2026-10-18）

用户本轮明确授权新增 Docker 支持，并将现有 4 个云游戏任务迁入本地 Docker 生产环境。
原先 AGENTS.md 的隔离测试限制不阻止本轮明确授权的生产切换；切换须保留全部数据及回退点。

## 已完成的源码工作

- 单容器多阶段构建：Linux Rust 宿主、静态前端、Chromium、FFmpeg、OCR 中英模型及字体。
- Compose 独立数据卷、回环发布端口、init、共享内存、Chromium 用户命名空间 seccomp 和优雅停止窗口。
- 便携 AES-256-GCM 凭据格式；Windows 默认继续读取/写入原 DPAPI，显式指定安装密钥后支持跨平台格式；不改写退休 AI 私密数据。
- `scheduler_enabled` 默认 true，隔离/开发配置显式关闭；不改变原任务定义。
- 离线迁移工具复制全部文件，SQLite Backup API 生成一致库，备份与运行目录分离，凭据只在进程内解密并重新封装。

## 当前验证状态

- Windows `cargo check` 通过。
- Windows 凭据专项回归通过：重开、篡改、错误密钥、丢失密钥与拒绝明文。
- Windows 自动调度配置回归通过；SDK 同步检查通过。
- Linux amd64 Release 镜像构建通过；构建上下文约 55 MiB，运行数据及本机配置明确排除。
- 第一次前端构建暴露跨插件依赖安装顺序问题，已改成先安装全部依赖再构建。
- 构建补齐编译期 WIT 与 YAML 文档依赖；编译宿主与插件来自本地工作区，根仓基准 HEAD 为 `827d722f33dfa09151c164e7a0484de08debd9b6`，包含本轮未提交改动。插件宿主接口改动已本地提交 `9fff23e`，未推送。
- Linux 非 root Chromium 沙箱打开合成页面通过，未关闭沙箱、未使用 privileged。
- OCR 提供 `eng`、`chi_sim`、`osd`，FFmpeg 合成数字图识别通过。
- 全新隔离容器生产模式初始化、readiness、UID 1000、SIGTERM 协调退出通过。
- 缺少既有凭据密钥时拒绝生成替代密钥，通过隔离回归。
- 命名卷备份/恢复隔离演练通过，SQLite 内容与原密钥一致；只清理本轮合成演练卷。
- ARM 原生构建工作流已加入，当前机器未执行 ARM 构建/实机验收。

## 数据迁移进度

- 原 Windows 后端已优雅停止。4 个账号通过各自 Gamer 专属浏览器正常导出 Cookie，输出只包含数量：15/29/39/39。传输包加密保存在 `D:/Gamer/session-transfer-20261018/`。
- 完整备份 `D:/Gamer/migration-20261018/backup/`，独立运行副本 `D:/Gamer/migration-20261018/data/`；14,524 文件、11,426,266,588 字节，逐文件 SHA-256 清单为 `backup-manifest.json`。
- 源库、备份库、迁移库均 `integrity_check=ok`、schema v7；各含 4 个 enabled 任务、160 条运行记录、125,175 条事件。
- 活动 Notify/OCR/Live 的 3 份凭据经原 Windows 账号 DPAPI 解密后重新加密；仅 OCR 工具路径调整为 Linux，源文件及备份未改。
- Windows 原浏览器资料保留，Linux 使用原站点存储与正常浏览器导出的 Cookie。
- 生产卷 `gamer-production-data` 导入完成；除显式转换的 3 份活动凭据外，14,521 个原文件逐字节 SHA-256 全部匹配（原浏览器资料映射到 migration-originals）。
- 本地开发改为 `server/data-dev`，开发配置 `scheduler_enabled=false`，开发库的 4 个任务均 `enabled=0`；原 `server/data` 保留。

## 本机生产验收结果

生产入口 `http://localhost:18443`，容器 `gamer-production-gamer-1`，镜像标签 `gamer:docker-20261018`。
容器中 Rust 进程和 ADB 均为 UID 1000，Chromium 使用沙箱；生产命名卷为 external，Compose 不删除其生命周期。

- `/health/ready` 为 true，ADB/FFmpeg/JAR/SQLite/数据目录检查均通过。
- 7 个现用插件 Running；退休 gamer-ai 保持原数据和 Enabled + host_feature_unavailable 状态，未恢复执行。
- OCR status 可用，中文/英文模型可识别；Notify channels.read 在 Linux 中正常解密，不在证据中输出凭据。
- Linux 浏览器已实际显示云游戏画面，4 个账号真实执行原任务均 success：

| 任务 | Docker 运行 ID | 北京时间完成 | 结果 |
| --- | --- | --- | --- |
| 155日常 | ab7db518-4572-4e9c-999c-92c60b19643e | 2026-10-18 19:26:53 | success |
| 188日常 | 0784add2-a053-441e-8a96-dd2cef27ecbc | 2026-10-18 19:27:01 | success |
| 136日常 | ebb710f3-80e4-4bbd-aec3-9574b63040a4 | 2026-10-18 19:27:08 | success |
| 173日常 | 4f769c71-eb4f-4da5-92fd-a2c0fa449068 | 2026-10-18 19:27:02 | success |

验收前今天的日常已在 Windows 完成，因此此次真实云游戏运行主要覆盖登录、进入游戏、模板匹配、已完成奖励核验和收尾；
没有重新证明新一天的战斗、体力消耗和所有 OCR 分支。OCR 数字识别另有容器合成图测试，不替代真实战斗分支验收。
四路同时运行时采样内存约 3.06 GiB、CPU 约 458%（多核容器百分比）；这是运行态采样，不能当成固定上限。

- 所有任务终态后关闭浏览器并停止容器，再 force-recreate；8 个凭据/浏览器 Cookie 文件指纹全部一致。
- 重建后逐个启动 4 个账号，实际页面的「进入游戏」与通行证登录标记均存在，再关闭浏览器。
- 运行用户 HOME 已持久化到 `/data/state/home`，ADB 密钥及未来 gh 登录随卷保留；初始化时串行启动 ADB，避免并发探测/扫描抢占 daemon。
- 最终自动调度已开启，4 个任务保持 enabled + active；下次唤醒均为 `2026-10-18T20:01:00Z`，即北京时间 **2026-10-19 04:01**。
- `browser_idle_secs=1800`，容器 `TZ=Asia/Shanghai`，策略 `restart: unless-stopped`，开发后台未运行。
- 容器重建后无浏览器时初次空闲采样约 58 MiB；重新逐个验证登录后约 98 MiB，最终数据库检查后约 149 MiB / CPU 0.03%，Chromium 进程为 0（容器统计，不包含宿主 Docker/WSL 总开销，均为采样值）。

完整原始备份保留在上述迁移目录，部署选择位于仓库忽略的 `.env`；`D:/Gamer/migration-20261018/deployment.json` 记录最终镜像身份、健康状态和本次运行 ID。
最终 SQLite 再次完整性检查为 ok，保留原 160 条记录并增加本轮 4 条，合计 164 条运行记录。
后续固定镜像更新、停机一致备份及新卷回滚见 `docs/guides/DOCKER.md`。
Windows 宿主休眠/关机或 Docker 引擎停止期间不会执行日常；本轮没有创建宿主唤醒任务。

## 保留边界

保留正式脚本、模板、全部工作稿版本、独立素材、插件不可变安装版本、媒体、日志/Trace、历史运行、发布事务和退休插件旧数据。
原完整备份不可直接用作运行目录。开发实例不得与生产共享数据库、浏览器资料或自动调度。
