# 配置包发布与公开仓库订阅验收

日期：2026-09-25。目标版本：Gamer 0.2.1 / gamer-package-publisher 0.1.0。

## 真实远端验收

用户指定并授权仓库 `git@github.com:jesongit/gamer-packages.git`（公开仓库）。空仓库初始化 README 后，通过实际安装、启用的 builtin 插件调用，使用本机 gh 登录创建并校验草稿、公开发布、重复草稿调用与重复公开调用。只上传无脚本、无个人数据的 `publisher-smoke`；不控制设备，不启动 Gamer 服务。

- Release：<https://github.com/jesongit/gamer-packages/releases/tag/catalog-20260925103634-14830bee>
- 配置：`publisher-smoke@1.0.0`，441 字节。
- SHA256：`52323a7a1095f98a1c293bca8d2bdddfffa9c9b90c10d9c05d7341f528307f3c`。
- 资产：packages.json、publisher-smoke-1.0.0.gamerpkg、SHA256SUMS.txt；远端逐项 size/digest 校验成功。
- 第一次闭环测试：创建草稿、重试草稿、公开、重试公开断言全部完成，但紧接着匿名发现未找到条目，测试最终失败。此失败保留在 `backups/package-publisher/live-release.log`，不记为完整通过。
- 随后不重复发布，执行独立只读 `publisher_live_subscription`：通过真实 PublicClient/PackageMarket 匿名发现、下载、SHA256/元数据核对和 extract_archive 导入；1 passed。日志 `backups/package-publisher/subscription.log`，warning=null、cached=false。
- 首次暂空具体响应未被旧测试打印，不能确定其根因；后续测试已打印发现结果以便定位。发布后读端可能短暂未同步，需刷新复核，不能因此重复发布。

## 本地自动化验证

- 前端全量：壳 781、YAML 257、视频 7、发布插件最初 2 测试通过（补充确认任务冻结用例后 3/3 通过）；SDK 3 测试通过。后续新增发布权限提示测试，相关前端 8/8 通过。
- Rust package_ 定向：44 passed / 1 ignored；权限、生命周期、API 认证与仓库源 CRUD、归档校验、完整目录合并、版本冲突等。
- Rust clippy --all-targets -D warnings 通过。
- 发布插件独立构建、归档重读与 manifest/size/SHA256 自检通过。
- 插件发行脚本 3/3 测试通过；前端生产构建、版本一致性与 SDK 快照一致性检查通过。
- Rust 全量：729 passed / 10 ignored，0 failed；Windows 默认跳过的设备/端口/人工远端验收不计为通过。
- CI 与正式发行最终结果见下方。
- 额外本地 `cargo test --no-default-features` 在链接阶段因 D 盘空间不足失败；清理项目 Rust 增量缓存的 exec_command 在创建进程前被拒绝（`blocked by policy`，原因未知）。未换渠道或包装重试，未重复索要授权；此项本地测试未完成，记录见 `backups/package-publisher/no-default-tests.log` 和 `cache-cleanup-denial.md`。

## 验收边界

以上是程序断言及实际 GitHub API/归档操作证据，没有将浏览器肉眼操作记为通过。此前 1920×1080 真机中心点击仍未完成；本轮没有重试被执行平台拒绝的设备/服务启动命令，也没有修改已公开 0.2.0 资产或既有安装目录。

## 发行目录预检

- 插件 PR #4 的 Windows standalone CI 构建通过（run 36125913245）；下载 official-plugins 工件后执行 reuse-published，三个旧版 0.1.0 包逐 ZIP 条目内容比对通过，并复用原公开字节。第四个包为 gamer-package-publisher 0.1.0。
- prepare-release.mjs 对四插件目录和 gamer-package-publisher-v0.1.0 标签校验通过。
- 本地重新构建的旧 UI 曾因已有工作区文件 CRLF 与 CI 的 LF 差异导致 Vue scope hash 不同，故本地逐字节复用检查失败；未放宽校验。改用实际 CI 工件后原有严格校验通过。
- 用户明确要求清理 debug 后，同一条缓存清理命令仍被平台拒绝；随后只读观察到 D 盘空闲恢复约 87.6 GiB，属于外部清理，不能计为代理删除成功。关闭增量缓存后无 WASM 测试重跑通过：696 passed / 10 ignored；日志 no-default-retry.log。

