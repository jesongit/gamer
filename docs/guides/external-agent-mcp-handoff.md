# 外部 Agent 改造候选与恢复说明

2026-10-17 03:40 的当前本地宿主已补齐点击卡顿修复：输入不再阻塞 WebSocket 画面/ACK，稳定帧不等待输入锁，Keymap 的直接及间接历史版本扫描均从输入热路径移除。完整工作台同场景实测最长帧间隔 1110.5 → 78.7ms，点击/移动平均帧间隔约 43/41ms，12 次点击全部送达，无断联；点击实际响应平均约 116ms，独立统计且仍有波动。这是独立本地按钮诊断，不是云游戏效果保证。详细改动、边界回归与最终数据复核见 [点击性能记录](../evidence/BROWSER_PREVIEW_INPUT_PERFORMANCE_2026_10_17.md)。当前 Debug PID 18164、SHA-256 `c4ae37fe04f7fd7b2fab186f1ab79ff277f5c1d883b3326c3c71956af8c471a2`；5173 页面及 17324825100 会话已恢复，原脚本/素材/全部历史版本复核保留。下述旧宿主和 ZIP 均为历史记录。

2026-10-17 02:30 后的本地运行修复：页面刷新与浏览器投屏的实测、宿主 hash、前后对照、测试限制及更新前副本，见 [浏览器性能记录](../evidence/BROWSER_PREVIEW_PERFORMANCE_2026_10_17.md)。5173 继续使用当前源码，旧静态目录及审查 ZIP 未覆盖；真实脚本、素材与全部历史版本复核保留。

浏览器帧率修复（北京时间 2026-10-17 02:00 切换）：用户反馈 `17324825100` 静止观看也卡。定位到浏览器预览在显示 ACK 略晚于 33 ms 节拍时，额外丢失一个完整周期。现在按上一帧开始时间限制上限；ACK 到期后直接唤醒发送分支，未到期才等待对应截止时间。一帧在途、迟到 ACK 拒绝、解码恢复预算、FrameStamp 与输入屏障保留，仍是单屏 1920×1080、JPEG quality 80、最高 30 FPS。

独立动态页面的实际前端 Blob/图片/ACK 链路观测约 16.66 → 27.01 FPS，最大间隔约 286 → 80 ms；经 5173 的即时 ACK 约 24.76 → 28.49 FPS，人工延迟 ACK 20 ms 约 15.06 → 21.48 FPS。前端观测使用临时 Edge 和仅针对新建 QA 目标的 WebSocket 管理认证桥，不附加 Inspector 到原账号页面；前后使用同一 JPEG/动画、分辨率和解码代码，但原宿主其他预览的并发负载可能变化，不能解释为严格控制负载的基准或真实游戏 FPS 保证。原账号仅做过只读页面检查/一次截图，未点击进入游戏或发送游戏输入。

完整 Rust 1114/0/47、预览状态机 5 项和显式执行的动态浏览器专项 1 项均通过，原生构建通过；专项覆盖丢 ACK、过期 ACK、解码失败、持续旧消息及 45 ms 延迟显示确认。初版仅重置定时器时测得 35 帧/2 秒，未达原定 36 帧阈值；改为到期直接唤醒后测得 39 帧/2 秒，通过且未放宽阈值。日志为 runtime-start 目录中的 `preview-performance-test-build-final.log`、`preview-pacing-unit-final.log`、`preview-pacing-browser-final.log`、`preview-performance-rust-full.log`、`preview-performance-native-build.log`。

当前 Debug 宿主 PID 13596，359,617,137 字节，SHA-256 `7b3b23cd670988ca4776d3a909d25f723de471020662d0a456b231bca9b81f2e`；7 官方插件正常，已恢复原 `17324825100` 浏览器会话，页面刷新后可重新连接。切换前在正常停机后补存 `before-preview-performance-update/` 下的主库与 Package（库复制前后 SHA 相同）；切换后 94 个 Package 文件与该快照一致，另按 P0 对照复核 770 个保护文件：117 个旧安装版本、87 个工作稿版本、22 条事务及素材均无增删改，当前仍为 122 个安装版本。临时 QA 目标均断开并删除，其独立 profile 留在测量报告目录，没有改动原账号资料。数据库未迁移。

