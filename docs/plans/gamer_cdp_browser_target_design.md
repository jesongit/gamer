# CDP 浏览器目标轻量接入设计

日期：2026-09-26；2026-09-27 更新。状态：首版经隔离 worktree 实施与本机验收后已合入本地主仓；尚未重启现用服务，真实云游戏人工验收待完成。使用方法见 [浏览器目标](../guides/browser-targets.md)。

## 范围与已确定决策

Core 增加通用 CDP 浏览器目标：从页面取帧供识别，通过 CDP 向同一目标发送键鼠，在现有投屏区域显示与手动操作。首个验收对象为云·星穹铁道，网址由目标配置提供；不自建云游戏平台，不在 Core 写平台名称、页面选择器、登录、排队、进入游戏或断线页面处理。这些流程由用户操作或 Package 内 YAML 完成。

首版包括无窗口启动、多账号隔离、手动 YAML 与定时任务。使用已安装的 Chromium 系浏览器和专用资料目录；不默认引入 Node.js、Playwright、内置 Chromium、外部驱动服务或新的插件执行类型。先提供按需取帧、限频截图预览和控制闭环，不增加音频、CDP 录制或二次视频编码转播。

保留 Android 的连接、采集、控制、WebRTC、看门狗与低功耗行为，不为统一名称迁移全部目录或重命名 `device_id`。不同网站可配置不同 URL，通用 CDP 能力不等于所有网站的无窗口播放和特殊输入都已验证。

## 当前代码与复用边界

| 当前代码 | 处理方式 |
|---|---|
| `server/src/capabilities/frame.rs`、`input.rs` | 已分离画面与操作，沿用这两组契约；补元数据及输入语义，不再定义一套同义接口 |
| `server/src/capabilities/adapters/frame.rs`、`vision.rs` | 复用 `FrameStore`、`FrameHandle` 与视觉适配器；帧记录增加来源身份与坐标版本 |
| `server/src/capabilities/adapters/mod.rs`、`registry.rs` | 注册表仍每类一个服务；适配器按逻辑目标路由，Android 与 CDP 共用帧存储 |
| `server/src/capabilities/adapters/device.rs`、`input.rs`、`touch.rs` | 现有 Android 实现保留；新增 CDP 实现，避免业务调用方直接选 ADB/CDP |
| `plugins/gamer-yaml/host/runner_adapter.rs` | `prepare/acquire` 当前固定走 DeviceManager，必须改为目标会话准备及占用分派 |
| `plugins/gamer-yaml/host/yaml_extension.rs` | 复用函数派发与识别；调整 Android 专用按键解析、坐标来源校验和应用操作门禁 |
| `server/src/core/models.rs`、`api/runs.rs`、`capabilities/adapters/run.rs` | 当前强制 Android 包名；浏览器目标需允许缺省，Android 原校验保持 |
| `server/src/store.rs`、`timer_core.rs`、`api/tasks.rs` | 任务存储及还原同样强制 Android 包名，首版必须同步调整 |
| `web/src/components/console/ConsoleVideoStage.vue`、`useConsoleStage.js`、`web/src/views/Console.vue` | 当前实时尺寸、取帧和就绪依赖 video/WebRTC，控制依赖 DataChannel；增加图片画面及 CDP 控制分支 |

当前 `server/Cargo.toml` 已有 `reqwest`、`tokio-tungstenite`、`image`、`base64`、Tokio，可复用做 CDP 发现、WebSocket 通信及截图解码，不需要另配浏览器自动化运行时。

## 职责分离、目标统一

```text
YAML 原生函数 / 手动控制
       ├─ FrameService → 目标画面实现 → FrameStore → VisionService / matcher
       └─ InputService → 绑定及版本检查 → 目标控制实现

Android：现有 DeviceManager / scrcpy
浏览器：画面与控制共享 Arc<CdpSession>
```

新增 `targets.rs` 负责目标解析、准备和运行占用；现有 Frame/Input 适配器分别路由到同一个 `CdpSession`。帧接口不依赖控制接口，能力注册表仍允许缺少 input；未新增来源组合配置或插件框架。将来接入窗口采集时在帧适配器调整来源，并提供相同目标身份及坐标映射；该组合首版尚未实现。

`CdpSession` 是 Gamer 对页面的连接对象，保存浏览器实例身份、固定 `targetId`、附着信息、状态与坐标映射，不代表网站登录会话。允许底层 CDP 连接共享，不将“一目标一 WebSocket”写入契约；按绑定发送命令，不跟随用户当前激活的标签页。

取帧使用 `Page.captureScreenshot`，解码后进入共用帧存储；先以页面视口为画面范围，不依赖平台 DOM。操作使用 `Input.dispatchMouseEvent`、`Input.dispatchKeyEvent`、`Input.insertText`。视口尺寸、截图像素尺寸及输入坐标分别记录，显式处理缩放；不能假设截图像素天然等于 CDP 视口 CSS 坐标。

