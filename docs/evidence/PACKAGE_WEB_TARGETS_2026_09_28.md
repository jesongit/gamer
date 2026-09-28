# 配置包 Android / Web 适用目标

本次将配置包目标声明扩展到 `[targets.web].url_prefixes`，与 Android 包名并列。匹配属于 Core，插件的输入输出能力和 SDK 不变。插件仓只修改单元测试夹具以符合“必须至少声明一个目标”的约束，无业务插件代码或发行版本变更。

规则、只读目标身份和 API 见 [PACKAGE_TARGETS.md](../reference/PACKAGE_TARGETS.md)。网页取当前绑定标签页的 URL；无连接时明确标记配置网址预估，失效会话不回退活动标签页。

## 验证

- 前端 7 个测试文件、129 项通过，包括网页规则编辑保存、新建、目标切换后过期结果丢弃、未知结果和离线提示、配置包生命周期、API、市场与 Core 边界。
- Rust 全量在 `server/` 目录运行：759 通过、4 失败、16 忽略。本次资源、配置包 API、归档、市场元数据、架构边界相关测试通过。全量未全绿，失败保留在 `server/target/cdp-test/package-target-tests-server-cwd.log`。
- 4 个失败：`official_plugin_market_end_to_end_with_committed_artifacts`（夹具安装的 keymap 为 Enabled）；`yaml_acceptance_export_native_catalog`（前端 Schema 快照不同步）；`downloadable_core_module_requires_explicit_permission_and_sdk_requirement`（测试仍替换 `ui = "^1.0"`，当前 manifest 为 ^1.1）；`video_manifest_parses_with_builtin_execution_and_core_panel`（断言仍是 0.1.0，当前版本 0.1.1）。没有将它们计为通过。
- `node tools/check-plugin-sdk.mjs` 通过：7 文件，固定快照 `7973e186825cec7c4fb3bc2a45f534f74257a6fc`。
- `pnpm build` 已更新 `server/web-dist/`。
- Rust Debug 和 Release 构建成功，最终运行 `server/target/release/gamer-server.exe`（PID 41588，127.0.0.1:8443）；Release 启动后再次确认 5173 代理的云星铁 URL 检查返回 match。
- 主后端 8443 与 Vite 5173 API 验证：网页命中/路径边界不命中/Android 类型不命中/未知 URL 状态；空目标更新 HTTP 400 且 revision 不变；导出归档保留 web 前缀；5 个已安装插件均 Running。
- 本次未打开云游戏页面、未操作账号或设备；没有进行浏览器画面的人工验收。

## 本地数据

检查了主仓数据目录的 3 个配置包，没有空声明。经用户授权，把 `hsr-cloud` 从 Android `*` 改为网页前缀 `https://sr.mihoyo.com/cloud`；另外两个 Android 包保持不变。原 manifest 备份在 `server/target/cdp-test/hsr-cloud-before-web-target.toml`，验证导出在同目录 `hsr-cloud-web-target.gamerpkg`。这些都是本地文件，不提交用户数据。

重启时旧后端已关闭 HTTP 监听但超过 30 秒未退出；在停机前确认无运行任务/录制，核实旧 PID 和 exe 路径后结束该进程，未重置 ADB 或启动浏览器。

## 用户测试入口

刷新 `http://localhost:5173/`，选择云游戏目标和 `hsr-cloud` 配置，打开配置详情。适用目标应显示网页前缀；点击“检查当前目标”，未连接时有预估提示，连接后按绑定标签页判断。编辑时可添加 Android 包名和多个网页前缀；全部清空应拒绝保存。
