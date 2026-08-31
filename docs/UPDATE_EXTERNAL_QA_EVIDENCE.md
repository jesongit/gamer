# QA-008 外部发布冒烟证据模板

> 状态：**未完成（截至 2026-08-31）**。本工作区没有可供复核的真实 GitHub Release、GHCR
> immutable digest 或可用 `gh` 登录环境；以下只提供可执行命令和证据格式，不把
> `release/dist/`、本地 manifest、dev key 或离线 fixture 记作外部通过。
>
> 依据：`docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md` 的 REL-004/REL-005/QA-008、
> `docs/RELEASE.md` §2.5、`release/docs/ATTESTATION.md`。脚本为
> `tools/verify-external-release.ps1`。

## 1. 完整 QA-008 命令

前置：Windows PowerShell 5.1 或 PowerShell 7、Node、GitHub CLI `gh`（已登录并能读取
目标 Release）、Docker CLI/buildx（GHCR 部分）、目标 Release 已上传完整资产。`<digest>`
必须从发布 job / Release 记录中取得，不能由本地 tag 推测。

```powershell
$repo = '<owner>/<repo>'
$tag = 'v<semver>'
$commit = ((git ls-remote origin "refs/tags/$tag^{}" | Select-Object -First 1) -split '\s+')[0]
$digest = 'sha256:<64-hex-digest-from-release-job>'
$image = 'ghcr.io/<owner>/<repo>'
$evidenceDir = ".\qa-008-$($tag.TrimStart('v'))"
$log = Join-Path $evidenceDir 'verify-external-release.log'
New-Item -ItemType Directory -Path $evidenceDir -Force | Out-Null

& .\tools\verify-external-release.ps1 `
  -Repository $repo -Tag $tag -CommitSha $commit `
  -Image $image -Digest $digest `
  -DownloadDir (Join-Path $evidenceDir 'assets') 2>&1 |
  Tee-Object -FilePath $log
$exitCode = $LASTEXITCODE
Write-Host "QA-008 exit code: $exitCode"
```

完整命令会从 GitHub Release 重新下载 `*`，并且 fail closed 地检查：

- `SHA256SUMS.txt` 恰好覆盖 8 个内容资产，下载目录恰好 9 个文件，且逐项 SHA-256 一致；
- full ZIP 内 `SHA256SUMS.txt` 覆盖全部包内文件；发布级和包内 manifest / `.sig` 按原始字节一致；
- 仓库 `release/keys/` 与 full ZIP 内 `keys/` 各验签一次，并核对版本和通道；
- manifest 的 app artifact hash 等于重新下载的 app ZIP，下载的 SBOM 通过现有锁文件校验；
- full ZIP 内 launcher 的库存 doctor 和 manifest doctor 均退出 0；
- GHCR 版本 tag 与 immutable digest 均重新 pull，版本 tag 的 `RepoDigests` 包含期望 digest，
  两个引用的 image ID 一致；`org.opencontainers.image.version`、`revision`、`source` 分别
  等于 `<version>`、tag 指向的 commit、`https://github.com/<owner>/<repo>`；
- 默认调用现有 `verify-image-attestations.ps1`，确认 provenance 和 SBOM attestation 绑定该 digest。

最终完整通过必须看到：

```text
[external-release] PASS: full QA-008 external release + GHCR smoke completed
```

并且进程退出码为 `0`。`PASS (partial smoke...)`、缺少 `-Image`、
`-SkipLauncherDoctor` 或 `-SkipAttestations` 都不是 QA-008 完成证据。

## 2. 只有 Release 源时的资产/验签命令

此命令可以在没有 Docker 或 digest 时验证 Release 资产，但脚本会明确输出 partial，不能替代
GHCR 版本/commit/digest/OCI label 校验：

```powershell
.\tools\verify-external-release.ps1 `
  -Repository '<owner>/<repo>' -Tag 'v<semver>' `
  -DownloadDir '.\qa-008-<version>\assets'
```

