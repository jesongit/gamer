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

---

## 收口记录（2026-09-04）

- Original Plan：第 6 节延迟测量为"建议测量"（browser → server → WASM → DeviceAction → scrcpy control，比较迁移前后 KeyDown/KeyUp P50/P95 等）。
- Final Decision：已建成两层测量并形成 baseline——进程内微基准（native vs WASM dispatch p95 3.7µs）+ 真机 E2E 基准 `phase0_android_keymap_e2e_latency_native_vs_wasm`（进程内 DataChannel 客户端替代浏览器，7 阶段 trace 埋点，native/WASM 同设备同会话对照），结果在 `benchmarks/results/keymap-e2e.json`：Server Internal Total P95 native 74µs vs WASM 102µs（WASM 增量 ≈+28µs），burst 热身后 wasm 执行 P50=2µs、无尾延迟退化；浏览器端 JS 开销未含（Browser RTT 与 Server 内部阶段分开统计）。
- Reason：Validation-01 落地，Gate B「Keymap 延迟可接受」由真机实测数据支撑；trace 信封字段与收集器默认关闭零开销，生产行为不变，后续可按 baseline 设 WASM−Native P95 增量阈值防架构退化。
