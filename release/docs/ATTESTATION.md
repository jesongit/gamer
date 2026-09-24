# 发布资产验证

发行采用官方 GitHub HTTPS + SHA256。清单和下载资产的一致性由哈希保证，来源可信性依赖 GitHub 账号与发布权限；不生成 `.sig` 或公钥，不需要签名 secrets。

流程：校验 tag → 空 draft → 构建打包 → 上传资产 → 重新下载核验 → 人工批准 smoke → 正式发布。

## 下载并核对

```bash
gh release download <tag> --repo jesongit/gamer --pattern '*' --dir assets
cd assets
sha256sum -c SHA256SUMS.txt
```

`SHA256SUMS.txt` 包含 12 个内容资产：full/app/adb/ffmpeg/scrcpy-server/launcher/official-plugins/licenses 共 8 个 ZIP、独立启动器 EXE、版本化清单 JSON、固定入口 `gamer-release.json`、CycloneDX SBOM。加上校验和文件，下载目录共 13 个文件。

```powershell
node release/contracts/validate-manifest.mjs check assets/<v>.json --expect-current-version <v> --expect-channel stable
Expand-Archive assets/Gamer-<v>-windows-x64-full.zip -DestinationPath pkg
node release/contracts/validate-manifest.mjs check pkg/manifests/<v>.json --expect-current-version <v> --expect-channel stable
```

发布级、固定入口、包内清单必须逐字节一致；app 与所有组件 ZIP 必须匹配清单中的 size/SHA256；full 包内每个文件都必须列入并匹配包内 SHA256SUMS。独立启动器及 full 包启动器应匹配 launcher 组件的文件哈希。

`tools/verify-external-release.ps1` 执行下载资产校验与启动器探针；`release/packaging/verify-sbom.ps1 -SbomPath <sbom> -ExpectedVersion <v>` 校验 SBOM 与锁定依赖。

## 离线回归

```powershell
node release/contracts/validate-manifest.mjs selftest
pwsh -NoProfile -File release/packaging/test-release-workflow.ps1
```

离线回归覆盖清单规则、不可变 tag 与 SBOM，不代表真实 GitHub Release 已发布。历史密钥演练记录仅作历史证据，不是当前发布前置条件。
