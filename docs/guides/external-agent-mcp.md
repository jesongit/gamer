# 外部 Agent 与统一 MCP

Gamer 提供画面、受控输入、资源、运行证据和插件工具。模型、对话、规划与修改策略由你选择的外部 Agent 承担。本页适用于 Gamer 0.4.0 及配套官方插件，请先升级宿主。

## 连接

1. 在 Gamer「设置 → MCP 与外部控制」创建令牌，填写名称和有效期。
2. 勾选需要的操作，以及明确的目标、配置包、插件、执行器范围。空范围不授予访问；“全部”包括以后新增的对象。运行和任务默认仅允许令牌自己创建的对象，访问已有对象需要填写 ID。
3. 保存仅显示一次的令牌。Gamer 只保存哈希；令牌不进入配置包，也不继承旧 AI 插件令牌。
4. 在自己的 Agent 中添加 Streamable HTTP MCP 服务，地址采用页面显示的本机端点，例如 `http://127.0.0.1:8443/api/mcp`；请求头填写 `Authorization: Bearer <新令牌>`。具体配置文件格式由客户端决定。
5. 连接后先读取 `gamer.core.guide`、`gamer.core.capabilities` 和当前工具目录。插件启停或更新后重新读取目录。

客户端必须运行在 Gamer 服务所在电脑。端点检查真实回环连接、Host、Origin 和转发头；远程网页登录、管理员 Cookie、开发代理和旧 `/api/extensions/gamer-ai/mcp` 地址都不能替代这条连接。不要将密钥发给其他人或写入源码。

协议基线为 `2025-11-25`。POST 使用 JSON-RPC 2.0，`Content-Type: application/json`、`Accept: application/json, text/event-stream`；初始化后携带协商的 `MCP-Protocol-Version`。支持 initialize、ping、tools/list、tools/call，通知返回 202，GET 返回 405。当前不提供 SSE、会话头或工具变更推送；目录分页游标与当前授权和目录绑定。

## 按用途授权

页面的可选 scope 来自当前真实工具目录，安装或停用插件后会变化。对象范围与操作 scope 同时满足才可调用。

| 用途 | 主要 scope 与对象 |
| --- | --- |
| 观察画面 | `observe` + 明确目标；不会自动开始控制或扫描设备 |
| 读取配置 | `resource.read` + 配置包 + 插件；Core 资源工具只读 |
| 编写 YAML 工作稿 | `plugin.gamer-yaml.read`、`plugin.gamer-yaml.draft.write` + gamer-yaml + 配置包 |
| 读取独立素材 | `plugin.gamer-video.read`、`media.read` + gamer-video + 素材 ID；配置包权限不代替独立素材权限 |
| 私有版本试跑 | 工具声明的 trial scope、`run.submit` + 目标、配置包、插件、执行器；试跑受令牌撤销和到期取消 |
| 提交正式脚本 | `run.submit` + 目标、配置包、插件、执行器；读取另需 `run.read`，取消另需 `run.cancel` |
| 编辑定时任务 | `task.write` + 任务及目标等实际对象；新任务默认禁用，启用或立即运行另需 `run.submit` |
| 直接操作 | `control` + 明确目标和可选配置包；用户还必须先在界面开始会话 |

插件可有更细的 scope，始终以 `tools/list` 中声明为准。令牌只持有权限，不使缺失、停用或声明不完整的插件自动可用。

YAML 私有试跑还需精确授权执行器 `gamer-yaml-version`，可在“其他执行器与既有对象范围”填写；它与正式执行器 `gamer-yaml` 分开，不一定出现在正式执行器下拉中。

YAML 工作稿引用独立视频素材时，还需授权实际素材所属的 `gamer-video` 插件、`media.read` 及准确素材 ID；只允许 gamer-yaml 或配置包不能间接读取另一插件的素材。OCR 读取运行 Trace 也会核对该运行的实际插件归属。

## 编写与验证脚本

1. 用 YAML 插件的 `grammar` 和 `functions` 读取当前语法、原生函数 Schema 与当前配置包函数源码。不要从旧脚本猜语法版本。
2. 用 `draft_list` / `draft_get` 读取版本；新建用 `draft_create`，修改已有正式脚本先用 `draft_import`。
3. 读取必要的独立素材 manifest、事件和实际 PNG。素材用于判断和编写，不用于回放输入。按需通过 `draft_crop` 从原始像素裁切模板。
4. 用 `draft_save` 提交完整 YAML、版本并发条件和明确的 `operation_id`。省略素材关联字段沿用旧关联，显式空列表只清关联，不删除原素材。保存新增版本，不覆盖正式脚本。
5. 用校验和素材匹配工具检查语法、引用、ZIP/PNG、模板 NCC；它们不执行分支、等待、OCR 或真实点击，不证明游戏完成。
6. 用户授权后用私有试跑工具执行指定版本，保留返回的实际 `run_id`。按这个 ID 读取 events / trace / image，不能使用全局“最新运行”代替。
7. Agent 可以继续保存修改后的版本。用户在 Gamer 查看差异、测试并发布大版本；MCP 没有正式发布、回退或删除工具。