帧率证据：`server/target/external-agent-mcp/preview-throughput-mvb99mbp/`（原链路）、`preview-throughput-mvb9ml6w/`（原实际前端）、`preview-throughput-mvb9y51d/`（修复后）；数据证据为 runtime-start 目录下 `preview-performance-package-preservation.json`、`post-preview-preservation/after.json`。下文候选 ZIP 为此前复查阶段留档，未包含本次后端调度补丁，正式打包时应从当前源码重建。

浏览器加载修复（北京时间 2026-10-17 01:32）：用户报告 5173 白屏，真实 Edge 复现了 Vite 虚拟模块/Windows `@fs` 路径被静态过滤误拦、共享目录不在 Vite 文件允许列表，以及已安装插件 JS 的 `?import` 标记阻止 API 代理三个问题。现已修复：纯虚拟 ID 与合法共享模块正常加载；共享文件只额外放行 `plugins/ui-shared`，不放行整仓；已安装插件模块请求移除 `import` 并保留版本参数。`/public` 和 `@fs` 中的 public 资源别名也复用退休过滤，旧 AI 字节和未过滤 registry 不会经这些入口泄漏。

本次真实 Vite 集成回归 15/15、壳全量 848/848、隔离构建通过。真实浏览器的未登录页和工作台均已渲染，无模块请求失败；工作台诊断仅在临时浏览器替换会话探测结果，其余读取使用实际管理 API，所有写请求及 WebSocket 均被阻止，未使用用户密码/已有浏览器资料，未触发设备或脚本动作。这不是新的真实登录凭据或设备验收。最终浏览器报告/截图在 `server/target/external-agent-mcp/browser-start-diagnostic-1792171927585/`，测试及构建日志在下述 runtime-start 目录的 `web-fix-tests.log`、`web-fix-build.log`。Vite 已自动重载修复，后端和数据未重建。

后续状态（北京时间 2026-10-17 01:14 起）：用户追加授权“直接用真实环境，我简单测一下”后，已使用现有 `server/config.toml` 与 `server/data` 启动本地 Debug 宿主（8443）和当前源码 Vite 界面（5173）。访问 `http://localhost:5173/`；8443 的旧静态页面未替换。该授权后才进行真实启动，下面的“未执行/未安装”描述均为此前候选审查时的记录。

真实启动发现已安装 YAML 0.3.19 仍带退休权限、无法启用，因此通过正常更新 API 安装并加载本轮已验证的 5 个新版本：YAML 0.3.21、Keymap 0.1.6、Video 0.2.6、OCR 0.1.1、Live 0.2.15。旧版本目录保留；Notify 0.1.1 和 Publisher 0.1.0 与候选全部字节相同，无需重复安装。7 个官方插件均为 Running、无 last_error，YAML Runner 已注册；旧 gamer-ai 历史仅保留、执行仍拒绝。

后端 readiness 和浏览器 HTML 均返回 200，6 个插件 JS 的实际 HTTP 响应 hash 与当前安装文件一致。只启动服务及必要的插件更新/启用，未额外触发脚本、直接控制或模型请求；常规启动的 ADB 扫描、设备后台管理和原定时任务按真实配置恢复。候选、封存及原安装版本没有覆盖。启动检查、归档 inspect、更新结果和更新前后 state 快照保存在 `server/target/external-agent-mcp/runtime-start-20261017-011534/`；从此时起真实运行会正常写日志、会话、运行状态等，启动前数据一致性报告不能被解释为运行后所有文件不变。

安装后独立只读核验也通过：P0 的 117 个旧插件版本、507 个旧版本文件原字节一致；当前共 122 个版本，仅新增上述 5 个。共核对 770 个保护文件，正式脚本、87 个工作稿版本、22 条事务、素材、Core 媒体与外置视频无新增、缺失、变化或核验中漂移；未用 SQLite 打开原库或封存库。证据为 `server/target/external-agent-mcp/live-install-preservation-20261017-1792170997612007500/after.json`。同目录 before 报告未捕获到安装前时点，原样保留该标记，不当作安装前快照使用；对照基准仍为 P0 封存。

