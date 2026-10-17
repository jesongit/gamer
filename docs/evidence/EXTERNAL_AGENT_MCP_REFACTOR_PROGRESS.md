# Gamer 外部 Agent 与统一 MCP 改造进度

实施日期：2026-10-16 至 2026-10-17。依据：[已确认改造计划](../plans/gamer_external_agent_mcp_refactor_plan.md)。

本地开发与隔离验证已获授权。暂不部署、不更新安装、不操作真实设备和实际游戏目标。保留工作区原有未提交改动；本次改动以 P0 源码快照为比较基线。

后续全面复查发现的遗漏及补充修复见 [2026-10-17 复查记录](EXTERNAL_AGENT_MCP_REAUDIT_2026_10_17.md)。下列数字和候选哈希是首次本地交付基线，后续复查结果单独记录，原候选和证据不覆盖。

## 阶段状态

| 阶段 | 状态 | 结果 |
| --- | --- | --- |
| P0 完整备份与隔离恢复验证 | 完成 | 17,156 文件、12,726,739,405 字节；501 个实际 SQLite 完整性通过；87 个工作稿版本保留 |
| P1 确定性服务拆分 | 完成 | 默认编译和工作稿、试跑、动作目录、候选过滤回归通过，持久路径不变 |
| P2 中立 MCP 与外部控制 | 完成 | 独立令牌/收据/管理界面、控制屏障与冻结执行策略通过最终回归 |
| P3 插件工具契约与主工作流 | 完成 | 显式工具、Schema、YAML/素材/撤销回归通过；修复旧 WASM 线程异常，9 项真实合成 guest 测试通过 |
| P4 其他插件与界面收敛 | 完成 | 5 插件共 39 工具；前端 1018 项通过，7 插件隔离 pack/verify 通过 |
| P5 AI 执行与发行移除 | 完成 | 旧装配/模型/生成循环/AI UI 移除；历史数据及退休权限元数据保留；发行副本过滤旧 AI 产物 |
| P6 完整回归与候选交付 | 本地完成 | Rust 1094 通过/0 失败/47 跳过，双配置检查、候选 ZIP 与数据复核完成；真实客户端/设备验收未执行 |

## P0 完整备份与隔离恢复验证

### 备份范围

封存位置：`D:/code/gamer/backups/external-agent-mcp-20261016-223000-v2`。

恢复验证副本：`D:/code/gamer/backups/external-agent-mcp-20261016-223000-validation`。

| 范围 | 文件数 | 字节 |
| --- | ---: | ---: |
| 完整主 data | 14,363 | 11,519,230,887 |
| 原宿主、静态资源与 assets | 393 | 582,487,217 |
| 外置轮播视频 | 1 | 423,720,604 |
| 主仓和插件仓源码现状 | 2,383 | 194,945,971 |
| Git 索引、状态、HEAD 和补丁基线 | 14 | 6,352,538 |
| 配置与原始管理凭据文件 | 2 | 2,188 |
| 合计 | 17,156 | 12,726,739,405 |

备份包含正式 YAML、函数、模板、全部工作稿版本、发布锚点和事务 before 内容、独立素材库、媒体和录制历史、数据库及 WAL/SHM、浏览器资料、AI 私密历史和完整插件安装版本。原 candidates、retained-traces 为空，空目录也已记录。

备份根 ACL 限制到当前 Windows 用户与 SYSTEM。密钥、DPAPI 文件和浏览器资料原样复制，不解密、不输出、不纳入 Git。

### 一致性与验证证据

1. 默认配置指向 `server/data`。核查时 8443 未监听，主库无未完成运行，相关进程没有打开主数据文件；源队列文件稳定。
2. 另有两个既存候选服务使用 `server/target/live-daily-qa/v5/data` 与 `server/target/audience-feedback-20261015/data`。它们不使用本次主数据目录，未停止、未重启、未调用其接口，且未纳入主数据离线快照。构建缓存内的这些辅助环境继续原样保留。
3. 完整复制后再次枚举源文件和目录，逐文件重新计算源与副本 SHA-256，校验大小、时间和集合；Git 基线也复核一致。无跳过文件或重解析点。
4. 独立恢复副本的全部文件再次逐字节核对。SQLite 仅打开带旁车的短路径工作副本，封存和原始数据库不由验证器打开。
5. 501 个实际 SQLite 数据库 `PRAGMA integrity_check` 均返回 `ok`。15 个原有零字节 DawnGraphiteCache/cache.db 为非 SQLite 占位文件，严格按已验证路径和零字节特征单列；其他空数据库不会被忽略。
6. 5 份工作稿包含 87 个版本，22 条版本事务，5 份独立素材均已记录；5 份素材 ZIP 的全部条目 CRC 检查通过。

