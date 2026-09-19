# Gamer iframe 轻量 UI 接入

将 `example.html` 放入插件 UI 资产目录；使用已有 iframe contribution 的入口字段指向 `ui/example.html`。本目录不是独立 WASM 插件，不改变插件能力或权限。开发指南的 manifest/打包说明仍适用。

宿主 iframe 保留 `sandbox="allow-scripts"`。示例自身携带严格 CSP：默认禁止资源，只允许哈希匹配的内联脚本/样式，没有 unsafe-inline、CDN 或网络请求。初始连接校验 parent window、协议版本和 MessagePort，后续全部使用专属端口。不要添加 `allow-same-origin` 或 `allow-forms` 来换肤或执行按钮动作。

修改 `example.template.html`、`example.js`、`gamer-ui.js`、`gamer-ui.css` 后，在仓库根目录执行 `node tools/build-ui-sdk.mjs`，重新生成全局脚本和自包含的 `example.html`（包括 CSP 哈希）。模块源码供构建工具打包复用。不要直接修改生成文件，也不要在打包后更改内联资源的换行，否则哈希失效。

采用单文件是为了适配不透明来源沙盒：独立 JS/CSS 请求不能依赖宿主的 SameSite=Strict 登录 Cookie，ES module 还受跨源检查约束。示例按钮显式 `type="button"`，通过 click 发桥消息；沙盒禁用原生表单提交，不能依赖 submit 事件。

`connectGamer().call(method, params)` 支持现有 `gamer-ui@1` 桥协议；请求包含唯一 ID、超时和关闭清理。`context.get` 的 theme 映射为本 iframe 的 CSS 变量，白名单仅接收六位十六进制颜色。状态结果通过 `status.set` 发给宿主，复制/详情使用声明式数据。不可通过桥传函数或 HTML。

控件默认 28px，正文 13px、说明 12px；必填内容直接展示，可选参数折叠；使用 label、焦点轮廓、状态区域和内联错误。官方插件遵守统一主题，第三方可自主采用；不要求 Vue 或任何组件库。