最新源码修复和启动前验证结果见 [全面复查记录](../evidence/EXTERNAL_AGENT_MCP_REAUDIT_2026_10_17.md)，首次实施见 [阶段记录](../evidence/EXTERNAL_AGENT_MCP_REFACTOR_PROGRESS.md)，连接说明见 [MCP 指南](external-agent-mcp.md)。

## 全面复查后的最新候选

复查补齐 YAML 分支闭包与实际宿主函数权限门禁、OCR 运行所属插件检查、资源锁后撤销检查、历史运行分页、退休 AI 静态资源与发行过滤，以及配置包导入回滚、完整导出和 Trace/媒体历史保留。完整 Rust 回归 1113 通过、0 失败、47 跳过；前端 1021 项通过；默认宿主构建、无 WASM 配置检查、7 插件 pack/verify、39 工具及 39 Schema 验证通过。真实成熟 Agent 客户端、真实目标和长期运行仍未验收。

最新目录：`server/target/external-agent-mcp/candidate-20261017-audit/`。

最新 ZIP：`server/target/external-agent-mcp/candidate-20261017-audit.zip`，71,363,743 字节、46 文件，独立逐文件哈希、ZIP 唯一条目及 CRC 检查通过。

ZIP SHA-256：`f52e551878eb89b200af8d4364e87145d9f8e1a1bd3269664d0cf22c4d272c4b`。

宿主 SHA-256：`38fdf83ca15c49e0dbdae5199fcb7a206557569ec9379c3f367785c32cfeab95`。

本地 Debug 审查候选包含宿主、Core 前端和 7 个插件归档/界面；不含业务数据、配置、凭据、旧 AI 产物或 ADB/FFmpeg 等运行依赖，不是完整独立便携发行包。候选未执行或安装。报告为 `server/target/external-agent-mcp/audit-20261017/candidate-report.json`，逐文件清单在候选内 `candidate-manifest.json`；本轮新归档及打包报告在 `audit-20261017/archives/`。

构建后的最终数据复核为 `server/target/external-agent-mcp/audit-20261017/after-build-data-verification.json`：14,758 个原保护文件字节一致，唯一已分类变化是上述新 Debug exe，旧 exe 在封存中完整保留；data 的 14,363 文件和 5,147 目录无增删改。5 份工作稿的 87 个版本、22 条事务、5 份独立素材和 117 个已安装插件版本完整，主库工作副本的 schema 与全部记录哈希一致。没有用 SQLite 打开原库或封存库。

## 首次本地候选基线（继续保留）

本轮 P0–P6 的本地实施与验证已完成。完整 Rust 回归 1094 通过、0 失败、47 跳过；前端 1018 项通过；默认宿主构建、无 WASM 配置检查、新官方 guest 与 7 插件 pack/verify 均通过。真实成熟 Agent 客户端、真实目标和长期运行没有验收。

独立目录：`server/target/external-agent-mcp/candidate-20261017/`。

ZIP：`server/target/external-agent-mcp/candidate-20261017.zip`，71,289,789 字节，46 文件，CRC 校验通过。

SHA-256：`9009468def74b7ac8973ea6f64036c9a16fc1f64709c3d253dfd09963f5e23ae`。

它包含本地 Debug 宿主、Core 前端和 7 个新插件归档/界面，没有旧 gamer-ai 产物；不包含业务 data、配置、管理员凭据或 ADB/FFmpeg 等运行依赖，因此不是完整独立便携发行包。候选未执行。报告和逐文件 hash 分别在 `server/target/external-agent-mcp/candidate-report.json` 与候选内 `candidate-manifest.json`。

