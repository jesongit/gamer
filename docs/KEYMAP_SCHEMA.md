# 按键映射 YAML

按键映射独立于脚本 YAML，按应用包名存储在 `data/<package>/keymap/`。文件名是方案名加 `.yaml`，方案 ID 是 `<package>/<name>.yaml`。

```yaml
version: 1
name: 战斗方案

bindings:
  - key: Space
    action:
      type: tap
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

约束如下：

- 顶层只允许 `version`、`name`、`bindings`；`version` 当前必须为 `1`，`bindings` 必须是列表。
- `key` 使用浏览器 `KeyboardEvent.code`，同一方案内不得重复；未知物理键不能启用。
- `tap` 需要 `at`；`swipe` 需要 `from`、`to` 和 `duration_ms`；`raw_key` 需要正整数 `keycode`；`hold` 预留为按下/释放态动作，可携带 `from`、`to`。
- 坐标是 `[x, y]` 归一化值，范围为 `0..=1`；时长为毫秒，必须为正数并受服务端上限约束。
- 动作字段采用白名单，未知字段、未知动作和非法 YAML 都会拒绝保存。

前端编辑器支持可视化和原文两种模式；原文通过服务端校验后才会写盘。更新必须携带 `expected_version`，只有显式 `force: true` 才跳过版本冲突检查。

运行时规则：游戏模式先查当前方案，命中动作就消费键盘事件；未命中继续走原有 Android 按键透传。文本模式完全绕过映射。方案切换、文本模式切换、页面失焦、页面隐藏、WebRTC 断开和组件卸载都会释放映射层的按住状态。
