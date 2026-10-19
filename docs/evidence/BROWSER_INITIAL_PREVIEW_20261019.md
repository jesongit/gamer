# 首次投屏断开：本机 Debug 与 Docker release 验证

日期：2026-10-19。范围为浏览器首次导航期间的投屏启动；真实游戏、Android、远程主机和 Docker Hub 发布不在本次验证范围。

## 原因与修复

上一轮账号设置验收在更新前、更新后镜像都复现了 Chromium CDP `-32000 / Not attached to an active page`。刚启动的浏览器尚在切换初始文档时请求投屏，流启动会直接返回错误，关闭 WebSocket。Cookie 与本机管理通道均受影响；等待 750ms 后不再复现，不能据此将立即投屏视为通过。

源码提交 `6725629` 将原启动过程保留为单次尝试，仅对上述精确错误在同一存活绑定上重试：100ms 间隔、5 秒重试窗口。正常路径没有固定等待；失败的尝试清除缓存帧，成功恢复只记录 debug，重试后失败记录 warn。每次 CDP 调用仍使用原来的有界超时，并未通过取消调用放弃 pending 清理。

不寻找其它标签、不重启浏览器、不接受旧帧；目标关闭、其它 CDP 错误、坐标和输入代次校验仍按原逻辑处理。预览、录制和媒体输出继续共用原 stream gate 和原清理流程。

新增 `initial_preview_survives_navigation_without_a_settle_delay`：6 个全新资料，单屏、四宫格、九宫格各两次，启动后立即请求预览，验证首帧、当前 stamp 和停止后清理。测试需要实际安装的浏览器，显式 `--ignored` 运行；全部页面为隔离的本机 HTML。

## 本地验证

- Windows `cargo clippy -j1 --all-targets --all-features -- -D warnings`、格式和 Debug 构建通过，固定插件 SDK 检查通过。
- 完整后端：1143 passed、0 failed、52 ignored；测试 EXE 从 `server/` 单线程执行。
- 另显式执行 15 个实际浏览器专项，全部通过：首次启动、单屏/网格、滚动条坐标、小视口、持续帧、慢导航与重载、失败导航恢复、缺 ACK/消息洪泛/输入错误、弹窗遮挡、媒体输出切换、HLS 像素和录制导航/边界。测试仅使用本机合成页面与临时资料。
- Debug 已更新到 `http://localhost:8443/#/console`，就绪正常；配置 SHA256 保持 `542d9d8eba2e8b0ca81617afd69e3f4667d5920b95575bf3ac87c3af026e5cee`，账号信息没有更改。SQLite schema 7、integrity=ok，任务 0、设备 1、浏览器目标 1、运行记录 160 与冷备份一致。

验证明细在忽略目录 `server/target/preview-backend-tests.log`、`preview-focused.log`、`preview-cold-native.log` 和 `preview-debug-acceptance.json`。没有修改前端源码，前端全量测试沿用上一轮证据；此次 Docker 构建的前端 production build 已通过。

## 隔离 Docker 验证

候选 `gamer:preview-start-20261019` 使用生产卷的独立恢复副本，`--network none`、1500 MiB 内存上限，写入显式测试标记与合成凭据；没有连接真实网站或运行日常。

- Cookie、本机管理通道各 8 个全新资料，共 16/16 个实例启动后立即连接 WebSocket，没有导航后的固定等待。每个实例取得首帧后 ACK 并继续收图 2 秒，实际各收到 21～22 帧，无断开或错误。
- 4 个禁用的合成任务在账号修改、重启与全部投屏检查后，数据库行逐项相同；调度保持关闭。这 4 个任务是隔离夹具，不是远程主机上的任务。
- prod HTTP 登录、修改账号和撤销旧会话、旧投屏退出而底层浏览器保留、拒绝过期设置 revision、重启后新账号/会话保持、独立 MCP 令牌均通过。
- 6 项运行依赖 ready；Core OCR 实际识别 `TEST 123`，MCP 越界包拒绝、撤销后 401。9 项关键软件包版本与更新前完全一致，记录在 `preview-package-acceptance.json`。
- Docker Linux release 编译和前端 production build 通过；镜像 ID `sha256:dd2fc80374addc3e13a4916d9a2f1d53283ff3e530de38c1852654b144f39289`，`linux/amd64`，源码提交 `071e9389fe0af08bbb805e99a08bfabe28cc1d60`。

