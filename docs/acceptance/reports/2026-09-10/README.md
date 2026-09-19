# Gamer 整体浏览器验收报告

验收日期：2026-09-10（Asia/Shanghai）。依据：[docs/acceptance/browser-acceptance.md:1](D:/code/gamer/docs/acceptance/browser-acceptance.md:1)。

**结论：不通过。发现 10 项问题，其中 4 项 P1、6 项 P2。** 视频离线制模闭环可用；脚本、函数和映射编辑的核心闭环存在阻断或内容风险。真实设备场景未执行，不能据此判断投屏、自动化执行或录制可靠性。本次仅出报告，未修改业务代码、现有测试或产品设计，也未运行单测/构建来代替浏览器验收。

## 1. 环境与边界

| 项目 | 实际环境 |
|---|---|
| 仓库 / 提交 | D:/code/gamer；602f00a6811e2fcf58e6760ce82af202624739e5 |
| 服务端 | 复用现有 debug 二进制；系统 API 确认版本 0.1.1、commit 与仓库一致；x86_64-pc-windows-gnu |
| 前端 | 当前工作区 Vue/Vite；Vite 5.0.0；既有 node_modules，未安装或升级依赖 |
| 隔离实例 | http://127.0.0.1:15173 → 后端 18443；独立配置和 SQLite/资源/媒体/插件目录 |
| 数据目录 | 验收期间使用 D:/code/gamer/.tmp-browser-acceptance-20260910/data |
| 浏览器 | Codex In-app Browser；默认截图 1280×720；另执行 768×900 viewport override 检查并复位 |
| 官方插件 | 通过页面安装 gamer.yaml 3.1.1、gamer.keymap 1.0.1、gamer.video 1.0.0；安装后均显示运行中 |
| 测试资源 | qa-invalid、qa-copy、qa-copy-check；合成 acceptance.mp4（640×360、3秒、10fps、30展示帧） |
| 测试设备 | 仅新建离线占位配置“QA 离线设备”，地址 127.0.0.1:1；无可确认的专用 Android 测试设备 |
| 设备隔离 | 测试配置的 adb_path 指向不存在路径，阻止启动扫描或后台保活接触用户设备；ffmpeg 与 scrcpy 使用现有工具 |
| 原实例 | 原 8443/5173（PID 10888/240）保持运行；未对其登录、数据、插件或任务执行修改 |

启动先按示例配置完成隔离。最初批量后台启动被自动审批拒绝（只返回 blocked by policy），未产生目录或进程；改用可跟踪的前台工具会话启动成功。一次最小测试配置因缺少 decode_frames 被拒绝，随后改为完整 config.example.toml 派生配置；这两项属于环境准备记录，不计产品缺陷。

先阅读 README 与验收清单，再从页面执行用户任务；发现问题后才针对相关实现、控制台与 API 定位。API 仅用于核验及准备导入归档，未用来绕过失败 UI 并把任务算作通过。浏览器的导出下载事件未能被工具捕获，后续导入测试使用同一测试包由 API 导出的归档，故不宣称完成“浏览器下载文件再导入”的完整往返。

环境证据：[system-info.json](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/system-info.json)、[extensions.json](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/extensions.json)、[devices.json](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/devices.json)。

## 2. 已执行的用户任务

“通过”仅指表内明确描述的子闭环；模块中仍可能存在其他失败项。