只有真实 Release 下载成功后才会产生 partial 结果；`gh` 未安装、未登录、Release 不存在或
资产不完整时是 `NOT COMPLETE` / 非零退出码，不是跳过。

## 3. 证据记录表（运行后填写）

| 项目 | 实际值 | 证据位置 |
|---|---|---|
| Repository / Release URL | `<owner>/<repo>` / `<URL>` | `gh release view <tag>` 输出 |
| Tag / version / channel | `<tag>` / `<version>` / `stable\|beta` | Release manifest、脚本日志 |
| Tag commit | `<40-hex>` | `git ls-remote` 与发布 job |
| GHCR image / expected digest | `<image>` / `<sha256:...>` | docker job summary |
| Release 资产数量 | `9`（8 + sums） | 脚本日志 |
| SHA256SUMS | `8/8` | `verify-external-release.log` |
| Manifest key_id | `<prod-ed25519-N>` | Node validator 输出 |
| 仓库/包内信任锚 | `verified / verified` | 脚本日志 |
| App artifact hash binding | `match` | 脚本日志 |
| SBOM | `CycloneDX 1.5 / locked dependencies` | `verify-sbom` 输出 |
| Launcher doctor | `inventory=0, manifest=0` | 脚本日志 |
| GHCR re-pull | `tag=ok, digest=ok` | 脚本日志 |
| GHCR digest resolution | `tag RepoDigests -> expected digest` | `docker image inspect` / 脚本日志 |
| OCI labels | `version / revision / source` | 脚本日志 |
| Provenance / SBOM attestation | `pass` | `verify-image-attestations` 输出 |
| Exit code / date / operator | `<0 or 1>` / `<UTC>` / `<name>` | transcript |

保留 `assets/`、`verify-external-release.log`、`gh release view --json` 输出、Docker inspect
输出和发布 job digest 摘要；不要把 token、私钥或带凭据的环境变量写入证据。

## 4. 当前本地状态与未完成项

- `release/packaging/test-release-workflow.ps1`：本轮独立重跑未通过；当前工作树已有的
  `release/packaging/verify-image-attestations.ps1` 改动在 Windows PowerShell 5.1 AST 下报错，
  PowerShell 7 则在既有 draft-release 静态匹配处失败；本任务没有修改该越界文件。
- `tools/verify-release.ps1 -CargoAuditNoFetch`：Compose 开发、USB、release、release override
  的 config 审计、server/launcher metadata，以及两个 lockfile 的严格离线 cargo audit 已通过；
  本机缓存载入 1233 条 advisory，仅保留仓库已记录的 `RUSTSEC-2025-0141` 豁免。
- `tools/verify-release.ps1` 默认模式仍会刷新 advisory DB；本轮因无法访问
  `https://github.com/RustSec/advisory-db.git` 失败。没有把网络刷新失败自动降级为离线模式，
  因此默认最终门禁仍为非零；需明确使用 `-CargoAuditNoFetch` 才使用已有缓存。
- 本轮没有运行任何真实 GitHub Release 重新下载、生产 manifest 验签、GHCR digest pull、OCI
  label/attestation 校验或真实 launcher 外部 smoke；REL-004/REL-005/QA-008 仍未完成。
- 不能用 `release/dist/`、`release/manifests/`、本地 dev key、离线 fixture 或 workflow 的构建
  workspace 产物填充上表的外部证据字段。

## 5. 2026-09-01 Agent C 执行轮（REL-004/005/006、QA-008）

> 口径不变：本节只记录本机可复现的离线验证、真实外部链路的**失败**证据与精确阻塞点；
> 不把 `release/dist/`、离线 fixture、本地 dev key 或"workflow 已触发"记作外部通过。
> QA-008 完整冒烟仍未完成；本轮新增的真实增量是：Release workflow 首次被真实 tag 触发，
> 并拿到公开可复核的失败 run 与逐层缺口定位。