首次验证报告把空缓存占位误归为数据库异常，原报告保留；新验证器对上述 15 个文件重新分类。501 个真实数据库的通过结果沿用首次实际检查，没有修复或更改数据库内容。最终结果为 `validation-report-classified.json`，封存根另有 `P0-VERIFIED.json`。

manifest SHA-256：`30a76b3adca931fd3fcacf26dea8ab97417a8b18ecd0d6e97b0840a4a32f7861`。

首个备份尝试因目标长路径失败，保留在不带 `-v2` 的目录，不能用于恢复。随后采用 extended Win32 路径，并用 354 字符源路径、330 字符备份根和 617 字符验证路径的合成 SQLite 用例验证。备份与验证 helper 分别封存在成功备份根，便于复查。

### P0 验证边界

已验证离线字节保留、恢复副本一致性、SQLite 完整性和数据清单。未启动恢复后的 Gamer、未访问账号、模型或真实目标，也未做正式部署。实施与后续回归只使用临时数据或另行创建的隔离副本，封存备份不可作为运行数据目录。

## P1 确定性服务拆分

### 改动

- YAML 工作稿成为独立 `AuthoringService`，私有试跑成为 `PrivateTrialRuntime` / `PrivateTrialExecutor`；工作稿创建、导入、保存、版本读取、校验、素材检查、试跑、发布管理和历史读取继续复用原实现。
- 删除工作稿的 `authoring.generate` 和专用模型/附图/提示词包装；候选模型循环留到 P5，在替代工作流完成后移除。
- `GenerationService` 暂时仍作为唯一 builtin 门面，工作稿和旧候选共享同一 gate、jobs、诊断和私有运行实例，避免同 ID 多服务分派冲突。
- Document / Version 用 flatten 原样承载未知元数据。保存不改变既有 authoring、revisions、authoring-published、candidates、素材和模板的存储路径。

### 验证

- `cargo check --manifest-path server/Cargo.toml --locked --bin gamer-server`：默认 WASM 配置通过。
- `cargo test --manifest-path server/Cargo.toml --locked --bin gamer-server authoring -- --test-threads=2`：14 项通过，包含独立无模型装配、历史未知字段/模板原字节保留、共享 jobs/gate 与生命周期回归。
- 使用同一次成功构建的测试可执行文件继续运行过滤集：private_trial 1 项、actions 8 项、generation 48 项通过；过滤集可能包含重叠测试，不按简单相加计算独立覆盖数。
- 2 项原有环境验收测试 ignored，未执行真实浏览器/WASM 试跑与本地模型证据 helper。首轮测试编译有一处模块路径错误并遇内存分配失败；路径修复后默认测试重跑成功，无需系统内存配置修改。
- 未启动 Gamer 服务、未连接设备、未调用真实模型；没有迁移、清理或重写业务数据。

## P2 中立 MCP 与外部控制

已接入独立 `/api/mcp`、管理认证的令牌和外部控制 API，以及 Core 工具适配。协议模块使用独立 Bearer 和真实回环 peer；令牌按设备、Package、插件、Runner、run/task/media 范围检查；有副作用操作先持久预留 operation_id 收据。直接控制仍经 Core 控制屏障、FrameStamp 与 RunManager。

默认 WASM 编译已通过。新中立协议/Core 22 项回归通过（从包含旧 AI 测试的过滤集中单独计数），覆盖真实回环/Bearer、通知/405、未知收据不重放、跨令牌/Package/嵌套上下文拒绝、正式受理 ID 所有权、任务写不授执行。外部控制另有 8 项通过，包含精确 token 绑定、旧代次/暂停拒绝、收尾失败屏障、TTL 暂停与用户恢复保留同一 run_id。

第一轮 Core 测试有两处 fixture 错误：不同参数重复使用 operation_id（被正确冲突拒绝）、cron 字段误用 expr 而非 expression。修正 fixture 后这两项通过，没有放宽权限或协议。

设置新增中立令牌与控制区，空对象范围默认不授权，密钥只显示一次且卸载组件清空；不自动开始/恢复会话。正式运行和任务用宿主冻结的 ExecutionPolicy 持久传递，Runner 默认拒绝外部受限执行，JSON payload 无法构造该权威字段。最终策略、精确对象授权与并发撤销回归已纳入 P6 全量通过结果。

