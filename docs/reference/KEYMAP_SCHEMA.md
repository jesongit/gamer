# 按键映射 YAML

按键映射独立于脚本 YAML，按应用包名存储在 `data/<package>/keymap/`。文件名是方案名加 `.yaml`，方案 ID 是 `<package>/<name>.yaml`。

```yaml
version: 1
name: 战斗方案

bindings:
  - key: Space
    action:
      type: hold
      at: [0.72, 0.86]
  - key: KeyE
    action:
      type: swipe
      from: [0.40, 0.80]
      to: [0.60, 0.80]
      duration_ms: 300
  - key: KeyQ
    action:
      type: raw_key
      keycode: 111
```

`hold` 是屏幕触控动作：键盘按下发送 `touch down`，键盘释放发送 `touch up`。因此快速按一下自然就是一次 `down → up` 点击；按住期间不重复发送触控消息，直到释放才发送 `up`。

约束如下：

- 顶层只允许 `version`、`name`、`bindings`；`version` 当前必须为 `1`，`bindings` 必须是列表。
- `key` 使用浏览器 `KeyboardEvent.code`，同一方案内不得重复；未知物理键不能启用。
- `hold` 需要单点 `at`，只用于屏幕触控按住，不使用 `from`、`to`；`swipe` 需要 `from`、`to` 和 `duration_ms`；`raw_key` 用于真实 Android key，需要可映射的 `code` 或正整数 `keycode`。
- `tap` 仅为旧版 keymap 的读取和运行兼容保留，新建绑定不生成它；读取旧 `tap` 后保存不会静默转换为 `hold`，只有用户显式修改动作时才转换。
- 坐标是 `[x, y]` 归一化值，范围为 `0..=1`；时长为毫秒，必须为正数并受服务端上限约束。
- 动作字段采用白名单，未知字段、未知动作和非法 YAML 都会拒绝保存。

触控运行时使用 `pointer_id` 区分同时存在的触点：`pointer_id=0` 永久保留给鼠标/投屏直接触控；键盘 hold 使用 `1..31`，同一绑定从 `down` 到 `up` 必须保持相同 ID。多个键（例如 W+A）各自占用不同 ID，释放一个不会取消另一个；keymap YAML 不需要手填这个运行时字段。

`raw_key` 与 `hold` 不可混同：前者发送真实 Android 按键的 `key action=0/1`，后者发送屏幕坐标的 `touch down/up`。未映射键仍按原有 Android key 透传规则处理。

前端编辑器支持可视化和原文两种模式；原文通过服务端校验后才会写盘。更新必须携带 `expected_version`，只有显式 `force: true` 才跳过版本冲突检查。

运行时规则：游戏模式先查当前方案，命中动作就消费键盘事件；未命中继续走原有 Android 按键透传。文本模式完全绕过映射。方案切换、文本模式切换、页面失焦、页面隐藏、WebRTC 断开和组件卸载都会释放映射层的按住状态。
