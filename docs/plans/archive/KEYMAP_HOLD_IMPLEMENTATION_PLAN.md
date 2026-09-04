# 键盘映射统一 Hold 实现计划

状态：无实机部分已完成；真机验收待执行

## 1. 背景与结论

当前键盘映射同时存在 `tap`、`hold`、`raw_key` 等动作。对于“键盘触发屏幕上的虚拟按键”这一场景，`tap` 与 `hold` 本质上都可以由同一套有状态触控事件表达：

```text
键盘 keydown → touch down
键盘 keyup   → touch up
```

因此，本计划将键盘映射中的屏幕触控动作统一为 `hold`：

- 快速按一下：`down → up`，自然表现为一次点击；
- 按住不放：持续保持 `down`；
- 松开按键：发送 `up`；
- 不根据按下时长使用定时器，也不新增“短按/长按识别”状态机。

`tap` 仍保留给脚本、REST 控制和其它明确的一次性动作；真正的 Android 物理按键仍使用 `key action=0/1`，不能混同为屏幕触控。

## 2. 目标

1. 键盘映射的虚拟触控统一使用 `hold`。
2. 鼠标触控与键盘触控共用同一套 `touch down/move/up` 语义。
3. 支持多个键同时按住，例如 W+A、W+D。
4. 在失焦、隐藏、切换方案、断开连接等情况下，远端设备不会残留按下状态。
5. 有状态控制只走 WebRTC DataChannel，不错误降级为不支持 `touch` 的 REST `press`。
6. 保持已有脚本 `tap`、REST `tap` 和未映射键盘 `key` 行为不变。

## 3. 非目标

- 不实现同一个绑定自动区分短按和长按的 `tap_or_hold` 模式。
- 不把普通 Android 按键改成屏幕坐标触控。
- 不用重复发送 `touch down` 模拟长按；长按只需要保持触点状态。
- 不修改脚本 v2 的 `tap` 语义。

## 4. 统一动作语义

### 4.1 键盘映射

| 映射动作 | keydown | keyup | 说明 |
|---|---|---|---|
| `hold` | `touch down` | `touch up` | 屏幕虚拟按键、方向键、摇杆等 |
| `swipe` | 发送完整滑动 | 无操作 | 一次性动作，保留现有语义 |
| `raw_key` | `key action=0` | `key action=1` | 发送真实 Android 按键 |
| 旧 `tap` | 兼容读取 | 无操作 | 新编辑器不再生成，后续可显式转换为 `hold` |

### 4.2 DataChannel 消息

单点按住使用以下消息：

```json
{
  "type": "touch",
  "action": "down",
  "pointer_id": 1,
  "x": 768,
  "y": 648
}
```

释放时使用相同的 `pointer_id`：

```json
{
  "type": "touch",
  "action": "up",
  "pointer_id": 1,
  "x": 768,
  "y": 648
}
```

约定如下：

- `pointer_id=0` 保留给鼠标/投屏直接触控；
- 键盘映射使用 `1..31`，同一绑定在 `down` 到 `up` 期间必须保持相同 ID；
- `down`、`move`、`up` 必须按顺序处理；
- `up` 即使坐标与 `down` 不同，也只按 `pointer_id` 结束对应触点。

## 5. 兼容策略

当前 keymap schema 是 `version: 1`，已有文件可能包含 `tap`。为避免保存旧方案后直接失效，建议采用兼容过渡：

1. 服务端暂时继续接受并原样保存旧 `tap`。
2. 新增或编辑绑定时，前端只提供 `hold`、`swipe`、`raw_key`。
3. 读取旧 `tap` 时可以显示“旧版点击”，运行期继续按旧语义执行。
4. 提供显式的“转换为 hold”或保存时转换，不在后台静默改写用户文件。
5. 待确认没有旧 `tap` 使用后，再考虑 keymap schema v2 并移除兼容分支。

脚本 YAML、脚本引擎和 REST 的 `tap` 不做迁移。

## 6. 实现分层

### 阶段一：冻结模型和发送边界

- 确认 keymap 新模型中 `hold` 只需要 `at`；暂不实现 `from/to` 形式的 hold-drag。
- 明确 `pointer_id` 是运行时触控标识，优先由前端按绑定稳定分配，不要求普通用户手工填写。
- 保留旧 `tap` 的读取和运行兼容，编辑器新增动作不再生成 `tap`。
- 将控制发送分为两类：
  - 一次性动作：`tap`、`swipe`，可以使用 REST fallback；
  - 有状态动作：`touch down/move/up`、`key down/up`，只允许 DataChannel。

涉及文档：

- `docs/reference/KEYMAP_SCHEMA.md`
- `docs/plans/archive/KEYBOARD_CONTROL_PLAN.md`
- 本文档

### 阶段二：前端键盘映射改为统一触控

涉及文件：

- `web/src/keymap-control.js`
- `web/src/keymap-control.test.js`
- `web/src/components/console/KeymapPanel.vue`
- `web/src/views/Console.vue`

工作项：