## P3 插件工具与主工作流

manifest 显式 tools + 离线 JSON Schema + scope/permission/effect/context；动态目录仅接入 Running 的实际安装版本，不自动导出 admin call、UI 按钮或原生函数。Builtin 显式注册语义 handler；第三方 WASM 每调用独立 guest/Store/句柄，复用 Component 缓存。缺少安全控制租约适配的低层 input/touch/app/run imports 保持隐藏和拒绝，是当前支持范围。

YAML 提供 18 个明确工具，素材提供 8 个。工作稿锁后及真实写入/试跑提交前再次核查撤销；私有运行注册临时工作，令牌撤销/到期取消并等待真正运行槽清理。正式运行使用冻结策略和同一权威解释器，检查可达函数闭包，拒绝未开放通知、直播确认/切换和推流。旧候选只读/原字节分页导出，显式导入新稿；读取旧文档不顺便写身份或恢复事务。

阶段验证：external_tools 5 项、private_trial 2 项、legacy 1 项、video::sample 4 项、authoring 16 项通过（部分含将退休旧测试，最终以 P6 后的新集为准）。新 scope/资源/撤销/字节导出与裁图测试均只用临时数据和合成 PNG，没有真实设备/模型。

通用 WASM 的句柄授权测试通过，但真实合成 guest 测试虽返回 ok 却出现两次 Tokio/Fiber 收尾 panic。审查确认旧 catch_unwind + 忽略 join 会隐藏线程异常，故不把该项记为验收通过。正在修复线程收尾并使 panic 传播，之后重跑全部通用 guest 测试。

后续修复与验证：通用 Component 使用专用普通线程的同步调用，异步 capability I/O 通过宿主 Tokio Handle 桥接，不再创建 Wasmtime async Fiber。stop 等待实际线程/Store 清理，传播 panic/错误，CPU entry 可按 store 取消中断。9 项 generic WASM 测试全部通过，包含异步 sleep import、两令牌独立上下文/内存、句柄再授权、实际线程错误不冒充成功与 CPU 中断。先前 Tokio 收尾断言未再出现。

Builtin 外部 handler 还提供编译期 trusted_manifest；目录、安装和调用核对真实 action/tool_id 的 effect、权限、scope 和 context。安装 manifest 不能把写伪装为读，也不能删掉真实需要的插件权限。对应篡改/生命周期和官方实际 Schema 校验一并通过，新 external_tools 过滤集 9 项通过。

## P4 界面与构建配套

- 自动化保留脚本库/模板/工作稿与版本/历史候选/日志；去掉模型 readiness、生成/分析/修复按钮与 AI 消息桥。失败版本严格使用运行时冻结源码，不回退最新版本；新增复制 Agent 引用。
- YAML UI 全部 97 项通过，另新增历史候选只读/显式导入/迟到切包 2 项通过。视频 UI 19 项通过，壳新增接入测试 2 项通过；壳全量首次 832/838，通过修正过时按钮/状态文案期望后对应 15 项全部通过，全量复跑待最终源码稳定。
- 7 官方插件在临时目录实际 pack → verify 通过，共 39 个 tools（YAML18/Video8/Keymap4/OCR2/Live7；通知/发布为0）。报告：`server/target/external-agent-mcp/packer-smoke-ewfbwzby/smoke-report.json`。未写 public、安装归档或启动服务。
- packer 3/3、发行脚本 9/9 通过；JS/PowerShell 静态解析通过。SDK 为真实 HEAD 的 working_tree 快照，7 文件双向 hash 通过；正式发行必须提交后重新导出，不能将该快照当作已提交来源或伪造发布哈希。
- 独立审查修复 Live 规则保存与管理绑定变化之间的竞态，新增锁后目标/撤销回归；同时补强互动规则异步触发时的受限执行策略传播。最终测试未完成前不标记阶段完成。

最终界面/归档结果：壳 91 文件、838 测试全过；YAML 12 文件、99 测试全过；Video 19、Live 56、Notify 3、Publisher 3 通过。Keymap 当前 UI 项目没有测试文件，明确不计作测试通过。全部现有插件 UI 和隔离壳 build 成功。Impeccable detect 返回空发现，另有一条 RunDetails 目标路径拼写警告，该文件未据机械检查宣称通过；其组件/日志回归和插件 build 已覆盖。