| 用户任务 | 结果 | 实际执行与边界 |
|---|---|---|
| 错误密码后正常登录 | 通过 | 错误提示“账号或密码错误”；提交时禁用登录按钮；改正后进入控制台；新页面沿用会话 |
| 新建配置、查看详情 | 通过 | 创建 qa-invalid，名称、版本、通用目标可查看；大写输入 QA-invalid 自动归一为小写，不记为校验缺陷 |
| 复制配置及携带资源 | 部分通过 | 文件与项目引用进入副本；复制表单填写的名称/版本被忽略，见 F08 |
| 编辑配置元数据 | 通过 | qa-copy 名称改为“QA 副本”、作者 Browser Acceptance，保存后详情和后续新页保持 |
| 配置导出 | 部分验证 | 打开引用清单、勾选包含合成素材、提交后弹窗关闭；工具未捕获下载事件，文件落地未验证 |
| 重名配置覆盖导入 | 通过 | 页面上传测试归档，显示现有包摘要和覆盖警告；确认后显示“配置已覆盖导入” |
| 插件安装与取消 | 通过 | 自动化安装先取消，数量仍为0；再确认安装三个官方插件，均运行中，业务导航出现 |
| 插件停用、恢复、卸载 | 阻塞 | 点击视频插件停用出现确认提示；浏览器控制工具随后无法取得该同步确认框；未确认服务端停用成功，不算通过 |
| 带参数的可视化脚本 | 失败 | 创建 message 默认值参数与 return 引用；另建 find 步骤，选择真实模板并设置 threshold=0.9；后续保存均 version_required，见 F02 |
| 新建函数 | 失败 | qa_function + return true；保存被脚本顶层校验拒绝，见 F03 |
| 编辑恢复与错误校验 | 部分通过 | 空参数名/空默认值有诊断；跨功能页仍保留脚本草稿；取消编辑出现放弃确认，但最终确认操作受工具限制 |
| 创建、保存并重开映射 | 失败 | 可录入 KeyQ、设置坐标、切换原文；保存成功但直接“编辑”显示0绑定；磁盘仍有绑定，见 F04；设备应用效果未验 |
| 导入视频并制作项目 | 通过 | 导入合成 MP4，创建 qa-video，定位展示帧5（500000µs），保存“验收帧标记”；离开项目再回来仍有标记 |
| 视频定帧制模与离线匹配 | 通过 | 在帧5拖拽137×93区域，生成 qa-template#627_606_841_864.png；同帧匹配置信度1.000，中心(470,265) |
| 删除已被项目引用的素材 | 通过 | 二次确认后被 media_referenced 阻止，提示先移除项目引用，素材仍在 |
| 模板重新发现与跨功能使用 | 失败后可恢复 | 新页面模板为空；切换默认配置再切回后出现，选择器可选 qa-template.png；见 F06 |
| 定时任务表单 | 失败/阻塞 | 空名称、缺设备有明确提示；能读脚本参数默认值；配置离线设备且缺 Android 应用时保存仅 HTTP 422；改合法 cron 仍失败，见 F07 |
| 日志 | 部分验证 | 打开空日志页、手动刷新，仍0条；未产生真实运行日志，筛选有数据的结果未验 |
| 系统设置 | 部分通过 | 展示当前版本、依赖与直跑限制；策略改 off，保存、刷新、新页均保持；就绪文案冲突见 F09 |
| 配置市场 | 部分验证 | 进入并刷新；远端暂无配置，本地三个包可见；远端配置安装无样本可验 |
| 分栏与窄窗口 | 失败 | 分隔条键盘调整有效；默认窄侧栏及768宽检查出现配置栏/编辑内容裁切，见 F10 |

视频闭环证据：

![视频第5帧制作模板并离线匹配命中](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/09-template-offline-match.png)
![已引用素材删除被阻止](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/10-media-reference-protection.png)

## 3. 按影响排序的问题

严重度口径：P1 = 核心流程阻断或有内容覆盖风险；P2 = 可恢复的功能错误或明显影响操作的体验问题。排序不代表已对所有设备/浏览器完成验证。

### F01 · P1 · Console：已有设备时初始化抛出未定义函数异常