控制接口按实际需求补移动、按钮按下/松开、滚轮与逻辑按键。现有 Android 数字 keycode 保持原义，不能直接作为浏览器键码；通用键名由后端转换，Android 专属 HOME/BACK/应用启停等在不支持的目标上拒绝。鼠标锁定下的相对移动不视为普通移动已自动覆盖。新增 YAML 能力采用插件函数，不加 DSL 关键字。

## 目标身份、多账号与持久化

新增浏览器目标记录，最小字段为稳定 ID、显示名、URL、浏览器资料 ID、视口设置；浏览器可执行路径属于宿主配置。建议以独立 `browser_targets` 表存储，现有 Android `devices` 表不塞入 URL 或伪造 serial；两个集合的逻辑目标 ID 不得冲突，运行前经同一解析入口定位。

首版一个浏览器目标对应一个账号资料目录，例如 `<data_dir>/browser-profiles/<profile-id>/`，不允许两个独立目标共享同一资料 ID，避免用两个目标 ID 绕过同账号运行互斥。目录保存完整浏览器资料，不只是 Cookie；账号显示名不作为已登录身份的验证。目录不进 Package、不随配置包导出。停止连接保留资料，清理资料须是独立的显式操作。

按需启动和复用该账号的无窗口浏览器实例，登录状态由浏览器保存。复用既有实例时不得每次运行都导航回初始 URL；新启动才按配置打开页面。专用调试入口仅供宿主连接，通过现有认证 API/WS 对外提供 Gamer 功能；不向前端暴露原始 CDP 连接。退出时只管理 Gamer 自己启动的浏览器，不能结束用户日常浏览器。

保留 `AppContext.device_id` 和 `content_package` 的现有职责，`android_package` 调整为可缺省：浏览器目标无该字段；Android 运行仍按现有规则从设备配置取得并校验。不能填空字符串、网址或 Package ID 代替 Android 包名。任务中非空 `android_package` 列及序列化一起调整，通过既有编号迁移保留全部 Android 任务。本次使用 schema v6，v5→v6 迁移与 schema 契约同步；迁移测试仅操作临时数据库。

## 会话与坐标失效规则

区分两个版本：目标连接代次（关闭、重新绑定、连接恢复后变化）与页面/坐标代次（导航、视口或映射变化后变化）。帧及输入请求携带对应身份；入队和实际发送前都校验，过期响应丢弃，过期操作不自动重放。

- 同一标签页正常跳转：暂时禁止输入，废弃旧帧与排队操作，页面重新可取帧后可继续同一 YAML。Core 只判断技术连接与映射就绪，不判断已登录、排队结束或游戏就绪。
- 关闭、崩溃或重新绑定到另一个标签页：原运行失败；不偷偷选择同 URL、同标题或当前激活的另一个页面。下一次运行可重新准备目标。
- 新标签页或登录弹窗：允许在目标管理中明确选择；不得隐式改绑运行中的任务。切换 Gamer 舞台查看另一个账号仅改变观看来源。
- 识别结果必须保留帧的页面/坐标代次。当前 `tap` 会把匹配对象提取为 `center`，需在提取前校验，并使 `$result.center` 这类点对象保留来源标记；新截图不能让旧结果重新有效。单纯字面量坐标按当前有效坐标系解释；脚本主动拆成裸数值会失去来源，不承诺推断其历史。
- 每次 `__fn` 都会新建函数宿主，绑定与版本不能仅存在该临时对象里。目标绑定属于运行上下文，识别结果身份跨步骤保留；不采用“每次调用接受最新版本”来掩盖失效。
- 取消、控制通道断开或停止会话时，清理已按下的键和指针；连接已断无法释放时标记会话失效，不把释放操作发送到其他目标。

## 舞台、自动化与定时任务

Android 继续使用原有 video/WebRTC。浏览器目标在同一舞台增加 canvas/图片来源；尺寸、就绪、冻结取帧由来源提供，复用框选和叠加层，不要求存在 `videoWidth` 或已连接的 WebRTC。

有人观看才限频刷新预览，慢客户端只保留最新帧；预览压缩或降采样不影响自动化按需取帧。模板裁切冻结具体帧和身份，保存时不重新抓最新页面。CDP 手动控制走认证、有序的 WebSocket，校验目标与坐标代次；键盘 DOWN/UP 不降级为无状态 HTTP 点击。复用现有控制占用规则，运行中不接受会干扰任务的手动输入。浏览器来源下，录制/声音等尚未支持的操作明确不可用。

手动运行、插件 `run.submit` 和定时任务统一解析持久化目标 ID；复用 RunManager 的同目标互斥、取消、日志与运行历史。不同账号可独立运行，同一账号不能重复建浏览器实例或同时运行两个脚本。前端关闭不终止后台任务，切换观看目标不改变运行目标；有运行占用时不得因无人观看而关闭浏览器。

任务触发先准备浏览器和固定标签页，然后执行原 YAML；定时持久化不保存临时 CDP `targetId`。服务端重启后任务仍指向同一账号配置，新一次触发重新准备连接，不自动续跑中断脚本。平台登录、排队与断线页面的识别、等待预算和处理全部由 YAML 实现；Core 只对浏览器连接/操作设置超时。已有运行事件如果通过 DataChannel 展示，浏览器目标复用事件源转发到预览 WS，不另外生成一套事件语义。

