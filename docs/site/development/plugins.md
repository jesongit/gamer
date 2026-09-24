# 开发与分发插件

插件用 `.gplugin` 归档分发，清单使用 manifest v2。新的插件使用 `gamer-` 前缀。第三方独立执行逻辑采用 WASM，已有官方视频功能使用宿主注册的 builtin 实现。

## 从 SDK 示例开始

仓库有三个可复制的示例：

| 示例 | 内容 |
| --- | --- |
| `sdk/examples/echo-minimal/` | 最小 WASM、声明式表单与 call 回显 |
| `sdk/examples/hello/` | 资源、日志、上下文与权限调用 |
| `sdk/examples/vision-probe/` | 设备与视觉能力探针 |

先按示例 README 运行构建、安装和调用，再替换自己的业务逻辑。示例可以复制到仓库外开发；打包时通过 `-Packer` 指向 Gamer 的 `plugin-packer` 工具。

```powershell
rustup target add wasm32-unknown-unknown
Set-Location sdk/examples/echo-minimal
.\build.ps1
```

归档打包不要求签名密钥。manifest 声明身份、版本、执行入口、权限、宿主接口版本、依赖和 UI。完整字段与可运行步骤见[插件开发指南源码](https://github.com/jesongit/gamer/blob/main/docs/guides/plugin-dev.md)和 [SDK README](https://github.com/jesongit/gamer/blob/main/sdk/README.md)。

## 官方插件目录

官方源码位于独立的 [gamer-plugins 仓库](https://github.com/jesongit/gamer-plugins)。主仓的 `plugins/` 是固定提交 submodule；用 `git submodule update --init --recursive` 初始化，不跟随远端最新提交。插件独立构建使用其 `sdk/lock.json` 锁定的 SDK 快照，主仓用 `node tools/check-plugin-sdk.mjs` 检查接口一致。

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
.\tools\build-plugins.ps1 -Plugin gamer-yaml
.\tools\build-plugins.ps1 -Plugin gamer-keymap
.\tools\build-plugins.ps1 -Plugin gamer-video
```

单插件构建保留其他插件的发行条目，生成 `web/public/plugins/*.gplugin` 并更新 `web/public/registry.json`。发布新版本时更新 manifest 版本，在「插件」页检查权限并导入或更新归档，然后保存编辑内容、刷新页面。

独立插件仓内运行 `./build.ps1`，产物默认进入该仓的 `dist/`；上面的主仓包装入口将产物放到主仓本地市场。插件修改先在插件仓提交，再更新主仓 gitlink；已发布的插件种子仍由发行锁固定。

`node tools/build-plugin-ui.mjs gamer-yaml` 用于主仓 UI 联调。普通 `pnpm --dir web build` 仅构建主界面壳，已安装插件从安装归档读取资产，因此也需要更新归档才能看到对应变化。

插件仓按插件标签发布 Release，每次携带完整 registry 和经过同一基线验证的插件归档；主发行的首次安装种子固定某个已公开版本。Gamer 插件页会独立发现新的公开目录，通过同源下载校验大小和 SHA256，安装或更新仍需用户确认。

**UI 和 WASM 可以在既有 SDK/ABI 内独立更新。** 修改 host 适配层、原生函数注册、资源保存校验或 WIT 接口时，必须同步构建兼容的服务端。

## 依赖与权限

依赖声明不会授予权限。必需依赖是启动门禁，可选依赖应通过能力发现降级。宿主不会自动下载或启用依赖，也不会级联停用其他插件。

插件读写资源时使用当前配置与自身插件作用域。不要跨目录读取其他插件数据；跨插件功能通过公开动作或共享消息契约交互。