- **类型：功能错误。**
- **前置条件：** 数据库中至少有一个设备；本次仅为离线占位设备。
- **最小复现：** 新建设备后，在新页面打开控制台；检查浏览器控制台。
- **预期：** 完成初始化，注册键盘/失焦/可见性监听，并继续恢复运行状态。
- **实际：** 页面部分渲染，但控制台报 `ReferenceError: loadForm is not defined`，伴随 mounted hook 未处理异常；新页面3实际捕获。
- **影响：** 初始化后半段被异常中断。代码显示恢复运行态与输入监听位于异常之后；这些机制被跳过是代码层结论，实际设备断线/输入后果未执行验证。
- **定位：** [web/src/views/Console.vue:1131](D:/code/gamer/web/src/views/Console.vue:1131) 调用未定义 loadForm；之后是事件注册与 restoreRunState。旁边空设备分支的 mode 引用也可疑，但本次不将其另算已复现问题。
- **证据：** [console-tab3.json](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/console-tab3.json)。截图只显示“页面仍部分可用”，异常本身以控制台记录为准。

![异常初始化后的页面仍能部分渲染；控制台异常见JSON](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/12-system-status.png)

### F02 · P1 · 自动化：首次自动保存后继续保存失败，最终步骤未落盘

- **类型：功能错误。**
- **前置条件：** gamer.yaml 已运行；新建脚本，存在编辑失焦触发自动保存。
- **最小复现：** 新建 qa-flow.yaml → 加 message 参数和默认值 → 切换函数页再返回 → 加 return 引用 message → 点保存。另一复现：新建 qa-native.yaml → 添加 find → 选 qa-template.png、threshold=0.9 → 保存。
- **预期：** 全部步骤保存，重新打开保持参数和资源引用。
- **实际：** 提示 `version_required: 更新资源必须提供 expected_version，或显式 force:true`；反复保存无恢复入口。qa-flow.yaml 已存在，但仅有参数及 `run: []`，未包含后添返回步骤。
- **影响：** 用户无法完成编辑闭环；早期自动保存的部分内容与当前画布不一致。
- **定位：** [web/src/composables/useScriptEditorShell.js:203](D:/code/gamer/web/src/composables/useScriptEditorShell.js:203) 依赖保存响应 rep.id 更新 resourceId；[web/src/components/console/current-api-adapters.js:16](D:/code/gamer/web/src/components/console/current-api-adapters.js:16) 无 id 仍走创建。实际资源响应只有 package/path/version、没有 id；[server/src/api/packages.rs:653](D:/code/gamer/server/src/api/packages.rs:653) 直接返回通用资源条目。因而后续 PUT 仍作为创建发送且不带 expected_version，命中已有文件门禁。加载路径同样读取 s.id（shell:132）。
- **证据：** [script-resource.json](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/script-resource.json)、[script-on-disk.txt](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/script-on-disk.txt)。

![选取真实模板后保存仍被version_required拒绝](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/14-script-save-repro.png)

### F03 · P1 · 函数：新建函数库保存路径缺少 .yaml

- **类型：功能错误。**
- **前置条件：** 当前包没有默认函数库。
- **最小复现：** 函数 → 新建函数 → 名称 qa_function → 返回值 true → 保存。
- **预期：** 保存到 automations/_function.yaml，可重新打开/调用。
- **实际：** 提示“未知顶层字段 functions——V1 只支持 name/params/vars/run”；未创建函数库。
- **影响：** 函数面板无法从空包建立函数库。
- **定位：** [web/src/composables/useScriptEditorShell.js:172](D:/code/gamer/web/src/composables/useScriptEditorShell.js:172) 去掉 .yaml，保存分支213原样发送 name；[web/src/components/console/current-api-adapters.js:25](D:/code/gamer/web/src/components/console/current-api-adapters.js:25) 和 [web/src/api.js:561](D:/code/gamer/web/src/api.js:561) 不补扩展名。服务端 [server/src/extensions/gamer_yaml/resources.rs:42](D:/code/gamer/server/src/extensions/gamer_yaml/resources.rs:42) 仅将 _function*.yaml 认作函数库。调用链与页面诊断一致。