桌面 1440×1000 与窄屏 390×844 对设置、工作稿和旧候选进行合成页面实拍检查，6 份均无页面异常/横向溢出；手工检查了主要页面。前端预览仅使用临时 Vite、mock API 与独立 headless Chrome，所有 API 网络请求拦截，检查后关闭，无 Gamer 后端/账号/设备连接。证据位于 `server/target/external-agent-mcp/ui-qa/`。

官方 YAML/Keymap guest 均实际完成 `--locked --release --lib --target wasm32-unknown-unknown`，各自 componentize 的 `ComponentEncoder.validate(true)` 通过。新 Component 与最新 UI 再次对 7 插件实际 pack/verify，39 工具/39 Schema 全通过，归档内 WASM hash 与新 Component 一致。最终报告为 `server/target/external-agent-mcp/packer-smoke-p6-guests-12_lbprq/smoke-report.json`，guest 报告为 `official-guest-p6-report.json`；替代前面复用缓存 guest 的打包结果。

## P5 移除 AI 与保留历史

- 删除 gamer-ai host/provider/conversation/search/memory加工/MCP/模型配置和 UI 源码，移除 main 装配、Runner registrar、builtin 注册和旧插件 MCP 路由；删除 YAML 模型生成、修复及自主循环，保留确定性 authoring、私有执行、素材验证和原路径历史门面。
- 删除仅 AI 消费的 jpegli、sqlite-vec、jieba-rs 依赖及对应 lock 项；保留 OCR 识别引擎和语言资源机制，它们不属于模型 Agent 编排。
- 旧 `ai.connect` 严格安装拒绝；installed reader 保留 retired_permissions 供展示，从有效权限集合移除。伪造 Running 状态也不能执行；旧 gamer-ai 明确宿主实现不可用，旧 YAML 需更新。历史不可变 manifest/所有版本 hash 不改，7 项相关退休权限/历史安装回归通过。
- 市场读端过滤旧 catalog 中的 AI 条目，不重写 public 下的历史归档和原安装数据。默认构建/发行不含 AI；工作树 SDK 继续明确阻止冒充已提交来源发行。
- README、AGENTS、PRODUCT、当前 YAML/插件参考及教程改为外部 Agent 和工作稿流程；旧设计合同标为被新计划取代，历史证据不批量抹除。新增 [MCP 接入指南](../guides/external-agent-mcp.md)。

发行卫生补强：`package-app.ps1` 只过滤复制后的私有 stage 内旧 AI UI、归档与目录条目，先校验绝对路径边界并拒绝重解析点；原 public/web-dist 不变。`fetch-plugins.ps1` 正常发行拒绝仍含 gamer-ai 的旧发布锁，要求真实新插件发布与锁更新；只有显式 `-TestFixturesOnly` 可以使用旧合集作退休能力回归。没有伪造新资产地址或哈希，也没有实际下载/发行。

`-TestFixturesOnly` 不是 dry-run，完整脚本仍会下载并写 public/输出目录；本轮仅在临时目录抽取纯辅助函数做合成验证，没有执行该完整入口。

`release/packaging/test-package-app-retired-artifacts.ps1` 的合成测试在 PowerShell 7 与 Windows PowerShell 5.1 均通过，覆盖源/其它产物字节保留、重复执行、缺 registry、junction 越界拒绝及正式/fixture 发布集合策略；三个脚本 AST 解析通过。一个早期兼容性测试的纯合成 Temp 目录清理被自动审批拒绝（仅返回 `blocked by policy`），已保留在 `C:/Users/Jeson/AppData/Local/Temp/gamer-retired-stage-43a7f59e8486477898726492a3b2bdfb`；其 junction 已移除，不影响业务数据、最终验证或候选。

## P6 本地回归与保护数据对照

宿主最终构建前，原始数据/配置/宿主资源/外置视频/管理凭据共 14,759 文件、12,525,440,896 字节重新核对 P0 manifest，全部 hash 一致；data 文件集合 14,363、目录集合 5,147，新增/缺失/变化均为零。没有打开原 SQLite、解密密钥或执行迁移。报告：`server/target/external-agent-mcp/original-data-verification.json`。

