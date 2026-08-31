# Manifest 签名密钥轮换（REL-006 runbook）

适用对象：release manifest 的 Ed25519 分离签名（`.sig` 首行 `gamebot-manifest-sig-1 <key_id>`）。
信任锚 = 仓库 `release/keys/<key_id>.pem` **公钥**（随 full 包分发到 `keys/`，验签按 `.sig`
首行 key_id 在信任库取对应公钥）。**私钥永不入库**。

## 密钥命名与存放

| 密钥 | key_id 形态 | 私钥存放 | 公钥存放 |
|---|---|---|---|
| 开发/本地 | `dev-ed25519-1`（固定） | 本机 `release/keys/dev-ed25519-1.private.pem`（.gitignore 忽略，仅本地） | 仓库 `release/keys/dev-ed25519-1.pem` |
| 生产（当前） | `prod-ed25519-<N>`（N 递增） | GitHub environment `release-sign` 的 secret `RELEASE_MANIFEST_PRIVATE_KEY`（PKCS#8 PEM 多行原样） | 仓库 `release/keys/prod-ed25519-<N>.pem` |
| 生产（轮换预备） | 同上，N+1 | 仅本地离线保管，**不入 CI** | 同一 PR 提前入库（双钥共存） |

配对 secret：`RELEASE_MANIFEST_KEY_ID` = 当前生产 key_id。发布签名禁止使用 dev key
（workflow 内显式拒绝 `dev-ed25519-1`）。

## 正常轮换步骤（当前/下一把双公钥共存）

1. **离线生成新密钥对**（断网机器更佳）：

       node release/packaging/sign-manifest.mjs keygen --id prod-ed25519-<N+1> --out-dir <本地目录>

2. **PR 提交新公钥** `release/keys/prod-ed25519-<N+1>.pem`。此时仓库同时存在新旧公钥
   （双钥共存）：验签按 key_id 选钥，历史版本清单不受影响；workflow 的发布门禁只检查
   `RELEASE_MANIFEST_KEY_ID` 指向的那把公钥在库。
3. **更新两个 secret**（Settings → Environments → `release-sign`）：
   `RELEASE_MANIFEST_PRIVATE_KEY` ← 新私钥 PEM；`RELEASE_MANIFEST_KEY_ID` ← `prod-ed25519-<N+1>`。
4. **演练**：推一个预发布 tag（如 `v0.x.y-rc.1`）走完整 release workflow，确认
   build-windows 签名步骤用新 key_id 签名且验签通过。
5. **宣布切换**：新 key 成为"当前"，旧 key 降为"退役"——旧公钥**保留在仓库**
   （历史 full 包的信任锚还要用它验签），在本文件末尾"密钥台账"补一行记录。
6. **清理**：本地删除已退役私钥文件；GitHub 侧旧 secret 值被第 3 步覆盖，无需删除。

## 泄露应急（生产私钥怀疑/确认泄露）

1. **立即换钥**：按"正常轮换"第 1、3 步换成**全新生成的密钥对**（新 N+1，不复用预备钥），
   公钥以最高优先级 PR 入库；两步之间不留可签名的窗口。
2. **评估影响面**：泄露私钥可伪造任意 manifest。用 `gh api` 比对已发布 Release 资产的
   SHA256 与本地/流水线记录是否一致（命令清单见 [ATTESTATION.md](ATTESTATION.md)），
   确认是否存在被篡改的已发布版本；发现篡改立即删除对应 Release/镜像并公告。
3. **发补丁版本**：下一个发布版本用新 key 签名；launcher 侧通过版本目录内 `keys/`
   随包分发新公钥（用户升级即获得新信任锚）。
4. **记录**：在下方台账登记泄露时间、处置与新 key_id；撤销 ≠ 删除旧公钥（历史包验证仍需要）。

## Release environment 权限矩阵

| 项 | `release-sign` | `release` |
|---|---|---|
| 用途 | 签名 secrets 作用域隔离（build-windows job 引用） | draft 转正前人工批准门（smoke job 引用，publish 依赖 smoke） |
| secrets | `RELEASE_MANIFEST_PRIVATE_KEY`、`RELEASE_MANIFEST_KEY_ID` | 无 |
| 必需评审人 | 不配置（不阻断自动构建） | **必须配置** ≥1 人（建议 ≥2）；批准动作 = 放行 smoke |
| Deployment branches/tags | 建议限制为 tag `v*` | 建议限制为 tag `v*` |
| 可见/可改者 | 仓库 admin（Settings → Environments） | 同左 |

## 密钥台账（轮换/泄露后必须更新）

| key_id | 状态（当前/退役） | 公钥入库 PR/commit | 启用日期 | 备注 |
|---|---|---|---|---|
| dev-ed25519-1 | 仅本地 | 首提交 | — | 禁止签发布 |
