# 开发与分发插件

插件用 `.gplugin` 归档分发，清单使用 manifest v2。新的插件使用 `gamer-` 前缀。第三方独立执行逻辑采用 WASM，已有官方视频功能使用宿主注册的 builtin 实现。

## 从 SDK 示例开始

仓库有三个可复制的示例：

| 示例 | 内容 |
| --- | --- |
| `sdk/examples/echo-minimal/` | 最小 WASM、声明式表单与 call 回显 |
| `sdk/examples/hello/` | 资源、日志、上下文与权限调用 |
| `sdk/examples/vision-probe/` | 设备与视觉能力探针 |

先按示例 README 运行构建、安装和调用，再替换自己的业务逻辑。示例可以复制到仓库外开发；打包时通过 `-Signer` 指向 Gamer 的打包工具。

```powershell
rustup target add wasm32-unknown-unknown
Set-Location sdk/examples/echo-minimal
.\build.ps1
```

归档打包不要求签名密钥。manifest 声明身份、版本、执行入口、权限、宿主接口版本、依赖和 UI。完整字段与可运行步骤见[插件开发指南源码](https://github.com/jesongit/gamer/blob/main/docs/guides/plugin-dev.md)和 [SDK README](https://github.com/jesongit/gamer/blob/main/sdk/README.md)。

## 官方插件目录

```text
plugins/gamer-yaml/
├── manifest.toml
├── README.md
├── build.ps1
├── host/           # Rust 宿主适配，随服务端编译
├── guest/          # WASM Component
├── interpreter/    # YAML 解释器，与 guest 和宿主测试共用
└── ui/             # 独立 Vue / Vite 工程
```

按键映射插件有 host、guest 和 ui；视频插件有 host 和 ui，没有占位 WASM。

## 独立构建与更新

在仓库根目录运行：

```powershell
.\plugins\gamer-yaml\build.ps1
.\plugins\gamer-keymap\build.ps1
.\plugins\gamer-video\build.ps1
```

单插件构建保留其他插件的发行条目，生成 `web/public/plugins/*.gplugin` 并更新 `web/public/registry.json`。发布新版本时更新 manifest 版本，在「插件」页检查权限并导入或更新归档，然后保存编辑内容、刷新页面。

`node sdk/ui/build-modules.mjs gamer-yaml` 只同步本地静态 UI。已安装插件从安装归档读取资产，因此也需要更新归档才能看到对应变化。

**UI 和 WASM 可以在既有 SDK/ABI 内独立更新。** 修改 host 适配层、原生函数注册、资源保存校验或 WIT 接口时，必须同步构建兼容的服务端。

## 依赖与权限

依赖声明不会授予权限。必需依赖是启动门禁，可选依赖应通过能力发现降级。宿主不会自动下载或启用依赖，也不会级联停用其他插件。

插件读写资源时使用当前配置与自身插件作用域。不要跨目录读取其他插件数据；跨插件功能通过公开动作或共享消息契约交互。
