# ADB 统一与插件拆分工作报告

日期：2026-09-20。

后续目录扫描与缓存清理结果见 [项目目录清理报告](gamer-workspace-cleanup-report.md)。本文的当前验收日志及最新成功浏览器证据保留，较早的临时演练副本和构建缓存已清理。

**备份状态更新**：用户随后确认这些均为开发测试数据并明确要求删除，本文提到的完整转换前备份、补丁快照、旧迁移备份与失败转换临时目录已清理。下文备份路径仅记录当时执行过程，现已不能用于恢复；当前服务数据和配置保留。

## 完成范围

本次按确认方案删除 Docker 支持、统一 ADB 设备接入、拆分三个官方插件并转换本机存量数据。此前未提交的界面与品牌调整保留。未发布远程版本，未执行 Git 提交。

### 设备与部署

- 删除 Dockerfile、Compose、Docker 专属配置、镜像发布与升级验证脚本，以及服务端和前端的 Docker 模式分支。Windows launcher、直跑模式、通用 NAT/WebRTC 配置继续保留；历史 docker-data 未删除。
- 设备模型、API、界面不再区分真机、模拟器或容器。USB、模拟器和 mDNS 使用完整 ADB serial；显式 host:port 地址执行网络连接。扫描按完整地址去重，不再按型号、名称或子串猜测设备。
- 屏幕模式、虚拟分辨率、DPI、FPS 为独立参数。虚拟显示失败不会自动切换到物理屏，避免改变运行目标和脚本坐标系。是否支持虚拟显示仍取决于 Android 系统及应用。
- SQLite 升到 schema v4，移除 devices.kind；迁移测试覆盖设备标识、名称、地址、应用及显示参数保留。

### 插件结构与独立开发

官方身份统一为 gamer-yaml、gamer-keymap、gamer-video；manifest、runner ID、资源作用域、发行文件名与引用同步调整。插件显示名称仍使用中文功能名。

| 插件目录 | 内容 |
| --- | --- |
| plugins/gamer-yaml | manifest、host、guest、interpreter、ui、build.ps1、README |
| plugins/gamer-keymap | manifest、host、guest、ui、build.ps1、README |
| plugins/gamer-video | manifest、host、ui、build.ps1、README；builtin，无 WASM guest |

每个 UI 有独立依赖锁、构建和测试入口。单插件 build.ps1 只构建并替换自身产物，保留其他插件的 registry 条目。共享接口与构建工具位于 sdk/ui，主程序不再直接编译这些业务面板源码。SDK 示例也使用 gamer- 命名；底层通用 ID 校验没有强制拒绝所有其他合法第三方名称。

独立交付范围为：在现有宿主 SDK/ABI 内，UI 和 WASM 解释执行逻辑可单独构建、打包、安装和更新。浏览器验证已实际更新一个视频插件归档并刷新加载新代码，未重编译主程序。

**原生宿主适配器仍静态编译。** host/ 代码虽归插件目录所有，仍由服务端装配；修改原生函数注册表、资源内容钩子、宿主能力或 WIT 接口，需要同步构建服务端。这是本次独立交付的明确边界，不应描述成任意 Rust 插件后端均可热替换。新增可复用能力优先扩展既有 WASM/SDK 接口，避免建立第二套动态原生插件机制。

### UI 接入与权限

- 安装归档包含 ui/plugin.js 和样式，面板加载按插件所有者隔离，只有 Running 且获授权的扩展注册面板。
- 新增 ui.host 权限和 UI SDK 版本声明；安装确认明确提示这类 UI 与宿主页共享执行环境，可接触当前会话和工作区，仅应授予可信插件。
- 既有 iframe 的沙盒和 CSP 保留。宿主模块与沙盒 iframe 是不同能力，不能混称同等隔离。
- 单个插件加载失败展示错误占位，不阻塞整个工作台。官方 Console 接线所需的静态备用 UI 由独立构建生成，不代替插件 Running 状态控制面板出现。
- 插件 UI 在当前页面生命周期内固定。更新后应先保存编辑内容再刷新；停用或卸载会移除面板，但已加载的 JavaScript 需刷新才能完全卸载。

## 存量数据与当前服务

新增 tools/convert-plugin-ids.py：默认演练；实际应用要求 --apply --server-stopped。工具离线复制数据、备份 SQLite、检查冲突和路径后原子切换目录；只转换结构化插件身份和引用，不替换脚本文本、函数正文或任务业务 payload。官方已安装归档从经过 SHA-256 校验的新产物更新。

本机实际执行结果：转换 7 个目录、3 个已安装官方插件；原数据完整备份位于：

