# 局域网配置导入与配置源兼容性（2026-10-23）

已修复源码并完成前端构建及隔离验证。未发布 Gamer 新版本、未部署到用户线上环境、未修改真实安装或操作设备。本次涉及 Gamer 本体的前端与服务端，更新 HSR 配置包本身不能带入这些修复。

## 原因与行为

本地 `.gamerpkg` 导入原先强制依赖 `crypto.subtle`；局域网 HTTP 页面没有该接口，文件尚未上传就会失败。`sha256Hex` 现在优先使用 WebCrypto，接口缺失时动态加载随前端构建的 `@noble/hashes@2.4.0` SHA-256 实现；继续向导入接口传递 `X-Expected-Sha256`。没有跳过完整性校验，也不依赖 CDN。原生哈希接口存在但运行报错时仍保留该错误。

配置源原先只接受 GitHub 仓库路径。新增独立的下载地址解析，接受任意域名的 HTTPS 前缀代理（包括带路径前缀的代理）、独立目录或直接 JSON 地址，依据实际目录格式验证。发布插件仍使用原有严格 GitHub 仓库解析，不将下载代理作为 `gh` 发布目标。

前缀代理优先读取代理后的最新 Release 接口；接口不支持或没有返回 Release 时，探测代理后的 `releases/latest/download/packages.json`。已返回 Release 但资产校验不通过时保留失败，不能用目录回退隐藏校验错误。独立目录只从目录 JSON 的同级路径下载指定文件。所有模式继续校验目录 schema、条目数/大小预算、固定文件名、归档大小、SHA-256、归档结构和包内元数据；目录不能提供任意外部下载 URL。

添加源时保存规范地址，前端随后读取目录并显示探测结果。保存成功只说明地址合法；目录读取或验证失败仍显示对应源错误，不将网络错误当作空目录。源设置、缓存和已安装配置独立，镜像请求不会静默绕回直连 GitHub。

## 验证

- 前端定向测试：`sha256.test.js`、`package-context.test.js`、`package-market.test.js`、`plugin-center.test.js`，共 **44/44**。覆盖 SHA 标准向量/多块输入、视图偏移、原生接口及错误、HTTP 导入的真实摘要传递、错误摘要拒绝、多域名镜像显示与保存。
- `pnpm --dir web build` 通过；SHA-256 回退实现成为本地构建 chunk，可在离线环境加载。
- Rust `package_market` 定向测试 **12/12**，另有一项默认忽略的显式联网测试。覆盖通用镜像前缀、仅支持下载的代理、独立目录、缓存与持久化、错误摘要/包内元数据拒绝、资产地址不匹配拒绝以及原有严格发布仓库解析。
- 配置源 REST 回归 **1/1**，覆盖多域名镜像和静态目录的规范地址保存、非法地址拒绝以及移除源后的下载拒绝。原先将所有非 GitHub 域名都视为非法的断言已更新为按新地址规则检查。
- 发布插件相关回归 **6/6**，两项需显式指定仓库的远端集成测试保持忽略，未创建新的远端 Release。
- 真实 Chromium 的隔离 HTTP 测试：访问非 localhost 的 `http://package-http.test:18555/`，实际 `isSecureContext=false`、`crypto.subtle` 缺失；调用生产 `usePackageContext` 导入完整已发布包，独立 Python 服务复核上传摘要、ZIP 与 manifest，成功显示 `hsr@1.0.1`。容器禁网、只读、无宿主端口、临时浏览器资料。这验证浏览器及生产导入前端；接收端为独立校验夹具，不能代称线上 Gamer 导入验收。
- 显式联网测试分别使用用户原始镜像仓库地址及完整 `packages.json` 地址，两次均通过实际服务端 `PackageMarket` 保存源、读取目录、下载归档并校验包内元数据，未安装归档：

  ```text
  https://gh-proxy.com/https://github.com/jesongit/gamer-packages
  https://gh-proxy.com/https://github.com/jesongit/gamer-packages/releases/latest/download/packages.json
  ```

两次实际下载均为 `hsr@1.0.1`，671733 字节，SHA-256 为 `7f6b5eff1400bac634b5097e504c285a114521ab3a72d4bd585a17fa45c4139a`，与先前公开发布的配置包一致。其他域名使用确定性网络夹具测试，未声称验证了每家第三方镜像的实时可用性。

本地详细记录位于 `server/target/http-package-compat-20261023/`：`browser-report.json`、`live-source-report.json` 和浏览器验收夹具。

单独执行 `pnpm --dir web exec tsc --noEmit` 未通过：现有 `web/tsconfig.json` 仍只包含已经迁移的 `src/script-editor/**/*.ts`，报 `TS18003: No inputs were found`；配置与当前 HEAD 一致，本次未修改。前述 Vite 构建和 Vitest 通过不能等同于全项目 TypeScript 类型检查通过。

## 重现命令

```powershell
pnpm --dir web exec vitest run src/sha256.test.js src/package-context.test.js src/package-market.test.js src/plugin-center.test.js
pnpm --dir web build
cargo test --manifest-path server/Cargo.toml --locked --offline package_market -- --nocapture
cargo test --manifest-path server/Cargo.toml --locked --offline package_sources_are_authenticated_persistent_settings_independent_of_packages -- --nocapture

$env:GAMER_TEST_CONFIG_SOURCE = 'https://gh-proxy.com/https://github.com/jesongit/gamer-packages'
cargo test --manifest-path server/Cargo.toml --locked --offline package_market::tests::configured_source_live_acceptance -- --ignored --exact --nocapture
```

`--offline` 仅约束 Cargo 依赖解析；最后一项测试仍显式访问所配置的公开远端源。普通测试不运行该联网验收。
