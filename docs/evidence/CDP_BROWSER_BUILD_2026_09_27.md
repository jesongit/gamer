# CDP release 构建与独立运行验收（2026-09-27）

源：主仓 `ff2e4e4`，插件 `b8804da7451200a20b13e99a0dc8e15f288a44ba`。

- 默认特性（含 WASM）`cargo build --locked --release` 通过，服务端版本 0.2.4。
- 官方插件构建入口生成 `gamer-yaml-0.1.0.gplugin`，guest Component、UI、pack/verify、SHA-256 自检通过；输出到独立测试目录，不改当前市场文件。
- Vite 生产构建输出到 `server/target/cdp-test/web-dist`，不覆盖现用静态资源。
- 独立环境 `server/target/cdp-test`：监听 127.0.0.1:8444、独立数据库与浏览器资料、禁用 Android 调用和自动更新；原 8443 debug 服务保持运行。
- 本地管理 API 安装构建出的 YAML 插件、导入示例配置包、登记本机测试页面目标。通过真实 WASM runner 手动运行 smoke.yaml：成功，页面显示中文“CDP 输入正常”和 W 按键。
- 在无前端 viewer 的情况下由 cron 实际触发 counter.yaml：成功，截图累计点击为 2（一次手动脚本 + 一次定时触发）。验收后任务改为每分钟一次并停用，供用户手动启用测试。
- 服务端截图可见输入结果，未连接真实云平台。本轮未验证云崩铁登录或视频播放，未操作用户主仓数据库。

构建产物、`BUILD.json` 哈希清单、`test.ps1` 启停脚本、README、示例 .gamerpkg、执行 JSON 与截图在 `server/target/cdp-test/`（忽略的构建目录）。测试环境已预装插件和示例包；首次打开测试网页由用户设置管理员密码，未写入默认口令。测试 UI 应使用无痕窗口，隔离不同端口同主机名的登录 Cookie。
