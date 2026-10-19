# 管理员账号设置：本机 Debug 与 Docker release 验证

日期：2026-10-19。范围为管理账号、会话治理和本机 release 更新；没有操作远程主机、真实游戏或设备输入，没有修改真实用户名或密码，没有发布 Docker Hub 镜像。

## 实现

- `7c056bb`：受管理认证保护的 `GET/PUT /api/auth/account`，保存用户名或密码前验证当前密码。新密码留空只改用户名；旧配置缺少用户名时继续使用 `admin`。
- `f08c553`：设置页「管理员账号」、保存后重新登录、仅在内存预填新用户名，以及阻止迟到的会话探测恢复旧登录。
- 密码继续使用固定参数 Argon2id PHC；当前密码验证共享登录限流。更新只有一个阻塞工作者，与通用设置共用配置锁，校验账号 revision 和外部配置改动，再原子替换配置。
- 每次修改持久化新的账号 revision，清空所有普通/保持登录会话并关闭旧 Cookie 投屏连接；会话文件清理失败也不能在重启后恢复旧会话。独立 MCP 令牌、本机管理令牌、任务及底层浏览器会话不受账号修改影响。
- 开发密码由 `GAMER_ADMIN_PASSWORD` 管理时，界面允许改用户名，密码修改须更新环境变量并重启。

## 本地验证

- Windows `cargo clippy -j1 --all-targets --all-features -- -D warnings`、`cargo fmt`、Debug 构建通过。
- 完整后端：1143 passed，51 ignored，0 failed；测试 EXE 在 `server/` 下单线程运行。账号新增 6 项状态/持久化测试和 1 项 HTTP 集成测试均通过。忽略项仍需各自的设备、浏览器、网络或历史组件等环境。
- 前端：97 个文件，862 tests passed；Vite production build 通过。
- 隔离浏览器表单：1280×900、390×844，无横向溢出或页面异常；验证错误密码、编辑保留、密码清除、只改用户名和重新登录。用户确认真实 Debug 设置页显示正常。
- Debug `http://localhost:8443/#/console` 已更新。配置 SHA256 保持 `542d9d8eba2e8b0ca81617afd69e3f4667d5920b95575bf3ac87c3af026e5cee`；SQLite schema 7、integrity=ok，任务 0、设备 1、浏览器目标 1、运行记录 160，均与冷备份一致。

本地明细在忽略目录 `server/target/account-*.json` / `account-*.log`，UI 复现入口为 `tools/test-admin-account-ui.cjs`。

## 隔离 Linux Docker 验证

候选使用生产数据的独立恢复副本，`--network none`、内存上限 1500 MiB，明确写入测试标记及合成密码。新增 4 个禁用的合成任务，调度保持关闭，没有执行日常任务。

`account-candidate-acceptance.json` 通过：

- prod HTTP 下修改账号，错误当前密码返回 403 并保留登录；正确修改撤销普通及保持登录 Cookie，旧用户名不能登录。
- 旧投屏 WebSocket 退出，底层 CDP 浏览器仍可访问；旧通用设置 revision 不能覆盖新凭据。
- 重启后新用户名、密码、新会话有效，旧会话保持无效；4 个合成任务的数据库行逐项保留。
- 原 MCP 令牌在账号修改及重启后仍有效；另验证 OCR 工具授权、越界配置包拒绝与令牌撤销 401。6 项运行依赖 ready，Core OCR 实际识别 `TEST 123`。
- Chromium、FFmpeg、ADB、Python、Tesseract 软件包版本与更新前一致。

### 独立观察：首次导航投屏

立即请求首次导航中的页面，曾收到 Chromium `Not attached to an active page`。用更新前镜像 `a174e9f…` 对照，同样在 Cookie 和本机管理通道复现；本次账号逻辑不是该错误的来源。新镜像等待页面初始导航 750ms 后，两个通道共 6 个独立浏览器实例均取得首帧。

该观察有新旧对照记录 `account-preview-baseline.json`、`account-preview-acceptance.json`，就绪后的验证记录为 `account-preview-settled-acceptance.json`。本次没有修复这个既有首次导航问题，也没有将合成 HTML 投屏验证当作真实云游戏验收。

## 备份、更新与回滚

完整备份根目录：`D:\Gamer\backups\admin-account-20261019-191500`，限制为当前用户访问。原数据和之前的备份均保留，备份目录不作运行目录。

| 快照 | 内容 | SHA256 |
| --- | --- | --- |
| `debug/data-dev.zip` | 1607 个文件，712869238 字节；另保留原配置和旧 Debug EXE | `3ed230e7dd0aee7b32a89e7edf423f752c9a3ca4f5b1c32f5e2367fb44404877` |
| `release/data.tar.gz` | 更新前完整冷备份，99 个条目，607457 字节 | `d36a6c4fa91efa29a257d4c4f2a648b3d3bb6be7475d7b21ac1083efd48f8ce7` |

更新后的本机服务：

- 容器 `gamer-release`，URL `http://127.0.0.1:18443/#/console`，prod + HTTP。
- 镜像 `gamer:admin-account-20261019`，`linux/amd64`，ID `sha256:e735a01a2069017cebb4095725ef891525e993dc5b6ffc071567ebe01297b4f2`，源码提交 `f08c553823114faecacc1a4baee439ab48b59013`。
- 复用原卷 `gamer-release-data`，运行参数和环境变量与原容器一致。54 个受保护文件字节一致，插件状态一致，直播队列业务摘要一致（仅允许 revision 单调递增）。
- 本机 release 原有任务、设备、浏览器目标、运行记录均为 0；schema 7、integrity=ok，调度仍关闭。远程主机上的 4 个任务不在本次操作范围。

`release/` 包含原容器 inspect、环境文件、安全配置、清单、README 和 `rollback.ps1`。脚本先校验当前镜像及快照 SHA256，再恢复到独立空卷后启动原镜像，保留升级后的原卷。`-ValidateOnly` 已实际执行通过，没有修改服务。

最终 `release/data.tar.gz` 已实际恢复到独立空卷并启动旧镜像 `sha256:a174e9fda0c61fe56f727ec46bb1284fd8d69f92fb5220e0d15b7db3b8d57567`，配置、凭据密钥、数据库计数和完整性验证通过。原镜像继续保留；没有在正式服务上执行回滚。

## 资源与清理

Docker 构建与 Windows Rust 编译错开，Cargo 单 job；专属 cgroup 为 memory.max=6 GiB、memory.high=4 GiB、swap.max=0。峰值 4295684096 字节，max/oom/oom_kill 均为 0。构建结束后仅对已空的自建组回收约 2.3 GiB 文件缓存，再移除空组；没有全局清缓存或重启 WSL/Docker。

本次合成验收容器、合成卷、旧停止容器均已按身份和挂载校验后清理；只保留一个 Debug 后端和一个 Gamer release 容器。其它应用容器没有操作。原生产卷、旧镜像、完整备份及此前保留的恢复卷均未删除。
