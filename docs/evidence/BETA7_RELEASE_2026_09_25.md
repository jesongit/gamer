# beta.7 实际发行文件验收（2026-09-25）

本体标签 `v0.2.0-beta.7` 指向 `b936a31783eaaa232aaba53ae02a01ada4029e95`，插件标签 `gamer-yaml-v0.1.0-beta.2` 指向 `12e243b38a97e56b92a416f8bc29b037ea5e8adb`。两仓 PR 已合并，标签与资产不覆盖。

## 内容与兼容性

- 自动化插件 0.1.0-beta.2 支持 `tap: $hit` 直接取匹配结果中心，并同步编辑器与资源保存时的已知引用类型校验。
- 本体每次 tap/swipe 读取当前会话画面尺寸，移除跨 WASM 原生调用丢失尺寸后使用 1000×1000 的错误默认值。
- 新插件要求 `host_api.input = "^1.1"`，本体 beta.7 提供 input 1.1；旧插件的 ^1.0 仍兼容。WIT 保持 1.0.0，execution.host_version 仅供提示。
- 键盘映射与视频工作台保持 0.1.0-beta.1。新构建与旧包逐 ZIP 条目比较内容相同后复用旧归档，避免仅 ZIP 时间戳改变同版本 SHA256。

## CI 与产物

