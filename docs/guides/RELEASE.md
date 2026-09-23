# Gamer 发布与人工恢复手册（维护者向）

> 面向维护者的发布 runbook：draft 发布流程、SHA256 完整性校验、`manual_recovery` 人工恢复、
> 首次真实 tag 演练 checklist。
> 事实依据：`.github/workflows/release.yml`（发布 workflow）、`docs/guides/UPDATE_CONTRACT.md`（安装目录契约）、
> `release/contracts/`（manifest / system-api / IPC 契约）、`launcher/` 与 `release/packaging/`（当前实现）。
> 计划文档仅作背景，不作为“已完成”依据；最终用户以完整包内生成的 `INSTALL.md` 和 launcher CLI 为准。
> 本手册不代表当前已经有成功的 GitHub Release 发布或生产演练结果。

## 1. 版本单一来源与发布链路总览

产品版本权威源 = `server/Cargo.toml` 的 `package.version`；`web/package.json` 的 `version`
必须与之一致（CI `version` job 用 `tools/check-version.ps1` 把关）。发布 tag 必须形如
`v<semver>` 且与该版本一致；前端源码不得硬编码版本（`tools/check-web-version.ps1` 为 CI
web job 硬门禁），页面一律展示 `/api/system/info` 返回的 `app.version`，前端构建版本由
vite 注入 `__APP_VERSION__`（取自 `web/package.json`），与服务端不一致时 MainLayout 顶部
显示混包警告条（不阻塞使用）。

发布由 push tag `v*` 触发 `.github/workflows/release.yml`（不提供 workflow_dispatch，
避免 tag 语义分叉；预发演练用带预发布后缀的 tag，如 `v0.1.0-rc.1`）。链路不允许中途取消
（`cancel-in-progress: false`）：

```text
push tag v*
  → verify（win）        版本门禁 + scrcpy 绑定 + tag 指向触发 commit
  → draft-release        创建空 draft Release
  → build-windows（win） 构建本体、启动器、依赖、官方插件、清单与 full 包 + SBOM
  → upload-assets        同名资产 hash 门禁，生成 SHA256SUMS
  → artifact-verify      从 Release 重新下载全量资产核验
  → smoke（win）         environment: release 人工批准后运行安装探针
  → publish              draft → 正式；纯 semver 才标 latest
```

## 2. Draft 发布流程

### 2.1 verify：tag 与版本门禁

- `tools/check-version.ps1 -Tag`：tag 必须等于 `v + Cargo.toml 版本`，且 Cargo == web。
- `tools/check-scrcpy-binding.ps1`：代码 / jar / lock 三方绑定一致。
- tag 指向的 commit 必须就是触发 workflow 的 commit（防止 tag 重打到别的历史）。

### 2.2 build-windows：构建、打包、校验

1. 从 tag 派生产品版本（`v` 剥离；不合法立即失败）。
2. `release/packaging/fetch-adb.ps1` / `fetch-ffmpeg.ps1`：按 `release/dependencies.lock.toml`
   锁定版本拉取依赖（ffmpeg 有许可红线与冒烟检查）。
3. `package-app.ps1 -Channel stable`：构建 server + web-dist + scrcpy jar 并打 zip，注入
   构建信息（git commit / 构建时间 / channel / target），生成包内 `SHA256SUMS`。
4. `package-components.ps1`：adb / ffmpeg 组件 zip。
5. `package-launcher.ps1` 打包独立启动器和官方插件；`gen-manifest.ps1` 实算
   归档/文件 SHA256 与大小，并执行结构/语义校验。不需要签名或密钥。
6. `package-full.ps1 -SkipBuild`：使用已写入清单的启动器字节生成完整包，
   校验包内 SHA256SUMS、manifest 与 doctor 冒烟。
7. `tools/gen-sbom.ps1`：CycloneDX SBOM。资产暂存 artifact 并上传。

本地只想复现构包链路时，入口按以下顺序执行；这些命令不会创建或发布 GitHub Release：

```powershell
.\release\packaging\fetch-adb.ps1
.\release\packaging\fetch-ffmpeg.ps1
.\tools\build-plugins.ps1
.\release\packaging\package-app.ps1 -Channel stable
.\release\packaging\package-components.ps1
.\release\packaging\package-launcher.ps1
.\release\packaging\gen-manifest.ps1 -Version '<version>' -Channel stable `
  -DownloadBaseUrl 'https://<发布源>/download/<tag>' `
  -ReleaseNotesUrl 'https://<发布源>/releases/tag/<tag>'
.\release\packaging\package-full.ps1 -SkipBuild -Version '<version>'
.\tools\gen-sbom.ps1
$sbom = Get-ChildItem .\release\sbom\*.cdx.json | Select-Object -First 1
.\release\packaging\augment-sbom.ps1 -SbomPath $sbom.FullName
.\release\packaging\verify-sbom.ps1 -SbomPath $sbom.FullName -ExpectedVersion '<version>'
```

