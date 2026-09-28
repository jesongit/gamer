# 便携应用内更新验证（2026-09-28）

## 交付行为

- 新完整包入口为 `Gamer.exe`，直接启动服务、托盘和浏览器；不再构建或发布独立启动器。
- 设置页「更新并重启」一次预约下载、空闲等待、安装、重启；预约持久化，安装开始前可以取消。
- 应用归档整体更新前端与后端，运行依赖按清单校验并切换。已安装且没有新增权限的官方配套插件随发行更新；不自动安装未安装插件或第三方插件。
- 更新等待任务、录制、媒体输出结束，再阻止新业务。候选版本通过身份、schema、就绪和前端入口校验后提交；提交前失败恢复快照与旧版。
- 根入口也原子替换；浏览器检测已提交的新构建并刷新。未保存编辑阻止自动刷新，投屏保留设备选择并遵守已有页面接管规则。
- 不实现旧启动器迁移。现有开发服务、用户数据和插件 submodule 没有被替换。

实现和使用说明见 `docs/guides/UPDATE.md`、`docs/guides/UPDATE_CONTRACT.md`。

## 自动验证

| 检查 | 结果 |
| --- | --- |
| `cargo test --locked --manifest-path updater/Cargo.toml` | 182 项通过，无忽略；包括恢复矩阵、归档校验、官方插件权限预检 |
| `cargo clippy --locked --manifest-path updater/Cargo.toml --all-targets --all-features -- -D warnings` | 通过 |
| `cargo test --locked --manifest-path server/Cargo.toml update` | 76 项通过 |
| `cargo build --locked`（server） | 通过 |
| 更新相关 5 个 Vitest 文件 | 80 项通过；包含 available/staged 两个入口的一键更新流程 |
| `pnpm build`（web） | 通过 |
| manifest 验证器自测 | 21 项通过 |
| 发行脚本解析、工作流静态验证、版本一致性、diff 空白检查 | 通过 |

前端测试文件：`system-api.test.js`、`system-components.test.js`、`system-settings.test.js`、`settings.test.js`、`update-recovery.test.js`。

## 实际进程与浏览器验证

`release/packaging/test-portable-update.py` 在临时安装中使用真实 Windows 程序、当前前端和本地校验归档完成：

1. 策略关闭时手动预约，下载、快照、真实退出和重启、版本指针及根入口同步、业务数据保留。
2. 故意使用与程序身份不符的候选版本，验证安装失败后旧服务恢复就绪、版本指针和业务数据保留。

首次完成的证据目录：`C:\Users\Jeson\AppData\Local\Temp\gamer-portable-e2e-xumhb6g3`。基线使用当前程序配较旧的安装版本指针，验证更新事务；不声称验证历史二进制兼容性。错误候选为测试制造，非真实发行。

最终程序和前端重新构建后又完整复跑一次，两条路径均通过，退出码 0；证据目录：`C:\Users\Jeson\AppData\Local\Temp\gamer-portable-e2e-_tdq9x99`，隔离端口 18462。浏览器测试服务（18460）及 E2E 服务均已关闭，原开发服务保留。

`release/packaging/test-portable-ui.cjs` 使用本机 Edge 无头浏览器打开真实设置页，验证便携模式、available 状态点击更新后确认并仅提交一次预约、弹窗自动关闭、页面无运行异常，以及新构建提交后自动刷新。浏览器测试通过拦截接口模拟预约受理和后续构建身份，实际更新 API 与进程重启另由上述 E2E 验证。截图：`release/dist/portable-settings.png`。

`package-full.ps1` 完整包烟测通过：v2 清单、153 项文件哈希、根入口与版本目录程序一致、实际服务就绪、前端可访问、更新能力开启和退出。测试包目录：`C:\Users\Jeson\AppData\Local\Temp\gamer-package-smoke-aa2cbf59c37f433eab6070b08b49c31c`。这是本地 debug 验证包，未包含官方插件归档，未作为正式发行发布。

## 验证边界与清理限制

- 没有实测物理 Android 设备在升级后的投屏重连，也没有在真实安装中执行新版官方插件归档更新；后者有权限预检与宿主更新逻辑检查，不等同于完整发行集成验收。
- 宿主全量严格 Clippy 仍被仓库已有问题阻止：`browser/session.rs` 的 `collapsible_if`、`browser/transport.rs` 的 `type_complexity`、`recording/browser.rs` 的 `manual_is_multiple_of`、`media/output_tests.rs` 的两项 lint，以及插件 submodule 的 `gamer-live/host/protocol_tests.rs` 的 `await_holding_lock`。没有修改这些无关文件或更新 submodule。
- 旧启动器 crate 入口、构建配置、相关打包脚本与旧指南已移除。自动删除剩余文件和缓存曾被执行策略拒绝；用户随后手动删除整个 `launcher/`，已重新确认目录不存在，并移除不再需要的缓存忽略规则。
- 没有发布 release、创建 tag 或替换用户正在运行的安装。原有未提交文档修改及 `server/data/` 保留。
