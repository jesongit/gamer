# beta.6 实际发行文件验收（2026-09-24）

发行提交为 `03c38c7293fcad3d5a725dc77c4dcf0d198c6c94`，标签 `v0.2.0-beta.6`。后续提交仅补充验收文档，产品代码与标签一致。beta.5 已取消发布，保留草稿、标签和构建资产追溯，不覆盖。

## CI 与产物

- [主仓 CI](https://github.com/jesongit/gamer/actions/runs/35983883689) 全绿；Linux 服务端 708 passed / 7 ignored，前端、SDK、静态检查、guest 与 release 构建通过。Windows 本地回归另见 `LOCAL_ONLY_DEVICE_2026_09_24.md`。
- [发行流水线](https://github.com/jesongit/gamer/actions/runs/35983298691) 的 Windows MSVC 构建、上传及下载后校验通过，随后才在本机下载并验收实际草稿资产。
- 本机目录 `backups/beta-release/ci-beta6-assets`：13 个文件、SHA256SUMS 中 12 条全部匹配，并额外核对全部 13 个文件的 GitHub asset digest。
- 完整包：`Gamer-0.2.0-beta.6-windows-x64-full.zip`，131679976 字节，SHA256 `1fe34491c50d70748de8bbfbe813dc8f920d04c16abfed6140a952d3074d42fc`。
- 独立启动器：`gamer-launcher.exe`，12200960 字节，SHA256 `271e880221631116443cacce4b86048d658b9c7645f7691fbd5cb0900c0ba39c`。

## 实际 MSVC 文件验收

1. `test-published-plugins.ps1 -RemotePlugins -FreshConfig` 在 `backups/beta-release/beta6-ci-install` 通过。移出插件种子后从公开插件仓下载；无配置首启生成完整配置；三款插件首次及重复选择安装、Running、UI、本体版本和正常退出全部通过。
2. `test-web-update.ps1` 使用公开 beta.4 MSVC 完整包和本次 CI beta.6 实际启动器、清单及组件。在旧路径 `beta5-web-update-r3` 重建隔离安装，以复用已有防火墙规则；原 beta.5 成功证据已完整归档到 `beta5-web-update-r3-archived`。
3. 网页 REST → IPC 下载、接受候选、实际切换 `beta.4 → beta.6` 通过；下载后修改发现源不改变安装候选，更新后 IPC 仍可检查版本。
4. 插件市场匿名请求被拒绝；独立仓目录刷新、同源下载、SHA256、inspect、安装及 UI 全部通过。
5. 声称 `0.9.98`、实际为 beta.6 的错误候选触发身份校验失败，自动恢复 beta.6，服务及 IPC 存活，配置逐字节保持、用户数据和三插件状态保留，最后正常退出。

## 真机范围与防火墙

同版本本地 GNU 构建已完成 Android 16 实际投屏、截图、DataChannel 休眠/唤醒，详见上一份证据。本次追加实际 CI 文件真机复测时设备已不在 ADB 列表，未将该次记为通过；隔离测试进程及浏览器已清理，服务经认证关闭。未执行游戏脚本或干净 Windows 桌面人工交互。

新程序验收使用 `GAMER_LOCAL_ONLY=1`，HTTP 与 WebRTC 同时使用回环地址。旧 beta.4 不支持该开关，复用已有规则的测试路径处理；未关闭防火墙或修改系统规则。

## 公开结果与正常在线流程

- [v0.2.0-beta.6](https://github.com/jesongit/gamer/releases/tag/v0.2.0-beta.6) 已公开，`isDraft=false`、`isPrerelease=true`，13 个资产；完整发行流水线成功，stable latest 仍为 `v0.1.1`。
- `backups/beta-release/public-beta6-online` 从空目录验收，无清单覆盖、无 seeds、无配置：默认 beta 发现找到 beta.6，下载公开启动器并按清单校验，真实 EXE 下载全部六个归档并逐项核对缓存 SHA256，自行生成首装配置后服务就绪。
- 通过正常启动器选择流程安装三款插件，版本均为 `0.1.0-beta.1` 且 Running，UI 可访问；网页检查更新确认当前 beta.6，无新候选；最后认证关闭服务，启动器正常退出。日志：`backups/beta-release/beta6-public-install.log`。
- 验证后的 13 个实际 CI 资产同步到本地 `release/dist/`，对应版本清单同步到 `release/manifests/0.2.0-beta.6.json`；未改动用户原有 `Gamer-0.2.0-windows-x64-full` 安装目录。
