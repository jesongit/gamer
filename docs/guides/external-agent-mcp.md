# 外部 Agent 与统一 MCP

Gamer 提供画面、受控输入、资源、运行证据和插件工具。模型、对话、规划与修改策略由你选择的外部 Agent 承担。

## 连接

1. 在 Gamer「设置 → MCP 与外部控制」填写名称和有效期，点击“添加令牌”。有效期单位为小时，`0` 表示不过期。
2. 新令牌可使用创建时已经开放的 MCP 功能，不需要逐项勾选权限或对象。令牌的操作权限在创建时固定，对象范围包括现有及以后新增的目标、配置包、插件、执行器、运行、任务和素材。
3. 点击“查看”或“复制”使用令牌，离开页面后仍可再次查看。原文经现有凭据保护机制加密保存，认证仍使用哈希；列表接口不返回原文。旧版只有哈希的令牌继续有效，但无法还原原文，可以创建新令牌后删除旧令牌。令牌不进入配置包，也不继承旧 AI 插件令牌。
4. 默认只允许服务本机连接。需要其他电脑、宿主机或其他容器中的 Agent 接入时，开启“允许远程 MCP 连接”，保存后即时生效。也可在配置顶层设置 `mcp_allow_remote = true` 后启动；缺省为 `false`。
5. 在自己的 Agent 中添加 Streamable HTTP MCP 服务，请求头填写 `Authorization: Bearer <新令牌>`。本机地址例如 `http://127.0.0.1:8443/api/mcp`；远程开启后，从其他电脑可访问的 Gamer 网页打开设置，复制显示的连接地址，例如 `http://192.168.31.10:18443/api/mcp`。地址沿用网页的协议、主机和映射端口。具体配置文件格式由客户端决定。
6. 连接后先读取 `gamer.core.guide`、`gamer.core.capabilities` 和当前工具目录。插件启停或更新后重新读取目录。

远程开关只放开 MCP 的网络来源和对应的 Host，不改变令牌、对象范围、工具权限或输入屏障。关闭后立即拒绝新的远程调用，本机连接仍可使用；已受理的正式运行和持久任务继续按冻结授权执行。`local_only = true` 时仍只允许本机，需关闭该监听限制并重启才能远程连接。

端点继续校验 Host 和 Origin，浏览器 Origin 必须与 Host 同源；管理员 Cookie 和本机管理令牌不能代替 MCP Bearer。当前不支持含 `Forwarded` / `X-Forwarded-For` 的代理请求，不从转发头推导身份。Docker 使用已发布的 HTTP 端口，开启远程开关后允许实际容器桥接来源。跨公网请使用 HTTPS；不要将令牌写入源码。旧 `/api/extensions/gamer-ai/mcp` 地址不再使用。

协议基线为 `2025-11-25`。POST 使用 JSON-RPC 2.0，`Content-Type: application/json`、`Accept: application/json, text/event-stream`；初始化后携带协商的 `MCP-Protocol-Version`。支持 initialize、ping、tools/list、tools/call，通知返回 202，GET 返回 405。当前不提供 SSE、会话头或工具变更推送；目录分页游标与当前授权和目录绑定。

## 按用途授权

首版设置页不展示 scope、对象选择和控制会话参数。服务端仍检查显式操作权限与对象范围；新令牌从当前真实工具目录确定权限。安装新插件后，若新工具需要创建时尚不存在的权限，请重新创建令牌。

| 用途 | 主要 scope 与对象 |
| --- | --- |
| 观察画面 | `observe` + 明确目标；不会自动开始控制或扫描设备 |
| 读取配置 | `resource.read` + 配置包 + 插件；Core 资源工具只读 |
| 编写 YAML 工作稿 | `plugin.gamer-yaml.read`、`plugin.gamer-yaml.draft.write` + gamer-yaml + 配置包 |
| 读取独立素材 | `plugin.gamer-video.read`、`media.read` + gamer-video + 素材 ID；配置包权限不代替独立素材权限 |
| 私有版本试跑 | 工具声明的 trial scope、`run.submit` + 目标、配置包、插件、执行器；试跑受令牌撤销和到期取消 |
| 提交正式脚本 | `run.submit` + 目标、配置包、插件、执行器；读取另需 `run.read`，取消另需 `run.cancel` |
| 编辑定时任务 | `task.write` + 任务及目标等实际对象；新任务默认禁用，启用或立即运行另需 `run.submit` |
| 直接操作 | `control` + 目标和可选配置包；仍须管理端显式开始会话 |

