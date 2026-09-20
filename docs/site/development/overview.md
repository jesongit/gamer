# 架构与开发环境

Gamer 的 Core 提供设备、WebRTC、视觉、媒体、录制、资源、运行、任务和插件生命周期机制。业务流程与面板由插件提供。

```text
浏览器（Vue 3）
  ├─ HTTP / WebSocket → Rust Core（axum）
  ├─ WebRTC 视频轨 ← 设备画面
  └─ DataChannel → 设备控制与运行反馈

Rust Core
  ├─ ADB / scrcpy → Android 设备
  ├─ PackageStore → 配置包与插件资源
  ├─ 任务 / 调度 / 运行管理
  └─ 插件宿主 → WASM / builtin + 插件 UI
```

## 源码归属

| 目录 | 职责 |
| --- | --- |
| `server/src/` | Core 稳定机制、API 与插件宿主 |
| `web/src/` | 工作台壳、公共控件、上下文与插件 UI 加载 |
| `plugins/gamer-yaml/` | 自动化、模板、函数及 YAML 解释器 |
| `plugins/gamer-keymap/` | 按键映射界面、输入规则与 WASM guest |
| `plugins/gamer-video/` | 视频面板、项目与宿主适配 |
| `sdk/` | 第三方插件示例和 UI SDK |

一个资源由配置包 ID、插件 ID、插件内路径定位。Core 负责存储、隔离与版本检查，插件解释自己目录内的业务格式。Android 应用包名与配置包 ID 是独立的命名空间。

## 准备环境

按[源码安装](../guide/installation.md#从源码启动)准备工具。服务端和前端可以分别调试：

```powershell
# 终端一，从仓库根进入服务端目录
Set-Location server
cargo run

# 终端二，在仓库根目录
pnpm --dir web dev
```

## 验证修改

```powershell
pnpm --dir web test:run
cargo test --manifest-path server/Cargo.toml
cargo clippy --manifest-path server/Cargo.toml --all-targets -- -D warnings
```

完整本地门禁为 `tools/ci-local.ps1`。测试中的 WASM 场景需要 `wasm32-unknown-unknown` 目标。修改 UI 时同时核对紧凑布局、键盘焦点与两种主题下的可读性。

新增自动化操作优先通过插件函数和现有 capability 组合实现。变更共享 SDK 或 WIT 时，同时检查消费者与宿主接口；详细约定见仓库 [AGENTS.md](https://github.com/jesongit/gamer/blob/main/AGENTS.md)。
