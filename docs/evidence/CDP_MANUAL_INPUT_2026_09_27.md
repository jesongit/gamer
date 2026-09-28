# CDP 手动投屏输入与工具条修复

- 隔离开发：`codex/cdp-browser-target`；本轮不合入主仓。仅部署前端产物到 `D:/code/gamer/server/target/cdp-test/web-dist`，8443 原服务不重启。
- 布局：第一行保留设备/配置包，第二行复用 Android 应用区的位置显示标签页/读取，与截图/粘贴/全屏并列；浏览器设置/关闭/删除收入设备更多菜单，帧率移到现有状态栏。
- 手动输入不再调用模板插件的坐标换算，用 Core geometry 和已显示图片尺寸映射。原服务安装的 gamer-yaml 3.3.13 UI 只有 videoWidth/videoHeight 路径；这种旧 UI 会把浏览器 img 当作缺少尺寸，错误回退至 1920×1080。核心手动操控不应要求插件更新。
- 真鼠标事件复现焦点错误：mousedown 后舞台内部 focusout 导致 down → release → up，打断按住。现在只有焦点离开整个舞台才释放，按下先聚焦舞台并阻止默认焦点转移；窗口失焦/隐藏也释放浏览器输入。
- 验证：前端 84 文件 / 789 测试通过；新增整壳图像坐标、焦点移动、按键和失焦释放回归，生产构建通过。
- 本机 Headless Edge 用真实鼠标与键盘事件操作 8444 Gamer 界面，再经 WebSocket/CDP 操作本机 fixture：点击计数加一、输入 z、中文粘贴、保持拖动直到 mouseup 均通过；页面无 JS 错误。GUI 截图核对第二行布局。测试使用本机管理 token，页面认证探测为测试桩；不覆盖登录鉴权、实际系统剪贴板授权或外部云平台。
- 本地证据（ignored）：测试目录 `ui-input-acceptance.json`、`ui-input-acceptance.log`、`ui-qa.png`。测试辅助 Edge 使用独立 profile，不操作用户云游戏页面。
