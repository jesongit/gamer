# 投屏键盘控制接入计划

状态：已实施（首版代码与自动化测试完成，真机手工验收待执行）

## 1. 当前现状

投屏画面目前已支持鼠标点击、拖动和滚轮控制，前端通过 WebRTC `control` DataChannel 发送控制消息。

服务端已经支持按键消息：

```json
{
  "type": "key",
  "action": 0,
  "keycode": 29,
  "repeat": 0,
  "meta": 0
}
```

其中 `action=0` 表示按下，`action=1` 表示释放；服务端会将消息编码为 scrcpy 的 Android key event。

当前前端没有把浏览器 `keydown/keyup` 转成控制消息。现有全局键盘监听只处理 `Escape`，用于关闭取点模式、工具栏菜单、设置弹窗和资源预览等页面 UI，不负责设备按键转发。

## 2. 推荐交互方式

采用“投屏区域获得焦点后接收键盘”的模式：

- 给投屏区域增加 `tabindex`，用户点击投屏画面后获得键盘焦点；
- 只在投屏区域处于键盘焦点时发送设备按键；
- 焦点离开投屏区域后停止接收键盘；
- 输入框、文本域、下拉框、脚本编辑器和弹窗内的键盘输入不转发；
- 保留现有全局 `Escape` 监听，页面 UI 关闭优先于设备按键；
- 可在投屏工具条增加“键盘已启用”状态提示，避免用户误以为任意页面按键都会控制设备。

不建议直接把所有 `window.keydown` 事件发送给设备，否则编辑脚本名称、填写设置或操作下拉框时也会控制 Android 设备。

## 3. 前端实现

### 3.1 投屏组件接线

在 `ConsoleVideoStage.vue` 的投屏容器或视频元素上增加：

- `tabindex="0"`；
- `keydown` 回调；
- `keyup` 回调；
- 获得焦点和失去焦点处理；
- 窗口失焦、页面隐藏时的按键释放处理。

事件回调通过 `Console.vue` 传入，与现有鼠标事件保持同一层级。

### 3.2 按键映射

新增独立的键盘映射模块，并使用 `KeyboardEvent.code` 映射 Android keycode。首批覆盖：

- 字母、数字；
- 方向键、Home、End、PageUp、PageDown；
- Space、Enter、Tab、Escape、Backspace、Delete；
- Shift、Ctrl、Alt、Meta；
- F1～F12；
- 常用标点和数字键盘按键。

使用 `event.code` 适合游戏的物理按键控制；不支持的按键应忽略并避免产生错误请求。`Escape` 初始建议映射为 Android `KEYCODE_ESCAPE`，不要默认替换成 Android Back；如需“Esc 等于返回”，应作为明确的产品选项。

### 3.3 按下、释放和长按

普通键盘按键使用 `type: "key"`，不使用现有工具栏的 `type: "press"`：

- `keydown` 发送 `action: 0`；
- `keyup` 发送 `action: 1`；
- 使用按键集合按 `event.code` 去重，避免重复发送按下事件；
- 浏览器自动重复时继续发送重复的按下事件，并填写 `repeat`；
- 根据当前按下的 Shift/Ctrl/Alt/Meta 计算 `meta`；
- 失焦、窗口切换或页面隐藏时，为所有仍处于按下状态的按键补发释放事件。

这样可以支持游戏中的长按、连续移动和组合键，同时避免设备出现按键卡住。

### 3.4 浏览器默认行为和快捷键

只对处于投屏键盘焦点、且成功映射的按键调用 `preventDefault()`，防止空格滚动页面或方向键移动页面。浏览器和操作系统保留的快捷键（例如关闭标签页、地址栏、刷新、开发者工具等）不能保证被网页拦截，应在文档和界面提示中说明。

## 4. DataChannel 与 REST fallback

键盘按下/释放必须优先通过 WebRTC DataChannel 发送。当前 REST `/api/devices/:id/control` 只支持 `press`，不支持带按下/释放状态的 `key`。

推荐分两步处理：

1. 首版键盘只走 DataChannel；通道不可用时提示“键盘控制通道未连接”，不要为每次按键调用不兼容的 REST fallback；
2. 如果需要断链期间仍支持键盘，再扩展 REST `ControlReq`、校验逻辑和 `Ctl` 枚举，使 REST 与 DataChannel 共用 `key(action/keycode/repeat/meta)` 语义。

现有服务端 DataChannel 处理和 scrcpy 按键编码无需重新设计，只需补充测试确认动作顺序和错误处理。

## 5. 测试计划

### 前端单元测试

- 常用 `KeyboardEvent.code` 到 Android keycode 的映射；
- 未知按键被忽略；
- `keydown`/`keyup` 的 action 正确；
- 自动重复和按键去重；
- Shift/Ctrl/Alt/Meta 组合键的 `meta` 状态；
- 投屏失焦时释放所有已按下按键；
- 输入框、脚本编辑器和弹窗内的按键不发送。

### 服务端测试

- `key` 消息能被解析为按下和释放；
- keycode、action、repeat、meta 能正确编码为 scrcpy 控制包；
- 非法 keycode 或非法 action 不会造成异常；
- 若实现 REST fallback，REST 与 DataChannel 的语义一致。

### 手工验收

- 点击投屏画面后，A/W/S/D、方向键、空格和 Enter 能控制设备；
- 长按按键能持续触发，松开后设备立即停止；
- 日志中能看到按键 DataChannel 消息，顺序为按下、重复、释放；
- 投屏失焦、切换标签页或刷新前不会留下卡住的按键；
- 编辑脚本和设置时键盘不会控制设备；
- 工具栏 Home/返回/音量按钮和全局 Escape 行为保持不变。

## 6. 实施顺序

1. 新增键盘映射和按键状态管理模块；
2. 为投屏组件增加焦点与 `keydown/keyup` 接线；
3. 在 `Console.vue` 中接入 DataChannel 按键发送和失焦释放；
4. 增加前端与服务端测试；
5. 根据实际设备验证游戏按键、长按、组合键和输入控件隔离；
6. 只有确认确实需要时，再补 REST `key` fallback。

## 7. 首版实施结果

- 新增 `web/src/keyboard-control.js`：物理按键映射、修饰键 meta、目标过滤、重复按键去重和释放状态管理。
- `ConsoleVideoStage.vue` 增加可聚焦投屏区域；`Console.vue` 仅通过 WebRTC `control` DataChannel 发送 `key`，并在失焦、窗口失焦、页面隐藏和连接清理时释放按键。
- 服务端 DataChannel `key` 字段现在显式校验 action/keycode/repeat/meta，非法消息拒绝并不下发到 scrcpy；REST fallback 仍按计划暂不扩展。
- 已完成前端映射/接线测试与服务端协议测试；点击画面后的真实设备长按、组合键和浏览器快捷键行为仍需按第 5 节手工验收。
