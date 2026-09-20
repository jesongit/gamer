# gamer-keymap

按键映射编辑器与 WASM 输入规则。插件 ID、目录名和发行归档均使用 `gamer-keymap`。

## 目录与开发

- `manifest.toml`：身份、版本、权限、依赖和面板声明的单一来源。
- `ui/`：独立 Vue/Vite 工程；`entry.js` 导出 SDK v1 接入契约及面板描述。
- `host/`：Rust 宿主适配代码，随 Core 编译；新原生函数、WIT 或资源校验契约变更需要更新宿主。
- `guest/`：可单独构建/安装的 WASM Component。

```powershell
# 仓库根目录执行；只打包本插件，其他市场条目保留
.\plugins\gamer-keymap\build.ps1
# 仅 UI 开发构建（同步到本地 web/public/plugin-ui 与已存在的 web-dist）
node sdk/ui/build-modules.mjs gamer-keymap
pnpm --dir plugins/gamer-keymap/ui test
```

依赖 Node 20+、pnpm、Rust；WASM 插件还需 `rustup target add wasm32-unknown-unknown`。构建脚本安装锁定的 UI 依赖并校验 .gplugin 的 ID、版本和 SHA-256。产物在 `web/public/plugins/`；在 Gamer「插件」页导入后保存当前编辑并刷新页面。

UI 与 WASM 可在现有宿主契约内独立更新，无需重编译主程序。`host/` 不是热加载 Rust 库，这部分改动需服务端构建。跨插件功能使用公开动作或共享消息通道；不能跨目录修改其他插件的数据。

UI 与 Core 共享的模块清单在 `sdk/ui/host-modules.json`，构建时映射为宿主 SDK，不打包另一份 Vue/store。新增宿主 API 要先扩展并验证该契约。宿主 UI 声明 `ui.host` 权限，在安装确认中明确其页面访问能力；第三方需要隔离时使用原有 sandbox iframe SDK。

通用说明：[插件开发指南](../../docs/guides/plugin-dev.md)，[界面设计规范](../../docs/design/gamer-ui-spec.md)。