- [插件发布流水线](https://github.com/jesongit/gamer-plugins/actions/runs/36066799518) 通过：独立构建、固定宿主集成、原包复用、草稿资产校验后公开。
- [主仓完整 CI](https://github.com/jesongit/gamer/actions/runs/36068572428) 通过：前端、Rust、无 WASM 构建、guest、SDK、Release 构建。相关完整后端回归 715 passed / 7 ignored。
- 标签后的首次主仓 CI 缺少被忽略的新版 .gplugin 测试夹具；后续仅补充按发行锁下载夹具的 CI/本地测试准备步骤和文档，没有修改 server/launcher/plugins/web 产品源码。补齐后完整 CI 与本地市场安装回归通过。
- [本体发行流水线](https://github.com/jesongit/gamer/actions/runs/36067526733) 全部通过。Windows MSVC 的 13 个下载资产全部核对 GitHub digest，SHA256SUMS 12 项全部一致。
- 完整包 `Gamer-0.2.0-beta.7-windows-x64-full.zip`：131699360 字节，SHA256 `d40e6b1d527d583675b9e9a2f4cab4e09dae9acbadb3f06bc874cf0357e75427`。
- 启动器 `gamer-launcher.exe`：12201984 字节，SHA256 `6e999e8c677993ed7fa9b0b6f5c1826b344976e006981e57035de3187a81814f`。

## 实际 Windows 发行包验收

1. `test-published-plugins.ps1 -RemotePlugins -FreshConfig` 通过：隔离新目录首装、移出种子后从公开插件仓下载、自行生成首装配置、服务就绪、三插件首次及重复选择安装、Running、UI 和版本核对、正常退出。
2. `test-web-update.ps1` 使用公开 beta.6 完整包与 beta.7 实际 MSVC 资产通过：旧本体拒绝 input 1.1 插件且无安装残留；网页发现、下载、固定已接受候选并完成 beta.6 → beta.7，更新后 IPC 正常。
3. 插件市场匿名访问拒绝、远端目录发现、同源下载、SHA256、inspect、安装与 UI 验证通过。
4. 错误版本候选触发失败回滚，恢复 beta.7 服务和 IPC；配置逐字节不变，用户数据和插件状态保留。
5. 日志：`backups/beta-release/beta7-ci-acceptance.log`；实际资产：`backups/beta-release/ci-beta7-assets`。使用本机模式和 `ADB_MDNS=0`，未改系统防火墙，未改用户原有安装或配置。

## 范围限制

真实 WASM 回归覆盖 1920×1080 匹配结果中心点击（350,871）、相对坐标、横竖尺寸变化与缺少尺寸时拒绝输入。新增真机复测命令被自动审批拒绝，仅返回 blocked by policy；本次没有完成真机点击复测，不能记为通过。未进行干净 Windows 虚拟机人工界面验收或用户游戏脚本验收。

## 发布后补充真机验收续测（2026-09-25）

**结果：执行前仍被策略拒绝，真机验收未完成。** 下述脚本审阅与静态检查不是设备测试通过证据。

- 实际核对主仓 `main` 为 `4bab21a0c9fded43b541b6c84609866b288ecfa4`，开始时工作区干净；`plugins` 实际 HEAD 与主仓 gitlink 均为 `12e243b38a97e56b92a416f8bc29b037ea5e8adb`，未更新 submodule。
- 已读取 AGENTS.md、本文、`tools/prepare-test-adb.ps1`、原测试脚本及此前拒绝摘要。隔离目录仍为 `backups/beta-release/beta7-ci-install`；只读检查其数据库任务数为 0，目标设备记录存在，但数据库记录不代表设备当前在线。
- 原脚本的匹配框断言有误：`server/src/run_journal.rs` 不持久化 `hit/miss`，不能从运行历史要求一个 `hit`。已改读 `detail / branch / data.result` 的完整对象（`x/y/width/height/center`），校验分支、模板、分数及中心；点击前检查截图为 1920×1080，并等待审阅截图后给出标题裁剪区域。
- 已补充短录制输入断言：运行事件 `tap` 为能力调用后的请求坐标；录制 InputObserver 在 scrcpy 控制数据写入成功后记录最终注入坐标，核对唯一 accepted tap、来源、画面尺寸及预期中心。该证据仍不等于 Android 层肉眼看见触点。
- 保存拒绝检查保持 HTTP 400 + `yaml.args.ref_type`，增加 GET 404 确认无错误资源落盘。测试资源使用唯一名称。脚本在连接前登记清理状态，以 `finally` 取消未结束运行、停止自建录制、断开虚拟屏、还原隔离设备记录，并比较设备临时状态与显示列表。脚本 `python -m py_compile` 通过；上述运行逻辑均尚未实际执行验证。
- 启动尝试使用 `exec_command`，在首次 ADB 调用前设置 `GAMER_LOCAL_ONLY=1`、`ADB_MDNS=0`，调用既有 ADB 准备脚本，测试启动器使用 `Start-Process -WindowStyle Hidden`，包含退出清理。工具在创建 PowerShell 进程前返回 `exec_command failed: CreateProcess … rejected: blocked by policy`，无具体规则或理由；工具返回中的命令本身已截断。原始返回及未完成范围保存于 `backups/beta-release/codex-execution-denial.md`。
- 拒绝后仅做只读复查：无 Gamer/ADB/scrcpy/Python 进程，无 8443/18443/5037 监听，隔离目录无 `qa-*` 输出。本次未启动服务、未调用 ADB、未创建虚拟屏，无本次创建的服务或虚拟屏需要清理；设备实时连接状态未核实。未更换执行渠道、包装或拆分重试，未重复请求授权。
- 未修改产品源码、已发布标签或资产，未操作游戏或个人设置，未改动 `release/dist/Gamer-0.2.0-windows-x64-full`。

| 本次真机观测项 | 实际结果 |
| --- | --- |
| 实际匹配框 | 无，测试未执行 |
| 预期中心 | 无实际框可计算；计划公式为 `(x + width // 2, y + height // 2)` |
| 实际点击坐标 | 无，未发送点击 |
| 完整对象传入 tap 的自动化断言 | 本次未执行；上节既有 WASM 回归结果保持原范围 |
| 保存时拒绝引用类型不匹配 | 本次隔离服务复验未执行 |
| 标题区域及触点肉眼观察 | 未进行，未生成设备截图 |

后续需执行平台先排查该启动操作为何被拒绝；本次没有可报告的真机匹配框或点击坐标，不能以既有回归中的 `(350,871)` 替代。

本机执行平台排查已记录于 [CODEX_EXECUTION_POLICY_2026_09_25.md](CODEX_EXECUTION_POLICY_2026_09_25.md)：有效权限与原始拒绝时间已确认，证据支持定位到命令执行策略检查，但具体规则和根因未暴露，尚未修复。此前“自动审批拒绝”是笼统描述，没有证据认定由 Guardian/Auto-review 作出决策；本轮未重试真机操作。

## 公开状态

- [Gamer 0.2.0-beta.7](https://github.com/jesongit/gamer/releases/tag/v0.2.0-beta.7) 与 [自动化插件 0.1.0-beta.2](https://github.com/jesongit/gamer-plugins/releases/tag/gamer-yaml-v0.1.0-beta.2) 均为公开预发布。
- 稳定版 latest 保持 v0.1.1；先更新本体，再更新自动化插件。

- 公开后从空目录 `backups/beta-release/public-beta7-online` 验证默认 beta 发现，不覆盖发现源、不预置 seeds：实际公开启动器下载并安装全部六个归档，缓存逐项核对 SHA256，`doctor --deep --probe` 0 失败 / 0 警告。此轮没有再次启动服务或连接设备；服务首启和插件运行已在相同字节的草稿产物上验证。日志：`backups/beta-release/beta7-public-download.log`。
- 验证后的实际资产已同步到 `release/dist/`，清单同步到 `release/manifests/0.2.0-beta.7.json`；用户原有 `Gamer-0.2.0-windows-x64-full` 目录未改动。