![函数保存被错误地按脚本顶层字段校验](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/06-function-save.png)

### F04 · P1 · 映射：直接编辑已保存方案时加载为空绑定

- **类型：功能错误。**
- **前置条件：** 已保存包含 KeyQ/hold 的映射；尚未先选中方案加载详情。
- **最小复现：** 新增 qa-keymap → 添加绑定 → 录入按键Q → 保存 → 直接点击列表“编辑”。第二次在原文模式确认含 KeyQ 后保存再编辑，结果相同。
- **预期：** 原绑定、动作及坐标原样显示，列表计数为1。
- **实际：** 保存成功后计数0；重新编辑为“绑定列表（0）”。API与磁盘实际仍有一个绑定、valid=true。
- **影响：** 用户误判保存丢失；在空白草稿上继续保存可能覆盖原绑定。本次未执行空草稿覆盖，因此“数据已被删”不是结论。
- **定位：** [web/src/api.js:433](D:/code/gamer/web/src/api.js:433) 列表只保留文件元数据；[web/src/components/console/KeymapPanel.vue:334](D:/code/gamer/web/src/components/console/KeymapPanel.vue:334) startEdit 从列表项立刻建立草稿，缺模型时回退空模型；[web/src/components/console/useConsoleKeymap.js:324](D:/code/gamer/web/src/components/console/useConsoleKeymap.js:324) 仅接 onSelect，未接 onEdit 拉详情。
- **证据：** [keymap-resource.json](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/keymap-resource.json)、[keymap-on-disk.txt](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/keymap-on-disk.txt)。实际设备应用未验证。

![保存前原文确有KeyQ绑定](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/07-keymap-before-save.png)
![保存后直接编辑显示零绑定](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/08-keymap-after-save.png)

### F05 · P2 · 自动化：同页首次安装插件后原生函数目录不刷新

- **类型：功能错误。**
- **前置条件：** 从零插件实例打开 Console，随后通过市场安装 gamer.yaml。
- **最小复现：** 安装成功 → 插件/自动化 → 新建脚本 → 添加步骤 → 搜索 tap。
- **预期：** 安装完成即可选择插件提供的 tap/find/sleep 等函数。
- **实际：** 只有流程与配置包函数，没有插件函数。新开页面后出现18个原生函数，问题有页面重开恢复方式。
- **影响：** 新用户安装后无法直接完成基本自动化，且空列表未提示需要重新加载。
- **定位：** [web/src/components/console/useConsoleScriptRunner.js:166](D:/code/gamer/web/src/components/console/useConsoleScriptRunner.js:166) 请求前即把 nativeFunctionsLoaded 设为 true，失败后仍保持完成状态；未随插件安装/runner注册重取。
- **证据：** [native-functions.json](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/native-functions.json) 为安装后的服务端目录核验，不替代页面操作。

![安装后同页搜索tap无原生函数](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/03-native-functions-missing.png)

### F06 · P2 · 模板：新页面预选已有配置时资源列表为空

- **类型：功能错误。**
- **前置条件：** qa-copy 中已经存在视频制作的模板，当前配置在新页面恢复为 qa-copy。
- **最小复现：** 新开 Console → 模板面板；或新建 find 步骤打开模板选择器。然后切换 default 再切回 qa-copy。
- **预期：** 首次进入便列出当前包模板，能在编辑器选择。
- **实际：** 初始显示“暂无模板”/“无匹配模板”；切换配置后同一文件出现，随后选择器可选 qa-template.png。
- **影响：** 已有资源被误报为空，阻断视频资源向自动化的自然衔接。
- **定位：** [web/src/components/console/useConsoleTemplates.js:493](D:/code/gamer/web/src/components/console/useConsoleTemplates.js:493) 初始化时只触发一次读取且吞掉异常；[web/src/views/Console.vue:1123](D:/code/gamer/web/src/views/Console.vue:1123) 稍后异步 loadPackages；包切换刷新位于 Console:478。**疑似原因：** 初始化包上下文与资源读取时序没有联动重试；未抓取首次请求时序，不能把该推断写成已验证根因。F01 的初始化异常也需一并排查。

