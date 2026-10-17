# 外部 Agent 与统一 MCP 改造复查

日期：2026-10-17。范围为已确认改造的源码、构建/发行入口、MCP/Core/插件调用链，以及 P0 封存和原始业务数据。复查发现的问题直接修复；不部署、不更新现有安装、不启动生产服务、不操作真实设备或实际模型。

后续追加授权说明：本报告记录真实启动前的审查结果。用户之后明确要求“直接用真实环境”进行手动检查，已于北京时间 2026-10-17 01:14 起启动真实配置和数据，并新增安装 5 个已验证插件版本；启动状态、边界与证据见 [交接说明](../guides/external-agent-mcp-handoff.md)。此后正常运行数据会变化，不能将本报告启动前的零变化核验外推到运行后。

原实施结果见 [阶段记录](EXTERNAL_AGENT_MCP_REFACTOR_PROGRESS.md)，接入方式见 [MCP 指南](../guides/external-agent-mcp.md)。工作区包含此前未提交工作，未把全部 Git diff 当作本轮新增修改。

## 发现与修复

| 范围 | 发现 | 修复与验证要求 |
| --- | --- | --- |
| 前端产物与开发入口 | 保留在 public 的旧 AI 文件仍可能被普通 Vite 构建复制或被开发服务器访问 | 只过滤输出和响应，保留源快照；按准确插件目录/归档映射判断，避免误伤相似插件名 |
| 历史插件测试 | fixture 获取入口仍会写共享 public 和默认发行目录 | 将历史测试合集/cache 隔离到 target 或明确的临时根，测试显式读取；禁止把该开关当作 dry-run |
| 插件打包 | packer 对退休权限/旧 AI builtin 的检查弱于宿主安装边界 | pack、inspect、verify 统一拒绝退休权限和旧 builtin，并同步 SDK 镜像 |
| OCR 证据授权 | 读取 Trace 核验了运行/目标/Runner/Package，但缺实际 Runner owner 插件核验 | 对齐 Core：当前 owner、匹配的冻结 owner 或明确全插件范围；补越权与历史缺依赖回归 |
| Keymap/Live 写入 | 等待 Package 资源锁期间撤销，部分路径仍可能落盘 | 将有效性检查放到实际写入锁之后，补等待锁期间撤销的合成回归 |
| Core 历史运行 | 缺 Runner 时，不完整或不匹配的 ownership receipt 可能跳过 owner 要求 | 仅接受实际 Runner 匹配且非空插件 owner 的冻结记录，否则要求明确全插件范围 |
| Core 历史分页 | 整个原始页均被授权过滤后，没有游标可继续找到较早的授权记录 | 返回扫描页的 next_before；即使 runs 为空仍可继续，游标不授予运行访问权限 |
| YAML 工具 Schema | 私有模板读取指定 script_id 时，Schema 未同步要求 expected_version | Schema 与实际固定版本读取契约一致，避免目录提供无法成功调用的参数组合 |
| YAML 受限执行 | 模板匹配及轮询的 cases[].do 未进入闭包检查，且正式运行校验晚于设备准备 | 按权威解释器全部指令种类扫描分支与函数闭包，未知指令拒绝；设备准备前冻结并检查，执行只消费同一快照 |
| YAML 宿主派发 | 替换的 guest 若无视冻结程序，实际 __fn 派发未再次核验外部策略 | 以 Rust 类型将 ExecutionPolicy 贯穿到实际原生函数入口，静态闭包与宿主派发复用同一授权函数；JSON 无法清空或伪造策略，WIT 不变 |
| 素材范围与快照 | YAML 间接读取 Video 素材少了所属插件检查，Video 分块导出校验与读取分离 | 补实际插件授权；导出从同一归档字节快照校验身份/指纹并切块 |
| 配置包导出 | 子目录枚举错误被吞掉，且任意层级的 package.toml 被省略 | 枚举错误必须使导出失败；只由顶层 manifest 单独打包，插件内部同名文件原样保留 |
| Trace 启动恢复 | 无法解析的已有 index 可能使其 PNG 被误判为孤立文件 | 不可解析/未知格式索引对应目录保留原字节；不因读取失败清理既有证据 |
| 配置包导入 | 冲突检查晚于媒体引用写入，失败回滚可能不能恢复旧引用、目录及部分发布的预设 | 冲突检查提前，媒体/目录变更可回滚；全部预设在同一数据库事务内发布，成功提交后才处理旧目录 |
| 媒体元数据 | 正常 refs 更新整份序列化可能丢掉未知顶层字段 | 写入保留原 JSON 未识别键，当前已知字段为准，不复活显式清空的 refs；成功和失败路径均有合成未知字段回归 |

## 数据与验证证据

独立复核已通过：P0 封存 17,156 文件、12,726,739,405 字节，文件集合与哈希全部一致；当前 14,759 个保护文件只有上轮已授权的 Debug exe 差异，原 exe 仍封存且哈希正确。data 的 14,363 文件、5,147 目录集合没有增删改或审计中漂移。

5 份工作稿共 87 个完整版本、22 条版本事务、5 份正式独立素材、88 个模板和 117 个插件安装版本（含 14 个旧 gamer-ai 版本）元数据对照一致。全部 6 个 `.gamersample`（含额外历史副本）两侧 ZIP CRC 检查通过。来源与封存文件均只做字节读取，不解密凭据。