### 5.1 环境与凭据事实（2026-09-01 实测）

| 项 | 实测结果 |
|---|---|
| gh CLI | 未安装 → 本轮经 `winget install --id GitHub.cli` 装上 **2.97.0**（机器 PATH 已含 `C:\Program Files\GitHub CLI\`，新进程可用）；`gh auth status` → **未登录**；进程环境无 `GITHUB_TOKEN`/`GH_TOKEN` → **无 GitHub API 凭据**（按纪律不做交互式登录） |
| git push 凭据 | `ssh -T git@github.com` 认证成功（`Hi jesongit!`，exit 1 属正常无 shell）→ **SSH push/pull 可用** |
| Docker / GHCR | daemon 可用；`docker-credential-desktop list` = `{}`（未登录任何 registry）；匿名 `docker pull ghcr.io/jesongit/gamer:latest` → `denied`；`ghcr.io/token` 匿名请求 → DENIED → **GHCR 无凭据、包状态不可见** |
| 仓库可见性 | `api.github.com/repos/jesongit/gamer` 匿名 200、`"private": false`（公开仓库）；Actions runs / releases 列表匿名只读可达（job 日志文本需认证） |
| 远端基线 | `origin/main` HEAD = 本地 HEAD = `6f7792a`；远端此前**无任何 `v*` tag**（Release workflow 从未触发过） |
| PowerShell 7 | 本机原无 pwsh → 本轮经 `winget install --id Microsoft.PowerShell` 装上 **7.6.5**（MSIX） |

### 5.2 任务 1：PS 5.1 / pwsh 7 双版本门禁（当前工作树）

两条命令对**当前工作树**均 PASS（exit 0）：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File release\packaging\test-release-workflow.ps1   # PS 5.1.26100.9168 → PASS
pwsh -NoLogo -NoProfile -ExecutionPolicy Bypass -File release\packaging\test-release-workflow.ps1     # pwsh 7.6.5      → PASS
```

输出覆盖：7 个脚本 AST OK、compose 静态契约 OK、`test-upgrade-release.ps1` PASS、
immutable/SBOM/attestation 正反例全部按预期 PASS/reject、key-rotation fixture PASS、
末行 `[release-workflow-test] PASS: workflow contract + ... offline behavior`。

- 上一轮证据（§4）记录的两个破损（verify-image-attestations.ps1 的 PS 5.1 AST 报错、
  test-release-workflow.ps1 的 pwsh draft-release 静态匹配失败）**在当前工作树已不存在**——
  工作树中已有的未提交修复已生效，本轮对这两个文件零修改。
- 编码检查：6 个相关 .ps1（verify-image-attestations / test-release-workflow / verify-sbom /
  verify-key-rotation / check-immutable-release / augment-sbom）头 3 字节均为 `ef bb bf`
  （UTF-8 BOM），PS 5.1 无 BOM 按 GBK 解码的风险不存在。
- **但远端 HEAD 仍是破损版**（见 5.4 演练证据）：verify step 3 在 CI 上真实失败，
  与 §4 记录的 pwsh 破损完全对应——修复只存在于本地工作树、尚未提交推送。

### 5.3 任务 2：REL-006 本地可做项（4 个脚本实跑）

