# YAML 验收结果 · 2026-09-14

本轮修复脚本名称显示与保存、原生参数名只读呈现、函数目录加载、自动保存误诊、资源响应字段、列表摘要、版本冲突、简写参数绑定、对象引用调用和模板改名遗漏。

| 检查 | 结果 |
|---|---|
| 前端全量 Vitest | 87 个文件、858 项通过，包含参数按钮、模板页面加载、中文函数名与运行配置包透传回归 |
| Rust 服务端全量 | 653 项通过，6 项按原测试配置忽略 |
| 独立 yaml-interp | 13 项通过 |
| 真实 WASM | 全部 18 个内置函数、Package 函数、对象传参、嵌套引用、流程与事件均通过，包含在服务端测试中 |
| 真实 REST + 前端 API/编辑器 | 11 组通过，涵盖插件安装、全部夹具保存与重开、连续保存、中文函数目录发现与入口参数查询、改名、并发冲突、模板改名、非法输入、任务 CRUD |
| 浏览器 | 创建脚本、名称无扩展名、日志参数编辑、失焦自动保存、保存后重开；新建 browser_wait 函数、wait_find 参数名为文本、修改超时并自动保存、函数搜索与原文保存通过 |
| 前端生产构建 | 通过；func1 修复已构建至 server/web-dist，开发页面同步更新 |
| 当前 server/data/packages 全部自动化 YAML | 5 份中 1 份通过，4 份未通过，见下表 |

本次发现的不合格业务文件仅做只读检查，未改写：

| 文件（相对于 server/data/packages） | 原因 |
|---|---|
| `com.mihoyo.hkrpg/plugins/gamer-yaml/functions/工具.yaml` | 旧 functions/ 目录，缺 functions 包装 |
| `com.tencent.nrc/plugins/gamer-yaml/functions/verify_lib.yaml` | 旧 functions/ 目录，缺 functions 包装 |
| `com.tencent.nrc/plugins/gamer-yaml/automations/verify_a.yml` | 已删除的 version 字段 |
| `com.tencent.nrc/plugins/gamer-yaml/automations/加灵.yml` | 非 V1 顶层字段，缺 run |

通过的业务文件为 `com.mihoyo.hkrpg/plugins/gamer-yaml/automations/每日任务.yml`。逐条诊断、测试输出和 API 报告在 `server/target/yaml-acceptance/`；扫描脚本可对实际部署的其他 Package 根目录重复执行。

模板空列表补充验收：本地两个业务包仍有 54 + 3 张模板文件；修复模板面板仅挂载时加载、遗漏异步 Package 恢复的问题，并拒绝旧 Package 请求覆盖当前列表。两项新增测试先复现失败、修复后通过；隔离浏览器登录及整页刷新后，无需切换配置，打开模板面板均显示已有 `button.png`。

参数按钮补充验收（已按最新交互调整）：全部 18 个原生函数按参数声明展示按钮，默认值不再自动展开或写入新步骤；仅必填且无默认值的参数自动展开；可选参数默认收起、不高亮，点击后空白不落盘，填写后重开保留。覆盖 false/0/空串/显式 null、对象默认值独立复制、恢复默认、撤销与保存重开。浏览器新建 `wait_find` 步骤，填写模板、点击 timeout 并改成 5s，自动保存后重开保留 5s，自定义之外的默认参数仍收起；生产构建通过并更新 server/web-dist。

中文函数补充验收：`每日任务跳转` 已加入共享函数库夹具，前后端命名用例共用 `function-names.json`；验证中文定义、脚本调用、序列化重开、资源保存、目录与参数 schema 查询，并由真实 WASM 执行中文函数→嵌套调用→返回值传递。现有业务文件未改写。本轮日常服务更新操作被自动审批拒绝，仍需手动执行 `.\gamer.ps1 restart -BackendOnly -Build` 使后端校验生效。

func1 真机补充验收（19:14）：复用实际前端 `runYamlFunction` → 当前 8443 服务 → WASM → 设备截图与模板匹配，原函数仅执行 `wait_find(指南.png, timeout=30s)`，未修改用户函数或模板。修复前首次失败 4754ms、热运行同类失败 18ms；修复通用 API 丢弃 `content_package` 后运行成功，服务端耗时 484ms（run_id `12828dd1-caf5-4581-9cba-7a88a2c2f409`）。后端另修正入口 `#` 分隔推导、提前拒绝非法配置包 ID，增加首次组件编译耗时日志；这些后端补强尚需重新构建并重启，前端显式传参已在当前服务验证生效。

边界：11 组 API/浏览器验收使用隔离服务，ADB 扫描关闭；18 个原生函数的 WASM 测试使用可记录 capability。新增的 func1 验收使用实际设备和现有游戏画面，只截图找图，不注入触控。复测命令见 [README](README.md)，使用教程见 [YAML 教程](../../docs/guides/yaml-tutorial.md)。

教程补齐为 7 个注释案例，覆盖全部 18 个原生函数及每个参数、四种步骤、自定义函数、变量与运行参数，并列出全部 11 种参数类型。教程测试同时校验原案例与启用注释中可选参数的版本，按真实函数目录校验调用、模板和序列化往返；新增清单覆盖断言防止遗漏。本轮定向执行 yaml_acceptance.test.js，26 项测试全部通过（未重跑上表的全量验收）。