结果见 `preview-candidate-acceptance.json` 与 `preview-cold-acceptance.json`。以上是本机合成 HTML 和协议/画面验证，不是实际云游戏平台、Android、远程机器、macOS 或 ARM 验收。

首次镜像构建下载 Debian `dconf-gsettings-backend` 遇 502，APT 返回 100；该次构建不算通过，也没有切换服务。`071e938` 给两处 APT 更新/安装增加 `Acquire::Retries=3`，随后重新构建；没有修改软件包选择或源地址。

构建采用单 Cargo job，与 Windows Rust 编译错开。专属 cgroup 上限 6 GiB、soft high 4 GiB、swap 0；两次尝试合计峰值 4295725056 字节，max/oom/oom_kill 均为 0。构建结束确认组内无进程后，仅回收该组缓存（memory.current 从 2303729664 降至 7434240 字节），再移除空组；没有全局清缓存、重启 WSL 或管理其它应用容器。

## 备份、更新与回滚

完整备份根目录 `D:\Gamer\backups\preview-start-20261019-195215`，仅当前用户访问。原数据及此前全部备份保留，备份目录没有作为运行目录。

| 快照 | 内容 | SHA256 |
| --- | --- | --- |
| `debug/data-dev.zip` | 1607 个文件、712869255 字节；另保存原配置和旧 Debug EXE | `0e36b6d0cb31497833eb068b73cf3d8fa5590e1e569c4abf58344876ab66d706` |
| `release/data.tar.gz` | 切换前完整冷备份、99 条目、607462 字节 | `55f5276f225acc860503ebae51403e3263df44b811662990ff21dcc1930692a7` |

更新后的本机容器为 `gamer-release`，使用上方候选镜像，访问 `http://127.0.0.1:18443/#/console`，prod + HTTP。复用原卷 `gamer-release-data`，原环境、端口、安全配置、共享内存及日志轮转参数均一致。54 个受保护文件字节一致，插件状态一致，直播队列业务摘要一致（revision 仅允许单调增加）；真实用户名、密码和凭据密钥均未更改。

本机 release 原有任务/设备/浏览器目标/运行记录均为 0，更新后计数相同，schema 7、integrity=ok，调度仍关闭。远程主机的 4 个任务不在此次本机操作范围；没有发布 Docker Hub 镜像。

`release/` 保留原容器 inspect、环境文件、安全配置、清单、README 和 `rollback.ps1`。脚本先校验当前镜像与快照 SHA256，再将快照恢复到新空卷，启动原镜像；升级后的原卷保留。检查命令为：

```powershell
powershell -NoProfile -File "D:\Gamer\backups\preview-start-20261019-195215\release\rollback.ps1" -ValidateOnly
```

去掉 `-ValidateOnly` 执行回滚。最初的卷不存在检查在 Windows PowerShell 5 下因预期 stderr 被提升为终止错误而未通过；已改为成功枚举卷后精确比较名称，在实际 `powershell -NoProfile` 调用下重跑预检通过。最终冷备份也已实际恢复到独立空卷，启动旧镜像 `sha256:e735a01a2069017cebb4095725ef891525e993dc5b6ffc071567ebe01297b4f2`，核对 54 个受保护文件字节（含配置和密钥）、插件状态、队列业务摘要和数据库完整性通过。没有对正式服务执行回滚。

本次合成验收容器及其专属卷、被替换的旧停止容器均按精确身份/镜像/挂载核对后清理；原卷、旧镜像及备份保留。Gamer 只保留一个 Debug 后端及一个运行中的 Docker release。完整验证回执和日志复制到备份根目录的 `verification/`。
