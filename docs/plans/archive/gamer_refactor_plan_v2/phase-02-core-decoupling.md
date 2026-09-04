# Phase 2：Core 解耦与运行模型泛化

## 目标

在不引入 WASM 的情况下，把 Core 从现有 YAML / Script / Viewer / 路径语义中解耦，为后续 App Package 与多 Runner 做准备。

---

## 1. 引入 AppContext

不要继续让一个 `pkg` 同时表示：

- Android package name
- 脚本目录
- 模板目录
- Keymap 分区
- 运行资源身份

建议：

```rust
struct AppContext {
    device_id: DeviceId,
    android_package: AndroidPackageName,
    content_package: Option<AppPackageId>,
}
```

运行时：

```rust
struct RunContext {
    run_id: RunId,
    app: AppContext,
}
```

---

## 2. 引入 ResourceId

业务接口逐步禁止传入：

```rust
PathBuf
```

改为：

```rust
ResourceId
```

例如：

```rust
vision.match_template(resource_id, options)
```

真正路径解析由：

```text
ResourceResolver
```

负责。

---

## 3. RunManager 去 YAML 化

RunManager 只负责：

- run_id
- device-level 互斥
- cancel
- 状态
- 生命周期
- 运行历史

目标请求模型：

```rust
struct RunRequest {
    device_id: DeviceId,
    app: AppContext,
    runner_id: String,
    entrypoint: String,
    payload: RunPayload,
}
```

例如：

```text
runner_id = gamer.yaml
entrypoint = daily
```

未来：

```text
runner_id = gamer.macro
runner_id = gamer.python
```

不修改 RunManager。

---

## 4. Runner 不再直接持有 ViewerMap

引入极薄接口：

```rust
trait EventSink {
    async fn emit(&self, event: RuntimeEvent);
}
```

Adapter 决定发送到：

- WebRTC DataChannel
- WebSocket
- Log
- Ignore

第一阶段不要造完整 EventBus。

---

## 5. DeviceManager 去“脚本消费者”语义

逐步引入：

```text
DeviceActivity / Lease
```

例如：

- ViewerLease
- RunLease
- CaptureLease
- ExtensionLease

DeviceManager 只关心：

```text
device currently has active consumers?
```

而不是消费者具体是什么。

---

## 6. 调整 Composition Root

当前启动路径、router 构造、background task 尽量统一。

目标：

```text
RuntimeServices::start()
        ↓
AppState
        ↓
build_router(&services)
```

Router 只注册 HTTP 路由，不负责隐式启动后台生命周期。

---

## 验收标准

- RunManager 不需要知道 YAML 类型
- Runner 不直接依赖 ViewerMap
- Resource 接口开始使用 ResourceId
- Android package 与内容包身份分离
- DeviceManager 不再硬编码“script running”
- 当前旧 YAML 仍可正常运行

## 回滚点

保持旧 ScriptStore adapter，在迁移未完成前允许双轨。
