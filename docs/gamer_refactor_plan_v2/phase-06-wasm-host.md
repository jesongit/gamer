# Phase 6：WASM Host、WIT 与 Extension 生命周期

## 目标

正式引入真正可安装、删除的代码级 Extension。WASM 负责规则、状态和业务编排，重计算和系统访问仍留 Native Core。

---

## 1. Runtime 选择原则

优先考虑：

```text
Wasmtime + Component Model + WIT
```

原因：

- Host API 类型结构化
- 适合 InputEvent / MatchResult / RunRequest
- 避免手工 ptr + len ABI
- 便于接口版本化

---

## 2. Host API 分域版本化

建议：

```text
gamer:device@1
gamer:vision@1
gamer:input@1
gamer:touch@1
gamer:resource@1
gamer:run@1
gamer:runtime@1
gamer:log@1
```

不要一个巨型 `gamer_api_v1`。

---

## 3. 权限模型

manifest 显式声明：

```text
device.control
device.touch
vision.match
resource.read
run.submit
log.write
event.emit
```

默认禁止：

```text
filesystem
network
device.shell
process.spawn
```

如确需 shell，单独高权限确认。

---

## 4. Runtime lazy init

没有安装任何 WASM extension：

```text
不要初始化 Wasmtime runtime
```

首次加载插件时再启动。

全局共享：

- Engine
- 编译缓存
- Host linker

Compiled module 按 SHA256 缓存。

---

## 5. Extension 包格式

建议：

```text
.gplugin
```

内容：

```text
manifest.toml
plugin.wasm
ui/
  index.html   # 可选
```

App Package 与 `.gplugin` 必须严格区分。

---

## 6. manifest 基础字段

建议包含：

```text
id
name
version
publisher
gamer version
host API requirements
permissions
runtime entry
UI contribution
hash/signature metadata
```

---

## 7. 生命周期

状态建议：

```text
Available
Installed
Disabled
Starting
Running
Failed
Stopping
```

操作：

```text
install
uninstall
enable
disable
start
stop
update
```

---

## 8. UI Contribution 注册

Extension 加载后：

```text
manifest
→ PluginManager
→ UI Contributions
→ Frontend PanelRegistry
```

新增插件后右侧 Tab 可即时出现，不要求重新编译 Gamer。

---

## 9. Host API 只暴露稳定原子能力

不要提供：

```text
find_and_click_and_retry
```

而是：

```text
vision.match
device.tap
runtime.sleep
```

复杂策略由 Extension 自己组合。

---

## 10. 大数据不跨 WASM

不要：

```text
完整 RGB frame → WASM memory
```

推荐：

```text
ResourceHandle
FrameHandle
small MatchResult
```

甚至模板匹配直接：

```text
vision.match(device, resource, options)
```

由 Core 自己取最新 frame。

---

## 11. 安全边界

WASM 不直接：

- 读任意文件
- 访问任意网络
- 执行 shell
- 操作进程
- 获取 Gamer 内部对象

只能调用授权 Host Capability。

---

## 验收标准

安装 `hello.gplugin`：

- 可解析 manifest
- 可验证版本
- 可显示权限
- 可加载 WASM
- 可写日志
- 可读取 AppContext
- 可调用一个测试 capability
- 可卸载并彻底移除
- 不需要重新编译 Gamer

## 不做

暂不迁移 YAML。
先让插件基础设施稳定。