![新页当前包有模板但列表为空](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/15-template-empty-existing.png)
![切换配置后原模板恢复](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/16-template-restored.png)

### F07 · P2 · 任务：缺少 Android 应用目标只提示 HTTP 422

- **类型：体验问题（错误定位与恢复提示缺失）。**
- **前置条件：** 有设备配置但尚未配置 Android 应用；本次为离线测试设备。
- **最小复现：** 新建任务 → 选择该设备、现有脚本 → 使用“每天8:00”生成合法 cron → 保存。
- **预期：** 明确提示设备未选择应用，并指引到工具条应用入口；保留当前任务输入。
- **实际：** 输入保留，但仅显示“保存失败：HTTP 422”。先输入非法cron再修正也仍只有422，无法据UI判断失败字段。
- **影响：** 用户不知道下一步如何恢复，容易误以为cron或脚本参数错误。拒绝缺目标任务本身合理，不要求后端放行。
- **定位：** [web/src/components/TaskBoard.vue:449](D:/code/gamer/web/src/components/TaskBoard.vue:449) 无应用时发送空字符串；saveTask未检查该项。[web/src/api.js:51](D:/code/gamer/web/src/api.js:51) 非JSON错误体被丢弃。诊断用API复现原UI body返回 `app.android_package: android_package must not be empty`，见 [task-error.txt](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/task-error.txt)；该调用仍失败，未创建任务。

![合法cron任务只收到HTTP422提示](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/11-task-save.png)

### F08 · P2 · 配置包：复制表单的名称和版本被静默忽略

- **类型：功能错误。**
- **前置条件：** 已有 qa-copy（名称QA副本、版本1.0.0）。
- **最小复现：** 复制 → ID qa-copy-check → 名称“复制时指定的名称” → 版本2.0.0 → 保存 → 详情。
- **预期：** 新包采用表单填写的名称及版本。
- **实际：** 新ID生效，但名称仍“QA副本”、版本仍1.0.0。早先默认带“（副本）”的名称同样未生效。
- **影响：** 用户需复制后再编辑一次，且可能不发现版本未变。
- **定位：** [web/src/composables/usePackageContext.js:358](D:/code/gamer/web/src/composables/usePackageContext.js:358) 复制分支只传源ID和新ID；同表单收集的name/version/targets未提交。[web/src/api.js:370](D:/code/gamer/web/src/api.js:370) 请求体仅new_id。

![复制前明确填写新名称与2.0.0](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/19-copy-metadata-before.png)
![复制后仍保留旧名称与1.0.0](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/20-copy-metadata-after.png)

### F09 · P2 · 系统状态：顶部与设置页就绪信息冲突

- **类型：功能错误。**
- **前置条件：** 测试服务已经启动并可处理API。
- **最小复现：** 登录 → 设置 → 刷新系统信息 → 比较顶部与设置页。
- **预期：** 两处使用一致的服务启动状态；单项ADB缺失应单独表达。
- **实际：** 顶部持续“服务未就绪”，设置显示“就绪”；API为 startup.stage=ready，没有readiness字段。
- **影响：** 用户无法信任全局状态指示。
- **定位：** [web/src/layouts/MainLayout.vue:56](D:/code/gamer/web/src/layouts/MainLayout.vue:56) 读取不存在的 readiness.status；真实响应见 [system-info.json](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/system-info.json)。ADB为隔离故意缺失，不是本条缺陷成因。

![顶部未就绪与设置页就绪同时出现](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/12-system-status.png)

### F10 · P2 · 布局：默认侧栏和窄窗口下当前配置难辨、操作被裁切