插件可有更细的 scope，始终以 `tools/list` 中声明为准。令牌只持有权限，不使缺失、停用或声明不完整的插件自动可用。

YAML 私有试跑的执行器为 `gamer-yaml-version`，与正式执行器 `gamer-yaml` 分开。简化设置创建的令牌已覆盖执行器对象范围。

YAML 工作稿引用独立视频素材时，还需授权实际素材所属的 `gamer-video` 插件、`media.read` 及准确素材 ID；只允许 gamer-yaml 或配置包不能间接读取另一插件的素材。OCR 读取运行 Trace 也会核对该运行的实际插件归属。

## 编写与验证脚本

### 从录像名称制作素材

原始录像与已制作的素材包是两个集合。`sample_list` 只列素材包，不能用它判断录像是否存在。

1. 用 `gamer.core.recording_list` 传 `name`（例如“退出云游戏”），按当前视频显示名做不区分大小写的包含搜索；也可按 `device_id`、`state` 过滤。返回录制 `id`、各段 `media_id`、真实会话时间段与显示名称。同名结果都保留，结合设备和时间选择，不擅自取另一个账号的最新录像。
2. 用 `gamer.core.recording_get` 查看该录制；结束录制后用 `gamer.core.recording_events` 读取事件，仍在录制或收尾时会明确拒绝。`kind:"key"` 可定位 Alt+Q 等已记录快捷键，`start_us` / `end_us` 使用录制会话微秒，事件不是输入回放指令。
3. 用 `gamer.core.media_get` / `media_frames` 查看原媒体的时长、尺寸和真实展示帧 PTS；`media_frame` 按 `index` 或 `pts_us` 二选一返回真实 PNG。事件时间先减去对应段的 `start_us`，得到该段媒体 PTS，不按固定帧率换算。
4. 确认时间段后调用现有 `gamer.plugin.gamer-video.sample_create`：将录制 `id` 填入 `recording_id`，指定 `start_us` / `end_us`、不冲突的 `new_sample_id`、名称和 `operation_id`。随后用 `sample_read` / `sample_frames` / `sample_events` 检查保存后的独立素材，再读取原图和裁切模板。

导入的视频没有录制事件时，用 `gamer.core.media_list` 按名称查找并读取帧；不能凭空补造 Alt+Q 或其他原始操作证据。发现工具只要求已有 `media.read`，按实际媒体对象过滤；录制还需实际设备和全部分段媒体都在授权内。没有生成任何分段的失败录制需明确授权全部媒体，避免空分段绕过媒体范围。已有令牌包含该 scope 和全部对象时无需重建。

列表与事件默认每页 20、最多 100 条，用 `next_offset` 继续；总数仅统计有权读取且符合过滤条件的记录。事件流单次读取预算为 16 MiB / 100000 条，缺失、损坏或超出预算明确报错，不将截断结果称为完整素材。录像名称搜索不加载事件文件，读取历史不会触发录制恢复写入。媒体引用只返回有权读取的配置包/插件信息；不返回服务端路径或原始工具 stderr。

### 保存脚本工作稿

1. 用 YAML 插件的 `grammar` 和 `functions` 读取当前语法、原生函数 Schema 与当前配置包函数源码。不要从旧脚本猜语法版本。
2. 用 `draft_list` / `draft_get` 读取版本；新建用 `draft_create`，修改已有正式脚本先用 `draft_import`。
3. 读取必要的独立素材 manifest、事件和实际 PNG。素材用于判断和编写，不用于回放输入。按需通过 `draft_crop` 从原始像素裁切模板。
4. 用 `draft_save` 提交完整 YAML、版本并发条件和明确的 `operation_id`。省略素材关联字段沿用旧关联，显式空列表只清关联，不删除原素材。保存新增版本，不覆盖正式脚本。
5. 用校验和素材匹配工具检查语法、引用、ZIP/PNG、模板 NCC；它们不执行分支、等待、OCR 或真实点击，不证明游戏完成。
6. 用户授权后用私有试跑工具执行指定版本，保留返回的实际 `run_id`。按这个 ID 读取 events / trace / image，不能使用全局“最新运行”代替。
7. Agent 可以继续保存修改后的版本。用户在 Gamer 查看差异、测试并发布大版本；MCP 没有正式发布、回退或删除工具。

