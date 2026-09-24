# 网页更新与独立插件市场验收（2026-09-24）

本地候选 `0.2.0-beta.5`；宿主与启动器源码改动，插件子模块仍固定 `5c88d10f6f0c4358a551b7eb2b4a71c0daf8d9ed`，三款公开插件仍为 `0.1.0-beta.1`。本记录不代表已发布新标签或资产。

## 实现与门禁

- IPC `prepare_install` 使用已下载且复验通过的候选，执行停旧服务、快照、切换、候选身份与就绪验证、提交或自动回滚。CLI 启动器等待事务完成并接管新/恢复服务；保留同一 IPC 会话。
- SQLite 检查可能改变已有 WAL/SHM：改为临时复制数据库和已封存旁车文件后诊断，避免触碰封存备份；不复制媒体等大文件。回归测试让诊断器修改已有 WAL/SHM 并失败，仍能校验、恢复原始字节，临时副本清理正常。
- 默认插件目录改为宿主认证 API，发现官方插件仓库最新公开完整 registry。beta/stable 分流、草稿过滤、固定资产 URL、大小及 SHA256 校验、五分钟缓存、离线回退；归档同源下载后继续现有 inspect/权限确认/安装链。首装发行快照仍锁定。
- 前端正确比较 SemVer 预发布数字字段（beta.10 大于 beta.2），刷新按钮强制检查新目录，网络失败有缓存提示。

## 自动化验证

- 服务端全量：709 passed，7 ignored；其中独立插件市场 4 项离线单元测试通过。
- 显式执行 ignored 的 `published_official_catalog_and_archive_are_downloadable`：空本地目录，从真实公开 GitHub Release 发现目录并下载三款归档，大小/SHA256 全部通过。
- 前端全量：壳 776、自动化 UI 243、视频 UI 7，共 1026 项通过。
- 启动器全量：204 passed，6 ignored；包含本次封存快照副作用回归。
- server/launcher 格式检查与 Clippy all-targets / deny warnings、SDK 固定快照、版本一致性、发行 workflow 静态与 immutable/SBOM 检查通过。

## 真实网页升级与回滚

`release/packaging/test-web-update.ps1` 在全新 `backups/beta-release/beta5-web-update-r3` 执行；旧本体来自公开 beta.4 的 MSVC 完整包，新本体/启动器为本机 GNU 构建。

1. 旧版安装、启动真实服务后，经网页 REST → named pipe 完成检查、下载和安装；版本指针及运行中服务从 beta.4 切到 beta.5。
2. 下载完成后故意修改发现源的版本，安装仍消费已接受的 beta.5 清单；配置 SHA256 和用户数据保持。
3. 新服务再次通过同一 IPC 检查更新成功，验证启动器未随旧进程退出。
4. 新插件市场列表和归档匿名访问均 401；认证刷新确认使用远端发布目录，同源下载三款归档并校验 SHA256，inspect、安装、Running 和 UI 资源均通过。同版本优先使用发行内置字节；纯远端下载另由上一节真实网络测试验证。
5. 提供声称 0.9.98、实际仍为 beta.5 的候选，触发身份不符；自动恢复 beta.5，IPC/服务健康，配置、用户数据和三插件运行状态保留，最后认证退出成功。

首次故障演练发现封存 `gamer.db-shm` 被诊断修改，导致回滚失败；修复前证据保留在 `beta5-web-update-r2`，上面列出的 r3 为修复后通过结果。

## 桌面启动器与完整包

显式执行 `real_desktop_install_repair_restart_update_and_rollback`，最终通过目录为 `backups/beta-release/beta5-desktop-r2`：暂停续装、首装选择插件、重启、损坏修复、校验缓存复用、显式修复保留用户数据、发现更新时不自动启动、错误候选回滚并恢复服务、正常退出均通过。

桌面首次复测暴露了失败提示被接管恢复进程后的 Running 状态覆盖，修复后再次完成上述全部流程。错误保持显示，下一次用户操作才更新状态。

本地完整包：`release/dist/Gamer-0.2.0-beta.5-windows-x64-full.zip`，157912569 bytes；SHA256 为 `af34aef414484b82ccb67f7805f130742d8cf5f4845e5e3d3c6dd8a7c8b6d01c`。包内清单、逐项校验和与 doctor 冒烟通过。此为本地测试包，尚未发布 beta.5 标签/Release。

## 范围

所有测试使用独立目录，未改动用户已有完整包安装和个人数据。本次未执行干净 Windows 桌面的交互验收或 Android 真机投屏/触控/脚本；已有主仓 PR 的合并状态不属于这些测试结论。