工具全名采用 `gamer.plugin.<插件ID>.<tool_id>`，例如 `gamer.plugin.gamer-yaml.draft_get`。参数名称、必填项和上限从当前 Schema 读取。

## 直接控制与生命周期

在设置中选择令牌、目标、可选配置包及 TTL，点击开始外部控制。浏览器目标需要用户先连接；MCP 不会自行打开页面。Android 和浏览器共用现有 Core 控制屏障及单目标运行槽。

Agent 先读取 `control_status`，再用带当前 session_id、generation 的 `capture` 获取可用于输入的 frame_id。输入携带这三个身份和独立 operation_id。每次动作后重新截图；旧连接 epoch、坐标 revision、generation 或已消费帧都会被拒绝。

暂停会等待动作排空并释放按住输入，完成后才允许人工接手；暂停仍保留原运行槽。恢复只能由用户在界面操作，递增 generation 并要求重新截图。TTL 到期也暂停，keepalive 不能复活过期会话或旧代次。令牌到期、撤销或用户结束会话会完整释放，收尾失败时保持屏障。

| 业务 | HTTP 结束或令牌失效后的行为 |
| --- | --- |
| 直接控制 | HTTP 结束不续期；TTL 暂停，令牌失效完整结束 |
| 私有试跑、临时 OCR | 撤销或到期取消并等待清理；不会误取消其他调用者的工作 |
| 已受理的正式运行、持久任务 | 按受理时冻结的授权独立继续，可在管理界面查看/停止 |
| 已受理的本机直播输出 | 独立继续；外部停止必须匹配冻结 stream_id、目标和配置包，不能停止后来新建的输出 |

外部 YAML 执行会检查冻结程序及可达函数调用，不开放通知、切换直播目标、直播确认与推流等间接副作用。OCR 还要满足 OCR 插件和识别 scope。未实现受限执行策略的第三方 Runner 拒绝外部提交。

## 重试、图片与旧数据

有副作用的工具要求 operation_id。网络失败重试时保留同一个 ID 和完全相同的参数；JSON-RPC id 只关联响应。参数不同会冲突。宿主在执行前持久预留收据，进程重启后未完成的收据返回 unknown，不会盲目重做。此时先查询实际业务状态，再由人决定下一步。

截图和判断帧使用 MCP image content 返回 PNG 和尺寸/来源元数据。Trace 的 frame_id 是调用栈帧，image_id 才是图片身份。素材和旧候选导出支持有界分块；返回的 base64 归档片段属于下载数据，不应当作图片显示能力。

`run_history` 按实际对象授权过滤记录。继续查询时把返回的 `next_before` 传为 `before`，即使当前页 `runs` 为空也继续；`next_before:null` 才表示扫描完毕。这个游标不授予读取或取消对应运行的权限。

旧候选 JSON 与 `.snapshot` 原字节保留，可读取与分块导出；导入必须明确选择不冲突的新工作稿名称。界面“导出候选记录 JSON”仅导出可读记录，完整原文件用 MCP 分块导出。原工作稿、全部版本、发布事务、正式资源、素材、Trace、任务及不可变插件版本均不自动迁移或清理。

旧 gamer-ai 安装资料仍可列出，但宿主执行能力已移除。包含退休 `ai.connect` 的旧 YAML 等版本需更新后才能启用；新归档拒绝该权限。旧私密数据库、凭据文件和配置包数据保留在原位置，不会自动发送给外部 Agent。

## 插件如何接入

安装并启用带 `[[tools]]` 声明的新插件版本。宿主读取插件明确声明的 tool_id、action、Schema、scope、权限、effect 与上下文字段，再统一通过 `/api/mcp` 发现和调用。禁用、卸载、更新或缺失实现时不可继续调用旧工具。仅安装旧 UI 或声明权限不会自动生成工具。

官方接入包括 YAML 18、视频素材 8、键盘映射 4、OCR 2、直播 7 项工具。通知和配置发布不开放 MCP 发送、凭据或发布动作。键盘映射工具保存资源，不自动热切换；用户在 Gamer 选用。

第三方 WASM 使用每调用独立 guest/Store、上下文和句柄表，复用编译缓存。当前支持安全的读取和资源处理；没有专用受控租约适配的低层 input/touch/app/run imports 隐藏并拒绝。无需因此放宽原输入屏障。开发契约见 [插件 API](https://github.com/jesongit/gamer/blob/main/docs/reference/PLUGIN_API.md)。

## 本轮验证范围

外部 Agent 改造阶段的验收覆盖本地源码构建、临时数据与合成目标，不代表所有真实 Agent 客户端、Android/浏览器游戏或跨网络投屏已经验收。完整测试及备份证据见 [改造进度](https://github.com/jesongit/gamer/blob/main/docs/evidence/EXTERNAL_AGENT_MCP_REFACTOR_PROGRESS.md)。

历史阶段候选、SDK 来源和保留新增数据的恢复步骤见 [交接与恢复说明](https://github.com/jesongit/gamer/blob/main/docs/guides/external-agent-mcp-handoff.md)。历史 Runner 的所属插件已不可用且没有可信 owner 记录时，读取对应旧运行/任务需要显式“全部插件”范围；不会从 Runner 字符串猜身份。