工具全名采用 `gamer.plugin.<插件ID>.<tool_id>`，例如 `gamer.plugin.gamer-yaml.draft_get`。参数名称、必填项和上限从当前 Schema 读取。

### 调度、原录像 OCR 与键盘映射

- 保存定时任务前，用 `gamer.core.schedule_preview` 传 `schedule:{provider_id,config}`，可选带时区偏移的 `after` 和 `count`（1..10）。它通过当前注册的 provider 校验并计算触发时间，不创建或启用任务。返回 UTC `next_times` 与服务器时区的 `next_times_local`；未知 provider 返回 `provider_available:false`，不能把“允许先保存未知 provider”误当作校验通过。没有未来触发时间时 `valid:true, exhausted:true`。
- `gamer.core.ocr_recognize` 可用 `source:{kind:"media_frame",media_id,index}` 识别原录像帧，无需先制作素材包。需要 `ocr.recognize`、`media.read` 与实际媒体授权；输入仍只接受宿主解析的证据 ID。返回帧的原始/工作尺寸和 PTS，工作图宽度最多 1920，文字坐标对应工作图。原媒体帧提取并不等于已经完成文字识别，OCR 引擎可用性另查 `ocr_status`。
- 已制作素材可用 `gamer.plugin.gamer-video.sample_list` 的 `name` 搜索；名称过滤与对象授权都在分页前完成。
- 从空白编写映射前，先读取 `gamer.plugin.gamer-keymap.grammar`，再用 `profiles_validate` / `profiles_write`。映射仍是独立的 `version:1` 语法，与无版本字段的自动化 YAML 不同；保存不会自动选用方案或发送输入。语法工具只需插件的 profiles.read scope，不要求任意配置包权限。

录像发现、调度预览和原录像 OCR 需要包含这些实现的新宿主；素材名称搜索与映射语法还需要对应新版插件声明和 Schema。只更新插件不能更新其原生 host 代码，源码版本不代表已公开发行。

## 直接控制与生命周期

首版简化设置页只管理令牌和显示连接地址，不提供直接控制的会话表单。既有 `/api/external-control` 管理接口保留；MCP 不能自行开始或恢复直接控制。已有控制会话可通过删除对应令牌结束。浏览器目标仍需先连接；MCP 不会自行打开页面。Android 和浏览器共用现有 Core 控制屏障及单目标运行槽。

Agent 先读取 `control_status`，再用带当前 session_id、generation 的 `capture` 获取可用于输入的 frame_id。输入携带这三个身份和独立 operation_id。每次动作后重新截图；旧连接 epoch、坐标 revision、generation 或已消费帧都会被拒绝。

暂停会等待动作排空并释放按住输入，完成后才允许人工接手；暂停仍保留原运行槽。恢复必须经管理端操作，递增 generation 并要求重新截图。TTL 到期也暂停，keepalive 不能复活过期会话或旧代次。令牌到期、删除或用户结束会话会完整释放，收尾失败时保持屏障。

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

官方插件接入包括 YAML 18、视频素材 8、键盘映射 5、直播 7 项工具。OCR 的 2 项工具已经属于 Core，历史 gamer-ocr 插件不再是安装前提。通知和配置发布不开放 MCP 发送、凭据或发布动作。键盘映射工具保存资源，不自动热切换；用户在 Gamer 选用。

第三方 WASM 使用每调用独立 guest/Store、上下文和句柄表，复用编译缓存。当前支持安全的读取和资源处理；没有专用受控租约适配的低层 input/touch/app/run imports 隐藏并拒绝。无需因此放宽原输入屏障。开发契约见 [插件 API](https://github.com/jesongit/gamer/blob/main/docs/reference/PLUGIN_API.md)。

## 本轮验证范围

外部 Agent 改造阶段的验收覆盖本地源码构建、临时数据与合成目标，不代表所有真实 Agent 客户端、Android/浏览器游戏或跨网络投屏已经验收。完整测试及备份证据见 [改造进度](https://github.com/jesongit/gamer/blob/main/docs/evidence/EXTERNAL_AGENT_MCP_REFACTOR_PROGRESS.md)。

历史阶段候选、SDK 来源和保留新增数据的恢复步骤见 [交接与恢复说明](https://github.com/jesongit/gamer/blob/main/docs/guides/external-agent-mcp-handoff.md)。历史 Runner 的所属插件已不可用且没有可信 owner 记录时，读取对应旧运行/任务需要显式“全部插件”范围；不会从 Runner 字符串猜身份。
