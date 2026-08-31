# Manifest 签名密钥轮换（REL-006 runbook）

适用对象：release manifest 的 Ed25519 分离签名。`.sig` 首行格式固定为
`gamebot-manifest-sig-1 <key_id>`，签名覆盖 manifest 原始字节（包括原始换行和编码）。
信任锚是 `release/keys/<key_id>.pem` 公钥；私钥永不入库，也不写入构建产物。

## 密钥命名与存放

| 用途 | key_id 形态 | 私钥 | 公钥 |
|---|---|---|---|
| 开发/fixture | `dev-*` / `test-*` | 仅本机或 fixture 生成目录 | 可随代码/fixture 分发，但不得发布 |
| 生产当前/下一把 | `prod-ed25519-N`（N 为正整数、递增） | GitHub environment `release-sign` 的 `RELEASE_MANIFEST_PRIVATE_KEY` | 仓库 `release/keys/prod-ed25519-N.pem` |

配对 secret 是 `RELEASE_MANIFEST_KEY_ID`。workflow 的生产签名 gate 只接受
`^prod-ed25519-[1-9][0-9]*$`，并要求同名公钥已在仓库信任库；任何 dev、test、未知命名、
缺公钥、空 secret 或私钥不能完成验签的情况都 fail closed。

当前工作区只看到开发/fixture 公钥，未看到生产公钥或生产 secrets；这不是生产轮换证据，
因此真实 Release 仍必须先配置 `release-sign` environment 后再验收。

## 正常轮换（当前/下一把双公钥共存）

1. 在离线机器生成新生产密钥对（私钥目录不要放进仓库）：

   ```powershell
   node release/packaging/sign-manifest.mjs keygen `
     --id prod-ed25519-<N+1> --out-dir <离线私钥目录>
   ```

2. 只把 `prod-ed25519-<N+1>.pem` 作为 PR 提交到 `release/keys/`；检查 `git ls-files
   release/keys` 不包含任何 `*.private.pem`。旧公钥必须暂时保留，历史 manifest 仍依赖它。
3. 在 GitHub Settings → Environments → `release-sign` 更新两个 secrets：
   `RELEASE_MANIFEST_PRIVATE_KEY` 写入 PKCS#8 PEM 原文，`RELEASE_MANIFEST_KEY_ID` 写入新 key id。
   不把私钥复制到 repository secret、workflow 文件、日志或 ZIP。
4. 先用预发布 tag（如 `v0.2.0-rc.1`）走完整 workflow。必须观察到 build job 使用新 key id，
   仓库信任锚和 full 包内信任锚均验签通过，并由 `release` environment 的评审人放行 smoke。
5. 在本台账登记新 key 为当前、旧 key 为退役；旧公钥保留在仓库，旧私钥的本地副本全部删除。
6. 轮换完成后再发布正式 tag；不要用同一个 immutable semver tag 覆盖旧镜像或已发布 Release。

离线双钥 fixture 回归（只验证工具和信任模型，不接触生产 secret）：

```powershell
.\release\packaging\verify-key-rotation.ps1 `
  -FixtureDir .\release\contracts\fixtures\key-rotation
```

该脚本验证 current/next fixture 公钥均可验签，并验证未签名、单字节篡改、移除 current
公钥、错误公钥四种负例被拒。fixture key id 不是生产 key id，fixture PASS 不能勾选真实
GitHub/GHCR 轮换验收。

## 泄露应急

1. 立即生成全新递增的 `prod-ed25519-N+1`，不复用预备钥；优先提交新公钥，并尽快更新
   `release-sign` 两个 secrets。
2. 按 [ATTESTATION.md](ATTESTATION.md) 重新下载并核对受影响 Release 的 SHA256、manifest
   签名和 GHCR digest/attestation；发现篡改时按组织应急流程下架受影响 Release/镜像并公告。
3. 用新 key 发布修复版本；full 包会携带新公钥。历史公钥不要从仓库删除，否则旧版本无法
   验签；撤销的是签发能力，不是历史验证材料。
4. 在台账记录泄露时间、影响版本、处置人、新 key id 和旧 key 退役时间；不要记录私钥内容。

## Environment 权限矩阵

| 项 | `release-sign` | `release` |
|---|---|---|
| 用途 | 仅 build-windows 的生产签名 secret 作用域 | smoke 前人工批准，放行 draft 转正链路 |
| secrets | `RELEASE_MANIFEST_PRIVATE_KEY`、`RELEASE_MANIFEST_KEY_ID` | 无 |
| 必需评审人 | 不配置，避免阻断构建前的签名 gate | 至少 1 人（建议 2 人） |
| 限制 | 建议只允许 `v*` tag deployment | 建议只允许 `v*` tag deployment |
| 发布权限 | `contents: read`，不创建/发布 Release | smoke 使用 `contents: write` 读取 draft 资产但不写 Release；publish 才改变 draft 状态 |

真实验收前必须确认 `release` environment 已配置 required reviewers；本地 fixture 和静态
检查不能替代该审批门。

## 密钥台账

| key_id | 状态 | 公钥入库 PR/commit | 启用日期 | 备注 |
|---|---|---|---|---|
| dev-ed25519-1 | 仅本地 | 首提交 | — | 禁止签发布 |

生产 current/retired 记录必须在实际公钥 PR 与 environment secret 切换完成后填写；在此之前
不得凭空补写生产 key id、commit 或启用日期。