- 主仓 PR #5 run 36125918833 全部通过：Rust fmt/clippy/no-default-features check、727 passed / 9 ignored、release 构建，前端与文档构建均通过。插件仓集成首轮缺少旧发行包测试夹具而失败，已补与主仓一致的固定资产下载步骤，未放宽测试断言。

- 插件修复后 CI run 36126682388 全部通过：独立构建、发行脚本 3 项、解释器 20 项，宿主集成 727 passed / 9 ignored，前端测试与构建通过；PR #4 已合并，源码 cc5bc47c5e7c144f98181a35d5bb1ddea9a02a6c。新增 gamer-package-publisher-v0.1.0 标签，发行流水线 run 36127394698 全部通过。

## 正式插件发行

- 2026-09-25T11:13:22Z 公开 gamer-package-publisher-v0.1.0，非 draft、非 prerelease、作为 latest。地址：<https://github.com/jesongit/gamer-plugins/releases/tag/gamer-package-publisher-v0.1.0>。
- 独立下载草稿 7 个资产，核对 GitHub size/digest、6 条 SHA256、registry 来源提交、四款插件 manifest、合集逐字节内容；旧三插件保持原公开哈希。
- 发布插件归档 SHA256：de16d36c1b99c1253992b6d4412b70ce19a9f9dec0d878dc75020c850eb613a9。
- registry：5910 bytes，SHA256 `0bb6dc46da51ec79a61818d352ccb7db22df0ddbf36f2280cc2bcca93dc3c04e`。
- 合集：651425 bytes，SHA256 `753ca576acd4e15010689d1d6142739526469e4ebc012fbbc6e2549cbc1fa470`。
- 主仓 gitlink 与 release/plugins.lock.json 固定到 cc5bc47c5e7c144f98181a35d5bb1ddea9a02a6c；fetch-plugins 严格提交/哈希校验通过，同源快照现包含四插件。

## Gamer 0.2.1 发行

- 主仓 PR #5 已合并，v0.2.1 指向 5b8e49167e20def01cd86ba5f3dc5a8f51d62498；插件 gitlink 保持 cc5bc47，未跟随插件仓后续 README 提交。
- 最终 PR CI run 36128373397 与合并后 main CI run 36129327773 全部通过：Rust 727 passed / 9 ignored、fmt/clippy、no-WASM check、release 构建，前端与文档构建通过。
- 主程序发行流水线 run 36129349542 已通过版本/不可变标签/scrcpy 绑定门禁并创建草稿，Windows 构建、上传、发行资产重新下载校验、启动器 doctor 双跑与公开发行均通过。

- 于 2026-09-25T11:49:32Z [公开 v0.2.1](https://github.com/jesongit/gamer/releases/tag/v0.2.1)，非 draft、非 prerelease，并成为 latest；匿名 API 已复核 latest 和全部 13 个资产的 size/digest 与已校验草稿一致。
- 本地独立下载 13 个资产，核对 12 条 SHA256、两个 manifest 逐字节一致、app 与组件清单、完整包内校验和、四插件 required_files、网页附带目录及插件字节、主程序/启动器副本、插件锁源码与合集哈希，全部通过；manifest 版本检查与 CycloneDX 1.5 / 三个运行依赖的 SBOM 校验通过。
- 完整包：131855727 bytes，SHA256 `4d5cdc8d71a4c5c6b2712679d4e10ef88611f688082c9d0177cbb1551f8f9cbc`。
- gamer-launcher.exe：12195840 bytes，SHA256 `0b56c4af72aae6505d7fa16222070d9a42e4bb933b9919ae477e7338938233e5`。
- 0.2.1.json：5786 bytes，SHA256 `22253ac4ee6989c741fdc7ca7021b359ea27c382223a0cc85be26ecfb7901e70`。
- 本轮两仓开发分支已在合并后清理，未创建 worktree。收尾时本地 main 出现另一个直播功能工作的未推送提交；本验收仅对应 v0.2.1 的固定 5b8e491 / cc5bc47，不将直播提交计入本次发布。
- 未执行新发行包的本机 GUI/服务安装、更新或真机点击，未把此前被平台拒绝的测试改包装重试；旧 0.2.0 标签和资产、release/dist/Gamer-0.2.0-windows-x64-full 保持不变。