| 脚本 / 调用 | 结果 | 关键输出 |
|---|---|---|
| `verify-sbom.ps1 -SbomPath release\sbom\gamer-sbom-0.1.0-windows-x64.cdx.json -ExpectedVersion 0.1.0` | **PASS** | `OK: adb@37.0.1`、`OK: ffmpeg@N-126335-gb32f8d1c23-20260830`、`OK: scrcpy-server@3.3.3`、`PASS: CycloneDX 1.5 / 0.1.0 / 3 个锁定运行依赖` |
| `augment-sbom.ps1 -SbomPath <产物>`（幂等） | **PASS** | `追加 0 个运行依赖条目`（产物已 augment 过） |
| `augment-sbom.ps1` 完整追加语义（临时副本剥离 3 个 generic 条目后重跑） | **PASS** | 剥离后 385 条 → `追加 3 个运行依赖条目` → 复跑 verify-sbom PASS（临时副本，产物未动） |
| `verify-key-rotation.ps1 -FixtureDir release\contracts\fixtures\key-rotation` | **PASS** | current/next（`test-rotation-current-1`/`-next-1`）双公钥均可验签；负例全拒：未签名→`[unsigned-manifest]`、篡改 1 字节→`[signature-invalid]`、撤销 current→`[unknown-key-id]`、错误 key→`[signature-invalid]` |
| `check-immutable-release.ps1 -Mode Snapshot` 8 用例（临时 snapshot fixture） | **全部符合预期** | fresh tag→0；已发布 Release→1（拒绝重跑）；已有镜像无 expected digest→1；同 digest→0（允许只读复用）；异 digest→1；tag commit 不符→1；`v0.2.0+build.1`→1（OCI tag 不接受 +build）；draft 存在→0（允许进入同 hash 门禁） |

边界（无凭据路径的 fail closed 实测，符合 REL-005/006 "不能把查不到当没有"要求）：

- `-Mode GitHub`（gh 未登录/无本地 tag）：非零退出，fail closed；
- `-Mode Registry`（GHCR 匿名 403 Forbidden）：exit 1，`403 Forbidden` 未被误判为
  manifest-not-found；
- `tools\verify-external-release.ps1 -Repository jesongit/gamer -Tag v0.1.0 -DownloadDir <tmp>`：
  `gh release download` 失败 → `[external-release] NOT COMPLETE` → **exit 1**（不是跳过）。
- 显示层小坑：PS 5.1 下 node 子进程输出的 UTF-8 em dash（`—`）在 GBK 控制台显示为 `鈥`，
  仅影响显示，不影响退出码与 ASCII 错误码匹配。

### 5.4 任务 3：外部链路受限演练（真实 REL-004 缺口证据）

凭据判定：git push（SSH）可用 + 观察（匿名 API）可用 + 演练产物可完整清理（tag 删除走 SSH；
draft/GHCR 按 workflow 结构推断不会产生）→ 满足"拿到可用凭据才演练"的前提。未做交互式登录，
未发布任何正式 Release。

演练步骤与结果：

1. `git tag v0.2.0-rc-drill1 6f7792a` + `git push origin v0.2.0-rc-drill1`（tag 指向远端已有
   commit，不推送任何代码对象）→ 触发 Release workflow。
2. **run 33416440024**（`https://github.com/jesongit/gamer/actions/runs/33416440024`，
   event=push，head_branch=v0.2.0-rc-drill1，run_attempt=1）→ `conclusion: failure`。
3. job/step 结论（匿名 API `actions/runs/33416440024/jobs`）：
   - `verify (version gate + scrcpy binding)` → **failure**，失败在 **step 3
     "Check release workflow locally (AST + offline behavior)"**；step 4-8 skipped；
   - build-windows / docker / draft-release / artifact-verify / smoke / publish
     **全部 skipped**（needs verify）。
4. 失败根因本地精确复现：把 HEAD 版 `release/packaging/*.ps1` 导出临时目录、用 pwsh 7.6.5 以
   `-RepoRoot` 指向本仓库重跑 →
   `[release-workflow-test] FAIL: draft Release 创建必须带 --draft`（HEAD 版的
   verify-image-attestations.ps1 在 PS 5.1 AST 下是 OK 的，失败的是 pwsh 静态匹配）。
   即：**CI 失败 = 远端 HEAD 仍带 §5.2 所述 pwsh 破损，本地工作树修复尚未提交**。