清单及资产默认来自官方 GitHub HTTPS，文件用 SHA256 校验。SHA256 无法认证发布者；
来源可信性依赖 GitHub 账号与发布权限。发布脚本不再读取签名 secrets，也不生成密钥。
`gen-manifest.ps1` 默认 URL 是示例值；正式发布必须传入实际下载和 release notes URL。

### 2.4 draft-release：draft 与资产上传

- 幂等创建 draft Release（说明含安装三步、资产核验）。
- 资产上传带 **hash 门禁**：同名资产已存在时重新下载比对，hash 一致跳过、不一致直接
  失败——同一 tag 绝不静默覆盖不同内容。
- 最后生成 `SHA256SUMS.txt`，覆盖 12 个内容资产：full/app/adb/ffmpeg/scrcpy/
  launcher/official-plugins/licenses 共 8 个 ZIP、独立 EXE、两个 JSON 清单和 SBOM。

### 2.5 smoke：重新下载核验（人工批准闸）

`environment: release` 挂起等待批准（需在仓库 Settings → Environments → release 配置
必需评审人；**首次演练前必须先配置**）。批准后：

1. 从 Release **重新下载全部资产**（QA-008 语义：脱离构建 workspace，以发布物为准）；
2. 校验 `SHA256SUMS.txt` 覆盖且仅覆盖 12 个内容资产、下载目录 13 文件不多不少；
3. 解压 full 包，核对包内 `SHA256SUMS` 逐条一致；
4. 发布级 manifest 与包内副本字节一致；
5. 发布与包内 manifest 各跑一次 `validate-manifest.mjs check`（绑定版本与通道）；
6. manifest 声明的 app sha256 == 实际发布的 app zip；SBOM 为合法 JSON；
7. `gamer-launcher doctor` 双跑：未安装库存（应 WARN 不 FAIL）+ `--manifest` 校验校验；
   workflow 的 artifact verify 还会执行深度包内容与 probe 校验。

### 2.6 publish：draft → 正式

smoke 全过后自动 `gh release edit --draft=false`；纯 `X.Y.Z` 追加 `--latest`，预发布版本
不标 latest。发布后 URL 与 draft 状态打印在 job 日志。

## 3. 来源与完整性

默认发布源为 `https://github.com/jesongit/gamer/releases/latest/download/gamer-release.json`。
发行清单不附 `.sig`，完整包不含 `keys/`，构建不需要签名 secrets 或密钥轮换。
在线传输使用 HTTPS；下载归档先对 size/SHA256，再安全解压并校验组件逐文件清单。
离线种子、下载缓存和修复走相同完整性检查。远程清单拒绝 HTTP，loopback 测试除外。
HTTPS 请求禁止重定向降级到 HTTP。已有安装里的旧签名文件不再被读取。

## 4. `manual_recovery` 人工恢复指引

> 依据当前 `launcher/` 实现与 `docs/guides/UPDATE_CONTRACT.md` 的目录契约编写。
> `state/current.json` 保存版本字符串而不是目录路径；仓库没有独立的
> `manual-recovery` / `reset-journal` CLI，journal 修复属于受控人工维护动作。

### 4.1 什么时候进入人工恢复

升级失败且**自动回滚也失败**时进入 `manual_recovery_required`（契约 §5.3：任一原子步骤
崩溃后结果只有三种——新版健康、旧版健康、manual_recovery_required）。这是 11 态中唯一
没有自动出口的终态：系统停止全部自动重试，并完整保留全部证据。设置页更新卡会显示
`manual_recovery` 状态与 journal 摘要（事务 id / 阶段 / 最后错误 / 状态时间）。

### 4.2 现场证据清单（全部保留，不要手删）

| 位置 | 内容 |
|---|---|
| `state/update-journal.json` | 升级状态机 journal：update id、from/to、child PID、current/previous、snapshot、schema before/after、最后完成步骤、错误摘要 |
| `backups/<update-id>/` | 升级前 data+config 离线快照（manifest + 逐文件 hash），自动回滚/人工恢复数据的唯一依据 |
| `versions/<semver>/` | 新旧两个版本目录（安装后只读，升级证据） |
| `manifests/<version>.json` | 已校验 manifest |
| `quarantine/` | 回滚失败/损坏数据保留区（只增不自动删，供取证） |

### 4.3 处置步骤

1. **取证**：读 `state/update-journal.json` 的「最后完成步骤 + 错误摘要」，结合设置页
   错误码（`signature_invalid`/`artifact_invalid`/`insufficient_space`/`schema_incompatible`
   等）判断停在哪个阶段、数据处于哪个状态；可先只读查看：

   ```powershell
   Get-Content -LiteralPath .\state\update-journal.json -Raw
   Get-Content -LiteralPath .\state\current.json -Raw
   ```

2. **定方向**：候选版本未提交（journal 无 `committed`）→ 走「恢复旧版本 + 升级前快照」；
   已提交后才发现问题 → 属于人工降级（见 4.4），需先确认 schema 兼容性与数据代价；