1. `hold` 的 `keydown` 只发送一次 `touch down`，浏览器 `repeat` 不重复发送。
2. `keyup` 根据已记录的绑定状态发送对应 `touch up`，不重新读取当前映射。
3. `held` 状态记录 `code`、坐标和 `pointer_id`，保证方案切换时仍能正确释放旧触点。
4. 修正 `sendControl` 返回值处理：DataChannel 未打开时，不能把 `hold` 标记为已发送，也不能调用 REST fallback。
5. `releaseAll()` 按所有活动触点发送 `up`，并清理本地状态。
6. 统一鼠标和键盘的 `sendTouchPhase()` 消息构造，避免两套协议字段逐渐分叉。
7. 修正按键映射 overlay：`hold` 按 `at` 显示为单点，不再错误要求 `from/to`。
8. `raw_key` 继续复用真实键盘控制的 `meta`、重复键和释放语义。

### 阶段三：服务端严格处理触控状态

涉及文件：

- `server/src/webrtc/mod.rs`
- `server/src/webrtc/viewer.rs`
- `server/src/device/scrcpy.rs`
- 必要时新增独立的触控状态模块

工作项：

1. `touch.action` 只接受 `down`、`move`、`up`，非法值直接拒绝。
2. 严格解析 `x`、`y` 和 `pointer_id`，不再使用缺省值静默转成坐标 `(0,0)` 或 `move`。
3. 将收到的 `pointer_id` 传入 `ScrcpySession::inject_touch()`，不再固定为 `0`。
4. 对每个 viewer 记录活动 pointer，校验 `move/up` 是否对应已建立的触点。
5. viewer、DataChannel 或 peer 终止时，对仍活动的 pointer 发送 `ACTION_UP`，然后清空状态。
6. 保持控制队列的串行消费，确保 `down → move → up` 顺序不被并发任务打乱。
7. 继续复用已有 scrcpy 触控包编码，不重新设计底层 `INJECT_TOUCH_EVENT` 格式。

### 阶段四：编辑器和文档收口

工作项：

- `KeymapPanel.vue` 将新建默认动作改为 `hold`。
- “hold · 按住（预留）”改为正式文案。
- hold 只显示单点坐标和取点操作。
- 更新 keymap schema 示例，说明一次快速按键就是 `down/up` 点击。
- 更新键盘控制计划，区分真实 Android key 与屏幕触控 hold。
- 明确 `pointer_id=0` 的鼠标保留规则和多键按住行为。

### 阶段五：测试和真机验收

#### 前端单元测试

- `hold` 首次 `keydown` 发送一次 `touch down`。
- `keydown repeat` 不重复发送 `touch down`。
- `keyup` 发送匹配坐标和 pointer ID 的 `touch up`。
- 快速按键的消息顺序为 `down → up`。
- 长时间按住期间没有额外触控消息，释放后才发送 `up`。
- W+A 两个绑定使用不同 pointer ID，并可独立释放。
- 方案切换、文本模式、窗口失焦、页面隐藏和 WebRTC 断开都会释放触点。
- DataChannel 不可用时，hold 不调用 REST fallback。
- 旧 `tap` keymap 仍能读取和运行。

#### 服务端测试

- `touch` 的 down/move/up 解析和非法字段拒绝。
- pointer ID 正确写入 scrcpy 触控包。
- 多 pointer 的顺序和独立释放。
- 未建立 pointer 的 move/up 被拒绝或安全忽略。
- viewer 断开时活动 pointer 全部释放。
- 现有 `key`、`tap`、`swipe` 协议测试不回归。

#### 真机验收

1. Space 绑定屏幕按钮，快速按一下只触发一次点击。
2. W 绑定方向键，按住期间游戏持续保持方向，松开立即停止。
3. W+A、W+D 可同时按住，松开任意一个不会影响另一个。
4. 鼠标拖动与键盘 hold 同时存在时，两个 pointer 不互相取消。
5. 切换标签页、窗口失焦、刷新或关闭页面后，设备没有残留触点。
6. 普通未映射键仍发送 Android key down/up，文本模式仍可输入文本。

## 7. 验收标准

满足以下条件才算完成：

- 新建 keymap 不再产生 `tap` 屏幕触控绑定。
- 键盘虚拟触控严格遵循 `keydown → down`、`keyup → up`。
- 单键和多键 hold 在真机上均能稳定工作。
- 所有异常退出路径都能释放活动触点。
- DataChannel 和 REST 的有状态/无状态边界清晰，没有 `touch` 请求误落 REST。
- 相关前端测试、服务端测试和真机验收均通过。

## 8. 风险与待确认项

1. 旧 keymap 中 `tap` 的兼容周期需要在实现前确定；建议先兼容读取，再在 UI 中逐步迁移。
2. `pointer_id` 是否完全由前端自动分配，还是允许高级用户在 YAML 中指定，需要统一约定；默认不建议用户手填。
3. 某些 Android 游戏可能依赖特定压力值或触控按钮字段；第一版保持 `down=1.0`、`up=0.0`，如真机验证发现差异再单独扩展。
4. 如果未来确实需要“同一按键短按是 tap、长按是 hold”，应新增独立动作类型和明确的 `hold_ms`，不要改变本计划中 `hold` 的 down/up 语义。