5. 清理（全部完成，零残留）：`git push --delete origin v0.2.0-rc-drill1` + 本地 `git tag -d`；
   releases API 返回 `[]`（无 draft/正式 Release 产生）；docker job skipped → 无 GHCR 推送、
   无 package version；临时目录已删。

### 5.5 剩余缺口（精确阻塞清单，按层递进；逐层解除后同一 tag 重演练即可逐层暴露）

| 层 | 缺口 | 精确位置 / 所需凭据或配置 | 解除条件 |
|---|---|---|---|
| A | verify step 3 破损：**远端 HEAD** 的 test-release-workflow.ps1 在 pwsh 下 draft-release 静态匹配 FAIL（本轮 run 33416440024 实证） | 工作树已有修复，**未提交** | 提交并推送工作树修复（feat/fix(release)） |
| B | `check-immutable-release.ps1` GitHub preflight **必挂**：L152 `"$Tag^{{commit}}"` 在 PowerShell 双引号里是字面量（不转义 `{}`），传给 `git rev-parse --verify` → `fatal: Needed a single revision`（本地实测 exit 128）；L103 ls-remote peel 模式 `refs/tags/{0}^{{}}` 同类问题（annotated tag 会取到 tag object SHA 而非 peeled commit）。**HEAD 与当前工作树版本都带此 bug** | `release/packaging/check-immutable-release.ps1:152`（工作树行号）；CI 路径：verify step 5 | 改为单引号 `'^{commit}'` / `'^{}'`。该文件不在本轮 Agent C 允许修改名单，未动；**A 修复后这是下一个必炸点** |
| C | 生产签名 secrets 未配置：build-windows 签名步骤显式 throw（缺 key_id/私钥即失败） | environment `release-sign`：`RELEASE_MANIFEST_PRIVATE_KEY`（PKCS#8 PEM）+ `RELEASE_MANIFEST_KEY_ID`（须 `prod-ed25519-N` 且公钥已 PR 入库 `release/keys/`）；workflow L157-158、L247-267 | 按 release/docs/KEY_ROTATION.md 配置；这是 REL-004 预期观察点（演练 tag 到位后 workflow 会真实失败于此） |
| D | smoke 人工批准门未配置 | environment `release` 需 required reviewers（docs/RELEASE.md §5 首次演练前必须配置） | 仓库 Settings → Environments → release |
| E | 本机无 GitHub API 凭据 | `gh auth login` 或 `GH_TOKEN`；删除 draft release / GHCR package version、读取 job 日志文本、QA-008 partial 下载都需要它 | 用户配置 token 后才能做完整演练闭环（含 draft 清理）与 QA-008 §2 partial 冒烟 |
| F | 本机 docker 未登录 ghcr.io | `docker login ghcr.io`（QA-008 的 GHCR 段需要） | 用户执行 |
| G | 远端 main 上一轮 CI 为 failure（run 33386993573，2026-08-31） | 与本轮演练无关，但 docs/RELEASE.md §5 要求 main CI 全绿为发布前置 | 另行排查 |

### 5.6 本轮环境副作用（如实记录）

- winget 新装：GitHub CLI 2.97.0（机器范围）、PowerShell 7.6.5（MSIX，用户范围）。
- 远端短暂存在 tag `v0.2.0-rc-drill1`，已删除；产生一次公开仓库的 Release workflow 失败运行
  （run 33416440024，可公开复核，属本轮目标证据）。
- 本轮对仓库文件零修改（含名下文件——双门禁在工作树已 PASS，无需再改）；本节为本轮唯一
  交付写入。

**结论：REL-004/REL-005/QA-008 仍未完成。**本轮真实增量 = 首次真实触发 Release workflow 的
失败证据 + 双 PS 门禁在当前工作树恢复 PASS 的验证 + A~G 分层阻塞清单；QA-008 证据表（§3）
仍全部待真实 Release 后填写。
