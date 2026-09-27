# 输入输出边界

Core 负责目标连接、画面、操作和媒体机制，插件负责识别、映射、自动化、互动规则等业务。输入输出实现目前在 Core 内装配，不增加接入插件框架。

| 边界 | 当前入口 | 消费约束 |
| --- | --- | --- |
| 目标 | `targets::app_context/prepare/acquire/check_available` | 使用目标 ID 和独立的 Package 上下文；业务不直接查 Android snapshot 或要求 Android 包名 |
| 画面 | 后端 `FrameService`；前端 `useConsoleStage` | 尺寸从 Stage 获取；裁切用固定帧；取色、放大镜用 `captureDisplayFrame`。插件不判断 video/img 或读取原生尺寸 |
| 控制 | `InputService`、`TouchService` 与目标适配器 | 通用按键保留名称，协议转换在 Core；Android 数字键码、持续多点触控是明确的专用能力 |
| 媒体 | `RecordingService`、`media::output` | 截图能力不代表录制、编码流或音频能力；插件不直接订阅 scrcpy 帧 |

## 画面与控制

画面和控制是独立能力，绑定同一个明确目标。CDP 当前由一个 `CdpSession` 提供这两组能力；预览与自动化取帧保持分开。

Stage 的 `captureFrame()` 返回固定画面及其尺寸、generation；实时视频先同步复制到 Canvas，连续图片从最后已显示的帧复制，离线媒体按确定帧读取。`captureDisplayFrame()` 用于取色、放大镜，读取当前显示像素，不为放大镜暂停媒体。插件只处理返回的图像，不读取底层预览元素的尺寸或类型。

手动输入先通过启用的映射，再进入具体目标的控制出口；自动化调用统一输入能力。浏览器映射的 `DeviceHandle` 携带原始 `FrameStamp`，跨 WASM 工作线程后仍校验目标、会话、页面修订和运行占用。拒绝过期操作，不改投当前活动页。

`key_named` 表达通用按键；WIT `named-key` 将名称交给 Core。既有数字 `key` 明确为 Android 键码，不能解释成浏览器虚拟键码。未提供映射方案时直接透传输入。`hold` 属于持续触控能力，当前浏览器实现明确拒绝，不转换成单鼠标以伪装支持多指操作。

## 能力发现

`GET /api/devices`（包含浏览器）和浏览器列表返回 `capabilities`：`frame`、`keyboard`、`pointer`、`multitouch`、`android_app`、`recording`、`media_output`。

这些字段表示当前实现支持的功能，不代表连接已经就绪，也不授予插件权限。界面按功能禁用对应操作，服务端再次检查。浏览器当前支持截图和普通键鼠；录制、音视频输出、持续多点触控未实现。媒体导入和离线编辑照常使用。

未来增加窗口采集等来源时，在 Core 接线并提供已有能力即可。采集和控制来源不同的组合必须明确目标对应和坐标转换；不因来源名称相同就认为坐标一致。需要新能力时再扩展接口，不预建通用注册平台。

## 发布与约束

此次 UI 快照接口要求 `host_api.ui ^1.1`；命名按键 WIT 要求 `host_api.input ^1.2`。宿主与官方 keymap guest 需协同更新，不能只替换 UI。插件仍在独立仓提交，SDK 快照、哈希和主仓 gitlink 一起校验。

`web/src/io-boundary.test.js` 约束画面工具不依赖预览元素原生尺寸、直播目标业务不访问 Android 会话；既有 Rust 架构守卫约束 Core 与业务依赖方向。新增来源应复用相同契约测试，并验证实际安装的插件资产。
