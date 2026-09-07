# ADR-15：插件免签名安装与 manifest v2 执行类型

状态：ACCEPTED（2026-09-07）

## 背景

Gamer 的目标用户是个人/局域网自部署者：官方插件与第三方插件走同一条本地导入链路，没有不可信的多租户分发场景。此前的强制签名链（官方来源必须 Ed25519 签名 + Registry proof、dev keypair、内嵌信任锚）带来三类成本：

- 构建链强依赖私钥：`plugin-signer pack` 强制 `--key`，官方包无法在无密钥环境复现构建；
- 双执行形态无法表达：`gamer.video` 是宿主预置的 Native 扩展（无 WASM guest），却被 manifest 契约强制声明 `entry = "plugin.wasm"`，只能靠假 WASM 字节绕过安装校验；
- 用户自建插件被变相拦截：前端 `installPolicy` 对 official+unsigned 的第二道门禁使本地构建的市场包装不上。

安全边界并未因此消失：ZIP 中央目录/路径穿越/解压限额校验、权限闭集与增量确认、Host API 版本门禁、builtin 伪装检测、下载侧 sha256 校验全部保留。launcher/主程序更新签名（`launcher/src/manifest/sig.rs`、`verify-release.ps1`、release manifest `.sig`）是另一套独立机制，明确不在本决策范围内，继续保留。

## 决策

1. **插件安装免签名**：删除 `extensions/signature.rs` 验签链与 `x-gamer-registry-proof` 头；official 与本地安装统一无签名，`X-Gamer-Extension-Source: official` 仅作来源标注。完整性改由可选 `x-expected-sha256` 请求头（inspect/install/update）与官方 registry 条目 sha256（前端下载强制校验）承载。存量已装包内遗留 `signature.sig` 为死文件，静默忽略。
2. **manifest v2 执行类型**：`manifest_version=2` + 可选 `[execution] kind = "wasm" | "builtin"`（缺省 wasm）。`kind=wasm` 维持 entry 全检（`.wasm` 后缀 + `\0asm` magic，不允许借 builtin 绕过 guest 校验）；`kind=builtin` 必带 `builtin_id`、必须缺省 entry、包内禁止携带 `plugin.wasm`（防伪装），且 id 必须在宿主静态注册表 `extensions/builtin.rs::BUILTIN_EXTENSIONS` 内（未注册 → 409 `host_feature_unavailable`；wasm 包冒充 builtin id、builtin 别名包、builtin→wasm 降级一律拒绝）。注册表无运行时扩展 API——下载包不能把自己变成 builtin。
3. **UI 贡献仅 Running 可见**：插件面板与 ui 资产只在扩展 Running 时注册/服务，stop/disable/uninstall 即撤销（消除「Enabled 半启用面板」的模糊态）。

## 后果

- 官方三插件（gamer.yaml 3.1.1 / gamer.keymap 1.0.1 / gamer.video 1.0.0）由 `tools/build-plugins.ps1` 以 manifest 为元数据单源零密钥构建，registry schema v2（条目带 `execution{kind}`，无 signature 字段）。
- 第三方用户无需修改宿主即可从零开发、打包、安装、更新自己的 WASM 插件（`sdk/` 与 `docs/guides/plugin-dev.md`）。
- 放弃的防护：安装来源不再有密码学认证。接受理由：分发信任由「官方 registry sha256 + 用户主动导入」承载，与自部署威胁模型匹配；若未来引入公共市场再评估按条目签名/最小集证明，不回滚本决策。
- v1 已安装包读端容忍（`parse_manifest_installed`），再安装/更新必须 v2；无自动迁移（ADR-14）。
