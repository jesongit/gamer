# 发布资产验证命令清单（REL-006）

对 Gamer Release（Windows ZIP 资产）做独立核验。`<owner>`、`<v>`、`<tag>`
按实际替换（`<v>` 是去掉 `v` 前缀的产品版本）。本文只描述真实发布物验证；
`release/packaging/test-release-workflow.ps1` 另提供不访问网络或生产 secrets 的离线回归。

发布顺序必须保持为：`v*` tag → 校验 tag/版本 → 创建空 draft → 构建并签名 ZIP、推送
不可变 tag → 上传 draft 资产 → 从 draft 重新下载并验签/smoke → `release` environment
人工批准 → 将 draft 转正式。任何一步失败都不得执行 publish。

前置工具：`gh`、PowerShell 7 或 bash、Node ≥ 20；真实 Release
核验还必须有可读取目标仓库和包的 GitHub token。无这些条件时只能报告阻塞，不能以 fixture 结果代替。

## 1. 资产完整性（SHA256SUMS）

```bash
gh release download <tag> --repo <owner>/gamebot --pattern '*' --dir assets
cd assets
sha256sum -c SHA256SUMS.txt
```

PowerShell 可按同一文件逐行执行 `Get-FileHash -Algorithm SHA256` 比对。`SHA256SUMS.txt`
必须恰列 8 个内容资产（它本身不列入自身）：

- `Gamer-<v>-windows-x64-full.zip`
- `gamer-app-<v>-windows-x64.zip`
- `gamer-adb-<v>-windows-x64.zip`
- `gamer-ffmpeg-<v>-windows-x64.zip`
- `Gamer-<v>-licenses.zip`
- `<v>.json` 与 `<v>.sig`
- `gamer-sbom-<v>-windows-x64.cdx.json`

下载目录应为上述 8 个文件加 `SHA256SUMS.txt`，共 9 个文件；同名资产 hash 不一致时应停止，
不得使用 `--clobber` 静默覆盖后继续。

## 2. Manifest 验签（两套信任锚）

```bash
# 仓库信任锚
node release/contracts/validate-manifest.mjs check assets/<v>.json \
  --sig assets/<v>.sig --keys-dir release/keys \
  --expect-current-version <v> --expect-channel stable
```

```powershell
# 包内信任锚
Expand-Archive assets/Gamer-<v>-windows-x64-full.zip -DestinationPath pkg
node release/contracts/validate-manifest.mjs check pkg/manifests/<v>.json `
  --sig pkg/manifests/<v>.sig --keys-dir pkg/keys `
  --expect-current-version <v> --expect-channel stable
```

通过标准是签名验证成功、manifest v1 语义校验成功，且 `.sig` 首行的 key id 匹配 `prod-ed25519-N`。
`dev-*`、fixture key、未知 key、缺失签名或 manifest 单字节改变都必须失败。

## 3. Manifest ↔ ZIP 绑定

```bash
node -e "const m=require('./assets/<v>.json');console.log(m.platforms['windows-x86_64'].app.artifact.sha256)"
sha256sum assets/gamer-app-<v>-windows-x64.zip
```

两者必须相同；full 包内的 manifest 和签名副本还必须与 Release 下载的对应文件逐字节相同。

## 4. SBOM（CycloneDX 1.5）

```bash
node -e "const x=JSON.parse(require('fs').readFileSync('assets/gamer-sbom-<v>-windows-x64.cdx.json','utf8')); if(x.bomFormat!=='CycloneDX'||x.specVersion!=='1.5') process.exit(1); console.log('valid CycloneDX 1.5')"
# 可选深校验（需预装 cyclonedx-cli）
cyclonedx validate --input-file assets/gamer-sbom-<v>-windows-x64.cdx.json --input-format json
```

SBOM 由 `tools/gen-sbom.ps1` 生成，再由 `augment-sbom.ps1` 加入锁定的 adb/ffmpeg/scrcpy-server
来源、版本和逐文件 hash；`verify-sbom.ps1 -ExpectedVersion <v>` 必须通过。

## 7. 密钥状态核查

```bash
git ls-files release/keys
gh api repos/<owner>/gamebot/actions/secrets --jq '.secrets[].name'
```

仓库中只允许公钥 `.pem`；任何 `*.private.pem` 入库即为事故。发布私钥只存在 GitHub environment
`release-sign` 的 `RELEASE_MANIFEST_PRIVATE_KEY`，配对的 `RELEASE_MANIFEST_KEY_ID` 必须为
`prod-ed25519-N` 且对应公钥已入库。仓库级 secrets 不应存放发布私钥。

轮换和泄露处置流程见 [KEY_ROTATION.md](KEY_ROTATION.md)。

## 8. 离线回归（不替代真实验收）

```powershell
pwsh -NoLogo -NoProfile -File release\packaging\test-release-workflow.ps1
```

该命令只检查 workflow 静态契约，并使用仓库内非生产 key-rotation fixture、临时 SBOM/OCI
attestation JSON 和 immutable snapshot 覆盖成功与拒绝路径；它不访问 GitHub
registry 或生产 secrets。若本机没有 `gh`、生产 `release-sign` secret、`release` required
reviewer或真实 Windows runner，只能记录为外部阻塞，不能把离线 PASS 当作
REL-004/005/006 的真实 checklist 证据。