`D:\code\gamer\server\data.before-plugin-ids-20260920-102207-329917`

转换后验证：61 个配置资源文件逐字节相同；2 个 manifest 除插件 ID 外语义一致；原设备 1 条、日志 5 条保留；任务、预设、运行记录原本均为空；数据库 schema 为 4，没有旧 usb/空地址占位。工具测试另行验证了任务引用转换及业务 payload 不变。

服务端已重新启动，/health/ready 返回 HTTP 200，三个已安装插件均为 running 且 last_error 为空。旧浏览器会话可能因服务重启失效，刷新后重新登录即可。

一次早期演练因 SQLite 连接未显式关闭而触发 Windows 文件占用，已改用显式 closing 并通过后续转换和测试。失败演练目录仍保留：

`D:\code\gamer\server\data.plugin-convert-20260920-101619-855823`

自动审批拦截了该临时目录的清理，返回理由仅为 blocked by policy；没有绕过拦截重试。此目录与正式备份均已被 Git 忽略，不影响当前运行，也不会进入发行包。

## 验证结果

证据日志位于本机 server/target，不作为发行资源提交。

| 验证 | 结果与证据 |
| --- | --- |
| 服务端完整测试 | 665 通过、6 忽略、0 失败；plugin-tests-acceptance.log |
| Rust 静态检查 | clippy --all-targets -- -D warnings 通过；plugin-clippy-final.log |
| 无 WASM 构建 | cargo check --no-default-features 通过；plugin-no-wasm-check.log |
| 服务端 / launcher 构建检查 | 通过；plugin-server-build.log、launcher-no-docker-check.log |
| 前端完整回归 | Core 724、YAML 188、视频 7，共 919 通过；plugin-web-tests-acceptance.log。按键映射独立 UI 暂无独立测试文件，其集成与运行测试在 Core/Rust 套件中 |
| 前端生产构建 | 通过；plugin-ui-build-acceptance.log |
| 官方插件构建 | 三插件打包、归档元数据和哈希自检通过；build-new-plugins.log |
| 单插件独立构建 | 视频插件 wrapper 构建通过，其他两个 registry 条目未变；plugin-single-build-acceptance.log |
| 数据转换工具 | 4 项通过：演练不写原数据、数据保留、冲突拒绝、离线门禁 |
| 发布脚本 | 移除 Docker 后的离线发布工作流测试通过；no-docker-release-tests-final.log；本次修改的 PowerShell 启动与插件构建入口语法检查通过 |
| 浏览器集成 | 隔离数据实例、真实 Chrome 1920×1080：六个主页、五个插件面板、三个已安装 UI 模块均可加载，无捕获到的页面异常或 UI 资源失败；单插件更新后刷新加载新 UI 成功；plugin-ui-smoke-updated.log |

浏览器验证使用独立临时账号和数据，未读取实际管理员凭据，未连接实际设备。测试中创建的是设备配置，不能据此宣称真实投屏、ADB 控制或录制链路验收通过。

## 未实测项与建议

1. **真实设备矩阵**：分别验证 USB、模拟器 serial、网络 host:port、mDNS；每种覆盖连接/重连、应用启停、镜像与支持时的虚拟屏、输入和截图。建议先用当前常用设备完成日常链路，再覆盖其他设备，不恢复设备类别分支。
2. **虚拟屏兼容性**：按设备验证 1920×1080、DPI、方向变化和应用是否支持副屏；失败时显示明确错误，用户主动选择镜像，不自动改变坐标上下文。
3. **外部发布及跨平台**：此次没有发布 GitHub Release、生产升级/回滚，也没有在 Linux/macOS 做实机运行。正式发布前使用清洁环境构包并走一次安装/升级/恢复演练。
4. **宿主适配层更新**：需要改 host/ 或接口的插件版本，发布时同步声明最低宿主版本；纯 UI/WASM 更新维持单插件发行。将更多逻辑下沉 WASM 应以实际开发需求推进。
5. **UI 刷新契约**：本次采取保存后刷新，避免动态替换正在编辑的组件。未来如增加更新提示，继续保留未保存状态守卫；不强行无提示刷新。
6. **自动化覆盖**：按键映射目前无独立 UI 测试文件，后续修改复杂交互时补对应行为测试；服务端 6 项忽略测试未计入通过数。真实硬件和外部环境测试按上述矩阵单独验收。

README、设备接入说明、插件开发指南、各插件 README、架构计划与 AGENTS 已同步本次结构和边界。历史计划/证据中的旧部署背景不代表当前仍提供 Docker 支持。
