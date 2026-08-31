# 发布资产验证命令清单（REL-006）

对 GameBot Release（Windows ZIP 资产 + GHCR 镜像）做独立核验。`<owner>`、`<v>`、`<tag>`
按实际替换（`<v>` 是去掉 `v` 前缀的产品版本）。本文只描述真实发布物验证；
`release/packaging/test-release-workflow.ps1` 另提供不访问网络、GHCR 或生产 secrets 的离线回归。

发布顺序必须保持为：`v*` tag → 校验 tag/版本 → 创建空 draft → 构建并签名 ZIP、推送
GHCR semver tag → 上传 draft 资产 → 从 draft 重新下载并验签/smoke → `release` environment
人工批准 → 将 draft 转正式。任何一步失败都不得执行 publish。

前置工具：`gh`、PowerShell 7 或 bash、Node ≥ 20、Docker + buildx；真实 Release/GHCR
核验还必须有可读取目标仓库和包的 GitHub token。无这些条件时只能报告阻塞，不能以 fixture 结果代替。

## 1. 资产完整性（SHA256SUMS）

```bash
gh release download <tag> --repo <owner>/gamebot --pattern '*' --dir assets
cd assets
sha256sum -c SHA256SUMS.txt
```

PowerShell 可按同一文件逐行执行 `Get-FileHash -Algorithm SHA256` 比对。`SHA256SUMS.txt`
必须恰列 8 个内容资产（它本身不列入自身）：

- `GameBot-<v>-windows-x64-full.zip`
- `gamer-app-<v>-windows-x64.zip`
- `gamer-adb-<v>-windows-x64.zip`
- `gamer-ffmpeg-<v>-windows-x64.zip`
- `GameBot-<v>-licenses.zip`
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
Expand-Archive assets/GameBot-<v>-windows-x64-full.zip -DestinationPath pkg
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

## 5. GHCR 镜像（semver tag / immutable digest / attestations）

版本 tag 是去掉 `v` 的 semver，例如 `:0.2.0`；它只允许首次绑定一个 digest。
`@sha256:<digest>` 是部署和审计的规范引用。`:stable` 若由正式版本生成，只是明确的滚动别名，
允许随新版本移动，不能当作 immutable version tag 使用。
正式 `vX.Y.Z` 使用 `stable` 通道；带 prerelease 标识的 `vX.Y.Z-...` 使用 `beta` 通道，
不创建 `:stable` 别名。两类 tag 都必须能无损映射为 GHCR 的版本 tag，禁止 `+build` metadata。

```bash
IMAGE=ghcr.io/<owner>/gamebot
DIGEST=sha256:<64-hex>

docker pull "$IMAGE@$DIGEST"
docker image inspect "$IMAGE@$DIGEST" \
  --format '{{ index .Config.Labels "org.opencontainers.image.version" }} / {{ index .Config.Labels "org.opencontainers.image.revision" }}'
docker buildx imagetools inspect "$IMAGE@$DIGEST"
```

label `org.opencontainers.image.version` 必须等于 `<v>`，label `org.opencontainers.image.revision`
必须等于 `git rev-parse <tag>^{commit}`；镜像 index 必须同时含普通 image manifest 与至少一个
subject 指向该 digest 的 `attestation-manifest`。一个 attestation manifest 可以承载多个 layer，
因此验收标准是所有 attestation layer 合计同时出现 provenance 和 SBOM，而不是固定 descriptor 数量。
raw index 必须声明 `application/vnd.oci.image.index.v1+json`，attestation descriptor 必须是 OCI
image manifest；缺失或伪造 mediaType 时 fail closed。

仓库同一门禁的在线调用必须把基础镜像和 digest 分开传入，脚本会自行读取两个引用：

```powershell
.\release\packaging\verify-image-attestations.ps1 `
  -Image 'ghcr.io/<owner>/gamebot' -ExpectedDigest 'sha256:<64-hex>'
```

BuildKit 的 provenance 与 SBOM layer 都可能是 `application/vnd.in-toto+json`；不能只按 layer
mediaType 区分。脚本要求 provenance layer 的 `in-toto.io/predicate-type` 含 SLSA provenance，
SBOM layer 的 predicate 含 SPDX/CycloneDX，并要求每个 attestation 通过 index descriptor 的
`vnd.docker.reference.digest` 或 OCI artifact 的 `manifest.subject.digest` 绑定到 `ExpectedDigest`；
两处都存在时还必须相互一致。

## 6. 镜像 ↔ ZIP 同源

- ZIP 侧：`<v>.json` 的 `release.version`、Cargo package version、Git tag 去前缀必须一致，且 tag
  指向触发 workflow 的 commit。
- 镜像侧：OCI `version`/`revision` label 和容器启动日志必须分别对应 `<v>`/commit。
- 发布 workflow 的 `artifact-verify` 重新下载并验 ZIP，docker job 验 digest/label/启动日志/attestation，
  `smoke` 全部通过后才允许 GitHub Release 从 draft 转正式。

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
attestation JSON 和 immutable snapshot 覆盖成功与拒绝路径；它不访问 GitHub、GHCR、Docker
registry 或生产 secrets。若本机没有 `gh`、生产 `release-sign` secret、`release` required
reviewer、可读取的 GHCR 包或真实 Windows runner，只能记录为外部阻塞，不能把离线 PASS 当作
REL-004/005/006 的真实 checklist 证据。
