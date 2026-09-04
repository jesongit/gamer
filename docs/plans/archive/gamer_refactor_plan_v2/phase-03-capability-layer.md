# Phase 3：Core Capability Layer 与 Vision 数据通路

## 目标

把 Gamer Core 的稳定能力正式抽象为 Capability Layer。当前 Rust YAML Engine 先调用这些接口，未来 WASM 直接复用。

---

## 1. Device Capability

建议：

```text
device.tap
device.swipe
device.key
device.text

app.start
app.stop
```

`device.shell` 单独作为 privileged capability，不默认暴露。

---

## 2. Input / Touch Capability

Core 负责：

- Keyboard / Mouse / Gamepad 标准化
- Touch pointer 分配
- Touch 生命周期
- scrcpy control backend

接口语义：

```text
touch.begin
touch.move
touch.end
```

WASM 不直接管理 scrcpy pointer id。

---

## 3. Frame Capability

第一版：

```text
frame.latest
frame.capture
frame.size
```

设计原则：

> 尽量传 Handle，不把完整 Frame bytes 跨层复制。

---

## 4. Vision Capability

第一版建议：

```text
vision.match_template
vision.match_many
vision.sample_color
```

未来可扩：

```text
vision.ocr
vision.detect
```

### 为什么 `match_many` 应属于 Core

它具有明确性能语义：

```text
一次 frame decode
→ 多模板复用
```

而不是简单业务语法糖。

---

## 5. Vision Frame 去 PNG 化

目标：

```text
H264 GOP
   ↓
decode once
   ↓
DecodedFrame / FrameHandle
   ├── match_template
   ├── match_many
   ├── sample_color
   └── screenshot 时才 encode PNG
```

避免：

```text
decode → PNG encode → Matcher PNG decode
```

---

## 6. Resource Capability

建议：

```text
resource.resolve
resource.open
```

插件和自动化层只看：

```text
ResourceId / ResourceHandle
```

不直接暴露主机文件路径。

---

## 7. Runtime / Run / Log

```text
runtime.sleep
runtime.cancelled

run.submit
run.cancel
run.status

log.write
```

---

## 8. Capability Registry

Core 内部先使用 trait registry：

```text
DeviceService
VisionService
RunService
ResourceService
```

未来 WASM Host 只是这些接口的 adapter。

---

## 验收标准

- 旧 YAML Engine 可以完全通过 Capability Layer 工作
- Runner 不直接调用 scrcpy implementation
- Matcher API 不依赖模板 PathBuf
- 多模板匹配可以复用一帧
- Screenshot 不再是 Vision 的唯一中间格式
- Capability API 输入输出尽量为小结构或 Handle

## 不做

- 不引入 WIT
- 不引入 Wasmtime
- 不做第三方 Provider