最终构建后再次对照同一封存 manifest：14,758 个保护文件仍逐字节一致，唯一预期变化是本次授权构建生成的 `server/target/debug/gamer-server.exe`（356,117,425 → 359,071,304 字节）。新 Debug hash 精确匹配候选，原 Debug exe 在 P0 封存副本中的 hash 再验通过；原 release exe、静态资源和配置未变。data 的 14,363 文件及 5,147 目录集合仍无增删改，全部脚本、素材和版本保持原字节。最终报告：`server/target/external-agent-mcp/original-data-verification-final.json`，结果 `ok:true`。

最终审查新增 Core 任务原子 compare-and-save：在 IMMEDIATE 事务中比较完整已授权快照，锁后再检查令牌；启停直接提交该快照，不再次按 ID 操作已换域任务；取消仅作用于已核验的冻结 child run_id。执行派发重新检查真实 Runner owner 与冻结授权。MCP body 在缓冲前检查真实来源和 Bearer，缓存收据在 await 后也复查撤销。这一批次连同 OCR 异常收尾、Live 冻结规则授权均已通过最终全量回归。

首次全量预检用测试 exe 在仓库根运行，19 项相对路径 fixture 因工作目录错误失败。另有架构组合根白名单及 Video 版本期望过时、旧市场测试仍期望退休 AI 归档可安装，共 3 项已修正测试契约。原报告保留为 `rust-regression-preflight.log`，不能把首轮 1062/22/47 结果称为全量通过。

### 最终验证结果

| 检查 | 实际结果 |
| --- | --- |
| 默认宿主测试编译 | `cargo test --locked --manifest-path server/Cargo.toml --bin gamer-server --no-run` 通过 |
| 完整 Rust 回归 | 从 `server/` 执行最终测试 exe，1094 通过、0 失败、47 ignored，耗时 236.77 秒 |
| 默认本地宿主构建 | `cargo build --locked --manifest-path server/Cargo.toml --bin gamer-server` 通过；生成 Debug exe，未执行 |
| 无 WASM 配置 | `cargo check --locked --manifest-path server/Cargo.toml --no-default-features --bin gamer-server` 通过，最终无警告 |
| 前端测试 | 壳 838、YAML 99、Video 19、Live 56、Notify 3、Publisher 3，共 1018 通过；Keymap 无测试文件，不计通过数 |
| 前端构建与视觉 | 隔离壳及现有插件 UI build 通过；6 份桌面/窄屏合成页面无异常与横向溢出 |
| 插件 SDK | 7 文件双向 hash 对齐；快照为 working_tree，正式发行门禁仍拒绝 |
| 官方 guest / 归档 | YAML、Keymap 新 guest 构建及 Component 校验通过；7 插件 pack/verify，39 工具/39 Schema 通过 |
| 发行脚本 | packer 3 项、发行 Node 9 项、JS/PowerShell 静态解析、双 PowerShell stage 合成回归通过 |

Rust 最终日志：`server/target/external-agent-mcp/rust-regression-final.log`。覆盖真实回环 HTTP 客户端 initialize/通知/目录/PNG/撤销、数据库写锁后撤销、完整 Task 快照竞态、Runner owner 更换、控制代次与租约、WASM 实际线程终止及历史数据保留。47 项 ignored 没有执行，不能算通过；真实游戏、Android/浏览器控制、实际成熟 Agent 客户端和长期运行仍待另行验收。没有调用模型或真实提供方。

### 独立本地审查候选

候选目录：`server/target/external-agent-mcp/candidate-20261017/`。

ZIP：`server/target/external-agent-mcp/candidate-20261017.zip`，71,289,789 字节，46 文件，解压 CRC 检查通过。

ZIP SHA-256：`9009468def74b7ac8973ea6f64036c9a16fc1f64709c3d253dfd09963f5e23ae`。

宿主 SHA-256：`93ad03b4bb8882a4e3cc8af6be6a2899368b6248bb4809973cc0141efcfc52aa`。

包含本次本地 Debug 宿主、隔离壳与 7 个新插件归档/UI；不含业务 data、配置、管理员凭据或 ADB/FFmpeg 等依赖，不是正式独立便携发行包。组装时排除旧 public 插件目录，按实际新归档哈希生成带 working_tree 来源的本地目录；无 gamer-ai 文件。报告为 `server/target/external-agent-mcp/candidate-report.json`，文件清单在候选内 `candidate-manifest.json`。未启动、安装或部署候选。

后续正式发行前仍须整理提交、重新导出已提交 SDK、发布真实插件资产并更新固定发行锁；本轮未提交或推送。恢复与升级后的新增数据保留要求见 [交付与恢复说明](../guides/external-agent-mcp-handoff.md)。
