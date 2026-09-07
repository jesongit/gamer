# Gamer 插件 SDK（示例与模板）

本目录是第三方插件开发者的起点：三个可复制、可独立构建的示例插件 +
随示例走的 WIT 契约快照。**写一个 Gamer 插件不需要修改 Gamer 源码、
不需要签名密钥、不需要加入任何市场。**

> 状态（2026-09-07，对应服务端基线 `eae786c` 工作树）：三个示例的
> 构建/打包/inspect/install 均已实测通过；**但携带 Host API import 的插件
> （hello / vision-probe）在当前服务端基线上 start 会触发宿主进程 abort
> （宿主侧缺陷，非示例问题），修复前请先阅读
> [docs/evidence/phase3_sdk_examples.md](../docs/evidence/phase3_sdk_examples.md)
> 的「能力缺口」一节**；`echo-minimal`（零 import）可完整运行。

## 示例清单

| 示例 | 演示内容 | 权限 | 当前基线可完整运行 |
| --- | --- | --- | --- |
| [`echo-minimal`](examples/echo-minimal/) | 零 import 最小插件：declarative 面板 + `call` 回显/计算、全生命周期 | 无 | ✅ |
| [`hello`](examples/hello/) | manifest v2 + declarative UI + call 命令 + Package 私有数据 + 插件日志 + 运行上下文 + 权限闭集自检 | `resource.read` `log.write` | ❌ 宿主缺陷，待修复 |
| [`vision-probe`](examples/vision-probe/) | device/vision/input 权限声明 + 受控 Host API 真实调用（capture/sample-color 只读探针、tap dry-run） | `device.read` `vision.match` `vision.color` `input.tap` | ❌ 宿主缺陷，待修复 |

三个示例工程结构完全同构，学会一个就会全部：

```text
examples/hello/
├── manifest.toml          # 插件清单（manifest v2）：id/版本/entry/权限/UI/执行形态
├── Cargo.toml             # guest crate（独立，不属于任何 workspace）
├── build.ps1              # 一键：wasm32 构建 → componentize → signer pack → verify
├── wit/gamer/host.wit     # gamer:host@1.0.0 契约快照（随示例走，仓库外可用）
├── src/lib.rs             # guest：实现 run + call（业务全在这里，约 200 行）
├── src/bin/componentize.rs# core module → WASM Component 的后处理（可直接照抄）
└── dist/                  # 构建产物 *.gplugin
```

## 快速开始（echo-minimal，5 分钟）

```sh
rustup target add wasm32-unknown-unknown     # 首次
cd sdk/examples/echo-minimal
pwsh ./build.ps1                             # 产物 dist/com.example.echo-1.0.0.gplugin
# 按 README.md 里的 curl 序列 inspect → install → call
```

## 从 Gamer 仓库外开发

把任一示例目录整体复制到任意位置即可：

```sh
pwsh ./build.ps1 -Signer <gamer 仓库>/tools/plugin-signer/target/release/gamer-plugin-signer.exe
```

`plugin-signer` 是 Gamer 仓库自带的打包/校验 CLI（`pack` / `inspect` /
`verify`，免签名，无需密钥）；除此之外不依赖 Gamer 源码树的任何部分。

## 文档

- 插件开发指南（从零到安装运行、manifest 全字段、权限闭集、调试与错误对照）：
  [docs/guides/plugin-dev.md](../docs/guides/plugin-dev.md)
- 插件 API 参考（WIT world、Host API 域与版本、19 项权限闭集、UI contribution schema）：
  [docs/reference/PLUGIN_API.md](../docs/reference/PLUGIN_API.md)
