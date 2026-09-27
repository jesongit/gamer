# 云星穹铁道手动输入延迟修复

## 复现

使用实际 5173 前端、8443 后端及 `https://sr.mihoyo.com/cloud/`，为测试新建独立空账号资料。仅打开登录框，在手机号输入框填入无效测试串 `123`；不发送验证码、不提交登录、不进入游戏。用户原目标只读取画面/布局，没有代为输入。

- CDP 直接读取确认键鼠事件能进入跨域登录 iframe，DOM 中能出现数字；早先截图取到的是输入仍在排队时的旧画面，不能据此断言 iframe 不支持输入。
- 1920×1080 云页面的 PNG base64 约 2.4 MB，单次 `Page.captureScreenshot` 实测约 515～548 ms。500 ms 预览定时器几乎一直就绪，默认随机 `tokio::select!` 在积压的 down/up 之间反复选择截图，造成数秒延迟；鼠标移动也进入同一队列。
- 原主服务一次输入超过 8 秒才开始生效。独立 release 环境修复前连续 `123` 为 4608 ms；输入优先后同环境为 1354 ms。更新主服务后，5173 全链路为 976 ms，DOM 值及截图均显示 `123`。这些是本机单次测量，不是性能保证。

## 改动与验证

- `api/browser.rs` 接收循环优先清理已排队输入，再处理事件/截图；保留 FrameStamp 校验及单目标绑定，不把旧操作重新解释为新页面操作。
- 被拒绝的输入标记 `source: input`；前端显示错误 toast，同一连接相同错误不连续刷屏，避免下一帧清空状态时用户误以为按钮没有反应。
- 修复 Console 恢复已有设备时漏接 `loadForm`，此前会抛异常并中断 mounted 后续的自动连接、运行状态恢复和失焦清理注册；新增带已有目标的挂载回归。
- 默认特性 release 和 dev 构建均通过；前端生产构建及全量 84 文件 / 791 项测试通过。全过程使用无窗口浏览器，辅助监听及更新后的 8443 仅绑定回环，保留原配置文件、数据库、登录资料和 5173 进程。
- 8443 原 debug 进程经管理 API 优雅退出，使用本次 release 产物启动，未重置 ADB server。运行参数仅本次设置 `GAMER_LOCAL_ONLY=1`、`ADB_MDNS=0`，配置文件未修改。后端重启会断开投屏，需重新连接。

本地 ignored 证据位于 `server/target/cdp-test/`：`cloud-repro.log`、`cloud-priority-before.log`、`cloud-priority-after.log`、`cloud-main-fixed.log`、`cloud-typed.png`，以及 `input-priority-*-build.log` 和 `input-priority-web-tests.log`。不覆盖云游戏内操作、真实登录、支付或验证码流程。
