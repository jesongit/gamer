# 发布资产验证命令清单（REL-006 attestation）

对 GameBot Release（ZIP 资产 + GHCR 镜像）做独立核验的命令清单。`<owner>`/`<v>`/`<tag>`
按实际替换（`<v>` = tag 去掉 `v` 前缀，如 `0.2.0`）。仓库内自动执行的同套校验见
`.github/workflows/release.yml` smoke job。

前置工具：`gh`、PowerShell 7 或 bash、Node ≥ 20、docker + buildx。

## 1. 资产完整性（SHA256SUMS）

    gh release download <tag> --repo <owner>/gamebot --pattern '*' --dir assets
    cd assets && sha256sum -c SHA256SUMS.txt          # bash
    # PowerShell：Get-FileHash -Algorithm SHA256 <file> 与 SUMS 逐行比对

要求：SUMS 恰列 8 个内容资产（full/app/adb/ffmpeg/licenses zip + `<v>.json` + `<v>.sig`
+ SBOM `gamer-sbom-<v>-windows-x64.cdx.json`），全部 `[OK]`。

## 2. Manifest 验签（两套信任锚）

    # ① 仓库信任锚（release/keys/ 下的公钥）
    node release/contracts/validate-manifest.mjs check assets/<v>.json \
      --sig assets/<v>.sig --keys-dir release/keys \
      --expect-current-version <v> --expect-channel stable

    # ② 包内信任锚（解压 full 包后，用包内 keys/ 验签包内 manifests/ 副本）
    Expand-Archive assets/GameBot-<v>-windows-x64-full.zip -DestinationPath pkg   # PowerShell
    node release/contracts/validate-manifest.mjs check pkg/manifests/<v>.json \
      --sig pkg/manifests/<v>.sig --keys-dir pkg/keys \
      --expect-current-version <v> --expect-channel stable

通过标准：`signature: verified (key_id=prod-ed25519-N)` + `OK — release manifest v1 valid`；
key_id 必须是 `prod-*`（dev key 签发的发布无效）。

## 3. Manifest ↔ ZIP 绑定

    # manifest 声明的 app sha256 必须等于实际发布的 app zip 实算值
    node -e "const m=require('./assets/<v>.json');console.log(m.platforms['windows-x86_64'].app.artifact.sha256)"
    Get-FileHash -Algorithm SHA256 assets/gamer-app-<v>-windows-x64.zip   # PowerShell
    sha256sum assets/gamer-app-<v>-windows-x64.zip                        # bash

## 4. SBOM（CycloneDX 1.5）

    node -e "JSON.parse(require('fs').readFileSync('assets/gamer-sbom-<v>-windows-x64.cdx.json','utf8'));console.log('valid JSON')"
    # 可选：官方工具深校验（需 npm i -g @cyclonedx/cyclonedx-npm@… 或 cyclonedx-cli）
    cyclonedx validate --input-file assets/gamer-sbom-<v>-windows-x64.cdx.json --input-format json

SBOM 由 `tools/gen-sbom.ps1` 从 server/launcher 两个 Cargo.lock 生成，覆盖全部 Rust 传递依赖。

## 5. GHCR 镜像（digest / labels / provenance+SBOM attestation）

    # 固定 digest 引用拉取（digest 见 Release 页/workflow job summary，不可变）
    docker pull ghcr.io/<owner>/gamebot@sha256:<digest>
    docker image inspect ghcr.io/<owner>/gamebot@sha256:<digest> \
      --format '{{ index .Config.Labels "org.opencontainers.image.version" }} / {{ index .Config.Labels "org.opencontainers.image.revision" }}'

    # provenance + SBOM attestation：输出须含 attestation-manifest 条目
    docker buildx imagetools inspect ghcr.io/<owner>/gamebot@sha256:<digest>

通过标准：label `version` == `<v>`；label `revision` == tag 指向的 commit SHA
（`git rev-parse <tag>^{commit}`）；attestation 存在。

## 6. 镜像 ↔ ZIP 同源

- ZIP 侧：`<v>.json` 的 `release.version` == `<v>`，且 workflow verify job 已断言
  tag == Cargo 版本 == 触发 commit。
- 镜像侧：第 5 步 label `version`/`revision` 与同一 `<v>`/commit 一致。
- 两边都对上同一 `<tag>` 即同源；release workflow 的 smoke job 内置第 ⑧ 项断言
  （`needs.docker.outputs.version` 必须等于 manifest 版本）。

## 7. 密钥状态核查

    git ls-files release/keys          # 只允许 .pem 公钥；任何 *.private.pem 入库即为事故
    gh api repos/<owner>/gamebot/actions/secrets --jq '.secrets[].name'   # 仓库级应为空

轮换与泄露处置流程见 [KEY_ROTATION.md](KEY_ROTATION.md)。