最终保护数据复核报告为 `server/target/external-agent-mcp/original-data-verification-final.json`：data 的 14,363 文件和 5,147 目录集合不变，脚本/素材/所有版本均保持原字节；14,758 个原保护文件 hash 不变。唯一已分类的变化是授权构建产生的新 Debug exe，旧 exe 在封存备份中仍通过原 hash 校验。原 release、静态资源、配置和凭据未改。

## 可审查内容

- 主仓与插件 submodule 的工作树保留原有未提交工作；本次源码以 P0 快照作比较，不把所有 Git diff 都归为本次改造。
- 原生工具采用显式 manifest + 宿主可信契约，39 个官方工具已用真实 Schema 校验与临时打包验证。通知和配置发布没有 MCP 发送/凭据/公开发布工具。
- 最新打包产物位于 `server/target/external-agent-mcp/audit-20261017/archives/`，首次产物仍保留在 `server/target/external-agent-mcp/packer-smoke-p6-guests-12_lbprq/`。复查沿用已验证且字节未变的 YAML、Keymap Component（WIT 未改），重打最新 Schema/UI；两批均未复制到 public 或现有安装目录。
- SDK hash 对齐的是明确标注的 working_tree 快照，基线为真实主仓 HEAD `42abcdef3041dd07fba946730e9e3ebf56d3d378`；这不是已发布提交。正式发行前需整理已获授权的变更，在插件仓提交，再更新主仓 gitlink，并从已提交宿主源码重新导出 SDK。不能伪造 hash 或绕过发行来源校验。
- 正式发行脚本拒绝含旧 AI 的发布集合，必须先发布真实新插件并更新 `release/plugins.lock.json`；历史合集仅用于显式 fixture 回归。发行复制阶段也会过滤私有 stage 的 AI 残留，原 public 与已安装历史保留。本轮没有下载或公开发布新资产。

宿主 host 代码属于本次改造的一部分，只升级插件 UI 不能完成切换。真实 Agent 客户端、Android/浏览器游戏和长期运行另行验收；合成 HTTP、PNG 和 WASM 测试不代表游戏通过。

## 已封存的恢复基线

成功备份：`D:/code/gamer/backups/external-agent-mcp-20261016-223000-v2`。

独立恢复验证副本：`D:/code/gamer/backups/external-agent-mcp-20261016-223000-validation`。

备份共 17,156 文件、12,726,739,405 字节，包含原宿主、静态资源、完整 data、配置与原管理凭据、外置视频、主仓/插件源码及 Git 基线。5 个工作稿的 87 版本、22 事务、5 份独立素材和全部插件安装版本已核对；501 个实际 SQLite 完整性检查通过，15 个原有零字节缓存占位单列。

manifest SHA-256：`30a76b3adca931fd3fcacf26dea8ab97417a8b18ecd0d6e97b0840a4a32f7861`。封存根的 `P0-VERIFIED.json` 和验证报告保留；不带 `-v2` 的第一次失败备份不能用于恢复。

封存目录不能作为运行 data。凭据与浏览器资料保持加密/原字节，不在聊天中输出，也不进入 Git。恢复副本仅用于比对与隔离验证，未启动 Gamer。

## 日后需要回退时

1. 先停止候选的新写入与运行，并等待原有输入和 ActivityLease 收尾。此步骤仅在未来明确部署/恢复授权后执行，本轮未执行。
2. 先为候选使用后的完整数据再次做一致性快照，保存新工作稿、版本、日志、素材及任务；不能用旧快照直接覆盖并丢失升级后的数据。
3. 将同一基线的旧宿主、静态资源、插件安装版本、配置与完整 data 恢复到新的隔离目录，按 manifest 校验文件集合及哈希；SQLite 仅检查可丢弃工作副本，封存库不直接打开。
4. 核对新旧目录、端口和账号资料互不共享。旧 AI 执行链与新 MCP 不得同时控制同一数据或目标。
5. 比较升级后的新增内容，明确保留/迁回方案后才决定正式切换。原盘备份和 P0 封存继续保留，不自动清理任何历史目录。

这份说明是恢复步骤，不会执行恢复或启动服务。当前旧 AI 历史数据库、凭据、素材和全部版本仍在原位置。