原库和封存库各自复制成新的临时工作副本后，`integrity_check` 均为 ok，9 张表的 schema 和逐行全列哈希完全一致：运行 149、运行事件 92,206、任务 3、调度记录 12，未完成运行 0。两侧 `user_version` 原本均为 7，与当前迁移注册表一致；AGENTS 的旧 v5 描述已纠正，本次没有迁移。当前 10 份实际 Core media 元数据没有未知顶层键；上述修复另有合成未知字段回归，未重写原文件。

证据：`data-preservation-20261016T161236Z.json`、`sqlite-copy-and-inventory-20261016T161710Z.json`（均在下述 audit 目录）。没有用 SQLite 打开原库或封存库。源码编辑与授权构建的 Debug exe 单独分类。

完成最终构建后再次核对，`after-build-data-verification.json` 确认 14,759 个保护文件中 14,758 个原字节一致；唯一变化是本轮授权生成的 Debug exe（359,608,849 字节，SHA-256 `38fdf83ca15c49e0dbdae5199fcb7a206557569ec9379c3f367785c32cfeab95`），旧 exe 仍通过封存原 hash 校验。data 的 14,363 文件和 5,147 目录仍无新增、缺失、变化或核验中漂移。

证据统一放在 `server/target/external-agent-mcp/audit-20261017/`，不覆盖上一轮报告或候选。

## 最终验证

| 检查 | 实际结果 |
| --- | --- |
| Rust 全量 | 1113 通过、0 失败、47 ignored，198.54 秒；日志 `rust-final.log`，从 server 目录执行 |
| 默认宿主构建 | `cargo build --locked --manifest-path server/Cargo.toml --bin gamer-server` 通过，112.59 秒；Debug exe 未执行 |
| 无 WASM 配置 | `cargo check --locked --manifest-path server/Cargo.toml --no-default-features --bin gamer-server` 通过，无警告 |
| 前端 | 壳 841、YAML 99、Video 19、Live 56、Notify 3、Publisher 3，共 1021 通过；Keymap 无 UI 测试文件，不计入通过数 |
| 静态资源最终补强 | 原始/编码重复斜线、Windows 尾点/空格/ADS/短名别名拒绝；对应 3 项 Web hook 回归重验通过，Rust fallback 回归包含在全量中 |
| 壳构建 | 使用真实 Vite 配置输出到隔离 target，164 模块通过；原 public/web-dist 未修改 |
| 插件工具与打包 | 最新 packer 对 7 官方插件实际 pack/verify、ZIP CRC 通过；39 工具及 39 Schema；沿用已构建验证且哈希未变的 YAML/Keymap Component，WIT 未变 |
| SDK / 发行 | 7 文件 working_tree SDK 双向 hash 对齐；packer 4、发行 Node 10、PowerShell 7 与 5.1 合成夹具均通过 |

关键回归覆盖：所有模板匹配分支与函数闭包、guest 无视程序直接请求受禁原生函数、设备准备前拒绝、OCR 实际 owner、资源锁等待期间撤销、单份素材快照导出、缺 Runner 的冻结 owner、跨授权空页游标、未知插件嵌套 manifest 原字节往返、坏/未知 Trace 索引保留图片、未确认覆盖零媒体写入、媒体恢复失败与第二个 preset 写入失败后的完整回滚。

历史市场测试只读取新建的隔离 fixture 副本（9 文件、源/副本/源后 hash 相同）；没有运行 fixture 获取脚本下载或写共享 public。47 项 ignored 不计通过；没有启动生产 RuntimeServices、调用真实模型或操作真实设备。

失败过程保留：首次编译有一处新增 Trace 测试 helper 作用域错误；修正后编译通过。首轮全量 1111 通过/2 失败/47 ignored：一项新 RunRecord fixture 的兼容展示字段需要精确架构白名单，另一项禁通知 fixture 自身缺必填 content、尚未到执行策略检查。只修测试配置和精确白名单，两项定向通过后再跑完整 1113/0/47。对应初始日志保留，没有把中间失败报告算作最终通过。

## 更新后的独立审查候选

候选目录：`server/target/external-agent-mcp/candidate-20261017-audit/`。

ZIP：`server/target/external-agent-mcp/candidate-20261017-audit.zip`，71,363,743 字节、46 文件，CRC 通过。

ZIP SHA-256：`f52e551878eb89b200af8d4364e87145d9f8e1a1bd3269664d0cf22c4d272c4b`。

宿主 SHA-256：`38fdf83ca15c49e0dbdae5199fcb7a206557569ec9379c3f367785c32cfeab95`。

报告：`audit-20261017/candidate-report.json`；逐文件清单在候选内 `candidate-manifest.json`。这是源码工作树的本地 Debug 审查候选，不含业务 data、配置、管理员凭据及 ADB/FFmpeg 等运行依赖，不是正式独立便携发行包，未启动或安装。上一轮候选与封存备份继续保留。

独立只读实物复核通过：目录 46 文件（45 项清单登记文件及清单本身）与 ZIP 的 46 个唯一条目逐项匹配，大小和解压流 SHA 全部一致。7 个插件归档与 registry 大小/哈希一致，实际包含 39 个工具和 39 份可解析 Schema；外层和内层均未发现 AI 路径、退休权限、旧 builtin、业务数据、配置或凭据。

正式发行仍需真实提交、重导已提交 SDK、发布真实新插件资产并更新固定发布锁；本轮未提交、推送、公开发布或部署。

## 验证边界

本地合成目标、HTTP、WASM、构建和数据一致性验证不能替代真实成熟 Agent 客户端、Android/浏览器游戏、跨网络投屏或长期运行验收。旧已安装版本和旧静态资源在原位置保留，本轮源码修复不代表现有安装已更新。
