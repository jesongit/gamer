# tools/plugin-signing — Dev 插件市场签名密钥（仅本地市场用）

- `<key-id>.key`：ed25519 私钥（64 位 hex）。**不入库**（.gitignore `*.key`），
  由 `tools/build-plugins.ps1` 缺失时自动生成，用于给官方演示 `.gplugin` 签 manifest。
- `<key-id>.pem`：SPKI PEM 公钥。**入库**，且被 server 内嵌为内置信任锚
  （`server/src/extensions/signature.rs` 的 `BUNDLED_DEV_PUBLIC_KEY_PEM`，有同步锁测试）。

换 keypair 的步骤：
1. 删除 `.key`，重跑 `tools/build-plugins.ps1`（自动生成并打印新公钥）。
2. 把新 `.pem` 内容同步进 `BUNDLED_DEV_PUBLIC_KEY_PEM`（或放入服务端信任目录
   `GAMER_PLUGIN_TRUST_DIR` / `<data>/plugin-trust/<key-id>.pem` 覆盖内置锚）。
3. 重新构建 server 与插件产物（`.gplugin` 的签名随新私钥变化）。

生产部署绝不能复用这里的 keypair：官方发布应使用独立 keypair 并通过信任目录注入。