3. **恢复数据**：以 `backups/<update-id>/manifest.json` 为准，逐文件复验大小与 SHA-256；
   任何缺失或 hash 不一致都停止。将当前 `data/`、`config/config.toml` 和需要替换的运行
   目录移入带时间戳的 `quarantine/`，不要直接删除，再只恢复 manifest 列出的文件；
4. **恢复程序**：确保 `versions/<from_version>/` 完整。`state/current.json` 的 `current` /
   `previous` 字段写 SemVer 版本字符串，不写 `versions/...` 文件系统路径；保留原文件副本
   后，只有在 snapshot 和版本目录都验证通过时，才将 journal 受控收敛到 schema v1 的合法
   `idle` 状态；
5. **处置 quarantine**：取证完成前不动；确认不再需要后用显式人工清理动作移除，不做
   静默删除；
6. **重启验证**：用包内 launcher 执行 `doctor --manifest --deep --probe` → `start` →
   `status`，确认版本、boot id 和依赖状态恢复正常。仓库没有命令可自动重置 manual state，
   不要用“清空 journal”绕过保护。

验证时使用实际 launcher 入口（在另一个终端执行 `status`，因为 `start` 会前台托管服务）：

```powershell
.\gamer-launcher.exe --install-root 'D:\Gamer' doctor --manifest .\manifests\<version>.json --deep --probe
.\gamer-launcher.exe --install-root 'D:\Gamer' start
# 另一个维护终端：
.\gamer-launcher.exe --install-root 'D:\Gamer' status
```

### 4.4 回滚边界与纪律

- 自动回滚只承诺「提交（committed）之前」；提交成功后的新数据不在自动回滚范围，
  人为降级旧版本必须明确接受「丢失升级后数据」的代价（schema 不兼容时不可执行）。
- CLI 没有独立 rollback 子命令。确认允许只回退运行时版本时，可用旧 manifest 走仓库已有的
  `repair` 入口；这不会恢复升级后的数据，也不绕过 schema/版本校验：

  ```powershell
  .\gamer-launcher.exe --install-root 'D:\Gamer' repair --manifest .\manifests\<previous-version>.json --probe
  ```

  执行前必须先完成数据兼容性与备份决策；需要恢复数据时，按 backup manifest 和业务策略处理，
  不要把旧版本运行时修复误当作数据回滚。
- `backups/` 清理只能依赖系统按数量/年龄/磁盘上限的保留策略，且永不删除 current、
  previous 与唯一有效回滚点；`versions/`、`runtime/`、`manifests/` 同理交由 launcher 管理。
  当作 Windows 完整包的 `state/` 结构处理。

## 5. 首次真实 tag 演练 checklist

首次发布建议用**预发布 tag**（如 `v0.1.0-rc.1`）完整走一遍链路，不追求 latest 标记。

前置（一次性）：

- [ ] 仓库 Settings → Environments → `release` 配置必需评审人（否则 smoke 永远挂起）；
- [ ] main 分支 CI 三 job（version / rust / web，含 WEB-006 硬门禁）全绿。

演练链路：

- [ ] push 预发布 tag 后 Release workflow 触发，verify 三项门禁通过（tag==版本==Cargo==web、
      scrcpy 绑定、tag 指向触发 commit）；
- [ ] build-windows 产物齐全：full zip / app zip / adb zip / ffmpeg zip / licenses zip /
      scrcpy/launcher/official-plugins ZIP、独立 EXE、两个 manifest JSON / SBOM，清单校验通过；
- [ ] draft-release 创建 draft 且 12 个内容资产 + SHA256SUMS.txt 上传齐全；人为重跑一次确认
      同名同 hash 跳过（幂等），不同 hash 拒绝覆盖；
- [ ] 批准 release environment，smoke 七步全过（§2.5）；
- [ ] publish 后 draft 转正且**未标 latest**（预发布语义验证）。

演练后验证：

- [ ] 任一干净 Windows 环境（或干净目录）按 full 包内 `INSTALL.md`：解压 full 包 →
      `gamer-launcher doctor --manifest` 校验 → `repair` → `start` → 浏览器登录；
- [ ] 设置页显示版本与 `/api/system/info` 一致、无混包警告条；更新能力按部署模式正确
      降级（直跑显示 update_not_managed）；
- [ ] 双里程碑升级证据（计划 §14）：先安装 N-1 基线版本，再经基线 launcher 自动升级到
      本 tag 版本，验证快照/切换/回滚与 journal 记录；
- [ ] 踩到的坑按仓库规则记入 `docs/PITFALLS.md`。

## 6. 相关文档

- 计划与批次 checklist：`docs/plans/AUTO_UPDATE_DEVELOPMENT_PLAN.md`（§14 / §17.5）
- 安装目录契约（journal/backups/quarantine 语义）：`docs/guides/UPDATE_CONTRACT.md`
- manifest/API/IPC/schema/许可契约与 fixtures：`release/contracts/`
- 完整包用户入口：打包生成的 `INSTALL.md`；launcher 参数以 `launcher/src/cli.rs` 为准
- 踩坑记录：`docs/PITFALLS.md`