- **类型：体验问题。**
- **前置条件：** 默认1280×720页面、初始右栏340px；长配置名；另作窄窗口检查。
- **最小复现：** 登录初始页查看配置栏 → 创建长名称配置 → 打开带参数的find步骤 → 缩窄窗口或收窄右栏。
- **预期：** 当前配置至少可辨认；关键操作可发现，内容溢出有明确可用的滚动或折行。
- **实际：** 初始配置选择框几乎只剩箭头，末尾操作被裁；步骤参数区出现横向滚动和局部截断，多个参数输入难同时查看。拖宽分栏可缓解部分问题。
- **影响：** 当前数据上下文难确认，增加误编辑配置与寻找入口成本。设备工具条本身支持横向滚动，因此不把所有屏外按钮一概判为功能不可用。
- **定位：** [web/src/workspace/PackageContextBar.vue:164](D:/code/gamer/web/src/workspace/PackageContextBar.vue:164) 单行flex、按钮flex:none、选择器min-width:0；[web/src/views/Console.vue:1255](D:/code/gamer/web/src/views/Console.vue:1255) 默认340px侧栏且overflow:hidden。步骤内部更细的宽度成因未完整定位。
- **截图说明：** 窄窗口图为浏览器工具原始捕获，未裁图或拼接；不据此推断移动端触控可用性。

![默认布局中配置下拉收缩且右侧操作裁切](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/02-initial-layout.png)
![窄窗口下编辑内容的实际截图](D:/code/gamer/docs/acceptance/reports/2026-09-10/evidence/17-narrow-layout.png)

## 4. 未完成与未检查的范围

- 无专用测试Android设备：未验证真实连接/WebRTC画面、触控键盘输入、应用启动停止/APK安装、真实任务执行与取消、定时触发、运行冲突、跨页viewer接管、断线恢复、空闲低功耗。
- 无真实录制事件：未验证开始/停止/分段录制、浏览器断开后继续录制、事件转YAML草稿保存与打开编辑器。草稿页因无录制会话无法载入事件，不能记为通过。
- 插件：三个官方插件的安装与进入功能区已验；停用到确认阶段被工具阻塞，启用恢复/更新/卸载/删除数据及依赖循环、版本不兼容、权限变化未完成。第三方declarative/iframe插件没有测试包，未验。
- 任务：创建提交失败，故CRUD成功闭环、启停调度、预设应用/发布、实际运行记录未验；错误cron校验被缺Android目标的422先拦截，没有验证到后端cron诊断。
- 资源：未完成脚本/函数成功保存后重开、并发版本冲突/覆盖恢复、模板重命名引用改写、ZIP模板批量上传、无引用素材删除、包删除及跨包媒体碰撞导入。
- 视频：未测VFR/B帧/旋转黑边校准、长视频、大文件/磁盘压力、缺失素材重挂、批量或并发项目编辑。
- 导出下载事件工具超时；浏览器实际下载路径与文件字节未验证。成功API导出只作为导入夹具准备，不计UI下载通过。
- 配置市场远端清单为空，未执行远端配置安装。日志无真实运行数据，未验有数据的筛选及清空。
- 仅一个浏览器内核与有限窗口尺寸；没有跨Chrome/Edge/手机适配、全键盘可访问性、长时间稳定性、性能或安全专项结论。
- 同步JavaScript确认框两次使工具无法继续操作原页（放弃编辑、停用插件）；getJsDialog返回undefined且后续控制超时。市场安装的异步确认成功操作。该差异记录为工具覆盖限制，不直接报产品缺陷；新建页面可继续其他检查。

## 5. 交付与收尾

报告及原始证据位于本目录；所有截图来自实际页面，未伪造、未编辑。控制台异常、资源GET、磁盘内容与页面现象分别注明，诊断用API结果不冒充浏览器网络捕获。

测试数据仅用于本次验收；收尾状态记录在本目录 cleanup.txt。未自动开始修复，未提交Git，未修改用户既有 docs/acceptance/browser-acceptance.md、server/data 或现有测试。