## 最小改动位置

| 改动面 | 文件/目录与范围 |
|---|---|
| 新增 Core 模块（建议位置） | `server/src/browser/`：浏览器实例、CDP 传输、CdpSession、目标配置；`server/src/targets.rs`：轻量目标解析与能力绑定，不建立插件框架 |
| 能力接线 | `server/src/capabilities/{device,frame,input}.rs`、`adapters/`：路由、帧元数据、输入语义与失效检查；`server/src/main.rs`、`api/mod.rs` 组合根 |
| 上下文与任务存储 | `server/src/core/models.rs`、`store.rs`、`migrations.rs`、`timer_core.rs`、`api/{runs,tasks}.rs`、`capabilities/adapters/run.rs`；同步 `release/contracts/schema-policy.md` |
| 浏览器目标 API | 新增目标登记、选择标签页、连接状态、截图和控制 WS；复用现有认证，Android API 行为保持 |
| YAML 宿主接入 | `plugins/gamer-yaml/host/{runner_adapter,yaml_extension,native_funcs}.rs`：准备/占用、按键与点对象元数据；若函数参数调整，同步插件 UI 校验和 `docs/reference/YAML.md` |
| 前端 | `web/src/components/console/{ConsoleVideoStage.vue,useConsoleStage.js,useConsoleDeviceManager.js}`、`views/Console.vue`、`api.js`、`components/TaskBoard.vue`：浏览器目标选择、图片舞台、控制路由及任务目标；审查仅按 Android 应用筛选的功能入口 |
| 分仓与契约 | 插件 `host/` 修改仍在固定插件仓提交，再更新主仓 gitlink；涉及公开 Host API/WIT/UI SDK 时同步版本、固定快照与兼容检查，不默认改动全部 SDK |

## 验收条件

1. 云崩铁以 headless 打开；Gamer 显示页面，能手动输入和扫码登录；关闭 Gamer 网页后，已启动的自动化仍能取帧及操作。
2. 两个账号使用不同资料目录，重启后分别保留登录状态；切换投屏、同时触发不同账号任务不串号；同账号重复运行按现有互斥规则拒绝。
3. 按需截图 → 现有 matcher → YAML 点击闭环成立，冻结截图能制作模板；画面来源无控制能力时仍能识别，输入被服务端拒绝。
4. 同标签页正常导航后，YAML 可等待并重新找图；旧帧响应、旧匹配对象、旧 `center` 和排队输入都不能作用于新页面。尺寸/缩放变化、目标关闭、连接恢复同样验证。
5. 常用键鼠、拖动、文本及取消释放有效；控制 WS 断开不留下可继续误发的输入队列。游戏视角所需相对鼠标移动单独记录实测结果，不用普通点击验收代替。
6. 定时任务无需前端在线，触发时可启动对应浏览器；服务端重启后任务绑定不变；未登录/排队等由 YAML 处理，不在 Core 新增网站分支。
7. 在临时数据库上验证迁移、Android 任务往返及浏览器任务往返；目标绑定、过期丢弃、坐标映射用已安装浏览器与本机测试页验证。真实云崩铁仅作明确的人工验收，记录浏览器版本与限制。
8. Android 现有运行、投屏、互斥、取消及媒体只读门禁无回退；遵守本仓本机验收要求，首次 ADB 调用设置 `ADB_MDNS=0`。自动化验收仅使用 127.0.0.1 页面、临时数据库及资料目录，不启动主仓服务。

## 参考

- [CDP Page：截图及页面事件](https://chromedevtools.github.io/devtools-protocol/tot/Page/)
- [CDP Input：鼠标、键盘与文本](https://chromedevtools.github.io/devtools-protocol/tot/Input/)
- [CDP Target：目标附着与身份](https://chromedevtools.github.io/devtools-protocol/tot/Target/)
- [Chrome Headless](https://developer.chrome.com/docs/automation-and-testing/headless)
- [Chromium 用户资料目录](https://chromium.googlesource.com/chromium/src/+/HEAD/docs/user_data_dir.md)
- [Chrome 远程调试与专用资料目录](https://developer.chrome.com/blog/remote-debugging-port)

## 本次验证与保留范围

已验证已安装 Edge 的无窗口本地页面：截图 → FrameStore → NCC 匹配 → InputService 点击；键盘、文本、修饰键；账号资料隔离和 Cookie/localStorage 重启持久化；导航、目标重连及显式标签页改绑后拒绝旧操作。任务 API 与临时数据库覆盖浏览器上下文往返、Android 数据保留，沿用现有 scheduler 与 runner 准备链路。预览测试覆盖显示帧就绪后才开放输入、切换目标时丢弃迟到连接。

真实云·星穹铁道的登录、受保护画面、长时间视频播放和游戏内输入尚未验收；约 2fps 截图预览用于自动化观察，不承诺代替流畅串流。未实现相对鼠标锁定、音频、CDP 录制和非 CDP 画面来源配置。定时触发不依赖浏览器前端在线，但真实账号定时脚本尚待人工验收。
