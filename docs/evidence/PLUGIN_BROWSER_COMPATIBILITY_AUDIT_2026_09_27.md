# 官方插件浏览器目标兼容性审计（2026-09-27）

审计基线：主仓 `6ff4fe4`，插件固定提交 `f35d5c3`。本轮检查源码及本机已启用插件清单，未操作真实云账号、直播连接、推流、Android 设备或定时任务。此文是兼容性审计，不代表下列缺口已实现或完成真实平台验收。

## 结论

接入方式应由 Core 适配，同一种能力的插件消费方不应关心 ADB/CDP、video/img。当前实现已统一基础截图、识别、点击、拖动、命名按键和文本输入，但前端画面消费、按键映射、触控、录制/推流与部分目标上下文仍有 Android 假设。因此不能把“浏览器投屏和自动化基础链路通过”当成“所有插件能力均已兼容”。

本机实际启用版本：gamer-yaml 0.1.0、gamer-keymap 1.0.1、gamer-video 1.0.2、gamer-live 0.2.3、gamer-package-publisher 0.1.0，均为 running。running 仅表示生命周期正常，不证明当前目标支持插件使用的全部能力。插件源码/壳构建与已安装插件资产是不同部署对象。

## 按插件检查

| 插件 | 已接入或不受目标 I/O 影响的部分 | 查到的缺口 |
| --- | --- | --- |
| gamer-yaml | 后端 FrameService/VisionService/InputService 基础路径已有浏览器分派；浏览器定时执行已有统一目标准备；框选快照已修复并部署 | 取点、取色、Bridge 框选返回尺寸还有直接读取动态 DOM 的路径；网页换帧时可能临时无尺寸。Android 应用启停和 Android 数字键码不能等同于网页操作 |
| gamer-keymap | 方案编辑/保存属于配置数据；部分 tap/swipe 能力底层已支持浏览器 | Console 浏览器键鼠分支直接发送 CDP，绕过映射控制器；持续触控 TouchAdapter 只有 scrcpy 实现；raw_key 使用 Android 数字键码；叠加图仍只读 videoWidth/videoHeight |
| gamer-video | 媒体导入、已有视频播放、项目、离线取帧等不依赖当前 Android/CDP 目标 | 新建录制消费 scrcpy H.264 帧及 scrcpy 注入处的输入观察；浏览器目标会在 Android snapshot 检查处被判为不存在；不能据此认定离线视频功能不兼容 |
| gamer-live | 平台互动连接/规则配置本身不要求 Android；协议层与目标采集方式分离 | 互动队列 target/submit 直接读取 Android snapshot、强制 Android 包名，未复用 Core targets::app_context；音视频输出直接消费 scrcpy 会话和音视频流。前者是可复用现有目标接口修正的耦合，后者是尚未提供的源能力 |
| gamer-package-publisher | 操作配置包、GitHub CLI 和发布任务 | 本次检查未发现依赖实时画面或设备输入的路径；未执行外部发布来验证 |

## 具体证据

- `server/src/api/devices.rs::device_views` 已合并 browser targets，`api.listDevices()` 并非只列 Android。列表能选到浏览器，不代表后续业务实现接受浏览器。
- `server/src/capabilities/adapters/frame.rs`、`input.rs` 为浏览器实现截图、尺寸、tap/swipe、key_named、text；`input.rs::key` 和 `touch.rs::inject` 仍只走 Android session。`KeyCode` 注释称 backend-neutral，但数值执行语义实际为 Android keycode。
- `web/src/views/Console.vue::onKeyDown/onKeyUp/onMouseDown` 的浏览器分支提前返回，未走 keymap 输入管线。
- `plugins/gamer-keymap/host/mod.rs` guest Key 转换校验 Android keycode；`guest/src/lib.rs` raw_key 也按 Android 表映射；`ui/src/components/console/useConsoleKeymap.js::keymapOverlay` 只读 videoWidth/videoHeight。
- `plugins/gamer-yaml/ui/src/components/console/useConsoleTemplates.js::finishBridgeRegionSelect/samplePixelHex/finishCellPick` 仍直接读画面元素。前一轮修复了 openCrop 和主要框选几何，不能据此认定所有画面工具都已收口。
- `plugins/gamer-live/host/runtime.rs::target/submit` 构造 Android 必填的上下文；`host/queue.rs::Target.android_package` 为必填 String。Core `server/src/targets.rs::app_context` 已能为浏览器构造 android_package=None 的上下文，应该复用。
- `server/src/recording/mod.rs::RecordingService::start` 在设备 snapshot 校验后读取 session/frames_tx；`server/src/media/output.rs::start/pump` 同样直接使用 Android 连接及音视频分发。
- `web/src/workspace/PluginWorkspace.vue::pluginSupportsApp` 仅按 Android 应用声明过滤，不是能力发现；官方插件的通用应用声明不能用于推断多点触控、音频或编码流可用。

## 最小修复边界

1. 画面消费者统一使用已有 Stage 的尺寸、固定帧快照和坐标转换；把取点/取色/叠加框的剩余 DOM 依赖收回该边界。无需另建画面插件框架。
2. 所有运行入口复用 Core 的目标解析和 AppContext 构造，Android 包名只对 Android App 生命周期动作有意义；先修直播互动队列这一误耦合。
3. 明确普通键鼠、持续触控/多点触控、Android 键码的不同语义。映射执行通过统一能力层接入；无法提供的能力应明确拒绝，不能把多指操作静默当作单鼠标。
4. 将截图能力与编码音视频/录制能力分开报告。CDP JPEG 预览不自动提供录制所需的 H.264/音频/输入事件时间线；补齐前，界面应禁用相应操作并说明原因，后端也应给出能力不支持错误，而非“设备不存在”。
5. 兼容性验证覆盖实际安装模块：Android 视频、浏览器连续图片、离线视频三种画面来源；对截图/框选/取点/取色/映射/录制/互动队列分别验证。源码测试和插件 running 状态不能替代该矩阵。

本轮仅形成审计结果；上述剩余实现缺口尚未修改或构建部署。浏览器录制、带音频推流和多点触控也不应作为一次兼容性检查中的隐含新增功能。
