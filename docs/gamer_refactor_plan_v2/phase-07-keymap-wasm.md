# Phase 7：Keymap 作为首个真实 WASM Extension

## 目标

用 Keymap 验证完整动态插件体系，因为它数据量小、输入输出小、延迟可测，而且卸载后不影响基础投屏。

---

## 1. Core 负责 Input Gateway

浏览器 / Gamepad 输入统一标准化为：

```text
InputEvent
├── KeyDown
├── KeyUp
├── MouseDown
├── MouseUp
├── MouseMove
├── Wheel
├── GamepadButton
└── GamepadAxis
```

---

## 2. Keymap WASM 负责

```text
InputEvent
   ↓
mapping rule
   ↓
state
   ↓
InputResult / DeviceAction
```

建议：

```rust
struct InputResult {
    consume: bool,
    actions: Vec<DeviceAction>,
}
```

支持：

```text
Consumed
Pass
Actions
```

---

## 3. Touch pointer 归 Core

WASM 不直接处理 scrcpy pointer id。

接口：

```text
touch.begin
touch.move
touch.end
```

返回：

```text
TouchHandle
```

Keymap 只保存 Handle。

---

## 4. Keymap YAML 与 Keymap Engine 分离

```text
keymap.wasm
=
Keymap Engine
```

应用专属：

```text
default.yaml
controller.yaml
...
```

来自 App Package。

---

## 5. 前端 Panel

Keymap Extension manifest 贡献：

```text
映射
```

Rich UI 推荐 sandbox iframe。

UI 通过 Bridge：

- 读取当前 AppContext
- 读取/保存插件数据
- 请求左侧 pickPoint / selectRegion
- 调用自己的 plugin backend

不直接访问 Device REST API。

---

## 6. 延迟测量

重点测：

```text
browser event
→ server
→ WASM
→ DeviceAction
→ scrcpy control
```

比较迁移前后：

- KeyDown P50/P95
- KeyUp P50/P95
- hold stability
- multi-key
- mouse movement
- memory

---

## 7. Plugin Runtime 与 UI 生命周期

Keymap runtime 必须可以：

```text
Panel closed
但 runtime 仍 Running
```

否则切到日志页时键盘映射会停止，这是错误设计。

---

## 8. 本地安装验证

`.gplugin`：

```text
manifest.toml
plugin.wasm
ui/index.html
```

安装：

```text
上传
→ 权限确认
→ install
→ start
→ tab 出现
```

卸载：

```text
stop
→ unregister panel
→ remove files
```

---

## Gate B

通过以下条件后才迁移 YAML：

- WASM 空闲 RSS 可接受
- Keymap 延迟可接受
- 启停/卸载稳定
- 权限边界成立
- iframe Bridge 成立
- UI/Runtime 生命周期解耦
